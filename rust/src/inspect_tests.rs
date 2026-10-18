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
fn simple_symbols_resolve_without_prefixes() {
    let kind = symbol_type("build").unwrap();
    assert_eq!(
        resolve_simple_symbol(kind, "123").unwrap(),
        ResolvedSymbol {
            kind: "id",
            value: "123".to_string(),
        }
    );
}

#[test]
fn user_symbols_cover_functions_ids_and_phids() {
    // Bare calls with inner whitespace are functions.
    assert_eq!(resolve_user_symbol("viewer( )").unwrap().kind, "function");
    assert_eq!(resolve_user_symbol("viewer(\t)").unwrap().kind, "function");
    // A head with whitespace is not a call; the whole symbol then
    // fails username validation too.
    assert!(resolve_user_symbol("a b()").is_err());
    // An unclosed call falls back to a username.
    assert_eq!(
        resolve_user_symbol("viewer(").unwrap(),
        ResolvedSymbol {
            kind: "username",
            value: "viewer(".to_string(),
        }
    );
    // Decimal IDs and user PHIDs resolve like PHP.
    assert_eq!(
        resolve_user_symbol("123").unwrap(),
        ResolvedSymbol {
            kind: "id",
            value: "123".to_string(),
        }
    );
    assert_eq!(
        resolve_user_symbol("PHID-USER-abc").unwrap(),
        ResolvedSymbol {
            kind: "phid",
            value: "PHID-USER-abc".to_string(),
        }
    );
}

#[test]
fn renders_unexplored_commit_objects() {
    let out = render_commit_symbol(
        "s",
        CommitSymbolObject::Commit(CommitInfo {
            hash: "h".to_string(),
            message: "m".to_string(),
        }),
        false,
    );
    assert!(
        out.contains("   o [ArcanistScalarHardpoint] message"),
        "{out}"
    );
}

#[test]
fn hg_paths_parse_strictly() {
    assert_eq!(
        parse_hg_paths("default = https://x\nup = ssh://y").unwrap(),
        vec![
            ("default".to_string(), "https://x".to_string()),
            ("up".to_string(), "ssh://y".to_string()),
        ]
    );
    assert!(parse_hg_paths("garbage line").is_err());
    // A lone empty line matches nothing.
    assert!(parse_hg_paths("").is_err());
}

#[test]
fn commit_symbols_outside_repos_fail_cleanly() {
    let plain = TempDir::new("nonrepo");
    let err = resolve_commit_symbol(plain.path(), "HEAD").unwrap_err();
    assert!(err.contains("unexpected line"), "{err}");
}

#[test]
fn remote_lookup_covers_hg_and_svn_roots() {
    // Without an `hg` binary the hg lookup fails before parsing paths
    // (and a bare `.hg` dir fails it when one exists).
    let hg_dir = TempDir::new("hgroot");
    std::fs::create_dir_all(hg_dir.path().join(".hg")).unwrap();
    assert!(lookup_remote(hg_dir.path(), "default").is_err());
    // Subversion roots hit the not-implemented fatal.
    let svn_dir = TempDir::new("svnroot");
    std::fs::create_dir_all(svn_dir.path().join(".svn")).unwrap();
    assert_eq!(
        lookup_remote(svn_dir.path(), "trunk").unwrap_err(),
        "Method newRemoteRefQueryTemplate in class ArcanistSubversionAPI is not implemented!"
    );
}

#[test]
fn explores_browse_fallbacks_and_uris() {
    use std::collections::HashMap;
    let tmp = init_repo("browseblob");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    let blob = git(tmp.path(), &["rev-parse", "HEAD:f.txt"]);
    let mut lookup = serde_json::Map::new();
    lookup.insert(
        blob.clone(),
        serde_json::json!({"uri": "https://phab.test/blob"}),
    );
    let mut routes = HashMap::new();
    routes.insert("repository.query".to_string(), serde_json::json!([]));
    routes.insert("phid.lookup".to_string(), serde_json::Value::Object(lookup));
    routes.insert("differential.query".to_string(), serde_json::json!([]));
    let call = move |method: &str, _: &serde_json::Value| {
        routes
            .get(method)
            .cloned()
            .ok_or_else(|| format!("unexpected call {method}"))
    };
    let mut ctx = browse::BrowseCtx::new(tmp.path(), &call, "");
    let block = render_browse(tmp.path(), &blob, true, Some(&mut ctx)).unwrap();
    // The blob resolves to file content, not a commit, so the message
    // load falls back to empty...
    assert!(block.contains("Commit \"one\""), "{block}");
    assert!(block.contains("> \"\""), "{block}");
    // ...while the object lookup still contributes its URI.
    assert!(
        block.contains("Browse URI \"https://phab.test/blob\""),
        "{block}"
    );
}

