use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn classifies_help_and_version() {
    assert_eq!(classify(&args(&["--help"])), Action::Help);
    assert_eq!(classify(&args(&["-h"])), Action::Help);
    assert_eq!(classify(&args(&["help"])), Action::Help);
    assert_eq!(classify(&args(&["--version"])), Action::Version);
    assert_eq!(classify(&args(&["-V"])), Action::Version);
}

#[test]
fn classifies_call_conduit() {
    assert_eq!(
        classify(&args(&["call-conduit", "user.whoami"])),
        Action::CallConduit(CallArgs {
            method: "user.whoami".to_string(),
            conduit_uri: None,
        })
    );
    assert_eq!(
        classify(&args(&[
            "call-conduit",
            "--conduit-uri",
            "https://orsay.hhh.pa/",
            "--",
            "user.whoami"
        ])),
        Action::CallConduit(CallArgs {
            method: "user.whoami".to_string(),
            conduit_uri: Some("https://orsay.hhh.pa/".to_string()),
        })
    );
}

#[test]
fn rejects_bad_call_conduit_invocations() {
    for argv in [
        vec!["call-conduit"],
        vec!["call-conduit", "a", "b"],
        vec!["call-conduit", "m", "--bogus"],
        vec!["call-conduit", "m", "--conduit-uri"],
    ] {
        assert!(
            matches!(classify(&args(&argv)), Action::Error(_)),
            "{argv:?}"
        );
    }
    assert_eq!(
        classify(&args(&["call-conduit"])),
        Action::Error("Provide exactly one Conduit method name to call.".to_string())
    );
}

#[test]
fn reads_params_json_from_stdin_bytes() {
    assert_eq!(
        read_params_json(&b"{\"a\": 1}"[..]).unwrap(),
        serde_json::json!({"a": 1})
    );
    assert_eq!(
        read_params_json(&b"nope"[..]).unwrap_err(),
        "Provide method parameters on stdin as a JSON blob."
    );
    struct Failing;
    impl std::io::Read for Failing {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }
    assert!(
        read_params_json(Failing)
            .unwrap_err()
            .contains("failed to read stdin")
    );
}

#[test]
fn renders_call_conduit_envelope() {
    assert_eq!(
        render_envelope(None, serde_json::json!({"user": "x"})),
        "{\n  \"error\": null,\n  \"errorMessage\": null,\n  \"response\": {\n    \"user\": \"x\"\n  }\n}"
    );
    assert_eq!(
        render_envelope(Some(("E", "no")), serde_json::Value::Null),
        "{\n  \"error\": \"E\",\n  \"errorMessage\": \"no\",\n  \"response\": null\n}"
    );
}

#[test]
fn call_conduit_execute_rejects_bad_stdin_without_io() {
    // T1153 R4 (CORE-6): the stdin error is an `ArcanistUsageException`
    // in the toolset runtime, which escapes into a PHP fatal (255),
    // not a caught exit 1.
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: None,
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"nope"[..], &mut out),
        ExitCode::from(255)
    );
    assert!(out.is_empty());
}

#[test]
fn call_conduit_execute_reports_missing_token_without_io() {
    // T1153 R4 (CORE-6): conduit setup failures escape the toolset
    // runtime into a PHP fatal (255), like transport failures.
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some("https://call-conduit-test.invalid/".to_string()),
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"{}"[..], &mut out),
        ExitCode::from(255)
    );
    assert!(out.is_empty());
}

#[test]
fn classifies_missing_and_unknown() {
    assert_eq!(classify(&args(&[])), Action::Missing);
    assert_eq!(
        classify(&args(&["bogus"])),
        Action::Unknown("bogus".to_string())
    );
}

#[test]
fn usage_lists_commands() {
    let u = usage();
    assert!(u.contains("call-conduit"));
    assert!(u.contains("diff"));
    assert!(u.contains("alias"));
}

#[test]
fn alias_command_is_registered() {
    let run = COMMANDS
        .iter()
        .find(|(name, _)| *name == alias::NAME)
        .map(|(_, run)| *run);
    let Some(run) = run else {
        panic!("alias is not registered");
    };
    assert_eq!(run(&args(&["--help"])), ExitCode::SUCCESS);
}

#[test]
fn builtin_set_matches_dispatch_table() {
    for (name, _) in COMMANDS {
        assert!(alias::is_builtin_command(name), "{name}");
    }
    assert!(alias::is_builtin_command("call-conduit"));
    assert!(alias::is_builtin_command("help"));
    assert_eq!(alias::builtin_names().len(), COMMANDS.len() + 2);
}

