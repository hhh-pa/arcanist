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
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
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
                granularity: None,
                locations: Vec::new(),
                bypass_changed_line_filtering: None,
                original_bytes: None,
                replacement_bytes: None,
            }]
        }
    }
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    engine.add_linter(Box::new(Noisy));
    assert_eq!(codes(&engine.lint_bytes("e.txt", b"")), vec!["TXT10"]);
    // Direct call pins the helper shape; the engine above must not
    // reach it (the empty-file message stops later linters).
    assert_eq!(Noisy.lint_bytes("e.txt", b"")[0].code, "N1");
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
        // T1153 LINT-40: chain messages render one per line.
        "Error in parsing '.arclint' file, for linter 't'.\n'text.max-line-length' must be an integer"
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
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
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
            granularity: None,
            locations: Vec::new(),
            bypass_changed_line_filtering: None,
            original_bytes: None,
            replacement_bytes: None,
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
    // T1153 LINT-5: one leading space.
    assert_eq!(
        render_result_code_okay(RESULT_OKAY, false),
        " OKAY  No lint messages.\n"
    );
    assert_eq!(render_result_code_okay(RESULT_WARNINGS, false), "");
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
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
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
    // T1153 LINT-24: no synthetic trailing newline; content wraps at
    // 78 - indent (78 total here), like `phutil_console_wrap()`.
    assert_eq!(word_wrap("hi", 4), "    hi");
    assert_eq!(word_wrap("a\n\nb", 2), "  a\n  \n  b");
    let long = "word ".repeat(20);
    let wrapped = word_wrap(long.trim_end(), 4);
    assert!(wrapped.lines().count() > 1);
    assert!(wrapped.lines().all(|line| line.starts_with("    ")));
    assert!(wrapped.lines().all(|line| line.len() <= 78));
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
            granularity: None,
            locations: Vec::new(),
            bypass_changed_line_filtering: None,
            original_bytes: None,
            replacement_bytes: None,
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
    // T1153 LINT-9: CLI order is kept (never sorted); directories pass
    // through unlisted. `/`-typed CLI paths keep `/` displays on every
    // platform (the join is byte-preserving without `.`/`..`).
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].1, "sub/file.txt");
    assert_eq!(files[1].1, "sub/empty");
    assert_eq!(files[0].0, file);
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
    // LINT-8: only `y`/`n`/empty answer; `yes`/`no` re-prompt like PHP.
    assert!(!ask("YES\n", false));
    assert!(!ask("n\n", true));
    assert!(ask("No\n", true));
    assert!(ask("yes\ny\n", false));
    assert!(!ask("no\nn\n", true));
    assert!(ask("\n", true));
    assert!(!ask("\n", false));
    assert!(ask("", true));
    assert!(!ask("", false));
    // Garbage re-prompts until an answer arrives.
    assert!(ask("huh\nY\n", false));
    assert!(!ask("huh\n", false));
}

fn error_message(code: &str, name: &str, description: &str) -> LintMessage {
    LintMessage {
        path: "f.c".to_string(),
        line: 0,
        col: 0,
        code: code.to_string(),
        severity: SEVERITY_ERROR.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        original: None,
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    }
}

#[test]
fn console_result_with_ansi_badges_severity() {
    let result = console_result("f.c", b"x\n", vec![error_message("E1", "Bad", "Fix it.")]);
    assert_eq!(
        render_console_result_with_ansi(&result, true).unwrap(),
        "\x1b[1m>>>\x1b[m Lint for \x1b[4mf.c\x1b[m:\n\n\n  \x1b[1m\x1b[41m Error \
         \x1b[49m\x1b[m (E1) \x1b[4mBad\x1b[m\n    Fix it.\n"
    );
    assert_eq!(
        render_console_result_with_ansi(&result, false).unwrap(),
        ">>> Lint for f.c:\n\n\n   Error  (E1) Bad\n    Fix it.\n"
    );
    let warned = console_result("f.c", b"x\n", vec![warn123(0, 0, None, None)]);
    let tty = render_console_result_with_ansi(&warned, true).unwrap();
    assert!(tty.contains("\x1b[1m\x1b[43m Warning \x1b[49m\x1b[m"));
    assert_eq!(
        render_console_result_with_ansi(&warned, false).unwrap(),
        render_console_result(&warned).unwrap()
    );
}

#[test]
fn summary_and_compiler_with_ansi_match_plain() {
    let result = console_result(
        "src/a.php",
        b"ok\n",
        vec![error_message("E1", "Bad", "Fix it.")],
    );
    assert_eq!(
        render_summary_result_with_ansi(&result, true).unwrap(),
        "src/a.php::Error: Bad\n"
    );
    assert_eq!(
        render_summary_result_with_ansi(&result, false).unwrap(),
        "src/a.php::Error: Bad\n"
    );
    assert_eq!(
        render_compiler_result_with_ansi(&result, true).unwrap(),
        "src/a.php:0:Error (E1) Fix it.\n"
    );
    assert_eq!(
        render_compiler_result_with_ansi(&result, false).unwrap(),
        "src/a.php:0:Error (E1) Fix it.\n"
    );
}

#[test]
fn result_code_okay_with_ansi_uses_green_writer() {
    assert_eq!(
        render_result_code_okay(RESULT_OKAY, true),
        "\x1b[42m\x1b[1m OKAY \x1b[m\x1b[49m No lint messages.\n"
    );
    // T1153 LINT-5: one leading space, stripped from the log-line markup.
    assert_eq!(
        render_result_code_okay(RESULT_OKAY, false),
        " OKAY  No lint messages.\n"
    );
    assert_eq!(render_result_code_okay(RESULT_WARNINGS, true), "");
}

#[test]
fn lint_notice_line_uses_yellow_writer() {
    assert_eq!(
        lint_notice_line(true),
        "\x1b[43m\x1b[1m LINT NOTICE \x1b[m\x1b[49m Automatically amending HEAD with lint \
         patches.\n"
    );
    // T1153 LINT-6: the markup strips on non-TTYs.
    assert_eq!(
        lint_notice_line(false),
        " LINT NOTICE  Automatically amending HEAD with lint patches.\n"
    );
}

#[test]
fn patch_prompt_with_ansi_underlines_path() {
    assert_eq!(
        patch_prompt_with_ansi("src/a.php", true),
        "Apply this patch to \x1b[4msrc/a.php\x1b[m?"
    );
    assert_eq!(
        patch_prompt_with_ansi("src/a.php", false),
        "Apply this patch to src/a.php?"
    );
}

#[test]
fn commit_range_prompt_with_ansi_badges_header() {
    let ask = |ansi: bool| {
        let mut out = String::new();
        let mut cursor = std::io::BufReader::new("origin/master\n".as_bytes());
        let mut sink = |text: &str| out.push_str(text);
        let relative = prompt_default_relative_commit(&mut cursor, &mut sink, ansi);
        (relative, out)
    };
    let (relative, tty) = ask(true);
    assert_eq!(relative, "origin/master");
    assert!(tty.starts_with("\x1b[42m\x1b[1m Select a Default Commit Range \x1b[m\x1b[49m\n\n"));
    for line in tty.lines() {
        assert!(line.len() <= 78, "tty line overflows: {line:?}");
    }
    let (relative, plain) = ask(false);
    assert_eq!(relative, "origin/master");
    assert!(plain.starts_with("** Select a Default Commit Range **\n\n"));
    assert!(plain.contains("Facebook's historic git-svn workflow"));
}

#[test]
fn context_with_ansi_highlights_single_line_change() {
    let message = LintMessage {
        path: "f.c".to_string(),
        line: 1,
        col: 7,
        code: "E1".to_string(),
        severity: SEVERITY_ERROR.to_string(),
        name: "Bad".to_string(),
        description: "Fix it.".to_string(),
        original: Some("world".to_string()),
        replacement: Some("there".to_string()),
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    };
    let tty = render_context_with_ansi(&message, b"hello world\nbye\n", true);
    assert!(tty.contains(">>> -      1 hello \x1b[7mworld\x1b[m\n"));
    assert!(tty.contains("        +        hello \x1b[7mthere\x1b[m\n"));
    let plain = render_context_with_ansi(&message, b"hello world\nbye\n", false);
    assert!(!plain.contains('\x1b'));
    assert_eq!(plain, render_context(&message, b"hello world\nbye\n"));
}

#[test]
fn context_with_ansi_skips_multiline_highlight() {
    let message = LintMessage {
        path: "f.c".to_string(),
        line: 1,
        col: 1,
        code: "E1".to_string(),
        severity: SEVERITY_ERROR.to_string(),
        name: "Bad".to_string(),
        description: "Fix it.".to_string(),
        original: Some("one\ntwo".to_string()),
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    };
    let tty = render_context_with_ansi(&message, b"one\ntwo\n", true);
    assert!(!tty.contains('\x1b'));
}

// -( R5 TTY gating: LINT-8 )--------------------------------------------------

#[test]
fn r5_lint8_confirm_with_tty_gates_pipes() {
    // `phutil_console_prompt()` echoes, then `require_tty()` throws
    // (`src/console/format.php:42-52`); scripted TTY sessions answer.
    let mut piped = std::io::BufReader::new("y\n".as_bytes());
    assert_eq!(
        confirm_with_tty("Apply this patch to f?", true, &mut piped, false).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    let mut tty = std::io::BufReader::new("y\n".as_bytes());
    assert!(confirm_with_tty("Apply this patch to f?", true, &mut tty, true).unwrap());
    // Amend-prompt default stays yes; Enter amends.
    let mut enter = std::io::BufReader::new("\n".as_bytes());
    assert!(confirm_with_tty("Amend HEAD with lint patches?", true, &mut enter, true).unwrap());
}

#[test]
fn r4_lint1_no_effect_kinds() {
    use arc_rs::conduit::EnvelopeKind;
    // Both `ArcanistNoEffectException` sites route to stdout + exit 0.
    assert_eq!(
        classify_gather_error("No paths are lintable.".to_string()).kind,
        EnvelopeKind::NoEffect
    );
    let error = arc_rs::conduit::ClassifiedError::no_effect("No linters to run.".to_string());
    assert_eq!(error.kind, EnvelopeKind::NoEffect);
    assert_eq!(error.kind.exit_code(), std::process::ExitCode::SUCCESS);
}

#[test]
fn r4_lint2_lint3_gather_emit_kinds() {
    use arc_rs::conduit::EnvelopeKind;
    // Amend declines are usage; renderer and git failures are generic.
    assert_eq!(
        classify_emit_error(
            "Sort out the lint changes that were applied to the working copy and relint."
                .to_string()
        )
        .kind,
        EnvelopeKind::Usage
    );
    assert_eq!(
        classify_emit_error("Lint renderer \"x\" is unknown.".to_string()).kind,
        EnvelopeKind::Exception
    );
    assert_eq!(
        classify_gather_error("failed to run git: boom".to_string()).kind,
        EnvelopeKind::Exception
    );
    // Usage renders with no dump: the classified parse path keeps the
    // bare message (the `Usage Exception:` label applies at `run_in`).
    assert!(matches!(
        parse_args(&["--bogus".to_string()]),
        ParseOutcome::Error(_)
    ));
}

// -( T1153 Wave 2: lint retrocompat )--------------------------------------

fn lint4_failing_parser(_output: &str) -> Vec<crate::lint_script_and_regex::ScriptRegexMatch> {
    unreachable!("the script fails before parsing")
}

#[test]
fn lint4_linter_failures_aggregate_without_rendering() {
    // A failing linter never aborts the run: every path still lints and
    // any failure becomes `Some linters failed:` (nothing renders).
    let tmp = TempDir::new("lint4");
    tmp.write("ok.txt", "ok\n");
    let work = WorkCopy {
        vcs: Vcs::Git,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(
        crate::lint_script_and_regex::ScriptAndRegexLinter::with_script_and_parser(
            "exit 1",
            lint4_failing_parser,
        ),
    ));
    assert_eq!(
        workflow_lint_selection(
            &engine,
            &work,
            &["ok.txt".to_string(), "ok.txt".to_string()]
        )
        .unwrap_err(),
        "Some linters failed:"
    );
}

#[test]
#[cfg(unix)]
fn lint20_unreadable_files_fold_into_the_aggregate() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("lint20");
    let locked = tmp.write("locked.txt", "x\n");
    tmp.write("ok.txt", "ok\n");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&locked).is_ok() {
        // Privileged runner (e.g. root in CI containers): permission
        // bits do not block reads, so the failure under test cannot
        // trigger.
        return;
    }
    let work = WorkCopy {
        vcs: Vcs::Git,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    // The good path still lints, but the failure aggregates like PHP.
    assert_eq!(
        workflow_lint_selection(
            &engine,
            &work,
            &["locked.txt".to_string(), "ok.txt".to_string()]
        )
        .unwrap_err(),
        "Some linters failed:"
    );
}

