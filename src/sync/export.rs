//! Pattern export and import functionality
//!
//! Exports patterns to JSON format with optional encryption.
//! Supports importing and merging patterns from other workspaces.

use anyhow::{Result, anyhow};
use chrono::Utc;
use std::path::Path;
use tracing::info;

use crate::storage::{Pattern, PatternStore};
use crate::sync::{
    ExportBundle, ExportMetadata, ExportablePattern, SecurityConfig,
    crypto::{encrypt_string, decrypt_string, hash_workspace_id, EncryptedData},
    sanitize::sanitize_pattern,
};

/// Export format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Plain JSON (readable, but not secure for sharing)
    Json,
    /// Encrypted JSON (secure for sharing)
    EncryptedJson,
}

/// Export patterns to a file
///
/// Applies sanitization based on security config and optionally encrypts.
pub fn export_patterns(
    db_path: &Path,
    output_path: &Path,
    security: &SecurityConfig,
    passphrase: Option<&str>,
) -> Result<usize> {
    let store = PatternStore::open_readonly(db_path)?;

    // Get all patterns
    let patterns = get_all_patterns(&store)?;
    let pattern_count = patterns.len();

    if pattern_count == 0 {
        return Err(anyhow!("No patterns to export"));
    }

    // Sanitize patterns
    let sanitized: Vec<ExportablePattern> = patterns
        .iter()
        .map(|p| {
            if security.sanitize_paths || security.redact_secrets {
                sanitize_pattern(p)
            } else {
                // No sanitization, just convert to exportable format
                ExportablePattern {
                    pattern_hash: p.pattern_hash.clone(),
                    tool_type: p.tool_type.clone(),
                    command_category: p.command_category.clone(),
                    context_query: p.context_query.clone(),
                    success_count: p.success_count,
                    failure_count: p.failure_count,
                }
            }
        })
        .collect();

    // Create export bundle
    let workspace_id = hash_workspace_id(&std::env::current_dir()?.to_string_lossy());
    let encrypted = security.encrypt && passphrase.is_some();

    let bundle = ExportBundle {
        metadata: ExportMetadata {
            version: "1.0".to_string(),
            exported_at: Utc::now().to_rfc3339(),
            source_workspace: workspace_id,
            pattern_count,
            encrypted,
        },
        patterns: sanitized,
    };

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&bundle)?;

    // Encrypt if requested
    if encrypted {
        let passphrase = passphrase.ok_or_else(|| anyhow!("Passphrase required for encryption"))?;
        let encrypted_data = encrypt_string(&json, passphrase)?;
        let encrypted_json = serde_json::to_string_pretty(&encrypted_data)?;
        std::fs::write(output_path, encrypted_json)?;
        info!("Exported {} patterns (encrypted) to {:?}", pattern_count, output_path);
    } else {
        std::fs::write(output_path, json)?;
        info!("Exported {} patterns to {:?}", pattern_count, output_path);
    }

    Ok(pattern_count)
}

