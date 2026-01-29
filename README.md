# Lok

Orchestration layer for multiple LLM backends. Run it in your project directory
and it coordinates multiple LLMs to analyze your code, find bugs, and propose fixes.

## How It Works

An agentic LLM (Claude Code, Gemini CLI, etc.) acts as the conductor, running
lok commands and implementing the results.

```
1. Conductor:      Runs lok run fix 123
2. Lok analyzes:   Fetches issue, queries Claude + Codex, synthesizes proposals
3. Lok comments:   Posts consensus proposal to the GitHub issue
4. Conductor:      Reads the proposal, implements the fix, pushes a PR
5. Human:          Reviews and approves
```

The conductor is the brain (an agentic LLM). Lok is the orchestra (multiple
specialized backends). The human provides oversight.

## Quick Start

```bash
cargo install lokomotiv

lok doctor                    # Check what backends are available
lok ask "Explain this code"   # Query all available backends
lok hunt .                    # Find bugs in current directory
```

## Prerequisites

Lok wraps existing LLM CLI tools. Install the ones you want to use:

| Backend | Install | Notes |
|---------|---------|-------|
| Codex | `npm install -g @openai/codex` | Fast code analysis |
| Gemini | `npm install -g @google/gemini-cli` | Deep security audits |
| Claude | [claude.ai/download](https://claude.ai/download) | Claude Code CLI |
| Ollama | [ollama.ai](https://ollama.ai) | Local models, no API keys |

For issue/PR workflows, you also need:

| Tool | Install | Used by |
|------|---------|---------|
| gh | [cli.github.com](https://cli.github.com) | `lok run fix`, `lok run review-pr` |

Run `lok doctor` to see which backends are detected. Core commands (`lok ask`,
`lok hunt`, `lok audit`) work without `gh`.

## Commands

### Analysis

```bash
lok ask "Find N+1 queries"              # Query all backends
lok ask -b codex "Find dead code"       # Specific backend
lok hunt .                              # Bug hunt (multiple prompts)
lok hunt --issues                       # Bug hunt + create GitHub issues
lok audit .                             # Security audit
lok explain                             # Explain codebase structure
```

### Code Review

```bash
lok diff                                # Review staged changes
lok diff main..HEAD                     # Review branch vs main
lok run review-pr 123                   # Multi-backend PR review + comment
```

### Issue Management

```bash
lok run fix 123                         # Analyze issue, propose fix, comment
lok ci 123                              # Analyze CI failures
```

### Multi-Agent Modes

```bash
lok debate "Should we use async here?"  # Backends argue and refine
lok spawn "Build a REST API"            # Break into parallel subtasks
lok conduct "Find and fix perf issues"  # Fully autonomous
```

### Workflows

```bash
lok run workflow-name                   # Run a workflow
lok workflow list                       # List available workflows
```

### Utilities

```bash
lok doctor                              # Check installation
lok backends                            # List configured backends
lok suggest "task"                      # Suggest best backend for task
lok init                                # Create config file
```

## Workflows

Workflows are TOML files that define multi-step LLM pipelines. Steps can depend
on previous steps and run in parallel when possible.

```toml
# .lok/workflows/example.toml
name = "example"

[[steps]]
name = "scan"
backend = "codex"
prompt = "Find obvious issues in this codebase"

[[steps]]
name = "deep-dive"
backend = "gemini"
depends_on = ["scan"]
prompt = "Investigate these findings: {{ steps.scan.output }}"

[[steps]]
name = "comment"
depends_on = ["deep-dive"]
shell = "gh issue comment 123 --body '{{ steps.deep-dive.output }}'"
```

### Built-in Workflows

| Workflow | Description |
|----------|-------------|
| `fix` | Analyze issue with multiple backends, post proposal |
| `review-pr` | Multi-backend PR review with consensus verdict |
| `full-heal` | Autonomous: hunt bugs, fix, PR, review, merge |

### Agentic Features

Workflows can apply code edits and verify them:

```toml
[[steps]]
name = "fix"
backend = "claude"
apply_edits = true
verify = "cargo build"
prompt = """
Fix this issue. Output JSON:
{"edits": [{"file": "src/main.rs", "old": "...", "new": "..."}]}
"""
```

## Configuration

Works without config. For customization, create `lok.toml` or
`~/.config/lok/lok.toml`:

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

[cache]
enabled = true
ttl_hours = 24
```

## Backend Strengths

| Backend | Best For | Speed |
|---------|----------|-------|
| Codex | Code patterns, N+1, dead code | Fast |
| Gemini | Security audits, deep analysis | Slow (thorough) |
| Claude | Orchestration, reasoning | Medium |
| Ollama | Local/private, no rate limits | Varies |

## Real World Results

Lok found 25 bugs in its own codebase, then found a real bug in Discourse
(35k stars) that became a merged PR.

```bash
lok hunt ~/dev/discourse --issues -y    # Found hardlink limit bug
```

## Why "Lok"?

Swedish/German: Short for "lokomotiv" (locomotive). The conductor sends trained
models down the tracks.

Sanskrit/Hindi: "lok" means "world" or "people", as in "Lok Sabha" (People's
Assembly). Multiple agents working as a collective.

## License

MIT
