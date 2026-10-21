# T1191: Windows prompt deadlock, dispatcher short flags, upgrade library detection

## Part 1 — user/functional specification

**Goal:** three fixes in the interactive-prompt/stdin family, with
regression coverage and zero behavior change on Unix:
(1) Windows prompts survive LF-only input and externally-enabled
win32-input mode instead of blocking forever;
(2) `arc diff --update <rev> -m <msg>` (and every other workflow short
flag, plus `arc -V`) works instead of printing help;
(3) `arc upgrade --apply` stops treating an incidental CWD checkout as
the arcanist library.

**Background:**

- Prompt deadlock (Windows-only): `prompt_for_key` →
  `next_prompt_candidate` → `read_answer_line` (`rust/src/land.rs`)
  blocks forever (zero CPU, no error) when a Windows console delivers
  an answer without CR. Unix is unaffected: the PTY line discipline
  completes reads on LF, while the Windows console (incl.
  ConPTY-backed PTYs) in cooked line-input mode terminates lines on CR
  (Enter) only. Under externally-enabled win32-input mode
  (`CSI ? 9001 h`) keys arrive as escape sequences with no newline at
  all, which starves `read_line` the same way. Interim audit: a
  repo-wide search finds no `?9001h` (or any console-input-mode)
  emitter in this codebase — the mode comes from the driving
  harness/terminal, not from arc. PHP-parity note: PHP `fgets(STDIN)`
  blocks identically in every identified scenario (cooked console
  without Enter, open pipe without a newline, 9001-mode sequences),
  so there is no retrocompat gap to close — the work is hardening
  (shared helper, `?9001l` guard, harness guidance) plus
  regression-locking the family.
- `diff --update -m` (portable dispatcher bug, observed on Windows):
  `strip_standard` (`rust/src/main.rs:507`) canonicalizes EVERY
  single-dash token to global `help`, so `-m` is eaten, `show_help` is
  set, and `dispatch_help_flag` prints the diff help; the `-m` value
  survives as a stray positional. Same fate for `diff -a`/`-F`,
  every other workflow short flag, and `arc -V` (prints help instead
  of the version). The port's own workflow specs define `-m`/`-F`/`-a`
  as functional flags with parse tests, so the dispatcher contradicts
  the port's own contract.
- `upgrade --apply` (portable design flaw, observed on Windows):
  `resolve_release_target` treats the CWD's enclosing checkout as THE
  arcanist library: on a feature branch it hard-errors naming that
  checkout `arcanist` (blocking a legitimate binary upgrade); on
  master/stable of a non-arcanist repo it fetches releases from that
  repo's fork/origin remote. (`resolve_apply_target`/`run_apply`, the
  `git pull` flow, have no non-test callers and are out of scope.)

**Scenarios:**

- Given `arc diff --update 1 -m "msg"`, When it runs, Then no help
  text prints and the update proceeds (to repo/conduit stages).
- Given any workflow short flag (`diff -a`, `-F`, ...), When parsed
  by the dispatcher, Then it reaches the workflow untouched; Given
  `-h`/`--help`, When parsed, Then help still shows.
- Given `arc -V`, When it runs, Then it prints the version (not help).
- Given `arc upgrade --apply` run from inside a non-arcanist checkout
  (any branch), When it resolves, Then it upgrades the binary on the
  `stable` channel from the default repo, ignoring the checkout.
- Given a dev binary (exe inside the checkout) on a feature branch,
  When `upgrade --apply` resolves, Then it still refuses (branch
  rule protects the dev binary from a stable clobber).
- Given a staged `y\n` (LF-only, no CR), `y\r\n`, `y\r`, empty, or
  EOF input, When the shared prompt reader runs, Then it returns
  `y`/`y`/`y`/`""`/`None` without blocking.
- Given a prompt with piped (non-TTY) stdin on any OS, When asked,
  Then it fails fast with the not-interactive error (never blocks on
  the pipe). (`diff`'s TTY gate keeps its deliberate PHP parity: no
  gate on Windows, pinned by `diff_tests`.)
- Given a Windows console with win32-input mode left on by the
  harness, When a prompt reads live stdin, Then arc first restores
  cooked reporting (`CSI ? 9001 l`, ignored where unsupported) so a
  following Enter completes the read.

**Acceptance criteria:**

- New/updated unit tests pass on all 3 CI OSes, covering: short-flag
  passthrough + `-h` help + `-V` version; incidental-checkout
  upgrade (feature/master) + dev-binary gate; the LF/CRLF/CR/empty/
  EOF/error matrix; fail-fast-before-read (parking-reader proof).
- Full local gates green and branch CI green (fmt, clippy
  `-D warnings`, tests on 3 OSes, loom, install-smoke,
  quality-gates incl. the 100% floor, go-tools tests).
- Geiger stays 0 (no new `unsafe`); no new dependencies; zero
  behavior change on Unix (the `?9001l` write is `cfg(windows)`).
