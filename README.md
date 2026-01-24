# Council

Multi-LLM orchestration tool for code analysis. Run queries across multiple LLM
backends in parallel, aggregate results, and execute predefined analysis tasks.

## Installation

```bash
# Build from source
nix-shell --run "cargo build --release"

# Binary will be at target/release/council
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

## Configuration

Create a `council.toml` in your project or `~/.config/council/council.toml`:

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
```

## Backends

Council supports pluggable backends. Currently implemented:

- **codex**: OpenAI Codex CLI (efficient, direct answers)
- **gemini**: Google Gemini CLI (thorough, agentic)

Adding a new backend requires implementing the `Backend` trait:

```rust
#[async_trait]
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    async fn query(&self, prompt: &str, cwd: &Path) -> Result<String>;
    fn is_available(&self) -> bool;
}
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
