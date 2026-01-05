//! Comprehensive Criterion.rs benchmark suite for MANA
//!
//! This benchmark suite measures performance across all critical MANA operations:
//! - Database operations (insert, search, record access)
//! - Vector search (brute-force vs HNSW)
//! - Serialization (JSON)
//! - Distance metrics (cosine, euclidean, dot product)
//! - Quantization (scalar and binary compression)
//! - End-to-end retrieval pipeline
//! - Similarity cache performance
//! - Pattern search with similarity matching
//! - Batch operations
//!
//! Run with: cargo bench --bench comprehensive

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use rusqlite::{Connection, params};
use std::path::PathBuf;
use tempfile::TempDir;

// Import MANA modules
use mana::storage::{calculate_similarity, similarity::clear_cache};

/// Setup a temporary MANA database for benchmarking
fn setup_test_db(pattern_count: usize) -> (TempDir, PathBuf) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.sqlite");

    let conn = Connection::open(&db_path).unwrap();

    // Create patterns table
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS patterns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            pattern_hash TEXT UNIQUE NOT NULL,
            tool_type TEXT NOT NULL,
            command_category TEXT,
            context_query TEXT NOT NULL,
            success_count INTEGER DEFAULT 0,
            failure_count INTEGER DEFAULT 0,
            last_used DATETIME,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            embedding_id INTEGER
        );
        CREATE INDEX idx_patterns_tool ON patterns(tool_type);
        CREATE INDEX idx_patterns_hash ON patterns(pattern_hash);
        CREATE INDEX idx_patterns_tool_score ON patterns(tool_type, (success_count - failure_count) DESC);
        "#,
    ).unwrap();

    // Insert test patterns
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO patterns (pattern_hash, tool_type, context_query, success_count, failure_count)
             VALUES (?, ?, ?, ?, ?)"
        ).unwrap();

        for i in 0..pattern_count {
            let tool_type = match i % 5 {
                0 => "Edit",
                1 => "Bash",
                2 => "Write",
                3 => "Read",
                _ => "Task",
            };

            let context = format!(
                "Task: Fix {} error | Approach: {} - editing file_{}.rs with context {}",
                if i % 3 == 0 { "type" } else { "runtime" },
                tool_type,
                i,
                "a".repeat(50 + (i % 100))
            );

            let hash = format!("hash_{}", i);
            let success = (i % 10) as i64;
            let failure = (i % 3) as i64;

            stmt.execute(params![hash, tool_type, context, success, failure]).unwrap();
        }
    }
    tx.commit().unwrap();

    (temp_dir, db_path)
}

/// Generate random normalized vector
fn random_vector(dim: usize, seed: u64) -> Vec<f32> {
    let mut v = vec![0.0; dim];
    let mut state = seed;
    for x in &mut v {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *x = ((state as f32) / (u64::MAX as f32)) * 2.0 - 1.0;
    }
    // Normalize
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// Cosine similarity
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    dot // Already normalized, so just dot product
}

/// Euclidean distance
fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum::<f32>().sqrt()
}

/// Dot product
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Scalar quantization (8-bit)
fn quantize_scalar8(vector: &[f32]) -> (Vec<u8>, f32, f32) {
    let min_val = vector.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = vector.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max_val - min_val;

    let data: Vec<u8> = if range < 1e-10 {
        vec![128u8; vector.len()]
    } else {
        vector.iter()
            .map(|&v| {
                let normalized = (v - min_val) / range;
                (normalized * 255.0).round() as u8
            })
            .collect()
    };

    (data, min_val, max_val)
}

/// Binary quantization
fn quantize_binary(vector: &[f32]) -> Vec<u8> {
    let mean: f32 = vector.iter().sum::<f32>() / vector.len() as f32;

    let bits: Vec<bool> = vector.iter()
        .map(|&v| v > mean)
        .collect();

    bits.chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                if bit {
                    byte |= 1 << i;
                }
            }
            byte
        })
        .collect()
}

//
// BENCHMARK GROUPS
//

