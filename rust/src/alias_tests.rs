use super::*;
use std::io::Cursor;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn alias(toolset: &str, trigger: &str, command: &[&str], source: &str) -> Alias {
    Alias {
        toolset: toolset.to_string(),
        trigger: trigger.to_string(),
        command: args(command),
        source: source.to_string(),
    }
}

fn builtin(name: &str) -> bool {
    is_builtin_command(name)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-alias-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn arcrc(&self) -> PathBuf {
        self.path.join(".arcrc")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn builtin_set_covers_registered_commands() {
    for name in [
        "alias",
        "amend",
        "diff",
        "close-revision",
        "get-config",
        "install-certificate",
        "set-config",
        "work",
        "call-conduit",
        "help",
    ] {
        assert!(is_builtin_command(name), "{name}");
    }
    assert!(!is_builtin_command("draft"));
    assert!(!is_builtin_command(""));
    assert!(!is_builtin_command("--help"));
}

#[test]
fn parses_old_and_new_storage_formats() {
    let old = serde_json::json!(["diff", "--draft"]);
    let parsed = parse_entry("draft", &old).unwrap();
    assert_eq!(parsed.toolset, "arc");
    assert_eq!(parsed.trigger, "draft");
    assert_eq!(parsed.command, args(&["diff", "--draft"]));

    let new = serde_json::json!({
        "trigger": "draft",
        "toolset": "arc",
        "command": ["diff", "--draft"],
    });
    assert_eq!(parse_entry("0", &new).unwrap().trigger, "draft");

    assert!(parse_entry("bad", &serde_json::json!("diff")).is_err());
    assert!(parse_entry("bad", &serde_json::json!(42)).is_err());
    assert!(
        parse_entry(
            "bad",
            &serde_json::json!({"trigger": "x", "toolset": "arc", "command": "diff"}),
        )
        .is_err()
    );
    assert!(
        parse_entry(
            "bad",
            &serde_json::json!({"trigger": "x", "toolset": "arc"}),
        )
        .is_err()
    );
    assert!(parse_entry("bad", &serde_json::json!(["diff", 42])).is_err());
}

#[test]
fn storage_parse_warns_and_ignores_invalid_entries() {
    let value = serde_json::json!([
        {"trigger": "draft", "toolset": "arc", "command": ["diff"]},
        {"trigger": "broken", "toolset": "arc"},
    ]);
    let (aliases, warnings) = aliases_from_storage(&value, "user (u)").unwrap();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].trigger, "draft");
    assert_eq!(aliases[0].source, "user (u)");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("user (u)"), "{}", warnings[0]);
    assert!(warnings[0].contains("ignored"), "{}", warnings[0]);
}

#[test]
fn storage_parse_rejects_non_array_values() {
    // PHP-fatal: `ArcanistAliasesConfigOption` throws here and startup
    // validation does not catch it, so a non-array whole value errors
    // instead of warning.
    for value in [
        serde_json::json!("nope"),
        serde_json::json!(42),
        serde_json::json!(null),
        serde_json::json!(true),
    ] {
        let err = aliases_from_storage(&value, "user (u)").unwrap_err();
        assert!(err.contains("user (u)"), "{err}");
        assert!(err.contains("is not valid"), "{err}");
        assert!(err.contains("Expected a list or dictionary!"), "{err}");
    }
}

#[test]
fn detects_shell_commands() {
    assert!(is_shell_command(&args(&["!ls"])));
    assert!(is_shell_command(&args(&["!ls", "-la"])));
    assert!(!is_shell_command(&args(&["diff"])));
    assert!(!is_shell_command(&[]));
    assert!(!is_shell_command(&args(&["diff", "!x"])));
}

#[test]
fn expansion_passes_builtins_through_untouched() {
    let aliases = vec![alias("arc", "diff", &["lint"], "user")];
    assert_eq!(
        expand(&args(&["diff", "--lintall"]), &aliases, builtin),
        Expansion::Dispatch {
            argv: args(&["diff", "--lintall"]),
            hints: Vec::new(),
        }
    );
}

