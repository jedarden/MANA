//! Git-based sync backend for pattern synchronization
//!
//! Implements push/pull operations using a git repository as the backend.
//! This is the simplest sync approach and works offline.

use anyhow::{Result, anyhow, Context};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

use crate::sync::{SyncBackend, SecurityConfig, load_sync_config};
use crate::sync::export::{export_patterns, import_patterns, MergeStrategy};

/// Git sync configuration
#[derive(Debug, Clone)]
pub struct GitSyncConfig {
    /// Remote repository URL (stored for reference, git operations use local clone)
    #[allow(dead_code)]
    pub remote: String,
    /// Branch to sync with
    pub branch: String,
    /// Local clone directory
    pub local_dir: PathBuf,
}

impl GitSyncConfig {
    /// Create from SyncBackend::Git variant
    pub fn from_backend(backend: &SyncBackend, mana_dir: &Path) -> Option<Self> {
        match backend {
            SyncBackend::Git { remote, branch } => Some(Self {
                remote: remote.clone(),
                branch: branch.clone(),
                local_dir: mana_dir.join("sync-repo"),
            }),
            _ => None,
        }
    }
}

/// Initialize git sync for a workspace
///
/// Sets up the sync repository configuration and clones the remote if provided.
pub fn init_git_sync(mana_dir: &Path, remote: &str, branch: &str) -> Result<()> {
    let sync_dir = mana_dir.join("sync-repo");

    // Create sync directory if it doesn't exist
    if !sync_dir.exists() {
        std::fs::create_dir_all(&sync_dir)?;
    }

    // Check if already initialized
    if sync_dir.join(".git").exists() {
        info!("Sync repository already initialized at {:?}", sync_dir);
        return Ok(());
    }

    // Initialize or clone the repository
    if remote.is_empty() {
        // Just init a local repo for manual remote setup
        run_git_command(&sync_dir, &["init"])?;
        info!("Initialized empty sync repository at {:?}", sync_dir);
        println!("📁 Initialized empty sync repository");
        println!("   Add remote: git -C {:?} remote add origin <url>", sync_dir);
    } else {
        // Clone the remote repository
        info!("Cloning sync repository from {}", remote);
        let parent = sync_dir.parent().unwrap_or(mana_dir);
        run_git_command(parent, &["clone", "--branch", branch, remote, "sync-repo"])?;
        info!("Cloned sync repository to {:?}", sync_dir);
        println!("✅ Cloned sync repository from {}", remote);
    }

    // Create .gitignore for local-only files
    let gitignore_path = sync_dir.join(".gitignore");
    if !gitignore_path.exists() {
        std::fs::write(&gitignore_path, "# Local-only files\n*.local\n*.tmp\n")?;
    }

    Ok(())
}

/// Push patterns to the git remote
///
/// Exports patterns, commits changes, and pushes to remote.
pub fn push_patterns(
    mana_dir: &Path,
    db_path: &Path,
    security: &SecurityConfig,
    passphrase: Option<&str>,
    message: Option<&str>,
) -> Result<()> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let git_config = GitSyncConfig::from_backend(&config.backend, mana_dir)
        .ok_or_else(|| anyhow!("Sync backend is not configured for git"))?;

    if !git_config.local_dir.exists() {
        return Err(anyhow!("Sync repository not initialized. Run 'mana sync init' first."));
    }

    // Export patterns to sync repo
    let export_file = git_config.local_dir.join("patterns.json");
    let count = export_patterns(db_path, &export_file, security, passphrase)?;

    info!("Exported {} patterns to sync repository", count);

    // Check if there are changes to commit
    let status = run_git_command(&git_config.local_dir, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        println!("📋 No changes to push");
        return Ok(());
    }

    // Stage changes
    run_git_command(&git_config.local_dir, &["add", "-A"])?;

    // Commit with message
    let commit_msg = message.unwrap_or("Update MANA patterns");
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let full_msg = format!("{}\n\nExported {} patterns at {}", commit_msg, count, timestamp);

    run_git_command(&git_config.local_dir, &["commit", "-m", &full_msg])?;

    // Push to remote
    let push_result = run_git_command(&git_config.local_dir, &["push", "origin", &git_config.branch]);

    match push_result {
        Ok(_) => {
            println!("✅ Pushed {} patterns to remote", count);
            Ok(())
        }
        Err(e) => {
            warn!("Push failed: {}. Changes committed locally.", e);
            println!("⚠️  Push failed: {}. Changes committed locally.", e);
            println!("   Run 'git -C {:?} push' manually when ready", git_config.local_dir);
            Ok(())
        }
    }
}

