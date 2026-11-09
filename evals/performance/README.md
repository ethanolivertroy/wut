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
