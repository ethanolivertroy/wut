# wut

A tiny, read-only coding agent powered by Cerebras. It can inspect and search your
workspace, but it cannot edit files or run commands.

Requires a [Cerebras API key](https://cloud.cerebras.ai/).

## Install

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/ethanolivertroy/wut/main/install.sh | sh
```

The installer can save your API key securely, or you can provide it through
`CEREBRAS_API_KEY`.

## Use

```sh
wut "why is this broken?" # ask
wut                       # chat
wut -c                    # continue
wut --sessions            # history
wut --settings            # configure
```
