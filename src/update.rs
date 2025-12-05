//! Self-update functionality for MANA
//!
//! Checks GitHub releases for updates and downloads new binary.
//! Supports both automatic update and manual download.

use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info, warn};

const GITHUB_REPO: &str = "jedarden/MANA";
const BINARY_NAME: &str = "mana";

/// Check for available updates from GitHub releases
pub async fn check_for_updates() -> Result<Option<UpdateInfo>> {
    let current_version = env!("CARGO_PKG_VERSION");
    info!("Current version: {}", current_version);

    // Use gh CLI to fetch latest release info
    let output = Command::new("gh")
        .args(["release", "view", "--repo", GITHUB_REPO, "--json", "tagName,name,publishedAt,body"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let json_str = String::from_utf8_lossy(&out.stdout);
            debug!("Release info: {}", json_str);

            let release: serde_json::Value = serde_json::from_str(&json_str)?;

            let tag = release["tagName"]
                .as_str()
                .ok_or_else(|| anyhow!("No tag found in release"))?;

            // Parse version from tag (e.g., "v0.2.0" -> "0.2.0")
            let latest_version = tag.trim_start_matches('v');

            if is_newer_version(latest_version, current_version) {
                Ok(Some(UpdateInfo {
                    current_version: current_version.to_string(),
                    latest_version: latest_version.to_string(),
                    tag: tag.to_string(),
                    name: release["name"].as_str().unwrap_or(tag).to_string(),
                    body: release["body"].as_str().unwrap_or("").to_string(),
                }))
            } else {
                Ok(None)
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("no releases found") || stderr.contains("release not found") {
                info!("No releases found for {}", GITHUB_REPO);
                Ok(None)
            } else {
                debug!("gh command failed: {}", stderr);
                Err(anyhow!("Failed to check releases: {}", stderr))
            }
        }
        Err(e) => {
            warn!("gh CLI not available: {}. Install GitHub CLI to enable update checking.", e);
            println!("Note: Install GitHub CLI (gh) to enable automatic update checking.");
            println!("  macOS: brew install gh");
            println!("  Linux: https://github.com/cli/cli/blob/trunk/docs/install_linux.md");
            Ok(None)
        }
    }
}

/// Information about an available update
#[derive(Debug)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub tag: String,
    #[allow(dead_code)]
    pub name: String,
    pub body: String,
}

/// Compare version strings (semver-like comparison)
fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse_version = |v: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = v
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };

    let (l_major, l_minor, l_patch) = parse_version(latest);
    let (c_major, c_minor, c_patch) = parse_version(current);

    (l_major, l_minor, l_patch) > (c_major, c_minor, c_patch)
}

/// Perform the update by downloading new binary from GitHub release
pub async fn perform_update(info: &UpdateInfo) -> Result<()> {
    info!("Updating from {} to {}", info.current_version, info.latest_version);

    // Determine install location
    let install_dir = get_install_dir()?;
    let temp_path = install_dir.join(format!("{}-new", BINARY_NAME));

    println!("Downloading MANA {}...", info.latest_version);

    // Download using gh CLI
    let download_result = Command::new("gh")
        .args([
            "release", "download", &info.tag,
            "--repo", GITHUB_REPO,
            "--pattern", BINARY_NAME,
            "--dir", install_dir.to_str().unwrap(),
            "--clobber",
        ])
        .output();

    match download_result {
        Ok(out) if out.status.success() => {
            // Binary downloaded successfully
            info!("Downloaded new binary");
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("no assets match") || stderr.contains("no assets found") {
                // No binary in release - suggest building from source
                println!();
                println!("No pre-built binary found in release {}.", info.tag);
                println!("You can build from source:");
                println!();
                println!("  git pull origin main");
                println!("  cargo build --release");
                println!("  cp target/release/mana .mana/mana");
                println!();
                return Ok(());
            }
            return Err(anyhow!("Download failed: {}", stderr));
        }
        Err(e) => {
            return Err(anyhow!("Failed to run gh download: {}", e));
        }
    }

    // Check if download created the file
    let downloaded_path = install_dir.join(BINARY_NAME);
    if !downloaded_path.exists() {
        return Err(anyhow!("Download completed but binary not found at {:?}", downloaded_path));
    }

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&downloaded_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&downloaded_path, perms)?;
    }

    // Verify the new binary works
    let verify = Command::new(&downloaded_path)
        .arg("--version")
        .output();

    match verify {
        Ok(out) if out.status.success() => {
            let version_output = String::from_utf8_lossy(&out.stdout);
            println!("Successfully updated to: {}", version_output.trim());
            info!("Update complete");
        }
        _ => {
            // Rollback: remove potentially corrupted download
            warn!("New binary verification failed, keeping current version");
            return Err(anyhow!("Downloaded binary failed verification"));
        }
    }

    // Clean up temp file if it exists
    if temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }

    Ok(())
}

/// Get the directory where MANA is installed
fn get_install_dir() -> Result<PathBuf> {
    // Check for .mana directory in current project first
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    // Fall back to home directory
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}

/// Main update command handler
pub async fn update_command(force: bool) -> Result<()> {
    println!("Checking for updates...");

    match check_for_updates().await? {
        Some(info) => {
            println!();
            println!("Update available!");
            println!("  Current version: {}", info.current_version);
            println!("  Latest version:  {}", info.latest_version);
            println!();

            if !info.body.is_empty() {
                println!("Release notes:");
                // Show first 5 lines of release notes
                for line in info.body.lines().take(5) {
                    println!("  {}", line);
                }
                if info.body.lines().count() > 5 {
                    println!("  ...");
                }
                println!();
            }

            if force {
                perform_update(&info).await?;
            } else {
                println!("Run 'mana update --force' to install the update.");
                println!("Or manually: gh release download {} --repo {} -p mana", info.tag, GITHUB_REPO);
            }
        }
        None => {
            println!("You are running the latest version ({}).", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.2.0", "0.1.0"));
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(is_newer_version("0.1.1", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.2.0"));
        assert!(!is_newer_version("0.0.9", "0.1.0"));
    }

    #[test]
    fn test_version_parsing() {
        // Test with various version formats
        assert!(is_newer_version("0.2", "0.1.0"));
        assert!(is_newer_version("1", "0.9.9"));
    }
}