/// Benchmark database operations
fn bench_database_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("database");

    // Insert benchmark
    group.bench_function("insert_single", |b| {
        b.iter_batched(
            || {
                let temp_dir = TempDir::new().unwrap();
                let db_path = temp_dir.path().join("bench.sqlite");
                let conn = Connection::open(&db_path).unwrap();
                conn.execute_batch(
                    "CREATE TABLE patterns (
                        id INTEGER PRIMARY KEY,
                        pattern_hash TEXT,
                        tool_type TEXT,
                        context_query TEXT,
                        success_count INTEGER,
                        failure_count INTEGER
                    )"
                ).unwrap();
                (temp_dir, conn)
            },
            |(temp_dir, conn)| {
                conn.execute(
                    "INSERT INTO patterns (pattern_hash, tool_type, context_query, success_count, failure_count)
                     VALUES (?, ?, ?, ?, ?)",
                    params!["hash_test", "Edit", "Test pattern for benchmarking insert operations", 1, 0]
                ).unwrap();
                drop(conn);
                drop(temp_dir);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // Search by tool type
    for size in [100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::new("search_by_tool", size), &size, |b, &size| {
            let (_temp, db_path) = setup_test_db(size);
            let conn = Connection::open(&db_path).unwrap();

            b.iter(|| {
                let mut stmt = conn.prepare_cached(
                    "SELECT id, tool_type, context_query, success_count, failure_count
                     FROM patterns
                     WHERE tool_type = ?
                     ORDER BY (success_count - failure_count) DESC
                     LIMIT 10"
                ).unwrap();

                let rows: Vec<(i64, String, String, i64, i64)> = stmt.query_map(["Edit"], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                }).unwrap().filter_map(|r| r.ok()).collect();

                black_box(rows)
            });
        });
    }

    // Count records
    group.bench_function("count_records", |b| {
        let (_temp, db_path) = setup_test_db(1000);
        let conn = Connection::open(&db_path).unwrap();

        b.iter(|| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM patterns",
                [],
                |row| row.get(0)
            ).unwrap();
            black_box(count)
        });
    });

    // Record access with index
    group.bench_function("indexed_access", |b| {
        let (_temp, db_path) = setup_test_db(5000);
        let conn = Connection::open(&db_path).unwrap();

        b.iter(|| {
            let pattern: Option<String> = conn.query_row(
                "SELECT context_query FROM patterns WHERE pattern_hash = ?",
                ["hash_42"],
                |row| row.get(0)
            ).ok();
            black_box(pattern)
        });
    });

    group.finish();
}

/// Benchmark vector search operations
fn bench_vector_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_search");

    // Brute force search
    for (vec_count, dim) in [(100, 384), (1000, 384), (10000, 384)] {
        group.throughput(Throughput::Elements(vec_count as u64));
        group.bench_with_input(
            BenchmarkId::new("brute_force", format!("{}x{}", vec_count, dim)),
            &(vec_count, dim),
            |b, &(count, dimensions)| {
                let vectors: Vec<Vec<f32>> = (0..count)
                    .map(|i| random_vector(dimensions, i as u64))
                    .collect();
                let query = random_vector(dimensions, 99999);

                b.iter(|| {
                    let mut scores: Vec<(usize, f32)> = vectors.iter()
                        .enumerate()
                        .map(|(idx, v)| (idx, cosine_similarity(&query, v)))
                        .collect();
                    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                    black_box(&scores[..10.min(scores.len())])
                });
            },
        );
    }

    group.finish();
}

/// Benchmark distance metrics
fn bench_distance_metrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("distance_metrics");

    let dim = 384;
    let v1 = random_vector(dim, 1);
    let v2 = random_vector(dim, 2);

    group.bench_function("cosine_similarity", |b| {
        b.iter(|| {
            black_box(cosine_similarity(black_box(&v1), black_box(&v2)))
        });
    });

    group.bench_function("euclidean_distance", |b| {
        b.iter(|| {
            black_box(euclidean_distance(black_box(&v1), black_box(&v2)))
        });
    });

    group.bench_function("dot_product", |b| {
        b.iter(|| {
            black_box(dot_product(black_box(&v1), black_box(&v2)))
        });
    });

    group.finish();
}

