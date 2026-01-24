# Lok

Local orchestration layer for coordinating multiple LLM backends through a
single control plane. Lok is the brain that controls the arms you already have.

Features smart delegation (knowing which backend suits which task), multi-round
debates between backends, and team mode for coordinated analysis.

## Why "Lok"?

The name has two meanings:

1. **Swedish/German** - short for "lokomotiv/Lokomotive" (locomotive). The tool
   has a `conduct` command where an AI conductor sends out *trained* models down
   the tracks.

2. **Sanskrit/Hindi** - "lok" (लोक) means "world" or "people", as in "Lok Sabha"
   (People's Assembly). Fits the idea of multiple agents working together as a
   collective.

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
cd ~/dev/lok
nix-shell --run "cargo build --release"

# Binary at target/release/lok
# Or run directly: nix-shell --run "./target/release/lok --help"
```

## Usage

```bash
# Ask all configured backends a question
lok ask "Find N+1 queries in this codebase"

# Ask specific backend(s)
lok ask --backend codex "Find dead code"
lok ask --backend codex,gemini "Review this code"

# Smart mode: auto-select best backend for the task
lok smart "Find N+1 queries"      # Uses codex (good for patterns)
lok smart "Security audit"         # Uses gemini (good for deep analysis)

# Suggest which backend to use without running
lok suggest "Find SQL injection vulnerabilities"

# Team mode: smart delegation with optional debate
lok team "Analyze this codebase for issues"
lok team --debate "Should we use async here?"

# Debate mode: multi-round discussion between backends
lok debate "What's the best way to handle auth?"

# Run predefined tasks
lok hunt .     # Bug hunt (N+1, dead code)
lok audit .    # Security audit

# List available backends
lok backends

# Check what's installed and ready
lok doctor

# Initialize config file
lok init
```

Note: Codex requires being in a git repo it trusts. Run from your project
directory.

## Example Output

```
$ lok hunt .

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

Lok works without config (uses defaults). For customization, create
`lok.toml` in your project or `~/.config/lok/lok.toml`:

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

## Smart Delegation

Lok knows which backend suits which task:

| Task Type | Best Backend | Why |
|-----------|--------------|-----|
| N+1 queries, code smells | codex | Efficient, direct pattern matching |
| Dead code, cleanup | codex | Quick analysis, no investigation needed |
| Security audit | gemini | Thorough, investigative, goes deep |
| Architecture review | gemini | Multi-step analysis, considers tradeoffs |

Use `lok suggest "your task"` to see recommendations without running.

## Backends

Lok wraps existing LLM CLIs as pluggable backends:

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

The `lok conduct` command runs Claude as an orchestrating agent that
delegates to other backends. Currently requires Claude API setup (WIP to
simplify to CLI wrapper).

```bash
lok conduct "Find and fix the most impactful performance issues"
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
