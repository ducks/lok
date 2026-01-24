use crate::config::BackendConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct GeminiBackend {
    command: String,
    args: Vec<String>,
    skip_lines: usize,
}

impl GeminiBackend {
    pub fn new(config: &BackendConfig) -> Result<Self> {
        let command = config.command.clone().unwrap_or_else(|| "npx".to_string());

        let args = if config.args.is_empty() {
            vec!["@google/gemini-cli".to_string()]
        } else {
            config.args.clone()
        };

        Ok(Self {
            command,
            args,
            skip_lines: config.skip_lines,
        })
    }

    fn parse_output(&self, output: &str) -> String {
        output
            .lines()
            .skip(self.skip_lines)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl super::Backend for GeminiBackend {
    fn name(&self) -> &str {
        "gemini"
    }

    async fn query(&self, prompt: &str, cwd: &Path) -> Result<String> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .arg(prompt)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .context("Failed to execute gemini command")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            anyhow::bail!("Gemini failed: {}", stderr);
        }

        Ok(self.parse_output(&stdout))
    }

    fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }
}
