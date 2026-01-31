//! Result caching for lok queries
//!
//! Caches query results by hashing prompt + backend + working directory.
//! Stored in ~/.cache/lok/ with configurable TTL.

use colored::Colorize;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
// std::fs only used in tests for setup
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::backend::QueryResult;

/// Cache operation types for warning context
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheOperation {
    Init,
    Read,
    Parse,
    Write,
    Delete,
    Clock,
}

impl fmt::Display for CacheOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CacheOperation::Init => write!(f, "init"),
            CacheOperation::Read => write!(f, "read"),
            CacheOperation::Parse => write!(f, "parse"),
            CacheOperation::Write => write!(f, "write"),
            CacheOperation::Delete => write!(f, "delete"),
            CacheOperation::Clock => write!(f, "clock"),
        }
    }
}

/// Warning about a cache operation failure
#[derive(Debug, Clone)]
pub struct CacheWarning {
    pub operation: CacheOperation,
    pub path: Option<PathBuf>,
    pub error: String,
}

impl fmt::Display for CacheWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(p) => write!(
                f,
                "cache {}: {} ({})",
                self.operation,
                self.error,
                p.display()
            ),
            None => write!(f, "cache {}: {}", self.operation, self.error),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CacheConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ttl_hours")]
    pub ttl_hours: u64,
}

fn default_enabled() -> bool {
    true
}

