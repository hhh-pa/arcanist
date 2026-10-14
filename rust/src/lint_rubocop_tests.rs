use super::*;

#[test]
fn name_binary_and_flags_match_php() {
    assert_eq!(RuboCopLinter::new().name(), "RuboCop");
    assert_eq!(RuboCopLinter.name(), "RuboCop");
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
