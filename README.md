# Lok

Orchestration layer for coordinating multiple LLM backends. Query Codex, Gemini,
Claude, Ollama, and Bedrock through a single interface.

## How It Works

Lok has several modes of operation:

### Direct Query (Simplest)

Ask one or more backends directly:

```bash
lok ask "Find N+1 queries"                    # All backends
lok ask -b codex "Find dead code"             # Specific backend
lok ask -b codex,gemini "Review this code"    # Multiple backends
```

### Debate Mode

Have backends argue and refine their answers through multiple rounds:

```bash
lok debate "What's the best way to handle auth in this app?"
lok debate --rounds 3 "Should we use async here?"
```

Each backend sees previous responses and can challenge or build on them.
Great for architectural decisions where you want multiple perspectives.

### Spawn Mode (Parallel Agents)

Break a task into subtasks and run them in parallel:

```bash
lok spawn "Build a REST API with tests"
lok spawn "task" --agent "api:Build endpoints" --agent "tests:Write test suite"
```

The conductor plans subtasks, delegates to appropriate backends, runs them
in parallel, and synthesizes results. Good for larger tasks that can be
parallelized.

### Workflows (Declarative Pipelines)

Define multi-step pipelines in TOML. Each step can use a different backend and
depend on previous steps:

```bash
lok run security-review    # Run a workflow
lok workflow list          # List available workflows
```

```toml
# Scout -> Tank -> Support pattern
[[steps]]
name = "scan"
backend = "codex"          # Fast initial scan
prompt = "Find obvious issues"

[[steps]]
name = "investigate"
backend = "gemini"         # Deep investigation
depends_on = ["scan"]
prompt = "Investigate: {{ steps.scan.output }}"

[[steps]]
name = "summarize"
backend = "ollama"         # Local synthesis (no rate limits)
depends_on = ["scan", "investigate"]
prompt = "Prioritize findings..."
```

### Diff Review

Review git changes before committing:

```bash
lok diff                    # Review staged changes
lok diff --unstaged         # Review all uncommitted changes
lok diff main..HEAD         # Review branch vs main
lok diff HEAD~3             # Review last 3 commits
```

Catches bugs, security issues, and style problems in your changes before they
land. Great for pre-commit review.

### Conductor Mode (Fully Autonomous)

Let an LLM orchestrate everything automatically:

```bash
lok conduct "Find and fix performance issues"
```

The conductor analyzes your request, decides which backends to query, reviews
results, and synthesizes a final answer. Multiple rounds if needed.

## Recommended: LLM-as-Conductor

The simplest way to use lok is from inside an LLM session. If you're already in
Claude Code or similar, just call lok as a tool:

```
You: Find performance issues in this codebase
Claude: [runs: lok ask -b codex "Find N+1 queries"]
Claude: Found 3 issues. Let me get a second opinion on auth...
Claude: [runs: lok ask -b gemini "Review auth module for performance"]
```

Your LLM handles orchestration naturally. It sees results, reasons about them,
and decides when to query other backends.

## Backend Strengths

| Backend | Best For | Speed |
|---------|----------|-------|
| codex | Code patterns, N+1, dead code | Fast |
| gemini | Security audits, deep analysis | Slow (thorough) |
| claude | Orchestration, reasoning | Medium |
| ollama | Local/private, no rate limits | Varies (CPU/GPU) |
| bedrock | AWS-native deployments | Medium |

Use `lok suggest "your task"` to see which backend fits best.

## Installation

```bash
# From crates.io
cargo install lokomotiv

# From source
git clone https://github.com/ducks/lok
cd lok && cargo build --release
```

Prerequisites: Install the LLM CLIs you want to use (codex, gemini, ollama, etc.)

## Quick Start

```bash
# Initialize config (optional)
lok init

# Check what's working
lok doctor

# Run a query
lok ask "Explain this codebase"

# Run predefined tasks
lok hunt .     # Bug hunt
lok audit .    # Security audit
```

## Configuration

Works without config. For customization: `lok.toml` or `~/.config/lok/lok.toml`

```toml
[defaults]
parallel = true
timeout = 300

[backends.codex]
enabled = true
command = "codex"
args = ["exec", "--json", "-s", "read-only"]

[backends.ollama]
enabled = true
command = "http://localhost:11434"
model = "qwen2.5-coder:7b"

[tasks.hunt]
description = "Find bugs"
backends = ["codex"]
prompts = [
  { name = "n+1", prompt = "Find N+1 queries..." },
]
```

## Workflows

Workflows live in `.lok/workflows/` (project) or `~/.config/lok/workflows/` (global).

Features:
- `{{ steps.NAME.output }}` - interpolate previous output
- `depends_on = ["step1", "step2"]` - execution order
- `when = "..."` - conditional execution

## Commands

```bash
# Querying
lok ask "prompt"              # Query all backends
lok ask -b codex "prompt"     # Specific backend

# Multi-agent modes
lok debate "question"         # Backends debate each other
lok spawn "task"              # Parallel subtask agents
lok conduct "task"            # Fully autonomous orchestration

# Code review
lok diff                      # Review staged changes
lok diff main..HEAD           # Review branch diff

# Predefined tasks
lok hunt .                    # Bug hunt
lok audit .                   # Security audit

# Workflows
lok run workflow-name         # Run a workflow
lok workflow list             # List workflows

# Utilities
lok suggest "task"            # Suggest best backend
lok backends                  # List backends
lok doctor                    # Check installation
lok init                      # Create config file
```

## Architecture

```
                         ┌─────────────┐
                         │  CONDUCTOR  │
                         │  (or user)  │
                         └──────┬──────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        │                       │                       │
        ▼                       ▼                       ▼
┌───────────────┐       ┌───────────────┐       ┌───────────────┐
│    CODEX      │       │    GEMINI     │       │    OLLAMA     │
│    (scout)    │       │    (tank)     │       │   (support)   │
│  Fast scans   │       │  Deep dives   │       │  Synthesis    │
└───────────────┘       └───────────────┘       └───────────────┘
```

Think of it like an RPG party:
- **Scout** (Codex): Fast, finds obvious issues
- **Tank** (Gemini): Thorough, investigates deeply
- **Support** (Ollama): Always available, synthesizes results

## Why "Lok"?

1. **Swedish/German**: Short for "lokomotiv" (locomotive). The conductor sends
   trained models down the tracks.

2. **Sanskrit/Hindi**: "lok" means "world" or "people", as in "Lok Sabha"
   (People's Assembly). Multiple agents working as a collective.

## License

MIT
