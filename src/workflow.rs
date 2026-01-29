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

/// Regex for matching "steps.X.output contains 'Y'" conditions
static CONDITION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"steps\.([a-zA-Z0-9_-]+)\.output\s+contains\s+['"](.+)['"]"#).unwrap()
});

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
    #[serde(default)]
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
}

/// Result of executing a step
#[derive(Debug, Clone)]
pub struct StepResult {
    pub name: String,
    pub output: String,
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
                steps_to_run.push(PreparedStep {
                    step,
                    prompt,
                    shell,
                    format,
                    verify,
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
                    } = prepared;
                    let config = self.config.clone();
                    let cwd = self.cwd.clone();
                    let step_name = step.name.clone();
                    let backend_name = step.backend.clone();
                    let apply_edits_flag = step.apply_edits;

                    async move {
                        println!("{} {}", "[step]".cyan(), step_name.bold());
                        let start = std::time::Instant::now();

                        // Shell step - run command directly
                        if let Some(ref shell_cmd) = shell {
                            println!("  {} {}", "shell:".dimmed(), shell_cmd.dimmed());
                            match run_shell(shell_cmd, &cwd) {
                                Ok(output) => {
                                    let elapsed_ms = start.elapsed().as_millis() as u64;
                                    println!(
                                        "  {} ({:.1}s)",
                                        "✓".green(),
                                        elapsed_ms as f64 / 1000.0
                                    );
                                    return StepResult {
                                        name: step_name,
                                        output,
                                        success: true,
                                        elapsed_ms,
                                        backend: None,
                                    };
                                }
                                Err(e) => {
                                    let elapsed_ms = start.elapsed().as_millis() as u64;
                                    println!("  {} {}", "✗".red(), e);
                                    return StepResult {
                                        name: step_name,
                                        output: format!("Error: {}", e),
                                        success: false,
                                        elapsed_ms,
                                        backend: None,
                                    };
                                }
                            }
                        }

                        // LLM step - query backend
                        let backend_config = match config.backends.get(&backend_name) {
                            Some(cfg) => cfg,
                            None => {
                                return StepResult {
                                    name: step_name,
                                    output: format!("Backend not found: {}", backend_name),
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
                                success: false,
                                elapsed_ms: 0,
                                backend: Some(backend_name),
                            };
                        }

                        // Execute LLM query
                        let output = backend.query(&prompt, &cwd).await;
                        let elapsed_ms = start.elapsed().as_millis() as u64;

                        match output {
                            Ok(text) => {
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
                                                match apply_edits(&agentic.edits, &cwd) {
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
                                                success: false,
                                                elapsed_ms,
                                                backend: Some(backend_name.clone()),
                                            };
                                        }
                                    }
                                }

                                StepResult {
                                    name: step_name,
                                    output: text,
                                    success: true,
                                    elapsed_ms,
                                    backend: Some(backend_name),
                                }
                            }
                            Err(e) => {
                                println!("  {} {}", "✗".red(), e);
                                StepResult {
                                    name: step_name,
                                    output: format!("Error: {}", e),
                                    success: false,
                                    elapsed_ms,
                                    backend: Some(backend_name),
                                }
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
    fn interpolate(
        &self,
        template: &str,
        results: &HashMap<String, StepResult>,
        workflow_name: &str,
        current_step: &str,
    ) -> Result<String, WorkflowError> {
        let mut output = template.to_string();

        for cap in INTERPOLATE_RE.captures_iter(template) {
            let full_match = cap.get(0).expect("regex group 0 always exists").as_str();
            let referenced_step = cap.get(1).expect("regex group 1 always exists").as_str();

            let replacement = results
                .get(referenced_step)
                .map(|r| r.output.as_str())
                .ok_or_else(|| WorkflowError::MissingStepOutput {
                    workflow: workflow_name.to_string(),
                    step: current_step.to_string(),
                    referenced: referenced_step.to_string(),
                })?;

            output = output.replace(full_match, replacement);
        }

        Ok(output)
    }

    /// Evaluate a simple condition like "steps.scan.output contains 'critical'"
    fn evaluate_condition(&self, condition: &str, results: &HashMap<String, StepResult>) -> bool {
        // Simple parser for "steps.X.output contains 'Y'"
        if let Some(caps) = CONDITION_RE.captures(condition) {
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
    fn interpolate_with_fields(
        &self,
        template: &str,
        results: &HashMap<String, StepResult>,
        workflow_name: &str,
        current_step: &str,
    ) -> Result<String, WorkflowError> {
        let mut output = self.interpolate(template, results, workflow_name, current_step)?;

        // Handle {{ steps.X.field }} for JSON field access
        for cap in FIELD_RE.captures_iter(template) {
            let full_match = cap.get(0).expect("regex group 0 always exists").as_str();
            let referenced_step = cap.get(1).expect("regex group 1 always exists").as_str();
            let field_name = cap.get(2).expect("regex group 2 always exists").as_str();

            // Skip "output" - that's handled by regular interpolate
            if field_name == "output" {
                continue;
            }

            // Check if step exists first
            let step_result =
                results
                    .get(referenced_step)
                    .ok_or_else(|| WorkflowError::MissingStepOutput {
                        workflow: workflow_name.to_string(),
                        step: current_step.to_string(),
                        referenced: referenced_step.to_string(),
                    })?;

            let replacement = extract_json_field(&step_result.output, field_name)
                .unwrap_or_else(|| format!("[field {} not found]", field_name));

            output = output.replace(full_match, &replacement);
        }

        // Handle {{ env.VAR }} for environment variables
        for cap in ENV_RE.captures_iter(template) {
            let full_match = cap.get(0).expect("regex group 0 always exists").as_str();
            let var_name = cap.get(1).expect("regex group 1 always exists").as_str();

            let replacement =
                std::env::var(var_name).unwrap_or_else(|_| format!("[env {} not set]", var_name));

            output = output.replace(full_match, &replacement);
        }

        // Handle {{ arg.N }} for positional arguments (1-indexed)
        for cap in ARG_RE.captures_iter(template) {
            let full_match = cap.get(0).expect("regex group 0 always exists").as_str();
            let arg_index: usize = cap
                .get(1)
                .expect("regex group 1 always exists")
                .as_str()
                .parse()
                .unwrap_or(0);

            let replacement = if arg_index > 0 && arg_index <= self.args.len() {
                self.args[arg_index - 1].clone()
            } else {
                format!("[arg {} not provided]", arg_index)
            };

            output = output.replace(full_match, &replacement);
        }

        // Handle {{ workflow.backends }} - list unique backends used
        if WORKFLOW_BACKENDS_RE.is_match(&output) {
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

            output = WORKFLOW_BACKENDS_RE
                .replace_all(&output, &replacement)
                .to_string();
        }

        // Check for any remaining unknown {{ ... }} variables
        if let Some(cap) = UNKNOWN_VAR_RE.captures(&output) {
            let variable = cap
                .get(1)
                .expect("regex group 1 always exists")
                .as_str()
                .trim();
            return Err(WorkflowError::UnknownVariable {
                workflow: workflow_name.to_string(),
                step: current_step.to_string(),
                variable: variable.to_string(),
            });
        }

        Ok(output)
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

/// Extract JSON object from text, handling markdown code blocks
fn extract_json_from_text(text: &str) -> Option<String> {
    // Try to find ```json ... ``` block first
    if let Some(start) = text.find("```json") {
        let after_marker = &text[start + 7..];
        if let Some(end) = after_marker.find("```") {
            return Some(after_marker[..end].trim().to_string());
        }
    }

    // Try to find ``` ... ``` block
    if let Some(start) = text.find("```") {
        let after_marker = &text[start + 3..];
        if let Some(end) = after_marker.find("```") {
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
    serde_json::from_str(&json_str).context("Failed to parse edits JSON")
}

/// Apply file edits
fn apply_edits(edits: &[FileEdit], cwd: &Path) -> Result<usize> {
    let mut applied = 0;

    for edit in edits {
        let file_path = cwd.join(&edit.file);

        if !file_path.exists() {
            anyhow::bail!("File not found: {}", edit.file);
        }

        let content =
            std::fs::read_to_string(&file_path).context(format!("Failed to read {}", edit.file))?;

        if !content.contains(&edit.old) {
            anyhow::bail!(
                "Old text not found in {}: {}",
                edit.file,
                edit.old.chars().take(50).collect::<String>()
            );
        }

        let new_content = content.replace(&edit.old, &edit.new);
        std::fs::write(&file_path, new_content)
            .context(format!("Failed to write {}", edit.file))?;

        println!("    {} {}", "edited".green(), edit.file);
        applied += 1;
    }

    Ok(applied)
}

/// Find workflow file by name, checking project-local and global paths
pub fn find_workflow(name: &str) -> Result<PathBuf> {
    // If it's already a path, use it directly
    let path = Path::new(name);
    if path.exists() {
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
    if local_path.exists() {
        return Ok(local_path);
    }

    // Check global ~/.config/lok/workflows/
    if let Some(home) = dirs::home_dir() {
        let global_path = home.join(".config/lok/workflows").join(&filename);
        if global_path.exists() {
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
pub fn list_workflows() -> Result<Vec<(PathBuf, Workflow)>> {
    let mut workflows = Vec::new();

    // Check project-local
    let local_dir = PathBuf::from(".lok/workflows");
    if local_dir.exists() {
        workflows.extend(load_workflows_from_dir(&local_dir)?);
    }

    // Check global
    if let Some(home) = dirs::home_dir() {
        let global_dir = home.join(".config/lok/workflows");
        if global_dir.exists() {
            workflows.extend(load_workflows_from_dir(&global_dir)?);
        }
    }

    Ok(workflows)
}

fn load_workflows_from_dir(dir: &Path) -> Result<Vec<(PathBuf, Workflow)>> {
    let mut workflows = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().map(|e| e == "toml").unwrap_or(false) {
            match load_workflow(&path) {
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

    Ok(workflows)
}

/// Load a workflow from a TOML file, resolving any `extends` inheritance
pub fn load_workflow(path: &Path) -> Result<Workflow> {
    load_workflow_with_depth(path, 0)
}

/// Load workflow with recursion depth tracking to prevent infinite loops
fn load_workflow_with_depth(path: &Path, depth: usize) -> Result<Workflow> {
    if depth > 10 {
        anyhow::bail!("Workflow inheritance depth exceeded (max 10) - possible circular extends");
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read workflow file: {}", path.display()))?;

    let mut workflow: Workflow = toml::from_str(&content)
        .with_context(|| format!("Failed to parse workflow: {}", path.display()))?;

    // Handle extends inheritance
    if let Some(ref parent_name) = workflow.extends {
        let parent_path = find_workflow(parent_name).with_context(|| {
            format!(
                "Failed to find parent workflow '{}' for extends",
                parent_name
            )
        })?;

        let parent = load_workflow_with_depth(&parent_path, depth + 1)?;
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
}
