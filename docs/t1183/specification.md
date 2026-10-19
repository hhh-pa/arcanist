# T1183: Harden racy test paths

## Part 1 — user/functional specification

**Goal:** eliminate the test flakes observed during the v0.3.1 release:
a flaky coverage line plus flaky fake-binary spawns — with deterministic
tests and root-cause retries, no behavior change.

**Background (all diagnosed on identical trees, green-on-rerun):**

1. `patch_tests.rs:7024` missed the 100% floor on master twice
   (153503/153504, all tests passing).
   Mechanism: `stub_server_tolerates_aborted_clients` is fire-and-forget —
   it returns without synchronizing with its spawned `serve_routes`
   server threads, and nextest runs each test in its own process, so
   process exit can kill a server thread before it executes the abort
   arm. Pure scheduling race, worse on loaded runners.
2. `git_2_0_skips_config_fallback_after_ls_remote_miss` failed when a
   fake-`git` spawn failed (version detection failed open to the config
   sentinel, untrimmed).
3. `amend_git_passes_through_nonempty_summaries` failed with
   `failed to run git: Text file busy (os error 26)` — ETXTBSY executing
   a freshly-written fake under parallel load.

**Scenarios:**

- Given the abort test, When it runs (even on a loaded machine or as the
  last test in the process), Then both server threads observably process
  their aborts before the test returns, and the abort arms are covered
  every run.
- Given a transient ETXTBSY spawning `git`, When any git operation runs,
  Then it rides through to success like the `hg` seam already does.

**Acceptance criteria:**

- `stub_server_tolerates_aborted_clients` waits for both aborts
  (functional assertion, not timing luck); the abort arms stay covered.
- All `git` spawns retry bounded ETXTBSY via one shared helper; a git
  test proves the retry end-to-end on Linux.
- Full local gates green; branch CI green (quality gates incl. floor).

## Part 2 — technical specification

- **Abort sync:** new `serve_routes_with_abort_sink(routes, sink)` seam
  (`*_with` pattern); each abort arm sends `()` down the sink (when
  present) before breaking. Plain `serve_routes` delegates with `None`
  (sole other caller, the synchronous dependencies flow test, unchanged).
  The abort test shares one channel across its two stubs and
  `recv_timeout`s (30s) twice — receipt proves execution, converting the
  race into a functional assertion. No platform specifics (loopback TCP
  + channel).
- **Shared spawn retry:** consolidate the three identical ETXTBSY retry
  loops (`hg`, `lint`, `svn`) into `proc_env.rs` (the documented shared
  choke point: "every spawn site builds its `Command` here"), and adopt
  it in `git` (which has none — the observed flakes): one core plus
  `spawn_output` /
  `spawn_output_with_limit` (injectable bound, from `hg`) /
  `spawn_output_retry` (injectable predicate, from `svn`) wrappers so
  every existing test seam keeps working. `hg`/`svn`/`lint` delete
  their copies (net deletion; `lint::spawn_with_retry` stays as the
  public choke point delegating to shared code); `git::output_with_git`
  (the only direct `.output()` in `git.rs`, covering all 37 wrapper
  uses) adopts it — this fixes the observed git-seam flakes.
  Existing retry tests stay in place, retargeted at `crate::proc_env::`
  paths; add one `git_tests.rs` test (Linux-only, held-write-handle +
  50ms release, mirroring the hg transient-success test) proving
  `output_with_git` rides through.
- **Evaluated and rejected:** fail-closed version detection (user-visible
  behavior change with unknown PHP parity; retries remove the observed
  root cause instead); `svn`-seam retry (no observed flakes — same
  one-line adoption available if any appear); auditing every
  `thread::spawn` test site (only the abort test lacks request/response
  synchronization; floor history confirms the rest).
- **TDD/verification:** the abort race can't be forced deterministically,
  but the fix is deterministic by construction (recv proves execution);
  verify with 20× single-process `--exact` runs of the abort test plus
  the full suite and CI gates. Retry: new git test + retargeted hg
  tests (Linux) / passthrough (macOS).
- **CI impact:** none — no workflow/Dockerfile changes.
