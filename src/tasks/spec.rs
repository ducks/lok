use crate::backend;
use crate::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::path::Path;

const SPEC_PROMPT: &str = r#"You are a software architect decomposing a task into subtasks.

## Task
{task}

## Instructions

Break this task into 3-7 discrete, well-scoped subtasks. Each subtask should be:
- Independently implementable
- Clearly bounded (not overlapping with others)
- Testable in isolation

For each subtask, output a TOML block with this exact format:

```toml
[spec.name_of_subtask]
what = "One-line description of what to build"
why = "Why this subtask is needed, what problem it solves"
how = "Implementation approach, key algorithms or patterns to use"
backup = "Fallback approach if primary approach fails"
inputs = "What this component receives"
outputs = "What this component produces"
dependencies = ["list", "of", "other", "subtask", "names"]
tests = "How to verify this works correctly"
```

Use snake_case for subtask names. Order subtasks by dependency (foundations first).

Output ONLY the TOML blocks, no other text."#;

#[derive(Debug, Deserialize)]
struct SpecEntry {
    what: String,
    why: String,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    backup: Option<String>,
    #[serde(default)]
    inputs: Option<String>,
    #[serde(default)]
    outputs: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    tests: Option<String>,
}

pub async fn run(
    config: &Config,
    dir: &Path,
    task: &str,
    backend_filter: Option<&str>,
) -> Result<()> {
    println!("{} Planning: {}", "spec:".cyan().bold(), task);
    println!();

    // Build prompt
    let prompt = SPEC_PROMPT.replace("{task}", task);

    // Get backends (prefer claude for planning)
    let backends = backend::get_backends(config, backend_filter)?;

    // Run query
    let results = backend::run_query(&backends, &prompt, dir, config).await?;

    // Use first successful result
    let output = results
        .iter()
        .find(|r| r.success)
        .map(|r| r.output.as_str())
        .ok_or_else(|| anyhow::anyhow!("All backends failed to generate specs"))?;

    // Parse specs from output
    let specs = parse_specs(output)?;

    if specs.is_empty() {
        anyhow::bail!("No specs parsed from LLM output");
    }

    // Create .arf/specs directory
    let specs_dir = dir.join(".arf").join("specs");
    fs::create_dir_all(&specs_dir).context("Failed to create .arf/specs directory")?;

    // Write each spec
    println!("{}", "=".repeat(50).dimmed());
    println!(
        "{} Generated {} specs in .arf/specs/:",
        "spec:".cyan().bold(),
        specs.len()
    );
    println!();

    for (name, spec) in &specs {
        let filename = format!("{}.arf", name);
        let path = specs_dir.join(&filename);
        let content = format_spec(spec);
        fs::write(&path, &content).with_context(|| format!("Failed to write {}", filename))?;
        println!("  {} {}", "+".green(), filename);
    }

    println!();
    println!("{}", "Review with: arf spec list".dimmed());

    Ok(())
}

fn parse_specs(output: &str) -> Result<Vec<(String, SpecEntry)>> {
    let mut specs = Vec::new();

    // Find all [spec.name] sections
    let mut current_name: Option<String> = None;
    let mut current_block = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Check for [spec.name] header
        if trimmed.starts_with("[spec.") && trimmed.ends_with(']') {
            // Save previous block if exists
            if let Some(name) = current_name.take() {
                if let Ok(entry) = parse_single_spec(&current_block) {
                    specs.push((name, entry));
                }
            }

            // Extract name
            let name = trimmed
                .trim_start_matches("[spec.")
                .trim_end_matches(']')
                .to_string();
            current_name = Some(name);
            current_block.clear();
        } else if current_name.is_some() && !trimmed.starts_with("```") {
            // Accumulate lines for current block
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    // Don't forget last block
    if let Some(name) = current_name {
        if let Ok(entry) = parse_single_spec(&current_block) {
            specs.push((name, entry));
        }
    }

    Ok(specs)
}

fn parse_single_spec(block: &str) -> Result<SpecEntry> {
    toml::from_str(block).context("Failed to parse spec TOML")
}

fn format_spec(spec: &SpecEntry) -> String {
    let mut out = String::new();

    out.push_str(&format!("what = {:?}\n", spec.what));
    out.push_str(&format!("why = {:?}\n", spec.why));

    if let Some(ref how) = spec.how {
        out.push_str(&format!("how = {:?}\n", how));
    }

    if let Some(ref backup) = spec.backup {
        out.push_str(&format!("backup = {:?}\n", backup));
    }

    out.push('\n');
    out.push_str("[context]\n");

    if let Some(ref inputs) = spec.inputs {
        out.push_str(&format!("inputs = {:?}\n", inputs));
    }

    if let Some(ref outputs) = spec.outputs {
        out.push_str(&format!("outputs = {:?}\n", outputs));
    }

    if !spec.dependencies.is_empty() {
        out.push_str(&format!("dependencies = {:?}\n", spec.dependencies));
    }

    if let Some(ref tests) = spec.tests {
        out.push_str(&format!("tests = {:?}\n", tests));
    }

    out
}
