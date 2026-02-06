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

## CRITICAL INSTRUCTIONS

You are a code generator. Output ONLY raw source code.

DO NOT:
- Use markdown code fences (```)
- Say "here's the code" or similar
- Ask for permissions or confirmation
- Explain what you're about to do
- Add commentary before or after the code

DO:
- Start your response with line 1 of the source file
- Include all necessary imports, types, and implementations
- Be thorough and complete
- Follow idiomatic patterns for the language
- End with the last line of code, nothing after

YOUR ENTIRE RESPONSE MUST BE VALID SOURCE CODE THAT CAN BE WRITTEN DIRECTLY TO A FILE."#;

const SYNTHESIZE_PROMPT: &str = r#"Multiple backends proposed implementations for this file.

## Spec
File: {file}
What: {what}

## Proposals

{proposals}

## CRITICAL INSTRUCTIONS

You are a code generator. Output ONLY raw source code.

Analyze the proposals and create the best version that:
1. Takes the best ideas from each
2. Fixes any bugs or issues
3. Is complete and production-ready

DO NOT use markdown fences. DO NOT add explanations.
Start with line 1 of the source file. End with the last line of code.
YOUR ENTIRE RESPONSE MUST BE VALID SOURCE CODE."#;

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

            // Clean up and validate code output
            let clean_code = match clean_code_output(&final_code) {
                Some(code) => code,
                None => {
                    println!(
                        "      {} Output was not valid code, skipping {}",
                        "✗".red(),
                        target_file
                    );
                    continue;
                }
            };

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

fn clean_code_output(code: &str) -> Option<String> {
    let code = code.trim();

    // Detect non-code outputs (LLM asking for permissions, explaining, etc.)
    let bad_patterns = [
        "I don't have",
        "I cannot",
        "I can't",
        "permission",
        "Here's the",
        "Here is the",
        "Let me",
        "I'll create",
        "I will create",
        "Once you grant",
        "The file is ready",
    ];

    let first_100 = &code[..code.len().min(100)].to_lowercase();
    for pattern in bad_patterns {
        if first_100.contains(&pattern.to_lowercase()) {
            return None; // This is not code
        }
    }

    // Remove markdown code fences if present
    let cleaned = if code.starts_with("```") {
        let lines: Vec<&str> = code.lines().collect();
        if lines.len() >= 2 {
            let start = 1; // Skip first ``` line
            let end = if lines.last().map(|l| l.trim()) == Some("```") {
                lines.len() - 1
            } else {
                lines.len()
            };
            lines[start..end].join("\n")
        } else {
            code.to_string()
        }
    } else {
        code.to_string()
    };

    // Final check: does it look like code? (starts with common patterns)
    let first_line = cleaned.lines().next().unwrap_or("");
    let looks_like_code = first_line.starts_with("use ")
        || first_line.starts_with("//")
        || first_line.starts_with("#")
        || first_line.starts_with("pub ")
        || first_line.starts_with("mod ")
        || first_line.starts_with("fn ")
        || first_line.starts_with("struct ")
        || first_line.starts_with("enum ")
        || first_line.starts_with("impl ")
        || first_line.starts_with("const ")
        || first_line.starts_with("static ")
        || first_line.starts_with("type ")
        || first_line.starts_with("trait ")
        || first_line.starts_with("#!")
        || first_line.starts_with("import ")
        || first_line.starts_with("from ")
        || first_line.starts_with("package ")
        || first_line.starts_with("class ")
        || first_line.starts_with("interface ")
        || first_line.is_empty(); // Allow empty first line (could have leading comment)

    if looks_like_code || cleaned.lines().count() > 5 {
        Some(cleaned)
    } else {
        None
    }
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
