//! HNSW (Hierarchical Navigable Small World) index for fast approximate nearest neighbor search
//!
//! This module provides a high-performance vector index using the HNSW algorithm,
//! which offers O(log n) search complexity with high recall.
//!
//! Performance characteristics:
//! - Build time: O(n log n)
//! - Search time: O(log n)
//! - Memory: O(n * M) where M is the max connections per node

#![allow(dead_code)] // New API - will be integrated in future versions

use anyhow::Result;
use instant_distance::{Builder, HnswMap, Search};
use std::path::Path;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

/// A point in the HNSW index
#[derive(Clone, Debug)]
pub struct HnswPoint {
    /// Embedding vector
    pub vector: Vec<f32>,
}

impl instant_distance::Point for HnswPoint {
    fn distance(&self, other: &Self) -> f32 {
        // Cosine distance = 1 - cosine_similarity
        // For normalized vectors, this is equivalent to: 1 - dot_product
        let dot: f32 = self.vector.iter()
            .zip(other.vector.iter())
            .map(|(a, b)| a * b)
            .sum();
        1.0 - dot
    }
}

/// A match result from HNSW search
#[derive(Debug, Clone)]
pub struct HnswMatch {
    /// Pattern ID
    pub id: i64,
    /// Similarity score (0.0 to 1.0)
    pub similarity: f32,
}

/// HNSW index for fast approximate nearest neighbor search
pub struct HnswIndex {
    /// The HNSW graph with values being pattern IDs
    hnsw: Option<HnswMap<HnswPoint, i64>>,
    /// Points stored in the index (needed for rebuilding)
    points: Vec<HnswPoint>,
    /// Pattern IDs corresponding to points
    ids: Vec<i64>,
    /// Dimensions per vector
    dimensions: usize,
    /// Index configuration
    config: HnswConfig,
}

/// Configuration for HNSW index
#[derive(Debug, Clone)]
pub struct HnswConfig {
    /// Max connections per node (M parameter)
    /// Higher = more accurate but more memory
    pub max_connections: usize,
    /// Size of the dynamic candidate list (ef_construction)
    /// Higher = better quality index but slower build
    pub ef_construction: usize,
    /// Size of the dynamic candidate list during search (ef_search)
    /// Higher = better recall but slower search
    pub ef_search: usize,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            max_connections: 24,      // Good balance for 384-dim vectors
            ef_construction: 200,     // High quality index
            ef_search: 100,           // Good recall
        }
    }
}

impl HnswIndex {
    /// Create a new empty HNSW index
    pub fn new(dimensions: usize) -> Self {
        Self::with_config(dimensions, HnswConfig::default())
    }

    /// Create a new HNSW index with custom configuration
    pub fn with_config(dimensions: usize, config: HnswConfig) -> Self {
        Self {
            hnsw: None,
            points: Vec::new(),
            ids: Vec::new(),
            dimensions,
            config,
        }
    }

    /// Load index from file
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        // Read header: dimensions (u32), count (u64), config (3 x u32)
        let mut header = [0u8; 24];
        reader.read_exact(&mut header)?;

        let dimensions = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let count = u64::from_le_bytes([
            header[4], header[5], header[6], header[7],
            header[8], header[9], header[10], header[11],
        ]) as usize;
        let max_connections = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
        let ef_construction = u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
        let ef_search = u32::from_le_bytes([header[20], header[21], header[22], header[23]]) as usize;

        let config = HnswConfig {
            max_connections,
            ef_construction,
            ef_search,
        };

        // Read points
        let mut points = Vec::with_capacity(count);
        let mut ids = Vec::with_capacity(count);

        for _ in 0..count {
            // Read ID (8 bytes)
            let mut id_bytes = [0u8; 8];
            reader.read_exact(&mut id_bytes)?;
            let id = i64::from_le_bytes(id_bytes);

            // Read vector
            let mut vec_bytes = vec![0u8; dimensions * 4];
            reader.read_exact(&mut vec_bytes)?;
            let vector: Vec<f32> = vec_bytes
                .chunks(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            ids.push(id);
            points.push(HnswPoint { vector });
        }

        // Build index from points
        let mut index = Self {
            hnsw: None,
            points: Vec::new(),
            ids: Vec::new(),
            dimensions,
            config,
        };

        if !points.is_empty() {
            index.points = points;
            index.ids = ids;
            index.rebuild()?;
        }

        Ok(index)
    }

    /// Save index to file
    pub fn save(&self, path: &Path) -> Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writer.write_all(&(self.dimensions as u32).to_le_bytes())?;
        writer.write_all(&(self.points.len() as u64).to_le_bytes())?;
        writer.write_all(&(self.config.max_connections as u32).to_le_bytes())?;
        writer.write_all(&(self.config.ef_construction as u32).to_le_bytes())?;
        writer.write_all(&(self.config.ef_search as u32).to_le_bytes())?;

        // Write points with IDs
        for (point, id) in self.points.iter().zip(self.ids.iter()) {
            writer.write_all(&id.to_le_bytes())?;
            for val in &point.vector {
                writer.write_all(&val.to_le_bytes())?;
            }
        }

