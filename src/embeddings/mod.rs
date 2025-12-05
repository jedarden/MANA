//! Embeddings module for semantic similarity matching
//!
//! This module provides vector embeddings for patterns to enable
//! semantic similarity search instead of basic string matching.
//!
//! Architecture:
//! - EmbeddingModel: Generates embeddings from text
//! - VectorIndex: HNSW index for fast nearest neighbor search
//! - EmbeddingStore: Manages embedding persistence and caching

use anyhow::Result;
use std::path::Path;

mod model;
mod index;
mod store;

pub use model::EmbeddingModel;
pub use index::VectorIndex;
pub use store::EmbeddingStore;

/// Embedding dimensions for the default model (gte-small)
pub const EMBEDDING_DIM: usize = 384;

/// Configuration for embeddings
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Model name (gte-small, all-MiniLM-L6-v2, etc.)
    pub model: String,
    /// Embedding dimensions
    pub dimensions: usize,
    /// Batch size for embedding generation
    #[allow(dead_code)] // Reserved for future batch embedding operations
    pub batch_size: usize,
    /// Whether to cache embeddings
    #[allow(dead_code)] // Reserved for future cache configuration
    pub cache_embeddings: bool,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "gte-small".to_string(),
            dimensions: EMBEDDING_DIM,
            batch_size: 32,
            cache_embeddings: true,
        }
    }
}

/// Initialize the embeddings system
pub fn init(mana_dir: &Path, config: &EmbeddingConfig) -> Result<EmbeddingStore> {
    EmbeddingStore::new(mana_dir, config)
}

/// Check if embeddings are enabled and ready
pub fn is_available(mana_dir: &Path) -> bool {
    let index_path = mana_dir.join("vectors.usearch");
    index_path.exists()
}

/// Get embedding status information
pub fn status(mana_dir: &Path) -> Result<EmbeddingStatus> {
    let store = EmbeddingStore::open(mana_dir)?;
    store.status()
}

/// Embedding system status
#[derive(Debug, Clone)]
pub struct EmbeddingStatus {
    /// Whether the embedding system is initialized
    #[allow(dead_code)] // Available for status checks
    pub initialized: bool,
    /// Model name in use
    pub model_name: String,
    /// Model version
    pub model_version: String,
    /// Number of dimensions
    pub dimensions: usize,
    /// Number of indexed vectors
    pub vector_count: usize,
    /// Number of patterns without embeddings
    pub unembedded_count: usize,
    /// Index size in bytes
    pub index_size_bytes: u64,
}

/// Search for similar patterns using embeddings
pub fn search(mana_dir: &Path, query: &str, k: usize) -> Result<Vec<(i64, f32)>> {
    let store = EmbeddingStore::open(mana_dir)?;
    store.search(query, k)
}

/// Delete a pattern from the vector index
pub fn delete_from_index(mana_dir: &Path, pattern_id: i64) -> Result<bool> {
    let mut store = EmbeddingStore::open(mana_dir)?;
    let removed = store.remove_pattern(pattern_id);
    if removed {
        store.save_index()?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.model, "gte-small");
        assert_eq!(config.dimensions, 384);
        assert_eq!(config.batch_size, 32);
        assert!(config.cache_embeddings);
    }

    #[test]
    fn test_is_available_false_without_index() {
        let temp = TempDir::new().unwrap();
        assert!(!is_available(temp.path()));
    }
}