#[test]
fn expansion_appends_user_args_after_alias_args() {
    let aliases = vec![alias("arc", "draft", &["diff", "--draft"], "user")];
    let expanded = expand(&args(&["draft", "extra"]), &aliases, builtin);
    let Expansion::Dispatch { argv, hints } = expanded else {
        panic!("expected dispatch, got {expanded:?}");
    };
    assert_eq!(argv, args(&["diff", "--draft", "extra"]));
    assert_eq!(hints.len(), 1);
    assert!(
        hints[0].contains("arc draft -> arc diff --draft"),
        "{}",
        hints[0]
    );
}

#[test]
fn expansion_resolves_chains() {
    let aliases = vec![
        alias("arc", "a", &["b", "--from-a"], "user"),
        alias("arc", "b", &["diff"], "user"),
    ];
    let expanded = expand(&args(&["a", "user-arg"]), &aliases, builtin);
    let Expansion::Dispatch { argv, hints } = expanded else {
        panic!("expected dispatch, got {expanded:?}");
    };
    assert_eq!(argv, args(&["diff", "--from-a", "user-arg"]));
    assert_eq!(hints.len(), 2);
}

#[test]
fn expansion_resolves_shell_aliases() {
    let aliases = vec![alias("arc", "ls", &["!ls", "-la"], "user")];
    let expanded = expand(&args(&["ls", "/tmp"]), &aliases, builtin);
    let Expansion::Shell { argv, hint } = expanded else {
        panic!("expected shell, got {expanded:?}");
    };
    assert_eq!(argv, args(&["ls", "-la", "/tmp"]));
    assert!(hint.contains("arc ls -> $ 'ls' '-la' '/tmp'"), "{hint}");
}

#[test]
fn expansion_quotes_shell_display_words() {
    let aliases = vec![alias("arc", "g", &["!echo", "a b"], "user")];
    let Expansion::Shell { argv, hint } = expand(&args(&["g"]), &aliases, builtin) else {
        panic!("expected shell");
    };
    assert_eq!(argv, args(&["echo", "a b"]));
    assert!(hint.contains("'echo' 'a b'"), "{hint}");
}

#[test]
fn display_quoting_matches_php_readable_charset() {
    // PHP `%R` stays bare only for `^[a-zA-Z0-9:/@._+-]+$`
    // (`xsprintf_command`); `%`, `=`, `,` and friends quote.
    for bare in [
        "ls",
        "-la",
        "--draft",
        "/tmp/x",
        "a:b@c.d_e+f-g",
        "9",
        "UPPER",
    ] {
        assert_eq!(shell_quote(bare), bare, "{bare}");
    }
    assert_eq!(shell_quote(""), "''");
    assert_eq!(shell_quote("a b"), "'a b'");
    assert_eq!(shell_quote("a=b"), "'a=b'");
    assert_eq!(shell_quote("100%"), "'100%'");
    assert_eq!(shell_quote("a,b"), "'a,b'");
    assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    assert_eq!(shell_quote("$HOME"), "'$HOME'");
    assert_eq!(
        shell_display(&args(&["echo", "a b", "--x"])),
        "'echo' 'a b' '--x'"
    );
    assert_eq!(shell_display(&[]), "");
}

#[test]
fn expansion_quotes_effect_alias_tail_like_pct_lr() {
    // PHP renders the `EFFECT_ALIAS` tail with `%LR`: words needing
    // quoting are quoted, safe words stay bare.
    let aliases = vec![alias("arc", "draft", &["diff", "a b", "--x"], "user")];
    let expanded = expand(&args(&["draft"]), &aliases, builtin);
    let Expansion::Dispatch { argv, hints } = expanded else {
        panic!("expected dispatch, got {expanded:?}");
    };
    assert_eq!(argv, args(&["diff", "a b", "--x"]));
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0], "arc draft -> arc diff 'a b' --x");

    let aliases = vec![alias("arc", "eq", &["diff", "a=b"], "user")];
    let Expansion::Dispatch { hints, .. } = expand(&args(&["eq"]), &aliases, builtin) else {
        panic!("expected dispatch");
    };
    assert_eq!(hints[0], "arc eq -> arc diff 'a=b'");
}

