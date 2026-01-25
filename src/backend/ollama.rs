//! Ollama backend - HTTP API for local LLMs

use super::Backend;
use crate::config::BackendConfig;
use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

pub struct OllamaBackend {
    client: Client,
    base_url: String,
    model: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: Option<ChatMessage>,
}

impl OllamaBackend {
    pub fn new(config: &BackendConfig) -> Result<Self> {
        let base_url = config
            .command
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".to_string());

        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "llama3.2".to_string());

        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout.unwrap_or(300)))
            .build()?;

        Ok(Self {
            client,
            base_url,
            model,
        })
    }

    async fn chat(&self, prompt: &str) -> Result<String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            stream: false,
        };

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Ollama error: {}", error_text);
        }

        let chat_response: ChatResponse = response.json().await?;

        match chat_response.message {
            Some(msg) => Ok(msg.content),
            None => Ok(String::new()),
        }
    }
}

#[async_trait]
impl Backend for OllamaBackend {
    fn name(&self) -> &str {
        "ollama"
    }

    async fn query(&self, prompt: &str, _cwd: &Path) -> Result<String> {
        self.chat(prompt).await
    }

    fn is_available(&self) -> bool {
        // Ollama is a server, not a CLI. Can't easily check synchronously.
        // Return true and let runtime connection fail if not running.
        true
    }
}
