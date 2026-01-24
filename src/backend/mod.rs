mod bedrock;
mod claude;
mod codex;
mod gemini;

pub use bedrock::BedrockBackend;
pub use claude::ClaudeBackend;

use crate::config::{BackendConfig, Config};
use anyhow::Result;
use async_trait::async_trait;
use colored::Colorize;
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[async_trait]
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    async fn query(&self, prompt: &str, cwd: &Path) -> Result<String>;
    fn is_available(&self) -> bool;
}

pub struct QueryResult {
    pub backend: String,
    pub output: String,
    pub success: bool,
}

pub fn create_backend(name: &str, config: &BackendConfig) -> Result<Arc<dyn Backend>> {
    match name {
        "codex" => Ok(Arc::new(codex::CodexBackend::new(config)?)),
        "gemini" => Ok(Arc::new(gemini::GeminiBackend::new(config)?)),
        "claude" => Ok(Arc::new(claude::ClaudeBackend::new(config)?)),
        _ => anyhow::bail!("Unknown backend: {}", name),
    }
}

pub fn create_claude_backend(config: &Config) -> Result<ClaudeBackend> {
    let backend_config = config
        .backends
        .get("claude")
        .ok_or_else(|| anyhow::anyhow!("Claude backend not configured"))?;
    ClaudeBackend::new(backend_config)
}

pub async fn create_bedrock_backend(config: &Config) -> Result<BedrockBackend> {
    let backend_config = config
        .backends
        .get("bedrock")
        .ok_or_else(|| anyhow::anyhow!("Bedrock backend not configured"))?;
    BedrockBackend::new(backend_config).await
}

pub fn get_backends(
    config: &Config,
    filter: Option<&str>,
) -> Result<Vec<Arc<dyn Backend>>> {
    let mut backends = Vec::new();

    let filter_names: Option<Vec<&str>> = filter.map(|f| f.split(',').collect());

    for (name, backend_config) in &config.backends {
        if !backend_config.enabled {
            continue;
        }

        if let Some(ref names) = filter_names {
            if !names.contains(&name.as_str()) {
                continue;
            }
        }

        match create_backend(name, backend_config) {
            Ok(backend) => {
                if backend.is_available() {
                    backends.push(backend);
                } else {
                    eprintln!("{} Backend {} is not available", "warning:".yellow(), name);
                }
            }
            Err(e) => {
                eprintln!("{} Failed to create backend {}: {}", "warning:".yellow(), name, e);
            }
        }
    }

    if backends.is_empty() {
        anyhow::bail!("No backends available");
    }

    Ok(backends)
}

pub async fn run_query(
    backends: &[Arc<dyn Backend>],
    prompt: &str,
    cwd: &Path,
) -> Result<Vec<QueryResult>> {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());

    let pb = ProgressBar::new(backends.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let futures: Vec<_> = backends
        .iter()
        .map(|backend| {
            let backend = Arc::clone(backend);
            let prompt = prompt.to_string();
            let cwd = cwd.clone();
            let pb = pb.clone();

            async move {
                pb.set_message(format!("Querying {}...", backend.name()));

                let result = tokio::time::timeout(
                    Duration::from_secs(300),
                    backend.query(&prompt, &cwd),
                )
                .await;

                pb.inc(1);

                match result {
                    Ok(Ok(output)) => QueryResult {
                        backend: backend.name().to_string(),
                        output,
                        success: true,
                    },
                    Ok(Err(e)) => QueryResult {
                        backend: backend.name().to_string(),
                        output: format!("Error: {}", e),
                        success: false,
                    },
                    Err(_) => QueryResult {
                        backend: backend.name().to_string(),
                        output: "Error: Timeout".to_string(),
                        success: false,
                    },
                }
            }
        })
        .collect();

    let results = join_all(futures).await;
    pb.finish_and_clear();

    Ok(results)
}

pub fn list_backends(config: &Config) -> Result<()> {
    println!("{}", "Available backends:".bold());
    println!();

    for (name, backend_config) in &config.backends {
        let status = if backend_config.enabled {
            "enabled".green()
        } else {
            "disabled".red()
        };

        let available = match create_backend(name, backend_config) {
            Ok(b) if b.is_available() => "available".green(),
            _ => "not available".yellow(),
        };

        println!(
            "  {} - {} ({})",
            name.bold(),
            status,
            available
        );

        if let Some(ref cmd) = backend_config.command {
            println!("    command: {} {}", cmd, backend_config.args.join(" "));
        }
    }

    Ok(())
}
