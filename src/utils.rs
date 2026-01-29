//! Shared utility functions

use colored::Colorize;
use std::path::{Path, PathBuf};

/// Attempts to canonicalize a path, printing a warning and returning
/// the original path if canonicalization fails.
pub fn canonicalize_or_warn(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|e| {
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
}