#[test]
fn expansion_reports_cycles() {
    let aliases = vec![
        alias("arc", "a", &["b"], "user"),
        alias("arc", "b", &["a"], "user"),
    ];
    let expanded = expand(&args(&["a"]), &aliases, builtin);
    let Expansion::Failed { message, .. } = expanded else {
        panic!("expected failure, got {expanded:?}");
    };
    assert!(message.contains("cycle"), "{message}");
    assert!(message.contains("b -> a -> b"), "{message}");
}

#[test]
fn expansion_reports_self_cycles() {
    let aliases = vec![alias("arc", "loop", &["loop"], "user")];
    let expanded = expand(&args(&["loop"]), &aliases, builtin);
    let Expansion::Failed { message, hints } = expanded else {
        panic!("expected failure, got {expanded:?}");
    };
    assert!(message.contains("cycle"), "{message}");
    assert!(message.contains("loop -> loop"), "{message}");
    assert_eq!(hints.len(), 1);
}

#[test]
fn expansion_chain_ending_at_builtin_keeps_every_hint() {
    let aliases = vec![
        alias("arc", "a", &["b"], "user"),
        alias("arc", "b", &["diff", "--all"], "user"),
    ];
    let expanded = expand(&args(&["a", "user-arg"]), &aliases, builtin);
    let Expansion::Dispatch { argv, hints } = expanded else {
        panic!("expected dispatch, got {expanded:?}");
    };
    assert_eq!(argv, args(&["diff", "--all", "user-arg"]));
    assert_eq!(
        hints,
        vec![
            "arc a -> arc b ".to_string(),
            "arc b -> arc diff --all".to_string(),
        ]
    );
}

#[test]
fn expansion_multi_hop_to_shell_keeps_only_the_shell_hint() {
    // PHP parity: `ArcanistRuntime` returns `executeShellAlias()` on the
    // first `EFFECT_SHELL` and prints only its message, so earlier
    // `EFFECT_ALIAS` hints never surface for shell chains.
    let aliases = vec![
        alias("arc", "a", &["b", "--from-a"], "user"),
        alias("arc", "b", &["!echo", "hi"], "user"),
    ];
    let expanded = expand(&args(&["a", "user-arg"]), &aliases, builtin);
    let Expansion::Shell { argv, hint } = expanded else {
        panic!("expected shell, got {expanded:?}");
    };
    assert_eq!(argv, args(&["echo", "hi", "--from-a", "user-arg"]));
    assert_eq!(hint, "arc b -> $ 'echo' 'hi' '--from-a' 'user-arg'");
}

#[test]
fn expansion_reports_dangling_chains_but_passes_unknown_through() {
    let aliases = vec![alias("arc", "a", &["gone"], "user")];
    let expanded = expand(&args(&["a"]), &aliases, builtin);
    let Expansion::Failed { message, hints } = expanded else {
        panic!("expected failure, got {expanded:?}");
    };
    assert!(message.contains("Alias resolved to \"gone\""), "{message}");
    assert_eq!(hints.len(), 1);

    let expanded = expand(&args(&["hlep"]), &aliases, builtin);
    assert_eq!(
        expanded,
        Expansion::Dispatch {
            argv: args(&["hlep"]),
            hints: Vec::new(),
        }
    );
}

#[test]
fn expansion_suggests_cross_toolset_aliases() {
    let aliases = vec![alias("phage", "draft", &["diff"], "user")];
    let expanded = expand(&args(&["draft"]), &aliases, builtin);
    let Expansion::Dispatch { argv, hints } = expanded else {
        panic!("expected dispatch, got {expanded:?}");
    };
    assert_eq!(argv, args(&["draft"]));
    assert_eq!(hints.len(), 1);
    assert!(hints[0].contains("did you mean"), "{}", hints[0]);
    assert!(hints[0].contains("phage draft"), "{}", hints[0]);
}

#[test]
fn expansion_prefers_last_definition_and_hints_shadowed_ones() {
    let aliases = vec![
        alias("arc", "draft", &["lint"], "user"),
        alias("arc", "draft", &["diff"], "local"),
    ];
    let expanded = expand(&args(&["draft"]), &aliases, builtin);
    let Expansion::Dispatch { argv, hints } = expanded else {
        panic!("expected dispatch, got {expanded:?}");
    };
    assert_eq!(argv, args(&["diff"]));
    assert!(hints.iter().any(|h| h.contains("Multiple")), "{hints:?}");
    assert!(
        hints.iter().any(|h| h.contains("will be ignored")),
        "{hints:?}"
    );
}

