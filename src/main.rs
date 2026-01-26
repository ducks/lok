mod backend;
mod cache;
mod conductor;
mod config;
mod context;
mod debate;
mod delegation;
mod output;
mod spawn;
mod tasks;
mod team;
mod workflow;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::{Path, PathBuf};

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

        /// Skip cache and force fresh query
        #[arg(long)]
        no_cache: bool,
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

    /// Run or manage workflows (multi-step pipelines)
    #[command(subcommand)]
    Workflow(WorkflowCommands),

    /// Shorthand for 'workflow run'
    Run {
        /// Workflow name or path
        name: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },

    /// Show detected codebase context
    Context {
        /// Directory to analyze
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum WorkflowCommands {
    /// Run a workflow
    Run {
        /// Workflow name or path to .toml file
        name: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },

    /// List available workflows
    List,

    /// Validate a workflow file
    Validate {
        /// Path to workflow file
        path: PathBuf,
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
            no_cache,
        } => {
            let backends = backend::get_backends(&config, backend.as_deref())?;
            if cli.verbose {
                backend::print_verbose_header(&prompt, &backends, &dir);
            }

            let backend_names: Vec<String> =
                backends.iter().map(|b| b.name().to_string()).collect();
            let cwd = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            let cwd_str = cwd.to_string_lossy().to_string();

            // Check cache first (unless --no-cache)
            let cache = cache::Cache::new(&config.cache);
            let cache_key = cache.cache_key(&prompt, &backend_names, &cwd_str);

            if !no_cache {
                if let Some(cached_results) = cache.get(&cache_key) {
                    println!("{}", "(cached)".dimmed());
                    output::print_results(&cached_results);
                    return Ok(());
                }
            }

            let results = backend::run_query(&backends, &prompt, &dir, &config).await?;

            // Cache the results
            if !no_cache {
                let _ = cache.set(&cache_key, &results);
            }

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
        Commands::Workflow(subcmd) => match subcmd {
            WorkflowCommands::Run { name, dir } => {
                run_workflow(&name, &dir, &config).await?;
            }
            WorkflowCommands::List => {
                list_workflows()?;
            }
            WorkflowCommands::Validate { path } => {
                validate_workflow(&path)?;
            }
        },
        Commands::Run { name, dir } => {
            // Shorthand for 'workflow run'
            run_workflow(&name, &dir, &config).await?;
        }
        Commands::Context { dir } => {
            show_context(&dir);
        }
    }

    Ok(())
}

fn show_context(dir: &Path) {
    use colored::Colorize;

    let ctx = context::CodebaseContext::detect(dir);

    println!("{}", "Detected Codebase Context".bold());
    println!("{}", "=".repeat(40));

    if let Some(lang) = &ctx.detected_language {
        println!("Language: {}", lang.cyan());
    }

    // Ruby/Rails
    if ctx.is_rails || ctx.has_goldiloader || ctx.has_bullet || ctx.has_brakeman
        || ctx.has_rubocop || ctx.has_strong_migrations || ctx.has_rspec
        || ctx.has_sidekiq || ctx.has_sorbet
    {
        println!();
        println!("{}", "Ruby/Rails:".bold());
        if ctx.is_rails { println!("  {} Rails", "+".green()); }
        if ctx.has_goldiloader { println!("  {} Goldiloader (auto N+1 prevention)", "+".green()); }
        if ctx.has_bullet { println!("  {} Bullet (N+1 detection)", "+".green()); }
        if ctx.has_brakeman { println!("  {} Brakeman (security)", "+".green()); }
        if ctx.has_rubocop { println!("  {} RuboCop (linting)", "+".green()); }
        if ctx.has_strong_migrations { println!("  {} StrongMigrations (safe migrations)", "+".green()); }
        if ctx.has_rspec { println!("  {} RSpec (testing)", "+".green()); }
        if ctx.has_sidekiq { println!("  {} Sidekiq (background jobs)", "+".green()); }
        if ctx.has_sorbet { println!("  {} Sorbet (type checking)", "+".green()); }
    }

    // JavaScript/TypeScript
    if ctx.has_typescript || ctx.has_eslint || ctx.has_prettier || ctx.has_jest
        || ctx.has_vitest || ctx.has_react || ctx.has_vue || ctx.has_nextjs || ctx.has_tailwind
    {
        println!();
        println!("{}", "JavaScript/TypeScript:".bold());
        if ctx.has_typescript { println!("  {} TypeScript", "+".green()); }
        if ctx.has_react { println!("  {} React", "+".green()); }
        if ctx.has_vue { println!("  {} Vue", "+".green()); }
        if ctx.has_nextjs { println!("  {} Next.js", "+".green()); }
        if ctx.has_eslint { println!("  {} ESLint (linting)", "+".green()); }
        if ctx.has_prettier { println!("  {} Prettier (formatting)", "+".green()); }
        if ctx.has_jest { println!("  {} Jest (testing)", "+".green()); }
        if ctx.has_vitest { println!("  {} Vitest (testing)", "+".green()); }
        if ctx.has_tailwind { println!("  {} Tailwind CSS", "+".green()); }
    }

    // Python
    if ctx.is_python || ctx.is_django || ctx.is_fastapi || ctx.has_sqlalchemy
        || ctx.has_pytest || ctx.has_mypy || ctx.has_ruff || ctx.has_alembic
    {
        println!();
        println!("{}", "Python:".bold());
        if ctx.is_django { println!("  {} Django", "+".green()); }
        if ctx.is_fastapi { println!("  {} FastAPI", "+".green()); }
        if ctx.has_sqlalchemy { println!("  {} SQLAlchemy", "+".green()); }
        if ctx.has_alembic { println!("  {} Alembic (migrations)", "+".green()); }
        if ctx.has_pytest { println!("  {} pytest (testing)", "+".green()); }
        if ctx.has_mypy { println!("  {} mypy (type checking)", "+".green()); }
        if ctx.has_ruff { println!("  {} Ruff (linting)", "+".green()); }
    }

    // Rust
    if ctx.is_rust || ctx.has_tokio || ctx.has_diesel || ctx.has_sqlx {
        println!();
        println!("{}", "Rust:".bold());
        if ctx.has_tokio { println!("  {} Tokio (async runtime)", "+".green()); }
        if ctx.has_diesel { println!("  {} Diesel (ORM)", "+".green()); }
        if ctx.has_sqlx { println!("  {} SQLx (database)", "+".green()); }
    }

    // Go
    if ctx.is_go || ctx.has_golangci_lint {
        println!();
        println!("{}", "Go:".bold());
        if ctx.has_golangci_lint { println!("  {} golangci-lint", "+".green()); }
    }

    // Infrastructure
    if ctx.has_docker || ctx.has_kubernetes || ctx.has_terraform
        || ctx.has_github_actions || ctx.has_gitlab_ci
    {
        println!();
        println!("{}", "Infrastructure:".bold());
        if ctx.has_docker { println!("  {} Docker", "+".green()); }
        if ctx.has_kubernetes { println!("  {} Kubernetes", "+".green()); }
        if ctx.has_terraform { println!("  {} Terraform", "+".green()); }
        if ctx.has_github_actions { println!("  {} GitHub Actions", "+".green()); }
        if ctx.has_gitlab_ci { println!("  {} GitLab CI", "+".green()); }
    }

    // Prompt adjustments
    println!();
    println!("{}", "Prompt Adjustments:".bold());

    let mut has_adjustments = false;
    if ctx.n1_context().is_some() {
        println!("  {} N+1 prompts will note Goldiloader/Bullet usage", "*".yellow());
        has_adjustments = true;
    }
    if ctx.security_context().is_some() {
        println!("  {} Security prompts will note existing security tooling", "*".yellow());
        has_adjustments = true;
    }

    if !has_adjustments {
        println!("  {} No prompt adjustments", "-".dimmed());
    }
}

async fn run_workflow(name: &str, dir: &Path, config: &config::Config) -> Result<()> {
    let path = workflow::find_workflow(name)?;
    let wf = workflow::load_workflow(&path)?;

    let cwd = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let runner = workflow::WorkflowRunner::new(config.clone(), cwd);

    let results = runner.run(&wf).await?;
    workflow::print_results(&results);

    Ok(())
}

fn list_workflows() -> Result<()> {
    let workflows = workflow::list_workflows()?;

    if workflows.is_empty() {
        println!("{}", "No workflows found.".yellow());
        println!();
        println!("Create workflows in:");
        println!("  - .lok/workflows/           (project-local)");
        println!("  - ~/.config/lok/workflows/  (global)");
        return Ok(());
    }

    println!("{}", "Available workflows:".bold());
    println!();

    for (path, wf) in workflows {
        let location = if path.starts_with(".lok") {
            "(local)".dimmed()
        } else {
            "(global)".dimmed()
        };

        println!("  {} {}", wf.name.cyan(), location);
        if let Some(desc) = &wf.description {
            println!("    {}", desc.dimmed());
        }
        println!("    {} steps", wf.steps.len());
        println!();
    }

    Ok(())
}

fn validate_workflow(path: &Path) -> Result<()> {
    let wf = workflow::load_workflow(path)?;

    println!("{} {}", "✓".green(), "Workflow is valid".bold());
    println!();
    println!("  Name: {}", wf.name);
    if let Some(desc) = &wf.description {
        println!("  Description: {}", desc);
    }
    println!("  Steps: {}", wf.steps.len());
    println!();

    for (i, step) in wf.steps.iter().enumerate() {
        println!(
            "  {}. {} (backend: {})",
            i + 1,
            step.name.cyan(),
            step.backend
        );
        if !step.depends_on.is_empty() {
            println!("     depends on: {}", step.depends_on.join(", "));
        }
    }

    Ok(())
}
