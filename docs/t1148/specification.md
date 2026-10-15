# T1148: Port `scripts/ci/lib` from Python to Go

(Compact spec; no template in this repo. Task: https://orsay.hhh.pa/T1148.)

## Goal

Rewrite the five CI helpers in `scripts/ci/lib` (~1672 lines Python)
in Go with identical CLI contracts, stdout bytes, and exit codes,
then move the workflow callers over and remove the `.py` files.

## Layout

- New module `scripts/ci/go/` (`go.mod`, stdlib only).
- `cmd/coverage-floor`, `cmd/coverage-summary`, `cmd/geiger`,
  `cmd/complexity`, `cmd/rust-quality-gates` (one `main` each).
- `internal/lcov`: shared LCOV parse/merge/emit (the three existing
  parsers differ: floor merges DA-only, gates keep DA/BRDA/FNDA/FN,
  summary reads LF/LH — one package, per-tool adapters).
- Committed `*_test.go` per tool (golden + oracle-diff tests).

## CLI parity (each tool keeps its exact argv/exit contract)

| Tool | Usage | Exits |
|---|---|---|
| coverage-floor | `<lcov> [floor=80]` | 2 usage/bad-floor, 1 missing/below, 0 ok |
| coverage-summary | `[--out F] <lcov>` | 2 usage, 0 always (missing report = "unavailable" note) |
| geiger | `<workspace> [--regen]` | 2 usage/tool/baseline, 1 grew, 0 ok |
| complexity | `<workspace> [--regen]` | 2 usage/tool/baseline, 1 new/worsened, 0 ok |
| rust-quality-gates | `<ws> <report> <runner...>` / `<ws> --provision-only` | nonzero citing first failure |

Stdout must match the Python oracles byte-for-byte on fixtures
(formats like `%6.2f%%`, `%5d/%-5d`, `%.1fs` map directly to Go
`fmt`); `--regen` JSON uses `indent=1` equivalents (Go: 1-space
indent via custom marshal — `json.MarshalIndent` with prefix "" and
one space matches Python `indent=1`).

## Behavior notes

- Gates keep: pinned versions, prebuilt-then-compile provisioning,
  nextest-`run` refusal, deferred-exit `attempt`, llvm-cov seeding,
  canonical-LCOV superset validation with cargo fallback, absurd-count
  clamping, internal calls rewritten to sibling Go binaries (resolved
  next to the running executable, like `LIB_DIR`).
- `coverage-summary` keeps THRESHOLD=80 and the `GITHUB_*` run link.
- Complexity thresholds (cy>15/co>20) stay mirrored with
  `scripts/ci/complexity-scan.ps1` (post-T1128-cleanup name).

## CI caching design (Actions cache, no Docker image)

- `rust.yml` quality-gates job grows a Go preamble: `setup-go`
  (version from `scripts/ci/go/go.mod`, module cache off — the
  module is stdlib-only), `actions/cache` on `~/.cache/go-build`
  keyed by OS + `hashFiles('scripts/ci/go/**')`, then
  `gofmt`/`vet`/`test` and a `go build` of the five tools into
  `./ci-tools-bin`. No Go code change = warm cache + fast rebuild;
  no Docker image, no registry credentials involved.
- Gate invocation bind-mounts the host dir into the gate container
  (`docker run -v ./ci-tools-bin:/tools ... /tools/rust-quality-gates`);
  sibling tools resolve next to the running executable (the `LIB_DIR`
  analogue). The host summary step runs `./ci-tools-bin/coverage-summary`
  directly.
- `quality-gates.Dockerfile`: `--provision-only` via the Go binary
  (a `golang` build stage inside that Dockerfile compiles it from
  the same source; pins grep follows the Go source).
- Remove the `.py` files only when the Go path is green on CI.

## Verification

- Each tool: golden fixtures + diff against the Python oracle on the
  live tree (lcov from `rust/target/llvm-cov`, real baselines).
- Full `rust-quality-gates` live run compared log-against-log.
- CI impact: rust.yml Go preamble (+ a `golang` stage in the gate
  Dockerfile at caller-switch time); gate semantics unchanged.

## Slices

1. Spec (this file). 2. `internal/lcov` + floor + summary. 3. geiger
   + complexity. 4. rust-quality-gates. 5. Caller switch. 6. Remove
   `.py`, close.
