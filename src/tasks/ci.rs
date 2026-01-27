use crate::backend;
use crate::config::Config;
use crate::output;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy)]
enum CiBackend {
    GitHub,
    GitLab,
}

impl CiBackend {
    fn detect_from_remote(dir: &Path) -> Option<Self> {
        let output = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(dir)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let url = String::from_utf8_lossy(&output.stdout).to_lowercase();

        if url.contains("github.com") || url.contains("github:") {
            Some(CiBackend::GitHub)
        } else if url.contains("gitlab.com") || url.contains("gitlab:") || url.contains("gitlab.") {
            Some(CiBackend::GitLab)
        } else {
            // Fall back to checking which CLI is available
            if which::which("gh").is_ok() {
                Some(CiBackend::GitHub)
            } else if which::which("glab").is_ok() {
                Some(CiBackend::GitLab)
            } else {
                None
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            CiBackend::GitHub => "GitHub",
            CiBackend::GitLab => "GitLab",
        }
    }

    fn cli(&self) -> &'static str {
        match self {
            CiBackend::GitHub => "gh",
            CiBackend::GitLab => "glab",
        }
    }
}

pub async fn run(
    config: &Config,
    dir: &Path,
    pr: &str,
    backend_filter: Option<&str>,
) -> Result<()> {
    println!("{}", "Lok CI Analysis".cyan().bold());
    println!("{}", "=".repeat(50).dimmed());

    // Detect CI backend
    let ci_backend = CiBackend::detect_from_remote(dir).ok_or_else(|| {
        anyhow::anyhow!(
            "Could not detect CI backend. Ensure you're in a git repo with a GitHub/GitLab remote,\n\
            or have gh/glab CLI installed."
        )
    })?;

    // Check CLI is available
    if which::which(ci_backend.cli()).is_err() {
        anyhow::bail!(
            "{} CLI ({}) is required.\n\
            Install it from: {}",
            ci_backend.name(),
            ci_backend.cli(),
            match ci_backend {
                CiBackend::GitHub => "https://cli.github.com/",
                CiBackend::GitLab => "https://gitlab.com/gitlab-org/cli",
            }
        );
    }

    println!("Backend: {}", ci_backend.name().cyan());
    println!("PR: #{}", pr.yellow());
    println!();

    // Get check status and failed logs
    let (status, failed_logs) = match ci_backend {
        CiBackend::GitHub => get_github_ci_status(dir, pr)?,
        CiBackend::GitLab => get_gitlab_ci_status(dir, pr)?,
    };

    println!("{}", status);
    println!();

    if failed_logs.is_empty() {
        println!("{}", "No failed checks to analyze.".green());
        return Ok(());
    }

    println!(
        "{}",
        format!("Analyzing {} failed check(s)...", failed_logs.len()).yellow()
    );
    println!();

    // Build analysis prompt
    let prompt = build_analysis_prompt(&failed_logs);

    // Run through LLM backends
    let backends = backend::get_backends(config, backend_filter)?;
    let results = backend::run_query(&backends, &prompt, dir, config).await?;
    output::print_results(&results);

    Ok(())
}

#[derive(Debug)]
struct FailedCheck {
    name: String,
    log: String,
}

fn get_github_ci_status(dir: &Path, pr: &str) -> Result<(String, Vec<FailedCheck>)> {
    // Get check status
    let status_output = Command::new("gh")
        .args(["pr", "checks", pr, "--json", "name,state,link"])
        .current_dir(dir)
        .output()
        .context("Failed to run gh pr checks")?;

    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        anyhow::bail!("Failed to get PR checks: {}", stderr);
    }

    let checks: Vec<serde_json::Value> =
        serde_json::from_slice(&status_output.stdout).context("Failed to parse checks JSON")?;

    // Build status summary
    let mut status_lines = vec!["Check Status:".to_string()];
    let mut failed_checks_info: Vec<(String, String)> = Vec::new(); // (name, link)

    for check in &checks {
        let name = check["name"].as_str().unwrap_or("unknown");
        let state = check["state"].as_str().unwrap_or("unknown");
        let link = check["link"].as_str().unwrap_or("");

        let status_icon = match state {
            "SUCCESS" => "✓".green().to_string(),
            "FAILURE" => {
                failed_checks_info.push((name.to_string(), link.to_string()));
                "✗".red().to_string()
            }
            "IN_PROGRESS" | "QUEUED" | "PENDING" => "○".yellow().to_string(),
            "CANCELLED" | "SKIPPED" => "○".dimmed().to_string(),
            _ => "?".dimmed().to_string(),
        };

        status_lines.push(format!(
            "  {} {} ({})",
            status_icon,
            name,
            state.to_lowercase()
        ));
    }

    let status = status_lines.join("\n");

    // Get logs for failed checks
    let mut failed_logs = Vec::new();

    for (name, link) in &failed_checks_info {
        println!("{}", format!("Fetching logs for {}...", name).dimmed());

        // Extract run ID from link (format: .../runs/12345/job/67890)
        if let Some(run_id) = extract_run_id(link) {
            let log_output = Command::new("gh")
                .args(["run", "view", &run_id, "--log-failed"])
                .current_dir(dir)
                .output();

            if let Ok(log) = log_output {
                if log.status.success() {
                    let log_text = String::from_utf8_lossy(&log.stdout).to_string();
                    // Truncate very long logs
                    let truncated = if log_text.len() > 15000 {
                        format!(
                            "{}...\n[truncated, showing last 15000 chars]",
                            &log_text[log_text.len() - 15000..]
                        )
                    } else {
                        log_text
                    };

                    failed_logs.push(FailedCheck {
                        name: name.clone(),
                        log: truncated,
                    });
                }
            }
        }
    }

    Ok((status, failed_logs))
}

