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
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: None,
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"nope"[..], &mut out),
        ExitCode::FAILURE
    );
    assert!(out.is_empty());
}

#[test]
fn call_conduit_execute_reports_missing_token_without_io() {
    let args = CallArgs {
        method: "m".to_string(),
        conduit_uri: Some("https://call-conduit-test.invalid/".to_string()),
    };
    let mut out = Vec::new();
    assert_eq!(
        run_call_conduit_with(&args, &b"{}"[..], &mut out),
        ExitCode::FAILURE
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
    let (aliases, warnings) = alias::aliases_from_storage(&value, "user (test)");
    assert!(warnings.is_empty());
    aliases
}

fn dispatch_target(argv: &[String]) -> &'static str {
    let cmd = argv.first().expect("expanded argv has a command");
    COMMANDS
        .iter()
        .find(|(name, _)| *name == cmd.as_str())
        .map(|(name, _)| *name)
        .unwrap_or_else(|| panic!("{cmd} is not registered"))
}

#[test]
fn stored_alias_expands_through_main_dispatch() {
    for stored in [
        serde_json::json!({"draft": ["diff", "--draft"]}),
        serde_json::json!([{"trigger": "draft", "toolset": "arc", "command": ["diff", "--draft"]}]),
    ] {
        let aliases = stored_aliases(stored);
        let expanded = pre_dispatch(args(&["draft", "extra"]), &aliases);
        let alias::Expansion::Dispatch { argv, hints } = expanded else {
            panic!("expected dispatch, got {expanded:?}");
        };
        assert_eq!(argv, args(&["diff", "--draft", "extra"]));
        assert_eq!(hints.len(), 1);
        assert_eq!(dispatch_target(&argv), diff::NAME);
    }
}

#[test]
fn pre_dispatch_leaves_builtins_and_flags_alone() {
    let aliases = stored_aliases(serde_json::json!({"diff": ["lint"]}));
    assert_eq!(
        pre_dispatch(args(&["diff"]), &aliases),
        alias::Expansion::Dispatch {
            argv: args(&["diff"]),
            hints: Vec::new(),
        }
    );
    assert_eq!(
        pre_dispatch(args(&["--help"]), &aliases),
        alias::Expansion::Dispatch {
            argv: args(&["--help"]),
            hints: Vec::new(),
        }
    );
    assert_eq!(
        pre_dispatch(args(&["bogus"]), &aliases),
        alias::Expansion::Dispatch {
            argv: args(&["bogus"]),
            hints: Vec::new(),
        }
    );
}

#[test]
fn pre_dispatch_reports_shell_and_broken_chains() {
    let aliases = stored_aliases(serde_json::json!({"ls": ["!ls"]}));
    let expanded = pre_dispatch(args(&["ls", "-la"]), &aliases);
    assert_eq!(
        expanded,
        alias::Expansion::Shell {
            argv: args(&["ls", "-la"]),
            hint: "arc ls -> $ ls -la".to_string(),
        }
    );

    let aliases = stored_aliases(serde_json::json!({"a": ["gone"]}));
    assert!(matches!(
        pre_dispatch(args(&["a"]), &aliases),
        alias::Expansion::Failed { .. }
    ));
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
