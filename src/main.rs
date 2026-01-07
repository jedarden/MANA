use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;
use std::path::PathBuf;
use rusqlite::{Connection, params};

// Type aliases for complex types (clippy::type_complexity)
type VerdictRow = (String, Option<i64>, String, f64, Option<String>, String);
type PatternRow = (String, String, i64, i64, Option<Vec<u8>>);

mod bench;
mod daemon;
mod embeddings;
mod hooks;
mod learning;
mod reflection;
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
    Bench {
        #[command(subcommand)]
        action: Option<BenchAction>,
    },

    /// Manage vector embeddings for semantic search
    Embed {
        #[command(subcommand)]
        action: EmbedAction,
    },

    /// Reflection system for analyzing pattern effectiveness
    Reflect {
        #[command(subcommand)]
        action: ReflectAction,
    },

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

    /// Team management commands
    Team {
        #[command(subcommand)]
        action: TeamAction,
    },

    /// Pattern management and inspection
    Patterns {
        #[command(subcommand)]
        action: PatternsAction,
    },

    /// Background daemon for faster context injection
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Memory health monitoring and pruning
    Health {
        #[command(subcommand)]
        action: Option<HealthAction>,
    },

    /// Trajectory failure analysis
    Analyze {
        #[command(subcommand)]
        action: AnalyzeAction,
    },

    /// Transfer learning between sessions and projects
    Transfer {
        #[command(subcommand)]
        action: TransferAction,
    },

    /// Explainability and provenance tracking
    Provenance {
        #[command(subcommand)]
        action: ProvenanceAction,
    },
}

#[derive(Subcommand)]
enum TransferAction {
    /// Transfer patterns from a source to destination
    From {
        /// Source: path to database, project directory, or session ID
        source: String,
        /// Destination database path (defaults to current .mana)
        #[arg(long)]
        to: Option<String>,
        /// Minimum pattern score to transfer
        #[arg(long, default_value = "0")]
        min_score: i64,
        /// Minimum success rate (0.0-1.0)
        #[arg(long, default_value = "0.5")]
        min_success_rate: f64,
        /// Preview mode (don't actually transfer)
        #[arg(long)]
        preview: bool,
        /// Filter by tool types (comma-separated)
        #[arg(long)]
        tool_types: Option<String>,
        /// Domain keywords for filtering (comma-separated)
        #[arg(long)]
        domains: Option<String>,
        /// Transfer only top N percentile (e.g., 0.9 for top 10%)
        #[arg(long)]
        top: Option<f64>,
        /// Adaptation strategy: direct, contextualize, generalize, specialize
        #[arg(long, default_value = "direct")]
        adapt: String,
        /// Target domain for adaptation
        #[arg(long)]
        target_domain: Option<String>,
    },

    /// List transferable patterns from a source
    List {
        /// Source: path to database, project directory, or session ID
        source: String,
        /// Maximum number of patterns to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Minimum transferability score (0.0-1.0)
        #[arg(long, default_value = "0.0")]
        min_score: f64,
    },

