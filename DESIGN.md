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

Runtime speed is the primary optimization target; binary size is secondary. Changes must preserve the safety and persistence invariants above, avoid speculative dependencies or async machinery, and win a deterministic local benchmark before release.

The session index intentionally deserializes metadata and turn counts without retaining transcript bodies. Listing sessions and choosing a continuation therefore scale with JSON scanning rather than transcript allocation; only the selected session is fully deserialized and validated before provider launch.

Verified on Linux with randomized execution order, isolated state, and deterministic fake Cursor providers. Values are median process runtimes; the session fixture contains 200 sessions and 20.7 MB of transcripts.

| Local path | Pre-optimization | Runtime-first build | Change |
| --- | ---: | ---: | ---: |
| `--help` (500 samples) | 2.714 ms | 2.722 ms | tied (0.3% slower) |
| One-event durable turn (500 samples) | 18.769 ms | 18.797 ms | tied (0.2% slower) |
| 20,000-event streamed turn (50 samples) | 41.536 ms | 37.283 ms | 10.2% faster |
| `sessions --json` (80 samples) | 25.771 ms | 7.319 ms | 71.6% faster |
| Continue latest session (60 samples) | 52.281 ms | 26.304 ms | 49.7% faster |

Static footprint remains below inherited `ask` v0.2.1 even though the release profile now uses `opt-level = 3`:

| Metric | Inherited `ask` | Runtime-first `wut` | Reduction |
| --- | ---: | ---: | ---: |
| Rust source lines | 6,783 | 2,510 | 63.0% |
| Release binary | 721,648 bytes | 671,800 bytes | 6.9% |
| Cargo dependency nodes | 34 | 17 | 50.0% |
