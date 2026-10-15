use super::super::xhp_ast as ast;

fn check_with(
    source: &str,
    path: &str,
    options: &ast::XhpOptions,
) -> Vec<(u32, usize, usize, String)> {
    let canned = ast::tokens_to_canned(source, &ast::tokenize(source));
    let tree = ast::tree_from_canned(&canned).expect("canned output must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path,
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

fn check(source: &str) -> Vec<(u32, usize, usize, String)> {
    check_with(source, "test.php", &ast::XhpOptions::default())
}

fn for_rule(source: &str, rule: u32) -> Vec<(usize, usize, String)> {
    check(source)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

fn for_rule_with(
    source: &str,
    path: &str,
    options: &ast::XhpOptions,
    rule: u32,
) -> Vec<(usize, usize, String)> {
    check_with(source, path, options)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

fn versioned(version: &str) -> ast::XhpOptions {
    ast::XhpOptions {
        php_version: Some(version.to_string()),
        ..ast::XhpOptions::default()
    }
}

#[test]
fn undeclared_flags_first_bad_use_only() {
    let hits = for_rule(
        "<?php function f($a) { $b = 1; echo $a; echo $b; echo $c; }\n",
        5,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 1);
    assert_eq!(
        hits[0].2,
        "Declare variables prior to use (even if you are passing them \
         as reference parameters). You may have misspelled this variable name."
    );
    assert!(for_rule("<?php function f($a) { echo $a; }\n", 5).is_empty());
}

#[test]
fn undeclared_isset_extract_and_foreach() {
    assert_eq!(
        for_rule("<?php function f() { isset($x); $x++; }\n", 5).len(),
        1
    );
    assert!(for_rule("<?php function f() { isset($x); }\n", 5).is_empty());
    assert!(for_rule("<?php function f($a) { extract($a); $x++; }\n", 5).is_empty());
    assert!(
        for_rule(
            "<?php function f($a) { foreach ($a as $k => $v) {} $v++; }\n",
            5
        )
        .is_empty()
    );
    assert!(for_rule("<?php function f() { Foo::$x; }\n", 5).is_empty());
    assert_eq!(for_rule("<?php function f() { $y++; $y++; }\n", 5).len(), 1);
}

#[test]
fn undeclared_closures_and_strings() {
    let hits = for_rule(
        "<?php function f($x, $y) { $g = function ($z) use ($x) { echo $y; }; }\n",
        5,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 1);
    let hits = for_rule(
        "<?php function f() { $a = 1; echo \"$a\"; echo \"$b\"; }\n",
        5,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Declare variables prior to use (even if you are passing them \
         as reference parameters). You may have misspelled this variable name."
    );
}

#[test]
fn naming_declarations_and_members() {
    let hits = for_rule("<?php final class a {}\n", 9);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Follow naming conventions: classes should be named using `UpperCamelCase`."
    );
    assert!(for_rule("<?php final class Quack {}\n", 9).is_empty());
    let hits = for_rule("<?php interface i {}\n", 9);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Follow naming conventions: interfaces should be named using `UpperCamelCase`."
    );
    let hits = for_rule("<?php function YY($ZZ) {}\n", 9);
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].2,
        "Follow naming conventions: functions should be named using \
         `lowercase_with_underscores`."
    );
    assert_eq!(
        hits[1].2,
        "Follow naming conventions: parameters should be named using \
         `lowercase_with_underscores`"
    );
    let hits = for_rule(
        "<?php class C { public function F($G) {} protected $E; }\n",
        9,
    );
    assert_eq!(hits.len(), 3);
    assert_eq!(
        hits[0].2,
        "Follow naming conventions: methods should be named using `lowerCamelCase`."
    );
    assert_eq!(
        hits[2].2,
        "Follow naming conventions: class properties should be named using `lowerCamelCase`."
    );
}

