use super::super::xhp_ast as ast;

fn check(source: &str) -> Vec<(u32, usize, usize, String)> {
    check_with_options(source, &ast::XhpOptions::default())
}

fn check_with_options(source: &str, options: &ast::XhpOptions) -> Vec<(u32, usize, usize, String)> {
    let canned = ast::tokens_to_canned(source, &ast::tokenize(source));
    let tree = ast::tree_from_canned(&canned).expect("canned output must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options,
    };
    let mut out = Vec::new();
    super::apply(&ctx, &mut out);
    out.iter()
        .map(|f| {
            let (line, col) = tree.line_col(f.offset);
            (f.rule, line, col, f.message.clone())
        })
        .collect()
}

fn for_rule(source: &str, rule: u32) -> Vec<(usize, usize, String)> {
    check(source)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

#[test]
fn variable_variable_flags_dynamic_names() {
    let hits = for_rule("<?php echo $$x;\n", 3);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Rewrite this code to use an array. Variable variables are unclear and hinder \
         static analysis."
    );
    assert_eq!(for_rule("<?php echo ${$x};\n", 3).len(), 1);
    assert!(for_rule("<?php echo $x;\n", 3).is_empty());
}

#[test]
fn global_variable_flags_each_statement() {
    assert_eq!(for_rule("<?php function f() { global $x; }\n", 79).len(), 1);
    assert_eq!(
        for_rule("<?php function f() { global $x, $y; }\n", 79).len(),
        1
    );
    assert!(for_rule("<?php $x = 1;\n", 79).is_empty());
}

#[test]
fn nested_namespaces_flag_inner() {
    let hits = for_rule("<?php namespace A { namespace B {} }\n", 90);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "`namespace` declarations cannot be nested. This construct will cause a PHP fatal \
         error."
    );
    assert!(for_rule("<?php namespace A;\n", 90).is_empty());
}

#[test]
fn use_prefix_flags_leading_backslash() {
    assert_eq!(for_rule("<?php use \\Foo\\Bar;\n", 97).len(), 1);
    assert_eq!(
        for_rule("<?php use \\Foo\\Bar;\n", 97)[0].2,
        "Imported symbols should not be prefixed with `\\`."
    );
    assert!(for_rule("<?php use Foo\\Bar;\n", 97).is_empty());
    assert!(for_rule("<?php class C { use T; }\n", 97).is_empty());
    assert!(for_rule("<?php $f = function () use ($x) {};\n", 97).is_empty());
}

#[test]
fn namespace_first_requires_namespace_statement() {
    assert!(for_rule("<?php namespace A;\n", 98).is_empty());
    assert!(for_rule("<?php echo 1;\n", 98).is_empty());
    let hits = for_rule("<?php echo 1;\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_ECHO_LIST"), "{}", hits[0].2);
    let hits = for_rule("<?php 1;\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_NUMERIC_SCALAR"), "{}", hits[0].2);
    let hits = for_rule("<?php $x = 1;\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_BINARY_EXPRESSION"), "{}", hits[0].2);
}

#[test]
fn symbol_alias_flags_identical_alias() {
    let hits = for_rule("<?php use Foo\\Bar as Bar;\n", 99);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Importing `Foo\\Bar` with `as Bar` is unnecessary because the aliased name is \
         identical to the imported symbol name."
    );
    assert!(for_rule("<?php use Foo\\Bar as Baz;\n", 99).is_empty());
    assert!(for_rule("<?php use Foo\\Bar;\n", 99).is_empty());
    assert!(for_rule("<?php use Foo\\Bar as bar;\n", 99).is_empty());
}

#[test]
fn hand_written_canned_json_parses() {
    let json = r#"{"source": "<?php echo $$x;",
      "tokens": [["T_OPEN_TAG", "<?php", 0], ["T_WHITESPACE", " ", 5],
        ["T_ECHO", "echo", 6], ["T_WHITESPACE", " ", 10], ["$", "$", 11],
        ["T_VARIABLE", "$x", 12], [";", ";", 14]]}"#;
    let tree = ast::tree_from_canned(json).expect("hand-written JSON must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options: &ast::XhpOptions::default(),
    };
    let mut out = Vec::new();
    super::apply(&ctx, &mut out);
    assert!(out.iter().any(|f| f.rule == 3));
}

// -( T1170 COV5: close xhp_flow.rs )--------------------------------------------

fn check_canned(json: &str) -> Vec<(u32, usize, usize, String)> {
    let tree = ast::tree_from_canned(json).expect("hand-written JSON must parse");
    let options = ast::XhpOptions::default();
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options: &options,
    };
    let mut out = Vec::new();
    super::apply(&ctx, &mut out);
    out.iter()
        .map(|f| {
            let (line, col) = tree.line_col(f.offset);
            (f.rule, line, col, f.message.clone())
        })
        .collect()
}

