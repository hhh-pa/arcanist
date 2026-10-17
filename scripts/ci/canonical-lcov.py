#!/usr/bin/env python3
"""Reproduce CI's canonical LCOV locally (T1170; dev-only helper).

CI's published lcov.info is NOT cargo-llvm-cov's output: rust-quality-gates
re-exports the same profile data with raw `llvm-cov export` and merges twin
path spellings (`tests/../src` -> `src`), recovering genuinely-executed lines
cargo's filter drops (see the T970 coverage note in
scripts/ci/go/cmd/rust-quality-gates/main.go). A plain local `cargo llvm-cov`
run therefore under-reports (72% vs CI's 93% on the same suite).

This script replicates that pipeline over an existing instrumented run:

    cargo llvm-cov nextest --run-ignored all --no-fail-fast   # or via gates
    python3 scripts/ci/canonical-lcov.py [out.lcov]

and prints the floor-style line % over the canonical record set — the same
number coverage-floor and codecov/project compute in CI. Validated to
reproduce CI's 92.89% (114,791/123,582, 211 files) to within 2 lines.

Run the test suite with an isolated HOME (the repo's hermetic pattern);
several tests read the user config layer and fail or mis-cover otherwise.
Stdlib only.
"""

import collections
import glob
import os
import posixpath
import subprocess
import sys

EXCLUDED = (
    "/tests/",
    "/benches/",
    "/examples/",
    "/target/",
    "/.cargo/",
    "/rustup/",
    "/rustc/",
)
CLAMP = 1 << 62


def run(cmd, cwd):
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if proc.returncode != 0:
        print("FAILED:", " ".join(cmd[:4]), "\n", proc.stderr[-2000:], file=sys.stderr)
        sys.exit(1)
    return proc.stdout


def main() -> None:
    top = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    rust = os.path.join(top, "rust")
    covtarget = os.path.join(rust, "target", "llvm-cov-target")
    sysroot = subprocess.run(
        ["rustc", "--print", "sysroot"], capture_output=True, text=True, check=True
    ).stdout.strip()
    found = sorted(glob.glob(os.path.join(sysroot, "lib", "rustlib", "*", "bin", "llvm-cov")))
    if not found:
        print("llvm-cov not found under rustc sysroot (need llvm-tools-preview)", file=sys.stderr)
        sys.exit(1)
    llvm_bin = os.path.dirname(found[0])
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(rust, "target", "llvm-cov", "canon.lcov.info")
    os.makedirs(os.path.dirname(out), exist_ok=True)

    profraws = sorted(
        os.path.join(covtarget, e) for e in os.listdir(covtarget) if e.endswith(".profraw")
    )
    if not profraws:
        print(f"no profraw files under {covtarget} (run an instrumented suite first)", file=sys.stderr)
        sys.exit(1)
    profdata = os.path.join(covtarget, "canonical.profdata")
    print(f"merging {len(profraws)} profraws...")
    run(
        [os.path.join(llvm_bin, "llvm-profdata"), "merge", "-sparse"] + profraws + ["-o", profdata],
        rust,
    )
    objs = []
    for sub in ("debug", "release"):
        deps = os.path.join(covtarget, sub, "deps")
        if not os.path.isdir(deps):
            continue
        for entry in os.listdir(deps):
            if "." in entry:
                continue
            path = os.path.join(deps, entry)
            if os.path.isfile(path) and os.access(path, os.X_OK):
                objs.append(path)
    if not objs:
        print(f"no test objects under {covtarget}", file=sys.stderr)
        sys.exit(1)
    print(f"exporting {len(objs)} objects...")
    raw = run(
        [os.path.join(llvm_bin, "llvm-cov"), "export", "-instr-profile=" + profdata, "--format=lcov"]
        + [item for obj in objs for item in ("-object", obj)],
        rust,
    )
    merged: dict[str, dict[int, int]] = collections.defaultdict(dict)
    current = None
    for line in raw.splitlines():
        if line.startswith("SF:"):
            path = line[3:]
            if not os.path.isabs(path):
                path = os.path.join(top, path)
            norm = posixpath.normpath(path)
            if not norm.startswith(top + "/"):
                current = None
                continue
            if any(part in norm for part in EXCLUDED):
                current = None
                continue
            current = norm
        elif line.startswith("DA:") and current is not None:
            lineno, count = line[3:].split(",")[:2]
            count = int(count)
            if count >= CLAMP:
                count = 0
            record = merged[current]
            lineno = int(lineno)
            record[lineno] = min(record.get(lineno, 0) + count, (1 << 63) - 1)
    hit = total = 0
    with open(out, "w") as handle:
        for sf in sorted(merged):
            handle.write(f"SF:{sf}\n")
            covered = 0
            for lineno in sorted(merged[sf]):
                count = merged[sf][lineno]
                handle.write(f"DA:{lineno},{count}\n")
                if count > 0:
                    covered += 1
            size = len(merged[sf])
            handle.write(f"LF:{size}\nLH:{covered}\nend_of_record\n")
            hit += covered
            total += size
    print(f"canonical: {hit}/{total} = {100 * hit / max(1, total):.2f}%  files: {len(merged)}")
    print("wrote", out)


main()
