# T1232: call-conduit pipe audit — Rule 4 clean, regression locks, no upstream integration

## Part 1 — user/functional specification

**Goal:** answer two reported concerns with evidence, then lock the
correct behavior with binary-level regression tests (no behavior
change):
(1) whether an upstream change should be integrated into the Rust
`call-conduit` code;
(2) whether `echo '<json>' | arc call-conduit --conduit-uri
https://orsay.hhh.pa/ -- maniphest.edit` is supposed to work with a
pipe, and why it was reported broken.

**Background — upstream check (verdict: do NOT integrate):**

- Rust upstream is in sync: `m-hu/arcanist` HEAD == `origin` HEAD ==
  local HEAD (`b7eb67d5`). Nothing to integrate.
- PHP upstream (`phacility/arcanist`) `ceb082ef` (the pinned reference,
  2020-07-23) → `master` (`43b0796`):
  - `ArcanistCallConduitWorkflow.php` differs in ONE help sentence only
    ("writing scripts which integrate with Phabricator" → "writing
    scripts"). Zero behavioral effect; the Rust help pins the ceb
    bytes (`core67_call_conduit_help_is_ceb_bytes`).
  - `ArcanistWorkflow.php` / `ArcanistRuntime.php` diffs are
    `strlen($x)` → `$x !== null` hardening (PHP 8.1), message
    rewords, de-Arcanist branding, and one missing `$key` format arg
    fix in the duplicate-workflow-name error. None touch the
    stdin/pipe/terminator paths (`readStdin`, the `--` rule at
    `ArcanistRuntime.php:80-86,172-194` are byte-identical).
- House policy (`docs/t1153/decisions.md`) pins the frozen ceb
  reference and does not track post-ceb drift, so the cosmetic
  upstream reword stays unintegrated.

**Background — pipe audit (verdict: Rule 4 CLEAN, not reproduced):**

- The documented form works. Live probes against orsay.hhh.pa (all
  exit 0 with valid `{error:null,...}` envelopes): echo-pipe,
  herestring, and stdin-redirect × `diffusion.repository.search`,
  `maniphest.search`, `maniphest.edit` (this task was created that
  way), with and without `2>&1` and downstream pipes.
- Every pipe-path edge matches PHP @ ceb:
  - Piped JSON + `--` → params delivered, envelope on stdout, exit 0
    (PHP `readStdin` + `phutil_json_decode`; the `INPUT` "Waiting for
    input" hint prints only when interactive, so piped runs are
    silent on both sides).
  - Piped stdin without `--` → terminator usage error, exit 1
    (`ArcanistRuntime.php:80-86,172-194`; already pinned by
    `core17_piped_toolset_commands_require_dashes`).
  - Empty/bad JSON on stdin → `ArcanistUsageException`, which
    `extends Exception` (verified in upstream source), so it escapes
    `ArcanistRuntime::execute()` into a PHP fatal: bare message on
    stderr, exit 255. Rust `Fatal` renders exactly that (message +
    255 are the stable observables per the house fatal policy).
- Root cause (confirmed by reproduction): the failure is caller-side
  shell mangling, not arc. Piping through `echo` expands the JSON
  newline escapes before arc runs (both this tool's shell and zsh do),
  so arc receives invalid JSON — literal newlines inside string values
  — which it correctly rejects with the stdin-blob fatal, exactly like
  PHP. The same bytes via `printf '%s'` or a file redirect succeed
  (this task's own Spec-line update proved it). Fix: the five echo-pipe
  conduit snippets in the agent skills (start-task twice,
  address-review, land-review, request-review) now use `printf '%s'`,
  matching the safe `printf` form the same files already used
  elsewhere. Per T1153 Rule 4 the Rust behavior is recorded clean with
  this evidence plus regression tests; correct code is not edited to
  chase the symptom.

**Follow-on scope (added mid-task at user request):** harden
the three amend tests that fail on hosts whose `~/.arcrc` sets
`history.immutable` (unit `d22_bare_child_amend_gates_like_argv_flags`
plus integration `lint_amend_all_amends_and_decline_aborts` and
`d22_bare_child_amend_prompts_and_resolves`): `amend_enabled` reads
the real user/system config layers underneath, and the tests staged
no project layer, so a developer config flipped them (green in CI,
red locally). Fix stages an explicit project `.arcconfig` in each
(test-only; production layering is correct PHP behavior).

**Scenarios:**

- Given `echo '{"ping":1}' | arc call-conduit --conduit-uri <mock>
  -- conduit.ping`, When it runs, Then exit 0, stdout parses as the
  `{error:null,errorMessage:null,response:<mock result>}` envelope,
  stderr is empty, and the mock server saw the piped params on the
  wire.
- Given garbage bytes piped to `arc call-conduit -- m`, When it runs,
  Then exit 255, stdout is empty, stderr is exactly `Provide method
  parameters on stdin as a JSON blob.\n` (errors ride stderr, so a
  trailing `2>&1` merges them into any downstream pipe).
- Given piped stdin without `--`, When `arc call-conduit m` runs,
  Then exit 1 with the terminator usage text (pre-existing pin).

**Acceptance criteria:**

- New binary-level tests pass on all 3 CI OSes (mock binds
  127.0.0.1, no external network).
- `cargo fmt --check`, `cargo test`, `cargo clippy -- -D warnings`
  green locally; branch CI green.
- Zero behavior change (test-only diff); no new dependencies; geiger
  stays 0.
- Task records the no-integration verdict with file-level evidence.

**Out of scope (decided, not deferred):**

- Post-ceb upstream cosmetic drift (help reword, branding) stays
  unintegrated per the frozen-ceb pin.
- TTY-only `INPUT` "Waiting for input on stdin..." hint: PHP prints
  it when stdin is a TTY before blocking for EOF; Rust stays silent
  (pre-existing, pipe-orthogonal; `paste` already ports the hint
  where it matters). Follow-up if wanted, not this task.

## Part 2 — technical specification

- **Slice 1 — spec + task reference (this file):** commit the spec on
  its own, then add the `**Spec**` line to T1232 via an echo-pipe
  `maniphest.edit` call — the exact reported shape, doubling as live
  proof.
- **Slice 2 — Rule 4 regression locks (`rust/tests/dispatch_core.rs`,
  test-only):** extend the binary-level dispatcher suite (hermetic
  fixture: scratch HOME + repo, scrubbed `ARC_*` env, real `arc`
  spawn via `CARGO_BIN_EXE_arc`):
  - `t1232_piped_stdin_json_reaches_envelope`: inline one-shot mock
    Conduit server (127.0.0.1:0, canned
    `{"result": <marker>, "error_code": null}`); spawn
    `call-conduit --conduit-uri <mock> -- conduit.ping` with
    `Stdio::piped()` stdin, write a JSON blob, close stdin,
    `wait_with_output`; assert exit 0, stdout parses with `error`
    null and `response` == marker, stderr empty, and the wire body
    contains the piped params. Tokenless explicit-URI client setup
    accepts the `http://` mock (no scheme gate in
    `config::build_client` / `conduit::Client::new`).
  - `t1232_piped_garbage_stdin_is_fatal`: spawn `call-conduit --
    m` with piped garbage (no server; stdin parsing precedes all
    I/O); assert exit 255, empty stdout, exact blob message on
    stderr.
  - No red-green cycle applies: behavior is already correct, so the
    tests pass on first run — they are characterization locks, not
    fix drivers (Rule 4). New test-only lines keep the 100% shipped
    line floor untouched; `[lints.rust] unused = deny` holds (no
    dead helpers).
- **Slice 3 — amend-test hermetic fix (test-only):** the same
  real-`~/.arcrc` leak broke three amend tests on hosts with
  `history.immutable` set (green in CI, red locally). Fix stages
  an explicit project `.arcconfig`
  (`{"history.immutable": false}`, which shadows user+system) in
  each: `d22_bare_child_amend_gates_like_argv_flags`
  (`rust/src/lint_tests.rs`, whose existing overwrite with `true`
  keeps pinning the immutable arm),
  `lint_amend_all_amends_and_decline_aborts` and
  `d22_bare_child_amend_prompts_and_resolves`
  (`rust/tests/lint_text.rs`). All three failures reproduced on the
  untouched base first. No production lines change, so the 100%
  shipped-line floor and geiger-0 are untouched; production
  layering (user config gates amend) is correct PHP behavior.
  Audited: `config_bool`'s sole production caller is
  `amend_enabled`, and no other test reaches real user config.
- **TDD/verification:** slices in order; run the focused
  `dispatch_core` target, then the full `cargo test --locked`,
  `cargo fmt --check`, `cargo clippy -- -D warnings`; push branch
  and watch CI (3 OSes + quality gates) to green before reporting.
- **CI impact:** none — no workflow/Dockerfile changes (GitHub
  Actions repo; no Jenkinsfile/.arclint changes needed).
