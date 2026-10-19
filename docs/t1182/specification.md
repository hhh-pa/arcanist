# T1182: Cut v0.3.1 release and install arc on host

Trivial change (5-line spec): bump `rust/Cargo.toml` (+ `Cargo.lock`)
`0.3.0` -> `0.3.1` so a fresh patch release can ship the T1181 tree
(`v0.3.0` is taken on both GitHub and crates.io). No test asserts the
version string (verified by grep); `version()` follows the manifest.
Acceptance: branch CI green -> land -> `cargo publish` 0.3.1 -> tag
`v0.3.1` -> release workflow attaches 3 binaries -> host
`~/.cargo/bin/arc --version` reports `0.3.1`.
