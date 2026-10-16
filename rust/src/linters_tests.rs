//! Unit tests for [`super`]: the registry, version parsers,
//! selection, and rendering.

use super::*;

fn meta(class: &str) -> &'static LinterMeta {
    LINTERS.iter().find(|meta| meta.class == class).unwrap()
}

fn entry(class: &str) -> LinterEntry {
    LinterEntry {
        meta: meta(class),
        status: LinterStatus::Available,
        version: None,
        error: None,
    }
}

#[test]
fn registry_lists_all_thirty_three_classes() {
    // T1153 LINTERS-L1: `filename` is listed, so the registry holds 34 classes.
    assert_eq!(LINTERS.len(), 34);
    let mut classes: Vec<&str> = LINTERS.iter().map(|meta| meta.class).collect();
    classes.sort_unstable();
    classes.dedup();
    assert_eq!(classes.len(), 34);
    // Config names are unique; displays collide only on JSON.
    let mut configs: Vec<&str> = LINTERS.iter().map(|meta| meta.config_name).collect();
    configs.sort_unstable();
    configs.dedup();
    assert_eq!(configs.len(), 34);
    assert_eq!(meta("ArcanistClosureLinter").display, "GJSLINT");
    assert_eq!(meta("ArcanistClosureLinter").config_name, "gjslint");
    assert_eq!(meta("ArcanistClosureLinter").info_name, "Closure Linter");
}

#[test]
fn compat_registry_keeps_thirty_four_display_names() {
    let all = available_linters();
    assert_eq!(all.len(), 34);
    assert!(all.iter().any(|info| info.name == "TXT"));
    assert!(all.iter().all(|info| !info.description.is_empty()));
    assert_eq!(find_linter("txt").unwrap().name, "TXT");
    assert!(find_linter("nope").is_none());
}

