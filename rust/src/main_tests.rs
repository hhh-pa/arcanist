use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn classifies_help_and_version() {
    // T1153 CORE-22: `--version`/`-V` are additive (PHP has no such
    // flag; bare `arc` asks to choose a workflow).
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
    // T1153 CORE-6 (R4): the stdin error is an `ArcanistUsageException`
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
    // T1153 CORE-6 (R4): conduit setup failures escape the toolset
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
    let run = run.expect("alias is not registered");
    // T1153 B21/B23: piped runs carry `--`.
    assert_eq!(run(&args(&["--help", "--"])), ExitCode::SUCCESS);
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

/// Unwrap a [`Resolution::Run`]; the rejection arm is covered by
/// [`expect_run_rejects_unknown`].
fn expect_run(resolved: Resolution) -> (&'static str, Option<String>) {
    match resolved {
        Resolution::Run { name, notice } => (name, notice),
        Resolution::Unknown { message } => panic!("expected run, got unknown: {message}"),
    }
}

/// Unwrap a [`Resolution::Unknown`]; the rejection arm is covered by
/// [`expect_unknown_rejects_run`].
fn expect_unknown(resolved: Resolution) -> String {
    match resolved {
        Resolution::Unknown { message } => message,
        Resolution::Run { name, .. } => panic!("expected unknown, got run: {name}"),
    }
}

/// Shell-spawn callback for expansions that must stay in-process;
/// covered by [`no_shell_panics`].
fn no_shell(shell: &[String]) -> Result<alias::ShellStatus, String> {
    panic!("no shell execution expected: {shell:?}")
}

#[test]
#[should_panic(expected = "expected run")]
fn expect_run_rejects_unknown() {
    expect_run(resolve_command("bogus", false));
}

#[test]
#[should_panic(expected = "expected unknown")]
fn expect_unknown_rejects_run() {
    expect_unknown(resolve_command("land", true));
}

#[test]
#[should_panic(expected = "no shell execution expected")]
fn no_shell_panics() {
    let _ = no_shell(&[]);
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
            no_shell,
        );
        assert!(outcome.is_ok(), "expected dispatch, got {outcome:?}");
        let argv = outcome.unwrap();
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
        let (outcome, notices) =
            alias::apply_expansion_with(argv.clone(), || Some((aliases, Vec::new())), no_shell);
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
            // T1153 CORE-23: PHP `phutil_passthru()` mediates the
            // shell, so Unix runs through `sh -c` (direct elsewhere).
            #[cfg(unix)]
            assert_eq!(shell, &args(&["sh", "-c", "'ls' '-la'"]));
            #[cfg(not(unix))]
            assert_eq!(shell, &args(&["ls", "-la"]));
            Ok(alias::ShellStatus::Exited(0))
        },
    );
    assert_eq!(outcome, Err(ExitCode::SUCCESS));
    // T1153 CORE-23: cyan `SHELL ALIAS` log-engine hint on stderr.
    assert!(
        notices.iter().any(|n| n.contains("SHELL ALIAS")),
        "{notices:?}"
    );

    let aliases = stored_aliases(serde_json::json!({"a": ["gone"]}));
    let (outcome, _) =
        alias::apply_expansion_with(args(&["a"]), || Some((aliases, Vec::new())), no_shell);
    // T1153 CORE-27 (kept): a broken chain fails fast instead of
    // PHP's legacy fallthrough to the unknown-command error.
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
    // T1153 RB-D5: header helpers live in the shared `help` renderer.
    assert_eq!(help::workflow_header("bogus", false), "BOGUS WORKFLOW");
    assert_eq!(
        help::no_workflow_line("bogus", false),
        "There is no bogus workflow."
    );
    // Bold markers are interpreted on TTYs (exact ANSI bytes are
    // covered by the terminal console-format tests).
    assert!(help::workflow_header("bogus", true).contains('\x1b'));
    assert!(help::no_workflow_line("bogus", true).contains('\x1b'));
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
    // T1153 R10: help renders from the reference entry (the old
    // `CALL_CONDUIT_USAGE` duplicate is gone); exact bytes are locked
    // by `r10_call_conduit_help_matches_generated_format`.
    let text = render_workflow_help("call-conduit", true, false);
    assert!(text.contains("Call Conduit API methods."));
    assert!(text.contains("call-conduit -- method"));
    assert!(text.contains("conduit.ping"));
    // T1153 CORE-67: the ceb help names Phabricator, not "writing scripts".
    assert!(text.contains("integrate with\nPhabricator"));
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

