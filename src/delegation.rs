use std::collections::HashMap;

/// Task categories that backends can be suited for
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCategory {
    /// N+1 queries, code smells, patterns
    CodeAnalysis,
    /// SQL injection, auth bypass, XSS
    SecurityAudit,
    /// Dead code, unused imports
    DeadCode,
    /// Performance issues, slow queries
    Performance,
    /// Architecture, design patterns
    Architecture,
    /// General questions
    General,
}

/// Backend strengths and characteristics
#[derive(Debug, Clone)]
pub struct BackendProfile {
    pub name: String,
    /// Categories this backend excels at
    pub strengths: Vec<TaskCategory>,
    /// Brief description of the backend's approach
    pub style: String,
}

/// Smart delegation based on task type
pub struct Delegator {
    profiles: HashMap<String, BackendProfile>,
}

impl Delegator {
    pub fn new() -> Self {
        let mut profiles = HashMap::new();

        profiles.insert(
            "codex".to_string(),
            BackendProfile {
                name: "codex".to_string(),
                strengths: vec![
                    TaskCategory::CodeAnalysis,
                    TaskCategory::DeadCode,
                    TaskCategory::Performance,
                    TaskCategory::General, // fallback for general tasks
                ],
                style: "Efficient, direct answers. Good for pattern matching and quick analysis."
                    .to_string(),
            },
        );

        profiles.insert(
            "gemini".to_string(),
            BackendProfile {
                name: "gemini".to_string(),
                strengths: vec![
                    TaskCategory::SecurityAudit,
                    TaskCategory::Architecture,
                    TaskCategory::General, // fallback for general tasks
                ],
                style: "Thorough, investigative. Goes deep with multi-step analysis.".to_string(),
            },
        );

        profiles.insert(
            "claude".to_string(),
            BackendProfile {
                name: "claude".to_string(),
                strengths: vec![
                    TaskCategory::General,
                    TaskCategory::Architecture,
                    TaskCategory::CodeAnalysis,
                ],
                style: "Balanced, good at orchestration and nuanced analysis.".to_string(),
            },
        );

        Self { profiles }
    }

    /// Classify a prompt into task categories
    pub fn classify_task(&self, prompt: &str) -> Vec<TaskCategory> {
        let lower = prompt.to_lowercase();
        let mut categories = Vec::new();

        // Code analysis patterns
        if lower.contains("n+1")
            || lower.contains("query")
            || lower.contains("code smell")
            || lower.contains("refactor")
            || lower.contains("pattern")
        {
            categories.push(TaskCategory::CodeAnalysis);
        }

        // Security patterns
        if lower.contains("security")
            || lower.contains("injection")
            || lower.contains("xss")
            || lower.contains("auth")
            || lower.contains("vulnerability")
            || lower.contains("audit")
        {
            categories.push(TaskCategory::SecurityAudit);
        }

        // Dead code patterns
        if lower.contains("dead code")
            || lower.contains("unused")
            || lower.contains("remove")
            || lower.contains("cleanup")
        {
            categories.push(TaskCategory::DeadCode);
        }

        // Performance patterns
        if lower.contains("performance")
            || lower.contains("slow")
            || lower.contains("optimize")
            || lower.contains("speed")
        {
            categories.push(TaskCategory::Performance);
        }

        // Architecture patterns
        if lower.contains("architecture")
            || lower.contains("design")
            || lower.contains("structure")
            || lower.contains("organize")
        {
            categories.push(TaskCategory::Architecture);
        }

        // Default to general if nothing specific
        if categories.is_empty() {
            categories.push(TaskCategory::General);
        }

        categories
    }

    /// Get recommended backends for a task, ordered by suitability
    pub fn recommend(&self, prompt: &str) -> Vec<&BackendProfile> {
        let categories = self.classify_task(prompt);
        let mut recommendations: Vec<(&BackendProfile, usize)> = Vec::new();

        for profile in self.profiles.values() {
            // Score: number of matching categories, weighted by position in strengths list
            let score: usize = categories
                .iter()
                .filter_map(|cat| {
                    profile.strengths.iter().position(|s| s == cat).map(|pos| {
                        // Higher score for earlier positions in strengths list
                        10 - pos.min(9)
                    })
                })
                .sum();

            if score > 0 {
                recommendations.push((profile, score));
            }
        }

        // Sort by score descending, then by name for stability
        recommendations.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));

        // Return profiles only
        recommendations.into_iter().map(|(p, _)| p).collect()
    }

    /// Get the best single backend for a task
    pub fn best_for(&self, prompt: &str) -> Option<&str> {
        self.recommend(prompt).first().map(|p| p.name.as_str())
    }

    /// Explain why a backend is recommended for a task
    pub fn explain(&self, prompt: &str) -> String {
        let categories = self.classify_task(prompt);
        let recommendations = self.recommend(prompt);

        let mut explanation = format!(
            "Task categories detected: {:?}\n\n",
            categories
                .iter()
                .map(|c| format!("{:?}", c))
                .collect::<Vec<_>>()
        );

        explanation.push_str("Recommended backends:\n");
        for (i, profile) in recommendations.iter().enumerate() {
            explanation.push_str(&format!(
                "{}. {} - {}\n",
                i + 1,
                profile.name.to_uppercase(),
                profile.style
            ));
        }

        explanation
    }
}

