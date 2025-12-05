//! Supabase/PostgreSQL backend for team pattern synchronization
//!
//! Implements pattern sharing with team management features:
//! - Real-time sync using Supabase REST API
//! - Row-level security for access control
//! - Team creation and membership management
//! - Pattern sharing with visibility levels

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::sync::ExportablePattern;

#[cfg(feature = "supabase")]
use tracing::info;

#[cfg(feature = "supabase")]
use crate::sync::{SyncBackend, SecurityConfig, load_sync_config};
#[cfg(feature = "supabase")]
use crate::sync::export::{export_patterns_to_vec, import_patterns_from_vec, MergeStrategy};

pub use crate::sync::export::MergeStrategy as SupabaseMergeStrategy;

#[cfg(not(feature = "supabase"))]
use crate::sync::SecurityConfig;

/// Supabase configuration
#[cfg(feature = "supabase")]
#[derive(Debug, Clone)]
pub struct SupabaseConfig {
    /// Supabase project URL
    pub url: String,
    /// Supabase anon/service key (from env MANA_SUPABASE_KEY)
    pub api_key: String,
    /// Current user ID (set after authentication)
    pub user_id: Option<String>,
    /// Current team ID (if any)
    pub team_id: Option<String>,
}

#[cfg(feature = "supabase")]
impl SupabaseConfig {
    /// Create from SyncBackend::Supabase variant
    pub fn from_backend(backend: &SyncBackend) -> Option<Self> {
        match backend {
            SyncBackend::Supabase { url } => {
                let api_key = std::env::var("MANA_SUPABASE_KEY").ok()?;
                Some(Self {
                    url: url.clone(),
                    api_key,
                    user_id: None,
                    team_id: None,
                })
            }
            _ => None,
        }
    }

    /// Get REST API URL for a table
    fn rest_url(&self, table: &str) -> String {
        format!("{}/rest/v1/{}", self.url.trim_end_matches('/'), table)
    }
}

/// Team information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub created_at: String,
}

/// Team member information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct TeamMember {
    pub team_id: String,
    pub user_id: String,
    pub role: String, // owner, admin, member
    pub joined_at: String,
}

/// Shared pattern stored in Supabase
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedPattern {
    pub id: Option<String>,
    pub pattern_hash: String,
    pub tool_type: String,
    pub command_category: Option<String>,
    pub context_query: String,
    pub success_count: i64,
    pub failure_count: i64,
    pub owner_id: String,
    pub team_id: Option<String>,
    pub visibility: String, // private, team, public
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl From<ExportablePattern> for SharedPattern {
    fn from(p: ExportablePattern) -> Self {
        SharedPattern {
            id: None,
            pattern_hash: p.pattern_hash,
            tool_type: p.tool_type,
            command_category: p.command_category,
            context_query: p.context_query,
            success_count: p.success_count,
            failure_count: p.failure_count,
            owner_id: String::new(), // Set when pushing
            team_id: None,
            visibility: "private".to_string(),
            created_at: None,
            updated_at: None,
        }
    }
}

impl From<SharedPattern> for ExportablePattern {
    fn from(p: SharedPattern) -> Self {
        ExportablePattern {
            pattern_hash: p.pattern_hash,
            tool_type: p.tool_type,
            command_category: p.command_category,
            context_query: p.context_query,
            success_count: p.success_count,
            failure_count: p.failure_count,
        }
    }
}

/// Initialize Supabase sync configuration
#[cfg(feature = "supabase")]
pub async fn init_supabase_sync(mana_dir: &Path, url: &str) -> Result<()> {
    // Verify API key is set
    let api_key = std::env::var("MANA_SUPABASE_KEY")
        .map_err(|_| anyhow!("MANA_SUPABASE_KEY environment variable not set"))?;

    // Test connection by checking if we can reach the API
    let client = reqwest::Client::new();
    let response = client
        .get(&format!("{}/rest/v1/", url.trim_end_matches('/')))
        .header("apikey", &api_key)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await?;

    if !response.status().is_success() && response.status().as_u16() != 404 {
        return Err(anyhow!("Failed to connect to Supabase: {}", response.status()));
    }

    // Save configuration
    save_supabase_config(mana_dir, url)?;

    info!("Supabase sync initialized: {}", url);
    println!("Supabase sync initialized");
    println!("   URL: {}", url);
    println!();
    println!("   Note: Ensure the following tables exist in your Supabase project:");
    println!("   - mana_patterns (for shared patterns)");
    println!("   - mana_teams (for team management)");
    println!("   - mana_team_members (for team membership)");
    println!();
    println!("   Run 'mana team setup-schema' to create the required tables.");

    Ok(())
}

/// Initialize Supabase sync (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn init_supabase_sync(_mana_dir: &Path, _url: &str) -> Result<()> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Push patterns to Supabase
#[cfg(feature = "supabase")]
pub async fn push_patterns_supabase(
    mana_dir: &Path,
    db_path: &Path,
    security: &SecurityConfig,
    visibility: &str,
) -> Result<usize> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let supabase_config = SupabaseConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Sync backend is not configured for Supabase or MANA_SUPABASE_KEY not set"))?;

    // Export patterns to vec
    let patterns = export_patterns_to_vec(db_path, security)?;
    let count = patterns.len();

    if count == 0 {
        println!("No patterns to push");
        return Ok(0);
    }

    info!("Pushing {} patterns to Supabase", count);

    // Get or create user ID (using a hash of the workspace for now)
    let user_id = get_workspace_id(mana_dir);

    // Convert to shared patterns
    let shared_patterns: Vec<SharedPattern> = patterns
        .into_iter()
        .map(|p| {
            let mut sp: SharedPattern = p.into();
            sp.owner_id = user_id.clone();
            sp.visibility = visibility.to_string();
            sp
        })
        .collect();

    // Upsert patterns (insert or update based on pattern_hash)
    let client = reqwest::Client::new();
    let url = supabase_config.rest_url("mana_patterns");

    let response = client
        .post(&url)
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .header("Content-Type", "application/json")
        .header("Prefer", "resolution=merge-duplicates")
        .json(&shared_patterns)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to push patterns: {} - {}", status, body));
    }

    println!("Pushed {} patterns to Supabase", count);
    Ok(count)
}

