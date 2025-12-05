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