#[test]
fn lint9_result_sections_keep_first_seen_order() {
    let mut first = warn123(1, 1, None, None);
    first.path = "b.txt".to_string();
    let mut second = warn123(1, 1, None, None);
    second.path = "a.txt".to_string();
    let results = group_results(vec![first, second], &|path| PathBuf::from(path));
    let paths: Vec<&str> = results.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, vec!["b.txt", "a.txt"]);
}

#[test]
fn lint9_explicit_paths_keep_cli_order_and_dupes() {
    let tmp = TempDir::new("lint9");
    tmp.write("b.txt", "b\n");
    tmp.write("a.txt", "a\n");
    let files = collect_explicit(
        &[
            "b.txt".to_string(),
            "a.txt".to_string(),
            "b.txt".to_string(),
        ],
        tmp.path(),
        tmp.path(),
    )
    .unwrap();
    let displays: Vec<&str> = files.iter().map(|(_, display)| display.as_str()).collect();
    assert_eq!(displays, vec!["b.txt", "a.txt", "b.txt"]);
}

#[test]
fn lint10_dotdot_segments_normalize_lexically() {
    let tmp = TempDir::new("lint10");
    tmp.write("file.txt", "x\n");
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    let files = collect_explicit(&["sub/../file.txt".to_string()], tmp.path(), tmp.path()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].1, "file.txt");
    assert_eq!(files[0].0, tmp.path().join("file.txt"));
}

#[test]
fn lint11_json_dict_carries_granularity_locations_and_bypass() {
    let mut message = warn123(2, 3, None, None);
    message.granularity = Some(4);
    message.bypass_changed_line_filtering = Some(true);
    message.locations = vec![OtherLocation {
        path: Some("other.php".to_string()),
        line: 9,
        char: 0,
        original: None,
    }];
    let dict = message_json_dict(&message, b"l1\nl2\nl3\n");
    assert_eq!(dict["granularity"], serde_json::json!(4));
    assert_eq!(dict["bypassChangedLineFiltering"], serde_json::json!(true));
    assert_eq!(dict["locations"][0]["path"], serde_json::json!("other.php"));
    assert_eq!(dict["locations"][0]["line"], serde_json::json!(9));
    assert!(
        !dict["locations"][0]
            .as_object()
            .unwrap()
            .contains_key("original")
    );
    // Key order matches `toDictionary()` plus trailing `context`.
    let keys: Vec<&str> = dict
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "line",
            "char",
            "code",
            "severity",
            "name",
            "description",
            "original",
            "replacement",
            "granularity",
            "locations",
            "bypassChangedLineFiltering",
            "context",
        ]
    );
}

#[test]
fn lint11_engine_stamps_cache_granularity() {
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    let found = engine.lint_bytes("f.txt", b"ok  \n");
    assert!(!found.is_empty());
    assert!(found.iter().all(|m| m.granularity == Some(1)));
    assert_eq!(
        crate::lint_phutil_library::PhutilLibraryLinter::new().cache_granularity(),
        4
    );
}

#[test]
fn lint11_del_passes_through_raw_in_json() {
    assert_eq!(json_escape_nonascii("a\x7fb"), "a\x7fb");
    assert_eq!(json_escape_nonascii("caf\u{e9}"), "caf\\u00e9");
}

#[test]
fn lint12_xml_escapes_tabs_newlines_and_returns() {
    assert_eq!(
        xml_escape("a\tb\nc\rd\"e<f>g&h"),
        "a&#9;b&#10;c&#13;d&quot;e&lt;f&gt;g&amp;h"
    );
    // Unknown severities render empty, like PHP's null attribute.
    assert_eq!(xml_severity("bogus"), "");
}

#[test]
fn lint13_console_lists_other_locations() {
    let mut message = warn123(1, 1, None, None);
    message.path = "a.php".to_string();
    message.locations = vec![
        OtherLocation {
            path: None,
            line: 9,
            char: 0,
            original: None,
        },
        OtherLocation {
            path: Some("b.php".to_string()),
            line: 0,
            char: 0,
            original: None,
        },
    ];
    let mut result = LintResult::new("a.php".to_string(), PathBuf::from("a.php"), b"x\n".to_vec());
    result.add_message(message);
    let rendered = render_console_result_with_ansi(&result, false).unwrap();
    assert!(
        rendered.contains("Other locations: a.php:9, b.php"),
        "{rendered}"
    );
}

#[test]
fn lint14_relevance_honors_bypass_and_other_locations() {
    let changed: HashMap<String, Option<BTreeSet<usize>>> = [
        ("a.php".to_string(), Some(BTreeSet::from([9]))),
        ("b.php".to_string(), Some(BTreeSet::from([1]))),
    ]
    .into_iter()
    .collect();
    let mut bypassed = warn123(1, 1, None, None);
    bypassed.path = "a.php".to_string();
    bypassed.bypass_changed_line_filtering = Some(true);
    let mut rescued = warn123(1, 1, None, None);
    rescued.path = "a.php".to_string();
    rescued.locations = vec![OtherLocation {
        path: None,
        line: 9,
        char: 0,
        original: None,
    }];
    // A line-less message on an unknown path still drops.
    let mut dropped = warn123(0, 0, None, None);
    dropped.path = "elsewhere.php".to_string();
    let kept = filter_changed_lines(vec![bypassed, rescued, dropped], &changed);
    assert_eq!(kept.len(), 2);
}

#[test]
fn lint15_position_sort_is_stable_without_code_tiebreak() {
    let mut result = LintResult::new("a.txt".to_string(), PathBuf::from("a.txt"), b"x\n".to_vec());
    let mut first = warn123(2, 1, None, None);
    first.code = "ZZZ".to_string();
    let mut second = warn123(2, 1, None, None);
    second.code = "AAA".to_string();
    result.add_message(first);
    result.add_message(second);
    result.sort_messages();
    assert_eq!(codes(&result.messages), vec!["ZZZ", "AAA"]);
}

#[test]
fn lint16_renderer_list_follows_class_map_order() {
    assert_eq!(
        all_renderers(),
        vec!["xml", "compiler", "console", "json", "none", "summary"]
    );
}

#[test]
fn lint17_binary_diff_markers_span_git_svn_and_hg() {
    assert!(is_binary_diff_marker("Binary files a/b and b/b differ"));
    assert!(is_binary_diff_marker("Files a/b and b/b differ"));
    assert!(is_binary_diff_marker(
        "Cannot display: file marked as a binary type."
    ));
    assert!(is_binary_diff_marker("Binary file b/b has changed"));
    assert!(is_binary_diff_marker("GIT binary patch"));
    assert!(is_binary_diff_marker("GIT binary patch\r"));
    assert!(!is_binary_diff_marker("Binary files differ"));
    assert!(!is_binary_diff_marker("+GIT binary patch"));
    assert!(!is_binary_diff_marker("diff --git a/b b/b"));
}

#[test]
fn lint18_patcher_allows_one_past_end_and_null_char_backs_up() {
    // One past the last line appends instead of erroring.
    let append = warn123(3, 1, Some(""), Some("tail"));
    let (patched, applied) = build_patched_data(b"a\n", &[append]).unwrap();
    assert_eq!(patched, b"a\ntail");
    assert_eq!(applied, vec![true]);
    // Two past the end still errors, with PHP's `$line`-renders-0 text.
    let past = warn123(4, 1, Some(""), Some("tail"));
    assert_eq!(
        build_patched_data(b"a\n", &[past]).unwrap_err(),
        "Data has fewer than 0 lines."
    );
    // A null char on line 1 lands before the start, so the dirty cursor
    // skips the message.
    let skipped = warn123(1, 0, Some(""), Some("Q"));
    let (patched, applied) = build_patched_data(b"a\n", &[skipped]).unwrap();
    assert_eq!(patched, b"a\n");
    assert_eq!(applied, vec![false]);
    // On later lines it patches one byte before the line start.
    let backed = warn123(2, 0, Some("\n"), Some("Q"));
    let (patched, applied) = build_patched_data(b"a\nb\n", &[backed]).unwrap();
    assert_eq!(patched, b"aQb\n");
    assert_eq!(applied, vec![true]);
}

#[test]
#[cfg(unix)]
fn lint22_single_engine_strips_directories() {
    let tmp = TempDir::new("lint22");
    let dir = tmp.path().join("sub");
    std::fs::create_dir(&dir).unwrap();
    let mut single = Engine::new();
    single.single = true;
    assert!(single.is_single());
    assert!(single.lint_file(&dir, "sub").unwrap().is_empty());
    assert!(!Engine::new().is_single());
}

#[test]
fn lint24_wrap_matches_phutil_console_wrap() {
    // Short lines pass through verbatim (no synthetic newline, spacing kept).
    assert_eq!(word_wrap("aa  bb", 4), "    aa  bb");
    assert_eq!(word_wrap("", 4), "");
    assert_eq!(word_wrap("a\nb", 4), "    a\n    b");
    // Content wraps at 78 - indent over characters, not bytes: the
    // multibyte tail stays whole on the second line.
    let text = format!("{} {} tail", "a ".repeat(9) + "a", "\u{e9}".repeat(60));
    let wrapped = word_wrap(&text, 4);
    assert_eq!(
        wrapped,
        format!("    {}a\n    {} tail", "a ".repeat(9), "\u{e9}".repeat(60))
    );
    // ANSI escapes ride along without counting toward the width.
    assert_eq!(word_wrap("\x1b[31mhi\x1b[m", 4), "    \x1b[31mhi\x1b[m");
}

#[test]
fn lint25_empty_file_renders_one_blank_context_line() {
    let message = warn123(1, 1, None, None);
    let rendered = render_context(&message, b"");
    assert!(!rendered.contains("past the end"), "{rendered}");
    assert!(rendered.contains('^'), "{rendered}");
}

