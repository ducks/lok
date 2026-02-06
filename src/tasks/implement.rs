use crate::backend::{self, QueryResult};
use crate::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::path::Path;
use std::process::Command;

const IMPLEMENT_PROMPT: &str = r#"Implement this component based on the spec.

## Spec
File: {file}
What: {what}
Why: {why}
How: {how}

## Context
Inputs: {inputs}
Outputs: {outputs}

## Parent Component
{parent_what}

## Instructions

Write the complete implementation for this file. Output ONLY the code, no markdown
fences, no explanations. The code should be ready to write directly to the file.

Be thorough and complete. Include all necessary imports, types, and implementations.
Follow idiomatic patterns for the language."#;

const SYNTHESIZE_PROMPT: &str = r#"Multiple backends proposed implementations for this file.

## Spec
File: {file}
What: {what}

## Proposals

{proposals}

## Instructions

Analyze these implementations and create the best version that:
1. Takes the best ideas from each
2. Fixes any bugs or issues
3. Is complete and production-ready

Output ONLY the final code, no markdown fences, no explanations."#;

#[derive(Debug, Deserialize)]
struct Roadmap {
    what: String,
    #[serde(default)]
    steps: Vec<RoadmapStep>,
}

#[derive(Debug, Deserialize)]
struct RoadmapStep {
    order: u32,
    spec: String,
    dir: String,
    summary: String,
    #[serde(default)]
    #[allow(dead_code)] // Used for future dependency ordering
    depends_on: Vec<String>,
}

#[allow(dead_code)] // Fields used for future enhancements
#[derive(Debug, Deserialize)]
struct StepSpec {
    #[serde(default)]
    order: u32,
    what: String,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    context: Option<ContextSection>,
}

