#!/usr/bin/env python3
"""Rust quality gates (T968): audit, deny, udeps, geiger, complexity, coverage.

Usage: rust-quality-gates.py <workspace-dir> <report-dir> <cov-runner> [cov-args...]
  workspace-dir: Rust workspace root (Cargo.toml, Cargo.lock, deny.toml,
    .cargo/audit.toml, *-baseline.json)
  report-dir: where coverage reports land (lcov.info + html/)
  cov-runner + cov-args: how llvm-cov runs the suite, e.g.
    nextest --profile ci --run-ignored all
    test --workspace
  NOTE: for nextest, OMIT the run subcommand. `cargo llvm-cov nextest`
  calls `cargo nextest run` internally, so a literal run would be treated
  as a test-name filter and the coverage run would silently execute only
  the tests with "run" in their name. The script refuses such args.

Runs every gate plus coverage on every build; exits nonzero citing the
first failure (a red gate never hides the others' signal, and coverage —
report-only observability — always publishes so Jenkins trends never gap).
Tools are pinned below and provisioned idempotently (prebuilt binaries
into the stage's cargo bin with a cargo-install fallback when a prebuilt
cannot execute here — upstream gnu builds may need newer glibc than
bookworm's 2.36 — plus rustup components).
Linux-only:
every call site is a Linux container. Python 3.9 compatible, stdlib only.

Coverage note (T970): rustc records #[path]-included modules — the
headless GUI tours in binary crates — under `<crate>/tests/../src/*.rs`,
which cargo-llvm-cov 0.9.1 silently drops from every report. The LCOV
step therefore re-exports from the same profile data with raw llvm-cov
and merges those records onto their canonical paths (pure test sources
stay excluded), validated as a line-superset of cargo's view with a
fallback to cargo's reports. The recovered lines are genuinely executed
by the suite; without this the binary workspaces report ~10-66% for
suites that actually cover >80%. Execution counts above 2**62 (llvm
occasionally emits u64::MAX on async-block boundaries) are clamped to 0
on every published LCOV — they crash the Coverage plugin's parser.
"""
import hashlib
import os
import platform
import posixpath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request

# Pinned tool versions. Bump deliberately (see CLAUDE.md toolchain pinning):
# unpinned "latest" would let a new tool release break master, and these
# tools judge code content the same way rustfmt/clippy do.
AUDIT_VERSION = "0.22.2"
DENY_VERSION = "0.20.2"
LLVM_COV_VERSION = "0.9.1"
GEIGER_VERSION = "0.13.0"
UDEPS_VERSION = "0.1.61"
RCA_VERSION = "0.0.25"
UDEPS_NIGHTLY = "nightly-2026-10-01"

LIB_DIR = os.path.dirname(os.path.abspath(__file__))


def machine_arch():
    m = platform.machine()
    # Arcanist import: Windows reports AMD64 for x86_64.
    if m in ("x86_64", "AMD64"):
        return "x86_64"
    if m in ("aarch64", "arm64"):
        return "aarch64"
    raise SystemExit("unsupported arch: %s" % m)


def fetch(url, dest):
    last = None
    for _ in range(3):
        try:
            urllib.request.urlretrieve(url, dest)
            return
        except Exception as e:  # noqa: BLE001 - retry then report
            last = e
    raise SystemExit("download failed %s: %s" % (url, last))


def smoked(name, env):
    """True when the tool executes (catches stale/wrong-libc binaries).

    Cargo plugins are probed via cargo dispatch, exactly as the gates run
    them: several (llvm-cov, udeps) reject a bare direct --version.
    """
    if name.startswith("cargo-"):
        cmd = ["cargo", name[len("cargo-"):], "--version"]
    else:
        path = shutil.which(name, path=env["PATH"])
        if path is None:
            return False
        cmd = [path, "--version"]
    try:
        r = subprocess.run(
            cmd,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=60,
        )
    except Exception:  # noqa: BLE001 - any launch failure means unusable
        return False
    return r.returncode == 0


def cargo_install(env, tools_root, crate, version, features=None):
    cmd = [
        "cargo",
        "install",
        "--force",
        "--locked",
        "--version",
        version,
        "--root",
        tools_root,
    ]
    if features:
        cmd += ["--features", features]
    cmd.append(crate)
    run(cmd, env, os.getcwd())


