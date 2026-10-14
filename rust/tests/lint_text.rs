//! `arc-rs lint` behavior through the wired library.

use std::path::{Path, PathBuf};

use arc_rs::lint::{self, Linter};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Inside the crate dir so display paths stay relative and boring
        // for the NAME linter on every platform (absolute Windows paths
        // carry a drive prefix no filename rule accepts).
        let root = Path::new("target/lint-test-tmp");
        std::fs::create_dir_all(root).unwrap();
        let path = root.join(format!("arc-rs-lint-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file(&self, name: &str, contents: &[u8]) -> String {
        let disk = self.path.join(name);
        if let Some(parent) = disk.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&disk, contents).unwrap();
        disk.to_string_lossy().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn module_name_is_lint() {
    assert_eq!(lint::NAME, "lint");
    assert_eq!(lint::TextLinter::new().name(), "TXT");
}

#[test]
fn trailing_whitespace_is_autofix() {
    let engine = lint::Engine::new();
    let messages = engine.lint_bytes("f.txt", b"ok\nbad   \n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "TXT6");
    assert_eq!(messages[0].severity, lint::SEVERITY_AUTOFIX);
    assert_eq!((messages[0].line, messages[0].col), (2, 4));
}

#[test]
fn tabs_and_missing_newline_are_errors() {
    let engine = lint::Engine::new();
    let messages = engine.lint_bytes("f.txt", b"a\tb\nno-newline");
    assert!(
        messages
            .iter()
            .any(|m| m.code == "TXT2" && m.severity == "error")
    );
    assert!(
        messages
            .iter()
            .any(|m| m.code == "TXT4" && m.severity == "error")
    );
}

#[test]
fn clean_empty_and_binary_files_pass() {
    let engine = lint::Engine::new();
    assert!(engine.lint_bytes("f.txt", b"hello\nworld\n").is_empty());
    assert!(engine.lint_bytes("f.txt", b"").is_empty());

    let tmp = TempDir::new("binary");
    let path = tmp.file("bin.dat", b"a\0b\n");
    let found = engine.lint_file(Path::new(&path), &path).unwrap();
    assert!(found.is_empty());
}

#[test]
fn engine_supports_extra_linters() {
    struct Silent;
    impl lint::Linter for Silent {
        fn name(&self) -> &'static str {
            "silent"
        }
        fn lint_bytes(&self, _path: &str, _data: &[u8]) -> Vec<lint::LintMessage> {
            Vec::new()
        }
    }
    let mut engine = lint::Engine::empty();
    engine.add_linter(Box::new(Silent));
    assert!(engine.lint_bytes("f.txt", b"bad   \n").is_empty());
}

#[test]
fn fixtures_match_expectations() {
    let engine = lint::Engine::new();
    let base = Path::new("tests/fixtures/lint");
    let clean = std::fs::read(base.join("clean.txt")).unwrap();
    assert!(engine.lint_bytes("clean.txt", &clean).is_empty());

    let trailing = std::fs::read(base.join("trailing.txt")).unwrap();
    let found = engine.lint_bytes("trailing.txt", &trailing);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "TXT6");

    let tabs = std::fs::read(base.join("tabs.txt")).unwrap();
    assert!(
        engine
            .lint_bytes("tabs.txt", &tabs)
            .iter()
            .any(|m| m.code == "TXT2")
    );

    let noeol = std::fs::read(base.join("noeol.txt")).unwrap();
    assert!(
        engine
            .lint_bytes("noeol.txt", &noeol)
            .iter()
            .any(|m| m.code == "TXT4")
    );
}

