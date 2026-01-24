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
        recommendations.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.name.cmp(&b.0.name))
        });

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
}
