# T1193: Cut v0.3.2 release

Trivial change (5-line spec): bump `rust/Cargo.toml` (+ `rust/Cargo.lock`)
`0.3.1` -> `0.3.2` so a fresh patch release can ship #22 (test-only),
#23 (zero-behavior-change cleanup), and #24 (prompt/dispatcher/upgrade
fixes). No test asserts the version string (verified by grep);
`version()` follows the manifest. `cargo publish` stays with hhu (no
crates.io credentials here).
Acceptance: branch CI green -> land -> tag `v0.3.2` -> release workflow
attaches 3 binaries -> host `~/.cargo/bin/arc --version` reports `0.3.2`.
