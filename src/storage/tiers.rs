//! Hierarchical memory tier system
//!
//! 4-tier hierarchy with automatic fallback search:
//! - Global: Shared across all projects (universal truths)
//! - Domain: Domain-specific (infrastructure/k8s, tools/websearch, etc.)
//! - Project: Project-specific memories
//! - Agent: Session-specific (current Claude Code instance)

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Memory tier levels in the hierarchy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryTier {
    /// Global tier: Universal truths shared across all projects and domains
    Global,
    /// Domain tier: Domain-specific knowledge (e.g., infrastructure/k8s, tools/websearch)
    Domain,
    /// Project tier: Project-specific memories (e.g., mana, my-app)
    Project,
    /// Agent tier: Session-specific for current Claude Code instance
    Agent,
}

impl MemoryTier {
    /// Get the numeric priority level (lower = higher priority)
    /// Used for sorting search results by tier
    pub fn priority(&self) -> u8 {
        match self {
            MemoryTier::Agent => 0,
            MemoryTier::Project => 1,
            MemoryTier::Domain => 2,
            MemoryTier::Global => 3,
        }
    }

    /// Parse from string representation
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "global" => Ok(MemoryTier::Global),
            "domain" => Ok(MemoryTier::Domain),
            "project" => Ok(MemoryTier::Project),
            "agent" => Ok(MemoryTier::Agent),
            _ => Err(anyhow!("Invalid memory tier: {}", s)),
        }
    }

    /// Convert to string representation
    pub fn to_string(&self) -> &'static str {
        match self {
            MemoryTier::Global => "global",
            MemoryTier::Domain => "domain",
            MemoryTier::Project => "project",
            MemoryTier::Agent => "agent",
        }
    }
}

/// Complete path to a memory tier including context
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TierPath {
    /// The tier level
    pub tier: MemoryTier,
    /// Domain context (e.g., "infrastructure/k8s", "tools/websearch")
    pub domain: Option<String>,
    /// Project context (e.g., "mana", "my-app")
    pub project: Option<String>,
    /// Agent session identifier (e.g., session UUID)
    pub agent_session: Option<String>,
}

impl TierPath {
    /// Create a global tier path
    pub fn global() -> Self {
        Self {
            tier: MemoryTier::Global,
            domain: None,
            project: None,
            agent_session: None,
        }
    }

    /// Create a domain tier path
    pub fn domain(domain: &str) -> Self {
        Self {
            tier: MemoryTier::Domain,
            domain: Some(domain.to_string()),
            project: None,
            agent_session: None,
        }
    }

    /// Create a project tier path
    pub fn project(project: &str) -> Self {
        Self {
            tier: MemoryTier::Project,
            domain: None,
            project: Some(project.to_string()),
            agent_session: None,
        }
    }

    /// Create a project tier path with domain context
    pub fn project_with_domain(project: &str, domain: &str) -> Self {
        Self {
            tier: MemoryTier::Project,
            domain: Some(domain.to_string()),
            project: Some(project.to_string()),
            agent_session: None,
        }
    }

    /// Create an agent tier path
    pub fn agent(session: &str) -> Self {
        Self {
            tier: MemoryTier::Agent,
            domain: None,
            project: None,
            agent_session: Some(session.to_string()),
        }
    }

    /// Create an agent tier path with full context
    pub fn agent_with_context(session: &str, project: Option<&str>, domain: Option<&str>) -> Self {
        Self {
            tier: MemoryTier::Agent,
            domain: domain.map(|s| s.to_string()),
            project: project.map(|s| s.to_string()),
            agent_session: Some(session.to_string()),
        }
    }

    /// Get the fallback search order for this tier
    /// Returns a list of tier paths to search in priority order
    ///
    /// # Examples
    ///
    /// For Agent tier with project "mana" and domain "infrastructure/k8s":
    /// 1. Agent[session=abc, project=mana, domain=infrastructure/k8s]
    /// 2. Project[project=mana, domain=infrastructure/k8s]
    /// 3. Domain[domain=infrastructure/k8s]
    /// 4. Global[]
    ///
    /// For Project tier with project "mana":
    /// 1. Project[project=mana]
    /// 2. Global[]
    pub fn search_fallback_order(&self) -> Vec<TierPath> {
        let mut fallbacks = vec![self.clone()];

        match self.tier {
            MemoryTier::Agent => {
                // Agent -> Project -> Domain -> Global
                if let Some(ref project) = self.project {
                    if let Some(ref domain) = self.domain {
                        fallbacks.push(TierPath::project_with_domain(project, domain));
                    } else {
                        fallbacks.push(TierPath::project(project));
                    }
                }
                if let Some(ref domain) = self.domain {
                    fallbacks.push(TierPath::domain(domain));
                }
                fallbacks.push(TierPath::global());
            }
            MemoryTier::Project => {
                // Project -> Domain (if specified) -> Global
                if let Some(ref domain) = self.domain {
                    fallbacks.push(TierPath::domain(domain));
                }
                fallbacks.push(TierPath::global());
            }
            MemoryTier::Domain => {
                // Domain -> Global
                fallbacks.push(TierPath::global());
            }
            MemoryTier::Global => {
                // Global has no fallback
            }
        }

        fallbacks
    }