/// Benchmark quantization operations
fn bench_quantization(c: &mut Criterion) {
    let mut group = c.benchmark_group("quantization");

    let dim = 384;
    let vector = random_vector(dim, 42);

    group.bench_function("scalar_8bit", |b| {
        b.iter(|| {
            black_box(quantize_scalar8(black_box(&vector)))
        });
    });

    group.bench_function("binary", |b| {
        b.iter(|| {
            black_box(quantize_binary(black_box(&vector)))
        });
    });

    // Memory comparison
    let (q8, _, _) = quantize_scalar8(&vector);
    let qbin = quantize_binary(&vector);
    let orig_size = vector.len() * 4;

    println!("\nQuantization compression ratios ({}D vector):", dim);
    println!("  Original: {} bytes", orig_size);
    println!("  Scalar-8: {} bytes ({:.1}x compression)", q8.len(), orig_size as f32 / q8.len() as f32);
    println!("  Binary:   {} bytes ({:.1}x compression)", qbin.len(), orig_size as f32 / qbin.len() as f32);

    group.finish();
}

/// Benchmark serialization
fn bench_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization");

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Pattern {
        id: i64,
        pattern_hash: String,
        tool_type: String,
        context_query: String,
        success_count: i64,
        failure_count: i64,
    }

    let pattern = Pattern {
        id: 123,
        pattern_hash: "hash_abc123".to_string(),
        tool_type: "Edit".to_string(),
        context_query: "Task: Fix type error in main.rs | Approach: Edit - editing main.rs replacing old code with new code".to_string(),
        success_count: 5,
        failure_count: 1,
    };

    group.bench_function("json_serialize", |b| {
        b.iter(|| {
            black_box(serde_json::to_string(black_box(&pattern)).unwrap())
        });
    });

    let json = serde_json::to_string(&pattern).unwrap();
    group.bench_function("json_deserialize", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<Pattern>(black_box(&json)).unwrap())
        });
    });

    group.finish();
}

/// Benchmark similarity cache
fn bench_similarity_cache(c: &mut Criterion) {
    let mut group = c.benchmark_group("similarity_cache");

    // Cache miss (first computation)
    group.bench_function("cache_miss", |b| {
        b.iter_batched(
            || {
                clear_cache();
                let query = format!("This is a comprehensive test query for benchmarking similarity calculations iteration {}", rand::random::<u32>());
                let pattern = "This is a comprehensive sample pattern text that we want to match against for testing the MANA pattern matching system with detailed context information";
                (query, pattern.to_string())
            },
            |(query, pattern)| {
                black_box(calculate_similarity(&query, &pattern))
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // Cache hit (repeated computation)
    let query = "This is a comprehensive test query for benchmarking similarity calculations in the MANA memory system with sufficient length to trigger caching behavior";
    let pattern = "This is a comprehensive sample pattern text that we want to match against for testing the MANA pattern matching system with detailed context information";

    // Pre-warm cache
    calculate_similarity(query, pattern);

    group.bench_function("cache_hit", |b| {
        b.iter(|| {
            black_box(calculate_similarity(black_box(query), black_box(pattern)))
        });
    });

    group.finish();
}

/// Benchmark pattern search (DB + similarity)
fn bench_pattern_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("pattern_search");

    let (_temp, db_path) = setup_test_db(500);
    let conn = Connection::open(&db_path).unwrap();
    let test_query = "Editing rs rust cargo toml crate file main.rs";

    group.bench_function("db_query_with_similarity", |b| {
        b.iter(|| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, tool_type, context_query, success_count, failure_count
                 FROM patterns
                 WHERE tool_type = ?
                 ORDER BY (success_count - failure_count) DESC
                 LIMIT 20"
            ).unwrap();

            let rows: Vec<(i64, String, String, i64, i64)> = stmt.query_map(["Edit"], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            }).unwrap().filter_map(|r| r.ok()).collect();

            // Score with similarity
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
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            black_box(scored)
        });
    });

    group.finish();
}