    /// Transfer RL policy (Q-table) from source
    Policy {
        /// Source: path to database, project directory, or session ID
        source: String,
        /// Destination database path (defaults to current .mana)
        #[arg(long)]
        to: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProvenanceAction {
    /// Explain why a pattern was selected
    Explain {
        /// Pattern ID to explain
        pattern_id: i64,
        /// Optional context for the explanation
        #[arg(long)]
        context: Option<String>,
    },

    /// Show full provenance chain for a pattern
    Show {
        /// Pattern ID to show provenance for
        pattern_id: i64,
    },

    /// Justify a recent action
    Justify {
        /// Action description
        action: String,
        /// Pattern ID that was used (optional)
        #[arg(long)]
        pattern_id: Option<i64>,
    },

    /// Show recent reasoning chains
    Chains {
        /// Number of chains to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Verify provenance certificate integrity
    Verify {
        /// Pattern ID to verify
        pattern_id: i64,
    },
}

#[derive(Subcommand)]
enum AnalyzeAction {
    /// Analyze a specific trajectory for failures
    Trajectory {
        /// Session ID to analyze
        session_id: String,
    },

    /// Show failure statistics across all trajectories
    Stats,
}

#[derive(Subcommand)]
enum SyncAction {
    /// Initialize sync with a git repository
    Init {
        /// Backend type: git (default), s3, supabase, or p2p
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
        /// Supabase project URL (for supabase backend)
        #[arg(long, default_value = "")]
        url: String,
        /// Discovery method for P2P: static (default), mdns, dht
        #[arg(long, default_value = "static")]
        discover: String,
        /// Listen port for P2P sync
        #[arg(long, default_value = "4222")]
        port: u16,
        /// Static peers for P2P (comma-separated, e.g., "192.168.1.10:4222,192.168.1.11:4222")
        #[arg(long, default_value = "")]
        peers: String,
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

    /// Manage P2P peers
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
}

#[derive(Subcommand)]
enum PeerAction {
    /// Add a peer to the P2P network
    Add {
        /// Peer address (e.g., "192.168.1.10:4222")
        address: String,
    },

    /// Remove a peer from the P2P network
    Remove {
        /// Peer address to remove
        address: String,
    },

    /// List all configured peers
    List,

    /// Sync with a specific peer
    SyncWith {
        /// Peer address to sync with
        address: String,
    },
}

#[derive(Subcommand)]
enum BenchAction {
    /// Benchmark SIMD vs naive distance calculations
    Simd {
        /// Vector dimensions to test
        #[arg(long, default_value = "384")]
        dimensions: usize,
        /// Number of iterations
        #[arg(long, default_value = "10000")]
        iterations: usize,
    },
}

#[derive(Subcommand)]
enum EmbedAction {
    /// Show embedding system status
    Status,

    /// Rebuild all embeddings (useful after model update)
    Rebuild,

    /// Test semantic search with a query
    Search {
        /// Search query text
        query: String,
        /// Number of results
        #[arg(short, long, default_value = "5")]
        limit: usize,
    },

    /// Generate embeddings for patterns that don't have them
    Generate,
}

#[derive(Subcommand)]
enum ReflectAction {
    /// Show reflection system status and statistics
    Status,

    /// Run a manual reflection cycle
    Run {
        /// Trigger type label (manual by default)
        #[arg(long, default_value = "manual")]
        trigger: String,
    },

    /// Show recent verdicts
    Verdicts {
        /// Number of verdicts to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Analyze a specific pattern's reflection history
    Analyze {
        /// Pattern ID to analyze
        pattern_id: i64,
    },

    /// Initialize reflection tables (run once)
    Init,
}

#[derive(Subcommand)]
enum TeamAction {
    /// Create a new team
    Create {
        /// Team name
        name: String,
    },

    /// List teams you belong to
    List,

    /// Invite a user to your team
    Invite {
        /// Team ID
        #[arg(long)]
        team: String,
        /// Email of the user to invite
        email: String,
    },

    /// Join a team using an invite code
    Join {
        /// Invite code
        code: String,
    },

    /// Share a pattern with your team
    Share {
        /// Pattern hash to share
        #[arg(long)]
        pattern: String,
        /// Team ID to share with
        #[arg(long)]
        team: String,
    },

    /// Print the SQL schema for Supabase tables
    SetupSchema,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the daemon in background
    Start {
        /// Run in foreground (for debugging)
        #[arg(long)]
        foreground: bool,
        /// Internal flag for double-fork daemonization (do not use directly)
        #[arg(long, hide = true)]
        daemonize: bool,
    },

    /// Stop the running daemon
    Stop,

    /// Show daemon status
    Status,
}

#[derive(Subcommand)]
enum HealthAction {
    /// Run automatic pruning based on health conditions
    AutoPrune {
        /// Show what would be pruned without actually pruning
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum PatternsAction {
    /// List all patterns with filtering options
    List {
        /// Filter by tool type (e.g., Edit, Bash, Write)
        #[arg(long)]
        tool: Option<String>,
        /// Maximum number of patterns to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Sort by: score (default), recent, uses
        #[arg(long, default_value = "score")]
        sort: String,
        /// Show only patterns with score above this threshold
        #[arg(long)]
        min_score: Option<i64>,
    },

    /// Show detailed information about a specific pattern
    Show {
        /// Pattern ID to show
        pattern_id: i64,
    },

    /// Search patterns by content
    Search {
        /// Search query (semantic search if embeddings available)
        query: String,
        /// Number of results to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Show pattern statistics summary
    Summary,

    /// Delete a specific pattern by ID
    Delete {
        /// Pattern ID to delete
        pattern_id: i64,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

/// Main entry point - uses sync main for inject command to avoid tokio overhead
fn main() -> Result<()> {
    // OPTIMIZATION: Parse args without initializing tokio runtime
    // The inject command needs <10ms latency, but tokio::main adds ~50ms overhead
    let cli = Cli::parse();

    // For inject command, run without tokio for maximum speed
    if let Commands::Inject { tool } = &cli.command {
        // Skip logging setup for inject - it adds overhead and we don't need it
        // Just run the context injection synchronously
        return hooks::inject_context(tool);
    }

    // For all other commands, use the async runtime
    run_async_main(cli)
}

/// Async main for commands that need tokio runtime
#[tokio::main(flavor = "current_thread")]
async fn run_async_main(cli: Cli) -> Result<()> {
    // Initialize logging - for most commands
    let filter = if cli.verbose {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
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
            // Should never reach here due to early return in main()
            // But keep for completeness
            hooks::inject_context(&tool)?;
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
        Commands::Bench { action } => {
            match action {
                Some(BenchAction::Simd { dimensions, iterations }) => {
                    use storage::benchmark_simd;
                    println!("SIMD Distance Benchmark");
                    println!("======================\n");
                    println!("Testing {} dimensions with {} iterations\n", dimensions, iterations);

                    let result = benchmark_simd(dimensions, iterations);

                    println!("Results:");
                    println!("  SIMD:  {:.2} ns/op", result.simd_ns_per_op);
                    println!("  Naive: {:.2} ns/op", result.naive_ns_per_op);
                    println!("  Speedup: {:.2}x", result.speedup);
                    println!();

                    if result.speedup > 1.0 {
                        println!("✓ SIMD acceleration is working ({:.2}x faster)", result.speedup);
                    } else {
                        println!("⚠ SIMD not providing speedup (might not be available on this CPU)");
                    }
                }
                None => {
                    bench::run_benchmarks().await?;
                }
            }
        }
        Commands::Embed { action } => {
            let mana_dir = get_mana_dir()?;

            match action {
                EmbedAction::Status => {
                    if embeddings::is_available(&mana_dir) {
                        let status = embeddings::status(&mana_dir)?;
                        println!("Embedding Status");
                        println!("================");
                        println!();
                        println!("Model: {} ({})", status.model_name, status.model_version);
                        println!("Dimensions: {}", status.dimensions);
                        println!("Indexed vectors: {}", status.vector_count);
                        if status.unembedded_count > 0 {
                            println!("Patterns without embeddings: {}", status.unembedded_count);
                            println!();
                            println!("   Run 'mana embed generate' to create missing embeddings");
                        }
                        println!("Index size: {} bytes", status.index_size_bytes);
                    } else {
                        println!("Embeddings not initialized.");
                        println!();
                        println!("Run 'mana embed generate' to create embeddings for all patterns.");
                    }
                }
                EmbedAction::Rebuild => {
                    println!("Rebuilding all embeddings...");
                    let config = embeddings::EmbeddingConfig::default();
                    let mut store = embeddings::init(&mana_dir, &config)?;
                    let count = store.rebuild()?;
                    println!("Rebuilt embeddings for {} patterns", count);
                }
                EmbedAction::Generate => {
                    println!("Generating embeddings for patterns without them...");
                    let config = embeddings::EmbeddingConfig::default();
                    let mut store = embeddings::init(&mana_dir, &config)?;
                    let count = store.embed_missing()?;
                    if count > 0 {
                        println!("Generated embeddings for {} patterns", count);
                    } else {
                        println!("All patterns already have embeddings.");
                    }
                }
                EmbedAction::Search { query, limit } => {
                    // Use open to load existing embeddings
                    let store = embeddings::EmbeddingStore::open(&mana_dir)?;

                    // Make sure we have embeddings
                    let status = store.status()?;
                    if status.vector_count == 0 {
                        println!("No embeddings found. Run 'mana embed generate' first.");
                        return Ok(());
                    }

                    println!("Searching for: \"{}\"", query);
                    println!();

                    let results = store.search_with_context(&query, limit)?;

                    if results.is_empty() {
                        println!("No matching patterns found.");
                    } else {
                        for (i, m) in results.iter().enumerate() {
                            let success_rate = m.success_rate() * 100.0;
                            println!("{}. [{}] (sim: {:.3}, success: {:.0}%)",
                                i + 1, m.tool_type, m.similarity, success_rate);
                            println!("   {}", m.context_query);
                            println!();
                        }
                    }
                }
            }
        }
        Commands::Reflect { action } => {
            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            match action {
                ReflectAction::Status => {
                    let status = reflection::get_reflection_status(&db_path)?;

                    println!("Reflection Status");
                    println!("=================");
                    println!();

                    if !status.tables_exist {
                        println!("Reflection tables not initialized.");
                        println!();
                        println!("Run 'mana reflect init' to set up reflection.");
                        return Ok(());
                    }

                    println!("Total verdicts: {}", status.total_verdicts);
                    println!("  Effective: {} ({})",
                        status.effective_count,
                        format_emoji(status.effective_count, "check"));
                    println!("  Neutral: {}", status.neutral_count);
                    println!("  Ineffective: {}", status.ineffective_count);
                    println!("  Harmful: {} ({})",
                        status.harmful_count,
                        format_emoji(status.harmful_count, "warning"));
                    println!();
                    println!("Reflection cycles: {}", status.total_cycles);

                    if status.last_trigger.is_some() {
                        println!();
                        println!("Last cycle:");
                        println!("  Trigger: {}", status.last_trigger.as_ref().unwrap());
                        println!("  Trajectories: {}", status.last_trajectories);
                        println!("  Verdicts: {}", status.last_verdicts);
                        println!("  Duration: {}ms", status.last_duration_ms);
                    }
                }
                ReflectAction::Run { trigger } => {
                    use std::time::Instant;

                    println!("Running reflection cycle ({})...", trigger);

                    // Initialize tables if needed
                    let conn = rusqlite::Connection::open(&db_path)?;
                    reflection::init_reflection_tables(&conn)?;

                    // Parse recent trajectories from all JSONL files
                    let start = Instant::now();
                    let log_dir = dirs::home_dir()
                        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                        .join(".claude")
                        .join("projects");

                    let mut all_trajectories = Vec::new();
                    if log_dir.exists() {
                        // Collect all JSONL files in subdirectories (same approach as foreground learning)
                        for entry in std::fs::read_dir(&log_dir)? {
                            let entry = entry?;
                            let path = entry.path();
                            if path.is_dir() {
                                // Scan all JSONL files in project directory
                                if let Ok(subentries) = std::fs::read_dir(&path) {
                                    for subentry in subentries.flatten() {
                                        let subpath = subentry.path();
                                        if subpath.extension().map(|e| e == "jsonl").unwrap_or(false) {
                                            if let Ok(trajectories) = learning::trajectory::parse_trajectories(&subpath, 0) {
                                                all_trajectories.extend(trajectories);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if all_trajectories.is_empty() {
                        println!("No trajectories found for reflection.");
                        return Ok(());
                    }

                    println!("Found {} trajectories to analyze", all_trajectories.len());

                    // Run reflection with database path for pattern linking
                    let config = reflection::ReflectionConfig::default();
                    let engine = reflection::ReflectionEngine::with_db_path(config, &db_path);

                    let verdicts = engine.reflect(&all_trajectories)?;
                    let updated = engine.apply_verdicts(&conn, &verdicts)?;

                    let duration = start.elapsed();

                    // Log the cycle
                    reflection::log_reflection_cycle(
                        &conn,
                        &trigger,
                        all_trajectories.len(),
                        verdicts.len(),
                        updated,
                        0, // new patterns
                        0, // demoted
                        duration.as_millis() as u64,
                    )?;

                    println!();
                    println!("Reflection complete:");
                    println!("  Trajectories analyzed: {}", all_trajectories.len());
                    println!("  Verdicts produced: {}", verdicts.len());
                    println!("  Patterns updated: {}", updated);
                    println!("  Duration: {:?}", duration);
                }
                ReflectAction::Verdicts { limit } => {
                    let conn = rusqlite::Connection::open(&db_path)?;

                    let mut stmt = conn.prepare(
                        "SELECT trajectory_hash, pattern_id, verdict, confidence, root_cause, created_at
                         FROM reflection_verdicts
                         ORDER BY created_at DESC
                         LIMIT ?1"
                    )?;

                    let verdicts: Vec<VerdictRow> = stmt
                        .query_map([limit as i64], |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                                row.get(5)?,
                            ))
                        })?
                        .filter_map(|r| r.ok())
                        .collect();

                    if verdicts.is_empty() {
                        println!("No verdicts found.");
                        println!();
                        println!("Run 'mana reflect run' to generate verdicts.");
                        return Ok(());
                    }

                    println!("Recent Verdicts");
                    println!("===============");
                    println!();

                    for (hash, pattern_id, verdict, confidence, root_cause, created_at) in verdicts {
                        let emoji = match verdict.as_str() {
                            "EFFECTIVE" => "",
                            "HARMFUL" => "",
                            "INEFFECTIVE" => "",
                            _ => "",
                        };
                        let pattern_str = pattern_id
                            .map(|id| format!("pattern #{}", id))
                            .unwrap_or_else(|| "no pattern".into());

                        println!("{} {} ({:.0}% confidence)", emoji, verdict, confidence * 100.0);
                        println!("   Trajectory: {}...", &hash[..8]);
                        println!("   {}", pattern_str);
                        if let Some(cause) = root_cause {
                            println!("   Root cause: {}", cause);
                        }
                        println!("   {}", created_at);
                        println!();
                    }
                }
                ReflectAction::Analyze { pattern_id } => {
                    let conn = rusqlite::Connection::open(&db_path)?;

                    // Get pattern info
                    let pattern: Option<(String, String, i64, i64)> = conn.query_row(
                        "SELECT tool_type, context_query, success_count, failure_count
                         FROM patterns WHERE id = ?1",
                        [pattern_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    ).ok();

                    match pattern {
                        Some((tool_type, context, success, failure)) => {
                            println!("Pattern Analysis: #{}", pattern_id);
                            println!("=================={}", "=".repeat(pattern_id.to_string().len()));
                            println!();
                            println!("Tool type: {}", tool_type);
                            println!("Context: {}", context);
                            println!("Success/Failure: {}/{}", success, failure);
                            println!();

                            // Get verdict stats
                            let stats = reflection::MemoryDistiller::get_pattern_stats(&conn, pattern_id)?;

                            if stats.total > 0 {
                                println!("Reflection History:");
                                println!("  Total verdicts: {}", stats.total);
                                println!("  Effective: {} ({:.0}%)",
                                    stats.effective,
                                    stats.effectiveness_ratio() * 100.0);
                                println!("  Harmful: {} ({:.0}%)",
                                    stats.harmful,
                                    stats.harm_ratio() * 100.0);
                                println!("  Avg confidence: {:.2}", stats.avg_confidence);

                                // Get recent verdicts
                                println!();
                                println!("Recent verdicts:");
                                let verdicts = reflection::MemoryDistiller::get_pattern_verdicts(&conn, pattern_id, 5)?;
                                for v in verdicts {
                                    let emoji = match v.category.as_str() {
                                        "EFFECTIVE" => "",
                                        "HARMFUL" => "",
                                        _ => "",
                                    };
                                    println!("  {} {} ({:.0}%)", emoji, v.category, v.confidence * 100.0);
                                    if let Some(cause) = v.root_cause {
                                        println!("     {}", cause);
                                    }
                                }
                            } else {
                                println!("No reflection verdicts for this pattern.");
                            }
                        }
                        None => {
                            println!("Pattern #{} not found.", pattern_id);
                        }
                    }
                }
                ReflectAction::Init => {
                    let conn = rusqlite::Connection::open(&db_path)?;
                    reflection::init_reflection_tables(&conn)?;
                    println!("Reflection tables initialized.");
                }
            }
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
                SyncAction::Init { backend, remote, branch, bucket, prefix, region, url, discover, port, peers } => {
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
                        "supabase" => {
                            if url.is_empty() {
                                return Err(anyhow::anyhow!("Supabase URL is required. Use --url <project-url>"));
                            }
                            if !sync::is_supabase_available() {
                                return Err(anyhow::anyhow!(
                                    "Supabase sync not available. Rebuild MANA with: cargo build --release --features supabase"
                                ));
                            }
                            sync::init_supabase_sync(&mana_dir, &url).await?;
                        }
                        "p2p" => {
                            // Parse discovery method
                            let discovery = match discover.to_lowercase().as_str() {
                                "mdns" => sync::DiscoveryMethod::Mdns,
                                "dht" => sync::DiscoveryMethod::Dht,
                                _ => sync::DiscoveryMethod::Static,
                            };

                            // Parse static peers
                            let static_peers: Vec<String> = if peers.is_empty() {
                                Vec::new()
                            } else {
                                peers.split(',').map(|s| s.trim().to_string()).collect()
                            };

                            sync::init_p2p_sync(&mana_dir, discovery, port, static_peers)?;
                        }
                        _ => {
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
                            if !sync::is_supabase_available() {
                                return Err(anyhow::anyhow!(
                                    "Supabase sync not available. Rebuild MANA with: cargo build --release --features supabase"
                                ));
                            }
                            let count = sync::push_patterns_supabase(
                                &mana_dir,
                                &db_path,
                                &security,
                                &config.security.visibility.to_string(),
                            ).await?;
                            println!("✅ Pushed {} patterns to Supabase", count);
                        }
                        sync::SyncBackend::P2P { .. } => {
                            // P2P push = sync with all peers
                            let results = sync::sync_with_all_peers(&mana_dir, &db_path, &security)?;
                            let total_new: usize = results.iter().map(|r| r.new_patterns).sum();
                            let successful = results.iter().filter(|r| r.success).count();
                            println!("✅ P2P sync complete: {} peers, +{} patterns", successful, total_new);
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
                            if !sync::is_supabase_available() {
                                return Err(anyhow::anyhow!(
                                    "Supabase sync not available. Rebuild MANA with: cargo build --release --features supabase"
                                ));
                            }
                            let result = sync::pull_patterns_supabase(
                                &mana_dir,
                                &db_path,
                                merge_strategy,
                                true,   // include team patterns
                                false,  // don't include public by default
                            ).await?;
                            println!("✅ Pulled patterns from Supabase");
                            println!("   Total: {}, New: {}, Merged: {}",
                                result.total, result.imported, result.merged);
                            if result.skipped > 0 {
                                println!("   Skipped: {}", result.skipped);
                            }
                        }
                        sync::SyncBackend::P2P { .. } => {
                            // P2P pull = sync with all peers
                            let security = sync::SecurityConfig::default();
                            let results = sync::sync_with_all_peers(&mana_dir, &db_path, &security)?;
                            let total_new: usize = results.iter().map(|r| r.new_patterns).sum();
                            let successful = results.iter().filter(|r| r.success).count();
                            println!("✅ P2P sync complete: {} peers, +{} patterns", successful, total_new);
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
                            if sync::is_supabase_available() {
                                let status = sync::supabase_status(&mana_dir).await?;
                                if status.connected {
                                    println!("Connected: ✅");
                                    if let Some(count) = status.pattern_count {
                                        println!("Remote patterns: {}", count);
                                    }
                                } else {
                                    println!("Connected: ❌");
                                    println!("Check MANA_SUPABASE_KEY environment variable");
                                }
                            } else {
                                println!("Status: ⚠️  Feature not compiled");
                                println!("Rebuild with: cargo build --release --features supabase");
                            }
                        }
                        sync::SyncBackend::P2P { discovery, listen_port, peers } => {
                            let status = sync::p2p_status(&mana_dir)?;
                            println!("Backend: p2p");
                            println!("Discovery: {}", discovery);
                            println!("Listen port: {}", listen_port);
                            println!("Node ID: {}", status.node_id);
                            println!("CRDT entries: {}", status.entry_count);
                            println!();
                            println!("Configured peers: {}", peers.len());
                            for peer in peers {
                                println!("  - {}", peer);
                            }
                            if peers.is_empty() {
                                println!("   (none configured)");
                                println!();
                                println!("   Add peers with: mana sync peer add <address>");
                            }
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
                SyncAction::Peer { action } => {
                    match action {
                        PeerAction::Add { address } => {
                            sync::add_peer(&mana_dir, &address)?;
                        }
                        PeerAction::Remove { address } => {
                            sync::remove_peer(&mana_dir, &address)?;
                        }
                        PeerAction::List => {
                            let peers = sync::list_peers(&mana_dir)?;
                            println!("P2P Peers");
                            println!("=========");
                            println!();
                            if peers.is_empty() {
                                println!("No peers configured.");
                                println!();
                                println!("Add a peer with: mana sync peer add <address>");
                            } else {
                                for peer in peers {
                                    let status = if peer.online { "🟢" } else { "⚪" };
                                    println!("{} {} ({})", status, peer.address, peer.node_id);
                                }
                            }
                        }
                        PeerAction::SyncWith { address } => {
                            let security = sync::SecurityConfig::default();
                            let result = sync::sync_with_peer(&mana_dir, &db_path, &address, &security, 30)?;
                            if result.success {
                                println!("✅ Synced with {}", address);
                                println!("   Received: {}, Merged: {}, New: {}",
                                    result.received, result.merged, result.new_patterns);
                            } else {
                                println!("❌ Sync with {} failed", address);
                            }
                        }
                    }
                }
            }
        }
        Commands::Team { action } => {
            let mana_dir = get_mana_dir()?;

            match action {
                TeamAction::Create { name } => {
                    if !sync::is_supabase_available() {
                        return Err(anyhow::anyhow!(
                            "Team features require Supabase. Rebuild MANA with: cargo build --release --features supabase"
                        ));
                    }
                    let team = sync::create_team(&mana_dir, &name).await?;
                    println!("✅ Team '{}' created", team.name);
                    println!("   ID: {}", team.id);
                    println!();
                    println!("   To invite members:");
                    println!("   mana team invite --team {} <email>", team.id);
                }
                TeamAction::List => {
                    if !sync::is_supabase_available() {
                        return Err(anyhow::anyhow!(
                            "Team features require Supabase. Rebuild MANA with: cargo build --release --features supabase"
                        ));
                    }
                    let teams = sync::list_teams(&mana_dir).await?;
                    if teams.is_empty() {
                        println!("You are not a member of any teams.");
                        println!();
                        println!("Create a team with: mana team create <name>");
                        println!("Join a team with: mana team join <invite-code>");
                    } else {
                        println!("Your Teams");
                        println!("==========");
                        println!();
                        for team in teams {
                            println!("📁 {} ({})", team.name, team.id);
                        }
                    }
                }
                TeamAction::Invite { team, email } => {
                    if !sync::is_supabase_available() {
                        return Err(anyhow::anyhow!(
                            "Team features require Supabase. Rebuild MANA with: cargo build --release --features supabase"
                        ));
                    }
                    sync::invite_to_team(&mana_dir, &team, &email).await?;
                }
                TeamAction::Join { code } => {
                    if !sync::is_supabase_available() {
                        return Err(anyhow::anyhow!(
                            "Team features require Supabase. Rebuild MANA with: cargo build --release --features supabase"
                        ));
                    }
                    sync::join_team(&mana_dir, &code).await?;
                }
                TeamAction::Share { pattern, team } => {
                    if !sync::is_supabase_available() {
                        return Err(anyhow::anyhow!(
                            "Team features require Supabase. Rebuild MANA with: cargo build --release --features supabase"
                        ));
                    }
                    sync::share_pattern(&mana_dir, &pattern, &team).await?;
                }
                TeamAction::SetupSchema => {
                    println!("Supabase Schema for MANA Team Features");
                    println!("======================================");
                    println!();
                    println!("Run this SQL in your Supabase SQL editor:");
                    println!();
                    println!("{}", sync::get_schema_sql());
                }
            }
        }
        Commands::Patterns { action } => {
            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            match action {
                PatternsAction::List { tool, limit, sort, min_score } => {
                    let conn = rusqlite::Connection::open(&db_path)?;

                    // Build query based on filters
                    let order_by = match sort.as_str() {
                        "recent" => "p.id DESC",
                        "uses" => "(p.success_count + p.failure_count) DESC",
                        _ => "(p.success_count - p.failure_count) DESC", // score
                    };

                    let tool_filter = tool.as_ref().map(|t| format!("AND p.tool_type = '{}'", t)).unwrap_or_default();
                    let score_filter = min_score.map(|s| format!("AND (p.success_count - p.failure_count) >= {}", s)).unwrap_or_default();

                    let query = format!(
                        "SELECT p.id, p.tool_type, p.context_query,
                                p.success_count, p.failure_count,
                                (p.success_count - p.failure_count) as score
                         FROM patterns p
                         WHERE 1=1 {} {}
                         ORDER BY {}
                         LIMIT ?",
                        tool_filter, score_filter, order_by
                    );

                    let mut stmt = conn.prepare(&query)?;
                    let patterns: Vec<(i64, String, String, i64, i64, i64)> = stmt
                        .query_map([limit as i64], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
                        })?
                        .filter_map(|r| r.ok())
                        .collect();

                    println!("Patterns ({})", patterns.len());
                    println!("{}", "=".repeat(50));
                    println!();

                    if patterns.is_empty() {
                        println!("No patterns found matching filters.");
                    } else {
                        for (id, tool_type, context, success, failure, score) in patterns {
                            let rate = if success + failure > 0 {
                                (success as f64 / (success + failure) as f64) * 100.0
                            } else {
                                0.0
                            };

                            // Truncate context for display
                            let context_display = if context.len() > 60 {
                                format!("{}...", &context[..57])
                            } else {
                                context
                            };

                            println!("#{} [{}] score:{} ({:.0}%)", id, tool_type, score, rate);
                            println!("   {}", context_display);
                            println!();
                        }
                    }
                }
                PatternsAction::Show { pattern_id } => {
                    let conn = rusqlite::Connection::open(&db_path)?;

                    let result: Option<PatternRow> = conn
                        .query_row(
                            "SELECT tool_type, context_query, success_count, failure_count, embedding
                             FROM patterns WHERE id = ?1",
                            [pattern_id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                        )
                        .ok();

                    match result {
                        Some((tool_type, context, success, failure, embedding)) => {
                            let score = success - failure;
                            let rate = if success + failure > 0 {
                                (success as f64 / (success + failure) as f64) * 100.0
                            } else {
                                0.0
                            };

                            println!("Pattern #{}", pattern_id);
                            println!("{}", "=".repeat(30));
                            println!();
                            println!("Tool type: {}", tool_type);
                            println!("Score: {} ({:.0}% success rate)", score, rate);
                            println!("Uses: {} success, {} failure", success, failure);
                            println!("Has embedding: {}", if embedding.is_some() { "✅" } else { "❌" });
                            println!();
                            println!("Context:");
                            println!("{}", context);

                            // Get reflection stats if available
                            let stats = reflection::MemoryDistiller::get_pattern_stats(&conn, pattern_id);
                            if let Ok(stats) = stats {
                                if stats.total > 0 {
                                    println!();
                                    println!("Reflection history:");
                                    println!("  Verdicts: {}", stats.total);
                                    println!("  Effective: {} ({:.0}%)", stats.effective, stats.effectiveness_ratio() * 100.0);
                                    println!("  Harmful: {} ({:.0}%)", stats.harmful, stats.harm_ratio() * 100.0);
                                }
                            }
                        }
                        None => {
                            println!("Pattern #{} not found.", pattern_id);
                        }
                    }
                }
                PatternsAction::Search { query, limit } => {
                    // Try semantic search first, fall back to text search
                    if embeddings::is_available(&mana_dir) {
                        match embeddings::search(&mana_dir, &query, limit) {
                            Ok(results) => {
                                println!("Semantic Search Results for: \"{}\"", query);
                                println!("{}", "=".repeat(50));
                                println!();

                                if results.is_empty() {
                                    println!("No matching patterns found.");
                                } else {
                                    let conn = rusqlite::Connection::open(&db_path)?;

                                    for (pattern_id, similarity) in results {
                                        if let Ok((tool_type, context, success, failure)) = conn.query_row::<(String, String, i64, i64), _, _>(
                                            "SELECT tool_type, context_query, success_count, failure_count FROM patterns WHERE id = ?1",
                                            [pattern_id],
                                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                                        ) {
                                            let score = success - failure;
                                            let context_display = if context.len() > 50 {
                                                format!("{}...", &context[..47])
                                            } else {
                                                context
                                            };
                                            println!("#{} [{}] similarity:{:.2} score:{}", pattern_id, tool_type, similarity, score);
                                            println!("   {}", context_display);
                                            println!();
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                println!("Semantic search failed: {}", e);
                                println!("Falling back to text search...");
                            }
                        }
                    } else {
                        // Text-based search fallback
                        let conn = rusqlite::Connection::open(&db_path)?;
                        let search_pattern = format!("%{}%", query);

                        let mut stmt = conn.prepare(
                            "SELECT id, tool_type, context_query, success_count, failure_count
                             FROM patterns
                             WHERE context_query LIKE ?1
                             ORDER BY (success_count - failure_count) DESC
                             LIMIT ?2"
                        )?;

                        let results: Vec<(i64, String, String, i64, i64)> = stmt
                            .query_map(rusqlite::params![search_pattern, limit as i64], |row| {
                                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                            })?
                            .filter_map(|r| r.ok())
                            .collect();

                        println!("Text Search Results for: \"{}\"", query);
                        println!("{}", "=".repeat(50));
                        println!();

                        if results.is_empty() {
                            println!("No matching patterns found.");
                        } else {
                            for (id, tool_type, context, success, failure) in results {
                                let score = success - failure;
                                let context_display = if context.len() > 50 {
                                    format!("{}...", &context[..47])
                                } else {
                                    context
                                };
                                println!("#{} [{}] score:{}", id, tool_type, score);
                                println!("   {}", context_display);
                                println!();
                            }
                        }
                    }
                }
                PatternsAction::Summary => {
                    let conn = rusqlite::Connection::open(&db_path)?;

                    // Get overall stats
                    let total: i64 = conn.query_row("SELECT COUNT(*) FROM patterns", [], |row| row.get(0))?;
                    let total_success: i64 = conn.query_row("SELECT COALESCE(SUM(success_count), 0) FROM patterns", [], |row| row.get(0))?;
                    let total_failure: i64 = conn.query_row("SELECT COALESCE(SUM(failure_count), 0) FROM patterns", [], |row| row.get(0))?;
                    let with_embedding: i64 = conn.query_row("SELECT COUNT(*) FROM patterns WHERE embedding IS NOT NULL", [], |row| row.get(0))?;

                    // Get stats by tool type
                    let mut stmt = conn.prepare(
                        "SELECT tool_type, COUNT(*), SUM(success_count), SUM(failure_count)
                         FROM patterns GROUP BY tool_type ORDER BY COUNT(*) DESC"
                    )?;

                    let by_tool: Vec<(String, i64, i64, i64)> = stmt
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
                        .filter_map(|r| r.ok())
                        .collect();

                    println!("Pattern Summary");
                    println!("{}", "=".repeat(40));
                    println!();
                    println!("Total patterns: {}", total);
                    println!("Total uses: {} ({} success, {} failure)", total_success + total_failure, total_success, total_failure);
                    if total_success + total_failure > 0 {
                        println!("Overall success rate: {:.1}%", (total_success as f64 / (total_success + total_failure) as f64) * 100.0);
                    }
                    println!("With embeddings: {} ({:.0}%)", with_embedding, if total > 0 { (with_embedding as f64 / total as f64) * 100.0 } else { 0.0 });
                    println!();

                    println!("By Tool Type:");
                    for (tool_type, count, success, failure) in by_tool {
                        let rate = if success + failure > 0 {
                            (success as f64 / (success + failure) as f64) * 100.0
                        } else {
                            0.0
                        };
                        println!("  {}: {} patterns ({:.0}% success)", tool_type, count, rate);
                    }
                }
                PatternsAction::Delete { pattern_id, force } => {
                    let conn = rusqlite::Connection::open(&db_path)?;

                    // Check if pattern exists
                    let exists: bool = conn
                        .query_row("SELECT 1 FROM patterns WHERE id = ?1", [pattern_id], |_| Ok(true))
                        .unwrap_or(false);

                    if !exists {
                        println!("Pattern #{} not found.", pattern_id);
                        return Ok(());
                    }

                    if !force {
                        // Show pattern info and ask for confirmation
                        let (tool_type, context): (String, String) = conn.query_row(
                            "SELECT tool_type, context_query FROM patterns WHERE id = ?1",
                            [pattern_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )?;

                        let context_display = if context.len() > 50 {
                            format!("{}...", &context[..47])
                        } else {
                            context
                        };

                        println!("About to delete pattern #{}:", pattern_id);
                        println!("  Type: {}", tool_type);
                        println!("  Context: {}", context_display);
                        println!();
                        println!("Use --force to confirm deletion.");
                        return Ok(());
                    }

                    // Delete the pattern
                    conn.execute("DELETE FROM patterns WHERE id = ?1", [pattern_id])?;

                    // Also delete from vector index if available
                    if embeddings::is_available(&mana_dir) {
                        let _ = embeddings::delete_from_index(&mana_dir, pattern_id);
                    }

                    println!("✅ Pattern #{} deleted.", pattern_id);
                }
            }
        }
        Commands::Health { action } => {
            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            if !db_path.exists() {
                println!("MANA not initialized. Run 'mana init' first.");
                return Ok(());
            }

            let conn = rusqlite::Connection::open(&db_path)?;

            match action {
                None => {
                    // Show health status
                    let monitor = storage::HealthMonitor::default();
                    let status = monitor.check_health(&conn)?;

                    println!("MANA Health Status");
                    println!("==================");
                    println!();
                    println!("Overall Health: {:.1}% {}",
                        status.health_score * 100.0,
                        if status.is_healthy { "HEALTHY" } else { "NEEDS ATTENTION" }
                    );
                    println!();
                    println!("Pattern Statistics:");
                    println!("  Total patterns: {}", status.total_patterns);
                    println!("  Low confidence (score < -2): {} ({:.1}%)",
                        status.low_confidence_count,
                        if status.total_patterns > 0 {
                            (status.low_confidence_count as f64 / status.total_patterns as f64) * 100.0
                        } else { 0.0 }
                    );
                    println!("  Floor confidence (score < -5): {} ({:.1}%)",
                        status.floor_confidence_count,
                        if status.total_patterns > 0 {
                            (status.floor_confidence_count as f64 / status.total_patterns as f64) * 100.0
                        } else { 0.0 }
                    );
                    println!("  Stale (30+ days unused): {} ({:.1}%)",
                        status.stale_count,
                        if status.total_patterns > 0 {
                            (status.stale_count as f64 / status.total_patterns as f64) * 100.0
                        } else { 0.0 }
                    );
                    println!("  Never accessed: {} ({:.1}%)",
                        status.never_accessed_count,
                        if status.total_patterns > 0 {
                            (status.never_accessed_count as f64 / status.total_patterns as f64) * 100.0
                        } else { 0.0 }
                    );
                    println!("  Average score: {:.2}", status.avg_score);
                    println!();
                    println!("Storage:");
                    println!("  Database size: {:.2} MB", status.storage_size_bytes as f64 / (1024.0 * 1024.0));
                    println!();

                    // Show recommended actions
                    let actions = monitor.recommend_actions(&status);
                    if actions.is_empty() {
                        println!("No pruning actions recommended.");
                    } else {
                        println!("Recommended Actions:");
                        for action in &actions {
                            println!("  - {}", action);
                        }
                        println!();
                        println!("Run 'mana health auto-prune' to execute these actions.");
                    }
                }

                Some(HealthAction::AutoPrune { dry_run }) => {
                    let monitor = storage::HealthMonitor::default();

                    if dry_run {
                        // Show what would be pruned
                        let status = monitor.check_health(&conn)?;
                        let actions = monitor.recommend_actions(&status);

                        println!("DRY RUN: Health Auto-Pruning");
                        println!("=============================");
                        println!();

                        // Show what would be pruned without making changes
                        let status = monitor.check_health(&conn)?;
                        let actions = monitor.recommend_actions(&status);

                        if actions.is_empty() {
                            println!("No pruning actions would be taken.");
                        } else {
                            println!("The following actions would be taken:");
                            for action in &actions {
                                println!("  - {}", action);
                            }
                        }
                    } else {
                        // Actually perform auto-pruning
                        let result = monitor.auto_prune(&conn)?;

                        println!("Health Auto-Pruning Complete");
                        println!("============================");
                        println!();
                        println!("Actions taken:");
                        for action in &result.actions_taken {
                            println!("  - {:?}", action);
                        }
                        println!();
                        println!("Patterns deleted: {}", result.patterns_deleted);
                        println!("Patterns decayed: {}", result.patterns_decayed);
                        println!();
                        println!("Health Score:");
                        println!("  Before: {:.1}% {}",
                            result.before_health.health_score * 100.0,
                            if result.before_health.is_healthy { "(healthy)" } else { "(unhealthy)" }
                        );
                        println!("  After:  {:.1}% {}",
                            result.after_health.health_score * 100.0,
                            if result.after_health.is_healthy { "(healthy)" } else { "(unhealthy)" }
                        );

                        if result.after_health.is_healthy && !result.before_health.is_healthy {
                            println!();
                            println!("Database health improved!");
                        }
                    }
                }
            }
        }

        Commands::Analyze { action } => {
            use learning::failure_analysis::FailureAnalyzer;

            match action {
                AnalyzeAction::Trajectory { session_id } => {
                    // Find and parse the trajectory with this session ID
                    let log_dir = dirs::home_dir()
                        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                        .join(".claude")
                        .join("projects");

                    let mut found_trajectory = None;

                    if log_dir.exists() {
                        for entry in std::fs::read_dir(&log_dir)? {
                            let entry = entry?;
                            let path = entry.path();
                            if path.is_dir() {
                                if let Ok(subentries) = std::fs::read_dir(&path) {
                                    for subentry in subentries.flatten() {
                                        let subpath = subentry.path();
                                        if subpath.extension().map(|e| e == "jsonl").unwrap_or(false) {
                                            if let Ok(trajectories) = learning::trajectory::parse_trajectories(&subpath, 0) {
                                                if let Some(traj) = trajectories.into_iter().find(|t| t.session_id == *session_id) {
                                                    found_trajectory = Some(traj);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                                if found_trajectory.is_some() {
                                    break;
                                }
                            }
                        }
                    }

                    match found_trajectory {
                        Some(trajectory) => {
                            let analyzer = FailureAnalyzer::new();
                            let analysis = analyzer.analyze(&trajectory);

                            println!("Trajectory Analysis: {}", session_id);
                            println!("{}", "=".repeat(50));
                            println!();
                            println!("Session: {}", analysis.session_id);
                            println!("Total steps: {}", analysis.total_steps);
                            println!("Success rate: {:.1}%", analysis.success_rate * 100.0);
                            println!();

                            if analysis.failure_points.is_empty() {
                                println!("No failures detected. This trajectory succeeded!");
                            } else {
                                println!("Failure Points: {}", analysis.failure_points.len());
                                println!();

                                for (i, fp) in analysis.failure_points.iter().enumerate() {
                                    println!("{}. [Step {}] {} - {:?}",
                                        i + 1, fp.step_index, fp.tool_name, fp.failure_type);
                                    println!("   Severity: {:?} | Recoverable: {}",
                                        fp.severity, if fp.is_recoverable { "Yes" } else { "No" });
                                    println!("   Context: {}", fp.context_before);

                                    // Truncate error message for display
                                    let err_display = if fp.error_message.len() > 100 {
                                        format!("{}...", &fp.error_message[..97])
                                    } else {
                                        fp.error_message.clone()
                                    };
                                    println!("   Error: {}", err_display);

                                    if let Some(suggestion) = &fp.suggested_fix {
                                        println!("   Suggestion: {}", suggestion);
                                    }
                                    println!();
                                }

                                if let Some(primary) = &analysis.primary_failure {
                                    println!("Primary Failure:");
                                    println!("  Type: {:?}", primary.failure_type);
                                    println!("  Step: {}", primary.step_index);
                                    println!("  Tool: {}", primary.tool_name);
                                    println!();
                                }

                                if let Some(root_cause) = &analysis.root_cause {
                                    println!("Root Cause Analysis:");
                                    println!("  Category: {:?}", root_cause.category);
                                    println!("  Confidence: {:.0}%", root_cause.confidence * 100.0);
                                    println!("  Description: {}", root_cause.description);

                                    if !root_cause.contributing_factors.is_empty() {
                                        println!("  Contributing factors:");
                                        for factor in &root_cause.contributing_factors {
                                            println!("    - {}", factor);
                                        }
                                    }
                                    println!();
                                }

                                if !analysis.recovery_suggestions.is_empty() {
                                    println!("Recovery Suggestions:");
                                    for (i, suggestion) in analysis.recovery_suggestions.iter().enumerate() {
                                        println!("  {}. {}", i + 1, suggestion);
                                    }
                                }
                            }
                        }
                        None => {
                            println!("Trajectory with session ID '{}' not found.", session_id);
                            println!();
                            println!("Available sessions can be found in:");
                            println!("  ~/.claude/projects/*/");
                        }
                    }
                }
                AnalyzeAction::Stats => {
                    // Collect all trajectories and analyze them
                    println!("Analyzing all trajectories for failure statistics...");
                    println!();

                    let log_dir = dirs::home_dir()
                        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                        .join(".claude")
                        .join("projects");

                    let mut all_trajectories = Vec::new();

                    if log_dir.exists() {
                        for entry in std::fs::read_dir(&log_dir)? {
                            let entry = entry?;
                            let path = entry.path();
                            if path.is_dir() {
                                if let Ok(subentries) = std::fs::read_dir(&path) {
                                    for subentry in subentries.flatten() {
                                        let subpath = subentry.path();
                                        if subpath.extension().map(|e| e == "jsonl").unwrap_or(false) {
                                            if let Ok(trajectories) = learning::trajectory::parse_trajectories(&subpath, 0) {
                                                all_trajectories.extend(trajectories);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if all_trajectories.is_empty() {
                        println!("No trajectories found for analysis.");
                        return Ok(());
                    }

                    let analyzer = FailureAnalyzer::new();
                    let analyses: Vec<_> = all_trajectories.iter()
                        .map(|t| analyzer.analyze(t))
                        .collect();

                    let stats = analyzer.failure_statistics(&analyses);

                    println!("Failure Statistics");
                    println!("{}", "=".repeat(50));
                    println!();
                    println!("Total trajectories analyzed: {}", stats.total_trajectories);
                    println!("Trajectories with failures: {} ({:.1}%)",
                        stats.failed_trajectories,
                        if stats.total_trajectories > 0 {
                            (stats.failed_trajectories as f64 / stats.total_trajectories as f64) * 100.0
                        } else { 0.0 }
                    );
                    println!("Avg failure points per failed trajectory: {:.2}", stats.avg_failure_points_per_trajectory);
                    println!("Recovery rate: {:.1}%", stats.recovery_rate * 100.0);
                    println!();

                    if !stats.most_common_failures.is_empty() {
                        println!("Most Common Failure Types:");
                        for (i, (failure_type, count)) in stats.most_common_failures.iter().take(10).enumerate() {
                            let percentage = (*count as f64 / stats.failed_trajectories.max(1) as f64) * 100.0;
                            println!("  {}. {:?}: {} occurrences ({:.1}%)",
                                i + 1, failure_type, count, percentage);
                            println!("      {}", failure_type.description());
                        }
                    }
                }
            }
        }

        Commands::Transfer { action } => {
            use learning::{TransferEngine, TransferConfig, TransferSource, AdaptationStrategy};

            let mana_dir = get_mana_dir()?;
            let dest_db = mana_dir.join("metadata.sqlite");

            match action {
                TransferAction::From {
                    source,
                    to,
                    min_score,
                    min_success_rate,
                    preview,
                    tool_types,
                    domains,
                    top,
                    adapt,
                    target_domain,
                } => {
                    // Determine destination
                    let dest_path = if let Some(path) = to {
                        PathBuf::from(path)
                    } else {
                        dest_db.clone()
                    };

                    // Parse source
                    let transfer_source = if std::path::Path::new(&source).exists() {
                        // Check if it's a database file or directory
                        let source_path = std::path::Path::new(&source);
                        if source_path.is_file() {
                            TransferSource::Database(source_path.to_path_buf())
                        } else {
                            TransferSource::Project(source.clone())
                        }
                    } else {
                        // Assume it's a session ID
                        TransferSource::Session(source.clone())
                    };

                    // Create transfer engine
                    let config = TransferConfig {
                        min_score,
                        min_success_rate,
                        adapt_tier: true,
                        preserve_provenance: true,
                        merge_duplicates: true,
                        similarity_threshold: 0.85,
                    };

                    let engine = TransferEngine::new(config);

                    // Preview mode
                    if preview {
                        println!("Transfer Preview");
                        println!("================");
                        println!();

                        let preview_result = engine.preview_transfer(&transfer_source, &dest_path)?;

                        println!("Total patterns in source: {}", preview_result.total_patterns);
                        println!("Eligible for transfer: {}", preview_result.eligible_patterns);
                        println!("Would merge with existing: {}", preview_result.would_merge);
                        println!("Would skip: {}", preview_result.would_skip);
                        println!("Estimated benefit score: {:.2}", preview_result.estimated_benefit);

                        if !preview_result.conflicts.is_empty() {
                            println!();
                            println!("Potential issues:");
                            for conflict in &preview_result.conflicts {
                                println!("  - {}", conflict);
                            }
                        }

                        println!();
                        println!("Run without --preview to execute transfer.");
                        return Ok(());
                    }

                    // Execute transfer based on options
                    let result = if let Some(percentile) = top {
                        println!("Transferring top {:.0}% patterns...", (1.0 - percentile) * 100.0);
                        engine.transfer_top_patterns(&transfer_source, &dest_path, percentile)?
                    } else if tool_types.is_some() || domains.is_some() {
                        let tool_filter: Option<Vec<String>> = tool_types.as_ref().map(|s| {
                            s.split(',').map(|t| t.trim().to_string()).collect()
                        });
                        let domain_filter: Option<Vec<String>> = domains.as_ref().map(|s| {
                            s.split(',').map(|d| d.trim().to_string()).collect()
                        });

                        println!("Transferring filtered patterns...");
                        engine.transfer_filtered(
                            &transfer_source,
                            &dest_path,
                            tool_filter.as_deref(),
                            domain_filter.as_deref(),
                        )?
                    } else if adapt != "direct" || target_domain.is_some() {
                        let strategy = match adapt.to_lowercase().as_str() {
                            "contextualize" => AdaptationStrategy::Contextualize,
                            "generalize" => AdaptationStrategy::Generalize,
                            "specialize" => AdaptationStrategy::Specialize,
                            _ => AdaptationStrategy::Direct,
                        };

                        let domain = target_domain.as_deref().unwrap_or("general");
                        println!("Transferring with {:?} adaptation to '{}'...", strategy, domain);
                        engine.transfer_with_adaptation(&transfer_source, &dest_path, strategy, domain)?
                    } else {
                        println!("Transferring patterns...");
                        engine.transfer(&transfer_source, &dest_path)?
                    };

                    println!();
                    println!("Transfer Complete");
                    println!("=================");
                    println!();
                    println!("Patterns transferred: {}", result.patterns_transferred);
                    println!("Patterns merged: {}", result.patterns_merged);
                    println!("Patterns skipped: {}", result.patterns_skipped);
                    println!("Skills transferred: {}", result.skills_transferred);
                    println!("Causal edges transferred: {}", result.causal_edges_transferred);
                    println!();
                    println!("From: {}", result.source);
                    println!("To:   {}", result.destination);
                }

                TransferAction::List { source, limit, min_score } => {
                    // Parse source
                    let transfer_source = if std::path::Path::new(&source).exists() {
                        let source_path = std::path::Path::new(&source);
                        if source_path.is_file() {
                            TransferSource::Database(source_path.to_path_buf())
                        } else {
                            TransferSource::Project(source.clone())
                        }
                    } else {
                        TransferSource::Session(source.clone())
                    };

                    let config = TransferConfig::default();
                    let engine = TransferEngine::new(config);

                    let patterns = engine.get_transferable(&transfer_source)?;

                    // Filter and limit
                    let filtered: Vec<_> = patterns.into_iter()
                        .filter(|p| p.transferability_score >= min_score)
                        .take(limit)
                        .collect();

                    println!("Transferable Patterns (showing {} of available)", filtered.len());
                    println!("{}", "=".repeat(70));
                    println!();

                    if filtered.is_empty() {
                        println!("No transferable patterns found.");
                    } else {
                        for (i, pattern) in filtered.iter().enumerate() {
                            println!("{}. [{}] score:{} success:{:.0}% transfer:{:.2}",
                                i + 1,
                                pattern.tool_type,
                                pattern.score,
                                pattern.success_rate * 100.0,
                                pattern.transferability_score
                            );
                            println!("   {}", pattern.context_preview);
                            if !pattern.tier.is_empty() {
                                println!("   Tier: {}", pattern.tier);
                            }
                            println!();
                        }
                    }
                }

                TransferAction::Policy { source, to } => {
                    // Determine destination
                    let dest_path = if let Some(path) = to {
                        PathBuf::from(path)
                    } else {
                        dest_db.clone()
                    };

                    // Parse source
                    let transfer_source = if std::path::Path::new(&source).exists() {
                        let source_path = std::path::Path::new(&source);
                        if source_path.is_file() {
                            TransferSource::Database(source_path.to_path_buf())
                        } else {
                            TransferSource::Project(source.clone())
                        }
                    } else {
                        TransferSource::Session(source.clone())
                    };

                    let config = TransferConfig::default();
                    let engine = TransferEngine::new(config);

                    println!("Transferring Q-learning policy...");
                    let result = engine.transfer_policy(&transfer_source, &dest_path)?;

                    println!();
                    println!("Policy Transfer Complete");
                    println!("========================");
                    println!();
                    println!("States transferred: {}", result.states_transferred);
                    println!("Actions transferred: {}", result.actions_transferred);
                    println!("Q-values adapted: {}", result.q_values_adapted);
                }
            }
        }
        Commands::Provenance { action } => {
            use storage::ProvenanceStore;

            let mana_dir = get_mana_dir()?;
            let db_path = mana_dir.join("metadata.sqlite");

            if !db_path.exists() {
                println!("No database found. Run 'mana init' first.");
                return Ok(());
            }

            let conn = Connection::open(&db_path)?;
            let prov_store = ProvenanceStore::new(conn)?;

            match action {
                ProvenanceAction::Explain { pattern_id, context } => {
                    let context_str = context.as_deref().unwrap_or("general context");
                    let explanation = prov_store.explain_selection(pattern_id, context_str)?;
                    println!("{}", explanation);
                }

                ProvenanceAction::Show { pattern_id } => {
                    let cert = prov_store.get_provenance(pattern_id)?;

                    println!("Provenance Certificate for Pattern #{}", pattern_id);
                    println!("{}", "=".repeat(60));
                    println!();

                    println!("Merkle Root: {}", cert.merkle_root);
                    println!("Created: {}", format_timestamp(cert.creation_timestamp));
                    println!();

                    println!("Source Trajectories ({}):", cert.source_trajectories.len());
                    for (i, traj) in cert.source_trajectories.iter().enumerate() {
                        println!("  {}. {}", i + 1, traj);
                    }
                    println!();

                    println!("Confidence Factors ({}):", cert.confidence_factors.len());
                    for factor in &cert.confidence_factors {
                        println!("  {} = {:.2} (weight: {:.2})",
                            factor.factor_name, factor.value, factor.weight);
                        if let Some(evidence) = &factor.evidence {
                            println!("    Evidence: {}", evidence);
                        }
                    }
                    println!();

                    println!("Derivation Chain ({} steps):", cert.derivation_chain.len());
                    for (i, step) in cert.derivation_chain.iter().enumerate() {
                        println!("  {}. {:?} (delta: {:+.2})",
                            i + 1, step.step_type, step.confidence_delta);
                        println!("     {}", step.evidence);
                        println!("     Time: {}", format_timestamp(step.timestamp));
                    }

                    // Verify certificate
                    let verified = prov_store.verify_certificate(&cert)?;
                    println!();
                    println!("Certificate Status: {}", if verified { "VERIFIED ✓" } else { "INVALID ✗" });
                }

                ProvenanceAction::Justify { action, pattern_id } => {
                    // Get top patterns as alternatives for comparison
                    let alternatives: Vec<(i64, f64)> = if pattern_id.is_some() {
                        let conn_alt = Connection::open(&db_path)?;
                        let mut stmt = conn_alt.prepare(
                            "SELECT id, (success_count - failure_count) as score
                             FROM patterns
                             WHERE id != ?1
                             ORDER BY score DESC
                             LIMIT 5"
                        )?;

                        let pid = pattern_id.unwrap();
                        let mut alts = Vec::new();
                        let rows = stmt.query_map(params![pid], |row| {
                            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                        })?;

                        for row in rows {
                            if let Ok(alt) = row {
                                alts.push(alt);
                            }
                        }
                        alts
                    } else {
                        Vec::new()
                    };

                    let justification = prov_store.justify_action(&action, pattern_id, &alternatives)?;

                    println!("Action Justification");
                    println!("{}", "=".repeat(60));
                    println!();

                    println!("Action: {}", justification.action_type);
                    if let Some(pid) = justification.pattern_used {
                        println!("Pattern Used: #{}", pid);
                    }
                    println!("Confidence: {:.1}%", justification.confidence * 100.0);
                    println!();

                    println!("Reasoning:");
                    println!("  {}", justification.reasoning);
                    println!();

                    if !justification.supporting_evidence.is_empty() {
                        println!("Supporting Evidence ({}):", justification.supporting_evidence.len());
                        for (i, evidence) in justification.supporting_evidence.iter().enumerate() {
                            println!("  {}. [{}] {}", i + 1, evidence.evidence_type, evidence.content);
                            println!("     Strength: {:.1}% | Source: {}",
                                evidence.strength * 100.0, evidence.source);
                        }
                        println!();
                    }

                    if !justification.alternatives_considered.is_empty() {
                        println!("Alternatives Considered ({}):", justification.alternatives_considered.len());
                        for (i, alt) in justification.alternatives_considered.iter().enumerate() {
                            println!("  {}. [score: {:.2}] {}", i + 1, alt.score, alt.action);
                            println!("     Rejected: {}", alt.reason_rejected);
                        }
                    }
                }

                ProvenanceAction::Chains { limit } => {
                    let chains = prov_store.get_recent_reasoning(limit)?;

                    println!("Recent Reasoning Chains (showing {})", chains.len());
                    println!("{}", "=".repeat(60));
                    println!();

                    if chains.is_empty() {
                        println!("No reasoning chains recorded yet.");
                    } else {
                        for (i, chain) in chains.iter().enumerate() {
                            println!("{}. Chain #{} (Pattern #{})", i + 1, chain.id, chain.pattern_id);
                            println!("   Context: {}", chain.task_context);
                            println!("   Confidence: {:.1}%", chain.confidence * 100.0);
                            println!("   Steps: {}", chain.steps.len());
                            for (j, step) in chain.steps.iter().enumerate() {
                                println!("     {}. {:?}: {}", j + 1, step.step_type, step.content);
                                if let Some(evidence) = &step.evidence {
                                    println!("        Evidence: {}", evidence);
                                }
                            }
                            println!("   Decision: {}", chain.final_decision);
                            println!("   Time: {}", format_timestamp(chain.timestamp));
                            println!();
                        }
                    }
                }

                ProvenanceAction::Verify { pattern_id } => {
                    let cert = prov_store.get_provenance(pattern_id)?;
                    let verified = prov_store.verify_certificate(&cert)?;

                    println!("Provenance Verification for Pattern #{}", pattern_id);
                    println!("{}", "=".repeat(60));
                    println!();

                    println!("Merkle Root: {}", cert.merkle_root);
                    println!("Derivation Steps: {}", cert.derivation_chain.len());
                    println!();

                    if verified {
                        println!("✓ Certificate is VALID");
                        println!();
                        println!("The provenance chain has not been tampered with.");
                        println!("All derivation steps are cryptographically verified.");
                    } else {
                        println!("✗ Certificate is INVALID");
                        println!();
                        println!("WARNING: The provenance chain may have been altered!");
                        println!("The Merkle root does not match the derivation chain.");
                    }
                }
            }
        }
        Commands::Daemon { action } => {
            let mana_dir = get_mana_dir()?;

            match action {
                DaemonAction::Start { foreground, daemonize } => {
                    // Handle --daemonize flag (second fork in double-fork pattern)
                    // This is called internally by ensure_daemon_running()
                    if daemonize {
                        #[cfg(unix)]
                        {
                            // Second fork: fork again and have parent exit immediately
                            // This ensures the daemon is orphaned and adopted by init
                            match unsafe { libc::fork() } {
                                -1 => {
                                    eprintln!("Second fork failed");
                                    std::process::exit(1);
                                }
                                0 => {
                                    // Child: this becomes the actual daemon
                                    // Close any inherited file descriptors
                                    unsafe {
                                        libc::close(0); // stdin
                                        libc::close(1); // stdout
                                        libc::close(2); // stderr
                                    }
                                    // Run the daemon
                                    if daemon::start_daemon(&mana_dir).is_err() {
                                        // Can't print since we closed stderr, just exit
                                        std::process::exit(1);
                                    }
                                }
                                _ => {
                                    // Parent: exit immediately so grandchild is orphaned
                                    std::process::exit(0);
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            // On non-Unix, just run directly
                            daemon::start_daemon(&mana_dir)?;
                        }
                        return Ok(());
                    }

                    if daemon::is_running() {
                        println!("Daemon is already running");
                        return Ok(());
                    }

                    if foreground {
                        println!("Starting daemon in foreground...");
                        daemon::start_daemon(&mana_dir)?;
                    } else {
                        // Fork to background using proper daemonization
                        println!("Starting daemon in background...");

                        // Use setsid (via 'setsid' command or nohup) to properly detach
                        // This prevents zombie processes by creating a new session
                        let exe = std::env::current_exe()?;

                        // Try using setsid first (Linux), fall back to direct spawn
                        let child_result = std::process::Command::new("setsid")
                            .arg("--fork")
                            .arg(&exe)
                            .arg("daemon")
                            .arg("start")
                            .arg("--foreground")
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();

                        let child = match child_result {
                            Ok(c) => c,
                            Err(_) => {
                                // setsid not available, use direct spawn with detach
                                // On Unix, we can use pre_exec to call setsid()
                                #[cfg(unix)]
                                {
                                    use std::os::unix::process::CommandExt;
                                    unsafe {
                                        std::process::Command::new(&exe)
                                            .arg("daemon")
                                            .arg("start")
                                            .arg("--foreground")
                                            .stdin(std::process::Stdio::null())
                                            .stdout(std::process::Stdio::null())
                                            .stderr(std::process::Stdio::null())
                                            .pre_exec(|| {
                                                // Create new session to fully detach
                                                libc::setsid();
                                                Ok(())
                                            })
                                            .spawn()?
                                    }
                                }
                                #[cfg(not(unix))]
                                {
                                    std::process::Command::new(&exe)
                                        .arg("daemon")
                                        .arg("start")
                                        .arg("--foreground")
                                        .stdin(std::process::Stdio::null())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::null())
                                        .spawn()?
                                }
                            }
                        };

                        println!("Daemon started with PID {}", child.id());
                        println!("Socket: {:?}", daemon::socket_path());

                        // Wait briefly for daemon to initialize
                        std::thread::sleep(std::time::Duration::from_millis(300));

                        // Verify daemon started
                        if daemon::is_running() {
                            println!("✅ Daemon is running");
                        } else {
                            println!("⚠️  Daemon may not have started correctly");
                        }
                    }
                }
                DaemonAction::Stop => {
                    if !daemon::is_running() {
                        println!("Daemon is not running");
                        return Ok(());
                    }

                    println!("Stopping daemon...");
                    daemon::stop_daemon()?;
                    println!("Daemon stopped");
                }
                DaemonAction::Status => {
                    let status = daemon::daemon_status();
                    println!("Daemon Status: {}", status);
                }
            }
        }
    }

    Ok(())
}

pub fn get_mana_dir() -> Result<std::path::PathBuf> {
    // Priority 1: If the binary is inside a .mana directory, use that
    // This ensures consistent behavior regardless of working directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            if exe_dir.file_name().map(|n| n == ".mana").unwrap_or(false) {
                return Ok(exe_dir.to_path_buf());
            }
        }
    }

    // Priority 2: Check for .mana directory in current project
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    // Priority 3: Fall back to home directory
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}

/// Format count with appropriate emoji for status display
fn format_emoji(count: i64, kind: &str) -> String {
    if count == 0 {
        return String::new();
    }
    match kind {
        "check" => "".to_string(),
        "warning" => "".to_string(),
        _ => String::new(),
    }
}

/// Format Unix timestamp for human reading
fn format_timestamp(ts: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + Duration::from_secs(ts as u64);
    let chrono_dt = chrono::DateTime::<chrono::Utc>::from(datetime);
    chrono_dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
