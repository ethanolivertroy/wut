<div align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/light-banner.png">
    <source media="(prefers-color-scheme: light)" srcset="assets/dark-banner.png">
    <img alt="wut" src="assets/dark-banner.png" width="320">
  </picture>
</div>

<div align="center">
  Ask a coding agent a question. Get an answer, not a takeover.
</div>

<br>

## What it feels like

Ask a question and get back to your shell:

```console
$ wut how do i find TODO or FIXME comments in src
Use ripgrep:

  rg 'TODO|FIXME' src
```

Follow up in the same agent session:

```console
$ wut -c how do i exclude generated files
Add an exclude:

  rg 'TODO|FIXME' src -g '!generated/**'
```

Or run `wut` to start an interactive session:

```console
$ wut
> why is this container restarting?

Check the last termination reason:

  kubectl get pod <pod> -o jsonpath='{.status.containerStatuses[0].lastState}'

> /settings
```

Every successful answer is saved. Use `wut -c` to continue the latest session in the current folder.

## Why

`wut` is a normal terminal command for asking AI questions through whichever coding agent or model you already use. There is no TUI to live inside: ask something, read the answer, and get your shell back.

Coding agents can do a lot in the background, and sometimes that is exactly what you do not want. `wut` starts the selected provider with its native read-only or planning controls. You run commands and make changes yourself.

## Install

This repository is private, so install from an authenticated checkout:

```sh
git clone git@github.com:ethanolivertroy/wut.git
cd wut
./install.sh
```

Running `./install.sh` from the checkout performs a locked release build, installs `wut` to `~/.local/bin/wut`, and configures managed zsh punctuation support when zsh is your login shell. Set `WUT_INSTALL_DIR` to change the binary destination.

The zsh integration is installer-owned and idempotent. It lets literal questions such as `wut what is kubernetes?` reach the executable without disabling zsh `NOMATCH` globally. You do not need to maintain an alias yourself.

Tagged releases use checksum-verified archives for Intel/ARM Macs and x86_64/ARM64 Linux.

## Usage

```text
wut [QUESTION...]          ask once
wut                        start a session
wut -c [QUESTION...]       continue the latest session here
wut --sessions             reopen a saved session
wut --settings             set defaults for new sessions
wut --upgrade              update wut from a tagged release
wut -V                     print the version
```

Use `wut -- -why did this fail` when the question itself begins with a dash. After the first question word, dash-prefixed terms are treated as question text, so `wut compare -O2 and -O3` works normally.

## Updates

`wut` checks for updates at most once a day in a detached helper process. A newly discovered tagged release is announced after a later successful run, without holding the current command open. Set `WUT_NO_UPDATE_CHECK=1` to disable the check. Updates happen only when you run `wut --upgrade`.

Because this repository is private, release discovery and downloads prefer an authenticated GitHub CLI (`gh auth status`). The public curl/wget path remains available if repository visibility changes.

## Agents

Under the hood, `wut` runs a coding-agent CLI already installed and authenticated on your machine. Fresh installs default to Codex with the `fast` model alias, which prefers GPT-5.3 Codex Spark when available. Run `wut --settings` to choose another default agent, model, reasoning level, or answer style.

| Agent | Command | Read-only control | Continuation |
|---|---|---|---|
| Cursor | `cursor-agent` (falls back to `agent`) | Ask mode (`--mode ask`) | Saved chat IDs |
| Codex | `codex app-server` | Read-only sandbox | Saved thread IDs |
| Claude Code | `claude` | Plan permission mode | Saved session IDs |
| Pi | `pi` | Read-only tools (`read,grep,find,ls`) | Saved session IDs |
| OpenCode | `opencode` | Generated deny-by-default permissions | Saved session IDs |
| Grok | `grok` | Plan permission mode | Saved session IDs |

OpenCode's policy is a mutation boundary, not a complete confidentiality boundary: workspace reads required to answer questions remain available.

Canonical executable overrides are `WUT_CURSOR_BIN`, `WUT_CODEX_BIN`, `WUT_CLAUDE_BIN`, `WUT_PI_BIN`, `WUT_OPENCODE_BIN`, and `WUT_GROK_BIN`.

## State and migration

Canonical locations follow XDG conventions:

- config: `${XDG_CONFIG_HOME:-$HOME/.config}/wut/config.json`
- sessions: `${XDG_STATE_HOME:-$HOME/.local/state}/wut/sessions/`
- update cache: `${XDG_CACHE_HOME:-$HOME/.cache}/wut/update.json`

`WUT_CONFIG` overrides the config file. `WUT_STATE_DIR` overrides the state root; sessions are stored in its `sessions/` child for compatibility with wut v0.1/v0.2. New directories are private (`0700`) and new files are private (`0600`). Writes are atomic.

On first use, existing state from the authorized predecessor is imported into canonical `wut` paths without deleting or modifying the source files. Existing `wut` v0.1/v0.2 config and session schemas remain readable. Canonical `WUT_*` settings always win over legacy compatibility aliases.

Failed provider turns do not create or update sessions. Session listings show local metadata, not provider-native continuation IDs or transcript contents.

## Origins

This implementation is derived from code by Benjamin Akar with permission. The authorized import was pinned to source commit `3d1cd5d90603586aeba9ba47612d0c0625a04d3a`; development and releases target the private `ethanolivertroy/wut` repository. The original MIT copyright notice is preserved in [LICENSE](LICENSE).

## License

MIT. See [LICENSE](LICENSE).
