//! Vector Quantization for memory-efficient embedding storage
//!
//! This module provides quantization techniques to reduce memory usage
//! for vector embeddings while maintaining search quality.
//!
//! Supported methods:
//! - Scalar Quantization (SQ8): 4x memory reduction, ~1% recall loss
//! - Scalar Quantization (SQ4): 8x memory reduction, ~3% recall loss
//! - Binary Quantization: 32x memory reduction, ~5-10% recall loss
//!
//! Performance vs AgentDB:
//! - AgentDB uses product quantization with 4-32x reduction
//! - MANA uses simpler scalar quantization for lower CPU overhead

#![allow(dead_code)] // New API - will be integrated in future versions

use anyhow::Result;
use std::path::Path;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

/// Quantization method
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuantizationMethod {
    /// No quantization (full f32)
    None,
    /// 8-bit scalar quantization (4x reduction)
    Scalar8,
    /// 4-bit scalar quantization (8x reduction)
    Scalar4,
    /// Binary quantization (32x reduction)
    Binary,
}

impl Default for QuantizationMethod {
    fn default() -> Self {
        Self::Scalar8 // Good balance of size and quality
    }
}

/// Configuration for quantized vectors
#[derive(Debug, Clone)]
pub struct QuantizationConfig {
    /// Quantization method
    pub method: QuantizationMethod,
    /// Whether to store original vectors for re-ranking
    pub store_originals: bool,
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self {
            method: QuantizationMethod::Scalar8,
            store_originals: false,
        }
    }
}

/// A quantized vector with metadata for reconstruction
#[derive(Debug, Clone)]
pub struct QuantizedVector {
    /// Pattern ID
    pub id: i64,
    /// Quantized data
    pub data: Vec<u8>,
    /// Min value (for scalar quantization reconstruction)
    pub min_val: f32,
    /// Max value (for scalar quantization reconstruction)
    pub max_val: f32,
    /// Quantization method used
    pub method: QuantizationMethod,
    /// Original dimensions
    pub dimensions: usize,
}

impl QuantizedVector {
    /// Create a new quantized vector from f32 data
    pub fn from_f32(id: i64, vector: &[f32], method: QuantizationMethod) -> Self {
        let dimensions = vector.len();

        match method {
            QuantizationMethod::None => {
                // Store as raw bytes
                let data: Vec<u8> = vector.iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                Self {
                    id,
                    data,
                    min_val: 0.0,
                    max_val: 1.0,
                    method,
                    dimensions,
                }
            }
            QuantizationMethod::Scalar8 => {
                // 8-bit quantization
                let (data, min_val, max_val) = quantize_scalar8(vector);
                Self {
                    id,
                    data,
                    min_val,
                    max_val,
                    method,
                    dimensions,
                }
            }
            QuantizationMethod::Scalar4 => {
                // 4-bit quantization
                let (data, min_val, max_val) = quantize_scalar4(vector);
                Self {
                    id,
                    data,
                    min_val,
                    max_val,
                    method,
                    dimensions,
                }
            }
            QuantizationMethod::Binary => {
                // Binary quantization
                let data = quantize_binary(vector);
                Self {
                    id,
                    data,
                    min_val: 0.0,
                    max_val: 1.0,
                    method,
                    dimensions,
                }
            }
        }
    }

