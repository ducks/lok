use crate::config::BackendConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct BedrockBackend {
    client: Client,
    pub model_id: String,
}

#[derive(Serialize)]
struct BedrockRequest {
    anthropic_version: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Deserialize, Debug)]
pub struct BedrockResponse {
    pub content: Vec<ResponseBlock>,
    pub stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

impl BedrockBackend {
    pub async fn new(config: &BackendConfig) -> Result<Self> {
        let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = Client::new(&aws_config);

        let model_id = config
            .model
            .clone()
            .unwrap_or_else(|| "us.anthropic.claude-sonnet-4-20250514-v1:0".to_string());

        Ok(Self { client, model_id })
    }

    pub async fn invoke_with_messages(
        &self,
        system: Option<&str>,
        messages: Vec<Message>,
        tools: Option<Vec<serde_json::Value>>,
    ) -> Result<BedrockResponse> {
        let request = BedrockRequest {
            anthropic_version: "bedrock-2023-05-31".to_string(),
            max_tokens: 4096,
            system: system.map(|s| s.to_string()),
            messages,
            tools,
        };

        let body = serde_json::to_vec(&request)?;

        let response = self
            .client
            .invoke_model()
            .model_id(&self.model_id)
            .content_type("application/json")
            .body(Blob::new(body))
            .send()
            .await
            .context("Failed to invoke Bedrock model")?;

        let response_body = response.body.as_ref();
        let response: BedrockResponse =
            serde_json::from_slice(response_body).context("Failed to parse Bedrock response")?;

        Ok(response)
    }
}

#[async_trait]
impl super::Backend for BedrockBackend {
    fn name(&self) -> &str {
        "bedrock"
    }

    async fn query(&self, prompt: &str, _cwd: &Path) -> Result<String> {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(prompt.to_string()),
        }];

        let response = self.invoke_with_messages(None, messages, None).await?;

        let text = response
            .content
            .into_iter()
            .filter_map(|block| match block {
                ResponseBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }

    fn is_available(&self) -> bool {
        // Check if AWS credentials are available
        std::env::var("AWS_ACCESS_KEY_ID").is_ok()
            || std::env::var("AWS_PROFILE").is_ok()
            || std::path::Path::new(&format!(
                "{}/.aws/credentials",
                std::env::var("HOME").unwrap_or_default()
            ))
            .exists()
    }
}