def install_tarball(env, name, url):
    tmpdir = tempfile.mkdtemp(prefix="rqg-")
    try:
        archive = os.path.join(tmpdir, "tool.tar.gz")
        fetch(url, archive)
        with tarfile.open(archive) as tf:
            tf.extractall(tmpdir)
        found = None
        for root, _dirs, files in os.walk(tmpdir):
            if name in files:
                found = os.path.join(root, name)
                break
        if found is None:
            raise SystemExit(
                "tarball for %s has no %s binary: %s" % (name, name, url)
            )
        dest = os.path.join(env["TOOLS_BIN"], name)
        shutil.copy(found, dest)
        os.chmod(dest, 0o755)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def ensure_tool(env, tools_root, name, crate, version, url, features=None):
    """Provision one pinned tool: prebuilt when it runs, else compile.

    Upstream gnu prebuilts are linked against newer glibc than bookworm's
    2.36, so a download that cannot execute falls back to cargo install
    (slow once, then cached in the shared cargo home). A download never
    overwrites a present binary: present-but-broken goes straight to the
    authoritative compile (and off Linux there are no downloads at all —
    the local proxy needs its tools installed by hand).
    """
    if smoked(name, env):
        return
    if sys.platform != "linux":
        raise SystemExit(
            "%s is missing or broken and this script only provisions "
            "Linux; install %s %s by hand for the local proxy"
            % (name, crate, version)
        )
    if shutil.which(name, path=env["PATH"]) is None and url is not None:
        print("installing %s (pinned prebuilt)..." % name)
        install_tarball(env, name, url)
        if smoked(name, env):
            return
        print("%s prebuilt cannot execute here; compiling..." % name)
    elif url is None:
        print("installing %s (pinned compile)..." % name)
    else:
        print("%s is present but cannot execute; compiling..." % name)
    cargo_install(env, tools_root, crate, version, features)
    if not smoked(name, env):
        raise SystemExit("cannot provision %s" % name)


def run(cmd, env, cwd):
    print("+ %s" % " ".join(cmd), flush=True)
    r = subprocess.run(cmd, env=env, cwd=cwd)
    if r.returncode != 0:
        raise SystemExit("FAILED: %s (exit %d)" % (" ".join(cmd), r.returncode))


def attempt(label, cmd, env, cwd):
    """run() that records failure instead of raising (T970: deferred exit).

    Every gate plus coverage runs on every build, so one red gate never
    hides the others' signal or gaps the coverage trend; main() exits
    nonzero citing the first failure. Returns None on success, the
    failure line otherwise.
    """
    print("==> %s: %s" % (cwd, label), flush=True)
    print("+ %s" % " ".join(cmd), flush=True)
    r = subprocess.run(cmd, env=env, cwd=cwd)
    if r.returncode != 0:
        msg = "FAILED: %s (exit %d)" % (" ".join(cmd), r.returncode)
        print(msg, flush=True)
        return msg
    return None


def cargo_target_dir(env, workspace):
    """Workspace target dir, honoring CARGO_TARGET_DIR like cargo does."""
    base = env.get("CARGO_TARGET_DIR") or os.path.join(workspace, "target")
    if not os.path.isabs(base):
        base = os.path.join(workspace, base)
    return base


def restamp_tree(root):
    """Uniform mtime stamp over a seeded target tree (T971).

    Same stamp the seed applies (see seed-rust-workspace.sh): every file and
    dir gets one shared timestamp, so no dependency output is newer than its
    dependent and dep-info files are newer than every input they list. Needed
    after provisioning because the gates install the nightly toolchain into
    this container's ephemeral rustup home AFTER the seed stamped target/,
    and udeps' binary dep-info records sysroot rmetas as inputs: without a
    restamp, every unit listing a sysroot file rebuilds. Best-effort: a
    missing tree just compiles from scratch.
    """
    if not os.path.isdir(root):
        return
    now = time.time()
    for dirpath, dirnames, filenames in os.walk(root):
        for name in dirnames + filenames:
            try:
                os.utime(os.path.join(dirpath, name), (now, now))
            except OSError:
                pass
    try:
        os.utime(root, (now, now))
    except OSError:
        pass


