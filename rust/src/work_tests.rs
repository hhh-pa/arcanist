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
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_args_with_tty(&args(&["feature"]), true),
        ParseOutcome::Args(Options {
            start: None,
            symbol: "feature".to_string(),
        })
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--start", "main", "feature"]), true),
        ParseOutcome::Args(Options {
            start: Some("main".to_string()),
            symbol: "feature".to_string(),
        })
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--start=HEAD~1", "--", "feature"]), true),
        ParseOutcome::Args(Options {
            start: Some("HEAD~1".to_string()),
            symbol: "feature".to_string(),
        })
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--help"]), true),
        ParseOutcome::Help
    );
}

#[test]
fn rejects_missing_extra_and_empty_symbols() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_args_with_tty(&args(&[]), true),
        ParseOutcome::Error(
            "Provide a branch, bookmark, task, or revision name to begin or resume work on."
                .to_string()
        )
    );
    assert_eq!(
        parse_args_with_tty(&args(&["a", "b"]), true),
        ParseOutcome::Error("Too many arguments: provide exactly one argument.".to_string())
    );
    assert_eq!(
        parse_args_with_tty(&args(&[""]), true),
        ParseOutcome::Error("Provide a nonempty symbol to begin or resume work on.".to_string())
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--start"]), true),
        ParseOutcome::Error("--start requires a value".to_string())
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--bogus", "a"]), true),
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
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("resolve");
    commit_file(tmp.path(), "f.txt", "one");
    let branch = current_branch(tmp.path());
    assert!(branch_exists(tmp.path(), &branch));
    assert!(!branch_exists(tmp.path(), "no-such-branch"));
    assert!(!branch_exists(tmp.path(), "D123"));

    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    assert_eq!(canonical_revision(tmp.path(), "HEAD"), Some(head.clone()));
    assert_eq!(canonical_revision(tmp.path(), &head), Some(head.clone()));
    assert_eq!(canonical_revision(tmp.path(), "no-such-rev"), None);
    assert_eq!(default_start(tmp.path()), Some(branch));
}

#[test]
fn creates_and_resumes_branches() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("create");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());

    assert_eq!(
        run_in(tmp.path(), &args(&["--", "feature"])),
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
        run_in(tmp.path(), &args(&["--", first.as_str()])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), first);
}

#[test]
fn creates_from_explicit_startpoints() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("start");
    commit_file(tmp.path(), "f.txt", "one");
    commit_file(tmp.path(), "f.txt", "two");
    let base = git(tmp.path(), &["rev-parse", "HEAD~1"]);

    assert_eq!(
        run_in(tmp.path(), &args(&["--start", "HEAD~1", "--", "older"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(git(tmp.path(), &["rev-parse", "older"]), base);

    git(tmp.path(), &["checkout", "--quiet", "older"]);
    assert_eq!(
        run_in(
            tmp.path(),
            &args(&[&format!("--start={base}"), "--", "from-sha"])
        ),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(git(tmp.path(), &["rev-parse", "from-sha"]), base);
}

#[test]
fn rejects_bad_startpoints_and_conduit_symbols() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("reject");
    commit_file(tmp.path(), "f.txt", "one");
    assert_eq!(
        run_in(
            tmp.path(),
            &args(&["--start", "no-such-rev", "--", "feature"])
        ),
        std::process::ExitCode::FAILURE
    );
    // Unknown revision/task symbols fail through the mocked search (no
    // network), and bare IDs never become branch names.
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "D123"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "T1"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "123"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    // None of the failures created a branch.
    assert!(!branch_exists(tmp.path(), "feature"));
    assert!(!branch_exists(tmp.path(), "123"));
}

fn expect_work_args(words: &[&str]) -> Options {
    match parse_args_with_tty(&args(words), true) {
        ParseOutcome::Args(options) => options,
        _ => panic!("feature parses"),
    }
}

#[test]
fn requires_a_supported_working_copy() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = TempDir::new("plain");
    assert_eq!(
        run_in(tmp.path(), &args(&["--", "feature"])),
        std::process::ExitCode::FAILURE
    );
    // Hermetic: real `hg` treats a bare `.hg` as operable (`log`
    // prints the null node, `bookmark` exits 0), so the system binary
    // would succeed where hg is installed (Ubuntu CI). Drive the hg
    // engine through an always-failing command instead (T1153 WORK-HG).
    let hg = TempDir::new("hg");
    std::fs::create_dir(hg.path().join(".hg")).unwrap();
    let options = expect_work_args(&["feature"]);
    let hg_cmd = crate::hg::HgCommand::custom("false", &[]);
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_hg_in_with_cmd(hg.path(), &hg_cmd, &options, None, &mut input),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    // T1153 B21/B23: piped runs carry `--` to reach each arm.
    assert_eq!(
        run(&args(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(run(&args(&["--"])), std::process::ExitCode::FAILURE);
    assert_eq!(
        run(&args(&["a", "b", "--"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn parses_object_symbols_like_php() {
    use ObjectSymbol::{Id, Phid};
    assert_eq!(revision_symbol("D123"), Some(Id("0".to_string())));
    assert_eq!(revision_symbol("123"), Some(Id("123".to_string())));
    assert_eq!(revision_symbol("d7"), Some(Id("0".to_string())));
    assert_eq!(
        revision_symbol("PHID-DREV-abc"),
        Some(Phid("PHID-DREV-abc".to_string()))
    );
    for bad in [
        "D",
        "D0",
        "0",
        "D12x",
        "T123",
        "PHID-DREV-",
        "PHID-DREV-a b",
        "feature",
        "",
    ] {
        assert_eq!(revision_symbol(bad), None, "{bad}");
    }
    assert_eq!(task_symbol("T123"), Some(Id("0".to_string())));
    assert_eq!(task_symbol("123"), Some(Id("123".to_string())));
    assert_eq!(task_symbol("t9"), Some(Id("0".to_string())));
    assert_eq!(
        task_symbol("PHID-TASK-abc"),
        Some(Phid("PHID-TASK-abc".to_string()))
    );
    for bad in ["T", "T0", "D123", "PHID-TASK-", "feature"] {
        assert_eq!(task_symbol(bad), None, "{bad}");
    }
    // Bare IDs parse as both; revisions win the dispatch order.
    assert!(looks_like_revision("123"));
    assert!(looks_like_task("123"));
    assert!(symbol_needs_conduit("D1"));
    assert!(!symbol_needs_conduit("feature"));
}

fn search_mock() -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    |method: &str, params: &serde_json::Value| {
        if method == "differential.revision.search" {
            let ids = params
                .get("constraints")
                .and_then(|c| c.get("ids"))
                .cloned()
                .unwrap_or_default();
            if ids == serde_json::json!([123]) {
                return Ok(serde_json::json!({"data": [{
                    "id": 123,
                    "phid": "PHID-DREV-1",
                    "fields": {"title": "Fix it"},
                }]}));
            }
            return Ok(serde_json::json!({"data": []}));
        }
        if method == "maniphest.search" {
            return Ok(serde_json::json!({"data": [{
                "id": 7,
                "phid": "PHID-TASK-1",
            }]}));
        }
        Ok(serde_json::json!({"data": []}))
    }
}

#[test]
fn loads_revision_and_task_objects() {
    let call = search_mock();
    let revision = load_revision(&call, &ObjectSymbol::Id("123".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(revision.monogram, "D123");
    assert_eq!(revision.title, "Fix it");
    assert_eq!(revision.phid, "PHID-DREV-1");
    assert!(
        load_revision(&call, &ObjectSymbol::Id("999".to_string()))
            .unwrap()
            .is_none()
    );
    // Rows whose ID differs from the requested one do not match, and
    // monogram symbols (ID 0 upstream) never match.
    let mismatch = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": [{"id": 123, "phid": "PHID-DREV-1"}]}))
    };
    assert!(
        load_revision(&mismatch, &ObjectSymbol::Id("999".to_string()))
            .unwrap()
            .is_none()
    );
    assert!(
        load_revision(&mismatch, &ObjectSymbol::Id("0".to_string()))
            .unwrap()
            .is_none()
    );
    let task = load_task(&call, &ObjectSymbol::Id("7".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(task.monogram, "T7");
}

#[test]
fn renders_ref_views_and_ambiguous_block() {
    let revision = Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    };
    assert_eq!(
        revision_ref_view(&revision, 80, false),
        "  *   D123 Fix it\n"
    );
    let untitled = Revision {
        title: String::new(),
        ..revision
    };
    assert_eq!(revision_ref_view(&untitled, 80, false), "  *   D123\n");
    let tip = BranchTip {
        name: "feature".to_string(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        tree: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        epoch: 9,
        summary: "work".to_string(),
        message: "work".to_string(),
        // T1153 WORK-HG: git tips render as branches.
        marker_type: "branch".to_string(),
    };
    assert_eq!(
        marker_ref_view(&tip, 80, false),
        "  *   Branch \"feature\" 0123456789ab work\n"
    );
    let block = ambiguous_marker_block("D123", std::slice::from_ref(&tip), 80, false);
    assert!(block.starts_with("\n <!> AMBIGUOUS MARKER \n"), "{block}");
    assert!(block.contains("using the most recent one."), "{block}");
    assert!(block.contains("Branch \"feature\""), "{block}");
    assert!(block.ends_with("\n\n"), "{block:?}");
}

#[test]
fn groups_dirty_paths_by_state() {
    let tmp = init_repo("dirty");
    commit_file(tmp.path(), "a.txt", "one");
    commit_file(tmp.path(), "b.txt", "two");
    // Unstaged modification, staged addition, untracked file.
    std::fs::write(tmp.path().join("a.txt"), "changed\n").unwrap();
    std::fs::write(tmp.path().join("c.txt"), "staged\n").unwrap();
    git(tmp.path(), &["add", "c.txt"]);
    std::fs::write(tmp.path().join("new.txt"), "untracked\n").unwrap();
    let lists = dirty_lists(tmp.path()).unwrap();
    assert_eq!(lists.untracked, vec!["new.txt".to_string()]);
    assert_eq!(lists.unstaged, vec!["a.txt".to_string()]);
    assert_eq!(lists.uncommitted, vec!["c.txt".to_string()]);
    assert!(lists.externals.is_empty());

    let block = render_uncommitted_block(tmp.path(), &lists, false);
    assert!(block.contains(" <!> UNCOMMITTED CHANGES "), "{block}");
    assert!(
        block.contains("Untracked changes in working copy:"),
        "{block}"
    );
    assert!(block.contains("    new.txt\n"), "{block}");
    assert!(
        block.contains("Unstaged changes in working copy:"),
        "{block}"
    );
    assert!(
        block.contains("Uncommitted changes in working copy:"),
        "{block}"
    );
    assert!(block.contains("  ?   To configure Git"), "{block}");

    let externals = render_externals_block(&["sub".to_string()], false);
    assert!(
        externals.contains("A submodule has uncommitted or untracked changes:"),
        "{externals}"
    );
    assert!(externals.contains("      - sub\n"), "{externals}");
    let many = render_externals_block(&["a".to_string(), "b".to_string()], false);
    assert!(
        many.contains("Submodules have uncommitted or untracked changes:"),
        "{many}"
    );
    assert_eq!(
        externals_query(1),
        "Ignore the changes to this submodule and continue?"
    );
    assert_eq!(
        externals_query(3),
        "Ignore the changes to these submodules and continue?"
    );
}

#[test]
fn prompts_accept_retry_and_abort() {
    let mut yes = std::io::Cursor::new(b"y\n");
    assert!(console_confirm(&mut yes, "Go?", true).unwrap());
    let mut empty_no = std::io::Cursor::new(b"\n");
    assert!(!console_confirm(&mut empty_no, "Go?", true).unwrap());
    let mut empty_yes = std::io::Cursor::new(b"\n");
    assert!(console_confirm(&mut empty_yes, "Go?", false).unwrap());
    let mut retry = std::io::Cursor::new(b"maybe\nn\n");
    assert!(!console_confirm(&mut retry, "Go?", false).unwrap());
    let mut eof = std::io::Cursor::new(b"");
    assert!(!console_confirm(&mut eof, "Go?", true).unwrap());

    let mut stash = std::io::Cursor::new(b"y\n");
    assert!(stash_prompt(&mut stash, "Stash?").is_ok());
    let mut saved = std::io::Cursor::new(b"?\ny*\n");
    assert!(stash_prompt(&mut saved, "Stash?").is_ok());
    let mut no = std::io::Cursor::new(b"\n");
    assert_eq!(stash_prompt(&mut no, "Stash?").unwrap_err(), ABORT);
    let mut eof = std::io::Cursor::new(b"");
    assert_eq!(stash_prompt(&mut eof, "Stash?").unwrap_err(), ABORT);
}

fn stash_list(dir: &Path) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(["stash", "list"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn stashes_dirty_trees_around_branch_creation() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("stash");
    commit_file(tmp.path(), "f.txt", "one");
    std::fs::write(tmp.path().join("extra.txt"), "untracked\n").unwrap();
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "feature"]), None, &mut input),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), "feature");
    // The stash was re-applied and dropped.
    assert!(tmp.path().join("extra.txt").is_file());
    assert!(stash_list(tmp.path()).is_empty());
    assert!(
        dirty_lists(tmp.path())
            .unwrap()
            .untracked
            .contains(&"extra.txt".to_string())
    );
}

#[test]
fn declining_the_stash_aborts() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("nostash");
    commit_file(tmp.path(), "f.txt", "one");
    std::fs::write(tmp.path().join("f.txt"), "dirty\n").unwrap();
    let mut input = std::io::Cursor::new(b"n\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "feature"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
    assert!(!branch_exists(tmp.path(), "feature"));
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn failures_restore_state_and_stash() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("restore");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "dirty\n").unwrap();
    // "HEAD" is not a valid new branch name, so creation fails after the
    // stash; the failure must restore the branch and the stashed change.
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "HEAD"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(current_branch(tmp.path()), first);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("f.txt")).unwrap(),
        "dirty\n"
    );
    assert!(stash_list(tmp.path()).is_empty());
}

#[test]
fn missing_tasks_fail_and_existing_tasks_hit_todo() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("tasks");
    commit_file(tmp.path(), "f.txt", "one");
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "T1"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    // Monograms resolve to ID 0 upstream, so they never match, even when
    // the object exists; bare IDs hit the upstream TODO instead.
    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "T7"]), Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "7"]), Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
}

