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
            std::env::temp_dir().join(format!("arc-rs-inspect-{tag}-{}-{n}", std::process::id()));
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
    git(tmp.path(), &["config", "user.email", "inspect@test"]);
    git(tmp.path(), &["config", "user.name", "inspect"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    tmp
}

#[test]
fn parses_explore_objects_and_separators() {
    assert_eq!(
        parse_args(&args(&[])),
        ParseOutcome::Args(Options {
            explore: false,
            objects: Vec::new(),
        })
    );
    assert_eq!(
        parse_args(&args(&["--explore", "--", "working-copy(HEAD)"])),
        ParseOutcome::Args(Options {
            explore: true,
            objects: vec!["working-copy(HEAD)".to_string()],
        })
    );
    assert_eq!(parse_args(&args(&["--help"])), ParseOutcome::Help);
    assert_eq!(
        parse_args(&args(&["--bogus"])),
        ParseOutcome::Error("unknown flag '--bogus'".to_string())
    );
}

#[test]
fn parses_object_specifications() {
    assert_eq!(
        parse_spec("working-copy"),
        Ok(ObjectSpec {
            function: "working-copy".to_string(),
            arguments: None,
        })
    );
    assert_eq!(
        parse_spec("working-copy(HEAD)"),
        Ok(ObjectSpec {
            function: "working-copy".to_string(),
            arguments: Some("HEAD".to_string()),
        })
    );
    assert_eq!(
        parse_spec("browse(abc123)"),
        Ok(ObjectSpec {
            function: "browse".to_string(),
            arguments: Some("abc123".to_string()),
        })
    );
    // Greedy arguments may contain closing parens, like the PHP pattern.
    assert_eq!(
        parse_spec("working-copy(a)b)"),
        Ok(ObjectSpec {
            function: "working-copy".to_string(),
            arguments: Some("a)b".to_string()),
        })
    );
    for bad in [
        "",
        "has space",
        "trailing)b",
        "name(args",
        "(args)",
        "weird!name",
    ] {
        assert!(parse_spec(bad).is_err(), "for {bad:?}");
    }
}

#[test]
fn renders_values_with_escapes_and_truncation() {
    assert_eq!(render_value("abc"), "\"abc\"");
    assert_eq!(
        render_value("a\nb\tc\rd\\e\"f"),
        "\"a\\nb\\tc\\rd\\\\e\\\"f\""
    );
    let long = "x".repeat(100);
    assert_eq!(render_value(&long), format!("\"{}\"", "x".repeat(64)));
}

#[test]
fn renders_working_copy_trees() {
    let info = CommitInfo {
        hash: "abc123".to_string(),
        message: "subject\n\nbody".to_string(),
    };
    let plain = render_working_copy(&info, false);
    assert!(plain.contains("+ [ArcanistWorkingCopyStateRef] Working Copy State"));
    assert!(plain.contains(" * [ArcanistScalarHardpoint] commitRef"));
    assert!(plain.contains("  + [ArcanistCommitRef] Commit \"abc123\""));
    assert!(plain.contains("   o [ArcanistScalarHardpoint] message"));
    assert!(plain.contains("   o [ArcanistScalarHardpoint] upstream"));
    assert!(plain.contains(" o [ArcanistObjectListHardpoint] revisionRefs"));

    let explored = render_working_copy(&info, true);
    assert!(explored.contains("   * [ArcanistScalarHardpoint] message"));
    assert!(explored.contains("    > \"subject\\n\\nbody\""));
    assert!(explored.contains("   o [ArcanistScalarHardpoint] upstream"));
}

#[test]
fn reads_commits_from_git() {
    let tmp = init_repo("read");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);

    let info = read_commit(tmp.path(), "HEAD").unwrap();
    assert_eq!(info.hash, head);
    assert!(info.message.contains("first subject"));
    assert!(read_commit(tmp.path(), "no-such-rev").is_err());

    let plain = TempDir::new("plain");
    assert!(read_commit(plain.path(), "HEAD").is_err());
}

#[test]
fn inspects_single_objects() {
    let tmp = init_repo("one");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);

    let block = inspect_one(tmp.path(), "working-copy(HEAD)", false).unwrap();
    assert!(block.contains(&format!("Commit \"{head}\"")));
    let explored = inspect_one(tmp.path(), "working-copy(HEAD)", true).unwrap();
    assert!(explored.contains("first subject"));

    assert!(inspect_one(tmp.path(), "working-copy", false).is_err());
    assert!(inspect_one(tmp.path(), "browse", false).is_err());
    assert_eq!(
        inspect_one(tmp.path(), "browse(token)", false),
        Err("\"browse(...)\" is not yet ported (requires Conduit; see T1128)".to_string())
    );
    let unknown = inspect_one(tmp.path(), "commit(HEAD)", false).unwrap_err();
    assert!(unknown.contains("Unknown object type \"commit\""));
    assert!(unknown.contains("browse, working-copy"));
    assert!(inspect_one(tmp.path(), "not valid!", false).is_err());
}

#[test]
fn run_in_lists_types_and_joins_blocks() {
    let tmp = init_repo("run");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);

    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["working-copy(HEAD)"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(
            tmp.path(),
            &args(&["--explore", "working-copy(HEAD)", "working-copy(HEAD)"])
        ),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["commit(HEAD)"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}
