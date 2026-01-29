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
mod utils;
mod workflow;

use anyhow::{Context, Result};
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

        /// Create issues for each finding (auto-detects gh or glab)
        #[arg(long)]
        issues: bool,

        /// Issue backend: github, gitlab, or auto (default: auto)
        #[arg(long, default_value = "auto")]
        issue_backend: String,

        /// Skip confirmation prompt when creating issues
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Fix a GitHub issue
    Fix {
        /// Issue number, #number, or full URL
        issue: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Specific backend to use
        #[arg(short, long)]
        backend: Option<String>,

        /// Dry run - analyze but don't suggest applying changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Analyze CI failures for a PR
    Ci {
        /// PR number (e.g., "123")
        pr: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Specific backends to use (comma-separated)
        #[arg(short, long)]
        backend: Option<String>,
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
    #[command(trailing_var_arg = true)]
    Run {
        /// Workflow name or path
        name: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Write full output to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Positional arguments for the workflow (accessible as {{ arg.1 }}, {{ arg.2 }}, etc.)
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Show detected codebase context
    Context {
        /// Directory to analyze
        #[arg(default_value = ".")]
        dir: PathBuf,
    },

    /// Review git changes with LLM analysis
    Diff {
        /// Git diff spec (e.g., "main..HEAD", "HEAD~3"). Default: staged changes
        #[arg(default_value = "")]
        spec: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Specific backends to use (comma-separated)
        #[arg(short, long)]
        backend: Option<String>,

        /// Include unstaged changes (when no spec given)
        #[arg(long)]
        unstaged: bool,
    },

    /// Review a GitHub pull request
    Pr {
        /// PR number or URL (e.g., "123" or "owner/repo#123")
        pr: String,

        /// Repository (owner/repo). Defaults to current repo.
        #[arg(short, long)]
        repo: Option<String>,

        /// Specific backends to use (comma-separated)
        #[arg(short, long)]
        backend: Option<String>,
    },

    /// Explain a codebase structure and architecture
    Explain {
        /// Directory to analyze
        #[arg(default_value = ".")]
        dir: PathBuf,

        /// Specific backends to use (comma-separated)
        #[arg(short, long)]
        backend: Option<String>,

        /// Focus on a specific aspect (e.g., "auth", "database", "api")
        #[arg(short, long)]
        focus: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkflowCommands {
    /// Run a workflow
    #[command(trailing_var_arg = true)]
    Run {
        /// Workflow name or path to .toml file
        name: String,

        /// Working directory
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Write full output to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Positional arguments for the workflow (accessible as {{ arg.1 }}, {{ arg.2 }}, etc.)
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
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
            let cwd = utils::canonicalize_or_warn(&dir);
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
                if let Err(e) = cache.set(&cache_key, &results) {
                    eprintln!("{} Failed to cache results: {}", "warning:".yellow(), e);
                }
            }

            output::print_results(&results);

            if cli.verbose {
                backend::print_verbose_timing(&results);
            }
        }
        Commands::Hunt {
            dir,
            issues,
            issue_backend,
            yes,
        } => {
            tasks::hunt::run(&config, &dir, issues, &issue_backend, yes).await?;
        }
        Commands::Fix {
            issue,
            dir,
            backend,
            dry_run,
        } => {
            tasks::fix::run(&config, &dir, &issue, backend.as_deref(), dry_run).await?;
        }
        Commands::Ci { pr, dir, backend } => {
            tasks::ci::run(&config, &dir, &pr, backend.as_deref()).await?;
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
            WorkflowCommands::Run {
                name,
                dir,
                output,
                args,
            } => {
                run_workflow(&name, &dir, output.as_deref(), args, &config).await?;
            }
            WorkflowCommands::List => {
                list_workflows()?;
            }
            WorkflowCommands::Validate { path } => {
                validate_workflow(&path)?;
            }
        },
        Commands::Run {
            name,
            dir,
            output,
            args,
        } => {
            // Shorthand for 'workflow run'
            run_workflow(&name, &dir, output.as_deref(), args, &config).await?;
        }
        Commands::Context { dir } => {
            show_context(&dir);
        }
        Commands::Diff {
            spec,
            dir,
            backend,
            unstaged,
        } => {
            run_diff(
                &spec,
                &dir,
                backend.as_deref(),
                unstaged,
                &config,
                cli.verbose,
            )
            .await?;
        }
        Commands::Pr { pr, repo, backend } => {
            run_pr_review(
                &pr,
                repo.as_deref(),
                backend.as_deref(),
                &config,
                cli.verbose,
            )
            .await?;
        }
        Commands::Explain {
            dir,
            backend,
            focus,
        } => {
            run_explain(
                &dir,
                backend.as_deref(),
                focus.as_deref(),
                &config,
                cli.verbose,
            )
            .await?;
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
    if ctx.is_rails
        || ctx.has_goldiloader
        || ctx.has_bullet
        || ctx.has_brakeman
        || ctx.has_rubocop
        || ctx.has_strong_migrations
        || ctx.has_rspec
        || ctx.has_sidekiq
        || ctx.has_sorbet
    {
        println!();
        println!("{}", "Ruby/Rails:".bold());
        if ctx.is_rails {
            println!("  {} Rails", "+".green());
        }
        if ctx.has_goldiloader {
            println!("  {} Goldiloader (auto N+1 prevention)", "+".green());
        }
        if ctx.has_bullet {
            println!("  {} Bullet (N+1 detection)", "+".green());
        }
        if ctx.has_brakeman {
            println!("  {} Brakeman (security)", "+".green());
        }
        if ctx.has_rubocop {
            println!("  {} RuboCop (linting)", "+".green());
        }
        if ctx.has_strong_migrations {
            println!("  {} StrongMigrations (safe migrations)", "+".green());
        }
        if ctx.has_rspec {
            println!("  {} RSpec (testing)", "+".green());
        }
        if ctx.has_sidekiq {
            println!("  {} Sidekiq (background jobs)", "+".green());
        }
        if ctx.has_sorbet {
            println!("  {} Sorbet (type checking)", "+".green());
        }
    }

    // JavaScript/TypeScript
    if ctx.has_typescript
        || ctx.has_eslint
        || ctx.has_prettier
        || ctx.has_jest
        || ctx.has_vitest
        || ctx.has_react
        || ctx.has_vue
        || ctx.has_nextjs
        || ctx.has_tailwind
    {
        println!();
        println!("{}", "JavaScript/TypeScript:".bold());
        if ctx.has_typescript {
            println!("  {} TypeScript", "+".green());
        }
        if ctx.has_react {
            println!("  {} React", "+".green());
        }
        if ctx.has_vue {
            println!("  {} Vue", "+".green());
        }
        if ctx.has_nextjs {
            println!("  {} Next.js", "+".green());
        }
        if ctx.has_eslint {
            println!("  {} ESLint (linting)", "+".green());
        }
        if ctx.has_prettier {
            println!("  {} Prettier (formatting)", "+".green());
        }
        if ctx.has_jest {
            println!("  {} Jest (testing)", "+".green());
        }
        if ctx.has_vitest {
            println!("  {} Vitest (testing)", "+".green());
        }
        if ctx.has_tailwind {
            println!("  {} Tailwind CSS", "+".green());
        }
    }

    // Python
    if ctx.is_python
        || ctx.is_django
        || ctx.is_fastapi
        || ctx.has_sqlalchemy
        || ctx.has_pytest
        || ctx.has_mypy
        || ctx.has_ruff
        || ctx.has_alembic
    {
        println!();
        println!("{}", "Python:".bold());
        if ctx.is_django {
            println!("  {} Django", "+".green());
        }
        if ctx.is_fastapi {
            println!("  {} FastAPI", "+".green());
        }
        if ctx.has_sqlalchemy {
            println!("  {} SQLAlchemy", "+".green());
        }
        if ctx.has_alembic {
            println!("  {} Alembic (migrations)", "+".green());
        }
        if ctx.has_pytest {
            println!("  {} pytest (testing)", "+".green());
        }
        if ctx.has_mypy {
            println!("  {} mypy (type checking)", "+".green());
        }
        if ctx.has_ruff {
            println!("  {} Ruff (linting)", "+".green());
        }
    }

    // Rust
    if ctx.is_rust || ctx.has_tokio || ctx.has_diesel || ctx.has_sqlx {
        println!();
        println!("{}", "Rust:".bold());
        if ctx.has_tokio {
            println!("  {} Tokio (async runtime)", "+".green());
        }
        if ctx.has_diesel {
            println!("  {} Diesel (ORM)", "+".green());
        }
        if ctx.has_sqlx {
            println!("  {} SQLx (database)", "+".green());
        }
    }

    // Go
    if ctx.is_go || ctx.has_golangci_lint {
        println!();
        println!("{}", "Go:".bold());
        if ctx.has_golangci_lint {
            println!("  {} golangci-lint", "+".green());
        }
    }

    // Infrastructure
    if ctx.has_docker
        || ctx.has_kubernetes
        || ctx.has_terraform
        || ctx.has_github_actions
        || ctx.has_gitlab_ci
    {
        println!();
        println!("{}", "Infrastructure:".bold());
        if ctx.has_docker {
            println!("  {} Docker", "+".green());
        }
        if ctx.has_kubernetes {
            println!("  {} Kubernetes", "+".green());
        }
        if ctx.has_terraform {
            println!("  {} Terraform", "+".green());
        }
        if ctx.has_github_actions {
            println!("  {} GitHub Actions", "+".green());
        }
        if ctx.has_gitlab_ci {
            println!("  {} GitLab CI", "+".green());
        }
    }

    // Prompt adjustments
    println!();
    println!("{}", "Prompt Adjustments:".bold());

    let mut has_adjustments = false;
    if ctx.n1_context().is_some() {
        println!(
            "  {} N+1 prompts will note Goldiloader/Bullet usage",
            "*".yellow()
        );
        has_adjustments = true;
    }
    if ctx.security_context().is_some() {
        println!(
            "  {} Security prompts will note existing security tooling",
            "*".yellow()
        );
        has_adjustments = true;
    }

    if !has_adjustments {
        println!("  {} No prompt adjustments", "-".dimmed());
    }
}

async fn run_workflow(
    name: &str,
    dir: &Path,
    output: Option<&Path>,
    args: Vec<String>,
    config: &config::Config,
) -> Result<()> {
    let path = workflow::find_workflow(name)?;
    let wf = workflow::load_workflow(&path)?;

    let cwd = utils::canonicalize_or_warn(dir);
    let runner = workflow::WorkflowRunner::new(config.clone(), cwd, args);

    let results = runner.run(&wf).await?;

    if let Some(output_path) = output {
        // Write full results to file
        let output_str = workflow::format_results(&results);
        std::fs::write(output_path, &output_str)
            .with_context(|| format!("Failed to write output to {}", output_path.display()))?;
        println!(
            "{} Results written to {}",
            "✓".green(),
            output_path.display()
        );
    } else {
        workflow::print_results(&results);
    }

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

async fn run_diff(
    spec: &str,
    dir: &Path,
    backend_filter: Option<&str>,
    unstaged: bool,
    config: &config::Config,
    verbose: bool,
) -> Result<()> {
    use std::process::Command;

    println!("{}", "Lok Diff Review".cyan().bold());
    println!("{}", "=".repeat(50).dimmed());

    // Build git diff command
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);
    cmd.arg("diff");

    let diff_description = if spec.is_empty() {
        if unstaged {
            // Show all changes (staged + unstaged)
            "all uncommitted changes"
        } else {
            // Default: staged changes only
            cmd.arg("--cached");
            "staged changes"
        }
    } else {
        // User-provided spec
        cmd.arg(spec);
        spec
    };

    println!("Analyzing: {}", diff_description.yellow());
    println!();

    let output = cmd.output().context("Failed to run git diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {}", stderr);
    }

    let diff = String::from_utf8_lossy(&output.stdout);

    if diff.trim().is_empty() {
        println!("{}", "No changes to review.".yellow());
        println!();
        println!("Tips:");
        println!(
            "  {} - review staged changes (default)",
            "lok diff".dimmed()
        );
        println!(
            "  {} - review all uncommitted changes",
            "lok diff --unstaged".dimmed()
        );
        println!(
            "  {} - review branch vs main",
            "lok diff main..HEAD".dimmed()
        );
        return Ok(());
    }

    // Count lines changed
    let additions = diff.lines().filter(|l| l.starts_with('+')).count();
    let deletions = diff.lines().filter(|l| l.starts_with('-')).count();
    println!(
        "Changes: {} additions, {} deletions",
        format!("+{}", additions).green(),
        format!("-{}", deletions).red()
    );
    println!();

    // Build review prompt
    let prompt = format!(
        r#"Review the following git diff. Look for:
- Bugs or logic errors
- Security issues
- Performance problems
- Code style issues
- Missing error handling
- Suggestions for improvement

Be concise and specific. Reference line numbers when possible.

```diff
{}
```"#,
        diff
    );

    let backends = backend::get_backends(config, backend_filter)?;

    if verbose {
        backend::print_verbose_header(&prompt, &backends, dir);
    }

    let results = backend::run_query(&backends, &prompt, dir, config).await?;
    output::print_results(&results);

    if verbose {
        backend::print_verbose_timing(&results);
    }

    Ok(())
}

async fn run_pr_review(
    pr: &str,
    repo: Option<&str>,
    backend_filter: Option<&str>,
    config: &config::Config,
    verbose: bool,
) -> Result<()> {
    use std::process::Command;

    println!("{}", "Lok PR Review".cyan().bold());
    println!("{}", "=".repeat(50).dimmed());

    // Check if gh CLI is available
    if which::which("gh").is_err() {
        anyhow::bail!(
            "GitHub CLI (gh) is required for PR review.\n\
            Install it from: https://cli.github.com/"
        );
    }

    // Parse PR identifier
    let (owner_repo, pr_number) = parse_pr_identifier(pr, repo)?;

    println!("Repository: {}", owner_repo.as_str().cyan());
    println!("PR: #{}", pr_number.as_str().yellow());
    println!();

    // Get PR details
    println!("{}", "Fetching PR details...".dimmed());
    let pr_json = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number,
            "--repo",
            &owner_repo,
            "--json",
            "title,body,state,additions,deletions,changedFiles,baseRefName,headRefName,author",
        ])
        .output()
        .context("Failed to run gh pr view")?;

    if !pr_json.status.success() {
        let stderr = String::from_utf8_lossy(&pr_json.stderr);
        anyhow::bail!("Failed to fetch PR: {}", stderr);
    }

    let pr_data: serde_json::Value =
        serde_json::from_slice(&pr_json.stdout).context("Failed to parse PR JSON")?;

    let title = pr_data["title"].as_str().unwrap_or("(no title)");
    let body = pr_data["body"].as_str().unwrap_or("(no description)");
    let state = pr_data["state"].as_str().unwrap_or("unknown");
    let additions = pr_data["additions"].as_i64().unwrap_or(0);
    let deletions = pr_data["deletions"].as_i64().unwrap_or(0);
    let changed_files = pr_data["changedFiles"].as_i64().unwrap_or(0);
    let base_ref = pr_data["baseRefName"].as_str().unwrap_or("main");
    let head_ref = pr_data["headRefName"].as_str().unwrap_or("unknown");
    let author = pr_data["author"]["login"].as_str().unwrap_or("unknown");

    println!("Title: {}", title.bold());
    println!("Author: {}", author);
    println!("State: {}", state);
    println!("Branch: {} -> {}", head_ref.cyan(), base_ref.green());
    println!(
        "Changes: {} files, {} {}, {} {}",
        changed_files,
        format!("+{}", additions).green(),
        "additions".dimmed(),
        format!("-{}", deletions).red(),
        "deletions".dimmed()
    );
    println!();

    // Get the diff
    println!("{}", "Fetching PR diff...".dimmed());
    let diff_output = Command::new("gh")
        .args(["pr", "diff", &pr_number, "--repo", &owner_repo])
        .output()
        .context("Failed to run gh pr diff")?;

    if !diff_output.status.success() {
        let stderr = String::from_utf8_lossy(&diff_output.stderr);
        anyhow::bail!("Failed to fetch PR diff: {}", stderr);
    }

    let diff = String::from_utf8_lossy(&diff_output.stdout);

    if diff.trim().is_empty() {
        println!("{}", "PR has no changes to review.".yellow());
        return Ok(());
    }

    // Truncate diff if too large (LLMs have context limits)
    let max_diff_chars = 50000;
    let diff_for_review = if diff.len() > max_diff_chars {
        println!(
            "{}",
            format!(
                "Note: Diff truncated from {} to {} chars",
                diff.len(),
                max_diff_chars
            )
            .yellow()
        );
        &diff[..max_diff_chars]
    } else {
        &diff
    };

    // Build review prompt
    let prompt = format!(
        r#"Review this GitHub Pull Request.

## PR Info
- Title: {title}
- Author: {author}
- Branch: {head_ref} -> {base_ref}
- Changes: {changed_files} files, +{additions}/-{deletions} lines

## Description
{body}

## Diff
```diff
{diff_for_review}
```

## Review Instructions
Provide a thorough code review. Look for:
1. Bugs or logic errors
2. Security vulnerabilities
3. Performance issues
4. Code style and best practices
5. Missing tests or documentation
6. Potential edge cases

Be specific and reference file names and line numbers when possible.
Organize your review by severity (critical, important, minor, nitpick)."#
    );

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let backends = backend::get_backends(config, backend_filter)?;

    if verbose {
        backend::print_verbose_header(&prompt, &backends, &cwd);
    }

    let results = backend::run_query(&backends, &prompt, &cwd, config).await?;
    output::print_results(&results);

    if verbose {
        backend::print_verbose_timing(&results);
    }

    Ok(())
}

async fn run_explain(
    dir: &Path,
    backend_filter: Option<&str>,
    focus: Option<&str>,
    config: &config::Config,
    verbose: bool,
) -> Result<()> {
    use std::fs;

    println!("{}", "Lok Explain".cyan().bold());
    println!("{}", "=".repeat(50).dimmed());

    let cwd = utils::canonicalize_or_warn(dir);
    println!("Analyzing: {}", cwd.display().to_string().yellow());
    println!();

    // Gather codebase information
    let mut info = String::new();

    // Check for README
    let readme_variants = ["README.md", "README", "readme.md", "README.txt"];
    for readme in readme_variants {
        let path = cwd.join(readme);
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                let truncated = if content.len() > 3000 {
                    format!("{}...\n[truncated]", &content[..3000])
                } else {
                    content
                };
                info.push_str(&format!("=== {} ===\n{}\n\n", readme, truncated));
            }
            break;
        }
    }

    // Check for package manifests
    let manifests = [
        ("Cargo.toml", "Rust"),
        ("package.json", "Node.js"),
        ("pyproject.toml", "Python"),
        ("go.mod", "Go"),
        ("Gemfile", "Ruby"),
        ("pom.xml", "Java/Maven"),
        ("build.gradle", "Java/Gradle"),
    ];

    for (manifest, lang) in manifests {
        let path = cwd.join(manifest);
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                let truncated = if content.len() > 2000 {
                    format!("{}...\n[truncated]", &content[..2000])
                } else {
                    content
                };
                info.push_str(&format!(
                    "=== {} ({}) ===\n{}\n\n",
                    manifest, lang, truncated
                ));
            }
        }
    }

    // Build directory tree (top 2 levels)
    info.push_str("=== Directory Structure ===\n");
    if let Ok(entries) = fs::read_dir(&cwd) {
        let mut dirs: Vec<String> = Vec::new();
        let mut files: Vec<String> = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden files and common noise
            if name.starts_with('.')
                || name == "node_modules"
                || name == "target"
                || name == "vendor"
            {
                continue;
            }

            if entry.path().is_dir() {
                // List contents of subdirectory
                let mut subentries = Vec::new();
                if let Ok(subdir) = fs::read_dir(entry.path()) {
                    for subentry in subdir.flatten().take(10) {
                        let subname = subentry.file_name().to_string_lossy().to_string();
                        if !subname.starts_with('.') {
                            let suffix = if subentry.path().is_dir() { "/" } else { "" };
                            subentries.push(format!("{}{}", subname, suffix));
                        }
                    }
                }

                if subentries.is_empty() {
                    dirs.push(format!("{}/", name));
                } else {
                    dirs.push(format!("{}/\n    {}", name, subentries.join(", ")));
                }
            } else {
                files.push(name);
            }
        }

        dirs.sort();
        files.sort();

        for d in dirs {
            info.push_str(&format!("  {}\n", d));
        }
        for f in files {
            info.push_str(&format!("  {}\n", f));
        }
    }
    info.push('\n');

    // Detect context
    let ctx = context::CodebaseContext::detect(&cwd);
    if ctx.detected_language.is_some() || ctx.is_rails || ctx.has_docker {
        info.push_str("=== Detected Stack ===\n");
        if let Some(lang) = &ctx.detected_language {
            info.push_str(&format!("Language: {}\n", lang));
        }
        if ctx.is_rails {
            info.push_str("Framework: Rails\n");
        }
        if ctx.has_react {
            info.push_str("Frontend: React\n");
        }
        if ctx.has_vue {
            info.push_str("Frontend: Vue\n");
        }
        if ctx.has_nextjs {
            info.push_str("Framework: Next.js\n");
        }
        if ctx.is_django {
            info.push_str("Framework: Django\n");
        }
        if ctx.is_fastapi {
            info.push_str("Framework: FastAPI\n");
        }
        if ctx.has_docker {
            info.push_str("Infrastructure: Docker\n");
        }
        if ctx.has_kubernetes {
            info.push_str("Infrastructure: Kubernetes\n");
        }
        if ctx.has_terraform {
            info.push_str("Infrastructure: Terraform\n");
        }
        info.push('\n');
    }

    // Build the prompt
    let focus_instruction = match focus {
        Some(f) => format!(
            "\n\nFocus specifically on: {}. Explain how {} is handled in this codebase.",
            f, f
        ),
        None => String::new(),
    };

    let prompt = format!(
        r#"Explain the structure and architecture of this codebase. Include:

1. **Purpose**: What does this project do? What problem does it solve?
2. **Architecture**: How is the code organized? What are the main components?
3. **Key Files**: Which files are most important for understanding the codebase?
4. **Entry Points**: Where does execution start? How do you run/use it?
5. **Dependencies**: What external libraries/services does it rely on?

Be concise but thorough. A developer new to this codebase should understand the big picture after reading your explanation.{}

{}
"#,
        focus_instruction, info
    );

    let backends = backend::get_backends(config, backend_filter)?;

    if verbose {
        backend::print_verbose_header(&prompt, &backends, dir);
    }

    let results = backend::run_query(&backends, &prompt, dir, config).await?;
    output::print_results(&results);

    if verbose {
        backend::print_verbose_timing(&results);
    }

    Ok(())
}