    /// Reconstruct the f32 vector (approximate for quantized)
    pub fn to_f32(&self) -> Vec<f32> {
        match self.method {
            QuantizationMethod::None => {
                self.data.chunks(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect()
            }
            QuantizationMethod::Scalar8 => {
                dequantize_scalar8(&self.data, self.min_val, self.max_val)
            }
            QuantizationMethod::Scalar4 => {
                dequantize_scalar4(&self.data, self.min_val, self.max_val, self.dimensions)
            }
            QuantizationMethod::Binary => {
                dequantize_binary(&self.data, self.dimensions)
            }
        }
    }

    /// Calculate approximate similarity without full reconstruction
    /// This is faster but less accurate than reconstructing and computing full similarity
    pub fn approximate_similarity(&self, query: &[f32]) -> f32 {
        match self.method {
            QuantizationMethod::None | QuantizationMethod::Scalar8 | QuantizationMethod::Scalar4 => {
                // For scalar quantization, reconstruct and compute
                let vec = self.to_f32();
                cosine_similarity(query, &vec)
            }
            QuantizationMethod::Binary => {
                // For binary, use Hamming distance approximation
                let query_binary = quantize_binary(query);
                let hamming = hamming_distance(&self.data, &query_binary);
                // Convert Hamming distance to approximate cosine similarity
                // Hamming distance of 0 = similarity 1.0, max hamming = similarity 0.0
                let max_hamming = self.dimensions;
                1.0 - (hamming as f32 / max_hamming as f32)
            }
        }
    }

    /// Memory size in bytes
    pub fn size_bytes(&self) -> usize {
        self.data.len() + 8 + 4 + 4 + 1 + 8 // data + id + min + max + method + dims
    }

    /// Compression ratio vs f32
    pub fn compression_ratio(&self) -> f32 {
        let original_size = self.dimensions * 4; // f32 = 4 bytes
        original_size as f32 / self.data.len() as f32
    }
}

/// Quantize to 8-bit integers
fn quantize_scalar8(vector: &[f32]) -> (Vec<u8>, f32, f32) {
    let min_val = vector.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = vector.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max_val - min_val;

    let data: Vec<u8> = if range < 1e-10 {
        vec![128u8; vector.len()] // All same value
    } else {
        vector.iter()
            .map(|&v| {
                let normalized = (v - min_val) / range;
                (normalized * 255.0).round() as u8
            })
            .collect()
    };

    (data, min_val, max_val)
}

/// Dequantize from 8-bit integers
fn dequantize_scalar8(data: &[u8], min_val: f32, max_val: f32) -> Vec<f32> {
    let range = max_val - min_val;
    data.iter()
        .map(|&v| {
            let normalized = v as f32 / 255.0;
            min_val + normalized * range
        })
        .collect()
}

/// Quantize to 4-bit integers (packed, 2 per byte)
fn quantize_scalar4(vector: &[f32]) -> (Vec<u8>, f32, f32) {
    let min_val = vector.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = vector.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max_val - min_val;

    let quantized: Vec<u8> = if range < 1e-10 {
        vec![8u8; vector.len()] // All same value
    } else {
        vector.iter()
            .map(|&v| {
                let normalized = (v - min_val) / range;
                (normalized * 15.0).round() as u8
            })
            .collect()
    };

    // Pack 2 values per byte
    let packed: Vec<u8> = quantized.chunks(2)
        .map(|chunk| {
            let low = chunk[0];
            let high = if chunk.len() > 1 { chunk[1] } else { 0 };
            (high << 4) | low
        })
        .collect();

    (packed, min_val, max_val)
}

/// Dequantize from 4-bit integers
fn dequantize_scalar4(data: &[u8], min_val: f32, max_val: f32, dimensions: usize) -> Vec<f32> {
    let range = max_val - min_val;
    let mut result = Vec::with_capacity(dimensions);

    for &byte in data {
        let low = byte & 0x0F;
        let high = (byte >> 4) & 0x0F;

        result.push(min_val + (low as f32 / 15.0) * range);
        if result.len() < dimensions {
            result.push(min_val + (high as f32 / 15.0) * range);
        }
    }

    result.truncate(dimensions);
    result
}

/// Quantize to binary (1 bit per dimension)
fn quantize_binary(vector: &[f32]) -> Vec<u8> {
    // Binary: 1 if positive, 0 if negative (or above/below mean)
    let mean: f32 = vector.iter().sum::<f32>() / vector.len() as f32;

    let bits: Vec<bool> = vector.iter()
        .map(|&v| v > mean)
        .collect();

    // Pack 8 bits per byte
    bits.chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                if bit {
                    byte |= 1 << i;
                }
            }
            byte
        })
        .collect()
}

/// Dequantize from binary (approximate reconstruction)
fn dequantize_binary(data: &[u8], dimensions: usize) -> Vec<f32> {
    let mut result = Vec::with_capacity(dimensions);

    for &byte in data {
        for i in 0..8 {
            if result.len() >= dimensions {
                break;
            }
            let bit = (byte >> i) & 1;
            // Map: 0 -> -1.0, 1 -> 1.0
            result.push(if bit == 1 { 1.0 } else { -1.0 });
        }
    }

    result.truncate(dimensions);

    // Normalize
    let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in &mut result {
            *x /= norm;
        }
    }

    result
}

