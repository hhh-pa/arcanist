# arc-rs — Rust port of Arcanist

Tracks T1128. Goal: a single static `arc-rs` binary matching the PHP `arc`
CLI behavior, ported workflow by workflow with `arc` as the reference.

## Status

| Command | State |
|---|---|
| `diff` | basic (local stat+patch/`--json` vs base; no Differential submit) |
| `land` | basic (squash onto local branch; `--preview`/`--push`; no Differential checks) |
| `lint` | basic (TextLinter TXT2/TXT4/TXT6; changed-files or PATHS; `--json`) |
| `unit` | basic (cargo engine only: exit-status pass/fail, `--json`, no per-test parsing) |
| `call-conduit` | basic (direct HTTP/S, `~/.arcrc` token) |

The Conduit client (mirroring `src/conduit/ConduitClient.php`) is the next
piece: `params` JSON blob carrying `__conduit__` token metadata, `output=json`.

## Gates

```sh
cargo fmt --check
cargo build
cargo test
cargo clippy -- -D warnings
```

Run from this directory.