/// Benchmark batch insert operations
fn bench_batch_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_insert");

    for batch_size in [10, 100, 1000] {
        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(batch_size), &batch_size, |b, &size| {
            b.iter_batched(
                || {
                    let temp_dir = TempDir::new().unwrap();
                    let db_path = temp_dir.path().join("bench.sqlite");
                    let conn = Connection::open(&db_path).unwrap();
                    conn.execute_batch(
                        "CREATE TABLE patterns (
                            id INTEGER PRIMARY KEY,
                            pattern_hash TEXT,
                            tool_type TEXT,
                            context_query TEXT,
                            success_count INTEGER,
                            failure_count INTEGER
                        )"
                    ).unwrap();
                    (temp_dir, conn, size)
                },
                |(temp_dir, conn, size)| {
                    let tx = conn.unchecked_transaction().unwrap();
                    {
                        let mut stmt = tx.prepare_cached(
                            "INSERT INTO patterns (pattern_hash, tool_type, context_query, success_count, failure_count)
                             VALUES (?, ?, ?, ?, ?)"
                        ).unwrap();

                        for i in 0..size {
                            let hash = format!("bench_hash_{}", i);
                            let context = format!("Benchmark pattern {} for testing batch insert performance with longer context text", i);
                            stmt.execute(params![hash, "Bash", context, 1, 0]).unwrap();
                        }
                    }
                    tx.commit().unwrap();
                    drop(conn);
                    drop(temp_dir);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// Benchmark end-to-end retrieval pipeline
fn bench_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("end_to_end");

    let (_temp, db_path) = setup_test_db(2000);

    group.bench_function("full_retrieval_pipeline", |b| {
        b.iter(|| {
            // 1. Open connection
            let conn = Connection::open(&db_path).unwrap();

            // 2. Query database
            let mut stmt = conn.prepare_cached(
                "SELECT id, tool_type, context_query, success_count, failure_count
                 FROM patterns
                 WHERE tool_type = ?
                 ORDER BY (success_count - failure_count) DESC
                 LIMIT 15"
            ).unwrap();

            let rows: Vec<(i64, String, String, i64, i64)> = stmt.query_map(["Edit"], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            }).unwrap().filter_map(|r| r.ok()).collect();

            // 3. Calculate similarity
            let query = "Fix type error in Rust main.rs file cargo project";
            let mut scored: Vec<_> = rows.iter()
                .map(|(id, _tool, context, success, failure)| {
                    let sim = calculate_similarity(query, context);
                    (*id, sim, *success - *failure)
                })
                .collect();

            // 4. Sort and filter
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            let top_5: Vec<_> = scored.into_iter().take(5).collect();

            black_box(top_5)
        });
    });

    group.finish();
}

/// Benchmark text similarity with varying lengths
fn bench_text_similarity_lengths(c: &mut Criterion) {
    let mut group = c.benchmark_group("text_similarity_lengths");

    let short_query = "edit file";
    let medium_query = "editing main.rs rust cargo toml configuration file";
    let long_query = "Task: Fix compilation error in the main.rs file for the Rust project using cargo build system with proper error handling and type annotations";

    let short_pattern = "edit code";
    let medium_pattern = "Task: Edit file | Approach: Edit - editing main.rs rust project";
    let long_pattern = "Task: Fix type error in main source file | Approach: Edit - editing main.rs replacing old implementation with new code that includes proper error handling and uses the correct type annotations for the Rust programming language";

    group.bench_function("short_vs_short", |b| {
        b.iter(|| {
            black_box(calculate_similarity(black_box(short_query), black_box(short_pattern)))
        });
    });

    group.bench_function("medium_vs_medium", |b| {
        b.iter(|| {
            black_box(calculate_similarity(black_box(medium_query), black_box(medium_pattern)))
        });
    });

    group.bench_function("long_vs_long", |b| {
        b.iter(|| {
            black_box(calculate_similarity(black_box(long_query), black_box(long_pattern)))
        });
    });

    group.finish();
}

// Configure and register all benchmark groups
criterion_group!(
    benches,
    bench_database_operations,
    bench_vector_search,
    bench_distance_metrics,
    bench_quantization,
    bench_serialization,
    bench_similarity_cache,
    bench_pattern_search,
    bench_batch_insert,
    bench_end_to_end,
    bench_text_similarity_lengths,
);

criterion_main!(benches);