fn commit_with_footer(dir: &Path, name: &str, footer: &str) {
    let message = format!("subject {name}\n\nDifferential Revision: {footer}\n");
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-m", &message]);
}

#[test]
fn revision_symbols_switch_to_the_associated_branch() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("revwork");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "-q", "-b", "feature"]);
    commit_with_footer(tmp.path(), "g.txt", "D123");
    git(tmp.path(), &["checkout", "-q", &first]);

    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "123"]), Some(&call), &mut input),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), "feature");
}

#[test]
fn unassociated_revisions_fail_without_switching() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("revnone");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "123"]), Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(current_branch(tmp.path()), first);
}

fn commit_at_date(dir: &Path, name: &str, footer: &str, date: &str) {
    let message = format!("subject {name}\n\nDifferential Revision: {footer}\n");
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(["commit", "-m", &message])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn ambiguous_revisions_pick_the_most_recent_branch() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("revamb");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "-q", "-b", "older"]);
    commit_at_date(tmp.path(), "o.txt", "D123", "2005-01-01T00:00:00");
    git(tmp.path(), &["checkout", "-q", &first]);
    git(tmp.path(), &["checkout", "-q", "-b", "newer"]);
    commit_at_date(tmp.path(), "n.txt", "D123", "2025-01-01T00:00:00");
    git(tmp.path(), &["checkout", "-q", &first]);

    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "123"]), Some(&call), &mut input),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), "newer");
}

#[test]
fn monogram_symbols_never_match_upstream() {
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = init_repo("revmono");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "-q", "-b", "feature"]);
    commit_with_footer(tmp.path(), "g.txt", "D123");
    git(tmp.path(), &["checkout", "-q", &first]);
    // The same fixture resolves under the bare ID but not the monogram.
    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "D123"]), Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(current_branch(tmp.path()), first);
}

#[test]
fn save_and_restore_roundtrips_branches() {
    let tmp = init_repo("savestate");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    let state = save_work_state(tmp.path()).unwrap();
    assert_eq!(state.ref_name.as_deref(), Some(first.as_str()));
    git(tmp.path(), &["checkout", "-q", "-b", "other"]);
    restore_work_state(tmp.path(), &state).unwrap();
    assert_eq!(current_branch(tmp.path()), first);

    git(tmp.path(), &["checkout", "-q", "--detach", "HEAD"]);
    let detached = save_work_state(tmp.path()).unwrap();
    assert!(detached.ref_name.is_none());
    git(tmp.path(), &["checkout", "-q", &first]);
    restore_work_state(tmp.path(), &detached).unwrap();
    assert_eq!(
        git(tmp.path(), &["rev-parse", "HEAD"]),
        git(tmp.path(), &["rev-parse", &detached.commit])
    );
}
// T1153 WORK-HG: hg tests use an inline `sh` fake (no `hg` binary in
// CI), like `land35_hg_without_shelve_matches_php_error`.
fn hg_fake_script() -> &'static str {
    // NOTE: the `log` arm dispatches on fixture presence, not on the
    // `--template` value: matching `{desc}` in sh fails under Git
    // Bash on Windows (observed: footer association came back
    // empty), while plain `cat`/`printf` works everywhere. The
    // single `.fakehg-msg` test then also sees message bytes as the
    // canonical node; its hash-miss still holds and `commit` is
    // unasserted there (T1153 WORK-HG).
    "#!/bin/sh\necho \"$*\" >> .fakehg.log\nargs=\"$*\"\ncase \"$args\" in\n  *--encoding*) shift 2 2>/dev/null || true;;\nesac\nsub=\"\"\nfor a in \"$@\"; do case \"$a\" in --encoding|--config|extensions.*) continue;; -*) continue;; *) sub=\"$a\"; break;; esac; done\nif [ -e \".fakehg-fail-$sub\" ]; then exit 1; fi\nNODE=\"0123456789abcdef0123456789abcdef01234567\"\ncase \"$sub\" in\n  branch) if [ \"$args\" != \"${args%--force*}\" ]; then exit 0; fi\n    if [ -e .fakehg-branch ]; then cat .fakehg-branch; else echo \"default\"; fi;;\n  bookmarks) if [ -e .fakehg-bookmarks ]; then cat .fakehg-bookmarks; fi;;\n  branches) if [ -e .fakehg-branches ]; then cat .fakehg-branches; fi;;\n  status) if [ -e .fakehg-status ]; then cat .fakehg-status; fi;;\n  log) if [ -e .fakehg-msg ]; then cat .fakehg-msg; else printf '%s' \"$NODE\"; fi;;\n  update|checkout|bookmark) exit 0;;\n  shelve|unshelve) if [ -e .fakehg-no-shelve ]; then exit 1; fi; exit 0;;\n  *) exit 0;;\nesac\nexit 0\n"
}