#[test]
fn lint30_phutil_xhpast_standard_matches_php() {
    let config = phutil_xhpast_standard_config();
    assert_eq!(config["xhpast.php-version"], serde_json::json!("5.2.3"));
    assert_eq!(
        config["xhpast.php-version.windows"],
        serde_json::json!("5.3.0")
    );
    assert!(
        config["xhpast.blacklisted.function"]["eval"]
            .as_str()
            .unwrap()
            .contains("should be avoided")
    );
    assert_eq!(
        config["xhpast.dynamic-string.classes"]["ExecFuture"],
        serde_json::json!(0)
    );
    assert_eq!(
        config["xhpast.dynamic-string.functions"]["qsprintf"],
        serde_json::json!(1)
    );
    assert_eq!(
        config["xhpast.dynamic-string.functions"]["pht"],
        serde_json::json!(0)
    );
}

#[test]
fn lint36_line_wrap_width_prints_raw() {
    let mut data = vec![b'x'; 1501];
    data.push(b'\n');
    let found = TextLinter::with_max_line_length(1500).lint_bytes("w.txt", &data);
    assert_eq!(codes(&found), vec!["TXT3"]);
    assert_eq!(
        found[0].description,
        "This line is 1,501 characters long, but the convention is 1500 characters."
    );
}

#[test]
fn lint37_sections_and_rules_keep_file_order() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"z-last": {"type": "text"}, "a-first": {"type": "text"}}}"#,
    )
    .unwrap();
    let names: Vec<&str> = config.linters.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["z-last", "a-first"]);
    // `severity.rules` first match wins in file order, not sorted order.
    let overrides = arclint::SeverityOverrides {
        map: HashMap::new(),
        rules: vec![
            ("/TXT9/".to_string(), "warning".to_string()),
            ("/T/".to_string(), "error".to_string()),
        ],
    };
    assert_eq!(overrides.severity_for("TXT9", "error"), "warning");
}

