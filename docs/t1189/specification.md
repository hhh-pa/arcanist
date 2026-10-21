# T1189: Zero geiger baseline, audit support/ and redundant files

## Part 1 — user/functional specification

**Goal:** two quality-cleanup deliverables with zero behavior change:
(1) workspace `unsafe` 12 → 0 and `geiger-baseline.json` tightened to
match; (2) a file audit answering "is `support/` still useful?" and
deleting any tracked file proven redundant.

**Background:**

- `cargo-geiger` attributes the whole workspace total (12, all
  `unused`) to `rust/tests/common/mod.rs`: `HomeGuard` mutates
  `HOME`/`USERPROFILE` in-process (`unsafe` in edition 2024) so 11
  integration-test files (~20 call sites) can point the user-config
  layer at temp homes. Shipped code is already zero-`unsafe`; this
  helper is the last holdout. The house solved the same problem twice
  before — `globals.rs` OnceLock cells for main-set options, and
  child-process env staging (`Command::env`, safe) for `src/` unit
  tests — but integration tests call high-level `run()` entry points
  in-process, so neither transfers directly: this task adds the third,
  scoped-override variant.
- `support/` (4 PHP-era files), `rust/support/hg/arc-hg.py`, `tla/`,
  `scripts/ci/complexity-scan.ps1`, and
  `scripts/ci/go/cmd/canonical-lcov` look legacy but pre-task probes
  show each referenced; the audit confirms-or-deletes each with
  evidence.

**Scenarios:**

- Given the geiger gate, When it runs on this branch, Then the
  workspace total is 0, the baseline records 0, and the gate prints OK
  (no tighten note).
- Given any migrated integration test, When it runs (parallel, any
  order, under nextest or cargo test), Then it observes its own fake
  home; the real process environment is never mutated.
- Given a production build, When any command resolves home-derived
  paths, Then behavior is identical to today (the override is always
  unset outside tests).
- Given the audit table (Part 2), When reviewed, Then every candidate
  file carries a live-with-evidence verdict or a deletion commit, and
  `support/`'s usefulness is answered per file.

**Acceptance criteria:**

- `geiger-baseline.json`: `unsafe_total` 0; `cargo geiger` shows
  `arcanist-rs` 0/0; no `unsafe` blocks/fns remain in workspace
  source (docs/comments excepted); no `set_var`/`remove_var` anywhere
  in the workspace.
- Full local gates green and branch CI green (fmt, clippy
  `-D warnings`, tests on 3 OSes, loom, install-smoke,
  quality-gates incl. the 100% floor, go-tools tests).
- Audit table complete; zero live files deleted; `anoid`/`liberate`/
  hg tests still pass.

## Part 2 — technical specification

- **Seam:** a test-scoped home override in `rust/src/globals.rs` (the
  house process-global-cell module): `Mutex<Option<PathBuf>>` plus a
  scoped guard (`set_test_home(&Path) -> Guard`,
  `test_home() -> Option<PathBuf>`, `Drop` restores the saved value).
  Branch-free, `.expect()` on poison like today's
  `ENV_LOCK.lock().unwrap()`. Follows the module's documented
  contract (cells read deep in commands that cannot take parameters;
  env fallback when unset). Non-reentrant by construction like
  today's `ENV_LOCK`, so the existing no-nesting discipline carries
  over; cross-test visibility is identical to today's real-env
  mutation under both runners (holder-serialized, process-wide while
  held) — no new flake class.
- **Readers (override-first, fallbacks byte-identical):**
  `config::arcrc_path`, `lint::default_user_config_path`,
  `shell_complete.rs` HOME read. Exhaustiveness gate: grep
  `HOME|USERPROFILE|APPDATA` over non-test `src/` must show no other
  reader; a missed reader fails safe (the suite catches it: tests
  would observe the real home).
- **Migration:** ~20 call sites in 11 files become
  `let _home = arc_rs::globals::set_test_home(..)`; each file keeps
  its own `TempDir` cleanup (audit the 6 `with_owned_home` users for
  guard-removal dependence first); delete
  `HomeGuard`/`install`/`ENV_LOCK` from `tests/common/mod.rs`
  (`CwdGuard` stays); hand-edit the baseline to 0 with an updated
  comment (`--regen` would clobber it with the T968 boilerplate).
- **Coverage-floor constraint:** the repo holds a 100% line floor —
  every new shipped line must be covered by the migrated tests; keep
  the seam branch-free (no poison-fallback arms, no platform `cfg` in
  the seam itself).
- **Spawn-inheritance audit:** today's guard leaks the fake home to
  child processes; the cell does not. Grep fixtures plus fake-script
  writers for `$HOME`/USERPROFILE dependence; stage `Command::env`
  explicitly (safe) at any spawn site that needs it, else let the
  suite prove none does.
- **Evaluated and rejected:** `*_with` param threading through every
  command `run()` (infeasible depth); `OnceLock` (set-once per
  process, tests need per-test values); a dev-dependency scoped-env
  crate (moves `unsafe` to deps where the gate ignores it —
  metric-gaming, not cleanup); child-process probe staging for
  integration tests (an 11-file rewrite for no gain over the cell).
- **TDD/verification:** no new behavior, so no failing-first tests —
  verification is the existing suite (full `cargo test --locked` plus
  nextest via the gates on 3 OSes), the geiger gate at 0, and grep
  gates (`unsafe` blocks, `set_var|remove_var`). For the audit: the
  per-file evidence below, confirmed during implementation.
- **CI impact:** none — no workflow/Dockerfile changes.

**Audit table (probe verdicts — confirm each during implementation):**

| File(s) | Probe verdict | Evidence |
|---|---|---|
| `support/arcanoid/arcanoid.py` | live | exec'd by `arc anoid` (`anoid.rs`) |
| `support/init/init-script.php` | live | required by both PHP tools below |
| `support/lib/extract-symbols.php` | live | symbol-mapper side of the liberate toolchain; pinned by `liberate_tests` |
| `support/lib/rebuild-map.php` | live | shelled out by `arc liberate` (`liberate.rs`) |
| `rust/support/hg/arc-hg.py` | live | `include_str!` in `markers.rs`; T1181 cargo-package constraint |
| `tla/*.tla`, `tla/*.cfg` | live | TLC runs in `docs/t1176/model-checking.md`; `LocksBad` is the negative control |
| `scripts/ci/complexity-scan.ps1` | live | documented Windows dev mirror of the Go gate |
| `scripts/ci/go/cmd/canonical-lcov/` | live | dev-only `go run` helper (unwired in CI by design) |
| `rust/resources/php/symbol-information.json` | live | `include_str!` in `xhp_rem1.rs` |
| `rust/tests/fixtures/*` (17 dirs) | confirm each | consumer grep per dir |
| `scripts/ci/go/internal/*` | confirm each | importer grep per package |
| `.arcconfig` | live | upstream-inherited project config, read by `arc` in checkouts |