fn hg_test_repo(tag: &str) -> (TempDir, crate::hg::HgCommand) {
    let tmp = TempDir::new(tag);
    std::fs::create_dir_all(tmp.path().join(".hg")).unwrap();
    let script = tmp.path().join("fakehg.sh");
    std::fs::write(&script, hg_fake_script()).unwrap();
    let hg_cmd = crate::hg::HgCommand::custom("sh", &[&script.to_string_lossy()]);
    (tmp, hg_cmd)
}

fn hg_read_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".fakehg.log")).unwrap_or_default()
}

#[test]
fn hg_default_start_is_dot() {
    // T1153 WORK-HG: `ArcanistMercurialAPI.php:451-453` returns `.`.
    assert_eq!(hg_default_start(), ".");
    assert_eq!(crate::hg::get_working_copy_revision(), ".");
}

#[test]
fn hg_parses_bookmarks_and_active_mark() {
    // T1153 WORK-HG: `*` marks the active bookmark.
    assert_eq!(
        parse_hg_bookmarks_active("  feat  3:abc\n* active  5:def\n"),
        Some("active".to_string())
    );
    assert_eq!(parse_hg_bookmarks_active("  feat  3:abc\n"), None);
    assert_eq!(parse_hg_bookmarks_active(""), None);
    let names = parse_hg_bookmark_names("  feat  3:abc\n* active  5:def\n");
    assert_eq!(names, vec!["feat".to_string(), "active".to_string()]);
    let nodes = parse_hg_bookmark_nodes("  feat  3:abc123\n* active  5:def456\n");
    assert_eq!(
        nodes,
        vec![
            ("feat".to_string(), "abc123".to_string()),
            ("active".to_string(), "def456".to_string()),
        ]
    );
}

#[test]
fn b21_work_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and missing symbols. Unknown flags still report
    // first; symbol-count validation runs after the terminator check.
    assert!(matches!(
        parse_args_with_tty(&args(&["feature"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["--help"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&[]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["a", "b"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["--bogus", "a"]), false),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["--", "feature"]), false),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["feature"]), true),
        ParseOutcome::Args(_)
    ));
    assert_eq!(
        arc_rs::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn hg_renders_bookmark_ref_views() {
    // T1153 WORK-HG: `ArcanistMarkerRef.php:27-36` renders
    // `Bookmark "x"` for bookmarks, `Branch "x"` otherwise.
    let bookmark = BranchTip {
        name: "feat".to_string(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        tree: String::new(),
        epoch: 0,
        summary: "work".to_string(),
        message: "work".to_string(),
        marker_type: "bookmark".to_string(),
    };
    assert_eq!(
        marker_ref_view(&bookmark, 80, false),
        "  *   Bookmark \"feat\" 0123456789ab work\n"
    );
    let branch = BranchTip {
        marker_type: "branch".to_string(),
        ..bookmark
    };
    assert!(marker_ref_view(&branch, 80, false).contains("Branch \"feat\""));
}

#[test]
fn hg_shelve_names_match_php_shape() {
    // T1153 WORK-HG: `arc-` plus 12 `[a-z2-7]`
    // (`Filesystem.php:538-558`).
    for _ in 0..10 {
        let name = new_hg_shelve_name();
        assert!(name.starts_with("arc-"), "{name}");
        assert_eq!(name.len(), 16, "{name}");
        assert!(
            name[4..]
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7')),
            "{name}"
        );
    }
}

#[test]
fn hg_dirty_lists_group_status_flags() {
    // T1153 WORK-HG: tracked files are uncommitted
    // (`ArcanistMercurialAPI.php:307-323`).
    let (tmp, hg_cmd) = hg_test_repo("hg-dirty");
    std::fs::write(tmp.path().join(".fakehg-status"), "M a.txt\n? new.txt\n").unwrap();
    let lists = hg_dirty_lists_with(tmp.path(), &hg_cmd).unwrap();
    assert_eq!(lists.untracked, vec!["new.txt".to_string()]);
    assert_eq!(lists.uncommitted, vec!["a.txt".to_string()]);
    assert!(lists.unstaged.is_empty());
    assert!(lists.externals.is_empty());
}

#[test]
fn hg_save_and_restore_roundtrips_state() {
    // T1153 WORK-HG: save reads commit/branch/bookmark; restore runs
    // `update`, `branch --force`, `bookmark --force`
    // (`ArcanistMercurialLocalState.php:82-107`).
    let (tmp, hg_cmd) = hg_test_repo("hg-state");
    std::fs::write(
        tmp.path().join(".fakehg-bookmarks"),
        "  feat  3:abc\n* active  5:def\n",
    )
    .unwrap();
    let state = save_hg_state_with(tmp.path(), &hg_cmd).unwrap();
    assert_eq!(state.commit, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(state.branch, "default");
    assert_eq!(state.bookmark.as_deref(), Some("active"));
    restore_hg_state_with(tmp.path(), &hg_cmd, &state).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("update -- 0123456789abcdef"), "{log}");
    assert!(log.contains("branch --force -- default"), "{log}");
    assert!(log.contains("bookmark --force -- active"), "{log}");
}

#[test]
fn hg_markers_match_bookmarks_and_branches() {
    // T1153 WORK-HG: symbols match bookmarks plus branches
    // (`ArcanistWorkEngine.php:37-57`).
    let (tmp, hg_cmd) = hg_test_repo("hg-markers");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  feat  3:abc\n").unwrap();
    std::fs::write(
        tmp.path().join(".fakehg-branches"),
        "default                    1:abc123\n",
    )
    .unwrap();
    let markers = hg_markers_for_symbol_with(tmp.path(), &hg_cmd, "feat");
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].1, "bookmark");
    let markers = hg_markers_for_symbol_with(tmp.path(), &hg_cmd, "default");
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].1, "branch");
    assert!(hg_markers_for_symbol_with(tmp.path(), &hg_cmd, "missing").is_empty());
    // Same name in both lists is ambiguous.
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  default  3:abc\n").unwrap();
    let markers = hg_markers_for_symbol_with(tmp.path(), &hg_cmd, "default");
    assert_eq!(markers.len(), 2);
}

#[test]
fn hg_dispatch_checks_out_and_creates_bookmarks() {
    // T1153 WORK-HG: existing markers check out
    // (`:31-54`); new symbols become bookmarks (`:11-29`).
    let (tmp, hg_cmd) = hg_test_repo("hg-dispatch");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  feat  3:abc\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let call = empty_conduit_data;
    hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "feat", None, &call).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("checkout feat --"), "{log}");
    // New symbol creates a bookmark from `.` (no `update` when the
    // start is the default).
    std::fs::remove_file(tmp.path().join(".fakehg.log")).ok();
    hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "novel", None, &call).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("bookmark novel --"), "{log}");
    assert!(!log.contains("update --"), "{log}");
    // Explicit startpoints update first.
    std::fs::remove_file(tmp.path().join(".fakehg.log")).ok();
    hg_dispatch_symbol_with(
        tmp.path(),
        &hg_cmd,
        "branched",
        Some("abc123".to_string()),
        &call,
    )
    .unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("update -- abc123"), "{log}");
    assert!(log.contains("bookmark branched --"), "{log}");
}

#[test]
fn hg_ambiguous_symbols_fail_like_php() {
    // T1153 WORK-HG: `ArcanistWorkEngine.php:42-51` throws
    // `Symbol "x" is ambiguous.`
    let (tmp, hg_cmd) = hg_test_repo("hg-ambiguous");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  dup  3:abc\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "dup  1:abc123\n").unwrap();
    let call = empty_conduit_data;
    let err = hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "dup", None, &call).unwrap_err();
    assert_eq!(err, "Symbol \"dup\" is ambiguous.");
}

#[test]
fn hg_without_shelve_fails_like_php() {
    // T1153 WORK-HG: without shelve, hg fails with the verbatim PHP
    // usage error (`ArcanistRepositoryLocalState.php:143-148`).
    let (tmp, hg_cmd) = hg_test_repo("hg-noshelve");
    std::fs::write(tmp.path().join(".fakehg-status"), "M a.txt\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-no-shelve"), "").unwrap();
    let lists = hg_dirty_lists_with(tmp.path(), &hg_cmd).unwrap();
    let mut input: &[u8] = b"y\n";
    let err = hg_maybe_shelve_with(tmp.path(), &hg_cmd, &mut input, &lists).unwrap_err();
    assert_eq!(
        err,
        "You can not continue with uncommitted changes. Commit or discard them before proceeding."
    );
}

#[test]
fn hg_revision_symbols_find_hash_and_footer_markers() {
    // T1153 WORK-HG: hg markers associate via `hgcm` hashes
    // (`ArcanistMercurialAPI.php:578-592`) plus `Differential
    // Revision:` footers, like git.
    let (tmp, hg_cmd) = hg_test_repo("hg-rev");
    std::fs::write(
        tmp.path().join(".fakehg-bookmarks"),
        "  feat  3:0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    std::fs::write(
        tmp.path().join(".fakehg-msg"),
        "subject\n\nDifferential Revision: https://phab.example/D123\n",
    )
    .unwrap();
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.query" {
            // Hash path misses (wrong node); footer path hits.
            return Ok(serde_json::json!([{
                "phid": "PHID-DREV-1",
                "hashes": [["hgcm", "ffffffffffffffffffffffffffffffffffffffff"]],
            }]));
        }
        assert_eq!(method, "differential.revision.search");
        let ids = params
            .get("constraints")
            .and_then(|c| c.get("ids"))
            .cloned()
            .unwrap_or_default();
        if ids == serde_json::json!(["123"]) || ids == serde_json::json!([123]) {
            return Ok(serde_json::json!({"data": [{
                "id": 123,
                "phid": "PHID-DREV-1",
            }]}));
        }
        Ok(serde_json::json!({"data": []}))
    };
    let revision = Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    };
    let tips = hg_branches_for_revision_with(&call, tmp.path(), &hg_cmd, &revision).unwrap();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].name, "feat");
    assert_eq!(tips[0].marker_type, "bookmark");
    // A footer naming an unknown revision searches other ids (empty
    // result) and associates nothing.
    std::fs::write(
        tmp.path().join(".fakehg-msg"),
        "subject\n\nDifferential Revision: https://phab.example/D999\n",
    )
    .unwrap();
    let tips = hg_branches_for_revision_with(&call, tmp.path(), &hg_cmd, &revision).unwrap();
    assert!(tips.is_empty());
}