def seed_llvm_cov_target(env, workspace):
    """Copy the seeded test-profile target into llvm-cov's target dir.

    llvm-cov builds in <target>/llvm-cov-target, fresh every CI build, but
    instruments workspace crates only: coverage flags reach rustc through a
    wrapper limited to workspace members, deps keep plain test-profile
    flags, and the wrapper itself is not fingerprinted. Copying the
    already-seeded debug tree to <target>/llvm-cov-target/debug makes every
    dependency fresh, so the coverage run compiles workspace crates only.
    Skipped when the dest already has content (local re-runs and stage
    retries stay incremental). Best-effort: any failure warns and continues
    unseeded (status quo ante: slower, still green).
    """
    base = cargo_target_dir(env, workspace)
    src = os.path.join(base, "debug")
    covtarget = os.path.join(base, "llvm-cov-target")
    dest = os.path.join(covtarget, "debug")
    if os.path.isdir(dest) and os.listdir(dest):
        return
    if not os.path.isdir(src):
        print(
            "WARNING: no %s to seed llvm-cov from; running unseeded" % src,
            flush=True,
        )
        return
    print("seeding llvm-cov target dir from %s" % src, flush=True)
    start = time.monotonic()
    try:
        shutil.rmtree(covtarget, ignore_errors=True)
        shutil.copytree(src, dest, symlinks=True)
    except Exception as e:
        print(
            "WARNING: llvm-cov seed failed (%s); running unseeded" % e,
            flush=True,
        )
        shutil.rmtree(covtarget, ignore_errors=True)
        return
    print("llvm-cov target seeded in %.1fs" % (time.monotonic() - start), flush=True)


# Path parts excluded from canonicalized coverage, mirroring
# cargo-llvm-cov's own report filtering (test/bench/example sources,
# build output, and non-workspace code). Applied AFTER normpath, so
# `crates/x/tests/../src/y.rs` (#[path]-included production code, see
# canonicalize_lcov_records) survives while `crates/x/tests/y.rs`
# (test code) does not.
_COV_EXCLUDED_PARTS = (
    "/tests/",
    "/benches/",
    "/examples/",
    "/target/",
    "/.cargo/",
    "/rustup/",
    "/rustc/",
)


def llvm_tool_path(env, workspace, name):
    """Locate an llvm tool (llvm-cov, llvm-profdata) for the active toolchain."""
    sysroot = ""
    try:
        r = subprocess.run(
            ["rustc", "--print", "sysroot"],
            env=env,
            cwd=workspace,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=60,
            universal_newlines=True,
        )
        if r.returncode == 0:
            sysroot = r.stdout.strip()
    except Exception:
        pass
    if sysroot:
        host = ""
        try:
            r = subprocess.run(
                ["rustc", "-vV"],
                env=env,
                cwd=workspace,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                timeout=60,
                universal_newlines=True,
            )
            for line in r.stdout.splitlines():
                if line.startswith("host: "):
                    host = line[len("host: "):].strip()
        except Exception:
            pass
        if host:
            cand = os.path.join(sysroot, "lib", "rustlib", host, "bin", name)
            if os.access(cand, os.X_OK):
                return cand
            # Arcanist import: Windows tool files carry .exe.
            if os.access(cand + ".exe", os.X_OK):
                return cand + ".exe"
    found = shutil.which(name, path=env.get("PATH", ""))
    if found:
        return found
    raise SystemExit(
        "cannot find %s (the llvm-tools-preview rustup component "
        "provides it)" % name
    )


# Execution counts above this are profiler garbage, never real executions
# (llvm occasionally emits u64::MAX for lines on async-block boundaries;
# no suite executes a line 4e18 times). They crash the Coverage plugin's
# parser and would corrupt trends, so they are clamped to 0 (the
# conservative direction: never invent hits) with a logged warning.
_ABSURD_COUNT = 1 << 62


def _sane_count(raw, sf, what, sanitized):
    try:
        count = int(raw)
    except ValueError:
        return None
    if count > _ABSURD_COUNT or count < 0:
        sanitized.append((sf, what, raw))
        return 0
    return count


def parse_lcov_records(text):
    """Parse LCOV text into ([(sf, record)], [sanitized...]).

    record maps 'da' -> {line: count}, 'brda' -> {(line, block, branch):
    taken}, 'fnda' -> {name: count}, 'fn' -> set((name, line)); summary
    lines (LF/LH/BRF/BRH/FNF/FNH) are dropped and recomputed on emit.
    Absurd counts are clamped to 0 and reported as sanitized
    (sf, what, raw-value) entries — see _ABSURD_COUNT.
    """
    records = []
    sanitized = []
    sf = None
    rec = None

    def flush():
        if sf is not None:
            records.append((sf, rec))

    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("SF:"):
            flush()
            sf = line[3:]
            rec = {"da": {}, "brda": {}, "fnda": {}, "fn": set()}
        elif line == "end_of_record":
            flush()
            sf = None
        elif sf is None:
            continue
        elif line.startswith("DA:"):
            parts = line[3:].split(",")
            if len(parts) < 2:
                continue
            count = _sane_count(parts[1], sf, "DA:" + parts[0], sanitized)
            if count is None:
                continue
            rec["da"][parts[0]] = rec["da"].get(parts[0], 0) + count
        elif line.startswith("BRDA:"):
            parts = line[5:].split(",")
            if len(parts) >= 4:
                taken = parts[3]
                if taken != "-":
                    sane = _sane_count(
                        taken,
                        sf,
                        "BRDA:%s,%s,%s" % (parts[0], parts[1], parts[2]),
                        sanitized,
                    )
                    taken = "-" if sane is None else str(sane)
                _merge_brda_taken(
                    rec["brda"], (parts[0], parts[1], parts[2]), taken
                )
        elif line.startswith("FNDA:"):
            parts = line[5:].split(",", 1)
            if len(parts) == 2:
                count = _sane_count(parts[0], sf, "FNDA:" + parts[1], sanitized)
                if count is None:
                    continue
                rec["fnda"][parts[1]] = rec["fnda"].get(parts[1], 0) + count
        elif line.startswith("FN:"):
            parts = line[3:].split(",", 1)
            if len(parts) == 2:
                rec["fn"].add((parts[1], parts[0]))
    flush()
    return records, sanitized


