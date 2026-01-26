pub mod audit;
pub mod hunt;

use crate::backend;
use crate::config::Config;
use crate::context::CodebaseContext;
use crate::output;
use anyhow::Result;
use std::path::Path;

pub async fn run_task(config: &Config, task_name: &str, dir: &Path) -> Result<()> {
    let task = config
        .tasks
        .get(task_name)
        .ok_or_else(|| anyhow::anyhow!("Task not found: {}", task_name))?;

    output::print_task_header(task_name, task.description.as_deref());

    // Detect codebase context
    let context = CodebaseContext::detect(dir);

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

        // Prepend relevant context based on prompt type
        let prompt_with_context = prepend_context(&prompt_config.prompt, &prompt_config.name, task_name, &context);

        let results = backend::run_query(&backends, &prompt_with_context, dir, config).await?;
        output::print_results(&results);
    }

    Ok(())
}

/// Prepend relevant codebase context to a prompt based on its type
fn prepend_context(prompt: &str, prompt_name: &str, task_name: &str, context: &CodebaseContext) -> String {
    let name_lower = prompt_name.to_lowercase();
    let task_lower = task_name.to_lowercase();

    // N+1 related prompts
    if name_lower.contains("n+1") || name_lower.contains("n-plus") || name_lower.contains("query") {
        if let Some(ctx) = context.n1_context() {
            return format!("{}{}", ctx, prompt);
        }
    }

    // Security related prompts
    if task_lower.contains("audit") || task_lower.contains("security")
        || name_lower.contains("injection") || name_lower.contains("xss")
        || name_lower.contains("auth") || name_lower.contains("security")
    {
        if let Some(ctx) = context.security_context() {
            return format!("{}{}", ctx, prompt);
        }
    }

    prompt.to_string()
}
