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
            std::env::temp_dir().join(format!("arc-rs-work-{tag}-{}-{n}", std::process::id()));
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init"]);
    git(tmp.path(), &["config", "user.email", "work@test"]);
    git(tmp.path(), &["config", "user.name", "work"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    tmp
}

fn commit_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-m", contents]);
}

fn current_branch(dir: &Path) -> String {
    git(dir, &["symbolic-ref", "--short", "HEAD"])
}

#[test]
fn parses_symbol_and_start_forms() {
    assert_eq!(
        parse_args(&args(&["feature"])),
        ParseOutcome::Args(Options {
            start: None,
            symbol: "feature".to_string(),
        })
    );
    assert_eq!(
        parse_args(&args(&["--start", "main", "feature"])),
        ParseOutcome::Args(Options {
            start: Some("main".to_string()),
            symbol: "feature".to_string(),
        })
    );
    assert_eq!(
        parse_args(&args(&["--start=HEAD~1", "--", "feature"])),
        ParseOutcome::Args(Options {
            start: Some("HEAD~1".to_string()),
            symbol: "feature".to_string(),
        })
    );
    assert_eq!(parse_args(&args(&["--help"])), ParseOutcome::Help);
}

#[test]
fn rejects_missing_extra_and_empty_symbols() {
    assert_eq!(
        parse_args(&args(&[])),
        ParseOutcome::Error(
            "Provide a branch, bookmark, task, or revision name to begin or resume work on."
                .to_string()
        )
    );
    assert_eq!(
        parse_args(&args(&["a", "b"])),
        ParseOutcome::Error("Too many arguments: provide exactly one argument.".to_string())
    );
    assert_eq!(
        parse_args(&args(&[""])),
        ParseOutcome::Error("Provide a nonempty symbol to begin or resume work on.".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--start"])),
        ParseOutcome::Error("--start requires a value".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus", "a"])),
        ParseOutcome::Error("unknown flag '--bogus'".to_string())
    );
}

#[test]
fn classifies_revision_and_task_monograms() {
    assert!(looks_like_revision("D123"));
    assert!(looks_like_revision("d1"));
    assert!(!looks_like_revision("D"));
    assert!(!looks_like_revision("D12x"));
    assert!(!looks_like_revision("feature"));
    assert!(looks_like_task("T123"));
    assert!(looks_like_task("t9"));
    assert!(!looks_like_task("T"));
    assert!(!looks_like_task("D123"));
}

#[test]
fn resolves_branches_commits_and_default_start() {
    let tmp = init_repo("resolve");
    commit_file(tmp.path(), "f.txt", "one");
    let branch = current_branch(tmp.path());
    assert!(branch_exists(tmp.path(), &branch));
    assert!(!branch_exists(tmp.path(), "no-such-branch"));
    assert!(!branch_exists(tmp.path(), "D123"));

    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    assert_eq!(resolve_commit(tmp.path(), "HEAD"), Some(head.clone()));
    assert_eq!(resolve_commit(tmp.path(), &head), Some(head.clone()));
    assert_eq!(resolve_commit(tmp.path(), "no-such-rev"), None);
    assert_eq!(default_start(tmp.path()), Some(branch));
}

#[test]
fn creates_and_resumes_branches() {
    let tmp = init_repo("create");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());

    assert_eq!(
        run_in(tmp.path(), &args(&["feature"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), "feature");
    // The startpoint was a branch, so the new branch tracks it like the PHP
    // engine's `checkout --track -b`.
    assert_eq!(
        git(tmp.path(), &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        first
    );

    assert_eq!(
        run_in(tmp.path(), &args(&[&first])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), first);
}

#[test]
fn creates_from_explicit_startpoints() {
    let tmp = init_repo("start");
    commit_file(tmp.path(), "f.txt", "one");
    commit_file(tmp.path(), "f.txt", "two");
    let base = git(tmp.path(), &["rev-parse", "HEAD~1"]);

    assert_eq!(
        run_in(tmp.path(), &args(&["--start", "HEAD~1", "older"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(git(tmp.path(), &["rev-parse", "older"]), base);

    git(tmp.path(), &["checkout", "--quiet", "older"]);
    assert_eq!(
        run_in(tmp.path(), &args(&[&format!("--start={base}"), "from-sha"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(git(tmp.path(), &["rev-parse", "from-sha"]), base);
}

#[test]
fn rejects_bad_startpoints_and_conduit_symbols() {
    let tmp = init_repo("reject");
    commit_file(tmp.path(), "f.txt", "one");
    assert_eq!(
        run_in(tmp.path(), &args(&["--start", "no-such-rev", "feature"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["D123"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["T1"])),
        std::process::ExitCode::FAILURE
    );
    // None of the failures created a branch.
    assert!(!branch_exists(tmp.path(), "feature"));
}

#[test]
fn requires_a_supported_working_copy() {
    let tmp = TempDir::new("plain");
    assert_eq!(
        run_in(tmp.path(), &args(&["feature"])),
        std::process::ExitCode::FAILURE
    );
    let hg = TempDir::new("hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(
        run_in(hg.path(), &args(&["feature"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&[])), std::process::ExitCode::FAILURE);
    assert_eq!(run(&args(&["a", "b"])), std::process::ExitCode::FAILURE);
}
