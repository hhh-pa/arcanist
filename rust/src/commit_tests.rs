use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-commit-tests-{tag}-{}-{n}",
            std::process::id()
        ));
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

#[cfg(unix)]
fn write_fake_svn(dir: &Path, stdout: &str) -> String {
    let script = dir.join("svn");
    std::fs::write(&script, format!("#!/bin/sh\nprintf '%s' '{stdout}'\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).unwrap();
    script.to_string_lossy().to_string()
}

fn empty_options() -> Options {
    Options {
        messages: Vec::new(),
        all: false,
        allow_empty: false,
        show: false,
        revision: None,
        conduit_uri: None,
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(parse_options(&args(&[])), Ok(Some(empty_options())));
}

#[test]
fn parses_message_forms() {
    let mut expected = empty_options();
    expected.messages = vec!["one".to_string()];
    assert_eq!(
        parse_options(&args(&["-m", "one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options(&args(&["--message", "one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options(&args(&["--message=one"])),
        Ok(Some(expected.clone()))
    );
    assert_eq!(parse_options(&args(&["-mone"])), Ok(Some(expected)));
}

#[test]
fn messages_are_repeatable() {
    let options = parse_options(&args(&["-m", "one", "-m", "two"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.messages, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn parses_flags() {
    // T1153 CORE-64: git support plus `-m`/`--all`/`--allow-empty`
    // are pure extensions (PHP commits svn only).
    let options = parse_options(&args(&["--all", "--allow-empty", "--show"]))
        .unwrap()
        .unwrap();
    assert!(options.all && options.allow_empty && options.show);
    let options = parse_options(&args(&["-a"])).unwrap().unwrap();
    assert!(options.all);
}

#[test]
fn parses_revision_and_conduit_uri() {
    let options = parse_options(&args(&["--revision", "D123"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.revision, Some("D123".to_string()));
    let options = parse_options(&args(&["--revision=123"])).unwrap().unwrap();
    assert_eq!(options.revision, Some("123".to_string()));
    let options = parse_options(&args(&["--conduit-uri", "http://x/"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
    let options = parse_options(&args(&["--conduit-uri=http://x/"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
}

#[test]
fn help_returns_none() {
    assert_eq!(parse_options(&args(&["--help"])), Ok(None));
    assert_eq!(parse_options(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options(&args(&["file.txt"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options(&args(&["--"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(parse_options(&args(&["-m"])).is_err());
    assert!(parse_options(&args(&["--revision"])).is_err());
    assert!(parse_options(&args(&["--revision="])).is_err());
    assert!(parse_options(&args(&["--conduit-uri"])).is_err());
    assert!(parse_options(&args(&["--message="])).is_err());
}

#[test]
fn parses_revision_ids() {
    assert_eq!(parse_revision_id("123"), Ok(123));
    assert_eq!(parse_revision_id("D123"), Ok(123));
    assert_eq!(parse_revision_id("d7"), Ok(7));
    for bad in ["", "D", "D0", "0", "abc", "D12x", "12.5", "-3", "D 1"] {
        assert!(parse_revision_id(bad).is_err(), "accepted: {bad:?}");
    }
}

#[test]
fn normalizes_messages() {
    assert_eq!(normalize_message("hello"), Ok("hello\n".to_string()));
    assert_eq!(
        normalize_message("  hello  \n\n"),
        Ok("hello\n".to_string())
    );
    assert!(normalize_message("").is_err());
    assert!(normalize_message("   \n\t\n").is_err());
}

#[test]
fn usage_covers_options_and_limits() {
    // T1153 B21/B23: `--help` renders the generated workflow help
    // (extensions stay undocumented).
    let text = usage_text(false);
    assert!(text.contains("COMMIT WORKFLOW"), "{text}");
    assert!(
        text.contains("--revision") && text.contains("--show"),
        "{text}"
    );
    assert!(text.contains("Supports: svn"), "{text}");
    assert!(!text.contains("--conduit-uri"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-commit-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn rejects_git_only_options_under_svn() {
    let mut options = empty_options();
    options.messages = vec!["x".to_string()];
    assert!(reject_git_only_options(&options).is_err());
    let mut options = empty_options();
    options.all = true;
    assert!(reject_git_only_options(&options).is_err());
    let mut options = empty_options();
    options.allow_empty = true;
    assert!(reject_git_only_options(&options).is_err());
    assert!(reject_git_only_options(&empty_options()).is_ok());
}

#[test]
fn parent_dirs_stop_at_top() {
    assert_eq!(parent_dir("a/b/c.txt"), "a/b");
    assert_eq!(parent_dir("a/b"), "a");
    assert_eq!(parent_dir("a"), ".");
    assert_eq!(parent_dir("."), ".");
    assert_eq!(parent_dir("a/b/"), "a");
}

#[test]
fn expands_commit_dirs() {
    let commit = expand_commit_dirs(&["a/b/c.txt".to_string(), "top.txt".to_string()]);
    let sorted: Vec<&str> = commit.iter().map(String::as_str).collect();
    assert_eq!(sorted, vec!["a", "a/b", "a/b/c.txt", "top.txt"]);
    assert!(expand_commit_dirs(&[]).is_empty());
}

#[test]
fn matches_source_paths() {
    let dir = scratch("source");
    let raw = dir.to_string_lossy().into_owned();
    assert!(root_matches_source(&dir, &raw));
    assert!(!root_matches_source(&dir, "/elsewhere/entirely"));
    assert!(!root_matches_source(&dir, ""));
    let canonical = std::fs::canonicalize(&dir)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    assert!(root_matches_source(&dir, canonical));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn descendant_checks_need_dirs_on_disk() {
    let dir = scratch("descendant");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), "x").unwrap();
    std::fs::write(dir.join("file.txt"), "x").unwrap();
    assert!(is_path_descendant(&dir, "sub/inner.txt", "sub"));
    assert!(!is_path_descendant(&dir, "sub", "sub"));
    assert!(!is_path_descendant(&dir, "file.txt", "sub"));
    assert!(!is_path_descendant(&dir, "sub/inner.txt", "file.txt"));
    assert!(!is_path_descendant(&dir, "sub/missing.txt", "sub"));
    assert!(!is_path_descendant(&dir, "nope/x.txt", "nope"));
    assert!(is_path_descendant(&dir, "sub/inner.txt", "."));
    assert!(!is_path_descendant(&dir, ".", "."));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn partitions_uncommitted_status() {
    let dir = scratch("partition");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), "x").unwrap();
    std::fs::write(dir.join("other.txt"), "x").unwrap();
    let mut status = svn::StatusMap::new();
    status.insert("sub/inner.txt".to_string(), svn::FLAG_MODIFIED);
    status.insert("other.txt".to_string(), svn::FLAG_MODIFIED);
    status.insert("sub/keep.txt".to_string(), svn::FLAG_MODIFIED);

    // Everything committed: nothing skipped.
    let mut commit = expand_commit_dirs(&[
        "sub/inner.txt".to_string(),
        "other.txt".to_string(),
        "sub/keep.txt".to_string(),
    ]);
    assert!(
        partition_uncommitted(&dir, &status, &commit)
            .unwrap()
            .is_empty()
    );

    // One path outside the set: skipped, and the set keeps its dirs.
    commit.remove("other.txt");
    let skipped = partition_uncommitted(&dir, &status, &commit).unwrap();
    assert_eq!(skipped, vec!["other.txt".to_string()]);
    assert!(commit.contains("sub"));

    // A modified path under a committed directory throws.
    std::fs::write(dir.join("sub").join("keep.txt"), "x").unwrap();
    commit.remove("sub/keep.txt");
    let err = partition_uncommitted(&dir, &status, &commit).unwrap_err();
    assert!(
        err.contains("NOT included in the commit"),
        "unexpected: {err}"
    );
    assert!(err.contains("'sub/keep.txt'"), "unexpected: {err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn drops_missing_paths_but_keeps_deletions() {
    let dir = scratch("missing");
    std::fs::write(dir.join("kept.txt"), "x").unwrap();
    let mut status = svn::StatusMap::new();
    status.insert("gone.txt".to_string(), svn::FLAG_DELETED);
    let mut commit = BTreeSet::from([
        "kept.txt".to_string(),
        "gone.txt".to_string(),
        "vanished.txt".to_string(),
    ]);
    let missing = remove_missing_paths(&dir, &status, &mut commit);
    assert_eq!(missing, vec!["vanished.txt".to_string()]);
    assert!(commit.contains("kept.txt"));
    assert!(commit.contains("gone.txt"));
    assert!(!commit.contains("vanished.txt"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn file_warnings_confirm_or_abort() {
    let mut io = land::Io::canned(&["y"]);
    prompt_file_warning(&mut io, "1 path(s):", "Proceed?", &["a.txt".to_string()]).unwrap();
    assert!(io.out.stdout.contains("    a.txt\n"), "{}", io.out.stdout);

    let mut io = land::Io::canned(&["n"]);
    let err =
        prompt_file_warning(&mut io, "1 path(s):", "Proceed?", &["a.txt".to_string()]).unwrap_err();
    assert_eq!(err, land::ABORT);

    let mut io = land::Io::canned(&["y", "n"]);
    let err = confirm_all(&mut io, &["One?".to_string(), "Two?".to_string()]).unwrap_err();
    assert_eq!(err, land::ABORT);
    let mut io = land::Io::canned(&["y", "y"]);
    confirm_all(&mut io, &["One?".to_string(), "Two?".to_string()]).unwrap();
}

#[test]
fn message_files_are_unique_under_concurrency() {
    // Regression: pid+nanos names collided when parallel tests wrote
    // within one clock tick (macOS vcs1_commit flake). A barrier forces
    // all threads to write simultaneously.
    const N: usize = 32;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let message = format!("message {i}");
                let path = write_message_file("test-race", &message).unwrap();
                (path, message)
            })
        })
        .collect();
    let mut paths = Vec::with_capacity(N);
    for handle in handles {
        let (path, message) = handle.join().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), message);
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), N);
    for path in &paths {
        std::fs::remove_file(path).ok();
    }
}

/// Serve conduit routes, recording `(method, body)` pairs.
fn serve_recording(
    routes: std::collections::HashMap<String, String>,
    expected: usize,
) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    use std::io::{Read, Write};
    let (send, recv) = std::sync::mpsc::channel();
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
            send.send((method.clone(), String::from_utf8_lossy(&body).to_string()))
                .ok();
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
    (base, recv)
}

#[test]
fn commit_c3_looksoon_nudges_repository_and_swallows_failures() {
    // T1153 COMMIT-C3: after resolving the repository, `commit` sends
    // the advisory `diffusion.looksoon`; a failing nudge is swallowed.
    let dir = std::env::temp_dir().join(format!("arc-rs-commit-c3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".arcconfig"), r#"{"repository.callsign": "X"}"#).unwrap();
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        r#"{"result": [{"phid": "PHID-REPO-1"}], "error_code": null, "error_info": null}"#
            .to_string(),
    );
    routes.insert(
        "diffusion.looksoon".to_string(),
        r#"{"result": null, "error_code": "ERR-CONDUIT-CALL", "error_info": "nope"}"#.to_string(),
    );
    let (base, seen) = serve_recording(routes, 2);
    let client = Client::new(&base, "t").unwrap();
    ask_for_repository_update(&client, &dir);
    // Bounded receives: a wiring regression must fail, never hang the suite.
    let wait = std::time::Duration::from_secs(30);
    let mut calls = [
        seen.recv_timeout(wait).unwrap(),
        seen.recv_timeout(wait).unwrap(),
    ];
    calls.sort();
    assert_eq!(calls[0].0, "diffusion.looksoon");
    // The nudge names the resolved repository PHID.
    assert!(calls[0].1.contains("PHID-REPO-1"), "{}", calls[0].1);
    assert_eq!(calls[1].0, "repository.query");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn commit_c3_looksoon_skips_untracked_working_copies() {
    // T1153 COMMIT-C3: with no identifiable repository, no call runs.
    let dir = std::env::temp_dir().join(format!("arc-rs-commit-c3-bare-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let client = Client::new("https://conduit.test/", "t").unwrap();
    // No identifiable repository (and no network): silently skipped.
    ask_for_repository_update(&client, &dir);
    std::fs::remove_dir_all(&dir).ok();
}
#[test]
fn r10_commit_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "COMMIT WORKFLOW\n\
         \n\
         commit\n\
         \x20     commit [--revision revision_id] [--show]\n\
         \n\
         \x20         Supports: svn\n\
         \x20         Commit a revision which has been accepted by a reviewer.\n\
         \n\
         \x20     --revision revision_id\n\
         \x20         Commit a specific revision. If you do not specify a revision, arc\n\
         \x20         will look for committable revisions.\n\
         \n\
         \x20     --show\n\
         \x20         Show the command which would be issued, but do not actually commit\n\
         \x20         anything.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("COMMIT WORKFLOW"), "{}", usage());
}

#[test]
fn b21_commit_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `commit`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_options(&args(&["--show"])).unwrap().is_some());
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    let text = usage_text(false);
    assert!(text.contains("COMMIT WORKFLOW"), "{text}");
}

#[test]
fn b23_commit_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help commit`.
    let text = usage_text(false);
    assert!(text.starts_with("COMMIT WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Commit a revision which has been accepted"),
        "{text}"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn conduit_uri_rejects_empty_inline_values() {
    assert_eq!(
        parse_options(&args(&["--conduit-uri="])).unwrap_err(),
        "--conduit-uri requires a non-empty value"
    );
}

#[test]
fn commit_message_rejects_non_string_results() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.getcommitmessage".to_string(),
        r#"{"result": 42, "error_code": null}"#.to_string(),
    );
    let (base, _rx) = serve_recording(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        fetch_commit_message(&client, 7).unwrap_err(),
        "conduit differential.getcommitmessage returned a non-string result"
    );
}

#[test]
fn message_file_allocation_fails_when_exhausted() {
    // Pre-create a wide run of colliding names: every attempt hits
    // `AlreadyExists` until the loop gives up.
    let start = MESSAGE_COUNTER.load(std::sync::atomic::Ordering::Relaxed);
    let mut created = Vec::new();
    for n in start..start + 1000 {
        let path = std::env::temp_dir().join(format!(
            "arc-rs-cov5e-exhaust-{}-{n}.msg",
            std::process::id()
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        created.push(path);
    }
    assert_eq!(
        write_message_file("cov5e-exhaust", "x").unwrap_err(),
        "failed to allocate a unique commit message file"
    );
    for path in &created {
        std::fs::remove_file(path).ok();
    }
}

#[test]
fn message_file_reports_uncreatable_paths() {
    let err = write_message_file("cov5e-no-such-dir-xyz/msg", "x").unwrap_err();
    assert!(
        err.starts_with("failed to write commit message file: "),
        "{err}"
    );
}

#[test]
fn root_match_rejects_unresolvable_roots() {
    assert!(!root_matches_source(
        std::path::Path::new("/definitely/missing/root-xyz"),
        "other"
    ));
}

#[test]
fn guessed_revision_propagates_query_failures() {
    let tmp = TempDir::new("guess-fail");
    let client = Client::new("http://127.0.0.1:9/", "t").unwrap();
    assert!(
        !svn_guessed_revision(&client, "PHID-USER-1", tmp.path())
            .unwrap_err()
            .is_empty()
    );
}

#[test]
fn guessed_revision_skips_foreign_source_paths() {
    let tmp = TempDir::new("guess-foreign");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        r#"{"result": [{"id": 1, "sourcePath": "/foreign/path"}], "error_code": null}"#.to_string(),
    );
    let (base, _rx) = serve_recording(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    let err = svn_guessed_revision(&client, "PHID-USER-1", tmp.path()).unwrap_err();
    assert!(err.contains("Unable to identify the revision"), "{err}");
}

#[test]
#[cfg(unix)]
fn commit_file_list_aborts_on_missing_paths() {
    let tmp = TempDir::new("file-list-abort");
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.getcommitpaths".to_string(),
        r#"{"result": ["ghost.txt"], "error_code": null}"#.to_string(),
    );
    let (base, _rx) = serve_recording(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    let program = write_fake_svn(tmp.path(), r#"<?xml version="1.0"?><status></status>"#);
    let svn = svn::SvnCommand::custom(&program, &[]);
    let mut io = land::Io::canned(&["n"]);
    let err = svn_commit_file_list(&mut io, &client, &svn, tmp.path(), 7).unwrap_err();
    assert_eq!(err, land::ABORT);
}

#[test]
#[cfg(unix)]
fn t1170_flow_git_reports_silent_hook_failures() {
    // T1170 band J: mirror of amend's silent-hook test — a
    // `commit-msg` hook that fails without output surfaces our own
    // error instead of git prose. Unix-only: hooks need `chmod`.
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("t1170-silent-fail");
    let git = |argv: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(tmp.path())
            .args(argv)
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    git(&["init", "-q", "-b", "master"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path().join("file.txt"), "base\n").unwrap();
    git(&["add", "file.txt"]);
    let hook = tmp.path().join(".git").join("hooks").join("commit-msg");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).unwrap();
    let mut options = empty_options();
    options.messages = vec!["msg".to_string()];
    options.all = true;
    assert_eq!(
        flow_git(tmp.path(), &options).unwrap_err(),
        "git commit failed"
    );
}

/// A fixture-local fake `git` program running `body` (Unix-only: the
/// seam passes a script path, which needs a shebang plus `+x`).
#[cfg(unix)]
fn write_fake_git(dir: &Path, body: &str) -> PathBuf {
    let script = dir.join("git");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).unwrap();
    script
}

#[cfg(unix)]
#[test]
fn flow_git_empty_success_output_falls_back() {
    // White-box: real git always prints a summary on success, so the
    // empty-output fallback is reachable only behind a fake backend.
    let tmp = TempDir::new("git-empty");
    let fake = write_fake_git(tmp.path(), "exit 0");
    let mut options = empty_options();
    options.messages = vec!["msg".to_string()];
    assert_eq!(
        flow_git_with(tmp.path(), &options, Some(&fake)),
        Ok("committed.\n".to_string())
    );
}

#[cfg(unix)]
#[test]
fn flow_git_passes_through_nonempty_summaries() {
    // The seam preserves the success path: git's summary passes
    // through verbatim (plus the trailing newline).
    let tmp = TempDir::new("git-summary");
    let fake = write_fake_git(tmp.path(), "printf '%s\\n' '[master abc1234] msg'");
    let mut options = empty_options();
    options.messages = vec!["msg".to_string()];
    assert_eq!(
        flow_git_with(tmp.path(), &options, Some(&fake)),
        Ok("[master abc1234] msg\n".to_string())
    );
}
