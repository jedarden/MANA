//! P2P sync backend for decentralized pattern synchronization
//!
//! Implements peer-to-peer pattern sharing without requiring a central server.
//! Uses CRDT (Conflict-free Replicated Data Types) for eventual consistency.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │                     P2P Sync Architecture                        │
//! │                                                                  │
//! │  ┌───────────┐         ┌───────────┐         ┌───────────┐     │
//! │  │  Peer A   │◄───────►│  Peer B   │◄───────►│  Peer C   │     │
//! │  │  MANA     │         │  MANA     │         │  MANA     │     │
//! │  └───────────┘         └───────────┘         └───────────┘     │
//! │       │                     │                     │            │
//! │       └─────────────────────┴─────────────────────┘            │
//! │                         mDNS/DHT Discovery                      │
//! │                         CRDT Merge Protocol                     │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # CRDT Implementation
//!
//! Uses a LWW-Map (Last-Writer-Wins Map) for pattern conflict resolution:
//! - Each pattern has a unique hash as key
//! - Each entry has a timestamp and node_id for ordering
//! - Concurrent writes are resolved by (timestamp, node_id) ordering
//!
//! # Discovery Methods
//!
//! - **mDNS**: Local network discovery (default)
//! - **Static**: Manual peer list configuration

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::io::{Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use crate::sync::ExportablePattern;
use crate::sync::export::{export_patterns_to_vec, import_patterns_from_vec, MergeStrategy};
use crate::sync::SecurityConfig;

/// P2P Sync Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PConfig {
    /// Whether P2P sync is enabled
    pub enabled: bool,
    /// Discovery method
    pub discovery: DiscoveryMethod,
    /// Port to listen on
    pub listen_port: u16,
    /// Static peer list (for manual discovery)
    pub static_peers: Vec<String>,
    /// This node's unique identifier
    pub node_id: String,
    /// CRDT merge strategy
    pub merge_strategy: CRDTMergeStrategy,
}

impl Default for P2PConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            discovery: DiscoveryMethod::Static,
            listen_port: 4222,
            static_peers: Vec::new(),
            node_id: generate_node_id(),
            merge_strategy: CRDTMergeStrategy::LWW,
        }
    }
}

/// Discovery method for finding peers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryMethod {
    /// Manual static peer list
    Static,
    /// mDNS for local network discovery (future)
    #[serde(rename = "mdns")]
    MDNS,
    /// DHT for internet-wide discovery (future)
    #[serde(rename = "dht")]
    DHT,
}

impl std::fmt::Display for DiscoveryMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryMethod::Static => write!(f, "static"),
            DiscoveryMethod::MDNS => write!(f, "mdns"),
            DiscoveryMethod::DHT => write!(f, "dht"),
        }
    }
}

/// CRDT merge strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CRDTMergeStrategy {
    /// Last-Writer-Wins (timestamp-based)
    LWW,
    /// Add-only (never delete, merge counts)
    AddOnly,
}

impl std::fmt::Display for CRDTMergeStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CRDTMergeStrategy::LWW => write!(f, "lww"),
            CRDTMergeStrategy::AddOnly => write!(f, "add-only"),
        }
    }
}

/// A CRDT entry wrapping a pattern with metadata for conflict resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CRDTEntry {
    /// The pattern data
    pub pattern: ExportablePattern,
    /// Timestamp when this entry was last updated (Unix millis)
    pub timestamp: u64,
    /// Node ID that last updated this entry
    pub node_id: String,
    /// Version vector for this entry (node_id -> version)
    pub version: HashMap<String, u64>,
    /// Whether this entry has been deleted (tombstone)
    pub deleted: bool,
}

impl CRDTEntry {
    /// Create a new CRDT entry
    pub fn new(pattern: ExportablePattern, node_id: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut version = HashMap::new();
        version.insert(node_id.to_string(), 1);

        Self {
            pattern,
            timestamp: now,
            node_id: node_id.to_string(),
            version,
            deleted: false,
        }
    }

