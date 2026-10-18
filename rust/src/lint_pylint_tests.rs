use super::*;

#[test]
fn name_binary_and_flags_match_php() {
    assert_eq!(PyLintLinter::new().name(), "PyLint");
    assert_eq!(PyLintLinter::new().name(), "PyLint");
    assert_eq!(BINARY, "pylint");
    assert_eq!(
        mandatory_flags(),
        vec![
            "--reports=no",
            "--msg-template={line}|{column}|{msg_id}|{symbol}|{msg}",
        ]
    );
}

#[test]
fn supported_paths_are_python_files() {
    for path in ["a.py", "dir/b.pyw", "C:\\proj\\c.PY"] {
        assert!(is_supported_path(path), "for {path}");
    }
    for path in ["a.pyx", "a.pyc", "a.txt", "a.rb", "py", ""] {
        assert!(!is_supported_path(path), "for {path}");
    }
}

#[test]
fn parses_template_output_with_severities() {
    let stdout = "1|0|C0114|missing-module-docstring|Missing module docstring\n\
        2|4|E0602|undefined-variable|Undefined variable 'x'\n\
        3|0|W0611|unused-import|Unused import os\n\
        4|8|R0903|too-few-public-methods|Too few public methods (0/2)\n\
        5|2|F0001|fatal|Some fatal thing\n";
    let messages = parse_output("pkg/mod.py", 1, stdout);
    assert_eq!(messages.len(), 5);

    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (1, 0));
    assert_eq!(messages[0].code, "C0114");
    assert_eq!(messages[0].name, "Missing Module Docstring");
    assert_eq!(messages[0].severity, "advice");

    assert_eq!(messages[1].code, "E0602");
    assert_eq!(messages[1].name, "Undefined Variable");
    assert_eq!(messages[1].severity, "error");

    assert_eq!(messages[2].severity, "warning");
    assert_eq!(messages[3].severity, "advice");
    assert_eq!(messages[4].severity, "error");
}

