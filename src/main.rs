use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod bench;
mod hooks;
mod learning;
mod storage;
mod sync;
mod update;

/// MANA - Memory-Augmented Neural Assistant
/// High-performance learning system for Claude Code context injection
#[derive(Parser)]
#[command(name = "mana")]
#[command(author = "MANA Autonomous Agent")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Memory-Augmented Neural Assistant for Claude Code", long_about = None)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Inject context from ReasoningBank (pre-hook)
    Inject {
        /// Tool type: edit, bash, task
        #[arg(long)]
        tool: String,
    },

    /// Process session end and trigger learning if threshold met
    SessionEnd,

    /// Run consolidation tasks manually
    Consolidate,

    /// Show current status and statistics
    Status,

    /// Show detailed statistics
    Stats,

    /// Initialize MANA configuration
    Init,

    /// Check for updates and self-update if available
    Update {
        /// Actually install the update (otherwise just checks)
        #[arg(long)]
        force: bool,
    },

    /// Debug: show sample patterns for inspection
    Debug {
        /// Number of patterns to show
        #[arg(long, default_value = "5")]
        limit: usize,
    },

    /// Prune low-quality or redundant patterns
    Prune {
        /// Minimum score threshold (success - failure)
        #[arg(long, default_value = "-2")]
        min_score: i64,
        /// Preview what would be pruned without deleting
        #[arg(long)]
        dry_run: bool,
    },

    /// Reset patterns and re-learn from logs
    Relearn,

    /// Run performance benchmarks
    Bench,

    /// Export patterns to a file (for sync/sharing)
    Export {
        /// Output file path
        #[arg(long, default_value = "mana-patterns.json")]
        output: String,
        /// Encrypt the export with a passphrase
        #[arg(long)]
        encrypted: bool,
        /// Passphrase for encryption (reads from MANA_SYNC_KEY env var if not provided)
        #[arg(long)]
        passphrase: Option<String>,
        /// Skip path sanitization (not recommended for sharing)
        #[arg(long)]
        no_sanitize: bool,
    },

    /// Import patterns from a file
    Import {
        /// Input file path
        input: String,
        /// Passphrase for decryption (reads from MANA_SYNC_KEY env var if not provided)
        #[arg(long)]
        passphrase: Option<String>,
        /// Merge strategy: add (default), replace, keep-best
        #[arg(long, default_value = "add")]
        merge: String,
    },

    /// Sync patterns with a remote repository
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },
}

#[derive(Subcommand)]
enum SyncAction {
    /// Initialize sync with a git repository
    Init {
        /// Backend type: git (default) or s3
        #[arg(long, default_value = "git")]
        backend: String,
        /// Git remote URL (for git backend, leave empty for local-only init)
        #[arg(long, default_value = "")]
        remote: String,
        /// Branch to sync with (for git backend)
        #[arg(long, default_value = "main")]
        branch: String,
        /// S3 bucket name (for s3 backend)
        #[arg(long, default_value = "")]
        bucket: String,
        /// S3 prefix/folder (for s3 backend)
        #[arg(long, default_value = "mana")]
        prefix: String,
        /// AWS region (for s3 backend)
        #[arg(long, default_value = "us-east-1")]
        region: String,
    },

    /// Push patterns to the remote repository
    Push {
        /// Commit message
        #[arg(short, long)]
        message: Option<String>,
        /// Passphrase for encryption (reads from MANA_SYNC_KEY env var if not provided)
        #[arg(long)]
        passphrase: Option<String>,
    },

    /// Pull patterns from the remote repository
    Pull {
        /// Passphrase for decryption (reads from MANA_SYNC_KEY env var if not provided)
        #[arg(long)]
        passphrase: Option<String>,
        /// Merge strategy: add (default), replace, keep-best
        #[arg(long, default_value = "add")]
        merge: String,
    },

    /// Show sync status
    Status,

    /// Set the encryption passphrase
    SetKey,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging - for inject command, silence logs unless verbose
    // because output goes through stdout hooks
    let is_inject = matches!(cli.command, Commands::Inject { .. });
    let filter = if cli.verbose {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
    } else if is_inject {
        // Silent for inject to avoid polluting hook stdout
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)  // Always write logs to stderr, not stdout
        .init();

