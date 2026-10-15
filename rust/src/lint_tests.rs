//! Unit tests for [`super`]: severity mapping, the nine text rules,
//! engine stops and routing, argument parsing, renderers, and patching.

use super::*;

fn text(path: &str, data: &[u8]) -> Vec<LintMessage> {
    TextLinter::new().lint_bytes(path, data)
}

fn codes(messages: &[LintMessage]) -> Vec<&str> {
    messages.iter().map(|m| m.code.as_str()).collect()
}

// -( severity )----------------------------------------------------------

#[test]
fn severity_ranks_match_php() {
    assert!(severity_allows("error", "advice"));
    assert!(severity_allows("warning", "warning"));
    assert!(!severity_allows("advice", "warning"));
    assert!(severity_allows("autofix", "advice"));
    assert!(!severity_allows("autofix", "warning"));
    assert!(!severity_allows("disabled", "advice"));
    assert!(severity_allows("disabled", "disabled"));
    // Unknown message severities pass; unknown minima pass everything.
    assert!(severity_allows("mystery", "error"));
    assert!(severity_allows("advice", "mystery"));
}

#[test]
fn severity_display_names_match_php() {
    assert_eq!(severity_display_name("advice").unwrap(), "Advice");
    assert_eq!(severity_display_name("autofix").unwrap(), "Auto-Fix");
    assert_eq!(severity_display_name("warning").unwrap(), "Warning");
    assert_eq!(severity_display_name("error").unwrap(), "Error");
    assert_eq!(severity_display_name("disabled").unwrap(), "Disabled");
    assert_eq!(
        severity_display_name("bogus").unwrap_err(),
        "Unknown lint severity 'bogus'!"
    );
}

#[test]
fn patchable_matrix_matches_php() {
    let base = LintMessage {
        path: "f".to_string(),
        line: 1,
        col: 1,
        code: "X".to_string(),
        severity: SEVERITY_ERROR.to_string(),
        name: "n".to_string(),
        description: "d".to_string(),
        original: None,
        replacement: None,
    };
    assert!(!base.is_patchable());
    let with_original = LintMessage {
        original: Some("a".to_string()),
        replacement: None,
        ..base.clone()
    };
    assert!(!with_original.is_patchable());
    let identical = LintMessage {
        original: Some("a".to_string()),
        replacement: Some("a".to_string()),
        ..base.clone()
    };
    assert!(!identical.is_patchable());
    let differs = LintMessage {
        original: Some("a".to_string()),
        replacement: Some("b".to_string()),
        ..base.clone()
    };
    assert!(differs.is_patchable());
    // Missing original with any replacement is patchable (PHP `!==`).
    let insertion = LintMessage {
        original: None,
        replacement: Some(String::new()),
        ..base.clone()
    };
    assert!(insertion.is_patchable());

    assert!(base.has_file_context());
    let path_level = LintMessage {
        line: 0,
        col: 0,
        ..base.clone()
    };
    assert!(!path_level.has_file_context());
    let _ = base;
}

// -( text rules )--------------------------------------------------------

#[test]
fn empty_files_raise_txt10_and_stop() {
    let found = text("empty.txt", b"");
    assert_eq!(codes(&found), vec!["TXT10"]);
    assert_eq!(found[0].line, 0);
    assert_eq!(found[0].name, "Empty File");
    assert!(found[0].is_error());

    let whitespace = text("ws.txt", b"  \n\t\n");
    assert_eq!(codes(&whitespace), vec!["TXT10"]);

    // Dotfiles and `__init__.py` are exempt from TXT10 (other rules
    // still run: trailing whitespace reports instead).
    assert!(text(".gitkeep", b"").is_empty());
    assert!(text("pkg/__init__.py", b"").is_empty());
    assert_eq!(codes(&text(".hidden", b"  \n")), vec!["TXT6", "TXT8"]);
}

#[test]
fn dos_newlines_raise_txt1_with_whole_file_patch() {
    let found = text("dos.txt", b"a\r\nb\r\n");
    assert_eq!(codes(&found), vec!["TXT1"]);
    let message = &found[0];
    assert_eq!((message.line, message.col), (1, 1));
    assert_eq!(message.name, "DOS Newlines");
    assert_eq!(message.original.as_deref(), Some("a\r\nb\r\n"));
    assert_eq!(message.replacement.as_deref(), Some("a\nb\n"));
    assert!(message.is_patchable());
}

#[test]
fn only_the_first_tab_is_reported() {
    let found = text("tabs.txt", b"a\tb\nc\td\n");
    assert_eq!(codes(&found), vec!["TXT2"]);
    assert_eq!((found[0].line, found[0].col), (1, 2));
    assert_eq!(found[0].original.as_deref(), Some("\t"));
    assert_eq!(found[0].replacement, None);
    assert!(!found[0].is_patchable());
}

#[test]
fn long_lines_raise_txt3_with_grouped_counts() {
    let line = "x".repeat(81);
    let data = format!("{line}\n");
    let found = text("long.txt", data.as_bytes());
    assert_eq!(codes(&found), vec!["TXT3"]);
    assert!(found[0].is_warning());
    assert_eq!(found[0].name, "Line Too Long");
    assert_eq!(
        found[0].description,
        "This line is 81 characters long, but the convention is 80 characters."
    );
    assert_eq!(found[0].original.as_deref(), Some(line.as_str()));

    let big = "y".repeat(1234);
    let found = text("big.txt", format!("{big}\n").as_bytes());
    assert_eq!(
        found[0].description,
        "This line is 1,234 characters long, but the convention is 80 characters."
    );

    let custom = TextLinter::with_max_line_length(4);
    let found = custom.lint_bytes("c.txt", b"12345\n");
    assert_eq!(codes(&found), vec!["TXT3"]);
    assert!(custom.lint_bytes("c.txt", b"1234\n").is_empty());
}

#[test]
fn missing_final_newline_raises_patchable_txt4() {
    let found = text("noeol.txt", b"hello\nworld");
    assert_eq!(codes(&found), vec!["TXT4"]);
    assert_eq!((found[0].line, found[0].col), (2, 6));
    assert_eq!(found[0].original.as_deref(), Some(""));
    assert_eq!(found[0].replacement.as_deref(), Some("\n"));
    assert!(text("eol.txt", b"hello\nworld\n").is_empty());
}

#[test]
fn bad_charset_spans_first_to_last_per_line() {
    let found = text("bad.txt", b"caf\xC3\xA9\nok\n\x01x\x02\n");
    assert_eq!(codes(&found), vec!["TXT5", "TXT5"]);
    assert_eq!((found[0].line, found[0].col), (1, 4));
    assert_eq!(found[0].original.as_deref(), Some("\u{E9}"));
    assert_eq!((found[1].line, found[1].col), (3, 1));
    assert_eq!(found[1].original.as_deref(), Some("\u{1}x\u{2}"));
    assert_eq!(found[0].name, "Bad Charset");
}