#[test]
fn core1_call_conduit_envelope_has_code_prefix() {
    // The envelope `errorMessage` is the `ConduitClientException`
    // message: `"{code}: {info}"` with no `<method>` infix at ceb.
    let text = render_api_envelope("ERR-CONDUIT-CORE", "no such method");
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["error"], serde_json::json!("ERR-CONDUIT-CORE"));
    assert_eq!(
        doc["errorMessage"],
        serde_json::json!("ERR-CONDUIT-CORE: no such method")
    );
}

#[test]
fn core16_toolset_exact_wins() {
    assert!(matches!(
        resolve_command("land", true),
        Resolution::Run {
            name: "land",
            notice: None
        }
    ));
    assert!(matches!(
        resolve_command("land", false),
        Resolution::Run {
            name: "land",
            notice: None
        }
    ));
}

#[test]
fn core16_toolset_spelling_corrects_only_when_interactive() {
    let (name, notice) = expect_run(resolve_command("ver", true));
    assert_eq!(name, "version");
    assert_eq!(
        notice.as_deref(),
        Some("(Assuming \"ver\" is the British spelling of \"version\".)")
    );
    // Piped: no autocorrect, falls to the classic prefix stage, which
    // also finds `version` (run without a notice).
    assert!(matches!(
        resolve_command("ver", false),
        Resolution::Run {
            name: "version",
            notice: None
        }
    ));
}

#[test]
fn core16_classic_exact_is_case_insensitive() {
    assert!(matches!(
        resolve_command("DIFF", false),
        Resolution::Run {
            name: "diff",
            notice: None
        }
    ));
}

#[test]
fn core16_classic_unique_prefix_runs() {
    assert!(matches!(
        resolve_command("wh", false),
        Resolution::Run {
            name: "which",
            notice: None
        }
    ));
    // T1153 CORE-66: `browse` is a concrete PHP workflow, so its
    // unique prefix resolves (unlike the abstract `markers`).
    assert!(matches!(
        resolve_command("bro", false),
        Resolution::Run {
            name: "browse",
            notice: None
        }
    ));
}

#[test]
fn core16_classic_multi_prefix_lists_suggestions() {
    let message = expect_unknown(resolve_command("l", false));
    assert!(message.contains("Unknown command 'l'. Try 'arc help'."));
    assert!(message.contains("Did you mean:\n    land\n    liberate\n    lint\n"));
}

#[test]
fn core16_unknown_command_without_suggestions() {
    let message = expect_unknown(resolve_command("bogus", false));
    assert_eq!(message, "Unknown command 'bogus'. Try 'arc help'.");
}

#[test]
fn core16_dash_led_names_never_correct() {
    let message = expect_unknown(resolve_command("--help", true));
    assert!(message.contains("Unknown command '--help'."));
}

#[test]
fn core20_bare_help_lists_synopses_sorted_with_help() {
    // T1153 CORE-20: the full WORKFLOWS reference with synopses.
    let text = render_help_bare(false, false);
    assert!(text.starts_with("WORKFLOWS\n\n"));
    assert!(text.contains("Amend the working copy, synchronizing the local commit message."));
    assert!(text.contains("Show this help, or workflow help for command."));
    assert!(text.contains("Install shell completion."));
    assert!(text.contains("OPTION REFERENCE"));
    assert!(text.contains("Use help command for a detailed command reference."));
    assert!(text.contains("Use --show-standard-options to show additional options."));
    let diff = text.find("diff [paths]").expect("diff entry");
    let download = text
        .find("download [options] -- file")
        .expect("download entry");
    assert!(diff < download, "ksorted");
    let install = text.find("install-certificate").expect("install entry");
    let inspect = text
        .find("inspect [options] -- object")
        .expect("inspect entry");
    assert!(inspect < install, "ksorted inspect before install");
}

#[test]
fn core20_detailed_help_matches_php_layout() {
    let text = render_workflow_help("diff", true, false);
    assert!(text.starts_with("diff\n"));
    assert!(text.contains("Supports: git, svn, hg"));
    assert!(text.contains("--add-all, -a"));
    assert!(text.contains("Automatically add all unstaged"));
    assert!(text.ends_with('\n'));
}

