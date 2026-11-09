# Local performance checks

Run the synthetic npm download benchmark from the repository root:

```sh
node evals/performance/download.cjs
```

It serves a tiny synthetic release archive and checksum on localhost, adding
100 ms before each response. Five measured downloads follow one warmup. This
isolates the benefit of overlapping the two requests; it does not estimate
real internet download speeds or cached command startup.

The recorded [before/after run](2026-11-08-download-zero.json) uses the same
fixture and delay on zero. The diagram comparison records output geometry and
pixel-buffer sizes, not wall-clock performance. No API credentials are used.

Run the isolated, optimized glob benchmark with:

```sh
cargo test --locked --release tools::tests::benchmark_glob_matching -- --exact --ignored --nocapture
```

It compares the previous recursive implementation, retained only in tests, with
the new matcher on synthetic file names. Pattern compilation, filesystem work
and model latency are excluded. The repeated-wildcard fixture is intentionally
small so the reference implementation completes safely. All samples from the
[recorded run](2026-11-08-glob-zero.json) are retained. Correctness tests compare
the implementations over exhaustive small inputs and separator/UTF-8 cases.

Run the synthetic early-match workspace search benchmark with:

```sh
cargo test --locked --release tools::tests::benchmark_early_grep -- --exact --ignored --nocapture
```

It creates 4,000 files, each with 201 matching lines, so any first file reaches
the 200-line result limit. The baseline reconstructs the previous eager file
collection. Both paths then use the same scanner and assert equal results.
This isolates traversal/collection and excludes the old redundant per-file
stat, fixture creation, regex compilation and provider latency. Each pair runs
eager then lazy; five pairs follow one warmup, so filesystem caching can affect
the comparison. The [zero run](2026-11-09-workspace-zero.json), dated in UTC,
retains every measured sample: median 1.368 ms eager versus 0.241 ms lazy.
This measures a favorable early-match case, not all workspace searches or
end-to-end answer latency. No API credentials are used.