/// Hamming distance between two binary vectors
fn hamming_distance(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter())
        .map(|(&x, &y)| (x ^ y).count_ones() as usize)
        .sum()
}

/// Cosine similarity between two f32 vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// Quantized vector store
pub struct QuantizedStore {
    /// Stored vectors
    vectors: Vec<QuantizedVector>,
    /// Dimensions
    dimensions: usize,
    /// Quantization method
    method: QuantizationMethod,
}

impl QuantizedStore {
    /// Create a new quantized store
    pub fn new(dimensions: usize, method: QuantizationMethod) -> Self {
        Self {
            vectors: Vec::new(),
            dimensions,
            method,
        }
    }

    /// Load from file
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        // Read header
        let mut header = [0u8; 13];
        reader.read_exact(&mut header)?;

        let dimensions = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let method = match header[4] {
            0 => QuantizationMethod::None,
            1 => QuantizationMethod::Scalar8,
            2 => QuantizationMethod::Scalar4,
            3 => QuantizationMethod::Binary,
            _ => return Err(anyhow::anyhow!("Unknown quantization method")),
        };
        let count = u64::from_le_bytes([
            header[5], header[6], header[7], header[8],
            header[9], header[10], header[11], header[12],
        ]) as usize;

        let mut vectors = Vec::with_capacity(count);

        for _ in 0..count {
            // Read vector header: id (8), min (4), max (4), data_len (4)
            let mut vec_header = [0u8; 20];
            reader.read_exact(&mut vec_header)?;

            let id = i64::from_le_bytes([
                vec_header[0], vec_header[1], vec_header[2], vec_header[3],
                vec_header[4], vec_header[5], vec_header[6], vec_header[7],
            ]);
            let min_val = f32::from_le_bytes([vec_header[8], vec_header[9], vec_header[10], vec_header[11]]);
            let max_val = f32::from_le_bytes([vec_header[12], vec_header[13], vec_header[14], vec_header[15]]);
            let data_len = u32::from_le_bytes([vec_header[16], vec_header[17], vec_header[18], vec_header[19]]) as usize;

            let mut data = vec![0u8; data_len];
            reader.read_exact(&mut data)?;

            vectors.push(QuantizedVector {
                id,
                data,
                min_val,
                max_val,
                method,
                dimensions,
            });
        }