#[test]
fn inspectors_reject_missing_arguments() {
    let tmp = TempDir::new("arity");
    for spec in ["commit", "user", "remote"] {
        let err = inspect_one(tmp.path(), spec, false).unwrap_err();
        assert!(
            err.contains("Expected exactly one argument"),
            "{spec}: {err}"
        );
    }
}

#[test]
fn conduit_errors_render_raw() {
    assert_eq!(
        conduit_error_text(conduit::Error::Transport("boom".to_string())),
        "boom"
    );
}

#[test]
fn run_in_reports_missing_terminator() {
    // Piped stdin is never a TTY under `cargo test`; bail out (rather
    // than parse differently) if that ever changes.
    if crate::prompt::stdin_is_terminal() {
        return;
    }
    let tmp = TempDir::new("terminator");
    assert_eq!(
        run_in(tmp.path(), &args(&["working-copy(HEAD)"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn browse_auth_failures_render_login_guidance() {
    let tmp = init_repo("authfail");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    let call = |method: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        assert_eq!(method, "differential.query");
        Err("ERR-INVALID-AUTH: bad credentials".to_string())
    };
    let mut ctx = browse::BrowseCtx::new(tmp.path(), &call, "");
    let client = conduit::Client::new("https://phab.test/", "tok").unwrap();
    let options = Options {
        explore: true,
        objects: vec!["browse(HEAD)".to_string()],
    };
    assert_eq!(
        finish_inspect_blocks(tmp.path(), &options, Some(&mut ctx), Some(&client)),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn bad_commits_escape_into_a_fatal() {
    let tmp = init_repo("fatal");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    assert_eq!(
        run_in(
            tmp.path(),
            &args(&["--explore", "--", "working-copy(no-such-rev-xyz)"])
        ),
        std::process::ExitCode::from(255)
    );
}

/// Serve `expected` requests from `routes` (method -> envelope
/// result JSON); returns the base URI. (Local replicate of the
/// `export_tests` stub.)
fn serve_inspect_routes(
    routes: std::collections::HashMap<String, String>,
    expected: usize,
) -> String {
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
            let result = routes.get(&method).cloned().unwrap_or("null".to_string());
            let canned = format!(r#"{{"result": {result}, "error_code": null}}"#);
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

#[test]
fn explore_browse_builds_a_tokenless_client_from_config() {
    // `--explore` over `browse(...)` without an injected caller builds
    // a tokenless client from the project `.arcconfig` and drives the
    // engine over live conduit calls (here a localhost stub).
    let tmp = init_repo("livebrowse");
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-m", "first subject"]);
    let mut routes = std::collections::HashMap::new();
    routes.insert("differential.query".to_string(), "[]".to_string());
    routes.insert("phid.lookup".to_string(), "{}".to_string());
    routes.insert("repository.query".to_string(), "[]".to_string());
    let base = serve_inspect_routes(routes, 2);
    std::fs::write(
        tmp.path().join(".arcconfig"),
        format!(r#"{{"phabricator.uri": "{base}"}}"#),
    )
    .unwrap();
    assert_eq!(
        run_in(tmp.path(), &args(&["--explore", "--", "browse(HEAD)"])),
        std::process::ExitCode::SUCCESS
    );
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
fn r10_inspect_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "INSPECT WORKFLOW\n\
         \n\
         inspect [options] -- object\n\
         Show internal object information.\n\
         \n\
         Inspect internal object properties.\n\
         \n\
         \x20     --explore\n\
         \x20         Load all object hardpoints.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("INSPECT WORKFLOW"), "{}", usage());
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

// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "failed:")]
fn git_helper_reports_failing_commands() {
    // A failing git invocation panics with the command and stderr.
    let tmp = TempDir::new("w6h-git-fail");
    git(tmp.path(), &["--bogus-flag-xyz"]);
}
