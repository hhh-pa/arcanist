# arc-rs — Rust port of Arcanist

Tracks T1128. Goal: a single static `arc-rs` binary matching the PHP `arc`
CLI behavior, ported workflow by workflow with `arc` as the reference.

## Status

| Command | State |
|---|---|
| `diff` | planned |
| `land` | planned |
| `lint` | planned |
| `unit` | planned |
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
