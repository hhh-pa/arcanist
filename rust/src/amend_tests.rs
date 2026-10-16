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
