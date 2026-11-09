# wut-cli

npm wrapper for [wut](https://github.com/ethanolivertroy/wut) — a tiny, fast
terminal assistant powered by Cerebras, Groq, or Vercel AI Gateway.

```sh
npm install --global wut-cli
wut "why is this container restarting?"
```

The wrapper downloads the prebuilt release binary for your platform
(macOS/Linux, Intel/ARM) on first run, checks it against the SHA-256 published
with the release, and caches it in `~/.cache/wut-npm/<version>/`. Every later
run starts the cached binary directly.

Archive and checksum downloads run in parallel with a shared 60-second deadline.
The archive response is limited to 32 MiB and the checksum to 4 KiB, including
chunked responses. Failed downloads leave an existing cached binary intact.

Requires Node 18 or newer and an API key for the selected provider. Save one
with hidden terminal input:

```sh
wut --set-key cerebras
wut --set-key groq
wut --set-key ai-gateway
```

Run `wut --set-key` to choose a provider, or use **API key** in
`wut --settings`. Never pass the key itself as an argument. Then select a model:

```sh
wut --models
wut --model groq:openai/gpt-oss-20b "why is this container restarting?"
```

Keys are saved by the Wut binary as **plaintext, not encrypted or stored in an
OS keychain**. Saving is atomic, sets the key file to `0600` and credential
directories to `0700`, and can repair broad permissions when replacing an owned
regular file. Loading rejects unsafe permissions or ownership, symlinks, and
hard-linked credential files. On macOS, extended ACL entries are also rejected.

`CEREBRAS_API_KEY`, `GROQ_API_KEY`, and `AI_GATEWAY_API_KEY` in the process
environment override saved keys for their provider. The
[credential documentation](https://github.com/ethanolivertroy/wut#install) lists
the per-provider paths and `$XDG_CONFIG_HOME` support. The npm wrapper itself
does not save credentials.

## Other install methods

```sh
brew tap ethanolivertroy/wut https://github.com/ethanolivertroy/wut
brew install ethanolivertroy/wut/wut
cargo install wut
curl -fsSL https://raw.githubusercontent.com/ethanolivertroy/wut/main/install.sh | sh
```

## Environment

- `WUT_NPM_CACHE_DIR` — where the binary is cached (default `$XDG_CACHE_HOME/wut-npm`)
- `WUT_NPM_BASE_URL` — override the release download base (defaults to the GitHub release)
- `WUT_NPM_VERBOSE` — print the install path when the binary is first unpacked

## Development

```sh
npm test          # hermetic: builds throwaway archives and serves them locally
```

Tests never touch the network or the real cache: they start a local HTTP server,
serve a stub `wut`, and check the checksum gate rejects a tampered archive.

## License

MIT.
