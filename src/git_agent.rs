//! Git-agent integration for safe edit rollback.
//!
//! When git-agent is available and initialized, lok will create checkpoints
//! before applying edits, enabling automatic rollback on failure.
//!
//! The agent history is stored on an orphan branch `agent-history` mounted
//! as a worktree at `.agent/`. This keeps agent reasoning history separate
//! from main code history while using git's native storage.

use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;
use tokio::process::Command;

/// Check if git-agent is installed and available.
pub async fn is_available() -> bool {
    Command::new("git-agent")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if git-agent is initialized in the given directory.
pub async fn is_initialized(cwd: &Path) -> bool {
    cwd.join(".agent").is_dir()
}

/// Check if there's an active git-agent session.
pub async fn has_active_session(cwd: &Path) -> bool {
    let current_file = cwd.join(".agent/current");
    if !current_file.exists() {
        return false;
    }
    std::fs::read_to_string(&current_file)
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Create a checkpoint with the given message.
/// Returns Ok(true) if checkpoint was created, Ok(false) if git-agent not ready.
pub async fn checkpoint(cwd: &Path, message: &str) -> Result<bool, String> {
    if !is_available().await {
        return Ok(false);
    }
    if !is_initialized(cwd).await {
        return Ok(false);
    }
    if !has_active_session(cwd).await {
        return Ok(false);
    }

    let output = Command::new("git-agent")
        .args(["checkpoint", "-m", message])
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| format!("Failed to run git-agent checkpoint: {}", e))?;

    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git-agent checkpoint failed: {}", stderr))
    }
}

/// Undo to the previous checkpoint.
/// Returns Ok(true) if undo was successful, Ok(false) if git-agent not ready.
pub async fn undo(cwd: &Path) -> Result<bool, String> {
    if !is_available().await {
        return Ok(false);
    }
    if !is_initialized(cwd).await {
        return Ok(false);
    }

    let output = Command::new("git-agent")
        .args(["undo", "1"])
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| format!("Failed to run git-agent undo: {}", e))?;

    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git-agent undo failed: {}", stderr))
    }
}

const AGENT_BRANCH: &str = "agent-history";
const AGENT_DIR: &str = ".agent";

/// Initialize git-agent with an orphan branch and worktree.
///
/// Creates an orphan branch `agent-history` (no shared history with main)
/// and mounts it as a worktree at `.agent/`. Agent checkpoints will be
/// stored as real git commits on this branch.
pub async fn init_worktree(cwd: &Path) -> Result<()> {
    let agent_path = cwd.join(AGENT_DIR);

    // Check if already initialized
    if agent_path.exists() {
        println!(
            "{} Agent worktree already exists at {}",
            "✓".green(),
            AGENT_DIR
        );
        return Ok(());
    }

    // Check if we're in a git repo
    let status = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(cwd)
        .output()
        .await?;

    if !status.status.success() {
        return Err(anyhow!("Not a git repository. Run 'git init' first."));
    }

    println!("{}", "Initializing git-agent worktree...".cyan());

    // Check if orphan branch already exists
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", AGENT_BRANCH])
        .current_dir(cwd)
        .output()
        .await?;

    if !branch_check.status.success() {
        // Create orphan branch
        println!("  Creating orphan branch '{}'...", AGENT_BRANCH);

        // Save current branch
        let current_branch = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(cwd)
            .output()
            .await?;
        let current_branch = String::from_utf8_lossy(&current_branch.stdout)
            .trim()
            .to_string();

        // Create orphan branch with initial commit
        let checkout = Command::new("git")
            .args(["checkout", "--orphan", AGENT_BRANCH])
            .current_dir(cwd)
            .output()
            .await?;

        if !checkout.status.success() {
            return Err(anyhow!(
                "Failed to create orphan branch: {}",
                String::from_utf8_lossy(&checkout.stderr)
            ));
        }

        // Remove all files from index (orphan branch starts with staged files)
        let _ = Command::new("git")
            .args(["rm", "-rf", "--cached", "."])
            .current_dir(cwd)
            .output()
            .await;

        // Create initial commit
        let commit = Command::new("git")
            .args(["commit", "--allow-empty", "-m", "Initialize agent history"])
            .current_dir(cwd)
            .output()
            .await?;

        if !commit.status.success() {
            // Try to recover by going back to original branch
            let _ = Command::new("git")
                .args(["checkout", &current_branch])
                .current_dir(cwd)
                .output()
                .await;
            return Err(anyhow!(
                "Failed to create initial commit: {}",
                String::from_utf8_lossy(&commit.stderr)
            ));
        }

        // Switch back to original branch
        let switch_back = Command::new("git")
            .args(["checkout", &current_branch])
            .current_dir(cwd)
            .output()
            .await?;

        if !switch_back.status.success() {
            return Err(anyhow!(
                "Failed to switch back to {}: {}",
                current_branch,
                String::from_utf8_lossy(&switch_back.stderr)
            ));
        }

        println!("  {} Created orphan branch '{}'", "✓".green(), AGENT_BRANCH);
    } else {
        println!(
            "  {} Orphan branch '{}' already exists",
            "✓".green(),
            AGENT_BRANCH
        );
    }

    // Add worktree
    println!("  Adding worktree at '{}'...", AGENT_DIR);

    let worktree = Command::new("git")
        .args(["worktree", "add", AGENT_DIR, AGENT_BRANCH])
        .current_dir(cwd)
        .output()
        .await?;

    if !worktree.status.success() {
        return Err(anyhow!(
            "Failed to add worktree: {}",
            String::from_utf8_lossy(&worktree.stderr)
        ));
    }

    println!("  {} Added worktree at '{}'", "✓".green(), AGENT_DIR);

    // Create initial structure in worktree
    let sessions_dir = agent_path.join("sessions");
    std::fs::create_dir_all(&sessions_dir)?;

    // Create README in agent worktree
    let readme_content = r#"# Agent History

This branch contains the decision history for AI agent sessions.

Each session is stored as a series of commits capturing:
- Intent: What the agent was trying to accomplish
- Checkpoints: Snapshots of decisions made
- Reasoning: Why each decision was made

This history is separate from the main code history but linked
via commit references.

## Structure

- `sessions/` - Session metadata and intent records
- Commits on this branch represent checkpoints

## Usage

This branch is managed by `lok` and should not be edited manually.
Use `lok report` to generate human-readable summaries.
"#;

    std::fs::write(agent_path.join("README.md"), readme_content)?;

    // Commit the initial structure
    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(&agent_path)
        .output()
        .await?;

    if add.status.success() {
        let _ = Command::new("git")
            .args(["commit", "-m", "Add initial structure"])
            .current_dir(&agent_path)
            .output()
            .await;
    }

    println!();
    println!("{} Git-agent initialized!", "✓".green().bold());
    println!();
    println!(
        "Agent history will be tracked on the '{}' branch.",
        AGENT_BRANCH
    );
    println!("Worktree mounted at '{}'.", AGENT_DIR);
    println!();
    println!("Next steps:");
    println!("  • Run workflows with `lok run <workflow>`");
    println!("  • Checkpoints will be created automatically");
    println!("  • Generate reports with `lok report`");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_is_available_returns_bool() {
        // Just verify it doesn't panic and returns a bool
        let _ = is_available().await;
    }

    #[tokio::test]
    async fn test_is_initialized_false_for_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_initialized(tmp.path()).await);
    }
}
