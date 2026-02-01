//! Shared utility functions

use colored::Colorize;
use std::path::{Path, PathBuf};

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
}
