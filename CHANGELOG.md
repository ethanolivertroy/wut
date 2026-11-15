# Changes

## Unreleased

## 0.0.6 — 2026-11-15

- Add DeepSeek V4.1 Flash 1017 Fast and Ultrafast through Vercel AI Gateway,
  with provider-specific credentials and supported reasoning settings.
- Extend hidden API-key setup to Cerebras, Groq and Vercel AI Gateway through
  `--set-key` and settings. Keep each provider's saved key separate and enforce
  private file permissions, ownership and link checks.
- Commit chat history only after output and session saving succeed. Restore
  previous session state on save failure, stop on tool-display errors, and
  check cancellation before committing an answer.
- Preserve permanent TypeSafe triage failures across settings changes. Keep
  temporary failures retryable, enforce request deadlines, and handle very long
  retry delays without overflowing.
- Keep update notices pending until displayed successfully. Retry failed or
  abandoned update checks after a short lease and serialize cache updates so
  refreshes and notifications preserve each other's changes.
- Track Markdown block and list states explicitly. Respect code-fence lengths
  and bare closing delimiters, and fix cursor positioning for command suggestions.
- Publish npm binaries atomically, preserve existing cache entries on failure,
  support custom cache roots, and forward repeated signals while preserving
  the binary's signal exit status.
- Use exclusive temporary files for installer and shell-integration updates.
  Failed storage writes clean up only temporary files owned by the current writer.
- Stop workspace traversal as soon as `grep` reaches its match limit. Bound
  directory listing work and output, and avoid redundant file metadata calls.
- Bound config, session and update-cache reads to regular files and explicit
  size limits. Reject pipes without blocking and preserve existing saved data
  when a new save exceeds its loader's limit.
- Compile workspace globs once per search, add fast paths for literals and
  suffixes, and replace recursive wildcard backtracking with a bounded regex
  matcher. Reject glob patterns longer than 512 bytes.
- Reuse the provider stream's line buffer and bound line/body sizes and
  tool-call indices before allocation. Reject malformed or oversized streams
  without recording an incomplete turn.
- Escape terminal controls in diagnostics, including provider messages and
  untrusted path or model names.
- Rasterize diagrams to their terminal display width and cap the pixel buffer
  at 16 MiB, falling back to Mermaid text for oversized images.
- Disable local-file and embedded-image references in diagrams, including
  nested SVG images that could bypass workspace restrictions or pixel limits.
- Download npm release archives and checksums concurrently, with a shared
  60-second deadline and bounded response sizes. Cancel failed download pairs
  and preserve an existing cached binary on failure.
- Clarify installation and everyday usage in the README. Remove the vendored
  TypeSafe coding-agent skill and lockfile while keeping the Rust integration.

## 0.0.5 — 2026-11-08

- Stream ordinary terminal paragraphs before a newline or completion, while
  preserving inline Markdown and complete code-block/diagram rendering.
- Add Groq GPT-OSS 20B and 120B with separate provider credentials. Select a
  model with `--model`, inspect available choices with `--models`, and override
  reasoning with `--reasoning` without changing saved defaults.
- Reuse HTTP clients across tool rounds and follow-up questions. Add a bounded
  `--benchmark` command for first-answer-text and completion latency.
- Validate explicit JSON-only replies before printing them. Remove complete
  valid JSON fences, allow one tool-free formatting attempt for invalid drafts,
  and preserve JSON string contents on terminals. Preserve large integers and
  high-precision decimals without floating-point conversion. This validates
  syntax, not factual correctness or a user-provided schema.
- Migrate archived Cerebras Gemma defaults and saved chats to GPT-OSS 120B with
  supported reasoning. Explicit archived-model choices report replacements.
- Harden saved Cerebras credentials against unsafe permissions, ownership,
  symlinks and hard links. The installer asks before saving a key in plaintext;
  environment keys take priority and are not saved.
- Block indirect reads through secret-path symlinks and bound workspace reads
  to regular text files, including files that grow while being read.
- Check all four shipping targets, archive contents, checksums, packaged CLI
  behavior and the crates.io source package before release. Linux and macOS
  binaries cover Intel and ARM; release and pull-request CI share build steps.

See [the inference checks](evals/inference/README.md) for live results, retained
failures and measurement limits. Groq has local protocol coverage; no live Groq
credential was available for these measurements.
