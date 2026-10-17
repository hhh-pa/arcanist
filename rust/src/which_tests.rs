use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-which-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
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

fn fresh_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    git(&tmp.path, &["config", "user.email", "t@t"]);
    git(&tmp.path, &["config", "user.name", "t"]);
    git(&tmp.path, &["config", "commit.gpgsign", "false"]);
    tmp
}

fn commit(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-qm", name]);
}

#[test]
fn conduit_resolution_reports_its_source() {
    let tmp = TempDir::new("conduit");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://cfg.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        resolve_conduit_in(
            &tmp.path,
            Some("https://f.example/"),
            Some("https://e.example/")
        ),
        (
            Some("https://f.example/".to_string()),
            "command line (--conduit-uri)"
        )
    );
    assert_eq!(
        resolve_conduit_in(&tmp.path, None, Some("https://e.example/")),
        (
            Some("https://e.example/".to_string()),
            "environment (ARC_CONDUIT_URI)"
        )
    );
    assert_eq!(
        resolve_conduit_in(&tmp.path, None, None),
        (
            Some("https://cfg.example/".to_string()),
            "project (.arcconfig)"
        )
    );
    // T1153 CORE-12: bare-dir discovery now honors ambient user/system config,
    // so `(None, "none")` is only observable on unconfigured machines;
    // that case moved to `tests/cfgsys_which.rs` under a temp HOME.
    let bare = TempDir::new("noconduit");
    let _ = resolve_conduit_in(&bare.path, None, None);
    assert_eq!(
        resolve_conduit_in(&tmp.path, Some("  "), None).1,
        "project (.arcconfig)"
    );
}

#[test]
fn token_check_is_lenient() {
    let tmp = TempDir::new("token");
    assert!(!token_installed(None, "https://h.example/"));
    assert!(!token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://h.example/"
    ));
    std::fs::write(tmp.path.join(".arcrc"), "broken").unwrap();
    assert!(!token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://h.example/"
    ));
    std::fs::write(
        tmp.path.join(".arcrc"),
        r#"{"hosts": {"https://h.example/api/": {"token": "t"}}}"#,
    )
    .unwrap();
    assert!(token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://h.example/"
    ));
    assert!(!token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://other.example/"
    ));
}

#[test]
fn project_and_callsign_discovery() {
    let tmp = TempDir::new("proj");
    let child = tmp.path.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    // T1153 CORE-33: discovery is working-copy-scoped, so the fixture
    // needs a VCS marker; a stray `.arcconfig` in a plain directory is
    // ignored like PHP ignores it.
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"repository.callsign": "X", "phabricator.uri": "https://h.example/"}"#,
    )
    .unwrap();
    let (root, file) = find_project(&child);
    assert_eq!(root.unwrap(), tmp.path);
    assert!(file.unwrap().ends_with(".arcconfig"));
    assert_eq!(find_project(&TempDir::new("noroot").path), (None, None));
    let stray = TempDir::new("stray");
    std::fs::write(stray.path.join(".arcconfig"), "{}").unwrap();
    assert_eq!(find_project(&stray.path), (None, None));

    // T1153 RB-D17: the callsign reads from any config source,
    // runtime first; project beats user here.
    std::fs::write(
        tmp.path.join(".arcrc"),
        r#"{"config": {"repository.callsign": "U"}}"#,
    )
    .unwrap();
    let merged = get_config::load_merged(
        &tmp.path,
        Some(&tmp.path.join(".arcrc")),
        &get_config::LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(repository_callsign(&merged).unwrap().as_deref(), Some("X"));
    std::fs::remove_file(tmp.path.join(".arcconfig")).unwrap();
    let merged = get_config::load_merged(
        &tmp.path,
        Some(&tmp.path.join(".arcrc")),
        &get_config::LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(repository_callsign(&merged).unwrap().as_deref(), Some("U"));
    assert_eq!(repository_callsign(&empty_loaded()).unwrap(), None);
}

fn never_prompt(_: &str) -> Option<String> {
    None
}

fn empty_loaded() -> get_config::MergedConfig {
    get_config::MergedConfig {
        sources: get_config::Sources {
            local: serde_json::Map::new(),
            project: serde_json::Map::new(),
            user: serde_json::Map::new(),
            system: serde_json::Map::new(),
            default: get_config::default_config(),
            project_file: None,
        },
        runtime: serde_json::Map::new(),
    }
}

fn base_inputs<'a>(
    runtime_rules: &'a str,
    prompt: &'a dyn Fn(&str) -> Option<String>,
) -> BaseInputs<'a> {
    BaseInputs {
        runtime_rules,
        prompt,
    }
}

#[test]
fn rbd39_query_rows_accept_lists_and_objects() {
    // Servers send lists, but `differential.query` rows also read
    // from the object shape (sorted by key), like PHP's `foreach`.
    let list = serde_json::json!([{"id": 1}, {"id": 2}]);
    assert_eq!(query_rows(&list).len(), 2);
    let map = serde_json::json!({"1": {"id": 2}, "0": {"id": 1}});
    let rows = query_rows(&map);
    assert_eq!(rows.len(), 2);
    assert_eq!(row_string(rows[0], "id"), "1");
    assert_eq!(query_rows(&serde_json::json!(null)).len(), 0);
}

#[test]
fn rbd38a_commit_line_in_body_does_not_split() {
    // T1153 RB-D38a (kept): PHP scans `--format=medium` with the diff
    // parser, so a body line `commit <40hex>` splits one commit into
    // two; the `%x01/%x02` framing keeps it inside the message (PHP
    // bug not replicated, Rust stays correct).
    let repo = fresh_repo("rbd38a-body");
    commit(&repo.path, "a.txt");
    git(
        &repo.path,
        &["commit", "--allow-empty", "-q", "-m", "second"],
    );
    git(
        &repo.path,
        &[
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "third",
            "-m",
            "commit 0123456789abcdef0123456789abcdef01234567",
            "-m",
            "Differential Revision: D77",
        ],
    );
    let messages = git_commit_messages(&repo.path, GIT_MAGIC_ROOT_COMMIT, "HEAD").unwrap();
    assert_eq!(messages.len(), 3);
    let (ids, _) = collect_explicit_revision_ids(&messages).unwrap();
    assert_eq!(ids, vec![serde_json::json!(77)]);
}

