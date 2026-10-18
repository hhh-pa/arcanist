use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn parses_defaults() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&[]), true),
        Ok(Some(Options {
            all: false,
            json: false,
            conduit_uri: None,
        }))
    );
}

#[test]
fn parses_all_json_and_conduit_uri() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--all", "--json"]), true),
        Ok(Some(Options {
            all: true,
            json: true,
            conduit_uri: None,
        }))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri", "https://host/api/"]), true),
        Ok(Some(Options {
            all: false,
            json: false,
            conduit_uri: Some("https://host/api/".to_string()),
        }))
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri=https://host/api/"]), true),
        Ok(Some(Options {
            all: false,
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
    assert_eq!(
        parse_options_with_tty(&args(&["--all", "--help"]), true),
        Ok(None)
    );
}

#[test]
fn rejects_bad_arguments() {
    // T1153 LAND-42: toolset parse texts (`PhutilArgumentParser`).
    // T1153 B21/B23: TTY runs need no `--`.
    assert_eq!(
        parse_options_with_tty(&args(&["--bogus"]), true).unwrap_err(),
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["feature"]), true).unwrap_err(),
        "Unrecognized argument \"feature\"."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--conduit-uri"]), true).unwrap_err(),
        "Argument \"--conduit-uri\" requires a parameter."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--all=x"]), true).unwrap_err(),
        "Argument \"--all\" does not take a parameter."
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--all", "--all"]), true).unwrap_err(),
        "Argument \"--all\" was provided twice."
    );
    // A lone `--` terminates flags and parses clean.
    assert!(
        parse_options_with_tty(&args(&["--"]), true)
            .unwrap()
            .is_some()
    );
    // Past `--`, flags are positionals.
    assert_eq!(
        parse_options_with_tty(&args(&["--", "--help"]), true).unwrap_err(),
        "Unrecognized argument \"--help\"."
    );
}

#[test]
fn parses_ref_line_with_upstream() {
    let branch = parse_ref_line(
        "feature\x1fabc1234\x1fadd thing\x1forigin/feature\x1f[ahead 2]",
        Some("feature"),
        false,
    )
    .unwrap();
    assert_eq!(
        branch,
        Branch {
            name: "feature".to_string(),
            current: true,
            hash: "abc1234".to_string(),
            subject: "add thing".to_string(),
            upstream: Some("origin/feature".to_string()),
            track: "[ahead 2]".to_string(),
            remote: false,
        }
    );
}

#[test]
fn parses_ref_line_without_upstream() {
    let branch = parse_ref_line(
        "main\x1fdef5678\x1ffirst commit\x1f\x1f",
        Some("main"),
        false,
    )
    .unwrap();
    assert_eq!(branch.upstream, None);
    assert_eq!(branch.track, "");
    assert!(branch.current);
    assert!(!branch.remote);
}

#[test]
fn remote_refs_are_never_current() {
    let branch = parse_ref_line(
        "origin/main\x1fdef5678\x1ffirst commit\x1f\x1f",
        Some("origin/main"),
        true,
    )
    .unwrap();
    assert!(!branch.current);
    assert!(branch.remote);
}

#[test]
fn rejects_malformed_ref_lines() {
    assert!(parse_ref_line("only-name", None, false).is_err());
    assert!(parse_ref_line("a\x1fb\x1fc\x1fd", None, false).is_err());
    assert!(parse_ref_line("a\x1fb\x1fc\x1fd\x1fe\x1fextra", None, false).is_err());
}

#[test]
fn formats_json_objects() {
    let branches = vec![Branch {
        name: "main".to_string(),
        current: true,
        hash: "abc1234".to_string(),
        subject: "first".to_string(),
        upstream: None,
        track: String::new(),
        remote: false,
    }];
    let value = format_json(&branches);
    assert_eq!(
        value,
        serde_json::json!([{
            "name": "main",
            "current": true,
            "hash": "abc1234",
            "subject": "first",
            "upstream": null,
            "track": "",
            "remote": false,
        }])
    );
}