def _merge_brda_taken(slot, key, taken):
    prev = slot.get(key)
    if prev is None or prev == "-":
        slot[key] = taken
    elif taken != "-":
        try:
            slot[key] = str(int(prev) + int(taken))
        except ValueError:
            pass


def canonicalize_lcov_records(records, workspace):
    """Merge raw-export records onto canonical workspace paths.

    rustc records #[path]-included modules (the headless GUI tours in
    binary crates) under `<crate>/tests/../src/*.rs`, which
    cargo-llvm-cov 0.9.1 silently drops from every report — thousands of
    genuinely-executed lines reported as uncovered. Lexical normpath
    collapses those onto the binary's own absolute paths so both copies
    merge; pure test sources stay excluded. Returns sorted
    [(sf, merged-record)].
    """
    merged = {}
    for sf, rec in records:
        path = sf if os.path.isabs(sf) else os.path.join(workspace, sf)
        norm = posixpath.normpath(path)
        if not norm.startswith(workspace + os.sep):
            continue
        if any(part in norm for part in _COV_EXCLUDED_PARTS):
            continue
        slot = merged.get(norm)
        if slot is None:
            merged[norm] = {
                "da": dict(rec["da"]),
                "brda": dict(rec["brda"]),
                "fnda": dict(rec["fnda"]),
                "fn": set(rec["fn"]),
            }
            continue
        for line, count in rec["da"].items():
            slot["da"][line] = slot["da"].get(line, 0) + count
        for key, taken in rec["brda"].items():
            _merge_brda_taken(slot["brda"], key, taken)
        for name, count in rec["fnda"].items():
            slot["fnda"][name] = slot["fnda"].get(name, 0) + count
        slot["fn"] |= rec["fn"]
    return sorted(merged.items())


def emit_lcov(canonical):
    """Render canonical [(sf, record)] back to LCOV with recomputed summaries."""
    out = []
    for sf, rec in canonical:
        out.append("SF:" + sf)
        for name, line in sorted(rec["fn"]):
            out.append("FN:%s,%s" % (line, name))
        for name in sorted(rec["fnda"]):
            out.append("FNDA:%d,%s" % (rec["fnda"][name], name))
        if rec["fn"] or rec["fnda"]:
            names = set(rec["fnda"]) | set(n for n, _ in rec["fn"])
            out.append("FNF:%d" % len(names))
            out.append(
                "FNH:%d" % sum(1 for n in names if rec["fnda"].get(n, 0) > 0)
            )
        for key in sorted(
            rec["brda"], key=lambda k: (int(k[0]), int(k[1]), int(k[2]))
        ):
            out.append(
                "BRDA:%s,%s,%s,%s" % (key[0], key[1], key[2], rec["brda"][key])
            )
        if rec["brda"]:
            out.append("BRF:%d" % len(rec["brda"]))
            out.append(
                "BRH:%d"
                % sum(1 for t in rec["brda"].values() if t != "-" and t != "0")
            )
        for line in sorted(rec["da"], key=int):
            out.append("DA:%s,%d" % (line, rec["da"][line]))
        out.append("LF:%d" % len(rec["da"]))
        out.append("LH:%d" % sum(1 for c in rec["da"].values() if c > 0))
        out.append("end_of_record")
    return "\n".join(out) + "\n" if out else ""


def lcov_line_sets(canonical):
    """(total, covered) line sets from canonical [(sf, record)]."""
    total, covered = set(), set()
    for sf, rec in canonical:
        for line, count in rec["da"].items():
            total.add((sf, line))
            if count > 0:
                covered.add((sf, line))
    return total, covered


