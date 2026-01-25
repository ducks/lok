pub mod audit;
pub mod hunt;

use crate::backend;
use crate::config::Config;
use crate::output;
use anyhow::Result;
use std::path::Path;

pub async fn run_task(config: &Config, task_name: &str, dir: &Path) -> Result<()> {
    let task = config
        .tasks
        .get(task_name)
        .ok_or_else(|| anyhow::anyhow!("Task not found: {}", task_name))?;

    output::print_task_header(task_name, task.description.as_deref());

    // Get backends for this task
    let backend_filter = if task.backends.is_empty() || task.backends.contains(&"all".to_string()) {
        None
    } else {
        Some(task.backends.join(","))
    };

    let backends = backend::get_backends(config, backend_filter.as_deref())?;

    // Run each prompt
    for prompt_config in &task.prompts {
        output::print_prompt_header(&prompt_config.name);

        let results = backend::run_query(&backends, &prompt_config.prompt, dir, config).await?;
        output::print_results(&results);
    }

    Ok(())
}