#[test]
fn r10_work_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "WORK WORKFLOW\n\
         \n\
         work [--start start] symbol\n\
         Begin or resume work.\n\
         \n\
         Begin or resume work on a branch, bookmark, task, or revision.\n\
         \n\
         The symbol may be a branch or bookmark name, a revision name (like \"D123\"), a\n\
         task name (like \"T123\"), or a new symbol.\n\
         \n\
         If you provide a symbol which currently does not identify any ongoing work,\n\
         Arcanist will create a new branch or bookmark with the name you provide.\n\
         \n\
         If you provide the name of an existing branch or bookmark, Arcanist will\n\
         switch to that branch or bookmark.\n\
         \n\
         If you provide the name of a revision or task, Arcanist will look for a\n\
         related branch or bookmark that exists in the working copy. If it finds one,\n\
         it will switch to it. If it does not find one, it will attempt to create a\n\
         new branch or bookmark.\n\
         \n\
         When \"arc work\" creates a branch or bookmark, it will use --start as the\n\
         branchpoint if it is provided. Otherwise, the current working copy state will\n\
         serve as the starting point.\n\
         \n\
         \x20     --start symbol\n\
         \x20         When creating a new branch or bookmark, use this as the branch\n\
         \x20         point.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("WORK WORKFLOW"), "{}", usage());
}