#[test]
fn options_tables_match_php() {
    // Native with one custom option before the base table.
    // T1153 LINTERS-L11: `text.max-line-length` is listed.
    let names: Vec<&str> = meta("ArcanistTextLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(
        names,
        vec![
            "text.max-line-length",
            "severity",
            "severity.rules",
            "standard"
        ]
    );
    // Non-customizing natives show nothing.
    assert!(meta("ArcanistGeneratedLinter").options().is_empty());
    assert!(meta("ArcanistJSONLinter").options().is_empty());
    // C# shows only its own option.
    let names: Vec<&str> = meta("ArcanistCSharpLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(names, vec!["embedded"]);
    // External: custom, tool, base.
    let names: Vec<&str> = meta("ArcanistJscsLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(
        names,
        vec![
            "jscs.preset",
            "jscs.config_path",
            "bin",
            "flags",
            "version",
            "severity",
            "severity.rules",
            "standard",
        ]
    );
    // lessc appends its options after the tool/base tables.
    let names: Vec<&str> = meta("ArcanistLesscLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(
        names,
        vec![
            "bin",
            "flags",
            "version",
            "severity",
            "severity.rules",
            "standard",
            "lessc.strict-math",
            "lessc.strict-units",
        ]
    );
    // External without base options.
    let names: Vec<&str> = meta("ArcanistPhpLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(names, vec!["bin", "flags", "version"]);
    // T1153 LINTERS-L9/L10: base and tool option types and helps match PHP.
    let text_opts = meta("ArcanistTextLinter").options();
    let severity = text_opts.iter().find(|o| o.name == "severity").unwrap();
    assert_eq!(severity.option_type, "optional map<string|int, string>");
    assert!(severity.help.starts_with("Provide a map from lint codes"));
    let standard = text_opts.iter().find(|o| o.name == "standard").unwrap();
    assert_eq!(standard.option_type, "optional string | list<string>");
    let flake_opts = meta("ArcanistFlake8Linter").options();
    let bin = flake_opts.iter().find(|o| o.name == "bin").unwrap();
    assert_eq!(bin.option_type, "optional string | list<string>");
    assert!(
        meta("ArcanistFlake8Linter")
            .option_help(bin)
            .starts_with("Specify a string")
    );
    let version = flake_opts.iter().find(|o| o.name == "version").unwrap();
    assert!(
        meta("ArcanistFlake8Linter")
            .option_help(version)
            .contains("version comparison operator")
    );
    // T1153 LINTERS-L17: XHP merges rule options after the base table.
    let names: Vec<&str> = meta("ArcanistXHPASTLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(
        names,
        vec![
            "severity",
            "severity.rules",
            "standard",
            "xhpast.php-version",
            "xhpast.php-version.windows",
            "xhpast.blacklisted.function",
            "xhpast.deprecated.functions",
            "xhpast.printf-functions",
            "xhpast.switchhook",
            "xhpast.naminghook",
            "xhpast.dynamic-string.classes",
            "xhpast.dynamic-string.functions",
        ]
    );
}

#[test]
fn version_parsers_match_php_regexes() {
    use VersionPattern::*;
    let parse = |pattern, stdout: &str| parse_version(pattern, stdout, "");
    assert_eq!(parse(ExactTriple, "1.9.1\n"), Some("1.9.1".to_string()));
    assert_eq!(parse(ExactTriple, "1.9\n"), None);
    assert_eq!(parse(ExactTriple, "v1.9.1\n"), None);
    assert_eq!(parse(Cppcheck, "Cppcheck 2.10\n"), Some("2.10".to_string()));
    // LINT-35: ceb `^\d+\.\d+$` rejects patch versions.
    assert_eq!(parse(Cppcheck, "Cppcheck 2.10.3\n"), None);
    assert_eq!(parse(Cppcheck, "Cppcheck 2.10rc1\n"), None);
    assert_eq!(parse(Cppcheck, "cppcheck 2.10\n"), None);
    assert_eq!(parse(CssV, "v2.2.4\n"), Some("2.2.4".to_string()));
    assert_eq!(parse(CssV, "2.2.4\n"), None);
    assert_eq!(
        parse(PrefixMinor, "1.7.0 (pep8: 1.7.1)\n"),
        Some("1.7.0".to_string())
    );
    assert_eq!(parse(PrefixMinor, "2.0\n"), Some("2.0".to_string()));
    assert_eq!(parse(PrefixMinor, "2.0b1\n"), None);
    assert_eq!(
        parse(Hlint, "HLint v3.2.7, (C) Neil Mitchell\n"),
        Some("3.2.7".to_string())
    );
    assert_eq!(parse(Hlint, "nope\n"), None);
    assert_eq!(
        parse_version(JshintStderr, "", "jshint v2.9.7\n"),
        Some("2.9.7".to_string())
    );
    assert_eq!(parse_version(JshintStderr, "jshint v2.9.7\n", ""), None);
    assert_eq!(parse(LenientTriple, "1.1.1\n"), Some("1.1.1".to_string()));
    assert_eq!(
        parse(PuppetLint, "puppet-lint 2.3.6\n"),
        Some("2.3.6".to_string())
    );
    // LINT-34: ceb `^\d+\.\d+\.\d+$` rejects the ` Python…` suffix.
    assert_eq!(parse(Pyflakes, "2.1.1 Python 3.9.0 on linux\n"), None);
    assert_eq!(parse(Pyflakes, "2.1.1\n"), Some("2.1.1".to_string()));
    assert_eq!(parse(Pyflakes, "2.1.1x\n"), None);
    assert_eq!(
        parse(Pylint, "pylint 2.6.0\nastroid 2.4.2\n"),
        Some("2.6.0".to_string())
    );
    assert_eq!(
        parse(Phpcs, "PHP_CodeSniffer version 3.5.8 (stable)\n"),
        Some("3.5.8".to_string())
    );
    assert_eq!(parse(Phpcs, "PHP_CodeSniffer version 3.5\n"), None);
    assert_eq!(
        parse(Ruby, "ruby 2.7.0p0 (2019-12-25) [x86_64-linux]\n"),
        Some("2.7.0".to_string())
    );
    assert_eq!(parse(PhpVersion, "8.1.2\n"), Some("8.1.2".to_string()));
    assert_eq!(parse(PhpVersion, "\n"), None);
    assert_eq!(parse(RunOnly, "whatever\n"), None);
}

#[test]
fn selection_matches_php() {
    let entries = collect_entries(&[]);
    // Exact matches configuration names only.
    let exact = filter_exact(&entries, &["text".to_string(), "gjslint".to_string()]);
    assert_eq!(exact.len(), 2);
    assert!(filter_exact(&entries, &["TXT".to_string()]).is_empty());
    assert!(filter_exact(&entries, &["Basic Text Linter".to_string()]).is_empty());
    // T1153 LINTERS-L3: exact match is case-insensitive (`strcasecmp`).
    assert_eq!(filter_exact(&entries, &["TEXT".to_string()]).len(), 1);
    assert_eq!(filter_exact(&entries, &["Text".to_string()]).len(), 1);
    // Search matches config, info, and description text.
    let found = filter_search(&entries, &["closure".to_string()]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].meta.class, "ArcanistClosureLinter");
    let found = filter_search(&entries, &["lint".to_string()]);
    assert!(found.len() > 10);
    let found = filter_search(&entries, &["".to_string()]);
    // T1153 LINTERS-L1: 34 with `filename`.
    assert_eq!(found.len(), 34);
    // Multi-term search dedups by class.
    let found = filter_search(&entries, &["php".to_string(), "PHPCS".to_string()]);
    let mut classes: Vec<&str> = found.iter().map(|entry| entry.meta.class).collect();
    classes.sort_unstable();
    classes.dedup();
    assert_eq!(classes.len(), found.len());
}

#[test]
fn sorting_is_case_insensitive_by_info_name() {
    let entries = collect_entries(&[]);
    let refs: Vec<&LinterEntry> = entries.iter().collect();
    let sorted = sort_entries(refs);
    // T1153 LINTERS-L1: 34 with `filename`.
    assert_eq!(sorted.len(), 34);
    let names: Vec<&str> = sorted.iter().map(|entry| entry.meta.info_name).collect();
    let mut expected = names.clone();
    expected.sort_by_key(|name| name.to_ascii_lowercase());
    assert_eq!(names, expected);
    // The JSON pair ties on info name; class order breaks it
    // (`ArcanistJSONLintLinter` sorts first: `L` (0x4C) is less than
    // `e` (0x65)).
    let json: Vec<&&LinterEntry> = sorted
        .iter()
        .filter(|entry| entry.meta.info_name == "JSON Lint")
        .collect();
    assert_eq!(json.len(), 2);
    assert_eq!(json[0].meta.class, "ArcanistJSONLintLinter");
    assert_eq!(json[1].meta.class, "ArcanistJSONLinter");
}

#[test]
fn plain_rendering_matches_php() {
    let entries = collect_entries(&[]);
    let refs: Vec<&LinterEntry> = filter_exact(&entries, &["text".to_string()]);
    let text = render_entries(&refs, false);
    assert!(text.starts_with(" AVAILABLE   text (Basic Text Linter)\n"));
    assert!(text.ends_with("(Run `arc linters --verbose` for more details.)\n"));
    // Statuses pad to ten columns.
    let mut configured = entry("ArcanistTextLinter");
    configured.status = LinterStatus::Configured;
    let text = render_entries(&[&configured], false);
    assert!(text.starts_with(" CONFIGURED  text (Basic Text Linter)\n"));
    let mut error = entry("ArcanistPhpLinter");
    error.status = LinterStatus::Error;
    let text = render_entries(&[&error], false);
    assert!(text.starts_with(" ERROR       php (php -l)\n"));
}

#[test]
fn verbose_rendering_matches_php() {
    let mut text = entry("ArcanistTextLinter");
    text.status = LinterStatus::Configured;
    text.version = Some("9.9".to_string());
    let rendered = render_entries(&[&text], true);
    // T1153 LINTERS-L5/L6: header shows config + info names with no leading blank.
    assert!(rendered.starts_with(" CONFIGURED  text (Basic Text Linter)\n\n    Version 9.9\n"));
    // T1153 LINTERS-L12: PHP description text.
    assert!(rendered.contains("Enforces basic text rules like line length"));
    assert!(rendered.contains("\n    Configuration Options\n\n"));
    // T1153 LINTERS-L9: PHP option types.
    assert!(rendered.contains("    severity (optional map<string|int, string>)\n"));
    assert!(rendered.ends_with("\n\n"));

    // Errors show the exception class and message.
    let mut error = entry("ArcanistPhpLinter");
    error.status = LinterStatus::Error;
    error.error = Some(LinterError {
        class: "CommandException",
        message: "boom".to_string(),
    });
    let rendered = render_entries(&[&error], true);
    assert!(rendered.contains("\n    CommandException\n    boom\n\n"));

    // XHP appends the rules table without setting the tail.
    let xhp = entry("ArcanistXHPASTLinter");
    let rendered = render_entries(&[&xhp], true);
    assert!(rendered.contains("\n    Linter Rules\n\n"));
    assert!(rendered.contains("ArcanistUnableToParseXHPASTLinterRule"));
    assert!(rendered.contains("| 2  |"));
}

#[test]
fn no_match_message_matches_php() {
    assert_eq!(
        no_match_message(&["frobnicate".to_string()]),
        "No match found. Try `arc linters --search frobnicate` to search for a linter.\n"
    );
    // T1153 LINTERS-L4: PHP reports only `$exact[0]`.
    assert_eq!(
        no_match_message(&["a".to_string(), "b".to_string()]),
        "No match found. Try `arc linters --search a` to search for a linter.\n"
    );
}

#[test]
fn args_parse_search_search_and_exact() {
    match parse_args(&["--search".to_string(), "php".to_string()]) {
        ParseOutcome::Args(options) => {
            assert_eq!(options.search, vec!["php".to_string()]);
            assert!(!options.verbose);
        }
        _ => panic!("expected args"),
    }
    match parse_args(&[
        "--search=a".to_string(),
        "--search".to_string(),
        "b".to_string(),
    ]) {
        ParseOutcome::Args(options) => {
            assert_eq!(options.search, vec!["a".to_string(), "b".to_string()]);
        }
        _ => panic!("expected args"),
    }
    match parse_args(&["--verbose".to_string(), "text".to_string()]) {
        ParseOutcome::Args(options) => {
            assert!(options.verbose);
            assert_eq!(options.exact, vec!["text".to_string()]);
        }
        _ => panic!("expected args"),
    }
    match parse_args(&["--search".to_string()]) {
        ParseOutcome::Error(message) => {
            assert_eq!(message, "Option '--search' requires a parameter.")
        }
        _ => panic!("expected error"),
    }
    match parse_args(&["--bogus".to_string()]) {
        ParseOutcome::Error(message) => {
            assert_eq!(message, "Unknown argument 'bogus'. Try 'arc help'.")
        }
        _ => panic!("expected error"),
    }
    assert!(matches!(
        parse_args(&["--help".to_string()]),
        ParseOutcome::Help
    ));
}

#[test]
fn outside_a_repo_nothing_is_configured() {
    let dir = std::env::temp_dir().join(format!("linters-norepo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let configured = configured_classes(&dir).unwrap();
    assert!(configured.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn xhpast_rules_table_has_one_ported_rule() {
    let table = xhpast_rules_table();
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 5);
    assert!(lines[0].starts_with('+'));
    assert!(lines[1].contains("ID"));
    assert!(lines[3].contains("Unable to Parse"));
}

#[test]
fn r4_lintersl24_usage_error_has_no_dump() {
    // Argument errors render `Usage Exception:` with exit 1 and no
    // usage dump (the label applies at `run_in`).
    match parse_args(&["--bogus".to_string()]) {
        ParseOutcome::Error(message) => {
            assert!(!message.contains("usage:"), "{message}");
        }
        _ => panic!("expected parse error"),
    }
    assert_eq!(
        run_in(Path::new("."), &["--bogus".to_string()]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn linters_l1_filename_linter_is_listed() {
    // T1153 LINTERS-L1: `filename` (NAME) is listed.
    let filename = meta("ArcanistFilenameLinter");
    assert_eq!(filename.config_name, "filename");
    assert_eq!(filename.display, "NAME");
    assert_eq!(filename.info_name, "Filename");
    assert!(filename.description.unwrap().contains("uninspired names"));
    assert_eq!(class_for_config("filename"), Some("ArcanistFilenameLinter"));
}

#[test]
fn linters_l2_exact_and_search_is_usage_error() {
    // T1153 LINTERS-L2: exact + `--search` is a usage error.
    assert_eq!(
        run_in(
            Path::new("."),
            &[
                "--search".to_string(),
                "php".to_string(),
                "text".to_string()
            ]
        ),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn linters_l3_exact_match_ignores_case() {
    // T1153 LINTERS-L3: exact match is case-insensitive.
    let entries = collect_entries(&[]);
    assert_eq!(filter_exact(&entries, &["TEXT".to_string()]).len(), 1);
    assert_eq!(
        filter_exact(&entries, &["Text".to_string()])[0].meta.class,
        "ArcanistTextLinter"
    );
}

#[test]
fn linters_l4_no_match_reports_first_term_only() {
    // T1153 LINTERS-L4: no-match names only the first term.
    assert_eq!(
        no_match_message(&["a".to_string(), "b".to_string()]),
        "No match found. Try `arc linters --search a` to search for a linter.\n"
    );
}

#[test]
fn linters_l5_verbose_header_matches_brief() {
    // T1153 LINTERS-L5: verbose header shows config + info names.
    let text = entry("ArcanistTextLinter");
    let verbose = render_entries(&[&text], true);
    let brief = render_entries(&[&text], false);
    let verbose_header = verbose.lines().next().unwrap().to_string();
    let brief_header = brief.lines().next().unwrap().to_string();
    assert_eq!(verbose_header, brief_header);
    assert!(verbose_header.contains("text (Basic Text Linter)"));
}

#[test]
fn linters_l6_verbose_has_no_leading_blank_line() {
    // T1153 LINTERS-L6: no leading blank line in verbose mode.
    let text = entry("ArcanistTextLinter");
    let rendered = render_entries(&[&text], true);
    assert!(!rendered.starts_with('\n'));
}

#[test]
fn linters_l7_wrapped_blocks_end_with_single_newline() {
    // T1153 LINTERS-L7: wrapped blocks end with one newline.
    let mut text = entry("ArcanistTextLinter");
    text.status = LinterStatus::Configured;
    let rendered = render_entries(&[&text], true);
    assert!(!rendered.contains("\n\n\n"), "{rendered}");
    let mut error = entry("ArcanistPhpLinter");
    error.status = LinterStatus::Error;
    error.error = Some(LinterError {
        class: "CommandException",
        message: "boom".to_string(),
    });
    let rendered = render_entries(&[&error], true);
    assert!(rendered.contains("\n    CommandException\n    boom\n"));
    assert!(!rendered.contains("boom\n\n\n"));
}

#[test]
fn linters_l8_wrap_stays_within_78_columns() {
    // T1153 LINTERS-L8: wrap stays within 78 columns.
    let entries = collect_entries(&[]);
    let refs: Vec<&LinterEntry> = entries.iter().collect();
    let rendered = render_entries(&refs, true);
    for line in rendered.lines() {
        assert!(
            line.len() <= 78,
            "overlong line ({len}): {line}",
            len = line.len()
        );
    }
}

#[test]
fn linters_l9_base_option_types_match_php() {
    // T1153 LINTERS-L9: base severity option types match PHP.
    let options = meta("ArcanistTextLinter").options();
    let find = |name| options.iter().find(|o| o.name == name).unwrap();
    assert_eq!(
        find("severity").option_type,
        "optional map<string|int, string>"
    );
    assert_eq!(
        find("severity.rules").option_type,
        "optional map<string, string>"
    );
    assert_eq!(
        find("standard").option_type,
        "optional string | list<string>"
    );
}

#[test]
fn linters_l10_tool_option_types_match_php() {
    // T1153 LINTERS-L10: tool option types match PHP, no placeholders.
    let options = meta("ArcanistFlake8Linter").options();
    let find = |name| options.iter().find(|o| o.name == name).unwrap();
    assert_eq!(find("bin").option_type, "optional string | list<string>");
    assert!(!find("bin").help.contains("{binary}"));
    assert!(!find("flags").help.contains("{display}"));
}

#[test]
fn linters_l11_text_max_line_length_option_exists() {
    // T1153 LINTERS-L11: `text.max-line-length` is listed.
    let options = meta("ArcanistTextLinter").options();
    let option = options
        .iter()
        .find(|o| o.name == "text.max-line-length")
        .unwrap();
    assert_eq!(option.option_type, "optional int");
}

#[test]
fn linters_l12_text_description_matches_php() {
    // T1153 LINTERS-L12: text description matches PHP.
    assert_eq!(
        meta("ArcanistTextLinter").description,
        Some(
            "Enforces basic text rules like line length, character encoding, and trailing whitespace."
        )
    );
}

#[test]
fn linters_l13_phpcs_facts_match_php() {
    // T1153 LINTERS-L13: phpcs facts match PHP.
    let phpcs = meta("ArcanistPhpcsLinter");
    assert_eq!(phpcs.info_name, "PHP_CodeSniffer");
    assert_eq!(
        phpcs.uri,
        Some("http://pear.php.net/package/PHP_CodeSniffer/")
    );
    assert!(phpcs.description.unwrap().contains("tokenizes PHP"));
    let option = phpcs
        .custom_options
        .iter()
        .find(|o| o.name == "phpcs.standard")
        .unwrap();
    assert_eq!(
        option.help,
        "The name or path of the coding standard to use."
    );
}

#[test]
fn linters_l14_xhpast_description_matches_php() {
    // T1153 LINTERS-L14: xhpast description matches PHP.
    assert_eq!(
        meta("ArcanistXHPASTLinter").description,
        Some("Use XHPAST to enforce coding conventions on PHP source files.")
    );
}

#[test]
fn linters_l15_xhpast_version_is_ported_rule_count() {
    // T1153 LINTERS-L15 (Rule 3 keep): version counts ported rules.
    // Rule 3 keep: only one rule is ported, so the version counts it.
    assert_eq!(
        probe_version(meta("ArcanistXHPASTLinter")),
        Ok(Some("1".to_string()))
    );
}

#[test]
fn linters_l16_xhpast_table_lists_ported_rules_only() {
    // T1153 LINTERS-L16 (Rule 3 keep): table lists ported rules only.
    // Rule 3 keep: 103 PHP-only rules are unportable; the table keeps
    // the reconstructed borders over the ported set.
    let table = xhpast_rules_table();
    assert_eq!(table.lines().count(), 5);
    assert!(table.contains("ArcanistUnableToParseXHPASTLinterRule"));
}

#[test]
fn linters_l17_xhpast_merges_rule_options() {
    // T1153 LINTERS-L17: XHP merges rule options.
    let names: Vec<&str> = meta("ArcanistXHPASTLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert!(names.contains(&"xhpast.php-version"));
    assert!(names.contains(&"xhpast.blacklisted.function"));
    assert!(names.contains(&"xhpast.dynamic-string.functions"));
    assert_eq!(names.len(), 12);
}

#[test]
fn linters_l20_jsonlint_marks_external_linter() {
    // T1153 LINTERS-L20: `jsonlint` configures the external wrapper.
    // `jsonlint` is the external wrapper's config name; the native
    // `json` linter shares its display but must not match first.
    let jsonlint = LINTERS
        .iter()
        .find(|m| m.config_name == "jsonlint")
        .unwrap();
    assert_eq!(jsonlint.class, "ArcanistJSONLintLinter");
    assert!(jsonlint.external.is_some());
    let json = LINTERS.iter().find(|m| m.config_name == "json").unwrap();
    assert_eq!(json.class, "ArcanistJSONLinter");
    assert!(json.external.is_none());
    // End to end through `.arclint`: the `jsonlint` type configures
    // the external wrapper, not the native linter.
    let dir = std::env::temp_dir().join(format!("linters-json-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arclint"),
        r#"{"linters": {"myjson": {"type": "jsonlint"}}}"#,
    )
    .unwrap();
    let configured = configured_classes(&dir).unwrap();
    assert_eq!(configured, vec!["ArcanistJSONLintLinter"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn linters_l22_comprehensive_builds_nine_linters() {
    // T1153 LINTERS-L22: comprehensive engine builds nine linters.
    let dir = std::env::temp_dir().join(format!("linters-comp-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"lint.engine": "ArcanistComprehensiveLintEngine"}"#,
    )
    .unwrap();
    let configured = configured_classes(&dir).unwrap();
    assert_eq!(
        configured,
        vec![
            "ArcanistGeneratedLinter",
            "ArcanistNoLintLinter",
            "ArcanistTextLinter",
            "ArcanistFilenameLinter",
            "ArcanistXHPASTLinter",
            "ArcanistPyFlakesLinter",
            "ArcanistPEP8Linter",
            "ArcanistRubyLinter",
            "ArcanistJSHintLinter",
        ]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn linters_l23_single_engine_reports_unknown_linter() {
    // T1153 LINTERS-L23 (Rule 3 keep): unknown linter reports PHP bytes.
    // Rule 3: custom PHP subclasses are unportable; unknown names
    // report the PHP "does not exist" bytes.
    let dir = std::env::temp_dir().join(format!("linters-single-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"lint.engine": "ArcanistSingleLintEngine", "lint.engine.single.linter": "ArcanistCustomLinter"}"#,
    )
    .unwrap();
    assert_eq!(
        configured_classes(&dir),
        Err("Linter 'ArcanistCustomLinter' configured in 'lint.engine.single.linter' does not exist!".to_string())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn linters_l25_help_uses_php_wording() {
    // T1153 LINTERS-L25: help uses PHP wording.
    // T1153 B21/B23: `--help` renders the generated workflow help.
    let help = super::usage_text(false);
    assert!(help.contains("LINTERS WORKFLOW"), "{help}");
    assert!(help.contains("List the available and configured linters"));
    assert!(help.contains("Show detailed information, including options."));
}

#[test]
fn linters_l26_probe_honors_configured_bin() {
    // T1153 LINTERS-L26: version probe honors the configured `bin`.
    // A configured `bin` replaces the default binary for the probe: a
    // bogus path yields the missing-binary error naming that path.
    let meta = meta("ArcanistFlake8Linter");
    let error = probe_version_with_bin(meta, Some("/definitely/not/a/flake8-binary")).unwrap_err();
    assert_eq!(error.class, "ArcanistMissingLinterException");
    assert!(error.message.contains("/definitely/not/a/flake8-binary"));
    // The `.arclint` `bin` reaches the configured set without probing.
    // The bin must be absolute on every platform (a bare `/...` is
    // drive-relative on Windows and resolves against the project root).
    let bin = if cfg!(windows) {
        "C:/custom/flake8"
    } else {
        "/custom/flake8"
    };
    let dir = std::env::temp_dir().join(format!("linters-bin-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arclint"),
        format!(r#"{{"linters": {{"py": {{"type": "flake8", "bin": "{bin}"}}}}}}"#),
    )
    .unwrap();
    let configured = configured_linters(&dir).unwrap();
    assert_eq!(configured.len(), 1);
    assert_eq!(configured[0].class, "ArcanistFlake8Linter");
    assert_eq!(configured[0].bin, Some(bin.to_string()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn linters_l27_missing_binary_reports_install_instructions() {
    // T1153 LINTERS-L27: missing binary reports install instructions.
    let meta = meta("ArcanistFlake8Linter");
    let error = probe_version_with_bin(meta, Some("arc-rs-no-such-binary-xyz")).unwrap_err();
    assert_eq!(error.class, "ArcanistMissingLinterException");
    assert!(error.message.contains("Unable to locate binary"));
    assert!(
        error
            .message
            .contains("TO INSTALL: Install flake8 using `pip install flake8`.")
    );
}

#[test]
fn linters_l28_brief_mode_shows_probe_errors() {
    // T1153 LINTERS-L28: brief mode shows probe errors too.
    let mut error = entry("ArcanistPhpLinter");
    error.status = LinterStatus::Error;
    error.error = Some(LinterError {
        class: "ArcanistMissingLinterException",
        message: "boom".to_string(),
    });
    let rendered = render_entries(&[&error], false);
    assert!(rendered.contains("\n    ArcanistMissingLinterException\n    boom\n"));
}

#[test]
fn b21_linters_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `linters`: it is a classic workflow,
    // so PHP runs it via the `scripts/arcanist.php` re-exec, which
    // never requires `--`. Parsing takes no TTY input, so
    // piped-style argv parses directly, and `--help` without `--`
    // still succeeds.
    assert!(matches!(
        parse_args(&["--verbose".to_string()]),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args(&["--help".to_string()]),
        ParseOutcome::Help
    ));
    let text = usage_text(false);
    assert!(text.contains("LINTERS WORKFLOW"), "{text}");
}

#[test]
fn b23_linters_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help linters`.
    let text = usage_text(false);
    assert!(text.starts_with("LINTERS WORKFLOW\n"), "{text}");
    assert!(text.contains("--search"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