#[test]
fn lint40_option_setter_failures_use_in_key_context() {
    let config =
        arclint::parse_arclint(r#"{"linters": {"j": {"type": "jshint", "standard": "bogus"}}}"#)
            .unwrap();
    let err = Engine::from_config(&config, Path::new(".")).err().unwrap();
    assert!(
        err.starts_with("Error in parsing '.arclint' file, in key 'standard' for linter 'j'.\n"),
        "{err}"
    );
    let config = arclint::parse_arclint(
        r#"{"linters": {"c": {"type": "csharp", "severity": {"SA1600": "disabled"}}}}"#,
    )
    .unwrap();
    let err = Engine::from_config(&config, Path::new(".")).err().unwrap();
    assert!(
        err.starts_with("Error in parsing '.arclint' file, in key 'severity' for linter 'c'.\n"),
        "{err}"
    );
    // Type-shape failures stay under the `for linter` context.
    let config =
        arclint::parse_arclint(r#"{"linters": {"j": {"type": "jshint", "flags": "nope"}}}"#)
            .unwrap();
    let err = Engine::from_config(&config, Path::new(".")).err().unwrap();
    assert!(
        err.starts_with("Error in parsing '.arclint' file, for linter 'j'.\n"),
        "{err}"
    );
}

#[test]
fn lint41_negative_max_line_length_flags_every_line() {
    assert!(option_value_matches(
        OptionType::Int,
        &serde_json::json!(-5)
    ));
    let mut linter = TextLinter::new();
    let ctx = OptionContext {
        project_root: Path::new("."),
    };
    linter
        .set_option("text.max-line-length", &serde_json::json!(-5), &ctx)
        .unwrap();
    // Every line, including empty ones (`strlen > -5` always holds).
    let found = linter.lint_bytes("n.txt", b"a\n\n");
    let wraps: Vec<_> = found.iter().filter(|m| m.code == "TXT3").collect();
    assert_eq!(wraps.len(), 3);
    assert!(
        wraps
            .iter()
            .all(|m| m.description.contains("convention is -5 characters"))
    );
}

#[test]
fn lint43_newline_patch_keeps_raw_bytes() {
    let data = b"a\xffb\r\n";
    let found = text("r.txt", data);
    let dos = found.iter().find(|m| m.code == "TXT1").unwrap();
    let (patched, _) = build_patched_data(data, std::slice::from_ref(dos)).unwrap();
    assert_eq!(patched, b"a\xffb\n");
}

#[test]
fn lint44_backslash_hidden_is_not_a_dotfile() {
    assert_eq!(codes(&text("foo\\.hidden", b"")), vec!["TXT10"]);
    assert!(text(".hidden", b"").is_empty());
}

#[test]
fn d22_message_to_dictionary_has_php_keys() {
    // T1153 DIFF-D22: `unresolvedLint` dicts carry the 12
    // `toDictionary()` keys (`ArcanistLintMessage.php:56-71`).
    let message = LintMessage {
        path: "f.php".to_string(),
        line: 3,
        col: 7,
        code: "X".to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: "n".to_string(),
        description: "d".to_string(),
        original: Some("a".to_string()),
        replacement: Some("b".to_string()),
        granularity: Some(2),
        locations: Vec::new(),
        bypass_changed_line_filtering: Some(true),
        original_bytes: None,
        replacement_bytes: None,
    };
    assert_eq!(
        message.to_dictionary(),
        serde_json::json!({
            "path": "f.php",
            "line": 3,
            "char": 7,
            "code": "X",
            "severity": "warning",
            "name": "n",
            "description": "d",
            "original": "a",
            "replacement": "b",
            "granularity": 2,
            "locations": [],
            "bypassChangedLineFiltering": true,
        })
    );
    // Unset line/char render null.
    let path_level = LintMessage {
        line: 0,
        col: 0,
        ..message
    };
    let dict = path_level.to_dictionary();
    assert_eq!(dict["line"], serde_json::Value::Null);
    assert_eq!(dict["char"], serde_json::Value::Null);
}

#[test]
fn d22_unresolved_dicts_skip_applied_messages() {
    // T1153 DIFF-D22: applied patches resolve their messages
    // (`ArcanistLintWorkflow.php:360-375`).
    let mut result = LintResult::new("f".to_string(), std::path::PathBuf::from("f"), Vec::new());
    let kept = LintMessage {
        path: "f".to_string(),
        line: 1,
        col: 1,
        code: "K".to_string(),
        severity: SEVERITY_ERROR.to_string(),
        name: "n".to_string(),
        description: "kept".to_string(),
        original: None,
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    };
    let fixed = LintMessage {
        code: "F".to_string(),
        description: "fixed".to_string(),
        ..kept.clone()
    };
    result.add_message(kept);
    result.add_message(fixed);
    result.applied = vec![false, true];
    let dicts = unresolved_dicts(std::slice::from_ref(&result));
    assert_eq!(dicts.len(), 1);
    assert_eq!(dicts[0]["description"], serde_json::json!("kept"));
}

#[test]
fn d22_bare_child_amend_gates_like_argv_flags() {
    // T1153 DIFF-D22: `diff`'s bare amend intent enables the amend
    // prompt (gated on VCS support + mutable history, `:248-252`).
    let tmp = TempDir::new("d22-amend");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let work = WorkCopy {
        vcs: Vcs::Git,
        vcs_root: root.clone(),
        project_root: root.clone(),
    };
    let bare = Options {
        child_amend: true,
        ..Default::default()
    };
    assert!(amend_enabled(&work, &bare).unwrap());
    assert!(!amend_enabled(&work, &Options::default()).unwrap());
    // Immutable history disables even the bare intent.
    std::fs::write(root.join(".arcconfig"), r#"{"history.immutable": true}"#).unwrap();
    assert!(!amend_enabled(&work, &bare).unwrap());
}
#[test]
fn r10_lint_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "LINT WORKFLOW\n\
         \n\
         lint\n\
         \x20     lint [options] [paths]\n\
         \x20     lint [options] --rev [rev]\n\
         \n\
         \x20         Supports: git, svn, hg\n\
         \x20         Run static analysis on changes to check for mistakes. If no files\n\
         \x20         are specified, lint will be run on all files which have been\n\
         modified.\n\
         \n\
         \x20     --amend-all\n\
         \x20         When linting git repositories, amend HEAD with all patches\n\
         \x20         suggested by lint without prompting.\n\
         \n\
         \x20     --amend-autofixes\n\
         \x20         When linting git repositories, amend HEAD with autofix patches\n\
         \x20         suggested by lint without prompting.\n\
         \n\
         \x20     --apply-patches\n\
         \x20         Apply patches suggested by lint to the working copy without\n\
         \x20         prompting.\n\
         \n\
         \x20     --engine classname\n\
         \x20         Override configured lint engine for this project.\n\
         \n\
         \x20     --everything\n\
         \x20         Lint all tracked files in the working copy. Ignored files and\n\
         \x20         untracked files will not be linted.\n\
         \n\
         \x20     --lintall\n\
         \x20         Show all lint warnings, not just those on changed lines. When paths\n\
         \x20         are specified, this is the default behavior.\n\
         \n\
         \x20     --never-apply-patches\n\
         \x20         Never apply patches suggested by lint.\n\
         \n\
         \x20     --outfile path\n\
         \x20         Output the linter results to a file. Defaults to stdout.\n\
         \n\
         \x20     --output format\n\
         \x20         Select an output format.\n\
         \n\
         \x20     --rev revision\n\
         \x20         Lint changes since a specific revision.\n\
         \n\
         \x20     --severity string\n\
         \x20         Set minimum message severity. One of: 'advice', 'autofix',\n\
         \x20         'warning', 'error', 'disabled'. Defaults to 'advice'.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("LINT WORKFLOW"), "{}", usage());
}

#[test]
fn b21_lint_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `lint`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(matches!(
        parse_args(&args(&["--lintall"])),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(parse_args(&args(&["--help"])), ParseOutcome::Help));
    let text = usage_text(false);
    assert!(text.contains("LINT WORKFLOW"), "{text}");
}

#[test]
fn b23_lint_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help lint`.
    let text = usage_text(false);
    assert!(text.starts_with("LINT WORKFLOW\n"), "{text}");
    assert!(text.contains("Run static analysis on changes"), "{text}");
    assert!(text.contains("--severity"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// -( T1170 coverage )-------------------------------------------------------

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

fn init_git_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init", "-q"]);
    git(tmp.path(), &["config", "user.email", "lint-unit@test"]);
    git(tmp.path(), &["config", "user.name", "lint unit"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    git(tmp.path(), &["config", "core.autocrlf", "false"]);
    tmp
}

fn commit_file(dir: &Path, name: &str, contents: &str) -> String {
    std::fs::write(dir.join(name), contents).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", contents]);
    git(dir, &["rev-parse", "HEAD"])
}

fn git_work(root: &Path) -> WorkCopy {
    WorkCopy {
        vcs: Vcs::Git,
        vcs_root: root.to_path_buf(),
        project_root: root.to_path_buf(),
    }
}

/// A linter keeping every trait default.
struct DefaultLinter;

impl Linter for DefaultLinter {
    fn name(&self) -> &'static str {
        "DEFAULT"
    }

    fn lint_bytes(&self, _path: &str, _data: &[u8]) -> Vec<LintMessage> {
        Vec::new()
    }
}

/// A linter failing like PHP's `lintPath()` throws.
struct FailLinter;

impl Linter for FailLinter {
    fn name(&self) -> &'static str {
        "FAIL"
    }

    fn lint_bytes(&self, _path: &str, _data: &[u8]) -> Vec<LintMessage> {
        Vec::new()
    }

    fn should_lint_binary_files(&self) -> bool {
        true
    }

    fn take_error(&self) -> Option<String> {
        Some("boom".to_string())
    }
}

/// A linter always raising one warning.
struct NoisyLinter;

impl Linter for NoisyLinter {
    fn name(&self) -> &'static str {
        "NOISY"
    }

    fn lint_bytes(&self, path: &str, _data: &[u8]) -> Vec<LintMessage> {
        vec![LintMessage {
            path: path.to_string(),
            ..warn123(1, 1, None, None)
        }]
    }
}

#[test]
fn cov_message_line_and_json_shapes() {
    let message = warn123(3, 7, None, None);
    assert_eq!(
        message.to_line(),
        "path/to/example.c:3:7: warning: Consider this."
    );
    let json = message.to_json();
    assert_eq!(json["path"], serde_json::json!("path/to/example.c"));
    assert_eq!(json["line"], serde_json::json!(3));
    assert_eq!(json["char"], serde_json::json!(7));
    assert_eq!(json["col"], serde_json::json!(7));
    assert_eq!(json["code"], serde_json::json!("WARN123"));
    assert_eq!(json["severity"], serde_json::json!("warning"));
    assert_eq!(json["name"], serde_json::json!("Lint Warning"));
    assert_eq!(json["description"], serde_json::json!("Consider this."));
    assert_eq!(json["message"], serde_json::json!("Consider this."));
}

#[test]
fn cov_linter_trait_defaults_reject_options_and_symlinks() {
    let linter = DefaultLinter;
    assert!(!linter.should_lint_symbolic_links());
    let mut linter = DefaultLinter;
    let ctx = OptionContext {
        project_root: Path::new("."),
    };
    assert_eq!(
        linter
            .set_option("bogus", &serde_json::json!(1), &ctx)
            .unwrap_err(),
        "unknown option 'bogus'."
    );
    assert_eq!(linter.name(), "DEFAULT");
    assert!(linter.lint_bytes("f.txt", b"ok\n").is_empty());
}

#[test]
fn cov_text_linter_constructors_match_new() {
    let mut map = HashMap::new();
    map.insert("1".to_string(), SEVERITY_DISABLED.to_string());
    let linter = TextLinter::with_severity_map(map);
    assert_eq!(linter.max_line_length(), 80);
    // The disabled DOS stop lets the charset phase flag the `\r`.
    let found = linter.lint_bytes("d.txt", b"a\r\n");
    assert_eq!(codes(&found), vec!["TXT1", "TXT5"]);
    assert_eq!(TextLinter::default().max_line_length(), 80);
    assert_eq!(TextLinter::default().name(), "TXT");
}

#[test]
fn cov_text_linter_rejects_unknown_options() {
    let mut linter = TextLinter::new();
    let ctx = OptionContext {
        project_root: Path::new("."),
    };
    assert_eq!(
        linter
            .set_option("text.bogus", &serde_json::json!(1), &ctx)
            .unwrap_err(),
        "unknown option 'text.bogus'."
    );
}

#[test]
fn cov_linter_factory_returns_none_past_xml() {
    assert!(linter_for_php_to_xml("zzz-no-such-linter").is_none());
}

#[test]
fn cov_cached_message_restores_null_bypass_and_locations() {
    let mut dict: HashMap<String, serde_json::Value> = HashMap::new();
    for (key, value) in [
        ("path", serde_json::json!("a.txt")),
        ("code", serde_json::json!("TXT2")),
        ("severity", serde_json::json!("warning")),
        ("name", serde_json::json!("Tab")),
        ("description", serde_json::json!("d")),
        ("bypassChangedLineFiltering", serde_json::Value::Null),
        (
            "locations",
            serde_json::json!([
                {"path": "b.txt", "line": 2, "char": 3, "original": "x"},
                42,
            ]),
        ),
    ] {
        dict.insert(key.to_string(), value);
    }
    let cached = CachedMessage::from_dict(&dict).unwrap();
    assert_eq!(cached.message.bypass_changed_line_filtering, None);
    assert_eq!(cached.message.locations.len(), 1);
    assert_eq!(
        cached.message.locations[0],
        OtherLocation {
            path: Some("b.txt".to_string()),
            line: 2,
            char: 3,
            original: Some("x".to_string()),
        }
    );
    // Non-object entries and missing numbers fall back.
    assert!(location_from_dict(&serde_json::json!(42)).is_none());
    assert_eq!(
        location_from_dict(&serde_json::json!({})).unwrap(),
        OtherLocation {
            path: None,
            line: 0,
            char: 0,
            original: None,
        }
    );
    dict.insert(
        "bypassChangedLineFiltering".to_string(),
        serde_json::json!(true),
    );
    dict.insert(
        "locations".to_string(),
        serde_json::json!([{"line": "not-a-number"}]),
    );
    let cached = CachedMessage::from_dict(&dict).unwrap();
    assert_eq!(cached.message.bypass_changed_line_filtering, Some(true));
    assert_eq!(cached.message.locations[0].line, 0);
}

#[test]
fn cov_discover_arclint_walks_up_from_files_and_dirs() {
    let tmp = TempDir::new("cov-arclint");
    tmp.write(".arclint", r#"{"linters": {}}"#);
    let deep = tmp.path().join("a/b");
    std::fs::create_dir_all(&deep).unwrap();
    let expected = tmp.path().join(".arclint");
    assert_eq!(discover_arclint(&deep), Some(expected.clone()));
    let file = tmp.write("a/b/f.txt", "x\n");
    assert_eq!(discover_arclint(&file), Some(expected));
    // No `.arclint` above the system temp dir: nothing found.
    let bare = std::env::temp_dir().join(format!(
        "lint-unit-cov-bare-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(discover_arclint(&bare), None);
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn cov_option_tables_cover_tool_linters() {
    for (name, first, custom_after_base) in [
        ("coffeelint", "coffeelint.config", false),
        ("jscs", "jscs.config", false),
        ("phpcs", "phpcs.standard", false),
        ("puppet-lint", "puppet-lint.config", false),
        ("pylint", "pylint.config", false),
        ("rubocop", "rubocop.config", false),
        ("script-and-regex", "script-and-regex.script", false),
        ("xhpast", "xhpast.php-version", true),
    ] {
        let (table, after) = linter_option_table(name);
        assert_eq!(table[0].0, first, "{name}");
        assert_eq!(after, custom_after_base, "{name}");
        assert!(table.len() > 1, "{name}");
    }
    let (table, after) = linter_option_table("xhpast");
    assert_eq!(table.len(), 9);
    assert!(after);
    let (table, _) = linter_option_table("script-and-regex");
    assert_eq!(table[1], ("script-and-regex.regex", OptionType::Regex));
}

#[test]
fn cov_option_value_shapes_match_phutil_types() {
    assert!(option_value_matches(
        OptionType::Bool,
        &serde_json::json!(true)
    ));
    assert!(!option_value_matches(
        OptionType::Bool,
        &serde_json::json!(1)
    ));
    assert!(option_value_matches(
        OptionType::Regex,
        &serde_json::json!("/^a+$/")
    ));
    assert!(!option_value_matches(
        OptionType::Regex,
        &serde_json::json!("/(/")
    ));
    assert!(!option_value_matches(
        OptionType::Regex,
        &serde_json::json!("(")
    ));
    assert!(!option_value_matches(
        OptionType::Regex,
        &serde_json::json!(7)
    ));
    assert!(option_value_matches(
        OptionType::MapStrList,
        &serde_json::json!({"a": ["x"]})
    ));
    assert!(!option_value_matches(
        OptionType::MapStrList,
        &serde_json::json!({"a": "x"})
    ));
    assert!(option_value_matches(
        OptionType::MapStrStr,
        &serde_json::json!({"a": "x"})
    ));
    assert!(!option_value_matches(
        OptionType::MapStrStr,
        &serde_json::json!({"a": 1})
    ));
    assert!(option_value_matches(
        OptionType::MapStrInt,
        &serde_json::json!({"a": 2})
    ));
    assert!(!option_value_matches(
        OptionType::MapStrInt,
        &serde_json::json!({"a": "x"})
    ));
    assert!(!option_value_matches(
        OptionType::MapStrInt,
        &serde_json::json!([])
    ));
    assert_eq!(option_type_expectation(OptionType::Str), "a string");
    assert_eq!(option_type_expectation(OptionType::Bool), "a boolean");
    assert_eq!(
        option_type_expectation(OptionType::StrOrStrList),
        "a string or a list of strings"
    );
    assert_eq!(
        option_type_expectation(OptionType::Regex),
        "a valid regular expression"
    );
    assert_eq!(
        option_type_expectation(OptionType::MapStrList),
        "a map from strings to lists of strings"
    );
    assert_eq!(
        option_type_expectation(OptionType::MapStrStr),
        "a map from strings to strings"
    );
    assert_eq!(
        option_type_expectation(OptionType::MapStrInt),
        "a map from strings to integers"
    );
    assert_eq!(
        check_option_type("k", OptionType::Regex, &serde_json::json!("(")).unwrap_err(),
        "'k' must be a valid regular expression"
    );
    assert!(check_option_type("k", OptionType::Bool, &serde_json::json!(false)).is_ok());
}

#[test]
fn cov_unknown_linter_type_names_the_type() {
    // `parse_arclint` rejects unknown types first, so the section
    // builder is reached directly with a hand-made spec.
    let spec = arclint::LinterSpec {
        name: "x".to_string(),
        linter_type: "nope".to_string(),
        display: String::new(),
        include: Vec::new(),
        exclude: Vec::new(),
        severity: arclint::SeverityOverrides::default(),
        extra: HashMap::new(),
    };
    assert_eq!(
        build_section(&spec, &[], Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file: unknown linter type 'nope'."
    );
}

#[test]
fn cov_unknown_option_keys_name_the_key() {
    let config =
        arclint::parse_arclint(r#"{"linters": {"t": {"type": "text", "bogus": 1}}}"#).unwrap();
    assert_eq!(
        Engine::from_config(&config, Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file, for linter 't'.\nunknown option 'bogus'."
    );
}

#[test]
fn cov_standard_rejected_without_severity_customization() {
    let config =
        arclint::parse_arclint(r#"{"linters": {"g": {"type": "generated", "standard": "x"}}}"#)
            .unwrap();
    assert_eq!(
        Engine::from_config(&config, Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file, for linter 'g'.\nunknown option 'standard'."
    );
}

#[test]
fn cov_lessc_custom_options_apply_after_base() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"l": {"type": "lessc", "lessc.strict-math": true}}}"#,
    )
    .unwrap();
    assert!(Engine::from_config(&config, Path::new(".")).is_ok());
    let config = arclint::parse_arclint(
        r#"{"linters": {"l": {"type": "lessc", "lessc.strict-math": "x"}}}"#,
    )
    .unwrap();
    assert_eq!(
        Engine::from_config(&config, Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file, for linter 'l'.\n'lessc.strict-math' must be a boolean"
    );
}

#[test]
fn cov_severity_keys_rejected_without_customization() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"g": {"type": "generated", "severity": {"1": "warning"}}}}"#,
    )
    .unwrap();
    assert_eq!(
        Engine::from_config(&config, Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file, for linter 'g'.\nunknown option 'severity'."
    );
    let config = arclint::parse_arclint(
        r#"{"linters": {"g": {"type": "generated", "severity.rules": {"/^1$/": "warning"}}}}"#,
    )
    .unwrap();
    assert_eq!(
        Engine::from_config(&config, Path::new(".")).err().unwrap(),
        "Error in parsing '.arclint' file, for linter 'g'.\nunknown option 'severity.rules'."
    );
    // The `standard` shape check is unreachable through the config
    // (surplus-key rejection fires first), so it is reached directly.
    let mut linter: Box<dyn Linter> = Box::new(crate::lint_generated::GeneratedLinter::new());
    let spec = arclint::LinterSpec {
        name: "g".to_string(),
        linter_type: "generated".to_string(),
        display: "GEN".to_string(),
        include: Vec::new(),
        exclude: Vec::new(),
        severity: arclint::SeverityOverrides::default(),
        extra: HashMap::from([("standard".to_string(), serde_json::json!("phutil.xhpast"))]),
    };
    let ctx = OptionContext {
        project_root: Path::new("."),
    };
    assert_eq!(
        apply_base_options(&mut linter, &spec, "generated", &ctx).unwrap_err(),
        "Error in parsing '.arclint' file, for linter 'g'.\nunknown option 'standard'."
    );
}

#[test]
fn cov_xhpast_standard_accepts_lists_like_php() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"x": {"type": "xhpast", "standard": ["phutil.xhpast"]}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    assert_eq!(
        engine.severity_for("XHP", "XHP16", SEVERITY_WARNING),
        SEVERITY_ADVICE
    );
    assert_eq!(
        engine.severity_for("XHP", "XHP34", SEVERITY_WARNING),
        SEVERITY_ERROR
    );
}

#[test]
fn cov_apply_standard_matches_php() {
    let ctx = OptionContext {
        project_root: Path::new("."),
    };
    let mut severity = arclint::SeverityOverrides::default();
    let mut linter: Box<dyn Linter> = Box::new(crate::lint_xhpast::XhpastLinter::new());
    apply_standard(&mut severity, &mut linter, "xhpast", "phutil.xhpast", &ctx).unwrap();
    assert_eq!(severity.map.get("16").map(String::as_str), Some("advice"));
    assert_eq!(severity.map.get("34").map(String::as_str), Some("error"));
    assert_eq!(severity.map.get("87").map(String::as_str), Some("warning"));
    // Explicit entries win over the standard.
    severity.map.insert("16".to_string(), "error".to_string());
    apply_standard(&mut severity, &mut linter, "xhpast", "phutil.xhpast", &ctx).unwrap();
    assert_eq!(severity.map.get("16").map(String::as_str), Some("error"));
    assert_eq!(
        apply_standard(&mut severity, &mut linter, "xhpast", "bogus", &ctx).unwrap_err(),
        "No such linter standard. Available standards are: phutil.xhpast."
    );
    assert_eq!(
        apply_standard(&mut severity, &mut linter, "text", "phutil.xhpast", &ctx).unwrap_err(),
        "No such linter standard. Available standards are: ."
    );
}

#[test]
fn cov_engine_default_matches_new() {
    let engine = Engine::default();
    assert!(!engine.is_empty());
    assert!(!engine.is_comprehensive());
    assert_eq!(engine.cache_version(), Engine::new().cache_version());
}

#[test]
fn cov_severity_for_unknown_display_returns_default() {
    assert_eq!(
        Engine::empty().severity_for("TXT", "TXT1", SEVERITY_ERROR),
        SEVERITY_ERROR
    );
    assert_eq!(
        Engine::new().severity_for("NOPE", "X", SEVERITY_WARNING),
        SEVERITY_WARNING
    );
}

#[test]
fn cov_routing_skips_unlisted_paths() {
    let config =
        arclint::parse_arclint(r#"{"linters": {"t": {"type": "text", "include": ["/\\.php$/"]}}}"#)
            .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    assert!(engine.lint_bytes("x.txt", b"x\ty\n").is_empty());
    let found = engine.lint_bytes("x.php", b"x\ty\n");
    assert_eq!(codes(&found), vec!["TXT2"]);
}

#[test]
fn cov_unusable_cache_runs_fresh_like_php() {
    // Global granularity never reuses the cache, so the section runs
    // fresh even with an entry recorded at this version.
    let config =
        arclint::parse_arclint(r#"{"linters": {"p": {"type": "phutil-library"}}}"#).unwrap();
    let mut engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let version = engine.cache_version();
    let mut by_version = HashMap::new();
    by_version.insert(
        version,
        CachedPathResults {
            repository_version: None,
            stopped: None,
            messages: Vec::new(),
        },
    );
    let mut cached = CachedResults::new();
    cached.insert("a.php".to_string(), by_version);
    engine.set_cached_results(cached);
    let found = engine.lint_bytes("a.php", b"<?php\nclass A {}\nfunction f() {}\n");
    assert_eq!(codes(&found), vec!["PHL3"]);
}

#[test]
fn cov_repository_version_gates_cache_merge() {
    let config = arclint::parse_arclint(r#"{"linters": {"t": {"type": "text"}}}"#).unwrap();
    let mut engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let version = engine.cache_version();
    let cached_message = CachedMessage {
        message: LintMessage {
            path: "a.txt".to_string(),
            ..warn123(3, 1, None, None)
        },
        granularity: Some(GRANULARITY_REPOSITORY),
    };
    let mut by_version = HashMap::new();
    by_version.insert(
        version,
        CachedPathResults {
            repository_version: Some("r1".to_string()),
            stopped: None,
            messages: vec![cached_message],
        },
    );
    let mut cached = CachedResults::new();
    cached.insert("a.txt".to_string(), by_version);
    engine.set_cached_results(cached);
    // Mismatched versions merge nothing (the file section still
    // skips execution, but the repository message is dropped).
    engine.set_repository_version(Some("r2".to_string()));
    assert!(engine.lint_bytes("a.txt", b"ok\x01\n").is_empty());
    // Matching versions merge the cached message instead.
    engine.set_repository_version(Some("r1".to_string()));
    let merged = engine.lint_bytes("a.txt", b"ok\x01\n");
    assert_eq!(codes(&merged), vec!["WARN123"]);
}

#[test]
fn cov_dos_stop_skips_later_linters() {
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    engine.add_linter(Box::new(NoisyLinter));
    assert_eq!(codes(&engine.lint_bytes("d.txt", b"a\r\n")), vec!["TXT1"]);
    // Direct call pins the helper shape; the engine above must not
    // reach it (the DOS stop skips later linters).
    let direct = NoisyLinter.lint_bytes("d.txt", b"x");
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].code, "WARN123");
}

#[test]
fn cov_lint_file_joins_section_errors() {
    let tmp = TempDir::new("cov-fail");
    let file = tmp.write("f.txt", "ok\n");
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(FailLinter));
    assert_eq!(engine.lint_file(&file, "f.txt").unwrap_err(), "boom");
}

#[test]
fn cov_comprehensive_skips_externals() {
    let engine = Engine::comprehensive();
    let missing = PathBuf::from("externals/no-such-file.php");
    assert!(
        engine
            .lint_file(&missing, "externals/no-such-file.php")
            .unwrap()
            .is_empty()
    );
}

#[test]
#[cfg(unix)]
fn cov_symlinks_reach_only_opted_in_linters() {
    let tmp = TempDir::new("cov-symlink");
    let target = tmp.write("target.txt", "\t\n");
    let link = tmp.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    assert!(engine.lint_file(&link, "link.txt").unwrap().is_empty());
    // The filename linter opts into symlinks and still runs.
    let config = arclint::parse_arclint(r#"{"linters": {"n": {"type": "filename"}}}"#).unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    assert!(engine.lint_file(&link, "link.txt").unwrap().is_empty());
    let bad = tmp.path().join("bad name.txt");
    std::os::unix::fs::symlink(&target, &bad).unwrap();
    let found = engine.lint_file(&bad, "bad name.txt").unwrap();
    assert_eq!(codes(&found), vec!["NAME1"]);
}

#[test]
fn cov_filtered_routing_skips_unlisted_paths() {
    let config = arclint::parse_arclint(
        r#"{"linters": {"n": {"type": "filename", "include": ["/\\.php$/"]}}}"#,
    )
    .unwrap();
    let engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let tmp = TempDir::new("cov-filter-route");
    let file = tmp.path().join("b.bin");
    std::fs::write(&file, b"a\0b").unwrap();
    assert!(engine.lint_file(&file, "b.bin").unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn cov_filtered_comprehensive_allow_list_skips_chmod() {
    use std::os::unix::fs::PermissionsExt;
    // An executable zip: CHMOD fires under the plain engine, but the
    // comprehensive allow-list for `.txt` drops it on filtered paths.
    let tmp = TempDir::new("cov-filter-allow");
    let file = tmp.path().join("tool.txt");
    let mut data = b"PK\x03\x04".to_vec();
    data.extend_from_slice(&[0u8; 16]);
    std::fs::write(&file, &data).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plain = Engine::new().lint_file(&file, "tool.txt").unwrap();
    assert_eq!(codes(&plain), vec!["CHMOD1"]);
    assert!(plain[0].description.contains("should not be executable"));
    let filtered = Engine::comprehensive()
        .lint_file(&file, "tool.txt")
        .unwrap();
    assert!(filtered.is_empty());
}

#[test]
fn cov_filtered_cache_skips_execution() {
    let config = arclint::parse_arclint(r#"{"linters": {"n": {"type": "filename"}}}"#).unwrap();
    let mut engine = Engine::from_config(&config, Path::new(".")).unwrap();
    let version = engine.cache_version();
    let cached_message = CachedMessage {
        message: LintMessage {
            path: "b.bin".to_string(),
            ..warn123(1, 1, None, None)
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
    cached.insert("b.bin".to_string(), by_version);
    engine.set_cached_results(cached);
    let tmp = TempDir::new("cov-filter-cache");
    let file = tmp.path().join("b.bin");
    std::fs::write(&file, b"a\0b").unwrap();
    let merged = engine.lint_file(&file, "b.bin").unwrap();
    assert_eq!(codes(&merged), vec!["WARN123"]);
}

#[test]
fn cov_filtered_section_errors_collected() {
    let tmp = TempDir::new("cov-filter-err");
    let file = tmp.path().join("b.bin");
    std::fs::write(&file, b"a\0b").unwrap();
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(FailLinter));
    let (_, errors) = engine.lint_file_with_errors(&file, "b.bin");
    assert_eq!(errors, vec!["boom".to_string()]);
}

#[test]
fn cov_vcs_names_and_capabilities() {
    assert_eq!(Vcs::Git.system_name(), "git");
    assert_eq!(Vcs::Hg.system_name(), "hg");
    assert_eq!(Vcs::Svn.system_name(), "svn");
    assert!(Vcs::Git.supports_commit_ranges());
    assert!(Vcs::Hg.supports_commit_ranges());
    assert!(!Vcs::Svn.supports_commit_ranges());
    assert!(!Vcs::Git.is_history_default_immutable());
    assert!(Vcs::Hg.is_history_default_immutable());
    assert!(Vcs::Svn.is_history_default_immutable());
    assert!(Vcs::Git.supports_amend(Path::new(".")));
    assert!(!Vcs::Svn.supports_amend(Path::new(".")));
    // Mercurial probes the working copy; the wrapper only maps errors
    // to false.
    let tmp = TempDir::new("cov-amend");
    assert_eq!(
        Vcs::Hg.supports_amend(tmp.path()),
        crate::hg::supports_amend(tmp.path()).unwrap_or(false)
    );
}

#[test]
fn cov_discover_working_copy_from_file() {
    let tmp = TempDir::new("cov-wc-file");
    std::fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
    let file = tmp.write("repo/sub/f.txt", "x\n");
    let work = discover_working_copy(&file).unwrap();
    assert_eq!(work.vcs, Vcs::Git);
    assert_eq!(work.vcs_root, tmp.path().join("repo"));
}

#[test]
fn cov_svn_project_root_from_file_and_fallback() {
    let tmp = TempDir::new("cov-svn-root");
    std::fs::create_dir_all(tmp.path().join("svn/.svn")).unwrap();
    tmp.write("svn/proj/.arcconfig", "{}");
    // A file start resolves through its parent directory.
    let file = tmp.write("svn/proj/sub/f.txt", "x\n");
    let work = discover_working_copy(&file).unwrap();
    assert_eq!(work.vcs, Vcs::Svn);
    assert_eq!(work.project_root, tmp.path().join("svn/proj"));
    // Without any `.arcconfig`, the project root is the VCS root.
    let bare = tmp.path().join("svn/other/deep");
    std::fs::create_dir_all(&bare).unwrap();
    let work = discover_working_copy(&bare).unwrap();
    assert_eq!(work.project_root, tmp.path().join("svn"));
}

#[test]
fn cov_config_string_ignores_non_strings() {
    let tmp = TempDir::new("cov-cfg-str");
    let work = git_work(tmp.path());
    tmp.write(".arcconfig", r#"{"lint.unit.probe.key": 5}"#);
    assert_eq!(config_string(&work, "lint.unit.probe.key").unwrap(), None);
    tmp.write(".arcconfig", r#"{"lint.unit.probe.key": "v"}"#);
    assert_eq!(
        config_string(&work, "lint.unit.probe.key").unwrap(),
        Some("v".to_string())
    );
}

#[test]
fn cov_config_layers_reject_bad_json_and_shapes() {
    // Non-object JSON contributes nothing.
    let tmp = TempDir::new("cov-cfg-layer");
    let array = tmp.write("layer.json", "[1, 2]");
    assert_eq!(read_config_layer(&array, "k", false).unwrap(), None);
    // Invalid project JSON names the `.arcconfig` file.
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(".arcconfig"), "{broken").unwrap();
    let error = config_value_in(&project, None, None, None, "lint.engine").unwrap_err();
    assert!(
        error.starts_with("Unable to parse '.arcconfig' file '"),
        "{error}"
    );
}

#[test]
fn cov_local_config_path_covers_svn_and_relative_gitfiles() {
    let tmp = TempDir::new("cov-local");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(root.join(".svn")).unwrap();
    let svn = WorkCopy {
        vcs: Vcs::Svn,
        vcs_root: root.clone(),
        project_root: root.clone(),
    };
    assert_eq!(local_config_path(&svn), Some(root.join(".svn/arc/config")));
    // Relative `gitdir:` pointers resolve against the VCS root.
    std::fs::create_dir_all(tmp.path().join("real.git")).unwrap();
    std::fs::write(root.join(".git"), "gitdir: ../real.git").unwrap();
    let work = git_work(&root);
    assert_eq!(
        local_config_path(&work),
        Some(root.join("../real.git/arc/config"))
    );
}

#[test]
fn cov_git_two_commit_detection() {
    let tmp = init_git_repo("cov-twocommit");
    assert!(!git_repository_has_two_commits(tmp.path()));
    commit_file(tmp.path(), "a.txt", "one\n");
    assert!(!git_repository_has_two_commits(tmp.path()));
    commit_file(tmp.path(), "b.txt", "two\n");
    assert!(git_repository_has_two_commits(tmp.path()));
}

#[test]
fn cov_git_default_base_single_commit_uses_empty_tree() {
    let tmp = init_git_repo("cov-rootbase");
    commit_file(tmp.path(), "a.txt", "one\n");
    let work = git_work(tmp.path());
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        git_default_base(&work, &mut input, &mut |_| {}, false).unwrap(),
        crate::git::GIT_MAGIC_ROOT_COMMIT
    );
}

#[test]
fn cov_git_default_base_reads_arcconfig() {
    let tmp = init_git_repo("cov-cfgbase");
    let first = commit_file(tmp.path(), "a.txt", "one\n");
    commit_file(tmp.path(), "b.txt", "two\n");
    tmp.write(".arcconfig", r#"{"git.default-relative-commit": "HEAD~1"}"#);
    let work = git_work(tmp.path());
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        git_default_base(&work, &mut input, &mut |_| {}, false).unwrap(),
        first
    );
}

#[test]
fn cov_git_default_base_resolves_upstream() {
    let bare = TempDir::new("cov-upstream-bare");
    git(bare.path(), &["init", "-q", "--bare"]);
    let work_dir = TempDir::new("cov-upstream-work");
    let source = std::env::current_dir()
        .unwrap()
        .join(bare.path())
        .to_string_lossy()
        .into_owned();
    git(work_dir.path(), &["clone", "-q", &source, "."]);
    git(work_dir.path(), &["config", "user.email", "lint-unit@test"]);
    git(work_dir.path(), &["config", "user.name", "lint unit"]);
    commit_file(work_dir.path(), "a.txt", "one\n");
    let head = commit_file(work_dir.path(), "b.txt", "two\n");
    git(work_dir.path(), &["push", "-q", "-u", "origin", "HEAD"]);
    let work = git_work(work_dir.path());
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        git_default_base(&work, &mut input, &mut |_| {}, false).unwrap(),
        head
    );
}

#[test]
fn cov_git_default_base_reads_scratch() {
    let tmp = init_git_repo("cov-scratchbase");
    let first = commit_file(tmp.path(), "a.txt", "one\n");
    commit_file(tmp.path(), "b.txt", "two\n");
    let scratch = tmp.path().join(".git/arc/default-relative-commit");
    std::fs::create_dir_all(scratch.parent().unwrap()).unwrap();
    std::fs::write(&scratch, "HEAD~1").unwrap();
    let work = git_work(tmp.path());
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        git_default_base(&work, &mut input, &mut |_| {}, false).unwrap(),
        first
    );
}

#[test]
fn cov_git_default_base_prompts_and_remembers() {
    let tmp = init_git_repo("cov-promptbase");
    let first = commit_file(tmp.path(), "a.txt", "one\n");
    commit_file(tmp.path(), "b.txt", "two\n");
    let work = git_work(tmp.path());
    let mut prompted = String::new();
    let mut input = std::io::BufReader::new("HEAD~1\n".as_bytes());
    let base = git_default_base(
        &work,
        &mut input,
        &mut |text| prompted.push_str(text),
        false,
    )
    .unwrap();
    assert_eq!(base, first);
    assert!(
        prompted.contains("Select a Default Commit Range"),
        "{prompted}"
    );
    // The answer is remembered in the scratch file.
    let scratch = tmp.path().join(".git/arc/default-relative-commit");
    assert_eq!(std::fs::read_to_string(&scratch).unwrap(), "HEAD~1");
}

#[test]
fn cov_git_default_base_rejects_bad_relative_commits() {
    let tmp = init_git_repo("cov-badbase");
    commit_file(tmp.path(), "a.txt", "one\n");
    commit_file(tmp.path(), "b.txt", "two\n");
    let work = git_work(tmp.path());
    // A blank prompt answer defaults to `origin/master`, which names
    // nothing here.
    let mut input = std::io::BufReader::new("\n".as_bytes());
    assert_eq!(
        git_default_base(&work, &mut input, &mut |_| {}, false).unwrap_err(),
        "Relative commit 'origin/master' is not the name of a commit!"
    );
    // An unknown `.arcconfig` symbol names the missing commit.
    tmp.write(
        ".arcconfig",
        r#"{"git.default-relative-commit": "no-such-ref-zzz"}"#,
    );
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        git_default_base(&work, &mut input, &mut |_| {}, false).unwrap_err(),
        "Unable to find any git commit named 'no-such-ref-zzz' in this repository."
    );
}

#[test]
fn cov_scratch_helpers_handle_missing_and_blank() {
    // Outside any repository (`rev-parse` would otherwise walk up to
    // the real worktree): no scratch path, silent writes.
    let dir = std::env::temp_dir().join(format!(
        "lint-unit-cov-scratch-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(read_default_relative_commit(&dir), None);
    assert!(git_scratch_path(&dir).is_none());
    write_default_relative_commit(&dir, "HEAD~1");
    assert_eq!(read_default_relative_commit(&dir), None);
    std::fs::remove_dir_all(&dir).ok();
    // Blank scratch reads as missing.
    let repo = init_git_repo("cov-scratch-blank");
    let scratch = repo.path().join(".git/arc/default-relative-commit");
    std::fs::create_dir_all(scratch.parent().unwrap()).unwrap();
    std::fs::write(&scratch, "  \n").unwrap();
    assert_eq!(read_default_relative_commit(repo.path()), None);
}

#[test]
fn cov_hg_base_reports_errors() {
    let tmp = TempDir::new("cov-hgbase");
    let work = WorkCopy {
        vcs: Vcs::Hg,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let error = hg_base_for(&work, Some("tip")).unwrap_err();
    assert!(!error.is_empty(), "{error}");
    let error = hg_base_for(&work, None).unwrap_err();
    assert!(!error.is_empty(), "{error}");
}

#[test]
fn cov_collect_explicit_covers_absolute_and_root_displays() {
    let tmp = TempDir::new("cov-explicit");
    tmp.write("sub/f.txt", "x\n");
    // Contained paths display project-relative.
    let files = collect_explicit(&["sub/f.txt".to_string()], tmp.path(), tmp.path()).unwrap();
    assert_eq!(files[0].1, "sub/f.txt");
    // Absolute paths keep their absolute display outside the project.
    let outside = TempDir::new("cov-explicit-out");
    let file = outside.write("o.txt", "x\n");
    let absolute = std::env::current_dir().unwrap().join(&file);
    let files = collect_explicit(
        &[absolute.to_string_lossy().into_owned()],
        tmp.path(),
        tmp.path(),
    )
    .unwrap();
    assert_eq!(PathBuf::from(&files[0].1), absolute);
    // The project root itself displays as given.
    let root = std::env::current_dir().unwrap().join(tmp.path());
    let files = collect_explicit(
        &[root.to_string_lossy().into_owned()],
        tmp.path(),
        tmp.path(),
    )
    .unwrap();
    assert_eq!(PathBuf::from(&files[0].1), root);
}

#[test]
fn cov_dot_segments_normalize() {
    let tmp = TempDir::new("cov-dot");
    tmp.write("f.txt", "x\n");
    tmp.write("g.txt", "x\n");
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    // A leading `./` survives `components()` and is skipped.
    let files = collect_explicit(&["./f.txt".to_string()], tmp.path(), tmp.path()).unwrap();
    assert_eq!(files[0].1, "f.txt");
    // Absolute dotdot paths rebuild through the root.
    let root = std::env::current_dir().unwrap().join(tmp.path());
    let dotted = format!("{}/sub/../g.txt", root.display());
    let files = collect_explicit(&[dotted], tmp.path(), &root).unwrap();
    assert_eq!(files[0].1, "g.txt");
}

#[test]
fn cov_status_paths_reports_hg_svn_errors() {
    let tmp = TempDir::new("cov-status");
    for vcs in [Vcs::Hg, Vcs::Svn] {
        let work = WorkCopy {
            vcs,
            vcs_root: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
        };
        let error = status_paths(&work, None).unwrap_err();
        assert!(!error.is_empty(), "{vcs:?}: {error}");
    }
}

#[test]
fn cov_merge_hg_svn_status_reports_errors() {
    let tmp = TempDir::new("cov-merge");
    let mut merged = BTreeMap::new();
    assert!(merge_hg_status(tmp.path(), None, &mut merged).is_err());
    assert!(merge_hg_status(tmp.path(), Some("tip"), &mut merged).is_err());
    assert!(merge_svn_status(tmp.path(), &mut merged).is_err());
    assert!(merged.is_empty());
}

#[test]
fn cov_everything_paths_reports_hg_svn_errors() {
    let tmp = TempDir::new("cov-everything");
    for vcs in [Vcs::Hg, Vcs::Svn] {
        let work = WorkCopy {
            vcs,
            vcs_root: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
        };
        let error = everything_paths(&work).unwrap_err();
        assert!(!error.is_empty(), "{vcs:?}: {error}");
    }
}

#[test]
fn cov_changed_files_span_git_range() {
    let tmp = init_git_repo("cov-changed");
    commit_file(tmp.path(), "a.txt", "one\n");
    commit_file(tmp.path(), "b.txt", "two\n");
    std::fs::write(tmp.path().join("a.txt"), "one-changed\n").unwrap();
    let files = changed_files_in(tmp.path(), Some("HEAD~1")).unwrap();
    let names: Vec<&str> = files.iter().map(|(_, display)| display.as_str()).collect();
    assert!(names.contains(&"a.txt"), "{names:?}");
    assert!(names.contains(&"b.txt"), "{names:?}");
    assert_eq!(files[0].0, tmp.path().join(&files[0].1));
}

#[test]
fn cov_hunk_parser_ignores_malformed_headers() {
    assert!(parse_changed_lines("@@ bogus\n+nope\n").is_empty());
    let changed = parse_changed_lines("@@ bogus\n@@ -1 +1 @@\n+yes\n");
    assert_eq!(changed, BTreeSet::from([1]));
}

#[test]
fn cov_changed_lines_none_for_missing_dir_and_binary() {
    let tmp = init_git_repo("cov-changed-lines");
    commit_file(tmp.path(), "a.txt", "one\n");
    let work = git_work(tmp.path());
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        changed_lines_for(&work, &head, "missing.txt").unwrap(),
        None
    );
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    assert_eq!(changed_lines_for(&work, &head, "sub").unwrap(), None);
    // A text edit reports its added lines.
    std::fs::write(tmp.path().join("a.txt"), "one\ntwo\n").unwrap();
    assert_eq!(
        changed_lines_for(&work, &head, "a.txt").unwrap(),
        Some(BTreeSet::from([2]))
    );
    // A binary edit reports nothing.
    std::fs::write(tmp.path().join("a.txt"), b"one\n\x00bin\n").unwrap();
    assert_eq!(changed_lines_for(&work, &head, "a.txt").unwrap(), None);
}

#[test]
fn cov_changed_lines_reports_hg_svn_errors() {
    let tmp = TempDir::new("cov-cl-err");
    tmp.write("f.txt", "x\n");
    for vcs in [Vcs::Hg, Vcs::Svn] {
        let work = WorkCopy {
            vcs,
            vcs_root: tmp.path().to_path_buf(),
            project_root: tmp.path().to_path_buf(),
        };
        let error = changed_lines_for(&work, "tip", "f.txt").unwrap_err();
        assert!(!error.is_empty(), "{vcs:?}: {error}");
    }
}

#[test]
fn cov_engine_for_dir_reports_config() {
    let tmp = init_git_repo("cov-enginedir");
    tmp.write(".arclint", r#"{"linters": {"t": {"type": "text"}}}"#);
    let (engine, config) = engine_for_dir(tmp.path()).unwrap();
    assert!(!engine.is_empty());
    assert_eq!(config, Some(tmp.path().join(".arclint")));
    // A configured engine without `.arclint` reports no config file.
    let bare = init_git_repo("cov-enginedir-bare");
    bare.write(
        ".arcconfig",
        r#"{"lint.engine": "ArcanistComprehensiveLintEngine"}"#,
    );
    let (engine, config) = engine_for_dir(bare.path()).unwrap();
    assert!(engine.is_comprehensive());
    assert_eq!(config, None);
}

#[test]
fn cov_explicit_paths_with_engine_sorts_and_joins_errors() {
    let tmp = TempDir::new("cov-run");
    tmp.write("b.txt", "\t\n");
    tmp.write("a.txt", "\t\n");
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    let found = lint_explicit_paths_with_engine(
        &engine,
        tmp.path(),
        tmp.path(),
        &["b.txt".to_string(), "a.txt".to_string()],
    )
    .unwrap();
    let paths: Vec<&str> = found.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, vec!["a.txt", "b.txt"]);
    let mut failing = Engine::empty();
    failing.add_linter(Box::new(FailLinter));
    assert_eq!(
        lint_explicit_paths_with_engine(&failing, tmp.path(), tmp.path(), &["a.txt".to_string()])
            .unwrap_err(),
        "a.txt: boom"
    );
}

#[test]
fn cov_lint_explicit_paths_discovers_workcopy() {
    let tmp = init_git_repo("cov-runpaths");
    tmp.write("f.txt", "x\ty\n");
    let found = lint_explicit_paths(tmp.path(), &["f.txt".to_string()]).unwrap();
    assert_eq!(codes(&found), vec!["TXT2"]);
}

#[test]
fn cov_changed_in_with_engine_skips_deleted_and_errors() {
    let tmp = init_git_repo("cov-changedin");
    commit_file(tmp.path(), "keep.txt", "ok\n");
    commit_file(tmp.path(), "gone.txt", "ok\n");
    std::fs::write(tmp.path().join("keep.txt"), "ok\nx\ty\n").unwrap();
    std::fs::remove_file(tmp.path().join("gone.txt")).unwrap();
    let mut engine = Engine::empty();
    engine.add_linter(Box::new(TextLinter::new()));
    // The deleted path is skipped; the edit still lints.
    let found = lint_changed_in_with_engine(&engine, tmp.path(), None).unwrap();
    assert_eq!(codes(&found), vec!["TXT2"]);
    let mut failing = Engine::empty();
    failing.add_linter(Box::new(FailLinter));
    assert_eq!(
        lint_changed_in_with_engine(&failing, tmp.path(), None).unwrap_err(),
        "keep.txt: boom"
    );
}

#[test]
fn cov_tty_renderer_appends_context_and_empty() {
    let message = LintMessage {
        line: 1,
        col: 1,
        original: Some("a".to_string()),
        replacement: Some("b".to_string()),
        ..warn123(1, 1, None, None)
    };
    let result = console_result("f.txt", b"a\n", vec![message]);
    let rendered = render_console_result_with_ansi(&result, true).unwrap();
    assert!(rendered.contains("Lint for"), "{rendered}");
    assert!(rendered.contains(" - "), "{rendered}");
    assert!(rendered.contains(" + "), "{rendered}");
    let empty = console_result("f.txt", b"a\n", Vec::new());
    assert_eq!(render_console_result_with_ansi(&empty, true).unwrap(), "");
}

#[test]
fn cov_highlight_helpers_cover_boundaries() {
    // A boundary line past the end is created on request.
    let mut lines = vec!["a".to_string()];
    highlight_line_at(&mut lines, 2, 1, "x", true);
    assert_eq!(lines.len(), 2);
    let mut lines = vec!["a".to_string()];
    highlight_line_at(&mut lines, 2, 1, "x", false);
    assert_eq!(lines, vec!["a".to_string()]);
    // Multi-line impacts skip highlighting entirely.
    let mut old = vec!["a".to_string(), "b".to_string()];
    let mut new = vec!["a".to_string(), "b".to_string()];
    let patch = ContextPatch {
        line: 1,
        char: 1,
        original: "a",
        replacement: Some("a"),
        old_impact: 2,
    };
    highlight_patchable_lines(&mut old, &mut new, &patch, 1);
    assert_eq!(old, vec!["a".to_string(), "b".to_string()]);
    // Non-patchable single lines highlight the old side only.
    let mut old = vec!["abc".to_string()];
    highlight_single_line_old(&mut old, 1, 1, "b", 1);
    assert_ne!(old, vec!["abc".to_string()]);
    let mut old = vec!["abc".to_string()];
    highlight_single_line_old(&mut old, 1, 1, "b", 2);
    assert_eq!(old, vec!["abc".to_string()]);
    // The past-end warning only fires past the boundary line.
    assert_eq!(context_past_end_message(2, 1, "", 0), None);
    assert_eq!(context_past_end_message(2, 1, "", 1), None);
    assert!(context_past_end_message(2, 1, "", 2).is_some());
    assert!(context_past_end_message(2, 1, "x", 0).is_some());
    assert!(context_past_end_message(3, 1, "", 0).is_some());
}

#[test]
fn cov_context_specs_skip_line_zero() {
    assert!(context_build_specs(&[], &[], 0, 0, 0).is_empty());
    let specs = context_build_specs(&["a".to_string()], &["a".to_string()], 1, 1, 1);
    assert!(!specs.is_empty());
}

#[test]
fn cov_shrink_ranges_cover_zero_impacts() {
    let mut start = 1;
    assert_eq!(
        shrink_ranges(&["a".to_string()], &["a".to_string()], &mut start, 1, 1),
        (0, 0)
    );
    let mut start = 1;
    assert_eq!(
        shrink_ranges(
            &["s".to_string()],
            &["d".to_string(), "s".to_string()],
            &mut start,
            1,
            2
        ),
        (0, 1)
    );
}

#[test]
fn cov_location_dict_carries_original() {
    let message = LintMessage {
        locations: vec![OtherLocation {
            path: Some("b.txt".to_string()),
            line: 2,
            char: 3,
            original: Some("x".to_string()),
        }],
        ..warn123(1, 1, None, None)
    };
    let dict = message.to_dictionary();
    assert_eq!(dict["locations"][0]["original"], serde_json::json!("x"));
    let plain = warn123(1, 1, None, None);
    assert!(
        plain.to_dictionary()["locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cov_write_patch_numbers_temp_files() {
    let tmp = TempDir::new("cov-patchnum");
    let file = tmp.write("f.txt", "a\n");
    // A stale temp file forces the numbered fallback.
    std::fs::write(format!("{}.linted", file.display()), "stale").unwrap();
    let message = LintMessage {
        original: Some("a".to_string()),
        replacement: Some("b".to_string()),
        ..warn123(1, 1, None, None)
    };
    let mut result = LintResult::new("f.txt".to_string(), file.clone(), b"a\n".to_vec());
    result.add_message(message);
    assert_eq!(write_patch_to_disk(&result).unwrap(), vec![true]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "b\n");
    // The stale temp file is left untouched; the patch renamed over it.
    assert_eq!(
        std::fs::read_to_string(format!("{}.linted", file.display())).unwrap(),
        "stale"
    );
}

#[test]
fn cov_write_patch_reports_rename_failures() {
    // An empty disk path can stage its temp file but never rename it.
    let _guard = RemoveOnDrop(PathBuf::from(".linted"));
    std::fs::remove_file(".linted").ok();
    let result = LintResult::new(String::new(), PathBuf::new(), Vec::new());
    let error = write_patch_to_disk(&result).unwrap_err();
    assert!(error.starts_with("Unable to overwrite path"), "{error}");
    assert!(error.contains("patched version was left at"), "{error}");
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

#[test]
fn cov_rename_over_retries_after_removal() {
    let tmp = TempDir::new("cov-rename");
    let dest = tmp.write("dest.txt", "old");
    let missing = tmp.path().join("no-such-src");
    assert!(rename_over(&missing, &dest).is_err());
    assert!(!dest.exists());
    let src = tmp.write("src.txt", "new");
    let dest = tmp.path().join("fresh.txt");
    assert!(rename_over(&src, &dest).is_ok());
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "new");
}

#[test]
fn cov_patch_prompt_reads_missing_disk_as_dev_null() {
    let result = LintResult::new(
        "missing.txt".to_string(),
        PathBuf::from("missing-cov-file.txt"),
        Vec::new(),
    );
    let mut sink = Sink::Buffer(String::new());
    prompt_for_patch(&result, b"", &mut sink);
    let Sink::Buffer(buffer) = &sink else {
        panic!("expected a buffered sink");
    };
    assert!(buffer.is_empty(), "{buffer:?}");
}

#[test]
fn cov_empty_engine_is_no_effect() {
    use crate::conduit::EnvelopeKind;
    let tmp = init_git_repo("cov-noeffect");
    tmp.write(".arclint", r#"{"linters": {}}"#);
    let mut input = std::io::BufReader::new("".as_bytes());
    let error = run_workflow_collecting(tmp.path(), &Options::default(), &mut input, false, true)
        .unwrap_err();
    assert_eq!(error.kind, EnvelopeKind::NoEffect);
    assert_eq!(error.message, "No linters to run.");
}

#[test]
fn cov_child_lint_rejects_help_and_bad_flags() {
    use crate::conduit::EnvelopeKind;
    let tmp = init_git_repo("cov-child");
    let mut input = std::io::BufReader::new("".as_bytes());
    let error =
        run_child_collecting(tmp.path(), &args(&["--help"]), false, &mut input, true).unwrap_err();
    assert_eq!(error.kind, EnvelopeKind::Usage);
    assert_eq!(error.message, "unexpected --help in child lint argv");
    let mut input = std::io::BufReader::new("".as_bytes());
    let error = run_child_collecting(
        tmp.path(),
        &args(&["--bogus-flag"]),
        false,
        &mut input,
        true,
    )
    .unwrap_err();
    assert_eq!(error.kind, EnvelopeKind::Usage);
    assert!(!error.message.is_empty(), "{error:?}");
}

#[test]
fn cov_run_workflow_delegates_classification() {
    let tmp = init_git_repo("cov-workflow");
    tmp.write(".arclint", r#"{"linters": {"t": {"type": "text"}}}"#);
    tmp.write("clean.txt", "ok\n");
    let options = Options {
        paths: vec!["clean.txt".to_string()],
        ..Default::default()
    };
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        run_workflow(tmp.path(), &options, &mut input, false, true).unwrap(),
        0
    );
}

#[test]
fn cov_workflow_base_covers_rev_and_default() {
    let tmp = init_git_repo("cov-wfbase");
    let first = commit_file(tmp.path(), "a.txt", "one\n");
    let head = commit_file(tmp.path(), "b.txt", "two\n");
    let work = git_work(tmp.path());
    let mut input = std::io::BufReader::new("".as_bytes());
    let rev = Options {
        rev: Some("HEAD".to_string()),
        ..Default::default()
    };
    assert_eq!(
        workflow_base(&work, &rev, &mut input, false).unwrap(),
        Some(head)
    );
    let mut input = std::io::BufReader::new("HEAD~1\n".as_bytes());
    assert_eq!(
        workflow_base(&work, &Options::default(), &mut input, false).unwrap(),
        Some(first)
    );
}

#[test]
fn cov_explicit_base_covers_hg() {
    let tmp = TempDir::new("cov-expbase");
    let work = WorkCopy {
        vcs: Vcs::Hg,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let error = workflow_explicit_base(&work, "tip").unwrap_err();
    assert!(!error.is_empty(), "{error}");
}

#[test]
#[should_panic(expected = "svn --rev rejected by check_supports")]
fn cov_explicit_base_rejects_svn() {
    let tmp = TempDir::new("cov-expbase-svn");
    let work = WorkCopy {
        vcs: Vcs::Svn,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let _ = workflow_explicit_base(&work, "123");
}

#[test]
fn cov_workflow_default_base_covers_vcs() {
    let tmp = init_git_repo("cov-defbase");
    let head = commit_file(tmp.path(), "a.txt", "one\n");
    assert_eq!(head.len(), 40);
    let work = git_work(tmp.path());
    let mut input = std::io::BufReader::new("".as_bytes());
    // A single commit needs no prompt: the empty tree is the base.
    assert_eq!(
        workflow_default_base(&work, &mut input, false).unwrap(),
        Some(crate::git::GIT_MAGIC_ROOT_COMMIT.to_string())
    );
    let tmp = TempDir::new("cov-defbase-hg");
    let work = WorkCopy {
        vcs: Vcs::Hg,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let mut input = std::io::BufReader::new("".as_bytes());
    assert!(workflow_default_base(&work, &mut input, false).is_err());
    let work = WorkCopy {
        vcs: Vcs::Svn,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        workflow_default_base(&work, &mut input, false).unwrap(),
        None
    );
}

#[test]
fn cov_workflow_emit_propagates_render_errors() {
    let tmp = TempDir::new("cov-emit");
    let work = git_work(tmp.path());
    let mut message = warn123(1, 1, None, None);
    message.severity = "bogus".to_string();
    let mut result = LintResult::new(
        "f.txt".to_string(),
        tmp.path().join("f.txt"),
        b"x\n".to_vec(),
    );
    result.add_message(message);
    let mut results = vec![result];
    let mut input = std::io::BufReader::new("".as_bytes());
    assert_eq!(
        workflow_emit(
            &work,
            &Options::default(),
            &mut input,
            &mut results,
            false,
            true
        )
        .unwrap_err(),
        "Unknown lint severity 'bogus'!"
    );
}

#[test]
fn cov_workflow_render_skips_empty_results() {
    let result = LintResult::new("f.txt".to_string(), PathBuf::from("f.txt"), b"x\n".to_vec());
    let mut results = vec![result];
    let mut input = std::io::BufReader::new("".as_bytes());
    let mut sink = Sink::Buffer(String::new());
    assert_eq!(
        workflow_render(
            &mut results,
            RENDERER_CONSOLE,
            &Options::default(),
            &mut input,
            &mut sink,
            false,
            true
        )
        .unwrap(),
        (false, true)
    );
}

#[test]
#[should_panic(expected = "renderer validated above")]
fn cov_unknown_renderer_is_unreachable() {
    let result = LintResult::new("f.txt".to_string(), PathBuf::from("f.txt"), b"x\n".to_vec());
    let mut sink = Sink::Buffer(String::new());
    let _ = render_workflow_result(&result, "bogus", &mut sink, false);
}

#[test]
fn cov_apply_patch_propagates_pipe_errors() {
    let tmp = TempDir::new("cov-applypatch");
    let message = LintMessage {
        original: Some("a".to_string()),
        replacement: Some("b".to_string()),
        ..warn123(1, 1, None, None)
    };
    let mut result = LintResult::new(
        "f.txt".to_string(),
        tmp.path().join("f.txt"),
        b"a\n".to_vec(),
    );
    result.add_message(message);
    assert!(result.is_patchable());
    assert!(!result.is_all_autofix());
    let mut sink = Sink::Buffer(String::new());
    let mut input = std::io::BufReader::new("".as_bytes());
    // Piped stdin cannot answer the patch prompt.
    assert_eq!(
        apply_result_patch(&mut result, true, &mut sink, &mut input, false, false).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
}

#[test]
fn cov_flush_outfile_stages_beside_bare_names() {
    let name = format!(
        "lint-unit-cov-out-{}-{}.txt",
        std::process::id(),
        rand_suffix()
    );
    let _guard = RemoveOnDrop(PathBuf::from(&name));
    flush_outfile(&name, "renderer output").unwrap();
    assert_eq!(std::fs::read_to_string(&name).unwrap(), "renderer output");
}

#[test]
fn cov_amend_head_covers_hg_and_svn() {
    let tmp = TempDir::new("cov-amendhead");
    let hg = WorkCopy {
        vcs: Vcs::Hg,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    assert!(amend_head(&hg).is_err());
    let svn = WorkCopy {
        vcs: Vcs::Svn,
        vcs_root: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
    };
    assert!(amend_head(&svn).is_ok());
}

#[test]
fn cov_amend_git_head_reports_add_failures() {
    // Outside any repository (git would otherwise walk up to the real
    // worktree and stage its changes): `git add -u` fails with stderr.
    let dir = std::env::temp_dir().join(format!(
        "lint-unit-cov-amendgit-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let error = amend_git_head(&dir).unwrap_err();
    assert!(error.starts_with("git add -u failed"), "{error}");
    assert!(error.contains("not a git repository"), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(target_os = "linux")]
fn cov_spawn_with_retry_retries_text_file_busy() {
    // Linux-only: executing a binary held open for writing fails with
    // ETXTBSY there (macOS permits the spawn), exercising the retry
    // arm; the runner retries, then surfaces the last error unchanged.
    let tmp = TempDir::new("cov-etxtbsy");
    let copy = tmp.path().join("busy-copy");
    std::fs::copy(std::env::current_exe().unwrap(), &copy).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _held = std::fs::File::options().write(true).open(&copy).unwrap();
    let mut command = std::process::Command::new(&copy);
    // `--help` only guards the impossible: were the spawn to succeed,
    // the harness would print help and exit instead of running tests.
    command.arg("--help");
    let err = spawn_with_retry(&mut command).unwrap_err();
    assert!(is_text_file_busy(&err), "{err:?}");
}

#[test]
fn cov_normalize_lexical_drops_dot_segments() {
    assert_eq!(normalize_lexical(Path::new("a/./b")), PathBuf::from("a/b"));
}

#[test]
fn cov_shrink_ranges_head_continues_past_shared_lines() {
    let old = vec!["a".to_string(), "x".to_string()];
    let new = vec!["a".to_string(), "x".to_string()];
    let mut start = 1;
    assert_eq!(shrink_ranges(&old, &new, &mut start, 2, 2), (0, 0));
    assert_eq!(start, 3);
}

#[test]
fn cov_shrink_ranges_tail_continues_past_shared_lines() {
    let old = vec!["p".to_string(), "a".to_string()];
    let new = vec!["q".to_string(), "a".to_string()];
    let mut start = 1;
    assert_eq!(shrink_ranges(&old, &new, &mut start, 2, 2), (1, 1));
}

#[test]
fn cov_rename_over_replaces_a_file_with_a_directory() {
    let tmp = TempDir::new("cov-renamedir");
    let from = tmp.path().join("from-dir");
    std::fs::create_dir(&from).unwrap();
    let dest = tmp.write("dest.txt", "old");
    rename_over(&from, &dest).unwrap();
    assert!(dest.is_dir());
    assert!(!from.exists());
}
