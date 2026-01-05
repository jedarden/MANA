//! Connection pooling for concurrent database access
//!
//! Uses r2d2 for connection pooling with rusqlite.
//! Enables safe concurrent access from multiple threads.

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use std::path::Path;
use std::sync::Arc;
use anyhow::Result;

/// Configuration for the connection pool
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_size: u32,           // Default: 5
    pub min_idle: Option<u32>,   // Default: 1
    pub max_lifetime_secs: Option<u64>,
    pub idle_timeout_secs: Option<u64>,
    pub connection_timeout_secs: u64, // Default: 30
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 5,
            min_idle: Some(1),
            max_lifetime_secs: Some(1800), // 30 minutes
            idle_timeout_secs: Some(600),   // 10 minutes
            connection_timeout_secs: 30,
        }
    }
}

/// Thread-safe connection pool for SQLite
pub struct ConnectionPool {
    pool: Pool<SqliteConnectionManager>,
    config: PoolConfig,
}

impl ConnectionPool {
    /// Create a new connection pool for the given database path
    pub fn new(db_path: &Path, config: PoolConfig) -> Result<Self> {
        let manager = SqliteConnectionManager::file(db_path)
            .with_init(|conn| {
                // Initialize each connection with optimal settings
                conn.execute_batch(r#"
                    PRAGMA journal_mode = WAL;
                    PRAGMA synchronous = NORMAL;
                    PRAGMA cache_size = 4000;
                    PRAGMA temp_store = MEMORY;
                    PRAGMA busy_timeout = 30000;
                "#)?;
                Ok(())
            });

        let pool = Pool::builder()
            .max_size(config.max_size)
            .min_idle(config.min_idle)
            .max_lifetime(config.max_lifetime_secs.map(std::time::Duration::from_secs))
            .idle_timeout(config.idle_timeout_secs.map(std::time::Duration::from_secs))
            .connection_timeout(std::time::Duration::from_secs(config.connection_timeout_secs))
            .build(manager)?;

        Ok(Self { pool, config })
    }

    /// Create a read-only pool for query operations
    pub fn new_readonly(db_path: &Path, config: PoolConfig) -> Result<Self> {
        let manager = SqliteConnectionManager::file(db_path)
            .with_flags(rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_init(|conn| {
                conn.execute_batch(r#"
                    PRAGMA cache_size = 8000;
                    PRAGMA temp_store = MEMORY;
                    PRAGMA mmap_size = 8388608;
                "#)?;
                Ok(())
            });

        let pool = Pool::builder()
            .max_size(config.max_size)
            .min_idle(config.min_idle)
            .build(manager)?;

        Ok(Self { pool, config })
    }

    /// Get a connection from the pool
    pub fn get(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        self.pool.get().map_err(|e| anyhow::anyhow!("Failed to get connection: {}", e))
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        let state = self.pool.state();
        PoolStats {
            max_size: self.config.max_size,
            connections: state.connections,
            idle_connections: state.idle_connections,
        }
    }

    /// Check if pool is healthy
    pub fn is_healthy(&self) -> bool {
        self.pool.get().is_ok()
    }
}

#[derive(Debug, Clone)]
pub struct PoolStats {
    pub max_size: u32,
    pub connections: u32,
    pub idle_connections: u32,
}

/// Global pool manager for application-wide connection sharing
pub struct PoolManager {
    write_pool: Option<Arc<ConnectionPool>>,
    read_pool: Option<Arc<ConnectionPool>>,
    db_path: std::path::PathBuf,
}

impl PoolManager {
    pub fn new(db_path: &Path) -> Self {
        Self {
            write_pool: None,
            read_pool: None,
            db_path: db_path.to_path_buf(),
        }
    }

    /// Initialize pools lazily
    pub fn init(&mut self) -> Result<()> {
        if self.write_pool.is_none() {
            let write_config = PoolConfig {
                max_size: 3,  // Fewer write connections
                ..Default::default()
            };
            self.write_pool = Some(Arc::new(ConnectionPool::new(&self.db_path, write_config)?));
        }

        if self.read_pool.is_none() {
            let read_config = PoolConfig {
                max_size: 10,  // More read connections
                ..Default::default()
            };
            self.read_pool = Some(Arc::new(ConnectionPool::new_readonly(&self.db_path, read_config)?));
        }

        Ok(())
    }

    /// Get a write connection
    pub fn write(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        self.write_pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Write pool not initialized"))?
            .get()
    }

    /// Get a read connection
    pub fn read(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        self.read_pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Read pool not initialized"))?
            .get()
    }

    /// Get read pool stats
    pub fn read_stats(&self) -> Option<PoolStats> {
        self.read_pool.as_ref().map(|p| p.stats())
    }

    /// Get write pool stats
    pub fn write_stats(&self) -> Option<PoolStats> {
        self.write_pool.as_ref().map(|p| p.stats())
    }
}

// Thread-local pool for the daemon
lazy_static::lazy_static! {
    static ref GLOBAL_POOL: std::sync::Mutex<Option<PoolManager>> = std::sync::Mutex::new(None);
}

pub fn init_global_pool(db_path: &Path) -> Result<()> {
    let mut pool = PoolManager::new(db_path);
    pool.init()?;
    *GLOBAL_POOL.lock().unwrap() = Some(pool);
    Ok(())
}

pub fn get_read_connection() -> Result<PooledConnection<SqliteConnectionManager>> {
    GLOBAL_POOL
        .lock()
        .unwrap()
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Global pool not initialized"))?
        .read()
}

pub fn get_write_connection() -> Result<PooledConnection<SqliteConnectionManager>> {
    GLOBAL_POOL
        .lock()
        .unwrap()
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Global pool not initialized"))?
        .write()
}

pub fn get_pool_stats() -> Option<(PoolStats, PoolStats)> {
    GLOBAL_POOL
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|pool| {
            match (pool.read_stats(), pool.write_stats()) {
                (Some(read), Some(write)) => Some((read, write)),
                _ => None,
            }
        })
}