#[test]
fn usage_covers_options() {
    // T1153 B21/B23: `--help` renders the generated workflow help
    // (ceb takes no arguments, so extensions stay undocumented).
    let text = usage_text(false);
    assert!(text.contains("BRANCHES WORKFLOW"), "{text}");
    assert!(
        text.contains("Show an enhanced view of branches in the working copy."),
        "{text}"
    );
    assert!(!text.contains("--all"), "{text}");
    assert!(!text.contains("--json"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

#[test]
fn b21_branches_requires_terminator_when_piped() {
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
        parse_options_with_tty(&args(&["--all"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    let err = parse_options_with_tty(&args(&["--bogus"]), false).unwrap_err();
    assert!(err.contains("unrecognized"), "unexpected: {err}");
    assert!(
        parse_options_with_tty(&args(&["--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(
        parse_options_with_tty(&args(&["--all", "--"]), false)
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
fn b23_branches_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help branches`.
    let text = usage_text(false);
    assert!(text.starts_with("BRANCHES WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Lists branches in the working copy"),
        "{text}"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

fn expect_execute_error(words: &[&str]) -> String {
    match execute(&std::env::temp_dir(), &args(words)) {
        Err(message) => message,
        _ => panic!("expected parse error"),
    }
}

#[test]
fn r4_rbd4_parse_error_has_no_dump() {
    // `branches` runs in the toolset runtime: argument errors render
    // `USAGE EXCEPTION` (exit 1) with no usage dump.
    let message = expect_execute_error(&["--bogus"]);
    assert!(!message.contains("usage:"), "{message}");
    assert_eq!(
        run_in(&std::env::temp_dir(), &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn r4_rbd4_outside_repo_is_toolset_usage_not_fatal() {
    // Outside a repo the marker-type support gate throws
    // `PhutilArgumentUsageException` (exit 1) — T1153 RB-D4's "255" does
    // not reproduce at ceb.
    // T1153 B21/B23: piped runs carry `--` to reach the support gate.
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-branches-r4-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        run_in(&dir, &args(&["--"])),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}
#[test]
fn r10_branches_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "BRANCHES WORKFLOW\n\
         \n\
         branches\n\
         Show an enhanced view of branches in the working copy.\n\
         \n\
         Lists branches in the working copy, annotated with additional information\n\
         about review status.\n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("BRANCHES WORKFLOW"), "{}", usage());
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
            "arc-rs-branches-tests-{tag}-{}-{n}",
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
    git(tmp.path(), &["init", "-q", "-b", "master"]);
    git(tmp.path(), &["config", "user.email", "t@t"]);
    git(tmp.path(), &["config", "user.name", "t"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path().join("f.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "f.txt"]);
    git(tmp.path(), &["commit", "-qm", "add f"]);
    tmp
}

#[test]
fn list_rejects_directories_outside_repos() {
    let dir = TempDir::new("outside");
    assert_eq!(
        list(dir.path(), false).unwrap_err(),
        "not inside a git repository"
    );
}

#[test]
fn list_propagates_git_query_failures() {
    // A corrupt `packed-refs` keeps `rev-parse` green (the work-tree
    // gate passes) while `for-each-ref` fails, surfacing the git
    // error through `list`.
    let repo = init_repo("corrupt-refs");
    git(repo.path(), &["pack-refs", "--all"]);
    let packed = repo.path().join(".git").join("packed-refs");
    assert!(packed.is_file());
    std::fs::write(&packed, "GARBAGE NOT A REF\n").unwrap();
    assert!(git::is_inside_work_tree(repo.path()));
    let err = list(repo.path(), false).unwrap_err();
    assert!(err.contains("packed-refs"), "{err}");
}

#[test]
fn detect_vcs_covers_hg() {
    let dir = TempDir::new("vcs");
    std::fs::create_dir(dir.path().join(".hg")).unwrap();
    assert_eq!(detect_vcs(dir.path()), Some("hg"));
    let empty = TempDir::new("vcs-empty");
    assert_eq!(detect_vcs(empty.path()), None);
}

#[test]
fn execute_reports_conduit_setup_failures() {
    // The explicit URI wins over ambient discovery, and its parse
    // failure surfaces without any network traffic.
    let repo = init_repo("setup-fail");
    let err = execute(repo.path(), &args(&["--conduit-uri", "::::", "--"])).unwrap_err();
    assert!(err.contains("must include a valid host"), "{err}");
}

#[test]
fn execute_with_client_covers_help_and_usage_errors() {
    let dir = TempDir::new("exec");
    // `--help` before `--` parses without TTY input.
    let text = execute_with_client(dir.path(), &args(&["--help", "--"]), None, None).unwrap();
    assert!(text.contains("BRANCHES WORKFLOW"), "{text}");
    // Toolset parse errors carry no dump.
    let err = execute_with_client(dir.path(), &args(&["--bogus"]), None, None).unwrap_err();
    assert!(err.contains("unrecognized"), "{err}");
    assert!(!err.contains("BRANCHES WORKFLOW"), "{err}");
}

#[test]
fn execute_core_hg_json_reaches_marker_list() {
    // Under Mercurial `--json` renders through the marker list. The
    // system `hg` backend is unassertable (presence varies), so both
    // outcomes only pin their shapes.
    let dir = TempDir::new("hg-json");
    std::fs::create_dir(dir.path().join(".hg")).unwrap();
    let options = Options {
        all: false,
        json: true,
        conduit_uri: None,
    };
    match execute_core(dir.path(), &options, None, None) {
        // LCOV_EXCL_START // Reason: Ok arm needs system hg; absent on CI, the Err arm pins here
        Ok(text) => {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(value.is_array(), "{text}"); // LCOV_EXCL_STOP
        }
        Err(message) => assert!(!message.is_empty()),
    }
}

#[test]
fn run_in_reports_query_failures_as_fatal() {
    // A git query failure is not a usage error, so it escapes into
    // a fatal (255) instead of the toolset usage arm.
    let repo = init_repo("fatal");
    git(repo.path(), &["pack-refs", "--all"]);
    let packed = repo.path().join(".git").join("packed-refs");
    std::fs::write(&packed, "GARBAGE NOT A REF\n").unwrap();
    assert_eq!(
        run_in(repo.path(), &args(&["--json", "--"])),
        std::process::ExitCode::from(255)
    );
}

// -( T1170 wave 6g batch D )--------------------------------------------------

#[test]
fn t1170_run_in_without_terminator_is_usage_failure() {
    // Piped runs (stdin not a TTY) fail argument parsing with
    // MISSING_TERMINATOR, rendered through `run_in`'s hints arm;
    // interactive runs reach the outside-repo support gate instead.
    // Both are toolset usage failures (exit 1 — the auth arm reports
    // exit 1 too), so the outcome pins without depending on stdin.
    let dir = TempDir::new("no-terminator");
    assert_eq!(
        run_in(dir.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
}
// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "failed:")]
fn git_helper_reports_failing_commands() {
    // A failing git invocation panics with the command and stderr.
    let tmp = TempDir::new("w6h-git-fail");
    git(tmp.path(), &["--bogus-flag-xyz"]);
}

#[test]
#[should_panic(expected = "expected parse error")]
fn r4_rbd4_parse_error_has_no_dump_defensive_ok_arm() {
    // `expect_execute_error` unwraps `Err`: `--help` (past `--`) succeeds.
    expect_execute_error(&["--help", "--"]);
}
