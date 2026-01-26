//! Result caching for lok queries
//!
//! Caches query results by hashing prompt + backend + working directory.
//! Stored in ~/.cache/lok/ with configurable TTL.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::backend::QueryResult;

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
}

impl Cache {
    pub fn new(config: &CacheConfig) -> Self {
        let dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("lok");

        Self {
            dir,
            ttl: Duration::from_secs(config.ttl_hours * 3600),
            enabled: config.enabled,
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
    pub fn get(&self, key: &str) -> Option<Vec<QueryResult>> {
        if !self.enabled {
            return None;
        }

        let path = self.dir.join(format!("{}.json", key));
        let content = fs::read_to_string(&path).ok()?;
        let entry: CacheEntry = serde_json::from_str(&content).ok()?;

        // Check TTL
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if now - entry.timestamp > self.ttl.as_secs() {
            // Expired, remove it
            let _ = fs::remove_file(&path);
            return None;
        }

        Some(entry.results.into_iter().map(QueryResult::from).collect())
    }

    /// Store results in cache
    pub fn set(&self, key: &str, results: &[QueryResult]) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure cache directory exists
        fs::create_dir_all(&self.dir)?;

        let entry = CacheEntry {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            results: results.iter().map(CachedResult::from).collect(),
        };

        let path = self.dir.join(format!("{}.json", key));
        let content = serde_json::to_string_pretty(&entry)?;
        fs::write(path, content)?;

        Ok(())
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

    #[test]
    fn test_cache_disabled() {
        let config = CacheConfig {
            enabled: false,
            ttl_hours: 24,
        };
        let cache = Cache::new(&config);

        let results = vec![QueryResult {
            backend: "test".to_string(),
            output: "output".to_string(),
            success: true,
            elapsed_ms: 100,
        }];

        // Should not error, but also not cache
        cache.set("key", &results).unwrap();
        assert!(cache.get("key").is_none());
    }
}
