//! Public-behavior tests for bundle D: the Python/Ruby external-tool
//! linters (`flake8`, `PYFLAKES`, `PyLint`, `PEP8`, `RuboCop`, `RUBY`).
//!
//! Parsing is exercised on canned output, `lint_bytes` on inputs every
//! linter must skip without a tool (wrong extension, NUL bytes), and the
//! tool-execution path on hermetic fake binaries (never the real tools).

use std::path::{Path, PathBuf};

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

/// Hermetic fake external tool: a script printing canned bytes from a
/// sibling file (no shell quoting involved). Removed on drop.
struct FakeTool {
    dir: PathBuf,
    tool: PathBuf,
}

impl FakeTool {
    /// Fake named `stem` printing `output` (to stderr when asked).
    fn with_output(tag: &str, stem: &str, output: &str, to_stderr: bool) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("arc-rs-lintD-{tag}-{n}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("out.txt"), output).unwrap();
        #[cfg(windows)]
        let tool = {
            let redirect = if to_stderr { " 1>&2" } else { "" };
            let bat = dir.join(format!("{stem}.bat"));
            std::fs::write(
                &bat,
                format!("@echo off\ntype \"%~dp0out.txt\"{redirect}\nexit /b 1\n"),
            )
            .unwrap();
            bat
        };
        #[cfg(not(windows))]
        let tool = {
            let redirect = if to_stderr { " >&2" } else { "" };
            let sh = dir.join(format!("{stem}.sh"));
            std::fs::write(
                &sh,
                format!("#!/bin/sh\ncat \"$(dirname \"$0\")/out.txt\"{redirect}\nexit 1\n"),
            )
            .unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&sh).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&sh, perms).unwrap();
            }
            sh
        };
        Self { dir, tool }
    }
}

impl Drop for FakeTool {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn lint_with_binary_none_or_missing_is_silent() {
    let missing = std::env::temp_dir().join("arc-rs-lintD-definitely-missing");
    assert!(lint_flake8::lint_with_binary("a.py", b"x\n", None).is_empty());
    assert!(lint_pep8::lint_with_binary("a.py", b"x\n", None).is_empty());
    assert!(lint_pyflakes::lint_with_binary("a.py", b"x\n", None).is_empty());
    assert!(lint_pylint::lint_with_binary("a.py", b"x\n", None).is_empty());
    assert!(lint_rubocop::lint_with_binary("a.rb", b"x\n", None).is_empty());
    assert!(lint_ruby::lint_with_binary("a.rb", b"x\n", None).is_empty());
    assert!(lint_flake8::lint_with_binary("a.py", b"x\n", Some(&missing)).is_empty());
    assert!(lint_pep8::lint_with_binary("a.py", b"x\n", Some(&missing)).is_empty());
    assert!(lint_pyflakes::lint_with_binary("a.py", b"x\n", Some(&missing)).is_empty());
    assert!(lint_pylint::lint_with_binary("a.py", b"x\n", Some(&missing)).is_empty());
    assert!(lint_rubocop::lint_with_binary("a.rb", b"x\n", Some(&missing)).is_empty());
    assert!(lint_ruby::lint_with_binary("a.rb", b"x\n", Some(&missing)).is_empty());
}

#[test]
fn flake8_runs_fake_tool_end_to_end() {
    let fake = FakeTool::with_output(
        "flake8",
        "flake8",
        "target.py:3:1: E302 expected 2 blank lines\n",
        false,
    );
    let messages = lint_flake8::lint_with_binary("src/main.py", b"x = 1\n", Some(&fake.tool));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "src/main.py");
    assert_eq!((messages[0].line, messages[0].col), (3, 1));
    assert_eq!(messages[0].code, "E302");
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn pep8_runs_fake_tool_end_to_end() {
    let fake = FakeTool::with_output(
        "pep8",
        "pep8",
        "target.py:2:5: E225 missing whitespace\n",
        false,
    );
    let messages = lint_pep8::lint_with_binary("pkg/mod.py", b"x = 1\n", Some(&fake.tool));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (2, 5));
    assert_eq!(messages[0].code, "E225");
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn pyflakes_runs_fake_tool_end_to_end() {
    let fake = FakeTool::with_output(
        "pyflakes",
        "pyflakes",
        "target.py:1:1 undefined name foo\n",
        false,
    );
    let messages = lint_pyflakes::lint_with_binary("pkg/mod.py", b"x = 1\n", Some(&fake.tool));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (1, 1));
    assert_eq!(messages[0].code, "PYFLAKES");
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn pylint_runs_fake_tool_end_to_end() {
    let fake = FakeTool::with_output(
        "pylint",
        "pylint",
        "2|4|E0602|undefined-variable|Undefined variable\n",
        false,
    );
    let messages = lint_pylint::lint_with_binary("pkg/mod.py", b"x = 1\n", Some(&fake.tool));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "pkg/mod.py");
    assert_eq!((messages[0].line, messages[0].col), (2, 4));
    assert_eq!(messages[0].code, "E0602");
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn rubocop_runs_fake_tool_end_to_end() {
    let fake = FakeTool::with_output(
        "rubocop",
        "rubocop",
        r#"{"files":[{"path":"x","offenses":[{"severity":"error","message":"Syntax error.","cop_name":"Lint/Syntax","location":{"line":9,"column":1,"length":1}}]}]}"#,
        false,
    );
    let messages = lint_rubocop::lint_with_binary("lib/app.rb", b"puts 1\n", Some(&fake.tool));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "lib/app.rb");
    assert_eq!((messages[0].line, messages[0].col), (9, 1));
    assert_eq!(messages[0].code, "Lint/Syntax");
    assert_eq!(messages[0].severity, "error");
}

#[test]
fn ruby_runs_fake_tool_end_to_end() {
    let fake = FakeTool::with_output(
        "ruby",
        "ruby",
        "target.rb:2: syntax error, unexpected end\n",
        true,
    );
    let messages = lint_ruby::lint_with_binary("lib/app.rb", b"puts 1\n", Some(&fake.tool));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].path, "lib/app.rb");
    assert_eq!((messages[0].line, messages[0].col), (2, 1));
    assert_eq!(messages[0].code, "RUBY");
    assert_eq!(messages[0].severity, "error");
}
