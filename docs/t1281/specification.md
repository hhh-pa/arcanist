# T1281: squash land after master merge-back + `url` RUSTSEC-2026-0592

## Goal

1. `arc land --onto master --strategy squash --revision <rev>` succeeds
   when the feature branch merged `master` back into itself, instead of
   failing the squash with `Encountered a merge conflict.` even though
   the merge-back already resolved every conflict.
2. Turn scheduled `master` CI green again after run
   [45027758185](https://github.com/hhh-pa/arcanist/actions/runs/45027758185),
   where quality gates fail on `cargo audit --deny warnings`:
   `url 2.5.8`, RUSTSEC-2026-0592 (stack overflow in `Url::origin()`
   on nested `blob:` URLs) — fix is `url >= 2.5.9`.

## Root cause (bug 1)

`git_execute_merge` (`rust/src/land.rs`) under squash rebases the
`min^..max` range onto the into-commit before squash-merging
(`git_apply_rebase`: `checkout max`, `rebase --onto <into> --
<min>^`). When the range contains a merge-back of the target, the
rebase replays pre-merge commits one by one and re-hits conflicts the
merge commit already resolved; the failure maps to the `MERGE
CONFLICT` / `Encountered a merge conflict.` error. A direct
squash-merge of the range tip would apply cleanly (into is an ancestor
of the tip, or a true 3-way when the target moved again after the
merge-back).

Reproduced: feature edits `file.txt`, master edits the same line,
feature merges master (conflict resolved in the merge), then
`--onto master --strategy squash --revision D1` fails with
`Encountered a merge conflict.` Non-conflicting merge-backs already
land (rebase drops the already-upstream commits), which is why only
the resolved-conflict shape fails.

## Fix

- **Bug 1 (`rust/src/land.rs`, `apply_merge_range`):** when a merge
  commit in the squash range `min^..max` has a parent already reachable
  from the into-commit (`git log --merges --format=%P` +
  `git merge-base --is-ancestor`), skip the rebase and squash-merge the
  range tip (`max`) directly onto the into-commit with the existing
  squash argv (`--ff --squash`). This covers the into target merged back
  in, including when the target moved again afterwards (then the direct
  merge is a true 3-way). Linear ranges — and ranges merging unrelated
  branches, whose rebase-first failure stays pinned by LAND-43
  (`land43_merge_commit_range_conflicts_like_ceb`) — keep the exact ceb
  git-command sequence; only the previously-failing merge-back shape is
  rerouted, and a genuine conflict there still reports through the
  existing `MERGE CONFLICT` path. Detection-command failure falls back
  to the rebase path (current behavior preserved).
- **Deviation record (T1153 Rule 2):** PHP @ `ceb082ef`
  (`ArcanistGitLandEngine::executeMerge()`, `:322-345`) runs the same
  rebase-first squash and fails identically on this shape (verified
  from source: same `rebase --onto` + `merge --ff --squash` sequence).
  PHP arc is frozen (last commit May 2023), so this spurious failure —
  rejecting content that merges cleanly — is fixed in the port rather
  than kept. Linear-range behavior and all banners are unchanged.
- **Bug 2 (`rust/Cargo.lock` only):** `cargo update -p url` to
  `>= 2.5.9` (`url` is transitive; no `Cargo.toml` change). Fix any
  `cargo deny` fallout if the bump splits a duplicated crate.
- **Enabling fix — gate image rebuild broke the coverage floor
  (found on branch CI):** the image key hashes `rust/Cargo.lock`, so
  the bug-2 bump forced a rebuild from `rust:slim-trixie`, which had
  floated from 1.99.1 to 1.100.0. Rust 1.100 (a) moved test binaries
  out of `target/<profile>/deps/` into per-unit
  `build/<pkg>/<unit>/out/` dirs, so canonical coverage found 0 of 41
  objects and fell back to the 98.57% raw report; and (b) maps
  coverage regions more finely, exposing ~400 never-executed lines
  the 1.99 mapping marked covered (verified: new image finds 0
  objects vs 41 on the old image; local 1.100 gate run recovers with
  discovery fixed but still reports 99.72%). Fix: pin the base to
  `rust:1.99.1-slim-trixie` (documented in the Dockerfile with the
  unpin condition) AND teach `rawLLVMCovExport` both layouts (new
  `scanTestObjects`, skipping `build_script_build` by name), so the
  next unpin only needs the suite re-baselined. Re-baselining the
  suite to 100% under the 1.100 mapping is a separate follow-up task,
  explicitly out of scope here.

## Acceptance criteria

1. New test `t1281_squash_land_tolerates_master_merged_into_feature`
   (`rust/tests/land_workflow.rs`, online engine + mock Conduit):
   conflicting merge-back lands via
   `--onto master --strategy squash --revision D1`; remote master
   gains exactly the squash commit with the `Land feature` message.
2. Existing land tests pass unchanged, including
   `land43_squash_rebase_failure_is_a_conflict` (linear-range rebase
   failure still maps to a conflict) and `lands_squash_onto_master`.
3. `cargo audit --deny warnings` and
   `cargo deny check licenses bans sources` pass.
4. `cargo fmt --all --check`, `cargo test --locked`,
   `cargo clippy --all-targets -- -D warnings` green; branch CI green.

## CI impact

No Jenkinsfile/.arclint changes needed (GitHub Actions repo; no
`.github/workflows` changes). `scripts/ci` changes: gate Dockerfile
base pin (`rust:1.99.1-slim-trixie`) + `scanTestObjects` discovery in
`rust-quality-gates` with stub/golden updates (enabling fix above).
Gates are the `rust` workflow checks plus Codecov statuses on the PR.
