use crate::backend::{self, Backend};
use crate::config::Config;
use crate::delegation::Delegator;
use anyhow::{Context, Result};
use colored::Colorize;
use futures::future::join_all;
use secrecy::ExposeSecret;
use std::path::Path;
use std::sync::Arc;

/// An agent task to be executed
#[derive(Debug, Clone)]
pub struct AgentTask {
    pub name: String,
    pub description: String,
    pub backend: Option<String>,
}

/// Result from an agent
#[derive(Debug)]
pub struct AgentResult {
    pub name: String,
    pub backend: String,
    pub output: String,
    pub success: bool,
}

/// Spawn coordinator - plans, delegates, summarizes
pub struct Spawn {
    config: Config,
    cwd: std::path::PathBuf,
    delegator: Delegator,
}

impl Spawn {
    pub async fn new(config: &Config, cwd: &Path) -> Result<Self> {
        Ok(Self {
            config: config.clone(),
            cwd: crate::utils::canonicalize_async(cwd).await,
            delegator: Delegator::new(),
        })
    }

    /// Plan a task by breaking it into agent subtasks
    pub async fn plan(&self, task: &str) -> Result<Vec<AgentTask>> {
        println!("{}", "=".repeat(50).dimmed());
        println!("{}", "SPAWN: Planning Phase".cyan().bold());
        println!("{}", "=".repeat(50).dimmed());
        println!();
        println!("Task: {}", task);
        println!();

        // Try Claude API first, fall back to other backends
        let claude_config = self.config.backends.get("claude");

        if let Some(cfg) = claude_config {
            if cfg.command.is_none() {
                // API mode - try it, but fall back on error
                match self.plan_with_conductor(task).await {
                    Ok(tasks) => return Ok(tasks),
                    Err(e) => {
                        println!("{} {}", "Claude API unavailable:".yellow(), e);
                        println!("{}", "Falling back to available backend...".yellow());
                    }
                }
            }
        }

        // Fallback: use codex/gemini to suggest a breakdown
        self.plan_with_backend(task).await
    }