#[test]
fn trailing_whitespace_is_autofix_and_cr_breaks_the_match() {
    let found = text("trail.txt", b"ok  \nplain\nend \n");
    assert_eq!(codes(&found), vec!["TXT6", "TXT6"]);
    assert!(found.iter().all(|m| m.is_autofix()));
    assert_eq!((found[0].line, found[0].col), (1, 3));
    assert_eq!(found[0].replacement.as_deref(), Some(""));
    assert_eq!(found[0].original.as_deref(), Some("  "));
    assert_eq!((found[1].line, found[1].col), (3, 4));

    // A carriage return breaks `[[:blank:]]+$`.
    assert_eq!(codes(&text("cr.txt", b"foo \r\n")), vec!["TXT1"]);
    // The final line without a newline still matches.
    let found = text("last.txt", b"ok\nend  ");
    assert_eq!(codes(&found), vec!["TXT6", "TXT4"]);
}

#[test]
fn bof_whitespace_matches_leading_gap_ending_in_newline() {
    let found = text("bof.txt", b"\n\nbody\n");
    assert_eq!(codes(&found), vec!["TXT8"]);
    assert_eq!((found[0].line, found[0].col), (1, 1));
    assert_eq!(found[0].original.as_deref(), Some("\n\n"));
    assert_eq!(found[0].replacement.as_deref(), Some(""));

    let found = text("bof2.txt", b"  \n  \nbody\n");
    let bof = found.iter().find(|m| m.code == "TXT8").unwrap();
    assert_eq!(bof.original.as_deref(), Some("  \n  \n"));
    assert!(text("nobo.txt", b"body\n").is_empty());
    assert!(text("nolf.txt", b"  body\n").is_empty());
}

#[test]
fn eof_whitespace_matches_trailing_gap_after_newline() {
    let found = text("eof.txt", b"body\n\n");
    assert_eq!(codes(&found), vec!["TXT9"]);
    assert_eq!((found[0].line, found[0].col), (2, 1));
    assert_eq!(found[0].original.as_deref(), Some("\n"));

    let found = text("eof2.txt", b"body\n  \n");
    let eof = found.iter().find(|m| m.code == "TXT9").unwrap();
    assert_eq!(eof.original.as_deref(), Some("  \n"));
    assert!(text("clean.txt", b"body\n").is_empty());
    // No newline before the tail: no match.
    assert_eq!(codes(&text("x.txt", b"body  ")), vec!["TXT6", "TXT4"]);
}

#[test]
fn text_stops_skip_later_phases() {
    // DOS newlines stop before the charset phase (`\r` is also a bad
    // charset byte, but TXT5 must not fire).
    assert_eq!(codes(&text("d.txt", b"a\r\n")), vec!["TXT1"]);
    // An empty file stops before everything else.
    assert_eq!(codes(&text("e.txt", b"\n\n")), vec!["TXT10"]);
}

#[test]
fn text_linter_priority_is_half() {
    assert_eq!(TextLinter::new().priority(), 0.5);
    assert_eq!(TextLinter::new().name(), "TXT");
    assert_eq!(TextLinter::new().max_line_length(), 80);
    assert_eq!(TextLinter::with_max_line_length(120).max_line_length(), 120);
}

// -( engine )------------------------------------------------------------

#[test]
fn engine_runs_every_ported_linter() {
    let engine = Engine::new();
    assert!(!engine.is_empty());
    assert!(!engine.is_comprehensive());
}

#[test]
fn all_linters_registers_thirty_four() {
    assert_eq!(all_linters().len(), 34);
}

#[test]
fn generated_and_nolint_markers_stop_the_engine() {
    let engine = Engine::new();
    assert!(
        engine
            .lint_bytes("g.php", b"<?php\n// @generated\n\t\n")
            .is_empty()
    );
    assert!(engine.lint_bytes("n.php", b"// @nolint\n\t\n").is_empty());
    // TXT2 fires without markers.
    assert!(
        engine
            .lint_bytes("t.php", b"plain\n\t\n")
            .iter()
            .any(|m| m.code == "TXT2")
    );
}

#[test]
fn stop_linters_must_be_selected_to_stop() {
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    // Without GEN/NOLINT in the engine, markers do not stop anything.
    assert!(
        engine
            .lint_bytes("g.php", b"// @generated\n\t\n")
            .iter()
            .any(|m| m.code == "TXT2")
    );
}

#[test]
fn empty_file_message_stops_later_linters() {
    struct Noisy;
    impl Linter for Noisy {
        fn name(&self) -> &'static str {
            "NOISY"
        }
        fn lint_bytes(&self, path: &str, _data: &[u8]) -> Vec<LintMessage> {
            vec![LintMessage {
                path: path.to_string(),
                line: 1,
                col: 1,
                code: "N1".to_string(),
                severity: SEVERITY_WARNING.to_string(),
                name: "n".to_string(),
                description: "d".to_string(),
                original: None,
                replacement: None,
            }]
        }
    }
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    engine.add_linter(Box::new(Noisy));
    assert_eq!(codes(&engine.lint_bytes("e.txt", b"")), vec!["TXT10"]);
}

#[test]
fn dos_and_charset_stops_honor_custom_severity_maps() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"dos": {"type": "text", "severity": {"1": "disabled"}}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    // TXT1 disabled: no stop, so the charset phase flags the `\r`.
    // The disabled TXT1 is still raised (the minimum filter drops it).
    let found = engine.lint_bytes("d.txt", b"a\r\n");
    assert_eq!(codes(&found), vec!["TXT1", "TXT5"]);
    assert_eq!(found[0].severity, SEVERITY_DISABLED);
    assert_eq!(
        engine.severity_for("TXT", "TXT1", SEVERITY_ERROR),
        SEVERITY_DISABLED
    );
    // Full-code keys do not match for `raiseLintAt*` linters (PHP looks
    // up the short code), so the default survives.
    let config = arclint::parse_arclint(
        r#"{"linters": {"t": {"type": "text", "severity": {"TXT6": "warning"}}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    assert_eq!(
        engine.severity_for("TXT", "TXT6", SEVERITY_AUTOFIX),
        SEVERITY_AUTOFIX
    );
}

