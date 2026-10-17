# T1169: CODEOWNERS default reviewer — short spec (trivial change)

**Goal**: every PR automatically requests review from michael-h-hu via
`.github/CODEOWNERS` (`* @michael-h-hu`).

**Acceptance criteria**:
1. New PRs show michael-h-hu as a requested reviewer without manual steps.
2. Branch CI fully green; michael-h-hu review approves; squash-merge.
3. Master CI stays green; approval rule + allowlist behavior unchanged.
