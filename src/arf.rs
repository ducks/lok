//! ARF (Agent Reasoning Format) record support
//!
//! Emits structured reasoning records at checkpoint granularity
//! so execution can be traced on a graph.
//!
//! Records are stored in a git worktree on an orphan branch `arf-history`,
//! keeping agent reasoning history separate from main code commits.

use anyhow::{anyhow, Result};
use chrono::Utc;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// ARF record - captures reasoning at a checkpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArfRecord {
    pub what: String,
    pub why: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub how: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ArfContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Failure,
    Partial,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArfContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backends_queried: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backends_succeeded: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backends_failed: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Link to the main repo's HEAD commit SHA at this checkpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_commit: Option<String>,
    /// Session ID for grouping related records
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl ArfContext {
    /// Create a new context with timestamp set to now
    pub fn now() -> Self {
        Self {
            timestamp: Some(Utc::now().to_rfc3339()),
            ..Default::default()
        }
    }
}

/// ARF recorder - writes records to .arf/records/
pub struct ArfRecorder {
    enabled: bool,
    cwd: PathBuf,
    base_path: PathBuf,
    session_id: String,
    record_count: u32,
    /// Cached code commit SHA from session start
    code_commit: Option<String>,
}

impl ArfRecorder {
    /// Create a new recorder for the given working directory
    pub fn new(cwd: &std::path::Path) -> Self {
        let arf_dir = cwd.join(".arf");
        let enabled = arf_dir.exists();
        let session_id = Utc::now().format("%Y%m%d-%H%M%S").to_string();

        Self {
            enabled,
            cwd: cwd.to_path_buf(),
            base_path: arf_dir.join("records").join(&session_id),
            session_id,
            record_count: 0,
            code_commit: None,
        }
    }

    /// Set the code commit SHA for this session (call once at start)
    pub fn set_code_commit(&mut self, sha: String) {
        self.code_commit = Some(sha);
    }

    /// Get the working directory
    #[allow(dead_code)] // Public API for external consumers
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Check if ARF recording is enabled
    #[allow(dead_code)] // Public API for external consumers
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the session ID
    #[allow(dead_code)] // Used by tests and future API consumers
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Record a checkpoint
    pub fn record(&mut self, mut record: ArfRecord) -> std::io::Result<String> {
        if !self.enabled {
            return Ok(String::new());
        }

        // Inject code_commit and session_id into context
        if let Some(ref mut ctx) = record.context {
            if ctx.code_commit.is_none() {
                ctx.code_commit = self.code_commit.clone();
            }
            if ctx.session_id.is_none() {
                ctx.session_id = Some(self.session_id.clone());
            }
        }

        // Ensure directory exists
        fs::create_dir_all(&self.base_path)?;

        // Generate record ID
        self.record_count += 1;
        let record_id = format!("{:04}", self.record_count);
        let filename = format!("{}.arf", record_id);
        let filepath = self.base_path.join(&filename);

        // Write TOML
        let toml = toml::to_string_pretty(&record).map_err(std::io::Error::other)?;

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&filepath)?;

        file.write_all(toml.as_bytes())?;