#[derive(Debug, Deserialize)]
struct ContextSection {
    #[serde(default)]
    inputs: Option<String>,
    #[serde(default)]
    outputs: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubtaskSpec {
    #[serde(default)]
    order: u32,
    what: String,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    context: Option<ContextSection>,
}

pub async fn run(
    config: &Config,
    dir: &Path,
    step_filter: Option<&str>,
    backend_filter: Option<&str>,
    verify: bool,
) -> Result<()> {
    let specs_dir = dir.join(".arf").join("specs");

    if !specs_dir.exists() {
        anyhow::bail!("No specs found. Run 'lok spec' first to generate specs in .arf/specs/");
    }

    // Read roadmap
    let roadmap_path = specs_dir.join("roadmap.arf");
    if !roadmap_path.exists() {
        anyhow::bail!("No roadmap.arf found in .arf/specs/");
    }

    let roadmap_content =
        fs::read_to_string(&roadmap_path).context("Failed to read roadmap.arf")?;
    let roadmap: Roadmap =
        toml::from_str(&roadmap_content).context("Failed to parse roadmap.arf")?;

    println!(
        "{} Implementing: {}",
        "implement:".cyan().bold(),
        roadmap.what
    );
    println!();

    // Filter steps if specified
    let steps_to_run: Vec<&RoadmapStep> = if let Some(filter) = step_filter {
        roadmap
            .steps
            .iter()
            .filter(|s| s.dir == filter || s.spec == filter)
            .collect()
    } else {
        roadmap.steps.iter().collect()
    };

    if steps_to_run.is_empty() {
        if let Some(filter) = step_filter {
            anyhow::bail!("Step '{}' not found in roadmap", filter);
        } else {
            anyhow::bail!("No steps found in roadmap");
        }
    }

    let backends = backend::get_backends(config, backend_filter)?;
    let backend_count = backends.len();

    for step in &steps_to_run {
        println!(
            "{} Step {}: {} ({})",
            "implement:".cyan().bold(),
            step.order,
            step.spec,
            step.summary
        );

        let step_dir = specs_dir.join(&step.dir);
        if !step_dir.exists() {
            println!("  {} Step directory not found, skipping", "!".yellow());
            continue;
        }

        // Read step spec
        let spec_path = step_dir.join("spec.arf");
        let step_spec: StepSpec = if spec_path.exists() {
            let content = fs::read_to_string(&spec_path)?;
            toml::from_str(&content).unwrap_or_else(|_| StepSpec {
                order: step.order,
                what: step.summary.clone(),
                why: None,
                how: None,
                context: None,
            })
        } else {
            StepSpec {
                order: step.order,
                what: step.summary.clone(),
                why: None,
                how: None,
                context: None,
            }
        };

        // Find and process subtasks
        let mut subtasks: Vec<(String, SubtaskSpec)> = Vec::new();
        for entry in fs::read_dir(&step_dir)? {
            let entry = entry?;
            let filename = entry.file_name().to_string_lossy().to_string();
            if filename.ends_with(".arf") && filename != "spec.arf" {
                let content = fs::read_to_string(entry.path())?;
                if let Ok(subtask) = toml::from_str::<SubtaskSpec>(&content) {
                    subtasks.push((filename, subtask));
                }
            }
        }

        subtasks.sort_by_key(|(_, s)| s.order);

        if subtasks.is_empty() {
            println!("  {} No subtasks found", "!".yellow());
            continue;
        }

        println!("  {} {} subtasks to implement", "→".cyan(), subtasks.len());

        for (filename, subtask) in &subtasks {
            let target_file = match &subtask.file {
                Some(f) => f.clone(),
                None => {
                    println!(
                        "    {} {} - no target file specified, skipping",
                        "!".yellow(),
                        filename
                    );
                    continue;
                }
            };

            println!("    {} {} → {}", "→".cyan(), filename, target_file);

            // Build the implementation prompt
            let ctx = subtask.context.as_ref();
            let prompt = IMPLEMENT_PROMPT
                .replace("{file}", &target_file)
                .replace("{what}", &subtask.what)
                .replace("{why}", subtask.why.as_deref().unwrap_or("Not specified"))
                .replace("{how}", subtask.how.as_deref().unwrap_or("Not specified"))
                .replace(
                    "{inputs}",
                    ctx.and_then(|c| c.inputs.as_deref())
                        .unwrap_or("Not specified"),
                )
                .replace(
                    "{outputs}",
                    ctx.and_then(|c| c.outputs.as_deref())
                        .unwrap_or("Not specified"),
                )
                .replace("{parent_what}", &step_spec.what);

            // Query backends
            let results = backend::run_query(&backends, &prompt, dir, config).await?;
            let successful: Vec<&QueryResult> = results.iter().filter(|r| r.success).collect();

            if successful.is_empty() {
                println!("      {} All backends failed", "✗".red());
                continue;
            }

            // If multiple backends, synthesize
            let final_code = if successful.len() > 1 && backend_count > 1 {
                println!(
                    "      {} {}/{} backends responded, synthesizing...",
                    "✓".green(),
                    successful.len(),
                    backend_count
                );

                let proposals = successful
                    .iter()
                    .map(|r| {
                        format!(
                            "## {}'s Implementation\n```\n{}\n```\n",
                            r.backend, r.output
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                let synth_prompt = SYNTHESIZE_PROMPT
                    .replace("{file}", &target_file)
                    .replace("{what}", &subtask.what)
                    .replace("{proposals}", &proposals);

                // Use first backend for synthesis
                let synth_backend = backend_filter.unwrap_or("claude");
                let synth_backends = backend::get_backends(config, Some(synth_backend))?;
                let synth_results =
                    backend::run_query(&synth_backends, &synth_prompt, dir, config).await?;

                synth_results
                    .iter()
                    .find(|r| r.success)
                    .map(|r| r.output.clone())
                    .unwrap_or_else(|| successful[0].output.clone())
            } else {
                println!("      {} Generated", "✓".green());
                successful[0].output.clone()
            };

            // Clean up code (remove markdown fences if present)
            let clean_code = clean_code_output(&final_code);

            // Create parent directories and write file
            let target_path = dir.join(&target_file);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target_path, &clean_code)
                .with_context(|| format!("Failed to write {}", target_file))?;

            println!("      {} Wrote {}", "+".green(), target_file);
        }

        // Verify step if enabled
        if verify {
            println!("  {} Verifying...", "→".cyan());
            if let Err(e) = run_verification(dir) {
                println!("  {} Verification failed: {}", "✗".red(), e);
            } else {
                println!("  {} Verification passed", "✓".green());
            }
        }

        println!();
    }

    println!(
        "{}",
        "Implementation complete. Review the generated code.".dimmed()
    );

    Ok(())
}

fn clean_code_output(code: &str) -> String {
    let code = code.trim();

    // Remove markdown code fences if present
    if code.starts_with("```") {
        let lines: Vec<&str> = code.lines().collect();
        if lines.len() >= 2 {
            let start = 1; // Skip first ``` line
            let end = if lines.last().map(|l| l.trim()) == Some("```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            return lines[start..end].join("\n");
        }
    }

    code.to_string()
}

fn run_verification(dir: &Path) -> Result<()> {
    // Try cargo build for Rust projects
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.exists() {
        let output = Command::new("cargo")
            .arg("check")
            .current_dir(dir)
            .output()
            .context("Failed to run cargo check")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("cargo check failed:\n{}", stderr);
        }
        return Ok(());
    }

    // Try npm/node for JS projects
    let package_json = dir.join("package.json");
    if package_json.exists() {
        // Just check if we can parse the main files
        return Ok(());
    }

    // No verification available
    Ok(())
}
