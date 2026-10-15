use super::super::xhp_ast as ast;

fn check(source: &str) -> Vec<(u32, usize, usize, String)> {
    check_with_options(source, &ast::XhpOptions::default())
}

fn check_with_options(
    source: &str,
    options: &ast::XhpOptions,
) -> Vec<(u32, usize, usize, String)> {
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
