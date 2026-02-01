//! Workflow engine - declarative multi-step LLM pipelines
//!
//! Workflows are TOML files that define a sequence of steps, each using
//! a backend to process a prompt. Steps can depend on previous steps
//! and interpolate their outputs.
//!
//! Agentic features:
//! - `shell` steps run shell commands instead of LLM queries
//! - `apply_edits` parses JSON edits from LLM output and applies them
//! - `verify` runs a shell command after edits to validate them

use crate::backend;
use crate::config::Config;
use crate::context::{resolve_format_command, resolve_verify_command, CodebaseContext};
use anyhow::{Context, Result};
use colored::Colorize;
use thiserror::Error;

/// Typed errors for workflow execution
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("Workflow '{workflow}': step '{step}' depends on unknown step '{missing}'\n  hint: check depends_on list")]
    MissingDependency {
        workflow: String,
        step: String,
        missing: String,
    },

    #[error(
        "Workflow '{workflow}': circular dependency detected: {chain}\n  hint: remove the cycle"
    )]
    CircularDependency { workflow: String, chain: String },

    #[error("Workflow '{workflow}': step '{step}' references unknown step '{referenced}' in interpolation\n  hint: ensure the step exists and runs before this one")]
    MissingStepOutput {
        workflow: String,
        step: String,
        referenced: String,
    },

    #[error("Workflow '{workflow}': step '{step}' has unknown variable '{{{{ {variable} }}}}'\n  hint: valid forms are steps.X.output, steps.X.field, env.VAR, arg.N, workflow.backends")]
    UnknownVariable {
        workflow: String,
        step: String,
        variable: String,
    },

    #[error("Workflow '{workflow}': duplicate step names: {}\n  hint: each step must have a unique name", duplicates.join(", "))]
    DuplicateStepNames {
        workflow: String,
        duplicates: Vec<String>,
    },
}
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

/// Regex for matching {{ steps.NAME.output }} patterns
static INTERPOLATE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*steps\.([a-zA-Z0-9_-]+)\.output\s*\}\}").unwrap());

/// Regex for matching "steps.X.output contains 'Y'" conditions (legacy syntax)
static CONDITION_LEGACY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"steps\.([a-zA-Z0-9_-]+)\.output\s+contains\s+['"](.+)['"]"#).unwrap()
});

/// Regex for matching contains(step.field, "string") conditions
/// Captures: (1) step name, (2) field name, (3) search string
static CONDITION_CONTAINS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"contains\(\s*([a-zA-Z0-9_-]+)\.([a-zA-Z0-9_]+)\s*,\s*['"](.+)['"]\s*\)"#)
        .unwrap()
});

/// Regex for matching equals(step.field, "string") conditions
/// Captures: (1) step name, (2) field name, (3) expected value
static CONDITION_EQUALS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"equals\(\s*([a-zA-Z0-9_-]+)\.([a-zA-Z0-9_]+)\s*,\s*['"](.+)['"]\s*\)"#)
        .unwrap()
});

/// Regex for matching not(...) conditions
static CONDITION_NOT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"not\(\s*(.+)\s*\)"#).unwrap());

/// Regex for matching {{ steps.NAME.field }} patterns (for JSON field access)
static FIELD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*steps\.([a-zA-Z0-9_-]+)\.([a-zA-Z0-9_]+)\s*\}\}").unwrap()
});

/// Regex for matching {{ env.VAR }} patterns (environment variables)
static ENV_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*env\.([a-zA-Z0-9_]+)\s*\}\}").unwrap());

/// Regex for matching {{ arg.N }} patterns (positional arguments, 1-indexed)
static ARG_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*arg\.(\d+)\s*\}\}").unwrap());

/// Regex for matching {{ workflow.backends }} pattern
static WORKFLOW_BACKENDS_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*workflow\.backends\s*\}\}").unwrap());

/// Regex for detecting unknown {{ ... }} variables after all substitutions
static UNKNOWN_VAR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*([^}]+)\s*\}\}").unwrap());

/// Regex for matching {{ item }} pattern (loop iteration item)
static ITEM_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*item\s*\}\}").unwrap());

/// Regex for matching {{ item.field }} pattern (loop item field access)
static ITEM_FIELD_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*item\.([a-zA-Z0-9_]+)\s*\}\}").unwrap());

/// Regex for matching {{ index }} pattern (loop iteration index)
static INDEX_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*index\s*\}\}").unwrap());

/// Placeholder for escaped braces - uses a pattern unlikely to appear in real content
const ESCAPED_OPEN_BRACE: &str = "\x00LOK_OPEN_BRACE\x00";

/// Escape {{ in content so it won't be treated as a variable reference
fn escape_braces(s: &str) -> String {
    s.replace("{{", ESCAPED_OPEN_BRACE)
}

/// Restore escaped braces after interpolation is complete
fn unescape_braces(s: &str) -> String {
    s.replace(ESCAPED_OPEN_BRACE, "{{")
}

/// A file edit to apply
#[derive(Debug, Deserialize, Clone)]
pub struct FileEdit {
    pub file: String,
    pub old: String,
    pub new: String,
}

/// Structured output from an LLM step with edits
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields used for JSON schema, extracted via extract_json_field()
pub struct AgenticOutput {
    #[serde(default)]
    pub edits: Vec<FileEdit>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// A workflow definition loaded from TOML
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Extend another workflow by name (inherits steps, can override by name)
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// A single step in a workflow
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Step {
    pub name: String,
    /// Backend to use (e.g. "claude", "codex"). Not needed for shell steps.
    #[serde(default)]
    pub backend: String,
    /// Prompt to send to LLM. Not needed for shell steps.
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional condition - step only runs if this evaluates true
    /// Supports both `when` and `if` in TOML (if takes precedence)
    #[serde(default, alias = "if")]
    pub when: Option<String>,

    // Agentic fields
    /// Shell command to run instead of LLM query
    #[serde(default)]
    pub shell: Option<String>,
    /// Parse JSON edits from output and apply them to files
    #[serde(default)]
    pub apply_edits: bool,
    /// Shell command to run after edits to verify they work
    #[serde(default)]
    pub verify: Option<String>,

    // Retry fields
    /// Number of retry attempts on failure (default 0 = no retries)
    #[serde(default)]
    pub retries: u32,
    /// Base delay between retries in milliseconds (default 1000, doubles each retry)
    #[serde(default = "default_retry_delay")]
    pub retry_delay: u64,

    // Loop fields
    /// Iterate over a JSON array from a previous step or inline array
    /// Examples: "steps.plan.output" or '["a", "b", "c"]'
    #[serde(default)]
    pub for_each: Option<String>,

    // Output parsing fields
    /// How to parse the step output: "text" (default), "json", or "lines"
    #[serde(default)]
    pub output_format: Option<String>,
}

fn default_retry_delay() -> u64 {
    1000
}

/// Parse step output based on format
fn parse_step_output(output: &str, format: Option<&str>) -> Option<serde_json::Value> {
    match format {
        Some("json") => {
            // Try to parse as JSON, extracting from markdown code blocks if needed
            // Check which bracket comes first to determine extraction order
            let array_pos = output.find('[');
            let object_pos = output.find('{');

            let json_str = match (array_pos, object_pos) {
                (Some(a), Some(o)) if a < o => {
                    // Array comes first, try array extraction first
                    extract_json_array_from_text(output).or_else(|| extract_json_from_text(output))
                }
                (Some(_), None) => extract_json_array_from_text(output),
                (None, Some(_)) => extract_json_from_text(output),
                _ => {
                    // Object comes first or neither found
                    extract_json_from_text(output).or_else(|| extract_json_array_from_text(output))
                }
            };

            if let Some(json_str) = json_str {
                serde_json::from_str(&json_str).ok()
            } else {
                // Try direct parse
                serde_json::from_str(output).ok()
            }
        }
        Some("lines") => {
            // Split into array of lines
            let lines: Vec<serde_json::Value> = output
                .lines()
                .map(|s| serde_json::Value::String(s.to_string()))
                .collect();
            Some(serde_json::Value::Array(lines))
        }
        _ => None, // "text" or unspecified - no parsing
    }
}

/// Result of executing a step
#[derive(Debug, Clone)]
pub struct StepResult {
    pub name: String,
    pub output: String,
    /// Parsed output when output_format is "json" or "lines"
    pub parsed_output: Option<serde_json::Value>,
    pub success: bool,
    pub elapsed_ms: u64,
    pub backend: Option<String>,
}

/// Prepared step ready for execution
struct PreparedStep<'a> {
    step: &'a Step,
    prompt: String,
    shell: Option<String>,
    format: Option<String>,
    verify: Option<String>,
    for_each_items: Option<Vec<serde_json::Value>>,
    output_format: Option<String>,
}

/// Workflow executor
pub struct WorkflowRunner {
    config: Config,
    cwd: PathBuf,
    args: Vec<String>,
    context: CodebaseContext,
}

impl WorkflowRunner {
    pub fn new(config: Config, cwd: PathBuf, args: Vec<String>) -> Self {
        let context = CodebaseContext::detect(&cwd);
        Self {
            config,
            cwd,
            args,
            context,
        }
    }

