// Context detection for codebases
// Scans for tooling patterns and generates context to prepend to prompts

use std::fs;
use std::path::Path;

#[derive(Debug, Default)]
pub struct CodebaseContext {
    // Ruby/Rails
    pub is_rails: bool,
    pub has_goldiloader: bool,
    pub has_bullet: bool,
    pub has_brakeman: bool,
    pub has_rubocop: bool,
    pub has_strong_migrations: bool,
    pub has_rspec: bool,
    pub has_sidekiq: bool,
    pub has_sorbet: bool,

    // JavaScript/TypeScript
    pub has_typescript: bool,
    pub has_eslint: bool,
    pub has_prettier: bool,
    pub has_jest: bool,
    pub has_vitest: bool,
    pub has_react: bool,
    pub has_vue: bool,
    pub has_nextjs: bool,
    pub has_tailwind: bool,

    // Python
    pub is_python: bool,
    pub is_django: bool,
    pub is_fastapi: bool,
    pub has_sqlalchemy: bool,
    pub has_pytest: bool,
    pub has_mypy: bool,
    pub has_ruff: bool,
    pub has_alembic: bool,

    // Rust
    pub is_rust: bool,
    pub has_tokio: bool,
    pub has_diesel: bool,
    pub has_sqlx: bool,

    // Go
    pub is_go: bool,
    pub has_golangci_lint: bool,

    // Infrastructure
    pub has_docker: bool,
    pub has_kubernetes: bool,
    pub has_terraform: bool,
    pub has_github_actions: bool,
    pub has_gitlab_ci: bool,

    // General
    pub detected_language: Option<String>,
}