#[test]
fn r10_browse_help_matches_ceb_bytes() {
    // T1153 R10: `arc help browse` renders the ceb workflow help
    // (`src/browse/workflow/ArcanistBrowseWorkflow.php:13-60`), not
    // the Rust-extended resolver text. Byte-identical to the
    // per-command `--help` (`browse::help_text`).
    let text = render_workflow_help("browse", true, false);
    assert!(
        text.contains("browse [options] -- target ...\n"),
        "{text:?}"
    );
    assert!(text.contains("browse -- file-name\n"), "{text:?}");
    assert!(
        text.contains("Open a file or object in a local web browser."),
        "{text:?}"
    );
    assert!(text.contains("$ arc browse README"), "{text:?}");
    assert!(text.contains("--branch branch-name\n"), "{text:?}");
    assert!(
        text.contains("Default branch name to view on server."),
        "{text:?}"
    );
    assert!(text.contains("--types type-list\n"), "{text:?}");
    assert!(text.contains("(DEPRECATED) Obsolete"), "{text:?}");
}

#[test]
fn core20_legacy_synopsis_keeps_source_indent() {
    let text = render_workflow_help("diff", false, false);
    assert!(text.contains("      diff [paths] (svn)\n"));
}

#[test]
fn core66_prompts_and_shell_complete_listed() {
    // T1153 CORE-66: `prompts` and `shell-complete` are listed.
    let text = render_help_bare(false, false);
    assert!(text.contains("prompts workflow"));
    assert!(text.contains("Install shell completion."));
    let detailed = render_workflow_help("shell-complete", true, false);
    assert!(detailed.contains("Internal. Current term in the argument list being completed."));
}

#[test]
fn core67_call_conduit_help_is_ceb_bytes() {
    let text = render_workflow_help("call-conduit", true, false);
    assert!(text.contains("Call Conduit API methods."));
    // The unwrapped ceb sentence (wrapping re-breaks the long line).
    assert!(text.contains(
        "primarily useful for writing scripts which integrate with\nPhabricator. Examples:"
    ));
}

#[test]
fn rbd3_strips_standard_globals_anywhere() {
    let (rest, globals) = strip_globals(&args(&["--trace", "land", "--no-ansi"])).unwrap();
    assert_eq!(rest, args(&["land"]));
    assert!(globals.trace);
    assert_eq!(globals.ansi, Some(false));
    let (rest, globals) = strip_globals(&args(&["--ansi"])).unwrap();
    assert!(rest.is_empty());
    assert_eq!(globals.ansi, Some(true));
}

#[test]
fn unitu22_global_flags_strip_after_command() {
    // T1153 UNIT-U22: PHP consumes globals anywhere (`unit --trace`
    // works); `--config` alone is initial-only (PHP throws past the
    // first non-flag word).
    let (rest, globals) = strip_globals(&args(&["unit", "--trace"])).unwrap();
    assert_eq!(rest, args(&["unit"]));
    assert!(globals.trace);
    let (rest, globals) = strip_globals(&args(&["unit", "--conduit-uri", "https://x/"])).unwrap();
    assert_eq!(rest, args(&["unit"]));
    assert_eq!(globals.conduit_uri.as_deref(), Some("https://x/"));
}

#[test]
fn rbd3_strips_valued_and_repeat_globals() {
    // T1153 CORE-34: `--config`/`--config-file`/`--arcrc-file`
    // (plus conduit overrides) strip like PHP; `--config` precedes
    // any non-flag word (initial-only).
    let (rest, globals) = strip_globals(&args(&[
        "--config",
        "base=a",
        "--config",
        "pager=b",
        "--config-file",
        "c.json",
        "--conduit-uri",
        "https://x/",
        "--conduit-token=abc",
        "--arcrc-file",
        "r.json",
        "--anonymous",
        "diff",
    ]))
    .unwrap();
    assert_eq!(rest, args(&["diff"]));
    assert_eq!(globals.conduit_uri.as_deref(), Some("https://x/"));
    assert_eq!(globals.conduit_token.as_deref(), Some("abc"));
    assert_eq!(
        globals.config,
        vec!["base=a".to_string(), "pager=b".to_string()]
    );
    assert_eq!(globals.config_files, vec!["c.json".to_string()]);
    assert_eq!(globals.arcrc_file.as_deref(), Some("r.json"));
    assert!(globals.anonymous);
}

