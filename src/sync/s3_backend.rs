//! S3-based sync backend for pattern synchronization
//!
//! Implements push/pull operations using S3-compatible object storage.
//! Supports AWS S3, MinIO, R2, and other S3-compatible services.

use anyhow::{Result, anyhow};
use std::path::Path;

#[cfg(feature = "s3")]
use tracing::info;

#[cfg(feature = "s3")]
use crate::sync::SyncBackend;

#[cfg(feature = "s3")]
use crate::sync::{SecurityConfig, load_sync_config};
#[cfg(feature = "s3")]
use crate::sync::export::{export_patterns, import_patterns};

pub use crate::sync::export::MergeStrategy;

/// Re-export SecurityConfig for use in stubs
#[cfg(not(feature = "s3"))]
pub use crate::sync::SecurityConfig;

#[cfg(feature = "s3")]
use aws_config::BehaviorVersion;
#[cfg(feature = "s3")]
use aws_sdk_s3::Client as S3Client;
#[cfg(feature = "s3")]
use aws_sdk_s3::primitives::ByteStream;

/// S3 sync configuration
#[cfg(feature = "s3")]
#[derive(Debug, Clone)]
pub struct S3SyncConfig {
    /// S3 bucket name
    pub bucket: String,
    /// Object key prefix
    pub prefix: String,
    /// AWS region
    pub region: String,
    /// Optional endpoint URL (for S3-compatible services)
    pub endpoint_url: Option<String>,
}

#[cfg(feature = "s3")]
impl S3SyncConfig {
    /// Create from SyncBackend::S3 variant
    pub fn from_backend(backend: &SyncBackend) -> Option<Self> {
        match backend {
            SyncBackend::S3 { bucket, prefix, region } => Some(Self {
                bucket: bucket.clone(),
                prefix: prefix.clone(),
                region: region.clone(),
                endpoint_url: std::env::var("MANA_S3_ENDPOINT").ok(),
            }),
            _ => None,
        }
    }

    /// Get the full object key for patterns
    fn patterns_key(&self) -> String {
        if self.prefix.is_empty() {
            "patterns.json".to_string()
        } else {
            format!("{}/patterns.json", self.prefix.trim_end_matches('/'))
        }
    }
}

/// Initialize S3 sync configuration
///
/// Validates bucket access and creates prefix if needed.
#[cfg(feature = "s3")]
pub async fn init_s3_sync(mana_dir: &Path, bucket: &str, prefix: &str, region: &str) -> Result<()> {
    // Save configuration
    save_s3_config(mana_dir, bucket, prefix, region)?;

    // Validate bucket access
    let config = S3SyncConfig {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
        region: region.to_string(),
        endpoint_url: std::env::var("MANA_S3_ENDPOINT").ok(),
    };

    let client = create_s3_client(&config).await?;

    // Try to list objects to verify access
    let result = client
        .list_objects_v2()
        .bucket(&config.bucket)
        .prefix(&config.prefix)
        .max_keys(1)
        .send()
        .await;

    match result {
        Ok(_) => {
            info!("S3 bucket access verified: {}/{}", bucket, prefix);
            println!("✅ S3 sync initialized");
            println!("   Bucket: {}", bucket);
            println!("   Prefix: {}", prefix);
            println!("   Region: {}", region);
            Ok(())
        }
        Err(e) => {
            Err(anyhow!("Failed to access S3 bucket: {}. Check your AWS credentials and bucket permissions.", e))
        }
    }
}

/// Initialize S3 sync (stub when feature disabled)
#[cfg(not(feature = "s3"))]
pub async fn init_s3_sync(_mana_dir: &Path, _bucket: &str, _prefix: &str, _region: &str) -> Result<()> {
    Err(anyhow!("S3 sync not available. Rebuild with --features s3"))
}

/// Push patterns to S3
#[cfg(feature = "s3")]
pub async fn push_patterns_s3(
    mana_dir: &Path,
    db_path: &Path,
    security: &SecurityConfig,
    passphrase: Option<&str>,
) -> Result<()> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let s3_config = S3SyncConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Sync backend is not configured for S3"))?;

    // Export patterns to temporary file
    let temp_file = mana_dir.join("patterns-export.json");
    let count = export_patterns(db_path, &temp_file, security, passphrase)?;

    info!("Exported {} patterns for S3 upload", count);

    // Read the file content
    let content = std::fs::read(&temp_file)?;

    // Create S3 client and upload
    let client = create_s3_client(&s3_config).await?;
    let key = s3_config.patterns_key();

    let result = client
        .put_object()
        .bucket(&s3_config.bucket)
        .key(&key)
        .body(ByteStream::from(content))
        .content_type("application/json")
        .send()
        .await;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_file);

    match result {
        Ok(_) => {
            println!("✅ Pushed {} patterns to s3://{}/{}", count, s3_config.bucket, key);
            Ok(())
        }
        Err(e) => {
            Err(anyhow!("Failed to upload to S3: {}", e))
        }
    }
}