#[test]
fn naming_class_const_singularity_quirk() {
    let hits = for_rule("<?php class Foo { const bar = 'baz'; }\n", 9);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Constants should be uppercase.");
    let hits = for_rule("<?php class C { const b = 1, c = 2; }\n", 9);
    assert_eq!(hits.len(), 4);
    assert_eq!(
        hits[0].2,
        "Follow naming conventions: class constants should be named using \
         `UPPERCASE_WITH_UNDERSCORES`"
    );
    assert_eq!(
        hits[1].2,
        "Follow naming conventions: class constants should be named using \
         `UPPERCASE_WITH_UNDERSCORES`"
    );
    assert_eq!(hits[2].2, "Constants should be uppercase.");
    assert_eq!(hits[3].2, "Constants should be uppercase.");
    assert!(for_rule("<?php class C { const R = 1, S = 2; }\n", 9).is_empty());
    let hits = for_rule("<?php define('foo', 'bar');\nconst bar = 'baz';\n", 9);
    assert_eq!(hits.len(), 2);
    assert!(for_rule("<?php define('FOO', 'bar');\n", 9).is_empty());
}

#[test]
fn naming_variables_globals_and_statics() {
    assert_eq!(
        for_rule("<?php function g() { $lowerCamelCase = 0; }\n", 9).len(),
        1
    );
    assert!(
        for_rule(
            "<?php function g() { $lowercase_with_underscores = 0; }\n",
            9
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php function h() { global $mIxEd_CaSe; $mIxEd_CaSe = 2; }\n",
            9
        )
        .is_empty()
    );
    assert!(for_rule("<?php function h() { $_ENV[0] = 1; }\n", 9).is_empty());
    assert!(for_rule("<?php function i() { self::$X_x[0]; }\n", 9).is_empty());
}

#[test]
fn implicit_constructor_matches_fixture() {
    let src = "<?php final class Platypus {\n  public function platypus() {}\n}\n";
    let hits = for_rule(src, 10);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 19));
    assert_eq!(
        hits[0].2,
        "Name constructors `__construct` explicitly. This method is a \
         constructor because it has the same name as the class it is defined in."
    );
    assert!(for_rule("<?php class A { public function __construct() {} }\n", 10).is_empty());
    assert!(for_rule("<?php class A { public function b() {} }\n", 10).is_empty());
}

#[test]
fn static_this_matches_fixture() {
    let src = "<?php final class A {\n  public function u() {\n    $this->f();\n  }\n  public static function v() {\n    $this->f();\n  }\n}\n";
    let hits = for_rule(src, 13);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (6, 5));
    assert_eq!(
        hits[0].2,
        "You can not reference `$this` inside a static method."
    );
    assert!(
        for_rule(
            "<?php class A { abstract public static function v(); }\n",
            13
        )
        .is_empty()
    );
}

#[test]
fn exit_expression_matches_fixture() {
    assert!(for_rule("<?php exit(-1);\n", 17).is_empty());
    let hits = for_rule("<?php exit -1;\n", 17);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 7));
    assert_eq!(hits[0].2, "Use `exit` as a statement, not an expression.");
    let hits = for_rule("<?php strtoupper(33 * exit - 6);\n", 17);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 23));
}

#[test]
fn exit_expression_statement_forms_ok() {
    assert!(for_rule("<?php exit;\n", 17).is_empty());
    assert!(for_rule("<?php die;\n", 17).is_empty());
    assert!(for_rule("<?php die -1;\n", 17).is_empty());
    assert!(for_rule("<?php if ($x) exit;\n", 17).is_empty());
    assert!(for_rule("<?php switch ($x) { case 1: exit; }\n", 17).is_empty());
    assert_eq!(for_rule("<?php $x = exit;\n", 17).len(), 1);
    assert_eq!(for_rule("<?php foo(exit);\n", 17).len(), 1);
}

#[test]
fn class_filename_mismatch_checks_lone_declaration() {
    let options = ast::XhpOptions::default();
    let hits = for_rule_with("<?php class Foo {}\n", "src/Bar.php", &options, 19);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "The name of this file differs from the name of the class or interface \
         it declares. Rename the file to `Foo.php`."
    );
    assert!(for_rule_with("<?php class Foo {}\n", "src/Foo.php", &options, 19).is_empty());
    assert!(for_rule_with("<?php class A {}\nclass B {}\n", "src/A.php", &options, 19).is_empty());
    assert!(for_rule_with("<?php interface I {}\n", "src/I.php", &options, 19).is_empty());
}