#[test]
fn direct_severity_linters_ignore_custom_maps_like_php() {
    // `xml` sets severities directly; a `severity` map is accepted
    // (identity validator) but never consulted, so the finding stays
    // an error.
    let config = arclint::parse_arclint(
        r#"{"linters": {"x": {"type": "xml", "severity": {"XML1": "disabled"}}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let found = engine.lint_bytes("a.xml", b"<a>\n");
    assert_eq!(codes(&found), vec!["XML1"]);
    assert_eq!(found[0].severity, SEVERITY_ERROR);
    assert!(!crate::lint_golint::GolintLinter::new().applies_custom_severities());
    assert!(!crate::lint_xml::XmlLinter.applies_custom_severities());
    assert!(crate::lint_flake8::Flake8Linter::new().applies_custom_severities());
}

#[test]
fn text_stops_honor_severity_rules_like_php() {
    // A `severity.rules` pattern disabling short code `1` (DOS
    // newlines) suppresses the stop, like `isMessageEnabled()`.
    let config = arclint::parse_arclint(
        r#"{"linters": {"t": {"type": "text", "severity.rules": {"/^1$/": "disabled"}}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let found = engine.lint_bytes("d.txt", b"a\r\n");
    assert_eq!(codes(&found), vec!["TXT1", "TXT5"]);
    assert_eq!(found[0].severity, SEVERITY_DISABLED);
}

#[test]
fn config_text_length_reaches_the_linter() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"t": {"type": "text", "text.max-line-length": 4}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    assert_eq!(codes(&engine.lint_bytes("c.txt", b"12345\n")), vec!["TXT3"]);
    let bad = arclint::parse_arclint(
        r#"{"linters": {"t": {"type": "text", "text.max-line-length": "wide"}}}"#,
    )
    .unwrap();
    assert_eq!(
        Engine::from_config(&bad, Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file, for linter 't': 'text.max-line-length' must be an integer"
    );
}

#[test]
fn comprehensive_routing_matches_php() {
    assert_eq!(
        comprehensive_linters_for("src/a.php").unwrap(),
        vec!["NAME", "GEN", "NOLINT", "TXT", "XHP"]
    );
    assert_eq!(
        comprehensive_linters_for("a.py").unwrap(),
        vec!["NAME", "GEN", "NOLINT", "TXT", "PYFLAKES", "PEP8"]
    );
    assert_eq!(
        comprehensive_linters_for("a.js").unwrap(),
        vec!["NAME", "JSHint"]
    );
    assert_eq!(
        comprehensive_linters_for("a.rb").unwrap(),
        vec!["NAME", "RUBY"]
    );
    assert_eq!(comprehensive_linters_for("Makefile").unwrap(), vec!["NAME"]);
    assert_eq!(comprehensive_linters_for("py").unwrap(), vec!["NAME"]);
    assert!(comprehensive_linters_for("externals/a.php").is_none());

    let engine = Engine::comprehensive();
    assert!(engine.is_comprehensive());
    // A bad filename still reports through NAME on any extension.
    assert!(
        engine
            .lint_bytes("BAD NAME.js", b"ok\n")
            .iter()
            .any(|m| m.code == "NAME1")
    );
    assert!(engine.lint_bytes("externals/x.php", b"\t\n").is_empty());
}

// -( arguments )-------------------------------------------------------

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn parse(words: &[&str]) -> Options {
    match parse_args(&args(words)) {
        ParseOutcome::Args(options) => options,
        ParseOutcome::Help => panic!("unexpected help"),
        ParseOutcome::Error(message) => panic!("unexpected error: {message}"),
    }
}

fn parse_error(words: &[&str]) -> String {
    match parse_args(&args(words)) {
        ParseOutcome::Error(message) => message,
        other => panic!(
            "expected error, got {}",
            if matches!(other, ParseOutcome::Help) {
                "help"
            } else {
                "args"
            }
        ),
    }
}

#[test]
fn args_cover_every_flag() {
    let options = parse(&[
        "--lintall",
        "--rev",
        "HEAD^",
        "--output",
        "json",
        "--outfile",
        "out.txt",
        "--engine",
        "E",
        "--apply-patches",
        "--amend-all",
        "--amend-autofixes",
        "--severity",
        "warning",
        "a.php",
        "b.php",
    ]);
    assert!(options.lintall);
    assert_eq!(options.rev.as_deref(), Some("HEAD^"));
    assert_eq!(options.output.as_deref(), Some("json"));
    assert_eq!(options.outfile.as_deref(), Some("out.txt"));
    assert_eq!(options.engine.as_deref(), Some("E"));
    assert!(options.apply_patches);
    assert!(!options.never_apply_patches);
    assert!(options.amend_all);
    assert!(options.amend_autofixes);
    assert!(!options.everything);
    assert_eq!(options.severity.as_deref(), Some("warning"));
    assert_eq!(options.paths, vec!["a.php", "b.php"]);
    assert!(parse(&["--everything"]).everything);
}

#[test]
fn args_accept_inline_values_and_terminators() {
    let options = parse(&["--rev=HEAD~2", "--", "--lintall"]);
    assert_eq!(options.rev.as_deref(), Some("HEAD~2"));
    assert!(!options.lintall);
    assert_eq!(options.paths, vec!["--lintall"]);

    // `--bool=value` splices the value back as a positional.
    let options = parse(&["--lintall=yes"]);
    assert!(options.lintall);
    assert_eq!(options.paths, vec!["yes"]);

    // A value that looks like a flag is still consumed.
    let options = parse(&["--rev", "--everything"]);
    assert_eq!(options.rev.as_deref(), Some("--everything"));
    assert!(!options.everything);

    // `-h`/`--help` anywhere shows help.
    assert!(matches!(parse_args(&args(&["--help"])), ParseOutcome::Help));
    assert!(matches!(
        parse_args(&args(&["a", "-h"])),
        ParseOutcome::Help
    ));
}

#[test]
fn args_reject_unknown_and_missing() {
    assert_eq!(
        parse_error(&["--bogus"]),
        "Unknown argument 'bogus'. Try 'arc help'."
    );
    assert_eq!(
        parse_error(&["--output=json", "--bogus=1"]),
        "Unknown argument 'bogus'. Try 'arc help'."
    );
    assert_eq!(
        parse_error(&["-x"]),
        "Unknown argument 'x'. Try 'arc help'."
    );
    assert_eq!(
        parse_error(&["--rev"]),
        "Option '--rev' requires a parameter."
    );
    assert_eq!(
        parse_error(&["--severity"]),
        "Option '--severity' requires a parameter."
    );
}

#[test]
fn args_enforce_conflicts_in_provision_order() {
    // `true` reasons render as `: 1`.
    assert_eq!(
        parse_error(&["--apply-patches", "--never-apply-patches"]),
        "Arguments '--apply-patches' and '--never-apply-patches' are mutually exclusive: 1"
    );
    assert_eq!(
        parse_error(&["--never-apply-patches", "--apply-patches"]),
        "Arguments '--never-apply-patches' and '--apply-patches' are mutually exclusive: 1"
    );
    assert_eq!(
        parse_error(&["--everything", "--rev", "HEAD"]),
        "Arguments '--everything' and '--rev' are mutually exclusive: --everything lints all files"
    );
    assert!(parse(&["--apply-patches"]).apply_patches);
}

#[test]
fn rev_needs_git_or_hg() {
    let options = parse(&["--rev", "HEAD^"]);
    assert!(check_supports(&options, Vcs::Git).is_ok());
    assert!(check_supports(&options, Vcs::Hg).is_ok());
    assert_eq!(
        check_supports(&options, Vcs::Svn).unwrap_err(),
        "Option '--rev' is not supported under svn. Lint does not currently support --rev in SVN."
    );
    let plain = parse(&[]);
    assert!(check_supports(&plain, Vcs::Svn).is_ok());
}

#[test]
fn lint_all_follows_php_precedence() {
    assert!(lint_all(&parse(&["--lintall", "--rev", "x"])));
    assert!(!lint_all(&parse(&["--rev", "x"])));
    assert!(!lint_all(&parse(&["--rev", "x", "f.php"])));
    assert!(lint_all(&parse(&["f.php"])));
    assert!(lint_all(&parse(&["--everything"])));
    assert!(!lint_all(&parse(&[])));
}

// -( changed lines )----------------------------------------------------

#[test]
fn hunk_parser_collects_added_lines_only() {
    let diff = "\
diff --git a/f b/f
index 111..222 100644
--- a/f
+++ b/f
@@ -1,3 +1,4 @@
 keep
-old
+new1
+new2
 keep
@@ -10 +11,0 @@
-gone
";
    assert_eq!(parse_changed_lines(diff), BTreeSet::from([2, 3]));
}

#[test]
fn hunk_parser_handles_zero_length_and_nose() {
    // `+6` means `+6,1`; `+5,0` starts after line 5.
    let diff = "@@ -5,0 +6 @@\n+added\n@@ -9 +8 @@\n+one\n@@ -1 +5,0 @@\n-old\n";
    assert_eq!(parse_changed_lines(diff), BTreeSet::from([6, 8]));
    // `\\ No newline` advances like context.
    let diff = "@@ -1,2 +1,2 @@\n a\n-b\n+BB\n\\ No newline at end of file\n";
    assert_eq!(parse_changed_lines(diff), BTreeSet::from([2]));
    assert!(parse_changed_lines("not a diff\n").is_empty());
}

#[test]
fn changed_line_filter_keeps_errors_and_line_less() {
    let warning = |path: &str, line: usize| LintMessage {
        path: path.to_string(),
        line,
        col: 1,
        code: "W".to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: "w".to_string(),
        description: "d".to_string(),
        original: None,
        replacement: None,
    };
    let mut changed: HashMap<String, Option<BTreeSet<usize>>> = HashMap::new();
    changed.insert("a.php".to_string(), Some(BTreeSet::from([2])));
    changed.insert("bin.dat".to_string(), None);

    // Off-line warnings drop; on-line stay.
    assert_eq!(
        filter_changed_lines(vec![warning("a.php", 1)], &changed).len(),
        0
    );
    assert_eq!(
        filter_changed_lines(vec![warning("a.php", 2)], &changed).len(),
        1
    );
    // Errors bypass; line-less bypass; unknown paths drop; `None` keeps.
    let error = LintMessage {
        severity: SEVERITY_ERROR.to_string(),
        ..warning("a.php", 99)
    };
    assert_eq!(filter_changed_lines(vec![error], &changed).len(), 1);
    let path_level = LintMessage {
        line: 0,
        col: 0,
        ..warning("a.php", 99)
    };
    assert_eq!(filter_changed_lines(vec![path_level], &changed).len(), 1);
    assert_eq!(
        filter_changed_lines(vec![warning("elsewhere.php", 1)], &changed).len(),
        0
    );
    assert_eq!(
        filter_changed_lines(vec![warning("bin.dat", 50)], &changed).len(),
        1
    );
    // Multi-line originals match any overlapped line.
    let multi = LintMessage {
        original: Some("x\ny\n".to_string()),
        ..warning("a.php", 1)
    };
    assert_eq!(filter_changed_lines(vec![multi], &changed).len(), 1);
}

#[test]
fn result_code_prefers_errors_then_warnings() {
    let result = |severity: &str, applied: bool| {
        let mut result = LintResult::new("f".to_string(), PathBuf::from("f"), Vec::new());
        result.add_message(LintMessage {
            path: "f".to_string(),
            line: 1,
            col: 1,
            code: "C".to_string(),
            severity: severity.to_string(),
            name: "n".to_string(),
            description: "d".to_string(),
            original: None,
            replacement: None,
        });
        result.applied[0] = applied;
        result
    };
    assert_eq!(result_code(&[]), RESULT_OKAY);
    assert_eq!(result_code(&[result("advice", false)]), RESULT_OKAY);
    assert_eq!(result_code(&[result("autofix", false)]), RESULT_OKAY);
    assert_eq!(result_code(&[result("warning", false)]), RESULT_WARNINGS);
    assert_eq!(result_code(&[result("error", false)]), RESULT_ERRORS);
    assert_eq!(
        result_code(&[result("warning", false), result("error", false)]),
        RESULT_ERRORS
    );
    // Applied patches do not count.
    assert_eq!(result_code(&[result("error", true)]), RESULT_OKAY);
    assert_eq!(
        render_result_code_okay(RESULT_OKAY),
        "   OKAY  No lint messages.\n"
    );
    assert_eq!(render_result_code_okay(RESULT_WARNINGS), "");
}

// -( renderers )---------------------------------------------------------

fn console_result(path: &str, data: &[u8], messages: Vec<LintMessage>) -> LintResult {
    let mut result = LintResult::new(path.to_string(), PathBuf::from(path), data.to_vec());
    for message in messages {
        result.add_message(message);
    }
    result.sort_messages();
    result
}

fn warn123(
    line: usize,
    col: usize,
    original: Option<&str>,
    replacement: Option<&str>,
) -> LintMessage {
    LintMessage {
        path: "path/to/example.c".to_string(),
        line,
        col,
        code: "WARN123".to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: "Lint Warning".to_string(),
        description: "Consider this.".to_string(),
        original: original.map(str::to_string),
        replacement: replacement.map(str::to_string),
    }
}

/// Join expected console lines (`~` decoded already).
fn expected_console(lines: &[&str]) -> String {
    let mut out = String::from(">>> Lint for path/to/example.c:\n\n\n");
    out.push_str("   Warning  (WARN123) Lint Warning\n");
    out.push_str("    Consider this.\n");
    out.push('\n');
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn console_matches_simple_golden() {
    // `src/lint/renderer/__tests__/data/simple.*`, highlights stripped.
    let result = console_result(
        "path/to/example.c",
        b"a\nb\nc\n",
        vec![warn123(1, 1, Some("a"), Some("z"))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "    >>> -      1 a",
            "        +        z",
            "               2 b",
            "               3 c",
        ])
    );
}

#[test]
fn console_matches_caret_golden() {
    // `src/lint/renderer/__tests__/data/caret.*`.
    let data = b"Apples are round.\nBananas are round.\nCherries are round.\nDates are round.\n";
    let message = LintMessage {
        name: "Fruit Misinformation".to_string(),
        description: "Arguably untrue.".to_string(),
        ..warn123(2, 13, None, None)
    };
    let result = console_result("path/to/example.c", data, vec![message]);
    let mut expected = String::from(">>> Lint for path/to/example.c:\n\n\n");
    expected.push_str("   Warning  (WARN123) Fruit Misinformation\n");
    expected.push_str("    Arguably untrue.\n");
    expected.push('\n');
    for line in [
        "               1 Apples are round.",
        "    >>>        2 Bananas are round.",
        "                             ^",
        "               3 Cherries are round.",
        "               4 Dates are round.",
    ] {
        expected.push_str(line);
        expected.push('\n');
    }
    assert_eq!(render_console_result(&result).unwrap(), expected);
}

#[test]
fn console_matches_inline_and_overlap_goldens() {
    // `src/lint/renderer/__tests__/data/inline.*`: shared affixes trim
    // to the changed span.
    let result = console_result(
        "path/to/example.c",
        b"adjudicated\n",
        vec![warn123(1, 7, Some("cat"), Some("dog"))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "    >>> -      1 adjudicated",
            "        +        adjudidoged",
        ])
    );

    // `overlap.*`: trim keeps the single changed byte.
    let result = console_result(
        "path/to/example.c",
        b"tantawount\n",
        vec![warn123(1, 1, Some("tantawount"), Some("tantamount"))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&["    >>> -      1 tantawount", "        +        tantamount",])
    );
}

#[test]
fn console_matches_newline_removal_golden() {
    // `newline.*`: removing a blank line shrinks the tail range.
    let result = console_result(
        "path/to/example.c",
        b"aaa\nbbb\nccc\nddd\neee\n\nfff\nggg\nhhh\niii\njjj\n",
        vec![warn123(6, 1, Some("\n"), Some(""))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "               3 ccc",
            "               4 ddd",
            "               5 eee",
            "    >>> -      6 ",
            "               7 fff",
            "               8 ggg",
            "               9 hhh",
            "              10 iii",
        ])
    );
}