#[test]
fn b23_work_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help work`.
    let text = usage_text(false);
    assert!(text.starts_with("WORK WORKFLOW\n"), "{text}");
    assert!(text.contains("Begin or resume work."), "{text}");
    assert!(text.contains("--start"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn default_start_falls_back_to_head_when_detached() {
    // T1170: the symbolic-ref arm is covered; detached HEAD exercises the
    // `canonical_revision(dir, "HEAD")` fallback (work.rs:285-286).
    let tmp = init_repo("detach-start");
    commit_file(tmp.path(), "f.txt", "one");
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["checkout", "--quiet", "--detach", "HEAD"]);
    assert_eq!(default_start(tmp.path()), Some(head));
}

#[test]
fn start_is_branch_rejects_unresolvable_revs() {
    // T1170: `rev-parse --verify --symbolic-full-name` fails for unknown
    // revs, so the new branch is created without `--track` (work.rs:298).
    let tmp = init_repo("start-branch");
    commit_file(tmp.path(), "f.txt", "one");
    let branch = current_branch(tmp.path());
    assert!(start_is_branch(tmp.path(), &branch));
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    assert!(!start_is_branch(tmp.path(), &head));
    assert!(!start_is_branch(tmp.path(), "no-such-rev-anywhere"));
}

#[test]
fn load_revision_accepts_non_numeric_and_phid_ids() {
    // T1170: non-numeric IDs serialize as strings (`id_number` fallback,
    // work.rs:318) and PHID symbols query by `phids` (work.rs:327,386).
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.revision.search");
        seen.borrow_mut().push(params.clone());
        Ok(serde_json::json!({"data": [{
            "id": "abc",
            "phid": "PHID-DREV-9",
            "fields": {"title": "Odd"},
        }]}))
    };
    let revision = load_revision(&call, &ObjectSymbol::Id("abc".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(revision.monogram, "Dabc");
    assert_eq!(
        seen.borrow().as_slice(),
        &[serde_json::json!({"constraints": {"ids": ["abc"]}})]
    );

    let seen_phid: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let phid_call = |_: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        seen_phid.borrow_mut().push(params.clone());
        Ok(serde_json::json!({"data": [
            {"id": 1, "phid": "PHID-DREV-OTHER"},
            {"id": 9, "phid": "PHID-DREV-9", "fields": {"title": "By phid"}},
        ]}))
    };
    let revision = load_revision(&phid_call, &ObjectSymbol::Phid("PHID-DREV-9".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(revision.monogram, "D9");
    assert_eq!(revision.title, "By phid");
    assert_eq!(
        seen_phid.borrow().as_slice(),
        &[serde_json::json!({"constraints": {"phids": ["PHID-DREV-9"]}})]
    );
    assert!(
        load_revision(
            &phid_call,
            &ObjectSymbol::Phid("PHID-DREV-MISSING".to_string())
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn branches_for_revision_ignores_scalar_query_results() {
    // T1170: a non-list, non-map `differential.query` payload has no rows
    // (`query_rows` fallback, work.rs:349).
    let tmp = init_repo("scalar-query");
    commit_file(tmp.path(), "f.txt", "one");
    let revision = Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    };
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!(7))
    };
    assert_eq!(
        branches_for_revision(&call, tmp.path(), &revision).unwrap(),
        Vec::new()
    );
}

#[test]
fn parse_tip_record_rejects_malformed_records() {
    // T1170: short or empty `for-each-ref` records yield no tip
    // (work.rs:467); well-formed records still parse.
    assert!(parse_tip_record("").is_none());
    assert!(parse_tip_record("just-a-branch-name").is_none());
    assert!(parse_tip_record("a\x01b\x01c").is_none());
    let tip = parse_tip_record("main\x01abc\x01def\x01123\x01subj\x01body\x02").unwrap();
    assert_eq!(tip.name, "main");
    assert_eq!(tip.commit, "abc");
    assert_eq!(tip.epoch, 123);
    assert_eq!(tip.marker_type, "branch");
}

#[test]
fn local_branch_tips_reports_for_each_ref_failures() {
    // T1170: outside a repo `for-each-ref` exits nonzero (work.rs:488-491).
    let tmp = TempDir::new("plain-tips");
    let err = local_branch_tips(tmp.path()).unwrap_err();
    assert!(err.starts_with("git for-each-ref failed:"), "{err}");
}

#[test]
fn hash_matches_git_commit_and_tree_pairs() {
    // T1170: `differential.query` hash entries match `gtcm`/`gttr` pairs
    // only (work.rs:500-510).
    let tip = BranchTip {
        name: "feature".to_string(),
        commit: "commit-hash".to_string(),
        tree: "tree-hash".to_string(),
        epoch: 0,
        summary: String::new(),
        message: String::new(),
        marker_type: "branch".to_string(),
    };
    assert!(hash_matches(
        &serde_json::json!(["gtcm", "commit-hash"]),
        &tip
    ));
    assert!(!hash_matches(&serde_json::json!(["gtcm", "other"]), &tip));
    assert!(hash_matches(
        &serde_json::json!(["gttr", "tree-hash"]),
        &tip
    ));
    assert!(!hash_matches(&serde_json::json!(["gttr", "other"]), &tip));
    assert!(!hash_matches(
        &serde_json::json!(["hgcm", "commit-hash"]),
        &tip
    ));
    assert!(!hash_matches(&serde_json::json!(["gtcm"]), &tip));
    assert!(!hash_matches(&serde_json::json!("gtcm"), &tip));
    assert!(!hash_matches(&serde_json::json!(7), &tip));
}

#[test]
fn branches_for_revision_matches_hash_hits_and_skips_decoys() {
    // T1170: rows for other revisions are skipped (work.rs:535) while a
    // `gtcm` entry for the tip marks the branch (work.rs:536-544).
    let tmp = init_repo("hash-hit");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "--quiet", "-b", "feature"]);
    commit_file(tmp.path(), "g.txt", "two");
    let feature_commit = git(tmp.path(), &["rev-parse", "feature"]);
    git(tmp.path(), &["checkout", "--quiet", &first]);
    let revision = Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    };
    let call = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.query");
        Ok(serde_json::json!([
            {"phid": "PHID-DREV-OTHER", "hashes": [["gtcm", feature_commit]]},
            {"phid": "PHID-DREV-1", "hashes": [
                ["gttr", "no-such-tree"],
                ["gtcm", feature_commit],
                ["bogus"],
                "not-a-pair",
                7,
            ]},
        ]))
    };
    let tips = branches_for_revision(&call, tmp.path(), &revision).unwrap();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].name, "feature");
}

#[test]
fn branches_for_revision_skips_footer_decoys() {
    // T1170: footer rows for other revisions (work.rs:573) and rows whose
    // ID names no local tip (work.rs:575-579) do not mark branches.
    let tmp = init_repo("footer-decoy");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "--quiet", "-b", "feature"]);
    commit_with_footer(tmp.path(), "g.txt", "D123");
    git(tmp.path(), &["checkout", "--quiet", &first]);
    let revision = Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    };
    let call = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.query" {
            return Ok(serde_json::json!([]));
        }
        Ok(serde_json::json!({"data": [
            {"id": 999, "phid": "PHID-DREV-OTHER"},
            {"id": 123, "phid": "PHID-DREV-1"},
            {"id": 555, "phid": "PHID-DREV-1"},
        ]}))
    };
    let tips = branches_for_revision(&call, tmp.path(), &revision).unwrap();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].name, "feature");
}

fn must_not_call_conduit(_: &str, _: &serde_json::Value) -> Result<serde_json::Value, String> {
    Err("must not be called".to_string())
}

fn empty_conduit_data(_: &str, _: &serde_json::Value) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({"data": []}))
}

#[test]
fn branches_for_revision_is_empty_without_branches() {
    // T1170: no local tips means no association and no Conduit traffic
    // (work.rs:592).
    let tmp = init_repo("no-branches");
    let revision = Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    };
    let call = must_not_call_conduit;
    assert_eq!(
        branches_for_revision(&call, tmp.path(), &revision).unwrap(),
        Vec::new()
    );
}

#[test]
fn flags_for_entry_parses_renames_unmerged_and_unknown() {
    // T1170: `2` rename records (work.rs:999), `u` unmerged records
    // (work.rs:1000), and unknown kinds (work.rs:1014).
    let (path, flags, is_rename) =
        flags_for_entry("2 R. N... 100644 100644 100644 abc1234 abc1234 R100 new.txt").unwrap();
    assert_eq!(path, "new.txt");
    assert!(flags.uncommitted);
    assert!(!flags.unstaged);
    assert!(is_rename);

    let (path, flags, is_rename) = flags_for_entry(
        "u UU N... 100644 100644 100644 100644 h111111 h222222 h333333 conflict.txt",
    )
    .unwrap();
    assert_eq!(path, "conflict.txt");
    assert!(flags.uncommitted);
    assert!(flags.unstaged);
    assert!(!is_rename);

    assert!(flags_for_entry("! ignored.txt").is_none());
    assert!(flags_for_entry("").is_none());
}

#[test]
fn dirty_lists_reports_status_failures() {
    // T1170: `git status` exiting nonzero surfaces its stderr
    // (work.rs:1024-1027).
    let tmp = TempDir::new("plain-status");
    let err = dirty_lists(tmp.path()).unwrap_err();
    assert!(err.starts_with("git status failed:"), "{err}");
}

#[test]
fn dirty_lists_tracks_renames_to_origins() {
    // T1170: staged renames (`2` records) flag both the new path and the
    // NUL-separated origin path (work.rs:1040-1044).
    let tmp = init_repo("rename");
    commit_file(tmp.path(), "a.txt", "one");
    git(tmp.path(), &["mv", "a.txt", "b.txt"]);
    let lists = dirty_lists(tmp.path()).unwrap();
    assert_eq!(
        lists.uncommitted,
        vec!["a.txt".to_string(), "b.txt".to_string()]
    );
    assert!(lists.unstaged.is_empty());
    assert!(lists.untracked.is_empty());
}

fn submodule_repo(tag: &str) -> TempDir {
    // T1170: a repo with a dirty submodule (local `file` clone only, no
    // network) for the externals gate.
    let tmp = TempDir::new(tag);
    let donor = tmp.path().join("donor");
    std::fs::create_dir(&donor).unwrap();
    git(&donor, &["init"]);
    git(&donor, &["config", "user.email", "work@test"]);
    git(&donor, &["config", "user.name", "work"]);
    git(&donor, &["config", "commit.gpgsign", "false"]);
    std::fs::write(donor.join("f.txt"), "x\n").unwrap();
    git(&donor, &["add", "f.txt"]);
    git(&donor, &["commit", "-m", "init"]);
    let main = tmp.path().join("main");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init"]);
    git(&main, &["config", "user.email", "work@test"]);
    git(&main, &["config", "user.name", "work"]);
    git(&main, &["config", "commit.gpgsign", "false"]);
    std::fs::write(main.join("g.txt"), "y\n").unwrap();
    git(&main, &["add", "g.txt"]);
    git(&main, &["commit", "-m", "init"]);
    let status = std::process::Command::new("git")
        .current_dir(&main)
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "../donor",
            "sub",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    git(&main, &["commit", "-m", "add submodule"]);
    std::fs::write(main.join("sub").join("f.txt"), "dirty\n").unwrap();
    tmp
}

#[test]
fn dirty_lists_flags_dirty_submodules_as_externals() {
    // T1170: a submodule with tracked modifications lands in `externals`
    // (work.rs:1059) instead of the staged/unstaged lists.
    let tmp = submodule_repo("sub-externals");
    let main = tmp.path().join("main");
    let lists = dirty_lists(&main).unwrap();
    assert_eq!(lists.externals, vec!["sub".to_string()]);
    assert!(lists.unstaged.is_empty());
    assert!(lists.uncommitted.is_empty());
    assert!(lists.untracked.is_empty());
}

#[test]
fn externals_gate_accepts_and_declines() {
    // T1170: the gate prints the block and honors `y`/`n`
    // (work.rs:1719-1728); clean trees pass without prompting.
    let clean = DirtyLists::default();
    let mut input = std::io::Cursor::new(b"");
    assert!(confirm_externals(&mut input, &clean).is_ok());

    let dirty = DirtyLists {
        externals: vec!["sub".to_string()],
        ..DirtyLists::default()
    };
    let mut yes = std::io::Cursor::new(b"y\n");
    assert!(confirm_externals(&mut yes, &dirty).is_ok());
    let mut no = std::io::Cursor::new(b"n\n");
    assert_eq!(confirm_externals(&mut no, &dirty).unwrap_err(), ABORT);
}

#[test]
fn declining_dirty_submodules_aborts_before_branching() {
    // T1170: declining the externals gate fails the run without creating
    // the branch (work.rs:1832-1834).
    let tmp = submodule_repo("sub-decline");
    let main = tmp.path().join("main");
    let mut input = std::io::Cursor::new(b"n\n");
    assert_eq!(
        run_in_with(&main, &args(&["--", "feature"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
    assert!(!branch_exists(&main, "feature"));
}

#[test]
fn accepting_dirty_submodules_continues() {
    // T1170: accepting the externals gate creates the branch; the
    // submodule stays dirty and untouched.
    let tmp = submodule_repo("sub-accept");
    let main = tmp.path().join("main");
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_in_with(&main, &args(&["--", "feature"]), None, &mut input),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(&main), "feature");
    assert_eq!(
        std::fs::read_to_string(main.join("sub").join("f.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn resolve_start_value_rejects_unresolvable_defaults() {
    // T1170: an unborn branch startpoint (work.rs:1444) and a missing
    // startpoint entirely (work.rs:1445) both fail dispatch.
    let tmp = init_repo("unborn-start");
    let err = resolve_start_value(tmp.path(), None).unwrap_err();
    assert!(err.starts_with("Unable to resolve startpoint"), "{err}");

    let tmp = init_repo("headless-start");
    commit_file(tmp.path(), "f.txt", "one");
    std::fs::remove_file(tmp.path().join(".git").join("HEAD")).unwrap();
    assert_eq!(
        resolve_start_value(tmp.path(), None).unwrap_err(),
        "Unable to determine a startpoint; pass --start <commit>."
    );
}

#[test]
fn work_on_task_reports_todo_when_found() {
    // T1170: an existing task reaches the upstream TODO
    // (work.rs:1633-1634); a missing task reports instead.
    let call = search_mock();
    assert_eq!(
        work_on_task("7", &ObjectSymbol::Id("7".to_string()), &call).unwrap_err(),
        "TODO: Implement this workflow."
    );
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    assert_eq!(
        work_on_task("7", &ObjectSymbol::Id("7".to_string()), &empty).unwrap_err(),
        "No task \"7\" exists, or you do not have permission to view it."
    );
}

#[test]
fn fail_with_restore_reports_restore_failures() {
    // T1170: when the saved position cannot be checked back out, the
    // restore error (not the original) is reported (work.rs:1778-1780).
    let tmp = init_repo("restore-fail");
    commit_file(tmp.path(), "f.txt", "one");
    let bogus = WorkState {
        ref_name: Some("feature".to_string()),
        commit: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string(),
    };
    assert_eq!(
        fail_with_restore(tmp.path(), &bogus, false, "original".to_string()),
        std::process::ExitCode::FAILURE
    );
    assert!(!branch_exists(tmp.path(), "feature"));
}

#[test]
fn fail_with_restore_reports_stash_apply_failures() {
    // T1170: when the state restores but the stash cannot be re-applied,
    // the stash error is reported (work.rs:1782-1784).
    let tmp = init_repo("apply-fail");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    let state = save_work_state(tmp.path()).unwrap();
    assert_eq!(
        fail_with_restore(tmp.path(), &state, true, "original".to_string()),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(current_branch(tmp.path()), first);
}

#[test]
fn execute_fails_when_save_state_fails() {
    // T1170: with no commits there is no position to save
    // (work.rs:1818-1822).
    let tmp = init_repo("empty-exec");
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "feature"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
    assert!(!branch_exists(tmp.path(), "feature"));
}

#[test]
fn execute_fails_when_status_fails() {
    // T1170: a corrupt index fails `dirty_lists` after a successful save
    // (work.rs:1825-1829).
    let tmp = init_repo("corrupt-status");
    commit_file(tmp.path(), "f.txt", "one");
    std::fs::write(tmp.path().join(".git").join("index"), "garbage").unwrap();
    assert!(save_work_state(tmp.path()).is_ok());
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "feature"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
    assert!(!branch_exists(tmp.path(), "feature"));
}

#[test]
fn execute_fails_when_stash_reapply_conflicts() {
    // T1170: a stash that no longer applies after the checkout fails the
    // run; the branch switch stands and the stash is kept
    // (work.rs:1843-1847).
    let tmp = init_repo("apply-conflict");
    commit_file(tmp.path(), "f.txt", "base\n");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "--quiet", "-b", "other"]);
    commit_file(tmp.path(), "f.txt", "other\n");
    git(tmp.path(), &["checkout", "--quiet", &first]);
    std::fs::write(tmp.path().join("f.txt"), "main-dirty\n").unwrap();
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "other"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(current_branch(tmp.path()), "other");
    assert!(!stash_list(tmp.path()).is_empty());
    let merged = std::fs::read_to_string(tmp.path().join("f.txt")).unwrap();
    assert!(merged.contains("<<<<<<<"), "{merged}");
    assert!(merged.contains("main-dirty"), "{merged}");
}

#[test]
fn run_reports_missing_terminator_without_dashes() {
    // T1170: without `--` the terminator arm reports toolset usage
    // (work.rs:1912-1917) whenever stdin is piped (as under `cargo test`
    // and CI); on a TTY the same argv parses and fails outside a repo, so
    // FAILURE holds on both paths.
    let tmp = TempDir::new("plain-terminator");
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["feature"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_in_dispatches_hg_repos() {
    // T1170: a `.hg` entry routes to the Mercurial engine even when it is
    // a plain file (work.rs:1941-1942,1973-1981); with no usable `hg` the
    // save fails.
    let tmp = TempDir::new("hg-file");
    std::fs::write(tmp.path().join(".hg"), "not-a-repo\n").unwrap();
    assert!(is_hg_repo(tmp.path()));
    assert!(!git::is_inside_work_tree(tmp.path()));
    let empty = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(
            tmp.path(),
            &args(&["--", "feature"]),
            Some(&empty),
            &mut input
        ),
        std::process::ExitCode::FAILURE
    );
}

fn revision_123() -> Revision {
    Revision {
        id: "123".to_string(),
        phid: "PHID-DREV-1".to_string(),
        monogram: "D123".to_string(),
        title: "Fix it".to_string(),
    }
}

const FAKE_NODE: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn hg_branch_tips_include_plain_branches() {
    // T1170: `hg branches` entries resolve to full nodes and render as
    // branch markers (work.rs:655-673).
    let (tmp, hg_cmd) = hg_test_repo("hg-branches");
    std::fs::write(
        tmp.path().join(".fakehg-branches"),
        "stable  2:abcdef123456\n",
    )
    .unwrap();
    let tips = hg_branch_tips_with(tmp.path(), &hg_cmd).unwrap();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].name, "stable");
    assert_eq!(tips[0].commit, FAKE_NODE);
    assert_eq!(tips[0].marker_type, "branch");
}

#[test]
fn hg_system_wrappers_handle_plain_dirs() {
    // T1170: the system-binary tips wrappers degrade to empty outside a
    // repo instead of failing (work.rs:678-680,746-752,732).
    let tmp = TempDir::new("hg-plain");
    assert_eq!(hg_branch_tips(tmp.path()).unwrap(), Vec::new());
    let call = must_not_call_conduit;
    assert_eq!(
        hg_branches_for_revision(&call, tmp.path(), &revision_123()).unwrap(),
        Vec::new()
    );
}

#[test]
fn hg_hash_association_skips_decoys() {
    // T1170: query rows for other revisions are skipped (work.rs:701)
    // while an `hgcm` entry for the tip marks the marker (work.rs:716).
    let (tmp, hg_cmd) = hg_test_repo("hg-hash");
    std::fs::write(
        tmp.path().join(".fakehg-bookmarks"),
        "  feat  3:abcdef123456\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let call = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.query");
        Ok(serde_json::json!([
            {"phid": "PHID-DREV-OTHER", "hashes": [["hgcm", FAKE_NODE]]},
            {"phid": "PHID-DREV-1", "hashes": [
                ["hgcm", "ffffffffffffffffffffffffffffffffffffffff"],
                ["gtcm", FAKE_NODE],
                ["hgcm", FAKE_NODE],
            ]},
        ]))
    };
    let tips = hg_branches_for_revision_with(&call, tmp.path(), &hg_cmd, &revision_123()).unwrap();
    assert_eq!(tips.len(), 1);
    assert_eq!(tips[0].name, "feat");
    assert_eq!(tips[0].marker_type, "bookmark");
}

#[test]
fn hg_branches_for_revision_is_empty_without_markers() {
    // T1170: no hg tips means no association and no Conduit traffic
    // (work.rs:732).
    let (tmp, hg_cmd) = hg_test_repo("hg-no-markers");
    let call = must_not_call_conduit;
    assert_eq!(
        hg_branches_for_revision_with(&call, tmp.path(), &hg_cmd, &revision_123()).unwrap(),
        Vec::new()
    );
}

#[test]
fn hg_system_wrappers_fail_outside_repos() {
    // T1170: the system-binary state/shelve wrappers surface `hg`
    // failures outside a repo (work.rs:896-898,949-951,1109-1111,
    // 1397-1399,1423-1425,1481-1483).
    let tmp = TempDir::new("hg-plain-fail");
    assert!(save_hg_state(tmp.path()).is_err());
    let state = HgWorkState {
        commit: FAKE_NODE.to_string(),
        branch: "default".to_string(),
        bookmark: None,
    };
    assert!(restore_hg_state(tmp.path(), &state).is_err());
    assert!(hg_dirty_lists(tmp.path()).is_err());
    assert!(save_hg_shelve(tmp.path()).is_err());
    assert!(apply_hg_shelve(tmp.path(), "arc-test").is_err());
    assert_eq!(hg_canonical_revision(tmp.path(), "."), None);
}

#[test]
fn parse_hg_bookmarks_active_ignores_nameless_marks() {
    // T1170: an active mark with no bookmark name matches nothing
    // (work.rs:909).
    assert_eq!(parse_hg_bookmarks_active("*\n"), None);
    assert_eq!(parse_hg_bookmarks_active("*   \n"), None);
}

#[test]
fn parse_hg_bookmark_names_ignores_blank_lines() {
    // T1170: blank `hg bookmarks` lines yield no names (work.rs:1496).
    assert!(parse_hg_bookmark_names("").is_empty());
    assert!(parse_hg_bookmark_names("   \n").is_empty());
    assert_eq!(
        parse_hg_bookmark_names("  feat  3:abc\n   \n"),
        vec!["feat".to_string()]
    );
}

#[test]
fn parse_hg_bookmark_nodes_ignores_malformed_lines() {
    // T1170: lines without a `rev:node` part yield no entries
    // (work.rs:619).
    assert!(parse_hg_bookmark_nodes("").is_empty());
    assert!(parse_hg_bookmark_nodes("  \n").is_empty());
    assert!(parse_hg_bookmark_nodes("  feat\n").is_empty());
    assert!(parse_hg_bookmark_nodes("  feat abc\n").is_empty());
}

#[test]
fn restore_hg_state_without_bookmark_skips_bookmark_force() {
    // T1170: restoring a bookmark-less position runs `update` plus
    // `branch --force` only (work.rs:931-937,944).
    let (tmp, hg_cmd) = hg_test_repo("hg-restore-plain");
    let state = HgWorkState {
        commit: FAKE_NODE.to_string(),
        branch: "default".to_string(),
        bookmark: None,
    };
    restore_hg_state_with(tmp.path(), &hg_cmd, &state).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("update -- 0123456789abcdef"), "{log}");
    assert!(log.contains("branch --force -- default"), "{log}");
    assert!(!log.contains("bookmark --force"), "{log}");
}

#[test]
fn render_hg_uncommitted_block_hints_hgignore() {
    // T1170: the hg block names `.hgignore` for untracked files
    // (work.rs:1264-1272) and omits the hint otherwise.
    let (tmp, _) = hg_test_repo("hg-render");
    let lists = DirtyLists {
        untracked: vec!["new.txt".to_string()],
        ..DirtyLists::default()
    };
    let block = render_hg_uncommitted_block(tmp.path(), &lists, false);
    assert!(block.contains(" <!> UNCOMMITTED CHANGES "), "{block}");
    assert!(block.contains("    new.txt\n"), "{block}");
    assert!(block.contains(".hgignore"), "{block}");
    let clean = DirtyLists {
        uncommitted: vec!["a.txt".to_string()],
        ..DirtyLists::default()
    };
    assert!(!render_hg_uncommitted_block(tmp.path(), &clean, false).contains(".hgignore"));
}

#[test]
fn save_and_apply_hg_shelve_roundtrip_through_fake() {
    // T1170: shelving returns an `arc-` name (work.rs:1383-1394) and
    // unshelving deletes the shelf (work.rs:1403-1420).
    let (tmp, hg_cmd) = hg_test_repo("hg-shelve");
    let name = save_hg_shelve_with(tmp.path(), &hg_cmd).unwrap();
    assert!(name.starts_with("arc-"), "{name}");
    assert_eq!(name.len(), 16, "{name}");
    apply_hg_shelve_with(tmp.path(), &hg_cmd, &name).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(
        log.contains(&format!("shelve --unknown --name {name} --")),
        "{log}"
    );
    assert!(
        log.contains(&format!("unshelve --keep --name {name} --")),
        "{log}"
    );
    assert!(log.contains(&format!("shelve --delete {name} --")), "{log}");
}

#[test]
fn hg_dispatch_checks_out_branch_markers() {
    // T1170: branch markers check out through the branch arm
    // (work.rs:1546-1547).
    let (tmp, hg_cmd) = hg_test_repo("hg-branch-out");
    std::fs::write(
        tmp.path().join(".fakehg-branches"),
        "stable  1:abcdef123456\n",
    )
    .unwrap();
    let call = empty_conduit_data;
    hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "stable", None, &call).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("checkout stable --"), "{log}");
}

#[test]
fn hg_dispatch_rejects_unresolvable_starts() {
    // T1170: an explicit startpoint that does not resolve fails dispatch
    // (work.rs:1582).
    let (tmp, hg_cmd) = hg_test_repo("hg-bad-start");
    std::fs::write(tmp.path().join(".fakehg-fail-log"), "").unwrap();
    let call = empty_conduit_data;
    assert_eq!(
        hg_dispatch_symbol_with(
            tmp.path(),
            &hg_cmd,
            "novel",
            Some("bogus".to_string()),
            &call
        )
        .unwrap_err(),
        "Unable to resolve startpoint \"bogus\"."
    );
}

#[test]
fn hg_revision_dispatch_reports_missing_and_unassociated() {
    // T1170: unknown revisions (work.rs:1664-1666) and revisions with no
    // local marker (work.rs:1668-1673) fail dispatch (work.rs:1706).
    let (tmp, hg_cmd) = hg_test_repo("hg-rev-missing");
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    assert_eq!(
        hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "999", None, &empty).unwrap_err(),
        "No revision \"999\" exists, or you do not have permission to view it."
    );
    let call = search_mock();
    assert_eq!(
        hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "123", None, &call).unwrap_err(),
        "Revision \"D123\" was not found anywhere in this working copy."
    );
}

#[test]
fn hg_revision_dispatch_checks_out_single_marker() {
    // T1170: one associated marker checks out without the ambiguous
    // banner (work.rs:1674-1677,1684-1687).
    let (tmp, hg_cmd) = hg_test_repo("hg-rev-single");
    std::fs::write(
        tmp.path().join(".fakehg-bookmarks"),
        "  feat  3:abcdef123456\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.query" {
            return Ok(serde_json::json!([{
                "phid": "PHID-DREV-1",
                "hashes": [["hgcm", FAKE_NODE]],
            }]));
        }
        search_mock()(method, params)
    };
    hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "123", None, &call).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("checkout feat --"), "{log}");
}

#[test]
fn live_client_without_credentials_fails_like_php() {
    // Git live-conduit path with no token for the conduit host: client
    // construction fails and the command exits FAILURE (nothing dials).
    let tmp = init_repo("live-git-nocreds");
    commit_file(tmp.path(), "a.txt", "one");
    let creds = TempDir::new("live-git-nocreds-creds");
    let arcrc = creds.path().join("fake.arcrc");
    std::fs::write(&arcrc, r#"{"hosts": {}}"#).unwrap();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with_user_file(
            tmp.path(),
            &args(&["--", "123"]),
            None,
            &mut input,
            Some(&arcrc),
            Some("http://127.0.0.1:9/"),
        ),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn live_client_without_credentials_fails_hg_like_php() {
    // Hg live-conduit twin: no token for the conduit host fails the
    // client before any marker lookup dials out.
    let (tmp, hg_cmd) = hg_test_repo("live-hg-nocreds");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  feat  3:abc\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let arcrc = tmp.path().join("fake.arcrc");
    std::fs::write(&arcrc, r#"{"hosts": {}}"#).unwrap();
    let options = hg_options("123");
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_hg_in_with_cmd_user_file(
            tmp.path(),
            &hg_cmd,
            &options,
            None,
            &mut input,
            Some(&arcrc),
            Some("http://127.0.0.1:9/"),
        ),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn hg_revision_dispatch_picks_most_recent_marker() {
    // T1170: several associated markers print the ambiguous banner and
    // check out the most recent one (work.rs:1678-1683).
    let (tmp, hg_cmd) = hg_test_repo("hg-rev-amb");
    std::fs::write(
        tmp.path().join(".fakehg-bookmarks"),
        "  feat1  1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n  feat2  2:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let call = |method: &str, params: &serde_json::Value| -> Result<serde_json::Value, String> {
        if method == "differential.query" {
            return Ok(serde_json::json!([{
                "phid": "PHID-DREV-1",
                "hashes": [["hgcm", FAKE_NODE]],
            }]));
        }
        search_mock()(method, params)
    };
    hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "123", None, &call).unwrap();
    let log = hg_read_log(tmp.path());
    assert!(log.contains("checkout feat2 --"), "{log}");
}

#[test]
fn hg_dispatch_routes_task_symbols() {
    // T1170: task monograms route to the task arm (work.rs:1709).
    let (tmp, hg_cmd) = hg_test_repo("hg-task");
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    assert_eq!(
        hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "T7", None, &empty).unwrap_err(),
        "No task \"T7\" exists, or you do not have permission to view it."
    );
}

#[test]
fn hg_maybe_shelve_returns_none_when_clean() {
    // T1170: clean trees shelve nothing (work.rs:1758).
    let (tmp, hg_cmd) = hg_test_repo("hg-clean");
    let mut input: &[u8] = b"";
    assert_eq!(
        hg_maybe_shelve_with(tmp.path(), &hg_cmd, &mut input, &DirtyLists::default()).unwrap(),
        None
    );
}

#[test]
fn hg_maybe_shelve_shelves_and_declines() {
    // T1170: dirty trees with shelve support shelve on `y`
    // (work.rs:1771-1772) and abort on `n`.
    let (tmp, hg_cmd) = hg_test_repo("hg-shelve-gate");
    std::fs::write(tmp.path().join(".fakehg-status"), "M a.txt\n").unwrap();
    let lists = hg_dirty_lists_with(tmp.path(), &hg_cmd).unwrap();
    assert_eq!(lists.uncommitted, vec!["a.txt".to_string()]);
    let mut yes: &[u8] = b"y\n";
    let name = hg_maybe_shelve_with(tmp.path(), &hg_cmd, &mut yes, &lists)
        .unwrap()
        .unwrap();
    assert!(name.starts_with("arc-"), "{name}");
    let mut no: &[u8] = b"n\n";
    assert_eq!(
        hg_maybe_shelve_with(tmp.path(), &hg_cmd, &mut no, &lists).unwrap_err(),
        ABORT
    );
}

#[test]
fn hg_fail_with_restore_reports_each_stage() {
    // T1170: restore failures (work.rs:1798-1800), unshelve failures
    // (work.rs:1802-1807), and the plain path (work.rs:1808-1809) all
    // report FAILURE.
    let state = HgWorkState {
        commit: FAKE_NODE.to_string(),
        branch: "default".to_string(),
        bookmark: None,
    };
    let dead = crate::hg::HgCommand::custom("false", &[]);
    let (tmp, _) = hg_test_repo("hg-fail-restore");
    assert_eq!(
        hg_fail_with_restore_with(tmp.path(), &dead, &state, None, "original".to_string()),
        std::process::ExitCode::FAILURE
    );

    let (tmp, hg_cmd) = hg_test_repo("hg-fail-unshelve");
    std::fs::write(tmp.path().join(".fakehg-fail-unshelve"), "").unwrap();
    assert_eq!(
        hg_fail_with_restore_with(
            tmp.path(),
            &hg_cmd,
            &state,
            Some("arc-test"),
            "original".to_string()
        ),
        std::process::ExitCode::FAILURE
    );
    let log = hg_read_log(tmp.path());
    assert!(log.contains("unshelve --keep --name arc-test"), "{log}");

    let (tmp, hg_cmd) = hg_test_repo("hg-fail-plain");
    assert_eq!(
        hg_fail_with_restore_with(tmp.path(), &hg_cmd, &state, None, "original".to_string()),
        std::process::ExitCode::FAILURE
    );
    let log = hg_read_log(tmp.path());
    assert!(!log.contains("unshelve"), "{log}");
}

fn hg_options(symbol: &str) -> Options {
    match parse_args_with_tty(&args(&[symbol]), true) {
        ParseOutcome::Args(options) => options,
        _ => panic!("{symbol} parses"),
    }
}

#[test]
fn run_hg_in_with_cmd_runs_bookmark_flow() {
    // T1170: the injected-call path (work.rs:1992) saves state
    // (work.rs:1865), passes clean gates, and creates the bookmark
    // (work.rs:1889-1897).
    let (tmp, hg_cmd) = hg_test_repo("hg-run-ok");
    let options = hg_options("novel");
    let call = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_hg_in_with_cmd(tmp.path(), &hg_cmd, &options, Some(&call), &mut input),
        std::process::ExitCode::SUCCESS
    );
    let log = hg_read_log(tmp.path());
    assert!(log.contains("bookmark novel --"), "{log}");
}

#[test]
fn run_hg_in_with_cmd_shelves_and_restores() {
    // T1170: dirty trees shelve before dispatch and unshelve after
    // success (work.rs:1891-1893).
    let (tmp, hg_cmd) = hg_test_repo("hg-run-shelve");
    std::fs::write(tmp.path().join(".fakehg-status"), "M a.txt\n").unwrap();
    let options = hg_options("novel");
    let call = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_hg_in_with_cmd(tmp.path(), &hg_cmd, &options, Some(&call), &mut input),
        std::process::ExitCode::SUCCESS
    );
    let log = hg_read_log(tmp.path());
    assert!(log.contains("shelve --unknown --name arc-"), "{log}");
    assert!(log.contains("bookmark novel --"), "{log}");
    assert!(log.contains("unshelve --keep --name arc-"), "{log}");
    assert!(log.contains("shelve --delete arc-"), "{log}");
}

#[test]
fn run_hg_in_with_cmd_fails_when_unshelve_fails() {
    // T1170: a shelf that cannot be restored after success fails the run
    // (work.rs:1891-1895).
    let (tmp, hg_cmd) = hg_test_repo("hg-run-unshelve");
    std::fs::write(tmp.path().join(".fakehg-status"), "M a.txt\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-fail-unshelve"), "").unwrap();
    let options = hg_options("novel");
    let call = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_hg_in_with_cmd(tmp.path(), &hg_cmd, &options, Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    let log = hg_read_log(tmp.path());
    assert!(log.contains("bookmark novel --"), "{log}");
    assert!(log.contains("unshelve --keep --name arc-"), "{log}");
}

#[test]
fn run_hg_in_with_cmd_fails_when_status_fails() {
    // T1170: a failing `hg status` fails the run after a successful save
    // (work.rs:1871-1875).
    let (tmp, hg_cmd) = hg_test_repo("hg-run-status");
    std::fs::write(tmp.path().join(".fakehg-fail-status"), "").unwrap();
    let options = hg_options("novel");
    let call = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_hg_in_with_cmd(tmp.path(), &hg_cmd, &options, Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    let log = hg_read_log(tmp.path());
    assert!(!log.contains("bookmark novel --"), "{log}");
}

#[test]
fn run_hg_in_with_cmd_fails_without_shelve() {
    // T1170: dirty trees without shelve support fail the run with the
    // PHP usage error (work.rs:1882-1886).
    let (tmp, hg_cmd) = hg_test_repo("hg-run-noshelve");
    std::fs::write(tmp.path().join(".fakehg-status"), "M a.txt\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-no-shelve"), "").unwrap();
    let options = hg_options("novel");
    let call = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_hg_in_with_cmd(tmp.path(), &hg_cmd, &options, Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    let log = hg_read_log(tmp.path());
    assert!(!log.contains("bookmark novel --"), "{log}");
}

#[test]
fn run_hg_in_with_cmd_restores_after_ambiguous_dispatch() {
    // T1170: dispatch failures restore the saved position
    // (work.rs:1899 via `hg_fail_with_restore_with`).
    let (tmp, hg_cmd) = hg_test_repo("hg-run-amb");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  dup  3:abc\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "dup  1:abc123\n").unwrap();
    let options = hg_options("dup");
    let call = empty_conduit_data;
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_hg_in_with_cmd(tmp.path(), &hg_cmd, &options, Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    let log = hg_read_log(tmp.path());
    assert!(log.contains("update -- 0123456789abcdef"), "{log}");
    assert!(log.contains("branch --force -- default"), "{log}");
}

#[test]
fn apply_hg_shelve_reports_failed_delete() {
    // T1170: a restored shelf that cannot be deleted fails the step
    // (work.rs:1412-1418).
    let (tmp, hg_cmd) = hg_test_repo("hg-apply-fail");
    std::fs::write(tmp.path().join(".fakehg-fail-shelve"), "").unwrap();
    let err = apply_hg_shelve_with(tmp.path(), &hg_cmd, "arc-test").unwrap_err();
    assert!(!err.is_empty());
    let log = hg_read_log(tmp.path());
    assert!(log.contains("unshelve --keep --name arc-test"), "{log}");
    assert!(log.contains("shelve --delete arc-test"), "{log}");
}

// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "--bogus parses")]
fn hg_options_panics_on_invalid_symbols() {
    // `hg_options` unwraps `Args`: an unknown flag panics.
    hg_options("--bogus");
}

#[test]
#[should_panic(expected = "failed:")]
fn git_helper_reports_failing_commands() {
    // A failing git invocation panics with the command and stderr.
    let tmp = TempDir::new("w6h-git-fail");
    git(tmp.path(), &["--bogus-flag-xyz"]);
}

#[test]
#[should_panic(expected = "feature parses")]
fn requires_a_supported_working_copy_defensive_error_arm() {
    // `expect_work_args` unwraps `Args`: an empty symbol errors.
    expect_work_args(&[]);
}
// -( T1176 S10: stub/guard coverage )----------------------------------------

#[test]
fn must_not_call_conduit_companion_reports_the_canned_error() {
    // The must-not-call guard keeps guarding: direct calls fail closed
    // with the byte-identical message the callers propagate.
    assert_eq!(
        must_not_call_conduit("m", &serde_json::json!({})).unwrap_err(),
        "must not be called"
    );
}

#[test]
fn empty_conduit_data_companion_returns_the_canned_payload() {
    // The never-invoked stub keeps its canned empty payload.
    assert_eq!(
        empty_conduit_data("m", &serde_json::json!({})).unwrap(),
        serde_json::json!({"data": []})
    );
}
// -( T1176 S12: dead-arm coverage )------------------------------------------

#[test]
fn conduit_unavailable_pill_companion_reports_the_canned_error() {
    // The poison-pill conduit caller keeps failing closed with the
    // byte-identical message the dispatch callers propagate.
    assert_eq!(
        conduit_unavailable_pill("m", &serde_json::json!({})).unwrap_err(),
        "conduit unavailable"
    );
}
// -( T1176 PC-4: work-upgrade demark )----------------------------------------

#[test]
fn flags_for_entry_rejects_unknown_kinds() {
    // `status_flags` skips unknown porcelain kinds via the `if let`
    // guard; the pure `None` contract pins here directly.
    assert!(flags_for_entry("# branch.oid abc123").is_none());
    assert!(flags_for_entry("X XY subm mH mI mW hH hI path").is_none());
    assert!(flags_for_entry("").is_none());
}

#[test]
fn hg_dispatch_revision_with_empty_conduit_reports_not_found() {
    // Sibling of the marker/ambiguity paths above: a revision symbol
    // with no markers DOES invoke the shared empty stub (via
    // `load_revision`), which answers no rows.
    let (tmp, hg_cmd) = hg_test_repo("hg-empty-rev");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let call = empty_conduit_data;
    let err = hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "D123", None, &call).unwrap_err();
    assert_eq!(
        err,
        "No revision \"D123\" exists, or you do not have permission to view it."
    );
}

#[test]
fn hg_dispatch_task_with_empty_conduit_reports_not_found() {
    // Sibling on the task path: `work_on_task` invokes the same shared
    // empty stub (via `maniphest.search`), which answers no rows.
    let (tmp, hg_cmd) = hg_test_repo("hg-empty-task");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let call = empty_conduit_data;
    let err = hg_dispatch_symbol_with(tmp.path(), &hg_cmd, "T7", None, &call).unwrap_err();
    assert_eq!(
        err,
        "No task \"T7\" exists, or you do not have permission to view it."
    );
}

/// Loopback stub routing each conduit method to its canned envelope.
fn serve_routes(routes: std::collections::HashMap<String, String>, expected: usize) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head_text = String::from_utf8_lossy(&head).to_string();
            let len: usize = head_text
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(key, value)| {
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let method = head_text
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .strip_prefix("/api/")
                .unwrap_or("")
                .to_string();
            let canned = routes
                .get(&method)
                .cloned()
                .unwrap_or_else(|| r#"{"result": null, "error_code": "ERR-NO-ROUTE"}"#.to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    base
}

fn live_envelope(result: &str) -> String {
    format!(r#"{{"result": {result}, "error_code": null}}"#)
}

/// The arcrc host key for a stub base, mirroring
/// `config::host_key` (`{scheme}://{authority}/api/`).
fn host_key_for(base: &str) -> String {
    let after_scheme = base.split("://").nth(1).unwrap_or(base);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let scheme = base.split("://").next().unwrap_or("https");
    format!("{scheme}://{authority}/api/")
}

