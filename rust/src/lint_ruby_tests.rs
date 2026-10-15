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