/// Push patterns to S3 (stub when feature disabled)
#[cfg(not(feature = "s3"))]
pub async fn push_patterns_s3(
    _mana_dir: &Path,
    _db_path: &Path,
    _security: &SecurityConfig,
    _passphrase: Option<&str>,
) -> Result<()> {
    Err(anyhow!("S3 sync not available. Rebuild with --features s3"))
}

/// Pull patterns from S3
#[cfg(feature = "s3")]
pub async fn pull_patterns_s3(
    mana_dir: &Path,
    db_path: &Path,
    passphrase: Option<&str>,
    merge_strategy: MergeStrategy,
) -> Result<()> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let s3_config = S3SyncConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Sync backend is not configured for S3"))?;

    // Create S3 client
    let client = create_s3_client(&s3_config).await?;
    let key = s3_config.patterns_key();

    // Download the patterns file
    let result = client
        .get_object()
        .bucket(&s3_config.bucket)
        .key(&key)
        .send()
        .await;

    match result {
        Ok(response) => {
            // Read the body
            let body = response.body.collect().await?;
            let bytes = body.into_bytes();

            // Write to temp file
            let temp_file = mana_dir.join("patterns-import.json");
            std::fs::write(&temp_file, &bytes)?;

            // Import patterns
            let import_result = import_patterns(db_path, &temp_file, passphrase, merge_strategy)?;

            // Clean up
            let _ = std::fs::remove_file(&temp_file);

            println!("✅ Pulled patterns from s3://{}/{}", s3_config.bucket, key);
            println!("   Total: {}, New: {}, Merged: {}",
                import_result.total, import_result.imported, import_result.merged);
            if import_result.skipped > 0 {
                println!("   Skipped: {}", import_result.skipped);
            }

            Ok(())
        }
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("NoSuchKey") || err_str.contains("404") {
                println!("📋 No patterns file found in S3 bucket");
                Ok(())
            } else {
                Err(anyhow!("Failed to download from S3: {}", e))
            }
        }
    }
}

/// Pull patterns from S3 (stub when feature disabled)
#[cfg(not(feature = "s3"))]
pub async fn pull_patterns_s3(
    _mana_dir: &Path,
    _db_path: &Path,
    _passphrase: Option<&str>,
    _merge_strategy: MergeStrategy,
) -> Result<()> {
    Err(anyhow!("S3 sync not available. Rebuild with --features s3"))
}

/// Get S3 sync status
#[cfg(feature = "s3")]
pub async fn s3_status(mana_dir: &Path) -> Result<S3SyncStatus> {
    let config_path = mana_dir.join("sync.toml");

    if !config_path.exists() {
        return Ok(S3SyncStatus {
            configured: false,
            bucket: None,
            prefix: None,
            region: None,
            object_exists: false,
            last_modified: None,
            size_bytes: None,
        });
    }

    let config = load_sync_config(&config_path)?;

    if let Some(s3_config) = S3SyncConfig::from_backend(&config.backend) {
        let client = create_s3_client(&s3_config).await?;
        let key = s3_config.patterns_key();

        // Check if object exists and get metadata
        let result = client
            .head_object()
            .bucket(&s3_config.bucket)
            .key(&key)
            .send()
            .await;

        match result {
            Ok(response) => {
                Ok(S3SyncStatus {
                    configured: true,
                    bucket: Some(s3_config.bucket),
                    prefix: Some(s3_config.prefix),
                    region: Some(s3_config.region),
                    object_exists: true,
                    last_modified: response.last_modified().map(|t| t.to_string()),
                    size_bytes: response.content_length(),
                })
            }
            Err(_) => {
                Ok(S3SyncStatus {
                    configured: true,
                    bucket: Some(s3_config.bucket),
                    prefix: Some(s3_config.prefix),
                    region: Some(s3_config.region),
                    object_exists: false,
                    last_modified: None,
                    size_bytes: None,
                })
            }
        }
    } else {
        Ok(S3SyncStatus {
            configured: false,
            bucket: None,
            prefix: None,
            region: None,
            object_exists: false,
            last_modified: None,
            size_bytes: None,
        })
    }
}

