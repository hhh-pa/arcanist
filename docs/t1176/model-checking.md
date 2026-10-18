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
| **TLA+ / TLC 1.8.0** (chosen, alongside Loom) | Exhaustive state-space proof over a TLA+ model: deadlock-freedom + invariants + liveness over *every* reachable state | Complements Loom: Loom explores interleavings of the real Rust code (bounded by its scheduler model); TLC proves properties of the protocol design itself. Models in `tla/`, run manually (java + tla2tools.jar), results below. |
| Shuttle | Randomized scheduler for threads/async | Rejected: schedules randomly rather than exhaustively; our protocols are small enough to exhaust. No async runtime to schedule. |
| Kani (CBMC) | Bit-precise proofs over bounded code + `unsafe` | Rejected: sequential bounded proofs only (no interleaving/concurrency story); zero `unsafe` in tree; needs nightly + CBMC in CI for no reachable proof target. Weaker than TLC's unbounded state-space proofs at our scale. |
| Verus | Deductive SMT-backed verification of Rust (specs + ghost state, incl. concurrent protocols) | Rejected on ROI: the only candidate that could exceed Loom+TLC (unbounded proof of the implementation itself), but needs pervasive annotations, nightly + Z3 in CI, and weeks of expert work per module — disproportionate for safe Rust already covered by exhaustive interleavings + protocol proofs + regression tests. Revisit only for future `unsafe` abstractions. |
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
- **F4 (proven): lock discipline + first-wins, by TLC.**
  `tla/Locks.tla` models the guard discipline (`C` = CWD_LOCK,
  `E` = ENV_LOCK; t1 takes C-then-E, t2 takes E, t3 takes C):
  TLC 1.8.0 checks all 40 distinct states — no deadlock, `TypeOK`
  holds, temporal `Liveness` (every thread finishes) holds.
  `tla/FirstWins.tla` models the `OnceLock` first-wins contract
  (2 writers, 1 reader): all 14 distinct states satisfy `Coherence`
  (readers never disagree), `Consistent` (cell == winner once set),
  and `Liveness`. Negative control `tla/LocksBad.tla` inverts the
  dual-lock orders (t1: E-then-C vs t3: C-then-E): TLC finds the
  deadly embrace at depth 4 (t1 holds E wants C, t3 holds C wants
  E), proving the model can catch the bug class — so the clean
  `Locks` result is not vacuous. Single-lock threads can never join
  a wait cycle, which is why the production shape (exactly one
  dual-holder, fixed C-then-E order) is provably safe.
  Reproduce: `java -cp tla2tools.jar tlc2.TLC -config tla/M.cfg tla/M`
  for M in Locks, FirstWins (expect exit 0, "No error has been
  found") and LocksBad (expect exit 11, "Deadlock reached").

## Residual risks

- TLC proves the *model*, not the implementation: the Locks/FirstWins
  results assume the Rust code matches the modeled protocol (guard
  orders, set-once cells). Loom covers the implementation side for
  #1–#3; first-wins has the merged regression test plus the TLA+
  proof, but no Loom model (Loom has no `OnceLock`).
- Future tests must route CWD dependence through `with_current_dir`
  (comment-enforced, like `HomeGuard` before it).
