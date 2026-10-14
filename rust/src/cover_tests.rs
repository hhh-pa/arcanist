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
            std::env::temp_dir().join(format!("arc-rs-cover-{tag}-{}-{n}", std::process::id()));
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
    git(tmp.path(), &["config", "user.email", "cover@test"]);
    git(tmp.path(), &["config", "user.name", "cover author"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    tmp
}

fn commit_file(dir: &Path, name: &str, contents: &str) {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-m", contents]);
}

#[test]
fn parses_rev_paths_and_help() {
    assert_eq!(
        parse_args(&args(&[])),
        ParseOutcome::Args(Options {
            rev: None,
            paths: Vec::new(),
        })
    );
    assert_eq!(
        parse_args(&args(&["--rev", "HEAD~1", "a.txt"])),
        ParseOutcome::Args(Options {
            rev: Some("HEAD~1".to_string()),
            paths: vec!["a.txt".to_string()],
        })
    );
    assert_eq!(
        parse_args(&args(&["--rev=abc", "--", "--weird"])),
        ParseOutcome::Args(Options {
            rev: Some("abc".to_string()),
            paths: vec!["--weird".to_string()],
        })
    );
    assert_eq!(parse_args(&args(&["--help"])), ParseOutcome::Help);
    assert_eq!(
        parse_args(&args(&["--rev"])),
        ParseOutcome::Error("--rev requires a value".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus"])),
        ParseOutcome::Error("unknown flag '--bogus'".to_string())
    );
}

#[test]
fn parses_unified_zero_diffs() {
    let diff = "diff --git a/f.txt b/f.txt\n\
         index 1111111..2222222 100644\n\
         --- a/f.txt\n\
         +++ b/f.txt\n\
         @@ -1,3 +1,4 @@\n\
         ctx\n\
         +added\n\
         diff --git a/gone.txt b/gone.txt\n\
         deleted file mode 100644\n\
         index 3333333..0000000\n\
         --- a/gone.txt\n\
         +++ /dev/null\n\
         @@ -1,2 +0,0 @@\n\
         -old\n\
         diff --git a/img.png b/img.png\n\
         Binary files a/img.png and b/img.png differ\n";
    let files = parse_diff_unified0(diff);
    assert_eq!(files.len(), 3);
    assert_eq!(
        files[0],
        FileChanges {
            path: "f.txt".to_string(),
            lines: vec![1, 2, 3, 4],
        }
    );
    assert_eq!(files[1].path, "gone.txt");
    assert!(files[1].lines.is_empty());
    assert_eq!(files[2].path, "img.png");
    assert!(files[2].lines.is_empty());
}

#[test]
fn parses_hunk_ranges() {
    assert_eq!(parse_hunk("-1,3 +5,7 @@"), Some((5, 7)));
    assert_eq!(parse_hunk("-1 +2 @@ context"), Some((2, 1)));
    assert_eq!(parse_hunk("-1,2 +0,0 @@"), None);
    assert_eq!(parse_hunk("garbage"), None);
}

#[test]
fn parses_porcelain_blame() {
    let blame = "abc123 1 1 1\n\
         author cover author\n\
         committer cover author\n\
         \tfirst\n\
         00000000 2 2 1\n\
         author Not Committed Yet\n\
         \tsecond\n";
    assert_eq!(
        parse_blame_porcelain(blame),
        vec!["cover author".to_string(), "Not Committed Yet".to_string()]
    );
    assert!(parse_blame_porcelain("").is_empty());
}

#[test]
fn formats_readable_sequences() {
    assert_eq!(readable_sequence(&[]), "");
    assert_eq!(readable_sequence(&[5]), "5");
    assert_eq!(readable_sequence(&[1, 2, 3, 5, 7, 8]), "1-3, 5, 7-8");
    assert_eq!(readable_sequence(&[3, 1, 2, 2]), "1-3");
}

#[test]
fn blames_files_through_git() {
    let tmp = init_repo("blame");
    commit_file(tmp.path(), "f.txt", "one\ntwo\n");
    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\n").unwrap();
    let authors = blame_file(tmp.path(), "f.txt").unwrap();
    assert_eq!(authors.len(), 2);
    assert_eq!(authors[0], "cover author");
    assert_eq!(authors[1], "Not Committed Yet");
    assert!(blame_file(tmp.path(), "no-such-file.txt").is_err());
}

#[test]
fn covers_nothing_when_clean() {
    let tmp = init_repo("clean");
    commit_file(tmp.path(), "f.txt", "one\n");
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn covers_worktree_changes_since_head() {
    let tmp = init_repo("worktree");
    commit_file(tmp.path(), "f.txt", "one\ntwo\nthree\n");
    std::fs::write(tmp.path().join("f.txt"), "one\nTWO\nthree\n").unwrap();
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["f.txt"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn covers_ranges_since_explicit_revisions() {
    let tmp = init_repo("rev");
    commit_file(tmp.path(), "f.txt", "one\ntwo\nthree\n");
    std::fs::write(tmp.path().join("f.txt"), "ONE\ntwo\nthree\n").unwrap();
    git(tmp.path(), &["commit", "-am", "change one"]);
    assert_eq!(
        run_in(tmp.path(), &args(&["--rev", "HEAD~1"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--rev", "no-such-rev"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn handles_untracked_and_missing_paths() {
    let tmp = init_repo("untracked");
    commit_file(tmp.path(), "f.txt", "one\n");
    std::fs::write(tmp.path().join("new.txt"), "hello\n").unwrap();
    // Untracked files carry no blame: covered, but unattributed.
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["new.txt"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["no-such-file.txt"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn rejects_bad_contexts_and_flags() {
    let plain = TempDir::new("plain");
    assert_eq!(
        run_in(plain.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
    let hg = TempDir::new("hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    assert_eq!(
        run_in(hg.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
    let tmp = init_repo("flags");
    commit_file(tmp.path(), "f.txt", "one\n");
    assert_eq!(
        run_in(tmp.path(), &args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}
