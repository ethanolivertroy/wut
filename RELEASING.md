# Releasing wut

Prepare a candidate on a pull request before pushing a version tag.

1. Update the package version in `Cargo.toml`, its root entry in `Cargo.lock`,
   and `npm/package.json`. Record user-visible changes in `CHANGELOG.md`.
2. Require green CI for formatting, Rust tests on stable and 1.88, clippy,
   Python CLI/installer tests, npm tests/package contents, the source package,
   and all four release targets. The shared `build-release.yml` workflow tests
   and packages the same targets used when publishing: Linux musl and macOS,
   each on Intel and ARM. Its artifacts include archives and SHA-256 files.
3. Review the live inference reports in `evals/inference/README.md`. Synthetic
   tests distinguish JSON syntax, answer accuracy, direct inference latency
   and visible terminal latency. Retain failed samples; do not treat a small
   live sample as a reliability guarantee. API credentials are opt-in and must
   never be saved in reports or committed.
4. Inspect the candidate archives, and smoke-test a downloaded native archive
   and the npm wrapper with isolated cache/config directories. Confirm the
   expected version, model list and credential errors without saving a key.

Homebrew stays on the last published release until the new archives exist.
Do not invent checksums or point the formula at unavailable assets. After
publishing, the release workflow verifies the downloads and updates the
formula and npm version on `main`.

After the candidate is approved and merged, publish by pushing its version tag
(for example `v0.0.5`) at that exact merged commit. The `Release` workflow checks
the tag against the crate version, rebuilds the shared four-target matrix,
publishes GitHub archives/checksums, publishes crates.io and npm through trusted
publishing, and updates Homebrew. A manual rerun accepts an existing tag and
skips versions already published to the registries.

Preparing a candidate and passing these checks does not publish a release.