    /// Merge two CRDT entries using LWW
    pub fn merge_lww(&self, other: &CRDTEntry) -> CRDTEntry {
        // LWW: Higher timestamp wins, break ties with node_id
        let self_wins = match self.timestamp.cmp(&other.timestamp) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => self.node_id > other.node_id,
        };

        let winner = if self_wins { self } else { other };

        // Merge version vectors (take max of each)
        let mut merged_version = self.version.clone();
        for (node, ver) in &other.version {
            let current = merged_version.get(node).copied().unwrap_or(0);
            merged_version.insert(node.clone(), current.max(*ver));
        }

        CRDTEntry {
            pattern: winner.pattern.clone(),
            timestamp: winner.timestamp,
            node_id: winner.node_id.clone(),
            version: merged_version,
            deleted: winner.deleted,
        }
    }

    /// Merge two CRDT entries using add-only semantics
    pub fn merge_add_only(&self, other: &CRDTEntry) -> CRDTEntry {
        // Merge version vectors
        let mut merged_version = self.version.clone();
        for (node, ver) in &other.version {
            let current = merged_version.get(node).copied().unwrap_or(0);
            merged_version.insert(node.clone(), current.max(*ver));
        }

        // Merge pattern data (take max counts, keep newer context)
        let merged_pattern = ExportablePattern {
            pattern_hash: self.pattern.pattern_hash.clone(),
            tool_type: self.pattern.tool_type.clone(),
            command_category: self.pattern.command_category.clone()
                .or(other.pattern.command_category.clone()),
            context_query: if self.timestamp >= other.timestamp {
                self.pattern.context_query.clone()
            } else {
                other.pattern.context_query.clone()
            },
            success_count: self.pattern.success_count.max(other.pattern.success_count),
            failure_count: self.pattern.failure_count.max(other.pattern.failure_count),
        };

        CRDTEntry {
            pattern: merged_pattern,
            timestamp: self.timestamp.max(other.timestamp),
            node_id: if self.timestamp >= other.timestamp {
                self.node_id.clone()
            } else {
                other.node_id.clone()
            },
            version: merged_version,
            deleted: self.deleted && other.deleted, // Only deleted if both agree
        }
    }
}

/// CRDT Map for pattern storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CRDTMap {
    /// Pattern hash -> CRDT entry
    pub entries: HashMap<String, CRDTEntry>,
    /// This node's ID
    pub node_id: String,
    /// Merge strategy
    pub strategy: CRDTMergeStrategy,
}

impl CRDTMap {
    /// Create a new CRDT map
    pub fn new(node_id: String, strategy: CRDTMergeStrategy) -> Self {
        Self {
            entries: HashMap::new(),
            node_id,
            strategy,
        }
    }

    /// Insert or update a pattern
    pub fn insert(&mut self, pattern: ExportablePattern) {
        let hash = pattern.pattern_hash.clone();
        let new_entry = CRDTEntry::new(pattern, &self.node_id);

        if let Some(existing) = self.entries.get(&hash) {
            let merged = match self.strategy {
                CRDTMergeStrategy::LWW => existing.merge_lww(&new_entry),
                CRDTMergeStrategy::AddOnly => existing.merge_add_only(&new_entry),
            };
            self.entries.insert(hash, merged);
        } else {
            self.entries.insert(hash, new_entry);
        }
    }

    /// Merge another CRDT map into this one
    pub fn merge(&mut self, other: &CRDTMap) {
        for (hash, other_entry) in &other.entries {
            if let Some(existing) = self.entries.get(hash) {
                let merged = match self.strategy {
                    CRDTMergeStrategy::LWW => existing.merge_lww(other_entry),
                    CRDTMergeStrategy::AddOnly => existing.merge_add_only(other_entry),
                };
                self.entries.insert(hash.clone(), merged);
            } else {
                self.entries.insert(hash.clone(), other_entry.clone());
            }
        }
    }

    /// Get all non-deleted patterns
    pub fn patterns(&self) -> Vec<ExportablePattern> {
        self.entries
            .values()
            .filter(|e| !e.deleted)
            .map(|e| e.pattern.clone())
            .collect()
    }

