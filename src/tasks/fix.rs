use crate::backend;
use crate::config::Config;
use crate::output;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct GitHubIssue {
    number: u64,
    title: String,
    body: Option<String>,
    labels: Vec<GitHubLabel>,
    state: String,
}

#[derive(Debug, Deserialize)]
struct GitHubLabel {
    name: String,
}

pub async fn run(
    config: &Config,
    dir: &Path,
    issue_ref: &str,
    backend_filter: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    // Parse issue reference (number or URL)
    let issue_number = parse_issue_ref(issue_ref)?;

    println!(
        "{} Fetching issue #{}...",
        "fix:".cyan().bold(),
        issue_number
    );

    // Fetch issue details
    let issue = fetch_issue(dir, issue_number)?;

    println!();
    println!("{}", "=".repeat(50).dimmed());
    println!(
        "{} #{}: {}",
        "Issue".cyan().bold(),
        issue.number,
        issue.title
    );
    println!("{}", "=".repeat(50).dimmed());

    if issue.state != "OPEN" {
        println!(
            "{}",
            format!("Warning: Issue is {} (not open)", issue.state).yellow()
        );
    }

    let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
    if !labels.is_empty() {
        println!("{}: {}", "Labels".dimmed(), labels.join(", "));
    }

    println!();
    if let Some(ref body) = issue.body {
        let preview = if body.len() > 500 {
            format!("{}...", &body[..500])
        } else {
            body.clone()
        };
        println!("{}", preview.dimmed());
    }
    println!();

    // Gather relevant code context based on issue content
    let code_context = gather_code_context(dir, &issue)?;

    // Build the fix prompt
    let prompt = build_fix_prompt(&issue, &code_context);

    println!(
        "{} Analyzing issue and generating fix...",
        "fix:".cyan().bold()
    );
    println!();

    // Get backends
    let backends = backend::get_backends(config, backend_filter)?;

    // Run query
    let results = backend::run_query(&backends, &prompt, dir, config).await?;
    output::print_results(&results);

    // If not dry run, try to apply the fix
    if !dry_run {
        println!();
        println!(
            "{}",
            "To apply changes, review the suggestions above and edit files manually.".yellow()
        );
        println!(
            "{}",
            "Future: --apply flag will attempt to apply changes automatically.".dimmed()
        );
    }

    Ok(())
}

fn parse_issue_ref(issue_ref: &str) -> Result<u64> {
    // Handle various formats:
    // - "42" or "#42" - just the number
    // - "https://github.com/owner/repo/issues/42" - full URL

    let trimmed = issue_ref.trim().trim_start_matches('#');

    // Try parsing as number first
    if let Ok(num) = trimmed.parse::<u64>() {
        return Ok(num);
    }

    // Try extracting from URL
    if trimmed.contains("/issues/") {
        if let Some(num_str) = trimmed.split("/issues/").last() {
            if let Ok(num) = num_str.trim_end_matches('/').parse::<u64>() {
                return Ok(num);
            }
        }
    }

    anyhow::bail!(
        "Invalid issue reference: '{}'. Use issue number (42), #42, or full URL.",
        issue_ref
    )
}

fn fetch_issue(dir: &Path, number: u64) -> Result<GitHubIssue> {
    let output = Command::new("gh")
        .args([
            "issue",
            "view",
            &number.to_string(),
            "--json",
            "number,title,body,labels,state",
        ])
        .current_dir(dir)
        .output()
        .context("Failed to run gh command")?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to fetch issue #{}: {}", number, err.trim());
    }

    let issue: GitHubIssue =
        serde_json::from_slice(&output.stdout).context("Failed to parse issue JSON")?;

    Ok(issue)
}

