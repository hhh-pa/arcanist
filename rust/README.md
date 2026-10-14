# arc-rs — Rust port of Arcanist

Tracks T1128. Goal: a single static `arc-rs` binary matching the PHP `arc`
CLI behavior, ported workflow by workflow with `arc` as the reference.

## Status

| Command | State |
|---|---|
| `amend` | basic (amend HEAD message from -m/--revision; refuses staged) |
| `anoid` | full (python3 passthru to support/arcanoid) |
| `bookmarks` | basic (hg bookmarks; git fails like arc; --json) |
| `branches` | basic (local git branches; --all/--json; no review status) |
| `call-conduit` | basic (direct HTTP/S, `~/.arcrc` token) |
| `close-revision` | basic (close via differential.close; --finalize/--quiet/--json/--force) |
| `commit` | basic (git commit from -m/--revision; --show; no push/SVN) |
| `cover` | basic (changed-line blame by author; --rev/paths; git only) |
| `diff` | basic (local stat+patch/`--json` vs base; no Differential submit) |
| `download` | basic (F123 download; --as/--as -; size-verified) |
| `export` | basic (--git/--unified local or revision export) |
| `get-config` | basic (precedence read; --show-source; 15 builtins) |
| `inspect` | basic (working-copy tree + --explore; no browse) |
| `install-certificate` | basic (token install; --token/--force; no legacy exchange) |
| `land` | basic (squash onto local branch; `--preview`/`--push`; no Differential checks) |
| `liberate` | basic (create/rebuild libphutil libs; needs php) |
| `lint` | basic (TextLinter TXT2/TXT4/TXT6; changed-files or PATHS; `--json`) |
| `linters` | basic (registry names + one line; --verbose/--search/exact; no engine config) |
| `list` | basic (open revs, \* = current WC; --json) |
| `look` | basic (Arcventure clearing/remotes/published; UTC flavor) |
| `paste` | basic (read/create pastes; --title/--lang/--input) |
| `patch` | basic (apply revision/diff/patch; --preview/--force; no arcbundle) |
| `set-config` | basic (user/--local write; typed coercion; empty deletes) |
| `tasks` | basic (Maniphest table/--json; --owner/--status/--order/--limit) |
| `todo` | basic (quick task create; --cc/--project/--json; no --browse) |
| `unit` | basic (cargo engine only: exit-status pass/fail, `--json`, no per-test parsing) |
| `upgrade` | report-only (version/status + manual steps; --check/--json) |
| `upload` | basic (chunked upload + fallback; --temporary/--json) |
| `weld` | full (fuse files; +--seed deterministic mode) |
| `which` | basic (context report; --head/--show-base/--json) |
| `work` | basic (checkout/create local branch; --start) |

### Known divergences from PHP `arc`

- `liberate` exits non-zero on failure (PHP always exits 0).
- `list` works outside a git repo with no markers (PHP fails).
- `upgrade` is report-only; it never fetches or replaces the binary.
- `get-config` treats `~/.arcrc` without a config section as empty user config.
- `look` uses the UTC hour for its time-of-day flavor.

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