def raw_llvm_cov_export(env, workspace, covtarget, llvm_cov, llvm_profdata):
    """Raw `llvm-cov export --format=lcov` over every test object.

    Unlike `cargo llvm-cov report`, this keeps the `<crate>/tests/../src`
    records that cargo's built-in filter drops (see
    canonicalize_lcov_records); the caller merges and validates them.
    """
    profraws = sorted(
        os.path.join(covtarget, fn)
        for fn in os.listdir(covtarget)
        if fn.endswith(".profraw")
    )
    if not profraws:
        raise SystemExit("no profraw files under %s" % covtarget)
    profdata = os.path.join(covtarget, "canonical.profdata")
    print(
        "+ %s merge -sparse ... (%d profraw) -o %s"
        % (llvm_profdata, len(profraws), profdata),
        flush=True,
    )
    r = subprocess.run(
        [llvm_profdata, "merge", "-sparse"] + profraws + ["-o", profdata],
        env=env,
        cwd=workspace,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=600,
        universal_newlines=True,
    )
    if r.returncode != 0:
        raise SystemExit(
            "llvm-profdata merge failed: %s" % r.stderr.strip()[-500:]
        )
    objs = []
    for sub in ("debug", "release"):
        deps = os.path.join(covtarget, sub, "deps")
        if not os.path.isdir(deps):
            continue
        for fn in sorted(os.listdir(deps)):
            if "." in fn:
                continue
            path = os.path.join(deps, fn)
            if os.path.isfile(path) and os.access(path, os.X_OK):
                objs.append(path)
    if not objs:
        raise SystemExit("no test objects under %s" % covtarget)
    cmd = [llvm_cov, "export", "-instr-profile=" + profdata, "--format=lcov"]
    for obj in objs:
        cmd += ["-object", obj]
    print(
        "+ %s ... (%d objects)" % (" ".join(cmd[:4]), len(objs)), flush=True
    )
    r = subprocess.run(
        cmd,
        env=env,
        cwd=workspace,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=600,
        universal_newlines=True,
    )
    if r.returncode != 0:
        raise SystemExit("llvm-cov export failed: %s" % r.stderr.strip()[-500:])
    return r.stdout, objs, profdata


def upgrade_lcov_with_canonical(env, workspace, report_dir):
    """Replace cargo's LCOV/HTML with the canonicalized raw export.

    Validated as a line-superset of cargo's view (same suite, same
    profile data — the raw path may only ADD genuinely-executed lines,
    never lose any); returns False to keep cargo's reports when the
    check fails. Raises on infrastructure errors (missing tools/data),
    which the caller downgrades to a warning + cargo's reports.
    """
    base = cargo_target_dir(env, workspace)
    covtarget = os.path.join(base, "llvm-cov-target")
    llvm_cov = llvm_tool_path(env, workspace, "llvm-cov")
    llvm_profdata = llvm_tool_path(env, workspace, "llvm-profdata")
    raw, objs, profdata = raw_llvm_cov_export(
        env, workspace, covtarget, llvm_cov, llvm_profdata
    )
    raw_records, raw_sanitized = parse_lcov_records(raw)
    for sf, what, value in raw_sanitized[:10]:
        print(
            "WARNING: raw export has absurd count %s at %s %s; clamped to 0"
            % (value, sf, what),
            flush=True,
        )
    canon = canonicalize_lcov_records(raw_records, workspace)
    lcov_path = os.path.join(report_dir, "lcov.info")
    with open(lcov_path, errors="replace") as fh:
        cargo_records, _ = parse_lcov_records(fh.read())
    cargo_canon = [
        (posixpath.normpath(sf), rec) for sf, rec in cargo_records
    ]
    canon_total, canon_covered = lcov_line_sets(canon)
    cargo_total, cargo_covered = lcov_line_sets(cargo_canon)
    if not cargo_total <= canon_total and cargo_covered <= canon_covered:
        print(
            "WARNING: canonical coverage is not a superset of cargo's "
            "view (cargo %d/%d lines, canonical %d/%d); keeping "
            "cargo-llvm-cov reports"
            % (
                len(cargo_covered),
                len(cargo_total),
                len(canon_covered),
                len(canon_total),
            ),
            flush=True,
        )
        return False
    # HTML first: it cannot merge the twin path spellings the way LCOV
    # can, so it shows every file (production under both spellings plus
    # test sources) with true per-file numbers — its index average will
    # differ from lcov.info's merged total, which is the published truth.
    html_dir = os.path.join(report_dir, "html")
    html_tmp = os.path.join(report_dir, "html.canonical")
    shutil.rmtree(html_tmp, ignore_errors=True)
    cmd = [
        llvm_cov,
        "show",
        "-instr-profile=" + profdata,
        "--format=html",
        "--output-dir=" + html_tmp,
    ]
    for obj in objs:
        cmd += ["-object", obj]
    print(
        "+ %s ... (%d objects)" % (" ".join(cmd[:6]), len(objs)), flush=True
    )
    r = subprocess.run(
        cmd,
        env=env,
        cwd=workspace,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=600,
        universal_newlines=True,
    )
    if r.returncode != 0:
        raise SystemExit(
            "llvm-cov show failed: %s" % r.stderr.strip()[-500:]
        )
    shutil.rmtree(html_dir, ignore_errors=True)
    os.rename(html_tmp, html_dir)
    with open(lcov_path, "w") as fh:
        fh.write(emit_lcov(canon))
    print(
        "canonical coverage: cargo %d/%d lines -> canonical %d/%d "
        "(recovered %d genuinely-executed lines)"
        % (
            len(cargo_covered),
            len(cargo_total),
            len(canon_covered),
            len(canon_total),
            len(canon_covered - cargo_covered),
        ),
        flush=True,
    )
    return True


