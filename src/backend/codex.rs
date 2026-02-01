use crate::config::BackendConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct CodexBackend {
    command: String,
    args: Vec<String>,
}

impl CodexBackend {
    pub fn new(config: &BackendConfig) -> Result<Self> {
        let command = config
            .command
            .clone()
            .unwrap_or_else(|| "codex".to_string());

        let args = if config.args.is_empty() {
            vec![
                "exec".to_string(),
                "--json".to_string(),
                "-s".to_string(),
                "read-only".to_string(),
            ]
        } else {
            config.args.clone()
        };

        Ok(Self { command, args })
    }

    fn parse_output(&self, output: &str) -> String {
        // Parse JSON output from codex
        // Look for agent_message in item.completed events
        for line in output.lines() {
            if line.contains("\"type\":\"item.completed\"") && line.contains("agent_message") {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(text) = json
                        .get("item")
                        .and_then(|i| i.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        return text.to_string();
                    }
                }
            }
        }

        // Fallback: return raw output
        output.to_string()
    }
}

#[async_trait]
impl super::Backend for CodexBackend {
    fn name(&self) -> &str {
        "codex"
    }

    async fn query(&self, prompt: &str, cwd: &Path) -> Result<String> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .arg("--") // Prevent prompt from being interpreted as flags
            .arg(prompt)
            .current_dir(cwd)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .context("Failed to execute codex command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Codex failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(self.parse_output(&stdout))
    }

    fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }
}
