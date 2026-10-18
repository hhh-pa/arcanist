use super::*;

use std::path::{Path, PathBuf};

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-anoid-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn parses_empty_and_help() {
    // T1153 BROWSE-B21: TTY runs need no `--`; piped runs do.
    assert_eq!(parse_args_with_tty(&args(&[]), true), Ok(Some(())));
    assert_eq!(
        parse_args_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(parse_args_with_tty(&args(&["--"]), false), Ok(Some(())));
    assert_eq!(parse_args_with_tty(&args(&["--help"]), true), Ok(None));
    assert_eq!(
        parse_args_with_tty(&args(&["--help", "--"]), false),
        Ok(None)
    );
    assert_eq!(parse_args_with_tty(&args(&["-h", "--"]), false), Ok(None));
}

#[test]
fn rejects_arguments_and_flags() {
    // Unknown flags and positionals report before the terminator
    // check (like PHP's `parseFull`); `--help` without `--` when
    // piped reports the terminator; `--help` after `--` is a
    // positional.
    assert!(parse_args_with_tty(&args(&["extra", "--"]), false).is_err());
    assert!(parse_args_with_tty(&args(&["--bogus"]), false).is_err());
    assert_eq!(
        parse_args_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert!(parse_args_with_tty(&args(&["--", "--help"]), false).is_err());
}

#[test]
fn finds_binaries_on_a_search_path() {
    let tmp = TempDir::new("path");
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(bin_dir.join("python3"), b"#!/bin/sh\n").unwrap();

    let search = std::env::join_paths([tmp.path().join("missing"), bin_dir.clone()]).unwrap();
    let found = find_on_path(&search, "python3").unwrap();
    assert_eq!(found, bin_dir.join("python3"));

    assert!(find_on_path(&search, "no-such-binary").is_none());
    let empty = std::env::join_paths([tmp.path().join("missing")]).unwrap();
    assert!(find_on_path(&empty, "python3").is_none());
}

#[test]
fn windows_probes_exe_extension() {
    let tmp = TempDir::new("exe");
    std::fs::write(tmp.path().join("python3.exe"), b"MZ").unwrap();
    let search = std::env::join_paths([tmp.path().to_path_buf()]).unwrap();
    let found = find_on_path(&search, "python3");
    #[cfg(windows)]
    assert_eq!(found, Some(tmp.path().join("python3.exe")));
    #[cfg(not(windows))]
    assert_eq!(found, None);
}

#[test]
fn finds_script_by_walking_up() {
    // T1153 CORE-58 (kept): no install tree exists in a compiled
    // binary, so the script resolves by walking up.
    let tmp = TempDir::new("script");
    let nested = tmp.path().join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(find_script(&nested).is_none());

    let script = tmp
        .path()
        .join("support")
        .join("arcanoid")
        .join("arcanoid.py");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, b"print('probe')\n").unwrap();
    assert_eq!(find_script(&nested), Some(script));
}

#[test]
fn passthru_maps_status_and_reports_spawn_failures() {
    // T1153 CORE-57: the child status comes back verbatim, like
    // `PhutilExecPassthru::execute()`.
    let tmp = TempDir::new("passthru");
    let git = std::env::var_os("PATH")
        .and_then(|path| find_on_path(&path, "git"))
        .expect("git must be on PATH for tests");
    assert_eq!(
        passthru(&git, &["--version"], tmp.path()).unwrap(),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        passthru(&git, &["no-such-subcommand"], tmp.path()).unwrap(),
        std::process::ExitCode::FAILURE
    );
    // A distinctive non-1 code survives verbatim.
    #[cfg(windows)]
    let third = passthru(Path::new("cmd"), &["/C", "exit 3"], tmp.path()).unwrap();
    #[cfg(not(windows))]
    let third = passthru(Path::new("sh"), &["-c", "exit 3"], tmp.path()).unwrap();
    assert_eq!(third, std::process::ExitCode::from(3));
    assert!(passthru(&tmp.path().join("no-such-program"), &[], tmp.path()).is_err());
}

#[test]
fn run_reports_help_and_usage_errors() {
    // T1153 BROWSE-B21: piped runs carry `--`.
    assert_eq!(
        run(&args(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn run_in_fails_without_python_or_script() {
    // Either precondition may fail depending on the machine, but a bare temp
    // dir (no support/ tree) can never launch the probe.
    // T1153 BROWSE-B21: piped runs carry `--`.
    let tmp = TempDir::new("missing");
    assert_eq!(
        run_in(tmp.path(), &args(&["--"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn b23_help_matches_generated_format() {
    // T1153 BROWSE-B23: `--help` renders the generated workflow help
    // (`ArcanistAnoidWorkflow.php:11-23`), byte-identical to
    // `arc help anoid`.
    assert_eq!(
        usage_text(false),
        "ANOID WORKFLOW\n\
         \n\
         anoid\n\
         Pilot a probe from the vessel \"Arcanoid\".\n\
         \n\
         Take control of a probe launched from the science vessel \"Arcanoid\".\n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn interpreter_candidates_cover_platform() {
    // T1153 CORE-59: the Windows `python` fallback stays additive.
    let found = interpreter_candidates();
    assert!(found.contains(&"python3"));
    assert_eq!(found.contains(&"python"), cfg!(windows));
}

#[test]
fn path_lookup_skips_empty_segments() {
    assert!(find_on_path(std::ffi::OsStr::new(":"), "no-such-tool-xyz").is_none());
}

#[test]
fn piped_run_without_terminator_fails() {
    // Without `--` on a pipe, `run_in` prints the terminator hints
    // and reports usage; on a TTY it proceeds (and the empty dir has
    // no script, so it still fails cleanly without spawning).
    let tmp = TempDir::new("piped");
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn passthru_returns_the_script_status() {
    let tmp = TempDir::new("passthru");
    let script_dir = tmp.path().join("support").join("arcanoid");
    std::fs::create_dir_all(&script_dir).unwrap();
    std::fs::write(script_dir.join("arcanoid.py"), "import sys\nsys.exit(0)\n").unwrap();
    if python3_on_path().is_none() {
        return; // LCOV_EXCL_LINE // Reason: python3 is installed on ubuntu CI; the skip never trips
    }
    assert_eq!(
        run_in(tmp.path(), &args(&["--"])),
        std::process::ExitCode::SUCCESS
    );
}

// -( T1170 coverage wave 6g band G, batch E )----------------------------------

#[test]
fn cov6g_run_in_missing_cwd_reports_launch_failure() {
    // `find_script` walks ancestors lexically, so it resolves above a
    // missing directory; the spawn then fails on `current_dir` and
    // `run_in` reports the launch failure.
    if python3_on_path().is_none() {
        return; // LCOV_EXCL_LINE // Reason: python3 is installed on ubuntu CI; the skip never trips
    }
    let tmp = TempDir::new("cov6g-missing-cwd");
    let script_dir = tmp.path().join("support").join("arcanoid");
    std::fs::create_dir_all(&script_dir).unwrap();
    std::fs::write(script_dir.join("arcanoid.py"), "# never launched\n").unwrap();
    let missing = tmp.path().join("gone");
    assert!(!missing.exists());
    // The script resolves lexically above the missing cwd, so the
    // failure below can only be the spawn/launch arm.
    assert!(find_script(&missing).is_some());
    assert_eq!(
        run_in(&missing, &args(&["--"])),
        std::process::ExitCode::FAILURE
    );
}
