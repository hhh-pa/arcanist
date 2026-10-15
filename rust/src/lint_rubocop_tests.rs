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
