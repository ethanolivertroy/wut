# wut

A tiny, fast terminal assistant powered by Cerebras. Ask a quick question, recall
a command, or let it inspect your current workspace. It cannot edit files or run
commands.

Requires a [Cerebras API key](https://cloud.cerebras.ai/).

## Install

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/ethanolivertroy/wut/main/install.sh | sh
```

The installer can save your API key securely, or you can provide it through
`CEREBRAS_API_KEY`.

Set `EXA_API_KEY` to optionally give wut fast web search. Without it, wut runs
normally with no Exa dependency.

## Use

```sh
wut "why is this broken?" # ask
wut                       # chat
wut -c                    # continue
wut --sessions            # history
wut --settings            # configure
```
