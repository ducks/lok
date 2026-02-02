# Agent History

This branch contains the decision history for AI agent sessions.

Each session is stored as a series of commits capturing:
- Intent: What the agent was trying to accomplish
- Checkpoints: Snapshots of decisions made
- Reasoning: Why each decision was made

This history is separate from the main code history but linked
via commit references.

## Structure

- `sessions/` - Session metadata and intent records
- Commits on this branch represent checkpoints

## Usage

This branch is managed by `lok` and should not be edited manually.
Use `lok report` to generate human-readable summaries.
