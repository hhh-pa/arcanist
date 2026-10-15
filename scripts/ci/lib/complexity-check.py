#!/usr/bin/env python3
"""rust-code-analysis gate (T968): fail on new/worsened complexity violations.

Collects per-function cyclomatic/cognitive metrics over every .rs file in the
workspace (excluding build output) and compares named-function violations
(cyclomatic > 15 or cognitive > 20) against the checked-in
<workspace>/complexity-baseline.json ratchet:

* a violation not in the baseline -> error (fix the function or, with a
  reviewer, extend the baseline)
* a baselined violation whose metrics grew -> error
* a baseline entry that no longer violates -> note (prune at leisure)

Fail-closed: no .rs files collected, an empty result against a non-empty
baseline, or any unparseable rca output file is an error, never a silent
OK — a green gate must mean "analyzed, nothing new", not "analyzed
nothing".

Usage: complexity-check.py <workspace-dir> [--regen]
  --regen rewrites the baseline instead of checking (adoption/updates only).

Requires rust-code-analysis-cli on PATH. Python 3.9 compatible (no tomllib).
"""
import json
import os
import subprocess
import sys
import tempfile

CY_THRESHOLD = 15
CO_THRESHOLD = 20


def walk(spaces, stack, path, out):
    for s in spaces or []:
        kind = s.get("kind")
        name = s.get("name", "?")
        if kind == "function" and name != "<anonymous>":
            m = s.get("metrics", {})
            cy = m.get("cyclomatic", {}).get("sum", 0)
            co = m.get("cognitive", {}).get("sum", 0)
            if cy > CY_THRESHOLD or co > CO_THRESHOLD:
                out.append(
                    {
                        "file": path,
                        "function": "::".join(stack + [name]),
                        "cyclomatic": cy,
                        "cognitive": co,
                    }
                )
            walk(s.get("spaces"), stack + [name], path, out)
        elif kind == "closure":
            # Parent metrics are inclusive of closures (verified across the
            # tree), so gating named functions alone covers them without
            # line-anchored brittleness.
            walk(s.get("spaces"), stack, path, out)
        else:
            extra = [name] if name and name != "?" else []
            walk(s.get("spaces"), stack + extra, path, out)


def collect(workspace):
    rss = []
    for root, dirs, files in os.walk(workspace):
        dirs[:] = [
            d
            for d in dirs
            if d != ".git" and d != "target" and not d.startswith("target-")
        ]
        for fn in files:
            if fn.endswith(".rs"):
                rss.append("./" + os.path.relpath(os.path.join(root, fn), workspace))
    rss.sort()
    if not rss:
        print("no .rs files collected under %s (wrong workspace?)" % workspace)
        sys.exit(2)
    with tempfile.TemporaryDirectory(prefix="rca-") as outdir:
        cmd = ["rust-code-analysis-cli", "-m", "-O", "json", "-o", outdir]
        for f in rss:
            cmd += ["-p", f]
        r = subprocess.run(cmd, capture_output=True, text=True, cwd=workspace)
        if r.returncode != 0:
            print("rust-code-analysis-cli failed:\n" + r.stderr[-2000:])
            sys.exit(2)
        viol = []
        skipped = []
        for root, _dirs, files in os.walk(outdir):
            for fn in files:
                if not fn.endswith(".json"):
                    continue
                with open(os.path.join(root, fn)) as fh:
                    try:
                        d = json.load(fh)
                    except ValueError:
                        skipped.append(os.path.join(root, fn))
                        continue
                # rca mirrors input paths under outdir; map back to the
                # workspace-relative source path.
                rel = os.path.relpath(os.path.join(root, fn)[:-5], outdir)
                if rel.startswith("./"):
                    rel = rel[2:]
                walk(d.get("spaces", []), [], rel, viol)
        if skipped:
            print(
                "unparseable rca output (%d file(s), blind spot):\n%s"
                % (len(skipped), "\n".join(sorted(skipped)[:10]))
            )
            sys.exit(2)
    viol.sort(key=lambda v: (v["file"], v["function"]))
    return viol


def key(v):
    return (v["file"], v["function"])


def main():
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help"):
        print(__doc__)
        return 2
    workspace = os.path.abspath(sys.argv[1])
    regen = len(sys.argv) > 2 and sys.argv[2] == "--regen"
    baseline_path = os.path.join(workspace, "complexity-baseline.json")

    current = collect(workspace)
    cur_map = {key(v): v for v in current}

    if regen:
        base = {
            "_comment": (
                "rust-code-analysis baseline (T968): named functions with "
                "cyclomatic>%d or cognitive>%d. CI fails on NEW violations "
                "or worsened ones; prune entries that no longer violate."
                % (CY_THRESHOLD, CO_THRESHOLD)
            ),
            "violations": current,
        }
        with open(baseline_path, "w") as fh:
            fh.write(json.dumps(base, indent=1) + "\n")
        print("%s: wrote %d baseline violations" % (workspace, len(current)))
        return 0

    try:
        with open(baseline_path) as fh:
            base_map = {key(v): v for v in json.load(fh)["violations"]}
    except (OSError, ValueError, KeyError) as e:
        print("cannot read baseline %s: %s" % (baseline_path, e))
        return 2

    if not cur_map and base_map:
        print(
            "FAIL: collected zero violations but the baseline lists %d "
            "(tool failure or mass fix?); regen explicitly if the code is "
            "really clean" % len(base_map)
        )
        return 1

    failed = False
    for k in sorted(set(cur_map) - set(base_map)):
        v = cur_map[k]
        print(
            "NEW complexity violation: %s:%s cyclomatic=%s cognitive=%s"
            % (v["file"], v["function"], v["cyclomatic"], v["cognitive"])
        )
        failed = True
    for k in sorted(set(cur_map) & set(base_map)):
        c, b = cur_map[k], base_map[k]
        if c["cyclomatic"] > b["cyclomatic"] or c["cognitive"] > b["cognitive"]:
            print(
                "WORSENED complexity violation: %s:%s was (cy=%s, co=%s), now (cy=%s, co=%s)"
                % (
                    c["file"],
                    c["function"],
                    b["cyclomatic"],
                    b["cognitive"],
                    c["cyclomatic"],
                    c["cognitive"],
                )
            )
            failed = True
    stale = sorted(set(base_map) - set(cur_map))
    if stale:
        print(
            "note: %d baseline entr%s no longer violate (prune at leisure)"
            % (len(stale), "y" if len(stale) == 1 else "ies")
        )
    worst = sorted(
        current, key=lambda v: -(v["cyclomatic"] + v["cognitive"])
    )[:10]
    print("--- most complex functions ---")
    for v in worst:
        print(
            "  %s:%s cyclomatic=%s cognitive=%s"
            % (v["file"], v["function"], v["cyclomatic"], v["cognitive"])
        )
    print(
        "complexity: %d violations (%d baselined) -> %s"
        % (len(current), len(base_map), "FAIL" if failed else "OK")
    )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
