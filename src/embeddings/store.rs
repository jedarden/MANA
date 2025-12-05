//! Embedding store for persistence and management
//!
//! Manages the lifecycle of embeddings including:
//! - Generation for new patterns
//! - Persistence to disk
//! - Index building and updating
//! - Status and statistics

use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

use super::{EmbeddingConfig, EmbeddingModel, EmbeddingStatus, VectorIndex};
use super::model::cosine_similarity;

/// Manages embedding storage and retrieval
pub struct EmbeddingStore {
    /// Path to MANA data directory
    mana_dir: PathBuf,
    /// Embedding model
    model: EmbeddingModel,
    /// Vector index
    index: VectorIndex,
    /// Configuration
    config: EmbeddingConfig,
}

impl EmbeddingStore {
    /// Create a new embedding store
    pub fn new(mana_dir: &Path, config: &EmbeddingConfig) -> Result<Self> {
        let model = EmbeddingModel::new(&config.model)?;
        let index = VectorIndex::new(config.dimensions);

        // Initialize SQLite schema
        Self::init_schema(mana_dir)?;

        Ok(Self {
            mana_dir: mana_dir.to_path_buf(),
            model,
            index,
            config: config.clone(),
        })
    }

    /// Open an existing embedding store
    pub fn open(mana_dir: &Path) -> Result<Self> {
        let config = Self::load_config(mana_dir)?;
        let model = EmbeddingModel::new(&config.model)?;

        // Load existing index if available
        let index_path = mana_dir.join("vectors.usearch");
        let index = if index_path.exists() {
            VectorIndex::load(&index_path)?
        } else {
            VectorIndex::new(config.dimensions)
        };

        Ok(Self {
            mana_dir: mana_dir.to_path_buf(),
            model,
            index,
            config,
        })
    }

    /// Initialize the database schema for embeddings
    fn init_schema(mana_dir: &Path) -> Result<()> {
        let db_path = mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        // Add embedding columns if they don't exist
        // Note: SQLite doesn't have IF NOT EXISTS for columns, so we check first
        let has_embedding_col: bool = conn
            .prepare("SELECT embedding FROM patterns LIMIT 1")
            .is_ok();

        if !has_embedding_col {
            conn.execute(
                "ALTER TABLE patterns ADD COLUMN embedding BLOB",
                [],
            ).ok(); // Ignore error if column exists

            conn.execute(
                "ALTER TABLE patterns ADD COLUMN embedding_version INTEGER DEFAULT 0",
                [],
            ).ok();
        }

        // Create embedding metadata table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS embedding_meta (
                id INTEGER PRIMARY KEY,
                model_name TEXT NOT NULL,
                model_version TEXT NOT NULL,
                dimensions INTEGER NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        Ok(())
    }

