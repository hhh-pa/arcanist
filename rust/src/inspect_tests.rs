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
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_args_with_tty(&args(&[]), true),
        ParseOutcome::Args(Options {
            explore: false,
            objects: Vec::new(),
        })
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--explore", "--", "working-copy(HEAD)"]), true),
        ParseOutcome::Args(Options {
            explore: true,
            objects: vec!["working-copy(HEAD)".to_string()],
        })
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--help"]), true),
        ParseOutcome::Help
    );
    assert_eq!(
        parse_args_with_tty(&args(&["--bogus"]), true),
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
    // The cut counts bytes, not characters: 40 two-byte characters keep 32.
    assert_eq!(
        render_value(&"é".repeat(40)),
        format!("\"{}\"", "é".repeat(32))
    );
    // A character split by the cut renders as U+FFFD.
    assert_eq!(
        render_value(&format!("{}é", "x".repeat(63))),
        format!("\"{}\u{fffd}\"", "x".repeat(63))
    );
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

    // T1153 RB-D14: without `--explore` the raw argument renders
    // unresolved; exploration resolves and loads the message.
    let block = inspect_one(tmp.path(), "working-copy(HEAD)", false).unwrap();
    assert!(block.contains("Commit \"HEAD\""), "{block}");
    let explored = inspect_one(tmp.path(), "working-copy(HEAD)", true).unwrap();
    assert!(
        explored.contains(&format!("Commit \"{head}\"")),
        "{explored}"
    );
    assert!(explored.contains("first subject"));

    assert!(inspect_one(tmp.path(), "working-copy", false).is_err());
    assert!(inspect_one(tmp.path(), "browse", false).is_err());
    assert!(inspect_one(tmp.path(), "working-copy(a,b)", false).is_err());
    assert!(inspect_one(tmp.path(), "browse(a,b)", false).is_err());

    let block = inspect_one(tmp.path(), "browse(HEAD)", false).unwrap();
    assert!(
        block.contains("+ [ArcanistBrowseRef] Browse Query \"HEAD\""),
        "{block}"
    );
    assert!(
        block.contains(" o [ArcanistVectorHardpoint] commitRefs"),
        "{block}"
    );
    assert!(
        block.contains(" o [ArcanistVectorHardpoint] uris"),
        "{block}"
    );

    // T1153 RB-D13: `commit(...)` is a known type now.
    let block = inspect_one(tmp.path(), "commit(HEAD)", false).unwrap();
    assert!(
        block.contains("+ [ArcanistCommitSymbolRef] Commit Symbol \"HEAD\""),
        "{block}"
    );
    let explored = inspect_one(tmp.path(), "commit(HEAD)", true).unwrap();
    assert!(
        explored.contains(&format!("Commit \"{head}\"")),
        "{explored}"
    );
    assert!(explored.contains("first subject"), "{explored}");

    let unknown = inspect_one(tmp.path(), "frobnicate(HEAD)", false).unwrap_err();
    assert!(unknown.contains("Unknown object type \"frobnicate\""));
    for name in [
        "browse",
        "working-copy",
        "commit",
        "user",
        "remote",
        "revision",
        "task",
    ] {
        assert!(unknown.contains(name), "{unknown}");
    }
    assert!(inspect_one(tmp.path(), "not valid!", false).is_err());
}

