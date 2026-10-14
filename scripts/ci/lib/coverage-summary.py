#!/usr/bin/env python3
"""Render an lcov.info coverage report as GitHub job-summary Markdown.

Usage: coverage-summary.py <lcov.info>
  Prints Markdown to stdout; exits 0 with an "unavailable" note when the
  report is missing (an earlier gate failed before coverage ran).

Shows the total line rate plus the 15 lowest-covered files; the full HTML
report rides along as the `coverage-report` workflow artifact.
"""

import os
import sys


def parse_lcov(path):
    """Return (total_found, total_hit, [(display, found, hit)])."""
    files = []
    found = hit = 0
    current = None
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            line = line.rstrip("\n")
            if line.startswith("SF:"):
                current = [line[3:], 0, 0]
            elif line.startswith("LF:"):
                current[1] = int(line[3:])
            elif line.startswith("LH:"):
                current[2] = int(line[3:])
            elif line == "end_of_record" and current is not None:
                files.append(tuple(current))
                found += current[1]
                hit += current[2]
                current = None
    return found, hit, files


def display_path(path):
    """Trim container-absolute paths down to the workspace-relative tail."""
    marker = "rust/"
    index = path.replace(os.sep, "/").rfind(marker)
    if index >= 0:
        return path[index:]
    return os.path.basename(path)


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    try:
        found, hit, files = parse_lcov(sys.argv[1])
    except OSError:
        print("## Coverage: unavailable")
        print()
        print("No `lcov.info` was produced (an earlier gate failed first).")
        return 0
    total = 100.0 * hit / found if found else 100.0
    print("## Coverage: %.2f%% (lines)" % total)
    print()
    print("| File | Lines | Hit | %% |")
    print("| ---- | ----- | --- | - |")
    rows = sorted(files, key=lambda f: (f[2] / f[1] if f[1] else 1.0, f[0]))
    for path, file_found, file_hit in rows[:15]:
        rate = 100.0 * file_hit / file_found if file_found else 100.0
        print(
            "| `%s` | %d | %d | %.2f%% |"
            % (display_path(path), file_found, file_hit, rate)
        )
    print()
    print(
        "Lowest %d of %d files. Full HTML report: download the "
        "`coverage-report` artifact." % (min(15, len(rows)), len(rows))
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