fn default_ttl_hours() -> u64 {
    24
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            ttl_hours: default_ttl_hours(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    timestamp: u64,
    results: Vec<CachedResult>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedResult {
    backend: String,
    output: String,
    success: bool,
    elapsed_ms: u64,
}

impl From<&QueryResult> for CachedResult {
    fn from(r: &QueryResult) -> Self {
        Self {
            backend: r.backend.clone(),
            output: r.output.clone(),
            success: r.success,
            elapsed_ms: r.elapsed_ms,
        }
    }
}

impl From<CachedResult> for QueryResult {
    fn from(r: CachedResult) -> Self {
        Self {
            backend: r.backend,
            output: r.output,
            success: r.success,
            elapsed_ms: r.elapsed_ms,
        }
    }
}

pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
    enabled: bool,
    warnings: Vec<CacheWarning>,
    seen_warnings: HashSet<(CacheOperation, Option<PathBuf>)>,
}

/// Get current Unix timestamp, returning None if system clock is invalid
fn current_timestamp() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

impl Cache {
    pub fn new(config: &CacheConfig) -> Self {
        let dir = match dirs::cache_dir() {
            Some(d) => d.join("lok"),
            None => {
                // Will add warning on first operation that needs the dir
                PathBuf::from("/tmp/lok")
            }
        };

        Self {
            dir,
            ttl: Duration::from_secs(config.ttl_hours * 3600),
            enabled: config.enabled,
            warnings: Vec::new(),
            seen_warnings: HashSet::new(),
        }
    }

    /// Record a warning, deduplicating by (operation, path)
    fn warn(&mut self, operation: CacheOperation, path: Option<&Path>, error: impl ToString) {
        let key = (operation, path.map(|p| p.to_path_buf()));
        if self.seen_warnings.insert(key.clone()) {
            self.warnings.push(CacheWarning {
                operation,
                path: key.1,
                error: error.to_string(),
            });
        }
    }

    /// Take all collected warnings, clearing the internal list
    pub fn take_warnings(&mut self) -> Vec<CacheWarning> {
        self.seen_warnings.clear();
        std::mem::take(&mut self.warnings)
    }

    /// Check if there are any warnings
    #[allow(dead_code)] // Used in tests
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Print warnings to stderr with formatting
    pub fn print_warnings(&mut self) {
        let warnings = self.take_warnings();
        if warnings.is_empty() {
            return;
        }

        eprintln!("\n{}", "Cache warnings:".yellow());
        for warning in warnings {
            eprintln!("  {} {}", "⚠".yellow(), warning);
        }
    }

    /// Generate cache key from prompt, backends, and working directory
    pub fn cache_key(&self, prompt: &str, backends: &[String], cwd: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prompt.as_bytes());
        for backend in backends {
            hasher.update(backend.as_bytes());
        }
        hasher.update(cwd.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Get cached results if valid
    pub async fn get(&mut self, key: &str) -> Option<Vec<QueryResult>> {
        if !self.enabled {
            return None;
        }

        let path = self.dir.join(format!("{}.json", key));

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                self.warn(CacheOperation::Read, Some(&path), e);
                return None;
            }
        };

        let entry: CacheEntry = match serde_json::from_str(&content) {
            Ok(e) => e,
            Err(e) => {
                self.warn(CacheOperation::Parse, Some(&path), e);
                return None;
            }
        };

        // Check TTL
        let now = match current_timestamp() {
            Some(ts) => ts,
            None => {
                self.warn(
                    CacheOperation::Clock,
                    None,
                    "system clock before UNIX epoch",
                );
                return None;
            }
        };

        if now - entry.timestamp > self.ttl.as_secs() {
            // Expired, remove it
            if let Err(e) = tokio::fs::remove_file(&path).await {
                self.warn(CacheOperation::Delete, Some(&path), e);
            }
            return None;
        }

        Some(entry.results.into_iter().map(QueryResult::from).collect())
    }

    /// Store results in cache
    pub async fn set(&mut self, key: &str, results: &[QueryResult]) {
        if !self.enabled {
            return;
        }

        // Skip caching if system clock is invalid
        let timestamp = match current_timestamp() {
            Some(ts) => ts,
            None => {
                self.warn(
                    CacheOperation::Clock,
                    None,
                    "system clock before UNIX epoch",
                );
                return;
            }
        };

        // Ensure cache directory exists
        if let Err(e) = tokio::fs::create_dir_all(&self.dir).await {
            let dir = self.dir.clone();
            self.warn(CacheOperation::Init, Some(&dir), e);
            return;
        }

        let entry = CacheEntry {
            timestamp,
            results: results.iter().map(CachedResult::from).collect(),
        };

        let path = self.dir.join(format!("{}.json", key));

        let content = match serde_json::to_string_pretty(&entry) {
            Ok(c) => c,
            Err(e) => {
                self.warn(CacheOperation::Write, Some(&path), e);
                return;
            }
        };

        if let Err(e) = tokio::fs::write(&path, content).await {
            self.warn(CacheOperation::Write, Some(&path), e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_deterministic() {
        let config = CacheConfig::default();
        let cache = Cache::new(&config);

        let key1 = cache.cache_key("prompt", &["codex".to_string()], "/tmp");
        let key2 = cache.cache_key("prompt", &["codex".to_string()], "/tmp");
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_cache_key_different_prompts() {
        let config = CacheConfig::default();
        let cache = Cache::new(&config);

        let key1 = cache.cache_key("prompt1", &["codex".to_string()], "/tmp");
        let key2 = cache.cache_key("prompt2", &["codex".to_string()], "/tmp");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_cache_key_different_backends() {
        let config = CacheConfig::default();
        let cache = Cache::new(&config);

        let key1 = cache.cache_key("prompt", &["codex".to_string()], "/tmp");
        let key2 = cache.cache_key("prompt", &["gemini".to_string()], "/tmp");
        assert_ne!(key1, key2);
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let config = CacheConfig {
            enabled: false,
            ttl_hours: 24,
        };
        let mut cache = Cache::new(&config);

        let results = vec![QueryResult {
            backend: "test".to_string(),
            output: "output".to_string(),
            success: true,
            elapsed_ms: 100,
        }];

        // Should not cache when disabled
        cache.set("key", &results).await;
        assert!(cache.get("key").await.is_none());
        assert!(!cache.has_warnings());
    }

    #[tokio::test]
    async fn test_cache_warnings_on_parse_failure() {
        let config = CacheConfig::default();
        let mut cache = Cache::new(&config);

        // Create invalid JSON file
        let temp_dir = tempfile::tempdir().unwrap();
        cache.dir = temp_dir.path().to_path_buf();

        let path = cache.dir.join("bad_key.json");
        fs::write(&path, "not valid json").unwrap();

        // Should warn on parse failure
        let result = cache.get("bad_key").await;
        assert!(result.is_none());
        assert!(cache.has_warnings());

        let warnings = cache.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].operation, CacheOperation::Parse);
    }

    #[tokio::test]
    async fn test_cache_warnings_deduplicated() {
        let config = CacheConfig::default();
        let mut cache = Cache::new(&config);

        // Create invalid JSON file
        let temp_dir = tempfile::tempdir().unwrap();
        cache.dir = temp_dir.path().to_path_buf();

        let path = cache.dir.join("bad_key.json");
        fs::write(&path, "not valid json").unwrap();

        // Multiple reads should deduplicate warnings
        cache.get("bad_key").await;
        cache.get("bad_key").await;
        cache.get("bad_key").await;

        let warnings = cache.take_warnings();
        assert_eq!(warnings.len(), 1);
    }
}
