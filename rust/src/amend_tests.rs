use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn empty_options() -> Options {
    Options {
        messages: Vec::new(),
        show: false,
        revision: None,
        conduit_uri: None,
    }
}

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-amend-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

// Unix-only: the sole caller chain serves the chmod-needing hook test below.
#[cfg(unix)]
fn git(dir: &std::path::Path, argv: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(argv)
        .output()
        .unwrap();
    let detail = format!(
        "git {} failed: {}",
        argv.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{detail}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[cfg(unix)]
fn amend_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init", "-q", "-b", "master"]);
    git(tmp.path(), &["config", "user.email", "amend@test"]);
    git(tmp.path(), &["config", "user.name", "amend"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    std::fs::write(tmp.path().join("file.txt"), "base\n").unwrap();
    git(tmp.path(), &["add", "file.txt"]);
    git(tmp.path(), &["commit", "-qm", "initial commit"]);
    tmp
}

/// Serve `expected` requests from `routes` (method -> envelope
/// result JSON); returns the base URI. (Local replicate of the
/// `export_tests` stub.)
fn serve_amend_routes(
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
fn parses_defaults() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&[]), true),
        Ok(Some(empty_options()))
    );
}

#[test]
fn parses_message_forms() {
    // T1153 B21/B23: TTY runs need no `--`.
    let mut expected = empty_options();
    expected.messages = vec!["one".to_string()];
    assert_eq!(
        parse_options_with_tty(&args(&["-m", "one"]), true),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--message", "one"]), true),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--message=one"]), true),
        Ok(Some(expected.clone()))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["-mone"]), true),
        Ok(Some(expected))
    );
}

#[test]
fn messages_are_repeatable() {
    // T1153 B21/B23: TTY runs need no `--`.
    let options = parse_options_with_tty(&args(&["-m", "one", "-m", "two"]), true)
        .unwrap()
        .unwrap();
    assert_eq!(options.messages, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn parses_revision_and_conduit_uri() {
    // T1153 B21/B23: TTY runs need no `--`.
    let options = parse_options_with_tty(&args(&["--revision", "D123"]), true)
        .unwrap()
        .unwrap();
    assert_eq!(options.revision, Some("D123".to_string()));
    let options = parse_options_with_tty(&args(&["--revision=123"]), true)
        .unwrap()
        .unwrap();
    assert_eq!(options.revision, Some("123".to_string()));
    let options = parse_options_with_tty(&args(&["--conduit-uri", "http://x/"]), true)
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
}

#[test]
fn help_returns_none() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(parse_options_with_tty(&args(&["--help"]), true), Ok(None));
    assert_eq!(parse_options_with_tty(&args(&["-h"]), true), Ok(None));
}

#[test]
fn rejects_bad_arguments() {
    // T1153 B21/B23: TTY runs need no `--`; a lone `--` is the
    // terminator (accepted), and `--help` after `--` is positional.
    let err = parse_options_with_tty(&args(&["--bogus"]), true).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    // Amend rewrites the message only; staging flags are not accepted.
    let err = parse_options_with_tty(&args(&["--all"]), true).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_options_with_tty(&args(&["file.txt"]), true).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), true)
            .unwrap()
            .is_some()
    );
    let err = parse_options_with_tty(&args(&["--", "--help"]), true).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(parse_options_with_tty(&args(&["-m"]), true).is_err());
    assert!(parse_options_with_tty(&args(&["--revision"]), true).is_err());
    assert!(parse_options_with_tty(&args(&["--conduit-uri"]), true).is_err());
}

