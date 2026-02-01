use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub conductor: ConductorConfig,
    #[serde(default)]
    pub cache: crate::cache::CacheConfig,
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,
    #[serde(default)]
    pub tasks: HashMap<String, TaskConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Defaults {
    #[serde(default = "default_parallel")]
    pub parallel: bool,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Optional wrapper for shell commands (e.g., "nix-shell --run '{cmd}'" or "docker exec dev sh -c '{cmd}'")
    /// The {cmd} placeholder will be replaced with the actual command
    #[serde(default)]
    pub command_wrapper: Option<String>,
}

fn default_parallel() -> bool {
    true
}

fn default_timeout() -> u64 {
    300
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            parallel: default_parallel(),
            timeout: default_timeout(),
            command_wrapper: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ConductorConfig {
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
}

fn default_max_rounds() -> usize {
    5
}

fn default_max_tokens() -> usize {
    4096
}

impl Default for ConductorConfig {
    fn default() -> Self {
        Self {
            max_rounds: default_max_rounds(),
            max_tokens: default_max_tokens(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct BackendConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub skip_lines: usize,
    pub api_key_env: Option<String>,
    pub model: Option<String>,
    /// Per-backend timeout in seconds (overrides defaults.timeout)
    pub timeout: Option<u64>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskConfig {
    pub description: Option<String>,
    #[serde(default)]
    pub backends: Vec<String>,
    #[serde(default)]
    pub prompts: Vec<TaskPrompt>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TaskPrompt {
    pub name: String,
    pub prompt: String,
}

impl Default for Config {
    fn default() -> Self {
        let mut backends = HashMap::new();

        backends.insert(
            "codex".to_string(),
            BackendConfig {
                enabled: true,
                command: Some("codex".to_string()),
                args: vec![
                    "exec".to_string(),
                    "--json".to_string(),
                    "-s".to_string(),
                    "read-only".to_string(),
                ],
                skip_lines: 0,
                api_key_env: None,
                model: None,
                timeout: None,
            },
        );

        backends.insert(
            "gemini".to_string(),
            BackendConfig {
                enabled: true,
                command: Some("npx".to_string()),
                args: vec!["@google/gemini-cli".to_string()],
                skip_lines: 1,
                api_key_env: None,
                model: None,
                timeout: Some(600), // Gemini goes agentic, needs more time
            },
        );

        backends.insert(
            "claude".to_string(),
            BackendConfig {
                enabled: true,
                command: Some("claude".to_string()), // CLI mode by default (Claude Code)
                args: vec![],
                skip_lines: 0,
                api_key_env: None,
                model: None, // Uses Claude Code's default model
                timeout: None,
            },
        );

        backends.insert(
            "ollama".to_string(),
            BackendConfig {
                enabled: true,
                command: Some("http://localhost:11434".to_string()), // Base URL
                args: vec![],
                skip_lines: 0,
                api_key_env: None,
                model: Some("llama3.2".to_string()), // Default model
                timeout: None,
            },
        );

        let mut tasks = HashMap::new();

        tasks.insert(
            "hunt".to_string(),
            TaskConfig {
                description: Some("Find bugs and code issues".to_string()),
                backends: vec!["codex".to_string()],
                prompts: vec![
                    TaskPrompt {
                        name: "errors".to_string(),
                        prompt: "Find error handling problems in this codebase. Look for: unchecked errors, panics/crashes waiting to happen, missing input validation, swallowed exceptions. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                    TaskPrompt {
                        name: "perf".to_string(),
                        prompt: "Find performance issues in this codebase. Look for: inefficient loops, unnecessary allocations, blocking calls in async code, O(n^2) patterns, missing caching opportunities. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                    TaskPrompt {
                        name: "dead-code".to_string(),
                        prompt: "Find unused or dead code in this codebase. Look for: unreachable code, unused functions/methods, redundant conditions, commented-out code that should be removed. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                ],
            },
        );

        tasks.insert(
            "audit".to_string(),
            TaskConfig {
                description: Some("Security audit".to_string()),
                backends: vec!["gemini".to_string()],
                prompts: vec![
                    TaskPrompt {
                        name: "injection".to_string(),
                        prompt: "Search for injection vulnerabilities (SQL, command, code injection). Look for: unsanitized user input in queries/commands, string interpolation in SQL, shell command construction. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                    TaskPrompt {
                        name: "auth".to_string(),
                        prompt: "Search for authentication/authorization issues. Look for: missing auth checks, privilege escalation, insecure token handling, hardcoded credentials. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                    TaskPrompt {
                        name: "secrets".to_string(),
                        prompt: "Search for exposed secrets and sensitive data. Look for: hardcoded API keys, passwords in code, secrets in logs, sensitive data in error messages. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                ],
            },
        );

        Self {
            defaults: Defaults::default(),
            conductor: ConductorConfig::default(),
            cache: crate::cache::CacheConfig::default(),
            backends,
            tasks,
        }
    }
}

pub fn load_config(path: Option<&Path>) -> Result<Config> {
    // Try explicit path first
    if let Some(p) = path {
        let content = fs::read_to_string(p)
            .with_context(|| format!("Failed to read config file: {}", p.display()))?;
        return toml::from_str(&content).context("Failed to parse config file");
    }

    // Try current directory
    if let Ok(content) = fs::read_to_string("lok.toml") {
        return toml::from_str(&content).context("Failed to parse lok.toml");
    }

    // Try home directory
    if let Some(home) = dirs::home_dir() {
        let home_config = home.join(".config/lok/lok.toml");
        if let Ok(content) = fs::read_to_string(&home_config) {
            return toml::from_str(&content).context("Failed to parse config file");
        }
    }

    // Return default config
    Ok(Config::default())
}

pub fn init_config() -> Result<()> {
    let config = Config::default();
    let content = toml::to_string_pretty(&config)?;

    if Path::new("lok.toml").exists() {
        anyhow::bail!("lok.toml already exists");
    }

    fs::write("lok.toml", content)?;
    println!("Created lok.toml");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        // Check defaults
        assert!(config.defaults.parallel);
        assert_eq!(config.defaults.timeout, 300);

        // Check default backends exist
        assert!(config.backends.contains_key("codex"));
        assert!(config.backends.contains_key("gemini"));
        assert!(config.backends.contains_key("claude"));

        // Check default tasks exist
        assert!(config.backends.contains_key("codex"));
        assert!(config.tasks.contains_key("hunt"));
        assert!(config.tasks.contains_key("audit"));
    }

    #[test]
    fn test_conductor_defaults() {
        let config = Config::default();

        assert_eq!(config.conductor.max_rounds, 5);
        assert_eq!(config.conductor.max_tokens, 4096);
    }

    #[test]
    fn test_conductor_custom_config() {
        let toml_str = r#"
[conductor]
max_rounds = 10
max_tokens = 8192
"#;
        let config: Config = toml::from_str(toml_str).unwrap();

        assert_eq!(config.conductor.max_rounds, 10);
        assert_eq!(config.conductor.max_tokens, 8192);
    }

    #[test]
    fn test_codex_backend_defaults() {
        let config = Config::default();
        let codex = config.backends.get("codex").unwrap();

        assert!(codex.enabled);
        assert_eq!(codex.command, Some("codex".to_string()));
        assert_eq!(codex.skip_lines, 0);
    }

    #[test]
    fn test_gemini_backend_defaults() {
        let config = Config::default();
        let gemini = config.backends.get("gemini").unwrap();

        assert!(gemini.enabled);
        assert_eq!(gemini.command, Some("npx".to_string()));
        assert_eq!(gemini.skip_lines, 1);
    }

    #[test]
    fn test_claude_backend_defaults() {
        let config = Config::default();
        let claude = config.backends.get("claude").unwrap();

        assert!(claude.enabled);
        assert_eq!(claude.command, Some("claude".to_string())); // CLI mode by default
        assert!(claude.api_key_env.is_none()); // No API key needed for CLI
        assert!(claude.model.is_none()); // Uses Claude Code's default
    }

    #[test]
    fn test_hunt_task_defaults() {
        let config = Config::default();
        let hunt = config.tasks.get("hunt").unwrap();

        assert_eq!(
            hunt.description,
            Some("Find bugs and code issues".to_string())
        );
        assert!(hunt.backends.contains(&"codex".to_string()));
        assert!(!hunt.prompts.is_empty());
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml_str = r#"
[defaults]
parallel = false
timeout = 60
"#;
        let config: Config = toml::from_str(toml_str).unwrap();

        assert!(!config.defaults.parallel);
        assert_eq!(config.defaults.timeout, 60);
        assert!(config.backends.is_empty());
        assert!(config.tasks.is_empty());
    }

    #[test]
    fn test_parse_custom_backend() {
        let toml_str = r#"
[backends.custom]
enabled = true
command = "my-llm"
args = ["--flag", "value"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let custom = config.backends.get("custom").unwrap();

        assert!(custom.enabled);
        assert_eq!(custom.command, Some("my-llm".to_string()));
        assert_eq!(custom.args, vec!["--flag", "value"]);
    }

    #[test]
    fn test_parse_custom_task() {
        let toml_str = r#"
[tasks.review]
description = "Code review"
backends = ["codex", "gemini"]

[[tasks.review.prompts]]
name = "style"
prompt = "Check code style"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let review = config.tasks.get("review").unwrap();

        assert_eq!(review.description, Some("Code review".to_string()));
        assert_eq!(review.backends, vec!["codex", "gemini"]);
        assert_eq!(review.prompts.len(), 1);
        assert_eq!(review.prompts[0].name, "style");
    }

    #[test]
    fn test_backend_config_defaults() {
        let toml_str = r#"
[backends.minimal]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let minimal = config.backends.get("minimal").unwrap();

        // Check default values are applied
        assert!(minimal.enabled); // default_enabled
        assert!(minimal.args.is_empty()); // default empty vec
        assert_eq!(minimal.skip_lines, 0); // default 0
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let original = Config::default();
        let serialized = toml::to_string_pretty(&original).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();

        // Check key fields survived roundtrip
        assert_eq!(original.defaults.parallel, deserialized.defaults.parallel);
        assert_eq!(original.defaults.timeout, deserialized.defaults.timeout);
        assert_eq!(original.backends.len(), deserialized.backends.len());
        assert_eq!(original.tasks.len(), deserialized.tasks.len());
    }

    #[test]
    fn test_command_wrapper_config() {
        let toml_str = r#"
[defaults]
command_wrapper = "nix-shell --run '{cmd}'"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();

        assert_eq!(
            config.defaults.command_wrapper,
            Some("nix-shell --run '{cmd}'".to_string())
        );
    }

    #[test]
    fn test_command_wrapper_default_none() {
        let config = Config::default();
        assert!(config.defaults.command_wrapper.is_none());
    }

    #[test]
    fn test_command_wrapper_docker_example() {
        let toml_str = r#"
[defaults]
command_wrapper = "docker exec dev sh -c '{cmd}'"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();

        assert_eq!(
            config.defaults.command_wrapper,
            Some("docker exec dev sh -c '{cmd}'".to_string())
        );
    }
}
