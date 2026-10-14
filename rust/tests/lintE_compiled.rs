//! Compiled-language external-tool linters (bundle E, T1128 round 3)
//! through the wired library.

use arc_rs::{
    lint::Linter, lint_cppcheck, lint_cpplint, lint_csharp, lint_golint, lint_hlint, lint_php,
    lint_phpcs, lint_puppetlint, lint_xhpast,
};

fn linters() -> Vec<Box<dyn Linter>> {
    vec![
        Box::new(lint_cppcheck::CppcheckLinter::new()),
        Box::new(lint_cpplint::CpplintLinter::new()),
        Box::new(lint_golint::GolintLinter::new()),
        Box::new(lint_hlint::HlintLinter::new()),
        Box::new(lint_csharp::CsharpLinter::new()),
        Box::new(lint_phpcs::PhpcsLinter::new()),
        Box::new(lint_php::PhpLinter::new()),
        Box::new(lint_puppetlint::PuppetlintLinter::new()),
        Box::new(lint_xhpast::XhpastLinter::new()),
    ]
}

#[test]
fn linter_names_match_php_exactly() {
    let names: Vec<&str> = linters().iter().map(|linter| linter.name()).collect();
    assert_eq!(
        names,
        vec![
            "cppcheck",
            "C++ Google's Styleguide",
            "GOLINT",
            "HLINT",
            "C#",
            "PHPCS",
            "PHP",
            "PUPPETLINT",
            "XHP",
        ]
    );
}

#[test]
fn all_linters_skip_foreign_files_fast() {
    for linter in linters() {
        assert!(
            linter
                .lint_bytes("notes.txt", b"int main() {}\n")
                .is_empty(),
            "{} must skip .txt",
            linter.name()
        );
    }
    // Spot-check native-vs-foreign routing without needing any tool.
    assert!(
        lint_golint::GolintLinter::new()
            .lint_bytes("a.cpp", b"package main\n")
            .is_empty()
    );
    assert!(
        lint_cppcheck::CppcheckLinter::new()
            .lint_bytes("a.go", b"int x;\n")
            .is_empty()
    );
    assert!(
        lint_php::PhpLinter::new()
            .lint_bytes("a.pp", b"<?php echo 1;\n")
            .is_empty()
    );
}

#[test]
fn all_linters_skip_binary_content() {
    let cases = ["a.c", "a.cpp", "a.go", "a.hs", "a.cs", "a.php", "a.pp"];
    for linter in linters() {
        for path in cases {
            assert!(
                linter.lint_bytes(path, b"binary \0 content\n").is_empty(),
                "{} must skip binaries",
                linter.name()
            );
        }
    }
}

#[test]
fn absent_tools_report_nothing() {
    assert!(lint_cppcheck::lint_with_binary("a.c", b"int x;\n", None).is_empty());
    assert!(lint_cpplint::lint_with_binary("a.cpp", b"int x;\n", None).is_empty());
    assert!(lint_golint::lint_with_binary("a.go", b"package main\n", None).is_empty());
    assert!(lint_hlint::lint_with_binary("a.hs", b"f x = x\n", None).is_empty());
    assert!(lint_csharp::lint_with_command("a.cs", b"class A {}\n", None).is_empty());
    assert!(lint_phpcs::lint_with_binary("a.php", b"<?php echo 1;\n", None).is_empty());
    assert!(lint_php::lint_with_binary("a.php", b"<?php echo 1;\n", None).is_empty());
    assert!(lint_puppetlint::lint_with_binary("a.pp", b"file { 'x': }\n", None).is_empty());
    assert!(lint_xhpast::lint_with_binary("a.php", b"<?php echo 1;\n", None).is_empty());
}

#[test]
fn tool_argv_matches_php_composition() {
    assert_eq!(
        lint_cppcheck::command_flags().join(" "),
        "--quiet --inline-suppr --xml --xml-version=2 -j2 \
         --enable=performance,style,portability,information"
    );
    assert!(lint_cpplint::command_flags().is_empty());
    assert!(lint_golint::command_flags().is_empty());
    assert_eq!(lint_hlint::command_flags(), vec!["--json".to_string()]);
    assert_eq!(
        lint_csharp::command_flags(),
        vec!["--settings-base64=bnVsbA==".to_string(), "-r=.".to_string(),]
    );
    assert_eq!(
        lint_phpcs::command_flags(),
        vec!["--report=xml".to_string()]
    );
    assert_eq!(lint_php::command_flags(), vec!["-l".to_string()]);
    assert_eq!(
        lint_puppetlint::command_flags(),
        vec![
            "--error-level=all".to_string(),
            "--log-format=%{linenumber}|%{column}|%{kind}|%{check}|%{message}".to_string(),
        ]
    );
    assert!(lint_xhpast::command_flags().is_empty());
}

