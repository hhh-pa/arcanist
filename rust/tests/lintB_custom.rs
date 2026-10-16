//! Public-behavior tests for bundle B config-driven linters (`SPELL`, `S&RX`,
//! `COMPOSER`, `PHL`) through the wired library.

use std::path::{Path, PathBuf};

use arc_rs::{
    lint::{self, Linter},
    lint_composer, lint_external, lint_phutil_library, lint_script_and_regex, lint_spelling,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-lintB-{tag}-{}-{n}", std::process::id()));
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

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new("tests/fixtures/lintB").join(name)).unwrap()
}

/// A fake external tool: a script that records its argv, replays canned
/// stdout/stderr, and exits with a fixed status.
struct FakeTool {
    dir: TempDir,
    script: PathBuf,
}

impl FakeTool {
    fn script(&self) -> &Path {
        &self.script
    }

    fn args_text(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("args.txt")).unwrap_or_default()
    }
}

fn fake_tool(tag: &str, stdout_text: &str, stderr_text: &str, exit_code: i32) -> FakeTool {
    let dir = TempDir::new(tag);
    std::fs::write(dir.path().join("out.txt"), stdout_text).unwrap();
    std::fs::write(dir.path().join("err.txt"), stderr_text).unwrap();
    let script = write_fake_script(
        &dir,
        !stdout_text.is_empty(),
        !stderr_text.is_empty(),
        exit_code,
    );
    FakeTool { dir, script }
}

#[cfg(windows)]
fn write_fake_script(
    dir: &TempDir,
    emit_stdout: bool,
    emit_stderr: bool,
    exit_code: i32,
) -> PathBuf {
    let path = dir.path().join("tool.bat");
    let out = dir.path().join("out.txt").display().to_string();
    let err = dir.path().join("err.txt").display().to_string();
    let args = dir.path().join("args.txt").display().to_string();
    let mut body = format!("@echo off\r\necho %* > \"{args}\"\r\n");
    if emit_stdout {
        body.push_str(&format!("type \"{out}\"\r\n"));
    }
    if emit_stderr {
        body.push_str(&format!("type \"{err}\" 1>&2\r\n"));
    }
    body.push_str(&format!("exit /b {exit_code}\r\n"));
    std::fs::write(&path, body).unwrap();
    path
}