    /// Execute a workflow, returning results for each step
    /// Steps at the same depth level (no dependencies between them) run in parallel
    pub async fn run(&self, workflow: &Workflow) -> Result<Vec<StepResult>> {
        let mut results: HashMap<String, StepResult> = HashMap::new();
        let mut ordered_results: Vec<StepResult> = Vec::new();

        // Group steps by depth level for parallel execution
        let depth_levels = self.group_by_depth(&workflow.steps, &workflow.name)?;

        println!("{} {}", "Running workflow:".bold(), workflow.name.cyan());
        if let Some(ref desc) = workflow.description {
            println!("{}", desc.dimmed());
        }
        println!("{}", "=".repeat(50).dimmed());
        println!();

        // Build step lookup map for O(1) access instead of O(n) linear scans
        let step_map: HashMap<&str, &Step> = workflow
            .steps
            .iter()
            .map(|s| (s.name.as_str(), s))
            .collect();

        for (depth, step_names) in depth_levels.iter().enumerate() {
            let parallel_count = step_names.len();
            if parallel_count > 1 {
                println!(
                    "{} Running {} steps in parallel (depth {})",
                    "[parallel]".cyan(),
                    parallel_count,
                    depth
                );
            }

            // Collect steps to run at this depth
            let mut steps_to_run: Vec<PreparedStep> = Vec::new();

            for step_name in step_names {
                let step = *step_map
                    .get(step_name.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Step '{}' not found in workflow", step_name))?;

                // Check condition if present
                if let Some(ref condition) = step.when {
                    if !self.evaluate_condition(condition, &results) {
                        println!(
                            "{} {} (condition not met)",
                            "[skip]".yellow(),
                            step.name.bold()
                        );
                        continue;
                    }
                }

                // Interpolate variables in prompt/shell (uses results from previous depths)
                let prompt = self.interpolate_with_fields(
                    &step.prompt,
                    &results,
                    &workflow.name,
                    &step.name,
                )?;
                let shell = step
                    .shell
                    .as_ref()
                    .map(|s| self.interpolate_with_fields(s, &results, &workflow.name, &step.name))
                    .transpose()?;
                // When verify is set, also resolve format command to run first
                let verify_value = step
                    .verify
                    .as_ref()
                    .map(|v| self.interpolate_with_fields(v, &results, &workflow.name, &step.name))
                    .transpose()?;
                let format = verify_value
                    .as_ref()
                    .and_then(|v| resolve_format_command(v, &self.context));
                let verify = verify_value.and_then(|v| resolve_verify_command(&v, &self.context));

                // Parse for_each array if present
                let for_each_items = step
                    .for_each
                    .as_ref()
                    .map(|fe| parse_for_each_array(fe, &results))
                    .transpose()
                    .map_err(|e| anyhow::anyhow!("Step '{}': {}", step.name, e))?;

                steps_to_run.push(PreparedStep {
                    step,
                    prompt,
                    shell,
                    format,
                    verify,
                    for_each_items,
                    output_format: step.output_format.clone(),
                });
            }

            if steps_to_run.is_empty() {
                continue;
            }

            // Execute steps at this depth in parallel
            let futures: Vec<_> = steps_to_run
                .into_iter()
                .map(|prepared| {
                    let PreparedStep {
                        step,
                        prompt,
                        shell,
                        format,
                        verify,
                        for_each_items,
                        output_format,
                    } = prepared;
                    let config = self.config.clone();
                    let cwd = self.cwd.clone();
                    let step_name = step.name.clone();
                    let backend_name = step.backend.clone();
                    let apply_edits_flag = step.apply_edits;
                    let max_retries = step.retries;
                    let retry_delay = step.retry_delay;

                    async move {
                        println!("{} {}", "[step]".cyan(), step_name.bold());
                        let start = std::time::Instant::now();

                        // Handle for_each loop steps
                        if let Some(items) = for_each_items {
                            println!(
                                "  {} iterating over {} items",
                                "[loop]".cyan(),
                                items.len()
                            );

                            let mut iteration_results: Vec<serde_json::Value> = Vec::new();
                            let mut all_success = true;

                            for (index, item) in items.iter().enumerate() {
                                // Interpolate item/index into prompt and shell
                                let iter_prompt = interpolate_loop_vars(&prompt, item, index);
                                let iter_shell = shell.as_ref().map(|s| interpolate_loop_vars(s, item, index));

                                println!(
                                    "    {} [{}/{}]",
                                    "→".dimmed(),
                                    index + 1,
                                    items.len()
                                );

                                let iter_output: String;
                                let iter_success: bool;

                                // Shell iteration
                                if let Some(ref shell_cmd) = iter_shell {
                                    match run_shell(shell_cmd, &cwd) {
                                        Ok(output) => {
                                            iter_output = output;
                                            iter_success = true;
                                        }
                                        Err(e) => {
                                            iter_output = format!("Error: {}", e);
                                            iter_success = false;
                                            all_success = false;
                                        }
                                    }
                                } else {
                                    // LLM iteration
                                    let backend_config = match config.backends.get(&backend_name) {
                                        Some(cfg) => cfg,
                                        None => {
                                            iter_output = format!("Backend not found: {}", backend_name);
                                            iter_success = false;
                                            all_success = false;
                                            iteration_results.push(serde_json::json!({
                                                "index": index,
                                                "item": item,
                                                "output": iter_output,
                                                "success": iter_success
                                            }));
                                            continue;
                                        }
                                    };

                                    let backend = match backend::create_backend(&backend_name, backend_config) {
                                        Ok(b) => b,
                                        Err(e) => {
                                            iter_output = format!("Failed to create backend: {}", e);
                                            iter_success = false;
                                            all_success = false;
                                            iteration_results.push(serde_json::json!({
                                                "index": index,
                                                "item": item,
                                                "output": iter_output,
                                                "success": iter_success
                                            }));
                                            continue;
                                        }
                                    };

                                    match backend.query(&iter_prompt, &cwd).await {
                                        Ok(text) => {
                                            iter_output = text;
                                            iter_success = true;
                                        }
                                        Err(e) => {
                                            iter_output = format!("Error: {}", e);
                                            iter_success = false;
                                            all_success = false;
                                        }
                                    }
                                }

                                let status = if iter_success { "✓".green() } else { "✗".red() };
                                println!("      {} iteration {}", status, index);

                                iteration_results.push(serde_json::json!({
                                    "index": index,
                                    "item": item,
                                    "output": iter_output,
                                    "success": iter_success
                                }));
                            }

                            let elapsed_ms = start.elapsed().as_millis() as u64;
                            let output_json = serde_json::to_string_pretty(&iteration_results)
                                .unwrap_or_else(|_| "[]".to_string());

                            println!(
                                "  {} ({:.1}s, {} iterations)",
                                if all_success { "✓".green() } else { "⚠".yellow() },
                                elapsed_ms as f64 / 1000.0,
                                items.len()
                            );

                            return StepResult {
                                name: step_name,
                                output: output_json,
                                parsed_output: None,
                                success: all_success,
                                elapsed_ms,
                                backend: if shell.is_none() { Some(backend_name) } else { None },
                            };
                        }

                        // Shell step - run command directly (with retry support)
                        if let Some(ref shell_cmd) = shell {
                            println!("  {} {}", "shell:".dimmed(), shell_cmd.dimmed());

                            let mut last_error = String::new();
                            for attempt in 0..=max_retries {
                                if attempt > 0 {
                                    let delay = retry_delay * 2_u64.pow(attempt - 1);
                                    println!(
                                        "  {} Retry {}/{} in {}ms...",
                                        "↻".yellow(),
                                        attempt,
                                        max_retries,
                                        delay
                                    );
                                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                                }

                                match run_shell(shell_cmd, &cwd) {
                                    Ok(output) => {
                                        let elapsed_ms = start.elapsed().as_millis() as u64;
                                        println!(
                                            "  {} ({:.1}s)",
                                            "✓".green(),
                                            elapsed_ms as f64 / 1000.0
                                        );
                                        let parsed = parse_step_output(
                                            &output,
                                            output_format.as_deref(),
                                        );
                                        return StepResult {
                                            name: step_name,
                                            output,
                                            parsed_output: parsed,
                                            success: true,
                                            elapsed_ms,
                                            backend: None,
                                        };
                                    }
                                    Err(e) => {
                                        last_error = e.to_string();
                                        if attempt == max_retries {
                                            let elapsed_ms = start.elapsed().as_millis() as u64;
                                            println!("  {} {}", "✗".red(), e);
                                            return StepResult {
                                                name: step_name,
                                                output: format!("Error: {}", e),
                                                parsed_output: None,
                                                success: false,
                                                elapsed_ms,
                                                backend: None,
                                            };
                                        }
                                        println!("  {} {} (will retry)", "⚠".yellow(), e);
                                    }
                                }
                            }

                            // Should never reach here, but just in case
                            let elapsed_ms = start.elapsed().as_millis() as u64;
                            return StepResult {
                                name: step_name,
                                output: format!("Error: {}", last_error),
                                parsed_output: None,
                                success: false,
                                elapsed_ms,
                                backend: None,
                            };
                        }

                        // LLM step - query backend
                        let backend_config = match config.backends.get(&backend_name) {
                            Some(cfg) => cfg,
                            None => {
                                return StepResult {
                                    name: step_name,
                                    output: format!("Backend not found: {}", backend_name),
                                    parsed_output: None,
                                    success: false,
                                    elapsed_ms: 0,
                                    backend: Some(backend_name),
                                };
                            }
                        };

                        let backend = match backend::create_backend(&backend_name, backend_config) {
                            Ok(b) => b,
                            Err(e) => {
                                return StepResult {
                                    name: step_name,
                                    output: format!("Failed to create backend: {}", e),
                                    parsed_output: None,
                                    success: false,
                                    elapsed_ms: 0,
                                    backend: Some(backend_name),
                                };
                            }
                        };

                        if !backend.is_available() {
                            println!("  {} Backend not available", "✗".red());
                            return StepResult {
                                name: step_name,
                                output: format!("Backend {} not available", backend_name),
                                parsed_output: None,
                                success: false,
                                elapsed_ms: 0,
                                backend: Some(backend_name),
                            };
                        }

                        // Execute LLM query (with retry support)
                        let mut last_error = String::new();
                        let mut text = String::new();
                        let mut query_success = false;

                        for attempt in 0..=max_retries {
                            if attempt > 0 {
                                let delay = retry_delay * 2_u64.pow(attempt - 1);
                                println!(
                                    "  {} Retry {}/{} in {}ms...",
                                    "↻".yellow(),
                                    attempt,
                                    max_retries,
                                    delay
                                );
                                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                            }

                            match backend.query(&prompt, &cwd).await {
                                Ok(t) => {
                                    text = t;
                                    query_success = true;
                                    break;
                                }
                                Err(e) => {
                                    last_error = e.to_string();
                                    if attempt == max_retries {
                                        let elapsed_ms = start.elapsed().as_millis() as u64;
                                        println!("  {} {}", "✗".red(), e);
                                        return StepResult {
                                            name: step_name,
                                            output: format!("Error: {}", e),
                                            parsed_output: None,
                                            success: false,
                                            elapsed_ms,
                                            backend: Some(backend_name),
                                        };
                                    }
                                    println!("  {} {} (will retry)", "⚠".yellow(), e);
                                }
                            }
                        }

                        let elapsed_ms = start.elapsed().as_millis() as u64;

                        if query_success {
                            println!("  {} ({:.1}s)", "✓".green(), elapsed_ms as f64 / 1000.0);

                                // Apply edits if requested
                                if apply_edits_flag {
                                    println!("  {} Applying edits...", "→".cyan());
                                    match parse_edits(&text) {
                                        Ok(agentic) => {
                                            if agentic.edits.is_empty() {
                                                println!(
                                                    "    {} No edits found in output",
                                                    "⚠".yellow()
                                                );
                                            } else {
                                                match apply_edits(&agentic.edits, &cwd).await {
                                                    Ok(count) => {
                                                        println!(
                                                            "    {} Applied {} edit(s)",
                                                            "✓".green(),
                                                            count
                                                        );
                                                    }
                                                    Err(e) => {
                                                        println!(
                                                            "    {} Failed to apply edits: {}",
                                                            "✗".red(),
                                                            e
                                                        );
                                                        return StepResult {
                                                            name: step_name,
                                                            output: format!(
                                                                "Edit failed: {}\n\nOriginal output:\n{}",
                                                                e, text
                                                            ),
                                                            parsed_output: None,
                                                            success: false,
                                                            elapsed_ms,
                                                            backend: Some(backend_name.clone()),
                                                        };
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            println!(
                                                "    {} Failed to parse edits: {}",
                                                "✗".red(),
                                                e
                                            );
                                            return StepResult {
                                                name: step_name,
                                                output: format!(
                                                    "Parse failed: {}\n\nOriginal output:\n{}",
                                                    e, text
                                                ),
                                                parsed_output: None,
                                                success: false,
                                                elapsed_ms,
                                                backend: Some(backend_name.clone()),
                                            };
                                        }
                                    }
                                }

                                // Run format before verify if requested
                                if let Some(ref format_cmd) = format {
                                    println!("  {} {}", "format:".dimmed(), format_cmd.dimmed());
                                    match run_shell(format_cmd, &cwd) {
                                        Ok(_) => {
                                            println!("    {} Format complete", "✓".green());
                                        }
                                        Err(e) => {
                                            println!(
                                                "    {} Format failed: {}",
                                                "✗".red(),
                                                e
                                            );
                                            // Format failure is not fatal, continue to verify
                                        }
                                    }
                                }

                                // Run verification if requested
                                if let Some(ref verify_cmd) = verify {
                                    println!("  {} {}", "verify:".dimmed(), verify_cmd.dimmed());
                                    match run_shell(verify_cmd, &cwd) {
                                        Ok(_) => {
                                            println!("    {} Verification passed", "✓".green());
                                        }
                                        Err(e) => {
                                            println!(
                                                "    {} Verification failed: {}",
                                                "✗".red(),
                                                e
                                            );
                                            return StepResult {
                                                name: step_name,
                                                output: format!(
                                                    "Verification failed: {}\n\nOriginal output:\n{}",
                                                    e, text
                                                ),
                                                parsed_output: None,
                                                success: false,
                                                elapsed_ms,
                                                backend: Some(backend_name.clone()),
                                            };
                                        }
                                    }
                                }

                            let parsed = parse_step_output(
                                &text,
                                output_format.as_deref(),
                            );
                            StepResult {
                                name: step_name,
                                output: text,
                                parsed_output: parsed,
                                success: true,
                                elapsed_ms,
                                backend: Some(backend_name),
                            }
                        } else {
                            // Should never reach here given retry loop logic, but just in case
                            StepResult {
                                name: step_name,
                                output: format!("Error: {}", last_error),
                                parsed_output: None,
                                success: false,
                                elapsed_ms,
                                backend: Some(backend_name),
                            }
                        }
                    }
                })
                .collect();

            // Wait for all steps at this depth to complete
            let level_results = join_all(futures).await;

            // Store results for use by dependent steps
            for result in level_results {
                results.insert(result.name.clone(), result.clone());
                ordered_results.push(result);
            }
        }

        println!();
        println!("{}", "=".repeat(50).dimmed());

        Ok(ordered_results)
    }

    /// Group steps by depth level for parallel execution
    /// Depth 0 = no dependencies, Depth N = depends on steps at depth < N
    fn group_by_depth(&self, steps: &[Step], workflow_name: &str) -> Result<Vec<Vec<String>>> {
        // Validate no duplicate step names (HashMap would silently overwrite)
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for step in steps {
            *seen.entry(step.name.as_str()).or_insert(0) += 1;
        }
        let duplicates: Vec<String> = seen
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, _)| name.to_string())
            .collect();
        if !duplicates.is_empty() {
            return Err(WorkflowError::DuplicateStepNames {
                workflow: workflow_name.to_string(),
                duplicates,
            }
            .into());
        }

        // Build step lookup map for O(1) access instead of O(n) linear scans
        let step_map: HashMap<&str, &Step> = steps.iter().map(|s| (s.name.as_str(), s)).collect();

        // Validate dependencies exist
        for step in steps {
            for dep in &step.depends_on {
                if !step_map.contains_key(dep.as_str()) {
                    return Err(WorkflowError::MissingDependency {
                        workflow: workflow_name.to_string(),
                        step: step.name.clone(),
                        missing: dep.clone(),
                    }
                    .into());
                }
            }
        }

        // Calculate depth for each step
        let mut depths: HashMap<String, usize> = HashMap::new();

        fn calc_depth(
            name: &str,
            step_map: &HashMap<&str, &Step>,
            depths: &mut HashMap<String, usize>,
            visiting: &mut Vec<String>, // Vec to preserve order for chain tracking
            workflow_name: &str,
        ) -> Result<usize> {
            if let Some(&d) = depths.get(name) {
                return Ok(d);
            }

            // Check for circular dependency and build chain
            if let Some(pos) = visiting.iter().position(|v| v == name) {
                let mut chain: Vec<_> = visiting[pos..].to_vec();
                chain.push(name.to_string());
                return Err(WorkflowError::CircularDependency {
                    workflow: workflow_name.to_string(),
                    chain: chain.join(" -> "),
                }
                .into());
            }

            visiting.push(name.to_string());

            let step = step_map
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("Step '{}' not found in workflow", name))?;
            let depth = if step.depends_on.is_empty() {
                0
            } else {
                let max_dep_depth = step
                    .depends_on
                    .iter()
                    .map(|dep| calc_depth(dep, step_map, depths, visiting, workflow_name))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .max()
                    .unwrap_or(0);
                max_dep_depth + 1
            };

            visiting.pop();
            depths.insert(name.to_string(), depth);
            Ok(depth)
        }

        let mut visiting = Vec::new();
        for step in steps {
            calc_depth(
                &step.name,
                &step_map,
                &mut depths,
                &mut visiting,
                workflow_name,
            )?;
        }

        // Group by depth
        let max_depth = depths.values().copied().max().unwrap_or(0);
        let mut levels: Vec<Vec<String>> = vec![Vec::new(); max_depth + 1];

        for (name, depth) in depths {
            levels[depth].push(name);
        }

        Ok(levels)
    }

    /// Interpolate {{ steps.X.output }} variables in a string
    ///
    /// Uses replace_all for O(n) complexity instead of O(n*m) with repeated replace()
    /// Step outputs are escaped to prevent their content from being treated as variables.
    fn interpolate(
        &self,
        template: &str,
        results: &HashMap<String, StepResult>,
        workflow_name: &str,
        current_step: &str,
    ) -> Result<String, WorkflowError> {
        // First pass: validate all step references exist
        for cap in INTERPOLATE_RE.captures_iter(template) {
            let referenced_step = cap.get(1).expect("regex group 1 always exists").as_str();
            if !results.contains_key(referenced_step) {
                return Err(WorkflowError::MissingStepOutput {
                    workflow: workflow_name.to_string(),
                    step: current_step.to_string(),
                    referenced: referenced_step.to_string(),
                });
            }
        }

        // Second pass: replace all in one pass (O(n) instead of O(n*m))
        // Escape {{ in step outputs so they don't get treated as variables
        let output = INTERPOLATE_RE
            .replace_all(template, |caps: &regex::Captures| {
                let step = &caps[1];
                results
                    .get(step)
                    .map(|r| escape_braces(&r.output))
                    .unwrap_or_default()
            })
            .into_owned();

        Ok(output)
    }

    /// Evaluate a condition expression
    ///
    /// Supported syntax:
    /// - `contains(step.output, "string")` - true if step output contains string
    /// - `equals(step.output, "string")` - true if step output equals string exactly
    /// - `not(condition)` - negates the inner condition
    /// - `steps.X.output contains 'Y'` - legacy syntax, still supported
    fn evaluate_condition(&self, condition: &str, results: &HashMap<String, StepResult>) -> bool {
        // Handle not(...) wrapper first
        if let Some(caps) = CONDITION_NOT_RE.captures(condition) {
            let inner = caps.get(1).unwrap().as_str().trim();
            return !self.evaluate_condition(inner, results);
        }

        // Handle contains(step.field, "string")
        if let Some(caps) = CONDITION_CONTAINS_RE.captures(condition) {
            let step_name = caps.get(1).unwrap().as_str();
            let field_name = caps.get(2).unwrap().as_str();
            let search_str = caps.get(3).unwrap().as_str();
            return results
                .get(step_name)
                .map(|r| {
                    let value = if field_name == "output" {
                        r.output.clone()
                    } else {
                        // Extract JSON field from output
                        extract_json_field(&r.output, field_name).unwrap_or_default()
                    };
                    value.contains(search_str)
                })
                .unwrap_or(false);
        }

        // Handle equals(step.field, "string")
        if let Some(caps) = CONDITION_EQUALS_RE.captures(condition) {
            let step_name = caps.get(1).unwrap().as_str();
            let field_name = caps.get(2).unwrap().as_str();
            let expected = caps.get(3).unwrap().as_str();
            return results
                .get(step_name)
                .map(|r| {
                    let value = if field_name == "output" {
                        r.output.trim().to_string()
                    } else {
                        // Extract JSON field from output
                        extract_json_field(&r.output, field_name).unwrap_or_default()
                    };
                    value == expected
                })
                .unwrap_or(false);
        }

        // Legacy syntax: "steps.X.output contains 'Y'"
        if let Some(caps) = CONDITION_LEGACY_RE.captures(condition) {
            let step_name = caps.get(1).unwrap().as_str();
            let search_str = caps.get(2).unwrap().as_str();
            return results
                .get(step_name)
                .map(|r| r.output.contains(search_str))
                .unwrap_or(false);
        }

        // Default: if we can't parse, return true (run the step)
        true
    }

    /// Interpolate with JSON field access: {{ steps.X.field }} and env vars: {{ env.VAR }}
    ///
    /// Uses replace_all for O(n) complexity per pattern instead of O(n*m) with repeated replace()
    fn interpolate_with_fields(
        &self,
        template: &str,
        results: &HashMap<String, StepResult>,
        workflow_name: &str,
        current_step: &str,
    ) -> Result<String, WorkflowError> {
        let output = self.interpolate(template, results, workflow_name, current_step)?;

        // Handle {{ steps.X.field }} for JSON field access
        // First validate all step references exist
        for cap in FIELD_RE.captures_iter(&output) {
            let referenced_step = cap.get(1).expect("regex group 1 always exists").as_str();
            let field_name = cap.get(2).expect("regex group 2 always exists").as_str();
            if field_name != "output" && !results.contains_key(referenced_step) {
                return Err(WorkflowError::MissingStepOutput {
                    workflow: workflow_name.to_string(),
                    step: current_step.to_string(),
                    referenced: referenced_step.to_string(),
                });
            }
        }
        // Then replace all in one pass
        let output = FIELD_RE
            .replace_all(&output, |caps: &regex::Captures| {
                let step = &caps[1];
                let field = &caps[2];
                if field == "output" {
                    // Already handled by interpolate(), return original match
                    caps[0].to_string()
                } else {
                    // Try parsed_output first if available, then fall back to string parsing
                    results
                        .get(step)
                        .and_then(|r| {
                            // Use parsed_output if available
                            if let Some(ref parsed) = r.parsed_output {
                                parsed.get(field).map(|v| match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                })
                            } else {
                                // Fall back to parsing from string
                                extract_json_field(&r.output, field)
                            }
                        })
                        .unwrap_or_else(|| format!("[field {} not found]", field))
                }
            })
            .into_owned();

        // Handle {{ env.VAR }} for environment variables - single pass
        let output = ENV_RE
            .replace_all(&output, |caps: &regex::Captures| {
                let var_name = &caps[1];
                std::env::var(var_name).unwrap_or_else(|_| format!("[env {} not set]", var_name))
            })
            .into_owned();

        // Handle {{ arg.N }} for positional arguments (1-indexed) - single pass
        let output = ARG_RE
            .replace_all(&output, |caps: &regex::Captures| {
                let arg_index: usize = caps[1].parse().unwrap_or(0);
                if arg_index > 0 && arg_index <= self.args.len() {
                    self.args[arg_index - 1].clone()
                } else {
                    format!("[arg {} not provided]", arg_index)
                }
            })
            .into_owned();

        // Handle {{ workflow.backends }} - list unique backends used - single pass
        let output = if WORKFLOW_BACKENDS_RE.is_match(&output) {
            let mut backends: Vec<String> =
                results.values().filter_map(|r| r.backend.clone()).collect();
            backends.sort();
            backends.dedup();

            // Capitalize first letter of each backend name
            let formatted: Vec<String> = backends
                .iter()
                .map(|b| {
                    let mut chars = b.chars();
                    match chars.next() {
                        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect();

            let replacement = if formatted.is_empty() {
                "lok".to_string()
            } else {
                formatted.join(" + ")
            };

            WORKFLOW_BACKENDS_RE
                .replace_all(&output, &replacement)
                .into_owned()
        } else {
            output
        };

        // Check for any remaining unknown {{ ... }} variables
        // Skip item, item.field, and index - those are handled by for_each loops
        for cap in UNKNOWN_VAR_RE.captures_iter(&output) {
            let variable = cap
                .get(1)
                .expect("regex group 1 always exists")
                .as_str()
                .trim();

            // Skip loop variables - they'll be interpolated later by for_each
            if variable == "item" || variable == "index" || variable.starts_with("item.") {
                continue;
            }

            return Err(WorkflowError::UnknownVariable {
                workflow: workflow_name.to_string(),
                step: current_step.to_string(),
                variable: variable.to_string(),
            });
        }

        // Restore escaped braces from step outputs
        Ok(unescape_braces(&output))
    }
}

/// Interpolate loop variables ({{ item }}, {{ item.field }}, {{ index }}) in a string
fn interpolate_loop_vars(template: &str, item: &serde_json::Value, index: usize) -> String {
    // Handle {{ item.field }} for object field access first
    let output = ITEM_FIELD_RE
        .replace_all(template, |caps: &regex::Captures| {
            let field = &caps[1];
            item.get(field)
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| format!("[item.{} not found]", field))
        })
        .into_owned();

    // Handle {{ item }} for the whole item
    let output = ITEM_RE
        .replace_all(&output, |_: &regex::Captures| match item {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .into_owned();

    // Handle {{ index }} for iteration index
    INDEX_RE
        .replace_all(&output, index.to_string().as_str())
        .into_owned()
}

/// Parse for_each value into a JSON array
/// Can be a reference to previous step (steps.X.output) or an inline JSON array
fn parse_for_each_array(
    for_each: &str,
    results: &HashMap<String, StepResult>,
) -> Result<Vec<serde_json::Value>> {
    // Try to parse as inline JSON array first
    if for_each.trim().starts_with('[') {
        let array: Vec<serde_json::Value> =
            serde_json::from_str(for_each).context("Failed to parse for_each as JSON array")?;
        return Ok(array);
    }

    // Parse as step reference: steps.X.output
    let step_ref_re = regex::Regex::new(r"^steps\.([a-zA-Z0-9_-]+)\.output$").unwrap();
    if let Some(caps) = step_ref_re.captures(for_each) {
        let step_name = &caps[1];
        let step_result = results
            .get(step_name)
            .ok_or_else(|| anyhow::anyhow!("for_each: step '{}' not found", step_name))?;

        // If parsed_output is available and is an array, use it directly
        if let Some(ref parsed) = step_result.parsed_output {
            match parsed {
                serde_json::Value::Array(arr) => return Ok(arr.clone()),
                _ => {
                    return Err(anyhow::anyhow!(
                        "for_each: step '{}' parsed_output is not an array",
                        step_name
                    ))
                }
            }
        }

        // Fall back to string parsing for backwards compatibility
        // Try to extract JSON from the step output
        // For for_each, prefer array extraction since we expect an array
        // Check which comes first: [ or { to decide extraction order
        let output = &step_result.output;
        let array_pos = output.find('[');
        let object_pos = output.find('{');

        let json_str = match (array_pos, object_pos) {
            (Some(a), Some(o)) if a < o => {
                // Array comes first, try array extraction first
                extract_json_array_from_text(output).or_else(|| extract_json_from_text(output))
            }
            _ => {
                // Object comes first or only one exists
                extract_json_from_text(output).or_else(|| extract_json_array_from_text(output))
            }
        }
        .ok_or_else(|| anyhow::anyhow!("for_each: no JSON found in step '{}' output", step_name))?;

        let value: serde_json::Value = serde_json::from_str(&json_str)
            .or_else(|_| serde_json::from_str(&sanitize_json_strings(&json_str)))
            .context(format!(
                "for_each: failed to parse JSON from step '{}'",
                step_name
            ))?;

        match value {
            serde_json::Value::Array(arr) => Ok(arr),
            _ => Err(anyhow::anyhow!(
                "for_each: step '{}' output is not a JSON array",
                step_name
            )),
        }
    } else {
        Err(anyhow::anyhow!(
            "for_each: invalid format '{}'. Use 'steps.X.output' or inline JSON array",
            for_each
        ))
    }
}

/// Run a shell command and return output
fn run_shell(cmd: &str, cwd: &Path) -> Result<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .output()
        .context("Failed to execute shell command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        anyhow::bail!("Shell command failed: {}\n{}", cmd, stderr);
    }

    Ok(format!("{}{}", stdout, stderr).trim().to_string())
}

/// Extract JSON array from text (similar to extract_json_from_text but for arrays)
fn extract_json_array_from_text(text: &str) -> Option<String> {
    // Try to find raw JSON array
    if let Some(start) = text.find('[') {
        // Find matching closing bracket
        let mut depth = 0;
        let mut end = start;
        for (i, c) in text[start..].char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth == 0 && end > start {
            return Some(text[start..end].to_string());
        }
    }

    None
}

/// Extract a field from JSON in text (handles markdown code blocks)
fn extract_json_field(text: &str, field: &str) -> Option<String> {
    // Try to find JSON in the text (may be wrapped in ```json blocks)
    let json_str = extract_json_from_text(text)?;

    // Try parsing, and if it fails due to control characters, sanitize and retry
    let value: serde_json::Value = serde_json::from_str(&json_str)
        .or_else(|_| {
            // LLMs sometimes output literal newlines/tabs in JSON strings instead of \n\t escapes
            // Sanitize by escaping control characters inside string values
            let sanitized = sanitize_json_strings(&json_str);
            serde_json::from_str(&sanitized)
        })
        .ok()?;

    value.get(field).map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// Sanitize JSON by escaping control characters inside string values
fn sanitize_json_strings(json: &str) -> String {
    let mut result = String::with_capacity(json.len());
    let mut in_string = false;
    for c in json.chars() {
        if c == '"' && !result.ends_with('\\') {
            in_string = !in_string;
            result.push(c);
        } else if in_string && c.is_control() {
            // Escape control characters inside strings
            match c {
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                _ => {
                    // Other control chars: use unicode escape
                    result.push_str(&format!("\\u{:04x}", c as u32));
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Find the closing fence for a markdown code block.
/// Must be on its own line (after a newline) to avoid matching ``` inside content.
/// Returns position where content ends (the newline before the fence).
fn find_closing_fence(text: &str) -> Option<usize> {
    // Look for \n``` to find fence at start of line
    if let Some(pos) = text.find("\n```") {
        return Some(pos); // Return position of newline (where content ends)
    }
    // If content starts right after opening fence, check for ``` at very start
    if text.starts_with("```") {
        return Some(0);
    }
    None
}

/// Extract JSON object from text, handling markdown code blocks
fn extract_json_from_text(text: &str) -> Option<String> {
    // Try to find ```json ... ``` block first
    if let Some(start) = text.find("```json") {
        let after_marker = &text[start + 7..];
        if let Some(end) = find_closing_fence(after_marker) {
            return Some(after_marker[..end].trim().to_string());
        }
    }

    // Try to find ``` ... ``` block
    if let Some(start) = text.find("```") {
        let after_marker = &text[start + 3..];
        if let Some(end) = find_closing_fence(after_marker) {
            let content = after_marker[..end].trim();
            // Skip language identifier if present
            let json_content = if content.starts_with('{') {
                content
            } else if let Some(newline) = content.find('\n') {
                content[newline + 1..].trim()
            } else {
                content
            };
            if json_content.starts_with('{') {
                return Some(json_content.to_string());
            }
        }
    }

    // Try to find raw JSON object
    if let Some(start) = text.find('{') {
        // Find matching closing brace
        let mut depth = 0;
        let mut end = start;
        for (i, c) in text[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth == 0 && end > start {
            return Some(text[start..end].to_string());
        }
    }

    None
}

/// Parse edits from LLM output
fn parse_edits(text: &str) -> Result<AgenticOutput> {
    let json_str = extract_json_from_text(text).context("No JSON found in output")?;
    serde_json::from_str(&json_str).or_else(|first_err| {
        serde_json::from_str(&sanitize_json_strings(&json_str)).map_err(|second_err| {
            anyhow::anyhow!(
                "Failed to parse edits JSON.\nFirst attempt: {}\nAfter sanitization: {}",
                first_err,
                second_err
            )
        })
    })
}

/// Apply file edits
async fn apply_edits(edits: &[FileEdit], cwd: &Path) -> Result<usize> {
    let mut applied = 0;

    for edit in edits {
        let file_path = cwd.join(&edit.file);

        let content = match tokio::fs::read_to_string(&file_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!("File not found: {}", edit.file);
            }
            Err(e) => {
                return Err(e).context(format!("Failed to read {}", edit.file));
            }
        };

        let match_count = content.matches(&edit.old).count();
        if match_count == 0 {
            anyhow::bail!(
                "Old text not found in {}: {}",
                edit.file,
                edit.old.chars().take(50).collect::<String>()
            );
        }
        if match_count > 1 {
            anyhow::bail!(
                "Ambiguous edit: old text appears {} times in {}. Make the edit more specific.",
                match_count,
                edit.file
            );
        }

        let new_content = content.replacen(&edit.old, &edit.new, 1);
        tokio::fs::write(&file_path, new_content)
            .await
            .context(format!("Failed to write {}", edit.file))?;

        println!("    {} {}", "edited".green(), edit.file);
        applied += 1;
    }

    Ok(applied)
}

/// Find workflow file by name, checking project-local and global paths
pub async fn find_workflow(name: &str) -> Result<PathBuf> {
    // If it's already a path, use it directly
    let path = Path::new(name);
    if tokio::fs::metadata(path).await.is_ok() {
        return Ok(path.to_path_buf());
    }

    // Add .toml extension if not present
    let filename = if name.ends_with(".toml") {
        name.to_string()
    } else {
        format!("{}.toml", name)
    };

    // Check project-local .lok/workflows/
    let local_path = PathBuf::from(".lok/workflows").join(&filename);
    if tokio::fs::metadata(&local_path).await.is_ok() {
        return Ok(local_path);
    }

    // Check global ~/.config/lok/workflows/
    if let Some(home) = dirs::home_dir() {
        let global_path = home.join(".config/lok/workflows").join(&filename);
        if tokio::fs::metadata(&global_path).await.is_ok() {
            return Ok(global_path);
        }
    }

    anyhow::bail!(
        "Workflow '{}' not found. Searched:\n  - .lok/workflows/{}\n  - ~/.config/lok/workflows/{}",
        name,
        filename,
        filename
    )
}

/// List all available workflows
pub async fn list_workflows() -> Result<Vec<(PathBuf, Workflow)>> {
    let mut workflows = Vec::new();

    // Check project-local
    let local_dir = PathBuf::from(".lok/workflows");
    if tokio::fs::metadata(&local_dir).await.is_ok() {
        workflows.extend(load_workflows_from_dir(&local_dir).await?);
    }

    // Check global
    if let Some(home) = dirs::home_dir() {
        let global_dir = home.join(".config/lok/workflows");
        if tokio::fs::metadata(&global_dir).await.is_ok() {
            workflows.extend(load_workflows_from_dir(&global_dir).await?);
        }
    }

    Ok(workflows)
}

/// Tracks consecutive errors during directory iteration with backoff logic.
///
/// Extracted to enable unit testing of error handling behavior.
#[derive(Debug)]
struct LoadErrorTracker {
    consecutive_errors: u32,
    max_errors: u32,
}

impl LoadErrorTracker {
    fn new(max_errors: u32) -> Self {
        Self {
            consecutive_errors: 0,
            max_errors,
        }
    }

    fn on_success(&mut self) {
        self.consecutive_errors = 0;
    }

    /// Returns Ok(backoff_ms) to continue, Err(()) if should bail.
    fn on_error(&mut self) -> Result<u64, ()> {
        self.consecutive_errors += 1;
        if self.consecutive_errors >= self.max_errors {
            Err(())
        } else {
            Ok(10 * self.consecutive_errors as u64)
        }
    }

    fn error_count(&self) -> u32 {
        self.consecutive_errors
    }
}

async fn load_workflows_from_dir(dir: &Path) -> Result<Vec<(PathBuf, Workflow)>> {
    let mut workflows = Vec::new();
    let mut tracker = LoadErrorTracker::new(10);

    let mut entries = tokio::fs::read_dir(dir).await?;
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => {
                tracker.on_success();
                let path = entry.path();
                if path.extension().map(|e| e == "toml").unwrap_or(false) {
                    match load_workflow(&path).await {
                        Ok(workflow) => workflows.push((path, workflow)),
                        Err(e) => {
                            eprintln!(
                                "{} Failed to load {}: {}",
                                "warning:".yellow(),
                                path.display(),
                                e
                            );
                        }
                    }
                }
            }
            Ok(None) => break, // End of directory
            Err(e) => match tracker.on_error() {
                Ok(backoff_ms) => {
                    eprintln!(
                        "{} Error reading directory entry ({}/{}): {}",
                        "warning:".yellow(),
                        tracker.error_count(),
                        10,
                        e
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
                Err(()) => {
                    anyhow::bail!(
                        "Too many consecutive errors ({}) reading directory {}: {}",
                        tracker.error_count(),
                        dir.display(),
                        e
                    );
                }
            },
        }
    }

    Ok(workflows)
}

/// Load a workflow from a TOML file, resolving any `extends` inheritance
pub async fn load_workflow(path: &Path) -> Result<Workflow> {
    load_workflow_with_depth(path, 0).await
}

/// Load workflow with recursion depth tracking to prevent infinite loops
async fn load_workflow_with_depth(path: &Path, depth: usize) -> Result<Workflow> {
    if depth > 10 {
        anyhow::bail!("Workflow inheritance depth exceeded (max 10) - possible circular extends");
    }

    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read workflow file: {}", path.display()))?;

    let mut workflow: Workflow = toml::from_str(&content)
        .with_context(|| format!("Failed to parse workflow: {}", path.display()))?;

    // Handle extends inheritance
    if let Some(ref parent_name) = workflow.extends {
        let parent_path = find_workflow(parent_name).await.with_context(|| {
            format!(
                "Failed to find parent workflow '{}' for extends",
                parent_name
            )
        })?;

        let parent = Box::pin(load_workflow_with_depth(&parent_path, depth + 1)).await?;
        workflow = merge_workflows(parent, workflow);
    }

    Ok(workflow)
}

/// Merge parent workflow with child workflow
/// - Child steps override parent steps with same name
/// - Child steps are appended after parent steps (unless overriding)
/// - Child name/description take precedence if set
fn merge_workflows(parent: Workflow, child: Workflow) -> Workflow {
    let mut merged_steps = parent.steps.clone();

    // Build index map once for O(1) lookups of parent steps
    let name_to_index: HashMap<String, usize> = merged_steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), i))
        .collect();

    for child_step in child.steps {
        if let Some(&pos) = name_to_index.get(&child_step.name) {
            // Override existing parent step at same position
            merged_steps[pos] = child_step;
        } else {
            // Append new step (no need to update map - we won't look it up)
            merged_steps.push(child_step);
        }
    }

    Workflow {
        name: child.name,
        description: child.description.or(parent.description),
        extends: None, // Clear extends after merging
        steps: merged_steps,
    }
}

/// Print workflow results
pub fn print_results(results: &[StepResult]) {
    print!("{}", format_results(results));
}

/// Format workflow results as a string (for file output)
pub fn format_results(results: &[StepResult]) -> String {
    let mut output = String::new();
    output.push_str("\nResults:\n\n");

    for result in results {
        let status = if result.success { "[OK]" } else { "[FAIL]" };

        output.push_str(&format!(
            "{} {} ({:.1}s)\n\n",
            status,
            result.name,
            result.elapsed_ms as f64 / 1000.0
        ));

        // Indent output
        for line in result.output.lines() {
            output.push_str(&format!("  {}\n", line));
        }
        output.push('\n');
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_extract_json_from_markdown_block() {
        let text = r#"```json
{
  "verdict": "APPROVE",
  "summary": "Looks good"
}
```"#;
        let result = extract_json_from_text(text);
        assert!(result.is_some());
        let json = result.unwrap();
        assert!(json.contains("\"verdict\": \"APPROVE\""));
    }

    #[test]
    fn test_extract_json_from_plain_block() {
        let text = r#"```
{
  "verdict": "APPROVE"
}
```"#;
        let result = extract_json_from_text(text);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_json_raw() {
        let text = r#"{"verdict": "APPROVE", "summary": "test"}"#;
        let result = extract_json_from_text(text);
        assert!(result.is_some());
        assert!(result.unwrap().contains("APPROVE"));
    }

    #[test]
    fn test_extract_json_with_text_before() {
        let text = r#"Here is the JSON:
```json
{"verdict": "APPROVE"}
```"#;
        let result = extract_json_from_text(text);
        assert!(result.is_some());
    }

    #[test]
    fn test_extract_json_field_string() {
        let text = r#"```json
{"verdict": "APPROVE", "summary": "Looks good"}
```"#;
        let result = extract_json_field(text, "verdict");
        assert_eq!(result, Some("APPROVE".to_string()));
    }

    #[test]
    fn test_extract_json_field_multiline() {
        let text = r#"```json
{
  "verdict": "REQUEST_CHANGES",
  "critical": "None",
  "important": "- First issue\n- Second issue",
  "summary": "Needs work"
}
```"#;
        assert_eq!(
            extract_json_field(text, "verdict"),
            Some("REQUEST_CHANGES".to_string())
        );
        assert_eq!(
            extract_json_field(text, "critical"),
            Some("None".to_string())
        );
        assert_eq!(
            extract_json_field(text, "important"),
            Some("- First issue\n- Second issue".to_string())
        );
    }

    #[test]
    fn test_extract_json_field_not_found() {
        let text = r#"{"verdict": "APPROVE"}"#;
        let result = extract_json_field(text, "missing");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_json_field_number() {
        let text = r#"{"count": 42}"#;
        let result = extract_json_field(text, "count");
        assert_eq!(result, Some("42".to_string()));
    }

    #[test]
    fn test_extract_json_field_bool() {
        let text = r#"{"approved": true}"#;
        let result = extract_json_field(text, "approved");
        assert_eq!(result, Some("true".to_string()));
    }

    #[test]
    fn test_interpolate_with_fields_json() {
        // Simulate the exact scenario from review-pr workflow
        let synthesize_output = r#"```json
{
  "verdict": "REQUEST_CHANGES",
  "critical": "None",
  "important": "- Issue one\n- Issue two",
  "minor": "- Minor thing",
  "summary": "Needs work before merge."
}
```"#;

        let mut results = HashMap::new();
        results.insert(
            "synthesize".to_string(),
            StepResult {
                name: "synthesize".to_string(),
                output: synthesize_output.to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 1000,
                backend: Some("claude".to_string()),
            },
        );

        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);

        let template =
            "Verdict: {{ steps.synthesize.verdict }}\nSummary: {{ steps.synthesize.summary }}";
        let result = runner
            .interpolate_with_fields(template, &results, "test-workflow", "test-step")
            .unwrap();

        assert!(
            result.contains("REQUEST_CHANGES"),
            "Expected verdict in output, got: {}",
            result
        );
        assert!(
            result.contains("Needs work"),
            "Expected summary in output, got: {}",
            result
        );
    }

    #[test]
    fn test_extract_json_with_literal_newlines() {
        // LLMs sometimes output literal newlines in JSON strings instead of \n escapes
        // This is invalid JSON but we should handle it gracefully
        let text = "```json
{
  \"verdict\": \"APPROVE\",
  \"important\": \"- First issue
- Second issue
- Third issue\"
}
```";
        let result = extract_json_field(text, "verdict");
        assert_eq!(result, Some("APPROVE".to_string()));

        let important = extract_json_field(text, "important");
        assert!(important.is_some());
        assert!(important.unwrap().contains("First issue"));
    }

    #[test]
    fn test_sanitize_json_strings() {
        // Test that literal newlines inside strings are escaped
        let input = r#"{"msg": "line1
line2"}"#;
        let sanitized = sanitize_json_strings(input);
        assert!(sanitized.contains("\\n"));
        assert!(!sanitized.contains('\n') || sanitized.matches('\n').count() == 0);

        // Verify it parses after sanitization
        let result: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
        assert_eq!(result["msg"], "line1\nline2");
    }

    #[test]
    fn test_duplicate_step_names_error() {
        let steps = vec![
            Step {
                name: "fetch".to_string(),
                backend: String::new(),
                prompt: String::new(),
                depends_on: vec![],
                when: None,
                shell: Some("echo test".to_string()),
                apply_edits: false,
                verify: None,
                retries: 0,
                retry_delay: 1000,
                for_each: None,
                output_format: None,
            },
            Step {
                name: "fetch".to_string(), // duplicate!
                backend: String::new(),
                prompt: String::new(),
                depends_on: vec![],
                when: None,
                shell: Some("echo test2".to_string()),
                apply_edits: false,
                verify: None,
                retries: 0,
                retry_delay: 1000,
                for_each: None,
                output_format: None,
            },
        ];

        let config = crate::config::Config::default();
        let runner = WorkflowRunner::new(config, std::path::PathBuf::from("/tmp"), vec![]);
        let result = runner.group_by_depth(&steps, "test-workflow");

        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("duplicate step names"),
            "Expected duplicate step names error, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("fetch"),
            "Expected 'fetch' in error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_group_by_depth_forward_declared_dependency() {
        // Issue #130: Test that steps depending on forward-declared steps are handled correctly.
        // "early_step" is defined first but depends on "late_step" which is defined second.
        // The depth calculation should still work correctly regardless of definition order.
        let steps = vec![
            Step {
                name: "early_step".to_string(),
                backend: String::new(),
                prompt: String::new(),
                depends_on: vec!["late_step".to_string()], // depends on step defined later
                when: None,
                shell: Some("echo early".to_string()),
                apply_edits: false,
                verify: None,
                retries: 0,
                retry_delay: 1000,
                for_each: None,
                output_format: None,
            },
            Step {
                name: "late_step".to_string(),
                backend: String::new(),
                prompt: String::new(),
                depends_on: vec![], // no dependencies
                when: None,
                shell: Some("echo late".to_string()),
                apply_edits: false,
                verify: None,
                retries: 0,
                retry_delay: 1000,
                for_each: None,
                output_format: None,
            },
        ];

        let config = crate::config::Config::default();
        let runner = WorkflowRunner::new(config, std::path::PathBuf::from("/tmp"), vec![]);
        let levels = runner.group_by_depth(&steps, "test-workflow").unwrap();

        // late_step has no dependencies, so it should be at depth 0
        // early_step depends on late_step, so it should be at depth 1
        assert_eq!(
            levels.len(),
            2,
            "Expected 2 depth levels, got: {:?}",
            levels
        );
        assert!(
            levels[0].contains(&"late_step".to_string()),
            "late_step should be at depth 0, got levels: {:?}",
            levels
        );
        assert!(
            levels[1].contains(&"early_step".to_string()),
            "early_step should be at depth 1, got levels: {:?}",
            levels
        );
    }

    fn make_test_results() -> HashMap<String, StepResult> {
        let mut results = HashMap::new();
        results.insert(
            "analyze".to_string(),
            StepResult {
                name: "analyze".to_string(),
                output: "Found ISSUES_FOUND in the code. Multiple problems detected.".to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );
        results.insert(
            "check".to_string(),
            StepResult {
                name: "check".to_string(),
                output: "PASS".to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 50,
                backend: Some("claude".to_string()),
            },
        );
        results
    }

    #[test]
    fn test_condition_contains() {
        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);
        let results = make_test_results();

        // New syntax: contains(step.output, "string")
        assert!(runner.evaluate_condition(r#"contains(analyze.output, "ISSUES_FOUND")"#, &results));
        assert!(!runner.evaluate_condition(r#"contains(analyze.output, "NO_ISSUES")"#, &results));

        // Step doesn't exist
        assert!(!runner.evaluate_condition(r#"contains(missing.output, "test")"#, &results));
    }

    #[test]
    fn test_condition_equals() {
        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);
        let results = make_test_results();

        // Exact match (trims whitespace)
        assert!(runner.evaluate_condition(r#"equals(check.output, "PASS")"#, &results));
        assert!(!runner.evaluate_condition(r#"equals(check.output, "FAIL")"#, &results));

        // Partial match should fail equals
        assert!(!runner.evaluate_condition(r#"equals(analyze.output, "ISSUES_FOUND")"#, &results));
    }

    #[test]
    fn test_condition_not() {
        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);
        let results = make_test_results();

        // Negation
        assert!(!runner
            .evaluate_condition(r#"not(contains(analyze.output, "ISSUES_FOUND"))"#, &results));
        assert!(
            runner.evaluate_condition(r#"not(contains(analyze.output, "NO_ISSUES"))"#, &results)
        );
        assert!(runner.evaluate_condition(r#"not(equals(check.output, "FAIL"))"#, &results));
    }

    #[test]
    fn test_condition_legacy_syntax() {
        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);
        let results = make_test_results();

        // Legacy syntax still works
        assert!(
            runner.evaluate_condition(r#"steps.analyze.output contains 'ISSUES_FOUND'"#, &results)
        );
        assert!(
            !runner.evaluate_condition(r#"steps.analyze.output contains 'NO_ISSUES'"#, &results)
        );
    }

    #[test]
    fn test_condition_unparseable_returns_true() {
        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);
        let results = make_test_results();

        // Unparseable conditions default to true (step runs)
        assert!(runner.evaluate_condition("some random text", &results));
        assert!(runner.evaluate_condition("", &results));
    }

    #[test]
    fn test_condition_json_field_access() {
        let config = Config::default();
        let runner = WorkflowRunner::new(config, PathBuf::from("."), vec![]);
        let mut results = HashMap::new();
        results.insert(
            "fix".to_string(),
            StepResult {
                name: "fix".to_string(),
                output: r#"{"action": "close", "reason": "Already fixed"}"#.to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );
        results.insert(
            "fix2".to_string(),
            StepResult {
                name: "fix2".to_string(),
                output: r#"{"action": "fix", "summary": "Fixed the bug"}"#.to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );

        // JSON field access: equals(step.field, "value")
        assert!(runner.evaluate_condition(r#"equals(fix.action, "close")"#, &results));
        assert!(!runner.evaluate_condition(r#"equals(fix.action, "fix")"#, &results));
        assert!(runner.evaluate_condition(r#"equals(fix2.action, "fix")"#, &results));
        assert!(!runner.evaluate_condition(r#"equals(fix2.action, "close")"#, &results));

        // JSON field access: contains(step.field, "substring")
        assert!(runner.evaluate_condition(r#"contains(fix.reason, "Already")"#, &results));
        assert!(!runner.evaluate_condition(r#"contains(fix.reason, "NotHere")"#, &results));

        // .output still works as before
        assert!(runner.evaluate_condition(r#"contains(fix.output, "action")"#, &results));

        // Missing field returns false
        assert!(!runner.evaluate_condition(r#"equals(fix.missing_field, "value")"#, &results));
    }

    #[test]
    fn test_step_if_alias() {
        // Test that `if` works as alias for `when` in TOML
        let toml_str = r#"
            name = "test"
            backend = "claude"
            prompt = "test prompt"
            if = "contains(analyze.output, \"ISSUES_FOUND\")"
        "#;
        let step: Step = toml::from_str(toml_str).unwrap();
        assert_eq!(
            step.when,
            Some(r#"contains(analyze.output, "ISSUES_FOUND")"#.to_string())
        );
    }

    #[test]
    fn test_interpolate_loop_vars_item_string() {
        let item = serde_json::json!("hello");
        let result = interpolate_loop_vars("Value: {{ item }}", &item, 0);
        assert_eq!(result, "Value: hello");
    }

    #[test]
    fn test_interpolate_loop_vars_item_object() {
        let item = serde_json::json!({"name": "tests", "pattern": "*.spec.rb"});
        let result = interpolate_loop_vars(
            "Name: {{ item.name }}, Pattern: {{ item.pattern }}",
            &item,
            0,
        );
        assert_eq!(result, "Name: tests, Pattern: *.spec.rb");
    }

    #[test]
    fn test_interpolate_loop_vars_item_whole_object() {
        let item = serde_json::json!({"name": "tests"});
        let result = interpolate_loop_vars("Item: {{ item }}", &item, 0);
        assert_eq!(result, r#"Item: {"name":"tests"}"#);
    }

    #[test]
    fn test_interpolate_loop_vars_index() {
        let item = serde_json::json!("value");
        let result = interpolate_loop_vars("Index: {{ index }}", &item, 5);
        assert_eq!(result, "Index: 5");
    }

    #[test]
    fn test_interpolate_loop_vars_combined() {
        let item = serde_json::json!({"file": "test.rb"});
        let result = interpolate_loop_vars(
            "Processing {{ item.file }} ({{ index }}/10): {{ item }}",
            &item,
            3,
        );
        assert!(result.contains("Processing test.rb"));
        assert!(result.contains("(3/10)"));
    }

    #[test]
    fn test_interpolate_loop_vars_missing_field() {
        let item = serde_json::json!({"name": "tests"});
        let result = interpolate_loop_vars("Missing: {{ item.missing }}", &item, 0);
        assert_eq!(result, "Missing: [item.missing not found]");
    }

    #[test]
    fn test_parse_for_each_inline_array() {
        let results = HashMap::new();
        let items = parse_for_each_array(r#"["a", "b", "c"]"#, &results).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], serde_json::json!("a"));
        assert_eq!(items[1], serde_json::json!("b"));
        assert_eq!(items[2], serde_json::json!("c"));
    }

    #[test]
    fn test_parse_for_each_inline_array_objects() {
        let results = HashMap::new();
        let items =
            parse_for_each_array(r#"[{"name": "tests"}, {"name": "frontend"}]"#, &results).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "tests");
        assert_eq!(items[1]["name"], "frontend");
    }

    #[test]
    fn test_parse_for_each_step_reference() {
        let mut results = HashMap::new();
        results.insert(
            "plan".to_string(),
            StepResult {
                name: "plan".to_string(),
                output: r#"["chunk1", "chunk2", "chunk3"]"#.to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );

        let items = parse_for_each_array("steps.plan.output", &results).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], serde_json::json!("chunk1"));
    }

    #[test]
    fn test_parse_for_each_step_reference_with_code_block() {
        let mut results = HashMap::new();
        results.insert(
            "plan".to_string(),
            StepResult {
                name: "plan".to_string(),
                output: r#"```json
[{"name": "tests", "pattern": "*.spec.rb"}, {"name": "frontend", "pattern": "*.js"}]
```"#
                    .to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );

        let items = parse_for_each_array("steps.plan.output", &results).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "tests");
        assert_eq!(items[1]["pattern"], "*.js");
    }

    #[test]
    fn test_parse_for_each_invalid_format() {
        let results = HashMap::new();
        let err = parse_for_each_array("invalid", &results).unwrap_err();
        assert!(err.to_string().contains("invalid format"));
    }

    #[test]
    fn test_parse_for_each_step_not_found() {
        let results = HashMap::new();
        let err = parse_for_each_array("steps.missing.output", &results).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_parse_for_each_not_array() {
        let mut results = HashMap::new();
        results.insert(
            "plan".to_string(),
            StepResult {
                name: "plan".to_string(),
                output: r#"{"not": "an array"}"#.to_string(),
                parsed_output: None,
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );

        let err = parse_for_each_array("steps.plan.output", &results).unwrap_err();
        assert!(err.to_string().contains("not a JSON array"));
    }

    #[test]
    fn test_step_for_each_toml_parsing() {
        let toml_str = r#"
            name = "review_chunk"
            backend = "claude"
            prompt = "Review {{ item.name }}"
            for_each = "steps.plan.output"
        "#;
        let step: Step = toml::from_str(toml_str).unwrap();
        assert_eq!(step.for_each, Some("steps.plan.output".to_string()));
    }

    #[test]
    fn test_step_for_each_inline_array_toml() {
        let toml_str = r#"
            name = "process"
            shell = "echo {{ item }}"
            for_each = '["a", "b", "c"]'
        "#;
        let step: Step = toml::from_str(toml_str).unwrap();
        assert_eq!(step.for_each, Some(r#"["a", "b", "c"]"#.to_string()));
    }

    #[test]
    fn test_output_format_toml_parsing() {
        let toml_str = r#"
            name = "get_issues"
            shell = "gh issue list --json number,title"
            output_format = "json"
        "#;
        let step: Step = toml::from_str(toml_str).unwrap();
        assert_eq!(step.output_format, Some("json".to_string()));
    }

    #[test]
    fn test_parse_step_output_json() {
        let output = r#"[{"name": "test"}, {"name": "test2"}]"#;
        let parsed = parse_step_output(output, Some("json"));
        assert!(parsed.is_some());
        let arr = parsed.unwrap();
        assert!(arr.is_array());
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_parse_step_output_lines() {
        let output = "line1\nline2\nline3";
        let parsed = parse_step_output(output, Some("lines"));
        assert!(parsed.is_some());
        let arr = parsed.unwrap();
        assert!(arr.is_array());
        let lines = arr.as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line1");
    }

    #[test]
    fn test_parse_step_output_text() {
        let output = "just some text";
        let parsed = parse_step_output(output, Some("text"));
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_step_output_none() {
        let output = "just some text";
        let parsed = parse_step_output(output, None);
        assert!(parsed.is_none());
    }

    #[test]
    fn test_for_each_with_parsed_output() {
        let mut results = HashMap::new();
        let parsed_array = serde_json::json!([
            {"name": "chunk1", "files": 5},
            {"name": "chunk2", "files": 3}
        ]);
        results.insert(
            "plan".to_string(),
            StepResult {
                name: "plan".to_string(),
                output: "some raw output".to_string(),
                parsed_output: Some(parsed_array),
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );

        // Should use parsed_output directly, not parse the raw output
        let items = parse_for_each_array("steps.plan.output", &results).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "chunk1");
        assert_eq!(items[1]["files"], 3);
    }

    #[test]
    fn test_for_each_parsed_output_not_array() {
        let mut results = HashMap::new();
        let parsed_object = serde_json::json!({"not": "an array"});
        results.insert(
            "plan".to_string(),
            StepResult {
                name: "plan".to_string(),
                output: "some raw output".to_string(),
                parsed_output: Some(parsed_object),
                success: true,
                elapsed_ms: 100,
                backend: Some("claude".to_string()),
            },
        );

        let err = parse_for_each_array("steps.plan.output", &results).unwrap_err();
        assert!(err.to_string().contains("not an array"));
    }

    #[test]
    fn test_parse_edits_with_literal_newlines() {
        // LLMs sometimes output literal newlines in JSON strings instead of \n escapes
        // This is invalid JSON but parse_edits should handle it via sanitization
        let text = r#"Here's the fix:

```json
{
  "edits": [
    {
      "file": "src/main.rs",
      "old": "fn main() {
    println!(\"hello\");
}",
      "new": "fn main() {
    println!(\"goodbye\");
}"
    }
  ],
  "summary": "Changed greeting"
}
```"#;

        let result = parse_edits(text);
        assert!(result.is_ok(), "parse_edits should handle literal newlines");

        let output = result.unwrap();
        assert_eq!(output.edits.len(), 1);
        assert_eq!(output.edits[0].file, "src/main.rs");
        assert!(output.edits[0].old.contains("hello"));
        assert!(output.edits[0].new.contains("goodbye"));
    }

    #[test]
    fn test_parse_edits_with_backticks_in_content() {
        // JSON content might contain ``` which should not be mistaken for the closing fence.
        // The closing fence must be on its own line.
        let text = r#"Here's the fix:

```json
{
  "edits": [
    {
      "file": "src/main.rs",
      "old": "context.push_str(\"```\\n\");",
      "new": "context.push_str(\"~~~\\n\");"
    }
  ],
  "summary": "Changed backticks to tildes"
}
```"#;

        let result = parse_edits(text);
        assert!(
            result.is_ok(),
            "parse_edits should handle backticks in content: {:?}",
            result.err()
        );

        let output = result.unwrap();
        assert_eq!(output.edits.len(), 1);
        assert!(output.edits[0].old.contains("```"));
        assert!(output.edits[0].new.contains("~~~"));
    }

    #[test]
    fn test_find_closing_fence() {
        // Normal case: fence on its own line
        assert_eq!(find_closing_fence("\n{\"a\": 1}\n```"), Some(9));

        // Backticks inside content should be ignored
        assert_eq!(find_closing_fence("\n{\"a\": \"```\"}\n```"), Some(13));

        // Fence at start (empty content)
        assert_eq!(find_closing_fence("```"), Some(0));

        // No fence
        assert_eq!(find_closing_fence("{\"a\": 1}"), None);

        // Backticks not at line start
        assert_eq!(find_closing_fence("\n{\"code\": \"x```y\"}\n```"), Some(18));
    }

    // LoadErrorTracker tests (Issue #125)

    #[test]
    fn test_load_error_tracker_backoff_progression() {
        let mut tracker = LoadErrorTracker::new(10);

        // First error: backoff 10ms
        assert_eq!(tracker.on_error(), Ok(10));
        assert_eq!(tracker.error_count(), 1);

        // Second error: backoff 20ms
        assert_eq!(tracker.on_error(), Ok(20));
        assert_eq!(tracker.error_count(), 2);

        // Third error: backoff 30ms
        assert_eq!(tracker.on_error(), Ok(30));
        assert_eq!(tracker.error_count(), 3);
    }

    #[test]
    fn test_load_error_tracker_bail_at_threshold() {
        let mut tracker = LoadErrorTracker::new(10);

        // 9 errors should succeed with increasing backoff
        for i in 1..10 {
            assert_eq!(tracker.on_error(), Ok(10 * i));
        }

        // 10th error should bail
        assert_eq!(tracker.on_error(), Err(()));
        assert_eq!(tracker.error_count(), 10);
    }

    #[test]
    fn test_load_error_tracker_reset_on_success() {
        let mut tracker = LoadErrorTracker::new(10);

        // Accumulate 5 errors
        for _ in 0..5 {
            let _ = tracker.on_error();
        }
        assert_eq!(tracker.error_count(), 5);

        // Success resets counter
        tracker.on_success();
        assert_eq!(tracker.error_count(), 0);

        // Next error starts fresh at 10ms, not 60ms
        assert_eq!(tracker.on_error(), Ok(10));
        assert_eq!(tracker.error_count(), 1);
    }

    #[test]
    fn test_load_error_tracker_success_with_no_prior_errors() {
        let mut tracker = LoadErrorTracker::new(10);

        // Calling on_success with no prior errors should not panic
        tracker.on_success();
        assert_eq!(tracker.error_count(), 0);

        // Multiple successes are fine
        tracker.on_success();
        tracker.on_success();
        assert_eq!(tracker.error_count(), 0);
    }

    // apply_edits tests (Issue #135)

    #[tokio::test]
    async fn test_apply_edits_single_occurrence() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let edits = vec![FileEdit {
            file: "test.txt".to_string(),
            old: "world".to_string(),
            new: "universe".to_string(),
        }];

        let result = apply_edits(&edits, dir.path()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello universe");
    }

    #[tokio::test]
    async fn test_apply_edits_multiple_occurrences_fails() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "foo bar foo baz foo").unwrap();

        let edits = vec![FileEdit {
            file: "test.txt".to_string(),
            old: "foo".to_string(),
            new: "qux".to_string(),
        }];

        let result = apply_edits(&edits, dir.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Ambiguous edit"));
        assert!(err.contains("3 times"));

        // File should be unchanged
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "foo bar foo baz foo");
    }

    #[tokio::test]
    async fn test_apply_edits_not_found_fails() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let edits = vec![FileEdit {
            file: "test.txt".to_string(),
            old: "not_present".to_string(),
            new: "replacement".to_string(),
        }];

        let result = apply_edits(&edits, dir.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Old text not found"));
    }

    #[tokio::test]
    async fn test_apply_edits_file_not_found_fails() {
        let dir = tempdir().unwrap();

        let edits = vec![FileEdit {
            file: "nonexistent.txt".to_string(),
            old: "foo".to_string(),
            new: "bar".to_string(),
        }];

        let result = apply_edits(&edits, dir.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("File not found"));
    }
}
