//! Skill storage and consolidation
//!
//! Skills are higher-level abstractions over patterns. They group
//! similar patterns together with aggregated success rates.
//!
//! Example:
//!   Patterns:
//!     - "Add .js extension to ESM imports" (89% success)
//!     - "Fix TypeScript ESM import by adding .js" (92% success)
//!     - "ESM requires .js in relative imports" (87% success)
//!
//!   Skill:
//!     Name: "TypeScript ESM Import Fix"
//!     Description: "Add .js extension to relative imports for ESM compatibility"
//!     Aggregated Success Rate: 89%
//!     Pattern Count: 3

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

#[allow(unused_imports)]
use super::Pattern;
use crate::storage::calculate_similarity;

/// A skill consolidating multiple similar patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// Comma-separated pattern IDs
    pub pattern_ids: String,
    /// Aggregated success count from all patterns
    pub total_success: i64,
    /// Aggregated failure count from all patterns
    pub total_failure: i64,
    /// Number of patterns in this skill
    pub pattern_count: i64,
    /// Tool type (Edit, Bash, Task, etc.)
    pub tool_type: String,
    /// Command category (rs, npm, git, etc.)
    pub command_category: Option<String>,
}

impl Skill {
    /// Calculate success rate as a percentage
    #[allow(dead_code)]
    pub fn success_rate(&self) -> f64 {
        let total = self.total_success + self.total_failure;
        if total > 0 {
            (self.total_success as f64 / total as f64) * 100.0
        } else {
            50.0
        }
    }

    /// Calculate a score for ranking skills
    #[allow(dead_code)]
    pub fn score(&self) -> i64 {
        self.total_success - self.total_failure
    }
}

/// Skill store backed by SQLite
pub struct SkillStore {
    conn: Connection,
}

impl SkillStore {
    /// Open skill store at the given database path
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        // Check if we need to migrate the existing skills table
        let has_tool_type: bool = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('skills') WHERE name = 'tool_type'",
            [],
            |row| Ok(row.get::<_, i64>(0)? > 0),
        ).unwrap_or(false);

        if !has_tool_type {
            // Drop and recreate skills table with new schema
            conn.execute_batch(
                r#"
                DROP TABLE IF EXISTS skills;

                CREATE TABLE skills (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT UNIQUE NOT NULL,
                    description TEXT,
                    pattern_ids TEXT,
                    total_success INTEGER DEFAULT 0,
                    total_failure INTEGER DEFAULT 0,
                    pattern_count INTEGER DEFAULT 0,
                    tool_type TEXT,
                    command_category TEXT,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
                );

                CREATE INDEX IF NOT EXISTS idx_skills_tool_type ON skills(tool_type);
                CREATE INDEX IF NOT EXISTS idx_skills_category ON skills(command_category);
                "#,
            )?;
        }

        Ok(Self { conn })
    }

    /// Open skill store in read-only mode
    #[allow(dead_code)]
    pub fn open_readonly(db_path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { conn })
    }

    /// Insert or update a skill
    pub fn upsert(&self, skill: &Skill) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO skills (name, description, pattern_ids, total_success, total_failure, pattern_count, tool_type, command_category, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, CURRENT_TIMESTAMP)
            ON CONFLICT(name) DO UPDATE SET
                description = excluded.description,
                pattern_ids = excluded.pattern_ids,
                total_success = excluded.total_success,
                total_failure = excluded.total_failure,
                pattern_count = excluded.pattern_count,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![
                skill.name,
                skill.description,
                skill.pattern_ids,
                skill.total_success,
                skill.total_failure,
                skill.pattern_count,
                skill.tool_type,
                skill.command_category,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get skills by tool type
    #[allow(dead_code)]
    pub fn get_by_tool(&self, tool_type: &str, limit: usize) -> Result<Vec<Skill>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, name, description, pattern_ids, total_success, total_failure, pattern_count, tool_type, command_category
            FROM skills
            WHERE tool_type = ?1
            ORDER BY (total_success - total_failure) DESC
            LIMIT ?2
            "#,
        )?;

        let skills = stmt.query_map(params![tool_type, limit as i64], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                pattern_ids: row.get(3)?,
                total_success: row.get(4)?,
                total_failure: row.get(5)?,
                pattern_count: row.get(6)?,
                tool_type: row.get(7)?,
                command_category: row.get(8)?,
            })
        })?;

        skills.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all skills
    #[allow(dead_code)]
    pub fn get_all(&self, limit: usize) -> Result<Vec<Skill>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, name, description, pattern_ids, total_success, total_failure, pattern_count, tool_type, command_category
            FROM skills
            ORDER BY (total_success - total_failure) DESC
            LIMIT ?1
            "#,
        )?;

        let skills = stmt.query_map(params![limit as i64], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                pattern_ids: row.get(3)?,
                total_success: row.get(4)?,
                total_failure: row.get(5)?,
                pattern_count: row.get(6)?,
                tool_type: row.get(7)?,
                command_category: row.get(8)?,
            })
        })?;

        skills.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get skill count
    #[allow(dead_code)]
    pub fn count(&self) -> Result<i64> {
        self.conn.query_row("SELECT COUNT(*) FROM skills", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// Delete a skill by ID
    #[allow(dead_code)]
    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM skills WHERE id = ?", params![id])?;
        Ok(())
    }

    /// Clear all skills
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM skills", [])?;
        Ok(())
    }
}