#[cfg(not(windows))]
fn write_fake_script(
    dir: &TempDir,
    emit_stdout: bool,
    emit_stderr: bool,
    exit_code: i32,
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.path().join("tool.sh");
    let out = dir.path().join("out.txt").display().to_string();
    let err = dir.path().join("err.txt").display().to_string();
    let args = dir.path().join("args.txt").display().to_string();
    let mut body = format!("#!/bin/sh\necho \"$@\" > \"{args}\"\n");
    if emit_stdout {
        body.push_str(&format!("cat \"{out}\"\n"));
    }
    if emit_stderr {
        body.push_str(&format!("cat \"{err}\" >&2\n"));
    }
    body.push_str(&format!("exit {exit_code}\n"));
    std::fs::write(&path, &body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn echo_parser(
    path: &str,
    exit_code: i32,
    stdout: &str,
    _stderr: &str,
) -> Option<Vec<lint::LintMessage>> {
    Some(vec![lint::LintMessage {
        path: path.to_string(),
        line: 1,
        col: 1,
        code: format!("ECHO.exit{exit_code}"),
        severity: "error".to_string(),
        name: "echo".to_string(),
        description: stdout.trim().to_string(),
        original: None,
        replacement: None,
        granularity: None,
        locations: Vec::new(),
        bypass_changed_line_filtering: None,
        original_bytes: None,
        replacement_bytes: None,
    }])
}

fn tool_config(tool: &FakeTool) -> lint_external::ExternalToolConfig {
    lint_external::ExternalToolConfig {
        binaries: vec![tool.script().display().to_string()],
        ..Default::default()
    }
}

#[test]
fn registry_names_match_config_names() {
    assert_eq!(lint_spelling::SpellingLinter::new().name(), "SPELL");
    assert_eq!(
        lint_script_and_regex::ScriptAndRegexLinter::new().name(),
        "S&RX"
    );
    assert_eq!(lint_composer::ComposerLinter::new().name(), "COMPOSER");
    assert_eq!(
        lint_phutil_library::PhutilLibraryLinter::new().name(),
        "PHL"
    );
    // Local-only runner identifier (the PHP base class is abstract and
    // nameless): must never be registered in `all_linters()`.
    assert_eq!(
        lint_external::ExternalLinter::new().name(),
        lint_external::NAME_LOCAL
    );
}

#[test]
fn spelling_fixture_reports_seven_warnings() {
    let linter = lint_spelling::SpellingLinter::new();
    let data = fixture("spelling_sample.txt");
    let messages = linter.lint_bytes("spelling_sample.txt", &data);
    assert_eq!(messages.len(), 7);
    assert!(
        messages
            .iter()
            .all(|m| m.severity == "warning" && m.name == "Possible Spelling Mistake")
    );
    let codes: Vec<&str> = messages.iter().map(|m| m.code.as_str()).collect();
    assert_eq!(codes.iter().filter(|c| **c == "SPELL1").count(), 4);
    assert_eq!(codes.iter().filter(|c| **c == "SPELL2").count(), 3);
    assert_eq!((messages[0].line, messages[0].col), (1, 5));
}

#[test]
fn spelling_supports_custom_dictionaries() {
    let mut linter = lint_spelling::SpellingLinter::empty();
    linter
        .load_dictionary_json(r#"{"rules": {"exact": {"wrod": "word"}}}"#)
        .unwrap();
    let messages = linter.lint_bytes("f.txt", b"a wrod here\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "SPELL1");

    let linter = lint_spelling::SpellingLinter::from_dictionary_json(
        r#"{"rules": {"partial": {"xx": "yy"}}}"#,
    )
    .unwrap();
    assert_eq!(linter.rule_counts(), (0, 1));
    assert!(lint_spelling::SpellingLinter::from_dictionary_json("{}").is_err());
}

#[test]
fn spelling_skips_binaries() {
    let linter = lint_spelling::SpellingLinter::new();
    assert!(linter.lint_bytes("f.txt", b"teh\0\n").is_empty());
}

#[test]
fn script_and_regex_default_is_silent_but_parser_ports_rules() {
    let silent = lint_script_and_regex::ScriptAndRegexLinter::new();
    assert!(silent.lint_bytes("f.txt", b"error:1 boom\n").is_empty());

    fn parser(output: &str) -> Vec<lint_script_and_regex::ScriptRegexMatch> {
        output
            .lines()
            .filter_map(|line| {
                let (severity, rest) = line.split_once(':')?;
                let (number, message) = rest.split_once(' ')?;
                Some(lint_script_and_regex::ScriptRegexMatch {
                    severity: Some(severity.to_string()),
                    line: Some(number.to_string()),
                    message: Some(message.to_string()),
                    ..lint_script_and_regex::ScriptRegexMatch::default()
                })
            })
            .collect()
    }
    let messages = lint_script_and_regex::parse_output_with(
        parser,
        "error:3 Too many goats!\n",
        "f.txt",
        b"a\nb\nc\n",
    )
    .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "S&RX");
    assert_eq!(messages[0].severity, "error");
    // T1153 LINT-27: no `char` group means PHP null (0).
    assert_eq!((messages[0].line, messages[0].col), (3, 0));
}

#[test]
fn script_and_regex_missing_script_is_inactive() {
    fn parser(_output: &str) -> Vec<lint_script_and_regex::ScriptRegexMatch> {
        vec![lint_script_and_regex::ScriptRegexMatch::message_only("x")]
    }
    let linter = lint_script_and_regex::ScriptAndRegexLinter::with_script_and_parser(
        "definitely-missing-lint-script-xyz",
        parser,
    );
    assert!(linter.lint_bytes("f.txt", b"data\n").is_empty());
}

#[test]
fn external_default_and_missing_tool_are_silent() {
    let silent = lint_external::ExternalLinter::new();
    assert!(silent.lint_bytes("f.txt", b"data\n").is_empty());

    fn parser(
        _path: &str,
        _exit_code: i32,
        _stdout: &str,
        _stderr: &str,
    ) -> Option<Vec<lint::LintMessage>> {
        Some(Vec::new())
    }
    let config = lint_external::ExternalToolConfig {
        binaries: vec!["definitely-missing-lint-binary-xyz".to_string()],
        ..lint_external::ExternalToolConfig::default()
    };
    let linter = lint_external::ExternalLinter::with_config(config, parser);
    assert!(linter.lint_bytes("f.txt", b"data\n").is_empty());
}

#[test]
fn external_parsers_resolve_on_canned_output() {
    fn parser(
        path: &str,
        _exit_code: i32,
        stdout: &str,
        _stderr: &str,
    ) -> Option<Vec<lint::LintMessage>> {
        if stdout.is_empty() {
            return Some(Vec::new());
        }
        Some(vec![lint::LintMessage {
            path: path.to_string(),
            line: 2,
            col: 1,
            code: "EXT1".to_string(),
            severity: "error".to_string(),
            name: "External".to_string(),
            description: stdout.to_string(),
            original: None,
            replacement: None,
            granularity: None,
            locations: Vec::new(),
            bypass_changed_line_filtering: None,
            original_bytes: None,
            replacement_bytes: None,
        }])
    }
    let messages = lint_external::resolve_output(parser, "f.txt", 0, "boom\n", "", true).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert!(lint_external::resolve_output(parser, "f.txt", 1, "", "", true).is_err());
}

#[test]
fn composer_fixtures_match_stale_and_fresh() {
    let linter = lint_composer::ComposerLinter::new();
    let ok_json = fixture("composer_ok/composer.json");
    let stale_json = fixture("composer_stale/composer.json");

    let messages = linter.lint_bytes("tests/fixtures/lintB/composer_ok/composer.json", &ok_json);
    assert!(messages.is_empty());

    let messages = linter.lint_bytes(
        "tests/fixtures/lintB/composer_stale/composer.json",
        &stale_json,
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "COMPOSER1");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "Lock file out-of-date");
    assert_eq!(
        messages[0].description,
        "The 'composer.lock' file seems to be out-of-date. You probably need to run \
         `composer update`."
    );

    // Missing or invalid sibling locks stay silent.
    let nolock_json = fixture("composer_nolock/composer.json");
    assert!(
        linter
            .lint_bytes(
                "tests/fixtures/lintB/composer_nolock/composer.json",
                &nolock_json
            )
            .is_empty()
    );
    let badlock_json = fixture("composer_badlock/composer.json");
    assert!(
        linter
            .lint_bytes(
                "tests/fixtures/lintB/composer_badlock/composer.json",
                &badlock_json
            )
            .is_empty()
    );
}

#[test]
fn composer_absolute_paths_round_trip() {
    let tmp = TempDir::new("composer");
    let json = b"{\n  \"name\": \"example/tmp\"\n}\n";
    let lock = format!("{{\n  \"hash\": \"{}\"\n}}\n", lint_composer::md5_hex(json));
    std::fs::write(tmp.path().join("composer.json"), json).unwrap();
    std::fs::write(tmp.path().join("composer.lock"), &lock).unwrap();
    let display = tmp
        .path()
        .join("composer.json")
        .to_string_lossy()
        .to_string();

    let linter = lint_composer::ComposerLinter::new();
    assert!(linter.lint_bytes(&display, json).is_empty());

    let edited = b"{\n  \"name\": \"example/changed\"\n}\n";
    let messages = linter.lint_bytes(&display, edited);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "COMPOSER1");
}

#[test]
fn composer_md5_matches_system_hash() {
    // Independent cross-check: the ok fixture hash was generated with the
    // OS hasher, and must equal the linter's own MD5 of those bytes.
    let json = fixture("composer_ok/composer.json");
    assert_eq!(
        lint_composer::md5_hex(&json),
        "a1249ea6a85e0af837799a740161cfb3"
    );
}

#[test]
fn phutil_fixtures_cover_one_class_per_file() {
    let linter = lint_phutil_library::PhutilLibraryLinter::new();
    let single = fixture("phutil_single_class.php");
    assert!(
        linter
            .lint_bytes("phutil_single_class.php", &single)
            .is_empty()
    );
    let functions = fixture("phutil_functions.php");
    assert!(
        linter
            .lint_bytes("phutil_functions.php", &functions)
            .is_empty()
    );
    let html = fixture("phutil_html.php");
    assert!(linter.lint_bytes("phutil_html.php", &html).is_empty());
    let anonymous = fixture("phutil_anonymous.php");
    assert!(
        linter
            .lint_bytes("phutil_anonymous.php", &anonymous)
            .is_empty()
    );

    let two = fixture("phutil_two_classes.php");
    let messages = linter.lint_bytes("phutil_two_classes.php", &two);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHL3");
    assert_eq!((messages[0].line, messages[0].col), (5, 1));

    let mixed = fixture("phutil_mixed.php");
    let messages = linter.lint_bytes("phutil_mixed.php", &mixed);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHL3");
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
}

#[test]
fn phutil_global_helpers_find_duplicates_and_unknowns() {
    let defs = [
        lint_phutil_library::SymbolDef {
            symbol: "Widget".to_string(),
            symbol_type: lint_phutil_library::SymbolType::Class,
            library: "lib".to_string(),
            file: "a.php".to_string(),
            offset: 0,
        },
        lint_phutil_library::SymbolDef {
            symbol: "Widget".to_string(),
            symbol_type: lint_phutil_library::SymbolType::Class,
            library: "lib".to_string(),
            file: "b.php".to_string(),
            offset: 10,
        },
    ];
    let duplicates = lint_phutil_library::check_duplicates(&defs, &[]);
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].code, "PHL2");

    let uses = [lint_phutil_library::SymbolUse {
        symbol: "Missing".to_string(),
        use_type: lint_phutil_library::UseType::Class,
        library: "lib".to_string(),
        file: "a.php".to_string(),
        offset: 5,
    }];
    let unknown = lint_phutil_library::check_unknown_symbols(&uses, &defs, &[], "arc", "/arc");
    assert_eq!(unknown.len(), 1);
    assert_eq!(unknown[0].code, "PHL1");
}

