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

Grok also defaults to the `fast` alias. It prefers a Cerebras-hosted model when Grok Build has one (see below), then Grok Code Fast 1, then a `-fast` variant of the current flagship such as `grok-4.6-fast`, then whatever Grok Build reports as its default. OpenCode and Pi offer `fast` too once a Cerebras or Groq provider is connected. Every alias resolves against the live catalog once a day and caches the result under `${XDG_CACHE_HOME:-$HOME/.cache}/wut/`; a cached model that the provider has since retired is re-resolved automatically.

### Cerebras

Cerebras serves open models at 1000+ tokens per second, and `wut` treats any Cerebras-hosted model as the fastest choice for `fast`. It prefers `gpt-oss-120b`, then `gemma-4-31b`, then any other model the provider lists. `wut` never calls Cerebras itself; it reaches it through whichever agent you have connected:

- Codex: GPT-5.3 Codex Spark already runs on Cerebras hardware. Nothing to configure.
- OpenCode: run `opencode`, use `/connect` to add Cerebras with an API key, then choose `fast` in `wut --settings`.
- Pi: export `CEREBRAS_API_KEY`, then choose `fast`.
- Grok Build: point it at Cerebras with `GROK_MODELS_BASE_URL=https://api.cerebras.ai/v1` and `XAI_API_KEY=<cerebras key>`, or add a `[model.<name>]` entry with that `base_url` in `~/.grok/config.toml`. Include "Cerebras" in the entry's display name if you use a custom model id, so `wut` can recognise it.

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

wut was inspired by work from Benjamin Akar, used with permission (import pinned to source commit `3d1cd5d90603586aeba9ba47612d0c0625a04d3a`). It is now an independent project, developed and maintained in the `ethanolivertroy/wut` repository.

## License

MIT. See [LICENSE](LICENSE).
