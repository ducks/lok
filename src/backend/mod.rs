#[cfg(feature = "bedrock")]
mod bedrock;
mod claude;
mod codex;
mod gemini;
mod ollama;

#[cfg(feature = "bedrock")]
pub use bedrock::BedrockBackend;
pub use claude::ClaudeBackend;

use crate::config::{BackendConfig, Config};
use anyhow::Result;
use async_trait::async_trait;
use colored::Colorize;
use futures::future::join_all;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    pub elapsed_ms: u64,
}

pub fn create_backend(name: &str, config: &BackendConfig) -> Result<Arc<dyn Backend>> {
    match name {
        "codex" => Ok(Arc::new(codex::CodexBackend::new(config)?)),
        "gemini" => Ok(Arc::new(gemini::GeminiBackend::new(config)?)),
        "claude" => Ok(Arc::new(claude::ClaudeBackend::new(config)?)),
        "ollama" => Ok(Arc::new(ollama::OllamaBackend::new(config)?)),
        #[cfg(feature = "bedrock")]
        "bedrock" => {
            // BedrockBackend::new is async, need runtime
            let rt = tokio::runtime::Handle::current();
            let config = config.clone();
            rt.block_on(async { Ok(Arc::new(bedrock::BedrockBackend::new(&config).await?) as Arc<dyn Backend>) })
        }
        #[cfg(not(feature = "bedrock"))]
        "bedrock" => anyhow::bail!("Bedrock backend requires the 'bedrock' feature. Rebuild with: cargo build --features bedrock"),
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

pub fn get_backends(config: &Config, filter: Option<&str>) -> Result<Vec<Arc<dyn Backend>>> {
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
                eprintln!(
                    "{} Failed to create backend {}: {}",
                    "warning:".yellow(),
                    name,
                    e
                );
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
    config: &Config,
) -> Result<Vec<QueryResult>> {
    run_query_with_config(backends, prompt, cwd, config).await
}

pub async fn run_query_with_config(
    backends: &[Arc<dyn Backend>],
    prompt: &str,
    cwd: &Path,
    config: &Config,
) -> Result<Vec<QueryResult>> {
    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("Directory not found: {}", cwd.display()))?;
    let default_timeout = config.defaults.timeout;
    let parallel = config.defaults.parallel;

    let pb = ProgressBar::new(backends.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let query_one = |backend: Arc<dyn Backend>,
                     prompt: String,
                     cwd: PathBuf,
                     pb: ProgressBar,
                     timeout: u64| async move {
        pb.set_message(format!("Querying {}...", backend.name()));

        let start = Instant::now();
        let result =
            tokio::time::timeout(Duration::from_secs(timeout), backend.query(&prompt, &cwd)).await;
        let elapsed_ms = start.elapsed().as_millis() as u64;

        pb.inc(1);

        match result {
            Ok(Ok(output)) => QueryResult {
                backend: backend.name().to_string(),
                output,
                success: true,
                elapsed_ms,
            },
            Ok(Err(e)) => QueryResult {
                backend: backend.name().to_string(),
                output: format!("Error: {}", e),
                success: false,
                elapsed_ms,
            },
            Err(_) => QueryResult {
                backend: backend.name().to_string(),
                output: format!("Error: Timeout ({}s)", timeout),
                success: false,
                elapsed_ms,
            },
        }
    };

    // Helper to get timeout for a backend
    let get_timeout = |backend_name: &str| -> u64 {
        config
            .backends
            .get(backend_name)
            .and_then(|b| b.timeout)
            .unwrap_or(default_timeout)
    };

    let results = if parallel {
        let futures: Vec<_> = backends
            .iter()
            .map(|backend| {
                let timeout = get_timeout(backend.name());
                query_one(
                    Arc::clone(backend),
                    prompt.to_string(),
                    cwd.clone(),
                    pb.clone(),
                    timeout,
                )
            })
            .collect();
        join_all(futures).await
    } else {
        let mut results = Vec::new();
        for backend in backends {
            let timeout = get_timeout(backend.name());
            let result = query_one(
                Arc::clone(backend),
                prompt.to_string(),
                cwd.clone(),
                pb.clone(),
                timeout,
            )
            .await;
            results.push(result);
        }
        results
    };

    pb.finish_and_clear();

    Ok(results)
}

/// Print verbose debug info before running a query
pub fn print_verbose_header(prompt: &str, backends: &[Arc<dyn Backend>], cwd: &Path) {
    println!("{}", "=== VERBOSE MODE ===".cyan().bold());
    println!();
    println!("{} {}", "Working directory:".dimmed(), cwd.display());
    println!(
        "{} {}",
        "Backends:".dimmed(),
        backends
            .iter()
            .map(|b| b.name())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!();
    println!("{}", "Prompt:".dimmed());
    println!("{}", "-".repeat(50).dimmed());
    println!("{}", prompt);
    println!("{}", "-".repeat(50).dimmed());
    println!();
}

/// Print verbose timing info after results
pub fn print_verbose_timing(results: &[QueryResult]) {
    println!();
    println!("{}", "=== TIMING ===".cyan().bold());
    for result in results {
        let status = if result.success {
            "OK".green()
        } else {
            "FAIL".red()
        };
        let time = format_duration(result.elapsed_ms);
        let chars = result.output.len();
        println!(
            "  {} {} ({}, {} chars)",
            result.backend.bold(),
            status,
            time,
            chars
        );
    }
    println!();
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}m", ms as f64 / 60000.0)
    }
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

        println!("  {} - {} ({})", name.bold(), status, available);

        if let Some(ref cmd) = backend_config.command {
            println!("    command: {} {}", cmd, backend_config.args.join(" "));
        }
    }

    Ok(())
}