#[test]
fn tautological_matches_fixture() {
    let hits = for_rule("<?php if ($x == $x) {}\n", 20);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 11));
    assert_eq!(
        hits[0].2,
        "Both sides of this expression are identical, so it always evaluates to a constant."
    );
    assert_eq!(for_rule("<?php if ($x->m(3) < $x->m(3)) {}\n", 20).len(), 1);
    assert_eq!(for_rule("<?php if ($y[2] - $y[2]) {}\n", 20).len(), 1);
    assert!(for_rule("<?php if ($x == $y) {}\n", 20).is_empty());
    assert!(for_rule("<?php if ($x <> $x) {}\n", 20).is_empty());
    let hits = for_rule("<?php $skip = true || $x;\n", 20);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "The logical value of this expression is static. \
         Did you forget to remove some debugging code?"
    );
    assert_eq!(for_rule("<?php $skip = false && f();\n", 20).len(), 1);
    assert!(for_rule("<?php $skip = $a || $b;\n", 20).is_empty());
    assert!(for_rule("<?php $skip = $a and true;\n", 20).is_empty());
}

#[test]
fn duplicate_keys_matches_fixture() {
    let src = "<?php $a = array(\n  'a' => 'v1',\n  $a => 'v2',\n  'a' => val3(),\n  1 => val4(),\n  '1' => val5(),\n  1 => $val6,\n);\n";
    let hits = for_rule(src, 22);
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].0, hits[0].1), (4, 3));
    assert_eq!((hits[1].0, hits[1].1), (7, 3));
    assert_eq!(
        hits[0].2,
        "Duplicate key in array initializer. PHP will ignore all but the last entry."
    );
    assert_eq!(
        for_rule("<?php $d = array(A => 1, A => 2, 'A' => 3);\n", 22).len(),
        1
    );
    assert_eq!(
        for_rule("<?php $e = array(self::B => 1, self::B => 2);\n", 22).len(),
        1
    );
    assert!(for_rule("<?php $c = array('a' => 1, 'b' => 2);\n", 22).is_empty());
    assert!(for_rule("<?php $c = array();\n", 22).is_empty());
}

#[test]
fn reused_iterator_matches_fixture() {
    let src = "<?php for ($ii = 0; $ii < $n; $ii++) {\n  foreach ($list as $ii => $jj) {}\n}\n";
    let hits = for_rule(src, 23);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 11));
    assert!(
        hits[0]
            .2
            .starts_with("This loop reuses iterator variables ($ii)")
    );
    let src = "<?php for ($ii = 0; $ii < $n; $ii++) {\n  for ($ii = 0; $ii < $m; $ii++) {}\n}\n";
    assert_eq!(for_rule(src, 23).len(), 1);
    let src = "<?php for ($ii = 0; $ii < $n; $ii++) {\n  for ($jj = $ii; $jj < $m; $jj++) {}\n}\n";
    assert!(for_rule(src, 23).is_empty());
    assert!(for_rule("<?php foreach ($a as $x) {}\nforeach ($b as $x) {}\n", 23).is_empty());
}

#[test]
fn brace_formatting_matches_fixture() {
    let hits = for_rule("<?php if (1)\n{}\n", 24);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 13));
    assert_eq!(
        hits[0].2,
        "Put opening braces on the same line as control statements and \
         declarations, with a single space before them."
    );
    assert!(for_rule("<?php if (1) {}\n", 24).is_empty());
    let hits = for_rule("<?php if ($x) foo();\n", 24);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Use braces to surround a statement block.");
    assert!(for_rule("<?php if ($x) {} else if ($y) {} else {}\n", 24).is_empty());
    let hits = for_rule("<?php if ($x) {}else{}\n", 24);
    assert_eq!(hits.len(), 2);
    let hits = for_rule("<?php class X{}\n", 24);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 14));
}

#[test]
fn fallthrough_matches_fixture() {
    let src = "<?php switch ($x) {\n  case 1:\n    $x++;\n  case 2:\n    break;\n}\n";
    let hits = for_rule(src, 30);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 3));
    assert!(
        hits[0]
            .2
            .starts_with("This `case` or `default` has a nonempty block")
    );
    assert!(
        for_rule(
            "<?php switch ($x) {\n  case 1:\n  case 2:\n    break;\n}\n",
            30
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php switch ($x) {\n  case 1:\n    $x++;\n    /* fallthrough */\n}\n",
            30
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php switch ($x) {\n  case 1:\n    while (true) {\n      break;\n    }\n}\n",
            30
        )
        .len()
            == 1
    );
    assert!(
        for_rule(
            "<?php switch ($x) {\n  case 1:\n    while (true) {\n      return;\n    }\n}\n",
            30
        )
        .is_empty()
    );
    assert!(for_rule("<?php switch ($x) {\n  case 1:\n    exit(1);\n}\n", 30).is_empty());
}