#[test]
fn rbd3_terminator_and_lone_dash_pass_through() {
    let (rest, _) = strip_globals(&args(&["--", "--trace", "land"])).unwrap();
    assert_eq!(rest, args(&["--", "--trace", "land"]));
    let (rest, _) = strip_globals(&args(&["-", "land"])).unwrap();
    assert_eq!(rest, args(&["-", "land"]));
}

#[test]
fn rbd3_standard_spec_errors_are_bold() {
    let error = strip_globals(&args(&["--trace=x"])).unwrap_err();
    assert_eq!(
        error.message,
        "Argument \"--trace\" does not take a parameter."
    );
    assert!(matches!(error.kind, conduit::EnvelopeKind::Usage));
    let error = strip_globals(&args(&["--trace", "--trace"])).unwrap_err();
    assert_eq!(error.message, "Argument \"--trace\" was provided twice.");
    let error = strip_globals(&args(&["--ansi", "--no-ansi"])).unwrap_err();
    assert_eq!(
        error.message,
        "Argument \"--no-ansi\" conflicts with argument \"--ansi\"."
    );
    // Reversed order: no conflict, last flag wins.
    let (_, globals) = strip_globals(&args(&["--no-ansi", "--ansi"])).unwrap();
    assert_eq!(globals.ansi, Some(true));
    let error = strip_globals(&args(&["--xprofile"])).unwrap_err();
    assert_eq!(
        error.message,
        "Argument \"--xprofile\" requires a parameter."
    );
}

#[test]
fn rbd3_config_and_toolset_errors_are_red() {
    let error = strip_globals(&args(&["--conduit-uri"])).unwrap_err();
    assert_eq!(
        error.message,
        "Argument \"--conduit-uri\" requires a parameter."
    );
    assert!(matches!(error.kind, conduit::EnvelopeKind::ToolsetUsage));
    let error = strip_globals(&args(&["land", "--config", "a=b"])).unwrap_err();
    assert!(
        error
            .message
            .contains("appears after the first non-flag argument")
    );
    assert!(matches!(error.kind, conduit::EnvelopeKind::ToolsetUsage));
}

#[test]
fn rbd3_classic_only_errors_use_subprocess_arm() {
    let error = strip_globals(&args(&["--arcrc-file"])).unwrap_err();
    assert_eq!(
        error.message,
        "Argument \"--arcrc-file\" requires a parameter."
    );
    assert!(matches!(error.kind, conduit::EnvelopeKind::Exception));
    let error = strip_globals(&args(&["--anonymous=x"])).unwrap_err();
    assert_eq!(
        error.message,
        "Argument \"--anonymous\" does not take a parameter."
    );
}

#[test]
fn core17_terminator_needed_only_for_toolset_without_dashes() {
    // T1153 CORE-17: piped toolset runs (including `call-conduit`)
    // require `--`; classic commands do not gate here.
    assert!(needs_terminator_error(&args(&["land"])));
    assert!(needs_terminator_error(&args(&["help"])));
    assert!(needs_terminator_error(&args(&["call-conduit", "m"])));
    assert!(!needs_terminator_error(&args(&["land", "--"])));
    assert!(!needs_terminator_error(&args(&["land", "--", "x"])));
    assert!(!needs_terminator_error(&args(&["diff"])));
    assert!(!needs_terminator_error(&args(&["DIFF"])));
    assert!(!needs_terminator_error(&args(&[])));
    assert!(!needs_terminator_error(&args(&["--", "land"])));
}

#[test]
fn core19_all_commands_export_arcanist() {
    // T1153 CORE-19 (R2): PHP putenvs `$command` after `selectWorkflow()`
    // for every command (`scripts/arcanist.php:229`), so hook children of
    // toolset-style commands inherit it too. `help` exercises a
    // non-classic name through the real dispatch (no other test sets the
    // process-once cell, so this read is deterministic).
    let globals = Globals::default();
    let code = dispatch_run("help", None, "help", &args(&["arc"]), &globals);
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(arc_rs::globals::command_name().as_deref(), Some("help"));
    // T1153 CORE-66: 37 concrete workflows at ceb (`browse` is real;
    // only `markers` is abstract).
    assert_eq!(ALL_WORKFLOWS.len(), 37);
}

