use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

mod hooks;
mod learning;
mod storage;

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
    Update,

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
        Commands::Update => {
            info!("Checking for updates");
            println!("Update functionality not yet implemented");
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
    }

    Ok(())
}