    /// Load configuration from disk or return defaults
    fn load_config(mana_dir: &Path) -> Result<EmbeddingConfig> {
        let db_path = mana_dir.join("metadata.sqlite");

        if !db_path.exists() {
            return Ok(EmbeddingConfig::default());
        }

        let conn = Connection::open(&db_path)?;

        // Try to load from embedding_meta table
        let result: Result<(String, usize), _> = conn.query_row(
            "SELECT model_name, dimensions FROM embedding_meta ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );

        match result {
            Ok((model, dimensions)) => Ok(EmbeddingConfig {
                model,
                dimensions,
                ..Default::default()
            }),
            Err(_) => Ok(EmbeddingConfig::default()),
        }
    }

    /// Get embedding status
    pub fn status(&self) -> Result<EmbeddingStatus> {
        let db_path = self.mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        // Count patterns without embeddings
        let unembedded: i64 = conn.query_row(
            "SELECT COUNT(*) FROM patterns WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        ).unwrap_or(0);

        let index_path = self.mana_dir.join("vectors.usearch");
        let index_size = if index_path.exists() {
            std::fs::metadata(&index_path)?.len()
        } else {
            0
        };

        Ok(EmbeddingStatus {
            initialized: self.index.len() > 0 || index_path.exists(),
            model_name: self.model.name().to_string(),
            model_version: self.model.version().to_string(),
            dimensions: self.config.dimensions,
            vector_count: self.index.len(),
            unembedded_count: unembedded as usize,
            index_size_bytes: index_size,
        })
    }

    /// Generate embeddings for patterns that don't have them
    pub fn embed_missing(&mut self) -> Result<usize> {
        let db_path = self.mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        // Get patterns without embeddings
        let mut stmt = conn.prepare(
            "SELECT id, context_query FROM patterns WHERE embedding IS NULL LIMIT 1000"
        )?;

        let patterns: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        if patterns.is_empty() {
            return Ok(0);
        }

        let mut count = 0;

        for (id, context_query) in &patterns {
            let embedding = self.model.embed(context_query)?;

            // Store in SQLite
            let embedding_bytes: Vec<u8> = embedding
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect();

            conn.execute(
                "UPDATE patterns SET embedding = ?, embedding_version = 1 WHERE id = ?",
                params![embedding_bytes, id],
            )?;

            // Add to index
            self.index.add(*id, &embedding)?;
            count += 1;
        }

        // Save index
        self.save_index()?;

        Ok(count)
    }

    /// Rebuild all embeddings
    pub fn rebuild(&mut self) -> Result<usize> {
        let db_path = self.mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        // Clear existing embeddings
        conn.execute("UPDATE patterns SET embedding = NULL, embedding_version = 0", [])?;

        // Reset index
        self.index = VectorIndex::new(self.config.dimensions);

        // Re-embed all
        self.embed_missing()
    }

    /// Search for similar patterns using vector similarity
    pub fn search(&self, query: &str, k: usize) -> Result<Vec<(i64, f32)>> {
        let query_embedding = self.model.embed(query)?;
        let matches = self.index.search(&query_embedding, k);

        Ok(matches.into_iter().map(|m| (m.id, m.similarity)).collect())
    }

    /// Search with combined vector and pattern info
    pub fn search_with_context(
        &self,
        query: &str,
        k: usize,
    ) -> Result<Vec<PatternMatch>> {
        let db_path = self.mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        let query_embedding = self.model.embed(query)?;
        let matches = self.index.search(&query_embedding, k * 2); // Get more for filtering

        let mut results = Vec::new();

        for m in matches {
            let pattern: Option<(String, String, i64, i64)> = conn
                .query_row(
                    "SELECT tool_type, context_query, success_count, failure_count
                     FROM patterns WHERE id = ?",
                    params![m.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .ok();

            if let Some((tool_type, context_query, success, failure)) = pattern {
                results.push(PatternMatch {
                    id: m.id,
                    similarity: m.similarity,
                    tool_type,
                    context_query,
                    success_count: success,
                    failure_count: failure,
                });
            }

            if results.len() >= k {
                break;
            }
        }

        Ok(results)
    }

    /// Add embedding for a new pattern
    pub fn add_pattern(&mut self, pattern_id: i64, context_query: &str) -> Result<()> {
        let embedding = self.model.embed(context_query)?;

        let db_path = self.mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        let embedding_bytes: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();

        conn.execute(
            "UPDATE patterns SET embedding = ?, embedding_version = 1 WHERE id = ?",
            params![embedding_bytes, pattern_id],
        )?;

        self.index.add(pattern_id, &embedding)?;
        Ok(())
    }

    /// Remove pattern from index
    pub fn remove_pattern(&mut self, pattern_id: i64) -> bool {
        self.index.remove(pattern_id)
    }

    /// Save the index to disk
    pub fn save_index(&self) -> Result<()> {
        let index_path = self.mana_dir.join("vectors.usearch");
        self.index.save(&index_path)?;

        // Update metadata
        let db_path = self.mana_dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        conn.execute(
            "INSERT OR REPLACE INTO embedding_meta (id, model_name, model_version, dimensions)
             VALUES (1, ?, ?, ?)",
            params![
                self.model.name(),
                self.model.version(),
                self.config.dimensions as i64
            ],
        )?;

        Ok(())
    }

    /// Load the index from disk
    pub fn load_index(&mut self) -> Result<()> {
        let index_path = self.mana_dir.join("vectors.usearch");
        if index_path.exists() {
            self.index = VectorIndex::load(&index_path)?;
        }
        Ok(())
    }

    /// Get the model
    pub fn model(&self) -> &EmbeddingModel {
        &self.model
    }

    /// Get the index
    pub fn index(&self) -> &VectorIndex {
        &self.index
    }

    /// Compute similarity between two texts
    pub fn similarity(&self, text1: &str, text2: &str) -> Result<f32> {
        let emb1 = self.model.embed(text1)?;
        let emb2 = self.model.embed(text2)?;
        Ok(cosine_similarity(&emb1, &emb2))
    }
}

/// A pattern match with full context
#[derive(Debug, Clone)]
pub struct PatternMatch {
    /// Pattern ID
    pub id: i64,
    /// Vector similarity score
    pub similarity: f32,
    /// Tool type
    pub tool_type: String,
    /// Context query
    pub context_query: String,
    /// Success count
    pub success_count: i64,
    /// Failure count
    pub failure_count: i64,
}

impl PatternMatch {
    /// Calculate success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.success_count + self.failure_count;
        if total == 0 {
            0.0
        } else {
            self.success_count as f64 / total as f64
        }
    }

    /// Calculate combined score (similarity * success_rate)
    pub fn combined_score(&self) -> f32 {
        self.similarity * self.success_rate() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_db(dir: &Path) -> Result<()> {
        let db_path = dir.join("metadata.sqlite");
        let conn = Connection::open(&db_path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS patterns (
                id INTEGER PRIMARY KEY,
                tool_type TEXT NOT NULL,
                context_query TEXT NOT NULL,
                success_count INTEGER DEFAULT 0,
                failure_count INTEGER DEFAULT 0,
                embedding BLOB,
                embedding_version INTEGER DEFAULT 0
            )",
            [],
        )?;

        // Insert test patterns
        conn.execute(
            "INSERT INTO patterns (tool_type, context_query, success_count)
             VALUES ('Bash', 'running npm install', 5)",
            [],
        )?;
        conn.execute(
            "INSERT INTO patterns (tool_type, context_query, success_count)
             VALUES ('Bash', 'running cargo build', 3)",
            [],
        )?;
        conn.execute(
            "INSERT INTO patterns (tool_type, context_query, success_count)
             VALUES ('Edit', 'editing main.rs', 2)",
            [],
        )?;

        Ok(())
    }

    #[test]
    fn test_embed_missing() {
        let temp = TempDir::new().unwrap();
        setup_test_db(temp.path()).unwrap();

        let config = EmbeddingConfig::default();
        let mut store = EmbeddingStore::new(temp.path(), &config).unwrap();

        let count = store.embed_missing().unwrap();
        assert_eq!(count, 3);

        // Check status
        let status = store.status().unwrap();
        assert_eq!(status.vector_count, 3);
        assert_eq!(status.unembedded_count, 0);
    }

    #[test]
    fn test_search() {
        let temp = TempDir::new().unwrap();
        setup_test_db(temp.path()).unwrap();

        let config = EmbeddingConfig::default();
        let mut store = EmbeddingStore::new(temp.path(), &config).unwrap();
        store.embed_missing().unwrap();

        let results = store.search("npm install packages", 2).unwrap();
        assert_eq!(results.len(), 2);

        // First result should be the npm pattern (most similar)
        // But we can't guarantee order without checking context
    }

    #[test]
    fn test_similarity() {
        let temp = TempDir::new().unwrap();
        setup_test_db(temp.path()).unwrap();

        let config = EmbeddingConfig::default();
        let store = EmbeddingStore::new(temp.path(), &config).unwrap();

        let sim1 = store.similarity("npm install", "npm install packages").unwrap();
        let sim2 = store.similarity("npm install", "cargo build").unwrap();

        // npm install should be more similar to npm install packages
        assert!(sim1 > sim2, "sim1={} should be > sim2={}", sim1, sim2);
    }
}
