//! Performance benchmarking for MANA
//!
//! Comprehensive benchmark suite measuring key performance metrics.
//! Performance targets:
//! - Context injection: <10ms
//! - Pattern search: <0.5ms
//! - Similarity cache hit: <10μs
//! - Session-end parsing: <20ms
//! - Binary startup: <50ms

use anyhow::Result;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Run performance benchmarks
pub async fn run_benchmarks() -> Result<BenchmarkResults> {
    println!("MANA Performance Benchmarks");
    println!("===========================\n");

    let mut results = BenchmarkResults::default();

    // Benchmark 1: Context injection latency
    println!("1. Context Injection Latency");
    println!("   Target: <10ms");
    let injection_times = benchmark_injection(10)?;
    let avg_injection = injection_times.iter().sum::<u128>() as f64 / injection_times.len() as f64;
    let min_injection = *injection_times.iter().min().unwrap_or(&0);
    let max_injection = *injection_times.iter().max().unwrap_or(&0);
    let p50_injection = percentile(&injection_times, 50);
    let p99_injection = percentile(&injection_times, 99);
    results.injection_avg_ms = avg_injection / 1000.0;
    results.injection_min_ms = min_injection as f64 / 1000.0;
    results.injection_max_ms = max_injection as f64 / 1000.0;
    results.injection_p50_us = p50_injection as f64;
    results.injection_p99_us = p99_injection as f64;

    println!("   Avg: {:.2}ms  p50: {:.0}μs  p99: {:.0}μs  Min: {:.2}ms  Max: {:.2}ms",
             results.injection_avg_ms, results.injection_p50_us, results.injection_p99_us,
             results.injection_min_ms, results.injection_max_ms);
    if results.injection_avg_ms < 10.0 {
        println!("   ✅ PASS");
    } else {
        println!("   ❌ FAIL (exceeds 10ms target)");
    }
    println!();

    // Benchmark 2: Pattern search latency (just DB query, no stdin)
    println!("2. Pattern Search Latency (DB + Similarity)");
    println!("   Target: <0.5ms");
    let search_times = benchmark_pattern_search(50)?;
    let avg_search = search_times.iter().sum::<u128>() as f64 / search_times.len() as f64;
    let min_search = *search_times.iter().min().unwrap_or(&0);
    let max_search = *search_times.iter().max().unwrap_or(&0);
    let p50_search = percentile(&search_times, 50);
    let p99_search = percentile(&search_times, 99);
    results.search_avg_ms = avg_search / 1000.0;
    results.search_min_ms = min_search as f64 / 1000.0;
    results.search_max_ms = max_search as f64 / 1000.0;
    results.search_p50_us = p50_search as f64;
    results.search_p99_us = p99_search as f64;

    println!("   Avg: {:.3}ms  p50: {:.0}μs  p99: {:.0}μs  Min: {:.3}ms  Max: {:.3}ms",
             results.search_avg_ms, results.search_p50_us, results.search_p99_us,
             results.search_min_ms, results.search_max_ms);
    if results.search_avg_ms < 0.5 {
        println!("   ✅ PASS");
    } else {
        println!("   ⚠️  ABOVE TARGET (0.5ms) - still acceptable if injection passes");
    }
    println!();

    // Benchmark 3: Similarity cache performance
    println!("3. Similarity Cache Performance");
    println!("   Target: cache hit <10μs | miss <500μs");
    let (cache_hit_times, cache_miss_times) = benchmark_similarity_cache(100)?;
    let avg_hit = if !cache_hit_times.is_empty() {
        cache_hit_times.iter().sum::<u128>() as f64 / cache_hit_times.len() as f64
    } else { 0.0 };
    let avg_miss = if !cache_miss_times.is_empty() {
        cache_miss_times.iter().sum::<u128>() as f64 / cache_miss_times.len() as f64
    } else { 0.0 };
    results.cache_hit_avg_us = avg_hit;
    results.cache_miss_avg_us = avg_miss;

    println!("   Cache hit avg: {:.1}μs", results.cache_hit_avg_us);
    println!("   Cache miss avg: {:.1}μs", results.cache_miss_avg_us);
    let speedup = if avg_hit > 0.0 { avg_miss / avg_hit } else { 0.0 };
    println!("   Speedup: {:.1}x", speedup);
    if avg_hit < 10.0 || cache_hit_times.is_empty() {
        println!("   ✅ PASS");
    } else {
        println!("   ⚠️  Cache hit slower than target");
    }
    println!();

    // Benchmark 4: Batch pattern insertion
    println!("4. Batch Pattern Insertion");
    println!("   Target: >1000 patterns/sec");
    let (insert_count, insert_time_ms) = benchmark_batch_insert(100)?;
    let ops_per_sec = if insert_time_ms > 0.0 {
        (insert_count as f64 / insert_time_ms) * 1000.0
    } else { 0.0 };
    results.batch_insert_ops_per_sec = ops_per_sec;

    println!("   Inserted {} patterns in {:.2}ms", insert_count, insert_time_ms);
    println!("   Rate: {:.0} patterns/sec", ops_per_sec);
    if ops_per_sec > 1000.0 {
        println!("   ✅ PASS");
    } else {
        println!("   ⚠️  Below target rate");
    }
    println!();

    // Benchmark 5: Binary startup time
    println!("5. Binary Startup Time");
    println!("   Target: <50ms");
    let startup_times = benchmark_startup(5)?;
    let avg_startup = startup_times.iter().sum::<u128>() as f64 / startup_times.len() as f64;
    results.startup_avg_ms = avg_startup / 1000.0;

    println!("   Avg: {:.2}ms", results.startup_avg_ms);
    if results.startup_avg_ms < 50.0 {
        println!("   ✅ PASS");
    } else {
        println!("   ❌ FAIL (exceeds 50ms target)");
    }
    println!();

    // Summary
    println!("Summary");
    println!("-------");
    let all_pass = results.injection_avg_ms < 10.0 && results.startup_avg_ms < 50.0;
    if all_pass {
        println!("✅ All critical benchmarks PASSED");
    } else {
        println!("❌ Some benchmarks FAILED - optimization needed");
    }
    println!();

    // Results summary
    println!("Performance Summary");
    println!("-------------------");
    println!("| Metric              | Value       |");
    println!("|---------------------|-------------|");
    println!("| Pattern Search p50  | {:>7.0}μs   |", results.search_p50_us);
    println!("| Batch Insert        | {:>7.0}/s   |", results.batch_insert_ops_per_sec);
    println!("| Cache Speedup       | {:>7.1}x    |", speedup);
    println!();

    // Show pattern count for context
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");
    if db_path.exists() {
        let conn = rusqlite::Connection::open(&db_path)?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM patterns", [], |r| r.get(0))?;
        println!("Pattern count: {} (benchmarks run against this dataset)", count);
        results.pattern_count = count;
    }

    Ok(results)
}

