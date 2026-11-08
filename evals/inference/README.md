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
a valid key, JSON string values containing literal Markdown characters, and
one-shot SIGINT cancellation without saving an incomplete turn.
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

Terminal rendering adds another distinction: the baseline renderer buffered a
line until a newline or completion. Single paragraphs appeared at 624–828 ms,
near completion; multi-line answers began appearing at 288–312 ms. Those are
observations from different generated answers, not an isolated estimate of
rendering overhead. All seven terminal/error/cancellation checks completed as
expected; the timing probes record behavior without imposing a speed threshold.

This is a small synthetic sample from one machine, not a global speed ranking
or a production reliability estimate. Groq, TypeSafe triage, and web search were
not exercised with live credentials. The short latency probes measure direct
inference callbacks; they do not measure visible terminal text.

## Incremental-rendering and JSON-format follow-up

The [prompt-only attempt](2026-11-08-core-after-stream-json-zero.json) still
produced one Qwen reply with prose and a JSON fence after a blocked file read.
It is retained as a failed attempt, not replaced with a green-only sample.

The validated-output implementation is recorded in the
[core rerun](2026-11-08-core-validated-json-zero.json). Qwen passed 8/8 tasks,
including all six strict raw-JSON cases. GPT-OSS passed JSON formatting, but
reported source line 3 instead of line 4 in the debugging case despite giving
the correct fix, so its overall result remains 7/8. Formatting validation does
not turn an incorrect value into a correct answer.

The [terminal rerun](2026-11-08-terminal-incremental-zero.json) shows ordinary
paragraphs arriving incrementally: first visible text was 238 ms for GPT-OSS
and 280 ms for Qwen, with 42 and 61 visible chunks rather than the baseline's
single completed paragraph. The before/after prompts were the same, but model
output and provider latency can vary; these are observations, not guaranteed
speedups. All seven terminal/error/cancellation checks passed.

A separate [live formatter probe](2026-11-08-json-repair-live-zero.json) calls
the actual production repair method against Cerebras, verifying both object
and array values. The repair request uses a JSON object envelope without tools;
the extracted value is parsed before any output is displayed. It can be rerun
with an environment key using:

```sh
cargo test --locked cerebras::tests::live_json_formatter_preserves_objects_and_arrays -- --exact --ignored
```

The [final installed-build terminal run](2026-11-08-terminal-final-zero.json)
passed all nine checks. First paragraph text appeared at 257 ms for GPT-OSS and
269 ms for Qwen, in 39 and 74 visible chunks. Both models copied an explicit
JSON object with literal `**keep**` and backtick characters intact, and the
terminal's parsed output matched the saved answer. Terminal JSON bypasses
Markdown rendering; embedded control characters are escaped without changing
the parsed values.

An earlier [unquoted value-copy probe](2026-11-08-terminal-json-unquoted-zero.json)
returned valid JSON from both models but changed the requested string values;
Qwen also changed a field name. That failure is retained. The explicit-object
check distinguishes intact terminal output from model instruction following;
it does not establish that arbitrary JSON contents are correct.
