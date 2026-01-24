use crate::backend::QueryResult;
use colored::Colorize;

pub fn print_results(results: &[QueryResult]) {
    for result in results {
        println!();
        let header = format!("=== {} ===", result.backend.to_uppercase());

        if result.success {
            println!("{}", header.green().bold());
        } else {
            println!("{}", header.red().bold());
        }

        println!();
        println!("{}", result.output);
    }

    println!();
}

pub fn print_task_header(task_name: &str, description: Option<&str>) {
    println!();
    println!("{}", format!("Task: {}", task_name).cyan().bold());
    if let Some(desc) = description {
        println!("{}", desc.dimmed());
    }
    println!("{}", "=".repeat(50).dimmed());
}

pub fn print_prompt_header(prompt_name: &str) {
    println!();
    println!("{}", format!("[{}]", prompt_name).yellow().bold());
}
