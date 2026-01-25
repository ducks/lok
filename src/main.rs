mod backend;
mod conductor;
mod config;
mod debate;
mod delegation;
mod output;
mod spawn;
mod tasks;
mod team;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "lok")]
#[command(about = "Multi-LLM orchestration tool for code analysis")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to config file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Verbose output (show prompts, timing, debug info)
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Ask LLM backends a question
    Ask {
        /// The prompt to send
        prompt: String,

        /// Specific backends to use (comma-separated)
        #[arg(short, long)]
        backend: Option<String>,

        /// Working directory for the query
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },

    /// Run a bug hunt on a codebase
    Hunt {
        /// Directory to analyze
        #[arg(default_value = ".")]
        dir: PathBuf,
    },

    /// Run a security audit on a codebase
    Audit {
        /// Directory to analyze
        #[arg(default_value = ".")]
        dir: PathBuf,
    },

    /// Initialize a new lok.toml config file
    Init,

    /// List available backends
    Backends,

    /// Run with Claude as conductor (multi-round orchestration)
    Conduct {
        /// The task to accomplish
        task: String,

        /// Working directory for the analysis
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },

    /// Run a multi-round debate between backends
    Debate {
        /// The topic/question to debate
        topic: String,

        /// Working directory for the analysis
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Specific backends to include (comma-separated)
        #[arg(short, long)]
        backend: Option<String>,
    },

    /// Suggest which backend to use for a task
    Suggest {
        /// The task/prompt to analyze
        task: String,
    },

    /// Ask with smart backend selection
    Smart {
        /// The prompt to send
        prompt: String,

        /// Working directory for the query
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },

    /// Run task with team mode (smart delegation + optional debate)
    Team {
        /// The task to accomplish
        task: String,

        /// Working directory for the analysis
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Enable debate mode (get second opinions)
        #[arg(long)]
        debate: bool,
    },

    /// Check which backends are available and ready
    Doctor,

    /// Spawn parallel agents to work on a task
    Spawn {
        /// The task to accomplish
        task: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Manually specify agents (format: "name:description")
        #[arg(short, long)]
        agent: Option<Vec<String>>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::load_config(cli.config.as_deref())?;

    match cli.command {
        Commands::Ask {
            prompt,
            backend,
            dir,
        } => {
            let backends = backend::get_backends(&config, backend.as_deref())?;

            if cli.verbose {
                backend::print_verbose_header(&prompt, &backends, &dir);
            }

            let results = backend::run_query(&backends, &prompt, &dir, &config).await?;
            output::print_results(&results);

            if cli.verbose {
                backend::print_verbose_timing(&results);
            }
        }
        Commands::Hunt { dir } => {
            tasks::hunt::run(&config, &dir).await?;
        }
        Commands::Audit { dir } => {
            tasks::audit::run(&config, &dir).await?;
        }
        Commands::Init => {
            config::init_config()?;
        }
        Commands::Backends => {
            backend::list_backends(&config)?;
        }
        Commands::Conduct { task, dir } => {
            let conductor = conductor::Conductor::new(&config)?;
            let result = conductor.conduct(&task, &dir).await?;
            println!();
            println!("{}", "=== Final Result ===".green().bold());
            println!();
            println!("{}", result);
        }
        Commands::Debate {
            topic,
            dir,
            backend,
        } => {
            let backends = backend::get_backends(&config, backend.as_deref())?;
            let debate = debate::Debate::new(backends, &topic, &dir, &config);
            let result = debate.run().await?;
            println!();
            println!("{}", result);
        }
        Commands::Suggest { task } => {
            let delegator = delegation::Delegator::new();
            println!("{}", delegator.explain(&task));
        }
        Commands::Smart { prompt, dir } => {
            let delegator = delegation::Delegator::new();
            let recommendations = delegator.recommend(&prompt);

            // Try each recommended backend in order until one is available
            let mut selected_backend = None;
            for rec in &recommendations {
                if backend::get_backends(&config, Some(&rec.name)).is_ok() {
                    selected_backend = Some(rec.name.as_str());
                    break;
                }
            }

            match selected_backend {
                Some(backend_name) => {
                    println!(
                        "{} Using {} for this task",
                        "smart:".cyan().bold(),
                        backend_name.to_uppercase().green()
                    );
                    println!();

                    let backends = backend::get_backends(&config, Some(backend_name))?;

                    if cli.verbose {
                        backend::print_verbose_header(&prompt, &backends, &dir);
                    }

                    let results = backend::run_query(&backends, &prompt, &dir, &config).await?;
                    output::print_results(&results);

                    if cli.verbose {
                        backend::print_verbose_timing(&results);
                    }
                }
                None => {
                    println!(
                        "{} No recommended backend available, using all",
                        "smart:".yellow()
                    );
                    let backends = backend::get_backends(&config, None)?;

                    if cli.verbose {
                        backend::print_verbose_header(&prompt, &backends, &dir);
                    }

                    let results = backend::run_query(&backends, &prompt, &dir, &config).await?;
                    output::print_results(&results);

                    if cli.verbose {
                        backend::print_verbose_timing(&results);
                    }
                }
            }
        }
        Commands::Team { task, dir, debate } => {
            let team = team::Team::new(&config, &dir)?;
            let result = team.execute(&task, debate).await?;
            println!();
            println!("{}", "=".repeat(50).dimmed());
            println!("{}", result);
        }
        Commands::Doctor => {
            println!("{}", "Lok Doctor".cyan().bold());
            println!("{}", "=".repeat(50).dimmed());
            println!();
            println!(
                "Lok is an orchestration layer for LLM backends. It's the brain\n\
                that coordinates the arms you already have installed.\n"
            );
            println!("{}", "Checking backends...".yellow());
            println!();

            let checks = vec![
                ("codex", "codex", "npm install -g @openai/codex"),
                ("gemini", "npx", "Install Node.js (npx comes with npm)"),
                (
                    "claude",
                    "claude",
                    "Install Claude Code: https://claude.ai/claude-code",
                ),
            ];

            let mut available = 0;
            for (name, binary, install_hint) in &checks {
                let found = which::which(binary).is_ok();

                if found {
                    println!("  {} {} - ready", "✓".green(), name);
                    available += 1;
                } else {
                    println!("  {} {} - not found", "✗".red(), name);
                    println!("    {}", install_hint.dimmed());
                }
            }

            // Check API keys
            println!();
            println!("{}", "Checking API keys...".yellow());
            println!();

            let keys = vec![
                ("ANTHROPIC_API_KEY", "claude backend"),
                ("GOOGLE_API_KEY", "gemini backend"),
                ("AWS_PROFILE", "bedrock backend (or AWS_ACCESS_KEY_ID)"),
            ];

            for (key, desc) in &keys {
                if std::env::var(key).is_ok() {
                    println!("  {} {} - set ({})", "✓".green(), key, desc);
                } else {
                    println!("  {} {} - not set ({})", "○".yellow(), key, desc);
                }
            }

            println!();
            if available > 0 {
                println!(
                    "{} {} backend(s) ready. Run {} to see them.",
                    "✓".green(),
                    available,
                    "lok backends".cyan()
                );
            } else {
                println!(
                    "{} No backends found. Install at least one LLM CLI to get started.",
                    "!".red()
                );
            }
        }
        Commands::Spawn { task, dir, agent } => {
            let spawner = spawn::Spawn::new(&config, &dir)?;

            // Parse manual agents if provided
            let manual_agents = agent.map(|agents| {
                agents
                    .iter()
                    .filter_map(|a| {
                        let parts: Vec<&str> = a.splitn(2, ':').collect();
                        if parts.len() == 2 {
                            Some(spawn::AgentTask {
                                name: parts[0].trim().to_string(),
                                description: parts[1].trim().to_string(),
                                backend: None,
                            })
                        } else {
                            eprintln!("Invalid agent format: {}. Use 'name:description'", a);
                            None
                        }
                    })
                    .collect()
            });

            let result = spawner.run(&task, manual_agents).await?;
            println!("{}", "=".repeat(50).dimmed());
            println!("{}", "Full output saved.".green());
            println!("{}", result);
        }
    }

    Ok(())
}