#[test]
fn core34_config_shape_dup_and_unknown_rejected() {
    let error = validate_runtime_config(&args(&["bad"])).unwrap_err();
    assert!(
        error
            .message
            .contains("Configuration option \"bad\" is not valid.")
    );
    assert!(matches!(error.kind, conduit::EnvelopeKind::ToolsetUsage));
    let error = validate_runtime_config(&args(&["base=a", "base=b"])).unwrap_err();
    assert!(error.message.contains("was provided multiple times"));
    let error = validate_runtime_config(&args(&["history.immutable=true"])).unwrap_err();
    assert!(error.message.contains("is not a recognized option"));
    let map = validate_runtime_config(&args(&["phabricator.uri=https://x/"])).unwrap();
    assert!(map.contains_key("phabricator.uri"));
}

#[test]
fn rbd1_call_conduit_flag_errors_use_parser_texts() {
    let (result, _) = parse_call_conduit_with(&args(&["--bogus", "--", "m"]), false);
    assert_eq!(
        result.unwrap_err(),
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    let (result, _) = parse_call_conduit_with(&args(&["--conduit-uri"]), false);
    assert_eq!(
        result.unwrap_err(),
        "Argument \"--conduit-uri\" requires a parameter."
    );
    let (result, _) = parse_call_conduit_with(
        &args(&["--conduit-uri", "a", "--conduit-uri", "b", "--", "m"]),
        false,
    );
    assert_eq!(
        result.unwrap_err(),
        "Argument \"--conduit-uri\" was provided twice."
    );
    let (result, _) =
        parse_call_conduit_with(&args(&["--conduit-uri=https://x/", "--", "m"]), false);
    assert_eq!(result.unwrap().conduit_uri.as_deref(), Some("https://x/"));
}

#[test]
fn rbd1_call_conduit_flag_correction_reports_notice() {
    let (result, notices) = parse_call_conduit_with(&args(&["--conduit-ur", "u", "--", "m"]), true);
    assert_eq!(result.unwrap().conduit_uri.as_deref(), Some("u"));
    assert_eq!(
        notices,
        vec![
            "(Assuming \"--conduit-ur\" is the British spelling of \"--conduit-uri\".)".to_string()
        ]
    );
    let (result, _) = parse_call_conduit_with(&args(&["--conduit-ur", "u", "--", "m"]), false);
    assert!(result.unwrap_err().contains("is unrecognized"));
}

#[test]
fn core21_call_conduit_count_error_uses_red_arm() {
    assert_eq!(
        dispatch(&args(&["call-conduit", "--bogus", "--", "m"])),
        ExitCode::FAILURE
    );
    assert_eq!(dispatch(&args(&["call-conduit"])), ExitCode::FAILURE);
}

#[test]
fn rbd4_bare_arc_chooses_workflow_without_dump() {
    assert_eq!(dispatch(&args(&[])), ExitCode::FAILURE);
}

#[test]
fn core66_prompts_and_shell_complete_dispatch() {
    assert_eq!(dispatch(&args(&["prompts", "--help"])), ExitCode::SUCCESS);
    assert_eq!(
        dispatch(&args(&["shell-complete", "--help"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn core36_get_config_help_is_ceb_verbatim() {
    // T1153 CORE-36: the `get-config` synopsis and help transcribe
    // `ArcanistGetConfigWorkflow` at ceb byte for byte.
    // Plain-mode render strips console markup, so this locks the ceb
    // wording (entry keeps the `**`/`__` bytes for ANSI bold).
    let brief = render_workflow_help("get-config", false, false);
    assert!(brief.contains("get-config [options] -- [name ...]"));
    let text = render_workflow_help("get-config", true, false);
    assert!(text.contains("Supports: cli"));
    assert!(text.contains("Reads an arc configuration option. With no argument, reads all"));
    assert!(text.contains("With --verbose, shows detailed information about one or more"));
}

#[test]
fn r10_call_conduit_help_matches_generated_format() {
    // T1153 R10: `arc call-conduit --help` reroutes to `arc help
    // call-conduit` (ceb `PhutilArgumentParser.php:461-475`); both render
    // this reference entry (ceb `ArcanistCallConduitWorkflow.php:12-38`).
    assert_eq!(
        render_workflow_help("call-conduit", true, false),
        "call-conduit -- method\n\
         Call Conduit API methods.\n\
         \n\
         Allows you to make a raw Conduit method call:\n\
         \n\
         \x20 - Run this command from a working directory.\n\
         \x20 - Call parameters are required, and read as a JSON blob from stdin.\n\
         \x20 - Results are written to stdout as a JSON blob.\n\
         \n\
         This workflow is primarily useful for writing scripts which integrate with\n\
         Phabricator. Examples:\n\
         \n\
         \x20 $ echo '{}' | arc call-conduit -- conduit.ping\n\
         \x20 $ echo '{\"phid\":\"PHID-FILE-xxxx\"}' | arc call-conduit -- file.download\n"
    );
    let ansi = render_workflow_help("call-conduit", true, true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
}

// -( T1170 coverage: global phases, resolution, dispatch )--------------------

#[test]
#[allow(unpredictable_function_pointer_comparisons)]
fn find_command_ci_matches_case_insensitively() {
    // The lookup returns fn items; comparing identity is the point of this test.
    assert_eq!(find_command_ci("DIFF"), find_command("diff"));
    assert_eq!(find_command_ci("diff"), find_command("diff"));
    assert!(find_command_ci("bogus").is_none());
}

#[test]
fn strip_standard_handles_dash_and_valued_flags() {
    // Any single dash requests help (PHP accepts help flags anywhere).
    let mut globals = Globals::default();
    assert_eq!(
        strip_standard(&args(&["-x", "diff"]), &mut globals).unwrap(),
        args(&["diff"])
    );
    assert!(globals.show_help);
    // `--xprofile` is the only valued standard flag (parsed, ignored).
    let mut globals = Globals::default();
    assert!(
        strip_standard(&args(&["--xprofile=prof"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    let mut globals = Globals::default();
    assert!(
        strip_standard(&args(&["--xprofile", "prof"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    assert!(!globals.show_help);
    // Display-only and PHP-only boolean flags.
    let mut globals = Globals::default();
    assert!(
        strip_standard(&args(&["--show-standard-options"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    assert!(globals.show_standard_options);
    let mut globals = Globals::default();
    assert!(
        strip_standard(&args(&["--recon"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    assert_eq!(globals, Globals::default());
}

#[test]
fn strip_config_takes_inline_values_and_reports_missing() {
    let mut globals = Globals::default();
    assert!(
        strip_config(&args(&["--config=k=v"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    assert_eq!(globals.config, args(&["k=v"]));
    let mut globals = Globals::default();
    assert!(strip_config(&args(&["--config"]), &mut globals).is_err());
}

#[test]
fn strip_toolset_reports_duplicates() {
    let mut globals = Globals::default();
    assert!(strip_toolset(&args(&["--conduit-uri", "a"]), &mut globals).is_ok());
    assert_eq!(globals.conduit_uri.as_deref(), Some("a"));
    let mut globals = Globals::default();
    assert!(
        strip_toolset(
            &args(&["--conduit-uri", "a", "--conduit-uri", "b"]),
            &mut globals
        )
        .is_err()
    );
}

#[test]
fn strip_classic_consumes_arcrc_and_anonymous() {
    let mut globals = Globals::default();
    assert!(
        strip_classic(&args(&["--arcrc-file=/tmp/x"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    assert_eq!(globals.arcrc_file.as_deref(), Some("/tmp/x"));
    let mut globals = Globals::default();
    assert!(
        strip_classic(
            &args(&["--arcrc-file", "a", "--arcrc-file", "b"]),
            &mut globals
        )
        .is_err()
    );
    let mut globals = Globals::default();
    assert!(strip_classic(&args(&["--anonymous", "--anonymous"]), &mut globals).is_err());
    let mut globals = Globals::default();
    assert!(strip_classic(&args(&["--anonymous"]), &mut globals).is_ok());
    assert!(globals.anonymous);
}

#[test]
fn strip_classic_ignores_phutil_library() {
    let mut globals = Globals::default();
    assert!(
        strip_classic(&args(&["--load-phutil-library=x"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    let mut globals = Globals::default();
    assert!(
        strip_classic(&args(&["--load-phutil-library", "x"]), &mut globals)
            .unwrap()
            .is_empty()
    );
    let mut globals = Globals::default();
    assert!(strip_classic(&args(&["--load-phutil-library"]), &mut globals).is_err());
}

#[test]
fn resolve_command_corrects_british_spellings() {
    let (name, notice) = expect_run(resolve_command("dift", false));
    assert_eq!(name, "diff");
    assert!(notice.unwrap().contains("dift"), "typo carries a notice");
    let message = expect_unknown(resolve_command("tl", false));
    assert!(message.contains("Did you mean"), "{message}");
    assert!(
        message.contains("lint") && message.contains("list"),
        "{message}"
    );
}

#[test]
fn dispatch_reports_global_and_config_errors() {
    assert_eq!(dispatch(&args(&["--config"])), ExitCode::FAILURE);
    assert_eq!(
        dispatch(&args(&["--config", "bad-value"])),
        ExitCode::FAILURE
    );
}

#[test]
fn help_flag_with_unknown_command_reports_usage() {
    assert_eq!(
        dispatch_help_flag(&args(&["bogus"]), &Globals::default()),
        ExitCode::FAILURE
    );
}

#[test]
fn dispatch_action_renders_help_version_and_missing() {
    assert_eq!(
        dispatch_inner(&args(&["-h"]), &Globals::default()),
        ExitCode::SUCCESS
    );
    assert_eq!(
        dispatch_inner(&args(&["--version"]), &Globals::default()),
        ExitCode::SUCCESS
    );
    assert_eq!(
        dispatch_inner(&args(&["call-conduit", "--help"]), &Globals::default()),
        ExitCode::SUCCESS
    );
    assert_eq!(
        dispatch_action(Action::Missing, &args(&[]), &Globals::default()),
        ExitCode::FAILURE
    );
}

#[test]
fn call_conduit_setup_failure_is_fatal() {
    let call_args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some("::not-a-uri".to_string()),
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&call_args, &b"{}"[..], &mut out),
        ExitCode::from(255)
    );
    assert!(out.is_empty());
}

// -( T1170 coverage: call-conduit over loopback )------------------------------

struct FailWriter;

impl std::io::Write for FailWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("t1170 sink closed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serve one canned Conduit JSON body over loopback HTTP, then close.
/// Returns the base URI plus the server thread (join it to surface
/// panics). Every line below runs on each use (no optional arms).
fn serve_once(body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        for _ in 0..1000 {
            if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
            let count = stream.read(&mut buf).unwrap();
            raw.extend_from_slice(&buf[..count]);
        }
        let text = String::from_utf8_lossy(&raw);
        let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let length: usize = text
            .to_ascii_lowercase()
            .lines()
            .filter_map(|line| {
                line.strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse().ok())
            })
            .next()
            .unwrap_or(0);
        let mut rest = vec![0u8; head_end + length - raw.len()];
        stream.read_exact(&mut rest).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    (uri, handle)
}

#[test]
fn call_conduit_api_error_lands_in_envelope() {
    let (uri, server) = serve_once(r#"{"error_code": "ERR-FOO", "error_info": "boom"}"#);
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some(uri),
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"{}"[..], &mut out),
        ExitCode::SUCCESS
    );
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("ERR-FOO: boom"), "{text}");
    server.join().unwrap();
}

#[test]
fn call_conduit_auth_error_reports_guidance() {
    let (uri, server) = serve_once(r#"{"error_code": "ERR-INVALID-AUTH", "error_info": "bad"}"#);
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some(uri),
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"{}"[..], &mut out),
        ExitCode::FAILURE
    );
    assert!(out.is_empty());
    server.join().unwrap();
}

#[test]
fn call_conduit_write_failure_is_fatal() {
    let (uri, server) = serve_once(r#"{"error_code": "ERR-FOO", "error_info": "boom"}"#);
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some(uri),
    };
    let mut out = FailWriter;
    assert_eq!(
        run_call_conduit_with(&args, &b"{}"[..], &mut out),
        ExitCode::FAILURE
    );
    assert!(std::io::Write::flush(&mut out).is_ok());
    server.join().unwrap();
}

// -( T1170 wave 5d )-----------------------------------------------------------

#[test]
fn core19_dispatch_run_prints_spelling_notice() {
    // A resolved command with a spelling notice prints the notice
    // before running. `help` renders without side effects; the name
    // matches the CORE-19 pin, so the process-once command cell stays
    // deterministic no matter which test runs first.
    let globals = Globals::default();
    let code = dispatch_run(
        "help",
        Some("(Assuming \"hep\" is the British spelling of \"help\".)"),
        "hep",
        &args(&["arc"]),
        &globals,
    );
    assert_eq!(code, ExitCode::SUCCESS);
}
