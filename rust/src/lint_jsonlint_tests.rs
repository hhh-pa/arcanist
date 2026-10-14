use super::*;

#[test]
fn parses_compact_lines_with_and_without_path_prefix() {
    let stderr = "a.json: line 1, col 15, Parse error on line 1\nline 2, col 3, trailing comma\nnot a finding\n";
    let messages = parse_compact_output("a.json", stderr);
    assert_eq!(messages.len(), 2);
    assert_eq!((messages[0].line, messages[0].col), (1, 15));
    assert_eq!(messages[0].description, "Parse error on line 1");
    assert_eq!(messages[0].code, "JSON");
    assert_eq!(messages[0].name, "JSON");
    assert_eq!(messages[0].severity, "error");
    assert_eq!((messages[1].line, messages[1].col), (2, 3));
    assert_eq!(messages[1].description, "Trailing comma");
    assert!(messages.iter().all(|m| m.path == "a.json"));
}

#[test]
fn greedy_prefix_split_matches_php() {
    let messages = parse_compact_output("a.json", "x: y: line 4, col 2, bad value\n");
    assert_eq!(messages.len(), 1);
    assert_eq!((messages[0].line, messages[0].col), (4, 2));
    assert_eq!(messages[0].description, "Bad value");
}

#[test]
fn empty_output_is_clean() {
    assert!(parse_compact_output("a.json", "").is_empty());
}

#[test]
fn native_check_validates_json_without_the_tool() {
    let valid = native_json_check("a.json", b"{\"a\": [1, 2]}\n");
    assert!(valid.is_empty());
    let invalid = native_json_check("a.json", b"{\"a\": [1, 2,}\n");
    assert_eq!(invalid.len(), 1);
    assert_eq!(invalid[0].code, "JSON1");
    assert_eq!(invalid[0].name, "Parse Error");
    assert_eq!(invalid[0].severity, "error");
    assert_eq!((invalid[0].line, invalid[0].col), (1, 13));
}

#[test]
fn skips_non_matching_and_binary_files() {
    let linter = JsonlintLinter::new();
    assert_eq!(linter.name(), "JSON");
    assert!(linter.lint_bytes("a.js", b"not json").is_empty());
    assert!(linter.lint_bytes("noext", b"not json").is_empty());
    assert!(linter.lint_bytes("a.json", b"{\0}").is_empty());
}

#[test]
fn missing_binary_falls_back_to_native_check() {
    let missing = std::env::temp_dir().join("arc-rs-lint-no-such-jsonlint-binary");
    let linter = JsonlintLinter::with_binary(&missing);
    assert!(linter.lint_bytes("a.json", b"{\"ok\": true}\n").is_empty());
    let found = linter.lint_bytes("a.json", b"{\"ok\": tru}\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "JSON1");
}
