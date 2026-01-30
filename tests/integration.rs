//! Integration tests for lok workflow engine
//!
//! These tests use shell-only workflows to verify engine behavior
//! without requiring LLM backends.

use std::process::Command;

fn run_workflow(workflow_path: &str) -> (bool, String) {
    let output = Command::new("cargo")
        .args(["run", "--quiet", "--bin", "lok", "--", "run", workflow_path])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to execute lok");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}\n{}", stdout, stderr);

    (output.status.success(), combined)
}

#[test]
fn test_interpolation_workflow() {
    let (success, output) = run_workflow("tests/workflows/test_interpolation.toml");

    assert!(success, "Workflow failed: {}", output);
    assert!(
        output.contains("hello from step1"),
        "Missing step1 output: {}",
        output
    );
    assert!(
        output.contains("step1 said:"),
        "Missing interpolation: {}",
        output
    );
}

#[test]
fn test_conditionals_workflow() {
    let (success, output) = run_workflow("tests/workflows/test_conditionals.toml");

    assert!(success, "Workflow failed: {}", output);

    // should_run should execute (ISSUES_FOUND matches)
    assert!(
        output.contains("CONDITIONAL_RAN: issues branch"),
        "Should have run issues branch: {}",
        output
    );

    // should_skip should NOT execute (NO_ISSUES doesn't match)
    assert!(
        !output.contains("CONDITIONAL_RAN: clean branch"),
        "Should have skipped clean branch: {}",
        output
    );

    // not() condition should work
    assert!(
        output.contains("NOT_CONDITION_RAN: correct"),
        "not() condition failed: {}",
        output
    );

    // equals() should work
    assert!(
        output.contains("EQUALS_WORKED"),
        "equals() condition failed: {}",
        output
    );
}

#[test]
fn test_retry_workflow() {
    // Clean up any existing counter file first
    let _ = std::fs::remove_file("/tmp/lok_retry_test_counter");

    let (success, output) = run_workflow("tests/workflows/test_retry.toml");

    assert!(success, "Workflow should succeed after retries: {}", output);
    assert!(
        output.contains("SUCCESS on attempt"),
        "Should show success message: {}",
        output
    );
    assert!(
        output.contains("Retry") || output.contains("will retry"),
        "Should show retry attempts: {}",
        output
    );
}

#[test]
fn test_parallel_workflow() {
    let (success, output) = run_workflow("tests/workflows/test_parallel.toml");

    assert!(success, "Workflow failed: {}", output);

    // Lok prints "[parallel] Running N steps in parallel" when parallelizing
    assert!(
        output.contains("[parallel]") || output.contains("Running 3 steps in parallel"),
        "Steps should run in parallel: {}",
        output
    );

    assert!(
        output.contains("A done") && output.contains("B done") && output.contains("C done"),
        "All parallel steps should complete: {}",
        output
    );
}