fn stored_aliases(value: serde_json::Value) -> Vec<alias::Alias> {
    let (aliases, warnings) =
        alias::aliases_from_storage(&value, "user (test)").expect("test aliases are valid storage");
    assert!(warnings.is_empty());
    aliases
}

fn dispatch_target(argv: &[String]) -> CommandFn {
    let cmd = argv.first().expect("expanded argv has a command");
    find_command(cmd).unwrap_or_else(|| panic!("{cmd} is not registered"))
}

#[test]
fn stored_alias_expands_through_main_dispatch() {
    for stored in [
        serde_json::json!({"draft": ["diff", "--draft"]}),
        serde_json::json!([{"trigger": "draft", "toolset": "arc", "command": ["diff", "--draft"]}]),
    ] {
        let aliases = stored_aliases(stored);
        let (outcome, notices) = alias::apply_expansion_with(
            args(&["draft", "extra"]),
            || Some((aliases, Vec::new())),
            |_| panic!("no shell execution expected"),
        );
        let Ok(argv) = outcome else {
            panic!("expected dispatch, got {outcome:?}");
        };
        assert_eq!(argv, args(&["diff", "--draft", "extra"]));
        assert_eq!(notices.len(), 1);
        assert!(
            std::ptr::fn_addr_eq(dispatch_target(&argv), diff::run as CommandFn),
            "expanded argv does not dispatch to diff"
        );
    }
}

#[test]
fn expansion_leaves_builtins_and_flags_alone() {
    for argv in [args(&["diff"]), args(&["--help"]), args(&["bogus"])] {
        let aliases = stored_aliases(serde_json::json!({"diff": ["lint"]}));
        let (outcome, notices) = alias::apply_expansion_with(
            argv.clone(),
            || Some((aliases, Vec::new())),
            |_| panic!("no shell execution expected"),
        );
        assert_eq!(outcome, Ok(argv));
        assert!(notices.is_empty());
    }
}

#[test]
fn expansion_reports_shell_and_broken_chains() {
    let aliases = stored_aliases(serde_json::json!({"ls": ["!ls"]}));
    let (outcome, notices) = alias::apply_expansion_with(
        args(&["ls", "-la"]),
        || Some((aliases, Vec::new())),
        |shell| {
            assert_eq!(shell, &args(&["ls", "-la"]));
            Ok(alias::ShellStatus::Exited(0))
        },
    );
    assert_eq!(outcome, Err(ExitCode::SUCCESS));
    assert!(
        notices.iter().any(|n| n.starts_with("SHELL ALIAS: ")),
        "{notices:?}"
    );

    let aliases = stored_aliases(serde_json::json!({"a": ["gone"]}));
    let (outcome, _) = alias::apply_expansion_with(
        args(&["a"]),
        || Some((aliases, Vec::new())),
        |_| panic!("no shell execution expected"),
    );
    assert_eq!(outcome, Err(ExitCode::FAILURE));
}

/// Every ported subcommand: the (NAME, run) pairs that must be registered
/// in COMMANDS, listed in usage, and reachable through `dispatch` (the
/// same path `main` uses). `call-conduit` is intentionally absent: it
/// dispatches through `classify`, not the table.
fn expected_commands() -> Vec<(&'static str, CommandFn)> {
    vec![
        (alias::NAME, alias::run),
        (amend::NAME, amend::run),
        (anoid::NAME, anoid::run),
        (bookmarks::NAME, bookmarks::run),
        (branches::NAME, branches::run),
        (browse::NAME, browse::run),
        (close_revision::NAME, close_revision::run),
        (commit::NAME, commit::run),
        (cover::NAME, cover::run),
        (diff::NAME, diff::run),
        (download::NAME, download::run),
        (export::NAME, export::run),
        (get_config::NAME, get_config::run),
        (inspect::NAME, inspect::run),
        (install_certificate::NAME, install_certificate::run),
        (land::NAME, land::run),
        (liberate::NAME, liberate::run),
        (lint::NAME, lint::run),
        (linters::NAME, linters::run),
        (list::NAME, list::run),
        (look::NAME, look::run),
        (markers::NAME, markers::run),
        (paste::NAME, paste::run),
        (patch::NAME, patch::run),
        (set_config::NAME, set_config::run),
        (tasks::NAME, tasks::run),
        (todo::NAME, todo::run),
        (unit::NAME, unit::run),
        (upgrade::NAME, upgrade::run),
        (upload::NAME, upload::run),
        (version::NAME, version::run),
        (weld::NAME, weld::run),
        (which::NAME, which::run),
        (work::NAME, work::run),
    ]
}

