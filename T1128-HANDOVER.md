# T1128 handover: arc-rs rewrite (this branch only)

Everything needed to continue lives on the `m-hu/arcanist` fork. Local
worktrees, logs, and agent sessions did NOT transfer; recreate them below.

## Where things stand (2026-10-15, paused)

- Main line: `T1128-rust-rewrite` @ `79b3c5fa`, pushed, CI green except the
  macOS tempfile race already fixed in `7e47000f` (CI re-ran green after).
- All 32 commands full-state; complexity 0 violations, baseline empty.
- Divergences left: `upgrade` report-only, `look` UTC hour, XHP rule tree.
- Color wave + divergence wave in flight, PAUSED. Merged: `cl-a`, `dv-a`.
  WIP (pushed, unverified): `cl-b`, `cl-c`, `cl-d`, `dv-b`, `dv-c`.

## Branch map (all based on `7e47000f`)

| Branch | Owns (rust/src) | Status |
|---|---|---|
| `cl-a` | amend, bookmarks, branches, close_revision, commit, weld, main, lib | MERGED |
| `cl-b` | diff, diff_local, diff_submit, export, patch | WIP + snapshot |
| `cl-c` | land, land_checks, lint, linters, lint_*.rs except xhpast, unit* | WIP + snapshot |
| `cl-d` | anoid, browser, call_conduit, download, inspect, markers, paste, todo, upload, which | WIP commit |
| `dv-a` | liberate, list, get_config, look | MERGED (look untouched) |
| `dv-b` | upgrade (self-update) | WIP commit |
| `dv-c` | lint_xhpast + new xhp_*.rs (rule tree) | WIP + snapshot |

Color rule: ansi=true must match PHP `phutil_console_format` bytes;
ansi=false byte-identical to current. `terminal.rs`/`prompt.rs`/`grid.rs`
are read-only for all waves. Never touch `complexity-baseline.json`
(empty, must stay empty) or Cargo files (no new deps).

## New-machine setup

```sh
git clone https://github.com/m-hu/arcanist.git && cd arcanist
git checkout T1128-rust-rewrite
git fetch fork cl-b cl-c cl-d dv-b dv-c
for b in cl-b cl-c cl-d dv-b dv-c; do git worktree add .fanout4/$b -b $b 2>/dev/null || git worktree add .fanout4/$b $b; done
```

Toolchain: stable Rust + `clippy` + `rustfmt`. CI floats stable (it ran
1.99 while dev was 1.98 — CI is authoritative on new lints). Gate tools
(same pins as CI): `cargo install cargo-deny --version 0.20.2`,
`cargo-audit --version 0.22.2`, `rust-code-analysis-cli 0.0.25`
(`LLVM_COV 0.9.1`, `GEIGER 0.13.0`, `UDEPS 0.1.61`,
`NEXTEST 0.9.148`, `nightly-2026-10-01` for full local gates).
Copy `scripts/ci/rca-viol.ps1` usage: `powershell -ExecutionPolicy
Bypass -File scripts/ci/rca-viol.ps1 -Ws '<worktree>/rust'`.

Network: the old machine needed `https_proxy=http://courbevoie.hhh.pa:8080`
for GitHub API (git itself worked). Set per local environment. Secrets
(`CODECOV_TOKEN`, Docker Hub) already live in the repo; nothing to copy.

## Resume procedure (per branch)

1. In `.fanout4/<b>`: `git status`, `git log --oneline -3`; finish the
   owned work (briefs: color = wire ANSI + tty-gated flag + dual-mode
   tests; dv-b = safe self-update with mock-server tests on copies only;
   dv-c = port XHPAST rules from `bd12c2df:src/lint/linter/xhpast/`).
2. Verify in `<worktree>/rust`: `cargo test --offline` (full),
   `cargo clippy --offline --all-targets -- -D warnings`,
   `cargo fmt --check`, rca scan zero for owned files.
3. Commit on the branch (never push agent branches except WIP backup).
4. On `T1128-rust-rewrite`: scope-check
   (`git diff --name-only <base>..<b>` within owned files),
   `git merge --no-ff`, rescan, targeted tests. No conflicts expected
   (disjoint files, static base) — resolve by taking the correct side.
5. After all merge: full suite + clippy + fmt + deny + audit locally,
   push, watch CI (all platforms + quality gates + coverage floor 80%).

## Definition of done

All 5 branches merged, CI fully green (incl. coverage), root + rust
READMEs current (update the table with every gap closed), Maniphest
T1128 commented, `.fanout*` worktrees + agent branches deleted.

## Gotchas (learned the hard way)

- Working-copy files are CRLF: exact-match edit tools fail; use
  PowerShell line surgery, then `cargo fmt`.
- Never background shell commands; always wait (`yield_time_ms` high).
  Two stuck agents burned 8h+ in wait-loops before being reaped.
- `GAP2-misc/43` may show as a zombie `closing` agent; its worktree was
  reaped and its work (beed8a84) merged long ago. Ignore it.
- `cargo update`/`cargo audit` need the network; builds/tests are
  `--offline`-clean. `rsa` RUSTSEC-2023-0071 is a documented ignore
  (signing-only use, no fixed release exists).
