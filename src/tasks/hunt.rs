use crate::backend::{self, QueryResult};
use crate::config::Config;
use crate::context::CodebaseContext;
use crate::output;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy)]
enum IssueBackend {
    GitHub,
    GitLab,
}

impl IssueBackend {
    fn detect_from_remote(dir: &Path) -> Option<Self> {
        // Check git remote origin URL to determine host
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
            Some(IssueBackend::GitHub)
        } else if url.contains("gitlab.com") || url.contains("gitlab:") || url.contains("gitlab.") {
            Some(IssueBackend::GitLab)
        } else {
            // Unknown host, fall back to checking which CLI is available
            if which::which("gh").is_ok() {
                Some(IssueBackend::GitHub)
            } else if which::which("glab").is_ok() {
                Some(IssueBackend::GitLab)
            } else {
                None
            }
        }
    }

    fn from_str(s: &str, dir: &Path) -> Result<Option<Self>> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::detect_from_remote(dir)),
            "github" | "gh" => Ok(Some(IssueBackend::GitHub)),
            "gitlab" | "glab" => Ok(Some(IssueBackend::GitLab)),
            _ => anyhow::bail!(
                "Unknown issue backend: {}. Use 'github', 'gitlab', or 'auto'",
                s
            ),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            IssueBackend::GitHub => "GitHub",
            IssueBackend::GitLab => "GitLab",
        }
    }

    fn cli(&self) -> &'static str {
        match self {
            IssueBackend::GitHub => "gh",
            IssueBackend::GitLab => "glab",
        }
    }

    fn install_url(&self) -> &'static str {
        match self {
            IssueBackend::GitHub => "https://cli.github.com/",
            IssueBackend::GitLab => "https://gitlab.com/gitlab-org/cli",
        }
    }
}

pub async fn run(
    config: &Config,
    dir: &Path,
    create_issues: bool,
    issue_backend: &str,
    skip_confirm: bool,
) -> Result<()> {
    let task = config
        .tasks
        .get("hunt")
        .ok_or_else(|| anyhow::anyhow!("Task not found: hunt"))?;

    output::print_task_header("hunt", task.description.as_deref());

    // Detect codebase context
    let context = CodebaseContext::detect(dir);

    // Get backends for this task
    let backend_filter = if task.backends.is_empty() || task.backends.contains(&"all".to_string()) {
        None
    } else {
        Some(task.backends.join(","))
    };

    let backends = backend::get_backends(config, backend_filter.as_deref())?;

    // Collect all results
    let mut all_results: Vec<QueryResult> = Vec::new();

    // Run each prompt
    for prompt_config in &task.prompts {
        output::print_prompt_header(&prompt_config.name);

        // Prepend relevant context based on prompt type
        let prompt_with_context =
            super::prepend_context(&prompt_config.prompt, &prompt_config.name, "hunt", &context);

        let results = backend::run_query(&backends, &prompt_with_context, dir, config).await?;
        output::print_results(&results);

        all_results.extend(results);
    }

    // Create issues if requested
    if create_issues {
        let backend = IssueBackend::from_str(issue_backend, dir)?;
        match backend {
            Some(b) => create_issues_with_backend(&all_results, dir, b, skip_confirm).await?,
            None => {
                anyhow::bail!(
                    "No issue CLI found. Install one of:\n  \
                    - GitHub CLI: https://cli.github.com/\n  \
                    - GitLab CLI: https://gitlab.com/gitlab-org/cli"
                );
            }
        }
    }

    Ok(())
}

async fn create_issues_with_backend(
    results: &[QueryResult],
    dir: &Path,
    backend: IssueBackend,
    skip_confirm: bool,
) -> Result<()> {
    // Check if CLI is available
    if which::which(backend.cli()).is_err() {
        anyhow::bail!(
            "{} CLI ({}) is required for --issues.\n\
            Install it from: {}",
            backend.name(),
            backend.cli(),
            backend.install_url()
        );
    }

    // Check if we're in a repo with a remote
    let repo = get_repo_name(dir, backend)?;

    println!();
    println!("{}", "=".repeat(50).dimmed());
    println!(
        "{} Creating {} issues in {}",
        "issues:".cyan().bold(),
        backend.name(),
        repo.yellow()
    );
    println!();

    // Parse findings from results and create issues
    let findings = parse_findings(results);

    if findings.is_empty() {
        println!(
            "{}",
            "No actionable findings to create issues for.".yellow()
        );
        return Ok(());
    }

    println!("Found {} potential issues:", findings.len());
    for (i, finding) in findings.iter().enumerate() {
        println!("  {}. {}", i + 1, finding.title);
    }
    println!();

    // Confirm before creating (unless --yes flag)
    if !skip_confirm {
        println!("{}", "Create these issues? [y/N] ".cyan());

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("{}", "Aborted.".yellow());
            return Ok(());
        }
    }

    // Create issues
    let mut created = 0;
    for finding in &findings {
        print!("Creating issue: {}... ", finding.title);

        let result = create_issue(dir, backend, &finding.title, &finding.body)?;

        if result.success {
            println!("{} {}", "✓".green(), result.url.dimmed());
            created += 1;
        } else {
            println!("{} {}", "✗".red(), result.error);
        }
    }

    println!();
    println!("{} Created {} issues", "✓".green(), created);

    Ok(())
}

