//! Sync module for multi-workspace pattern synchronization
//!
//! Enables pattern sharing across devpods, workspaces, and machines
//! with security features including sanitization and encryption.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod sanitize;
pub mod export;
pub mod crypto;
pub mod git_backend;

pub use export::{export_patterns, import_patterns};
pub use git_backend::{init_git_sync, push_patterns, pull_patterns, sync_status, save_git_config};

/// Configuration for sync operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Whether sync is enabled
    pub enabled: bool,
    /// Backend type: git, s3, or supabase
    pub backend: SyncBackend,
    /// Sync interval in minutes (for daemon mode)
    pub interval_minutes: u32,
    /// Security settings
    pub security: SecurityConfig,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: SyncBackend::Git {
                remote: String::new(),
                branch: "main".to_string()
            },
            interval_minutes: 60,
            security: SecurityConfig::default(),
        }
    }
}

/// Supported sync backends
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SyncBackend {
    /// Git-based sync (simplest, works offline)
    Git {
        remote: String,
        branch: String,
    },
    /// S3/object storage (scalable)
    S3 {
        bucket: String,
        prefix: String,
        region: String,
    },
    /// Supabase/PostgreSQL (team features, real-time)
    Supabase {
        url: String,
        // Key stored in MANA_SUPABASE_KEY env var
    },
}

/// Security configuration for pattern sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Strip absolute paths to relative
    pub sanitize_paths: bool,
    /// Redact secrets/tokens (regex detection)
    pub redact_secrets: bool,
    /// Encrypt patterns before export
    pub encrypt: bool,
    /// Pattern visibility: private, team, public
    pub visibility: Visibility,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            sanitize_paths: true,
            redact_secrets: true,
            encrypt: true,
            visibility: Visibility::Private,
        }
    }
}

/// Pattern visibility level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Team,
    Public,
}

/// Exportable pattern format (sanitized for sharing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportablePattern {
    /// Hash for deduplication (recalculated from sanitized content)
    pub pattern_hash: String,
    /// Tool type (Bash, Edit, Write, etc.)
    pub tool_type: String,
    /// Command category (cargo, npm, git, etc.)
    pub command_category: Option<String>,
    /// Sanitized context query (paths stripped, secrets redacted)
    pub context_query: String,
    /// Success count
    pub success_count: i64,
    /// Failure count
    pub failure_count: i64,
}

/// Export metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMetadata {
    /// Export format version
    pub version: String,
    /// Export timestamp
    pub exported_at: String,
    /// Source workspace identifier (hashed)
    pub source_workspace: String,
    /// Number of patterns
    pub pattern_count: usize,
    /// Whether data is encrypted
    pub encrypted: bool,
}

/// Complete export bundle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportBundle {
    /// Metadata about the export
    pub metadata: ExportMetadata,
    /// Exported patterns
    pub patterns: Vec<ExportablePattern>,
}

/// Load sync configuration from file
pub fn load_sync_config(config_path: &Path) -> Result<SyncConfig> {
    if !config_path.exists() {
        return Ok(SyncConfig::default());
    }

    let content = std::fs::read_to_string(config_path)?;
    let config: SyncConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save sync configuration to file
pub fn save_sync_config(config: &SyncConfig, config_path: &Path) -> Result<()> {
    let content = toml::to_string_pretty(config)?;
    std::fs::write(config_path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SyncConfig::default();
        assert!(!config.enabled);
        assert!(config.security.sanitize_paths);
        assert!(config.security.redact_secrets);
        assert!(config.security.encrypt);
    }

    #[test]
    fn test_visibility_serde() {
        let json = serde_json::to_string(&Visibility::Private).unwrap();
        assert_eq!(json, r#""private""#);

        let vis: Visibility = serde_json::from_str(r#""team""#).unwrap();
        assert_eq!(vis, Visibility::Team);
    }
}