#[test]
fn expansion_enforces_stack_limit() {
    let mut aliases = Vec::new();
    for index in 0..20 {
        aliases.push(Alias {
            toolset: TOOLSET.to_string(),
            trigger: format!("a{index}"),
            command: vec![format!("a{}", index + 1)],
            source: "user".to_string(),
        });
    }
    let expanded = expand(&args(&["a0"]), &aliases, builtin);
    let Expansion::Failed { message, .. } = expanded else {
        panic!("expected failure, got {expanded:?}");
    };
    assert!(message.contains("unreasonably deep"), "{message}");
}

#[test]
fn expansion_handles_empty_argv_and_empty_commands() {
    assert_eq!(
        expand(&[], &[], builtin),
        Expansion::Dispatch {
            argv: Vec::new(),
            hints: Vec::new(),
        }
    );
    let aliases = vec![alias("arc", "empty", &[], "user")];
    assert!(matches!(
        expand(&args(&["empty"]), &aliases, builtin),
        Expansion::Failed { .. }
    ));
}

#[test]
fn effective_aliases_dedupe_and_sort() {
    let aliases = vec![
        alias("arc", "zebra", &["diff"], "user"),
        alias("phage", "aaa", &["diff"], "user"),
        alias("arc", "apple", &["lint"], "system"),
        alias("arc", "apple", &["diff"], "user"),
    ];
    let effective = effective_aliases(&aliases);
    let triggers: Vec<&str> = effective.iter().map(|a| a.trigger.as_str()).collect();
    assert_eq!(triggers, vec!["apple", "zebra"]);
    assert_eq!(effective[0].command, args(&["diff"]));
}

#[test]
fn renders_list_table_and_empty_state() {
    // T1153 CORE-26 (kept): PHP's list stub is silent; Rust lists.
    assert_eq!(render_list(&[]), "You have not defined any aliases yet.\n");
    let aliases = vec![
        alias("arc", "ls", &["!ls"], "user"),
        alias("arc", "draft", &["diff", "--draft"], "user"),
    ];
    let effective = effective_aliases(&aliases);
    let text = render_list(&effective);
    assert!(text.contains("Alias"), "{text}");
    assert!(text.contains("arc draft"), "{text}");
    assert!(text.contains("arc diff --draft"), "{text}");
    assert!(text.contains("Arcanist Command"), "{text}");
    assert!(text.contains("arc ls"), "{text}");
    assert!(text.contains("Shell Command"), "{text}");
    assert!(!text.contains('!'), "{text}");
    assert!(text.find("arc draft").unwrap() < text.find("arc ls").unwrap());
}

#[test]
fn builds_json_document() {
    let aliases = vec![alias("arc", "draft", &["diff"], "user")];
    let effective = effective_aliases(&aliases);
    assert_eq!(
        aliases_json(&effective),
        serde_json::json!([{
            "trigger": "draft",
            "toolset": "arc",
            "command": ["diff"],
        }])
    );
}

