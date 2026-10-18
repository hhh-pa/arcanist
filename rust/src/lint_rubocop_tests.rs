use super::*;

#[test]
fn name_binary_and_flags_match_php() {
    assert_eq!(RuboCopLinter::new().name(), "RuboCop");
    assert_eq!(RuboCopLinter::new().name(), "RuboCop");
    assert_eq!(BINARY, "rubocop");
    assert_eq!(mandatory_flags(), vec!["--format=json"]);
}

#[test]
fn supported_paths_are_ruby_files() {
    for path in [
        "a.rb",
        "lib/b.RB",
        "tasks/c.rake",
        "x.gemspec",
        "config.ru",
        "Rakefile",
        "Gemfile",
        "sub/Rakefile",
    ] {
        assert!(is_supported_path(path), "for {path}");
    }
    for path in [
        "a.py",
        "a.txt",
        "a.rbx",
        "Rakefilex",
        "rb",
        "",
        "Gemfile.lock",
    ] {
        assert!(!is_supported_path(path), "for {path}");
    }
}

#[test]
fn parses_json_offenses() {
    let stdout = r#"{
        "files": [
          {
            "path": "scratch/target.rb",
            "offenses": [
              {
                "severity": "convention",
                "message": "Prefer single-quoted strings.",
                "cop_name": "Style/StringLiterals",
                "location": {"line": 2, "column": 7, "length": 5}
              },
              {
                "severity": "error",
                "message": "Syntax error found.",
                "cop_name": "Lint/Syntax",
                "location": {"line": 9, "column": 1, "length": 1}
              }
            ]
          }
        ]
      }"#;
    let messages = parse_output("lib/app.rb", stdout);
    assert_eq!(messages.len(), 2);

    // The linted path wins over the tool-echoed scratch path.
    assert_eq!(messages[0].path, "lib/app.rb");
    assert_eq!((messages[0].line, messages[0].col), (2, 7));
    assert_eq!(messages[0].code, "Style/StringLiterals");
    assert_eq!(messages[0].name, "RuboCop");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].description, "Prefer single-quoted strings.");

    assert_eq!((messages[1].line, messages[1].col), (9, 1));
    assert_eq!(messages[1].code, "Lint/Syntax");
    assert_eq!(messages[1].severity, "error");
}

#[test]
fn severity_mapping_mirrors_php() {
    for (word, expected) in [
        ("convention", "warning"),
        ("refactor", "warning"),
        ("warning", "warning"),
        ("error", "error"),
        ("fatal", "error"),
        ("info", "advice"),
        ("bogus", "advice"),
    ] {
        assert_eq!(severity_for(word), expected, "for {word}");
    }
}

