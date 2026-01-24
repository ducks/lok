use crate::backend::{self, Backend};
use crate::config::Config;
use crate::delegation::Delegator;
use anyhow::Result;
use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

/// Team mode: smart delegation with optional debate
pub struct Team {
    backends: Vec<Arc<dyn Backend>>,
    delegator: Delegator,
    cwd: std::path::PathBuf,
}

impl Team {
    pub fn new(config: &Config, cwd: &Path) -> Result<Self> {
        let backends = backend::get_backends(config, None)?;
        Ok(Self {
            backends,
            delegator: Delegator::new(),
            cwd: cwd.to_path_buf(),
        })
    }

    pub async fn execute(&self, task: &str, debate: bool) -> Result<String> {
        println!("{}", "Team Mode".cyan().bold());
        println!("{}", "=".repeat(50).dimmed());
        println!("Task: {}", task);
        println!();

        // Analyze the task
        let categories = self.delegator.classify_task(task);
        println!(
            "Categories: {}",
            categories
                .iter()
                .map(|c| format!("{:?}", c))
                .collect::<Vec<_>>()
                .join(", ")
                .yellow()
        );

        // Get recommendations filtered by available backends
        let recommendations = self.delegator.recommend(task);
        let available_names: Vec<_> = self.backends.iter().map(|b| b.name()).collect();

        // Find first recommended backend that's actually available
        let primary = recommendations
            .iter()
            .find(|r| available_names.contains(&r.name.as_str()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No suitable backends available. Recommended: {:?}, Available: {:?}",
                    recommendations.iter().map(|r| &r.name).collect::<Vec<_>>(),
                    available_names
                )
            })?;

        println!(
            "Primary: {} - {}",
            primary.name.to_uppercase().green().bold(),
            primary.style.dimmed()
        );
        println!();

        // Find the primary backend (guaranteed to exist now)
        let primary_backend = self
            .backends
            .iter()
            .find(|b| b.name() == primary.name)
            .unwrap();

        // Query primary
        println!(
            "{} Querying {}...",
            "→".cyan(),
            primary.name.to_uppercase()
        );
        let primary_result = primary_backend.query(task, &self.cwd).await?;

        println!();
        println!(
            "{}",
            format!("=== {} ===", primary.name.to_uppercase())
                .green()
                .bold()
        );
        println!("{}", primary_result);

        if !debate {
            return Ok(primary_result);
        }

        // If debate mode, get other opinions
        println!();
        println!("{}", "[Getting second opinions...]".yellow());

        let mut all_responses = vec![(primary.name.clone(), primary_result.clone())];

        for other_profile in recommendations.iter().skip(1).take(1) {
            if let Some(other_backend) = self.backends.iter().find(|b| b.name() == other_profile.name) {
                let prompt = format!(
                    "Another AI gave this analysis for the task '{}':\n\n{}\n\n\
                    Do you agree? What did they miss or get wrong? Provide your own analysis.",
                    task, primary_result
                );

                println!(
                    "{} Querying {}...",
                    "→".cyan(),
                    other_profile.name.to_uppercase()
                );

                match other_backend.query(&prompt, &self.cwd).await {
                    Ok(response) => {
                        println!();
                        println!(
                            "{}",
                            format!("=== {} ===", other_profile.name.to_uppercase())
                                .green()
                                .bold()
                        );
                        println!("{}", response);
                        all_responses.push((other_profile.name.clone(), response));
                    }
                    Err(e) => {
                        eprintln!(
                            "{} {} failed: {}",
                            "!".red(),
                            other_profile.name,
                            e
                        );
                    }
                }
            }
        }

        // Synthesize results
        let summary = all_responses
            .iter()
            .map(|(name, response)| format!("**{}**: {}", name.to_uppercase(), truncate(response, 500)))
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(format!(
            "Team Analysis Complete\n\nPrimary analyst: {}\n\n{}",
            primary.name.to_uppercase(),
            summary
        ))
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}