def sanitize_lcov_file(lcov_path, workspace):
    """Clamp absurd counts in an LCOV file in place (see _ABSURD_COUNT).

    Protects the Coverage plugin even on the cargo-report fallback path;
    returns the number of clamped values.
    """
    with open(lcov_path, errors="replace") as fh:
        records, sanitized = parse_lcov_records(fh.read())
    for sf, what, value in sanitized[:10]:
        short = sf[len(workspace) + 1:] if sf.startswith(workspace) else sf
        print(
            "WARNING: %s has absurd count %s at %s %s; clamped to 0"
            % (lcov_path, value, short, what),
            flush=True,
        )
    if sanitized:
        canonical = canonicalize_lcov_records(records, workspace)
        with open(lcov_path, "w") as fh:
            fh.write(emit_lcov(canonical))
    return len(sanitized)


def print_lcov_summary(lcov_path, workspace):
    """Log the total + per-file table parsed from the published lcov.info."""
    with open(lcov_path, errors="replace") as fh:
        records, _ = parse_lcov_records(fh.read())
        canonical = canonicalize_lcov_records(records, workspace)
    rows = []
    hit = tot = 0
    for sf, rec in canonical:
        h = sum(1 for c in rec["da"].values() if c > 0)
        t = len(rec["da"])
        rows.append((h / max(1, t), h, t, sf))
        hit += h
        tot += t
    rows.sort()
    print(
        "coverage for %s: %.2f%% lines (%d/%d, %d files)"
        % (workspace, 100 * hit / max(1, tot), hit, tot, len(rows)),
        flush=True,
    )
    for pct, h, t, sf in rows:
        short = sf[len(workspace) + 1:] if sf.startswith(workspace) else sf
        print("  %6.2f%% %5d/%-5d %s" % (100 * pct, h, t, short), flush=True)


def coverage_reports(env, workspace, report_dir, cov_runner):
    # The suite run stays fail-fast (a red suite is the primary signal
    # and its partial profile data would mislead the trend); the report
    # steps below then always publish from whatever cargo produced.
    seed_llvm_cov_target(env, workspace)
    covtarget = os.path.join(cargo_target_dir(env, workspace), "llvm-cov-target")
    try:
        for fn in os.listdir(covtarget):
            if fn.endswith((".profraw", ".profdata")):
                os.unlink(os.path.join(covtarget, fn))
    except OSError:
        pass
    run(["cargo", "llvm-cov", "--no-report"] + cov_runner, env, workspace)
    lcov_path = os.path.join(report_dir, "lcov.info")
    run(
        ["cargo", "llvm-cov", "report", "--lcov", "--output-path", lcov_path],
        env,
        workspace,
    )
    sanitize_lcov_file(lcov_path, workspace)
    run(
        ["cargo", "llvm-cov", "report", "--html", "--output-dir", report_dir],
        env,
        workspace,
    )
    try:
        upgrade_lcov_with_canonical(env, workspace, report_dir)
    except (Exception, SystemExit) as e:
        print(
            "WARNING: canonical coverage unavailable (%s); keeping "
            "cargo-llvm-cov reports" % e,
            flush=True,
        )
    print_lcov_summary(lcov_path, workspace)
    # The 80% status quo, enforced: attained coverage must never regress.
    run(
        [
            sys.executable,
            os.path.join(LIB_DIR, "coverage-floor-check.py"),
            lcov_path,
            "80",
        ],
        env,
        workspace,
    )


