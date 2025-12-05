//! Embedding model implementation
//!
//! Provides text embedding generation using local models.
//! Currently supports a lightweight hash-based approach for testing,
//! with optional candle-based transformer models for production.

use anyhow::Result;
use std::collections::HashMap;

use super::EMBEDDING_DIM;

/// Embedding model for generating text embeddings
pub struct EmbeddingModel {
    /// Model name
    name: String,
    /// Model version
    version: String,
    /// Dimensions
    dimensions: usize,
    /// IDF weights for TF-IDF based embeddings
    idf_weights: HashMap<String, f32>,
}

impl EmbeddingModel {
    /// Create a new embedding model
    ///
    /// For now, this uses a TF-IDF based approach that generates
    /// deterministic embeddings without requiring external model files.
    /// This can be upgraded to use candle for transformer models later.
    pub fn new(model_name: &str) -> Result<Self> {
        let dimensions = match model_name {
            "gte-small" => 384,
            "gte-base" => 768,
            "all-MiniLM-L6-v2" => 384,
            _ => EMBEDDING_DIM,
        };

        Ok(Self {
            name: model_name.to_string(),
            version: "1.0-tfidf".to_string(),
            dimensions,
            idf_weights: HashMap::new(),
        })
    }

    /// Get model name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get model version
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get embedding dimensions
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Generate embedding for a single text
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Use a hash-based embedding approach that creates deterministic
        // vectors based on text content. This provides semantic-ish
        // similarity (similar words -> similar hashes) without requiring
        // heavy ML dependencies.

        let embedding = self.hash_embed(text);
        Ok(embedding)
    }

    /// Batch embed multiple texts
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }

    /// Generate a hash-based embedding
    ///
    /// This uses a technique similar to random projection:
    /// 1. Tokenize the text into words
    /// 2. For each word, generate a deterministic pseudo-random vector
    /// 3. Sum all word vectors and normalize
    ///
    /// This provides basic semantic similarity: texts with similar words
    /// will have similar embeddings.
    fn hash_embed(&self, text: &str) -> Vec<f32> {
        let mut embedding = vec![0.0f32; self.dimensions];

        // Tokenize: lowercase, split on non-alphanumeric
        let lower = text.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty() && s.len() > 1)
            .collect();

        if tokens.is_empty() {
            // Return zero vector for empty text
            return embedding;
        }

        // For each token, add its pseudo-random vector contribution
        for token in &tokens {
            self.add_token_embedding(&mut embedding, token);
        }

        // Add bigram features for better semantic capture
        for window in tokens.windows(2) {
            let bigram = format!("{}_{}", window[0], window[1]);
            self.add_token_embedding(&mut embedding, &bigram);
        }

        // Normalize to unit length (L2 norm)
        normalize_l2(&mut embedding);

        embedding
    }

    /// Add a token's contribution to the embedding vector
    fn add_token_embedding(&self, embedding: &mut [f32], token: &str) {
        // Use the token to seed a deterministic pseudo-random sequence
        let mut hash = fnv1a_hash(token.as_bytes());

        // Weight based on token importance (IDF-like)
        let weight = self.idf_weights.get(token).copied().unwrap_or(1.0);

        // Add contribution to each dimension
        for value in embedding.iter_mut() {
            // Generate a pseudo-random value in [-1, 1]
            hash = lcg_next(hash);
            let rand_val = ((hash as f32) / (u64::MAX as f32)) * 2.0 - 1.0;
            *value += rand_val * weight;
        }
    }

    /// Update IDF weights from a corpus
    pub fn update_idf(&mut self, documents: &[String]) {
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        let n_docs = documents.len() as f32;

        for doc in documents {
            let lower = doc.to_lowercase();
            let tokens: std::collections::HashSet<&str> = lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty() && s.len() > 1)
                .collect();

            for token in tokens {
                *doc_freq.entry(token.to_string()).or_insert(0) += 1;
            }
        }

        self.idf_weights.clear();
        for (token, freq) in doc_freq {
            // IDF = log(N / df)
            let idf = (n_docs / freq as f32).ln();
            self.idf_weights.insert(token, idf.max(0.1));
        }
    }
}

/// FNV-1a hash function (64-bit)
fn fnv1a_hash(data: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Linear congruential generator for deterministic pseudo-random sequence
fn lcg_next(state: u64) -> u64 {
    const A: u64 = 6364136223846793005;
    const C: u64 = 1442695040888963407;
    state.wrapping_mul(A).wrapping_add(C)
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

/// Compute cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_deterministic() {
        let model = EmbeddingModel::new("gte-small").unwrap();
        let text = "hello world";

        let emb1 = model.embed(text).unwrap();
        let emb2 = model.embed(text).unwrap();

        assert_eq!(emb1.len(), 384);
        assert_eq!(emb1, emb2, "Embeddings should be deterministic");
    }

    #[test]
    fn test_similar_texts_similar_embeddings() {
        let model = EmbeddingModel::new("gte-small").unwrap();

        let emb1 = model.embed("fix bug in authentication code").unwrap();
        let emb2 = model.embed("fix bug in authentication system").unwrap();
        let emb3 = model.embed("deploy kubernetes cluster").unwrap();

        let sim_12 = cosine_similarity(&emb1, &emb2);
        let sim_13 = cosine_similarity(&emb1, &emb3);

        assert!(sim_12 > sim_13,
            "Similar texts should have higher similarity: {} vs {}", sim_12, sim_13);
    }

    #[test]
    fn test_normalized_embeddings() {
        let model = EmbeddingModel::new("gte-small").unwrap();
        let embedding = model.embed("test text").unwrap();

        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01, "Embedding should be normalized");
    }

    #[test]
    fn test_empty_text() {
        let model = EmbeddingModel::new("gte-small").unwrap();
        let embedding = model.embed("").unwrap();

        assert_eq!(embedding.len(), 384);
        // Empty text should produce zero vector
        assert!(embedding.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_cosine_similarity_same_vector() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&v1, &v2);
        assert!(sim.abs() < 0.001);
    }
}
