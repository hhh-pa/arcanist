# arc-rs — Rust port of Arcanist

Tracks T1128. Goal: a single static `arc-rs` binary matching the PHP `arc`
CLI behavior, ported workflow by workflow with `arc` as the reference.

## Status

All 32 commands are full-state ports of their PHP workflows. Discipline:
every gap-closing change updates this table in the same turn. `full*`
marks commands with a documented edge divergence (listed below); behavior
is otherwise exact, including error strings and exit codes.

Console colors: plain-text output matches PHP; the ANSI styling pass is
still in progress, so colored output may differ from `arc` for now.

| Command | State |
|---|---|
| `amend` | full (git/hg/svn message amend; `-m`/`--revision`/`--show`) |
| `anoid` | full (passthru to support/arcanoid via python3) |
| `bookmarks` | full (hg markers grid via arc-hg; `--json`/`--conduit-uri`; git/svn fail like arc) |
| `branches` | full (git branch list + hg markers grid; `--all`/`--json`/`--conduit-uri`) |
| `call-conduit` | full (direct HTTP/S; `~/.arcrc` token; stdin envelope) |
| `close-revision` | full (differential.close; `--finalize`/`--quiet`/`--json`/`--force`; author prompt) |
| `commit` | full (git + svn flows; `-m`/`--revision`/`--show`; finalize handoff) |
| `cover` | full (per-author changed-line blame; `--rev`/paths) |
| `diff` | full (local stat/patch/`--json`; message/range parsing; `--create`/`--update`/`--only` submit; git/hg/svn) |
| `download` | full (F123 download; `--as`/`--as -`; size-verified) |
| `export` | full (`--git`/`--unified`/arcbundle; local or revision export) |
| `get-config` | full* (precedence read; `--show-source`; builtins; see arcrc divergence) |
| `inspect` | full (working-copy tree; `--explore`; browse view) |
| `install-certificate` | full (token install; `--token`/`--force`; legacy exchange) |
| `land` | full (squash/merge strategies; `--preview`/`--push`; pre-land gates; `--revision`) |
| `liberate` | full* (create/rebuild libphutil libs; needs php; see exit-code divergence) |
| `lint` | full (34 linters; per-linter config; severities; `.arclint`; `--json`; GEN/NOLINT) |
| `linters` | full (registry; `--verbose`/`--search`/exact; engine config) |
| `list` | full* (open revs, `*` = current WC; `--json`; see outside-repo divergence) |
| `look` | full* (Arcventure clearing/remotes/published; see UTC-hour divergence) |
| `markers` | full (branch/bookmark grid; `--type`/`--json`; Differential status via conduit) |
| `paste` | full (read/create pastes; `--title`/`--lang`/`--input`) |
| `patch` | full (apply revision/diff/patch/arcbundle; `--preview`/`--force`) |
| `set-config` | full (user/`--local` write; typed coercion; empty deletes) |
| `tasks` | full (Maniphest table/`--json`; `--owner`/`--status`/`--order`/`--limit`) |
| `todo` | full (quick task create; `--browse`; `--cc`/`--project`/`--json`) |
| `unit` | full (engine selection; `.arcunit`/`unit.engine`; `--engine`/`--everything`; cargo/phpunit/xunit) |
| `upgrade` | report-only (version/status + manual steps; `--check`/`--json`; no self-update) |
| `upload` | full (chunked upload + fallback; `--temporary`/`--json`) |
| `weld` | full (fuse files; +`--seed` deterministic mode) |
| `which` | full (context report; `--head`/`--show-base`/`--json`) |
| `work` | full (checkout/create local branch; `--start`) |

### Linters (all 34 PHP lint names ported)

| Name | Behavior |
|---|---|
| `TXT` | native (TXT2/TXT4/TXT6) |
| `NAME` | native (NAME1 boring file names) |
| `JSON` (native) | native serde_json parse check |
| `JSON` (jsonlint) | external jsonlint, falls back to native when uninstalled |
| `XML` | native well-formedness (no DTD/namespaces) |
| `MERGECONFLICT` | native conflict-marker scan |
| `GEN` / `NOLINT` | markers (stop all linters for the file) |
| `CHMOD` | exec-bit + shebang check |
| `SPELL` | native dictionary (curated subset default; custom dict supported) |
| `S&RX` | script-and-regex runner (caller-supplied matcher) |
| `COMPOSER` | stale composer.lock MD5 check |
| `PHL` | phutil PHL3 per-file + PHL1/2/4 map helpers |
| `JSHint` `JSCS` `CSSLint` `LESSC` `GJSLINT` `COFFEE` | external JS/CSS tools (silent if uninstalled; rc/flags config supported) |
| `flake8` `PYFLAKES` `PyLint` `PEP8` `RuboCop` `RUBY` | external Python/Ruby tools (silent if uninstalled; config supported) |
| `cppcheck` `C++ Google's Styleguide` `GOLINT` `HLINT` `C#` `PHPCS` `PHP` `PUPPETLINT` | external compiled-language tools (silent if uninstalled; config supported) |
| `XHP` | external xhpast wrapper only: XHP2 unable-to-parse; rule tree not ported |

External linters skip silently when their binary is missing. Per-linter
configuration (`bin`, `flags`, `version`, `interpreter`, standards,
rcfiles) is supported.

### Known divergences from PHP `arc`

- `liberate` exits non-zero on failure (PHP always exits 0).
- `list` works outside a git repo with no markers (PHP fails).
- `upgrade` is report-only; it never fetches or replaces the binary.
- `get-config` treats `~/.arcrc` without a config section as empty user config.
- `look` uses the UTC hour for its time-of-day flavor.
- `XHP` reports parse failures only; the XHPAST rule tree is not ported.

## Gates

```sh
cargo fmt --check
cargo build
cargo test
cargo clippy -- -D warnings
```

Run from this directory. CI additionally enforces the complexity ratchet
(`complexity-baseline.json` must stay empty), coverage floor, `cargo udeps`,
`cargo bans`, `cargo geiger`, and `cargo audit` via the quality-gates job.