#[test]
fn reused_as_iterator_matches_fixture() {
    assert!(
        for_rule(
            "<?php function f($s, $t) {\n  foreach ($s as $ii) {}\n  foreach ($s as $ii) {}\n}\n",
            32
        )
        .is_empty()
    );
    let src = "<?php function j($stuff, $thing) {\n  foreach ($stuff as $thing) {\n    break;\n  }\n  f($thing);\n}\n";
    let hits = for_rule(src, 32);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 22));
    assert_eq!(
        hits[0].2,
        "This iterator variable is a previously declared local variable. \
         To avoid overwriting locals, do not reuse them as iterator variables."
    );
    assert!(
        for_rule(
            "<?php function g($s, $t) {\n  foreach ($s as $t) {}\n}\n",
            32
        )
        .is_empty()
    );
}

#[test]
fn reused_iterator_reference_matches_fixture() {
    let src = "<?php function a() {\n  $ar = array();\n  foreach ($ar as &$a) {}\n  $a = 1;\n}\n";
    let hits = for_rule(src, 39);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 3));
    assert_eq!(
        hits[0].2,
        "This variable was used already as a by-reference iterator variable. \
         Such variables survive outside the `foreach` loop, do not reuse."
    );
    assert!(
        for_rule(
            "<?php function u() {\n  $ar = array();\n  foreach ($ar as &$a) {}\n  unset($a);\n  $a++;\n}\n",
            39
        )
        .is_empty()
    );
    assert_eq!(
        for_rule(
            "<?php function u2() {\n  $ar = array();\n  foreach ($ar as &$a) {}\n  $a++;\n  unset($a);\n}\n",
            39
        )
        .len(),
        1
    );
    assert!(
        for_rule(
            "<?php function k() {\n  foreach ($a as $k => $v) {}\n  $v++;\n}\n",
            39
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php function v() {\n  foreach ($a as &$$x) {}\n  $x++;\n}\n",
            39
        )
        .is_empty()
    );
}