#[test]
fn console_matches_addline_and_remline_goldens() {
    // `addline.*`: pure insertions show only `+` lines.
    let result = console_result(
        "path/to/example.c",
        b"apple\nbanana\ndate\neclaire\nfig\n",
        vec![warn123(3, 1, Some(""), Some("cherry\n"))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "               1 apple",
            "               2 banana",
            "    >>> +        cherry",
            "               3 date",
            "               4 eclaire",
            "               5 fig",
        ])
    );

    // `remline.*`: multi-line originals trim to the removed newline.
    let result = console_result(
        "path/to/example.c",
        b"import apple;\nimport banana;\n\n\nimport cat;\nimport dog;\n",
        vec![warn123(
            1,
            1,
            Some("import apple;\nimport banana;\n\n\nimport cat;\nimport dog;"),
            Some("import apple;\nimport banana;\n\nimport cat;\nimport dog;"),
        )],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "               1 import apple;",
            "               2 import banana;",
            "               3 ",
            "    >>> -      4 ",
            "               5 import cat;",
            "               6 import dog;",
        ])
    );
}

#[test]
fn console_matches_eof_and_whitespace_goldens() {
    // `eofnewline.*`: appending the final newline. The trimmed ranges
    // keep a `-` line because kept endings differ.
    let result = console_result(
        "path/to/example.c",
        b"abcdef",
        vec![warn123(1, 7, Some(""), Some("\n"))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "    >>> -      1 abcdef",
            "        +        abcdef",
            "        +        ",
        ])
    );

    // `extrawhitespace.*`: removing trailing blank lines.
    let result = console_result(
        "path/to/example.c",
        b"Adrift upon the sea.\n\n",
        vec![warn123(2, 1, Some("\n"), Some(""))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&["               1 Adrift upon the sea.", "    >>> -      2 ",])
    );

    // `rmmulti.*`: head context plus a removed blank.
    let result = console_result(
        "path/to/example.c",
        b"A\n\n\nB\n",
        vec![warn123(2, 1, Some("\n"), Some(""))],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&[
            "               1 A",
            "               2 ",
            "    >>> -      3 ",
            "               4 B",
        ])
    );
}

