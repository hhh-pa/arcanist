use super::*;

fn severity_line_parser(output: &str) -> Vec<ScriptRegexMatch> {
    // Canned equivalent of `/^(?P<severity>warning|error):(?P<line>\d+)
    // (?P<message>.*)$/m` from the PHP docs.
    let mut out = Vec::new();
    for line in output.lines() {
        let Some((severity, rest)) = line.split_once(':') else {
            continue;
        };
        let Some((number, message)) = rest.split_once(' ') else {
            continue;
        };
        if !number.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        out.push(ScriptRegexMatch {
            severity: Some(severity.to_string()),
            line: Some(number.to_string()),
            message: Some(message.to_string()),
            ..ScriptRegexMatch::default()
        });
    }
    out
}

#[test]
fn default_linter_stays_silent() {
    let linter = ScriptAndRegexLinter::new();
    assert!(linter.lint_bytes("file.txt", b"error:3 boom\n").is_empty());
    assert!(linter.lint_bytes("file.txt", b"").is_empty());
}

#[test]
fn parses_canned_output_with_defaults() {
    let messages = parse_output_with(
        severity_line_parser,
        "error:3 Too many goats!\nwarning:22 Not enough boats.\n",
        "file.txt",
        b"a\nb\nc\n",
    )
    .unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].path, "file.txt");
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert_eq!(messages[0].code, "S&RX");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "Lint");
    assert_eq!(messages[0].description, "Too many goats!");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!((messages[1].line, messages[1].col), (22, 1));
}

