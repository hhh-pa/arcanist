# T1193: Cut v0.3.2 release

Version bump: `rust/Cargo.toml` (+ `rust/Cargo.lock`) `0.3.1` -> `0.3.2`
so a fresh patch release can ship #22 (test-only), #23
(zero-behavior-change cleanup), and #24 (prompt/dispatcher/upgrade
fixes). No test asserts the version string (verified by grep);
`version()` follows the manifest.

Workflow slice: `release.yml` gains a tag-gated `crates-publish` job
(`needs: build`, OIDC trusted publishing via
`rust-lang/crates-io-auth-action@v1`, `cargo publish --locked`) so
crates.io ships with the GitHub release instead of a manual step.
Prerequisite (owner, one-time, before pushing the tag): add this repo
as a trusted publisher for `arcanist-rs` on crates.io; without it the
job fails (re-runnable via failed-jobs rerun after setup).

Acceptance: branch CI green -> land -> tag `v0.3.2` -> release workflow
attaches 3 binaries AND publishes 0.3.2 to crates.io -> host
`~/.cargo/bin/arc --version` reports `0.3.2`.
