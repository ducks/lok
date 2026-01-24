use crate::backend::{self, Backend};
use anyhow::Result;
use colored::Colorize;
use std::path::Path;
use std::sync::Arc;

const MAX_ROUNDS: usize = 3;

pub struct Debate {
    backends: Vec<Arc<dyn Backend>>,
    topic: String,
    cwd: std::path::PathBuf,
}

struct Position {
    backend: String,
    stance: String,
}

impl Debate {
    pub fn new(backends: Vec<Arc<dyn Backend>>, topic: &str, cwd: &Path) -> Self {
        Self {
            backends,
            topic: topic.to_string(),
            cwd: cwd.to_path_buf(),
        }
    }

    pub async fn run(&self) -> Result<String> {
        println!("{}", "Lok Debate".cyan().bold());
        println!("{}", "=".repeat(50).dimmed());
        println!("Topic: {}", self.topic);
        println!();

        // Round 1: Initial positions
        println!("{}", "[Round 1: Initial Positions]".yellow().bold());
        let mut positions = self.get_initial_positions().await?;
        self.print_positions(&positions);

        if positions.len() < 2 {
            return Ok(positions
                .first()
                .map(|p| p.stance.clone())
                .unwrap_or_default());
        }

        // Round 2+: Responses to each other
        for round in 2..=MAX_ROUNDS {
            println!();
            println!(
                "{}",
                format!("[Round {}: Responses]", round).yellow().bold()
            );

            let new_positions = self.get_responses(&positions).await?;

            // Check for consensus
            if self.check_consensus(&new_positions) {
                println!();
                println!("{}", "[Consensus Reached]".green().bold());
                return Ok(self.summarize_consensus(&new_positions));
            }

            positions = new_positions;
            self.print_positions(&positions);
        }

        // Final summary
        println!();
        println!(
            "{}",
            "[Final Positions - No Full Consensus]".yellow().bold()
        );
        Ok(self.summarize_disagreement(&positions))
    }

    async fn get_initial_positions(&self) -> Result<Vec<Position>> {
        let prompt = format!(
            "Question: {}\n\nProvide your position on this. Be specific and concise. \
            If analyzing code, reference specific files/lines.",
            self.topic
        );

        let results = backend::run_query(&self.backends, &prompt, &self.cwd).await?;

        Ok(results
            .into_iter()
            .filter(|r| r.success)
            .map(|r| Position {
                backend: r.backend,
                stance: r.output,
            })
            .collect())
    }

    async fn get_responses(&self, positions: &[Position]) -> Result<Vec<Position>> {
        let mut new_positions = Vec::new();

        for backend in &self.backends {
            let others: Vec<_> = positions
                .iter()
                .filter(|p| p.backend != backend.name())
                .collect();

            if others.is_empty() {
                continue;
            }

            let other_positions = others
                .iter()
                .map(|p| format!("{} said: {}", p.backend.to_uppercase(), p.stance))
                .collect::<Vec<_>>()
                .join("\n\n");

            let prompt = format!(
                "Original question: {}\n\n\
                Other positions:\n{}\n\n\
                Respond to these positions. Do you agree, disagree, or partially agree? \
                Point out any errors in their analysis or things they missed. \
                Update your position if they made valid points. Be specific and concise.",
                self.topic, other_positions
            );

            println!("  {} thinking...", backend.name().dimmed());

            match backend.query(&prompt, &self.cwd).await {
                Ok(response) => {
                    new_positions.push(Position {
                        backend: backend.name().to_string(),
                        stance: response,
                    });
                }
                Err(e) => {
                    eprintln!("  {} error: {}", backend.name().red(), e);
                }
            }
        }

        Ok(new_positions)
    }

    fn print_positions(&self, positions: &[Position]) {
        for pos in positions {
            println!();
            println!(
                "{}",
                format!("=== {} ===", pos.backend.to_uppercase())
                    .green()
                    .bold()
            );
            println!("{}", pos.stance);
        }
    }

    fn check_consensus(&self, positions: &[Position]) -> bool {
        // Simple heuristic: if responses are short and contain agreement language
        if positions.len() < 2 {
            return true;
        }

        let agreement_signals = ["agree", "correct", "right", "valid point", "concur"];
        let disagreement_signals = ["disagree", "incorrect", "wrong", "missed", "but"];

        let mut agreement_count = 0;
        let mut disagreement_count = 0;

        for pos in positions {
            let lower = pos.stance.to_lowercase();
            for signal in &agreement_signals {
                if lower.contains(signal) {
                    agreement_count += 1;
                    break;
                }
            }
            for signal in &disagreement_signals {
                if lower.contains(signal) {
                    disagreement_count += 1;
                    break;
                }
            }
        }

        agreement_count > disagreement_count && disagreement_count == 0
    }

    fn summarize_consensus(&self, positions: &[Position]) -> String {
        let summary = positions
            .iter()
            .map(|p| format!("**{}**: {}", p.backend, truncate(&p.stance, 200)))
            .collect::<Vec<_>>()
            .join("\n\n");

        format!("Lok reached consensus:\n\n{}", summary)
    }

    fn summarize_disagreement(&self, positions: &[Position]) -> String {
        let summary = positions
            .iter()
            .map(|p| format!("**{}**: {}", p.backend, truncate(&p.stance, 300)))
            .collect::<Vec<_>>()
            .join("\n\n");

        format!(
            "Lok has differing views:\n\n{}\n\n\
            Consider both perspectives when making your decision.",
            summary
        )
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