#[test]
fn negative_column_clamps_to_zero() {
    // T9257: pylint sometimes reports -1 as the column.
    let messages = parse_output("a.py", 1, "3|-1|W0611|unused-import|Unused import os\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 0));
}

#[test]
fn disabled_severity_messages_are_dropped() {
    // Codes outside R/C/W/E/F are SEVERITY_DISABLED in PHP, which the
    // engine filters out.
    let stdout = "1|0|I0020|suppressed-message|Suppressed\n\
        2|0|E0602|undefined-variable|Undefined variable 'x'\n";
    let messages = parse_output("a.py", 1, stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "E0602");
}

#[test]
fn usage_error_exit_yields_no_messages() {
    let messages = parse_output("a.py", 32, "1|0|E0602|undefined-variable|msg\n");
    assert!(messages.is_empty());
}

#[test]
fn skips_short_and_malformed_lines() {
    let messages = parse_output("a.py", 1, "only|two\n\nxx|0|E0602|s|m\n");
    assert!(messages.is_empty());
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = PyLintLinter::new();
    assert!(linter.lint_bytes("a.txt", b"x\n").is_empty());
    assert!(linter.lint_bytes("a.py", b"a\0b\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-pylint-lookup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(binary_in(dir.as_os_str()).is_none());
    let marker = dir.join(BINARY);
    std::fs::write(&marker, b"x").unwrap();
    assert_eq!(binary_in(dir.as_os_str()), Some(marker.clone()));
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let both = std::env::join_paths([&empty, &dir]).unwrap();
    assert_eq!(binary_in(&both), Some(marker.clone()));
    assert!(binary_on_path(dir.join("nope").to_str().unwrap()).is_none());
    assert!(binary_on_path(marker.to_str().unwrap()).is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(windows)]
fn binary_lookup_probes_pathext_on_windows() {
    let dir = std::env::temp_dir().join(format!("arc-rs-pylint-pathext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exts = std::env::var_os("PATHEXT")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".BAT".to_string());
    let ext = exts
        .split(';')
        .map(str::trim)
        .find(|e| !e.is_empty())
        .unwrap_or(".BAT");
    std::fs::write(dir.join(format!("{BINARY}{ext}")), b"x").unwrap();
    let found = binary_in(dir.as_os_str()).unwrap();
    assert!(
        found
            .to_string_lossy()
            .to_lowercase()
            .ends_with(&ext.to_lowercase())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn skips_lines_with_broken_fields() {
    // PHP `(int)`-casts junk columns to 0 instead of skipping the line.
    let messages = parse_output("a.py", 1, "3|xx|E0602|s|m\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 0));
    assert!(parse_output("a.py", 1, "xx|0|E0602|s|m\n").is_empty());
    assert!(parse_output("a.py", 1, "nocolumns\n").is_empty());
    assert!(parse_output("a.py", 1, "\n").is_empty());
}

#[test]
fn display_names_humanize_symbols_byte_based() {
    assert_eq!(display_name("missing-docstring"), "Missing Docstring");
    assert_eq!(display_name("école-x"), "école X");
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-pylint-tests-{tag}-{}-{n}",
            std::process::id()
        ));
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

#[cfg(unix)]
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn option_ctx(root: &Path) -> OptionContext<'_> {
    OptionContext { project_root: root }
}

fn set_bin(linter: &mut PyLintLinter, ctx: &OptionContext, tool: &Path) {
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            ctx,
        )
        .unwrap();
}

#[test]
fn set_config_feeds_the_rcfile_flag() {
    let tmp = TempDir::new("flags");
    let ctx = option_ctx(tmp.path());
    let mut linter = PyLintLinter::new();
    linter
        .set_option("pylint.config", &serde_json::json!("pylintrc"), &ctx)
        .unwrap();
    assert!(mandatory_flag_strings(Some("pylintrc")).contains(&"--rcfile=pylintrc".to_string()));
    assert!(default_flags().is_empty());
    assert_eq!(
        linter.set_option("pylint.config", &serde_json::json!(42), &ctx),
        Err("'pylint.config' must be a string".to_string())
    );
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-pylint");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = PyLintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("flags", &serde_json::json!(["--extra"]), &ctx)
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(1), &ctx)
            .is_err()
    );
}

#[test]
fn unconfigured_runs_use_the_ambient_path() {
    // Whatever the ambient `PATH` holds decides: with no `pylint` the
    // linter stays silent, with one it runs it. Only the lookup runs.
    let linter = PyLintLinter::new();
    let _ = linter.lint_bytes("a.py", b"x = 1\n");
    let _ = binary_on_path(BINARY);
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_configured_tool_and_parses_output() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-pylint",
        "#!/bin/sh\necho '1|0|C0114|missing-docstring|Missing docstring'\nexit 1\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = PyLintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    let messages = linter.lint_bytes("a.py", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "C0114");
    assert_eq!(messages[0].severity, "advice");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(tmp.path(), "fake-pylint", "#!/bin/sh\necho junk\n");
    let mut linter = PyLintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.py", b"x = 1\n").is_empty());
    let tool = write_script(
        tmp.path(),
        "fake-pylint-new",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'pylint 2.1.0'; else echo '1|0|E0602|undefined-variable|Undefined'; fi\n",
    );
    let mut linter = PyLintLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    let messages = linter.lint_bytes("a.py", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn severity_codes_need_a_letter_and_four_digits_like_php() {
    let linter = PyLintLinter::new();
    for code in ["C0111", "E0602", "W0611", "R0903", "F0001"] {
        assert_eq!(linter.validate_severity_code(code), Ok(code.to_string()));
    }
    for code in ["", "C011", "C01111", "X0111", "c0111", "C011a"] {
        assert_eq!(
            linter.validate_severity_code(code),
            Err(format!(
                "Unrecognized lint message code \"{code}\". Expected a valid Pylint lint code \
                 like \"C0111\", or \"E0602\", or \"W0611\"."
            )),
            "{code:?}"
        );
    }
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("temp-fail");
    let tool = tmp.path().join("fake-pylint");
    std::fs::write(&tool, b"x").unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut linter = PyLintLinter {
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    linter.options.bin = Some(tool);
    assert!(linter.lint_bytes("a.py", b"x = 1").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}
