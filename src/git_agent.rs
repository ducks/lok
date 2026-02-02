//! Git-agent integration for safe edit rollback.
//!
//! When git-agent is available and initialized, lok will create checkpoints
//! before applying edits, enabling automatic rollback on failure.
//!
//! The agent history is stored on an orphan branch `agent-history` mounted
//! as a worktree at `.agent/`. This keeps agent reasoning history separate
//! from main code history while using git's native storage.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

/// Structured agent event for checkpoints.
///
/// Based on the agent-events spec. Every checkpoint captures:
/// - what: concrete action being taken (required)
/// - why: reasoning behind the approach (required)
/// - how: implementation details (optional)
/// - backup: rollback plan if it fails (optional)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AgentEvent {
    /// Concrete action being taken (required)
    /// Example: "Switch from eager loading to batch loading"
    pub what: String,

    /// Reasoning behind the approach (required)
    /// Example: "Eager loading broke pagination due to limit clause"
    pub why: String,

    /// Implementation details (optional)
    /// Example: "Using includes() instead of joins()"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,

    /// Rollback plan if it fails (optional)
    /// Example: "Revert to checkpoint 2"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,

    /// Timestamp of the event
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,

    /// Link to code commit SHA this event relates to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_commit: Option<String>,

    /// Session ID for grouping related events
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Outcome of the action (set after execution)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<EventOutcome>,
}

/// Outcome of an agent event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum EventOutcome {
    Success,
    Failure { reason: String },
    Partial { details: String },
}

#[allow(dead_code)]
impl AgentEvent {
    /// Create a new agent event with required fields
    pub fn new(what: impl Into<String>, why: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            why: why.into(),
            how: None,
            backup: None,
            timestamp: Utc::now(),
            code_commit: None,
            session_id: None,
            outcome: None,
        }
    }

    /// Add implementation details
    pub fn with_how(mut self, how: impl Into<String>) -> Self {
        self.how = Some(how.into());
        self
    }

    /// Add rollback plan
    pub fn with_backup(mut self, backup: impl Into<String>) -> Self {
        self.backup = Some(backup.into());
        self
    }

    /// Link to a code commit
    pub fn with_code_commit(mut self, sha: impl Into<String>) -> Self {
        self.code_commit = Some(sha.into());
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Mark as successful
    pub fn success(mut self) -> Self {
        self.outcome = Some(EventOutcome::Success);
        self
    }

    /// Mark as failed
    pub fn failure(mut self, reason: impl Into<String>) -> Self {
        self.outcome = Some(EventOutcome::Failure {
            reason: reason.into(),
        });
        self
    }

    /// Format as a git commit message
    pub fn to_commit_message(&self) -> String {
        let mut msg = format!("{}\n\nWhy: {}", self.what, self.why);

        if let Some(ref how) = self.how {
            msg.push_str(&format!("\n\nHow: {}", how));
        }

        if let Some(ref backup) = self.backup {
            msg.push_str(&format!("\n\nBackup: {}", backup));
        }

        if let Some(ref outcome) = self.outcome {
            let outcome_str = match outcome {
                EventOutcome::Success => "success".to_string(),
                EventOutcome::Failure { reason } => format!("failure: {}", reason),
                EventOutcome::Partial { details } => format!("partial: {}", details),
            };
            msg.push_str(&format!("\n\nOutcome: {}", outcome_str));
        }

        if let Some(ref sha) = self.code_commit {
            msg.push_str(&format!("\n\nCode-Commit: {}", sha));
        }

        msg
    }
}

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

/// Check if the agent worktree is initialized
#[allow(dead_code)]
pub fn has_agent_worktree(cwd: &Path) -> bool {
    let agent_path = cwd.join(AGENT_DIR);
    // Check for .git file (worktree marker) not .git directory
    agent_path.join(".git").exists()
}

/// Create a checkpoint as a git commit on the agent-history branch.
///
/// Takes a structured AgentEvent and commits it to the agent branch.
/// Also writes the event as JSON to sessions/ for queryability.
#[allow(dead_code)]
pub async fn checkpoint_event(cwd: &Path, event: &AgentEvent) -> Result<String> {
    let agent_path = cwd.join(AGENT_DIR);

    if !has_agent_worktree(cwd) {
        return Err(anyhow!(
            "Agent worktree not initialized. Run 'lok init --agent' first."
        ));
    }

    // Generate session ID if not set
    let session_id = event
        .session_id
        .clone()
        .unwrap_or_else(|| format!("{}", event.timestamp.format("%Y%m%d-%H%M%S")));

    // Ensure session directory exists
    let session_dir = agent_path.join("sessions").join(&session_id);
    std::fs::create_dir_all(&session_dir)?;

    // Write event as JSON for queryability
    let event_file = session_dir.join(format!("{}.json", event.timestamp.timestamp()));
    let event_json = serde_json::to_string_pretty(event)?;
    std::fs::write(&event_file, &event_json)?;

    // Stage the event file
    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(&agent_path)
        .output()
        .await?;

    if !add.status.success() {
        return Err(anyhow!(
            "Failed to stage event: {}",
            String::from_utf8_lossy(&add.stderr)
        ));
    }

    // Commit with structured message
    let commit_msg = event.to_commit_message();
    let commit = Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .current_dir(&agent_path)
        .output()
        .await?;

    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        // No changes to commit is ok
        if stderr.contains("nothing to commit") {
            return Ok("no-change".to_string());
        }
        return Err(anyhow!("Failed to commit event: {}", stderr));
    }

    // Get the commit SHA
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&agent_path)
        .output()
        .await?;

    let sha = String::from_utf8_lossy(&sha.stdout).trim().to_string();

    Ok(sha)
}

/// Get the current HEAD commit SHA from the main repo (for linking)
#[allow(dead_code)]
pub async fn get_code_head(cwd: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow!("Failed to get HEAD: not a git repository?"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

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
