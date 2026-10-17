# T1170: CI hygiene — cargo caching, zero warnings, 100% coverage

## Goal

Fast, silent, fully-covered CI: cargo builds reuse the GitHub Actions
cache instead of re-downloading ~150 crates and recompiling from scratch
on every run; no check-run carries warning annotations and no Rust build
step emits `warning:` lines; project line coverage is 100% and enforced
by the in-repo gates.

## Background

- The release workflow's `cargo build --release --locked` (and the same
  cold start in `rust.yml`'s `test` / `fmt-clippy` / `release` jobs)
  downloads the whole crates.io closure and compiles it on ubuntu,
  macOS, and Windows on every run. `rust.yml` caches only the Docker
  gate image and the Go toolchain; cargo's registry (`~/.cargo`) and
  `target/` are never cached. The `quality-gates` job runs cargo inside
  the gate image, so its cargo caching rides on Docker layer caching,
  not on the runner filesystem.
- Warning annotations observed on master-HEAD check-runs: Node.js 20
  deprecation (`actions/checkout@v4`, `actions/cache@v4`, … forced onto
  Node 24) and a deprecated Codecov uploader action. Full enumeration
  happens on the branch's own runs (annotation sweep is part of the
  acceptance below).
- Project coverage is ~92.89% (`codecov/project`); the only exclusion is
  `scripts/ci/go/**` in `codecov.yml`. Coverage is LLVM source-based,
  consumed by Codecov from the `quality-gates` job's `lcov.info`.

## Scope — the 3 slices

1. **Cache** (`CACHE`): cargo registry + `target/` caching for every job
   that runs cargo on the runner (`rust.yml`: `fmt-clippy`, `test` ×3,
   `release` ×3; `release.yml`: `build` ×3). Key on OS + toolchain +
   `rust/Cargo.lock` (+ profile where `target/` is shared across
   profiles). Docker-gate cargo caching stays layer-based; no change
   unless the sweep shows the image rebuilds crates unnecessarily.
2. **Warnings** (`WARN`): upgrade every deprecated action (checkout,
   cache/restore/save, upload/download-artifact, docker/*, toolchain,
   sticky-comment, codecov uploader — final list from the annotation
   sweep) to supported majors; eliminate any `warning:` output from
   `cargo build`/`clippy`/`test` steps (clippy already runs `-D
   warnings`, so this is about rustc/cargo warnings and action banners).
3. **Coverage** (`COV`): close the executable-line gap to 100% with real
   tests (unit/integration, hermetic — no browser, isolated HOME where
   user config leaks in). No new blanket exclusions; single-line
   `// no-coverage` style carve-outs only with a written reason each
   (unreachable-defensive arms). Raise the in-repo floor gates
   (`coverage-floor` default + `rust-quality-gates` floor constant,
   which must stay in sync per the `coverage-summary` comment) to 100.

## Fix design

- `CACHE`: add `Swatinem/rust-cache@v2` (or `actions/cache` on
  `~/.cargo/{registry,git}` + `rust/target`, whichever proves correct
  per-OS in a trial run) with `workspaces: rust`, `cache-targets` on,
  and a shared key prefix per OS+toolchain so `test` and `release`
  jobs share the registry while `target/` stays profile-correct. Save
  always runs (even on test failure) so red runs still warm the cache.
  `release.yml` gets the same step (its jobs run `cargo build
  --release --locked` on the runner).
- `WARN`: bump majors, re-run, sweep annotations via the check-runs
  API until the warning set is empty; grep the `cargo` step logs for
  `warning:` (case-insensitive, excluding the intentional
  `DOCKERHUB_TOKEN`-missing notice path when the secret exists).
- `COV`: generate the uncovered-line list from a local
  `cargo llvm-cov` run, work file by file (highest-count first),
  adding tests that execute the lines — never `#[cfg(not(tarpaulin))]`
  style wholesale ignores. Update `coverage-floor` invocation(s),
  the floor constant, and golden summaries together.

## Tests

- `CACHE` (executable check, not a unit test): two consecutive branch
  runs — the second run's `cargo` steps download zero crates
  (`Downloaded` count in logs) and complete materially faster; a
  `Cargo.lock`-touching push produces a miss and a correct rebuild.
- `WARN` (executable check): script sweeping every check-run of the
  branch HEAD for `annotation_level == "warning"` returns empty; log
  grep for rustc/cargo warnings returns empty.
- `COV` (TDD per file): `cargo llvm-cov report` shows the touched
  file(s) at 100% before moving on; full suite stays green
  (`cargo test --locked`, fmt, clippy `-D warnings`); final proof is
  `codecov/project` 100% + `codecov/patch` 100% on the branch build.

## Live verification

- Branch CI: full `rust.yml` matrix + `release.yml` dry-run
  (`workflow_dispatch`, builds without publishing) green; annotation
  sweep empty; Codecov statuses 100%/100%.
- Cache proof comes from the *second* run's logs (first run warms).

## Acceptance criteria

1. `CACHE`: a no-dependency-change push reuses the cargo cache on all
   three OSes (zero `Downloaded` lines, faster `cargo` steps) in both
   workflows; lockfile changes still rebuild correctly.
2. `WARN`: zero warning annotations on every check-run of a green
   branch build; zero `warning:` lines in cargo step logs.
3. `COV`: `codecov/project` 100% and patch 100% on the branch;
   in-repo floor gates set to 100 and green; no new coverage
   exclusions except individually-justified single lines.
4. `michael-h-hu` review approves; squash-merge; master CI stays green.

## Non-goals

- Changing what CI builds or tests (no new jobs, no matrix changes).
- Docker gate-image redesign (unless the cache trial proves it
  rebuilds crates unnecessarily).
- Coverage of `scripts/ci/go/**` (stays excluded: black-box golden
  tests, gate is `go test` itself).
