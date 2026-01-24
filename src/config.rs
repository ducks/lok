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
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
}

fn default_parallel() -> bool {
    true
}

fn default_timeout() -> u64 {
    300
}

fn default_output_dir() -> String {
    "/tmp/lok".to_string()
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            parallel: default_parallel(),
            timeout: default_timeout(),
            output_dir: default_output_dir(),
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
    #[serde(default = "default_parse")]
    pub parse: String,
    #[serde(default)]
    pub skip_lines: usize,
    pub api_key_env: Option<String>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_parse() -> String {
    "raw".to_string()
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
                parse: "json".to_string(),
                skip_lines: 0,
                api_key_env: None,
                model: None,
                endpoint: None,
            },
        );

        backends.insert(
            "gemini".to_string(),
            BackendConfig {
                enabled: true,
                command: Some("npx".to_string()),
                args: vec!["@google/gemini-cli".to_string()],
                parse: "raw".to_string(),
                skip_lines: 1,
                api_key_env: None,
                model: None,
                endpoint: None,
            },
        );

        backends.insert(
            "claude".to_string(),
            BackendConfig {
                enabled: true,
                command: None,
                args: vec![],
                parse: "raw".to_string(),
                skip_lines: 0,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                model: Some("claude-sonnet-4-20250514".to_string()),
                endpoint: None,
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
                        name: "n+1".to_string(),
                        prompt: "Search for N+1 query issues in this codebase. Look for queries inside loops, missing includes/preload. List up to 5 specific issues with file:line. Be concise.".to_string(),
                    },
                    TaskPrompt {
                        name: "dead-code".to_string(),
                        prompt: "Find unused or dead code in this codebase. List up to 5 specific issues with file:line. Be concise.".to_string(),
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
                        prompt: "Search for SQL injection vulnerabilities. List up to 5 specific issues with file paths. Be concise.".to_string(),
                    },
                    TaskPrompt {
                        name: "auth".to_string(),
                        prompt: "Search for authentication/authorization bypass vulnerabilities. List up to 5 specific issues with file paths. Be concise.".to_string(),
                    },
                ],
            },
        );

        Self {
            defaults: Defaults::default(),
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