impl Default for Delegator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_n1() {
        let d = Delegator::new();
        let cats = d.classify_task("Find N+1 queries in this codebase");
        assert!(cats.contains(&TaskCategory::CodeAnalysis));
    }

    #[test]
    fn test_classify_security() {
        let d = Delegator::new();
        let cats = d.classify_task("Find SQL injection vulnerabilities");
        assert!(cats.contains(&TaskCategory::SecurityAudit));
    }

    #[test]
    fn test_recommend_n1() {
        let d = Delegator::new();
        let best = d.best_for("Find N+1 queries");
        assert_eq!(best, Some("codex"));
    }

    #[test]
    fn test_recommend_security() {
        let d = Delegator::new();
        let best = d.best_for("Security audit for SQL injection");
        assert_eq!(best, Some("gemini"));
    }

    #[test]
    fn test_classify_dead_code() {
        let d = Delegator::new();
        let cats = d.classify_task("Find unused functions and dead code");
        assert!(cats.contains(&TaskCategory::DeadCode));
    }

    #[test]
    fn test_classify_performance() {
        let d = Delegator::new();
        let cats = d.classify_task("Optimize slow database queries");
        assert!(cats.contains(&TaskCategory::Performance));
    }

    #[test]
    fn test_classify_architecture() {
        let d = Delegator::new();
        let cats = d.classify_task("Review the overall architecture and design patterns");
        assert!(cats.contains(&TaskCategory::Architecture));
    }

    #[test]
    fn test_classify_multiple_categories() {
        let d = Delegator::new();
        let cats = d.classify_task("Find security vulnerabilities and N+1 queries");
        assert!(cats.contains(&TaskCategory::SecurityAudit));
        assert!(cats.contains(&TaskCategory::CodeAnalysis));
    }

    #[test]
    fn test_classify_general_fallback() {
        let d = Delegator::new();
        let cats = d.classify_task("What does this function do?");
        assert!(cats.contains(&TaskCategory::General));
        assert_eq!(cats.len(), 1);
    }

    #[test]
    fn test_recommend_dead_code() {
        let d = Delegator::new();
        let best = d.best_for("Remove unused imports");
        assert_eq!(best, Some("codex"));
    }

    #[test]
    fn test_recommend_architecture() {
        let d = Delegator::new();
        let best = d.best_for("Review the project structure and organization");
        // Both claude and gemini have Architecture in their strengths
        assert!(best == Some("claude") || best == Some("gemini"));
    }

    #[test]
    fn test_recommend_general_returns_backend() {
        let d = Delegator::new();
        let best = d.best_for("Explain this code");
        // Should return something (codex or gemini have General in their strengths)
        assert!(best.is_some());
    }

    #[test]
    fn test_recommend_returns_multiple() {
        let d = Delegator::new();
        let recommendations = d.recommend("Find security issues and performance problems");
        // Should return multiple backends
        assert!(recommendations.len() >= 2);
    }

    #[test]
    fn test_explain_contains_categories() {
        let d = Delegator::new();
        let explanation = d.explain("Find N+1 queries");
        assert!(explanation.contains("CodeAnalysis"));
    }

    #[test]
    fn test_explain_contains_recommendations() {
        let d = Delegator::new();
        let explanation = d.explain("Security audit");
        assert!(explanation.contains("GEMINI") || explanation.contains("gemini"));
    }

    #[test]
    fn test_case_insensitive_matching() {
        let d = Delegator::new();
        let cats_lower = d.classify_task("find n+1 queries");
        let cats_upper = d.classify_task("FIND N+1 QUERIES");
        let cats_mixed = d.classify_task("Find N+1 Queries");

        assert_eq!(cats_lower, cats_upper);
        assert_eq!(cats_lower, cats_mixed);
    }

    #[test]
    fn test_delegator_default() {
        let d = Delegator::default();
        // Should work the same as new()
        let best = d.best_for("Find N+1 queries");
        assert_eq!(best, Some("codex"));
    }

    #[test]
    fn test_backend_profiles_exist() {
        let d = Delegator::new();
        // All profiles should have strengths
        let recommendations = d.recommend("anything");
        for rec in recommendations {
            assert!(!rec.strengths.is_empty());
            assert!(!rec.style.is_empty());
        }
    }
}