    /// Parse from string like "global" or "domain/infrastructure" or "project/mana"
    ///
    /// # Format
    ///
    /// - "global" -> Global tier
    /// - "domain/infrastructure/k8s" -> Domain tier with domain "infrastructure/k8s"
    /// - "project/mana" -> Project tier with project "mana"
    /// - "agent/session-id" -> Agent tier with session "session-id"
    /// - "project/mana@infrastructure/k8s" -> Project tier with domain context
    pub fn from_str(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('/').collect();

        if parts.is_empty() {
            return Err(anyhow!("Empty tier path"));
        }

        let tier = MemoryTier::from_str(parts[0])?;

        match tier {
            MemoryTier::Global => {
                if parts.len() > 1 {
                    return Err(anyhow!("Global tier should not have additional path components"));
                }
                Ok(TierPath::global())
            }
            MemoryTier::Domain => {
                if parts.len() < 2 {
                    return Err(anyhow!("Domain tier requires domain path (e.g., domain/infrastructure/k8s)"));
                }
                let domain = parts[1..].join("/");
                Ok(TierPath::domain(&domain))
            }
            MemoryTier::Project => {
                if parts.len() < 2 {
                    return Err(anyhow!("Project tier requires project name (e.g., project/mana)"));
                }

                // Check for domain context using @ separator
                let project_part = parts[1];
                if let Some((project, domain)) = project_part.split_once('@') {
                    Ok(TierPath::project_with_domain(project, domain))
                } else {
                    Ok(TierPath::project(project_part))
                }
            }
            MemoryTier::Agent => {
                if parts.len() < 2 {
                    return Err(anyhow!("Agent tier requires session ID (e.g., agent/session-id)"));
                }
                Ok(TierPath::agent(parts[1]))
            }
        }
    }

    /// Convert to database-storable path string
    ///
    /// # Format
    ///
    /// Uses a consistent format for storage in SQLite:
    /// - Global: "global"
    /// - Domain: "domain/infrastructure/k8s"
    /// - Project: "project/mana" or "project/mana@infrastructure/k8s"
    /// - Agent: "agent/session-id"
    pub fn to_path_string(&self) -> String {
        match self.tier {
            MemoryTier::Global => "global".to_string(),
            MemoryTier::Domain => {
                format!("domain/{}", self.domain.as_ref().unwrap_or(&"unknown".to_string()))
            }
            MemoryTier::Project => {
                let project = self.project.as_ref().unwrap_or(&"unknown".to_string());
                if let Some(ref domain) = self.domain {
                    format!("project/{}@{}", project, domain)
                } else {
                    format!("project/{}", project)
                }
            }
            MemoryTier::Agent => {
                format!("agent/{}", self.agent_session.as_ref().unwrap_or(&"unknown".to_string()))
            }
        }
    }

    /// Get a display-friendly description of this tier path
    pub fn description(&self) -> String {
        match self.tier {
            MemoryTier::Global => "Global (universal)".to_string(),
            MemoryTier::Domain => {
                format!("Domain: {}", self.domain.as_ref().unwrap_or(&"unknown".to_string()))
            }
            MemoryTier::Project => {
                let project = self.project.as_ref().unwrap_or(&"unknown".to_string());
                if let Some(ref domain) = self.domain {
                    format!("Project: {} ({})", project, domain)
                } else {
                    format!("Project: {}", project)
                }
            }
            MemoryTier::Agent => {
                let session = self.agent_session.as_ref().unwrap_or(&"unknown".to_string());
                format!("Agent session: {}", session)
            }
        }
    }

