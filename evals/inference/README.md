# Live inference checks

These opt-in checks call Cerebras through the installed wut binary. They use
synthetic temporary workspaces and keep API credentials out of prompts and
reports. Set `CEREBRAS_API_KEY` in the process environment, or enter it at the
hidden prompt. No credential is saved by the runner.

```sh
python3 evals/inference/live_suite.py --binary /path/to/wut \
  --output /tmp/wut-core-results.json
python3 evals/inference/live_suite.py --binary /path/to/wut --phase terminal \
  --output /tmp/wut-terminal-results.json
```

The core phase alternates nine short latency probes per model, then checks
debugging a source-file bug, reading multiple files, searching a workspace,
extracting an error from a longer piped log, handling missing files, refusing
secret-file reads, producing a longer structured answer, and continuing a saved
chat. It checks synthetic workspace files for unintended changes. JSON tasks
are scored for both correct content and raw-JSON formatting; Markdown fences
fail the format check even if the enclosed values are correct.

The terminal phase measures when text actually appears on a terminal for one
paragraph and multiple lines. It also checks invalid-key errors, recovery with
a valid key, and one-shot SIGINT cancellation without saving an incomplete turn.
It does not test the interactive prompt's cancellation behavior.

## Recorded run on zero, 2026-11-08

The [core report](2026-11-08-live-suite-zero.json) and
[terminal report](2026-11-08-terminal-suite-zero.json) contain individual results.
The tested binary was built from commit `3af29e8`.

| Profile | Median first answer text | Maximum first answer text | Content correct | Strict task passes |
| --- | ---: | ---: | ---: | ---: |
| GPT-OSS 120B, low reasoning | 291 ms | 418 ms | 8/8 | 8/8 |
| Qwen 3.8 27B, no reasoning | 206 ms | 251 ms | 8/8 | 6/8 |

Qwen wrapped JSON in Markdown fences in the debugging and multi-file cases,
despite the JSON-only requests. GPT-OSS was faster on some longer answers; the
lower first-text latency did not predict the fastest completion on every task.

Terminal rendering adds another distinction: the current renderer buffers a
line until a newline or completion. Single paragraphs appeared at 624–828 ms,
near completion; multi-line answers began appearing at 288–312 ms. Those are
observations from different generated answers, not an isolated estimate of
rendering overhead. All seven terminal/error/cancellation checks completed as
expected; the timing probes record behavior without imposing a speed threshold.

This is a small synthetic sample from one machine, not a global speed ranking
or a production reliability estimate. Groq, TypeSafe triage, and web search were
not exercised with live credentials. The short latency probes measure direct
inference callbacks; they do not measure visible terminal text.