/// Import patterns from a file
///
/// Supports both plain JSON and encrypted JSON formats.
/// Merges imported patterns with existing ones.
pub fn import_patterns(
    db_path: &Path,
    input_path: &Path,
    passphrase: Option<&str>,
    merge_strategy: MergeStrategy,
) -> Result<ImportResult> {
    let content = std::fs::read_to_string(input_path)?;

    // Try to parse as encrypted data first
    let bundle: ExportBundle = if let Ok(encrypted) = serde_json::from_str::<EncryptedData>(&content) {
        let passphrase = passphrase.ok_or_else(|| anyhow!("Passphrase required to decrypt import file"))?;
        let decrypted = decrypt_string(&encrypted, passphrase)?;
        serde_json::from_str(&decrypted)?
    } else {
        // Try plain JSON
        serde_json::from_str(&content)?
    };

    info!("Importing {} patterns from {} (exported at {})",
        bundle.patterns.len(),
        bundle.metadata.source_workspace,
        bundle.metadata.exported_at
    );

    // Open store for writing
    let store = PatternStore::open(db_path)?;

    // Import patterns
    let mut imported = 0;
    let mut merged = 0;
    let mut skipped = 0;

    for exportable in &bundle.patterns {
        let pattern = Pattern {
            id: 0, // Will be assigned by database
            pattern_hash: exportable.pattern_hash.clone(),
            tool_type: exportable.tool_type.clone(),
            command_category: exportable.command_category.clone(),
            context_query: exportable.context_query.clone(),
            success_count: exportable.success_count,
            failure_count: exportable.failure_count,
            embedding_id: None,
        };

        match merge_strategy {
            MergeStrategy::Add => {
                // Use insert_fast which handles duplicates via hash
                let id = store.insert_fast(&pattern)?;
                if id > 0 {
                    imported += 1;
                } else {
                    merged += 1;
                }
            }
            MergeStrategy::Replace => {
                // Force insert, replacing existing
                store.insert_fast(&pattern)?;
                imported += 1;
            }
            MergeStrategy::KeepBest => {
                // Only import if better success rate
                if let Some(existing) = find_by_hash(&store, &pattern.pattern_hash)? {
                    let existing_rate = success_rate(&existing);
                    let new_rate = success_rate(&pattern);
                    if new_rate > existing_rate {
                        store.insert_fast(&pattern)?;
                        imported += 1;
                    } else {
                        skipped += 1;
                    }
                } else {
                    store.insert_fast(&pattern)?;
                    imported += 1;
                }
            }
        }
    }

    Ok(ImportResult {
        total: bundle.patterns.len(),
        imported,
        merged,
        skipped,
        source_workspace: bundle.metadata.source_workspace,
    })
}

/// Strategy for handling duplicate patterns during import
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeStrategy {
    /// Add new patterns, merge counts for existing (default)
    #[default]
    Add,
    /// Replace existing patterns with imported ones
    Replace,
    /// Keep whichever has better success rate
    KeepBest,
}

/// Result of import operation
#[derive(Debug, Clone)]
pub struct ImportResult {
    /// Total patterns in import file
    pub total: usize,
    /// Patterns imported as new
    pub imported: usize,
    /// Patterns merged with existing
    pub merged: usize,
    /// Patterns skipped (KeepBest strategy)
    pub skipped: usize,
    /// Source workspace identifier
    pub source_workspace: String,
}

/// Export patterns to a vector (for API-based backends like Supabase)
pub fn export_patterns_to_vec(
    db_path: &Path,
    security: &SecurityConfig,
) -> Result<Vec<ExportablePattern>> {
    let store = PatternStore::open_readonly(db_path)?;
    let patterns = get_all_patterns(&store)?;

    let sanitized: Vec<ExportablePattern> = patterns
        .iter()
        .map(|p| {
            if security.sanitize_paths || security.redact_secrets {
                sanitize_pattern(p)
            } else {
                ExportablePattern {
                    pattern_hash: p.pattern_hash.clone(),
                    tool_type: p.tool_type.clone(),
                    command_category: p.command_category.clone(),
                    context_query: p.context_query.clone(),
                    success_count: p.success_count,
                    failure_count: p.failure_count,
                }
            }
        })
        .collect();

    Ok(sanitized)
}

/// Import patterns from a vector (for API-based backends like Supabase)
pub fn import_patterns_from_vec(
    db_path: &Path,
    patterns: Vec<ExportablePattern>,
    merge_strategy: MergeStrategy,
) -> Result<ImportResult> {
    let store = PatternStore::open(db_path)?;

    let mut imported = 0;
    let mut merged = 0;
    let mut skipped = 0;

    for exportable in &patterns {
        let pattern = Pattern {
            id: 0,
            pattern_hash: exportable.pattern_hash.clone(),
            tool_type: exportable.tool_type.clone(),
            command_category: exportable.command_category.clone(),
            context_query: exportable.context_query.clone(),
            success_count: exportable.success_count,
            failure_count: exportable.failure_count,
            embedding_id: None,
        };

        match merge_strategy {
            MergeStrategy::Add => {
                let id = store.insert_fast(&pattern)?;
                if id > 0 {
                    imported += 1;
                } else {
                    merged += 1;
                }
            }
            MergeStrategy::Replace => {
                store.insert_fast(&pattern)?;
                imported += 1;
            }
            MergeStrategy::KeepBest => {
                if let Some(existing) = find_by_hash(&store, &pattern.pattern_hash)? {
                    let existing_rate = success_rate(&existing);
                    let new_rate = success_rate(&pattern);
                    if new_rate > existing_rate {
                        store.insert_fast(&pattern)?;
                        imported += 1;
                    } else {
                        skipped += 1;
                    }
                } else {
                    store.insert_fast(&pattern)?;
                    imported += 1;
                }
            }
        }
    }

    Ok(ImportResult {
        total: patterns.len(),
        imported,
        merged,
        skipped,
        source_workspace: "api".to_string(),
    })
}