#[test]
fn parses_revision_ids() {
    assert_eq!(parse_revision_id("123"), Ok(123));
    assert_eq!(parse_revision_id("D123"), Ok(123));
    assert_eq!(parse_revision_id("d7"), Ok(7));
    for bad in ["", "D", "D0", "0", "abc", "D12x", "-3"] {
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
    assert!(normalize_message("   \n").is_err());
}

#[test]
fn usage_covers_options_and_limits() {
    // T1153 B21/B23: `--help` renders the generated workflow help
    // (extensions `-m`/`--message`/`--conduit-uri` stay undocumented).
    let text = usage_text(false);
    assert!(text.contains("AMEND WORKFLOW"), "{text}");
    assert!(
        text.contains("--revision") && text.contains("--show"),
        "{text}"
    );
    assert!(text.contains("Mercurial 2.2 and newer"), "{text}");
    assert!(!text.contains("--message"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn b21_amend_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv. Unknown flags still report first.
    assert_eq!(
        parse_options_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--show"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    let err = parse_options_with_tty(&args(&["--bogus"]), false).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(
        parse_options_with_tty(&args(&["--show", "--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(parse_options_with_tty(&args(&[]), true).unwrap().is_some());
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn b23_amend_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help amend`.
    let text = usage_text(false);
    assert!(text.starts_with("AMEND WORKFLOW\n"), "{text}");
    assert!(
        text.contains("synchronizing the local commit message"),
        "{text}"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

fn revision(id: &str, title: &str) -> close_revision::Revision {
    close_revision::Revision {
        id: id.to_string(),
        title: title.to_string(),
        status: close_revision::STATUS_ACCEPTED,
        status_name: "Accepted".to_string(),
        author_phid: "PHID-USER-1".to_string(),
        commits: Vec::new(),
    }
}

#[test]
fn parses_ids_from_messages() {
    assert_eq!(
        revision_id_in_message("Title\n\nDifferential Revision: D123\n"),
        Ok(Some(123))
    );
    assert_eq!(revision_id_in_message("no trailer here\n"), Ok(None));
    assert!(revision_id_in_message("T\nDifferential Revision: bogus\n").is_err());
}

#[test]
fn rows_of_covers_result_shapes() {
    let array = serde_json::json!([{"id": 1}, {"id": 2}]);
    assert_eq!(rows_of(&array).len(), 2);
    let search = serde_json::json!({"data": [{"id": 3}], "cursor": {}});
    assert_eq!(rows_of(&search), vec![serde_json::json!({"id": 3})]);
    let map = serde_json::json!({"b": {"id": 2}, "a": {"id": 1}});
    assert_eq!(
        rows_of(&map),
        vec![serde_json::json!({"id": 1}), serde_json::json!({"id": 2})]
    );
    assert!(rows_of(&serde_json::json!(null)).is_empty());
    assert!(rows_of(&serde_json::json!("x")).is_empty());
}

#[test]
fn parses_revisions_skipping_idless_rows() {
    let rows = vec![
        serde_json::json!({"id": 7, "title": "Seven"}),
        serde_json::json!({"title": "no id"}),
    ];
    let parsed = parse_revisions(&rows);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "7");
    assert_eq!(parsed[0].title, "Seven");
}

#[test]
fn renders_ref_view_lines() {
    assert_eq!(ref_view_line("D123", "Title"), "**  *  ** D123 Title\n");
    assert_eq!(ref_view_line("@alice", ""), "**  *  ** @alice\n");
    assert_eq!(
        ref_view_line("@alice", "(Alice)"),
        "**  *  ** @alice (Alice)\n"
    );
}

#[test]
fn revision_numeric_id_rejects_garbage() {
    assert_eq!(revision_numeric_id(&revision("123", "T")), Ok(123));
    assert!(revision_numeric_id(&revision("abc", "T")).is_err());
}

#[test]
fn select_guessed_needs_exactly_one() {
    let mut io = land::Io::buffered();
    let err = select_guessed(&mut io, Vec::new()).unwrap_err();
    assert!(err.contains("no revisions found"), "unexpected: {err}");
    assert!(err.contains("Use \"--revision <id>\""), "unexpected: {err}");

    let mut io = land::Io::buffered();
    let err =
        select_guessed(&mut io, vec![revision("1", "One"), revision("2", "Two")]).unwrap_err();
    assert_eq!(
        err,
        "Use \"--revision <id>\" to specify which revision you want to amend."
    );
    assert!(
        io.out.stdout.contains("MULTIPLE REVISIONS IN WORKING COPY"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("**  *  ** D1 One\n"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("**  *  ** D2 Two\n"),
        "unexpected: {}",
        io.out.stdout
    );

    let mut io = land::Io::buffered();
    let only = select_guessed(&mut io, vec![revision("9", "Nine")]).unwrap();
    assert_eq!(only.id, "9");
    assert!(io.out.stdout.is_empty());
}

#[test]
fn immutable_config_beats_default() {
    let empty = get_config::Sources {
        local: serde_json::Map::new(),
        project: serde_json::Map::new(),
        user: serde_json::Map::new(),
        system: serde_json::Map::new(),
        default: get_config::default_config(),
        project_file: None,
    };
    assert!(!history_is_immutable_in(&empty, false));
    assert!(history_is_immutable_in(&empty, true));

    let mut project = serde_json::Map::new();
    project.insert("history.immutable".to_string(), serde_json::json!(true));
    let locked = get_config::Sources {
        local: serde_json::Map::new(),
        project,
        user: serde_json::Map::new(),
        system: serde_json::Map::new(),
        default: get_config::default_config(),
        project_file: None,
    };
    assert!(history_is_immutable_in(&locked, false));

    let mut project = serde_json::Map::new();
    project.insert(
        "history.immutable".to_string(),
        serde_json::json!("yes, really"),
    );
    let fuzzy = get_config::Sources {
        local: serde_json::Map::new(),
        project,
        user: serde_json::Map::new(),
        system: serde_json::Map::new(),
        default: get_config::default_config(),
        project_file: None,
    };
    assert!(!history_is_immutable_in(&fuzzy, false));
}

#[test]
fn immutable_prompt_keys_match_php() {
    assert_eq!(PROMPT_UNRELATED, "arc.amend.unrelated");
    assert_eq!(PROMPT_AUTHOR, "arc.amend.author");
    assert_eq!(PROMPT_IMMUTABLE, "arc.amend.immutable");
}

#[test]
fn ref_view_line_ansi_modes() {
    assert_eq!(
        ref_view_line_ansi("D123", "Title", false),
        "**  *  ** D123 Title\n"
    );
    assert_eq!(
        ref_view_line_ansi("@alice", "", false),
        "**  *  ** @alice\n"
    );
    assert_eq!(
        ref_view_line_ansi("D123", "Title", true),
        "\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1mD123\x1b[m Title\n"
    );
    assert_eq!(
        ref_view_line_ansi("@alice", "", true),
        "\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1m@alice\x1b[m\n"
    );
}

#[test]
fn ref_view_line_tty_truncates_long_titles() {
    let long = "x".repeat(100);
    assert!(ref_view_line_ansi("D123", &long, false).contains(&long));
    assert_eq!(
        ref_view_line_ansi("D123", &long, true),
        format!(
            "\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1mD123\x1b[m {}\u{2026}\n",
            "x".repeat(68)
        )
    );
}

#[test]
fn select_guessed_ansi_lists_candidates() {
    let mut io = land::Io::buffered();
    let err = select_guessed_ansi(
        &mut io,
        vec![revision("1", "One"), revision("2", "Two")],
        false,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Use \"--revision <id>\" to specify which revision you want to amend."
    );
    assert!(io.out.stdout.contains("**  *  ** D1 One\n"));
    assert!(io.out.stdout.contains("**  *  ** D2 Two\n"));

    let mut io = land::Io::buffered();
    select_guessed_ansi(
        &mut io,
        vec![revision("1", "One"), revision("2", "Two")],
        true,
    )
    .unwrap_err();
    assert!(
        io.out
            .stdout
            .contains("\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1mD1\x1b[m One\n"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1mD2\x1b[m Two\n"),
        "unexpected: {}",
        io.out.stdout
    );

    let mut io = land::Io::buffered();
    let only = select_guessed_ansi(&mut io, vec![revision("9", "Nine")], true).unwrap();
    assert_eq!(only.id, "9");
    assert!(io.out.stdout.is_empty());
}

#[test]
fn announce_text_modes() {
    let rev = revision("123", "Fix it");
    assert_eq!(
        announce_text(&rev, false),
        "Amending commit message to reflect revision:\n\n**  *  ** D123 Fix it\n\n"
    );
    assert_eq!(
        announce_text(&rev, true),
        "Amending commit message to reflect revision:\n\n\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1mD123\x1b[m Fix it\n\n"
    );
}
#[test]
fn rejects_empty_inline_flag_values() {
    // T1153 B21/B23: TTY runs need no `--`.
    for flag in ["--message=", "--revision=", "--conduit-uri="] {
        let err = parse_options_with_tty(&args(&[flag]), true).unwrap_err();
        assert!(
            err.contains("requires a non-empty value"),
            "unexpected: {err}"
        );
    }
    let options = parse_options_with_tty(&args(&["--conduit-uri=http://x/"]), true)
        .unwrap()
        .unwrap();
    assert_eq!(options.conduit_uri, Some("http://x/".to_string()));
}

fn tail_error(arg: &str) -> String {
    let Step::Error(message) = flag_tail(arg) else {
        panic!("expected flag_tail to reject '{arg}'");
    };
    message
}

#[test]
fn flag_tail_rejects_bare_terminator() {
    // Unreachable through the parser (it strips `--` first), but the
    // tail keeps its own positional rejection.
    let message = tail_error("--");
    assert!(message.contains("takes no paths"), "unexpected: {message}");
}

#[test]
#[should_panic(expected = "expected flag_tail to reject")]
fn tail_error_panics_on_handled_steps() {
    tail_error("-h");
}

#[test]
fn commit_message_requires_a_string_result() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.getcommitmessage".to_string(),
        "42".to_string(),
    );
    let base = serve_amend_routes(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    assert_eq!(
        fetch_commit_message(&client, 123).unwrap_err(),
        "conduit differential.getcommitmessage returned a non-string result"
    );
}

#[test]
fn loads_revisions_for_symbols() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        r#"[{"id": 123, "title": "Add thing"}]"#.to_string(),
    );
    let base = serve_amend_routes(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    let revision = load_revision_for_symbol(&client, "D123").unwrap();
    assert_eq!(revision.id, "123");
    assert_eq!(revision.title, "Add thing");
    assert!(load_revision_for_symbol(&client, "bogus").is_err());
}

#[test]
fn annotates_ids_in_one_batched_call() {
    // Empty input short-circuits without conduit traffic.
    let idle = Client::new("https://conduit.test/", "t").unwrap();
    assert!(annotate_ids(&idle, &[]).unwrap().is_empty());
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        r#"[{"id": 7, "title": "Seven", "statusName": "Accepted"}]"#.to_string(),
    );
    let base = serve_amend_routes(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    let map = annotate_ids(&client, &[7]).unwrap();
    assert_eq!(
        map.get("7"),
        Some(&AnnotatedRevision {
            monogram: "D7".to_string(),
            status: "Accepted".to_string(),
            title: "Seven".to_string(),
        })
    );
}

#[test]
fn annotates_head_messages_with_deduped_ids() {
    let idle = Client::new("https://conduit.test/", "t").unwrap();
    let (heads, map) = annotations_for_messages(&idle, &[]).unwrap();
    assert!(heads.is_empty());
    assert!(map.is_empty());
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        r#"[{"id": 123, "title": "Add thing", "statusName": "Accepted"}]"#.to_string(),
    );
    let base = serve_amend_routes(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    let (heads, map) = annotations_for_messages(
        &client,
        &[
            "work\n\nDifferential Revision: D123\n".to_string(),
            "work\n\nDifferential Revision: D123\n".to_string(),
            "no trailer here\n".to_string(),
        ],
    )
    .unwrap();
    assert_eq!(heads, vec![Some(123), Some(123), None]);
    assert_eq!(map.len(), 1);
    assert_eq!(map["123"].monogram, "D123");
}

#[test]
fn confirms_foreign_authors_without_real_names() {
    let mut routes = std::collections::HashMap::new();
    routes.insert(
        "user.search".to_string(),
        r#"{"data": [{"fields": {"username": "author"}}]}"#.to_string(),
    );
    let base = serve_amend_routes(routes, 1);
    let client = Client::new(&base, "t").unwrap();
    client.set_authenticated_user("viewer", "PHID-USER-VIEWER");
    let mut io = land::Io::canned(&["y"]);
    confirm_author(&mut io, &client, &revision("123", "T"), false).unwrap();
    assert!(
        io.out.stdout.contains("NOT REVISION AUTHOR"),
        "unexpected: {}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("**  *  ** @author\n"),
        "unexpected: {}",
        io.out.stdout
    );
}

#[test]
fn revision_client_with_explicit_uri_reports_config_errors() {
    // The explicit arm builds through real config discovery, which
    // fails for an unknown host without credentials; either way it
    // never touches the network.
    let mut options = empty_options();
    options.revision = Some("123".to_string());
    options.conduit_uri = Some("bogus-no-scheme".to_string());
    assert!(revision_client(&options).is_err());
}

#[test]
fn show_without_state_needs_an_explicit_source() {
    let mut options = empty_options();
    options.show = true;
    assert_eq!(
        show_without_state(&options).unwrap_err(),
        "amend --show requires --message <text> or --revision <id>"
    );
}

#[test]
#[cfg(unix)]
fn amend_git_reports_silent_hook_failures() {
    // A `commit-msg` hook that fails without output makes `git
    // commit` fail silently; our own error surfaces instead of git
    // prose. Unix-only: hook executability needs `chmod`.
    use std::os::unix::fs::PermissionsExt;
    let repo = amend_repo("silent-fail");
    let hook = repo.path().join(".git").join("hooks").join("commit-msg");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).unwrap();
    assert_eq!(
        amend_git(repo.path(), "msg\n").unwrap_err(),
        "git commit --amend failed"
    );
}

#[test]
fn finish_returns_messages_for_show() {
    let tmp = TempDir::new("show");
    let mut options = empty_options();
    options.show = true;
    assert_eq!(finish_git(tmp.path(), &options, "m\n").unwrap(), "m\n");
    assert_eq!(
        finish_hg(tmp.path(), &options, &hg::HgCommand::system(), "m\n").unwrap(),
        "m\n"
    );
}

#[test]
fn flows_short_circuit_show_with_messages() {
    // `--show` with `-m` skips support checks and resolution entirely.
    let tmp = TempDir::new("flows");
    let mut options = empty_options();
    options.show = true;
    options.messages = vec!["hello".to_string()];
    let mut io = land::Io::buffered();
    assert_eq!(
        flow_git(tmp.path(), &options, &mut io, false).unwrap(),
        "hello\n"
    );
    assert_eq!(
        flow_hg(
            tmp.path(),
            &options,
            &mut io,
            &hg::HgCommand::system(),
            false
        )
        .unwrap(),
        "hello\n"
    );
    assert!(io.out.stdout.is_empty());
}

#[test]
fn run_in_reports_missing_terminator() {
    // Piped runs miss the terminator; a TTY run would parse and then
    // fail outside a working copy. Either way the exit is a failure.
    let tmp = TempDir::new("terminator");
    assert_eq!(
        run_in(tmp.path(), &args(&["-m", "x"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn r10_amend_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "AMEND WORKFLOW\n\
         \n\
         amend [options] -- \n\
         Amend the working copy, synchronizing the local commit message.\n\
         \n\
         Amend the working copy, synchronizing the local commit message from\n\
         Differential.\n\
         \n\
         Supported in Mercurial 2.2 and newer.\n\
         \n\
         \x20     --revision id\n\
         \x20         Use the message from a specific revision. If you do not specify a\n\
         \x20         revision, arc will guess which revision is in the working copy.\n\
         \n\
         \x20     --show\n\
         \x20         Show the amended commit message, without modifying the working copy.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("AMEND WORKFLOW"), "{}", usage());
}