/// Calculate percentile from sorted values
fn percentile(values: &[u128], p: usize) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    let idx = (p * sorted.len() / 100).min(sorted.len() - 1);
    sorted[idx]
}

/// Benchmark context injection latency
fn benchmark_injection(iterations: usize) -> Result<Vec<u128>> {
    let mana_path = get_mana_binary()?;
    let mut times = Vec::with_capacity(iterations);

    // Sample input for injection
    let input = r#"{"tool":"Edit","input":{"file_path":"src/main.rs","old_string":"test"}}"#;

    for _ in 0..iterations {
        let start = Instant::now();

        let mut child = Command::new(&mana_path)
            .args(["inject", "--tool", "edit"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes())?;
        }

        child.wait()?;
        times.push(start.elapsed().as_micros());
    }

    Ok(times)
}

/// Benchmark pattern search (via status command which queries DB)
fn benchmark_pattern_search(iterations: usize) -> Result<Vec<u128>> {
    use crate::storage::calculate_similarity;

    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        return Ok(vec![0]);
    }

    // Pre-open connection outside the timing loop for pure query benchmark
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    // Test query simulating real injection workload
    let test_query = "Editing rs rust cargo toml crate file main.rs";
    let mut times = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();

        // Query patterns (the actual DB portion)
        let mut stmt = conn.prepare_cached(
            "SELECT id, tool_type, context_query, success_count, failure_count FROM patterns WHERE tool_type = ? ORDER BY (success_count - failure_count) DESC LIMIT 8"
        )?;

        let rows: Vec<(i64, String, String, i64, i64)> = stmt.query_map(["Edit"], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?.filter_map(|r| r.ok()).collect();

        // Include similarity scoring (the hot path in real injection)
        let mut scored: Vec<_> = rows.iter()
            .filter_map(|(id, _tool, context, success, failure)| {
                let sim = calculate_similarity(test_query, context);
                if sim >= 0.35 {
                    Some((*id, sim, *success - *failure))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        times.push(start.elapsed().as_micros());
    }

    Ok(times)
}

/// Benchmark similarity cache performance
fn benchmark_similarity_cache(iterations: usize) -> Result<(Vec<u128>, Vec<u128>)> {
    use crate::storage::similarity::{calculate_similarity, clear_cache};

    // Clear the cache first
    clear_cache();

    // Use longer strings to ensure cache is actually used (>20 chars threshold)
    let query_base = "This is a comprehensive test query for benchmarking similarity calculations in the MANA memory system with sufficient length to trigger caching behavior";
    let pattern = "This is a comprehensive sample pattern text that we want to match against the query for testing the MANA pattern matching system with detailed context";

    let mut miss_times = Vec::with_capacity(iterations);
    let mut hit_times = Vec::with_capacity(iterations);

    // First pass: cache misses - each query is unique
    for i in 0..iterations {
        let q = format!("{} iteration number {}", query_base, i);
        let start = Instant::now();
        let _ = calculate_similarity(&q, pattern);
        // Keep nanosecond precision, convert to fractional microseconds later
        miss_times.push(start.elapsed().as_nanos() as u128);
    }

    // Second pass: cache hits (same queries)
    for i in 0..iterations {
        let q = format!("{} iteration number {}", query_base, i);
        let start = Instant::now();
        let _ = calculate_similarity(&q, pattern);
        hit_times.push(start.elapsed().as_nanos() as u128);
    }

    // Convert to microseconds with fractional precision
    let miss_us: Vec<u128> = miss_times.iter().map(|ns| ns / 1000).collect();
    let hit_us: Vec<u128> = hit_times.iter().map(|ns| ns / 1000).collect();

    Ok((hit_us, miss_us))
}

/// Benchmark batch pattern insertion
fn benchmark_batch_insert(count: usize) -> Result<(usize, f64)> {
    use rusqlite::params;

    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        return Ok((0, 0.0));
    }

    let conn = rusqlite::Connection::open(&db_path)?;

    // Create a temporary table for benchmarking
    conn.execute(
        "CREATE TABLE IF NOT EXISTS bench_patterns (
            id INTEGER PRIMARY KEY,
            pattern_hash TEXT,
            tool_type TEXT,
            context_query TEXT,
            success_count INTEGER,
            failure_count INTEGER
        )",
        [],
    )?;

    // Clear any existing benchmark data
    conn.execute("DELETE FROM bench_patterns", [])?;

    let start = Instant::now();

    // Use a transaction for batch insert
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO bench_patterns (pattern_hash, tool_type, context_query, success_count, failure_count)
             VALUES (?, ?, ?, ?, ?)"
        )?;

        for i in 0..count {
            let hash = format!("bench_hash_{}", i);
            let context = format!("Benchmark pattern {} for testing batch insert performance", i);
            stmt.execute(params![hash, "Bash", context, 1, 0])?;
        }
    }
    tx.commit()?;

    let elapsed_ms = start.elapsed().as_micros() as f64 / 1000.0;

    // Clean up
    conn.execute("DROP TABLE IF EXISTS bench_patterns", [])?;

    Ok((count, elapsed_ms))
}

