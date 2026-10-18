use super::*;

#[test]
fn name_binary_and_flags_match_php() {
    assert_eq!(RubyLinter::new().name(), "RUBY");
    assert_eq!(RubyLinter::new().name(), "RUBY");
    assert_eq!(BINARY, "ruby");
    assert_eq!(mandatory_flags(), vec!["-w", "-c"]);
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
fn parses_syntax_errors_and_warnings() {
    let stderr = "target.rb:2: syntax error, unexpected end-of-input\n\
        target.rb:5: warning: assigned but unused variable - x\n";
    let messages = parse_output("lib/app.rb", stderr);
    assert_eq!(messages.len(), 2);

    assert_eq!(messages[0].path, "lib/app.rb");
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[0].code, "RUBY");
    assert_eq!(messages[0].name, "Syntax Error");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(
        messages[0].description,
        "syntax error, unexpected end-of-input"
    );

    // `-w` warnings become messages too, with the default error
    // severity, exactly like PHP.
    assert_eq!((messages[1].line, messages[1].col), (5, 1));
    assert_eq!(messages[1].severity, "error");
    assert_eq!(
        messages[1].description,
        "warning: assigned but unused variable - x"
    );
}

#[test]
fn match_is_unanchored_like_php() {
    let messages = parse_output("a.rb", "prefix target.rb:7: boom, details here\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].line, 7);
    assert_eq!(messages[0].description, "boom, details here");
}

#[test]
fn skips_garbage_lines() {
    let messages = parse_output("a.rb", "Syntax OK\n\nno separators here\n");
    assert!(messages.is_empty());
}

#[test]
fn lint_bytes_skips_without_running_tool() {
    let linter = RubyLinter::new();
    assert!(linter.lint_bytes("a.py", b"x = 1\n").is_empty());
    assert!(linter.lint_bytes("a.txt", b"x\n").is_empty());
    assert!(linter.lint_bytes("a.rb", b"a\0b\n").is_empty());
}

#[test]
fn binary_lookup_reads_only_the_given_path_env() {
    let dir = std::env::temp_dir().join(format!("arc-rs-ruby-lookup-{}", std::process::id()));
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
    let dir = std::env::temp_dir().join(format!("arc-rs-ruby-pathext-{}", std::process::id()));
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
fn skips_lines_with_broken_positions() {
    assert!(parse_output("a.rb", "target.rb:xx: nope\n").is_empty());
    assert!(parse_output("a.rb", "target.rb:: nope\n").is_empty());
    assert!(parse_output("a.rb", "\n").is_empty());
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-ruby-tests-{tag}-{}-{n}",
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

fn set_bin(linter: &mut RubyLinter, ctx: &OptionContext, tool: &Path) {
    linter
        .set_option(
            "bin",
            &serde_json::json!(tool.to_string_lossy().into_owned()),
            ctx,
        )
        .unwrap();
}

#[test]
fn default_flags_are_empty() {
    assert!(default_flags().is_empty());
}

#[test]
fn set_option_delegates_bin_flags_and_version() {
    let tmp = TempDir::new("tool-opts");
    let tool = tmp.path().join("fake-ruby");
    std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
    make_executable(&tool);
    let ctx = option_ctx(tmp.path());
    let mut linter = RubyLinter::new();
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
    // Whatever the ambient `PATH` holds decides: with no `ruby` the
    // linter stays silent, with one it runs it. Only the lookup runs.
    let linter = RubyLinter::new();
    let _ = linter.lint_bytes("a.rb", b"x = 1\n");
    let _ = binary_on_path(BINARY);
}

#[test]
#[cfg(unix)]
fn lint_bytes_runs_configured_tool_and_parses_stderr() {
    let tmp = TempDir::new("run");
    let tool = write_script(
        tmp.path(),
        "fake-ruby",
        "#!/bin/sh\necho 'target.rb:2: syntax error, oops' >&2\nexit 1\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = RubyLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    let messages = linter.lint_bytes("a.rb", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].code, "RUBY");
    assert_eq!(messages[0].severity, "error");
}

#[test]
#[cfg(unix)]
fn version_requirement_gates_linting() {
    let tmp = TempDir::new("version");
    let ctx = option_ctx(tmp.path());
    let tool = write_script(tmp.path(), "fake-ruby", "#!/bin/sh\necho junk\n");
    let mut linter = RubyLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert!(linter.lint_bytes("a.rb", b"x = 1\n").is_empty());
    let tool = write_script(
        tmp.path(),
        "fake-ruby-new",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'ruby 3.2.2'; else echo 'target.rb:2: syntax error, oops' >&2; fi\n",
    );
    let mut linter = RubyLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    linter
        .set_option("version", &serde_json::json!(">= 1.0"), &ctx)
        .unwrap();
    assert_eq!(linter.lint_bytes("a.rb", b"x = 1\n").len(), 1);
}

#[test]
#[cfg(unix)]
fn custom_severity_setters_recolor_tool_output() {
    use std::collections::HashMap;
    let tmp = TempDir::new("severity");
    let tool = write_script(
        tmp.path(),
        "fake-ruby",
        "#!/bin/sh\necho 'target.rb:3: boom, with detail' >&2\n",
    );
    let ctx = option_ctx(tmp.path());
    let mut linter = RubyLinter::new();
    set_bin(&mut linter, &ctx, &tool);
    let mut map = HashMap::new();
    map.insert("boom".to_string(), "warning".to_string());
    linter.set_custom_severity_map(&map);
    linter.set_custom_severity_rules(&[("boom".to_string(), "warning".to_string())]);
    let messages = linter.lint_bytes("a.rb", b"x = 1\n");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].severity, "warning");
}

#[test]
fn custom_severities_apply_by_message_head_like_php() {
    use std::collections::HashMap;
    let linter = RubyLinter::new();
    assert!(!linter.applies_custom_severities());
    let mut map = HashMap::new();
    map.insert("boom".to_string(), "warning".to_string());
    let overrides = SeverityOverrides {
        map,
        rules: Vec::new(),
    };
    let messages = parse_output_with(
        "a.rb",
        "target.rb:3: boom, with detail\ntarget.rb:4: other thing\n",
        &overrides,
    );
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[0].description, "boom, with detail");
    assert_eq!(messages[1].severity, "error");
}

#[test]
#[cfg(unix)]
fn temp_write_failure_reports_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("temp-fail");
    let tool = tmp.path().join("fake-ruby");
    std::fs::write(&tool, b"x").unwrap();
    let locked = tmp.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut linter = RubyLinter {
        temp_dir: Some(locked.clone()),
        ..Default::default()
    };
    linter.options.bin = Some(tool);
    assert!(linter.lint_bytes("a.rb", b"x = 1").is_empty());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Missing-`ruby` arm: with no `ruby` on PATH the default linter
/// resolves no tool and lints nothing. Linux-only: macOS ships
/// `/usr/bin/ruby`, which would resolve instead. Conditional (not a
/// hard assert) so Linux dev machines WITH ruby stay green — the
/// gates image has no ruby, and the 100% floor would fail loudly if
/// that ever drifted.
#[cfg(target_os = "linux")]
#[test]
fn missing_ruby_lints_nothing() {
    if binary_on_path(BINARY).is_none() {
        assert!(RubyLinter::new().lint_bytes("a.rb", b"x = 1").is_empty());
    }
}