impl CodebaseContext {
    /// Detect context from a codebase directory
    pub fn detect(cwd: &Path) -> Self {
        let mut ctx = CodebaseContext::default();

        // Ruby/Rails detection
        if let Ok(gemfile) = fs::read_to_string(cwd.join("Gemfile")) {
            ctx.is_rails = gemfile.contains("rails");
            ctx.has_goldiloader = gemfile.contains("goldiloader");
            ctx.has_bullet = gemfile.contains("bullet");
            ctx.has_brakeman = gemfile.contains("brakeman");
            ctx.has_rubocop = gemfile.contains("rubocop");
            ctx.has_strong_migrations = gemfile.contains("strong_migrations");
            ctx.has_rspec = gemfile.contains("rspec");
            ctx.has_sidekiq = gemfile.contains("sidekiq");
            ctx.has_sorbet = gemfile.contains("sorbet");

            if ctx.is_rails {
                ctx.detected_language = Some("ruby/rails".to_string());
            } else if gemfile.contains("gem ") {
                ctx.detected_language = Some("ruby".to_string());
            }
        }

        // JavaScript/TypeScript detection
        if let Ok(package_json) = fs::read_to_string(cwd.join("package.json")) {
            ctx.has_typescript =
                package_json.contains("typescript") || cwd.join("tsconfig.json").exists();
            ctx.has_eslint = package_json.contains("eslint")
                || cwd.join(".eslintrc").exists()
                || cwd.join("eslint.config").exists();
            ctx.has_prettier =
                package_json.contains("prettier") || cwd.join(".prettierrc").exists();
            ctx.has_jest = package_json.contains("\"jest\"");
            ctx.has_vitest = package_json.contains("vitest");
            ctx.has_react = package_json.contains("\"react\"");
            ctx.has_vue = package_json.contains("\"vue\"");
            ctx.has_nextjs = package_json.contains("\"next\"");
            ctx.has_tailwind =
                package_json.contains("tailwindcss") || cwd.join("tailwind.config.js").exists();

            if ctx.detected_language.is_none() {
                ctx.detected_language = Some(if ctx.has_typescript {
                    "typescript".to_string()
                } else {
                    "javascript".to_string()
                });
            }
        }

        // Python detection
        let requirements = fs::read_to_string(cwd.join("requirements.txt")).unwrap_or_default();
        let pyproject = fs::read_to_string(cwd.join("pyproject.toml")).unwrap_or_default();
        let has_python_files =
            !requirements.is_empty() || !pyproject.is_empty() || cwd.join("setup.py").exists();

        if has_python_files {
            ctx.is_python = true;
            let combined = format!("{}\n{}", requirements, pyproject);

            ctx.is_django = combined.contains("django");
            ctx.is_fastapi = combined.contains("fastapi");
            ctx.has_sqlalchemy = combined.contains("sqlalchemy");
            ctx.has_pytest = combined.contains("pytest");
            ctx.has_mypy = combined.contains("mypy");
            ctx.has_ruff = combined.contains("ruff");
            ctx.has_alembic = combined.contains("alembic");

            if ctx.detected_language.is_none() {
                ctx.detected_language = Some(if ctx.is_django {
                    "python/django".to_string()
                } else if ctx.is_fastapi {
                    "python/fastapi".to_string()
                } else {
                    "python".to_string()
                });
            }
        }

        // Rust detection
        if let Ok(cargo_toml) = fs::read_to_string(cwd.join("Cargo.toml")) {
            ctx.is_rust = true;
            ctx.has_tokio = cargo_toml.contains("tokio");
            ctx.has_diesel = cargo_toml.contains("diesel");
            ctx.has_sqlx = cargo_toml.contains("sqlx");

            if ctx.detected_language.is_none() {
                ctx.detected_language = Some("rust".to_string());
            }
        }

        // Go detection
        if let Ok(go_mod) = fs::read_to_string(cwd.join("go.mod")) {
            ctx.is_go = true;
            if ctx.detected_language.is_none() {
                ctx.detected_language = Some("go".to_string());
            }
            // Check for golangci-lint config
            ctx.has_golangci_lint = cwd.join(".golangci.yml").exists()
                || cwd.join(".golangci.yaml").exists()
                || go_mod.contains("golangci");
        }

        // Infrastructure detection
        ctx.has_docker = cwd.join("Dockerfile").exists() || cwd.join("docker-compose.yml").exists();
        ctx.has_kubernetes = cwd.join("k8s").exists()
            || cwd.join("kubernetes").exists()
            || cwd.join("helm").exists();
        ctx.has_terraform = cwd.join("main.tf").exists() || cwd.join("terraform").exists();
        ctx.has_github_actions = cwd.join(".github/workflows").exists();
        ctx.has_gitlab_ci = cwd.join(".gitlab-ci.yml").exists();

        ctx
    }

    /// Check if any context was detected
    pub fn has_context(&self) -> bool {
        self.has_goldiloader
            || self.has_bullet
            || self.has_brakeman
            || self.has_eslint
            || self.has_typescript
            || self.has_sqlalchemy
    }

    /// Generate context string for N+1 detection prompts
    pub fn n1_context(&self) -> Option<String> {
        let mut notes = Vec::new();

        if self.has_goldiloader {
            notes.push(
                "This codebase uses Goldiloader, which automatically eager-loads associations \
                 to prevent N+1 queries at runtime. Only report N+1 patterns that Goldiloader \
                 cannot fix: queries in views/serializers, queries across request boundaries, \
                 or queries in background jobs where associations aren't auto-loaded."
                    .to_string(),
            );
        }

        if self.has_bullet {
            notes.push(
                "This codebase uses Bullet gem for N+1 detection in development. \
                 Focus on patterns Bullet might miss: complex joins, polymorphic associations, \
                 or N+1s in production-only code paths."
                    .to_string(),
            );
        }

        if notes.is_empty() {
            None
        } else {
            Some(format!("CODEBASE CONTEXT:\n{}\n\n", notes.join("\n")))
        }
    }

