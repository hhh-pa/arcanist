use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parses_summary_words() {
    let options = parse_args(&args(&["fix", "the", "build"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.summary, args(&["fix", "the", "build"]));
    assert!(options.cc.is_empty());
    assert!(options.projects.is_empty());
    assert!(!options.browse);
    assert!(!options.json);
    assert_eq!(options.conduit_uri, None);
}

#[test]
fn parses_all_options() {
    let options = parse_args(&args(&[
        "do",
        "thing",
        "--cc",
        "alice",
        "-C",
        "bob",
        "--cc=carol",
        "--project",
        "web",
        "--project=api",
        "--json",
        "--conduit-uri",
        "https://example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.summary, args(&["do", "thing"]));
    assert_eq!(options.cc, args(&["alice", "bob", "carol"]));
    assert_eq!(options.projects, args(&["web", "api"]));
    assert!(options.json);
    assert_eq!(
        options.conduit_uri,
        Some("https://example.test/".to_string())
    );
}

#[test]
fn parses_double_dash_and_browse() {
    let options = parse_args(&args(&["--browse", "--", "--not-a-flag"]))
        .unwrap()
        .unwrap();
    assert!(options.browse);
    assert_eq!(options.summary, args(&["--not-a-flag"]));
}

#[test]
fn parses_help() {
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), None);
}

#[test]
fn rejects_bad_args() {
    assert!(parse_args(&args(&["--bogus"])).is_err());
    assert!(parse_args(&args(&["--cc"])).is_err());
    assert!(parse_args(&args(&["-C"])).is_err());
    assert!(parse_args(&args(&["--cc="])).is_err());
    assert!(parse_args(&args(&["--project"])).is_err());
    assert!(parse_args(&args(&["--conduit-uri"])).is_err());
}

#[test]
fn parses_created_tasks() {
    let task = parse_created(&serde_json::json!({
        "id": 55,
        "title": "Do thing",
        "uri": "https://example.test/T55",
    }))
    .unwrap();
    assert_eq!(task.id, "55");
    assert_eq!(task.title, "Do thing");
    assert_eq!(task.uri, "https://example.test/T55");
    assert!(parse_created(&serde_json::json!({"title": "no id"})).is_none());
}

#[test]
fn renders_created_line() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    assert_eq!(
        render_created(&task, false),
        "Created task T55: 'Do thing' at https://example.test/T55\n"
    );
    // Titles substitute raw (never escaped) and keep colors on TTYs.
    let tricky = CreatedTask {
        id: "1".to_string(),
        title: "a\nb".to_string(),
        uri: "u".to_string(),
    };
    assert_eq!(
        render_created(&tricky, false),
        "Created task T1: 'a\nb' at u\n"
    );
    assert_eq!(
        render_created(&task, true),
        "Created task T55: '\u{1b}[32m\u{1b}[1mDo thing\u{1b}[m\u{1b}[39m' at \
         \u{1b}[34m\u{1b}[1mhttps://example.test/T55\u{1b}[m\u{1b}[39m\n"
    );
}

fn plain_options(summary: &[&str]) -> Options {
    Options {
        summary: args(summary),
        cc: Vec::new(),
        projects: Vec::new(),
        browse: false,
        json: false,
        conduit_uri: None,
    }
}

#[test]
fn execute_threads_ansi_through_finish() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    let options = plain_options(&["Do", "thing"]);
    assert_eq!(
        render_created_task(&task, &options, false).unwrap(),
        render_created(&task, false)
    );
    let styled = render_created_task(&task, &options, true).unwrap();
    assert_eq!(styled, render_created(&task, true));
    assert!(styled.contains('\u{1b}'), "got: {styled}");
}

#[test]
fn rbd29b_create_returns_task_for_browse_after_print() {
    // `--browse` no longer runs inside `execute`: the outcome carries
    // the task so the caller prints first, then browses (like PHP).
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let options = plain_options(&[]);
    let outcome = create_in_ansi(std::env::temp_dir().as_path(), &client, &options, false).unwrap();
    assert_eq!(outcome.text, "Please provide a summary.\n");
    assert_eq!(outcome.task, None);
}

#[test]
fn blank_summary_creates_nothing_in_both_modes() {
    // No conduit traffic: the client points at a dead port and blank
    // summaries return before any call.
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    let options = plain_options(&[]);
    for ansi in [false, true] {
        assert_eq!(
            execute_in_ansi(std::env::temp_dir().as_path(), &client, &options, ansi).unwrap(),
            "Please provide a summary.\n"
        );
    }
}

#[test]
fn builds_created_json() {
    let task = CreatedTask {
        id: "55".to_string(),
        title: "Do thing".to_string(),
        uri: "https://example.test/T55".to_string(),
    };
    assert_eq!(
        created_json(&task),
        serde_json::json!({
            "id": "T55",
            "title": "Do thing",
            "uri": "https://example.test/T55",
        })
    );
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `todo` is a subprocess workflow: argument errors render `Usage
    // Exception:` (exit 1) with no usage dump.
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn rbd8_user_phid_prefers_cached_identity() {
    // The startup-auth identity answers without I/O: an unroutable
    // client still resolves when cached.
    let client = conduit::Client::new("http://127.0.0.1:9/", "t").unwrap();
    client.set_authenticated_user("alice", "PHID-USER-1");
    assert_eq!(user_phid(&client).unwrap(), "PHID-USER-1");
}
#[test]
fn r10_todo_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "TODO WORKFLOW\n\
         \n\
         todo\n\
         \x20     todo summary [options]\n\
         \n\
         \x20       Quickly create a task for yourself.\n\
         \n\
         \x20     --browse\n\
         \x20         After creating the task, open it in a web browser.\n\
         \n\
         \x20     --cc cc, -C cc\n\
         \x20         Other users to CC on the new task.\n\
         \n\
         \x20     --project project\n\
         \x20         Projects to assign to the task.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("TODO WORKFLOW"), "{}", usage());
}

#[test]
fn b21_todo_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `todo`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(parse_args(&args(&["fix", "it"])).unwrap().is_some());
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), None);
    let text = usage_text(false);
    assert!(text.contains("TODO WORKFLOW"), "{text}");
}

#[test]
fn b23_todo_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help todo`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("TODO WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Quickly create a task for yourself."),
        "{text}"
    );
    assert!(text.contains("--browse"), "{text}");
    assert!(!text.contains("--json"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
