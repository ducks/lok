//! Workflow engine - declarative multi-step LLM pipelines
//!
//! Workflows are TOML files that define a sequence of steps, each using
//! a backend to process a prompt. Steps can depend on previous steps
//! and interpolate their outputs.

use crate::backend;
use crate::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Regex for matching {{ steps.NAME.output }} patterns
static INTERPOLATE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{\s*steps\.([a-zA-Z0-9_-]+)\.output\s*\}\}").unwrap());

/// Regex for matching "steps.X.output contains 'Y'" conditions
static CONDITION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"steps\.([a-zA-Z0-9_-]+)\.output\s+contains\s+['"](.+)['"]"#).unwrap()
});

/// A workflow definition loaded from TOML
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// A single step in a workflow
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Step {
    pub name: String,
    pub backend: String,
    pub prompt: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional condition - step only runs if this evaluates true
    #[serde(default)]
    pub when: Option<String>,
}

/// Result of executing a step
#[derive(Debug, Clone)]
pub struct StepResult {
    pub name: String,
    pub output: String,
    pub success: bool,
    pub elapsed_ms: u64,
}

/// Workflow executor
pub struct WorkflowRunner {
    config: Config,
    cwd: PathBuf,
}

impl WorkflowRunner {
    pub fn new(config: Config, cwd: PathBuf) -> Self {
        Self { config, cwd }
    }

    /// Execute a workflow, returning results for each step
    /// Steps at the same depth level (no dependencies between them) run in parallel
    pub async fn run(&self, workflow: &Workflow) -> Result<Vec<StepResult>> {
        let mut results: HashMap<String, StepResult> = HashMap::new();
        let mut ordered_results: Vec<StepResult> = Vec::new();

        // Group steps by depth level for parallel execution
        let depth_levels = self.group_by_depth(&workflow.steps)?;

        println!("{} {}", "Running workflow:".bold(), workflow.name.cyan());
        if let Some(ref desc) = workflow.description {
            println!("{}", desc.dimmed());
        }
        println!("{}", "=".repeat(50).dimmed());
        println!();

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
            let mut steps_to_run: Vec<(&Step, String)> = Vec::new();

            for step_name in step_names {
                let step = workflow
                    .steps
                    .iter()
                    .find(|s| &s.name == step_name)
                    .unwrap();

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

                // Interpolate variables in prompt (uses results from previous depths)
                let prompt = self.interpolate(&step.prompt, &results);
                steps_to_run.push((step, prompt));
            }

            if steps_to_run.is_empty() {
                continue;
            }

            // Execute steps at this depth in parallel
            let futures: Vec<_> = steps_to_run
                .into_iter()
                .map(|(step, prompt)| {
                    let config = self.config.clone();
                    let cwd = self.cwd.clone();
                    let step_name = step.name.clone();
                    let backend_name = step.backend.clone();

                    async move {
                        println!("{} {}", "[step]".cyan(), step_name.bold());

                        // Get backend
                        let backend_config = match config.backends.get(&backend_name) {
                            Some(cfg) => cfg,
                            None => {
                                return StepResult {
                                    name: step_name,
                                    output: format!("Backend not found: {}", backend_name),
                                    success: false,
                                    elapsed_ms: 0,
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
                            };
                        }

                        // Execute
                        let start = std::time::Instant::now();
                        let output = backend.query(&prompt, &cwd).await;
                        let elapsed_ms = start.elapsed().as_millis() as u64;

                        match output {
                            Ok(text) => {
                                println!("  {} ({:.1}s)", "✓".green(), elapsed_ms as f64 / 1000.0);
                                StepResult {
                                    name: step_name,
                                    output: text,
                                    success: true,
                                    elapsed_ms,
                                }
                            }
                            Err(e) => {
                                println!("  {} {}", "✗".red(), e);
                                StepResult {
                                    name: step_name,
                                    output: format!("Error: {}", e),
                                    success: false,
                                    elapsed_ms,
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
    fn group_by_depth(&self, steps: &[Step]) -> Result<Vec<Vec<String>>> {
        // First validate dependencies exist
        let step_names: std::collections::HashSet<_> = steps.iter().map(|s| &s.name).collect();
        for step in steps {
            for dep in &step.depends_on {
                if !step_names.contains(dep) {
                    anyhow::bail!("Step '{}' depends on unknown step '{}'", step.name, dep);
                }
            }
        }

        // Calculate depth for each step
        let mut depths: HashMap<String, usize> = HashMap::new();

        fn calc_depth(
            name: &str,
            steps: &[Step],
            depths: &mut HashMap<String, usize>,
            visiting: &mut std::collections::HashSet<String>,
        ) -> Result<usize> {
            if let Some(&d) = depths.get(name) {
                return Ok(d);
            }

            if visiting.contains(name) {
                anyhow::bail!("Circular dependency detected at step: {}", name);
            }

            visiting.insert(name.to_string());

            let step = steps.iter().find(|s| s.name == name).unwrap();
            let depth = if step.depends_on.is_empty() {
                0
            } else {
                let max_dep_depth = step
                    .depends_on
                    .iter()
                    .map(|dep| calc_depth(dep, steps, depths, visiting))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .max()
                    .unwrap_or(0);
                max_dep_depth + 1
            };

            visiting.remove(name);
            depths.insert(name.to_string(), depth);
            Ok(depth)
        }

        let mut visiting = std::collections::HashSet::new();
        for step in steps {
            calc_depth(&step.name, steps, &mut depths, &mut visiting)?;
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
    fn interpolate(&self, template: &str, results: &HashMap<String, StepResult>) -> String {
        let mut output = template.to_string();

        for cap in INTERPOLATE_RE.captures_iter(template) {
            let full_match = cap.get(0).unwrap().as_str();
            let step_name = cap.get(1).unwrap().as_str();

            let replacement = results
                .get(step_name)
                .map(|r| r.output.as_str())
                .unwrap_or("[step not found]");

            output = output.replace(full_match, replacement);
        }

        output
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

/// Load a workflow from a TOML file
pub fn load_workflow(path: &Path) -> Result<Workflow> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read workflow file: {}", path.display()))?;

    toml::from_str(&content)
        .with_context(|| format!("Failed to parse workflow: {}", path.display()))
}

/// Print workflow results
pub fn print_results(results: &[StepResult]) {
    println!();
    println!("{}", "Results:".bold());
    println!();

    for result in results {
        let status = if result.success {
            format!("[{}]", "OK".green())
        } else {
            format!("[{}]", "FAIL".red())
        };

        println!(
            "{} {} ({:.1}s)",
            status,
            result.name.bold(),
            result.elapsed_ms as f64 / 1000.0
        );
        println!();

        // Indent output
        for line in result.output.lines() {
            println!("  {}", line);
        }
        println!();
    }
}
