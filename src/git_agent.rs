//! Git-agent integration for safe edit rollback.
//!
//! When git-agent is available and initialized, lok will create checkpoints
//! before applying edits, enabling automatic rollback on failure.

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
