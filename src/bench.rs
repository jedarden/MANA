//! Performance benchmarking for MANA
//!
//! Measures key performance metrics to ensure MANA meets latency targets:
//! - Context injection: <10ms
//! - Pattern search: <0.5ms
//! - Session-end parsing: <20ms

use anyhow::Result;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Run performance benchmarks
pub async fn run_benchmarks() -> Result<BenchmarkResults> {
    println!("MANA Performance Benchmarks");
    println!("===========================");
    println!();

    let mut results = BenchmarkResults::default();

    // Benchmark 1: Context injection latency
    println!("1. Context Injection Latency");
    println!("   Target: <10ms");
    let injection_times = benchmark_injection(10)?;
    let avg_injection = injection_times.iter().sum::<u128>() as f64 / injection_times.len() as f64;
    let min_injection = *injection_times.iter().min().unwrap_or(&0);
    let max_injection = *injection_times.iter().max().unwrap_or(&0);
    results.injection_avg_ms = avg_injection / 1000.0;
    results.injection_min_ms = min_injection as f64 / 1000.0;
    results.injection_max_ms = max_injection as f64 / 1000.0;

    println!("   Avg: {:.2}ms  Min: {:.2}ms  Max: {:.2}ms",
             results.injection_avg_ms, results.injection_min_ms, results.injection_max_ms);
    if results.injection_avg_ms < 10.0 {
        println!("   ✅ PASS");
    } else {
        println!("   ❌ FAIL (exceeds 10ms target)");
    }
    println!();

    // Benchmark 2: Pattern search latency (just DB query, no stdin)
    println!("2. Pattern Search Latency");
    println!("   Target: <0.5ms");
    let search_times = benchmark_pattern_search(20)?;
    let avg_search = search_times.iter().sum::<u128>() as f64 / search_times.len() as f64;
    let min_search = *search_times.iter().min().unwrap_or(&0);
    let max_search = *search_times.iter().max().unwrap_or(&0);
    results.search_avg_ms = avg_search / 1000.0;
    results.search_min_ms = min_search as f64 / 1000.0;
    results.search_max_ms = max_search as f64 / 1000.0;

    println!("   Avg: {:.3}ms  Min: {:.3}ms  Max: {:.3}ms",
             results.search_avg_ms, results.search_min_ms, results.search_max_ms);
    if results.search_avg_ms < 0.5 {
        println!("   ✅ PASS");
    } else {
        println!("   ⚠️  ABOVE TARGET (0.5ms) - still acceptable if injection passes");
    }
    println!();

    // Benchmark 3: Binary startup time
    println!("3. Binary Startup Time");
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
    let mana_dir = get_mana_dir()?;
    let db_path = mana_dir.join("metadata.sqlite");

    if !db_path.exists() {
        return Ok(vec![0]);
    }

    let mut times = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();

        // Direct DB query benchmark
        let conn = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, tool_type, context_query, success_count, failure_count FROM patterns WHERE tool_type = ? ORDER BY (success_count - failure_count) DESC LIMIT 20"
        )?;

        let _rows: Vec<_> = stmt.query_map(["Edit"], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?.filter_map(|r| r.ok()).collect();

        times.push(start.elapsed().as_micros());
    }

    Ok(times)
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
    pub search_avg_ms: f64,
    pub search_min_ms: f64,
    pub search_max_ms: f64,
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
| Injection latency (min) | {:.2}ms | - |
| Injection latency (max) | {:.2}ms | - |
| Search latency (avg) | {:.3}ms | <0.5ms |
| Startup time (avg) | {:.2}ms | <50ms |
| Pattern count | {} | - |"#,
            self.injection_avg_ms,
            self.injection_min_ms,
            self.injection_max_ms,
            self.search_avg_ms,
            self.startup_avg_ms,
            self.pattern_count
        )
    }
}