fn get_repo_name(dir: &Path, backend: IssueBackend) -> Result<String> {
    let (cmd, args) = match backend {
        IssueBackend::GitHub => (
            "gh",
            vec![
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "-q",
                ".nameWithOwner",
            ],
        ),
        IssueBackend::GitLab => ("glab", vec!["repo", "view", "--output", "json"]),
    };

    let output = Command::new(cmd)
        .args(&args)
        .current_dir(dir)
        .output()
        .context(format!("Failed to check {} repo", backend.name()))?;

    if !output.status.success() {
        anyhow::bail!(
            "Not in a {} repository or {} not authenticated.\n\
            Run '{} auth login' and ensure you're in a repo with a {} remote.",
            backend.name(),
            backend.cli(),
            backend.cli(),
            backend.name()
        );
    }

    let repo = match backend {
        IssueBackend::GitHub => String::from_utf8_lossy(&output.stdout).trim().to_string(),
        IssueBackend::GitLab => {
            // glab returns JSON, extract path_with_namespace
            let json: serde_json::Value = serde_json::from_slice(&output.stdout)
                .context("Failed to parse glab JSON output")?;
            json["path_with_namespace"]
                .as_str()
                .unwrap_or("unknown")
                .to_string()
        }
    };

    Ok(repo)
}

struct IssueResult {
    success: bool,
    url: String,
    error: String,
}

fn create_issue(dir: &Path, backend: IssueBackend, title: &str, body: &str) -> Result<IssueResult> {
    let result = match backend {
        IssueBackend::GitHub => Command::new("gh")
            .args([
                "issue", "create", "--title", title, "--body", body, "--label", "bug",
            ])
            .current_dir(dir)
            .output()
            .context("Failed to create GitHub issue")?,
        IssueBackend::GitLab => Command::new("glab")
            .args([
                "issue",
                "create",
                "--title",
                title,
                "--description",
                body,
                "--label",
                "bug",
            ])
            .current_dir(dir)
            .output()
            .context("Failed to create GitLab issue")?,
    };

    if result.status.success() {
        let url = String::from_utf8_lossy(&result.stdout).trim().to_string();
        Ok(IssueResult {
            success: true,
            url,
            error: String::new(),
        })
    } else {
        let err = String::from_utf8_lossy(&result.stderr).trim().to_string();
        Ok(IssueResult {
            success: false,
            url: String::new(),
            error: err,
        })
    }
}

#[derive(Debug)]
struct Finding {
    title: String,
    body: String,
}

fn parse_findings(results: &[QueryResult]) -> Vec<Finding> {
    let mut findings = Vec::new();

    for result in results {
        if !result.success {
            continue;
        }

        // Try to parse structured findings from the output
        // Look for patterns like:
        // - "1. **Title**: description"
        // - "## Issue: title"
        // - Numbered lists with file:line references

        let lines: Vec<&str> = result.output.lines().collect();
        let mut current_finding: Option<(String, Vec<String>)> = None;

        for line in &lines {
            let trimmed = line.trim();

            // Check for numbered findings like "1. **Something**" or "1. Something in file.rs:123"
            if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
                let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
                let rest = rest.trim();

                if !rest.is_empty() && rest.len() > 10 {
                    // Save previous finding if exists
                    if let Some((title, body_lines)) = current_finding.take() {
                        if !title.is_empty() {
                            findings.push(Finding {
                                title: truncate_title(&title),
                                body: format!(
                                    "Found by `lok hunt`:\n\n{}\n\n---\n*Backend: {}*",
                                    body_lines.join("\n"),
                                    result.backend
                                ),
                            });
                        }
                    }

                    // Start new finding
                    let title = rest
                        .trim_start_matches('*')
                        .trim_end_matches('*')
                        .trim_start_matches('#')
                        .trim()
                        .to_string();

                    current_finding = Some((title, vec![rest.to_string()]));
                }
            } else if let Some((_, ref mut body)) = current_finding {
                // Add to current finding body
                if !trimmed.is_empty() {
                    body.push(trimmed.to_string());
                }
            }
        }

        // Don't forget the last finding
        if let Some((title, body_lines)) = current_finding {
            if !title.is_empty() {
                findings.push(Finding {
                    title: truncate_title(&title),
                    body: format!(
                        "Found by `lok hunt`:\n\n{}\n\n---\n*Backend: {}*",
                        body_lines.join("\n"),
                        result.backend
                    ),
                });
            }
        }
    }

    // Dedupe by title
    findings.sort_by(|a, b| a.title.cmp(&b.title));
    findings.dedup_by(|a, b| a.title == b.title);

    findings
}

fn truncate_title(title: &str) -> String {
    // GitHub issue titles have a limit, and we want them readable
    let clean = title.replace("**", "").replace("##", "").trim().to_string();

    if clean.len() > 80 {
        format!("{}...", &clean[..77])
    } else {
        clean
    }
}