#[test]
fn engine_lint_file_covers_disk_semantics() {
    let tmp = TempDir::new("lintfile");
    let php = tmp.path().join("two.php");
    std::fs::write(&php, b"<?php\n\nclass A {}\n\nclass B {}\n").unwrap();
    let display = php.to_string_lossy().to_string();
    let mut engine = lint::Engine::empty();
    engine.add_linter(Box::new(lint_phutil_library::PhutilLibraryLinter::new()));
    let messages = engine.lint_file(&php, &display).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "PHL3");

    let bin = tmp.path().join("bin.php");
    std::fs::write(&bin, b"<?php\0 class A {} class B {}\n").unwrap();
    let bin_display = bin.to_string_lossy().to_string();
    assert!(engine.lint_file(&bin, &bin_display).unwrap().is_empty());

    let missing = tmp.path().join("missing.php");
    let missing_display = missing.to_string_lossy().to_string();
    // Deleted files are filtered from every linter: no error.
    assert!(
        engine
            .lint_file(&missing, &missing_display)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn lint_cli_plumbing_smoke() {
    // Shared `lint` entry points the bundle flows through.
    assert_eq!(
        lint::run(&["--help".to_string()]),
        std::process::ExitCode::SUCCESS
    );
    let tmp = TempDir::new("plumbing");
    assert!(
        lint::collect_explicit(&[], tmp.path(), tmp.path())
            .unwrap()
            .is_empty()
    );
    assert!(
        lint::lint_explicit_paths(tmp.path(), &["lintB-definitely-missing-xyz".to_string()])
            .is_err()
    );
    // Outside any working copy (the system temp dir is outside the
    // worktree), selection fails.
    let outside = std::env::temp_dir().join(format!("lintB-nogit-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    assert!(lint::changed_files_in(&outside, None).is_err());
    assert!(lint::lint_changed_in(&outside, None).is_err());
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn bundle_linters_run_together_in_one_engine() {
    let mut engine = lint::Engine::empty();
    engine.add_linter(Box::new(lint_spelling::SpellingLinter::new()));
    engine.add_linter(Box::new(lint_script_and_regex::ScriptAndRegexLinter::new()));
    engine.add_linter(Box::new(lint_composer::ComposerLinter::new()));
    engine.add_linter(Box::new(lint_phutil_library::PhutilLibraryLinter::new()));
    // Only the spelling linter fires on prose; the rest self-filter.
    let messages = engine.lint_bytes("notes.txt", b"fix teh begining\n");
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.code.starts_with("SPELL")));
    assert!(
        engine
            .lint_bytes("clean.txt", b"all good here\n")
            .is_empty()
    );
}

#[test]
fn configured_runner_executes_binary_with_flags() {
    let tool = fake_tool("external-run", "tool says hi\n", "", 0);
    let mut config = tool_config(&tool);
    config.flags = vec!["--check".to_string()];
    let linter = lint_external::ExternalLinter::with_config(config, echo_parser);
    let messages = linter.lint_bytes("any.txt", b"data\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "any.txt");
    assert_eq!(messages[0].code, "ECHO.exit0");
    assert_eq!(messages[0].description, "tool says hi");
    let argv = tool.args_text();
    assert!(argv.contains("--check"), "{argv}");
    assert!(argv.contains("any.txt"), "{argv}");
}

#[test]
fn version_gate_silences_mismatch_and_runs_match() {
    let tool = fake_tool("external-ver", "v\n", "", 1);
    let mut mismatch = tool_config(&tool);
    mismatch.version_requirement = Some(">= 99.0".to_string());
    mismatch.known_version = Some("1.0".to_string());
    let linter = lint_external::ExternalLinter::with_config(mismatch, echo_parser);
    assert!(linter.lint_bytes("any.txt", b"data\n").is_empty());

    let mut matched = tool_config(&tool);
    matched.version_requirement = Some(">= 1.0".to_string());
    matched.known_version = Some("1.0".to_string());
    let linter = lint_external::ExternalLinter::with_config(matched, echo_parser);
    let messages = linter.lint_bytes("any.txt", b"data\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "v");
}

#[test]
fn interpreter_mode_runs_through_interpreter() {
    let tool = fake_tool("external-interp", "via interp\n", "", 0);
    let script = tool.script().display().to_string();
    let config = lint_external::ExternalToolConfig {
        binaries: vec![script.clone()],
        interpreters: vec![script],
        ..Default::default()
    };
    let linter = lint_external::ExternalLinter::with_config(config, echo_parser);
    let messages = linter.lint_bytes("any.txt", b"data\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "via interp");
}

#[test]
fn missing_candidates_name_script_or_binary() {
    let missing_interp = lint_external::ExternalToolConfig {
        binaries: vec!["x".to_string()],
        interpreters: vec!["arc-rs-missing-interp".to_string()],
        ..Default::default()
    };
    let err = lint_external::check_binary_configuration(&missing_interp).unwrap_err();
    assert!(err.contains("interpreter"), "{err}");

    let tool = fake_tool("external-resolve", "", "", 0);
    let missing_script = lint_external::ExternalToolConfig {
        binaries: vec!["arc-rs-missing-script".to_string()],
        interpreters: vec![tool.script().display().to_string()],
        ..Default::default()
    };
    let err = lint_external::check_binary_configuration(&missing_script).unwrap_err();
    assert!(err.contains("script"), "{err}");

    let missing_binary = lint_external::ExternalToolConfig {
        binaries: vec!["arc-rs-missing-binary".to_string()],
        ..Default::default()
    };
    let err = lint_external::check_binary_configuration(&missing_binary).unwrap_err();
    assert!(err.contains("binary"), "{err}");
}

#[test]
fn binary_exists_edges() {
    assert!(!lint_external::binary_exists(""));
    assert!(!lint_external::binary_exists("arc-rs-definitely-missing"));
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    assert!(lint_external::binary_exists(shell));
}

#[test]
fn unknown_version_reports_upgrade_hint() {
    let err =
        lint_external::check_version_requirement("L", "b", None, ">= 1.0", Some("run update"))
            .unwrap_err();
    assert!(err.contains("TO UPGRADE: run update"), "{err}");
}