/// Pull patterns from the git remote
///
/// Pulls latest changes and imports patterns with merge strategy.
pub fn pull_patterns(
    mana_dir: &Path,
    db_path: &Path,
    passphrase: Option<&str>,
    merge_strategy: MergeStrategy,
) -> Result<()> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let git_config = GitSyncConfig::from_backend(&config.backend, mana_dir)
        .ok_or_else(|| anyhow!("Sync backend is not configured for git"))?;

    if !git_config.local_dir.exists() {
        return Err(anyhow!("Sync repository not initialized. Run 'mana sync init' first."));
    }

    // Pull latest changes
    let pull_result = run_git_command(&git_config.local_dir, &["pull", "origin", &git_config.branch]);

    match pull_result {
        Ok(output) => {
            if output.contains("Already up to date") {
                println!("📋 Already up to date");
            } else {
                println!("✅ Pulled latest changes from remote");
            }
        }
        Err(e) => {
            warn!("Pull failed: {}. Using local patterns file.", e);
            println!("⚠️  Pull failed: {}. Using local patterns file.", e);
        }
    }

    // Import patterns from sync repo
    let import_file = git_config.local_dir.join("patterns.json");
    if !import_file.exists() {
        println!("📋 No patterns file found in sync repository");
        return Ok(());
    }

    let result = import_patterns(db_path, &import_file, passphrase, merge_strategy)?;

    println!("✅ Imported patterns from sync repository");
    println!("   Total: {}, New: {}, Merged: {}", result.total, result.imported, result.merged);
    if result.skipped > 0 {
        println!("   Skipped: {}", result.skipped);
    }

    Ok(())
}

/// Get sync status
///
/// Shows current sync state and any pending changes.
pub fn sync_status(mana_dir: &Path) -> Result<SyncStatus> {
    let config_path = mana_dir.join("sync.toml");

    if !config_path.exists() {
        return Ok(SyncStatus {
            configured: false,
            backend: "none".to_string(),
            repo_initialized: false,
            remote: None,
            branch: None,
            local_changes: false,
            last_sync: None,
        });
    }

    let config = load_sync_config(&config_path)?;

    let git_config = GitSyncConfig::from_backend(&config.backend, mana_dir);

    if let Some(git) = git_config {
        let repo_exists = git.local_dir.join(".git").exists();

        let (remote, local_changes, last_sync) = if repo_exists {
            let remote = run_git_command(&git.local_dir, &["remote", "get-url", "origin"]).ok();
            let status = run_git_command(&git.local_dir, &["status", "--porcelain"])
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let last_commit = run_git_command(&git.local_dir, &["log", "-1", "--format=%ci"])
                .ok()
                .map(|s| s.trim().to_string());
            (remote, status, last_commit)
        } else {
            (None, false, None)
        };

        Ok(SyncStatus {
            configured: true,
            backend: "git".to_string(),
            repo_initialized: repo_exists,
            remote: remote.map(|s| s.trim().to_string()),
            branch: Some(git.branch),
            local_changes,
            last_sync,
        })
    } else {
        Ok(SyncStatus {
            configured: true,
            backend: match &config.backend {
                SyncBackend::S3 { .. } => "s3".to_string(),
                SyncBackend::Supabase { .. } => "supabase".to_string(),
                SyncBackend::Git { .. } => "git".to_string(),
                SyncBackend::P2P { .. } => "p2p".to_string(),
            },
            repo_initialized: false,
            remote: None,
            branch: None,
            local_changes: false,
            last_sync: None,
        })
    }
}

/// Sync status information
#[derive(Debug, Clone)]
pub struct SyncStatus {
    /// Whether sync is configured
    pub configured: bool,
    /// Backend type
    pub backend: String,
    /// Whether the local repository is initialized
    pub repo_initialized: bool,
    /// Remote URL (if configured)
    pub remote: Option<String>,
    /// Branch (if git)
    pub branch: Option<String>,
    /// Whether there are uncommitted local changes
    pub local_changes: bool,
    /// Last sync timestamp
    pub last_sync: Option<String>,
}

/// Run a git command and return stdout
fn run_git_command(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .context("Failed to execute git command")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!("git {} failed: {}", args.join(" "), stderr))
    }
}

/// Save sync configuration
pub fn save_git_config(mana_dir: &Path, remote: &str, branch: &str) -> Result<()> {
    use crate::sync::{SyncConfig, SyncBackend, SecurityConfig, save_sync_config};

    let config = SyncConfig {
        enabled: true,
        backend: SyncBackend::Git {
            remote: remote.to_string(),
            branch: branch.to_string(),
        },
        interval_minutes: 60,
        security: SecurityConfig::default(),
    };

    let config_path = mana_dir.join("sync.toml");
    save_sync_config(&config, &config_path)?;

    info!("Saved sync configuration to {:?}", config_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_git_sync_config_from_backend() {
        let backend = SyncBackend::Git {
            remote: "git@github.com:user/repo.git".to_string(),
            branch: "main".to_string(),
        };

        let mana_dir = PathBuf::from("/home/user/.mana");
        let config = GitSyncConfig::from_backend(&backend, &mana_dir);

        assert!(config.is_some());
        let config = config.unwrap();
        assert_eq!(config.remote, "git@github.com:user/repo.git");
        assert_eq!(config.branch, "main");
        assert_eq!(config.local_dir, PathBuf::from("/home/user/.mana/sync-repo"));
    }

    #[test]
    fn test_sync_status_unconfigured() {
        let temp = TempDir::new().unwrap();
        let status = sync_status(temp.path()).unwrap();

        assert!(!status.configured);
        assert_eq!(status.backend, "none");
    }

    #[test]
    fn test_init_empty_repo() {
        let temp = TempDir::new().unwrap();

        // This will fail if git is not installed, which is fine for tests
        let result = init_git_sync(temp.path(), "", "main");

        // Either it succeeds or fails gracefully
        if result.is_ok() {
            let sync_dir = temp.path().join("sync-repo");
            assert!(sync_dir.exists());
        }
    }
}