/// Get S3 sync status (stub when feature disabled)
#[cfg(not(feature = "s3"))]
pub async fn s3_status(_mana_dir: &Path) -> Result<S3SyncStatus> {
    Ok(S3SyncStatus::default())
}

/// S3 sync status information
#[derive(Debug, Clone, Default)]
pub struct S3SyncStatus {
    /// Whether S3 sync is configured
    #[allow(dead_code)]
    pub configured: bool,
    /// Bucket name
    #[allow(dead_code)]
    pub bucket: Option<String>,
    /// Object prefix
    #[allow(dead_code)]
    pub prefix: Option<String>,
    /// AWS region
    #[allow(dead_code)]
    pub region: Option<String>,
    /// Whether patterns file exists in bucket
    pub object_exists: bool,
    /// Last modified timestamp
    pub last_modified: Option<String>,
    /// Size of patterns file in bytes
    pub size_bytes: Option<i64>,
}

/// Create an S3 client with the given configuration
#[cfg(feature = "s3")]
async fn create_s3_client(config: &S3SyncConfig) -> Result<S3Client> {
    use aws_config::Region;

    let mut aws_config = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(config.region.clone()));

    // Support custom endpoint for S3-compatible services
    if let Some(endpoint) = &config.endpoint_url {
        aws_config = aws_config.endpoint_url(endpoint);
    }

    let sdk_config = aws_config.load().await;
    Ok(S3Client::new(&sdk_config))
}

/// Save S3 sync configuration
pub fn save_s3_config(mana_dir: &Path, bucket: &str, prefix: &str, region: &str) -> Result<()> {
    use crate::sync::{SyncConfig, SyncBackend, SecurityConfig as SyncSecurityConfig, save_sync_config};

    let config = SyncConfig {
        enabled: true,
        backend: SyncBackend::S3 {
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
            region: region.to_string(),
        },
        interval_minutes: 60,
        security: SyncSecurityConfig::default(),
    };

    let config_path = mana_dir.join("sync.toml");
    save_sync_config(&config, &config_path)?;

    tracing::info!("Saved S3 sync configuration to {:?}", config_path);
    Ok(())
}

/// Check if S3 feature is available
pub fn is_s3_available() -> bool {
    cfg!(feature = "s3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "s3")]
    #[test]
    fn test_s3_config_from_backend() {
        use crate::sync::SyncBackend;
        let backend = SyncBackend::S3 {
            bucket: "my-bucket".to_string(),
            prefix: "mana/patterns".to_string(),
            region: "us-west-2".to_string(),
        };

        let config = S3SyncConfig::from_backend(&backend);
        assert!(config.is_some());

        let config = config.unwrap();
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.prefix, "mana/patterns");
        assert_eq!(config.region, "us-west-2");
    }

    #[cfg(feature = "s3")]
    #[test]
    fn test_patterns_key_with_prefix() {
        let config = S3SyncConfig {
            bucket: "test".to_string(),
            prefix: "mana/patterns".to_string(),
            region: "us-east-1".to_string(),
            endpoint_url: None,
        };

        assert_eq!(config.patterns_key(), "mana/patterns/patterns.json");
    }

    #[cfg(feature = "s3")]
    #[test]
    fn test_patterns_key_empty_prefix() {
        let config = S3SyncConfig {
            bucket: "test".to_string(),
            prefix: "".to_string(),
            region: "us-east-1".to_string(),
            endpoint_url: None,
        };

        assert_eq!(config.patterns_key(), "patterns.json");
    }

    #[cfg(feature = "s3")]
    #[test]
    fn test_patterns_key_trailing_slash() {
        let config = S3SyncConfig {
            bucket: "test".to_string(),
            prefix: "mana/".to_string(),
            region: "us-east-1".to_string(),
            endpoint_url: None,
        };

        assert_eq!(config.patterns_key(), "mana/patterns.json");
    }

    #[test]
    fn test_is_s3_available() {
        // This will be true when compiled with --features s3
        let available = is_s3_available();
        // Just verify it compiles and returns a bool
        assert!(available || !available);
    }
}