/// Parse PR identifier into (owner/repo, pr_number)
fn parse_pr_identifier(pr: &str, repo: Option<&str>) -> Result<(String, String)> {
    use std::process::Command;

    // Handle "owner/repo#123" format
    if pr.contains('#') {
        let parts: Vec<&str> = pr.splitn(2, '#').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Handle URL format
    if pr.starts_with("http://") || pr.starts_with("https://") {
        // Strip query params and fragments
        let url_clean = pr.split(['?', '#']).next().unwrap_or(pr);

        // Split and filter empty segments (handles trailing slashes)
        let parts: Vec<&str> = url_clean.split('/').filter(|s| !s.is_empty()).collect();

        // Validate host - exact match for security (no .contains() which allows github.com.evil.com)
        let host = parts.get(1).copied().unwrap_or("");
        let is_github = host == "github.com";
        let is_gitlab = host == "gitlab.com" || host.contains("gitlab.");

        if !is_github && !is_gitlab {
            return Err(anyhow::anyhow!(
                "Invalid PR URL host '{}'. Expected github.com or gitlab.com",
                host
            ));
        }

        // GitHub: https://github.com/owner/repo/pull/123[/files]
        // Parts after filter: ["https:", "github.com", "owner", "repo", "pull", "123"]
        if is_github {
            if let Some(pull_pos) = parts.iter().position(|&s| s == "pull" || s == "pulls") {
                if pull_pos >= 4 && pull_pos + 1 < parts.len() {
                    let owner = parts[pull_pos - 2];
                    let repo = parts[pull_pos - 1];
                    let number_str = parts[pull_pos + 1];

                    // Validate PR number is numeric
                    if number_str.parse::<u64>().is_err() {
                        return Err(anyhow::anyhow!(
                            "Invalid PR number: '{}' is not a valid number",
                            number_str
                        ));
                    }

                    return Ok((format!("{}/{}", owner, repo), number_str.to_string()));
                }
            }
        }

        // GitLab: https://gitlab.com/owner/repo/-/merge_requests/123[/diffs]
        // Parts after filter: ["https:", "gitlab.com", "owner", "repo", "-", "merge_requests", "123"]
        if is_gitlab {
            if let Some(mr_pos) = parts.iter().position(|&s| s == "merge_requests") {
                if mr_pos >= 5 && mr_pos + 1 < parts.len() {
                    let owner = parts[mr_pos - 3];
                    let repo = parts[mr_pos - 2];
                    let number_str = parts[mr_pos + 1];

                    // Validate MR number is numeric
                    if number_str.parse::<u64>().is_err() {
                        return Err(anyhow::anyhow!(
                            "Invalid MR number: '{}' is not a valid number",
                            number_str
                        ));
                    }

                    return Ok((format!("{}/{}", owner, repo), number_str.to_string()));
                }
            }
        }

        // Invalid PR URL format - return error with both formats
        return Err(anyhow::anyhow!(
            "Invalid PR URL format. Expected:\n  GitHub: https://github.com/owner/repo/pull/123\n  GitLab: https://gitlab.com/owner/repo/-/merge_requests/123"
        ));
    }

    // If repo is provided, use it
    if let Some(r) = repo {
        return Ok((r.to_string(), pr.to_string()));
    }

    // Try to get repo from current directory
    let output = Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let repo_name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !repo_name.is_empty() {
                return Ok((repo_name, pr.to_string()));
            }
        }
        _ => {}
    }

    anyhow::bail!(
        "Could not determine repository. Use --repo owner/repo or run from within a git repo."
    )
}