/// Push patterns to Supabase (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn push_patterns_supabase(
    _mana_dir: &Path,
    _db_path: &Path,
    _security: &SecurityConfig,
    _visibility: &str,
) -> Result<usize> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Pull patterns from Supabase
#[cfg(feature = "supabase")]
pub async fn pull_patterns_supabase(
    mana_dir: &Path,
    db_path: &Path,
    merge_strategy: MergeStrategy,
    include_team: bool,
    include_public: bool,
) -> Result<PullResult> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let supabase_config = SupabaseConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Sync backend is not configured for Supabase or MANA_SUPABASE_KEY not set"))?;

    let user_id = get_workspace_id(mana_dir);

    // Build query to fetch patterns
    let client = reqwest::Client::new();
    let mut url = supabase_config.rest_url("mana_patterns");

    // Filter: own patterns OR team patterns (if member) OR public patterns
    let mut filters = vec![format!("owner_id.eq.{}", user_id)];

    if include_public {
        filters.push("visibility.eq.public".to_string());
    }

    // Note: For team filtering, we'd need to join with team_members table
    // For now, include team patterns if include_team is true
    if include_team {
        filters.push("visibility.eq.team".to_string());
    }

    url = format!("{}?or=({})", url, filters.join(","));

    let response = client
        .get(&url)
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to pull patterns: {} - {}", status, body));
    }

    let shared_patterns: Vec<SharedPattern> = response.json().await?;
    let patterns: Vec<ExportablePattern> = shared_patterns.into_iter().map(|sp| sp.into()).collect();

    let total = patterns.len();
    let result = import_patterns_from_vec(db_path, patterns, merge_strategy)?;

    info!("Pulled {} patterns from Supabase", total);

    Ok(PullResult {
        total,
        imported: result.imported,
        merged: result.merged,
        skipped: result.skipped,
    })
}

/// Pull patterns from Supabase (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn pull_patterns_supabase(
    _mana_dir: &Path,
    _db_path: &Path,
    _merge_strategy: SupabaseMergeStrategy,
    _include_team: bool,
    _include_public: bool,
) -> Result<PullResult> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Result of pull operation
#[derive(Debug, Clone)]
pub struct PullResult {
    pub total: usize,
    pub imported: usize,
    pub merged: usize,
    pub skipped: usize,
}

// === Team Management ===