def main():
    if len(sys.argv) < 4 or sys.argv[1] in ("-h", "--help"):
        print(__doc__)
        return 2
    workspace = os.path.abspath(sys.argv[1])
    report_dir = os.path.abspath(sys.argv[2])
    cov_runner = sys.argv[3:]
    if cov_runner[0] == "nextest" and "run" in cov_runner:
        # `cargo llvm-cov nextest` calls `cargo nextest run` internally; a
        # literal run becomes a test-name filter and silently guts coverage.
        raise SystemExit(
            "refusing nextest cov-runner containing 'run': omit the "
            "subcommand, llvm-cov inserts it (else it filters tests by name)"
        )
    arch = machine_arch()

    cargo_home = os.environ.get("CARGO_HOME")
    if cargo_home:
        tools_bin = os.path.join(cargo_home, "bin")
    else:
        # Courriel builder: cargo lives in the image, no CARGO_HOME set.
        tools_bin = os.path.dirname(shutil.which("cargo") or "/usr/bin")
    os.makedirs(tools_bin, exist_ok=True)

    # Child environment: tools on PATH, and none of Apple's python-injected
    # macOS SDK overrides (they break toolchain resolution for locally-run
    # gates; CI never sets them).
    env = {
        k: v
        for k, v in os.environ.items()
        if k not in ("SDKROOT", "CPATH", "LIBRARY_PATH")
    }
    env["PATH"] = tools_bin + os.pathsep + env.get("PATH", "")
    env["TOOLS_BIN"] = tools_bin

    print("==> rust quality gates: provisioning tools")
    tools_root = os.path.dirname(tools_bin)
    ensure_tool(
        env,
        tools_root,
        "cargo-audit",
        "cargo-audit",
        AUDIT_VERSION,
        "https://github.com/rustsec/rustsec/releases/download/cargo-audit/v%s/cargo-audit-%s-unknown-linux-gnu-v%s.tgz"
        % (AUDIT_VERSION, arch, AUDIT_VERSION),
    )
    if not smoked("cargo-deny", env):
        if sys.platform != "linux":
            raise SystemExit(
                "cargo-deny is missing or broken and this script only "
                "provisions Linux; install cargo-deny %s by hand for the "
                "local proxy" % DENY_VERSION
            )
        # Upstream ships only musl binaries for Linux (both arches); they
        # run on the glibc builders, checksum-verified.
        if shutil.which("cargo-deny", path=env["PATH"]) is None:
            target = "%s-unknown-linux-musl" % arch
            print("installing cargo-deny (pinned prebuilt)...")
            tmpdir = tempfile.mkdtemp(prefix="rqg-")
            try:
                base = (
                    "https://github.com/EmbarkStudios/cargo-deny/releases/download/%s/cargo-deny-%s-%s.tar.gz"
                    % (DENY_VERSION, DENY_VERSION, target)
                )
                archive = os.path.join(tmpdir, "deny.tar.gz")
                fetch(base, archive)
                want = os.path.join(tmpdir, "deny.sha256")
                fetch(base + ".sha256", want)
                with open(want) as fh:
                    expected = fh.read().strip().split()[0]
                with open(archive, "rb") as fh:
                    actual = hashlib.sha256(fh.read()).hexdigest()
                if actual != expected:
                    raise SystemExit("cargo-deny checksum mismatch")
                with tarfile.open(archive) as tf:
                    tf.extractall(tmpdir)
                dest = os.path.join(tools_bin, "cargo-deny")
                shutil.copy(
                    os.path.join(
                        tmpdir, "cargo-deny-%s-%s" % (DENY_VERSION, target),
                        "cargo-deny",
                    ),
                    dest,
                )
                os.chmod(dest, 0o755)
            finally:
                shutil.rmtree(tmpdir, ignore_errors=True)
            if not smoked("cargo-deny", env):
                print("cargo-deny prebuilt cannot execute here; compiling...")
                cargo_install(env, tools_root, "cargo-deny", DENY_VERSION)
                if not smoked("cargo-deny", env):
                    raise SystemExit("cannot provision cargo-deny")
        else:
            print("cargo-deny is present but cannot execute; compiling...")
            cargo_install(env, tools_root, "cargo-deny", DENY_VERSION)
            if not smoked("cargo-deny", env):
                raise SystemExit("cannot provision cargo-deny")
    ensure_tool(
        env,
        tools_root,
        "cargo-llvm-cov",
        "cargo-llvm-cov",
        LLVM_COV_VERSION,
        "https://github.com/taiki-e/cargo-llvm-cov/releases/download/v%s/cargo-llvm-cov-%s-unknown-linux-gnu.tar.gz"
        % (LLVM_COV_VERSION, arch),
    )
    ensure_tool(
        env,
        tools_root,
        "cargo-udeps",
        "cargo-udeps",
        UDEPS_VERSION,
        "https://github.com/est31/cargo-udeps/releases/download/v%s/cargo-udeps-v%s-%s-unknown-linux-gnu.tar.gz"
        % (UDEPS_VERSION, UDEPS_VERSION, arch),
    )
    if arch == "x86_64":
        rca_url = (
            "https://github.com/mozilla/rust-code-analysis/releases/download/v%s/rust-code-analysis-linux-cli-x86_64.tar.gz"
            % RCA_VERSION
        )
    else:
        # No aarch64 prebuilt upstream; compile (fast, cached).
        rca_url = None
    ensure_tool(
        env,
        tools_root,
        "rust-code-analysis-cli",
        "rust-code-analysis-cli",
        RCA_VERSION,
        rca_url,
    )
    # No prebuilt binaries upstream; the Courriel builder bakes this into
    # its image instead, every other stage compiles once into its cache.
    ensure_tool(
        env,
        tools_root,
        "cargo-geiger",
        "cargo-geiger",
        GEIGER_VERSION,
        None,
        "vendored-openssl",
    )
    run(["rustup", "component", "add", "llvm-tools-preview"], env, workspace)
    run(
        [
            "rustup",
            "toolchain",
            "install",
            UDEPS_NIGHTLY,
            "--profile",
            "minimal",
            "--no-self-update",
        ],
        env,
        workspace,
    )

    # Per-workspace advisory-db, freshly cloned every run: the default
    # ~/.cargo/advisory-db is shared across parallel stages on the node
    # cache, and audit cannot update an existing checkout anyway (a retry
    # reusing one fails the fetch with "failed to find `origin` remote").
    audit_db = os.path.join(workspace, "target", "advisory-db")
    shutil.rmtree(audit_db, ignore_errors=True)
    failures = []
    failures.append(
        attempt(
            "cargo audit --deny warnings",
            ["cargo", "audit", "--db", audit_db, "--deny", "warnings"],
            env,
            workspace,
        )
    )

    failures.append(
        attempt(
            "cargo deny check licenses bans sources",
            ["cargo", "deny", "check", "licenses", "bans", "sources"],
            env,
            workspace,
        )
    )

    # udeps runs in the cooked nested target dir (T971): its fingerprints
    # match only udeps-cooked artifacts, and the separate dir keeps its
    # nightly .rustc_info.json from dirtying the stable tree.
    udeps_env = dict(
        env,
        CARGO_TARGET_DIR=os.path.join(cargo_target_dir(env, workspace), "nightly"),
    )
    nightly_fp = os.path.join(udeps_env["CARGO_TARGET_DIR"], "debug", ".fingerprint")
    try:
        print(
            "==> %s: udeps seed holds %d fingerprints"
            % (workspace, len(os.listdir(nightly_fp)))
        )
    except OSError:
        print("==> %s: udeps seed MISSING (%s)" % (workspace, nightly_fp))
    # Restamp after provisioning (T971): the nightly toolchain above was
    # installed after the seed stamped target/, which would stale every
    # unit whose dep-info lists a sysroot file (branch build 4: 102 roots).
    restamp_tree(udeps_env["CARGO_TARGET_DIR"])
    failures.append(
        attempt(
            "cargo udeps (%s)" % UDEPS_NIGHTLY,
            ["cargo", "+" + UDEPS_NIGHTLY, "udeps", "--all-targets"],
            udeps_env,
            workspace,
        )
    )

    failures.append(
        attempt(
            "geiger unsafe ratchet",
            [sys.executable, os.path.join(LIB_DIR, "geiger-check.py"), workspace],
            env,
            workspace,
        )
    )

    failures.append(
        attempt(
            "complexity ratchet",
            [sys.executable, os.path.join(LIB_DIR, "complexity-check.py"), workspace],
            env,
            workspace,
        )
    )

    print("==> %s: coverage reports -> %s" % (workspace, report_dir))
    os.makedirs(report_dir, exist_ok=True)
    try:
        coverage_reports(env, workspace, report_dir, cov_runner)
    except SystemExit as e:
        failures.append("coverage: %s" % e.code)
    failures = [f for f in failures if f is not None]
    if failures:
        raise SystemExit(
            "rust quality gates: %d failure(s); first: %s"
            % (len(failures), failures[0])
        )

    print("rust quality gates: all green for %s" % workspace)
    return 0


if __name__ == "__main__":
    sys.exit(main())