        Ok(record_id)
    }

    /// Commit all pending records to the ARF worktree
    #[allow(dead_code)] // Public API for external consumers
    pub async fn commit(&self, message: &str) -> Result<String> {
        if !self.enabled {
            return Ok(String::new());
        }
        commit_records(&self.cwd, message).await
    }

    /// Record workflow start
    pub fn workflow_start(
        &mut self,
        name: &str,
        description: Option<&str>,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Start workflow: {}", name),
            why: description.unwrap_or("User invoked workflow").to_string(),
            how: None,
            backup: None,
            outcome: None,
            context: Some(ArfContext {
                workflow: Some(name.to_string()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record workflow complete
    pub fn workflow_complete(
        &mut self,
        name: &str,
        success: bool,
        elapsed_ms: u64,
        steps_succeeded: usize,
        steps_failed: usize,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Complete workflow: {}", name),
            why: if success {
                format!("{} steps succeeded", steps_succeeded)
            } else {
                format!("{} steps failed", steps_failed)
            },
            how: None,
            backup: None,
            outcome: Some(if success {
                Outcome::Success
            } else if steps_succeeded > 0 {
                Outcome::Partial
            } else {
                Outcome::Failure
            }),
            context: Some(ArfContext {
                workflow: Some(name.to_string()),
                elapsed_ms: Some(elapsed_ms),
                ..ArfContext::now()
            }),
        })
    }

    /// Record step start
    pub fn step_start(
        &mut self,
        workflow: &str,
        step: &str,
        backend: Option<&str>,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Start step: {}", step),
            why: backend
                .map(|b| format!("Query {} backend", b))
                .unwrap_or_else(|| "Execute shell command".to_string()),
            how: None,
            backup: None,
            outcome: None,
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                backend: backend.map(|s| s.to_string()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record backend query
    pub fn backend_query(
        &mut self,
        workflow: &str,
        step: &str,
        backend: &str,
        prompt_preview: &str,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Query backend: {}", backend),
            why: format!("Step '{}' requires LLM analysis", step),
            how: Some(format!(
                "Prompt: {}...",
                &prompt_preview.chars().take(100).collect::<String>()
            )),
            backup: Some("Retry with exponential backoff on transient failure".to_string()),
            outcome: None,
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                backend: Some(backend.to_string()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record backend response
    pub fn backend_response(
        &mut self,
        workflow: &str,
        step: &str,
        backend: &str,
        success: bool,
        elapsed_ms: u64,
        error: Option<&str>,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!(
                "Backend {} {}",
                backend,
                if success { "responded" } else { "failed" }
            ),
            why: if success {
                format!("Received response in {}ms", elapsed_ms)
            } else {
                error.unwrap_or("Unknown error").to_string()
            },
            how: None,
            backup: None,
            outcome: Some(if success {
                Outcome::Success
            } else {
                Outcome::Failure
            }),
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                backend: Some(backend.to_string()),
                elapsed_ms: Some(elapsed_ms),
                error: error.map(|s| s.to_string()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record retry attempt
    pub fn retry_attempt(
        &mut self,
        workflow: &str,
        step: &str,
        backend: &str,
        attempt: u32,
        reason: &str,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Retry {} (attempt {})", backend, attempt),
            why: reason.to_string(),
            how: Some("Exponential backoff before next attempt".to_string()),
            backup: Some("Fail step if max retries exceeded".to_string()),
            outcome: None,
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                backend: Some(backend.to_string()),
                retry_attempt: Some(attempt),
                ..ArfContext::now()
            }),
        })
    }

    /// Record synthesis decision
    #[allow(dead_code)] // Public API for future multi-backend synthesis
    pub fn synthesis(
        &mut self,
        workflow: &str,
        step: &str,
        backends_succeeded: &[String],
        backends_failed: &[String],
        decision: &str,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: "Synthesize backend responses".to_string(),
            why: format!(
                "{}/{} backends succeeded",
                backends_succeeded.len(),
                backends_succeeded.len() + backends_failed.len()
            ),
            how: Some(decision.to_string()),
            backup: None,
            outcome: Some(if backends_failed.is_empty() {
                Outcome::Success
            } else {
                Outcome::Partial
            }),
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                backends_succeeded: Some(backends_succeeded.to_vec()),
                backends_failed: Some(backends_failed.to_vec()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record edit application
    pub fn edit_apply(
        &mut self,
        workflow: &str,
        step: &str,
        file: &str,
        success: bool,
        error: Option<&str>,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Apply edit to {}", file),
            why: "LLM proposed code change".to_string(),
            how: None,
            backup: Some("Rollback via git if verification fails".to_string()),
            outcome: Some(if success {
                Outcome::Success
            } else {
                Outcome::Failure
            }),
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                file: Some(file.to_string()),
                error: error.map(|s| s.to_string()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record verification result
    pub fn verification(
        &mut self,
        workflow: &str,
        step: &str,
        command: &str,
        success: bool,
        error: Option<&str>,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Verify: {}", command),
            why: "Validate applied changes".to_string(),
            how: None,
            backup: if success {
                None
            } else {
                Some("Rollback changes".to_string())
            },
            outcome: Some(if success {
                Outcome::Success
            } else {
                Outcome::Failure
            }),
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                error: error.map(|s| s.to_string()),
                ..ArfContext::now()
            }),
        })
    }

    /// Record step complete
    pub fn step_complete(
        &mut self,
        workflow: &str,
        step: &str,
        success: bool,
        elapsed_ms: u64,
        error: Option<&str>,
    ) -> std::io::Result<String> {
        self.record(ArfRecord {
            what: format!("Complete step: {}", step),
            why: if success {
                format!("Step succeeded in {}ms", elapsed_ms)
            } else {
                error.unwrap_or("Step failed").to_string()
            },
            how: None,
            backup: None,
            outcome: Some(if success {
                Outcome::Success
            } else {
                Outcome::Failure
            }),
            context: Some(ArfContext {
                workflow: Some(workflow.to_string()),
                step: Some(step.to_string()),
                elapsed_ms: Some(elapsed_ms),
                error: error.map(|s| s.to_string()),
                ..ArfContext::now()
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_arf_recording() {
        let dir = tempdir().unwrap();
        let arf_dir = dir.path().join(".arf");
        fs::create_dir_all(&arf_dir).unwrap();

        let mut recorder = ArfRecorder::new(dir.path());
        assert!(recorder.is_enabled());

        let id = recorder
            .workflow_start("test-workflow", Some("Test workflow"))
            .unwrap();
        assert_eq!(id, "0001");

        let id = recorder
            .step_start("test-workflow", "step1", Some("claude"))
            .unwrap();
        assert_eq!(id, "0002");

        // Check files were created
        let records_dir = arf_dir.join("records").join(recorder.session_id());
        assert!(records_dir.join("0001.arf").exists());
        assert!(records_dir.join("0002.arf").exists());
    }

    #[test]
    fn test_arf_disabled_without_dir() {
        let dir = tempdir().unwrap();
        // No .arf directory

        let mut recorder = ArfRecorder::new(dir.path());
        assert!(!recorder.is_enabled());

        // Should return empty string, not error
        let id = recorder.workflow_start("test", None).unwrap();
        assert!(id.is_empty());
    }
}

// =============================================================================
// Git Worktree Storage
// =============================================================================
//
// ARF records are stored on an orphan branch `arf-history` mounted as a worktree
// at `.arf/`. This keeps reasoning history in git but separate from code commits.

const ARF_BRANCH: &str = "arf-history";
const ARF_DIR: &str = ".arf";

/// Check if the ARF worktree is initialized
pub fn has_arf_worktree(cwd: &Path) -> bool {
    let arf_path = cwd.join(ARF_DIR);
    // Check for .git file (worktree marker) not .git directory
    arf_path.join(".git").exists()
}

/// Get the current HEAD commit SHA from the main repo (for linking records to code)
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

/// Commit pending ARF records to the worktree
pub async fn commit_records(cwd: &Path, message: &str) -> Result<String> {
    let arf_path = cwd.join(ARF_DIR);

    if !has_arf_worktree(cwd) {
        return Err(anyhow!(
            "ARF worktree not initialized. Run 'lok init --arf' first."
        ));
    }

    // Stage all changes
    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(&arf_path)
        .output()
        .await?;

    if !add.status.success() {
        return Err(anyhow!(
            "Failed to stage records: {}",
            String::from_utf8_lossy(&add.stderr)
        ));
    }

    // Commit
    let commit = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(&arf_path)
        .output()
        .await?;

    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        // No changes to commit is ok
        if stderr.contains("nothing to commit") {
            return Ok("no-change".to_string());
        }
        return Err(anyhow!("Failed to commit records: {}", stderr));
    }

    // Get the commit SHA
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&arf_path)
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&sha.stdout).trim().to_string())
}

/// Initialize ARF with an orphan branch and worktree.
///
/// Creates an orphan branch `arf-history` (no shared history with main)
/// and mounts it as a worktree at `.arf/`. ARF records will be stored
/// as real git commits on this branch.
pub async fn init_worktree(cwd: &Path) -> Result<()> {
    let arf_path = cwd.join(ARF_DIR);

    // Check if already initialized as worktree
    if has_arf_worktree(cwd) {
        println!("{} ARF worktree already exists at {}", "✓".green(), ARF_DIR);
        return Ok(());
    }

    // Check if .arf exists as a regular directory (old-style)
    if arf_path.exists() && !has_arf_worktree(cwd) {
        println!(
            "{} Found existing .arf/ directory (non-worktree). Will preserve records.",
            "!".yellow()
        );
        // Rename to preserve existing records
        let backup = cwd.join(".arf-backup");
        fs::rename(&arf_path, &backup)?;
        println!("  Backed up existing records to .arf-backup/");
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

    println!("{}", "Initializing ARF worktree...".cyan());

    // Check if orphan branch already exists
    let branch_check = Command::new("git")
        .args(["rev-parse", "--verify", ARF_BRANCH])
        .current_dir(cwd)
        .output()
        .await?;

    if branch_check.status.success() {
        // Branch exists, just add the worktree
        println!(
            "  {} Orphan branch '{}' already exists",
            "✓".green(),
            ARF_BRANCH
        );
        println!("  Adding worktree at '{}'...", ARF_DIR);

        let worktree = Command::new("git")
            .args(["worktree", "add", ARF_DIR, ARF_BRANCH])
            .current_dir(cwd)
            .output()
            .await?;

        if !worktree.status.success() {
            return Err(anyhow!(
                "Failed to add worktree: {}",
                String::from_utf8_lossy(&worktree.stderr)
            ));
        }
    } else {
        // Create orphan branch AND worktree in one step (Git 2.41+)
        println!("  Creating orphan branch '{}' with worktree...", ARF_BRANCH);

        let worktree = Command::new("git")
            .args(["worktree", "add", "--orphan", "-b", ARF_BRANCH, ARF_DIR])
            .current_dir(cwd)
            .output()
            .await?;

        if !worktree.status.success() {
            return Err(anyhow!(
                "Failed to create orphan worktree: {}",
                String::from_utf8_lossy(&worktree.stderr)
            ));
        }

        println!("  {} Created orphan branch '{}'", "✓".green(), ARF_BRANCH);
    }

    println!("  {} Added worktree at '{}'", "✓".green(), ARF_DIR);

    // Create initial structure in worktree
    let records_dir = arf_path.join("records");
    fs::create_dir_all(&records_dir)?;

    // Create README in ARF worktree
    let readme_content = r#"# ARF (Agent Reasoning Format) History

This branch contains structured reasoning records from AI agent sessions.

Each session is stored as a series of TOML records capturing:
- what: Concrete action being taken
- why: Reasoning behind the approach
- how: Implementation details (optional)
- backup: Rollback plan if it fails (optional)
- outcome: success/failure/partial
- context: Metadata (workflow, step, backend, code_commit, etc.)

This history is separate from the main code history but linked
via code_commit references in each record.

## Structure

- `records/{session}/` - TOML records for each session
- Commits on this branch group related records

## Usage

This branch is managed by `lok` and should not be edited manually.
Use `lok report` to generate human-readable summaries.
"#;

    fs::write(arf_path.join("README.md"), readme_content)?;

    // Restore backed up records if any
    let backup = cwd.join(".arf-backup");
    if backup.exists() {
        let backup_records = backup.join("records");
        if backup_records.exists() {
            println!("  Restoring backed up records...");
            // Copy all session directories
            for entry in fs::read_dir(&backup_records)? {
                let entry = entry?;
                let dest = records_dir.join(entry.file_name());
                if entry.path().is_dir() {
                    copy_dir_recursive(&entry.path(), &dest)?;
                }
            }
            println!("  {} Restored records from backup", "✓".green());
        }
        // Remove backup
        fs::remove_dir_all(&backup)?;
    }

    // Commit the initial structure
    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(&arf_path)
        .output()
        .await?;

    if add.status.success() {
        let _ = Command::new("git")
            .args(["commit", "-m", "Initialize ARF structure"])
            .current_dir(&arf_path)
            .output()
            .await;
    }

    println!();
    println!("{} ARF initialized!", "✓".green().bold());
    println!();
    println!(
        "Reasoning history will be tracked on the '{}' branch.",
        ARF_BRANCH
    );
    println!("Worktree mounted at '{}'.", ARF_DIR);
    println!();
    println!("Next steps:");
    println!("  • Run workflows with `lok run <workflow>`");
    println!("  • Records will be created automatically");
    println!("  • Generate reports with `lok report`");

    Ok(())
}

/// Copy directory recursively
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