#[test]
fn console_marks_original_spans_without_patches() {
    // `original.*`: non-patchable with original text — chevron, no
    // caret, highlights stripped.
    let result = console_result(
        "path/to/example.c",
        b"He should of known.\n",
        vec![warn123(1, 4, Some("should of"), None)],
    );
    assert_eq!(
        render_console_result(&result).unwrap(),
        expected_console(&["    >>>        1 He should of known."])
    );
}

#[test]
fn console_reports_past_end_messages() {
    let result = console_result("f.c", b"a\n", vec![warn123(9, 1, None, None)]);
    let text = render_console_result(&result).unwrap();
    assert!(text.contains("(This message was raised at line 9, but the file only has 1 line(s).)"));
    assert_eq!(console_result("f.c", b"", Vec::new()).messages.len(), 0);
    assert_eq!(
        render_console_result(&console_result("f.c", b"", Vec::new())).unwrap(),
        ""
    );
}

#[test]
fn summary_compiler_and_xml_renderers() {
    let mut result = console_result(
        "src/a.php",
        b"ok\n",
        vec![
            LintMessage {
                path: "src/a.php".to_string(),
                line: 3,
                ..warn123(3, 1, None, None)
            },
            LintMessage {
                path: "src/a.php".to_string(),
                line: 0,
                col: 0,
                code: "NAME1".to_string(),
                name: "Bad Filename".to_string(),
                ..warn123(0, 0, None, None)
            },
        ],
    );
    result.sort_messages();
    assert_eq!(
        render_summary_result(&result).unwrap(),
        "src/a.php::Warning: Bad Filename\nsrc/a.php:3:Warning: Lint Warning\n"
    );
    assert_eq!(
        render_compiler_result(&result).unwrap(),
        "src/a.php:0:Warning (NAME1) Consider this.\nsrc/a.php:3:Warning (WARN123) Consider this.\n"
    );
    assert_eq!(
        render_xml_result(&result),
        [
            "  <file name=\"src/a.php\">",
            "    <error line=\"\" column=\"\" severity=\"warning\" message=\"Consider this.\" source=\"NAME1\"/>",
            "    <error line=\"3\" column=\"1\" severity=\"warning\" message=\"Consider this.\" source=\"WARN123\"/>",
            "  </file>",
        ]
        .join("\n")
            + "\n"
    );
    assert_eq!(xml_severity("advice"), "info");
    assert_eq!(xml_severity("autofix"), "info");
    assert_eq!(xml_severity("disabled"), "ignore");
    assert_eq!(xml_severity("mystery"), "");
    assert_eq!(xml_escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    assert_eq!(
        render_xml_header(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<checkstyle version=\"4.3\">\n"
    );
    assert_eq!(render_xml_footer(), "</checkstyle>\n");
}

#[test]
fn summary_rejects_unknown_severities_like_php() {
    let result = console_result(
        "f",
        b"",
        vec![LintMessage {
            severity: "mystery".to_string(),
            ..warn123(1, 1, None, None)
        }],
    );
    assert_eq!(
        render_summary_result(&result).unwrap_err(),
        "Unknown lint severity 'mystery'!"
    );
    assert!(render_console_result(&result).is_err());
    assert!(render_compiler_result(&result).is_err());
}

#[test]
fn json_renderer_escapes_slashes_and_adds_context() {
    let result = console_result(
        "path/to/example.c",
        b"l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n",
        vec![warn123(5, 2, Some("orig"), Some("new"))],
    );
    let text = render_json_result(&result);
    assert!(text.starts_with("{\"path\\/to\\/example.c\":[{"));
    assert!(text.contains("\"context\":\"l2\\nl3\\nl4\\nl5\\nl6\\nl7\\nl8\""));
    assert!(text.contains("\"original\":\"orig\""));
    assert!(text.contains("\"locations\":[]"));
    assert!(!text.contains("\"path\":"));
    // Line-less messages render nulls.
    let result = console_result("f", b"", vec![warn123(0, 0, None, None)]);
    let text = render_json_result(&result);
    assert!(text.contains("\"line\":null"));
    assert!(text.contains("\"char\":null"));
    assert!(text.contains("\"context\":\"\""));
    // Non-ASCII text escapes like PHP `json_encode()` (default flags).
    let mut message = warn123(1, 1, None, None);
    message.description = "café 𝄞".to_string();
    let result = console_result("dossier/é.txt", b"x\n", vec![message]);
    let text = render_json_result(&result);
    assert!(text.starts_with("{\"dossier\\/\\u00e9.txt\":[{"));
    assert!(text.contains("\"description\":\"caf\\u00e9 \\ud834\\udd1e\""));
    assert_eq!(
        json_escape_nonascii("plain ascii \\u00e9 \\/"),
        "plain ascii \\u00e9 \\/"
    );
}

#[test]
fn word_wrap_indents_and_wraps() {
    assert_eq!(word_wrap("hi", 4), "    hi\n");
    assert_eq!(word_wrap("a\n\nb", 2), "  a\n  \n  b\n");
    let long = "word ".repeat(20);
    let wrapped = word_wrap(long.trim_end(), 4);
    assert!(wrapped.lines().count() > 1);
    assert!(wrapped.lines().all(|line| line.starts_with("    ")));
    assert!(wrapped.lines().all(|line| line.len() <= 80));
}

// -( patcher )-----------------------------------------------------------

#[test]
fn patcher_applies_position_ordered_patches() {
    let trailing = LintMessage {
        original: Some("  ".to_string()),
        replacement: Some(String::new()),
        ..warn123(1, 3, None, None)
    };
    let (patched, applied) = build_patched_data(b"ok  \n", &[trailing]).unwrap();
    assert_eq!(patched, b"ok\n");
    assert_eq!(applied, vec![true]);

    // Overlapping losers are skipped via the dirty cursor.
    let first = LintMessage {
        original: Some("abcd".to_string()),
        replacement: Some("ABCD".to_string()),
        ..warn123(1, 1, None, None)
    };
    let overlap = LintMessage {
        original: Some("bc".to_string()),
        replacement: Some("BC".to_string()),
        ..warn123(1, 2, None, None)
    };
    let (patched, applied) = build_patched_data(b"abcd\n", &[first, overlap]).unwrap();
    assert_eq!(patched, b"ABCD\n");
    assert_eq!(applied, vec![true, false]);

    // Non-patchable and identical messages never apply.
    let (patched, applied) = build_patched_data(b"a\n", &[warn123(1, 1, None, None)]).unwrap();
    assert_eq!(patched, b"a\n");
    assert_eq!(applied, vec![false]);
}

#[test]
fn patcher_appends_at_end_and_rejects_past_end() {
    let eof = LintMessage {
        original: Some(String::new()),
        replacement: Some("\n".to_string()),
        ..warn123(1, 4, None, None)
    };
    let (patched, applied) = build_patched_data(b"abc", &[eof]).unwrap();
    assert_eq!(patched, b"abc\n");
    assert_eq!(applied, vec![true]);

    let bad = warn123(9, 1, Some("x"), Some("y"));
    assert_eq!(
        build_patched_data(b"a\n", &[bad]).unwrap_err(),
        "Data has fewer than 0 lines."
    );
}

// -( working copies and engines )----------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = Path::new("target").join("lint-unit-tmp").join(format!(
            "{}-{}-{}",
            name,
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}")
}