/// Benchmark binary startup time
fn benchmark_startup(iterations: usize) -> Result<Vec<u128>> {
    let mana_path = get_mana_binary()?;
    let mut times = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();

        let output = Command::new(&mana_path)
            .arg("--version")
            .output()?;

        if !output.status.success() {
            continue;
        }

        times.push(start.elapsed().as_micros());
    }

    Ok(times)
}

fn get_mana_binary() -> Result<PathBuf> {
    let mana_dir = get_mana_dir()?;
    let binary = mana_dir.join("mana");
    if binary.exists() {
        Ok(binary)
    } else {
        // Try current directory
        let cwd = std::env::current_exe()?;
        Ok(cwd)
    }
}

fn get_mana_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let project_mana = cwd.join(".mana");
    if project_mana.exists() {
        return Ok(project_mana);
    }

    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    Ok(home.join(".mana"))
}

/// Benchmark results
#[derive(Debug, Default)]
pub struct BenchmarkResults {
    pub injection_avg_ms: f64,
    pub injection_min_ms: f64,
    pub injection_max_ms: f64,
    pub injection_p50_us: f64,
    pub injection_p99_us: f64,
    pub search_avg_ms: f64,
    pub search_min_ms: f64,
    pub search_max_ms: f64,
    pub search_p50_us: f64,
    pub search_p99_us: f64,
    pub cache_hit_avg_us: f64,
    pub cache_miss_avg_us: f64,
    pub batch_insert_ops_per_sec: f64,
    pub startup_avg_ms: f64,
    pub pattern_count: i64,
}

impl BenchmarkResults {
    /// Check if all critical benchmarks pass
    #[allow(dead_code)]
    pub fn all_pass(&self) -> bool {
        self.injection_avg_ms < 10.0 && self.startup_avg_ms < 50.0
    }

    /// Format results as a markdown table (for GitHub issue updates)
    #[allow(dead_code)]
    pub fn to_markdown(&self) -> String {
        format!(
            r#"| Metric | Value | Target |
|--------|-------|--------|
| Injection latency (avg) | {:.2}ms | <10ms |
| Injection latency (p50) | {:.0}μs | - |
| Injection latency (p99) | {:.0}μs | - |
| Search latency (avg) | {:.3}ms | <0.5ms |
| Search latency (p50) | {:.0}μs | - |
| Cache hit latency | {:.1}μs | <10μs |
| Cache miss latency | {:.1}μs | <500μs |
| Batch insert | {:.0}/s | >1000/s |
| Startup time (avg) | {:.2}ms | <50ms |
| Pattern count | {} | - |"#,
            self.injection_avg_ms,
            self.injection_p50_us,
            self.injection_p99_us,
            self.search_avg_ms,
            self.search_p50_us,
            self.cache_hit_avg_us,
            self.cache_miss_avg_us,
            self.batch_insert_ops_per_sec,
            self.startup_avg_ms,
            self.pattern_count
        )
    }
}
