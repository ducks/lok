use crate::config::BackendConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::env;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Claude backend mode - API or CLI
#[derive(Clone)]
pub enum ClaudeMode {
    /// Use Claude API directly (requires ANTHROPIC_API_KEY)
    Api {
        api_key: SecretString,
        model: String,
        client: reqwest::Client,
    },
    /// Use Claude CLI (`claude -p`)
    Cli {
        command: String,
        model: Option<String>,
    },
}

pub struct ClaudeBackend {
    mode: ClaudeMode,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

impl ClaudeBackend {
    pub fn new(config: &BackendConfig) -> Result<Self> {
        // Check if we have a command configured (CLI mode) or API key (API mode)
        if let Some(ref cmd) = config.command {
            // CLI mode - use claude command
            Ok(Self {
                mode: ClaudeMode::Cli {
                    command: cmd.clone(),
                    model: config.model.clone(),
                },
            })
        } else {
            // API mode - requires API key
            let api_key_env = config
                .api_key_env
                .clone()
                .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());

            let api_key: SecretString = env::var(&api_key_env)
                .with_context(|| format!("Missing environment variable: {}", api_key_env))?
                .into();

            let model = config
                .model
                .clone()
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

            let client = reqwest::Client::new();

            Ok(Self {
                mode: ClaudeMode::Api {
                    api_key,
                    model,
                    client,
                },
            })
        }
    }

    /// Get API mode details (for conductor)
    pub fn api_details(&self) -> Option<(&SecretString, &str, &reqwest::Client)> {
        match &self.mode {
            ClaudeMode::Api {
                api_key,
                model,
                client,
            } => Some((api_key, model, client)),
            ClaudeMode::Cli { .. } => None,
        }
    }

    async fn query_api(&self, system: &str, prompt: &str) -> Result<String> {
        let (api_key, model, client) = match &self.mode {
            ClaudeMode::Api {
                api_key,
                model,
                client,
            } => (api_key, model, client),
            ClaudeMode::Cli { .. } => anyhow::bail!("API mode required for this operation"),
        };

        let request = serde_json::json!({
            "model": model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error {}: {}", status, body);
        }

        let response: ClaudeResponse = response
            .json()
            .await
            .context("Failed to parse Claude response")?;

        let text = response
            .content
            .into_iter()
            .filter_map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }

    async fn query_cli(&self, prompt: &str, cwd: &Path) -> Result<String> {
        let (command, model) = match &self.mode {
            ClaudeMode::Cli { command, model } => (command, model),
            ClaudeMode::Api { .. } => anyhow::bail!("CLI mode required for this operation"),
        };

        let mut cmd = Command::new(command);
        cmd.arg("-p") // print mode
            .arg("--output-format")
            .arg("text");

        if let Some(m) = model {
            cmd.arg("--model").arg(m);
        }

        cmd.arg("--") // Prevent prompt from being interpreted as flags
            .arg(prompt)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .context("Failed to execute claude command")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            anyhow::bail!("Claude CLI failed: {}", stderr);
        }

        Ok(stdout.trim().to_string())
    }

    pub async fn query_with_system(&self, system: &str, prompt: &str) -> Result<String> {
        match &self.mode {
            ClaudeMode::Api { .. } => self.query_api(system, prompt).await,
            ClaudeMode::Cli { .. } => {
                // For CLI mode, prepend system prompt to user prompt
                let full_prompt = format!("{}\n\n{}", system, prompt);
                self.query_cli(&full_prompt, Path::new(".")).await
            }
        }
    }
}

#[async_trait]
impl super::Backend for ClaudeBackend {
    fn name(&self) -> &str {
        "claude"
    }

    async fn query(&self, prompt: &str, cwd: &Path) -> Result<String> {
        match &self.mode {
            ClaudeMode::Api { .. } => {
                self.query_with_system("You are a helpful assistant.", prompt)
                    .await
            }
            ClaudeMode::Cli { .. } => self.query_cli(prompt, cwd).await,
        }
    }

    fn is_available(&self) -> bool {
        match &self.mode {
            ClaudeMode::Api { api_key, .. } => !api_key.expose_secret().is_empty(),
            ClaudeMode::Cli { command, .. } => which::which(command).is_ok(),
        }
    }
}