#[test]
fn leading_use_without_predecessor_is_an_import() {
    // `use` as the very first token has no previous semantic token.
    let hits = check_canned(
        r#"{"source": "use \\Foo;",
          "tokens": [["T_USE", "use", 0], ["T_WHITESPACE", " ", 3],
            ["\\", "\\", 4], ["T_STRING", "Foo", 5], [";", ";", 8]]}"#,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 97);
    assert_eq!(
        hits[0].3,
        "Imported symbols should not be prefixed with `\\`."
    );
}

#[test]
fn empty_valued_symbol_is_skipped() {
    // A semantic token with an empty value builds an empty name.
    let hits = check_canned(
        r#"{"source": "use X;",
          "tokens": [["T_USE", "use", 0], ["T_WHITESPACE", " ", 3],
            ["T_STRING", "", 4], [";", ";", 5]]}"#,
    );
    assert!(hits.is_empty());
}

#[test]
fn truncated_import_statements_yield_no_symbols() {
    // A bare closer ends the scan with no statement end.
    assert!(for_rule("<?php use ];\n", 97).is_empty());
    assert!(for_rule("<?php use ];\n", 99).is_empty());
    // No `;` at all: no statement end either.
    assert!(for_rule("<?php use Foo\n", 97).is_empty());
    assert!(for_rule("<?php use Foo\n", 99).is_empty());
}

#[test]
fn function_and_const_imports_skip_the_modifier() {
    assert!(for_rule("<?php use function foo\\bar;\n", 97).is_empty());
    assert_eq!(for_rule("<?php use function \\foo\\bar;\n", 97).len(), 1);
    assert!(for_rule("<?php use const X\\Y;\n", 99).is_empty());
}

#[test]
fn group_imports_cover_each_member() {
    let hits = for_rule("<?php use Foo\\{Bar, \\Baz, Qux as Qux};\n", 97);
    assert_eq!(hits.len(), 1);
    let hits = for_rule("<?php use Foo\\{Bar, \\Baz, Qux as Qux};\n", 99);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.contains("Qux"), "{}", hits[0].2);
    assert!(for_rule("<?php use Foo\\{Bar, Baz as B};\n", 99).is_empty());
}

#[test]
fn mismatched_group_close_abandons_the_import() {
    assert!(for_rule("<?php use Foo\\{Bar);\n", 97).is_empty());
    assert!(for_rule("<?php use Foo\\{Bar);\n", 99).is_empty());
}

#[test]
fn parenthesized_and_multi_imports_split() {
    // Bracketed segments still scan; commas split flat imports.
    assert!(for_rule("<?php use Foo($x);\n", 97).is_empty());
    let hits = for_rule("<?php use A\\X, \\B\\Y ;\n", 97);
    assert_eq!(hits.len(), 1);
    // An empty segment between commas is skipped.
    assert!(for_rule("<?php use Foo,, Bar;\n", 97).is_empty());
}

#[test]
fn missing_alias_is_not_an_alias() {
    assert!(for_rule("<?php use Foo as;\n", 99).is_empty());
}

#[test]
fn namespace_first_names_string_statements() {
    let hits = for_rule("<?php foo;\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_SYMBOL_NAME"), "{}", hits[0].2);
    let hits = for_rule("<?php foo = 1;\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_BINARY_EXPRESSION"), "{}", hits[0].2);
    let hits = for_rule("<?php foo();\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_FUNCTION_CALL"), "{}", hits[0].2);
    let hits = for_rule("<?php Foo::bar();\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    let message = &hits[0].2;
    assert!(message.ends_with("n_CLASS_STATIC_ACCESS"), "{message}");
    let hits = for_rule("<?php foo:\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_LABEL"), "{}", hits[0].2);
}

#[test]
fn namespace_first_names_plain_variables() {
    let hits = for_rule("<?php $x;\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_VARIABLE"), "{}", hits[0].2);
}

#[test]
fn namespace_first_binary_scan_handles_brackets() {
    // A depth-zero closer ends the scan without an operator.
    let hits = for_rule("<?php $x);\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_VARIABLE"), "{}", hits[0].2);
    // Balanced brackets scan through.
    let hits = for_rule("<?php $x == [1];\nnamespace A;\n", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_VARIABLE"), "{}", hits[0].2);
    // No `;` at all: the scan runs off the end.
    let hits = for_rule("<?php $x namespace A", 98);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].2.ends_with("n_VARIABLE"), "{}", hits[0].2);
}
