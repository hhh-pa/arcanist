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
    assert_eq!(parse_args(&args(&[])), Ok(Some(())));
    assert_eq!(parse_args(&args(&["--help"])), Ok(None));
    assert_eq!(parse_args(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_arguments_and_flags() {
    assert!(parse_args(&args(&["extra"])).is_err());
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--"])).is_err());
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
    if cfg!(windows) {
        assert_eq!(found, Some(tmp.path().join("python3.exe")));
    } else {
        assert_eq!(found, None);
    }
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
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn run_in_fails_without_python_or_script() {
    // Either precondition may fail depending on the machine, but a bare temp
    // dir (no support/ tree) can never launch the probe.
    let tmp = TempDir::new("missing");
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn interpreter_candidates_cover_platform() {
    // T1153 CORE-59: the Windows `python` fallback stays additive.
    let found = interpreter_candidates();
    assert!(found.contains(&"python3"));
    assert_eq!(found.contains(&"python"), cfg!(windows));
}