    /// Check if this tier path matches a search pattern
    /// Used for filtering patterns by tier during queries
    pub fn matches(&self, other: &TierPath) -> bool {
        // Exact tier must match
        if self.tier != other.tier {
            return false;
        }

        // Check context fields match (None matches anything)
        match self.tier {
            MemoryTier::Global => true,
            MemoryTier::Domain => {
                self.domain == other.domain || other.domain.is_none()
            }
            MemoryTier::Project => {
                let project_matches = self.project == other.project || other.project.is_none();
                let domain_matches = self.domain == other.domain || other.domain.is_none();
                project_matches && domain_matches
            }
            MemoryTier::Agent => {
                self.agent_session == other.agent_session || other.agent_session.is_none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_priority() {
        assert_eq!(MemoryTier::Agent.priority(), 0);
        assert_eq!(MemoryTier::Project.priority(), 1);
        assert_eq!(MemoryTier::Domain.priority(), 2);
        assert_eq!(MemoryTier::Global.priority(), 3);
    }

    #[test]
    fn test_tier_from_str() {
        assert_eq!(MemoryTier::from_str("global").unwrap(), MemoryTier::Global);
        assert_eq!(MemoryTier::from_str("domain").unwrap(), MemoryTier::Domain);
        assert_eq!(MemoryTier::from_str("project").unwrap(), MemoryTier::Project);
        assert_eq!(MemoryTier::from_str("agent").unwrap(), MemoryTier::Agent);
        assert!(MemoryTier::from_str("invalid").is_err());
    }

    #[test]
    fn test_tier_path_constructors() {
        let global = TierPath::global();
        assert_eq!(global.tier, MemoryTier::Global);
        assert_eq!(global.domain, None);
        assert_eq!(global.project, None);

        let domain = TierPath::domain("infrastructure/k8s");
        assert_eq!(domain.tier, MemoryTier::Domain);
        assert_eq!(domain.domain, Some("infrastructure/k8s".to_string()));

        let project = TierPath::project("mana");
        assert_eq!(project.tier, MemoryTier::Project);
        assert_eq!(project.project, Some("mana".to_string()));

        let agent = TierPath::agent("session-123");
        assert_eq!(agent.tier, MemoryTier::Agent);
        assert_eq!(agent.agent_session, Some("session-123".to_string()));
    }

    #[test]
    fn test_tier_path_from_str() {
        let global = TierPath::from_str("global").unwrap();
        assert_eq!(global.tier, MemoryTier::Global);

        let domain = TierPath::from_str("domain/infrastructure/k8s").unwrap();
        assert_eq!(domain.tier, MemoryTier::Domain);
        assert_eq!(domain.domain, Some("infrastructure/k8s".to_string()));

        let project = TierPath::from_str("project/mana").unwrap();
        assert_eq!(project.tier, MemoryTier::Project);
        assert_eq!(project.project, Some("mana".to_string()));

        let project_with_domain = TierPath::from_str("project/mana@infrastructure/k8s").unwrap();
        assert_eq!(project_with_domain.tier, MemoryTier::Project);
        assert_eq!(project_with_domain.project, Some("mana".to_string()));
        assert_eq!(project_with_domain.domain, Some("infrastructure/k8s".to_string()));

        let agent = TierPath::from_str("agent/session-123").unwrap();
        assert_eq!(agent.tier, MemoryTier::Agent);
        assert_eq!(agent.agent_session, Some("session-123".to_string()));
    }

    #[test]
    fn test_tier_path_to_string() {
        assert_eq!(TierPath::global().to_path_string(), "global");
        assert_eq!(TierPath::domain("infrastructure/k8s").to_path_string(), "domain/infrastructure/k8s");
        assert_eq!(TierPath::project("mana").to_path_string(), "project/mana");
        assert_eq!(TierPath::project_with_domain("mana", "infrastructure/k8s").to_path_string(), "project/mana@infrastructure/k8s");
        assert_eq!(TierPath::agent("session-123").to_path_string(), "agent/session-123");
    }

    #[test]
    fn test_fallback_order_agent() {
        let agent = TierPath::agent_with_context(
            "session-123",
            Some("mana"),
            Some("infrastructure/k8s")
        );

        let fallbacks = agent.search_fallback_order();
        assert_eq!(fallbacks.len(), 4);
        assert_eq!(fallbacks[0].tier, MemoryTier::Agent);
        assert_eq!(fallbacks[1].tier, MemoryTier::Project);
        assert_eq!(fallbacks[2].tier, MemoryTier::Domain);
        assert_eq!(fallbacks[3].tier, MemoryTier::Global);
    }

    #[test]
    fn test_fallback_order_project() {
        let project = TierPath::project_with_domain("mana", "infrastructure/k8s");

        let fallbacks = project.search_fallback_order();
        assert_eq!(fallbacks.len(), 3);
        assert_eq!(fallbacks[0].tier, MemoryTier::Project);
        assert_eq!(fallbacks[1].tier, MemoryTier::Domain);
        assert_eq!(fallbacks[2].tier, MemoryTier::Global);
    }

    #[test]
    fn test_fallback_order_domain() {
        let domain = TierPath::domain("infrastructure/k8s");

        let fallbacks = domain.search_fallback_order();
        assert_eq!(fallbacks.len(), 2);
        assert_eq!(fallbacks[0].tier, MemoryTier::Domain);
        assert_eq!(fallbacks[1].tier, MemoryTier::Global);
    }

    #[test]
    fn test_fallback_order_global() {
        let global = TierPath::global();

        let fallbacks = global.search_fallback_order();
        assert_eq!(fallbacks.len(), 1);
        assert_eq!(fallbacks[0].tier, MemoryTier::Global);
    }

    #[test]
    fn test_tier_path_matches() {
        let project_mana = TierPath::project("mana");
        let project_other = TierPath::project("other");
        let global = TierPath::global();

        assert!(project_mana.matches(&project_mana));
        assert!(!project_mana.matches(&project_other));
        assert!(!project_mana.matches(&global));
    }
}
