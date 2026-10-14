//! Public-behavior tests for bundle D: the Python/Ruby external-tool
//! linters (`flake8`, `PYFLAKES`, `PyLint`, `PEP8`, `RuboCop`, `RUBY`).
//!
//! Tool execution is never required: parsing is exercised on canned output,
//! and `lint_bytes` is exercised only on inputs every linter must skip
//! without a tool (wrong extension, NUL bytes).

use std::path::Path;

use arc_rs::{
    lint::{self, Linter},
    lint_flake8, lint_pep8, lint_pyflakes, lint_pylint, lint_rubocop, lint_ruby,
};

fn fixture(name: &str) -> String {
    let path = Path::new("tests/fixtures/lintD").join(name);
    std::fs::read_to_string(&path).unwrap()
}

#[test]
fn linter_names_match_php() {
    assert_eq!(lint_flake8::Flake8Linter::new().name(), "flake8");
    assert_eq!(lint_pyflakes::PyFlakesLinter::new().name(), "PYFLAKES");
    assert_eq!(lint_pylint::PyLintLinter::new().name(), "PyLint");
    assert_eq!(lint_pep8::Pep8Linter::new().name(), "PEP8");
    assert_eq!(lint_rubocop::RuboCopLinter::new().name(), "RuboCop");
    assert_eq!(lint_ruby::RubyLinter::new().name(), "RUBY");
}

#[test]
fn binaries_and_flags_match_php() {
    assert_eq!(lint_flake8::BINARY, "flake8");
    assert!(lint_flake8::mandatory_flags().is_empty());
    assert_eq!(lint_pyflakes::BINARY, "pyflakes");
    assert!(lint_pyflakes::mandatory_flags().is_empty());
    assert_eq!(lint_pylint::BINARY, "pylint");
    assert_eq!(
        lint_pylint::mandatory_flags(),
        vec![
            "--reports=no",
            "--msg-template={line}|{column}|{msg_id}|{symbol}|{msg}",
        ]
    );
    assert_eq!(lint_pep8::BINARY, "pep8");
    assert!(lint_pep8::mandatory_flags().is_empty());
    assert_eq!(lint_rubocop::BINARY, "rubocop");
    assert_eq!(lint_rubocop::mandatory_flags(), vec!["--format=json"]);
    assert_eq!(lint_ruby::BINARY, "ruby");
    assert_eq!(lint_ruby::mandatory_flags(), vec!["-w", "-c"]);
}

#[test]
fn python_linters_skip_non_python_files() {
    let linters: Vec<Box<dyn Linter>> = vec![
        Box::new(lint_flake8::Flake8Linter::new()),
        Box::new(lint_pyflakes::PyFlakesLinter::new()),
        Box::new(lint_pylint::PyLintLinter::new()),
        Box::new(lint_pep8::Pep8Linter::new()),
    ];
    for linter in &linters {
        for path in ["a.txt", "a.rb", "a.pyc", "a.pyx", "Rakefile", "noext"] {
            assert!(
                linter.lint_bytes(path, b"bad   \n").is_empty(),
                "for {} on {path}",
                linter.name()
            );
        }
        // Binary content is skipped without consulting the tool.
        assert!(
            linter.lint_bytes("a.py", b"a\0b\n").is_empty(),
            "binary for {}",
            linter.name()
        );
    }
}

#[test]
fn ruby_linters_skip_non_ruby_files() {
    let linters: Vec<Box<dyn Linter>> = vec![
        Box::new(lint_rubocop::RuboCopLinter::new()),
        Box::new(lint_ruby::RubyLinter::new()),
    ];
    for linter in &linters {
        for path in ["a.txt", "a.py", "a.rbx", "Gemfile.lock", "noext"] {
            assert!(
                linter.lint_bytes(path, b"bad   \n").is_empty(),
                "for {} on {path}",
                linter.name()
            );
        }
        assert!(
            linter.lint_bytes("a.rb", b"a\0b\n").is_empty(),
            "binary for {}",
            linter.name()
        );
    }
}

#[test]
fn engine_runs_the_new_linters() {
    let mut engine = lint::Engine::empty();
    engine.add_linter(Box::new(lint_flake8::Flake8Linter::new()));
    engine.add_linter(Box::new(lint_pyflakes::PyFlakesLinter::new()));
    engine.add_linter(Box::new(lint_pylint::PyLintLinter::new()));
    engine.add_linter(Box::new(lint_pep8::Pep8Linter::new()));
    engine.add_linter(Box::new(lint_rubocop::RuboCopLinter::new()));
    engine.add_linter(Box::new(lint_ruby::RubyLinter::new()));
    // No linter claims plain text, so the engine stays silent here
    // without needing any external tool.
    assert!(engine.lint_bytes("notes.txt", b"bad   \n").is_empty());
}

#[test]
fn flake8_fixture_parses() {
    let messages = lint_flake8::parse_output("src/main.py", &fixture("flake8.txt"));
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .map(|m| m.severity.as_str())
            .collect::<Vec<_>>(),
        vec!["error", "warning", "advice"]
    );
    assert!(messages.iter().all(|m| m.path == "src/main.py"));
}

#[test]
fn pyflakes_fixture_parses() {
    let messages = lint_pyflakes::parse_output("pkg/mod.py", &fixture("pyflakes.txt"));
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[1].severity, "warning");
    assert_eq!(messages[2].severity, "error");
    assert!(messages.iter().all(|m| m.code == "PYFLAKES"));
}

#[test]
fn pylint_fixture_parses() {
    let messages = lint_pylint::parse_output("pkg/mod.py", 1, &fixture("pylint.txt"));
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].name, "Missing Module Docstring");
    assert_eq!(messages[1].name, "Undefined Variable");
    assert_eq!(messages[2].name, "Unused Import");
    // Usage-error exit status stays silent instead of parsing.
    assert!(lint_pylint::parse_output("pkg/mod.py", 32, &fixture("pylint.txt")).is_empty());
}

#[test]
fn pep8_fixture_parses() {
    let messages = lint_pep8::parse_output("pkg/mod.py", &fixture("pep8.txt"));
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "E225");
    assert_eq!(messages[0].severity, "error");
    assert_eq!(messages[1].code, "W291");
    assert_eq!(messages[1].severity, "warning");
}

#[test]
fn rubocop_fixture_parses() {
    let messages = lint_rubocop::parse_output("lib/app.rb", &fixture("rubocop.json"));
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].code, "Style/StringLiterals");
    assert_eq!(messages[0].severity, "warning");
    assert_eq!(messages[1].code, "Lint/Syntax");
    assert_eq!(messages[1].severity, "error");
    // The linted path wins over the tool-echoed scratch path.
    assert!(messages.iter().all(|m| m.path == "lib/app.rb"));
}

#[test]
fn ruby_fixture_parses() {
    let messages = lint_ruby::parse_output("lib/app.rb", &fixture("ruby.txt"));
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|m| m.code == "RUBY"));
    assert!(messages.iter().all(|m| m.name == "Syntax Error"));
    assert!(messages.iter().all(|m| m.severity == "error"));
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
}
