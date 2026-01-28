# Lok

Orchestration layer for multiple LLM backends.

## The Big Idea

You're already in Claude Code (or Cursor, or Copilot). Instead of switching
tools, let your LLM call other LLMs:

```
You: Find performance issues in this codebase
Claude: [runs: lok ask -b codex "Find N+1 queries"]
Claude: Found 3 issues. Let me verify with another backend...
Claude: [runs: lok ask -b gemini "Review these findings"]
```

Your LLM becomes the conductor. Lok is just the orchestra.

## What Can It Do?

Lok found 25 bugs in its own codebase, then found a real bug in Discourse
(35k stars) that's now a merged PR. Here's how:

```bash
lok hunt .                    # Find bugs with multiple backends
lok hunt --issues -y          # Find bugs AND create GitHub issues

lok fix 123                   # Analyze a GitHub issue
lok ci 123                    # Analyze CI failures
lok pr 123                    # Review a pull request

lok run full-heal             # Autonomous: hunt → fix → PR → review → merge
```

## Installation

```bash
cargo install lokomotiv
```

Prerequisites: Install the LLM CLIs you want to use (codex, gemini, ollama, etc.)

## Quick Start

```bash
lok doctor                    # Check what's working
lok ask "Explain this codebase"
lok hunt .                    # Bug hunt
lok audit .                   # Security audit
```

## All The Modes

### Direct Query

Ask one or more backends:

```bash
lok ask "Find N+1 queries"                    # All backends
lok ask -b codex "Find dead code"             # Specific backend
lok ask -b codex,gemini "Review this code"    # Multiple backends
```

### Debate Mode

Have backends argue and refine answers:

```bash
lok debate "What's the best way to handle auth?"
lok debate --rounds 3 "Should we use async here?"
```

Each backend sees previous responses and can challenge or build on them.

### Spawn Mode

Break a task into parallel subtasks:

```bash
lok spawn "Build a REST API with tests"
```

### Workflows

Define multi-step pipelines in TOML:

```bash
lok run security-review       # Run a workflow
lok workflow list             # List available workflows
```

```toml
[[steps]]
name = "scan"
backend = "codex"
prompt = "Find obvious issues"

[[steps]]
name = "investigate"
backend = "gemini"
depends_on = ["scan"]
prompt = "Investigate: {{ steps.scan.output }}"

[[steps]]
name = "summarize"
backend = "ollama"
depends_on = ["scan", "investigate"]
prompt = "Prioritize findings..."
```

Steps without dependencies run in parallel.

### Agentic Workflows

Workflows can run shell commands and apply code edits:

```toml
[[steps]]
name = "fix"
backend = "claude"
apply_edits = true
verify = "cargo build"
prompt = """
Fix this issue. Output JSON:
{"edits": [{"file": "src/main.rs", "old": "...", "new": "..."}], "summary": "..."}
"""

[[steps]]
name = "commit"
shell = "git add -A && git commit -m '{{ steps.fix.summary }}'"
depends_on = ["fix"]
```

See `examples/workflows/full-heal.toml` for the complete autonomous loop.

### Code Review

```bash
lok diff                      # Review staged changes
lok diff main..HEAD           # Review branch vs main
lok pr 123                    # Review GitHub PR
```

### Conductor Mode

Let an LLM orchestrate everything:

```bash
lok conduct "Find and fix performance issues"
```

### Explain Mode

```bash
lok explain                   # Explain current directory
lok explain --focus auth      # Focus on specific aspect
```

## Backend Strengths

| Backend | Best For | Speed |
|---------|----------|-------|
| codex | Code patterns, N+1, dead code | Fast |
| gemini | Security audits, deep analysis | Slow (thorough) |
| claude | Orchestration, reasoning | Medium |
| ollama | Local/private, no rate limits | Varies |
| bedrock | AWS-native deployments | Medium |

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
  { name = "errors", prompt = "Find error handling issues..." },
]
```

## Commands

```bash
# Querying
lok ask "prompt"              # Query all backends
lok ask -b codex "prompt"     # Specific backend

# Multi-agent modes
lok debate "question"         # Backends debate each other
lok spawn "task"              # Parallel subtask agents
lok conduct "task"            # Fully autonomous

# Code review
lok diff                      # Review staged changes
lok pr 123                    # Review GitHub PR
lok ci 123                    # Analyze CI failures

# Issue analysis
lok fix 123                   # Analyze GitHub issue

# Codebase analysis
lok explain                   # Explain codebase
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

## Why "Lok"?

Swedish/German: Short for "lokomotiv" (locomotive). The conductor sends trained
models down the tracks.

Sanskrit/Hindi: "lok" means "world" or "people", as in "Lok Sabha" (People's
Assembly). Multiple agents working as a collective.

## License

MIT