    /// Generate context string for security audit prompts
    pub fn security_context(&self) -> Option<String> {
        let mut notes = Vec::new();

        if self.has_brakeman {
            notes.push(
                "This codebase uses Brakeman for static security analysis. \
                 Focus on issues Brakeman might miss: logic flaws, business logic vulnerabilities, \
                 race conditions, and authorization bypasses."
                    .to_string(),
            );
        }

        if self.has_eslint {
            notes.push(
                "This codebase uses ESLint. Security-focused ESLint plugins may catch basic XSS. \
                 Focus on framework-specific issues and logic vulnerabilities."
                    .to_string(),
            );
        }

        if self.has_rubocop {
            notes.push(
                "This codebase uses RuboCop which has some security cops. \
                 Focus on issues beyond static analysis: auth logic, IDOR, race conditions."
                    .to_string(),
            );
        }

        if self.has_sorbet || self.has_typescript || self.has_mypy {
            notes.push(
                "This codebase uses static typing which catches some classes of bugs. \
                 Focus on runtime issues, type coercion vulnerabilities, and logic flaws."
                    .to_string(),
            );
        }

        if notes.is_empty() {
            None
        } else {
            Some(format!("CODEBASE CONTEXT:\n{}\n\n", notes.join("\n")))
        }
    }

    /// Generate context string for database/migration prompts
    pub fn db_context(&self) -> Option<String> {
        let mut notes = Vec::new();

        if self.has_strong_migrations {
            notes.push(
                "This codebase uses StrongMigrations which catches unsafe migration patterns. \
                 Focus on data integrity issues, complex multi-step migrations, and rollback safety."
                    .to_string(),
            );
        }

        if self.has_alembic {
            notes.push("This codebase uses Alembic for database migrations.".to_string());
        }

        if notes.is_empty() {
            None
        } else {
            Some(format!("CODEBASE CONTEXT:\n{}\n\n", notes.join("\n")))
        }
    }

    /// Generate full context string for any prompt
    pub fn full_context(&self) -> Option<String> {
        let mut sections = Vec::new();

        if let Some(lang) = &self.detected_language {
            sections.push(format!("Primary language: {}", lang));
        }

        if self.has_goldiloader {
            sections.push("- Uses Goldiloader (auto N+1 prevention)".to_string());
        }
        if self.has_bullet {
            sections.push("- Uses Bullet (N+1 detection in dev)".to_string());
        }
        if self.has_brakeman {
            sections.push("- Uses Brakeman (security scanner)".to_string());
        }
        if self.has_typescript {
            sections.push("- Uses TypeScript".to_string());
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("CODEBASE CONTEXT:\n{}\n\n", sections.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_detect_rails_with_goldiloader() {
        let dir = tempdir().unwrap();
        let gemfile_path = dir.path().join("Gemfile");
        let mut file = File::create(&gemfile_path).unwrap();
        writeln!(file, "gem 'rails'").unwrap();
        writeln!(file, "gem 'goldiloader'").unwrap();

        let ctx = CodebaseContext::detect(dir.path());

        assert!(ctx.is_rails);
        assert!(ctx.has_goldiloader);
        assert!(!ctx.has_bullet);
        assert!(ctx.n1_context().is_some());
        assert!(ctx.n1_context().unwrap().contains("Goldiloader"));
    }

    #[test]
    fn test_detect_typescript() {
        let dir = tempdir().unwrap();
        let package_path = dir.path().join("package.json");
        let mut file = File::create(&package_path).unwrap();
        writeln!(file, r#"{{"devDependencies": {{"typescript": "^5.0.0"}}}}"#).unwrap();

        let tsconfig_path = dir.path().join("tsconfig.json");
        File::create(&tsconfig_path).unwrap();

        let ctx = CodebaseContext::detect(dir.path());

        assert!(ctx.has_typescript);
        assert_eq!(ctx.detected_language, Some("typescript".to_string()));
    }

    #[test]
    fn test_no_context() {
        let dir = tempdir().unwrap();
        let ctx = CodebaseContext::detect(dir.path());

        assert!(!ctx.has_context());
        assert!(ctx.n1_context().is_none());
        assert!(ctx.security_context().is_none());
    }
}
