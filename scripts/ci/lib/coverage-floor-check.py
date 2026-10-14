#!/usr/bin/env python3
"""LCOV line-coverage floor check (T970): the 80% status quo, enforced.

Fails when a workspace's published LCOV line coverage drops below the
floor, so attained coverage can never silently regress. Used for every
Rust workspace (called at the end of rust-quality-gates.py coverage
reporting, on the final canonicalized file) and for the frontend (called
from the Jenkinsfile Frontend stage on jest's lcov.info).

Only DA (line) records count: the floor is a line-coverage floor, the
same metric the Jenkins Coverage plugin trends. Branch/function data is
ignored.

Usage: coverage-floor-check.py <lcov.info> [floor=80.0]

Python 3.9 compatible, stdlib only. Linux + macOS (local proxies).
"""

import os
import sys


def parse_lcov_lines(text):
    """{sf: ({line: count})} merging duplicate records per file."""
    files = {}
    sf = None
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("SF:"):
            sf = line[3:]
            files.setdefault(sf, {})
        elif line == "end_of_record":
            sf = None
        elif sf is not None and line.startswith("DA:"):
            parts = line[3:].split(",")
            if len(parts) < 2:
                continue
            try:
                count = int(parts[1])
            except ValueError:
                continue
            slot = files[sf]
            slot[parts[0]] = slot.get(parts[0], 0) + count
    return files


def main():
    if len(sys.argv) < 2 or len(sys.argv) > 3:
        print("Usage: coverage-floor-check.py <lcov.info> [floor=80.0]")
        return 2
    path = sys.argv[1]
    try:
        floor = float(sys.argv[2]) if len(sys.argv) > 2 else 80.0
    except ValueError:
        print("floor must be a number, got %r" % sys.argv[2])
        return 2
    if not os.path.isfile(path):
        print("coverage floor: %s not found" % path)
        return 1
    with open(path, errors="replace") as fh:
        files = parse_lcov_lines(fh.read())
    rows = []
    hit = tot = 0
    for sf, das in files.items():
        h = sum(1 for c in das.values() if c > 0)
        t = len(das)
        rows.append((h / max(1, t), h, t, sf))
        hit += h
        tot += t
    pct = 100 * hit / max(1, tot)
    print(
        "coverage floor: %.2f%% lines (%d/%d, %d files) vs %.2f%% floor (%s)"
        % (pct, hit, tot, len(rows), floor, path)
    )
    if pct < floor:
        print("per-file table (lowest first):")
        for p, h, t, sf in sorted(rows):
            print("  %6.2f%% %5d/%-5d %s" % (100 * p, h, t, sf))
        print(
            "FAIL: line coverage %.2f%% is below the %.2f%% floor" % (pct, floor)
        )
        return 1
    print("coverage floor: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
