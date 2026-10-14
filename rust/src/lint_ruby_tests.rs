use super::*;

#[test]
fn name_binary_and_flags_match_php() {
    assert_eq!(RubyLinter::new().name(), "RUBY");
    assert_eq!(RubyLinter.name(), "RUBY");
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
