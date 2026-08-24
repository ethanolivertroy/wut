# wut

A fast question router for the coding agents already on your machine. Read-only on purpose.

```console
$ wut why is this service restarting
Check the container's last exit status and memory limit:

  docker inspect -f '{{.State.ExitCode}} {{.HostConfig.Memory}}' SERVICE

$ wut -c it exited 137
Exit 137 means SIGKILL, most often an out-of-memory kill.
```

`wut` is not another agent. It gives Codex, Claude Code, Cursor, Grok, OpenCode, and Pi one stable interface for questions, streaming output, and resumable local history. The selected agent still owns authentication and its native conversation.

## Why

Coding agents are useful even when you do not want them coding. `wut` constrains each CLI with its strongest native read-only mode, sends one question, prints the answer, records the native session ID, and gets out of the way.

No custom TUI. No self-updater. No provider API keys. No background service.

## Install

The repository is currently private and source-first:

```sh
git clone git@github.com:ethanolivertroy/wut.git
cd wut
./install.sh
```

The installer runs `cargo install --locked` and, when zsh is your login shell, manages a small
shell integration so punctuation such as a trailing `?` reaches `wut` literally. It backs up an
existing `.zshrc` once and adds one idempotent source block; no manually maintained alias is needed.

Requires Rust 1.88 or newer and at least one supported agent CLI that is already installed and authenticated. A binary-only `cargo install --path wut --locked` remains supported, but cannot change zsh expansion rules.

## Usage

```text
wut [OPTIONS] [QUESTION...]                  ask once
wut                                           start a plain session
wut -c [QUESTION...]                         continue the latest session here
wut --session ID [QUESTION...]               continue a specific wut session
wut agents [--json]                          inspect installed agents
wut models [AGENT]                           ask an agent for its model list
wut sessions [--json]                        list resumable sessions
wut config [show [--json] | path]            inspect configuration
wut config set KEY VALUE                     change a default
```

Per-request overrides:

```sh
wut --agent cursor --model MODEL_ID 'explain this crate'
wut --agent grok --reasoning high 'review this architecture'
wut --agent codex --reasoning low 'where is auth initialized?'
```

Answers go to stdout. Prompts and errors go to stderr, so one-shot output is safe to pipe.

Multi-word questions do not need quotes. The managed zsh integration installed by `./install.sh`
disables filename generation only for `wut`, so this works unchanged:

```sh
wut what is kubernetes?
```

Options belong before the first question word. Once the question starts, dash-prefixed words such
as `-O2` are prompt text. Use `--` only when the question itself starts with a dash. Other shell
metacharacters retain their normal zsh behavior.

## Configuration

`wut` uses `$WUT_CONFIG`, then `$XDG_CONFIG_HOME/wut/config.json`, then `~/.config/wut/config.json`.
With no config, it uses Cursor through the unambiguous `cursor-agent` executable.

```sh
wut config set agent cursor
wut config set cursor.model MODEL_ID
wut config set grok.reasoning high
wut config set instructions concise
wut config set instructions none
wut config show --json
```

Use `default` or `none` to clear a model or reasoning override. Quote custom instructions containing spaces.

Agent executable overrides are explicit and vendor-specific:

```sh
WUT_CURSOR_BIN=/path/to/cursor-agent wut 'question'
WUT_GROK_BIN=/path/to/grok wut 'question'
```

The same pattern works for `CODEX`, `CLAUDE`, `PI`, and `OPENCODE`. `wut` reads only
`WUT_*` configuration, state, and executable overrides. Existing `ask` files and variables
cannot silently change which provider `wut` launches.

## Safety model

| Agent | Default binary | Enforced mode |
| --- | --- | --- |
| Cursor | `cursor-agent` | `--mode ask` |
| Grok | `grok` | `--permission-mode plan` |
| Codex | `codex` | `--sandbox read-only` |
| Claude Code | `claude` | `--permission-mode plan` |
| Pi | `pi` | only `read,grep,find,ls` |
| OpenCode | `opencode` | deny by default; workspace reads/discovery only; external directories denied |

`wut` never installs or authenticates an agent. Read-only is a **mutation boundary**, not a confidentiality boundary: any provider CLI may read and transmit workspace files to its configured remote model. OpenCode also denies external-directory access and `.env` reads, but no filename policy can identify every secret. Do not run an agent in a workspace containing data that provider must not receive.

Sessions are private JSON files under `$WUT_STATE_DIR/sessions`, `$XDG_STATE_HOME/wut/sessions`, or `~/.local/state/wut/sessions`. Directories are mode `0700`; files are mode `0600` and replaced atomically.

## Architecture

The implementation is intentionally boring:

- manual typed CLI parser
- static agent registry
- small command builders
- one subprocess and JSONL decoder loop
- serde-backed config and session files
- standard input/output interactive mode

See [DESIGN.md](DESIGN.md) for the behavior contract and efficiency targets.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

## License and provenance

MIT licensed. `wut` began as a clean architectural rewrite of Benjamin Akar's MIT-licensed [`ask`](https://github.com/benja/ask); the original copyright notice remains in [LICENSE](LICENSE).