#[test]
fn all_ported_commands_are_registered() {
    assert_eq!(COMMANDS.len(), expected_commands().len());
    for (name, run) in expected_commands() {
        assert!(
            find_command(name).is_some_and(|f| std::ptr::fn_addr_eq(f, run)),
            "command '{name}' is not registered"
        );
        assert!(
            usage().contains(&format!("  {name}\n")),
            "usage omits command '{name}'"
        );
    }
}

#[test]
fn all_ported_commands_dispatch_through_main() {
    for (name, _) in expected_commands() {
        assert_eq!(
            dispatch(&args(&[name, "--help"])),
            ExitCode::SUCCESS,
            "command '{name}' did not dispatch through main"
        );
    }
}

#[test]
fn help_dispatches_each_name_with_workflow_header() {
    assert_eq!(dispatch(&args(&["help", "diff"])), ExitCode::SUCCESS);
    assert_eq!(dispatch(&args(&["help", "version"])), ExitCode::SUCCESS);
    // PHP renders every name, not just the first.
    assert_eq!(
        dispatch(&args(&["help", "diff", "version"])),
        ExitCode::SUCCESS
    );
    // PHP lowercases names before lookup.
    assert_eq!(dispatch(&args(&["help", "DIFF"])), ExitCode::SUCCESS);
    // `help help` succeeds: unknown names print a no-workflow message
    // (`PhutilHelpArgumentWorkflow` always returns 0).
    assert_eq!(dispatch(&args(&["help", "help"])), ExitCode::SUCCESS);
}

#[test]
fn help_unknown_name_succeeds_with_no_workflow_message() {
    assert_eq!(dispatch(&args(&["help"])), ExitCode::SUCCESS);
    assert_eq!(dispatch(&args(&["help", "bogus"])), ExitCode::SUCCESS);
}

#[test]
fn workflow_header_and_no_workflow_line_match_php() {
    assert_eq!(workflow_header("bogus", false), "BOGUS WORKFLOW");
    assert_eq!(
        no_workflow_line("bogus", false),
        "There is no bogus workflow."
    );
    // Bold markers are interpreted on TTYs (exact ANSI bytes are
    // covered by the terminal console-format tests).
    assert!(workflow_header("bogus", true).contains('\x1b'));
    assert!(no_workflow_line("bogus", true).contains('\x1b'));
}

#[test]
fn call_conduit_accepts_help_flags() {
    assert_eq!(
        classify(&args(&["call-conduit", "--help"])),
        Action::CallConduitHelp
    );
    assert_eq!(
        classify(&args(&["call-conduit", "-h"])),
        Action::CallConduitHelp
    );
    assert_eq!(
        dispatch(&args(&["call-conduit", "--help"])),
        ExitCode::SUCCESS
    );
    assert_eq!(
        dispatch(&args(&["help", "call-conduit"])),
        ExitCode::SUCCESS
    );
    assert!(CALL_CONDUIT_USAGE.contains("Call Conduit API methods."));
    assert!(CALL_CONDUIT_USAGE.contains("call-conduit -- method"));
    assert!(CALL_CONDUIT_USAGE.contains("conduit.ping"));
}

#[test]
fn call_conduit_help_flag_after_separator_stays_positional() {
    assert!(!call_conduit_wants_help(&args(&["--", "--help"])));
    assert!(parse_call_conduit(&args(&["--", "--help"])).is_ok());
}

#[test]
fn call_conduit_help_flag_as_uri_value_is_not_help() {
    // Mirrors the parser: `--conduit-uri` consumes the next token.
    assert!(!call_conduit_wants_help(&args(&[
        "--conduit-uri",
        "--help"
    ])));
    assert!(parse_call_conduit(&args(&["--conduit-uri", "--help"])).is_err());
    assert_eq!(
        classify(&args(&["call-conduit", "--conduit-uri", "--help"])),
        Action::Error("Provide exactly one Conduit method name to call.".to_string())
    );
}

#[test]
fn r4_core49_unknown_command_exits_1() {
    // Unknown commands render `Usage Exception:` (subprocess arm)
    // with exit 1 and no usage dump.
    assert_eq!(dispatch(&args(&["bogus-command"])), ExitCode::FAILURE);
}

#[test]
fn r4_core49_call_conduit_parse_error_exits_1() {
    // `call-conduit` parser errors render the toolset `USAGE
    // EXCEPTION` label with exit 1 and no usage dump.
    assert_eq!(
        dispatch(&args(&["call-conduit", "--bogus"])),
        ExitCode::FAILURE
    );
    assert_eq!(dispatch(&args(&["call-conduit"])), ExitCode::FAILURE);
}
