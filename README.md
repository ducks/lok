# Council

Multi-LLM orchestration tool for code analysis. Run queries across multiple LLM
backends in parallel, aggregate results, and execute predefined analysis tasks.

## Prerequisites

You need the LLM CLIs installed and authenticated:

```bash
# OpenAI Codex
npm install -g @openai/codex

# Google Gemini
# (uses npx, no install needed, but needs GOOGLE_API_KEY)
```

## Installation

```bash
cd ~/dev/council
nix-shell --run "cargo build --release"

# Binary at target/release/council
# Or run directly: nix-shell --run "./target/release/council --help"
```

## Usage

```bash
# Ask all configured backends a question
council ask "Find N+1 queries in this codebase"

# Ask specific backend(s)
council ask --backend codex "Find dead code"
council ask --backend codex,gemini "Review this code"

# Run predefined tasks
council hunt .     # Bug hunt (N+1, dead code)
council audit .    # Security audit

# List available backends
council backends

# Initialize config file
council init
```

Note: Codex requires being in a git repo it trusts. Run from your project
directory.

## Example Output

```
$ council hunt .

Task: hunt
Find bugs and code issues
==================================================

[n+1]

=== CODEX ===

- app/controllers/admin/notes_controller.rb:22 - missing .includes(:notable)
- app/controllers/admin/favorites_controller.rb:8 - missing includes for associations
...

[dead-code]

=== CODEX ===

- Gemfile:311 - TODO says remove ed25519 gem
- app/jobs/scheduled/heartbeat.rb:3 - deprecated, cleanup only
...
```

## Configuration

Council works without config (uses defaults). For customization, create
`council.toml` in your project or `~/.config/council/council.toml`:

```toml
[defaults]
parallel = true
timeout = 300

[backends.codex]
enabled = true
command = "codex"
args = ["exec", "--json", "-s", "read-only"]
parse = "json"

[backends.gemini]
enabled = true
command = "npx"
args = ["@google/gemini-cli"]
parse = "raw"
skip_lines = 1

[tasks.hunt]
description = "Find bugs and code issues"
backends = ["codex"]
prompts = [
  { name = "n+1", prompt = "Search for N+1 query issues..." },
  { name = "dead-code", prompt = "Find unused code..." },
]

[tasks.audit]
description = "Security audit"
backends = ["gemini"]
prompts = [
  { name = "injection", prompt = "Find SQL injection..." },
  { name = "auth", prompt = "Find auth bypass..." },
]
```

## Backends

Council wraps existing LLM CLIs as pluggable backends:

| Backend | CLI | Strengths |
|---------|-----|-----------|
| codex | `codex` | Efficient, direct answers, good for patterns |
| gemini | `npx @google/gemini-cli` | Thorough, investigative, goes deep |

Adding a new backend requires implementing the `Backend` trait in
`src/backend/`:

```rust
#[async_trait]
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    async fn query(&self, prompt: &str, cwd: &Path) -> Result<String>;
    fn is_available(&self) -> bool;
}
```

## WIP: Conductor Mode

The `council conduct` command runs Claude as an orchestrating agent that
delegates to other backends. Currently requires Claude API setup (WIP to
simplify to CLI wrapper).

```bash
council conduct "Find and fix the most impactful performance issues"
```

## Development

```bash
nix-shell              # Enter dev environment
cargo build            # Build
cargo run -- ask "..." # Run
cargo clippy           # Lint
```

## License

MIT