    /// Delete a pattern (mark as tombstone)
    #[allow(dead_code)]
    pub fn delete(&mut self, hash: &str) {
        if let Some(entry) = self.entries.get_mut(hash) {
            entry.deleted = true;
            entry.timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let ver = entry.version.get(&self.node_id).copied().unwrap_or(0);
            entry.version.insert(self.node_id.clone(), ver + 1);
        }
    }

    /// Get the state version (for delta sync)
    pub fn version_vector(&self) -> HashMap<String, u64> {
        let mut max_versions: HashMap<String, u64> = HashMap::new();
        for entry in self.entries.values() {
            for (node, ver) in &entry.version {
                let current = max_versions.get(node).copied().unwrap_or(0);
                max_versions.insert(node.clone(), current.max(*ver));
            }
        }
        max_versions
    }
}

/// P2P sync message protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum P2PMessage {
    /// Request to sync (includes version vector for delta sync)
    SyncRequest { version: HashMap<String, u64> },
    /// Response with patterns newer than requested version
    SyncResponse { crdt_map: CRDTMap },
    /// Ping to check if peer is alive
    Ping { node_id: String },
    /// Pong response
    Pong { node_id: String },
    /// Announce presence (for discovery)
    Announce { node_id: String, listen_port: u16 },
}

/// Peer information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Peer's node ID
    pub node_id: String,
    /// Peer's address
    pub address: String,
    /// Last seen timestamp
    pub last_seen: u64,
    /// Whether the peer is currently online
    pub online: bool,
}

/// P2P sync status
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct P2PStatus {
    /// Whether P2P sync is configured
    pub configured: bool,
    /// Discovery method
    pub discovery: String,
    /// This node's ID
    pub node_id: String,
    /// Listen port
    pub listen_port: u16,
    /// Known peers
    pub peers: Vec<PeerInfo>,
    /// CRDT entry count
    pub entry_count: usize,
    /// Last sync timestamp
    pub last_sync: Option<String>,
}

/// Generate a unique node ID
fn generate_node_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash hostname
    if let Ok(hostname) = std::process::Command::new("hostname").output() {
        hostname.stdout.hash(&mut hasher);
    }

    // Hash current time for uniqueness
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .hash(&mut hasher);

    // Hash random bytes if available
    let mut random_bytes = [0u8; 8];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = file.read(&mut random_bytes);
        random_bytes.hash(&mut hasher);
    }

    format!("mana-{:016x}", hasher.finish())
}

/// Initialize P2P sync
pub fn init_p2p_sync(
    mana_dir: &Path,
    discovery: DiscoveryMethod,
    listen_port: u16,
    static_peers: Vec<String>,
) -> Result<()> {
    let config = P2PConfig {
        enabled: true,
        discovery,
        listen_port,
        static_peers,
        node_id: generate_node_id(),
        merge_strategy: CRDTMergeStrategy::LWW,
    };

    save_p2p_config(mana_dir, &config)?;

    // Initialize CRDT state file
    let crdt_path = mana_dir.join("p2p-crdt.json");
    if !crdt_path.exists() {
        let crdt_map = CRDTMap::new(config.node_id.clone(), config.merge_strategy);
        let content = serde_json::to_string_pretty(&crdt_map)?;
        std::fs::write(&crdt_path, content)?;
    }

    println!("✅ P2P sync initialized");
    println!("   Node ID: {}", config.node_id);
    println!("   Discovery: {}", discovery);
    println!("   Listen port: {}", listen_port);
    if !config.static_peers.is_empty() {
        println!("   Static peers: {}", config.static_peers.join(", "));
    }

    Ok(())
}

