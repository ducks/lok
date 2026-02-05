//! Shared utility functions

use colored::Colorize;
use std::path::{Path, PathBuf};

/// Classification of backend errors for user-friendly messaging
#[derive(Debug, Clone, PartialEq)]
pub enum BackendErrorKind {
    /// Rate limited (429, quota exceeded)
    RateLimited,
    /// Capacity exhausted (no servers available)
    CapacityExhausted,
    /// Authentication error (401, 403)
    AuthError,
    /// Network error (connection refused, timeout)
    NetworkError,
    /// Command not found / not installed
    NotInstalled,
    /// Unknown error (show full message)
    Unknown,
}

impl BackendErrorKind {
    /// Returns a short description of the error kind
    pub fn description(&self) -> &'static str {
        match self {
            BackendErrorKind::RateLimited => "rate limited",
            BackendErrorKind::CapacityExhausted => "no capacity available",
            BackendErrorKind::AuthError => "authentication failed",
            BackendErrorKind::NetworkError => "network error",
            BackendErrorKind::NotInstalled => "not installed",
            BackendErrorKind::Unknown => "failed",
        }
    }

    /// Returns a hint for the user
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            BackendErrorKind::RateLimited => Some("try again later or use fewer backends"),
            BackendErrorKind::CapacityExhausted => Some("try again later"),
            BackendErrorKind::AuthError => Some("check your API credentials"),
            BackendErrorKind::NetworkError => Some("check your internet connection"),
            BackendErrorKind::NotInstalled => Some("install the backend CLI tool"),
            BackendErrorKind::Unknown => None,
        }
    }
}

/// Classify a backend error message into a known category
pub fn classify_backend_error(error: &str) -> BackendErrorKind {
    let error_lower = error.to_lowercase();

    // Rate limiting patterns
    if error_lower.contains("429")
        || error_lower.contains("rate limit")
        || error_lower.contains("ratelimit")
        || error_lower.contains("too many requests")
        || error_lower.contains("quota")
    {
        return BackendErrorKind::RateLimited;
    }

    // Capacity exhausted patterns
    if error_lower.contains("capacity")
        || error_lower.contains("resource_exhausted")
        || error_lower.contains("no capacity")
        || error_lower.contains("overloaded")
    {
        return BackendErrorKind::CapacityExhausted;
    }

    // Auth error patterns
    if error_lower.contains("401")
        || error_lower.contains("403")
        || error_lower.contains("unauthorized")
        || error_lower.contains("forbidden")
        || error_lower.contains("invalid api key")
        || error_lower.contains("authentication")
    {
        return BackendErrorKind::AuthError;
    }

    // Network error patterns
    if error_lower.contains("econnrefused")
        || error_lower.contains("etimedout")
        || error_lower.contains("enetunreach")
        || error_lower.contains("connection refused")
        || error_lower.contains("network")
        || error_lower.contains("dns")
    {
        return BackendErrorKind::NetworkError;
    }

    // Not installed patterns
    if error_lower.contains("command not found")
        || error_lower.contains("not found")
        || error_lower.contains("no such file")
        || error_lower.contains("enoent")
    {
        return BackendErrorKind::NotInstalled;
    }

    BackendErrorKind::Unknown
}

/// Generate a user-friendly one-line error summary
pub fn summarize_backend_error(backend_name: &str, error: &str) -> String {
    let kind = classify_backend_error(error);

    match kind {
        BackendErrorKind::Unknown => {
            // For unknown errors, show a truncated version
            let first_line = error.lines().next().unwrap_or(error);
            let clean = first_line
                .trim()
                .trim_start_matches(&format!("{} failed:", backend_name))
                .trim_start_matches(&format!("{} failed:", backend_name.to_uppercase()))
                .trim();
            truncate(clean, 80)
        }
        _ => {
            // For known errors, show the classification with optional hint
            match kind.hint() {
                Some(hint) => format!("{} ({})", kind.description(), hint),
                None => kind.description().to_string(),
            }
        }
    }
}

/// Attempts to canonicalize a path, logging a warning and returning the original path on failure.
pub async fn canonicalize_async(path: &Path) -> PathBuf {
    tokio::fs::canonicalize(path).await.unwrap_or_else(|e| {
        eprintln!(
            "{} Failed to canonicalize path '{}': {}",
            "warning:".yellow(),
            path.display(),
            e
        );
        path.to_path_buf()
    })
}