#[test]
fn parsers_turn_canned_tool_output_into_messages() {
    let cppcheck = r#"<results><errors><error id="unusedVar" severity="style" msg="Unused variable: x"><location file="t" line="2"/></error></errors></results>"#;
    let found = lint_cppcheck::parse_output("a.c", 0, "", cppcheck);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].severity, "warning");

    let cpplint = "a.cc:5:  Missing spaces around =  [whitespace/operators] [4]\n";
    let found = lint_cpplint::parse_output("a.cc", 0, "", cpplint);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "whitespace/operators");

    let golint = "/tmp/a.go:3:1: exported function Foo should have comment\n";
    let found = lint_golint::parse_output("a.go", 0, golint, "");
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].line, found[0].col), (3, 1));

    let hlint = r#"[{"module":"M","decl":"f","severity":"Warning","hint":"Use null","startLine":1,"startColumn":8,"note":[]}]"#;
    let found = lint_hlint::parse_output("a.hs", 0, hlint, "");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Use null");

    let csharp = r#"[{"FileName":"F.cs","Issues":[{"LineNumber":1,"Column":1,"Index":{"Code":"C","Name":"N","Message":"hi %s","Severity":3},"Parameters":["there"]}]}]"#;
    let found = lint_csharp::parse_output("a.cs", 0, csharp, "");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].description, "hi there");

    let phpcs = "<phpcs version=\"3\"><file name=\"f\"><error line=\"1\" column=\"2\" source=\"S.C\">msg</error></file></phpcs>";
    let found = lint_phpcs::parse_output("a.php", 1, phpcs, "");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "PHPCS.E.S.C");

    let php = "PHP Parse error:  oops in /tmp/a.php on line 2\n";
    let found = lint_php::parse_output("a.php", 255, "", php);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code, "PHP1");

    let puppet = "3|13|warning|unquoted_file_mode|mode should be quoted\n";
    let found = lint_puppetlint::parse_output("a.pp", 0, puppet, "");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "Unquoted File Mode");

    assert!(lint_xhpast::parse_output("a.php", 0, "", "").is_empty());
    assert_eq!(lint_xhpast::parse_output("a.php", 1, "", "").len(), 1);
}

/// Whatever the environment provides (tools present or absent), native
/// files yield only well-formed messages: the display path echoed back,
/// 1-based positions, and known severities.
#[test]
fn native_fixtures_yield_only_well_formed_messages() {
    for linter in linters() {
        let ext = match linter.name() {
            "cppcheck" => "c",
            "C++ Google's Styleguide" => "cpp",
            "GOLINT" => "go",
            "HLINT" => "hs",
            "C#" => "cs",
            "PHPCS" | "PHP" | "XHP" => "php",
            "PUPPETLINT" => "pp",
            other => panic!("no fixture for {other}"),
        };
        let display = format!("sample.{ext}");
        let data = std::fs::read(format!("tests/fixtures/lintE/sample.{ext}")).unwrap();
        for message in linter.lint_bytes(&display, &data) {
            assert_eq!(message.path, display, "{}", linter.name());
            assert!(message.line >= 1, "{}", linter.name());
            assert!(message.col >= 1, "{}", linter.name());
            assert!(
                ["error", "warning", "advice", "autofix", "disabled"]
                    .contains(&message.severity.as_str()),
                "{}: {}",
                linter.name(),
                message.severity
            );
        }
    }
}

#[test]
fn fixtures_are_skipped_by_foreign_linters() {
    let php = std::fs::read("tests/fixtures/lintE/sample.php").unwrap();
    assert!(
        lint_golint::GolintLinter::new()
            .lint_bytes("sample.php", &php)
            .is_empty()
    );
    assert!(
        lint_hlint::HlintLinter::new()
            .lint_bytes("sample.php", &php)
            .is_empty()
    );
    let go = std::fs::read("tests/fixtures/lintE/sample.go").unwrap();
    assert!(
        lint_php::PhpLinter::new()
            .lint_bytes("sample.go", &go)
            .is_empty()
    );
    assert!(
        lint_puppetlint::PuppetlintLinter::new()
            .lint_bytes("sample.go", &go)
            .is_empty()
    );
}
