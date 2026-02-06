use crate::backend::{self, QueryResult};
use crate::config::Config;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::path::Path;

const ROADMAP_PROMPT: &str = r#"You are planning a software project. Create a high-level roadmap.

## Task
{task}

## Instructions

Break this into 3-7 major components/phases. For each, provide:
- Name (snake_case)
- Order (1 = first, no dependencies)
- One-line description
- Key dependencies (which other components must be done first)
- Main technical challenge

Output as a simple numbered list. Be specific about the order of implementation."#;

const SYNTHESIZE_PROMPT: &str = r#"Multiple AI backends created roadmaps for this task:

## Task
{task}

{roadmaps}

## Instructions

Analyze these roadmaps and create a unified plan that:
1. Takes the best ideas from each
2. Resolves any contradictions
3. Ensures proper dependency ordering
4. Covers all necessary components

Output a final roadmap as a numbered list with:
- order: N
- name: component_name
- summary: one line description
- depends_on: [list of dependencies]
- rationale: why this component, why this order (one line)"#;

const SPEC_PROMPT: &str = r#"Generate detailed ARF spec files from this roadmap.

## Task
{task}

## Consensus Roadmap
{roadmap}

## Instructions

For EACH component in the roadmap, output a TOML block:

```toml
[spec.component_name]
order = N
what = "One-line description of what to build"
why = "Why this component is needed"
how = "Implementation approach, key algorithms or patterns"
backup = "Fallback approach if primary fails"
inputs = "What this component receives"
outputs = "What this component produces"
dependencies = ["list", "of", "deps"]
tests = "How to verify correctness"
```

IMPORTANT:
- Use snake_case names matching the roadmap
- Order must match the roadmap
- Be specific and technical in the how/tests fields
- Output ONLY the TOML blocks, no other text"#;

const SUBTASK_PROMPT: &str = r#"Break this component into subtasks (individual files).

## Component
Name: {name}
What: {what}
How: {how}

## Parent Task
{task}

## Instructions

Break this component into 2-5 subtasks, where each subtask is a separate file.
Consider: what distinct pieces make up this component? Each should be independently implementable.

For EACH subtask, output a TOML block:

```toml
[subtask.subtask_name]
order = N
file = "src/{component}/subtask_name.rs"
what = "One-line description"
why = "Why this file is needed"
how = "Implementation details"
inputs = "What this file receives"
outputs = "What this file exports"
tests = "How to verify"
```

IMPORTANT:
- Use snake_case for subtask names
- File paths should be logical (src/{component}/...)
- Order by dependencies within this component (1 = first)
- Output ONLY the TOML blocks, no other text"#;

