# wut

A tiny, fast terminal assistant powered by Cerebras or Groq. Ask a quick question, recall
a command, or let it inspect your current workspace. It cannot edit files or run
commands.



https://github.com/user-attachments/assets/2c1c2660-7630-4688-80a2-8beebeacf2c0



Requires a [Cerebras API key](https://cloud.cerebras.ai/) or a
[Groq API key](https://console.groq.com/keys).

## Install

```sh
cargo install wut                      # anywhere with Rust 1.88 or newer
npm install --global wut-cli           # anywhere with Node 18 or newer
```

With Homebrew on macOS or Linux, which installs the prebuilt binary:

```sh
brew tap ethanolivertroy/wut https://github.com/ethanolivertroy/wut
brew install ethanolivertroy/wut/wut
```

The installer works too, and it can save your API key for you:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/ethanolivertroy/wut/main/install.sh | sh
```

Package managers do not save an API key. Set `CEREBRAS_API_KEY` in the process
environment to avoid saving a key through wut, or use the installer's optional
credential prompt. Saved keys are **plaintext, not encrypted or stored in an OS
keychain**. The installer restricts the `~/.config/wut` directory to mode 700 and
the `credentials` file inside it to mode 600. If `XDG_CONFIG_HOME` is set, wut uses
`$XDG_CONFIG_HOME/wut/credentials` instead. Manually created credentials must have
the same protections: wut refuses files or directories accessible to other users,
symlinks at either of those two paths, files owned by another user, and hard-linked
credential files. Environment credentials take priority over saved credentials.

Provider streams are limited to 1 MiB per line, 16 MiB per response, and 128
tool-call slots. Over-limit or invalid streams fail without saving a completed
turn. Diagnostics show terminal control characters as visible escape text.

For Groq, set `GROQ_API_KEY` in the process environment. Groq never uses your
Cerebras key or its saved credentials file. Choose a model in `wut --settings`,
or pass its ID before a question:

```sh
wut --models
wut --model groq:openai/gpt-oss-20b "how do I find the process on port 3000?"
wut --model gpt-oss-120b --reasoning low "why is this failing?"
```

The Groq choices are GPT-OSS 20B and 120B, both with low reasoning by default.
`--reasoning medium` or `high` spends more time thinking; `auto` uses the optional
TypeSafe integration. Command-line model choices leave your saved defaults
unchanged. A continued chat remembers its model unless you override it.

Cerebras has archived Gemma 4 31B. Existing Gemma defaults and saved chats use
GPT-OSS 120B on Cerebras instead; its unsupported `none` reasoning level becomes
`low`. An explicit request for the archived model reports current replacements.

Use `wut --benchmark --model groq:openai/gpt-oss-20b` to send three small test
requests and emit JSON lines with `first_text_ms`, `total_ms`, and `output_chars`.
Without `--model`, it tries the catalog and reports unavailable credentials or
failed requests explicitly. Each test request has a 15-second deadline. This
measures direct inference with streaming, without triage or workspace tools;
first-text latency includes any model reasoning before visible text. It does
not equate provider-advertised token throughput with response latency.

A [live run from zero](evals/inference/2026-11-08-zero.json) records the raw
samples and successful answer/workspace-read checks. Results depend on the
prompt, reasoning level, network, and provider load.

Model IDs and protocol options were checked against the
[Groq model catalog](https://console.groq.com/docs/models),
[Groq reasoning guide](https://console.groq.com/docs/reasoning), and
[Cerebras catalog](https://inference-docs.cerebras.ai/models/overview).

Set `EXA_API_KEY` to optionally give wut fast web search. Without it, wut runs
normally with no Exa dependency. Once a question has read file contents, wut
refuses web searches for the rest of that question, so a search query can't
carry your files off the machine.

Workspace tools read only regular text files up to 512 KiB. Secret paths such as
`.env` and `.ssh` are blocked even when they are symlinks to ordinary-looking
files or directories.

Glob patterns for `find` and the `grep` file filter are compiled once per search
and limited to 512 bytes. `*` stays within one path component. Repeated wildcards
use a non-backtracking matcher instead of recursively trying every combination.

Set `TYPESAFE_API_KEY` to optionally let [TypeSafe](https://docs.typesafe.ai)'s
Jev model triage each question before it reaches Cerebras. One fast System One
request judges whether the question needs the workspace tools, whether it needs
web search, and how much thinking it calls for. wut drops tools only when Jev
is confident they are unnecessary, and a TypeSafe failure or an answer slower
than two seconds falls back to the normal behavior. To let Jev also choose the
reasoning level per question, pick `Auto` under Reasoning in `wut --settings`.
Without the key, wut runs normally with no TypeSafe dependency.
[`evals/README.md`](evals/README.md) explains how to measure triage on labeled
questions.

## Use

```sh
wut "why is this broken?" # ask
cat log | wut "why?"      # ask about piped input
wut                       # chat
wut -c                    # continue
wut --sessions            # history
wut --settings            # configure
```

In a chat, Ctrl-C stops the current answer; Ctrl-D or Ctrl-C on an empty line
exits.

Ordinary paragraphs appear as they stream, without waiting for a newline.
Code blocks and diagrams retain their complete-block rendering.

Explicit JSON-only directives such as `Return only JSON` select validated raw
output, including in a terminal. A complete JSON code fence can be removed
without changing its value. If a draft mixes prose with JSON, wut makes at most
one tool-free JSON-mode formatting request; if that fails validation, it reports
an error without printing a malformed document. This checks JSON syntax, not the
correctness of its values or adherence to a user-provided schema.

## Diagrams

In terminals that show images through the Kitty graphics protocol (Rex,
Ghostty, Kitty, WezTerm), wut draws diagrams: when a flow, a sequence or an
architecture is easier to see than to read, the answer includes a Mermaid
diagram and wut renders it inline with [Pele](https://pele.run). Piped output
and other terminals get the Mermaid text instead. Set `WUT_IMAGES=0` to turn
diagrams off, or `WUT_IMAGES=1` to turn them on in a terminal wut doesn't
recognize.

Diagrams are rasterized to fit their display width, with a 16 MiB pixel-buffer
limit. Oversized images fall back to Mermaid text. Diagram image references
cannot load local files or embedded images; ordinary vector shapes and labels
continue to render.