/// Fake arcrc granting a token for the stub `base`.
fn fake_arcrc(dir: &Path, base: &str) -> PathBuf {
    let path = dir.join("fake.arcrc");
    let key = host_key_for(base);
    std::fs::write(
        &path,
        format!(r#"{{"hosts": {{"{key}": {{"token": "x"}}}}}}"#),
    )
    .unwrap();
    path
}

#[test]
fn live_client_resumes_revision_branch() {
    // Git live-conduit path: no local `123` branch, so the seamed
    // client dials the loopback stub, associates the tip by hash, and
    // checks the branch out. The bare `123` resolves to ID 123
    // (`D123` would cast to 0, like PHP).
    let tmp = init_repo("live-git");
    commit_file(tmp.path(), "a.txt", "one");
    let branch = current_branch(tmp.path());
    let commit = git(tmp.path(), &["rev-parse", "HEAD"]);
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.revision.search".to_string(),
        live_envelope(
            r#"{"data": [{"id": 123, "phid": "PHID-DREV-1", "fields": {"title": "Fix it"}}]}"#,
        ),
    );
    routes.insert(
        "differential.query".to_string(),
        live_envelope(&format!(
            r#"[{{"phid": "PHID-DREV-1", "hashes": [["gtcm", "{commit}"]]}}]"#
        )),
    );
    let base = serve_routes(routes, 2);
    // Credentials live outside the repo so the checkout stays clean.
    let creds = TempDir::new("live-git-creds");
    let arcrc = fake_arcrc(creds.path(), &base);
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with_user_file(
            tmp.path(),
            &args(&["--", "123"]),
            None,
            &mut input,
            Some(&arcrc),
            Some(&base),
        ),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), branch);
}