#[test]
fn parses_args_with_terminator_and_json() {
    assert_eq!(
        parse_args(&args(&[])).unwrap().unwrap(),
        Options {
            json: false,
            positional: Vec::new(),
        }
    );
    let options = parse_args(&args(&["draft", "diff", "--", "--draft"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.positional, args(&["draft", "diff", "--draft"]));
    assert!(!options.json);
    let options = parse_args(&args(&["--json"])).unwrap().unwrap();
    assert!(options.json);
    assert!(parse_args(&args(&["--help"])).unwrap().is_none());
    assert!(parse_args(&args(&["-h"])).unwrap().is_none());
    assert!(
        parse_args(&args(&["--bogus"]))
            .unwrap_err()
            .contains("unknown flag")
    );
}

#[test]
fn validates_builtin_triggers() {
    assert!(validate_trigger("draft").is_ok());
    let err = validate_trigger("diff").unwrap_err();
    assert!(
        err.contains("You can not define an alias for \"diff\""),
        "{err}"
    );
    assert!(validate_trigger("alias").is_err());
    assert!(validate_trigger("call-conduit").is_err());
    assert!(validate_trigger("help").is_err());
}

#[test]
fn create_stores_new_format_and_preserves_rest_of_file() {
    let tmp = TempDir::new("create");
    let arcrc = tmp.arcrc();
    std::fs::write(
        &arcrc,
        r#"{"hosts": {"x": 1}, "config": {"editor": "vim"}, "aliases": {"old": ["diff"]}}"#,
    )
    .unwrap();

    let report = execute_create(&arcrc, "draft", &args(&["diff", "--draft"])).unwrap();
    // T1153 CORE-25: creation appends silently (no confirmation text).
    assert_eq!(report.text, "");
    assert!(report.warnings.is_empty());

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&arcrc).unwrap()).unwrap();
    assert_eq!(doc["hosts"], serde_json::json!({"x": 1}));
    assert_eq!(doc["config"], serde_json::json!({"editor": "vim"}));
    assert_eq!(
        doc["aliases"],
        serde_json::json!([
            {"trigger": "old", "toolset": "arc", "command": ["diff"]},
            {"trigger": "draft", "toolset": "arc", "command": ["diff", "--draft"]},
        ])
    );
}

#[test]
fn create_replaces_previous_definition_and_reports_shell() {
    let tmp = TempDir::new("replace");
    let arcrc = tmp.arcrc();
    execute_create(&arcrc, "ls", &args(&["!ls"])).unwrap();
    let report = execute_create(&arcrc, "ls", &args(&["!ls", "-la"])).unwrap();
    // T1153 CORE-25: creation appends silently (no confirmation text).
    assert_eq!(report.text, "");

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&arcrc).unwrap()).unwrap();
    // T1153 CORE-25: duplicates accumulate (the engine last-wins).
    assert_eq!(
        doc["aliases"],
        serde_json::json!([
            {"trigger": "ls", "toolset": "arc", "command": ["!ls"]},
            {"trigger": "ls", "toolset": "arc", "command": ["!ls", "-la"]},
        ])
    );
}

#[test]
fn create_rejects_builtins_and_bad_storage() {
    let tmp = TempDir::new("reject");
    let arcrc = tmp.arcrc();
    assert!(execute_create(&arcrc, "diff", &args(&["lint"])).is_err());
    assert!(!arcrc.is_file());

    std::fs::write(&arcrc, r#"{"aliases": 42}"#).unwrap();
    assert!(execute_create(&arcrc, "draft", &args(&["diff"])).is_err());
}

#[test]
fn delete_removes_after_confirmation() {
    let tmp = TempDir::new("delete");
    let arcrc = tmp.arcrc();
    execute_create(&arcrc, "draft", &args(&["diff"])).unwrap();

    let mut input = Cursor::new(b"y\n".to_vec());
    let report = execute_delete(&arcrc, "draft", &mut input, true).unwrap();
    assert!(
        report.text.contains("currently aliased to \"arc diff\""),
        "{}",
        report.text
    );
    assert!(
        report.text.contains("Removed alias \"arc draft\"."),
        "{}",
        report.text
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&arcrc).unwrap()).unwrap();
    assert_eq!(doc["aliases"], serde_json::json!([]));
}

#[test]
fn delete_aborts_on_decline_and_reports_missing() {
    let tmp = TempDir::new("decline");
    let arcrc = tmp.arcrc();
    execute_create(&arcrc, "draft", &args(&["diff"])).unwrap();

    let mut input = Cursor::new(b"n\n".to_vec());
    assert_eq!(
        execute_delete(&arcrc, "draft", &mut input, true).unwrap_err(),
        "User aborted the workflow."
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&arcrc).unwrap()).unwrap();
    assert_eq!(doc["aliases"].as_array().unwrap().len(), 1);

    let mut input = Cursor::new(b"y\n".to_vec());
    let report = execute_delete(&arcrc, "missing", &mut input, true).unwrap();
    assert_eq!(report.text, "No alias \"missing\" to remove.\n");
}

