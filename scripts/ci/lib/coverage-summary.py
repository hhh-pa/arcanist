#!/usr/bin/env python3
"""Render an lcov.info coverage report as GitHub Markdown.

Usage: coverage-summary.py [--out COMMENT.md] <lcov.info>
  Prints the job-summary flavor to stdout; with --out, also writes the
  PR-comment flavor to FILE (parents created). Exits 0 with an
  "unavailable" note when the report is missing (an earlier gate failed
  before coverage ran).

Both flavors show the total line rate plus the 15 lowest-covered files;
the full HTML report rides along as the `coverage-report` workflow
artifact. The comment flavor adds the threshold verdict plus a link to
the run that produced it.
"""

import os
import sys

# Must match the floor enforced by rust-quality-gates.py (the
# coverage-floor-check.py invocation): the gates fail the run, this only
# reports the verdict.
THRESHOLD = 80.0


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


def run_url():
    """Link to the current Actions run, or None outside CI."""
    server = os.environ.get("GITHUB_SERVER_URL")
    repo = os.environ.get("GITHUB_REPOSITORY")
    run_id = os.environ.get("GITHUB_RUN_ID")
    if not (server and repo and run_id):
        return None
    return "%s/%s/actions/runs/%s" % (server, repo, run_id)


def table_lines(files):
    """The 15 lowest-covered files as Markdown table lines."""
    lines = ["| File | Lines | Hit | %% |", "| ---- | ----- | --- | - |"]
    rows = sorted(files, key=lambda f: (f[2] / f[1] if f[1] else 1.0, f[0]))
    for path, file_found, file_hit in rows[:15]:
        rate = 100.0 * file_hit / file_found if file_found else 100.0
        lines.append(
            "| `%s` | %d | %d | %.2f%% |"
            % (display_path(path), file_found, file_hit, rate)
        )
    lines += [
        "",
        "Lowest %d of %d files. Full HTML report: download the "
        "`coverage-report` artifact." % (min(15, len(rows)), len(rows)),
    ]
    return lines


def render_summary(total, files):
    """Job-summary flavor (byte-identical to the pre---out stdout)."""
    return "\n".join(
        ["## Coverage: %.2f%% (lines)" % total, ""] + table_lines(files)
    ) + "\n"


def render_comment(total, files):
    """PR-comment flavor: threshold verdict plus a link to the run."""
    verdict = "PASS" if total >= THRESHOLD else "FAIL"
    lines = [
        "## Coverage report: %.2f%% (lines)" % total,
        "",
        "Threshold %.0f%%: **%s** (enforced by the quality gates)."
        % (THRESHOLD, verdict),
        "",
    ] + table_lines(files)
    url = run_url()
    if url:
        lines += [
            "",
            "<sub>Updated by the [quality gates run](%s) on every push.</sub>"
            % url,
        ]
    else:
        lines += ["", "<sub>Posted by the quality gates.</sub>"]
    return "\n".join(lines) + "\n"


def main(argv):
    out = None
    args = list(argv)
    if "--out" in args:
        index = args.index("--out")
        try:
            out = args[index + 1]
        except IndexError:
            print(__doc__)
            return 2
        del args[index : index + 2]
    if len(args) != 1:
        print(__doc__)
        return 2
    try:
        found, hit, files = parse_lcov(args[0])
    except OSError:
        summary = (
            "## Coverage: unavailable\n"
            "\n"
            "No `lcov.info` was produced (an earlier gate failed first).\n"
        )
        comment = summary.replace("## Coverage:", "## Coverage report:")
    else:
        total = 100.0 * hit / found if found else 100.0
        summary = render_summary(total, files)
        comment = render_comment(total, files)
    sys.stdout.write(summary)
    if out:
        parent = os.path.dirname(os.path.abspath(out))
        os.makedirs(parent, exist_ok=True)
        with open(out, "w", encoding="utf-8") as handle:
            handle.write(comment)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
