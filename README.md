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
cargo install lokomotiv      # Package is "lokomotiv", binary is "lok"

lok doctor                   # Check what backends are available
lok ask "Explain this code"  # Query all available backends
lok hunt .                   # Find bugs in current directory
```

Example `lok doctor` output when backends are configured:

```
Checking backends...

  ✓ codex - ready
  ✓ gemini - ready
  ✓ claude - ready

✓ 3 backend(s) ready.
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

### Workflow Resolution

Lok searches for workflows in this order (first match wins):

1. **Project**: `.lok/workflows/{name}.toml`
2. **User**: `~/.config/lok/workflows/{name}.toml`
3. **Embedded**: Built into the lok binary

This means you can override any built-in workflow by creating your own version
at the project or user level.

### Built-in Workflows

Lok ships with these workflows embedded in the binary:

| Workflow | Description |
|----------|-------------|
| `diff` | Review git changes with multiple backends |
| `explain` | Explain codebase structure and architecture |
| `audit` | Security audit with multiple backends |
| `hunt` | Bug hunt with multiple backends |

Run `lok workflow list` to see all available workflows. Built-in workflows show
as "(built-in)", which you can override by creating your own version:

```bash
lok workflow list              # Shows: diff (built-in)
# Create override:
mkdir -p .lok/workflows
lok run diff > /dev/null       # See what it does, then customize:
cat > .lok/workflows/diff.toml << 'EOF'
name = "diff"
description = "My custom diff review"
# ... your custom steps
EOF
lok workflow list              # Now shows: diff (local)
```

### Consensus and Error Handling

For multi-backend steps, you can require consensus and handle partial failures:

```toml
[[steps]]
name = "propose_claude"
backend = "claude"
continue_on_error = true    # Don't fail workflow if this step times out
timeout = 300000            # 5 minute timeout (milliseconds)
prompt = "Propose a fix..."

[[steps]]
name = "propose_codex"
backend = "codex"
continue_on_error = true
timeout = 300000
prompt = "Propose a fix..."

[[steps]]
name = "debate"
backend = "claude"
depends_on = ["propose_claude", "propose_codex", "propose_gemini"]
min_deps_success = 2        # Need at least 2/3 backends to succeed
prompt = "Synthesize the proposals: {{ steps.propose_claude.output }}..."
```

When `min_deps_success` is set:
- Step runs if at least N dependencies succeeded
- Failed dependencies with `continue_on_error` pass their error output to the prompt
- Logs "consensus reached (2/3 succeeded)" when threshold is met

This prevents wasted tokens when one backend times out or hits rate limits.

### Hard vs Soft Failures

Steps fail in two ways:

- **Hard failure**: Step fails and workflow stops. This is the default behavior.
- **Soft failure**: Step fails but workflow continues. Enabled with `continue_on_error = true`.

When a soft failure occurs, the error message is passed to dependent steps instead
of the normal output. This lets downstream steps handle the failure gracefully:

```toml
[[steps]]
name = "risky_step"
backend = "gemini"
continue_on_error = true   # Soft failure - workflow continues
prompt = "..."

[[steps]]
name = "handler"
depends_on = ["risky_step"]
prompt = """
{% if "error" in steps.risky_step.output %}
Handle the error: {{ steps.risky_step.output }}
{% else %}
Process result: {{ steps.risky_step.output }}
{% endif %}
"""
```

### Retries

Steps can retry on transient failures with exponential backoff:

```toml
[[steps]]
name = "flaky_backend"
backend = "gemini"
retries = 3              # Retry up to 3 times (default: 0)
retry_delay = 2000       # Start with 2 second delay (default: 1000ms)
prompt = "..."
```

The delay doubles after each retry: 2s, 4s, 8s. Retries help with rate limits
and temporary network issues. After all retries are exhausted, the step fails
normally (hard or soft depending on `continue_on_error`).

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

**How `apply_edits` works:**

1. Parses JSON from LLM output looking for `{"edits": [...]}`
2. For each edit, finds `old` text in `file` and replaces with `new`
3. If `verify` is set, runs the command after edits
4. If verification fails, the step fails (edits remain applied)

**Risks and failure modes:**

- **File not found**: Edit fails if the target file doesn't exist
- **Text not found**: Edit fails if `old` text isn't in the file
- **Ambiguous match**: Edit fails if `old` text appears multiple times
- **No rollback**: Failed edits are not automatically reverted
- **Partial application**: If edit 3 of 5 fails, edits 1-2 remain applied

**Recommendations:**

- Use version control (commit before running agentic workflows)
- Start with `verify` commands to catch bad edits early
- Review LLM output before running with `--apply` in production
- Keep `old` text specific enough to match exactly once

### Structured Output

Workflows can produce JSON output for programmatic consumption. Use the
`output_format` field to control how LLM responses are parsed:

```toml
[[steps]]
name = "analyze"
backend = "codex"
output_format = "json"    # Parse output as JSON
prompt = "Return findings as JSON array..."
```

Output format options:
- `text` (default): Raw text output
- `json`: Parse as JSON object
- `json_array`: Parse as JSON array
- `jsonl`: Parse as newline-delimited JSON

Downstream steps can access parsed fields:

```toml
[[steps]]
name = "report"
depends_on = ["analyze"]
prompt = "Summarize: {{ steps.analyze.output.findings }}"
```

## Configuration

Works without config. For customization, create `lok.toml` or
`~/.config/lok/lok.toml`:

```toml
[defaults]
parallel = true
timeout = 300
# Wrap shell commands for isolated environments (NixOS, Docker)
# command_wrapper = "nix-shell --run '{cmd}'"
# command_wrapper = "docker exec dev sh -c '{cmd}'"

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

### Command Wrapper (NixOS/Docker)

If you use isolated environments, shell commands in workflows may fail due to
missing dependencies. Use `command_wrapper` to wrap all shell commands:

```toml
[defaults]
# For NixOS with nix-shell
command_wrapper = "nix-shell --run '{cmd}'"

# For Docker
command_wrapper = "docker exec dev sh -c '{cmd}'"

# For direnv
command_wrapper = "direnv exec . {cmd}"
```

The `{cmd}` placeholder is replaced with the actual command.

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