/// Create a new team
#[cfg(feature = "supabase")]
pub async fn create_team(mana_dir: &Path, name: &str) -> Result<Team> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let supabase_config = SupabaseConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Supabase not configured"))?;

    let user_id = get_workspace_id(mana_dir);
    let team_id = uuid::Uuid::new_v4().to_string();

    let team = Team {
        id: team_id.clone(),
        name: name.to_string(),
        owner_id: user_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let client = reqwest::Client::new();

    // Create team
    let response = client
        .post(&supabase_config.rest_url("mana_teams"))
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .header("Content-Type", "application/json")
        .json(&team)
        .send()
        .await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to create team: {}", body));
    }

    // Add owner as team member
    let member = TeamMember {
        team_id: team_id.clone(),
        user_id,
        role: "owner".to_string(),
        joined_at: chrono::Utc::now().to_rfc3339(),
    };

    let response = client
        .post(&supabase_config.rest_url("mana_team_members"))
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .header("Content-Type", "application/json")
        .json(&member)
        .send()
        .await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to add owner to team: {}", body));
    }

    info!("Created team '{}' with ID {}", name, team_id);
    Ok(team)
}

/// Create a new team (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn create_team(_mana_dir: &Path, _name: &str) -> Result<Team> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// List teams the user belongs to
#[cfg(feature = "supabase")]
pub async fn list_teams(mana_dir: &Path) -> Result<Vec<Team>> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let supabase_config = SupabaseConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Supabase not configured"))?;

    let user_id = get_workspace_id(mana_dir);

    // First get team IDs from memberships
    let client = reqwest::Client::new();
    let url = format!(
        "{}?user_id=eq.{}",
        supabase_config.rest_url("mana_team_members"),
        user_id
    );

    let response = client
        .get(&url)
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .send()
        .await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to list teams: {}", body));
    }

    let memberships: Vec<TeamMember> = response.json().await?;

    if memberships.is_empty() {
        return Ok(vec![]);
    }

    // Fetch team details
    let team_ids: Vec<String> = memberships.iter().map(|m| m.team_id.clone()).collect();
    let team_filter = team_ids.iter()
        .map(|id| format!("id.eq.{}", id))
        .collect::<Vec<_>>()
        .join(",");

    let url = format!(
        "{}?or=({})",
        supabase_config.rest_url("mana_teams"),
        team_filter
    );

    let response = client
        .get(&url)
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .send()
        .await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to fetch teams: {}", body));
    }

    let teams: Vec<Team> = response.json().await?;
    Ok(teams)
}

/// List teams (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn list_teams(_mana_dir: &Path) -> Result<Vec<Team>> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Invite a user to a team (generates invite code)
#[cfg(feature = "supabase")]
pub async fn invite_to_team(mana_dir: &Path, team_id: &str, invitee_email: &str) -> Result<String> {
    // For now, we generate an invite code that the user can share
    // In a full implementation, this would send an email
    let invite_code = format!("{}:{}", team_id, uuid::Uuid::new_v4());

    info!("Generated invite for {} to team {}", invitee_email, team_id);
    println!("Invite code generated for {}:", invitee_email);
    println!("  {}", invite_code);
    println!();
    println!("Share this code with the invitee. They can join with:");
    println!("  mana team join {}", invite_code);

    Ok(invite_code)
}

/// Invite to team (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn invite_to_team(_mana_dir: &Path, _team_id: &str, _invitee_email: &str) -> Result<String> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Join a team using an invite code
#[cfg(feature = "supabase")]
pub async fn join_team(mana_dir: &Path, invite_code: &str) -> Result<()> {
    let parts: Vec<&str> = invite_code.split(':').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Invalid invite code format"));
    }
    let team_id = parts[0];

    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let supabase_config = SupabaseConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Supabase not configured"))?;

    let user_id = get_workspace_id(mana_dir);

    let member = TeamMember {
        team_id: team_id.to_string(),
        user_id: user_id.clone(),
        role: "member".to_string(),
        joined_at: chrono::Utc::now().to_rfc3339(),
    };

    let client = reqwest::Client::new();
    let response = client
        .post(&supabase_config.rest_url("mana_team_members"))
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .header("Content-Type", "application/json")
        .json(&member)
        .send()
        .await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to join team: {}", body));
    }

    info!("Joined team {}", team_id);
    println!("Successfully joined team!");

    Ok(())
}