/// Consolidate patterns into skills
///
/// Groups similar patterns by tool type and command category,
/// then creates skills from clusters of similar patterns.
pub fn consolidate_patterns_to_skills(db_path: &Path) -> Result<usize> {
    let conn = Connection::open(db_path)?;

    // Get all patterns grouped by tool type and command category
    let mut stmt = conn.prepare(
        r#"
        SELECT id, tool_type, command_category, context_query, success_count, failure_count
        FROM patterns
        WHERE tool_type != 'failure'
        ORDER BY tool_type, command_category, (success_count - failure_count) DESC
        "#,
    )?;

    let patterns: Vec<(i64, String, Option<String>, String, i64, i64)> = stmt
        .query_map([], |row| {
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

    if patterns.is_empty() {
        debug!("No patterns to consolidate");
        return Ok(0);
    }

    // Group patterns by tool_type and command_category
    let mut groups: std::collections::HashMap<(String, Option<String>), Vec<(i64, String, i64, i64)>> =
        std::collections::HashMap::new();

    for (id, tool_type, category, context, success, failure) in patterns {
        groups
            .entry((tool_type, category))
            .or_default()
            .push((id, context, success, failure));
    }

    let skill_store = SkillStore::open(db_path)?;
    // Clear existing skills before reconsolidating
    skill_store.clear()?;

    let mut skills_created = 0;

    for ((tool_type, category), group_patterns) in groups {
        // Cluster similar patterns within each group
        let skills = cluster_patterns(&tool_type, &category, &group_patterns);

        for skill in skills {
            match skill_store.upsert(&skill) {
                Ok(_) => skills_created += 1,
                Err(e) => debug!("Failed to create skill: {}", e),
            }
        }
    }

    info!("Consolidated patterns into {} skills", skills_created);
    Ok(skills_created)
}

/// Cluster similar patterns into skills
///
/// Uses a simple greedy clustering algorithm:
/// 1. Start with the highest-scoring pattern as a cluster seed
/// 2. Add patterns with similarity > threshold to the cluster
/// 3. Repeat for remaining patterns
/// 4. Create skills from clusters with 2+ patterns
/// 5. Also create skills from high-value single patterns (score >= 100)
fn cluster_patterns(
    tool_type: &str,
    category: &Option<String>,
    patterns: &[(i64, String, i64, i64)],
) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut used: std::collections::HashSet<i64> = std::collections::HashSet::new();

    // Minimum patterns to form a multi-pattern skill
    const MIN_CLUSTER_SIZE: usize = 2;
    // Similarity threshold for clustering (lowered for more matches)
    const CLUSTER_SIMILARITY: f64 = 0.5;
    // Score threshold for promoting single patterns to skills
    const HIGH_VALUE_SCORE: i64 = 100;

    for (seed_id, seed_context, seed_success, seed_failure) in patterns {
        if used.contains(seed_id) {
            continue;
        }

        // Start a new cluster with this pattern
        let mut cluster: Vec<(i64, &str, i64, i64)> = vec![(*seed_id, seed_context, *seed_success, *seed_failure)];
        used.insert(*seed_id);

        // Find similar patterns
        for (id, context, success, failure) in patterns {
            if used.contains(id) {
                continue;
            }

            let similarity = calculate_similarity(seed_context, context);
            if similarity >= CLUSTER_SIMILARITY {
                cluster.push((*id, context, *success, *failure));
                used.insert(*id);
            }
        }

        // Create skill if cluster is large enough OR if it's a high-value single pattern
        if cluster.len() >= MIN_CLUSTER_SIZE {
            let skill = create_skill_from_cluster(tool_type, category, &cluster);
            skills.push(skill);
        } else if cluster.len() == 1 {
            let (_, _, success, failure) = cluster[0];
            let score = success - failure;
            if score >= HIGH_VALUE_SCORE {
                // Promote high-value single patterns to skills
                let skill = create_skill_from_cluster(tool_type, category, &cluster);
                skills.push(skill);
            }
        }
    }

    skills
}

/// Create a skill from a cluster of similar patterns
fn create_skill_from_cluster(
    tool_type: &str,
    category: &Option<String>,
    cluster: &[(i64, &str, i64, i64)],
) -> Skill {
    // Use the highest-scoring pattern's context as the base
    let best_pattern = cluster
        .iter()
        .max_by_key(|(_, _, s, f)| s - f)
        .expect("Cluster should not be empty");

    // Extract a concise name from the context
    let name = extract_skill_name(tool_type, category.as_deref(), best_pattern.1);

    // Extract a description from the context
    let description = extract_skill_description(best_pattern.1);

    // Aggregate success/failure counts
    let total_success: i64 = cluster.iter().map(|(_, _, s, _)| s).sum();
    let total_failure: i64 = cluster.iter().map(|(_, _, _, f)| f).sum();

    // Build pattern IDs string
    let pattern_ids: String = cluster
        .iter()
        .map(|(id, _, _, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Skill {
        id: 0,
        name,
        description,
        pattern_ids,
        total_success,
        total_failure,
        pattern_count: cluster.len() as i64,
        tool_type: tool_type.to_string(),
        command_category: category.clone(),
    }
}

/// Extract a concise skill name from pattern context
fn extract_skill_name(tool_type: &str, category: Option<&str>, context: &str) -> String {
    // Try to extract from Task/Approach lines
    for line in context.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("Task:") {
            let task = trimmed.trim_start_matches("Task:").trim();
            if !task.is_empty() && task.len() > 5 {
                let cat_suffix = category.map(|c| format!(" ({})", c)).unwrap_or_default();
                return format!("{}{} - {}", tool_type, cat_suffix, truncate_str(task, 40));
            }
        }
    }

    // Fallback: use tool type and category
    match category {
        Some(cat) => format!("{} - {} patterns", tool_type, cat),
        None => format!("{} patterns", tool_type),
    }
}

/// Extract a skill description from pattern context
fn extract_skill_description(context: &str) -> String {
    // Try to extract from Approach line
    for line in context.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("Approach:") {
            let approach = trimmed.trim_start_matches("Approach:").trim();
            if !approach.is_empty() && approach.len() > 10 {
                return truncate_str(approach, 200).to_string();
            }
        }
    }

    // Fallback: use first meaningful line
    for line in context.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with("Task:")
            && !trimmed.starts_with("Outcome:")
            && trimmed.len() > 10
        {
            return truncate_str(trimmed, 200).to_string();
        }
    }

    context.lines().next().unwrap_or("").to_string()
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_db() -> Result<(NamedTempFile, Connection)> {
        let temp_file = NamedTempFile::new()?;
        let conn = Connection::open(temp_file.path())?;

        conn.execute_batch(
            r#"
            CREATE TABLE patterns (
                id INTEGER PRIMARY KEY,
                tool_type TEXT,
                command_category TEXT,
                context_query TEXT,
                success_count INTEGER,
                failure_count INTEGER
            );

            CREATE TABLE skills (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT UNIQUE NOT NULL,
                description TEXT,
                pattern_ids TEXT,
                total_success INTEGER DEFAULT 0,
                total_failure INTEGER DEFAULT 0,
                pattern_count INTEGER DEFAULT 0,
                tool_type TEXT,
                command_category TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )?;

        Ok((temp_file, conn))
    }

    #[test]
    fn test_skill_store_operations() -> Result<()> {
        let (temp_file, _) = create_test_db()?;
        let store = SkillStore::open(temp_file.path())?;

        let skill = Skill {
            id: 0,
            name: "Test Skill".to_string(),
            description: "A test skill".to_string(),
            pattern_ids: "1,2,3".to_string(),
            total_success: 10,
            total_failure: 2,
            pattern_count: 3,
            tool_type: "Bash".to_string(),
            command_category: Some("cargo".to_string()),
        };

        store.upsert(&skill)?;

        let count = store.count()?;
        assert_eq!(count, 1);

        let skills = store.get_all(10)?;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "Test Skill");
        assert_eq!(skills[0].pattern_count, 3);

        Ok(())
    }

    #[test]
    fn test_skill_success_rate() {
        let skill = Skill {
            id: 1,
            name: "Test".to_string(),
            description: "Test".to_string(),
            pattern_ids: "1,2".to_string(),
            total_success: 80,
            total_failure: 20,
            pattern_count: 2,
            tool_type: "Edit".to_string(),
            command_category: None,
        };

        assert!((skill.success_rate() - 80.0).abs() < 0.01);
    }

    #[test]
    fn test_consolidation() -> Result<()> {
        let (temp_file, conn) = create_test_db()?;

        // Insert similar patterns
        conn.execute(
            "INSERT INTO patterns (id, tool_type, command_category, context_query, success_count, failure_count) VALUES (1, 'Bash', 'cargo', 'Task: Build project\nApproach: cargo build --release', 10, 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO patterns (id, tool_type, command_category, context_query, success_count, failure_count) VALUES (2, 'Bash', 'cargo', 'Task: Build project\nApproach: cargo build for release', 8, 0)",
            [],
        )?;
        conn.execute(
            "INSERT INTO patterns (id, tool_type, command_category, context_query, success_count, failure_count) VALUES (3, 'Edit', 'rs', 'Task: Fix type error\nApproach: Edit rust file', 5, 1)",
            [],
        )?;

        let skills_created = consolidate_patterns_to_skills(temp_file.path())?;

        // Should create 1 skill from the 2 similar Bash patterns
        // The Edit pattern is alone, so no skill for it
        assert!(skills_created >= 1);

        let store = SkillStore::open(temp_file.path())?;
        let skills = store.get_all(10)?;

        // Should have at least one Bash skill
        let bash_skills: Vec<_> = skills.iter().filter(|s| s.tool_type == "Bash").collect();
        assert_eq!(bash_skills.len(), 1);
        assert_eq!(bash_skills[0].pattern_count, 2);
        assert_eq!(bash_skills[0].total_success, 18);

        Ok(())
    }
}
