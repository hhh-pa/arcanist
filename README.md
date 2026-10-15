# arc-rs — Arcanist in Rust

`arc-rs` is a full-state Rust port of [Arcanist](http://phabricator.org),
the command-line tool for Phabricator. One static binary, no PHP runtime:
it sends code for review, downloads patches, transfers files, shows
status, makes API calls, and everything else `arc` does. Behavior —
including output text, error strings, and exit codes — matches PHP `arc`.

All 35 commands are ported; per-command status and the remaining edge
divergences are tracked in [rust/README.md](rust/README.md).

## Build and run

```sh
cd rust
cargo build --release
./target/release/arc --help
```

The shipped binary is named `arc` (the cargo package stays `arc-rs`).

## Install to PATH

Prebuilt binaries for Linux, macOS, and Windows are attached to each
[GitHub release](https://github.com/m-hu/arcanist/releases): download
`arc-<target>.tar.gz`, unpack it, and put `arc` on your `PATH`.

Or build from source. From the repository root:

```sh
cargo install --path rust
```

This installs `arc` into `~/.cargo/bin`; keep that directory on your
`PATH`. Reinstall after `git pull` to pick up updates.

## Not ported: `phage`

PHP Arcanist shipped a second entrypoint, `bin/phage`, which was
byte-identical to `bin/arc`: `ArcanistRuntime` dispatches on
`basename($argv[0])`, so invoking it as `phage` selects the `phage`
toolset instead of the `arc` toolset. That toolset is a remote-agent
framework whose bootloader concatenates PHP files (`PhutilBallOfPHP`)
and ships them to remote PHP agents. It is a separate surface from the
35 `arc` commands and is inherently PHP-runtime-bound, so it is
intentionally not ported. Only `arc` is shipped.

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
