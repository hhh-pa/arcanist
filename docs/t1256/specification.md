# T1256: drop redundant `#[must_use]` on `ExitCode` helpers

## Goal

Turn the scheduled `rust` workflow green again after stable Rust 1.100.0
started denying `clippy::double_must_use` (see T1256 for the failing run).

## Fix

Remove the bare `#[must_use]` attribute from the three functions that
return `std::process::ExitCode` (itself already `#[must_use]`), leaving a
comment at each site so the attribute is not re-added:

- `rust/src/alias.rs` — `shell_status_exit_code`
- `rust/src/conduit.rs` — `EnvelopeKind::exit_code`
- `rust/src/unit.rs` — `exit_code_for_overall`

No behavior change: `ExitCode`'s own `#[must_use]` keeps enforcing the
same must-use check on all callers.

## Acceptance criteria

1. `cargo clippy --locked --all-targets --features loom -- -D warnings`
   passes on stable 1.100.0.
2. `cargo clippy --all-targets -- -D warnings` passes on stable 1.100.0.
3. `cargo fmt --all --check` passes.
4. Covering tests pass: `shell_status_mapping_passes_codes_and_fails_signals`,
   `r4_core6_envelope_exit_codes`, `harbormaster_and_exit_codes`.
5. No other `#[must_use]`-on-`ExitCode` sites remain in `rust/src`.

## CI impact

No workflow or lint-config changes. Gates are the `rust` workflow checks
plus Codecov statuses on the PR.