/// Load P2P configuration
pub fn load_p2p_config(mana_dir: &Path) -> Result<P2PConfig> {
    let config_path = mana_dir.join("p2p.toml");
    if !config_path.exists() {
        return Ok(P2PConfig::default());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let config: P2PConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save P2P configuration
pub fn save_p2p_config(mana_dir: &Path, config: &P2PConfig) -> Result<()> {
    let config_path = mana_dir.join("p2p.toml");
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&config_path, content)?;
    Ok(())
}

/// Load CRDT state
pub fn load_crdt_state(mana_dir: &Path) -> Result<CRDTMap> {
    let crdt_path = mana_dir.join("p2p-crdt.json");
    if !crdt_path.exists() {
        let config = load_p2p_config(mana_dir)?;
        return Ok(CRDTMap::new(config.node_id, config.merge_strategy));
    }

    let content = std::fs::read_to_string(&crdt_path)?;
    let crdt_map: CRDTMap = serde_json::from_str(&content)?;
    Ok(crdt_map)
}

/// Save CRDT state
pub fn save_crdt_state(mana_dir: &Path, crdt_map: &CRDTMap) -> Result<()> {
    let crdt_path = mana_dir.join("p2p-crdt.json");
    let content = serde_json::to_string_pretty(crdt_map)?;
    std::fs::write(&crdt_path, content)?;
    Ok(())
}

/// Sync patterns with a specific peer
pub fn sync_with_peer(
    mana_dir: &Path,
    db_path: &Path,
    peer_address: &str,
    security: &SecurityConfig,
    timeout_secs: u64,
) -> Result<SyncResult> {
    info!("Syncing with peer: {}", peer_address);

    // Load local CRDT state
    let mut local_crdt = load_crdt_state(mana_dir)?;
    let _config = load_p2p_config(mana_dir)?;

    // Export current patterns to CRDT
    let local_patterns = export_patterns_to_vec(db_path, security)?;
    for pattern in local_patterns {
        local_crdt.insert(pattern);
    }

    // Connect to peer
    let addr: SocketAddr = peer_address.parse()
        .map_err(|e| anyhow!("Invalid peer address: {}", e))?;

    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_secs))
        .map_err(|e| anyhow!("Failed to connect to peer: {}", e))?;

    stream.set_read_timeout(Some(Duration::from_secs(timeout_secs)))?;
    stream.set_write_timeout(Some(Duration::from_secs(timeout_secs)))?;

    // Send sync request with our version vector
    let request = P2PMessage::SyncRequest {
        version: local_crdt.version_vector(),
    };

    send_message(&stream, &request)?;

    // Receive response
    let response: P2PMessage = receive_message(&stream)?;

    match response {
        P2PMessage::SyncResponse { crdt_map: remote_crdt } => {
            let remote_count = remote_crdt.entries.len();
            let local_count_before = local_crdt.entries.len();

            // Merge remote CRDT into local
            local_crdt.merge(&remote_crdt);

            let local_count_after = local_crdt.entries.len();
            let new_patterns = local_count_after - local_count_before;

            // Save merged CRDT state
            save_crdt_state(mana_dir, &local_crdt)?;

            // Import merged patterns back to database
            let patterns = local_crdt.patterns();
            let import_result = import_patterns_from_vec(
                db_path,
                patterns,
                MergeStrategy::Add,
            )?;

            info!("Sync complete: received {} entries, {} new", remote_count, new_patterns);

            Ok(SyncResult {
                peer: peer_address.to_string(),
                received: remote_count,
                sent: local_count_before,
                merged: import_result.merged,
                new_patterns,
                success: true,
            })
        }
        _ => Err(anyhow!("Unexpected response from peer")),
    }
}

/// Sync result
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SyncResult {
    /// Peer address
    pub peer: String,
    /// Patterns received from peer
    pub received: usize,
    /// Patterns sent to peer
    pub sent: usize,
    /// Patterns merged
    pub merged: usize,
    /// New patterns added
    pub new_patterns: usize,
    /// Whether sync was successful
    pub success: bool,
}

