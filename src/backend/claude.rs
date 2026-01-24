use crate::config::BackendConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;

pub struct ClaudeBackend {
    pub api_key: String,
    pub model: String,
    pub client: reqwest::Client,
}

#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Message {
    role: String,
    content: String,
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
        let api_key_env = config
            .api_key_env
            .clone()
            .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());

        let api_key = env::var(&api_key_env)
            .with_context(|| format!("Missing environment variable: {}", api_key_env))?;

        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());

        let client = reqwest::Client::new();

        Ok(Self {
            api_key,
            model,
            client,
        })
    }

    pub async fn query_with_system(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<String> {
        let request = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        });

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
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
}

#[async_trait]
impl super::Backend for ClaudeBackend {
    fn name(&self) -> &str {
        "claude"
    }

    async fn query(&self, prompt: &str, _cwd: &Path) -> Result<String> {
        self.query_with_system("You are a helpful assistant.", prompt)
            .await
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}