fn gather_code_context(dir: &Path, issue: &GitHubIssue) -> Result<String> {
    let mut context = String::new();

    // Extract file references from issue body
    let body = issue.body.as_deref().unwrap_or("");
    let all_text = format!("{} {}", issue.title, body);

    // Look for file:line patterns like "src/main.rs:42" or "main.rs line 42"
    let file_refs = extract_file_references(&all_text);

    if !file_refs.is_empty() {
        context.push_str("## Referenced files from issue:\n\n");

        for file_ref in &file_refs {
            // Try to read the file
            let file_path = dir.join(&file_ref.path);
            if file_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&file_path) {
                    let lines: Vec<&str> = content.lines().collect();

                    // Show context around the referenced line
                    let start = file_ref.line.saturating_sub(10);
                    let end = (file_ref.line + 10).min(lines.len());

                    context.push_str(&format!("### {}", file_ref.path));
                    if file_ref.line > 0 {
                        context.push_str(&format!(" (around line {})", file_ref.line));
                    }
                    context.push_str("\n```\n");

                    for (i, line) in lines[start..end].iter().enumerate() {
                        let line_num = start + i + 1;
                        let marker = if line_num == file_ref.line {
                            ">>>"
                        } else {
                            "   "
                        };
                        context.push_str(&format!("{} {:4}: {}\n", marker, line_num, line));
                    }

                    context.push_str("```\n\n");
                }
            }
        }
    }

    // Also try to grep for keywords from the issue title
    let keywords = extract_keywords(&issue.title);
    if !keywords.is_empty() && context.is_empty() {
        // Only grep if we didn't find explicit file references
        context.push_str("## Potentially relevant code (from keyword search):\n\n");

        for keyword in keywords.iter().take(3) {
            if let Ok(grep_result) = grep_codebase(dir, keyword) {
                if !grep_result.is_empty() {
                    context.push_str(&format!("### Matches for '{}':\n", keyword));
                    context.push_str("```\n");
                    // Limit grep output
                    let limited: String =
                        grep_result.lines().take(20).collect::<Vec<_>>().join("\n");
                    context.push_str(&limited);
                    context.push_str("\n```\n\n");
                }
            }
        }
    }

    Ok(context)
}

#[derive(Debug)]
struct FileRef {
    path: String,
    line: usize,
}

fn extract_file_references(text: &str) -> Vec<FileRef> {
    let mut refs = Vec::new();

    // Pattern: file.ext:123 or file.ext line 123
    let re = regex::Regex::new(
        r"([a-zA-Z0-9_/.-]+\.(rs|rb|py|js|ts|go|java|c|cpp|h|hpp|tsx|jsx)):(\d+)",
    )
    .unwrap();

    for cap in re.captures_iter(text) {
        let path = cap[1].to_string();
        let line: usize = cap[3].parse().unwrap_or(0);
        refs.push(FileRef { path, line });
    }

    // Dedupe
    refs.sort_by(|a, b| a.path.cmp(&b.path));
    refs.dedup_by(|a, b| a.path == b.path && a.line == b.line);

    refs
}

fn extract_keywords(title: &str) -> Vec<String> {
    // Extract meaningful keywords from title for searching
    let stopwords = [
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "must", "shall",
        "can", "need", "dare", "ought", "used", "to", "of", "in", "for", "on", "with", "at", "by",
        "from", "as", "into", "through", "during", "before", "after", "above", "below", "between",
        "under", "again", "further", "then", "once", "here", "there", "when", "where", "why",
        "how", "all", "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not",
        "only", "own", "same", "so", "than", "too", "very", "just", "and", "but", "if", "or",
        "because", "until", "while", "this", "that", "these", "those", "bug", "fix", "error",
        "issue", "problem", "broken",
    ];

    title
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 3)
        .filter(|w| !stopwords.contains(&w.to_lowercase().as_str()))
        .map(|s| s.to_string())
        .collect()
}

fn grep_codebase(dir: &Path, pattern: &str) -> Result<String> {
    let output = Command::new("rg")
        .args([
            "--max-count",
            "5",
            "-n",
            "--no-heading",
            "-g",
            "!*.lock",
            "-g",
            "!node_modules",
            "-g",
            "!target",
            "-g",
            "!vendor",
            pattern,
        ])
        .current_dir(dir)
        .output()
        .context("Failed to run ripgrep")?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn build_fix_prompt(issue: &GitHubIssue, code_context: &str) -> String {
    let body = issue.body.as_deref().unwrap_or("(no description)");

    format!(
        r#"You are fixing a GitHub issue. Analyze the issue and provide a fix.

## Issue #{}: {}

{}

{}

## Instructions

1. Analyze the issue description and any referenced code
2. Identify the root cause
3. Provide a specific fix with code changes
4. Show the exact changes needed (before/after or unified diff format)
5. Explain why this fix addresses the issue

If you need more context about specific files, say which files you'd need to see.

Provide your fix:"#,
        issue.number, issue.title, body, code_context
    )
}
