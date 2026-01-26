//! Workflow engine - declarative multi-step LLM pipelines
//!
//! Workflows are TOML files that define a sequence of steps, each using
//! a backend to process a prompt. Steps can depend on previous steps
//! and interpolate their outputs.

use crate::backend;
use crate::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    pub async fn run(&self, workflow: &Workflow) -> Result<Vec<StepResult>> {
        let mut results: HashMap<String, StepResult> = HashMap::new();
        let mut ordered_results: Vec<StepResult> = Vec::new();

        // Topological sort of steps based on dependencies
        let execution_order = self.resolve_order(&workflow.steps)?;

        println!("{} {}", "Running workflow:".bold(), workflow.name.cyan());
        if let Some(ref desc) = workflow.description {
            println!("{}", desc.dimmed());
        }
        println!("{}", "=".repeat(50).dimmed());
        println!();

        for step_name in execution_order {
            let step = workflow.steps.iter().find(|s| s.name == step_name).unwrap();

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

            println!("{} {}", "[step]".cyan(), step.name.bold());

            // Interpolate variables in prompt
            let prompt = self.interpolate(&step.prompt, &results);

            // Get backend
            let backend_config = self
                .config
                .backends
                .get(&step.backend)
                .ok_or_else(|| anyhow::anyhow!("Backend not found: {}", step.backend))?;

            let backend = backend::create_backend(&step.backend, backend_config)?;

            if !backend.is_available() {
                let result = StepResult {
                    name: step.name.clone(),
                    output: format!("Backend {} not available", step.backend),
                    success: false,
                    elapsed_ms: 0,
                };
                println!("  {} Backend not available", "✗".red());
                results.insert(step.name.clone(), result.clone());
                ordered_results.push(result);
                continue;
            }

            // Execute
            let start = std::time::Instant::now();
            let output = backend.query(&prompt, &self.cwd).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            let result = match output {
                Ok(text) => {
                    println!("  {} ({:.1}s)", "✓".green(), elapsed_ms as f64 / 1000.0);
                    StepResult {
                        name: step.name.clone(),
                        output: text,
                        success: true,
                        elapsed_ms,
                    }
                }
                Err(e) => {
                    println!("  {} {}", "✗".red(), e);
                    StepResult {
                        name: step.name.clone(),
                        output: format!("Error: {}", e),
                        success: false,
                        elapsed_ms,
                    }
                }
            };

            results.insert(step.name.clone(), result.clone());
            ordered_results.push(result);
        }

        println!();
        println!("{}", "=".repeat(50).dimmed());

        Ok(ordered_results)
    }

    /// Resolve execution order using topological sort
    fn resolve_order(&self, steps: &[Step]) -> Result<Vec<String>> {
        let mut order = Vec::new();
        let mut visited = HashMap::new();
        let step_map: HashMap<_, _> = steps.iter().map(|s| (s.name.clone(), s)).collect();

        for step in steps {
            self.visit(&step.name, &step_map, &mut visited, &mut order)?;
        }

        Ok(order)
    }

    fn visit(
        &self,
        name: &str,
        steps: &HashMap<String, &Step>,
        visited: &mut HashMap<String, bool>,
        order: &mut Vec<String>,
    ) -> Result<()> {
        if let Some(&in_progress) = visited.get(name) {
            if in_progress {
                anyhow::bail!("Circular dependency detected at step: {}", name);
            }
            return Ok(()); // Already processed
        }

        visited.insert(name.to_string(), true); // Mark as in progress

        if let Some(step) = steps.get(name) {
            for dep in &step.depends_on {
                if !steps.contains_key(dep) {
                    anyhow::bail!("Step '{}' depends on unknown step '{}'", name, dep);
                }
                self.visit(dep, steps, visited, order)?;
            }
        }

        visited.insert(name.to_string(), false); // Mark as done
        order.push(name.to_string());

        Ok(())
    }

    /// Interpolate {{ steps.X.output }} variables in a string
    fn interpolate(&self, template: &str, results: &HashMap<String, StepResult>) -> String {
        let mut output = template.to_string();

        // Match {{ steps.NAME.output }}
        let re = regex::Regex::new(r"\{\{\s*steps\.([a-zA-Z0-9_-]+)\.output\s*\}\}").unwrap();

        for cap in re.captures_iter(template) {
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
        if let Some(caps) =
            regex::Regex::new(r#"steps\.([a-zA-Z0-9_-]+)\.output\s+contains\s+['"](.+)['"]"#)
                .unwrap()
                .captures(condition)
        {
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
