//! ARF (Agent Reasoning Format) record support
//!
//! Emits structured reasoning records at checkpoint granularity
//! so execution can be traced on a graph.

use chrono::Utc;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// ARF record - captures reasoning at a checkpoint
#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Failure,
    Partial,
}

#[derive(Debug, Clone, Serialize)]
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
}

impl Default for ArfContext {
    fn default() -> Self {
        Self {
            timestamp: Some(Utc::now().to_rfc3339()),
            workflow: None,
            step: None,
            backend: None,
            elapsed_ms: None,
            backends_queried: None,
            backends_succeeded: None,
            backends_failed: None,
            file: None,
            error: None,
            retry_attempt: None,
            parent: None,
        }
    }
}

/// ARF recorder - writes records to .arf/records/
pub struct ArfRecorder {
    enabled: bool,
    base_path: PathBuf,
    #[allow(dead_code)] // Used by tests and future API consumers
    session_id: String,
    record_count: u32,
}

impl ArfRecorder {
    /// Create a new recorder for the given working directory
    pub fn new(cwd: &std::path::Path) -> Self {
        let arf_dir = cwd.join(".arf");
        let enabled = arf_dir.exists();
        let session_id = Utc::now().format("%Y%m%d-%H%M%S").to_string();

        Self {
            enabled,
            base_path: arf_dir.join("records").join(&session_id),
            session_id,
            record_count: 0,
        }
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
    pub fn record(&mut self, record: ArfRecord) -> std::io::Result<String> {
        if !self.enabled {
            return Ok(String::new());
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
