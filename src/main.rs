mod backend;
mod config;
mod output;
mod tasks;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "council")]
#[command(about = "Multi-LLM orchestration tool for code analysis")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to config file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
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

    /// Initialize a new council.toml config file
    Init,

    /// List available backends
    Backends,
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
            let results = backend::run_query(&backends, &prompt, &dir).await?;
            output::print_results(&results);
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
    }

    Ok(())
}
