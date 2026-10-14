#[path = "../src/lint.rs"]
mod lint;

use std::path::{Path, PathBuf};

use lint::Linter;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-lint-{tag}-{}-{n}", std::process::id()));
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
    assert_eq!(lint::TextLinter::new().name(), "text");
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