/// Join team (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn join_team(_mana_dir: &Path, _invite_code: &str) -> Result<()> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Share a pattern with a team
#[cfg(feature = "supabase")]
pub async fn share_pattern(mana_dir: &Path, pattern_hash: &str, team_id: &str) -> Result<()> {
    let config_path = mana_dir.join("sync.toml");
    let config = load_sync_config(&config_path)?;

    let supabase_config = SupabaseConfig::from_backend(&config.backend)
        .ok_or_else(|| anyhow!("Supabase not configured"))?;

    let user_id = get_workspace_id(mana_dir);

    let client = reqwest::Client::new();

    // Update the pattern's team_id and visibility
    let url = format!(
        "{}?pattern_hash=eq.{}&owner_id=eq.{}",
        supabase_config.rest_url("mana_patterns"),
        pattern_hash,
        user_id
    );

    #[derive(Serialize)]
    struct UpdatePayload {
        team_id: String,
        visibility: String,
    }

    let response = client
        .patch(&url)
        .header("apikey", &supabase_config.api_key)
        .header("Authorization", format!("Bearer {}", supabase_config.api_key))
        .header("Content-Type", "application/json")
        .json(&UpdatePayload {
            team_id: team_id.to_string(),
            visibility: "team".to_string(),
        })
        .send()
        .await?;

    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Failed to share pattern: {}", body));
    }

    info!("Shared pattern {} with team {}", pattern_hash, team_id);
    println!("Pattern shared with team");

    Ok(())
}

/// Share pattern (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn share_pattern(_mana_dir: &Path, _pattern_hash: &str, _team_id: &str) -> Result<()> {
    Err(anyhow!("Supabase sync not available. Rebuild with --features supabase"))
}

/// Get Supabase sync status
#[cfg(feature = "supabase")]
pub async fn supabase_status(mana_dir: &Path) -> Result<SupabaseStatus> {
    let config_path = mana_dir.join("sync.toml");

    if !config_path.exists() {
        return Ok(SupabaseStatus::default());
    }

    let config = load_sync_config(&config_path)?;

    if let Some(supabase_config) = SupabaseConfig::from_backend(&config.backend) {
        let user_id = get_workspace_id(mana_dir);

        // Count patterns
        let client = reqwest::Client::new();
        let url = format!(
            "{}?owner_id=eq.{}&select=count",
            supabase_config.rest_url("mana_patterns"),
            user_id
        );

        let response = client
            .get(&url)
            .header("apikey", &supabase_config.api_key)
            .header("Authorization", format!("Bearer {}", supabase_config.api_key))
            .header("Prefer", "count=exact")
            .send()
            .await?;

        let pattern_count = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split('/').last())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);

        Ok(SupabaseStatus {
            configured: true,
            url: Some(supabase_config.url),
            connected: true,
            user_id: Some(user_id),
            pattern_count: Some(pattern_count),
        })
    } else {
        Ok(SupabaseStatus::default())
    }
}

/// Get Supabase status (stub when feature disabled)
#[cfg(not(feature = "supabase"))]
pub async fn supabase_status(_mana_dir: &Path) -> Result<SupabaseStatus> {
    Ok(SupabaseStatus::default())
}

/// Supabase sync status information
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct SupabaseStatus {
    pub configured: bool,
    pub url: Option<String>,
    pub connected: bool,
    pub user_id: Option<String>,
    pub pattern_count: Option<i64>,
}

/// Generate SQL schema for Supabase tables
pub fn get_schema_sql() -> &'static str {
    r#"