#[test]
fn empty_output_and_no_matches_stay_silent() {
    assert!(
        parse_output_with(severity_line_parser, "", "f.txt", b"x\n")
            .unwrap()
            .is_empty()
    );
    assert!(
        parse_output_with(severity_line_parser, "No lint errors.\n", "f.txt", b"x\n")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn severity_mapping_follows_php_quirk() {
    let plain = ScriptRegexMatch::message_only("m");
    assert_eq!(match_severity(&plain), "error");

    // The docs suggest bare `error`/`warning` groups set severity, but the
    // PHP code returns error unless a `severity` group also matched.
    let bare_error = ScriptRegexMatch {
        error: Some("E".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_severity(&bare_error), "error");

    let warning = ScriptRegexMatch {
        severity: Some("Warning".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_severity(&warning), "warning");

    // Named groups supersede the `severity` value only when they come
    // earlier in map order (error, warning, autofix, advice, disabled):
    // an `error` group beats severity "warning", but an `advice` group
    // does not.
    let superseded = ScriptRegexMatch {
        severity: Some("warning".to_string()),
        error: Some("E".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_severity(&superseded), "error");
    let not_superseded = ScriptRegexMatch {
        severity: Some("warning".to_string()),
        advice: Some("A".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_severity(&not_superseded), "warning");

    let unknown = ScriptRegexMatch {
        severity: Some("bogus".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_severity(&unknown), "error");

    for (name, expected) in [
        ("error", "error"),
        ("warning", "warning"),
        ("autofix", "autofix"),
        ("advice", "advice"),
        ("disabled", "disabled"),
    ] {
        let mut both = ScriptRegexMatch {
            severity: Some("disabled".to_string()),
            ..ScriptRegexMatch::default()
        };
        match name {
            "error" => both.error = Some("x".to_string()),
            "warning" => both.warning = Some("x".to_string()),
            "autofix" => both.autofix = Some("x".to_string()),
            "advice" => both.advice = Some("x".to_string()),
            _ => both.disabled = Some("x".to_string()),
        }
        assert_eq!(match_severity(&both), expected, "for {name}");
    }
}

#[test]
fn line_char_and_offset_groups_map_like_php() {
    let data = b"ab\ncde\n";
    let line = ScriptRegexMatch {
        line: Some("2".to_string()),
        char: Some("3".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_line_and_char(&line, data), (2, 3));

    // Zero lines become 1; "0" chars count as absent (PHP falsiness).
    let zero = ScriptRegexMatch {
        line: Some("0".to_string()),
        char: Some("0".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_line_and_char(&zero, data), (1, 1));

    // Missing groups mean whole-file, reported at 1:1.
    assert_eq!(
        match_line_and_char(&ScriptRegexMatch::default(), data),
        (1, 1)
    );

    // Offset supersedes line/char; garbage offsets behave like (int) 0.
    let offset = ScriptRegexMatch {
        offset: Some("4".to_string()),
        line: Some("9".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_line_and_char(&offset, data), (2, 2));
    let garbage = ScriptRegexMatch {
        offset: Some("bogus".to_string()),
        ..ScriptRegexMatch::default()
    };
    assert_eq!(match_line_and_char(&garbage, data), (1, 1));
}

#[test]
fn control_groups_ignore_stop_and_throw() {
    fn parser(_output: &str) -> Vec<ScriptRegexMatch> {
        vec![
            ScriptRegexMatch {
                message: Some("skip me".to_string()),
                ignore: Some("yes".to_string()),
                ..ScriptRegexMatch::default()
            },
            ScriptRegexMatch::message_only("kept"),
            ScriptRegexMatch {
                message: Some("unseen".to_string()),
                stop: Some("yes".to_string()),
                ..ScriptRegexMatch::default()
            },
            ScriptRegexMatch::message_only("also unseen"),
        ]
    }
    let messages = parse_output_with(parser, "out\n", "f.txt", b"x\n").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "kept");

    fn throwing(_output: &str) -> Vec<ScriptRegexMatch> {
        vec![ScriptRegexMatch {
            throw: Some("boom".to_string()),
            ..ScriptRegexMatch::default()
        }]
    }
    let error = parse_output_with(throwing, "out\n", "f.txt", b"x\n").unwrap_err();
    assert!(error.contains("'throw'"));
    assert!(error.contains("boom"));
}

#[test]
fn file_and_code_groups_override_defaults() {
    fn parser(_output: &str) -> Vec<ScriptRegexMatch> {
        vec![ScriptRegexMatch {
            message: Some("elsewhere".to_string()),
            file: Some("other.txt".to_string()),
            code: Some("EXAMPLE1".to_string()),
            name: Some("Example".to_string()),
            ..ScriptRegexMatch::default()
        }]
    }
    let messages = parse_output_with(parser, "out\n", "f.txt", b"x\n").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "other.txt");
    assert_eq!(messages[0].code, "EXAMPLE1");
    assert_eq!(messages[0].name, "Example");
}

#[test]
fn configured_linter_skips_binaries_and_missing_scripts() {
    let linter = ScriptAndRegexLinter::with_script_and_parser(
        "definitely-missing-lint-script-xyz",
        severity_line_parser,
    );
    assert!(linter.lint_bytes("file.txt", b"error:1 x\n").is_empty());
    assert!(linter.lint_bytes("file.txt", b"\0").is_empty());
    assert!(!binary_exists("definitely-missing-lint-script-xyz"));
}

#[test]
fn script_command_quotes_the_path() {
    let command = script_command("lint.sh --flag", "my file.txt");
    assert!(command.starts_with("lint.sh --flag "));
    assert!(command.contains("my file.txt"));
}

#[test]
fn pcre_parse_matches_the_php_doc_example() {
    let pattern =
        compile_pcre("/^(?P<severity>warning|error):(?P<line>\\d+) (?P<message>.*)$/m").unwrap();
    let (messages, halted) = parse_pcre_output(
        &pattern,
        "error:13 Too many goats!\nwarning:22 Not enough boats.\n",
        "file.txt",
        b"x\n",
    )
    .unwrap();
    assert!(!halted);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[0].line, messages[0].col), (13, 1));
    assert_eq!(messages[0].description, "Too many goats!");
    assert_eq!(messages[0].code, "S&RX");
    assert_eq!(messages[0].name, "Lint");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!(messages[1].line, 22);
}

#[test]
fn unmatched_known_groups_read_empty_like_php() {
    let pattern = compile_pcre("/^(?P<message>[a-z]+)( (?P<file>\\S+))?$/m").unwrap();
    let (messages, _) = parse_pcre_output(&pattern, "oops\n", "f.txt", b"x\n").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "oops");
    // The `file` group exists but did not participate: PHP `idx()` sees
    // the empty string, not the default path.
    assert_eq!(messages[0].path, "");
    let (messages, _) = parse_pcre_output(&pattern, "oops there.txt\n", "f.txt", b"x\n").unwrap();
    assert_eq!(messages[0].path, "there.txt");
}

#[test]
fn pcre_parse_carries_original_replacement_and_offset() {
    let pattern = compile_pcre(
        "/^(?P<message>.*) @(?P<offset>\\d+) \\[(?P<original>[^\\]]*)->(?P<replacement>[^\\]]*)\\]$/m",
    )
    .unwrap();
    let (messages, _) =
        parse_pcre_output(&pattern, "fix this @4 [old->new]\n", "f.txt", b"ab\ncde\n").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "fix this");
    assert_eq!((messages[0].line, messages[0].col), (2, 2));
    assert_eq!(messages[0].original.as_deref(), Some("old"));
    assert_eq!(messages[0].replacement.as_deref(), Some("new"));
}

#[test]
fn halt_flag_reaches_take_halt_and_keeps_prior_messages() {
    fn parser(_output: &str) -> Vec<ScriptRegexMatch> {
        vec![
            ScriptRegexMatch::message_only("kept"),
            ScriptRegexMatch {
                halt: Some("yes".to_string()),
                ..ScriptRegexMatch::default()
            },
            ScriptRegexMatch::message_only("unseen"),
        ]
    }
    let linter = ScriptAndRegexLinter::with_script_and_parser("echo out", parser);
    assert!(!linter.take_halt());
    let messages = linter.lint_bytes("f.txt", b"x\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "kept");
    assert!(linter.take_halt());
    assert!(!linter.take_halt());
}

#[test]
fn script_and_regex_options_configure_and_validate() {
    let dir = std::env::temp_dir().join(format!("arc-rs-script-opt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = OptionContext { project_root: &dir };
    let mut linter = ScriptAndRegexLinter::new();
    linter
        .set_option(
            "script-and-regex.script",
            &serde_json::Value::String("echo hello".to_string()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option(
            "script-and-regex.regex",
            &serde_json::Value::String("/^(?P<message>.*)$/".to_string()),
            &ctx,
        )
        .unwrap();
    let messages = linter.lint_bytes("f.txt", b"x\n");
    assert_eq!(messages.len(), 1);
    assert!(messages[0].description.contains("hello"));
    assert!(!linter.applies_custom_severities());
    assert!(
        linter
            .set_option(
                "script-and-regex.regex",
                &serde_json::Value::String("/(unclosed/".to_string()),
                &ctx,
            )
            .is_err()
    );
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(1), &ctx)
            .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn throw_message_text_is_exact() {
    fn throwing(_output: &str) -> Vec<ScriptRegexMatch> {
        vec![ScriptRegexMatch {
            throw: Some("boom".to_string()),
            ..ScriptRegexMatch::default()
        }]
    }
    let error = parse_output_with(throwing, "out\n", "f.txt", b"x\n").unwrap_err();
    assert_eq!(
        error,
        "ArcanistScriptAndRegexLinter: configuration captured a 'throw' named capturing group, \
         'boom'. Script output:\nout\n"
    );
}
