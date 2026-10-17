# T1168: repo-path rename — short spec (trivial change)

**Goal**: after the `m-hu/arcanist` → `hhh-pa/arcanist` transfer, no live-tree
reference to the old path remains (Go module + imports, upgrade default
release repo, README link). Pure rename, no behavior change.

**Acceptance criteria**:
1. `m-hu/arcanist` occurs nowhere outside `.git/`/stale worktrees.
2. Branch CI fully green (incl. `go ci-tools tests`, CodeQL, Codecov statuses
   reporting post-transfer).
3. michael-h-hu review approves; squash-merge; master CI stays green.