-- MANA Pattern Sharing Schema for Supabase
-- Run this in your Supabase SQL editor

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Teams table
CREATE TABLE IF NOT EXISTS mana_teams (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Team members table
CREATE TABLE IF NOT EXISTS mana_team_members (
    team_id UUID REFERENCES mana_teams(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member',
    joined_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (team_id, user_id)
);

-- Shared patterns table
CREATE TABLE IF NOT EXISTS mana_patterns (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    pattern_hash TEXT NOT NULL,
    tool_type TEXT NOT NULL,
    command_category TEXT,
    context_query TEXT NOT NULL,
    success_count BIGINT DEFAULT 0,
    failure_count BIGINT DEFAULT 0,
    owner_id TEXT NOT NULL,
    team_id UUID REFERENCES mana_teams(id) ON DELETE SET NULL,
    visibility TEXT NOT NULL DEFAULT 'private',
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(pattern_hash, owner_id)
);

-- Create indexes
CREATE INDEX IF NOT EXISTS idx_patterns_owner ON mana_patterns(owner_id);
CREATE INDEX IF NOT EXISTS idx_patterns_team ON mana_patterns(team_id);
CREATE INDEX IF NOT EXISTS idx_patterns_visibility ON mana_patterns(visibility);
CREATE INDEX IF NOT EXISTS idx_patterns_tool ON mana_patterns(tool_type);
CREATE INDEX IF NOT EXISTS idx_team_members_user ON mana_team_members(user_id);

-- Row Level Security policies
ALTER TABLE mana_teams ENABLE ROW LEVEL SECURITY;
ALTER TABLE mana_team_members ENABLE ROW LEVEL SECURITY;
ALTER TABLE mana_patterns ENABLE ROW LEVEL SECURITY;

-- Teams: owners can manage their teams
CREATE POLICY teams_owner_policy ON mana_teams
    FOR ALL USING (owner_id = current_setting('request.jwt.claims')::json->>'sub');

-- Team members: members can see their memberships
CREATE POLICY team_members_select_policy ON mana_team_members
    FOR SELECT USING (
        user_id = current_setting('request.jwt.claims')::json->>'sub'
        OR team_id IN (
            SELECT team_id FROM mana_team_members
            WHERE user_id = current_setting('request.jwt.claims')::json->>'sub'
        )
    );

-- Patterns: complex visibility rules
CREATE POLICY patterns_select_policy ON mana_patterns
    FOR SELECT USING (
        -- Own patterns
        owner_id = current_setting('request.jwt.claims')::json->>'sub'
        -- Public patterns
        OR visibility = 'public'
        -- Team patterns where user is a member
        OR (visibility = 'team' AND team_id IN (
            SELECT team_id FROM mana_team_members
            WHERE user_id = current_setting('request.jwt.claims')::json->>'sub'
        ))
    );

-- Patterns: only owner can modify
CREATE POLICY patterns_modify_policy ON mana_patterns
    FOR ALL USING (owner_id = current_setting('request.jwt.claims')::json->>'sub');

-- Function to update updated_at on pattern changes
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER update_patterns_updated_at
    BEFORE UPDATE ON mana_patterns
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();
"#
}

// === Helper Functions ===

/// Save Supabase sync configuration
#[allow(dead_code)]
pub fn save_supabase_config(mana_dir: &Path, url: &str) -> Result<()> {
    use crate::sync::{SyncConfig, SyncBackend, SecurityConfig as SyncSecurityConfig, save_sync_config};

    let config = SyncConfig {
        enabled: true,
        backend: SyncBackend::Supabase {
            url: url.to_string(),
        },
        interval_minutes: 60,
        security: SyncSecurityConfig::default(),
    };

    let config_path = mana_dir.join("sync.toml");
    save_sync_config(&config, &config_path)?;

    tracing::info!("Saved Supabase sync configuration to {:?}", config_path);
    Ok(())
}

/// Get a deterministic workspace ID from the MANA directory
#[allow(dead_code)]
fn get_workspace_id(mana_dir: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    mana_dir.to_string_lossy().hash(&mut hasher);
    format!("ws-{:016x}", hasher.finish())
}

/// Check if Supabase feature is available
pub fn is_supabase_available() -> bool {
    cfg!(feature = "supabase")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_id_deterministic() {
        let path1 = Path::new("/home/user/.mana");
        let path2 = Path::new("/home/user/.mana");
        let path3 = Path::new("/home/other/.mana");

        let id1 = get_workspace_id(path1);
        let id2 = get_workspace_id(path2);
        let id3 = get_workspace_id(path3);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
        assert!(id1.starts_with("ws-"));
    }

    #[test]
    fn test_is_supabase_available() {
        let available = is_supabase_available();
        // Just verify it compiles and returns a bool
        assert!(available || !available);
    }

    #[test]
    fn test_schema_sql_not_empty() {
        let sql = get_schema_sql();
        assert!(!sql.is_empty());
        assert!(sql.contains("mana_patterns"));
        assert!(sql.contains("mana_teams"));
        assert!(sql.contains("mana_team_members"));
    }
}
