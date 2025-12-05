//! Vector index for fast nearest neighbor search
//!
//! Uses a simple but efficient approach for vector search.
//! Can be upgraded to usearch for HNSW when needed.

#![allow(dead_code)] // Many methods reserved for future index operations

use anyhow::Result;
use std::collections::BinaryHeap;
use std::cmp::Ordering;
use std::path::Path;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

use super::model::cosine_similarity;

/// A match result from vector search
#[derive(Debug, Clone)]
pub struct VectorMatch {
    /// Pattern ID
    pub id: i64,
    /// Similarity score (0.0 to 1.0)
    pub similarity: f32,
}

impl Eq for VectorMatch {}

impl PartialEq for VectorMatch {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl PartialOrd for VectorMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VectorMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior
        other.similarity.partial_cmp(&self.similarity)
            .unwrap_or(Ordering::Equal)
    }
}

/// Vector index for fast nearest neighbor search
pub struct VectorIndex {
    /// Pattern IDs
    ids: Vec<i64>,
    /// Embedding vectors (flattened)
    vectors: Vec<f32>,
    /// Dimensions per vector
    dimensions: usize,
}

impl VectorIndex {
    /// Create a new empty index
    pub fn new(dimensions: usize) -> Self {
        Self {
            ids: Vec::new(),
            vectors: Vec::new(),
            dimensions,
        }
    }

    /// Load index from file
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        // Read header: dimensions (u32), count (u64)
        let mut header = [0u8; 12];
        reader.read_exact(&mut header)?;

        let dimensions = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let count = u64::from_le_bytes([
            header[4], header[5], header[6], header[7],
            header[8], header[9], header[10], header[11],
        ]) as usize;

        // Read IDs
        let mut id_bytes = vec![0u8; count * 8];
        reader.read_exact(&mut id_bytes)?;
        let ids: Vec<i64> = id_bytes
            .chunks(8)
            .map(|chunk| i64::from_le_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3],
                chunk[4], chunk[5], chunk[6], chunk[7],
            ]))
            .collect();

        // Read vectors
        let mut vec_bytes = vec![0u8; count * dimensions * 4];
        reader.read_exact(&mut vec_bytes)?;
        let vectors: Vec<f32> = vec_bytes
            .chunks(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        Ok(Self {
            ids,
            vectors,
            dimensions,
        })
    }

    /// Save index to file
    pub fn save(&self, path: &Path) -> Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writer.write_all(&(self.dimensions as u32).to_le_bytes())?;
        writer.write_all(&(self.ids.len() as u64).to_le_bytes())?;

        // Write IDs
        for id in &self.ids {
            writer.write_all(&id.to_le_bytes())?;
        }

        // Write vectors
        for val in &self.vectors {
            writer.write_all(&val.to_le_bytes())?;
        }

        writer.flush()?;
        Ok(())
    }

    /// Get the number of vectors in the index
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Check if the index is empty
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Get the dimensions of vectors in this index
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Add a vector to the index
    pub fn add(&mut self, id: i64, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            anyhow::bail!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimensions,
                vector.len()
            );
        }

        self.ids.push(id);
        self.vectors.extend_from_slice(vector);
        Ok(())
    }

    /// Remove a vector from the index
    pub fn remove(&mut self, id: i64) -> bool {
        if let Some(pos) = self.ids.iter().position(|&x| x == id) {
            self.ids.remove(pos);
            let start = pos * self.dimensions;
            let end = start + self.dimensions;
            self.vectors.drain(start..end);
            true
        } else {
            false
        }
    }

    /// Search for the k nearest neighbors
    pub fn search(&self, query: &[f32], k: usize) -> Vec<VectorMatch> {
        if query.len() != self.dimensions || self.is_empty() {
            return Vec::new();
        }

        // Use a min-heap to keep track of top-k
        let mut heap: BinaryHeap<VectorMatch> = BinaryHeap::new();

        for (i, id) in self.ids.iter().enumerate() {
            let start = i * self.dimensions;
            let end = start + self.dimensions;
            let vec = &self.vectors[start..end];

            let similarity = cosine_similarity(query, vec);

            if heap.len() < k {
                heap.push(VectorMatch { id: *id, similarity });
            } else if let Some(min) = heap.peek() {
                if similarity > min.similarity {
                    heap.pop();
                    heap.push(VectorMatch { id: *id, similarity });
                }
            }
        }

        // Convert to sorted vector (highest similarity first)
        let mut results: Vec<VectorMatch> = heap.into_iter().collect();
        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(Ordering::Equal));
        results
    }

    /// Bulk add vectors
    pub fn add_batch(&mut self, ids: &[i64], vectors: &[Vec<f32>]) -> Result<()> {
        if ids.len() != vectors.len() {
            anyhow::bail!("IDs and vectors length mismatch");
        }

        for (id, vec) in ids.iter().zip(vectors.iter()) {
            self.add(*id, vec)?;
        }
        Ok(())
    }

    /// Get index size in bytes (approximate)
    pub fn size_bytes(&self) -> u64 {
        let header = 12u64;
        let ids = (self.ids.len() * 8) as u64;
        let vecs = (self.vectors.len() * 4) as u64;
        header + ids + vecs
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
        let mut index = VectorIndex::new(4);

        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.9, 0.1, 0.0, 0.0]).unwrap();
        index.add(3, &[0.0, 1.0, 0.0, 0.0]).unwrap();

        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 2);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 1); // Exact match
        assert_eq!(results[1].id, 2); // Close match
    }

    #[test]
    fn test_save_and_load() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.idx");

        let mut index = VectorIndex::new(4);
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();
        index.save(&path).unwrap();

        let loaded = VectorIndex::load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.dimensions(), 4);
    }

    #[test]
    fn test_remove() {
        let mut index = VectorIndex::new(4);
        index.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        index.add(2, &[0.0, 1.0, 0.0, 0.0]).unwrap();

        assert_eq!(index.len(), 2);
        assert!(index.remove(1));
        assert_eq!(index.len(), 1);
        assert!(!index.remove(1)); // Already removed
    }

    #[test]
    fn test_search_large() {
        let mut index = VectorIndex::new(384);

        // Add 1000 random vectors
        for i in 0..1000 {
            let vec = random_vector(384, i as u64);
            index.add(i, &vec).unwrap();
        }

        // Search should return top-10
        let query = random_vector(384, 42);
        let results = index.search(&query, 10);

        assert_eq!(results.len(), 10);
        // Results should be sorted by similarity
        for i in 1..results.len() {
            assert!(results[i-1].similarity >= results[i].similarity);
        }
    }
}