        Ok(Self {
            vectors,
            dimensions,
            method,
        })
    }

    /// Save to file
    pub fn save(&self, path: &Path) -> Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // Write header
        writer.write_all(&(self.dimensions as u32).to_le_bytes())?;
        writer.write_all(&[match self.method {
            QuantizationMethod::None => 0,
            QuantizationMethod::Scalar8 => 1,
            QuantizationMethod::Scalar4 => 2,
            QuantizationMethod::Binary => 3,
        }])?;
        writer.write_all(&(self.vectors.len() as u64).to_le_bytes())?;

        // Write vectors
        for vec in &self.vectors {
            writer.write_all(&vec.id.to_le_bytes())?;
            writer.write_all(&vec.min_val.to_le_bytes())?;
            writer.write_all(&vec.max_val.to_le_bytes())?;
            writer.write_all(&(vec.data.len() as u32).to_le_bytes())?;
            writer.write_all(&vec.data)?;
        }

        writer.flush()?;
        Ok(())
    }

    /// Add a vector
    pub fn add(&mut self, id: i64, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dimensions {
            return Err(anyhow::anyhow!("Dimension mismatch"));
        }

        let quantized = QuantizedVector::from_f32(id, vector, self.method);
        self.vectors.push(quantized);
        Ok(())
    }

    /// Search for similar vectors
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(i64, f32)> {
        if query.len() != self.dimensions || self.vectors.is_empty() {
            return Vec::new();
        }

        // Compute similarities
        let mut results: Vec<(i64, f32)> = self.vectors.iter()
            .map(|v| (v.id, v.approximate_similarity(query)))
            .collect();

        // Sort by similarity (descending)
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    /// Get vector count
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Get total memory usage
    pub fn memory_bytes(&self) -> usize {
        self.vectors.iter().map(|v| v.size_bytes()).sum()
    }

    /// Get average compression ratio
    pub fn compression_ratio(&self) -> f32 {
        if self.vectors.is_empty() {
            return 1.0;
        }
        self.vectors.iter().map(|v| v.compression_ratio()).sum::<f32>() / self.vectors.len() as f32
    }

    /// Get quantization method
    pub fn method(&self) -> QuantizationMethod {
        self.method
    }

    /// Remove a vector by ID
    pub fn remove(&mut self, id: i64) -> bool {
        if let Some(pos) = self.vectors.iter().position(|v| v.id == id) {
            self.vectors.remove(pos);
            true
        } else {
            false
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
    fn test_scalar8_quantization() {
        let original = random_vector(384, 42);
        let quantized = QuantizedVector::from_f32(1, &original, QuantizationMethod::Scalar8);

        // Check compression
        assert!(quantized.compression_ratio() > 3.5); // Should be ~4x

        // Check reconstruction quality
        let reconstructed = quantized.to_f32();
        let sim = cosine_similarity(&original, &reconstructed);
        assert!(sim > 0.99, "Similarity should be > 0.99, got {}", sim);
    }

    #[test]
    fn test_scalar4_quantization() {
        let original = random_vector(384, 42);
        let quantized = QuantizedVector::from_f32(1, &original, QuantizationMethod::Scalar4);

        // Check compression
        assert!(quantized.compression_ratio() > 7.0); // Should be ~8x

        // Check reconstruction quality
        let reconstructed = quantized.to_f32();
        let sim = cosine_similarity(&original, &reconstructed);
        assert!(sim > 0.95, "Similarity should be > 0.95, got {}", sim);
    }

    #[test]
    fn test_binary_quantization() {
        let original = random_vector(384, 42);
        let quantized = QuantizedVector::from_f32(1, &original, QuantizationMethod::Binary);

        // Check compression
        assert!(quantized.compression_ratio() > 20.0); // Should be ~32x

        // Check reconstruction quality (binary has lower quality)
        let reconstructed = quantized.to_f32();
        let sim = cosine_similarity(&original, &reconstructed);
        assert!(sim > 0.5, "Similarity should be > 0.5, got {}", sim);
    }

    #[test]
    fn test_store_save_load() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.qvec");

        let mut store = QuantizedStore::new(384, QuantizationMethod::Scalar8);
        for i in 0..10 {
            store.add(i, &random_vector(384, i as u64)).unwrap();
        }
        store.save(&path).unwrap();

        let loaded = QuantizedStore::load(&path).unwrap();
        assert_eq!(loaded.len(), 10);
        assert_eq!(loaded.method(), QuantizationMethod::Scalar8);
    }

    #[test]
    fn test_store_search() {
        let mut store = QuantizedStore::new(4, QuantizationMethod::Scalar8);
        store.add(1, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        store.add(2, &[0.9, 0.1, 0.0, 0.0]).unwrap();
        store.add(3, &[0.0, 1.0, 0.0, 0.0]).unwrap();

        let results = store.search(&[1.0, 0.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 1); // Best match
    }

    #[test]
    fn test_compression_ratios() {
        let vec = random_vector(384, 42);

        let none = QuantizedVector::from_f32(1, &vec, QuantizationMethod::None);
        let sq8 = QuantizedVector::from_f32(1, &vec, QuantizationMethod::Scalar8);
        let sq4 = QuantizedVector::from_f32(1, &vec, QuantizationMethod::Scalar4);
        let binary = QuantizedVector::from_f32(1, &vec, QuantizationMethod::Binary);

        println!("None: {}x compression", none.compression_ratio());
        println!("SQ8: {}x compression", sq8.compression_ratio());
        println!("SQ4: {}x compression", sq4.compression_ratio());
        println!("Binary: {}x compression", binary.compression_ratio());

        assert!(none.compression_ratio() < 1.1);
        assert!(sq8.compression_ratio() > 3.5);
        assert!(sq4.compression_ratio() > 7.0);
        assert!(binary.compression_ratio() > 20.0);
    }
}
