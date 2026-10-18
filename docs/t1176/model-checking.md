# T1176 MC: concurrency inventory, checker decision, findings

## Inventory (all shared-state / concurrency sites in `rust/`)

`arc` is a single-threaded CLI: no `thread::spawn`, no async runtime,
zero `unsafe` (geiger gate). The complete inventory:

| # | Site | Kind | Sharing |
|---|------|------|---------|
| 1 | `conduit.rs`: `Mutex<SessionState>` | The one lock in prod; session/cache get-set protocol | Single-threaded in prod (`&self` interior mutability) |
| 2 | `commit.rs`: `MESSAGE_COUNTER: AtomicU64` (Relaxed) | Unique temp-file disambiguator | Atomics across parallel tests |
| 3 | `*_tests.rs` / `common`: per-file `COUNTER: AtomicU64` | Unique TempDir tags | Same pattern as #2 |
| 4 | `globals.rs`: 7 `OnceLock` cells | Write-once config (trace/ansi/uri/token/anon/opts/command) | Written by `main`/dispatch, read everywhere; parallel tests share the process cells |
| 5 | `terminal.rs`: `WIDTH: OnceLock` | Cached `tput` width | Same value for all callers; benign |
| 6 | `xhp_rem1.rs`: `DB: OnceLock` | Cached compat DB | Deterministic content; benign |
| 7 | `export.rs`: `thread_local! Cell<bool>` | Prompt-failure flag | Thread-confined by construction; race-free |
| 8 | `tests/common`: `ENV_LOCK` + `HomeGuard` | Serializes HOME/USERPROFILE mutation per test binary | Pre-existing T1153 solution |
| 9 | `tests/common`: `CWD_LOCK` + `CwdGuard` | Serializes CWD mutation per test binary | **New in T1176 (finding F1)** |
| 10 | `lint.rs` / `svn.rs`: retry loops with `thread::sleep` | Backoff, not concurrency | No shared state |

## Checker evaluation

| Checker | Class | Verdict |
|---------|-------|---------|
| **Loom 0.7** (chosen) | Exhaustive interleaving exploration for Mutex/RwLock/atomics/thread models | Fits #1–#3 exactly: small protocols, exhaustive schedules, stable toolchain, dev-only dep, fast (<1s). Implemented in `rust/src/conduit_loom.rs`, CI job `loom`. |
| Shuttle | Randomized scheduler for threads/async | Rejected: schedules randomly rather than exhaustively; our protocols are small enough to exhaust. No async runtime to schedule. |
| Kani (CBMC) | Bit-precise proofs over bounded code + `unsafe` | Rejected: zero `unsafe` in tree; needs nightly + CBMC in CI for no reachable proof target. |
| Miri | UB + data-race detection by interpretation | Rejected: the suite builds `reqwest`/TLS clients (FFI: ring, sockets) that Miri cannot execute; the remaining pure subset has no shared-memory threading, so expected signal ≈ 0 at high CI cost. |
| ThreadSanitizer | Runtime race detector (nightly `-Z sanitizer`) | Rejected: nightly-only, runtime sampling (weaker than Loom's exhaustiveness at our scale), same FFI drag as Miri. |

Feature (not `--cfg`) gating: `--cfg loom` via `RUSTFLAGS` leaks into
dependencies and breaks `hyper-util`'s build, so Loom is an optional
dependency behind `[features] loom = ["dep:loom"]`.

## Findings

- **F1 (fixed): unguarded CWD mutation in 6 integration binaries.**
  Each of `vcs1_{weld,branches,commit,amend,bookmarks}` and
  `land_workflow` had one `run_uses_current_dir` test doing raw
  set/restore around `::run`. A panic inside the window would poison
  the binary's CWD for all later tests, and any future CWD-dependent
  sibling would race silently. Fix: shared `common::CwdGuard`
  (Drop-restore + per-binary `CWD_LOCK`), mirroring `HomeGuard`.
  Sibling audit (this doc's evidence): exactly one `::run` per
  binary (the mutator itself), all fixture spawns pin `.current_dir`,
  no other `current_dir()` reads — so the race was latent-only, now
  structurally closed. `land_workflow` holds CWD-then-ENV (only
  dual-holder, fixed order — no inversion; sharing one lock would
  self-deadlock since `Mutex` is not reentrant).
- **F2 (audited clean): `OnceLock` multi-writer test races.** Writers
  per cell: `COMMAND_NAME` ("help" ×2 direct + merged call-cond,
  sequenced — T1170), lib-side probe (separate process, tolerant),
  `arcanist_env` probe (separate binary). All other globals cells are
  never written by tests (only `main`, untested) → deterministic
  `None`. `WIDTH`/`DB` are deterministic per machine/content.
- **F3 (modeled, sound): session protocol + counter uniqueness.**
  Loom explores every interleaving of concurrent
  set/extend/read/store on the real `Client` methods: no deadlock,
  no panic, capabilities union monotonic, identity pairs never torn.
  Fetch-add uniqueness holds on every schedule (negative-controlled:
  a broken assertion fails the model).

## Residual risks

- Loom has no `OnceLock`; first-wins is covered by audit + the merged
  regression test, not by a model.
- Future tests must route CWD dependence through `with_current_dir`
  (comment-enforced, like `HomeGuard` before it).
