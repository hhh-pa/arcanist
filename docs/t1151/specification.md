# T1151: Resolve CodeQL code-scanning alerts

(Compact spec; no template in this repo. Task: https://orsay.hhh.pa/T1151.)

## Goal

Zero open code-scanning alerts: 3 Go archive errors fixed in
code, 5 workflow-permission warnings fixed in YAML, 2 Rust
cleartext-logging warnings dismissed as false positives with a
recorded reason.

## Fix 1 — Go extractor (alerts 6,7,8; error)

`installTarball`'s untar in `cmd/rust-quality-gates/main.go`
joins `hdr.Name`/`hdr.Linkname` onto the destination without a
CodeQL-accepted containment proof (symlink/hardlink targets
entirely unvalidated).

- Add `safeJoin(dest, name)`: reject absolute paths and any
  `..` escape (clean + prefix check), error naming the entry.
- Regular files/dirs resolve through it; symlinks additionally
  require the resolved link target to stay in dest; hardlinks
  require the source to stay in dest.
- Malicious archives fail closed (error, nothing outside dest).
- Tests: crafted tarballs (`..`, absolute, escaping symlink and
  hardlink) assert the error; a normal tarball still extracts.

## Fix 2 — workflow permissions (alerts 1–5; warning)

- Top-level `permissions: {contents: read}` in both workflows.
- `build-release` (rust.yml) and `build` (release.yml) add
  `actions: write` (artifact upload).
- `publish` (release.yml) adds `actions: write` next to its
  `contents: write` (artifact download; same-repo scope, and the
  job already holds release rights).
- `go-tools-test` (rust.yml) adds `actions: read`
  (binary-cache restore; the gates job owns the save).
- `quality-gates` keeps its existing block. `fmt-clippy` and
  `test` inherit read-only.

## Dismissals (alerts 9,10; warning, false positive)

Both log a *username* to the terminal, mirroring PHP output:
`installing_message` formats only `Installing certificate for
'{user}'...`; `land.rs` `push_out` echoes generic child stdout.
No secret material; redacting would break PHP parity. Dismiss
via API with reason `false positive` and this rationale.

## Acceptance criteria

1. `gofmt`/`vet`/`go test ./...` green (new extractor tests pass).
2. YAML parses; branch CI green (proves permission blocks suffice).
3. Code-scanning shows 0 open alerts afterward.

## Follow-up fixes (CI-driven)

- CodeQL did not accept the `safeJoin` helper as a sanitizer (2
  high annotations on the PR): containment checks are inline at
  each sink in canonical `filepath.Join` + `HasPrefix` form, with
  `dest` cleaned once at the top. Same behavior, same messages.
- Master CI exposed a live `run(&["--apply"])` integration test:
  publishing v0.1.0 flipped it FAILURE → SUCCESS (and a future
  release would make the test replace its own binary). Removed
  from the integration suite; apply paths stay covered by the
  hermetic `upgrade_tests`; `run` docs warn against re-adding.
- Post-merge default-branch analysis filed two new
  `go/unsafe-unzip-symlink` alerts (14, 15): that query credits
  no lexical barrier, only a header-derived path reaching
  `filepath.EvalSymlinks`. Link creation now gates on
  `resolvesWithinDir` (lenient resolution + containment in the
  resolved dest), which also catches redirects planted on disk
  by earlier entries.

## Slices

1. Spec (this file). 2. Go extractor + tests. 3. Workflow
   permissions. 4. Dismissals. 5. Push, PR, verify 0 open.
6. Inline extractor form + hermetic upgrade test.
7. Real link resolution (this section).
