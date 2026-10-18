use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-bookmarks-{name}-{}-{}",
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
fn parses_defaults() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&[]), true),
        Ok(Some(Options {
            json: false,
            conduit_uri: None,
        }))
    );
}

#[test]
fn parses_json_and_conduit_uri() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--json"]), true),
        Ok(Some(Options {
            json: true,
            conduit_uri: None,
        }))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri=https://host/api/"]), true),
        Ok(Some(Options {
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
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
    let err = parse_options_with_tty(&args(&["mark"]), true).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), true)
            .unwrap()
            .is_some()
    );
    let err = parse_options_with_tty(&args(&["--", "--help"]), true).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
    let err = parse_options_with_tty(&args(&["--conduit-uri"]), true).unwrap_err();
    assert!(err.contains("requires a value"), "unexpected: {err}");
}

#[test]
fn finds_hg_root_upward() {
    let dir = scratch("root");
    let nested = dir.join("a").join("b");
    std::fs::create_dir_all(nested.join(".hg")).unwrap();
    assert_eq!(find_hg_root(&nested), Some(nested.clone()));
    assert_eq!(find_hg_root(&dir.join("sibling-without-hg")), None);
    assert_eq!(find_hg_root(&dir), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_like_arc_outside_mercurial() {
    let dir = scratch("nowhere");
    let err = list(&dir).unwrap_err();
    assert!(
        err.contains("not inside a working copy"),
        "unexpected: {err}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn formats_json_objects() {
    let marks = vec![Bookmark {
        name: "active".to_string(),
        active: true,
        rev: String::new(),
        node: "abc123".to_string(),
    }];
    assert_eq!(
        format_json(&marks),
        serde_json::json!([{
            "name": "active",
            "active": true,
            "rev": "",
            "node": "abc123",
        }])
    );
}

#[test]
fn usage_covers_options() {
    // T1153 B21/B23: `--help` renders the generated workflow help
    // (ceb takes no arguments, so extensions stay undocumented).
    let text = usage_text(false);
    assert!(text.contains("BOOKMARKS WORKFLOW"), "{text}");
    assert!(
        text.contains("Show an enhanced view of bookmarks in the working copy."),
        "{text}"
    );
    assert!(!text.contains("--json"), "{text}");
    assert!(!text.contains("--conduit-uri"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
#[test]
fn r10_bookmarks_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "BOOKMARKS WORKFLOW\n\
         \n\
         bookmarks\n\
         Show an enhanced view of bookmarks in the working copy.\n\
         \n\
         Lists bookmarks in the working copy, annotated with additional information\n\
         about review status.\n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("BOOKMARKS WORKFLOW"), "{}", usage());
}

#[test]
fn b21_bookmarks_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv (PHP's `parseFull` runs before
    // workflow validation). Unknown flags still report first.
    assert_eq!(
        parse_options_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--json"]), false),
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
        parse_options_with_tty(&args(&["--json", "--"]), false)
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
fn b23_bookmarks_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help bookmarks`.
    // No specs and no `__param__` markers, so no underline codes.
    let text = usage_text(false);
    assert!(text.starts_with("BOOKMARKS WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Lists bookmarks in the working copy"),
        "{text}"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// -( T1170 wave 5d )-----------------------------------------------------------

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-bookmarks-tests-{tag}-{}-{n}",
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

/// A fake `hg` whose stdout is `body` (mirrors `HgCommand::custom`;
/// the appended hg argv are ignored by `sh -c`).
fn fake_hg_stdout(body: &str) -> crate::hg::HgCommand {
    crate::hg::HgCommand {
        program: "sh".to_string(),
        prefix: vec!["-c".to_string(), format!("printf '%s' '{body}'")],
    }
}

#[test]
fn parses_space_separated_conduit_uri() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri", "https://host/api/"]), true),
        Ok(Some(Options {
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
}

#[test]
fn list_with_binary_reports_missing_working_copy() {
    // The binary seam forces the Mercurial-missing path without
    // depending on what is installed on PATH.
    let dir = TempDir::new("nowhere");
    let err = list_with_binary(dir.path(), "arc-rs-definitely-missing-hg").unwrap_err();
    assert!(err.contains("not inside a working copy"), "{err}");
}

#[test]
fn list_maps_and_sorts_bookmarks() {
    let dir = TempDir::new("marks");
    std::fs::create_dir(dir.path().join(".hg")).unwrap();
    let body = r#"[
        {"type": "bookmark", "name": "b10", "node": "aaa", "isActive": false},
        {"type": "bookmark", "name": "b2", "node": "bbb", "isActive": false},
        {"type": "bookmark", "name": "zeta", "node": "ccc", "isActive": true},
        {"type": "branch", "name": "main", "node": "ddd", "isActive": false}
    ]"#;
    let marks = list_with(dir.path(), &fake_hg_stdout(body)).unwrap();
    // Active first, then natural name order (`b2` before `b10`);
    // branches are filtered out, nodes map, revs stay empty.
    let names: Vec<&str> = marks.iter().map(|mark| mark.name.as_str()).collect();
    assert_eq!(names, vec!["zeta", "b2", "b10"]);
    assert_eq!(marks[0].node, "ccc");
    assert!(marks[0].active);
    assert!(marks.iter().all(|mark| mark.rev.is_empty()));
}

#[test]
fn execute_with_binary_covers_help_and_usage_errors() {
    let dir = TempDir::new("exec");
    // `--help` before `--` parses without TTY input.
    let text = execute_with_binary(dir.path(), &args(&["--help", "--"]), "hg").unwrap();
    assert!(text.contains("BOOKMARKS WORKFLOW"), "{text}");
    // Unknown flags fail before the terminator check, with a dump.
    let err = execute_with_binary(dir.path(), &args(&["--bogus"]), "hg").unwrap_err();
    assert!(err.contains("unknown flag"), "{err}");
    assert!(err.contains("BOOKMARKS WORKFLOW"), "{err}");
}

#[test]
fn execute_core_json_lists_bookmarks() {
    let dir = TempDir::new("json");
    std::fs::create_dir(dir.path().join(".hg")).unwrap();
    let body = r#"[
        {"type": "bookmark", "name": "wip", "node": "abc123", "isActive": true}
    ]"#;
    let options = Options {
        json: true,
        conduit_uri: None,
    };
    let text = execute_core(dir.path(), &options, &fake_hg_stdout(body), None, None).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value,
        serde_json::json!([{
            "name": "wip",
            "active": true,
            "rev": "",
            "node": "abc123",
        }])
    );
}

#[test]
fn t1170_run_in_reports_terminator_through_toolset_arm() {
    // T1170 band J: piped `run_in` without `--` takes the
    // MISSING_TERMINATOR arm (prints hints, toolset usage exit 1).
    // No TTY guard: piped runs fail parsing and interactive runs hit
    // the outside-repo gate, and both are toolset usage failures, so
    // the outcome pins without depending on stdin (same shape as the
    // branches twin).
    let dir = TempDir::new("t1170-terminator");
    assert_eq!(
        run_in(dir.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
}