fn get_gitlab_ci_status(dir: &Path, mr: &str) -> Result<(String, Vec<FailedCheck>)> {
    // Get pipeline status for the MR
    let status_output = Command::new("glab")
        .args(["mr", "view", mr, "--output", "json"])
        .current_dir(dir)
        .output()
        .context("Failed to run glab mr view")?;

    if !status_output.status.success() {
        let stderr = String::from_utf8_lossy(&status_output.stderr);
        anyhow::bail!("Failed to get MR: {}", stderr);
    }

    let mr_data: serde_json::Value =
        serde_json::from_slice(&status_output.stdout).context("Failed to parse MR JSON")?;

    let pipeline_id = mr_data["head_pipeline"]["id"]
        .as_i64()
        .map(|id| id.to_string());

    let status = match pipeline_id {
        Some(ref id) => format!("Pipeline: #{}", id),
        None => "No pipeline found".to_string(),
    };

    let mut failed_logs = Vec::new();

    if let Some(pipeline_id) = pipeline_id {
        // Get jobs for this pipeline
        let jobs_output = Command::new("glab")
            .args(["ci", "view", &pipeline_id, "--output", "json"])
            .current_dir(dir)
            .output();

        if let Ok(jobs) = jobs_output {
            if jobs.status.success() {
                let pipeline_data: serde_json::Value =
                    serde_json::from_slice(&jobs.stdout).unwrap_or_default();

                if let Some(jobs_array) = pipeline_data["jobs"].as_array() {
                    for job in jobs_array {
                        let status = job["status"].as_str().unwrap_or("");
                        if status == "failed" {
                            let job_id = job["id"].as_i64().map(|id| id.to_string());
                            let name = job["name"].as_str().unwrap_or("unknown").to_string();

                            if let Some(job_id) = job_id {
                                // Get job trace (logs)
                                let trace_output = Command::new("glab")
                                    .args(["ci", "trace", &job_id])
                                    .current_dir(dir)
                                    .output();

                                if let Ok(trace) = trace_output {
                                    if trace.status.success() {
                                        let log_text =
                                            String::from_utf8_lossy(&trace.stdout).to_string();
                                        let truncated = if log_text.len() > 15000 {
                                            format!(
                                                "{}...\n[truncated]",
                                                &log_text[log_text.len() - 15000..]
                                            )
                                        } else {
                                            log_text
                                        };

                                        failed_logs.push(FailedCheck {
                                            name,
                                            log: truncated,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok((status, failed_logs))
}

fn extract_run_id(link: &str) -> Option<String> {
    // Link format: https://github.com/owner/repo/actions/runs/12345/job/67890
    let parts: Vec<&str> = link.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == "runs" && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    None
}

fn build_analysis_prompt(failed_checks: &[FailedCheck]) -> String {
    let mut prompt = String::from(
        "Analyze these CI failures and identify the root cause. For each failure:\n\
        1. What is the actual error?\n\
        2. What file/line is causing it (if visible)?\n\
        3. What's the likely fix?\n\n\
        Be concise and actionable.\n\n",
    );

    for check in failed_checks {
        prompt.push_str(&format!(
            "=== {} ===\n```\n{}\n```\n\n",
            check.name, check.log
        ));
    }

    prompt
}
