# T1176: Ship v0.3.0; model-check concurrency; remove LCOV markers; cargo install; self-upgrade

## Goal

Ship what T1170 hardened and harden what T1170 could not: cut GitHub
release v0.3.0 from master; model-check every concurrent/parallel construct
in the Rust workspace with a state-of-the-art checker and fix all findings;
remove every `LCOV_EXCL_*` marker by testing the underlying lines for real
(keeping the 100% floor green throughout); make `arc` installable via
`cargo install`; and give `arc` a self-upgrade command.

## Background

- T1170 (landed as PR #16, `4d757dba`) reached 100% line coverage
  (147260/147260) with the floor pinned at 100. Part of that 100% rests
  on `LCOV_EXCL_*` markers filtered by the `internal/lcov` pipeline:
  root-only-unreachable chmod arms, terminal-less `tput` bodies,
  platform-gated skips, and TOCTOU-only races. Markers were the honest
  endgame move, but tested lines beat excluded lines.
- Concurrency in the tree was reviewed by hand (CORE-19 `OnceLock` command
  cell, parallel test harnesses, process spawning, the markers subsystem).
  No systematic interleaving/race exploration has ever run.
- Releases are manual (`v0.2.0` cut by hand with three OS binaries).
  `arc-rs` was never published to crates.io and may not satisfy
  `cargo install` (package metadata, binary targets, lockfile policy).
  No upgrade path exists besides re-downloading a binary by hand.

## Scope — the 5 slices

1. **Release** (`REL`): tag and publish v0.3.0 from master with
   `arc` binaries for x86_64-linux, macOS (arm64), and Windows, plus
   release notes summarizing T1153–T1170. Manual GitHub release (same
   as v0.2.0) unless automation already exists.
2. **Model checking** (`MC`): inventory every concurrency site
   (`std::thread`, `OnceLock`/`Mutex`/`RwLock`/atomics, `Command` spawn
   + pipes, parallel iterators, `tokio`/async if any, the nextest
   parallel harness interaction). Evaluate Loom vs Shuttle vs Kani
   against the inventory (effort, fidelity, CI cost) and record the
   decision; implement the winner over the inventory; fix every
   finding (race, deadlock, missed wakeup, leaky interleaving).
3. **Marker removal** (`DEMARK`): enumerate every `LCOV_EXCL_*` site;
   per site choose the best real-testing strategy: finer seams
   (dependency injection following the existing `*_with` pattern),
   hermetic fakes (fake `tput`/conduit/PATH backends), deterministic
   triggers (`/proc` files, pipes, symlinks), or platform-gated tests
   where the line is genuinely platform-only. Delete the marker only
   when the line executes under the gate configuration. Keep `100%`
   green on every push; do not lower floors.
4. **`cargo install`** (`INSTALL`): whatever `cargo install --locked
   --path rust` (and later crates.io packaging) needs: `[package]`
   metadata (description, license, homepage, repository, keywords),
   `[[bin]]` targets, README install section, and a CI smoke job that
   installs into a temp `CARGO_HOME` and runs `arc --version`.
5. **Self-upgrade** (`UPGRADE`): design and implement an upgrade command
   (name TBD from existing command parallels): query the latest GitHub
   release, compare with the running version, download the matching-OS
   asset, verify integrity (checksum/signature — at minimum SHA-256
   against the release notes), replace the running binary atomically
   (rename-swap, never partial overwrite), and report. Fully tested
   with a fake release server (loopback HTTP, never the real network).

## Acceptance criteria

- `REL`: `v0.3.0` exists on GitHub with three working `arc` binaries;
  each runs `--version` reporting `0.3.0` (or the agreed version).
- `MC`: checker chosen with a written decision (effort/fidelity/CI
  cost); every inventory site is modeled; CI runs the models; zero
  open findings.
- `DEMARK`: `grep -r LCOV_EXCL_ rust/` is empty; quality gates still
  report `100.00%` vs the `100.00%` floor; no floor/target lowered.
- `INSTALL`: `cargo install --locked --path rust` succeeds from a
  clean checkout into an empty `CARGO_HOME`; the installed `arc`
  runs; CI smoke job green.
- `UPGRADE`: `arc <upgrade-cmd>` upgrades a stale install against a
  fake server in tests (success, already-current, checksum-mismatch,
  and network-failure paths); no real-network dependency in tests.

## Part 2 — technical notes

- Work order: `REL` first (from master, no code changes), then `MC`
  inventory + decision, then `DEMARK` (largest slice; land in chunks,
  each green), then `INSTALL`, then `UPGRADE`. Slices are otherwise
  independent and may parallelize via subagents.
- TDD/BDD: `Given` a stale install `When` upgrade runs against a fake
  newer release `Then` the binary is atomically replaced and reports
  the new version. Failing-tests-first for every `DEMARK` site and
  every `MC` finding fix.
- CI impact: expect workflow changes — a model-checker job, an install
  smoke job, and longer gate times. `.github/workflows/*.yml` changes
  needed; no Jenkinsfile/`.arclint` in this repo.
