# arc-rs — Arcanist in Rust

`arc-rs` is a full-state Rust port of [Arcanist](http://phabricator.org),
the command-line tool for Phabricator. One static binary, no PHP runtime:
it sends code for review, downloads patches, transfers files, shows
status, makes API calls, and everything else `arc` does. Behavior —
including output text, error strings, and exit codes — matches PHP `arc`.

All 32 commands are ported; per-command status and the remaining edge
divergences are tracked in [rust/README.md](rust/README.md).

## Build and run

```sh
cd rust
cargo build --release
./target/release/arc-rs --help
```

## Gates

```sh
cd rust
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```

CI additionally enforces the complexity ratchet (no baseline growth),
coverage floor, `cargo udeps`, `cargo bans`, `cargo geiger`, and
`cargo audit` via the `quality gates` job.

**LICENSE**

Arcanist is released under the Apache 2.0 license except as otherwise noted.
