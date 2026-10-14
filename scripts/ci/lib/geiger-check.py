#!/usr/bin/env python3
"""cargo-geiger gate (T968): fail when workspace `unsafe` grows.

geiger 0.13 cannot run on virtual manifests, so this runs it once per
workspace member and sums each member's own unsafe counters (used + unused,
so used/unused reclassification alone never trips the gate). Crates whose
bins carry required-features (backend-worker, uniffi-bindgen) get those
features enabled from cargo metadata, so the bins are analyzed too.

The sum is compared against <workspace>/geiger-baseline.json: a higher total
fails, a lower one prints a tighten-at-leisure note. Dependency unsafe is
reported (informational) but never gated: new deps routinely allow unsafe.

Usage: geiger-check.py <workspace> [--regen]
  --regen rewrites the baseline instead of checking (adoption/updates only).

Requires cargo-geiger on PATH. Python 3.9 compatible.
"""
import json
import os
import subprocess
import sys

COUNTERS = ("functions", "exprs", "item_impls", "item_traits", "methods")


def member_unsafe_total(doc, name):
    hits = [
        p for p in doc.get("packages", []) if p["package"]["id"]["name"] == name
    ]
    if len(hits) != 1:
        raise SystemExit(
            "expected exactly 1 geiger entry for %s, found %d"
            % (name, len(hits))
        )
    unsafety = hits[0]["unsafety"]
    total = 0
    for side in ("used", "unused"):
        for counter in COUNTERS:
            total += unsafety[side][counter]["unsafe_"]
    return total


def main():
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help"):
        print(__doc__)
        return 2
    workspace = os.path.abspath(sys.argv[1])
    regen = len(sys.argv) > 2 and sys.argv[2] == "--regen"
    baseline_path = os.path.join(workspace, "geiger-baseline.json")
    target_dir = os.path.join(workspace, "target-geiger")

    meta = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        capture_output=True,
        text=True,
        cwd=workspace,
    )
    if meta.returncode != 0:
        print("cargo metadata failed:\n" + meta.stderr[-2000:])
        return 2
    members = json.loads(meta.stdout)["packages"]

    total = 0
    per_crate = []
    dep_unsafe_names = set()
    member_names = set(p["name"] for p in members)
    # Apple's python injects SDKROOT/CPATH/LIBRARY_PATH into os.environ; they
    # pin the macOS SDK/linker to a combination whose .tbd files the linker
    # rejects (tapi errors). CI never sets them; drop them for parity.
    env = {
        k: v
        for k, v in os.environ.items()
        if k not in ("SDKROOT", "CPATH", "LIBRARY_PATH")
    }
    env["CARGO_TARGET_DIR"] = target_dir
    for pkg in sorted(members, key=lambda p: p["name"]):
        features = sorted(
            {
                f
                for t in pkg["targets"]
                for f in (t.get("required-features") or [])
            }
        )
        cmd = [
            "cargo",
            "geiger",
            "--manifest-path",
            pkg["manifest_path"],
            "--output-format",
            "Json",
        ]
        if features:
            cmd += ["--features", " ".join(features)]
        print("geiger %s..." % pkg["name"], flush=True)
        r = subprocess.run(cmd, capture_output=True, text=True, env=env)
        if r.returncode != 0:
            print("cargo geiger failed for %s: %s" % (pkg["name"], " ".join(cmd)))
            errors = [ln for ln in r.stderr.splitlines() if ln.startswith("error")]
            if errors:
                print("\n".join(errors[:20]))
            print("--- stderr tail ---")
            print(r.stderr[-4000:])
            return 2
        try:
            doc = json.loads(r.stdout)
        except ValueError:
            print("cargo geiger emitted no JSON for %s" % pkg["name"])
            return 2
        own = member_unsafe_total(doc, pkg["name"])
        per_crate.append((pkg["name"], own))
        total += own
        for p in doc.get("packages", []):
            n = p["package"]["id"]["name"]
            if n in member_names:
                continue
            u = p["unsafety"]
            if any(
                u[side][c]["unsafe_"] > 0
                for side in ("used", "unused")
                for c in COUNTERS
            ):
                dep_unsafe_names.add(n)

    for name, own in per_crate:
        print("  unsafe in %-45s %d" % (name, own))
    print(
        "geiger: workspace unsafe total %d; deps allowing unsafe: %d (informational)"
        % (total, len(dep_unsafe_names))
    )

    if regen:
        base = {
            "_comment": (
                "cargo-geiger baseline (T968): total unsafe items/expressions "
                "summed over workspace crates (used + unused). CI fails when "
                "the total grows; lower it when unsafe is removed."
            ),
            "unsafe_total": total,
        }
        with open(baseline_path, "w") as fh:
            fh.write(json.dumps(base, indent=1) + "\n")
        print("%s: wrote unsafe_total=%d" % (workspace, total))
        return 0

    try:
        with open(baseline_path) as fh:
            baseline = json.load(fh)["unsafe_total"]
    except (OSError, ValueError, KeyError) as e:
        print("cannot read baseline %s: %s" % (baseline_path, e))
        return 2
    if total > baseline:
        print(
            "FAIL: workspace unsafe total grew %d -> %d" % (baseline, total)
        )
        return 1
    if total < baseline:
        print(
            "note: unsafe total fell %d -> %d (tighten the baseline at leisure)"
            % (baseline, total)
        )
    print("geiger: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
