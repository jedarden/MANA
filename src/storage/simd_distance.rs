//! SIMD-accelerated distance metrics
//!
//! Uses simsimd for hardware-accelerated vector operations:
//! - Cosine similarity (normalized vectors)
//! - Euclidean distance
//! - Dot product
//! - Inner product
//!
//! Performance target: <1μs for 384-dimensional vectors

use simsimd::SpatialSimilarity;

/// Distance metric types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMetric {
    Cosine,
    Euclidean,
    DotProduct,
    InnerProduct,
}

/// SIMD-optimized distance calculator
pub struct SimdDistance {
    metric: DistanceMetric,
}

impl SimdDistance {
    pub fn new(metric: DistanceMetric) -> Self {
        Self { metric }
    }

    /// Calculate similarity between two vectors (SIMD-accelerated)
    /// Returns value in range [0.0, 1.0] for similarity metrics
    #[inline]
    pub fn similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.metric {
            DistanceMetric::Cosine => {
                // simsimd returns cosine distance as f64, convert to similarity
                (1.0 - f32::cosine(a, b).unwrap_or(1.0)) as f32
            }
            DistanceMetric::DotProduct => {
                f32::dot(a, b).unwrap_or(0.0) as f32
            }
            DistanceMetric::Euclidean => {
                // Convert distance to similarity
                let dist = f32::sqeuclidean(a, b).unwrap_or(f64::MAX).sqrt();
                (1.0 / (1.0 + dist)) as f32
            }
            DistanceMetric::InnerProduct => {
                f32::dot(a, b).unwrap_or(0.0) as f32
            }
        }
    }

    /// Calculate distance between two vectors
    #[inline]
    pub fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.metric {
            DistanceMetric::Cosine => {
                f32::cosine(a, b).unwrap_or(1.0) as f32
            }
            DistanceMetric::Euclidean => {
                f32::sqeuclidean(a, b).unwrap_or(f64::MAX).sqrt() as f32
            }
            DistanceMetric::DotProduct => {
                // For dot product, higher is more similar
                -f32::dot(a, b).unwrap_or(0.0) as f32
            }
            DistanceMetric::InnerProduct => {
                -f32::dot(a, b).unwrap_or(0.0) as f32
            }
        }
    }

    /// Batch similarity calculation for multiple vectors against a query
    pub fn batch_similarity(&self, query: &[f32], vectors: &[Vec<f32>]) -> Vec<f32> {
        vectors.iter().map(|v| self.similarity(query, v)).collect()
    }

    /// Find top-k most similar vectors
    pub fn top_k(&self, query: &[f32], vectors: &[(usize, Vec<f32>)], k: usize) -> Vec<(usize, f32)> {
        let mut scored: Vec<(usize, f32)> = vectors
            .iter()
            .map(|(id, v)| (*id, self.similarity(query, v)))
            .collect();

        // Partial sort for efficiency
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }
}

/// Benchmark SIMD vs non-SIMD performance
pub fn benchmark_simd(dimensions: usize, iterations: usize) -> SimdBenchmarkResult {
    use std::time::Instant;

    // Generate random vectors
    let a: Vec<f32> = (0..dimensions).map(|i| (i as f32 * 0.01).sin()).collect();
    let b: Vec<f32> = (0..dimensions).map(|i| (i as f32 * 0.02).cos()).collect();

    // SIMD timing
    let simd = SimdDistance::new(DistanceMetric::Cosine);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = simd.similarity(&a, &b);
    }
    let simd_time = start.elapsed();

    // Non-SIMD timing (naive implementation)
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = naive_cosine(&a, &b);
    }
    let naive_time = start.elapsed();

    SimdBenchmarkResult {
        dimensions,
        iterations,
        simd_ns_per_op: simd_time.as_nanos() as f64 / iterations as f64,
        naive_ns_per_op: naive_time.as_nanos() as f64 / iterations as f64,
        speedup: naive_time.as_nanos() as f64 / simd_time.as_nanos() as f64,
    }
}

fn naive_cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b)
}

#[derive(Debug)]
pub struct SimdBenchmarkResult {
    pub dimensions: usize,
    pub iterations: usize,
    pub simd_ns_per_op: f64,
    pub naive_ns_per_op: f64,
    pub speedup: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let simd = SimdDistance::new(DistanceMetric::Cosine);

        // Identical vectors should have similarity ~1.0
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![1.0, 2.0, 3.0, 4.0];
        let sim = simd.similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.01, "Identical vectors should have similarity ~1.0, got {}", sim);

        // Orthogonal vectors should have similarity ~0.5 (since we do 1.0 - distance)
        let a = vec![1.0, 0.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0, 0.0];
        let sim = simd.similarity(&a, &b);
        assert!(sim < 0.6 && sim > 0.4, "Orthogonal vectors should have similarity ~0.5, got {}", sim);
    }

    #[test]
    fn test_euclidean_distance() {
        let simd = SimdDistance::new(DistanceMetric::Euclidean);

        // Identical vectors should have distance ~0.0
        let a = vec![1.0, 2.0, 3.0, 4.0];
        let b = vec![1.0, 2.0, 3.0, 4.0];
        let dist = simd.distance(&a, &b);
        assert!(dist < 0.01, "Identical vectors should have distance ~0.0, got {}", dist);
    }

    #[test]
    fn test_batch_similarity() {
        let simd = SimdDistance::new(DistanceMetric::Cosine);
        let query = vec![1.0, 2.0, 3.0, 4.0];
        let vectors = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![2.0, 3.0, 4.0, 5.0],
            vec![0.0, 0.0, 0.0, 1.0],
        ];

        let similarities = simd.batch_similarity(&query, &vectors);
        assert_eq!(similarities.len(), 3);
        assert!(similarities[0] > similarities[1]); // First is most similar
    }

    #[test]
    fn test_top_k() {
        let simd = SimdDistance::new(DistanceMetric::Cosine);
        let query = vec![1.0, 2.0, 3.0, 4.0];
        let vectors = vec![
            (0, vec![1.0, 2.0, 3.0, 4.0]),
            (1, vec![2.0, 3.0, 4.0, 5.0]),
            (2, vec![0.0, 0.0, 0.0, 1.0]),
        ];

        let top2 = simd.top_k(&query, &vectors, 2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].0, 0); // First vector should be most similar
    }

    #[test]
    fn test_benchmark_simd() {
        let result = benchmark_simd(384, 1000);
        assert!(result.speedup > 1.0, "SIMD should be faster than naive implementation");
        println!("SIMD speedup: {:.2}x", result.speedup);
        println!("SIMD: {:.2}ns/op, Naive: {:.2}ns/op", result.simd_ns_per_op, result.naive_ns_per_op);
    }
}
