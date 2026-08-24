# wut design

`wut` is a small, read-only multiplexer for coding-agent CLIs. It owns orchestration, local history, and a stable command line. Each agent continues to own authentication, model access, and its native conversation.

## Product contract

- `wut QUESTION...` asks once and streams the answer to stdout.
- `wut` starts a plain line-oriented session; Ctrl-D exits.
- `wut -c [QUESTION...]` resumes the latest session in the current directory.
- `wut --session ID [QUESTION...]` resumes an explicit local session.
- `--agent`, `--model`, and `--reasoning` override defaults for a new request.
- `wut agents`, `wut models`, `wut sessions`, and `wut config` are scriptable and support JSON where useful.
- Answers use stdout. Prompts, progress, and errors use stderr.
- Every built-in agent is constrained with its strongest native read-only mode.
- Authentication and agent installation are always out of scope.

## Architecture

The binary is a thin entry point over seven modules:

- `cli`: zero-dependency argument parsing into typed commands.
- `app`: command orchestration and the plain interactive loop.
- `agent`: static registry and command construction for Codex, Claude Code, Cursor, Grok, OpenCode, and Pi.
- `protocol`: one streaming subprocess runner plus compact JSONL protocol decoders.
- `config`: serde-backed defaults with explicit CLI overrides.
- `session`: private local transcripts and native agent session IDs.
- `store`: atomic private-file writes and XDG paths.

There is no custom TUI, Markdown renderer, spinner, self-updater, or network client. Model IDs are opaque strings; `wut` does not duplicate provider catalogs.

## Data and compatibility

Canonical paths:

- config: `$WUT_CONFIG` or `$XDG_CONFIG_HOME/wut/config.json`
- sessions: `$WUT_STATE_DIR/sessions` or `$XDG_STATE_HOME/wut/sessions`

Canonical binary overrides are `WUT_<AGENT>_BIN`. If no `wut` config or sessions exist, version-2 `ask` data is read as a one-way compatibility source and written back only under `wut` paths. `ASK_*` binary overrides remain read aliases for one release.

## Safety invariants

- Cursor: `--mode ask`
- Grok and Claude Code: `--permission-mode plan`
- Codex: `--sandbox read-only`
- Pi: allow only `read,grep,find,ls`
- OpenCode: deny all permissions, then allow workspace reads and discovery; deny external directories
- No provider subprocess inherits writable control through a `wut` option.
- Read-only is a mutation boundary, not a confidentiality boundary: agents may read and transmit workspace files.

## Efficiency targets

Relative to inherited `ask` v0.2.1:

- fewer than 3,500 Rust source lines (baseline: 6,783)
- no terminal UI dependency
- no update/network subsystem
- one subprocess/event loop instead of duplicated loops
- release binary no larger than the 721,648-byte baseline
- all behavior and safety invariants covered by deterministic tests

Verified on Linux with the release profile:

| Metric | Inherited baseline | `wut` 0.1.0 | Reduction |
| --- | ---: | ---: | ---: |
| Rust source lines | 6,783 | 2,329 | 65.7% |
| Release binary | 721,648 bytes | 537,136 bytes | 25.6% |
| Cargo dependency nodes | 34 | 17 | 50.0% |