/// Get all patterns from database
fn get_all_patterns(store: &PatternStore) -> Result<Vec<Pattern>> {
    // Get patterns of all known types
    let tool_types = ["Bash", "Edit", "Write", "Read", "Task", "Glob", "Grep", "WebSearch", "failure"];
    let mut all_patterns = Vec::new();

    for tool_type in tool_types {
        let patterns = store.get_by_tool(tool_type, 1000)?;
        all_patterns.extend(patterns);
    }

    // Also get top patterns to catch any we missed
    let top = store.get_top_patterns(1000)?;
    for p in top {
        if !all_patterns.iter().any(|existing| existing.id == p.id) {
            all_patterns.push(p);
        }
    }

    Ok(all_patterns)
}

/// Find pattern by hash
fn find_by_hash(store: &PatternStore, hash: &str) -> Result<Option<Pattern>> {
    // We don't have a direct lookup by hash in PatternStore, so use top patterns
    // This is O(n) but acceptable for import operations
    let patterns = store.get_top_patterns(10000)?;
    Ok(patterns.into_iter().find(|p| p.pattern_hash == hash))
}

/// Calculate success rate for a pattern
fn success_rate(pattern: &Pattern) -> f64 {
    let total = pattern.success_count + pattern.failure_count;
    if total == 0 {
        return 0.5; // Default for new patterns
    }
    pattern.success_count as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use rusqlite::Connection;
    #[allow(unused_imports)]
    use crate::storage::init as init_storage;

    fn setup_test_db() -> (TempDir, std::path::PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("metadata.sqlite");

        // Initialize database
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            std::env::set_current_dir(temp_dir.path()).unwrap();
            init_storage().await.unwrap();
        });

        (temp_dir, db_path)
    }

    #[test]
    fn test_export_empty_db() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("metadata.sqlite");

        // Create the database directly (without init which changes cwd)
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(r#"
            CREATE TABLE patterns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                pattern_hash TEXT UNIQUE NOT NULL,
                tool_type TEXT NOT NULL,
                command_category TEXT,
                context_query TEXT NOT NULL,
                success_count INTEGER DEFAULT 0,
                failure_count INTEGER DEFAULT 0,
                last_used DATETIME,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                embedding_id INTEGER
            );
        "#).unwrap();
        drop(conn);

        let output_path = temp_dir.path().join("export.json");
        let security = SecurityConfig::default();

        let result = export_patterns(&db_path, &output_path, &security, None);
        // Should error since no patterns exist
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No patterns"), "Unexpected error: {}", err_msg);
    }

    #[test]
    fn test_merge_strategy_default() {
        assert_eq!(MergeStrategy::default(), MergeStrategy::Add);
    }

    #[test]
    fn test_success_rate_calculation() {
        let pattern = Pattern {
            id: 1,
            pattern_hash: "test".to_string(),
            tool_type: "Bash".to_string(),
            command_category: None,
            context_query: "test".to_string(),
            success_count: 8,
            failure_count: 2,
            embedding_id: None,
        };

        let rate = success_rate(&pattern);
        assert!((rate - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_success_rate_zero_uses() {
        let pattern = Pattern {
            id: 1,
            pattern_hash: "test".to_string(),
            tool_type: "Bash".to_string(),
            command_category: None,
            context_query: "test".to_string(),
            success_count: 0,
            failure_count: 0,
            embedding_id: None,
        };

        let rate = success_rate(&pattern);
        assert!((rate - 0.5).abs() < 0.01);
    }
}
