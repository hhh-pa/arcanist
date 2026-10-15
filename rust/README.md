# arc-rs — Rust port of Arcanist

Tracks T1128. Goal: a single static `arc` binary matching the PHP `arc`
CLI behavior, ported workflow by workflow with `arc` as the reference.

## Status

All 35 commands are full-state ports of their PHP workflows. Discipline:
every gap-closing change updates this table in the same turn. Behavior
is exact, including error strings and exit codes; the remaining
edge divergences are listed below.

Console colors: plain-text output matches PHP, and ANSI styling matches
PHP `phutil_console_format` bytes on TTYs (plain text off-TTY).

| Command | State |
|---|---|
| `alias` | full (list/define shorthand; `!shell` targets; runs before dispatch; `--json`) |
| `amend` | full (git/hg/svn message amend; `-m`/`--revision`/`--show`) |
| `anoid` | full (passthru to support/arcanoid via python3) |
| `bookmarks` | full (hg markers grid via arc-hg; `--json`/`--conduit-uri`; git/svn fail like arc) |
| `branches` | full (git branch list + hg markers grid; `--all`/`--json`/`--conduit-uri`) |
| `browse` | full (open file/commit/revision in web browser; `--branch`/`--types`/`--force`) |
| `call-conduit` | full (direct HTTP/S; `~/.arcrc` token; stdin envelope) |
| `close-revision` | full (differential.close; `--finalize`/`--quiet`/`--json`/`--force`; author prompt) |
| `commit` | full (git + svn flows; `-m`/`--revision`/`--show`; finalize handoff) |
| `cover` | full (per-author changed-line blame; `--rev`/paths) |
| `diff` | full (local stat/patch/`--json`; message/range parsing; `--create`/`--update`/`--only` submit; git/hg/svn) |
| `download` | full (F123 download; `--as`/`--as -`; size-verified) |
| `export` | full (`--git`/`--unified`/arcbundle; local or revision export) |
| `get-config` | full (precedence read; `--show-source`; builtins; stock arcrc semantics) |
| `inspect` | full (working-copy tree; `--explore`; browse view) |
| `install-certificate` | full (token install; `--token`/`--force`; legacy exchange) |
| `land` | full (squash/merge strategies; `--preview`/`--push`; pre-land gates; `--revision`) |
| `liberate` | full (create/rebuild libphutil libs; always exits 0 like PHP; needs php) |
| `lint` | full (34 linters; per-linter config; severities; `.arclint`; `--json`; GEN/NOLINT) |
| `linters` | full (registry; `--verbose`/`--search`/exact; engine config) |
| `list` | full (open revs, `*` = current WC; `--json`; fails outside repos like PHP) |
| `look` | full (Arcventure clearing/remotes/published; local-tz hour like PHP) |
| `markers` | full (branch/bookmark grid; `--type`/`--json`; Differential status via conduit) |
| `paste` | full (read/create pastes; `--title`/`--lang`/`--input`) |
| `patch` | full (apply revision/diff/patch/arcbundle; `--preview`/`--force`) |
| `set-config` | full (user/`--local` write; typed coercion; empty deletes) |
| `tasks` | full (Maniphest table/`--json`; `--owner`/`--status`/`--order`/`--limit`) |
| `todo` | full (quick task create; `--browse`; `--cc`/`--project`/`--json`) |
| `unit` | full (engine selection; `.arcunit`/`unit.engine`; `--engine`/`--everything`; cargo/phpunit/xunit) |
| `upgrade` | full (version/status report; `--check`/`--json`; `--apply` self-updates with SHA-256 check) |
| `upload` | full (chunked upload + fallback; `--temporary`/`--json`) |
| `version` | full (toolset version info; git commit + date per library) |
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
| `XHP` | native 107/108 rules (XHP2 unable-to-parse; only XHP134 in `blocked_rules()`) |

External linters skip silently when their binary is missing. Per-linter
configuration (`bin`, `flags`, `version`, `interpreter`, standards,
rcfiles) is supported.

### Known divergences from PHP `arc`

- `XHP` ports 107/108 XHPAST rules natively; only XHP134 (product-name
  literals needing the libphutil runtime) remains, permanently unportable.
- XHP9/XHP30 enforce default determinations; custom `xhpast.switchhook` /
  `xhpast.naminghook` PHP classes cannot run here.
- XHP45 checks symbols against the vendored
  `rust/resources/php/symbol-information.json` snapshot instead of a
  live symbol table; XHP54 ports only the missing-format-string check
  (`xsprintf()` conversion validation needs libphutil and is not
  ported).
- `look` falls back to UTC where the OS zone is unreadable (for
  example Windows without `TZ`).
- `shell-complete` is not ported: the 666-line bash completion
  installer/engine is out of scope.
- `prompts` is not ported: per-workflow prompt introspection and
  saved-response review is out of scope.

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
