//! Embedded default workflows that ship with lok.
//!
//! These provide sensible defaults that users can override at:
//! - Project level: `.lok/workflows/{name}.toml`
//! - User level: `~/.config/lok/workflows/{name}.toml`
//!
//! When no override exists, lok uses the embedded version.

use crate::workflow::Workflow;
use std::collections::HashMap;

/// Built-in workflow definitions embedded at compile time
pub struct EmbeddedWorkflows {
    workflows: HashMap<&'static str, &'static str>,
}

impl EmbeddedWorkflows {
    /// Create a new registry of embedded workflows
    pub fn new() -> Self {
        let mut workflows = HashMap::new();

        // diff - review git changes
        workflows.insert("diff", include_str!("diff.toml"));

        // explain - explain codebase structure
        workflows.insert("explain", include_str!("explain.toml"));

        // audit - security audit
        workflows.insert("audit", include_str!("audit.toml"));

        // hunt - bug hunting
        workflows.insert("hunt", include_str!("hunt.toml"));

        Self { workflows }
    }

    /// Get an embedded workflow by name
    pub fn get(&self, name: &str) -> Option<&'static str> {
        self.workflows.get(name).copied()
    }

    /// Parse an embedded workflow into a Workflow struct
    pub fn parse(&self, name: &str) -> Option<anyhow::Result<Workflow>> {
        self.get(name).map(|toml| {
            toml::from_str(toml)
                .map_err(|e| anyhow::anyhow!("Failed to parse embedded workflow '{}': {}", name, e))
        })
    }

    /// List all embedded workflow names
    pub fn list(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.workflows.keys().copied().collect();
        names.sort();
        names
    }

    /// Check if an embedded workflow exists
    #[cfg(test)]
    pub fn contains(&self, name: &str) -> bool {
        self.workflows.contains_key(name)
    }
}

impl Default for EmbeddedWorkflows {
    fn default() -> Self {
        Self::new()
    }
}

/// Global registry instance
pub static EMBEDDED: std::sync::LazyLock<EmbeddedWorkflows> =
    std::sync::LazyLock::new(EmbeddedWorkflows::new);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_workflows_exist() {
        let embedded = EmbeddedWorkflows::new();
        assert!(embedded.contains("diff"));
        assert!(embedded.contains("explain"));
        assert!(embedded.contains("audit"));
        assert!(embedded.contains("hunt"));
    }

    #[test]
    fn test_embedded_workflows_parse() {
        let embedded = EmbeddedWorkflows::new();
        for name in embedded.list() {
            let result = embedded.parse(name);
            assert!(result.is_some(), "Workflow '{}' should exist", name);
            let parse_result = result.unwrap();
            if let Err(ref e) = parse_result {
                eprintln!("Failed to parse '{}': {}", name, e);
            }
            assert!(
                parse_result.is_ok(),
                "Workflow '{}' should parse successfully",
                name
            );
        }
    }
}
