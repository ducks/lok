# AI Agent Guide for Lok

Instructions for AI coding agents working on this codebase.

## Project Overview

Lok is a CLI tool that orchestrates multiple LLM backends (Claude, Codex, Gemini)
for code analysis tasks like bug hunting, security audits, and PR reviews. The key
insight is treating LLMs as composable infrastructure rather than chatbots.

**Core concepts:**
- **Backends**: Individual LLM providers (Claude API, Codex CLI, Gemini CLI)
- **Tasks**: Built-in operations (hunt, audit, fix) defined in `src/tasks/`
- **Workflows**: User-defined TOML pipelines in `.lok/workflows/`
- **Conductor**: Orchestration mode where one LLM delegates to others

## Project Structure

```
src/
├── main.rs           # CLI entry point (clap)
├── config.rs         # Configuration loading (~/.config/lok/config.toml)
├── backend/          # LLM provider implementations
│   ├── mod.rs        # Backend trait + factory
│   ├── claude.rs     # Claude API + claude-code CLI
│   ├── codex.rs      # OpenAI Codex CLI wrapper
│   ├── gemini.rs     # Google Gemini CLI wrapper
│   └── bedrock.rs    # AWS Bedrock (optional feature)
├── tasks/            # Built-in task implementations
│   ├── mod.rs        # Task registry
│   ├── hunt.rs       # Bug hunting (errors, perf, dead-code)
│   ├── audit.rs      # Security audit (injection, auth, etc.)
│   ├── fix.rs        # Issue fixing workflow
│   └── ci.rs         # CI integration
├── workflow.rs       # TOML workflow parser and executor
├── conductor.rs      # Multi-agent orchestration mode
├── spawn.rs          # Parallel agent spawning
├── debate.rs         # Agent debate/consensus logic
├── delegation.rs     # Task-to-backend routing
├── context.rs        # Codebase context detection (language, framework)
├── cache.rs          # Response caching
├── output.rs         # Terminal output formatting
└── utils.rs          # Shared utilities
```

## Key Files

- **`src/backend/mod.rs`**: The `Backend` trait that all providers implement
- **`src/workflow.rs`**: Workflow execution engine with variable interpolation
- **`src/config.rs`**: Config schema and defaults
- **`src/tasks/hunt.rs`**: Good example of a multi-prompt task

## Build & Test

This project uses NixOS. Always run commands via nix-shell:

```bash
# From the lok directory
cd ~/dev/lok
nix-shell --run "cargo build"
nix-shell --run "cargo test"
nix-shell --run "cargo clippy"

# Run lok (must specify --bin lok due to multiple binaries)
nix-shell --run "cargo run --bin lok -- doctor"
nix-shell --run "cargo run --bin lok -- hunt /path/to/repo"

# Run lok from another directory (e.g., analyzing discourse)
cd ~/discourse/discourse
nix-shell ~/dev/lok/shell.nix --run "cargo run --manifest-path ~/dev/lok/Cargo.toml --bin lok -- ci 123"
```

For Bedrock support: `cargo build --features bedrock`

## Coding Conventions

### Error Handling
- Use `anyhow::Result` for fallible functions
- Use `thiserror` for custom error types in `src/workflow.rs`
- Prefer `context()` over `unwrap()` for better error messages
- Log warnings with `eprintln!` using colored output

### Async
- All backend queries are async (`async fn query()`)
- Use `tokio` runtime, `futures::join_all` for parallelism
- CLI commands block on async with `#[tokio::main]`

### Output
- Use `colored` crate for terminal colors
- Keep output concise - users pipe this to files
- Use `output.rs` helpers for consistent formatting

### Secrets
- API keys wrapped in `secrecy::SecretString`
- Never log or display secrets
- Use `expose_secret()` only when sending to API

## Config Format

Config lives at `~/.config/lok/config.toml`. Don't break backwards compatibility
with existing config files. Add new fields with defaults.

```toml
[backends.claude]
enabled = true
model = "claude-sonnet-4-20250514"

[backends.codex]
enabled = true
command = "codex"
args = ["exec", "--json"]

[conductor]
max_rounds = 5
max_tokens = 8192
```

## Workflow Format

Workflows are TOML files in `.lok/workflows/`. Key features:
- Steps run sequentially by default
- `parallel = ["step1", "step2"]` for concurrent execution
- Variable interpolation: `{{ steps.previous.output }}`
- `verify = "true"` runs format + lint after step

Don't change the interpolation syntax (`{{ }}`). Brace escaping is handled
internally to prevent LLM outputs from being misinterpreted as variables.

## Adding a New Backend

1. Create `src/backend/newbackend.rs`
2. Implement the `Backend` trait
3. Add to factory in `src/backend/mod.rs`
4. Add config section to `src/config.rs`
5. Update README with setup instructions

## Adding a New Task

1. Create `src/tasks/newtask.rs`
2. Add to task registry in `src/tasks/mod.rs`
3. Define prompts in config or hardcode
4. Follow `hunt.rs` as a template

## Things to Avoid

- Don't add dependencies without good reason
- Don't change CLI argument names (breaks scripts)
- Don't remove config fields (breaks existing configs)
- Don't use `unwrap()` on user input or network responses
- Don't print secrets or API keys

## Multi-Agent Token Conservation

When orchestrating multiple backends, be conservative with tokens. Every query
costs money and time.

**Core principle:** If you fire off multiple agents, they must debate, compare,
or synthesize their outputs. Never query multiple backends just to pick one
response and discard the rest.

**Good patterns:**
- Query backends in parallel for different perspectives, then synthesize
- Have agents debate/argue to reach consensus
- Use one backend's output as input to another (pipeline)
- Fail fast with `continue_on_error` + `min_deps_success` for resilience

**Bad patterns:**
- Query 4 backends, use only 1 response
- Redundant queries asking the same thing multiple ways
- Sequential queries that could be parallel
- Retrying failed backends without backoff

**Example: The spec command**
```
Step 1: Query all backends for roadmaps (parallel)
Step 2: Synthesize roadmaps into consensus
Step 3: Generate specs from consensus
```

This ensures every backend's contribution is used, not wasted.

## Testing Locally

All commands require nix-shell (see Build & Test section above):

```bash
# Quick smoke test
nix-shell --run "cargo run --bin lok -- doctor"

# Test hunt on a repo
nix-shell --run "cargo run --bin lok -- hunt /path/to/repo"

# Test a workflow
nix-shell --run "cargo run --bin lok -- run fix 123"
```
