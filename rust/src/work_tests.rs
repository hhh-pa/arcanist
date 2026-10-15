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
    assert_eq!(canonical_revision(tmp.path(), "HEAD"), Some(head.clone()));
    assert_eq!(canonical_revision(tmp.path(), &head), Some(head.clone()));
    assert_eq!(canonical_revision(tmp.path(), "no-such-rev"), None);
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
    // Unknown revision/task symbols fail through the mocked search (no
    // network), and bare IDs never become branch names.
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["D123"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["T1"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["123"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    // None of the failures created a branch.
    assert!(!branch_exists(tmp.path(), "feature"));
    assert!(!branch_exists(tmp.path(), "123"));
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
    let tmp = init_repo("stash");
    commit_file(tmp.path(), "f.txt", "one");
    std::fs::write(tmp.path().join("extra.txt"), "untracked\n").unwrap();
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["feature"]), None, &mut input),
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
    let tmp = init_repo("nostash");
    commit_file(tmp.path(), "f.txt", "one");
    std::fs::write(tmp.path().join("f.txt"), "dirty\n").unwrap();
    let mut input = std::io::Cursor::new(b"n\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["feature"]), None, &mut input),
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
    let tmp = init_repo("restore");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    std::fs::write(tmp.path().join("f.txt"), "dirty\n").unwrap();
    // "HEAD" is not a valid new branch name, so creation fails after the
    // stash; the failure must restore the branch and the stashed change.
    let mut input = std::io::Cursor::new(b"y\n");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["HEAD"]), None, &mut input),
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
    let tmp = init_repo("tasks");
    commit_file(tmp.path(), "f.txt", "one");
    let empty = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"data": []}))
    };
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["T1"]), Some(&empty), &mut input),
        std::process::ExitCode::FAILURE
    );
    // Monograms resolve to ID 0 upstream, so they never match, even when
    // the object exists; bare IDs hit the upstream TODO instead.
    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["T7"]), Some(&call), &mut input),
        std::process::ExitCode::FAILURE
    );
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["7"]), Some(&call), &mut input),
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
    let tmp = init_repo("revwork");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    git(tmp.path(), &["checkout", "-q", "-b", "feature"]);
    commit_with_footer(tmp.path(), "g.txt", "D123");
    git(tmp.path(), &["checkout", "-q", &first]);

    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["123"]), Some(&call), &mut input),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), "feature");
}

#[test]
fn unassociated_revisions_fail_without_switching() {
    let tmp = init_repo("revnone");
    commit_file(tmp.path(), "f.txt", "one");
    let first = current_branch(tmp.path());
    let call = search_mock();
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_in_with(tmp.path(), &args(&["123"]), Some(&call), &mut input),
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
        run_in_with(tmp.path(), &args(&["123"]), Some(&call), &mut input),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(current_branch(tmp.path()), "newer");
}

#[test]
fn monogram_symbols_never_match_upstream() {
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
        run_in_with(tmp.path(), &args(&["D123"]), Some(&call), &mut input),
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
