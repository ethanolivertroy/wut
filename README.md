# wut

A tiny, fast terminal assistant powered by Cerebras. Ask a quick question, recall
a command, or let it inspect your current workspace. It cannot edit files or run
commands.



https://github.com/user-attachments/assets/2c1c2660-7630-4688-80a2-8beebeacf2c0



Requires a [Cerebras API key](https://cloud.cerebras.ai/).

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

If you installed from the old `ethanolivertroy/tap`, switch once:
`brew uninstall wut && brew untap ethanolivertroy/tap`, then run the two
commands above.

The installer works too, and it can save your API key for you:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/ethanolivertroy/wut/main/install.sh | sh
```

Package managers do not save an API key: export `CEREBRAS_API_KEY`, or write the
key to `~/.config/wut/credentials` (mode 600).

Set `EXA_API_KEY` to optionally give wut fast web search. Without it, wut runs
normally with no Exa dependency. Once a question has read file contents, wut
refuses web searches for the rest of that question, so a search query can't
carry your files off the machine.

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