#[test]
fn delete_requires_interactive_stdin() {
    let tmp = TempDir::new("tty");
    let arcrc = tmp.arcrc();
    execute_create(&arcrc, "draft", &args(&["diff"])).unwrap();
    let mut input = Cursor::new(b"y\n".to_vec());
    assert!(execute_delete(&arcrc, "draft", &mut input, false).is_err());
}

#[test]
fn list_reads_user_file_and_json() {
    let tmp = TempDir::new("list");
    let arcrc = tmp.arcrc();
    execute_create(&arcrc, "draft", &args(&["diff"])).unwrap();

    let report = execute_list(tmp.path(), Some(&arcrc), false).unwrap();
    assert!(report.text.contains("arc draft"), "{}", report.text);

    let report = execute_list(tmp.path(), Some(&arcrc), true).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&report.text).unwrap();
    assert_eq!(
        doc,
        serde_json::json!([{
            "trigger": "draft",
            "toolset": "arc",
            "command": ["diff"],
        }])
    );

    let report = execute_list(tmp.path(), None, false).unwrap();
    assert_eq!(report.text, "You have not defined any aliases yet.\n");
}

#[test]
fn load_merges_user_aliases() {
    let tmp = TempDir::new("load");
    let arcrc = tmp.arcrc();
    std::fs::write(&arcrc, r#"{"aliases": {"draft": ["diff"]}}"#).unwrap();
    let (aliases, warnings) = load_aliases_in(tmp.path(), Some(&arcrc)).unwrap();
    assert!(warnings.is_empty());
    assert!(
        aliases
            .iter()
            .any(|a| a.trigger == "draft" && a.toolset == "arc")
    );
}

#[test]
fn list_fails_on_non_array_aliases_value() {
    let tmp = TempDir::new("badvalue");
    let arcrc = tmp.arcrc();
    std::fs::write(&arcrc, r#"{"aliases": 42}"#).unwrap();
    let err = execute_list(tmp.path(), Some(&arcrc), false).unwrap_err();
    assert!(err.contains("is not valid"), "{err}");
    assert!(err.contains("Expected a list or dictionary!"), "{err}");
}

#[test]
fn shell_status_mapping_passes_codes_and_fails_signals() {
    assert_eq!(
        shell_status_exit_code(ShellStatus::Exited(0)),
        ExitCode::SUCCESS
    );
    assert_eq!(
        shell_status_exit_code(ShellStatus::Exited(3)),
        ExitCode::from(3)
    );
    // Unrepresentable codes fail instead of wrapping.
    assert_eq!(
        shell_status_exit_code(ShellStatus::Exited(256)),
        ExitCode::FAILURE
    );
    assert_eq!(
        shell_status_exit_code(ShellStatus::Exited(-1)),
        ExitCode::FAILURE
    );
    // Signal deaths have no single PHP value (`proc_close()` returns the
    // raw wait status, `-1`, or `128+N` depending on build and shell), so
    // they map to a fixed failure.
    assert_eq!(
        shell_status_exit_code(ShellStatus::Signaled(9)),
        ExitCode::FAILURE
    );
    assert_eq!(
        shell_status_exit_code(ShellStatus::Signaled(15)),
        ExitCode::FAILURE
    );
}

#[cfg(unix)]
#[test]
fn shell_status_from_real_children() {
    let exited = std::process::Command::new("sh")
        .args(["-c", "exit 3"])
        .status()
        .unwrap();
    assert_eq!(
        shell_status_from_exit_status(&exited),
        ShellStatus::Exited(3)
    );
    let signaled = std::process::Command::new("sh")
        .args(["-c", "kill -TERM $$"])
        .status()
        .unwrap();
    assert_eq!(
        shell_status_from_exit_status(&signaled),
        ShellStatus::Signaled(15)
    );
}

#[test]
fn shell_runner_maps_spawn_outcomes() {
    let (code, notices) = run_shell_alias_with(
        &args(&["echo", "hi"]),
        "arc g -> $ echo hi",
        |program, argv| {
            // T1153 CORE-23: Unix mediates the shell (`sh -c`).
            if cfg!(unix) {
                assert_eq!(program, "sh");
                assert_eq!(argv, &["-c".to_string(), "'echo' 'hi'".to_string()]);
            } else {
                assert_eq!(program, "echo");
                assert_eq!(argv, &["hi".to_string()]);
            }
            Ok(ShellStatus::Exited(0))
        },
    );
    assert_eq!(code, ExitCode::SUCCESS);
    // T1153 CORE-23: cyan log-engine hint (` SHELL ALIAS  ...` plain).
    assert_eq!(
        notices,
        vec![" SHELL ALIAS  arc g -> $ echo hi".to_string()]
    );

    let (code, notices) =
        run_shell_alias_with(&args(&["echo"]), "h", |_, _| Ok(ShellStatus::Exited(7)));
    assert_eq!(code, ExitCode::from(7));
    assert_eq!(notices.len(), 1);

    let (code, notices) =
        run_shell_alias_with(&args(&["echo"]), "h", |_, _| Ok(ShellStatus::Signaled(9)));
    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(notices.len(), 1);

    let (code, notices) = run_shell_alias_with(&args(&["missing"]), "h", |_, _| {
        Err::<ShellStatus, String>("not found".to_string())
    });
    assert_eq!(code, ExitCode::FAILURE);
    assert_eq!(notices.len(), 2);
    assert!(
        notices[1].contains("cannot execute shell alias"),
        "{}",
        notices[1]
    );
    assert!(notices[1].contains("not found"), "{}", notices[1]);

    // T1153 CORE-23: an empty shell command exits 0 like PHP's
    // `passthru('')`.
    let (code, notices) = run_shell_alias_with(&[], "h", |_, _| Ok(ShellStatus::Exited(0)));
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(notices.len(), 1);
}

fn loaded(aliases: Vec<Alias>) -> Option<(Vec<Alias>, Vec<String>)> {
    Some((aliases, Vec::new()))
}

#[test]
fn expansion_driver_dispatches_and_reports_shell_and_broken_chains() {
    let aliases = vec![alias("arc", "draft", &["diff", "--draft"], "user")];
    let (result, notices) = apply_expansion_with(
        args(&["draft", "extra"]),
        || loaded(aliases),
        |_| panic!("must not spawn for dispatch"),
    );
    assert_eq!(result, Ok(args(&["diff", "--draft", "extra"])));
    // T1153 CORE-23: cyan log-engine hint (` ALIAS  ...` plain).
    assert_eq!(
        notices,
        vec![" ALIAS  arc draft -> arc diff --draft".to_string()]
    );

    let aliases = vec![alias("arc", "g", &["!echo", "hi"], "user")];
    let (result, notices) = apply_expansion_with(
        args(&["g"]),
        || loaded(aliases),
        |shell| {
            // T1153 CORE-23: Unix mediates the shell (`sh -c`).
            if cfg!(unix) {
                assert_eq!(shell, &args(&["sh", "-c", "'echo' 'hi'"]));
            } else {
                assert_eq!(shell, &args(&["echo", "hi"]));
            }
            Ok(ShellStatus::Exited(0))
        },
    );
    assert_eq!(result, Err(ExitCode::SUCCESS));
    // T1153 CORE-23: cyan log-engine hint (` SHELL ALIAS  ...` plain).
    assert_eq!(
        notices,
        vec![" SHELL ALIAS  arc g -> $ 'echo' 'hi'".to_string()]
    );

    let aliases = vec![alias("arc", "g", &["!echo"], "user")];
    let (result, notices) = apply_expansion_with(
        args(&["g"]),
        || loaded(aliases),
        |_| Ok(ShellStatus::Exited(4)),
    );
    assert_eq!(result, Err(ExitCode::from(4)));
    assert_eq!(notices.len(), 1);

    let aliases = vec![alias("arc", "a", &["gone"], "user")];
    let (result, notices) = apply_expansion_with(
        args(&["a"]),
        || loaded(aliases),
        |_| panic!("must not spawn for broken chains"),
    );
    assert_eq!(result, Err(ExitCode::FAILURE));
    assert_eq!(notices.len(), 2);
    // T1153 CORE-23: cyan log-engine hint (` ALIAS  ...` plain).
    assert!(notices[0].starts_with(" ALIAS  "), "{}", notices[0]);
    assert!(
        notices[1].contains("Alias resolved to \"gone\""),
        "{}",
        notices[1]
    );
}

#[test]
fn expansion_driver_passes_through_flags_and_unreadable_config() {
    let (result, notices) = apply_expansion_with(
        args(&["--help"]),
        || panic!("must not load"),
        |_| panic!("must not spawn"),
    );
    assert_eq!(result, Ok(args(&["--help"])));
    assert!(notices.is_empty());

    let (result, notices) =
        apply_expansion_with(args(&["diff"]), || None, |_| panic!("must not spawn"));
    assert_eq!(result, Ok(args(&["diff"])));
    assert!(notices.is_empty());
}

#[test]
fn expansion_driver_emits_configuration_warnings_first() {
    // T1153 CORE-24 (kept): invalid aliases warn and are ignored.
    let aliases = vec![alias("arc", "draft", &["diff"], "user")];
    let warning = "Configuration source (\"user\") defines an invalid alias".to_string();
    let (result, notices) = apply_expansion_with(
        args(&["draft"]),
        || Some((aliases, vec![warning.clone()])),
        |_| panic!("must not spawn for dispatch"),
    );
    assert_eq!(result, Ok(args(&["diff"])));
    assert_eq!(notices.len(), 2);
    // T1153 CORE-23: cyan log-engine hints (` ALIAS  ...` plain).
    assert_eq!(notices[0], format!(" ALIAS  {warning}"));
    assert!(
        notices[1].starts_with(" ALIAS  arc draft -> "),
        "{}",
        notices[1]
    );
}

#[test]
fn rbd2_json_rejects_repeats() {
    let error = parse_args(&args(&["--json", "--json"])).unwrap_err();
    assert_eq!(error, "Argument \"--json\" was provided twice.");
}

#[test]
fn rbd25_json_rejects_inline_values() {
    let error = parse_args(&args(&["--json=x"])).unwrap_err();
    assert_eq!(error, "Argument \"--json\" does not take a parameter.");
}

#[test]
fn rbd6_lone_dash_is_positional() {
    let options = parse_args(&args(&["-"])).unwrap().unwrap();
    assert_eq!(options.positional, vec!["-".to_string()]);
}

#[test]
fn core16_toolset_names_match_ceb() {
    // T1153 CORE-66: `browse` is a concrete toolset workflow at ceb
    // (`ArcanistBrowseWorkflow extends ArcanistArcWorkflow`).
    let names = toolset_names();
    assert_eq!(names.len(), 21);
    for name in [
        "alias",
        "amend",
        "anoid",
        "bookmarks",
        "branches",
        "browse",
        "call-conduit",
        "download",
        "help",
        "inspect",
        "land",
        "liberate",
        "look",
        "paste",
        "prompts",
        "shell-complete",
        "upload",
        "upgrade",
        "version",
        "weld",
        "work",
    ] {
        assert!(names.contains(&name), "{name}");
    }
    for name in ["diff", "list", "which", "markers", "arc"] {
        assert!(!names.contains(&name), "{name}");
    }
}

#[test]
fn core21_alias_errors_use_toolset_arms() {
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
    assert_eq!(
        run(&args(&["--json", "x"])),
        std::process::ExitCode::FAILURE
    );
    // Builtin triggers fail before touching the filesystem.
    assert_eq!(
        run(&args(&["diff", "land"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn core28_user_aliases_prefer_config_section_on_collision() {
    // The toolset `ArcanistUserConfigurationSource` transform keeps the
    // section's `aliases` value when both layers define the key (`$result
    // += $config` never overwrites); the union is per-key, so the whole
    // top-level value drops, not just the colliding trigger.
    let tmp = TempDir::new("core28");
    let arcrc = tmp.arcrc();
    std::fs::write(
        &arcrc,
        r#"{
            "config": {"aliases": {"draft": ["diff", "--section"]}},
            "aliases": {"draft": ["diff", "--top"]}
        }"#,
    )
    .unwrap();
    let (aliases, warnings) = load_aliases_in(tmp.path(), Some(&arcrc)).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].trigger, "draft");
    assert_eq!(
        aliases[0].command,
        vec!["diff".to_string(), "--section".to_string()]
    );
}
