# T1147: Build and publish `arc` release binaries on tag push

(No `docs/SPEC_TEMPLATE.md` in this repo; compact spec per the
trivial-change rule. Task: https://orsay.hhh.pa/T1147.)

## Goal

Pushing a `v*` tag produces downloadable `arc` binaries for Linux,
macOS, and Windows on the GitHub release of the same name. Until now
CI built the release binary and threw it away; users had to build
from source.

## Behavior

- Given a `v*` tag push, when the `release` workflow runs, then each
  of ubuntu/macos/windows builds `arc --release --locked`,
  smoke-tests it (`--help` prints `usage:`), packages it as
  `arc-<host-target>.tar.gz`, and the archives are attached to the
  tag's GitHub release.
- Given a `workflow_dispatch` run (any branch, no tag), when the
  workflow runs, then the same three builds run and upload artifacts,
  and the publish job is skipped.
- Given a re-pushed tag whose release already exists, when publish
  runs, then assets are overwritten (`--clobber`), not duplicated.

## Acceptance criteria

1. `release.yml` exists with `push.tags: ["v*"]` + `workflow_dispatch`
   triggers and a tag-gated publish job (`contents: write` only there).
2. Package step fails closed: `set -euo pipefail`, executable bit
   asserted, `usage:` smoke asserted, `if-no-files-found: error`.
3. README install docs point at the Releases page.
4. After merge, a `workflow_dispatch` dry run from the default branch
   shows all three build jobs green and publish skipped (GitHub only
   resolves dispatch workflows from the default branch, so no dry run
   is possible pre-merge). No tag is cut by this task.

## Technical notes

- Toolchain/build commands mirror `rust.yml` (`dtolnay/rust-toolchain`,
  `cargo build --release --locked`); target triple from `rustc -vV`.
- Publishing uses preinstalled `gh` (`release create --generate-notes`
  / `release upload --clobber`), no third-party release action.
- `.tar.gz` on all three OSes (single code path; Windows opens them
  natively).
- CI impact: new workflow file only; no Jenkinsfile/.arclint changes.

## Slices

1. Spec (this file). 2. Workflow + README. 3. Push, dry-run, report.