    async fn plan_with_conductor(&self, task: &str) -> Result<Vec<AgentTask>> {
        let claude = backend::create_claude_backend(&self.config)?;

        let (api_key, model, client) = claude
            .api_details()
            .ok_or_else(|| anyhow::anyhow!("Claude API required for planning"))?;

        let system = r#"You are a task planner. Break down the given task into 2-4 parallel subtasks that can be worked on independently.

For each subtask, output a line in this exact format:
AGENT: <name> | <description>

Example:
AGENT: backend | Build the Express.js REST API with user authentication
AGENT: frontend | Build the React UI with login form and dashboard
AGENT: database | Design the PostgreSQL schema and migrations

Rules:
- Keep subtasks independent (can run in parallel)
- Be specific about what each agent should build
- 2-4 agents maximum
- Names should be short (one word)
- Descriptions should be actionable"#;

        let request = serde_json::json!({
            "model": model,
            "max_tokens": 1024,
            "system": system,
            "messages": [{
                "role": "user",
                "content": format!("Break down this task into parallel subtasks:\n\n{}", task)
            }]
        });

        println!("{}", "Asking conductor to plan...".dimmed());

        let response = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key.expose_secret())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send planning request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error {}: {}", status, body);
        }

        let response: serde_json::Value = response.json().await?;

        // Extract text from response, handling unexpected shapes
        let text = match response
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
        {
            Some(text) => text,
            None => {
                // Response shape unexpected - show what we got for debugging
                let response_preview = serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|_| format!("{:?}", response));
                anyhow::bail!(
                    "Unexpected response shape from Claude API. Expected content[0].text.\n\
                    Response preview:\n{}",
                    if response_preview.len() > 500 {
                        // Truncate at UTF-8 char boundary (~500 bytes)
                        let truncate_at = response_preview
                            .char_indices()
                            .take_while(|(i, _)| *i <= 500)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(response_preview.len().min(500));
                        format!("{}...", &response_preview[..truncate_at])
                    } else {
                        response_preview
                    }
                );
            }
        };

        self.parse_agent_tasks(text)
    }

    async fn plan_with_backend(&self, task: &str) -> Result<Vec<AgentTask>> {
        let backends = backend::get_backends(&self.config, None)?;
        let backend = backends
            .first()
            .ok_or_else(|| anyhow::anyhow!("No backends available"))?;

        let prompt = format!(
            r#"Break down this task into 2-4 parallel subtasks. For each, output:
AGENT: <name> | <description>

Task: {}

Example format:
AGENT: backend | Build the API
AGENT: frontend | Build the UI"#,
            task
        );

        let output = backend.query(&prompt, &self.cwd).await?;
        self.parse_agent_tasks(&output)
    }

    fn parse_agent_tasks(&self, text: &str) -> Result<Vec<AgentTask>> {
        let mut tasks = Vec::new();

        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("AGENT:") {
                if let Some((name_part, desc_part)) = rest.split_once('|') {
                    let name = name_part.trim().to_string();
                    let description = desc_part.trim().to_string();
                    tasks.push(AgentTask {
                        name,
                        description,
                        backend: None, // Will be assigned by delegator
                    });
                }
            }
        }

        if tasks.is_empty() {
            let text_preview = if text.len() > 300 {
                // Find last valid UTF-8 boundary at or before 300 bytes
                let truncate_at = text
                    .char_indices()
                    .take_while(|(i, _)| *i <= 300)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(text.len().min(300));
                format!("{}...", &text[..truncate_at])
            } else {
                text.to_string()
            };
            anyhow::bail!(
                "Failed to parse agent tasks from plan. Expected lines starting with 'AGENT: name | description'.\n\
                Raw response:\n{}",
                text_preview
            );
        }

        println!();
        println!("{}", "Planned agents:".green().bold());
        for task in &tasks {
            println!("  {} {}", "→".cyan(), task.name);
            println!("    {}", task.description.dimmed());
        }
        println!();

        Ok(tasks)
    }

    /// Execute agents in parallel
    pub async fn execute(
        &self,
        tasks: Vec<AgentTask>,
        shared_context: &str,
    ) -> Result<Vec<AgentResult>> {
        println!("{}", "=".repeat(50).dimmed());
        println!("{}", "SPAWN: Execution Phase".cyan().bold());
        println!("{}", "=".repeat(50).dimmed());
        println!();

        let backends = backend::get_backends(&self.config, None)?;
        let backend_map: std::collections::HashMap<String, Arc<dyn Backend>> = backends
            .into_iter()
            .map(|b| (b.name().to_string(), b))
            .collect();

        let futures: Vec<_> = tasks
            .into_iter()
            .map(|task| {
                let backend_map = backend_map.clone();
                let delegator = self.delegator.clone();
                let cwd = self.cwd.clone();
                let shared_context = shared_context.to_string();

                async move {
                    // Pick backend for this task
                    let backend_name = task.backend.clone().unwrap_or_else(|| {
                        delegator
                            .best_for(&task.description)
                            .unwrap_or("codex")
                            .to_string()
                    });

                    let backend = match backend_map.get(&backend_name) {
                        Some(b) => b.clone(),
                        None => {
                            // Fallback to first available
                            match backend_map.values().next() {
                                Some(b) => b.clone(),
                                None => {
                                    return AgentResult {
                                        name: task.name,
                                        backend: backend_name,
                                        output: "No backend available".to_string(),
                                        success: false,
                                    };
                                }
                            }
                        }
                    };

                    println!(
                        "{} {} → {}",
                        "Spawning:".green(),
                        task.name.bold(),
                        backend.name().yellow()
                    );

                    let prompt = format!(
                        "Context:\n{}\n\nYour task ({}):\n{}",
                        shared_context, task.name, task.description
                    );

                    match backend.query(&prompt, &cwd).await {
                        Ok(output) => AgentResult {
                            name: task.name,
                            backend: backend.name().to_string(),
                            output,
                            success: true,
                        },
                        Err(e) => AgentResult {
                            name: task.name,
                            backend: backend.name().to_string(),
                            output: format!("Error: {}", e),
                            success: false,
                        },
                    }
                }
            })
            .collect();

        let results = join_all(futures).await;

        println!();
        println!("{}", "All agents completed.".green());
        println!();

        Ok(results)
    }

    /// Summarize results
    pub fn summarize(&self, results: &[AgentResult]) -> String {
        println!("{}", "=".repeat(50).dimmed());
        println!("{}", "SPAWN: Results".cyan().bold());
        println!("{}", "=".repeat(50).dimmed());
        println!();

        let mut summary = String::new();

        for result in results {
            let status = if result.success {
                "✓".green()
            } else {
                "✗".red()
            };

            println!(
                "{} {} ({})",
                status,
                result.name.bold(),
                result.backend.yellow()
            );
            println!("{}", "-".repeat(40).dimmed());

            // Truncate long outputs for display
            let display_output = if result.output.len() > 500 {
                format!(
                    "{}...\n[truncated, {} chars total]",
                    &result.output.chars().take(500).collect::<String>(),
                    result.output.len()
                )
            } else {
                result.output.clone()
            };

            println!("{}", display_output);
            println!();

            summary.push_str(&format!("## {}\n\n{}\n\n", result.name, result.output));
        }

        summary
    }

    /// Full spawn flow: plan → execute → summarize
    pub async fn run(&self, task: &str, manual_agents: Option<Vec<AgentTask>>) -> Result<String> {
        // Phase 1: Plan (or use manual agents)
        let tasks = match manual_agents {
            Some(agents) => {
                println!("{}", "Using manually specified agents...".yellow());
                agents
            }
            None => self.plan(task).await?,
        };

        // Phase 2: Execute in parallel
        let shared_context = format!(
            "Overall project goal: {}\n\nYou are one of several agents working in parallel. \
            Focus only on your specific task. Be concise and actionable.",
            task
        );

        let results = self.execute(tasks, &shared_context).await?;

        // Phase 3: Summarize
        let summary = self.summarize(&results);

        Ok(summary)
    }
}