- Two contract corrections surfaced and recorded (dispatcher test,
  upgrade branch-gate test — both pin behavior this task proves
  wrong against the port's own specs).

## Part 2 — technical specification

- **Slice 1 — shared prompt reader (no behavior change):** new core
  `prompt::read_answer_result(&mut dyn BufRead) ->
  Result<Option<String>, std::io::Error>` (`\n`-split,
  trailing-`\r` trim, `None` on EOF); `prompt::read_answer` keeps its
  signature/semantics as the `None`-on-error wrapper;
  `land::read_answer_line` and `diff::prompt_confirm_from`'s inline
  `read_line` delegate to the core (diff keeps its
  `failed to read user input` error mapping). Follows the T1176 seam
  precedent (free function over `BufRead` stages bytes/EOF/errors in
  tests; no `Io` churn). Matrix tests: LF, CRLF, lone-CR-at-EOF,
  empty line, EOF-empty, multi-line (two reads), invalid UTF-8,
  erroring reader. Parking-reader test: a `BufRead` that parks
  forever + `stdin_tty=false` returns the not-interactive error
  immediately (structural no-block proof; land + prompt gates).
- **Slice 2 — dispatcher `-h` narrowing (contract correction #1):**
  in `strip_standard`, only `-h` (name `h`) canonicalizes to global
  help; every other single-dash token passes through to workflows
  (`-h=x` keeps today's does-not-take-a-parameter error; bare `-`
  already passes through). `strip_toolset`/`strip_classic` only strip
  `--long` flags (verified) — no other short-flag trap. Update
  `strip_standard_handles_dash_and_valued_flags` (pins `-x`→help) and
  `standard_token_kind`'s doc comment; add passthrough tests (`-m`,
  `-a`, `-F`, `-V`), `-h`-still-help, and a `dispatch(["-V"])`
  version test (no network). Compose the diff proof without conduit:
  `strip_globals` rest of `diff --update 1 -m x` feeds
  `diff::parse_options` successfully. In-repo proof the old behavior
  is wrong: the port's own `FLAG_SPECS`/`HELP_SPECS`/`-V` specs are
  unreachable while the dispatcher eats them.
- **Slice 3 — upgrade incidental-checkout rule (contract
  correction #2):** `resolve_release_target` takes the running exe
  (`resolve_release_target_with(dir, exe)` seam;
  `ApplyOptions.exe`/test override already exists for injection):
  when the exe lives inside the checkout root (dev flow, prefix
  check on canonicalized paths, case-insensitive on Windows), keep
  today's branch gate; otherwise the checkout is incidental — return
  the stable channel + `DEFAULT_RELEASE_REPO`, skipping branch and
  fork/origin probes. Rework `release_target_gates_branches` (pins
  the misfire): dev-exe-still-gates, incidental-feature-upgrades-
  stable, incidental-master-ignores-foreign-remote. Uncanonicalizable
  exe paths fall back to the raw-path prefix check (documented).
- **Slice 4 — `?9001l` guard (`cfg(windows)` only):** new
  `prompt::live_stdin_lock()` returning `StdinLock`, emitting the
  disable sequence on Windows before the lock; pure
  `win32_input_disable_sequence()` byte test + a call-the-function
  test (line coverage). Migrate prompt-live-stdin sites only
  (`land.rs:893`, `diff.rs:2683`, `upgrade.rs:1536`,
  `close_revision.rs:694`, install_certificate/which prompt sites
  after verifying each is a prompt); bulk-data stdin sites
  (export/cover/paste reads) are explicitly excluded (no stdout
  pollution of machine-readable output). Residual risk (negligible):
  resetting a mode an outer app wanted left on.
- **Out of scope (decided, not deferred):** a `ReadConsoleInputW` /
  `SetConsoleMode` console reader needs FFI `unsafe` (geiger
  regression) or a safe-wrapper dependency (the T1189 verdict calls
  dep-hiding metric-gaming); real interactive Windows users (Enter
  key) work today, and every starvation scenario blocks PHP
  identically — so no FFI. Harness guidance ships instead (task
  comment / docs note): drive Windows prompts with CR/CRLF-terminated
  input and do not leave win32-input mode enabled. `diff`'s Windows
  no-TTY-gate stays (deliberate PHP parity, pinned test).
- **Coverage-floor constraint:** the repo holds a 100% line floor —
  every new shipped line must be executed by tests on its platform;
  follow the existing `cfg(windows)` precedent (e.g. `upgrade.rs`
  windows fns + `*_tests.rs` windows sections). `[lints.rust]
  unused = deny` is package-wide: no platform leaves dead code.
- **TDD/verification:** slices land in order 1→4; matrix and
  passthrough/gate tests are written first and fail before their
  slice (2 and 3 fail against the old pinned tests, which are then
  corrected with the recorded justification). Verification: full
  `cargo test --locked` + nextest-via-gates locally, branch CI to
  green on all 3 OSes (the Windows job executes the portable
  regression tests); true-console deadlock verification (ConPTY +
  LF injection, cmd/PowerShell argv matrix) is a manual Windows step
  for the reporter — no reliable console exists in CI.
- **CI impact:** none — no workflow/Dockerfile changes.