#[test]
fn live_client_resumes_hg_marker() {
    // Hg live-conduit path: no `123` marker, so the seamed client
    // dials the loopback stub, associates the bookmark by `hgcm` hash,
    // and checks it out. The bare `123` resolves to ID 123 (`D123`
    // would cast to 0, like PHP).
    let (tmp, hg_cmd) = hg_test_repo("live-hg");
    std::fs::write(tmp.path().join(".fakehg-bookmarks"), "  feat  3:abc\n").unwrap();
    std::fs::write(tmp.path().join(".fakehg-branches"), "").unwrap();
    let node = "0123456789abcdef0123456789abcdef01234567";
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.revision.search".to_string(),
        live_envelope(
            r#"{"data": [{"id": 123, "phid": "PHID-DREV-1", "fields": {"title": "Fix it"}}]}"#,
        ),
    );
    routes.insert(
        "differential.query".to_string(),
        live_envelope(&format!(
            r#"[{{"phid": "PHID-DREV-1", "hashes": [["hgcm", "{node}"]]}}]"#
        )),
    );
    let base = serve_routes(routes, 2);
    let arcrc = fake_arcrc(tmp.path(), &base);
    let options = hg_options("123");
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_hg_in_with_cmd_user_file(
            tmp.path(),
            &hg_cmd,
            &options,
            None,
            &mut input,
            Some(&arcrc),
            Some(&base),
        ),
        std::process::ExitCode::SUCCESS
    );
    let log = hg_read_log(tmp.path());
    assert!(log.contains("checkout feat --"), "{log}");
}
