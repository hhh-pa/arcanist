# T1181: Publish arc to crates.io as arcanist-rs

## Part 1 — user/functional specification

**Goal:** `cargo install arcanist-rs` installs a working `arc` binary from
the crates.io registry.

**Background:** the cargo package is named `arc-rs`, which is taken on
crates.io (as is `arcanist`). The owner approved renaming; `arcanist-rs`
was verified available (registry API 404). Ref T1176 (close comment).

**Scenarios:**

- Given a machine with cargo and network access, When the user runs
  `cargo install arcanist-rs`, Then an `arc` binary is installed and
  `arc --version` reports the published version.
- Given the renamed tree, When the documented `--path`/`--git` installs
  run, Then they keep working unchanged (binary name stays `arc`).

**Acceptance criteria:**

- `[package] name` is `arcanist-rs`; `cargo publish --dry-run` succeeds.
- The lib target keeps the name `arc_rs` (no `arc_rs::` churn); the only
  source change is the `arc-hg.py` relocation below, with tests.
- `cargo install arcanist-rs` from the registry into an empty `CARGO_HOME`
  succeeds and the installed `arc --version` works (verified post-publish).
- Full local gates green (fmt, clippy `-D warnings`, tests, geiger
  baseline, coverage floor) and branch CI green before the publish step.

## Part 2 — technical specification

- **Rename shape:** package-only. `rust/Cargo.toml`: `name =
  "arcanist-rs"` plus an explicit `[lib] name = "arc_rs"` (73 files use
  `arc_rs::`; the default lib name would otherwise follow the package).
  `[[bin]] name = "arc"` unchanged. Refresh `rust/Cargo.lock` via a
  normal build (root entry only).
- **`arc-hg.py` relocation (dry-run finding):** `markers.rs` bundles the
  script via `include_str!("../../support/hg/arc-hg.py")`, which escapes
  the crate root and fails `cargo package` verification (the only such
  escape in the tree). Move it to `rust/support/hg/arc-hg.py`, repoint
  the `include_str!`, and extend the upward checkout search to try the
  legacy `support/` layout first and the crate-local `rust/support/`
  layout second (same bytes either way; legacy wins on ties). Covered
  by a new `markers_tests.rs` test for the crate-local layout plus
  tie precedence.
- **Docs:** root `README.md` + `rust/README.md` install sections gain the
  registry install line; the "package stays `arc-rs`" note is updated.
  Update live `arc-rs` references (fixture comment, audit comment).
  Historical `docs/t1153`/`docs/t1176` prose is left untouched.
- **Publish metadata:** description/license/homepage/repository/keywords
  already present (T1176 INSTALL); `rust/README.md` is picked up as the
  crate readme automatically. Verify with `cargo publish --dry-run`.
- **Auth:** publishing needs the owner's crates.io token (`cargo login`;
  no token is stored in this environment). The publish command itself and
  the registry install verification run after the code lands green.
- **TDD plan:** no behavior change, so no new tests; verification is
  `cargo publish --dry-run`, the existing `cargo install smoke` CI job
  (path-based, unaffected), and the post-publish registry install probe.
- **CI impact:** no `.github/workflows` changes needed; no
  Jenkinsfile/`.arclint` in this repo. One `scripts/ci` change: the
  gate-image warm layer stubs `src/lib.rs` too, because the manifest's
  new explicit `[lib]` makes the lib target mandatory (autodetect used
  to skip the missing file) — and `cargo clean -p arcanist-rs` drops
  the stub's own units from both warm target dirs afterward, because
  cargo reads a seeded stub rlib as Fresh (missing dep-info inputs
  don't dirty it) and integration tests then link the module-less
  stub (E0432); dependencies stay prebuilt.