/// Sync with all configured peers
pub fn sync_with_all_peers(
    mana_dir: &Path,
    db_path: &Path,
    security: &SecurityConfig,
) -> Result<Vec<SyncResult>> {
    let config = load_p2p_config(mana_dir)?;

    if !config.enabled {
        return Err(anyhow!("P2P sync is not enabled"));
    }

    let mut results = Vec::new();

    for peer in &config.static_peers {
        match sync_with_peer(mana_dir, db_path, peer, security, 30) {
            Ok(result) => {
                println!("✅ Synced with {}: +{} patterns", peer, result.new_patterns);
                results.push(result);
            }
            Err(e) => {
                warn!("Failed to sync with {}: {}", peer, e);
                println!("⚠️  Failed to sync with {}: {}", peer, e);
                results.push(SyncResult {
                    peer: peer.clone(),
                    received: 0,
                    sent: 0,
                    merged: 0,
                    new_patterns: 0,
                    success: false,
                });
            }
        }
    }

    Ok(results)
}

/// Handle incoming sync request (for running as a server)
#[allow(dead_code)]
pub fn handle_sync_request(
    mana_dir: &Path,
    db_path: &Path,
    security: &SecurityConfig,
    _request_version: HashMap<String, u64>,
) -> Result<CRDTMap> {
    let mut local_crdt = load_crdt_state(mana_dir)?;

    // Export current patterns to CRDT
    let local_patterns = export_patterns_to_vec(db_path, security)?;
    for pattern in local_patterns {
        local_crdt.insert(pattern);
    }

    // For now, return full state (future: delta based on version)
    Ok(local_crdt)
}

/// Get P2P sync status
pub fn p2p_status(mana_dir: &Path) -> Result<P2PStatus> {
    let config = load_p2p_config(mana_dir)?;

    if !config.enabled {
        return Ok(P2PStatus {
            configured: false,
            discovery: "none".to_string(),
            node_id: String::new(),
            listen_port: 0,
            peers: Vec::new(),
            entry_count: 0,
            last_sync: None,
        });
    }

    let crdt = load_crdt_state(mana_dir)?;

    // Build peer info from static peers
    let peers: Vec<PeerInfo> = config.static_peers.iter().map(|addr| {
        PeerInfo {
            node_id: "unknown".to_string(),
            address: addr.clone(),
            last_seen: 0,
            online: false,
        }
    }).collect();

    Ok(P2PStatus {
        configured: true,
        discovery: config.discovery.to_string(),
        node_id: config.node_id,
        listen_port: config.listen_port,
        peers,
        entry_count: crdt.entries.len(),
        last_sync: None,
    })
}

/// Add a peer to the configuration
pub fn add_peer(mana_dir: &Path, peer_address: &str) -> Result<()> {
    let mut config = load_p2p_config(mana_dir)?;

    // Validate address format
    let _: SocketAddr = peer_address.parse()
        .map_err(|e| anyhow!("Invalid peer address '{}': {}", peer_address, e))?;

    if config.static_peers.contains(&peer_address.to_string()) {
        return Err(anyhow!("Peer already exists: {}", peer_address));
    }

    config.static_peers.push(peer_address.to_string());
    save_p2p_config(mana_dir, &config)?;

    println!("✅ Added peer: {}", peer_address);
    Ok(())
}

/// Remove a peer from the configuration
pub fn remove_peer(mana_dir: &Path, peer_address: &str) -> Result<()> {
    let mut config = load_p2p_config(mana_dir)?;

    let initial_len = config.static_peers.len();
    config.static_peers.retain(|p| p != peer_address);

    if config.static_peers.len() == initial_len {
        return Err(anyhow!("Peer not found: {}", peer_address));
    }

    save_p2p_config(mana_dir, &config)?;

    println!("✅ Removed peer: {}", peer_address);
    Ok(())
}

/// List all configured peers
pub fn list_peers(mana_dir: &Path) -> Result<Vec<PeerInfo>> {
    let config = load_p2p_config(mana_dir)?;

    let peers: Vec<PeerInfo> = config.static_peers.iter().map(|addr| {
        PeerInfo {
            node_id: "unknown".to_string(),
            address: addr.clone(),
            last_seen: 0,
            online: false,
        }
    }).collect();

    Ok(peers)
}

/// Check if P2P sync is available (has peers configured)
#[allow(dead_code)]
pub fn is_p2p_available(mana_dir: &Path) -> bool {
    load_p2p_config(mana_dir)
        .map(|c| c.enabled && !c.static_peers.is_empty())
        .unwrap_or(false)
}

