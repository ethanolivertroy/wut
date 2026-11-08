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