#[test]
fn compat53_matches_fixture() {
    let options = versioned("5.2.3");
    let hits = for_rule_with("<?php namespace a;\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.2.3, but namespaces were not introduced until PHP 5.3."
    );
    assert_eq!(
        for_rule_with("<?php use b, c;\n", "a.php", &options, 45).len(),
        1
    );
    assert_eq!(
        for_rule_with("<?php f(function () {});\n", "a.php", &options, 45).len(),
        1
    );
    assert!(for_rule_with("<?php function f() {}\n", "a.php", &options, 45).is_empty());
    assert_eq!(
        for_rule_with("<?php static::m();\n", "a.php", &options, 45).len(),
        1
    );
    assert!(for_rule_with("<?php $a::m();\n", "a.php", &options, 45).is_empty());
    let hits = for_rule_with("<?php 1 ?: 2;\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 7));
    assert!(for_rule_with("<?php 1 ? 1 : 2;\n", "a.php", &options, 45).is_empty());
    assert_eq!(
        for_rule_with("<?php echo <<<'EOT'\nhi\nEOT;\n", "a.php", &options, 45).len(),
        1
    );
    assert!(
        for_rule_with(
            "<?php namespace a;\n",
            "a.php",
            &ast::XhpOptions::default(),
            45
        )
        .is_empty()
    );
}

#[test]
fn compat54_features_and_incompat() {
    let options = versioned("5.3.0");
    let hits = for_rule_with("<?php f()[0];\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 11));
    assert_eq!(
        hits[0].2,
        "The `f()[...]` syntax was not introduced until PHP 5.4, but this \
         codebase targets an earlier version of PHP. You can rewrite this \
         expression using `idx()`."
    );
    assert!(for_rule_with("<?php (f())[0];\n", "a.php", &options, 45).is_empty());
    assert_eq!(for_rule_with("<?php [];\n", "a.php", &options, 45).len(), 1);
    assert!(for_rule_with("<?php array(1);\n", "a.php", &options, 45).is_empty());
    assert_eq!(
        for_rule_with("<?php $x = 0b1;\n", "a.php", &options, 45).len(),
        1
    );
    let src = "<?php class C {\n  public function m() {\n    return function () {\n      $this->x();\n    };\n  }\n}\n";
    assert_eq!(for_rule_with(src, "a.php", &options, 45).len(), 1);
    assert!(for_rule_with("<?php break;\n", "a.php", &options, 45).is_empty());

    let modern = versioned("5.4.0");
    assert!(for_rule_with("<?php f()[0];\n", "a.php", &modern, 45).is_empty());
    assert!(for_rule_with("<?php break;\n", "a.php", &modern, 45).is_empty());
    assert!(for_rule_with("<?php break 1;\n", "a.php", &modern, 45).is_empty());
    let hits = for_rule_with("<?php break 0;\n", "a.php", &modern, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 13));
    assert_eq!(
        hits[0].2,
        "The `break` and `continue` statements no longer accept variable arguments."
    );
    assert_eq!(
        for_rule_with("<?php continue $x;\n", "a.php", &modern, 45).len(),
        1
    );
}

fn versioned_windows(version: &str, windows: &str) -> ast::XhpOptions {
    ast::XhpOptions {
        php_version: Some(version.to_string()),
        php_version_windows: Some(windows.to_string()),
        ..ast::XhpOptions::default()
    }
}

#[test]
fn compat_symbol_functions() {
    let options = versioned("5.3.0");
    let hits = for_rule_with("<?php array_column(array(), 'x');\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 7));
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.3.0, but `array_column()` was not introduced until PHP 5.5.0."
    );
    assert!(
        for_rule_with(
            "<?php array_column(array(), 'x');\n",
            "a.php",
            &versioned("5.5.0"),
            45
        )
        .is_empty()
    );
    let removed = versioned("5.4.0");
    let hits = for_rule_with("<?php define_syslog_variables();\n", "a.php", &removed, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.4.0, but `define_syslog_variables()` was removed in PHP 5.3.29."
    );
    assert!(
        for_rule_with(
            "<?php define_syslog_variables();\n",
            "a.php",
            &versioned("5.2.0"),
            45
        )
        .is_empty()
    );
    assert!(for_rule_with("<?php isset($x);\n", "a.php", &options, 45).is_empty());
    // Exact-name matching like PHP: case and `\` prefixes miss the table.
    assert!(for_rule_with("<?php ARRAY_COLUMN(array());\n", "a.php", &options, 45).is_empty());
    assert!(for_rule_with("<?php \\array_column(array());\n", "a.php", &options, 45).is_empty());
    assert!(
        for_rule_with(
            "<?php array_column(array());\n",
            "a.php",
            &ast::XhpOptions::default(),
            45
        )
        .is_empty()
    );
}

#[test]
fn compat_symbol_params() {
    let options = versioned("5.2.3");
    let hits = for_rule_with("<?php getopt('a', $b);\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.2.3, but parameter 2 of `getopt()` was not introduced until \
         PHP 5.3.0."
    );
    assert!(for_rule_with("<?php getopt('a');\n", "a.php", &options, 45).is_empty());
    // A firing min suppresses the parameter leg (PHP else-if chain).
    let old = versioned("5.2.0");
    let hits = for_rule_with("<?php idn_to_ascii($a, $b, $c);\n", "a.php", &old, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.2.0, but `idn_to_ascii()` was not introduced until PHP 5.2.4."
    );
    // Prerelease table versions compare below their release.
    let mid = versioned("5.3.0");
    assert_eq!(
        for_rule_with("<?php idn_to_ascii($a, $b, $c);\n", "a.php", &mid, 45).len(),
        1
    );
    assert!(
        for_rule_with(
            "<?php idn_to_ascii($a, $b, $c);\n",
            "a.php",
            &versioned("5.4.0"),
            45
        )
        .is_empty()
    );
}

#[test]
fn compat_symbol_windows() {
    let options = versioned_windows("5.2.3", "5.3.0");
    let hits = for_rule_with("<?php chroot('/tmp');\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.3.0 on Windows, but `chroot()` is not available there."
    );
    let old_win = versioned_windows("5.2.3", "5.2.0");
    let hits = for_rule_with("<?php getopt('a');\n", "a.php", &old_win, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.2.0 on Windows, but `getopt()` is not available there until \
         PHP 5.3.0."
    );
    assert!(for_rule_with("<?php getopt('a');\n", "a.php", &options, 45).is_empty());
    assert!(for_rule_with("<?php chroot('/tmp');\n", "a.php", &versioned("5.2.3"), 45).is_empty());
    let windows_only = ast::XhpOptions {
        php_version: None,
        php_version_windows: Some("5.2.0".to_string()),
        ..ast::XhpOptions::default()
    };
    assert!(for_rule_with("<?php chroot('/tmp');\n", "a.php", &windows_only, 45).is_empty());
}

#[test]
fn compat_symbol_classes() {
    let options = versioned("5.3.0");
    let hits = for_rule_with("<?php new CURLFile('');\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 11));
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.3.0, but `CURLFile` was not introduced until PHP 5.5.0."
    );
    assert!(
        for_rule_with(
            "<?php new CURLFile('');\n",
            "a.php",
            &versioned("5.5.0"),
            45
        )
        .is_empty()
    );
    assert_eq!(
        for_rule_with(
            "<?php class X extends DateTimeImmutable {}\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with("<?php CURLFile::create();\n", "a.php", &options, 45).len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php if ($x instanceof CURLFile) {}\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php try {} catch (CURLFile $e) {}\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php try {} catch (Exception | CURLFile $e) {}\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with("<?php function f(CURLFile $x) {}\n", "a.php", &options, 45).len(),
        1
    );
    assert_eq!(
        for_rule_with("<?php function f(): CURLFile {}\n", "a.php", &options, 45).len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php class X implements DateTimeInterface {}\n",
            "a.php",
            &versioned("5.4.0"),
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php new HttpException('x');\n",
            "a.php",
            &versioned("5.6.0"),
            45
        )
        .len(),
        1
    );
    assert!(
        for_rule_with(
            "<?php new HttpException('x');\n",
            "a.php",
            &versioned("5.5.0"),
            45
        )
        .is_empty()
    );
    // Declaration names are n_CLASS_NAME nodes in PHP, so they flag too.
    assert_eq!(
        for_rule_with("<?php class CURLFile {}\n", "a.php", &options, 45).len(),
        1
    );
    // Qualified names miss the table, exactly like PHP lookups.
    assert!(for_rule_with("<?php new \\CURLFile('');\n", "a.php", &options, 45).is_empty());
}

#[test]
fn compat_symbol_constants() {
    let options = versioned("5.3.0");
    let hits = for_rule_with("<?php echo JSON_PRETTY_PRINT;\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.3.0, but `JSON_PRETTY_PRINT` was not introduced until PHP \
         5.4.0."
    );
    assert!(
        for_rule_with(
            "<?php echo JSON_PRETTY_PRINT;\n",
            "a.php",
            &versioned("5.4.0"),
            45
        )
        .is_empty()
    );
    let old = versioned("5.2.3");
    assert_eq!(
        for_rule_with("<?php echo __DIR__;\n", "a.php", &old, 45).len(),
        1
    );
    assert!(for_rule_with("<?php echo __DIR__;\n", "a.php", &options, 45).is_empty());
    let removed = versioned("5.4.0");
    let hits = for_rule_with(
        "<?php var_dump(STREAM_ENFORCE_SAFE_MODE);\n",
        "a.php",
        &removed,
        45,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.4.0, but `STREAM_ENFORCE_SAFE_MODE` was removed in PHP 5.3.29."
    );
    assert_eq!(
        for_rule_with(
            "<?php switch ($x) { case JSON_PRETTY_PRINT: break; }\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert!(for_rule_with("<?php $x->JSON_PRETTY_PRINT;\n", "a.php", &options, 45).is_empty());
    assert!(for_rule_with("<?php Foo::JSON_PRETTY_PRINT;\n", "a.php", &options, 45).is_empty());
    assert!(
        for_rule_with(
            "<?php function JSON_PRETTY_PRINT() {}\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(
        for_rule_with(
            "<?php class C { const JSON_PRETTY_PRINT = 1; }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
}

#[test]
fn compat_symbol_whitelist() {
    let options = versioned("5.3.0");
    assert!(
        for_rule_with(
            "<?php if (function_exists('array_column')) { array_column(array(), 'x'); }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(
        for_rule_with(
            "<?php if (function_exists('array_column'))\n  array_column(array(), 'x');\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(
        for_rule_with(
            "<?php if (class_exists('CURLFile')) { new CURLFile(''); }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(
        for_rule_with(
            "<?php if (defined('JSON_PRETTY_PRINT')) { echo JSON_PRETTY_PRINT; }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert_eq!(
        for_rule_with(
            "<?php if (function_exists('array_column')) {} else { array_column(array()); }\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php if (!function_exists('array_column')) { array_column(array()); }\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert!(for_rule_with("<?php if (SomeClass::m($p)) {}\n", "a.php", &options, 45).is_empty());
    assert!(for_rule_with("<?php $v = 'f'; if ($v()) {}\n", "a.php", &options, 45).is_empty());
    assert_eq!(
        for_rule_with(
            "<?php if (function_exists($v)) { array_column(array()); }\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    // interface_exists guards file under a bucket no check reads, so uses still flag.
    assert_eq!(
        for_rule_with(
            "<?php if (interface_exists('DateTimeInterface')) { class X implements \
             DateTimeInterface {} }\n",
            "a.php",
            &versioned("5.4.0"),
            45
        )
        .len(),
        1
    );
}

#[test]
fn compat_symbol_gating() {
    let plain = ast::XhpOptions::default();
    assert!(for_rule_with("<?php array_column(array());\n", "a.php", &plain, 45).is_empty());
    assert!(for_rule_with("<?php new CURLFile('');\n", "a.php", &plain, 45).is_empty());
    assert!(for_rule_with("<?php echo __DIR__;\n", "a.php", &plain, 45).is_empty());
    assert!(
        for_rule_with(
            "<?php array_column(array());\n",
            "a.php",
            &versioned(""),
            45
        )
        .is_empty()
    );
}

#[test]
fn compat_db_shape_pins_vendored_json() {
    // The runtime parses the vendored table fail-soft (`Null` on
    // any corruption, silently disabling every symbol finding), so
    // pin its shape loudly here.
    let db = super::compat_db();
    for section in [
        "functions",
        "classes",
        "interfaces",
        "constants",
        "params",
        "functions_windows",
    ] {
        assert!(
            db.get(section).is_some_and(|value| value.is_object()),
            "missing symbol-table section: {section}"
        );
    }
    // The `has_uppercase` pre-filter relies on every constants key
    // carrying an uppercase letter.
    let constants = db.get("constants").expect("constants section");
    let map = constants.as_object().expect("constants object");
    assert!(!map.is_empty());
    for key in map.keys() {
        assert!(
            key.bytes().any(|byte| byte.is_ascii_uppercase()),
            "lowercase constants key: {key}"
        );
    }
    // `\`-prefixed lookups always miss: no key starts with one
    // (qualified keys like `http\Client\Curl\AUTH_ANY` do exist and
    // are matched by full-text run lookups).
    for section in ["functions", "classes", "interfaces", "constants"] {
        let entries = db.get(section).expect("lookup section");
        for key in entries.as_object().expect("lookup object").keys() {
            assert!(!key.starts_with('\\'), "slashed key in {section}: {key}");
        }
    }
    assert!(
        db.get("constants")
            .and_then(|entries| entries.get("http\\Client\\Curl\\AUTH_ANY"))
            .is_some(),
        "namespaced constant missing from table"
    );
    // Spot-check known symbols against the PHP-side table values.
    let min = |section: &str, name: &str| {
        db.get(section)
            .and_then(|entries| entries.get(name))
            .and_then(|entry| entry.get("php.min"))
            .and_then(serde_json::Value::as_str)
    };
    assert_eq!(min("constants", "JSON_PRETTY_PRINT"), Some("5.4.0"));
    assert_eq!(min("functions", "array_column"), Some("5.5.0"));
    assert_eq!(min("classes", "CURLFile"), Some("5.5.0"));
}

#[test]
fn compat_version_zero_is_falsy() {
    // PHP `!$version`: "0" disables the rule just like "" does.
    let zero = versioned("0");
    assert!(for_rule_with("<?php array_column(array());\n", "a.php", &zero, 45).is_empty());
    assert!(for_rule_with("<?php new CURLFile('');\n", "a.php", &zero, 45).is_empty());
    assert!(for_rule_with("<?php echo JSON_PRETTY_PRINT;\n", "a.php", &zero, 45).is_empty());
    assert!(for_rule_with("<?php namespace a;\n", "a.php", &zero, 45).is_empty());
    let zero_win = versioned_windows("5.2.3", "0");
    assert!(for_rule_with("<?php chroot('/tmp');\n", "a.php", &zero_win, 45).is_empty());
    // ...but "0.0" is truthy in PHP and still gates.
    assert_eq!(
        for_rule_with(
            "<?php array_column(array());\n",
            "a.php",
            &versioned("0.0"),
            45
        )
        .len(),
        1
    );
}

#[test]
fn compat_symbol_constant_imports() {
    let options = versioned("5.3.0");
    // Single-segment import names are n_SYMBOL_NAME in PHP: one lookup.
    let hits = for_rule_with("<?php use JSON_PRETTY_PRINT;\n", "a.php", &options, 45);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (1, 11));
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.3.0, but `JSON_PRETTY_PRINT` was not introduced until PHP \
         5.4.0."
    );
    let hits = for_rule_with(
        "<?php namespace JSON_PRETTY_PRINT;\n",
        "a.php",
        &options,
        45,
    );
    assert_eq!(hits.len(), 1);
    // Multi-segment and `\`-prefixed names miss, like PHP's full-string lookup.
    assert!(for_rule_with("<?php use A\\JSON_PRETTY_PRINT;\n", "a.php", &options, 45).is_empty());
    assert!(
        for_rule_with(
            "<?php namespace A\\JSON_PRETTY_PRINT;\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(for_rule_with("<?php use \\JSON_PRETTY_PRINT;\n", "a.php", &options, 45).is_empty());
    // `as` aliases are n_STRING: never scanned.
    assert!(for_rule_with("<?php use C as JSON_PRETTY_PRINT;\n", "a.php", &options, 45).is_empty());
    // Comma-separated imports each scan once.
    assert_eq!(
        for_rule_with("<?php use E, JSON_PRETTY_PRINT;\n", "a.php", &options, 45).len(),
        1
    );
    // `namespace\FOO` operator uses carry no declaration name.
    assert!(
        for_rule_with(
            "<?php echo namespace\\JSON_PRETTY_PRINT;\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    // Beyond-stock-grammar statements are skipped, never flagged.
    assert!(
        for_rule_with(
            "<?php use const JSON_PRETTY_PRINT;\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(for_rule_with("<?php use A\\{JSON_PRETTY_PRINT};\n", "a.php", &options, 45).is_empty());
    // Trait uses are n_CLASS_NAME, not constants.
    assert!(
        for_rule_with(
            "<?php class C { use JSON_PRETTY_PRINT; }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
}

#[test]
fn compat_symbol_constant_labels() {
    let options = versioned("5.3.0");
    // Labels are n_LABEL/n_STRING in PHP: never scanned.
    assert!(
        for_rule_with(
            "<?php function f() { goto JSON_PRETTY_PRINT; JSON_PRETTY_PRINT: return; }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    assert!(for_rule_with("<?php JSON_PRETTY_PRINT: echo 1;\n", "a.php", &options, 45).is_empty());
    assert!(
        for_rule_with(
            "<?php function g() { JSON_PRETTY_PRINT: return; }\n",
            "a.php",
            &options,
            45
        )
        .is_empty()
    );
    // ...but `case` values and ternary branches are expressions.
    assert_eq!(
        for_rule_with(
            "<?php switch ($x) { case JSON_PRETTY_PRINT: break; }\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule_with(
            "<?php $y = $x ? JSON_PRETTY_PRINT : 0;\n",
            "a.php",
            &options,
            45
        )
        .len(),
        1
    );
}

#[test]
fn compat_symbol_constant_namespaced() {
    let old = versioned("5.2.0");
    // Namespaced constants live in the table: full-text run lookup.
    let hits = for_rule_with(
        "<?php echo http\\Client\\Curl\\AUTH_ANY;\n",
        "a.php",
        &old,
        45,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This codebase targets PHP 5.2.0, but `http\\Client\\Curl\\AUTH_ANY` was not introduced \
         until PHP 5.3.0."
    );
    assert!(
        for_rule_with(
            "<?php echo http\\Client\\Curl\\AUTH_ANY;\n",
            "a.php",
            &versioned("5.3.0"),
            45
        )
        .is_empty()
    );
    // `\`-prefixed and gapped spellings miss, like PHP's concrete string.
    assert!(
        for_rule_with(
            "<?php echo \\http\\Client\\Curl\\AUTH_ANY;\n",
            "a.php",
            &old,
            45
        )
        .is_empty()
    );
    assert!(
        for_rule_with(
            "<?php echo http \\Client\\Curl\\AUTH_ANY;\n",
            "a.php",
            &old,
            45
        )
        .is_empty()
    );
    // Ordinary qualified names still miss.
    assert!(for_rule_with("<?php echo A\\B;\n", "a.php", &old, 45).is_empty());
}
