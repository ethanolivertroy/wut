# wut

wut answers questions, recalls commands, and reads your workspace from the
terminal. It uses Cerebras, Groq, Vercel AI Gateway, or an optional
[Tinfoil local proxy](#tinfoil-local-proxy) and cannot edit files or run commands.

https://github.com/user-attachments/assets/2c1c2660-7630-4688-80a2-8beebeacf2c0

## Install

Install with Cargo or npm:

```sh
cargo install wut            # Rust 1.88 or newer
npm install --global wut-cli # Node 18 or newer
```

Homebrew installs a prebuilt binary on macOS or Linux:

```sh
brew tap ethanolivertroy/wut https://github.com/ethanolivertroy/wut
brew install ethanolivertroy/wut/wut
```

Or use the shell installer, which can save your Cerebras API key:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/ethanolivertroy/wut/main/install.sh | sh
```

## Set up an API key

`--set-key` requires **wut v0.0.6 or newer**. If you see
`unknown option '--set-key'`, check the binary your shell is running:

```sh
wut --version
type -a wut
```

Update with the same method you used to install wut (choose one):

```sh
cargo install wut --locked --force
npm install --global wut-cli@latest
brew update && brew upgrade ethanolivertroy/wut/wut
```

For the shell installer, rerun the command in [Install](#install). Then run
`hash -r` or open a new terminal, and check `wut --version` again. If `type -a wut`
lists multiple copies, an older copy earlier in `PATH` may hide the updated one.
The older message recommending only `CEREBRAS_API_KEY` also indicates an old binary.

If Cargo reports a successful install but `wut` still behaves like an older
version, try `~/.cargo/bin/wut --version` and
`~/.cargo/bin/wut --set-key cerebras` directly. If those work, put
`export PATH="$HOME/.cargo/bin:$PATH"` in your shell configuration and open a
new terminal. `wut use v0.0.6` asks a question; it does not switch versions.
If `type -a wut` shows an alias or shell function, update or remove that override.

Get a key for the provider you want to use:
[Cerebras](https://cloud.cerebras.ai/), [Groq](https://console.groq.com/keys), or
[Vercel AI Gateway](https://vercel.com/ai-gateway). Save it with one of these
commands:

```sh
wut --set-key cerebras
wut --set-key groq
wut --set-key ai-gateway
```

wut hides your input as you type. Enter the key at the prompt; never put the key
itself in the command. Run `wut --set-key` to choose a provider, or select
**API key** in `wut --settings` to set the current model's provider key.

You need only the key for your selected provider. Cargo, npm, and Homebrew do
not save a key automatically. The shell installer can prompt for a Cerebras key.
See [Saved credentials](#saved-credentials) for storage details.

To keep your key in the process environment instead, set `CEREBRAS_API_KEY`,
`GROQ_API_KEY`, or `AI_GATEWAY_API_KEY`. Each provider uses its own credentials;
environment credentials take priority over saved credentials for that provider.
For Tinfoil keys and the required proxy process, see [Tinfoil local proxy](#tinfoil-local-proxy).

## Use

```sh
wut "why is this broken?" # ask a question
cat log | wut "why?"     # ask about piped input
wut                      # start a chat
wut -c                   # continue a chat
wut --sessions           # browse history
wut --settings           # configure defaults
```

In a chat, press Ctrl-C to stop the current answer. Press Ctrl-D, or Ctrl-C on an
empty line, to exit.

wut displays paragraphs as they stream and renders code blocks and diagrams
when each block is complete.

### Choose a model

Choose a default model in `wut --settings`, or pass a model ID with your question:

```sh
wut --models
wut --model groq:openai/gpt-oss-20b "how do I find the process on port 3000?"
wut --model gpt-oss-120b --reasoning low "why is this failing?"
```

A model chosen on the command line applies to that chat and leaves your saved
defaults unchanged. When you continue a chat, wut keeps its model unless you
choose another.

Groq offers GPT-OSS 20B and 120B, both with low reasoning by default. Use
`--reasoning medium` or `--reasoning high` for more thinking, or `--reasoning auto`
to let the optional [TypeSafe integration](#automatic-triage-with-typesafe) choose.

Vercel AI Gateway offers DeepSeek V4.1 Flash 1017 Fast and Ultrafast. Run
`wut --set-key ai-gateway` or set `AI_GATEWAY_API_KEY`, then choose a DeepSeek
model in `wut --settings`. Use `wut --models` to list available model IDs.

Cerebras has archived Gemma 4 31B. wut switches existing Gemma defaults and
saved chats to GPT-OSS 120B on Cerebras and replaces the unsupported `none`
reasoning level with `low`. If you request the archived model explicitly, wut
lists current replacements.

Model IDs and protocol options were checked against the
[Groq model catalog](https://console.groq.com/docs/models),
[Groq reasoning guide](https://console.groq.com/docs/reasoning), and
[Cerebras catalog](https://inference-docs.cerebras.ai/models/overview).

### JSON output

Include `Return only JSON` or a similar directive in your question to get raw,
validated JSON, even in a terminal. wut can strip a complete JSON code fence
without changing the enclosed value.

If a draft mixes prose with JSON, wut makes at most one formatting request in
JSON mode, with tools disabled. If validation fails, wut reports an error and
withholds the malformed document. It checks JSON syntax, not the accuracy of
values or compliance with your schema.

### Diagrams

wut uses Mermaid diagrams to explain flows, sequences, and architectures.
In terminals that support the Kitty graphics protocol—Rex, Ghostty, Kitty, and
WezTerm—it renders diagrams inline with [Pele](https://pele.run). When you pipe
output or use another terminal, wut returns Mermaid text.

Set `WUT_IMAGES=0` to turn diagrams off, or `WUT_IMAGES=1` to enable them in a
terminal wut does not recognize.

wut rasterizes diagrams to fit the display width. If an image exceeds the
16 MiB pixel-buffer limit, wut displays Mermaid text instead. It renders vector
shapes and labels but blocks diagram references to local files and embedded
images.

## Optional integrations

### Tinfoil local proxy

**Unreleased:** this provider requires a build from the current source
(`cargo install --path . --locked --force` from a checkout). Published v0.0.6
does not include Tinfoil support.

Install the official [Tinfoil proxy](https://docs.tinfoil.sh/local-proxy/cli)
separately, then keep it running in another terminal:

```sh
curl -fsSL https://github.com/tinfoilsh/tinfoil-proxy/raw/main/install.sh | sh
tinfoil-proxy
```

Get a [Tinfoil API key](https://docs.tinfoil.sh/get-api-key), save it with hidden
input, and select a Tinfoil model:

```sh
wut --set-key tinfoil
wut --model tinfoil:gpt-oss-120b "how do I find the process on port 3000?"
```

You can also set `TINFOIL_API_KEY`, or choose a Tinfoil model and its key in
`wut --settings`. `wut --models` lists all supported IDs. The catalog includes
GPT OSS 120B, Kimi K3.1 Pro, GLM 5.3, Gemma 4 31B, Llama 3.3 70B, and experimental
DeepSeek V4.1 Flash. wut defaults to low reasoning on GPT OSS, Kimi, and GLM,
and `none` on the others; `--reasoning` accepts each model's supported levels.
See Tinfoil's [chat catalog](https://docs.tinfoil.sh/models/chat) and
[reasoning guide](https://docs.tinfoil.sh/guides/reasoning).

wut connects to `http://127.0.0.1:3301/v1`. For a custom port, run
`tinfoil-proxy -p 9443` and set `TINFOIL_PROXY_PORT=9443` for wut. Only the port
is configurable; the address stays on loopback. wut bypasses environment HTTP
proxies and refuses redirects for these requests. If the local proxy is down,
wut reports an error; it never falls back to a direct Tinfoil API connection.

The local proxy receives your key and prompts over local HTTP. The official
proxy handles enclave attestation and the protected upstream connection; wut
does not implement attestation itself. See the proxy documentation for current
verification status, including the V3 beta. Only answer-model requests use this
proxy: optional TypeSafe triage and Exa search still contact their own services.

### Web search with Exa

Set `EXA_API_KEY` to enable web search. Once wut reads file contents for a
question, it blocks web searches for the rest of that question to keep those
contents out of search queries. Without the key, wut runs without Exa.

### Automatic triage with TypeSafe

Set `TYPESAFE_API_KEY` to let [TypeSafe](https://docs.typesafe.ai)'s Jev model
assess each question before it reaches the answer model. In one System One request,
Jev decides whether the question needs workspace tools or web search and how
much reasoning it needs.

wut omits tools only when Jev is confident the question does not need them.
If TypeSafe fails or takes more than two seconds, wut proceeds as usual.
Without the key, wut runs without TypeSafe.

To let Jev choose the reasoning level for each question, select `Auto` under
Reasoning in `wut --settings`. The [triage evaluation guide](evals/README.md)
explains how to measure triage on labeled questions.

## Privacy and limits

### Saved credentials

Saved keys are **plaintext, not encrypted or stored in an OS keychain**.
Each provider has a separate credential file:

| Provider | Credential file |
| --- | --- |
| Cerebras | `~/.config/wut/credentials` |
| Groq | `~/.config/wut/groq/credentials` |
| Vercel AI Gateway | `~/.config/wut/ai-gateway/credentials` |
| Tinfoil (unreleased) | `~/.config/wut/tinfoil/credentials` |

If you set `XDG_CONFIG_HOME`, wut uses it in place of `~/.config`. Saving writes
the key atomically with mode `0600` and sets credential directories to `0700`.

wut rejects unsafe permissions or ownership, symlinks, and hard-linked
credential files. On macOS, credential files and directories must also have no
extended ACL entries. Re-entering a key with `--set-key` can replace an owned
regular file and repair overly broad permissions. Loading credentials leaves
existing files unchanged.

### Workspace reads

Workspace tools read only regular text files up to 512 KiB. wut blocks secret
paths such as `.env` and `.ssh`, even when a symlink makes the file or directory
appear under another name.

Searches have these limits:

| Search | Limit |
| --- | --- |
| Glob pattern for `find` or the `grep` file filter | 512 bytes |
| `grep` results | 200 matching lines |
| Workspace walk or directory listing | 20,000 entries |
| Listing or text result | 32 KiB plus a limit notice |

wut compiles glob patterns once per search. `*` matches within one path
component; repeated wildcards use a non-backtracking matcher. `grep` reads files
as needed and stops when it reaches the result limit.

### Local data and provider responses

Local JSON reads accept regular files, including symlinks to regular files.
They reject pipes and devices without waiting for data.

| Local data | Size limit |
| --- | --- |
| Config | 1 MiB |
| Saved session | 64 MiB per session |
| Update cache | 8 KiB |

If a save exceeds its limit, wut preserves the previous data. If it cannot read
a session, it warns you and skips that session, keeping the rest of your history
accessible.

wut limits provider streams to 1 MiB per line, 16 MiB per response, and 128
tool-call slots. It rejects invalid or over-limit streams without saving a
completed turn. Diagnostics display terminal control characters as visible
escape text.

## Benchmarks

Run three small test requests to measure response latency:

```sh
wut --benchmark --model groq:openai/gpt-oss-20b
```

The benchmark writes JSON lines with `first_text_ms`, `total_ms`, and
`output_chars`. Without `--model`, it tries the catalog and reports missing
credentials or failed requests. Each request has a 15-second deadline.

The benchmark measures direct streaming inference without triage or workspace
tools. First-text latency includes any reasoning before visible text;
provider-advertised token throughput does not measure response latency.

A [live run from zero](evals/inference/2026-11-08-zero.json) records raw samples
and successful checks of answers and workspace reads. Results depend on the
prompt, reasoning level, network, and provider load.