#[test]
fn rbd14_working_copy_needs_no_repo_without_explore() {
    let dir = std::env::temp_dir().join(format!("arc-rs-rbd14-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let block = inspect_one(&dir, "working-copy(HEAD)", false).unwrap();
    assert!(block.contains("Working Copy State"), "{block}");
    assert!(block.contains("Commit \"HEAD\""), "{block}");
    assert!(inspect_one(&dir, "working-copy(HEAD)", true).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rbd13_symbols_users_and_remotes_render() {
    let tmp = init_repo("symbols");
    git(
        tmp.path(),
        &["remote", "add", "origin", "https://example.test/r.git"],
    );
    // Simple symbols resolve eagerly (IDs, monograms with PHP's
    // `(int)` quirk, and PHIDs); anything else is a usage error.
    let block = inspect_one(tmp.path(), "revision(D123)", false).unwrap();
    assert!(
        block.contains("+ [ArcanistRevisionSymbolRef] Revision Symbol \"0\""),
        "{block}"
    );
    assert!(
        block.contains(" o [ArcanistScalarHardpoint] ref.symbol.object"),
        "{block}"
    );
    let block = inspect_one(tmp.path(), "revision(123)", false).unwrap();
    assert!(block.contains("Revision Symbol \"123\""), "{block}");
    let block = inspect_one(tmp.path(), "task(PHID-TASK-abc)", false).unwrap();
    assert!(block.contains("Task Symbol \"PHID-TASK-abc\""), "{block}");
    let err = inspect_one(tmp.path(), "revision(abc)", false).unwrap_err();
    assert!(
        err.contains("The format of symbol \"abc\" is unrecognized"),
        "{err}"
    );
    assert!(is_toolset_usage_message(&err));
    let err = inspect_one(tmp.path(), "revision(1,2)", false).unwrap_err();
    assert!(err.contains("Expected exactly one argument"), "{err}");

    let block = inspect_one(tmp.path(), "user(@alice)", false).unwrap();
    assert!(block.contains("User Symbol \"alice\""), "{block}");
    let block = inspect_one(tmp.path(), "user(viewer())", false).unwrap();
    assert!(block.contains("User Symbol \"viewer()\""), "{block}");
    let err = inspect_one(tmp.path(), "user(a b)", false).unwrap_err();
    assert!(err.contains("The format of user symbol"), "{err}");
    assert!(is_toolset_usage_message(&err));

    let block = inspect_one(tmp.path(), "remote(origin)", false).unwrap();
    assert!(
        block.contains("+ [ArcanistRemoteRef] Remote \"origin\""),
        "{block}"
    );
    assert!(
        block.contains(" o [ArcanistObjectListHardpoint] arc.remote.repositoryRefs"),
        "{block}"
    );
    let err = inspect_one(tmp.path(), "remote(missing)", false).unwrap_err();
    assert!(
        err.contains("This working copy has no remote named \"missing\""),
        "{err}"
    );
    assert!(is_toolset_usage_message(&err));
}

#[test]
fn rbd13_remote_outside_repo_is_not_implemented() {
    let dir = std::env::temp_dir().join(format!("arc-rs-rbd13-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let err = inspect_one(&dir, "remote(origin)", false).unwrap_err();
    assert_eq!(
        err,
        "Method newRemoteRefQueryTemplate in class ArcanistFilesystemAPI is not implemented!"
    );
    assert!(!is_toolset_usage_message(&err));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rbd15_commit_explore_resolves_and_loads() {
    let tmp = init_repo("commitsym");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    git(tmp.path(), &["tag", "-a", "-m", "annotated", "v1"]);

    // Unknown and non-commit symbols attach null, rendering `*`
    // with an empty value — quietly, like PHP.
    for symbol in ["missing", "v1"] {
        let block = inspect_one(tmp.path(), &format!("commit({symbol})"), true).unwrap();
        assert!(
            block.contains(" * [ArcanistScalarHardpoint] ref.symbol.object"),
            "{block}"
        );
        assert!(block.contains("  > \"\""), "{block}");
    }
    // Newlines are fatal, with the c-escaped symbol.
    let err = inspect_one(tmp.path(), "commit(a\nb)", true).unwrap_err();
    assert!(
        err.contains("Commit symbol \"a\\nb\" contains a newline"),
        "{err}"
    );
    assert!(!is_toolset_usage_message(&err));
}

#[test]
fn run_in_lists_types_and_joins_blocks() {
    let tmp = init_repo("run");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);

    // T1153 B21/B23: piped runs carry `--`; `--bogus` stays bare to
    // lock that scan errors report before the terminator check.
    assert_eq!(
        run_in(tmp.path(), &args(&["--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--", "working-copy(HEAD)"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(
            tmp.path(),
            &args(&[
                "--explore",
                "--",
                "working-copy(HEAD)",
                "working-copy(HEAD)"
            ])
        ),
        std::process::ExitCode::SUCCESS
    );
    // T1153 RB-D13: `commit(...)` renders now.
    assert_eq!(
        run_in(tmp.path(), &args(&["--", "commit(HEAD)"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &args(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    // T1153 B21/B23: piped runs carry `--`.
    assert_eq!(
        run(&args(&["--help", "--"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn explores_browse_tokens_through_the_engine() {
    use std::collections::HashMap;
    let tmp = init_repo("browsex");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);

    let mut routes = HashMap::new();
    routes.insert("repository.query".to_string(), serde_json::json!([]));
    routes.insert(
        "phid.lookup".to_string(),
        serde_json::json!({"nope": {"uri": "https://phab.example/nope"}}),
    );
    routes.insert("differential.query".to_string(), serde_json::json!([]));
    let call = move |method: &str, _: &serde_json::Value| {
        routes
            .get(method)
            .cloned()
            .ok_or_else(|| format!("unexpected call {method}"))
    };
    // HEAD resolves to a commit but no URIs (no repo configured, the
    // object lookup misses, the revision query is empty).
    let mut ctx = browse::BrowseCtx::new(tmp.path(), &call, "");
    let block = inspect_one_with(tmp.path(), "browse(HEAD)", true, Some(&mut ctx)).unwrap();
    assert!(
        block.contains("+ [ArcanistBrowseRef] Browse Query \"HEAD\""),
        "{block}"
    );
    assert!(
        block.contains(" * [ArcanistVectorHardpoint] commitRefs"),
        "{block}"
    );
    assert!(block.contains(&format!("Commit \"{head}\"")), "{block}");
    assert!(block.contains("first subject"), "{block}");
    assert!(
        block.contains(" * [ArcanistVectorHardpoint] uris"),
        "{block}"
    );

    // End to end through run_in_with with an injected caller.
    // T1153 B21/B23: piped runs carry `--`.
    let mut routes = HashMap::new();
    routes.insert("repository.query".to_string(), serde_json::json!([]));
    routes.insert("phid.lookup".to_string(), serde_json::json!({}));
    routes.insert("differential.query".to_string(), serde_json::json!([]));
    let call = move |method: &str, _: &serde_json::Value| {
        routes
            .get(method)
            .cloned()
            .ok_or_else(|| format!("unexpected call {method}"))
    };
    assert_eq!(
        run_in_with(
            tmp.path(),
            &args(&["--explore", "--", "browse(HEAD)"]),
            Some(&call)
        ),
        std::process::ExitCode::SUCCESS
    );
    // Without --explore, no Conduit is needed at all.
    assert_eq!(
        run_in_with(tmp.path(), &args(&["--", "browse(HEAD)"]), None),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn ref_trees_carry_no_markup() {
    let info = CommitInfo {
        hash: "abc123".to_string(),
        message: "hello".to_string(),
    };
    for explore in [false, true] {
        let out = render_working_copy(&info, explore);
        assert!(!out.contains('\x1b'), "{out}");
        assert!(!out.contains("**"), "{out}");
    }
    let tmp = TempDir::new("nomarkup");
    let out = render_browse(tmp.path(), "branch", false, None).unwrap();
    assert!(!out.contains('\x1b'), "{out}");
    assert!(!out.contains("**"), "{out}");
}

#[test]
fn r4_rbd4_spec_errors_are_toolset_usage() {
    // Malformed specs and unknown types are toolset usage (exit 1);
    // bad commits escape into a fatal (255).
    assert!(is_toolset_usage_message(
        "Object specification \"x(y\" is unknown, expected a specification like \"commit(HEAD)\"."
    ));
    assert!(is_toolset_usage_message(
        "Unknown object type \"zzz\", supported types are: browse, working-copy."
    ));
    assert!(is_toolset_usage_message(
        "Expected exactly one argument to \"working-copy(...)\" with a commit hash."
    ));
    assert!(!is_toolset_usage_message("unknown commit \"zzz\"."));
    // T1153 B21/B23: piped runs carry `--`.
    let tmp = TempDir::new("r4-spec");
    assert_eq!(
        run_in(tmp.path(), &args(&["--", "not a spec!!"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn b21_inspect_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv. Unknown flags still report first.
    assert!(matches!(
        parse_args_with_tty(&args(&["working-copy(HEAD)"]), false),
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
        parse_args_with_tty(&args(&["--bogus"]), false),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["--", "working-copy(HEAD)"]), false),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&args(&["working-copy(HEAD)"]), true),
        ParseOutcome::Args(_)
    ));
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn b23_inspect_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help inspect`.
    let text = usage_text(false);
    assert!(text.starts_with("INSPECT WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Inspect internal object properties."),
        "{text}"
    );
    assert!(text.contains("--explore"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
