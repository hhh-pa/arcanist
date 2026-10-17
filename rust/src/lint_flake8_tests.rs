use super::*;

#[test]
fn name_and_binary_match_php() {
    assert_eq!(Flake8Linter::new().name(), "flake8");
    assert_eq!(Flake8Linter::new().name(), "flake8");
    assert_eq!(BINARY, "flake8");
    assert!(mandatory_flags().is_empty());
}

#[test]
fn supported_paths_are_python_files() {
    for path in ["a.py", "dir/b.pyw", "C:\\proj\\c.PY", "src/d.PyW", ".py"] {
        assert!(is_supported_path(path), "for {path}");
    }
    for path in [
        "a.pyx",
        "a.pyc",
        "a.pyi",
        "a.txt",
        "a.rb",
        "py",
        "",
        "a.py/b.txt",
    ] {
        assert!(!is_supported_path(path), "for {path}");
    }
}

#[test]
fn parses_canonical_output_with_severities() {
    let stdout = "target.py:3:1: E302 expected 2 blank lines, found 1\n\
        target.py:5:12: W291 trailing whitespace\n\
        target.py:9: C901 'f' is too complex (12)\n";
    let messages = parse_output("src/main.py", stdout);
    assert_eq!(messages.len(), 3);

    assert_eq!(messages[0].path, "src/main.py");
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert_eq!(messages[0].code, "E302");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "flake8 E302");
    assert_eq!(messages[0].description, "expected 2 blank lines, found 1");

    assert_eq!((messages[1].line, messages[1].col), (5, 12));
    assert_eq!(messages[1].code, "W291");
    assert_eq!(messages[1].severity, "warning");

    // No column in the tool output: PHP leaves char unset, we use 1.
    assert_eq!((messages[2].line, messages[2].col), (9, 1));
    assert_eq!(messages[2].code, "C901");
    assert_eq!(messages[2].severity, "advice");
}

#[test]
fn extension_codes_are_errors() {
    let messages = parse_output("a.py", "target.py:1:1: F821 undefined name 'x'\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[0].name, "flake8 F821");
}

#[test]
fn skips_garbage_and_trims_fields() {
    let stdout = "not a finding\n\n   \ntarget.py:1:1: E000    spaced out   \nno-colon line\n";
    let messages = parse_output("a.py", stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].description, "spaced out");
}

#[test]
fn tolerates_colons_in_path() {
    // The PHP path group is lazy: the first boundary giving a full
    // match wins, so drive-letter-style prefixes still parse.
    let messages = parse_output("a.py", "C:\\t.py:2:4: E225 missing whitespace\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (2, 4));
    assert_eq!(messages[0].code, "E225");
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = Flake8Linter::new();
    assert!(linter.lint_bytes("a.txt", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("a.rb", b"puts 1\n").is_empty());
    assert!(linter.lint_bytes("a.py", b"a\0b\n").is_empty());
    assert!(linter.lint_bytes("Makefile", b"all:\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-flake8-lookup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(binary_in(dir.as_os_str()).is_none());
    let marker = dir.join(BINARY);
    std::fs::write(&marker, b"x").unwrap();
    assert_eq!(binary_in(dir.as_os_str()), Some(marker.clone()));
    // Second-directory hit after an empty first segment.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let both = std::env::join_paths([&empty, &dir]).unwrap();
    assert_eq!(binary_in(&both), Some(marker.clone()));
    // Explicit paths bypass the search.
    assert!(binary_on_path(dir.join("nope").to_str().unwrap()).is_none());
    assert!(binary_on_path(marker.to_str().unwrap()).is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(windows)]
fn binary_lookup_probes_pathext_on_windows() {
    let dir = std::env::temp_dir().join(format!("arc-rs-flake8-pathext-{}", std::process::id()));
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
fn skips_lines_with_broken_columns() {
    assert!(parse_output("a.py", "f.py:3:5 E000 x\n").is_empty());
    assert!(parse_output("a.py", "f.py:1:1: \n").is_empty());
    assert!(parse_output("a.py", "\n").is_empty());
}

#[test]
fn severity_codes_need_one_letter_like_php() {
    let linter = Flake8Linter::new();
    for code in ["E225", "W291", "F811", "C901"] {
        assert_eq!(linter.validate_severity_code(code), Ok(code.to_string()));
    }
    for code in ["", "E", "EE225", "e225", "E22a", "[E225]"] {
        assert_eq!(
            linter.validate_severity_code(code),
            Err(format!(
                "Unrecognized lint message code \"{code}\". Expected a valid flake8 lint code \
                 like \"E225\", or \"W291\", or \"F811\", or \"C901\"."
            )),
            "{code:?}"
        );
    }
}

// -( T1170 wave 5d: options, tool runs )--------------------------------------

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-flake8-tests-{tag}-{}-{n}",
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

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-flake8");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = Flake8Linter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    assert_eq!(linter.options.bin, Some(tool));
    linter
        .set_option("flags", &serde_json::json!(["--extra"]), &ctx)
        .unwrap();
    assert_eq!(linter.options.flags, Some(vec!["--extra".to_string()]));
    assert!(
        linter
            .set_option("flags", &serde_json::json!("--extra"), &ctx)
            .is_err()
    );
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert_eq!(linter.options.version.as_deref(), Some(">= 1.0"));
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(true), &ctx)
            .is_err()
    );
}

#[test]
fn binary_on_path_bare_name_reads_path() {
    // A bare tool name searches `$PATH`, exactly like `binary_in`
    // over the ambient value; no assertion on what it finds.
    let expected = std::env::var_os("PATH").and_then(|path| binary_in(&path));
    assert_eq!(binary_on_path(BINARY), expected);
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_configured_tool() {
    let tmp = TempDir::new("run");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(
        tmp.path(),
        "fake-flake8",
        "#!/bin/sh\necho 'target.py:3:1: E302 expected 2 blank lines, found 1'\n",
    );
    let mut linter = Flake8Linter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    let messages = linter.lint_bytes("a.py", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert_eq!(messages[0].code, "E302");
    assert_eq!(messages[0].severity, "error");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    // A tool reporting no parseable version stays inactive.
    let tool = write_script(tmp.path(), "fake-flake8", "#!/bin/sh\necho junk\n");
    let mut linter = Flake8Linter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.py", b"x = 1\n").is_empty());
    // A tool answering `--version` with a minor-prefixed version
    // proceeds to lint.
    let tool = write_script(
        tmp.path(),
        "fake-flake8-new",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 3.9.2; else echo 'target.py:9: C901 f is too complex'; fi\n",
    );
    let mut linter = Flake8Linter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    let messages = linter.lint_bytes("a.py", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "C901");
    assert_eq!(messages[0].severity, "advice");
}
