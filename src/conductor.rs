use crate::backend;
use crate::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct Conductor {
    api_key: SecretString,
    model: String,
    client: reqwest::Client,
    config: Config,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Message {
    role: String,
    content: MessageContent,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
enum ContentBlock {
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

#[derive(Serialize)]
struct Tool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct ClaudeResponse {
    content: Vec<ResponseBlock>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

impl Conductor {
    pub fn new(config: &Config) -> Result<Self> {
        let claude = backend::create_claude_backend(config)?;
        let (api_key, model, client) = claude
            .api_details()
            .ok_or_else(|| anyhow::anyhow!(
                "Conductor requires Claude API mode. Set ANTHROPIC_API_KEY or configure claude backend without 'command' field."
            ))?;

        Ok(Self {
            api_key: api_key.clone(),
            model: model.to_string(),
            client: client.clone(),
            config: config.clone(),
        })
    }

    fn get_available_backends(&self) -> Vec<String> {
        self.config
            .backends
            .iter()
            .filter(|(name, cfg)| cfg.enabled && *name != "claude")
            .map(|(name, _)| name.clone())
            .collect()
    }

    fn build_system_prompt(&self) -> String {
        let backends = self.get_available_backends();
        let backend_list = backends
            .iter()
            .map(|b| {
                let desc = match b.as_str() {
                    "codex" => "OpenAI Codex - efficient, direct answers, good for code analysis",
                    "gemini" => "Google Gemini - thorough, investigative, good for deep analysis",
                    _ => "LLM backend",
                };
                format!("- {}: {}", b, desc)
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"You are the Lok Conductor, an AI orchestrator that delegates tasks to specialized LLM backends.

Available backends:
{}

Your job is to:
1. Analyze the user's request
2. Decide which backend(s) to query and with what prompts
3. Review the results and decide if you need more information
4. Synthesize a final answer when you have enough information

Guidelines:
- Use codex for quick, direct code analysis (N+1 queries, code patterns, refactoring)
- Use gemini for thorough investigation (security audits, complex analysis)
- You can query multiple backends in sequence
- You can do multiple rounds if needed to get complete information
- When you have enough information, provide a clear, synthesized answer

Always explain your reasoning briefly before making tool calls."#,
            backend_list
        )
    }

    fn build_tools(&self) -> Vec<Tool> {
        let backends = self.get_available_backends();

        vec![Tool {
            name: "query_backend".to_string(),
            description: "Query an LLM backend with a specific prompt. The backend will analyze the codebase in the current directory.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "backend": {
                        "type": "string",
                        "enum": backends,
                        "description": "Which backend to query"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to send to the backend"
                    }
                },
                "required": ["backend", "prompt"]
            }),
        }]
    }

    async fn execute_tool(
        &self,
        name: &str,
        input: &serde_json::Value,
        cwd: &Path,
    ) -> Result<String> {
        match name {
            "query_backend" => {
                let backend_name = input["backend"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing backend"))?;
                let prompt = input["prompt"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing prompt"))?;

                println!("  {} Querying {} ...", "→".cyan(), backend_name.yellow());

                let backend_config = self
                    .config
                    .backends
                    .get(backend_name)
                    .ok_or_else(|| anyhow::anyhow!("Backend not found: {}", backend_name))?;

                let backend = backend::create_backend(backend_name, backend_config)?;
                let result = backend.query(prompt, cwd).await?;

                println!(
                    "  {} {} responded ({} chars)",
                    "←".green(),
                    backend_name.yellow(),
                    result.len()
                );

                Ok(result)
            }
            _ => anyhow::bail!("Unknown tool: {}", name),
        }
    }

    pub async fn conduct(&self, task: &str, cwd: &Path) -> Result<String> {
        let cwd = crate::utils::canonicalize_async(cwd).await;

        println!("{}", "Conductor starting...".cyan().bold());
        println!("Task: {}", task);
        println!();

        let system = self.build_system_prompt();
        let tools = self.build_tools();

        let mut messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(task.to_string()),
        }];

        let max_rounds = self.config.conductor.max_rounds;
        for round in 0..max_rounds {
            println!(
                "{} {}",
                format!("[Round {}]", round + 1).dimmed(),
                "Thinking...".dimmed()
            );

            let request = serde_json::json!({
                "model": &self.model,
                "max_tokens": self.config.conductor.max_tokens,
                "system": system,
                "tools": tools,
                "messages": messages
            });

            let response = self
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", self.api_key.expose_secret())
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

            // Check for tool use
            let mut has_tool_use = false;
            let mut tool_results = Vec::new();
            let mut assistant_content = Vec::new();

            for block in &response.content {
                match block {
                    ResponseBlock::Text { text } => {
                        println!("{}", text.dimmed());
                        assistant_content.push(ContentBlock::Text { text: text.clone() });
                    }
                    ResponseBlock::ToolUse { id, name, input } => {
                        has_tool_use = true;
                        assistant_content.push(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });

                        let result = self.execute_tool(name, input, &cwd).await;
                        let result_text = match result {
                            Ok(text) => text,
                            Err(e) => format!("Error: {}", e),
                        };

                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: result_text,
                        });
                    }
                }
            }

            // Add assistant message
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(assistant_content),
            });

            // If we had tool use, add tool results and continue
            if has_tool_use {
                messages.push(Message {
                    role: "user".to_string(),
                    content: MessageContent::Blocks(tool_results),
                });
            } else {
                // No tool use means we're done
                let final_text = response
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ResponseBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                println!();
                return Ok(final_text);
            }
        }

        anyhow::bail!("Conductor reached maximum rounds without completing")
    }
}