#[test]
fn discovers_innermost_vcs_and_project_roots() {
    let tmp = TempDir::new("vcs");
    std::fs::create_dir_all(tmp.path().join("outer/.git")).unwrap();
    std::fs::create_dir_all(tmp.path().join("outer/inner/.hg")).unwrap();
    let deep = tmp.path().join("outer/inner/deep");
    std::fs::create_dir_all(&deep).unwrap();

    // Innermost wins (documented divergence from PHP's outermost).
    let work = discover_working_copy(&deep).unwrap();
    assert_eq!(work.vcs, Vcs::Hg);
    assert_eq!(work.vcs_root, tmp.path().join("outer/inner"));
    assert_eq!(work.project_root, tmp.path().join("outer/inner"));

    let work = discover_working_copy(&tmp.path().join("outer")).unwrap();
    assert_eq!(work.vcs, Vcs::Git);
    assert_eq!(work.vcs_root, tmp.path().join("outer"));

    // Subversion projects root at the nearest `.arcconfig`.
    std::fs::create_dir_all(tmp.path().join("svn/.svn")).unwrap();
    tmp.write("svn/proj/.arcconfig", "{}");
    let work = discover_working_copy(&tmp.path().join("svn/proj")).unwrap();
    assert_eq!(work.vcs, Vcs::Svn);
    assert_eq!(work.vcs_root, tmp.path().join("svn"));
    assert_eq!(work.project_root, tmp.path().join("svn/proj"));

    // Outside any VCS (the system temp dir sits outside the
    // worktree): an error naming the path.
    let bare = std::env::temp_dir().join(format!("lint-unit-bare-{}", std::process::id()));
    std::fs::create_dir_all(&bare).unwrap();
    let error = discover_working_copy(&bare).unwrap_err();
    assert!(error.contains("is not inside a working copy"));
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn arcconfig_reader_serves_lint_keys() {
    let tmp = TempDir::new("arcconfig");
    assert_eq!(arcconfig_value(tmp.path(), "lint.engine").unwrap(), None);
    tmp.write(
        ".arcconfig",
        r#"{"lint.engine": "E", "history.immutable": true}"#,
    );
    // Project-only cascade reads (hermetic: no ambient user/system files).
    let project_only = |key: &str| config_value_in(tmp.path(), None, None, None, key).unwrap();
    assert_eq!(project_only("lint.engine"), Some(serde_json::json!("E")));
    assert_eq!(
        project_only("history.immutable"),
        Some(serde_json::json!(true))
    );
    tmp.write(".arcconfig", "{broken");
    assert!(arcconfig_value(tmp.path(), "lint.engine").is_err());
}

#[test]
fn config_cascade_prefers_local_over_project_over_user_over_system() {
    let tmp = TempDir::new("cascade");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let local = tmp.path().join("local");
    let user = tmp.path().join("user");
    let system = tmp.path().join("system");
    std::fs::write(&system, r#"{"lint.engine": "System"}"#).unwrap();
    // System only.
    assert_eq!(
        config_value_in(&project, None, None, Some(&system), "lint.engine").unwrap(),
        Some(serde_json::json!("System"))
    );
    // User beats system; nested `config` wins over top level.
    std::fs::write(
        &user,
        r#"{"lint.engine": "UserTop", "config": {"lint.engine": "UserNested"}}"#,
    )
    .unwrap();
    assert_eq!(
        config_value_in(&project, None, Some(&user), Some(&system), "lint.engine").unwrap(),
        Some(serde_json::json!("UserNested"))
    );
    // Project beats user.
    std::fs::write(project.join(".arcconfig"), r#"{"lint.engine": "Project"}"#).unwrap();
    assert_eq!(
        config_value_in(&project, None, Some(&user), Some(&system), "lint.engine").unwrap(),
        Some(serde_json::json!("Project"))
    );
    // Local beats project.
    std::fs::write(&local, r#"{"lint.engine": "Local"}"#).unwrap();
    assert_eq!(
        config_value_in(
            &project,
            Some(&local),
            Some(&user),
            Some(&system),
            "lint.engine"
        )
        .unwrap(),
        Some(serde_json::json!("Local"))
    );
    // Null values fall through to the next layer.
    std::fs::write(&local, r#"{"lint.engine": null}"#).unwrap();
    assert_eq!(
        config_value_in(
            &project,
            Some(&local),
            Some(&user),
            Some(&system),
            "lint.engine"
        )
        .unwrap(),
        Some(serde_json::json!("Project"))
    );
    // Flat user files read top-level keys.
    std::fs::write(&user, r#"{"lint.engine": "UserFlat"}"#).unwrap();
    std::fs::remove_file(project.join(".arcconfig")).unwrap();
    assert_eq!(
        config_value_in(&project, None, Some(&user), None, "lint.engine").unwrap(),
        Some(serde_json::json!("UserFlat"))
    );
}

#[test]
fn config_cascade_validates_types_and_json_like_php() {
    let tmp = TempDir::new("cascade-err");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let user = tmp.path().join("user");
    let system = tmp.path().join("system");
    // Wrong types throw, even in losing layers.
    std::fs::write(project.join(".arcconfig"), r#"{"lint.engine": 5}"#).unwrap();
    std::fs::write(&system, r#"{"lint.engine": "System"}"#).unwrap();
    assert_eq!(
        config_value_in(&project, None, None, Some(&system), "lint.engine").unwrap_err(),
        "Type of setting 'lint.engine' must be string."
    );
    std::fs::write(
        project.join(".arcconfig"),
        r#"{"history.immutable": "yes"}"#,
    )
    .unwrap();
    assert_eq!(
        config_value_in(&project, None, None, None, "history.immutable").unwrap_err(),
        "Type of setting 'history.immutable' must be boolean."
    );
    // Wild keys pass anything through.
    std::fs::write(
        project.join(".arcconfig"),
        r#"{"lint.engine.single.linter": 5}"#,
    )
    .unwrap();
    assert_eq!(
        config_value_in(&project, None, None, None, "lint.engine.single.linter").unwrap(),
        Some(serde_json::json!(5))
    );
    // Invalid JSON errors per layer.
    std::fs::write(&user, "{broken").unwrap();
    assert_eq!(
        config_value_in(&project, None, Some(&user), None, "lint.engine").unwrap_err(),
        "Your '~/.arcrc' file is not a valid JSON file."
    );
    std::fs::write(&system, "{broken").unwrap();
    let error = config_value_in(&project, None, None, Some(&system), "lint.engine").unwrap_err();
    assert!(
        error.starts_with("Your '") && error.ends_with("file is not a valid JSON file."),
        "{error}"
    );
}

#[test]
fn local_config_path_follows_vcs_metadata_and_gitfiles() {
    let tmp = TempDir::new("local-path");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let work = WorkCopy {
        vcs: Vcs::Git,
        vcs_root: root.clone(),
        project_root: root.clone(),
    };
    assert_eq!(local_config_path(&work), Some(root.join(".git/arc/config")));
    // Linked worktrees keep a `gitdir:` pointer file instead.
    std::fs::remove_dir_all(root.join(".git")).unwrap();
    let target = std::env::current_dir()
        .unwrap()
        .join(tmp.path())
        .join("real.git");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(root.join(".git"), format!("gitdir: {}", target.display())).unwrap();
    assert_eq!(local_config_path(&work), Some(target.join("arc/config")));
    let hg = WorkCopy {
        vcs: Vcs::Hg,
        vcs_root: root.clone(),
        project_root: root.clone(),
    };
    assert_eq!(local_config_path(&hg), None);
    std::fs::create_dir_all(root.join(".hg")).unwrap();
    assert_eq!(local_config_path(&hg), Some(root.join(".hg/arc/config")));
}

#[test]
fn crc32_matches_standard_vectors() {
    assert_eq!(crc32_ieee(b""), 0);
    assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
    assert_eq!(
        crc32_ieee(b"1\nArcanistTextLinter:0"),
        crc32_ieee(b"1\nArcanistTextLinter:0")
    );
}

#[test]
fn every_canonical_linter_resolves_a_php_class() {
    for (canonical, _) in all_linter_pairs() {
        assert!(
            crate::linters::class_for_config(canonical).is_some(),
            "{canonical}"
        );
    }
    assert_eq!(
        crate::linters::class_for_config("closure"),
        Some("ArcanistClosureLinter")
    );
    assert_eq!(
        crate::linters::class_for_config("filename"),
        Some("ArcanistFilenameLinter")
    );
    assert_eq!(crate::linters::class_for_config("bogus"), None);
}

#[test]
fn cache_version_is_deterministic_and_section_sensitive() {
    let config = arclint::parse_arclint(r#"{"linters": {"t": {"type": "text"}}}"#).unwrap();
    let left = Engine::from_config(&config, Path::new(".")).unwrap();
    let right = Engine::from_config(&config, Path::new(".")).unwrap();
    assert_eq!(left.cache_version(), right.cache_version());
    let other =
        arclint::parse_arclint(r#"{"linters": {"t": {"type": "text"}, "j": {"type": "json"}}}"#)
            .unwrap();
    let other = Engine::from_config(&other, Path::new(".")).unwrap();
    assert_ne!(left.cache_version(), other.cache_version());
}

#[test]
fn cached_results_skip_execution_and_merge_like_php() {
    let config = arclint::parse_arclint(r#"{"linters": {"t": {"type": "text"}}}"#).unwrap();
    let mut engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let version = engine.cache_version();
    // Fresh run raises TXT5 for the bad byte.
    let fresh = engine.lint_bytes("a.txt", b"ok\x01\n");
    assert!(fresh.iter().any(|m| m.code == "TXT5"));
    // A file-granularity cache entry skips the linter and merges its
    // own messages instead.
    let cached_message = CachedMessage {
        message: LintMessage {
            path: "a.txt".to_string(),
            line: 3,
            col: 1,
            code: "TXT2".to_string(),
            severity: SEVERITY_WARNING.to_string(),
            name: "Tab".to_string(),
            description: "cached".to_string(),
            original: None,
            replacement: None,
        },
        granularity: Some(GRANULARITY_FILE),
    };
    let mut by_version = HashMap::new();
    by_version.insert(
        version,
        CachedPathResults {
            repository_version: None,
            stopped: None,
            messages: vec![cached_message],
        },
    );
    let mut cached = CachedResults::new();
    cached.insert("a.txt".to_string(), by_version);
    engine.set_cached_results(cached);
    let merged = engine.lint_bytes("a.txt", b"ok\x01\n");
    assert_eq!(codes(&merged), vec!["TXT2"]);
    assert_eq!(merged[0].description, "cached");
}

#[test]
fn cache_granularity_gates_reuse_like_php() {
    assert!(should_use_cache(Some(GRANULARITY_FILE), None, Some("r2")));
    assert!(should_use_cache(Some(GRANULARITY_FILE), Some("r1"), None));
    assert!(should_use_cache(
        Some(GRANULARITY_REPOSITORY),
        Some("abc"),
        Some("abc")
    ));
    assert!(!should_use_cache(
        Some(GRANULARITY_REPOSITORY),
        Some("abc"),
        Some("def")
    ));
    assert!(!should_use_cache(
        Some(GRANULARITY_DIRECTORY),
        None,
        Some("abc")
    ));
    assert!(should_use_cache(Some(GRANULARITY_DIRECTORY), None, None));
    assert!(!should_use_cache(Some(GRANULARITY_GLOBAL), None, None));
    assert!(!should_use_cache(None, None, None));
    assert_ne!(
        crate::lint_phutil_library::PhutilLibraryLinter.cache_granularity(),
        GRANULARITY_FILE
    );
}

#[test]
fn cached_stopped_markers_stop_later_sections() {
    let config =
        arclint::parse_arclint(r#"{"linters": {"t": {"type": "text"}, "j": {"type": "json"}}}"#)
            .unwrap();
    let mut engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let version = engine.cache_version();
    // Section 0 (text) stopped the path in the cached run: with a
    // usable cache entry, the JSON section never runs fresh.
    let mut by_version = HashMap::new();
    by_version.insert(
        version,
        CachedPathResults {
            repository_version: None,
            stopped: Some(0),
            messages: Vec::new(),
        },
    );
    let mut cached = CachedResults::new();
    cached.insert("a.json".to_string(), by_version);
    engine.set_cached_results(cached);
    assert!(engine.lint_bytes("a.json", b"{broken\n").is_empty());
}

#[test]
fn cached_message_from_dict_mirrors_new_from_dictionary() {
    let mut dict = HashMap::new();
    for (key, value) in [
        ("path", serde_json::json!("a.txt")),
        ("line", serde_json::json!(7)),
        ("char", serde_json::json!(3)),
        ("code", serde_json::json!("TXT2")),
        ("severity", serde_json::json!("warning")),
        ("name", serde_json::json!("Tab")),
        ("description", serde_json::json!("d")),
        ("original", serde_json::json!("o")),
        ("replacement", serde_json::json!("r")),
        ("granularity", serde_json::json!(1)),
    ] {
        dict.insert(key.to_string(), value);
    }
    let cached = CachedMessage::from_dict(&dict).unwrap();
    assert_eq!(cached.message.path, "a.txt");
    assert_eq!((cached.message.line, cached.message.col), (7, 3));
    assert_eq!(cached.message.original.as_deref(), Some("o"));
    assert_eq!(cached.granularity, Some(1));
    dict.remove("code");
    assert!(CachedMessage::from_dict(&dict).is_none());
}

fn engine_error(work: &WorkCopy, cli_engine: Option<&str>) -> String {
    match engine_for_workcopy(work, cli_engine) {
        Ok(_) => panic!("expected engine error"),
        Err(message) => message,
    }
}

#[test]
fn engine_resolution_matches_php_precedence() {
    let tmp = TempDir::new("engine");
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let work = discover_working_copy(tmp.path()).unwrap();

    // No `.arclint`, no config: the no-engine error.
    assert_eq!(
        engine_error(&work, None),
        "No lint engine is configured for this project. Create an '.arclint' file, or configure an advanced engine with 'lint.engine' in '.arcconfig'."
    );
    // Unknown classes are rejected.
    assert_eq!(
        engine_error(&work, Some("BogusEngine")),
        "Configured lint engine \"BogusEngine\" is not a subclass of \"ArcanistLintEngine\", but must be."
    );
    // The comprehensive engine needs no config.
    assert!(engine_for_workcopy(&work, Some(ENGINE_COMPREHENSIVE)).is_ok());
    // `--engine` wins over config, empty means unset.
    tmp.write(".arcconfig", r#"{"lint.engine": "BogusEngine"}"#);
    assert!(engine_for_workcopy(&work, Some("")).is_err());
    assert!(engine_for_workcopy(&work, Some(ENGINE_COMPREHENSIVE)).is_ok());

    // The single engine needs a configured linter.
    tmp.write(
        ".arcconfig",
        r#"{"lint.engine": "ArcanistSingleLintEngine"}"#,
    );
    assert!(engine_error(&work, None).contains("lint.engine.single.linter"));
    tmp.write(
        ".arcconfig",
        r#"{"lint.engine": "ArcanistSingleLintEngine", "lint.engine.single.linter": "ArcanistTextLinter"}"#,
    );
    let engine = engine_for_workcopy(&work, None).unwrap();
    assert_eq!(
        codes(&engine.lint_bytes("t.txt", b"ok\nx\ty\n")),
        vec!["TXT2"]
    );
    tmp.write(
        ".arcconfig",
        r#"{"lint.engine": "ArcanistSingleLintEngine", "lint.engine.single.linter": "Bogus"}"#,
    );
    assert!(engine_error(&work, None).contains("does not exist!"));
}

#[test]
fn explicit_paths_must_exist_and_stay_unwalked() {
    let tmp = TempDir::new("explicit");
    let file = tmp.write("sub/file.txt", "ok\n");
    std::fs::create_dir_all(tmp.path().join("sub/empty")).unwrap();
    let files = collect_explicit(
        &["sub/file.txt".to_string(), "sub/empty".to_string()],
        tmp.path(),
        tmp.path(),
    )
    .unwrap();
    // Sorted by display path; directories pass through unlisted.
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].1, "sub/empty");
    assert_eq!(files[1].1, "sub/file.txt");
    assert_eq!(files[1].0, file);
    assert_eq!(
        collect_explicit(&["nope.txt".to_string()], tmp.path(), tmp.path()).unwrap_err(),
        "Path 'nope.txt' does not exist!"
    );
}

#[test]
fn lint_file_filters_by_kind_like_php() {
    let engine = Engine::new();
    let tmp = TempDir::new("kinds");
    // Missing files lint nowhere.
    let missing = tmp.path().join("gone.txt");
    assert!(engine.lint_file(&missing, "gone.txt").unwrap().is_empty());
    // Directories reach only the filename linter.
    let dir = tmp.path().join("fine-dir");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(engine.lint_file(&dir, "fine-dir").unwrap().is_empty());
    let bad = tmp.path().join("bad dir");
    std::fs::create_dir_all(&bad).unwrap();
    assert_eq!(
        codes(&engine.lint_file(&bad, "bad dir").unwrap()),
        vec!["NAME1"]
    );
    // Binaries reach only filename and chmod.
    let bin = tmp.write("bin.dat", "ok\n");
    std::fs::write(&bin, b"has\0nul\t\n").unwrap();
    assert!(engine.lint_file(&bin, "bin.dat").unwrap().is_empty());
}

// -( prompts )-----------------------------------------------------------

#[test]
fn confirm_honors_answers_and_defaults() {
    let ask = |text: &str, default_yes: bool| {
        let mut cursor = std::io::BufReader::new(text.as_bytes());
        confirm("Do it?", default_yes, &mut cursor)
    };
    assert!(ask("y\n", false));
    assert!(ask("YES\n", false));
    assert!(!ask("n\n", true));
    assert!(!ask("No\n", true));
    assert!(ask("\n", true));
    assert!(!ask("\n", false));
    assert!(ask("", true));
    assert!(!ask("", false));
    // Garbage re-prompts until an answer arrives.
    assert!(ask("huh\nY\n", false));
    assert!(!ask("huh\n", false));
}