        writer.flush()?;
        Ok(())
    }

    /// Get the number of vectors in the index
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Check if the index is empty
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Get the dimensions of vectors in this index
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Add a vector to the index (triggers rebuild)
    pub fn add(&mut self, id: i64, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            anyhow::bail!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                vector.len()
            );
        }

        // Normalize the vector
        let mut normalized = vector.to_vec();
        normalize_l2(&mut normalized);

        self.points.push(HnswPoint { vector: normalized });
        self.ids.push(id);

        // Mark index as needing rebuild
        self.hnsw = None;

        Ok(())
    }

    /// Remove a vector from the index
    pub fn remove(&mut self, id: i64) -> bool {
        if let Some(pos) = self.ids.iter().position(|&i| i == id) {
            self.points.remove(pos);
            self.ids.remove(pos);
            self.hnsw = None; // Mark for rebuild
            true
        } else {
            false
        }
    }

    /// Rebuild the HNSW index (call after batch adds/removes)
    pub fn rebuild(&mut self) -> Result<()> {
        if self.points.is_empty() {
            self.hnsw = None;
            return Ok(());
        }

        let hnsw = Builder::default()
            .ef_construction(self.config.ef_construction)
            .build(self.points.clone(), self.ids.clone());

        self.hnsw = Some(hnsw);
        Ok(())
    }

    /// Ensure index is built
    fn ensure_built(&mut self) -> Result<()> {
        if self.hnsw.is_none() && !self.points.is_empty() {
            self.rebuild()?;
        }
        Ok(())
    }

    /// Search for the k nearest neighbors
    pub fn search(&mut self, query: &[f32], k: usize) -> Result<Vec<HnswMatch>> {
        if query.len() != self.dimensions || self.is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_built()?;

        let hnsw = match &self.hnsw {
            Some(h) => h,
            None => return Ok(Vec::new()),
        };

        // Normalize query
        let mut query_normalized = query.to_vec();
        normalize_l2(&mut query_normalized);

        let query_point = HnswPoint {
            vector: query_normalized,
        };

        // Search
        let mut search = Search::default();
        let results = hnsw.search(&query_point, &mut search);

        // Convert to matches
        let matches: Vec<HnswMatch> = results
            .take(k)
            .map(|item| {
                HnswMatch {
                    id: *item.value,
                    // Convert distance back to similarity
                    similarity: 1.0 - item.distance,
                }
            })
            .collect();

        Ok(matches)
    }

    /// Bulk add vectors (more efficient than individual adds)
    pub fn add_batch(&mut self, ids: &[i64], vectors: &[Vec<f32>]) -> Result<()> {
        if ids.len() != vectors.len() {
            anyhow::bail!("IDs and vectors length mismatch");
        }

        for (id, vec) in ids.iter().zip(vectors.iter()) {
            if vec.len() != self.dimensions {
                anyhow::bail!(
                    "Vector dimension mismatch: expected {}, got {}",
                    self.dimensions,
                    vec.len()
                );
            }

            let mut normalized = vec.clone();
            normalize_l2(&mut normalized);

            self.points.push(HnswPoint { vector: normalized });
            self.ids.push(*id);
        }

        // Rebuild after batch add
        self.rebuild()?;

        Ok(())
    }

    /// Get index size in bytes (approximate)
    pub fn size_bytes(&self) -> u64 {
        let header = 24u64;
        let points = (self.points.len() * (8 + self.dimensions * 4)) as u64;
        // HNSW graph overhead (estimated)
        let graph_overhead = (self.points.len() * self.config.max_connections * 8) as u64;
        header + points + graph_overhead
    }

    /// Get configuration
    pub fn config(&self) -> &HnswConfig {
        &self.config
    }
}

/// Normalize a vector to unit length (L2 normalization)
fn normalize_l2(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in vec.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn random_vector(dim: usize, seed: u64) -> Vec<f32> {
        let mut v = vec![0.0; dim];
        let mut state = seed;
        for x in &mut v {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *x = ((state as f32) / (u64::MAX as f32)) * 2.0 - 1.0;
        }
        // Normalize
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v {
            *x /= norm;
        }
        v
    }

    #[test]
    fn test_add_and_search() {
        let mut index = HnswIndex::new(4);

        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.9, 0.1, 0.0, 0.0]).unwrap();
        index.add(3, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.rebuild().unwrap();

        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 2).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1); // Exact match should be first
    }

    #[test]
    fn test_save_and_load() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.hnsw");

        let mut index = HnswIndex::new(4);
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.rebuild().unwrap();
        index.save(&path).unwrap();

        let mut loaded = HnswIndex::load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.dimensions(), 4);

        // Verify search still works
        let results = loaded.search(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].id, 1);
    }

    #[test]
    fn test_remove() {
        let mut index = HnswIndex::new(4);
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.rebuild().unwrap();

        assert_eq!(index.len(), 2);
        assert!(index.remove(1));
        assert_eq!(index.len(), 1);
        assert!(!index.remove(1)); // Already removed
    }

    #[test]
    fn test_search_large() {
        let mut index = HnswIndex::new(384);

        // Add 1000 random vectors
        let mut ids = Vec::new();
        let mut vectors = Vec::new();
        for i in 0..1000 {
            ids.push(i as i64);
            vectors.push(random_vector(384, i as u64));
        }
        index.add_batch(&ids, &vectors).unwrap();

        // Search should return top-10
        let query = random_vector(384, 42);
        let results = index.search(&query, 10).unwrap();

        assert_eq!(results.len(), 10);
        // Results should be sorted by similarity (descending)
        for i in 1..results.len() {
            assert!(results[i-1].similarity >= results[i].similarity);
        }
    }

    #[test]
    fn test_batch_add() {
        let mut index = HnswIndex::new(4);

        let ids = vec![1, 2, 3];
        let vectors = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ];

        index.add_batch(&ids, &vectors).unwrap();
        assert_eq!(index.len(), 3);

        let results = index.search(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].id, 1);
    }
}
