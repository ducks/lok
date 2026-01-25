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

# Ollama (local LLMs)
# Install from https://ollama.ai then:
ollama pull llama3.2
```

## Installation

### From crates.io

```bash
cargo install lok
```

### From source

```bash
git clone https://github.com/ducks/lok
cd lok
cargo build --release

# Binary at target/release/lok
```

### With Nix

```bash
cd ~/dev/lok
nix-shell --run "cargo build --release"
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

# Spawn mode: parallel agents working on subtasks
lok spawn "Build a todo app with frontend and backend"
lok spawn "task" --agent "api:Build REST endpoints" --agent "ui:Build React components"

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

[backends.gemini]
enabled = true
command = "npx"
args = ["@google/gemini-cli"]
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
| ollama | HTTP API | Local LLMs, private, no API costs |
| codex | `codex` | Efficient, direct answers, good for patterns |
| gemini | `npx @google/gemini-cli` | Thorough, investigative, goes deep |
| claude | `claude` or API | Balanced, good for orchestration |

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

## Claude Backend

Claude can run in two modes:

### CLI Mode (Simple, Recommended)

Uses the `claude` CLI (Claude Code). No API key needed if you're already
authenticated with Claude Code.

```toml
[backends.claude]
enabled = true
command = "claude"
model = "sonnet"  # optional
```

```bash
lok ask --backend claude "Explain this code"
```

### API Mode (For Conductor)

Uses the Anthropic API directly. Required for `lok conduct` which needs
multi-turn tool use.

```toml
[backends.claude]
enabled = true
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"
# Note: no 'command' field = API mode
```

## Ollama Backend

Ollama runs local LLMs via HTTP API. No CLI binary needed, just the Ollama
server running.

```toml
[backends.ollama]
enabled = true
command = "http://localhost:11434"  # Base URL
model = "llama3.2"                   # Default model
```

```bash
# Start Ollama server, then:
lok ask --backend ollama "Explain this function"
```

## Conductor Mode

The `lok conduct` command runs Claude as an orchestrating agent that
delegates to other backends. Requires Claude API mode (set `ANTHROPIC_API_KEY`).

```bash
export ANTHROPIC_API_KEY=sk-...
lok conduct "Find and fix the most impactful performance issues"
```

The conductor will analyze your request, decide which backends to query,
review results, and synthesize a final answer. It can do multiple rounds
of queries if needed.

## Architecture

Lok's orchestration flow, especially in spawn mode:

```
                                ┌─────────────┐
                                │    USER     │
                                │   (task)    │
                                └──────┬──────┘
                                       │
                                       ▼
                ┌──────────────────────────────────────────┐
                │             CONDUCTOR (BRAIN)            │
                │                                          │
                │  ┌────────────────────────────────────┐  │
                │  │          PLANNING PHASE            │  │
                │  │  • Analyze task complexity         │  │
                │  │  • Break into parallel subtasks    │  │
                │  │  • Assign backends via delegator   │  │
                │  └────────────────────────────────────┘  │
                └──────────────────┬───────────────────────┘
                                   │
          ┌────────────────────────┼────────────────────────┐
          │                        │                        │
          ▼                        ▼                        ▼
┌─────────────────┐      ┌─────────────────┐      ┌─────────────────┐
│   AGENT #1      │      │   AGENT #2      │      │   AGENT #3      │
│   "frontend"    │      │   "backend"     │      │   "database"    │
│                 │      │                 │      │                 │
│  ┌───────────┐  │      │  ┌───────────┐  │      │  ┌───────────┐  │
│  │  CODEX    │  │      │  │  GEMINI   │  │      │  │  CODEX    │  │
│  └───────────┘  │      │  └───────────┘  │      │  └───────────┘  │
└────────┬────────┘      └────────┬────────┘      └────────┬────────┘
         │                        │                        │
         │       ═══════ PARALLEL EXECUTION ═══════        │
         │                        │                        │
         └────────────────────────┼────────────────────────┘
                                  │
                                  ▼
                ┌──────────────────────────────────────────┐
                │           SUMMARIZATION PHASE            │
                │                                          │
                │  • Collect all agent outputs             │
                │  • Report success/failure per agent      │
                │  • Aggregate into final summary          │
                └──────────────────────────────────────────┘
                                  │
                                  ▼
                           ┌────────────┐
                           │   RESULT   │
                           └────────────┘
```

The spawn command implements this pattern:
1. **Plan** - Break task into 2-4 parallel subtasks (via Claude API or fallback)
2. **Delegate** - Assign each subtask to the best backend
3. **Execute** - Run all agents in parallel with shared context
4. **Summarize** - Collect results and report status

## Development

```bash
nix-shell              # Enter dev environment
cargo build            # Build
cargo run -- ask "..." # Run
cargo test             # Test
cargo clippy           # Lint
```

## License

MIT
