# T1149: Schedule master CI builds and refresh stale files

(Compact spec; no template in this repo. Task: https://orsay.hhh.pa/T1149.)

## Goal

1. Master builds daily (plus on-demand dispatch), so quiet days still
   validate the tree.
2. Every file untouched for over a year is triaged: deleted if
   obsolete, refreshed if stale-but-needed, kept with a recorded
   reason otherwise.

## Part 1 — scheduled builds (folded in, supersedes PR #7)

- `rust.yml` gains `schedule: "30 5 * * *"` (daily 05:30 UTC,
  default branch only, unfiltered matrix) and bare
  `workflow_dispatch`.
- Acceptance: YAML parses with all four triggers; PR CI green;
  PR-only steps already gate on event name.

## Part 2 — stale-file triage (10 files, 2014–2023 vintages)

| File | Last touched | Verdict |
|---|---|---|
| `.arcunit` | 2015 | DELETE: phutil/`.php`-only engines; cargo is the runner |
| `.arclint` | 2020 | DELETE: PHP linters + `src/**` paths that no longer exist |
| `.editorconfig` | 2023 | ADAPT: drop dead `src/**` sections; Rust 4-space, Go tabs, markdown no-trim exception |
| `support/init/init-script.php` | 2021 | KEEP: liberate presence contract |
| `support/lib/rebuild-map.php` | 2021 | KEEP: executed by `arc liberate` |
| `support/lib/extract-symbols.php` | 2021 | KEEP: liberate presence contract |
| `support/arcanoid/arcanoid.py` | 2021 | KEEP: executed by `arc anoid` |
| `support/hg/arc-hg.py` | 2023 | KEEP: hg extension script located by markers |
| `LICENSE` | 2015 | KEEP: Apache 2.0, matches README claim |
| `NOTICE` | 2014 | KEEP: Phacility attribution, legally required |

Deletion safety: repo tests use `rust/tests/fixtures` plus
temp-dir fixtures, never the repo-root configs (verified by grep
plus a base-vs-head coverage diff, which caught one implicit
cwd dependence in `misc2_commands.rs`, since fixed); CI reads
neither file.

## Acceptance criteria

1. Daily cron + dispatch land in `rust.yml`; PR CI green.
2. `.arcunit`/`.arclint` gone; `.editorconfig` matches the current
   tree (Rust/Go/md/yml); nothing else deleted.
3. Full Rust suite + clippy/fmt still green (deletions touch no code).
4. PR #7 closed as superseded with a pointer.

## Slices

1. Spec (this file). 2. Cherry-pick scheduled change. 3. Sweep.
4. Push, PR, close #7, report.