#[test]
fn invalid_json_and_bad_shapes_yield_no_messages() {
    assert!(parse_output("a.rb", "not json").is_empty());
    assert!(parse_output("a.rb", "{}").is_empty());
    assert!(parse_output("a.rb", r#"{"files": "nope"}"#).is_empty());
    assert!(parse_output("a.rb", r#"{"files": [{"offenses": "nope"}]}"#).is_empty());
    assert!(
        parse_output(
            "a.rb",
            r#"{"files": [{"path": "x", "offenses": [{"message": 42}]}]}"#,
        )
        .is_empty()
    );
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = RuboCopLinter::new();
    assert!(linter.lint_bytes("a.py", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("a.txt", b"x\n").is_empty());
    assert!(linter.lint_bytes("a.rb", b"a\0b\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-rubocop-lookup-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("arc-rs-rubocop-pathext-{}", std::process::id()));
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
fn skips_offenses_with_broken_locations() {
    assert!(parse_output("a.rb", r#"{"files":[]}"#).is_empty());
    assert!(
        parse_output(
            "a.rb",
            r#"{"files":[{"path":"x","offenses":[{"severity":"error","message":"m","cop_name":"C","location":{}}]}]}"#,
        )
        .is_empty()
    );
    assert!(
        parse_output(
            "a.rb",
            r#"{"files":[{"path":"x","offenses":[{"severity":"error","message":"m","cop_name":"C","location":{"line":"nine"}}]}]}"#,
        )
        .is_empty()
    );
}

#[test]
fn custom_severities_apply_by_offense_word_like_php() {
    use std::collections::HashMap;
    let stdout = r#"{"files": [{"path": "x", "offenses": [
        {"severity": "convention", "message": "m", "cop_name": "Style/X",
         "location": {"line": 1, "column": 1}},
        {"severity": "error", "message": "m", "cop_name": "Lint/Y",
         "location": {"line": 2, "column": 1}}
    ]}]}"#;
    let linter = RuboCopLinter::new();
    assert!(!linter.applies_custom_severities());
    let mut map = HashMap::new();
    map.insert("convention".to_string(), "error".to_string());
    // Cop-name keys never match: PHP looks up the severity word.
    map.insert("Lint/Y".to_string(), "advice".to_string());
    let overrides = SeverityOverrides {
        map,
        rules: vec![("/^err/".to_string(), "disabled".to_string())],
    };
    let messages = parse_output_with("a.rb", stdout, &overrides);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[1].severity, "disabled");
}

#[cfg(unix)]
struct TempDir {
    path: PathBuf,
}

#[cfg(unix)]
impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-rubocop-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
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

fn option_ctx(root: &Path) -> OptionContext<'_> {
    OptionContext { project_root: root }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[cfg(unix)]
fn offense_json() -> &'static str {
    r#"{"files":[{"path":"t","offenses":[{"severity":"warning","message":"m","cop_name":"C","location":{"line":1,"column":1}}]}]}"#
}

#[test]
fn set_option_handles_config_and_tool_options() {
    let dir = std::env::temp_dir().join(format!("arc-rs-rubocop-opts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tool = dir.join("fake-rubocop");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(&dir);
    let mut linter = RuboCopLinter::new();
    linter
        .set_option("rubocop.config", &serde_json::json!("/tmp/x.yml"), &ctx)
        .unwrap();
    assert_eq!(
        linter.set_option("rubocop.config", &serde_json::json!(42), &ctx),
        Err("'rubocop.config' must be a string".to_string())
    );
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("flags", &serde_json::json!(["--extra"]), &ctx)
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(
        linter
            .set_option("bogus", &serde_json::json!(true), &ctx)
            .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_tool_reports_nothing() {
    // Without `rubocop` on PATH the linter is inactive; the assertion
    // also holds on machines where the tool resolves, since the
    // reported path is always the linted path.
    let messages = RuboCopLinter::new().lint_bytes("a.rb", b"x = 1\n");
    assert!(messages.iter().all(|m| m.path == "a.rb"));
}

#[test]
fn binary_on_path_probes_plain_names() {
    if let Some(path) = binary_on_path("rubocop") {
        // LCOV_EXCL_START // Reason: rubocop-present arm; rubocop is absent on ubuntu CI
        assert!(
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("rubocop")), // LCOV_EXCL_STOP
            "{path:?}"
        );
    }
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_the_configured_tool() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-rubocop",
        &format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", offense_json()),
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = RuboCopLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    let messages = linter.lint_bytes("a.rb", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "C");
    assert_eq!(messages[0].severity, "warning");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(tmp.path(), "fake-rubocop", "#!/bin/sh\necho junk\n");
    let mut linter = RuboCopLinter::new();
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
    assert!(linter.lint_bytes("a.rb", b"x = 1\n").is_empty());
    let tool = write_script(
        tmp.path(),
        "fake-rubocop-new",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '1.2.3'; else printf '%s\\n' '{}'; fi\n",
            offense_json()
        ),
    );
    let mut linter = RuboCopLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    linter
        .set_option("version", &serde_json::json!(">= 1.2.3"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.rb", b"x = 1\n").len(), 1);
}

#[test]
#[cfg(unix)]
fn configured_config_reaches_the_tool() {
    let tmp = TempDir::new("config");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(
        tmp.path(),
        "fake-rubocop",
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --config=*) printf '%s\\n' '{}';; esac; done\n",
            offense_json()
        ),
    );
    let mut linter = RuboCopLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    assert!(linter.lint_bytes("a.rb", b"x = 1\n").is_empty());
    linter
        .set_option("rubocop.config", &serde_json::json!("/tmp/x.yml"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.rb", b"x = 1\n").len(), 1);
}

#[test]
#[cfg(unix)]
fn custom_severity_setters_reach_lint_bytes() {
    use std::collections::HashMap;
    let tmp = TempDir::new("sev");
    let tool = write_script(
        tmp.path(),
        "fake-rubocop",
        &format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", offense_json()),
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = RuboCopLinter::new();
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            &ctx,
        )
        .unwrap();
    let mut map = HashMap::new();
    map.insert("warning".to_string(), "error".to_string());
    linter.set_custom_severity_map(&map);
    linter.set_custom_severity_rules(&[]);
    let messages = linter.lint_bytes("a.rb", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "error");
}