#[test]
fn rbd38e_multiline_and_empty_wrappings_rejected() {
    // PHP's `(.*)`/`.+` never span `\n`, so multi-line `exec(...)`
    // is an unrecognized rule (never a shell command), and empty
    // git-side wrappings fall through to plain ref matching.
    let repo = fresh_repo("rbd38e-wrap");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let loaded = empty_loaded();
    let inputs = base_inputs("arc:exec(echo hi\nbye)", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("not a recognized rule"), "{err}");
    let inputs = base_inputs("git:merge-base()", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");
    assert_eq!(strip_wrapped("exec(a\nb)", "exec(", ")"), None);
    assert_eq!(strip_wrapped("exec(a)", "exec(", ")"), Some("a"));
    assert_eq!(
        strip_wrapped_nonempty("merge-base()", "merge-base(", ")"),
        None
    );
}

#[test]
fn rbd21_invalid_head_reports_missing_commit() {
    let repo = fresh_repo("rbd21-head");
    commit(&repo.path, "a.txt");
    let err = resolve_head(&repo.path, "no-such-rev").unwrap_err();
    assert_eq!(
        err,
        "Unable to find any git commit named 'no-such-rev' in this repository."
    );
    assert!(is_usage_message(&err));
    assert!(!is_usage_message("You can't get local commit information."));
    let head = resolve_head(&repo.path, "HEAD").unwrap();
    assert_eq!(head.len(), 40);
}

#[test]
fn rbd23_upstream_merge_base_failure_renders_command_exception() {
    // The upstream resolves but the histories are unrelated, so
    // `merge-base` exits 1 and `execxLocal` raises `CommandException`.
    let repo = fresh_repo("rbd23-upstream");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    git(&repo.path, &["checkout", "-q", "--orphan", "other"]);
    commit(&repo.path, "b.txt");
    git(&repo.path, &["checkout", "-q", "master"]);
    git(&repo.path, &["config", "branch.master.remote", "."]);
    git(
        &repo.path,
        &["config", "branch.master.merge", "refs/heads/other"],
    );
    let loaded = empty_loaded();
    let inputs = base_inputs("arc:upstream", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(
        err.starts_with("Command failed with error #1!\nCOMMAND\ngit merge-base other HEAD\n"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("\nSTDOUT\n(empty)\n\nSTDERR\n(empty)"),
        "{err}"
    );
}

/// Scripted mock Conduit server: serves each canned body in order,
/// capturing (request line, raw body) per request.
fn serve_script(
    script: Vec<&'static str>,
) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for canned in script {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .or_else(|| line.strip_prefix("content-length: "))
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let request = head.lines().next().unwrap_or("").to_string();
            tx.send((request, String::from_utf8(body).unwrap()))
                .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (base, rx)
}

fn mock_client(base: &str) -> conduit::Client {
    conduit::Client::new(base, "t").unwrap()
}

#[test]
fn base_prefers_explicit_then_upstream() {
    let repo = fresh_repo("base");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    assert_eq!(head.len(), 40);
    let loaded = empty_loaded();

    let inputs = base_inputs("", &never_prompt);
    let (explicit, why) =
        resolve_base(&repo.path, Some("HEAD~1"), "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(explicit, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("explicitly specified base commit"), "{why}");

    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(
        &repo.path,
        Some("no-such-rev"),
        "HEAD",
        false,
        &loaded,
        &inputs,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Unable to find any git commit named 'no-such-rev' in this repository."
    );

    // No upstream, no rules, no scratch: the prompt default
    // (`origin/master`) does not exist here, so resolution fails like PHP.
    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert_eq!(
        err,
        "Relative commit 'origin/master' is not the name of a commit!"
    );

    let origin = TempDir::new("worigin");
    git(&origin.path, &["init", "-q", "--bare"]);
    git(
        &repo.path,
        &["remote", "add", "origin", &origin.path.to_string_lossy()],
    );
    git(&repo.path, &["push", "-q", "-u", "origin", "master"]);
    commit(&repo.path, "c.txt");
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("merge-base"), "{why}");

    let commits = commits_between(&repo.path, &base, "HEAD").unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "c.txt");
    assert!(
        commits_between(&repo.path, &head, &head)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn base_rules_cover_literal_empty_and_refs() {
    let repo = fresh_repo("rules");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let loaded = empty_loaded();

    let inputs = base_inputs("literal:abc123", &never_prompt);
    assert_eq!(
        resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap(),
        ("abc123".to_string(), String::new())
    );

    let inputs = base_inputs("arc:empty", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("arc:empty"), "{why}");

    // Unknown refs fall through to the next rule.
    let inputs = base_inputs("git:no-such-ref, git:HEAD~1", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, "HEAD~1");

    let inputs = base_inputs("git:merge-base(HEAD~1)", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("merge-base of 'HEAD~1' and HEAD"), "{why}");

    // Nothing matched: the PHP usage error.
    let inputs = base_inputs("git:no-such-ref", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");

    // Malformed and unknown rules fail like PHP.
    let inputs = base_inputs("arcupstream", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("must have a type and name"), "{err}");

    let inputs = base_inputs("bogus:rule", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("not a recognized rule"), "{err}");
}

#[test]
fn base_engine_switches_skips_and_halts() {
    let repo = fresh_repo("engine");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let mut loaded = empty_loaded();
    loaded.sources.project.insert(
        "base".to_string(),
        serde_json::json!("literal:from-project"),
    );

    // `arc:project` switches to the project source mid-stream.
    let inputs = base_inputs("arc:project, literal:unreached", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, "from-project");

    // `arc:skip` moves to the next rule; `arc:halt` stops everything.
    let inputs = base_inputs("arc:skip, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");

    let inputs = base_inputs("arc:halt, literal:unreached", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");

    // `arc:prompt` uses the answer, or falls through when empty.
    let inputs = base_inputs("arc:prompt", &|_: &str| Some("typed-base".to_string()));
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "typed-base");
    assert!(why.contains("what you typed"), "{why}");

    let inputs = base_inputs("arc:prompt, literal:after-prompt", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "after-prompt");
}

#[test]
fn base_exec_runs_in_project_root() {
    let repo = fresh_repo("exec");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    std::fs::write(repo.path.join(".arcconfig"), r#"{"base": ""}"#).unwrap();
    let loaded = empty_loaded();

    let inputs = base_inputs("arc:exec(echo hi)", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, "hi");

    // A failing command falls through to the next rule.
    let inputs = base_inputs("arc:exec(exit 3), literal:fallback", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "fallback");
}

#[test]
fn base_default_chain_uses_project_and_scratch() {
    let repo = fresh_repo("defaultchain");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head_minus_1 = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let mut loaded = empty_loaded();
    loaded.sources.project.insert(
        "git.default-relative-commit".to_string(),
        serde_json::json!("HEAD~1"),
    );

    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, head_minus_1);
    assert!(why.contains("git.default-relative-commit"), "{why}");
}

#[test]
fn shallow_histories_use_the_empty_tree() {
    let repo = fresh_repo("shallow");
    let loaded = empty_loaded();
    let inputs = base_inputs("", &never_prompt);
    // Zero commits.
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("no commits"), "{why}");

    commit(&repo.path, "a.txt");
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("only one commit"), "{why}");
}

#[test]
fn explicit_magic_base_passes_through() {
    let repo = fresh_repo("magicexplicit");
    commit(&repo.path, "a.txt");
    let loaded = empty_loaded();
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(
        &repo.path,
        Some(GIT_MAGIC_ROOT_COMMIT),
        "HEAD",
        false,
        &loaded,
        &inputs,
    )
    .unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("empty tree"), "{why}");
}

#[test]
fn context_covers_repos_and_plain_dirs() {
    let repo = fresh_repo("ctx");
    commit(&repo.path, "a.txt");
    std::fs::write(
        repo.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://h.example/", "repository.callsign": "X"}"#,
    )
    .unwrap();
    let user_file = repo.path.join(".arcrc");
    std::fs::write(
        &user_file,
        r#"{"hosts": {"https://h.example/api/": {"token": "t"}}}"#,
    )
    .unwrap();
    // Single commit: the empty-tree base, no prompt needed. The mock
    // answers `repository.query` (unique match) plus `differential.query`
    // (no match) for `load_context`.
    let (base, _seen) = serve_script(vec![
        "{\"error_code\": null, \"result\": [{\"name\": \"demo\", \"callsign\": \"X\"}]}",
        "{\"error_code\": null, \"result\": []}",
    ]);
    let client = mock_client(&base);
    // T1153 RB-D8: the authed client caches the viewer, so matching
    // needs no `user.whoami` (the mock serves exactly two calls).
    client.set_authenticated_user("alice", "PHID-USER-1");
    let merged = get_config::load_merged(
        &repo.path,
        Some(&user_file),
        &get_config::LoadOptions::default(),
    )
    .unwrap();
    let inputs = base_inputs("", &never_prompt);
    let ctx = load_context(
        &repo.path,
        Some(&user_file),
        None,
        None,
        None,
        "HEAD",
        false,
        &merged,
        &inputs,
        &client,
        false,
    )
    .unwrap();
    assert_eq!(ctx.vcs, "git");
    assert!(ctx.vcs_root.is_some());
    assert!(ctx.project_file.is_some());
    assert_eq!(ctx.branch.as_deref(), Some("master"));
    assert!(ctx.head.is_some());
    assert_eq!(ctx.conduit_uri.as_deref(), Some("https://h.example/"));
    assert_eq!(ctx.conduit_source, "project (.arcconfig)");
    assert_eq!(ctx.token_installed, Some(true));
    assert_eq!(ctx.callsign.as_deref(), Some("X"));
    assert_eq!(ctx.repo_name.as_deref(), Some("demo"));
    assert_eq!(ctx.base.as_deref(), Some(GIT_MAGIC_ROOT_COMMIT));

    let text = render_ansi(&ctx, false);
    for section in ["REPOSITORY", "COMMIT RANGE", "MATCHING REVISIONS"] {
        assert!(text.contains(section), "{text}");
    }
    assert!(!text.contains("WORKING COPY"), "{text}");
    assert!(!text.contains("CONDUIT"), "{text}");
    let json = report_json(&ctx);
    assert_eq!(json["vcs"], serde_json::json!("git"));
    assert_eq!(json["callsign"], serde_json::json!("X"));
    assert_eq!(json["repository_name"], serde_json::json!("demo"));

    // Outside a working copy the load fails with the gate text.
    let plain = TempDir::new("plain");
    let inputs = base_inputs("", &never_prompt);
    let err = load_context(
        &plain.path,
        None,
        None,
        None,
        None,
        "HEAD",
        false,
        &empty_loaded(),
        &inputs,
        &client,
        false,
    )
    .unwrap_err();
    assert_eq!(err, WORKING_COPY_REQUIRED);
    assert!(is_usage_message(&err));
}

#[test]
fn args_cover_flags_ranges_and_unported() {
    assert!(matches!(parse_args(&[]), Parsed::Options(_)));
    assert!(matches!(parse_args(&["--help".to_string()]), Parsed::Help));
    match parse_args(&["--base".to_string(), "arc:upstream".to_string()]) {
        Parsed::Options(opts) => assert_eq!(opts.base_rules.as_deref(), Some("arc:upstream")),
        _ => panic!("expected --base options"),
    }
    match parse_args(&["--base=arc:empty".to_string()]) {
        Parsed::Options(opts) => assert_eq!(opts.base_rules.as_deref(), Some("arc:empty")),
        _ => panic!("expected --base= options"),
    }
    assert!(matches!(
        parse_args(&["--base".to_string()]),
        Parsed::Error(_)
    ));
    match parse_args(&["--any-status".to_string()]) {
        Parsed::Options(opts) => assert!(opts.any_status),
        _ => panic!("expected --any-status options"),
    }
    assert!(matches!(
        parse_args(&["--bogus".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(
        parse_args(&["--head".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(
        parse_args(&["--conduit-uri".to_string()]),
        Parsed::Error(_)
    ));
    // T1153 RB-D16: parsing collects every positional; the count
    // check runs after the repository section prints.
    match parse_args(&["a".to_string(), "b".to_string()]) {
        Parsed::Options(opts) => {
            assert_eq!(opts.commits, vec!["a".to_string(), "b".to_string()])
        }
        _ => panic!("expected two-commit options"),
    }
    match parse_args(&[
        "abc123".to_string(),
        "--head".to_string(),
        "HEAD".to_string(),
    ]) {
        Parsed::Options(opts) => {
            assert_eq!(opts.commits, vec!["abc123".to_string()]);
            assert_eq!(opts.head, "HEAD");
        }
        _ => panic!("expected options"),
    }
    // T1153 RB-D25: `--flag=value` works for `--head` and
    // `--conduit-uri` too.
    match parse_args(&["--head=abc123".to_string()]) {
        Parsed::Options(opts) => assert_eq!(opts.head, "abc123"),
        _ => panic!("expected --head= options"),
    }
    match parse_args(&["--conduit-uri=https://h.test/api/".to_string()]) {
        Parsed::Options(opts) => {
            assert_eq!(opts.conduit_uri.as_deref(), Some("https://h.test/api/"))
        }
        _ => panic!("expected --conduit-uri= options"),
    }
}

#[test]
fn tokenizes_base_specifications() {
    // T1153 RB-D38e: falsy rules (`""`, `"0"`) vanish silently.
    assert!(tokenize_base_rules("").unwrap().is_empty());
    assert!(tokenize_base_rules("0").unwrap().is_empty());
    assert_eq!(
        tokenize_base_rules("arc:upstream,0,git:master").unwrap(),
        vec!["arc:upstream".to_string(), "git:master".to_string()]
    );
    assert_eq!(
        tokenize_base_rules("arc:upstream, git:master").unwrap(),
        vec!["arc:upstream".to_string(), "git:master".to_string()]
    );
    assert_eq!(
        tokenize_base_rules("arc:upstream,, ,git:master").unwrap(),
        vec!["arc:upstream".to_string(), "git:master".to_string()]
    );
    assert_eq!(
        tokenize_base_rules("arcupstream").unwrap_err(),
        "Rule 'arcupstream' is invalid, it must have a type and name like 'arc:upstream'."
    );
}

#[test]
fn extracts_revision_ids_from_corpora() {
    assert_eq!(revision_id_from_corpus("no footer here").unwrap(), None);
    assert_eq!(
        revision_id_from_corpus("Subject\n\nDifferential Revision: D123\n").unwrap(),
        Some("123".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("differential revision: d7").unwrap(),
        Some("7".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: https://x.test/D45").unwrap(),
        Some("45".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: https://x.test/D45/").unwrap(),
        Some("45".to_string())
    );
    // `\s*` crosses newlines, so an empty footer consumes the next line
    // as its value (here: not a monogram, so it throws).
    assert!(
        revision_id_from_corpus("Differential Revision:   \nDifferential Revision: D9").is_err()
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:\nD9").unwrap(),
        Some("9".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:\n\n  D10  \n").unwrap(),
        Some("10".to_string())
    );
    // A bare prefix at end of input matches nothing and throws nothing.
    assert_eq!(
        revision_id_from_corpus("Differential Revision:\n").unwrap(),
        None
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:").unwrap(),
        None
    );
    // T1153 RB-D20: a whitespace-only tail never matches — even
    // with spaces in it — so empty footers fall through to hash
    // matching instead of throwing.
    assert_eq!(
        revision_id_from_corpus("Differential Revision:   ").unwrap(),
        None
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:  \n  \n").unwrap(),
        None
    );
    // The prefix must sit at a line start; matching is ASCII-only.
    assert_eq!(
        revision_id_from_corpus("xDifferential Revision: D1").unwrap(),
        None
    );
    assert_eq!(
        revision_id_from_corpus("DIFFERENTIAL REVISION: D5").unwrap(),
        Some("5".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: D8\r\n").unwrap(),
        Some("8".to_string())
    );
    assert!(revision_id_from_corpus("Differential Revision: \u{a0}D9").is_err());
    // Query strings and fragments are stripped before path matching.
    assert_eq!(
        revision_id_from_corpus("Differential Revision: D77?view=all").unwrap(),
        Some("77".to_string())
    );
    // Malformed footers fail like PHP.
    for bad in [
        "Differential Revision: 123",
        "Differential Revision: D0123",
        "Differential Revision: D",
        "Differential Revision: https://x.test/foo/D1",
        "Differential Revision: foo/D1",
    ] {
        assert!(revision_id_from_corpus(bad).is_err(), "{bad}");
    }
    assert!(
        revision_id_from_corpus("Differential Revision: nope")
            .unwrap_err()
            .contains("Invalid \"Differential Revision\" field")
    );
}

#[test]
fn rbd25_invalid_footer_uses_ceb_phabricator_text() {
    assert_eq!(
        revision_id_from_corpus("Differential Revision: nope").unwrap_err(),
        "Invalid \"Differential Revision\" field in commit message. This field should have a \
         revision identifier like \"D123\" or a Phabricator URI like \
         \"https://phabricator.example.com/D123\", but has \"nope\"."
    );
}

#[test]
fn rbd38b_scheme_split_uris_resolve() {
    // Without `://`, `PhutilURI` still splits a scheme: `host:D123`
    // carries path `D123`.
    assert_eq!(
        revision_id_from_corpus("Differential Revision: host:D123").unwrap(),
        Some("123".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: x:D45?view=all").unwrap(),
        Some("45".to_string())
    );
    // Non-scheme colons still throw.
    for bad in [
        "Differential Revision: 1abc:D123",
        "Differential Revision: :D123",
        "Differential Revision: host:D12x",
        "Differential Revision: a:b:c",
    ] {
        assert!(revision_id_from_corpus(bad).is_err(), "{bad}");
    }
}

#[test]
fn rejects_reversed_ranges() {
    let repo = fresh_repo("reversed");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let parent = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    assert!(check_range_direction(&repo.path, &parent, &head, "HEAD").is_ok());
    let err = check_range_direction(&repo.path, &head, &parent, "HEAD~1").unwrap_err();
    assert!(err.contains("empty, backward or invalid"), "{err}");
    assert!(err.contains(&head), "{err}");
    // An empty range is invalid too.
    assert!(check_range_direction(&repo.path, &head, &head, "HEAD").is_err());
}

#[test]
fn local_commits_carry_trees_and_messages() {
    let repo = fresh_repo("localinfo");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let parent = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let info = local_commit_information(&repo.path, &parent, "HEAD").unwrap();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].summary, "b.txt");
    assert_eq!(info[0].tree.len(), 40);
    assert!(info[0].message.contains("b.txt"));

    let messages = git_commit_messages(&repo.path, &parent, "HEAD").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0.len(), 40);

    let all = git_commit_messages(&repo.path, GIT_MAGIC_ROOT_COMMIT, "HEAD").unwrap();
    assert_eq!(all.len(), 2);
}

fn ansi_assoc() -> Association {
    Association {
        reasons: vec!["Configuration value \"repository.callsign\" is empty.".to_string()],
        repo_name: None,
    }
}

fn ansi_context(matching: Vec<MatchedRevision>) -> Context {
    Context {
        dir: "/tmp/demo".to_string(),
        vcs: "git",
        vcs_root: Some("/tmp/demo".to_string()),
        project_root: None,
        project_file: None,
        branch: Some("master".to_string()),
        head: Some("abc".to_string()),
        head_subject: Some("subject".to_string()),
        remote_url: None,
        callsign: None,
        conduit_uri: None,
        conduit_source: "none",
        token_installed: None,
        repo_name: None,
        repo_reasons: vec!["Configuration value \"repository.callsign\" is empty.".to_string()],
        base: Some("def0123456789abcdef0123456789abcdef0123".to_string()),
        base_explanation: Some("why".to_string()),
        base16: Some("def0123456789ab".to_string()),
        summary: Some("base subject".to_string()),
        command: Some("git diff def0123456789ab..HEAD".to_string()),
        head_ref: "HEAD".to_string(),
        head_explicit: false,
        commits: vec![Commit {
            sha: "abc0123456789abcdef0123456789abcdef0123".to_string(),
            subject: "top subject".to_string(),
        }],
        diff_arg: String::new(),
        matching,
    }
}

fn matched(monogram: &str) -> MatchedRevision {
    MatchedRevision {
        monogram: monogram.to_string(),
        title: "title".to_string(),
        author: None,
        why: "reason".to_string(),
    }
}

#[test]
fn section_headers_bold_only_on_tty() {
    let ctx = ansi_context(Vec::new());
    let plain = render_ansi(&ctx, false);
    for header in ["REPOSITORY", "COMMIT RANGE", "MATCHING REVISIONS"] {
        assert!(plain.contains(&format!("{header}\n")), "{plain}");
    }
    assert!(!plain.contains("WORKING COPY"), "{plain}");
    assert!(!plain.contains("CONDUIT"), "{plain}");
    assert!(!plain.contains('\x1b'), "{plain}");

    let styled = render_ansi(&ctx, true);
    for header in ["REPOSITORY", "COMMIT RANGE", "MATCHING REVISIONS"] {
        assert!(
            styled.contains(&format!("\x1b[1m{header}\x1b[m\n")),
            "{styled}"
        );
    }
}

#[test]
fn verdict_words_bold_only_on_tty() {
    let ctx = ansi_context(Vec::new());
    let plain = render_ansi(&ctx, false);
    assert!(
        plain.contains("new revision will be created if you run 'arc diff'.\n\n"),
        "{plain}"
    );
    assert!(!plain.contains('\x1b'), "{plain}");
    let styled = render_ansi(&ctx, true);
    assert!(
        styled.contains("new revision will be \x1b[1mcreated\x1b[m if you run 'arc diff'.\n\n"),
        "{styled}"
    );

    // The wrap breaks after "will" (verified against
    // `phutil_console_wrap` at ceb).
    let ctx = ansi_context(vec![matched("D1")]);
    let plain = render_ansi(&ctx, false);
    assert!(
        plain.contains("be updated if you run 'arc diff'.\n\n"),
        "{plain}"
    );
    let styled = render_ansi(&ctx, true);
    assert!(
        styled.contains("be \x1b[1mupdated\x1b[m if you run 'arc diff'.\n\n"),
        "{styled}"
    );
}

#[test]
fn rbd19_matching_subheader_and_double_space_quirk() {
    // `ArcanistWhichWorkflow.php:189-193`: the subheader prints on
    // every matched run (empty or not); with an explicit commit the
    // multi-revision sentence keeps PHP's double space (`'arc diff '`
    // plus the leading-space arg).
    let subheader = "These Differential revisions match the changes in this working copy:\n\n";
    let empty = ansi_context(Vec::new());
    let plain = render_ansi(&empty, false);
    assert!(plain.contains(subheader), "{plain}");
    assert!(
        plain.contains(&format!("MATCHING REVISIONS\n{subheader}")),
        "{plain}"
    );

    let mut ctx = ansi_context(vec![matched("D1"), matched("D2")]);
    ctx.diff_arg = " abc123".to_string();
    let plain = render_ansi(&ctx, false);
    assert!(plain.contains(subheader), "{plain}");
    assert!(
        plain.contains("if you run 'arc diff  abc123'.\n\n"),
        "{plain}"
    );
    let one = ansi_context(vec![matched("D1")]);
    assert!(
        render_ansi(&one, false).contains(subheader),
        "single-revision runs keep the subheader"
    );
}

#[test]
fn rbd24_empty_repo_fails_after_prefix() {
    // `ArcanistGitAPI.php:99-113`: PHP prints the repository section
    // first, then fails with this exact text (a generic exception).
    let repo = fresh_repo("empty");
    let err = load_range_data(
        &repo.path,
        &empty_loaded(),
        None,
        "HEAD",
        false,
        "",
        &never_prompt,
    )
    .unwrap_err();
    assert_eq!(err, NO_LOCAL_COMMITS);
    assert!(!is_usage_message(&err));

    // The staged prefix is the header plus the association body.
    let assoc = ansi_assoc();
    let prefix = format!(
        "{}{}\n",
        repository_header(false),
        render_repository_body(&assoc, false)
    );
    assert!(prefix.starts_with("REPOSITORY\n"), "{prefix}");
    assert!(prefix.contains("followed this process"), "{prefix}");
    assert!(!prefix.contains("COMMIT RANGE"), "{prefix}");
    assert!(!prefix.contains("MATCHING REVISIONS"), "{prefix}");
}

#[test]
fn multi_revision_guidance_stays_plain() {
    // PHP never passes the multi-revision sentence through
    // `phutil_console_format`, so both modes share the exact bytes.
    let ctx = ansi_context(vec![matched("D1"), matched("D2")]);
    // The wrap breaks after "you" (verified against
    // `phutil_console_wrap` at ceb).
    let sentence = "will be asked which revision you want to update if you run 'arc diff '.\n\n";
    let plain = render_ansi(&ctx, false);
    assert!(plain.contains(sentence), "{plain}");
    let styled = render_ansi(&ctx, true);
    assert!(styled.contains(sentence), "{styled}");
}

// -( R5 TTY gating: T1153 RB-D22 )---------------------------------------------------

#[test]
fn r5_rb_d22_banner_essay_bytes() {
    // `ArcanistGitAPI.php:323-352`: the green-badge banner plus the
    // 78-column essay, markup-stripped without a TTY.
    assert_eq!(
        default_range_banner(false),
        " Select a Default Commit Range \n\n"
    );
    let styled = default_range_banner(true);
    assert!(
        styled.contains("Select a Default Commit Range"),
        "{styled:?}"
    );
    assert!(styled.contains('\x1b'), "{styled:?}");
    let essay = default_range_essay();
    assert!(essay.starts_with("You're running a command"), "{essay:?}");
    assert!(essay.contains("origin/master"), "{essay:?}");
    assert!(essay.ends_with("start of the commit range.)"), "{essay:?}");
    for line in essay.split('\n') {
        assert!(line.chars().count() <= 78, "{line:?}");
    }
}

#[test]
fn r5_rb_d22_default_answer_mapping() {
    // `ArcanistGitAPI.php:356-358`: blank takes `origin/master`,
    // anything else passes through raw.
    assert_eq!(default_range_answer(""), "origin/master");
    assert_eq!(default_range_answer("   "), "origin/master");
    assert_eq!(default_range_answer("HEAD^"), "HEAD^");
    assert_eq!(default_range_answer("  HEAD^  "), "  HEAD^  ");
}

#[test]
fn r5_rb_d22_prompt_line_gate() {
    // Piped stdin records the TTY failure out-of-band for the prompt
    // call sites; the take resets it.
    assert_eq!(prompt_line_with_tty("Q?", false), None);
    assert!(take_prompt_tty_failed());
    assert!(!take_prompt_tty_failed());
}

#[test]
fn r5_rb_d22_non_tty_throws_after_echo() {
    // `ArcanistGitAPI.php:354-360` via `phutil_console_prompt()`:
    // piped stdin throws instead of silently defaulting.
    let repo = fresh_repo("r5d22");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let fail = |_: &str| {
        PROMPT_TTY_FAILED.with(|failed| failed.set(true));
        None
    };
    let inputs = base_inputs("", &fail);
    assert_eq!(
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    assert!(!take_prompt_tty_failed());
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `which` is a subprocess workflow: argument errors render `Usage
    // Exception:` (exit 1) with no usage dump.
    assert_eq!(
        run(&["--bogus".to_string()]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn rbd8_matching_reuses_cached_viewer_without_whoami() {
    // PHP resolves whoami once at startup auth; revision matching then
    // reuses the cached viewer (`getUserPHID()`). The mock serves
    // exactly the two workflow calls — a third `user.whoami` would
    // fail the match.
    use std::io::{Read, Write};
    let repo = fresh_repo("rbd8");
    commit(&repo.path, "a.txt");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let served = seen.clone();
    std::thread::spawn(move || {
        for canned in [
            "{\"error_code\": null, \"result\": [{\"id\": 7, \"title\": \"T\", \
             \"authorPHID\": \"PHID-USER-9\"}]}",
            "{\"error_code\": null, \"result\": [{\"phid\": \"PHID-USER-9\", \
             \"userName\": \"bob\"}]}",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let head = String::from_utf8(head).unwrap();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            stream.read_exact(&mut body).unwrap();
            let request = head.lines().next().unwrap_or("").to_string();
            served.lock().unwrap().push(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                canned.len(),
                canned
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let client = conduit::Client::new(&base, "t").unwrap();
    client.set_authenticated_user("alice", "PHID-USER-1");
    let matched =
        match_working_copy_revisions(&client, &repo.path, GIT_MAGIC_ROOT_COMMIT, "HEAD", false)
            .unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].monogram, "D7");
    assert_eq!(matched[0].author.as_deref(), Some("bob"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].contains("differential.query"), "{seen:?}");
    assert!(seen[1].contains("user.query"), "{seen:?}");
}

#[test]
fn core33_find_project_matches_vcs_scoping() {
    // Git: a subdir `.arcconfig` is invisible; only the root file counts.
    let tmp = TempDir::new("core33git");
    let child = tmp.path.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(child.join(".arcconfig"), "{}").unwrap();
    assert_eq!(find_project(&child), (None, None));
    // Subversion: nearest `.arcconfig` from `dir` up to the root wins.
    let tmp = TempDir::new("core33svn");
    let child = tmp.path.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(tmp.path.join(".svn")).unwrap();
    std::fs::write(tmp.path.join(".arcconfig"), "{}").unwrap();
    std::fs::write(tmp.path.join("a").join(".arcconfig"), "{}").unwrap();
    let (root, file) = find_project(&child);
    assert_eq!(root.unwrap(), tmp.path.join("a"));
    assert_eq!(file.unwrap(), tmp.path.join("a").join(".arcconfig"));
}

#[test]
fn core30_callsign_reads_modern_top_level_user_keys() {
    // No `config` section: the whole user file is the config, so a
    // top-level `repository.callsign` resolves (classic `readUserArcConfig`).
    let tmp = TempDir::new("core30call");
    let arcrc = tmp.path.join(".arcrc");
    std::fs::write(&arcrc, r#"{"repository.callsign": "U"}"#).unwrap();
    let merged =
        get_config::load_merged(&tmp.path, Some(&arcrc), &get_config::LoadOptions::default())
            .unwrap();
    assert_eq!(repository_callsign(&merged).unwrap().as_deref(), Some("U"));
    // With a section, only the section answers.
    std::fs::write(
        &arcrc,
        r#"{"config": {"repository.callsign": "S"}, "repository.callsign": "T"}"#,
    )
    .unwrap();
    let merged =
        get_config::load_merged(&tmp.path, Some(&arcrc), &get_config::LoadOptions::default())
            .unwrap();
    assert_eq!(repository_callsign(&merged).unwrap().as_deref(), Some("S"));
}

#[test]
fn carryover_conduit_labels_name_the_merged_source() {
    // Wave-2 carry-over: local URIs label with their VCS metadata dir
    // instead of the old hardcoded `project (.arcconfig)`. Local outranks
    // project/user, so ambient `~/.arcrc` cannot disturb these.
    for (meta, label) in [
        (".git", "local (.git/arc/config)"),
        (".hg", "local (.hg/arc/config)"),
        (".svn", "local (.svn/arc/config)"),
    ] {
        let tmp = TempDir::new("carrylocal");
        let child = tmp.path.join("sub");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(tmp.path.join(meta).join("arc")).unwrap();
        std::fs::write(
            tmp.path.join(meta).join("arc").join("config"),
            r#"{"phabricator.uri": "https://local.example/"}"#,
        )
        .unwrap();
        assert_eq!(
            resolve_conduit_in(&child, None, None),
            (Some("https://local.example/".to_string()), label),
            "{meta}"
        );
    }
}
#[test]
fn r10_which_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "WHICH WORKFLOW\n\
         \n\
         which\n\
         \x20     which [options] (svn)\n\
         \x20     which [options] [commit] (hg, git)\n\
         \n\
         \x20         Supports: svn, git, hg\n\
         \x20         Shows which repository the current working copy corresponds to,\n\
         \x20         which commits 'arc diff' will select, and which revision is in\n\
         \x20         the working copy (or which revisions, if more than one matches).\n\
         \n\
         \x20     --any-status\n\
         \x20         Show committed and abandoned revisions.\n\
         \n\
         \x20     --base rules\n\
         \x20         Additional rules for determining base revision.\n\
         \n\
         \x20     --head commit\n\
         \x20         Specify the end of the commit range to select.\n\
         \n\
         \x20     --show-base\n\
         \x20         Print base commit only and exit.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("WHICH WORKFLOW"), "{}", usage());
}

#[test]
fn b21_which_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `which`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(matches!(parse_args(&[]), Parsed::Options(_)));
    assert!(matches!(parse_args(&["--help".to_string()]), Parsed::Help));
    let text = usage_text(false);
    assert!(text.contains("WHICH WORKFLOW"), "{text}");
}

#[test]
fn b23_which_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help which`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("WHICH WORKFLOW\n"), "{text}");
    assert!(text.contains("Shows which repository"), "{text}");
    assert!(text.contains("--show-base"), "{text}");
    assert!(!text.contains("--json"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// -( Wave 6: T1153 RB-D16/RB-D17/RB-D18 prose port )----------------------------

fn merged_with(key: &str, value: serde_json::Value, source: &str) -> get_config::MergedConfig {
    let mut merged = empty_loaded();
    let map = match source {
        "runtime" => &mut merged.runtime,
        "local" => &mut merged.sources.local,
        "project" => &mut merged.sources.project,
        "user" => &mut merged.sources.user,
        "system" => &mut merged.sources.system,
        _ => panic!("bad source {source}"),
    };
    map.insert(key.to_string(), value);
    merged
}

#[test]
fn rbd38_string_winner_checks_types_and_skips_nulls() {
    // T1153 RB-D38: every present non-null value must be a string;
    // JSON nulls read as missing; the winner is the first non-null.
    let bad = merged_with("base", serde_json::json!(123), "user");
    assert_eq!(
        string_config_winner("base", &bad).unwrap_err(),
        "Type of setting 'base' must be string."
    );
    // A non-string in a losing source still throws (PHP builds the
    // whole map eagerly).
    let mut merged = merged_with("base", serde_json::json!("git:HEAD~1"), "project");
    merged
        .sources
        .user
        .insert("base".to_string(), serde_json::json!(7));
    assert!(string_config_winner("base", &merged).is_err());
    // Nulls are missing, so the user value wins over a project null.
    let mut merged = merged_with("base", serde_json::json!("git:HEAD~1"), "user");
    merged
        .sources
        .project
        .insert("base".to_string(), serde_json::Value::Null);
    assert_eq!(
        string_config_winner("base", &merged).unwrap().as_deref(),
        Some("git:HEAD~1")
    );
    // Runtime outranks everything.
    let mut merged = merged_with("base", serde_json::json!("git:HEAD~1"), "user");
    merged
        .runtime
        .insert("base".to_string(), serde_json::json!("arc:empty"));
    assert_eq!(
        string_config_winner("base", &merged).unwrap().as_deref(),
        Some("arc:empty")
    );
}

#[test]
fn rbd38_base_gate_reads_the_winner() {
    // T1153 RB-D38: the gate is PHP-truthy on the flag plus the
    // winning value — a falsy winner takes the default chain even
    // when lower sources hold rules.
    let repo = fresh_repo("rbd38gate");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    // Project `0` wins over a resolving user rule: default chain, no
    // upstream or scratch here, so the prompt default fails like PHP.
    let mut merged = merged_with("base", serde_json::json!("git:HEAD~1"), "user");
    merged
        .sources
        .project
        .insert("base".to_string(), serde_json::json!("0"));
    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &merged, &inputs).unwrap_err();
    assert_eq!(
        err,
        "Relative commit 'origin/master' is not the name of a commit!"
    );
    // Without the falsy winner the user rule resolves.
    let merged = merged_with("base", serde_json::json!("git:HEAD~1"), "user");
    let inputs = base_inputs("", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &merged, &inputs).unwrap();
    assert_eq!(base, "HEAD~1");
    // A whitespace-only flag still takes the rules path (PHP-truthy),
    // tokenizes to nothing, and reports no matching rules.
    let inputs = base_inputs("   ", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");
}

#[test]
fn rbd38_config_base_overrides_the_flag() {
    // T1153 RB-D38: `$all_sources + $base_commit_rules` lets a
    // `--config base=` value win over `--base`.
    let repo = fresh_repo("rbd38cfg");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let merged = merged_with("base", serde_json::json!("literal:from-config"), "runtime");
    let inputs = base_inputs("literal:from-flag", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &merged, &inputs).unwrap();
    assert_eq!(base, "from-config");
}

#[test]
fn rbd4_usage_classes_match_ceb_throw_sites() {
    // T1153 RB-D4: usage-class errors render `Usage Exception:`.
    for message in [
        "Unable to find any git commit named 'x' in this repository.",
        "The specified commit range is empty, backward or invalid: the base (a) is not an ancestor of the head (b). You can not diff an empty or reversed commit range.",
        "None of the rules in your 'base' configuration matched a valid commit. Adjust rules or specify which commit you want to use explicitly.",
        "Rule 'arcupstream' is invalid, it must have a type and name like 'arc:upstream'.",
        "Base commit rule 'bogus:rule' (from source 'runtime') is not a recognized rule.",
        "Invalid \"Differential Revision\" field in commit message. This field should have a revision identifier like \"D123\" or a Phabricator URI like \"https://phabricator.example.com/D123\", but has \"nope\".",
        "Specify exactly one base commit. The end of the commit range is always the working copy state.",
        "This command must be run in a Git, Mercurial or Subversion working copy.",
        "Type of setting 'base' must be string.",
        "Type of setting 'repository.callsign' must be string.",
    ] {
        assert!(is_usage_message(message), "{message}");
    }
    // Generic failures stay generic.
    for message in [
        "Relative commit 'origin/master' is not the name of a commit!",
        "You can't get local commit information for a repository with no commits.",
        "Command failed with error #1!\nCOMMAND\ngit merge-base other HEAD\n",
        "ERR-CONDUIT-CALL: unknown method",
        crate::prompt::NOT_INTERACTIVE,
    ] {
        assert!(!is_usage_message(message), "{message}");
    }
}

#[test]
fn rbd18_diff_arg_shapes() {
    // T1153 RB-D18: ` <commit>` plus ` --head %R` (`%R` quotes only
    // when the shell needs it).
    assert_eq!(diff_arg(None, None), "");
    assert_eq!(diff_arg(Some("HEAD~2"), None), " HEAD~2");
    assert_eq!(diff_arg(None, Some("HEAD")), " --head HEAD");
    assert_eq!(
        diff_arg(Some("HEAD~2"), Some("HEAD~1")),
        " HEAD~2 --head 'HEAD~1'"
    );
}

#[test]
fn rbd18_commit_summary_shapes() {
    // T1153 RB-D18: the empty tree, PHP-trimmed subjects, and the
    // `CommandException` text for unresolvable bases.
    let repo = fresh_repo("rbd18summary");
    commit(&repo.path, "a.txt");
    assert_eq!(
        commit_summary(&repo.path, GIT_MAGIC_ROOT_COMMIT).unwrap(),
        "(The Empty Tree)"
    );
    assert_eq!(commit_summary(&repo.path, "HEAD").unwrap(), "a.txt");
    let err = commit_summary(&repo.path, "zzz-no-such-rev").unwrap_err();
    assert!(
        err.starts_with(
            "Command failed with error #128!\nCOMMAND\ngit log -n 1 --format=%s zzz-no-such-rev\n"
        ),
        "{err}"
    );
    assert!(!is_usage_message(&err));
}

#[test]
fn rbd18_range_keeps_raw_bases() {
    // T1153 RB-D18: a rule base displays unexpanded — 16 chars of the
    // raw text, its summary, and the quoted diff command.
    let repo = fresh_repo("rbd18raw");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let range = load_range_data(
        &repo.path,
        &empty_loaded(),
        None,
        "HEAD",
        false,
        "git:HEAD~1",
        &never_prompt,
    )
    .unwrap();
    assert_eq!(range.base, "HEAD~1");
    assert_eq!(range.base16, "HEAD~1");
    assert_eq!(range.summary, "a.txt");
    assert_eq!(range.command, "git diff 'HEAD~1..HEAD'");
    assert_eq!(range.commits.len(), 1);
    assert_eq!(range.commits[0].subject, "b.txt");

    // Resolved bases diff unquoted (`.` is shell-safe).
    let head_minus_1 = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let range = load_range_data(
        &repo.path,
        &empty_loaded(),
        Some("HEAD~1"),
        "HEAD",
        false,
        "",
        &never_prompt,
    )
    .unwrap();
    assert_eq!(range.base, head_minus_1);
    assert_eq!(range.base16, &head_minus_1[..16]);
    assert_eq!(
        range.command,
        format!("git diff {}..HEAD", &head_minus_1[..16])
    );
}

#[test]
fn rbd17_callsign_query_and_reasons() {
    // T1153 RB-D17: a set callsign queries by callsign and skips the
    // UUID/remote reasons; a unique match associates.
    let repo = fresh_repo("rbd17call");
    commit(&repo.path, "a.txt");
    let merged = merged_with("repository.callsign", serde_json::json!("X"), "project");
    let (base, seen) = serve_script(vec![
        "{\"error_code\": null, \"result\": [{\"name\": \"demo\", \"callsign\": \"X\"}]}",
    ]);
    let client = mock_client(&base);
    let assoc = load_association(&client, &repo.path, &merged).unwrap();
    assert_eq!(assoc.repo_name.as_deref(), Some("demo"));
    assert_eq!(
        assoc.reasons,
        vec![
            "Configuration value \"repository.callsign\" is set to \"X\".".to_string(),
            "Found a unique matching repository.".to_string(),
        ]
    );
    let (request, body) = seen.recv().unwrap();
    assert!(request.contains("repository.query"), "{request}");
    assert!(body.contains("callsigns"), "{body}");
    assert!(body.contains('X'), "{body}");
}

#[test]
fn rbd17_remote_query_and_empty_verdicts() {
    // T1153 RB-D17: without a callsign the flow falls through UUID
    // (git: none) to the remote URI; no rows means unassociated.
    let repo = fresh_repo("rbd17remote");
    commit(&repo.path, "a.txt");
    git(
        &repo.path,
        &["remote", "add", "origin", "https://example.test/r.git"],
    );
    let (base, seen) = serve_script(vec!["{\"error_code\": null, \"result\": []}"]);
    let client = mock_client(&base);
    let assoc = load_association(&client, &repo.path, &empty_loaded()).unwrap();
    assert_eq!(assoc.repo_name, None);
    assert_eq!(assoc.reasons.len(), 4);
    assert!(
        assoc.reasons[1].contains("no VCS UUID"),
        "{:?}",
        assoc.reasons
    );
    assert!(
        assoc.reasons[2].contains("https://example.test/r.git"),
        "{:?}",
        assoc.reasons
    );
    assert!(
        assoc.reasons[3].contains("No repositories matched"),
        "{:?}",
        assoc.reasons
    );
    let (request, body) = seen.recv().unwrap();
    assert!(request.contains("repository.query"), "{request}");
    assert!(body.contains("remoteURIs"), "{body}");

    // Multiple matches name their callsigns (sic `repostories`).
    let (base, _seen) = serve_script(vec![
        "{\"error_code\": null, \"result\": [{\"name\": \"a\", \"callsign\": \"A\"}, {\"name\": \"b\", \"callsign\": \"B\"}]}",
    ]);
    let client = mock_client(&base);
    let assoc = load_association(&client, &repo.path, &empty_loaded()).unwrap();
    assert_eq!(assoc.repo_name, None);
    assert!(
        assoc.reasons[3].contains("Multiple repostories (A, B) matched"),
        "{:?}",
        assoc.reasons
    );

    // A unique match without a name still counts as unassociated.
    let (base, _seen) = serve_script(vec!["{\"error_code\": null, \"result\": [{}]}"]);
    let client = mock_client(&base);
    let assoc = load_association(&client, &repo.path, &empty_loaded()).unwrap();
    assert_eq!(assoc.repo_name, None);
    assert_eq!(assoc.reasons[3], "Found a unique matching repository.");
}

#[test]
fn rbd17_no_remote_means_no_query() {
    // T1153 RB-D17: with no identifier there is no `repository.query`
    // call at all — the mock serves nothing.
    let repo = fresh_repo("rbd17noquery");
    commit(&repo.path, "a.txt");
    let (base, seen) = serve_script(vec![]);
    let client = mock_client(&base);
    let assoc = load_association(&client, &repo.path, &empty_loaded()).unwrap();
    assert_eq!(assoc.repo_name, None);
    assert_eq!(
        assoc.reasons[2],
        "Unable to determine the remote URI for this repository."
    );
    assert!(seen.try_recv().is_err());
}

#[test]
fn rbd17_stale_server_and_conduit_errors() {
    // T1153 RB-D17: `ERR-CONDUIT-CALL` means unassociated with the
    // out-of-date reason; other conduit failures propagate.
    let repo = fresh_repo("rbd17stale");
    commit(&repo.path, "a.txt");
    git(
        &repo.path,
        &["remote", "add", "origin", "https://example.test/r.git"],
    );
    let (base, _seen) = serve_script(vec![
        "{\"error_code\": \"ERR-CONDUIT-CALL\", \"error_info\": \"unknown method\"}",
    ]);
    let client = mock_client(&base);
    let assoc = load_association(&client, &repo.path, &empty_loaded()).unwrap();
    assert_eq!(assoc.repo_name, None);
    assert!(
        assoc.reasons[3].contains("more recent than"),
        "{:?}",
        assoc.reasons
    );

    let (base, _seen) = serve_script(vec![
        "{\"error_code\": \"ERR-INVALID-AUTH\", \"error_info\": \"bad token\"}",
    ]);
    let client = mock_client(&base);
    let err = load_association(&client, &repo.path, &empty_loaded()).unwrap_err();
    assert_eq!(err, "ERR-INVALID-AUTH: bad token");
    assert!(!is_usage_message(&err));
}

#[test]
fn rbd17_falsy_callsign_falls_through() {
    // T1153 RB-D17: `""` and `"0"` are falsy like PHP, so the flow
    // continues to UUID/remote instead of querying.
    let repo = fresh_repo("rbd17falsy");
    commit(&repo.path, "a.txt");
    for falsy in ["", "0"] {
        let merged = merged_with(
            "repository.callsign",
            serde_json::Value::String(falsy.to_string()),
            "project",
        );
        let (base, seen) = serve_script(vec![]);
        let client = mock_client(&base);
        let assoc = load_association(&client, &repo.path, &merged).unwrap();
        assert_eq!(assoc.repo_name, None);
        assert_eq!(
            assoc.reasons[0],
            "Configuration value \"repository.callsign\" is empty."
        );
        assert!(seen.try_recv().is_err());
    }
    // A non-string callsign is a usage error before any reason.
    let merged = merged_with("repository.callsign", serde_json::json!(123), "project");
    let (base, _seen) = serve_script(vec![]);
    let client = mock_client(&base);
    let err = load_association(&client, &repo.path, &merged).unwrap_err();
    assert_eq!(err, "Type of setting 'repository.callsign' must be string.");
    assert!(is_usage_message(&err));
}

#[test]
fn rbd16_repository_section_bytes() {
    // T1153 RB-D16: the section shape — header, process prose,
    // indent-4 wrapped reasons, verdict — pinned against the oracle.
    let assoc = Association {
        reasons: vec![
            "Configuration value \"repository.callsign\" is empty.".to_string(),
            "This repository has no VCS UUID (this is normal for git/hg).".to_string(),
            "The remote URI for this working copy is \"/tmp/w6-which/origin.git\".".to_string(),
            "No repositories matched the query. Check that your configuration is correct, or use \
             \"repository.callsign\" to select a repository explicitly."
                .to_string(),
        ],
        repo_name: None,
    };
    assert_eq!(
        render_repository(&assoc, false),
        concat!(
            "REPOSITORY\n",
            "To identify the repository associated with this working copy, arc followed this process:\n",
            "\n",
            "    Configuration value \"repository.callsign\" is empty.\n",
            "\n",
            "    This repository has no VCS UUID (this is normal for git/hg).\n",
            "\n",
            "    The remote URI for this working copy is \"/tmp/w6-which/origin.git\".\n",
            "\n",
            "    No repositories matched the query. Check that your configuration is\n",
            "    correct, or use \"repository.callsign\" to select a repository explicitly.\n",
            "\n",
            "This working copy is not associated with any repository.\n",
        )
    );
    let assoc = Association {
        reasons: vec!["Found a unique matching repository.".to_string()],
        repo_name: Some("demo".to_string()),
    };
    let plain = render_repository(&assoc, false);
    assert!(
        plain.ends_with("This working copy is associated with the demo repository.\n"),
        "{plain}"
    );
    let styled = render_repository(&assoc, true);
    assert!(
        styled.contains("associated with the \x1b[1mdemo\x1b[m repository.\n"),
        "{styled}"
    );
}

#[test]
fn rbd16_range_section_bytes() {
    // T1153 RB-D16/T1153 RB-D18: the range prose pinned against the
    // oracle (plain and styled share the wrap; only the header and
    // verdicts carry markup).
    let range = RangeData {
        base: "1bea74c735cb5331aaaf7bdae5e7d4eb5cff7dc4".to_string(),
        explanation: "it is the merge-base of 'origin/master' and HEAD, as specified by \
            'git:merge-base(origin/master)' in your user 'base' configuration."
            .to_string(),
        base16: "1bea74c735cb5331".to_string(),
        summary: "third commit here".to_string(),
        command: "git diff 1bea74c735cb5331..HEAD".to_string(),
        commits: vec![Commit {
            sha: "152877c87d0cbc8ca1b2c3d4e5f60718293a4b5c6".to_string(),
            subject: "fourth commit here".to_string(),
        }],
        head: "152877c87d0cbc8ca1b2c3d4e5f60718293a4b5c6".to_string(),
    };
    assert_eq!(
        render_range(&range, "", None, false),
        concat!(
            "COMMIT RANGE\n",
            "If you run 'arc diff', changes between the commit:\n",
            "\n",
            "    1bea74c735cb5331  third commit here\n",
            "\n",
            "...and the current working copy state will be sent to Differential, because\n",
            "it is the merge-base of 'origin/master' and HEAD, as specified by\n",
            "'git:merge-base(origin/master)' in your user 'base' configuration.\n",
            "\n",
            "You can see the exact changes that will be sent by running this command:\n",
            "\n",
            "    $ git diff 1bea74c735cb5331..HEAD\n",
            "\n",
            "These commits will be included in the diff:\n",
            "\n",
            "    152877c87d0cbc8c  fourth commit here\n",
            "\n",
            "\n",
        )
    );
    // An explicit `--head` names the head instead of the working copy.
    let headed = render_range(&range, " --head HEAD", Some("HEAD"), false);
    assert!(
        headed.contains(
            "...and \"HEAD\" will be sent to Differential, because it is the merge-base of\n"
        ),
        "{headed}"
    );
    assert!(
        headed.contains("If you run 'arc diff --head HEAD', changes between the commit:\n"),
        "{headed}"
    );
    // Empty ranges keep the placeholder.
    let mut empty = range.clone();
    empty.commits.clear();
    assert!(
        render_range(&empty, "", None, false).contains("    (No commits.)\n\n\n"),
        "{}",
        render_range(&empty, "", None, false)
    );
}

// -( Wave 6 coverage: staged runners, separator, log failure, author arms )----

fn options_of(argv: &[&str]) -> Options {
    let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    match parse_args(&owned) {
        Parsed::Options(opts) => opts,
        Parsed::Help => panic!("unexpected help for {argv:?}"),
        Parsed::Error(message) => panic!("unexpected parse error for {argv:?}: {message}"),
    }
}

fn callsign_merged(name: &str) -> get_config::MergedConfig {
    merged_with(
        "repository.callsign",
        serde_json::Value::String(name.to_string()),
        "project",
    )
}

fn authed_client(base: &str) -> conduit::Client {
    let client = mock_client(base);
    client.set_authenticated_user("alice", "PHID-USER-1");
    client
}

const UNIQUE_REPO: &str =
    "{\"error_code\": null, \"result\": [{\"name\": \"demo\", \"callsign\": \"X\"}]}";

#[test]
fn w6_run_text_reports_success() {
    // Single commit: the empty-tree base needs no prompt; the mock
    // answers `repository.query` plus `differential.query` (no match).
    let repo = fresh_repo("w6text");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![UNIQUE_REPO, "{\"error_code\": null, \"result\": []}"]);
    let client = authed_client(&base);
    let options = options_of(&[]);
    assert_eq!(
        run_text(&repo.path, &options, &callsign_merged("X"), &client),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn w6_run_text_usage_error_on_bad_callsign() {
    // A non-string callsign fails association before any conduit
    // call, routing through the usage arm of `report_which_error`.
    let repo = fresh_repo("w6assocuse");
    commit(&repo.path, "a.txt");
    let merged = merged_with("repository.callsign", serde_json::json!(123), "project");
    let (base, seen) = serve_script(vec![]);
    let client = authed_client(&base);
    let options = options_of(&[]);
    assert_eq!(
        run_text(&repo.path, &options, &merged, &client),
        std::process::ExitCode::FAILURE
    );
    assert!(seen.try_recv().is_err());
}

#[test]
fn w6_run_text_exception_on_conduit_error() {
    // Other conduit failures propagate through the generic arm of
    // `report_which_error`.
    let repo = fresh_repo("w6assocgen");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![
        "{\"error_code\": \"ERR-INVALID-AUTH\", \"error_info\": \"bad token\"}",
    ]);
    let client = authed_client(&base);
    let options = options_of(&[]);
    assert_eq!(
        run_text(&repo.path, &options, &callsign_merged("X"), &client),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn w6_run_text_rejects_two_commits() {
    // The count check runs after the repository section prints.
    let repo = fresh_repo("w6many");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![UNIQUE_REPO]);
    let client = authed_client(&base);
    let options = options_of(&["a", "b"]);
    assert_eq!(
        run_text(&repo.path, &options, &callsign_merged("X"), &client),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn w6_run_text_range_error() {
    // An unknown explicit base fails after the repository section.
    let repo = fresh_repo("w6range");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![UNIQUE_REPO]);
    let client = authed_client(&base);
    let options = options_of(&["no-such-rev"]);
    assert_eq!(
        run_text(&repo.path, &options, &callsign_merged("X"), &client),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn w6_run_text_revisions_error() {
    // A malformed footer fails matching after the range prints; the
    // failure lands before any `differential.query` call.
    let repo = fresh_repo("w6rev");
    commit(&repo.path, "a.txt");
    std::fs::write(repo.path.join("b.txt"), "x\n").unwrap();
    git(&repo.path, &["add", "b.txt"]);
    git(
        &repo.path,
        &[
            "commit",
            "-q",
            "-m",
            "second",
            "-m",
            "Differential Revision: nope",
        ],
    );
    let (base, _seen) = serve_script(vec![UNIQUE_REPO]);
    let client = authed_client(&base);
    let options = options_of(&["HEAD~1"]);
    assert_eq!(
        run_text(&repo.path, &options, &callsign_merged("X"), &client),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn w6_run_show_base_prints_base() {
    // `--show-base` needs no conduit: the explicit base prints.
    let repo = fresh_repo("w6show");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let options = options_of(&["HEAD~1"]);
    assert_eq!(
        run_show_base(&repo.path, &options, &empty_loaded()),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn w6_run_show_base_rejects_two_commits() {
    let repo = fresh_repo("w6showmany");
    commit(&repo.path, "a.txt");
    let options = options_of(&["a", "b"]);
    assert_eq!(
        run_show_base(&repo.path, &options, &empty_loaded()),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn w6_run_show_base_resolve_error() {
    let repo = fresh_repo("w6showerr");
    commit(&repo.path, "a.txt");
    let options = options_of(&["no-such-rev"]);
    assert_eq!(
        run_show_base(&repo.path, &options, &empty_loaded()),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn w6_run_json_reports_success() {
    // Like the text success path, but through the whole-context load.
    let repo = fresh_repo("w6json");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![UNIQUE_REPO, "{\"error_code\": null, \"result\": []}"]);
    let client = authed_client(&base);
    let options = options_of(&[]);
    assert_eq!(
        run_json(&repo.path, None, &options, &callsign_merged("X"), &client),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn w6_run_json_rejects_two_commits() {
    // The count check runs before the context load: no conduit call.
    let repo = fresh_repo("w6jsonmany");
    commit(&repo.path, "a.txt");
    let (base, seen) = serve_script(vec![]);
    let client = authed_client(&base);
    let options = options_of(&["a", "b"]);
    assert_eq!(
        run_json(&repo.path, None, &options, &empty_loaded(), &client),
        std::process::ExitCode::FAILURE
    );
    assert!(seen.try_recv().is_err());
}

#[test]
fn w6_run_json_context_error_outside_repo() {
    // Outside a working copy the gate error wins before any call.
    let plain = TempDir::new("w6jsonplain");
    let (base, seen) = serve_script(vec![]);
    let client = authed_client(&base);
    let options = options_of(&[]);
    assert_eq!(
        run_json(&plain.path, None, &options, &empty_loaded(), &client),
        std::process::ExitCode::FAILURE
    );
    assert!(seen.try_recv().is_err());
}

#[test]
fn w6_double_dash_collects_positionals() {
    // Everything after `--` is a positional, even flag shapes.
    match parse_args(&["--".to_string(), "abc123".to_string(), "--head".to_string()]) {
        Parsed::Options(opts) => assert_eq!(
            opts.commits,
            vec!["abc123".to_string(), "--head".to_string()]
        ),
        _ => panic!("expected options"),
    }
}

#[test]
fn w6_local_log_failure_reports_command() {
    // An unresolvable base fails `git log` like `execxLocal`: the
    // `CommandException` text names the command and exit code.
    let repo = fresh_repo("w6logfail");
    commit(&repo.path, "a.txt");
    let err = match local_commit_information(&repo.path, "zzz-no-such-rev", "HEAD") {
        Ok(_) => panic!("expected log failure"),
        Err(message) => message,
    };
    assert!(
        err.starts_with(
            "Command failed with error #128!\nCOMMAND\ngit log HEAD --not zzz-no-such-rev"
        ),
        "{err}"
    );
    assert!(!is_usage_message(&err));
}

#[test]
fn w6_matching_renders_other_authors() {
    // `ArcanistWhichWorkflow.php:228` at ceb: other authors render
    // `    D9 (name) title`.
    let mut revision = matched("D1");
    revision.author = Some("bob".to_string());
    let text = render_matching(&[revision], "", false);
    assert!(text.contains("    D1 (bob) title\n"), "{text}");
}

#[test]
fn w6_json_carries_matching_revisions() {
    // `--json` is an arc-rs extension: each match carries its
    // monogram, title, author, and reason.
    let mut revision = matched("D7");
    revision.author = Some("bob".to_string());
    let ctx = ansi_context(vec![revision]);
    assert_eq!(
        report_json(&ctx)["matching"],
        serde_json::json!([{
            "revision": "D7",
            "title": "title",
            "author": "bob",
            "why": "reason",
        }])
    );
}

// -( T1170 coverage: conduit labels, engine paths, matching, runners )--------

#[test]
fn t1170_broken_arcconfig_has_no_conduit() {
    // A project file that fails to parse fails the merged load, so
    // conduit resolution reports none rather than erroring.
    let repo = fresh_repo("t1170broken");
    std::fs::write(repo.path.join(".arcconfig"), "{oops").unwrap();
    assert_eq!(resolve_conduit_in(&repo.path, None, None), (None, "none"));
}

#[test]
fn t1170_non_string_conduit_uri_has_no_conduit() {
    // A present-but-typeless `phabricator.uri` reads as missing.
    let repo = fresh_repo("t1170typeless");
    std::fs::write(repo.path.join(".arcconfig"), r#"{"phabricator.uri": 42}"#).unwrap();
    assert_eq!(resolve_conduit_in(&repo.path, None, None), (None, "none"));
}

#[test]
fn t1170_source_labels_cover_local_and_default() {
    // A local hit outside any working copy, plus unknown sources.
    let bare = TempDir::new("t1170labels");
    assert_eq!(
        merged_source_label(get_config::SOURCE_LOCAL, &bare.path),
        "local"
    );
    assert_eq!(merged_source_label("bogus-source", &bare.path), "default");
}

#[test]
#[cfg(not(windows))]
fn t1170_system_label_names_etc_arcconfig() {
    let bare = TempDir::new("t1170syslabel");
    assert_eq!(
        merged_source_label(get_config::SOURCE_SYSTEM, &bare.path),
        "system (/etc/arcconfig)"
    );
}

#[test]
#[cfg(windows)]
fn t1170_system_label_names_programdata() {
    let bare = TempDir::new("t1170syslabel");
    assert_eq!(
        merged_source_label(get_config::SOURCE_SYSTEM, &bare.path),
        "system (ProgramData\\Phabricator\\Arcanist\\config)"
    );
}

#[test]
fn t1170_svn_without_arcconfig_has_no_project() {
    // Subversion walks to the root and gives up when no `.arcconfig`
    // turns up along the way.
    let tmp = TempDir::new("t1170svnmiss");
    let child = tmp.path.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(tmp.path.join(".svn")).unwrap();
    assert_eq!(find_project(&child), (None, None));
}

#[test]
fn t1170_rev_to_commit_misses_unknown_revs() {
    let repo = fresh_repo("t1170rev");
    commit(&repo.path, "a.txt");
    assert_eq!(rev_to_commit(&repo.path, "no-such-rev"), None);
}

#[test]
fn t1170_base_sources_empty_reads_blank() {
    let empty = BaseSources::empty();
    assert!(empty.local.is_empty());
    assert!(empty.project.is_empty());
    assert!(empty.user.is_empty());
    assert!(empty.system.is_empty());
}

#[test]
fn t1170_engine_without_rulesets_matches_nothing() {
    // A ruleset list that omits a retried source skips it instead of
    // failing: no rules anywhere means no match.
    let repo = fresh_repo("t1170norules");
    commit(&repo.path, "a.txt");
    let mut engine = Engine::new(&repo.path, None, &never_prompt);
    assert_eq!(engine.resolve(&[]).unwrap(), None);
}

#[test]
fn t1170_engine_rejects_colonless_rules_directly() {
    // `tokenize_base_rules` normally screens these, but the engine
    // reports the same shape when handed one directly.
    let repo = fresh_repo("t1170colonless");
    commit(&repo.path, "a.txt");
    let mut engine = Engine::new(&repo.path, None, &never_prompt);
    let all = [SourceRules {
        name: "runtime",
        rules: std::collections::VecDeque::from(["arcupstream".to_string()]),
    }];
    assert_eq!(
        engine.resolve(&all).unwrap_err(),
        "Rule 'arcupstream' is invalid, it must have a type and name like 'arc:upstream'."
    );
}

#[test]
fn t1170_arc_legacy_names_switch_sources() {
    // `arc:global`/`arc:args` translate to `user`/`runtime` before
    // switching rulesets.
    let repo = fresh_repo("t1170legacy");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let merged = merged_with("base", serde_json::json!("literal:from-user"), "user");
    let inputs = base_inputs("arc:global, literal:unreached", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &merged, &inputs).unwrap();
    assert_eq!(base, "from-user");

    let inputs = base_inputs("arc:args, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_arc_verbose_resolves_and_logs() {
    // `arc:verbose` enables engine logging mid-stream; resolution
    // continues with the next rule.
    let repo = fresh_repo("t1170verbose");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs("arc:verbose, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_arc_prompt_reports_non_tty() {
    // A prompt answered without a TTY throws instead of resolving.
    let repo = fresh_repo("t1170prompttty");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    assert_eq!(prompt_line("Against which commit?"), None);
    let inputs = base_inputs("arc:prompt", &|_: &str| Some("typed".to_string()));
    assert_eq!(
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    assert!(!take_prompt_tty_failed());
}

#[test]
fn t1170_arc_source_and_flow_rules_cover_switches() {
    let repo = fresh_repo("t1170flows");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    // `arc:runtime` re-enters the runtime ruleset.
    let inputs = base_inputs("arc:runtime, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
    // `arc:system` switches to the system ruleset.
    let merged = merged_with("base", serde_json::json!("literal:from-system"), "system");
    let inputs = base_inputs("arc:system, literal:unreached", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &merged, &inputs).unwrap();
    assert_eq!(base, "from-system");
    // `arc:yield` moves the current source to the back; the runtime
    // queue resumes once the others run dry.
    let inputs = base_inputs("arc:yield, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_arc_nodiff_and_this_rules() {
    let repo = fresh_repo("t1170nothis");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    // `arc:nodiff(X)` resolves through the git rules, which never
    // match the wrapped name, so it falls through.
    let inputs = base_inputs("arc:nodiff(empty), literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
    // `arc:this` pins the parent of HEAD.
    let inputs = base_inputs("arc:this", &never_prompt);
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "HEAD^");
    assert!(why.contains("arc:this"), "{why}");
}

fn commit_with_message(dir: &Path, name: &str, subject: &str, body: &str) {
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", subject, "-m", body]);
}

#[test]
fn t1170_arc_amended_detects_revision_footers() {
    // HEAD amended with a footer resolves to its parent; without one
    // the rule falls through; a malformed footer errors.
    let repo = fresh_repo("t1170amended");
    commit(&repo.path, "a.txt");
    commit_with_message(&repo.path, "b.txt", "second", "Differential Revision: D1");
    let inputs = base_inputs("arc:amended", &never_prompt);
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "HEAD^");
    assert!(why.contains("amended"), "{why}");

    let plain = fresh_repo("t1170unamended");
    commit(&plain.path, "a.txt");
    commit(&plain.path, "b.txt");
    let inputs = base_inputs("arc:amended, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&plain.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");

    let broken = fresh_repo("t1170badamended");
    commit(&broken.path, "a.txt");
    commit_with_message(
        &broken.path,
        "b.txt",
        "second",
        "Differential Revision: nope",
    );
    let inputs = base_inputs("arc:amended", &never_prompt);
    let err =
        resolve_base(&broken.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err();
    assert!(
        err.contains("Invalid \"Differential Revision\" field"),
        "{err}"
    );
}

#[test]
fn t1170_arc_upstream_falls_through_without_upstream() {
    let repo = fresh_repo("t1170noupstream");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs("arc:upstream, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_arc_upstream_resolves_merge_base() {
    let repo = fresh_repo("t1170upstream");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let origin = TempDir::new("t1170uporigin");
    git(&origin.path, &["init", "-q", "--bare"]);
    git(
        &repo.path,
        &["remote", "add", "origin", &origin.path.to_string_lossy()],
    );
    git(&repo.path, &["push", "-q", "-u", "origin", "master"]);
    commit(&repo.path, "c.txt");
    let inputs = base_inputs("arc:upstream", &never_prompt);
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("merge-base"), "{why}");
}

#[test]
fn t1170_git_merge_base_failure_falls_through() {
    let repo = fresh_repo("t1170mbfail");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs("git:merge-base(no-such-ref), literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_branch_unique_needs_resolving_refs() {
    // An unresolving inner ref fails `merge-base` and falls through.
    let repo = fresh_repo("t1170bunoref");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs(
        "git:branch-unique(no-such-ref), literal:kept",
        &never_prompt,
    );
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_branch_unique_empty_range_matches_nothing() {
    // The merge-base is HEAD itself, so the range — and the match —
    // is empty.
    let repo = fresh_repo("t1170buempty");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs("git:branch-unique(HEAD)", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");
}

#[test]
fn t1170_branch_unique_log_failure_matches_nothing() {
    // `git log` can fail while `merge-base` succeeds (here via a bogus
    // repo-local date format); the rule falls through.
    let repo = fresh_repo("t1170bulogfail");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    git(&repo.path, &["config", "log.date", "BOGUS"]);
    let inputs = base_inputs("git:branch-unique(HEAD), literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");
}

#[test]
fn t1170_branch_unique_finds_fork_point() {
    // `feature` forks from the first commit; the fork point sits on
    // more branches than HEAD, so it resolves with its explanation.
    let repo = fresh_repo("t1170bufork");
    commit(&repo.path, "a.txt");
    git(&repo.path, &["branch", "feature"]);
    commit(&repo.path, "b.txt");
    git(&repo.path, &["checkout", "-q", "feature"]);
    commit(&repo.path, "c.txt");
    git(&repo.path, &["checkout", "-q", "master"]);
    let fork = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let inputs = base_inputs("git:branch-unique(feature)", &never_prompt);
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, fork);
    assert!(why.contains("merge-base"), "{why}");
    assert!(why.contains("feature"), "{why}");
}

#[test]
fn t1170_branch_unique_single_branch_matches_nothing() {
    // Every commit sits on the same branches as HEAD, so no commit
    // outranks it and the walk runs dry.
    let repo = fresh_repo("t1170busingle");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs("git:branch-unique(HEAD~1)", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");
}

#[test]
fn t1170_branch_helpers_list_and_fail_leniently() {
    let repo = fresh_repo("t1170brhelp");
    commit(&repo.path, "a.txt");
    git(&repo.path, &["branch", "feature"]);
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let mut branches = local_branch_names(&repo.path);
    branches.sort();
    assert_eq!(branches, ["feature", "master"]);
    let mut contained = branches_containing(&repo.path, &head, &branches);
    contained.sort();
    assert_eq!(contained, ["feature", "master"]);
    // Outside a repository the helpers report emptiness, never errors.
    let plain = TempDir::new("t1170brplain");
    assert!(local_branch_names(&plain.path).is_empty());
    assert!(branches_containing(&plain.path, &head, &branches).is_empty());
    // Unspawnable git (a missing directory) reads the same way.
    let gone = TempDir::new("t1170brgone");
    let missing = gone.path.join("nope");
    assert!(local_branch_names(&missing).is_empty());
    assert!(branches_containing(&missing, &head, &branches).is_empty());
}

#[test]
fn t1170_explicit_base_names_an_explicit_head() {
    let repo = fresh_repo("t1170exphead");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(
        &repo.path,
        Some("HEAD~1"),
        "HEAD",
        true,
        &empty_loaded(),
        &inputs,
    )
    .unwrap();
    assert_eq!(base, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("explicitly specified head commit"), "{why}");
}

#[test]
fn t1170_default_chain_prompts_persists_and_reuses_scratch() {
    // No upstream anywhere: the prompt answer persists to the scratch
    // file, and the next resolution reuses it without prompting.
    let repo = fresh_repo("t1170scratch");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let want = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let scratch = repo.path.join(".git/arc/default-relative-commit");
    let inputs = base_inputs("", &|_: &str| Some("HEAD~1".to_string()));
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, want);
    assert_eq!(std::fs::read_to_string(&scratch).unwrap(), "HEAD~1");
    let inputs = base_inputs("", &never_prompt);
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, want);
    assert!(why.contains("default-relative-commit"), "{why}");
    // With the scratch file gone the prompt default fails like PHP.
    std::fs::remove_file(&scratch).unwrap();
    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err();
    assert_eq!(
        err,
        "Relative commit 'origin/master' is not the name of a commit!"
    );
}

#[test]
fn t1170_default_chain_merge_base_failure_errors() {
    // The upstream resolves but shares no history, so the default
    // merge-base fails with the command text.
    let repo = fresh_repo("t1170defmb");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    git(&repo.path, &["checkout", "-q", "--orphan", "other"]);
    commit(&repo.path, "b.txt");
    git(&repo.path, &["checkout", "-q", "master"]);
    git(&repo.path, &["config", "branch.master.remote", "."]);
    git(
        &repo.path,
        &["config", "branch.master.merge", "refs/heads/other"],
    );
    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap_err();
    assert!(err.starts_with("git merge-base failed:"), "{err}");
}

#[test]
fn t1170_scratch_file_resolves_absolute_git_dirs() {
    // Linked worktrees report an absolute `--git-dir`; scratch reads
    // and writes follow it.
    let repo = fresh_repo("t1170worktree");
    commit(&repo.path, "a.txt");
    let wt = TempDir::new("t1170wt");
    let linked = wt.path.join("linked");
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "--detach",
            &linked.to_string_lossy(),
            "HEAD",
        ],
    );
    assert_eq!(read_scratch_file(&linked), "");
    write_scratch_file(&linked, "HEAD~1");
    assert_eq!(read_scratch_file(&linked), "HEAD~1");
}

#[test]
fn t1170_write_scratch_ignores_missing_git_dir() {
    // Outside a repository there is no scratch location; the write
    // is a silent no-op.
    let plain = TempDir::new("t1170writemiss");
    write_scratch_file(&plain.path, "HEAD~1");
    assert_eq!(read_scratch_file(&plain.path), "");
}

#[test]
fn t1170_prompt_line_reads_tty_answers() {
    // With a TTY stdin the prompt reads one answer line (EOF under
    // test capture answers nothing).
    assert_eq!(prompt_line_with_tty("Q?", true), None);
}

#[test]
fn t1170_commit_records_skip_short_rows() {
    assert!(parse_commit_records("").is_empty());
    assert!(parse_commit_records("aaa\x01bbb").is_empty());
    let one = "c1\x01t1\x01p\x01 0\x01a\x01e\x01subj\x01msg\n\nbody";
    let parsed = parse_commit_records(one);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].commit, "c1");
    assert_eq!(parsed[0].tree, "t1");
    assert_eq!(parsed[0].summary, "subj");
    assert_eq!(parsed[0].message, "msg\n\nbody");
}

#[test]
fn t1170_commit_messages_report_log_failures() {
    let repo = fresh_repo("t1170msgfail");
    commit(&repo.path, "a.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let err = git_commit_messages(&repo.path, &head, "no-such-rev").unwrap_err();
    assert!(err.starts_with("git log failed:"), "{err}");
}

#[test]
fn t1170_huge_revision_ids_stay_strings() {
    // A footer id overflowing u64 keeps its digit text instead of
    // becoming a number.
    let repo = fresh_repo("t1170hugeid");
    commit(&repo.path, "a.txt");
    let huge = "9".repeat(30);
    commit_with_message(
        &repo.path,
        "b.txt",
        "second",
        &format!("Differential Revision: D{huge}"),
    );
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let parent = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let messages = git_commit_messages(&repo.path, &parent, &head).unwrap();
    let (ids, _) = collect_explicit_revision_ids(&messages).unwrap();
    assert_eq!(ids, vec![serde_json::Value::String(huge)]);
}

#[test]
fn t1170_matching_reports_conduit_failures() {
    // A failing `differential.query` propagates its conduit text.
    let repo = fresh_repo("t1170matchfail");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![
        "{\"error_code\": \"ERR-BAD\", \"error_info\": \"nope\"}",
    ]);
    let client = mock_client(&base);
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let err =
        match_working_copy_revisions(&client, &repo.path, GIT_MAGIC_ROOT_COMMIT, &head, false)
            .unwrap_err();
    assert_eq!(err, "ERR-BAD: nope");
}

fn t1170_footer_repo(tag: &str) -> TempDir {
    let repo = fresh_repo(tag);
    commit(&repo.path, "a.txt");
    commit_with_message(&repo.path, "b.txt", "second", "Differential Revision: D77");
    commit_with_message(&repo.path, "c.txt", "third", "Differential Revision: D78");
    repo
}

#[test]
fn t1170_matching_queries_explicit_ids_and_whoami() {
    // Footer ids query by id; without a cached viewer the flow asks
    // `user.whoami` once, then resolves only the other authors.
    let repo = t1170_footer_repo("t1170byids");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let parent = rev_to_commit(&repo.path, "HEAD~2").unwrap();
    let (base, seen) = serve_script(vec![
        "{\"error_code\": null, \"result\": [{\"id\": 77, \"title\": \"Mine\", \
         \"authorPHID\": \"PHID-USER-1\"}, {\"id\": 78, \"title\": \"Theirs\", \
         \"authorPHID\": \"PHID-USER-9\"}]}",
        "{\"error_code\": null, \"result\": {\"userName\": \"alice\", \"phid\": \"PHID-USER-1\"}}",
        "{\"error_code\": null, \"result\": [{\"phid\": \"PHID-USER-9\", \
         \"userName\": \"bob\"}]}",
    ]);
    let client = mock_client(&base);
    let matched = match_working_copy_revisions(&client, &repo.path, &parent, &head, false).unwrap();
    assert_eq!(matched.len(), 2);
    assert_eq!(matched[0].monogram, "D77");
    assert_eq!(matched[0].author, None);
    assert!(
        matched[0].why.contains("explicit 'Differential Revision'"),
        "{}",
        matched[0].why
    );
    assert_eq!(matched[1].monogram, "D78");
    assert_eq!(matched[1].author.as_deref(), Some("bob"));
    let (request, body) = seen.recv().unwrap();
    assert!(request.contains("differential.query"), "{request}");
    assert!(body.contains("status-open"), "{body}");
    let (request, _) = seen.recv().unwrap();
    assert!(request.contains("user.whoami"), "{request}");
    let (request, _) = seen.recv().unwrap();
    assert!(request.contains("user.query"), "{request}");
}

#[test]
fn t1170_matching_any_status_queries_all_revisions() {
    let repo = t1170_footer_repo("t1170anystatus");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let parent = rev_to_commit(&repo.path, "HEAD~2").unwrap();
    let (base, seen) = serve_script(vec![
        "{\"error_code\": null, \"result\": []}",
        "{\"error_code\": null, \"result\": {\"userName\": \"alice\", \"phid\": \"PHID-USER-1\"}}",
    ]);
    let client = mock_client(&base);
    let matched = match_working_copy_revisions(&client, &repo.path, &parent, &head, true).unwrap();
    assert!(matched.is_empty());
    let (_, body) = seen.recv().unwrap();
    assert!(body.contains("status-any"), "{body}");
}

#[test]
fn t1170_matching_empty_range_matches_nothing() {
    let repo = fresh_repo("t1170nomatch");
    commit(&repo.path, "a.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let (base, seen) = serve_script(vec![]);
    let client = mock_client(&base);
    assert!(
        match_working_copy_revisions(&client, &repo.path, &head, &head, false)
            .unwrap()
            .is_empty()
    );
    assert!(seen.try_recv().is_err());
}

#[test]
fn t1170_repo_identity_covers_missing_and_bad_heads() {
    // Outside a repository every field reads empty; a bad head ref
    // drops the head pair while keeping the rest.
    let plain = TempDir::new("t1170noident");
    let vacant = load_repo_identity(&plain.path, "HEAD", false);
    assert_eq!(vacant.vcs_root, None);
    assert_eq!(vacant.branch, None);
    assert_eq!(vacant.head, None);
    assert_eq!(vacant.head_subject, None);
    assert_eq!(vacant.remote_url, None);

    let repo = fresh_repo("t1170badhead");
    commit(&repo.path, "a.txt");
    let bad = load_repo_identity(&repo.path, "no-such-rev", true);
    assert_eq!(bad.head, None);
    assert_eq!(bad.head_subject, None);
    assert!(bad.vcs_root.is_some());
    let good = load_repo_identity(&repo.path, "HEAD", true);
    assert!(good.head.is_some());
    assert_eq!(good.head_subject.as_deref(), Some("a.txt"));
}

#[test]
fn t1170_explicit_head_checks_range_direction() {
    let repo = fresh_repo("t1170headdirs");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let range = load_range_data(
        &repo.path,
        &empty_loaded(),
        Some("HEAD~1"),
        "HEAD",
        true,
        "",
        &never_prompt,
    )
    .unwrap();
    assert_eq!(range.commits.len(), 1);
    let err = load_range_data(
        &repo.path,
        &empty_loaded(),
        Some("HEAD"),
        "HEAD~1",
        true,
        "",
        &never_prompt,
    )
    .unwrap_err();
    assert!(err.contains("empty, backward or invalid"), "{err}");
}

#[test]
fn t1170_context_range_error_after_association() {
    // Association succeeds, then the bad explicit base fails the
    // context load.
    let repo = fresh_repo("t1170ctxerr");
    commit(&repo.path, "a.txt");
    let (base, _seen) = serve_script(vec![UNIQUE_REPO]);
    let client = authed_client(&base);
    let inputs = base_inputs("", &never_prompt);
    let err = load_context(
        &repo.path,
        None,
        None,
        None,
        Some("no-such-rev"),
        "HEAD",
        false,
        &callsign_merged("X"),
        &inputs,
        &client,
        false,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Unable to find any git commit named 'no-such-rev' in this repository."
    );
}

#[test]
fn t1170_render_uses_ambient_ansi() {
    // `render` matches `render_ansi` under the process setting.
    let ctx = ansi_context(vec![matched("D1")]);
    assert_eq!(render(&ctx), render_ansi(&ctx, terminal::ansi_enabled()));
    assert!(render(&ctx).contains("MATCHING REVISIONS"));
}

#[test]
fn t1170_conduit_uri_takes_a_separate_value() {
    match parse_args(&[
        "--conduit-uri".to_string(),
        "https://h.test/api/".to_string(),
    ]) {
        Parsed::Options(opts) => {
            assert_eq!(opts.conduit_uri.as_deref(), Some("https://h.test/api/"))
        }
        _ => panic!("expected --conduit-uri options"),
    }
}

fn t1170_arcrc_with_token(path: &Path, mock_base: &str) {
    // Mirror `config::host_key` for a loopback mock base (always
    // `http://127.0.0.1:PORT/` with a trailing slash).
    let key = format!("{mock_base}api/");
    let mut hosts = serde_json::Map::new();
    hosts.insert(key, serde_json::json!({"token": "t"}));
    let doc = serde_json::json!({"hosts": hosts});
    std::fs::write(path, serde_json::to_string(&doc).unwrap()).unwrap();
}

fn t1170_authed_repo(tag: &str) -> TempDir {
    // One commit plus a callsign: `run_in` loads the base without
    // prompting and associates without a remote.
    let repo = fresh_repo(tag);
    commit(&repo.path, "a.txt");
    std::fs::write(
        repo.path.join(".arcconfig"),
        r#"{"repository.callsign": "X"}"#,
    )
    .unwrap();
    repo
}

const T1170_WHOAMI: &str =
    "{\"error_code\": null, \"result\": {\"userName\": \"alice\", \"phid\": \"PHID-USER-1\"}}";
const T1170_NO_MATCH: &str = "{\"error_code\": null, \"result\": []}";

#[test]
fn t1170_run_in_rejects_plain_directories() {
    // The working-copy gate fires before any conduit setup.
    let plain = TempDir::new("t1170nogate");
    let options = options_of(&[]);
    let opts = get_config::LoadOptions::default();
    assert_eq!(
        run_in(&plain.path, &options, None, &opts),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn t1170_run_in_text_authenticates_and_reports() {
    let repo = t1170_authed_repo("t1170runtext");
    let (base, _seen) = serve_script(vec![T1170_WHOAMI, UNIQUE_REPO, T1170_NO_MATCH]);
    let user_file = repo.path.join("t1170.arcrc");
    t1170_arcrc_with_token(&user_file, &base);
    let options = options_of(&["--conduit-uri", base.as_str()]);
    let opts = get_config::LoadOptions::default();
    assert_eq!(
        run_in(&repo.path, &options, Some(user_file.as_path()), &opts),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn t1170_run_in_json_reports_the_whole_context() {
    let repo = t1170_authed_repo("t1170runjson");
    let (base, _seen) = serve_script(vec![T1170_WHOAMI, UNIQUE_REPO, T1170_NO_MATCH]);
    let user_file = repo.path.join("t1170.arcrc");
    t1170_arcrc_with_token(&user_file, &base);
    let options = options_of(&["--conduit-uri", base.as_str(), "--json"]);
    let opts = get_config::LoadOptions::default();
    assert_eq!(
        run_in(&repo.path, &options, Some(user_file.as_path()), &opts),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn t1170_run_in_show_base_prints_without_matching() {
    let repo = t1170_authed_repo("t1170runshow");
    let (base, seen) = serve_script(vec![T1170_WHOAMI]);
    let user_file = repo.path.join("t1170.arcrc");
    t1170_arcrc_with_token(&user_file, &base);
    let options = options_of(&["--conduit-uri", base.as_str(), "--show-base"]);
    let opts = get_config::LoadOptions::default();
    assert_eq!(
        run_in(&repo.path, &options, Some(user_file.as_path()), &opts),
        std::process::ExitCode::SUCCESS
    );
    // Auth only: `--show-base` never reaches conduit again.
    let (request, _) = seen.recv().unwrap();
    assert!(request.contains("user.whoami"), "{request}");
    assert!(seen.try_recv().is_err());
}

#[test]
fn t1170_run_in_reports_unreadable_config() {
    // Authentication succeeds, then the broken project file fails
    // the merged load as a generic exception.
    let repo = t1170_authed_repo("t1170runbroken");
    std::fs::write(repo.path.join(".arcconfig"), "{oops").unwrap();
    let (base, _seen) = serve_script(vec![T1170_WHOAMI]);
    let user_file = repo.path.join("t1170.arcrc");
    t1170_arcrc_with_token(&user_file, &base);
    let options = options_of(&["--conduit-uri", base.as_str()]);
    let opts = get_config::LoadOptions::default();
    assert_eq!(
        run_in(&repo.path, &options, Some(user_file.as_path()), &opts),
        std::process::ExitCode::FAILURE
    );
}