#[test]
fn explicit_paths_lint_and_missing_paths_fail() {
    let tmp = TempDir::new("explicit");
    let dirty = tmp.file("dirty.txt", b"bad   \n");
    let messages = lint::lint_explicit_paths(&[dirty]).unwrap();
    assert_eq!(messages.len(), 1);

    let missing = tmp.path().join("nope.txt").to_string_lossy().to_string();
    assert!(lint::lint_explicit_paths(&[missing]).is_err());

    let clean = tmp.file("clean.txt", b"ok\n");
    assert_eq!(
        lint::run(&[clean, "--json".to_string()]),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_rejects_bad_and_unported_flags() {
    assert_eq!(
        lint::run(&["--bogus".to_string()]),
        std::process::ExitCode::FAILURE
    );
    for flag in [
        "--rev",
        "--apply-patches",
        "--amend-all",
        "--engine",
        "--everything",
    ] {
        assert_eq!(
            lint::run(&[flag.to_string()]),
            std::process::ExitCode::FAILURE,
            "for {flag}"
        );
    }
}

#[test]
fn run_reports_missing_paths_and_help() {
    assert_eq!(
        lint::run(&["definitely/missing/file.txt".to_string()]),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        lint::run(&["--help".to_string()]),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_exit_reflects_error_severity() {
    let tmp = TempDir::new("exit");
    let tabbed = tmp.file("tab.txt", b"a\tb\n");
    assert_eq!(lint::run(&[tabbed]), std::process::ExitCode::FAILURE);
    let autofix_only = tmp.file("ws.txt", b"bad   \n");
    assert_eq!(lint::run(&[autofix_only]), std::process::ExitCode::SUCCESS);
}

#[test]
fn collect_walks_directories() {
    let tmp = TempDir::new("walk");
    tmp.file("sub/a.txt", b"ok\n");
    tmp.file("sub/b.txt", b"ok\n");
    let dir = tmp.path().join("sub").to_string_lossy().to_string();
    let files = lint::collect_explicit(&[dir]).unwrap();
    assert_eq!(files.len(), 2);
}

fn install_arclint(tmp: &TempDir, fixture: &str) {
    let body = std::fs::read(Path::new("tests/fixtures/lint").join(fixture)).unwrap();
    std::fs::write(tmp.path().join(".arclint"), body).unwrap();
}

#[test]
fn arclint_selection_limits_run() {
    let tmp = TempDir::new("arclint-select");
    install_arclint(&tmp, "arclint-text-only.json");
    let bad_json = tmp.file("bad.json", b"not json\n");

    let (engine, config) = lint::engine_for_dir(tmp.path()).unwrap();
    assert_eq!(config, Some(tmp.path().join(".arclint")));
    assert!(
        lint::lint_explicit_paths_with_engine(&engine, std::slice::from_ref(&bad_json))
            .unwrap()
            .is_empty(),
        "text-only config must not report JSON1"
    );
    assert!(
        lint::lint_explicit_paths(std::slice::from_ref(&bad_json))
            .unwrap()
            .iter()
            .any(|m| m.code == "JSON1"),
        "unconfigured run still reports JSON1"
    );
    assert_eq!(
        lint::run_in(tmp.path(), &[bad_json]),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn arclint_is_discovered_from_child_directories() {
    let tmp = TempDir::new("arclint-nested");
    install_arclint(&tmp, "arclint-text-only.json");
    let bad_json = tmp.file("sub/bad.json", b"not json\n");

    let child = tmp.path().join("sub");
    let (_engine, config) = lint::engine_for_dir(&child).unwrap();
    assert_eq!(config, Some(tmp.path().join(".arclint")));
    assert_eq!(
        lint::run_in(&child, &[bad_json]),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn invalid_arclint_fails_run() {
    for fixture in ["arclint-invalid.json", "arclint-unknown-type.json"] {
        let tmp = TempDir::new("arclint-bad");
        install_arclint(&tmp, fixture);
        let clean = tmp.file("clean.txt", b"ok\n");
        assert!(lint::engine_for_dir(tmp.path()).is_err(), "for {fixture}");
        assert_eq!(
            lint::run_in(tmp.path(), &[clean]),
            std::process::ExitCode::FAILURE,
            "for {fixture}"
        );
    }
}

#[test]
fn severity_flag_filters_messages() {
    let tmp = TempDir::new("severity");
    install_arclint(&tmp, "arclint-text-only.json");
    let ws = tmp.file("ws.txt", b"bad   \n");
    let tab = tmp.file("tab.txt", b"a\tb\n");

    // Library level: the autofix downgrade is dropped at error minimum.
    let engine = lint::Engine::new();
    let found = lint::lint_explicit_paths_with_engine(&engine, std::slice::from_ref(&ws)).unwrap();
    assert_eq!(found.len(), 1);
    assert!(lint::filter_by_severity(found, "error").is_empty());

    // CLI level: filtering never fails the run, errors still surface.
    assert_eq!(
        lint::run_in(tmp.path(), std::slice::from_ref(&ws)),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        lint::run_in(
            tmp.path(),
            &[ws, "--severity".to_string(), "error".to_string()]
        ),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        lint::run_in(tmp.path(), &[tab, "--severity=error".to_string()]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn severity_flag_rejects_bad_values() {
    let tmp = TempDir::new("severity-bad");
    let clean = tmp.file("clean.txt", b"ok\n");
    assert_eq!(
        lint::run_in(
            tmp.path(),
            &[clean.clone(), "--severity".to_string(), "bogus".to_string()]
        ),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        lint::run_in(tmp.path(), &[clean, "--severity".to_string()]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn changed_files_cover_staged_and_unstaged_only() {
    let tmp = TempDir::new("git");
    git(tmp.path(), &["init"]);
    git(tmp.path(), &["config", "user.email", "lint@test"]);
    git(tmp.path(), &["config", "user.name", "lint"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path().join("tracked.txt"), b"ok\n").unwrap();
    git(tmp.path(), &["add", "tracked.txt"]);
    git(tmp.path(), &["commit", "-m", "init"]);

    assert!(lint::changed_files_in(tmp.path()).unwrap().is_empty());

    // Unstaged modification plus an untracked file: only the tracked change
    // must be reported.
    std::fs::write(tmp.path().join("tracked.txt"), b"ok   \n").unwrap();
    std::fs::write(tmp.path().join("untracked.txt"), b"bad   \n").unwrap();
    let changed = lint::changed_files_in(tmp.path()).unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].1, "tracked.txt");

    // Staging the new file makes it visible to the diff versus HEAD.
    git(tmp.path(), &["add", "untracked.txt"]);
    let changed = lint::changed_files_in(tmp.path()).unwrap();
    assert_eq!(changed.len(), 2);

    let messages = lint::lint_changed_in(tmp.path()).unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m.path == "tracked.txt" && m.code == "TXT6")
    );
}