// Helper functions for message sending/receiving

fn send_message(stream: &TcpStream, msg: &P2PMessage) -> Result<()> {
    let data = serde_json::to_vec(msg)?;
    let len = data.len() as u32;

    let mut stream = stream;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&data)?;
    stream.flush()?;

    Ok(())
}

fn receive_message(stream: &TcpStream) -> Result<P2PMessage> {
    let mut stream = stream;

    // Read length prefix
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;

    // Sanity check
    if len > 100 * 1024 * 1024 {
        return Err(anyhow!("Message too large: {} bytes", len));
    }

    // Read message
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data)?;

    let msg: P2PMessage = serde_json::from_slice(&data)?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crdt_entry_merge_lww() {
        let pattern = ExportablePattern {
            pattern_hash: "hash1".to_string(),
            tool_type: "Bash".to_string(),
            command_category: Some("cargo".to_string()),
            context_query: "Build project".to_string(),
            success_count: 5,
            failure_count: 1,
        };

        let entry1 = CRDTEntry::new(pattern.clone(), "node1");

        // Simulate entry from another node with later timestamp
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut pattern2 = pattern.clone();
        pattern2.context_query = "Build and test".to_string();
        pattern2.success_count = 10;

        let entry2 = CRDTEntry::new(pattern2, "node2");

        let merged = entry1.merge_lww(&entry2);

        // Entry2 should win (later timestamp)
        assert_eq!(merged.pattern.success_count, 10);
        assert_eq!(merged.pattern.context_query, "Build and test");
        assert!(merged.version.contains_key("node1"));
        assert!(merged.version.contains_key("node2"));
    }

    #[test]
    fn test_crdt_entry_merge_add_only() {
        let pattern = ExportablePattern {
            pattern_hash: "hash1".to_string(),
            tool_type: "Bash".to_string(),
            command_category: Some("cargo".to_string()),
            context_query: "Build project".to_string(),
            success_count: 5,
            failure_count: 1,
        };

        let entry1 = CRDTEntry::new(pattern.clone(), "node1");

        let mut pattern2 = pattern.clone();
        pattern2.success_count = 3;
        pattern2.failure_count = 2;

        let entry2 = CRDTEntry::new(pattern2, "node2");

        let merged = entry1.merge_add_only(&entry2);

        // Add-only takes max of both counts
        assert_eq!(merged.pattern.success_count, 5);
        assert_eq!(merged.pattern.failure_count, 2);
    }

    #[test]
    fn test_crdt_map_merge() {
        let mut map1 = CRDTMap::new("node1".to_string(), CRDTMergeStrategy::LWW);
        let mut map2 = CRDTMap::new("node2".to_string(), CRDTMergeStrategy::LWW);

        let pattern1 = ExportablePattern {
            pattern_hash: "hash1".to_string(),
            tool_type: "Bash".to_string(),
            command_category: None,
            context_query: "Pattern 1".to_string(),
            success_count: 1,
            failure_count: 0,
        };

        let pattern2 = ExportablePattern {
            pattern_hash: "hash2".to_string(),
            tool_type: "Edit".to_string(),
            command_category: None,
            context_query: "Pattern 2".to_string(),
            success_count: 2,
            failure_count: 0,
        };

        map1.insert(pattern1);
        map2.insert(pattern2);

        map1.merge(&map2);

        assert_eq!(map1.entries.len(), 2);
        assert!(map1.entries.contains_key("hash1"));
        assert!(map1.entries.contains_key("hash2"));
    }

    #[test]
    fn test_generate_node_id() {
        let id1 = generate_node_id();
        let id2 = generate_node_id();

        assert!(id1.starts_with("mana-"));
        assert!(id2.starts_with("mana-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_discovery_method_display() {
        assert_eq!(DiscoveryMethod::Static.to_string(), "static");
        assert_eq!(DiscoveryMethod::MDNS.to_string(), "mdns");
        assert_eq!(DiscoveryMethod::DHT.to_string(), "dht");
    }
}