    match cli.command {
        Commands::Inject { tool } => {
            hooks::inject_context(&tool).await?;
        }
        Commands::SessionEnd => {
            info!("Processing session end");
            hooks::session_end().await?;
        }
        Commands::Consolidate => {
            info!("Running consolidation");
            learning::consolidate().await?;
        }
        Commands::Status => {
            storage::show_status().await?;
        }
        Commands::Stats => {
            storage::show_stats().await?;
        }
        Commands::Init => {
            info!("Initializing MANA");
            storage::init().await?;
        }
        Commands::Update { force } => {
            update::update_command(force).await?;
        }
        Commands::Debug { limit } => {
            storage::debug_patterns(limit).await?;
        }
        Commands::Prune { min_score, dry_run } => {
            storage::prune_patterns(min_score, dry_run).await?;
        }
        Commands::Relearn => {
            storage::relearn().await?;
        }
        Commands::Bench => {
            bench::run_benchmarks().await?;
        }
        Commands::Export { output, encrypted, passphrase, no_sanitize } => {
            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            // Get passphrase from arg or env
            let passphrase = passphrase.or_else(|| std::env::var("MANA_SYNC_KEY").ok());

            let security = sync::SecurityConfig {
                sanitize_paths: !no_sanitize,
                redact_secrets: !no_sanitize,
                encrypt: encrypted,
                visibility: sync::Visibility::Private,
            };

            let pass_ref = if encrypted {
                passphrase.as_deref()
            } else {
                None
            };

            let count = sync::export_patterns(&db_path, std::path::Path::new(&output), &security, pass_ref)?;
            println!("✅ Exported {} patterns to {}", count, output);
            if encrypted {
                println!("📦 Export is encrypted with AES-256-GCM");
            }
            if !no_sanitize {
                println!("🔒 Paths sanitized, secrets redacted");
            }
        }
        Commands::Import { input, passphrase, merge } => {
            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            // Get passphrase from arg or env
            let passphrase = passphrase.or_else(|| std::env::var("MANA_SYNC_KEY").ok());

            let merge_strategy = match merge.as_str() {
                "replace" => sync::export::MergeStrategy::Replace,
                "keep-best" => sync::export::MergeStrategy::KeepBest,
                _ => sync::export::MergeStrategy::Add,
            };

            let result = sync::import_patterns(&db_path, std::path::Path::new(&input), passphrase.as_deref(), merge_strategy)?;

            println!("✅ Import complete from {}", result.source_workspace);
            println!("   Total patterns: {}", result.total);
            println!("   New patterns: {}", result.imported);
            println!("   Merged: {}", result.merged);
            if result.skipped > 0 {
                println!("   Skipped: {}", result.skipped);
            }
        }
        Commands::Sync { action } => {
            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            match action {
                SyncAction::Init { backend, remote, branch, bucket, prefix, region } => {
                    match backend.to_lowercase().as_str() {
                        "s3" => {
                            if bucket.is_empty() {
                                return Err(anyhow::anyhow!("S3 bucket is required. Use --bucket <name>"));
                            }
                            if !sync::is_s3_available() {
                                return Err(anyhow::anyhow!(
                                    "S3 sync not available. Rebuild MANA with: cargo build --release --features s3"
                                ));
                            }
                            sync::save_s3_config(&mana_dir, &bucket, &prefix, &region)?;
                            sync::init_s3_sync(&mana_dir, &bucket, &prefix, &region).await?;
                        }
                        "git" | _ => {
                            // Save config first
                            sync::save_git_config(&mana_dir, &remote, &branch)?;
                            // Then initialize the repository
                            sync::init_git_sync(&mana_dir, &remote, &branch)?;
                            println!("✅ Sync initialized");
                            if !remote.is_empty() {
                                println!("   Remote: {}", remote);
                            }
                            println!("   Branch: {}", branch);
                        }
                    }
                }
                SyncAction::Push { message, passphrase } => {
                    let passphrase = passphrase.or_else(|| std::env::var("MANA_SYNC_KEY").ok());
                    let security = sync::SecurityConfig::default();

                    // Auto-detect backend from config
                    let config_path = mana_dir.join("sync.toml");
                    let config = sync::load_sync_config(&config_path)?;
                    match &config.backend {
                        sync::SyncBackend::S3 { .. } => {
                            sync::push_patterns_s3(
                                &mana_dir,
                                &db_path,
                                &security,
                                passphrase.as_deref(),
                            ).await?;
                        }
                        sync::SyncBackend::Git { .. } => {
                            sync::push_patterns(
                                &mana_dir,
                                &db_path,
                                &security,
                                passphrase.as_deref(),
                                message.as_deref(),
                            )?;
                        }
                        sync::SyncBackend::Supabase { .. } => {
                            return Err(anyhow::anyhow!("Supabase sync not yet implemented"));
                        }
                    }
                }
                SyncAction::Pull { passphrase, merge } => {
                    let passphrase = passphrase.or_else(|| std::env::var("MANA_SYNC_KEY").ok());
                    let merge_strategy = match merge.as_str() {
                        "replace" => sync::export::MergeStrategy::Replace,
                        "keep-best" => sync::export::MergeStrategy::KeepBest,
                        _ => sync::export::MergeStrategy::Add,
                    };

                    // Auto-detect backend from config
                    let config_path = mana_dir.join("sync.toml");
                    let config = sync::load_sync_config(&config_path)?;
                    match &config.backend {
                        sync::SyncBackend::S3 { .. } => {
                            sync::pull_patterns_s3(
                                &mana_dir,
                                &db_path,
                                passphrase.as_deref(),
                                merge_strategy,
                            ).await?;
                        }
                        sync::SyncBackend::Git { .. } => {
                            sync::pull_patterns(
                                &mana_dir,
                                &db_path,
                                passphrase.as_deref(),
                                merge_strategy,
                            )?;
                        }
                        sync::SyncBackend::Supabase { .. } => {
                            return Err(anyhow::anyhow!("Supabase sync not yet implemented"));
                        }
                    }
                }
                SyncAction::Status => {
                    // Auto-detect backend from config
                    let config_path = mana_dir.join("sync.toml");
                    let config = sync::load_sync_config(&config_path)?;

                    println!("MANA Sync Status");
                    println!("================");
                    println!();

                    match &config.backend {
                        sync::SyncBackend::S3 { bucket, prefix, region } => {
                            let s3_status = sync::s3_status(&mana_dir).await?;
                            println!("Backend: s3");
                            println!("Bucket: {}", bucket);
                            println!("Prefix: {}", prefix);
                            println!("Region: {}", region);
                            println!("Patterns file: {}", if s3_status.object_exists { "✅ Exists" } else { "❌ Not found" });
                            if let Some(modified) = &s3_status.last_modified {
                                println!("Last modified: {}", modified);
                            }
                            if let Some(size) = s3_status.size_bytes {
                                println!("Size: {} bytes", size);
                            }
                        }
                        sync::SyncBackend::Git { .. } => {
                            let status = sync::sync_status(&mana_dir)?;
                            if !status.configured {
                                println!("⚠️  Sync not configured");
                                println!("   Run 'mana sync init' to set up synchronization");
                            } else {
                                println!("Backend: {}", status.backend);
                                println!("Initialized: {}", if status.repo_initialized { "✅" } else { "❌" });

                                if let Some(remote) = &status.remote {
                                    println!("Remote: {}", remote);
                                }
                                if let Some(branch) = &status.branch {
                                    println!("Branch: {}", branch);
                                }
                                if status.local_changes {
                                    println!("Local changes: ⚠️  Uncommitted changes");
                                } else {
                                    println!("Local changes: ✅ None");
                                }
                                if let Some(last_sync) = &status.last_sync {
                                    println!("Last sync: {}", last_sync);
                                }
                            }
                        }
                        sync::SyncBackend::Supabase { url } => {
                            println!("Backend: supabase");
                            println!("URL: {}", url);
                            println!("Status: ⚠️  Not yet implemented");
                        }
                    }
                }
                SyncAction::SetKey => {
                    println!("🔑 To set the sync encryption key:");
                    println!();
                    println!("   Option 1: Environment variable");
                    println!("   export MANA_SYNC_KEY=\"your-secure-passphrase\"");
                    println!();
                    println!("   Option 2: Pass directly to commands");
                    println!("   mana sync push --passphrase \"your-secure-passphrase\"");
                    println!("   mana sync pull --passphrase \"your-secure-passphrase\"");
                    println!();
                    println!("   💡 Tip: Use a strong passphrase (32+ characters)");
                    println!("   Generate one: openssl rand -base64 32");
                }
            }
        }
    }

    Ok(())
}

fn get_mana_dir() -> Result<std::path::PathBuf> {
    // Check for .mana directory in current project first
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    // Fall back to home directory
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}