#[derive(Debug, Deserialize)]
struct SpecEntry {
    #[serde(default)]
    order: u32,
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

#[derive(Debug, Deserialize)]
struct SubtaskEntry {
    #[serde(default)]
    order: u32,
    #[serde(default)]
    file: Option<String>,
    what: String,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    inputs: Option<String>,
    #[serde(default)]
    outputs: Option<String>,
    #[serde(default)]
    tests: Option<String>,
}

pub async fn run(
    config: &Config,
    dir: &Path,
    task: &str,
    backend_filter: Option<&str>,
) -> Result<()> {
    let backends = backend::get_backends(config, backend_filter)?;
    let backend_count = backends.len();

    // If only one backend, skip consensus and go direct
    if backend_count == 1 {
        println!("{} Planning with single backend...", "spec:".cyan().bold());
        return run_single_backend(config, dir, task, backend_filter).await;
    }

    println!("{} Planning: {}", "spec:".cyan().bold(), task);
    println!();

    // Step 1: Get roadmaps from all backends in parallel
    println!(
        "{} Step 1/3: Getting roadmaps from {} backends...",
        "spec:".cyan().bold(),
        backend_count
    );

    let roadmap_prompt = ROADMAP_PROMPT.replace("{task}", task);
    let roadmap_results = backend::run_query(&backends, &roadmap_prompt, dir, config).await?;

    let successful_roadmaps: Vec<&QueryResult> =
        roadmap_results.iter().filter(|r| r.success).collect();

    if successful_roadmaps.is_empty() {
        anyhow::bail!("All backends failed to generate roadmaps");
    }

    println!(
        "  {} {}/{} backends responded",
        "✓".green(),
        successful_roadmaps.len(),
        backend_count
    );

    // Step 2: Synthesize roadmaps into consensus
    println!(
        "{} Step 2/3: Synthesizing consensus roadmap...",
        "spec:".cyan().bold()
    );

    let roadmaps_text = successful_roadmaps
        .iter()
        .map(|r| format!("## {}'s Roadmap\n{}\n", r.backend, r.output))
        .collect::<Vec<_>>()
        .join("\n");

    let synthesize_prompt = SYNTHESIZE_PROMPT
        .replace("{task}", task)
        .replace("{roadmaps}", &roadmaps_text);

    // Use first available backend for synthesis (prefer claude)
    let synth_backend = backend_filter.unwrap_or("claude");
    let synth_backends = backend::get_backends(config, Some(synth_backend))?;
    let synth_results =
        backend::run_query(&synth_backends, &synthesize_prompt, dir, config).await?;

    let consensus = synth_results
        .iter()
        .find(|r| r.success)
        .map(|r| r.output.as_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to synthesize consensus"))?;

    println!("  {} Consensus reached", "✓".green());

    // Step 3: Generate detailed specs from consensus
    println!(
        "{} Step 3/3: Generating detailed specs...",
        "spec:".cyan().bold()
    );

    let spec_prompt = SPEC_PROMPT
        .replace("{task}", task)
        .replace("{roadmap}", consensus);

    let spec_results = backend::run_query(&synth_backends, &spec_prompt, dir, config).await?;

    let spec_output = spec_results
        .iter()
        .find(|r| r.success)
        .map(|r| r.output.as_str())
        .ok_or_else(|| anyhow::anyhow!("Failed to generate specs"))?;

    // Parse and write specs
    let mut specs = parse_specs(spec_output)?;

    if specs.is_empty() {
        anyhow::bail!("No specs parsed from output");
    }

    specs.sort_by_key(|(_, spec)| spec.order);

    let specs_dir = dir.join(".arf").join("specs");
    fs::create_dir_all(&specs_dir).context("Failed to create .arf/specs directory")?;

    // Write roadmap
    let roadmap_content = format_roadmap(task, &specs);
    let roadmap_path = specs_dir.join("roadmap.arf");
    fs::write(&roadmap_path, &roadmap_content).context("Failed to write roadmap.arf")?;

    // Step 4: Generate subtasks for each spec
    println!(
        "{} Step 4/4: Breaking specs into subtasks...",
        "spec:".cyan().bold()
    );

    let mut all_subtasks: Vec<(String, Vec<(String, SubtaskEntry)>)> = Vec::new();

    for (name, spec) in &specs {
        let subtask_prompt = SUBTASK_PROMPT
            .replace("{name}", name)
            .replace("{what}", &spec.what)
            .replace("{how}", spec.how.as_deref().unwrap_or("Not specified"))
            .replace("{task}", task);

        let subtask_results =
            backend::run_query(&synth_backends, &subtask_prompt, dir, config).await?;

        if let Some(result) = subtask_results.iter().find(|r| r.success) {
            let mut subtasks = parse_subtasks(&result.output)?;
            subtasks.sort_by_key(|(_, s)| s.order);
            println!("  {} {} → {} subtasks", "✓".green(), name, subtasks.len());
            all_subtasks.push((name.clone(), subtasks));
        } else {
            println!("  {} {} → no subtasks", "!".yellow(), name);
            all_subtasks.push((name.clone(), Vec::new()));
        }
    }

    // Write specs with nested structure
    println!();
    println!("{}", "=".repeat(50).dimmed());
    println!(
        "{} Generated {} specs in .arf/specs/:",
        "spec:".cyan().bold(),
        specs.len()
    );
    println!();
    println!("  {} roadmap.arf", "+".green());

    for (name, spec) in &specs {
        let step_dir = specs_dir.join(format!("{:02}-{}", spec.order, name));
        fs::create_dir_all(&step_dir)?;

        // Write the step's main spec
        let spec_path = step_dir.join("spec.arf");
        fs::write(&spec_path, format_spec(spec))?;
        println!("  {} {:02}-{}/spec.arf", "+".green(), spec.order, name);

        // Write subtasks if any
        if let Some((_, subtasks)) = all_subtasks.iter().find(|(n, _)| n == name) {
            for (subtask_name, subtask) in subtasks {
                let subtask_filename = format!("{:02}-{}.arf", subtask.order, subtask_name);
                let subtask_path = step_dir.join(&subtask_filename);
                fs::write(&subtask_path, format_subtask(subtask))?;
                println!(
                    "  {} {:02}-{}/{}",
                    "+".green(),
                    spec.order,
                    name,
                    subtask_filename
                );
            }
        }
    }

    println!();
    println!("{}", "Review with: arf spec list".dimmed());

    Ok(())
}

async fn run_single_backend(
    config: &Config,
    dir: &Path,
    task: &str,
    backend_filter: Option<&str>,
) -> Result<()> {
    // Combined prompt for single backend
    let prompt = format!(
        r#"You are a software architect. Plan and spec out this task.

## Task
{task}

## Instructions

1. First, create a roadmap of 3-7 components
2. Then, for EACH component, output a TOML spec block:

```toml
[spec.component_name]
order = N
what = "One-line description"
why = "Why needed"
how = "Implementation approach"
backup = "Fallback if primary fails"
inputs = "What it receives"
outputs = "What it produces"
dependencies = ["deps"]
tests = "How to verify"
```

Use snake_case names. Order by dependencies (1 = first).
Output ONLY the TOML blocks."#,
        task = task
    );

    let backends = backend::get_backends(config, backend_filter)?;
    let results = backend::run_query(&backends, &prompt, dir, config).await?;

    let output = results
        .iter()
        .find(|r| r.success)
        .map(|r| r.output.as_str())
        .ok_or_else(|| anyhow::anyhow!("Backend failed to generate specs"))?;

    let mut specs = parse_specs(output)?;

    if specs.is_empty() {
        anyhow::bail!("No specs parsed");
    }

    specs.sort_by_key(|(_, spec)| spec.order);

    let specs_dir = dir.join(".arf").join("specs");
    fs::create_dir_all(&specs_dir)?;

    let roadmap_content = format_roadmap(task, &specs);
    fs::write(specs_dir.join("roadmap.arf"), &roadmap_content)?;

    // Generate subtasks for each spec
    println!("{} Breaking specs into subtasks...", "spec:".cyan().bold());

    let mut all_subtasks: Vec<(String, Vec<(String, SubtaskEntry)>)> = Vec::new();

    for (name, spec) in &specs {
        let subtask_prompt = SUBTASK_PROMPT
            .replace("{name}", name)
            .replace("{what}", &spec.what)
            .replace("{how}", spec.how.as_deref().unwrap_or("Not specified"))
            .replace("{task}", task);

        let subtask_results = backend::run_query(&backends, &subtask_prompt, dir, config).await?;

        if let Some(result) = subtask_results.iter().find(|r| r.success) {
            let mut subtasks = parse_subtasks(&result.output)?;
            subtasks.sort_by_key(|(_, s)| s.order);
            println!("  {} {} → {} subtasks", "✓".green(), name, subtasks.len());
            all_subtasks.push((name.clone(), subtasks));
        } else {
            println!("  {} {} → no subtasks", "!".yellow(), name);
            all_subtasks.push((name.clone(), Vec::new()));
        }
    }

    println!();
    println!("{}", "=".repeat(50).dimmed());
    println!("{} Generated {} specs:", "spec:".cyan().bold(), specs.len());
    println!();
    println!("  {} roadmap.arf", "+".green());

    for (name, spec) in &specs {
        let step_dir = specs_dir.join(format!("{:02}-{}", spec.order, name));
        fs::create_dir_all(&step_dir)?;

        let spec_path = step_dir.join("spec.arf");
        fs::write(&spec_path, format_spec(spec))?;
        println!("  {} {:02}-{}/spec.arf", "+".green(), spec.order, name);

        if let Some((_, subtasks)) = all_subtasks.iter().find(|(n, _)| n == name) {
            for (subtask_name, subtask) in subtasks {
                let subtask_filename = format!("{:02}-{}.arf", subtask.order, subtask_name);
                let subtask_path = step_dir.join(&subtask_filename);
                fs::write(&subtask_path, format_subtask(subtask))?;
                println!(
                    "  {} {:02}-{}/{}",
                    "+".green(),
                    spec.order,
                    name,
                    subtask_filename
                );
            }
        }
    }

    println!();
    println!("{}", "Review with: arf spec list".dimmed());

    Ok(())
}

fn parse_specs(output: &str) -> Result<Vec<(String, SpecEntry)>> {
    let mut specs = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_block = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("[spec.") && trimmed.ends_with(']') {
            if let Some(name) = current_name.take() {
                if let Ok(entry) = parse_single_spec(&current_block) {
                    specs.push((name, entry));
                }
            }

            let name = trimmed
                .trim_start_matches("[spec.")
                .trim_end_matches(']')
                .to_string();
            current_name = Some(name);
            current_block.clear();
        } else if current_name.is_some() && !trimmed.starts_with("```") {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

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

fn format_roadmap(task: &str, specs: &[(String, SpecEntry)]) -> String {
    let mut out = String::new();

    out.push_str(&format!("what = {:?}\n", task));
    out.push_str("why = \"Structured implementation plan with dependency ordering\"\n");
    out.push('\n');

    for (name, spec) in specs {
        out.push_str("[[steps]]\n");
        out.push_str(&format!("order = {}\n", spec.order));
        out.push_str(&format!("spec = {:?}\n", name));
        out.push_str(&format!("dir = \"{:02}-{}\"\n", spec.order, name));
        out.push_str(&format!("summary = {:?}\n", spec.what));
        if !spec.dependencies.is_empty() {
            out.push_str(&format!("depends_on = {:?}\n", spec.dependencies));
        }
        out.push('\n');
    }

    out
}

fn format_spec(spec: &SpecEntry) -> String {
    let mut out = String::new();

    out.push_str(&format!("order = {}\n", spec.order));
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

fn parse_subtasks(output: &str) -> Result<Vec<(String, SubtaskEntry)>> {
    let mut subtasks = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_block = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("[subtask.") && trimmed.ends_with(']') {
            if let Some(name) = current_name.take() {
                if let Ok(entry) = parse_single_subtask(&current_block) {
                    subtasks.push((name, entry));
                }
            }

            let name = trimmed
                .trim_start_matches("[subtask.")
                .trim_end_matches(']')
                .to_string();
            current_name = Some(name);
            current_block.clear();
        } else if current_name.is_some() && !trimmed.starts_with("```") {
            current_block.push_str(line);
            current_block.push('\n');
        }
    }

    if let Some(name) = current_name {
        if let Ok(entry) = parse_single_subtask(&current_block) {
            subtasks.push((name, entry));
        }
    }

    Ok(subtasks)
}

fn parse_single_subtask(block: &str) -> Result<SubtaskEntry> {
    toml::from_str(block).context("Failed to parse subtask TOML")
}

fn format_subtask(subtask: &SubtaskEntry) -> String {
    let mut out = String::new();

    out.push_str(&format!("order = {}\n", subtask.order));
    out.push_str(&format!("what = {:?}\n", subtask.what));

    if let Some(ref file) = subtask.file {
        out.push_str(&format!("file = {:?}\n", file));
    }

    if let Some(ref why) = subtask.why {
        out.push_str(&format!("why = {:?}\n", why));
    }

    if let Some(ref how) = subtask.how {
        out.push_str(&format!("how = {:?}\n", how));
    }

    out.push('\n');
    out.push_str("[context]\n");

    if let Some(ref inputs) = subtask.inputs {
        out.push_str(&format!("inputs = {:?}\n", inputs));
    }

    if let Some(ref outputs) = subtask.outputs {
        out.push_str(&format!("outputs = {:?}\n", outputs));
    }

    if let Some(ref tests) = subtask.tests {
        out.push_str(&format!("tests = {:?}\n", tests));
    }

    out
}