/// Truncate a string to a maximum number of characters, adding "..." if truncated
pub fn truncate(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

/// Truncate a string at a UTF-8 character boundary, staying under `max_bytes`.
/// Returns the original string if already within limit.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    s.char_indices()
        .take_while(|(i, c)| i + c.len_utf8() <= max_bytes)
        .last()
        .map(|(i, c)| &s[..i + c.len_utf8()])
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_unicode() {
        assert_eq!(truncate("héllo wörld", 5), "héllo...");
    }

    #[test]
    fn test_truncate_utf8_ascii() {
        assert_eq!(truncate_utf8("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_utf8_multibyte_boundary() {
        // 4-byte emoji at boundary - should not split the character
        let s = "hello\u{1F600}world"; // 😀 is 4 bytes
                                       // "hello" is 5 bytes, emoji starts at byte 5
                                       // With max_bytes=6, we can't fit the emoji, so truncate after "hello"
        assert_eq!(truncate_utf8(s, 6), "hello");
        // With max_bytes=9, we can fit "hello" + emoji
        assert_eq!(truncate_utf8(s, 9), "hello\u{1F600}");
    }

    #[test]
    fn test_truncate_utf8_empty_string() {
        assert_eq!(truncate_utf8("", 10), "");
    }

    #[test]
    fn test_truncate_utf8_zero_cap() {
        assert_eq!(truncate_utf8("hello", 0), "");
    }

    #[test]
    fn test_truncate_utf8_exact_boundary() {
        assert_eq!(truncate_utf8("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_utf8_within_limit() {
        assert_eq!(truncate_utf8("hi", 10), "hi");
    }

    #[test]
    fn test_classify_rate_limit_429() {
        assert_eq!(
            classify_backend_error("Error 429: Too Many Requests"),
            BackendErrorKind::RateLimited
        );
    }

    #[test]
    fn test_classify_rate_limit_quota() {
        assert_eq!(
            classify_backend_error("Quota exceeded for the day"),
            BackendErrorKind::RateLimited
        );
    }

    #[test]
    fn test_classify_capacity_exhausted() {
        assert_eq!(
            classify_backend_error("No capacity available for model gemini-3-flash"),
            BackendErrorKind::CapacityExhausted
        );
    }

    #[test]
    fn test_classify_resource_exhausted() {
        assert_eq!(
            classify_backend_error("RESOURCE_EXHAUSTED: Model overloaded"),
            BackendErrorKind::CapacityExhausted
        );
    }

    #[test]
    fn test_classify_auth_401() {
        assert_eq!(
            classify_backend_error("HTTP 401 Unauthorized"),
            BackendErrorKind::AuthError
        );
    }

    #[test]
    fn test_classify_auth_invalid_key() {
        assert_eq!(
            classify_backend_error("Invalid API key provided"),
            BackendErrorKind::AuthError
        );
    }

    #[test]
    fn test_classify_network_refused() {
        assert_eq!(
            classify_backend_error("ECONNREFUSED: Connection refused"),
            BackendErrorKind::NetworkError
        );
    }

    #[test]
    fn test_classify_not_installed() {
        assert_eq!(
            classify_backend_error("sh: npx: command not found"),
            BackendErrorKind::NotInstalled
        );
    }

    #[test]
    fn test_classify_unknown() {
        assert_eq!(
            classify_backend_error("Something weird happened"),
            BackendErrorKind::Unknown
        );
    }

    #[test]
    fn test_summarize_rate_limit() {
        let summary = summarize_backend_error("gemini", "Error 429: Too Many Requests");
        assert_eq!(
            summary,
            "rate limited (try again later or use fewer backends)"
        );
    }

    #[test]
    fn test_summarize_capacity() {
        let summary = summarize_backend_error("gemini", "No capacity available");
        assert_eq!(summary, "no capacity available (try again later)");
    }

    #[test]
    fn test_summarize_unknown_truncates() {
        let long_error = "Gemini failed: ".to_string() + &"x".repeat(200);
        let summary = summarize_backend_error("gemini", &long_error);
        assert!(summary.len() <= 83); // 80 chars + "..."
        assert!(summary.ends_with("..."));
    }
}
