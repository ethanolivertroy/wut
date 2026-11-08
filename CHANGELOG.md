# Changes

## 0.0.5 (unreleased)

- Stream ordinary terminal paragraphs before a newline or completion, while
  preserving inline Markdown and complete code-block/diagram rendering.
- Add Groq GPT-OSS 20B and 120B with separate provider credentials. Select a
  model with `--model`, inspect available choices with `--models`, and override
  reasoning with `--reasoning` without changing saved defaults.
- Reuse HTTP clients across tool rounds and follow-up questions. Add a bounded
  `--benchmark` command for first-answer-text and completion latency.
- Validate explicit JSON-only replies before printing them. Remove complete
  valid JSON fences, allow one tool-free formatting attempt for invalid drafts,
  and preserve JSON string contents on terminals. This validates syntax, not
  factual correctness or a user-provided schema.
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
