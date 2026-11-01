# Triage eval

Before each answer, wut can ask TypeSafe's Jev three questions (see
`src/triage.rs`): does the question need the workspace tools, does it need web
search, and how much reasoning does it call for. The thresholds that turn those
answers into decisions are starting points. This eval measures them on realistic
wut questions so they can be tuned on evidence, as the
[TypeSafe docs](https://docs.typesafe.ai/confidence) recommend.

## Run it

The eval calls the live API, so it is ignored by default and never runs in CI:

```sh
TYPESAFE_API_KEY=... cargo test --locked triage_eval -- --ignored --nocapture
```

Without `TYPESAFE_API_KEY` it prints a note and passes. `TYPESAFE_BASE_URL`
points it at another endpoint. A run sends one request per case, one at a time,
which stays far below the rate limits and costs well under a cent at the price
listed on [docs.typesafe.ai/models](https://docs.typesafe.ai/models).

Each run saves Jev's raw answers to `target/triage-eval/results-<unix time>.json`.
To re-score a saved run after editing labels or scoring code, without calling the
API again:

```sh
TRIAGE_EVAL_RESULTS=target/triage-eval/results-<unix time>.json \
  cargo test --locked triage_eval -- --ignored --nocapture
```

The report says so when the saved answers came from different triage questions
than the current ones.

## Read the report

- **Latency** is the request time per case. Triage gives up after
  `triage::BUDGET` (2 s), so a case over it would have fallen back to the
  default plan.
- **needs_workspace_files** and **needs_current_web_information** sweep the drop
  threshold. A tool is dropped when Jev's probability that it is needed is at or
  below the threshold. "Needed but dropped" is the costly error: the answer loses
  a tool it needed. "Not needed and dropped" is the saving. The report names the
  highest threshold with no false drops and lists any false drops at the current
  threshold.
- **thinking_required** shows labeled effort against Jev's choice, then sweeps
  the confidence that `Auto` reasoning requires before it uses Jev's choice.
  "Too little" counts cases where Jev picked less effort than every acceptable
  label, which costs answer quality; too much only costs time. Disagreements at
  the current cutoff are listed so the labels get a second look too.

No false drops on these cases is necessary, not sufficient. Prefer a threshold
with some margin below the lowest probability Jev gave a needed tool, and add a
case whenever a real question gets triaged badly.

Each response's `model` field names the versioned model that answered. wut pins
`MODEL` in `src/typesafe.rs` to the version the thresholds were checked
against, because `jev-latest` moves when TypeSafe ships a release. To try a
newer Jev, change `MODEL`, re-run the eval, and compare it with the last run
before keeping it.

## Cases

`triage/cases.jsonl` holds one case per line:

| Field | Meaning |
| --- | --- |
| `id` | Unique name |
| `workspace` | A key in `triage/workspaces.json` |
| `conversation` | Optional earlier exchanges, as `user` and `assistant` pairs |
| `question` | What the user asked |
| `needs_workspace` | `true`, `false`, or `"either"` |
| `needs_web` | `true`, `false`, or `"either"` |
| `effort` | `recall`, `explain`, `investigate`, or a list of acceptable levels, best first |
| `why` | One line explaining the labels |

The eval lays each workspace out as empty files and directories, so Jev sees the
same folder name and listing wut would send, with secret files like `.env`
filtered out.

Labeling rules:

- Label what a correct answer needs, not what a model might try. wut can read,
  search, and list files and search the web, but it cannot run commands, so
  "which git version do I have?" does not need the workspace.
- `needs_web` means information that changes: news, releases, prices, status, or
  the contents of a specific page.
- Use `"either"` only when both answers are defensible. Those cases are left out
  of the drop metrics.
- `recall` is a single fact, command, or location, including finding one thing
  in the project. `explain` describes how something works or summarizes it.
  `investigate` diagnoses a failure, reviews for problems, or plans a change.
- Do not copy the examples inside the triage questions, or the eval ends up
  grading its own prompt. `cargo test` checks this, along with unique ids and
  known workspaces.
