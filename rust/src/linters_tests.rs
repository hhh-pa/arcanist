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
    assert_eq!(LINTERS.len(), 33);
    let mut classes: Vec<&str> = LINTERS.iter().map(|meta| meta.class).collect();
    classes.sort_unstable();
    classes.dedup();
    assert_eq!(classes.len(), 33);
    // Config names are unique; displays collide only on JSON.
    let mut configs: Vec<&str> = LINTERS.iter().map(|meta| meta.config_name).collect();
    configs.sort_unstable();
    configs.dedup();
    assert_eq!(configs.len(), 33);
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
    // Native with base options only.
    let names: Vec<&str> = meta("ArcanistTextLinter")
        .options()
        .iter()
        .map(|option| option.name)
        .collect();
    assert_eq!(names, vec!["severity", "severity.rules", "standard"]);
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
    // Tool help substitutes the binary and display names.
    let bin = &meta("ArcanistFlake8Linter").options()[0];
    assert_eq!(
        meta("ArcanistFlake8Linter").option_help(bin),
        "The binary to run (default: 'flake8')."
    );
    let flags = &meta("ArcanistFlake8Linter").options()[1];
    assert_eq!(
        meta("ArcanistFlake8Linter").option_help(flags),
        "Additional flags to pass to 'flake8' when running flake8."
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
    assert_eq!(
        parse(Cppcheck, "Cppcheck 2.10.3\n"),
        Some("2.10.3".to_string())
    );
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
    assert_eq!(
        parse(Pyflakes, "2.1.1 Python 3.9.0 on linux\n"),
        Some("2.1.1".to_string())
    );
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
    // Search matches config, info, and description text.
    let found = filter_search(&entries, &["closure".to_string()]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].meta.class, "ArcanistClosureLinter");
    let found = filter_search(&entries, &["lint".to_string()]);
    assert!(found.len() > 10);
    let found = filter_search(&entries, &["".to_string()]);
    assert_eq!(found.len(), 33);
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
    assert_eq!(sorted.len(), 33);
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
    assert!(
        rendered.starts_with(
            "\n CONFIGURED  Basic Text Linter (ArcanistTextLinter)\n\n    Version 9.9\n"
        )
    );
    assert!(rendered.contains("\n    Basic linting for text files.\n"));
    assert!(rendered.contains("\n    Configuration Options\n\n"));
    assert!(rendered.contains("    severity (optional map<string, string>)\n"));
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
    assert_eq!(
        no_match_message(&["a".to_string(), "b".to_string()]),
        "No match found. Try `arc linters --search a,b` to search for a linter.\n"
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
