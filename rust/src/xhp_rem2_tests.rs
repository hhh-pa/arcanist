use super::super::xhp_ast as ast;

fn check_with(source: &str, options: &ast::XhpOptions) -> Vec<(u32, usize, usize, String)> {
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

fn check(source: &str) -> Vec<(u32, usize, usize, String)> {
    check_with(source, &ast::XhpOptions::default())
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
    rule: u32,
    options: &ast::XhpOptions,
) -> Vec<(usize, usize, String)> {
    check_with(source, options)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

#[test]
fn duplicate_case_flags_last_arm_only() {
    let src = "<?php switch ($x) {\ncase 1:\nbreak;\ncase 2:\nbreak;\ncase 1:\nbreak;\n}\n";
    let hits = for_rule(src, 50);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 6);
    assert_eq!(
        hits[0].2,
        "Duplicate case in switch statement. PHP will ignore all but the first case."
    );
    assert!(
        for_rule(
            "<?php switch ($x) {\ncase 1:\nbreak;\ncase 2:\nbreak;\n}\n",
            50
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php switch ($x) {\ncase 1:\nbreak;\ndefault:\nbreak;\n}\n",
            50
        )
        .is_empty()
    );
}

#[test]
fn duplicate_case_ignores_whitespace_differences() {
    let src = "<?php switch ($x) {\ncase 1+2:\nbreak;\ncase 1 + 2:\nbreak;\n}\n";
    assert_eq!(for_rule(src, 50).len(), 1);
    let src = "<?php switch ($x) {\ncase 'a':\nbreak;\ncase \"a\":\nbreak;\n}\n";
    assert!(for_rule(src, 50).is_empty());
}

#[test]
fn implicit_visibility_methods_need_visibility() {
    let hits = for_rule("<?php class A {\nfunction f() {}\n}\n", 52);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 2);
    assert_eq!(
        hits[0].2,
        "Methods should have their visibility declared explicitly."
    );
    assert!(for_rule("<?php class A {\npublic function f() {}\n}\n", 52).is_empty());
    assert!(for_rule("<?php class A {\nprotected static function f() {}\n}\n", 52).is_empty());
    assert!(for_rule("<?php function f() {}\n", 52).is_empty());
    assert!(for_rule("<?php interface I {\npublic function f();\n}\n", 52).is_empty());
    assert_eq!(
        for_rule("<?php interface I {\nfunction f();\n}\n", 52).len(),
        1
    );
}

#[test]
fn implicit_visibility_var_means_public() {
    let hits = for_rule("<?php class A {\nvar $x;\n}\n", 52);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Use `public` instead of `var` to indicate public visibility."
    );
    assert!(for_rule("<?php class A {\npublic $x;\n}\n", 52).is_empty());
    assert!(for_rule("<?php class A {\nprivate static $x;\n}\n", 52).is_empty());
    let hits = for_rule("<?php class A {\nstatic $x;\n}\n", 52);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Properties should have their visibility declared explicitly."
    );
}

#[test]
fn unnecessary_final_flags_final_methods_in_final_class() {
    let hits = for_rule(
        "<?php final class A {\nfinal public function f() {}\n}\n",
        55,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Unnecessary `final` modifier in `final` class.");
    assert!(for_rule("<?php final class A {\npublic function f() {}\n}\n", 55).is_empty());
    assert!(for_rule("<?php class A {\nfinal public function f() {}\n}\n", 55).is_empty());
}

#[test]
fn self_reference_flags_own_class_name() {
    let hits = for_rule(
        "<?php class A {\npublic function f() {\nreturn A::$x;\n}\n}\n",
        57,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 3);
    assert_eq!(
        hits[0].2,
        "Use `self::` for local static member references."
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function f() {\nreturn self::$x;\n}\n}\n",
            57
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function f() {\nreturn B::$x;\n}\n}\n",
            57
        )
        .is_empty()
    );
    assert!(for_rule("<?php class A {\npublic function f(A $x) {\n}\n}\n", 57).is_empty());
}

#[test]
fn self_reference_case_insensitive_but_not_qualified() {
    assert_eq!(
        for_rule(
            "<?php class A {\npublic function f() {\nreturn a::X;\n}\n}\n",
            57
        )
        .len(),
        1
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function f() {\nreturn \\A::X;\n}\n}\n",
            57
        )
        .is_empty()
    );
}

#[test]
fn self_reference_closure_flag_depends_on_version() {
    let src =
        "<?php class A {\npublic function f() {\n$g = function () {\nreturn A::X;\n};\n}\n}\n";
    assert_eq!(for_rule(src, 57).len(), 1);
    let old = ast::XhpOptions {
        php_version: Some("5.3.0".to_string()),
        ..ast::XhpOptions::default()
    };
    assert!(for_rule_with(src, 57, &old).is_empty());
    let new = ast::XhpOptions {
        php_version: Some("5.4.0".to_string()),
        ..ast::XhpOptions::default()
    };
    assert_eq!(for_rule_with(src, 57, &new).len(), 1);
    let direct = "<?php class A {\npublic function f() {\nreturn A::X;\n}\n}\n";
    assert_eq!(for_rule_with(direct, 57, &old).len(), 1);
}

#[test]
fn inner_functions_flag_named_nesting() {
    let hits = for_rule("<?php function outer() {\nfunction inner() {}\n}\n", 59);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 2);
    assert_eq!(hits[0].2, "Avoid the use of inner functions.");
    assert!(for_rule("<?php function outer() {\n$f = function () {};\n}\n", 59).is_empty());
    assert!(for_rule("<?php function a() {}\nfunction b() {}\n", 59).is_empty());
    assert!(
        for_rule(
            "<?php class A {\npublic function m() {\nfunction inner() {}\n}\n}\n",
            59
        )
        .is_empty()
    );
}

#[test]
fn default_parameters_must_trail() {
    let hits = for_rule("<?php function f($a = 1, $b) {}\n", 60);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 1);
    assert_eq!(
        hits[0].2,
        "Arguments with default values must be at the end of the argument list."
    );
    assert!(for_rule("<?php function f($a, $b = 1) {}\n", 60).is_empty());
    assert!(for_rule("<?php function f($a = 1, ...$rest) {}\n", 60).is_empty());
    assert!(for_rule("<?php function f($a, $b) {}\n", 60).is_empty());
}

#[test]
fn useless_override_flags_pure_forwarding() {
    let src = "<?php class B extends A {\npublic function f($x) {\nparent::f($x);\n}\n}\n";
    let hits = for_rule(src, 63);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Useless overriding method.");
    let src = "<?php class B extends A {\npublic function f($x) {\nreturn parent::f($x);\n}\n}\n";
    assert_eq!(for_rule(src, 63).len(), 1);
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\nparent::g($x);\n}\n}\n",
            63
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\nparent::f($y);\n}\n}\n",
            63
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\nparent::f($x);\nfoo();\n}\n}\n",
            63
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x = 1) {\nparent::f($x);\n}\n}\n",
            63
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\n$x = parent::f($x);\n}\n}\n",
            63
        )
        .is_empty()
    );
}

#[test]
fn no_parent_flags_parent_access_without_extends() {
    let hits = for_rule(
        "<?php class A {\npublic function f() {\nparent::g();\n}\n}\n",
        64,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 3);
    assert_eq!(
        hits[0].2,
        "Cannot access `parent::` when current class scope has no parent."
    );
    assert!(
        for_rule(
            "<?php class A extends B {\npublic function f() {\nparent::g();\n}\n}\n",
            64
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function f() {\nself::g();\n}\n}\n",
            64
        )
        .is_empty()
    );
}

#[test]
fn tostring_throw_flags_each_throw() {
    let src = "<?php class A {\npublic function __toString() {\nthrow new Exception();\n}\n}\n";
    let hits = for_rule(src, 67);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 3);
    assert_eq!(
        hits[0].2,
        "It is not possible to throw an `Exception` from within the `__toString` method."
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function f() {\nthrow new Exception();\n}\n}\n",
            67
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function __toString() {\nreturn 'x';\n}\n}\n",
            67
        )
        .is_empty()
    );
}

#[test]
fn instanceof_flags_constants_not_objects() {
    let hits = for_rule("<?php if (FOO instanceof Bar) {}\n", 69);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "`instanceof` expects an object instance, constant given."
    );
    assert_eq!(for_rule("<?php if (1 instanceof Bar) {}\n", 69).len(), 1);
    assert_eq!(for_rule("<?php if ('x' instanceof Bar) {}\n", 69).len(), 1);
    assert_eq!(for_rule("<?php $b = true instanceof Bar;\n", 69).len(), 1);
    assert!(for_rule("<?php if ($x instanceof Bar) {}\n", 69).is_empty());
    assert!(for_rule("<?php if (foo() instanceof Bar) {}\n", 69).is_empty());
    assert!(for_rule("<?php if ($x->y instanceof Bar) {}\n", 69).is_empty());
    assert!(for_rule("<?php if (Foo::BAR instanceof Bar) {}\n", 69).is_empty());
    assert!(for_rule("<?php if (new Foo instanceof Bar) {}\n", 69).is_empty());
}

#[test]
fn invalid_default_array_needs_array_or_null() {
    let hits = for_rule("<?php function f(array $x = 5) {}\n", 70);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Default value for parameters with `array` type hint can only be an `array` or `null`."
    );
    assert!(for_rule("<?php function f(array $x = array()) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f(array $x = []) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f(array $x = null) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f($x = 5) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f(string $x = 'a') {}\n", 70).is_empty());
}

#[test]
fn invalid_default_callable_and_class_need_null() {
    let hits = for_rule("<?php function f(callable $c = 'strlen') {}\n", 70);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Default value for parameters with `callable` type hint can only be `null`."
    );
    assert!(for_rule("<?php function f(callable $c = null) {}\n", 70).is_empty());
    let hits = for_rule("<?php function f(Foo $x = 5) {}\n", 70);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Default value for parameters with a class type hint can only be `null`."
    );
    assert!(for_rule("<?php function f(Foo $x = null) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f(?Foo $x = null) {}\n", 70).is_empty());
}

#[test]
fn invalid_modifiers_rejects_bad_combos() {
    let hits = for_rule("<?php class A {\nabstract $x;\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Properties cannot be declared \"abstract\".");
    let hits = for_rule("<?php class A {\nfinal $x;\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Properties can not be declared \"final\".");
    let hits = for_rule("<?php class A {\npublic private function f() {}\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Multiple access type modifiers are not allowed.");
    let hits = for_rule("<?php class A {\nabstract final function f();\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Methods may not be both \"abstract\" and \"final\"."
    );
    let hits = for_rule("<?php class A {\nprivate final function f() {}\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Methods may not be both \"private\" and \"final\"."
    );
    assert!(for_rule("<?php class A {\npublic static $x;\n}\n", 72).is_empty());
    assert!(for_rule("<?php class A {\nstatic static function f() {}\n}\n", 72).is_empty());
}

#[test]
fn array_value_requires_one_element_per_line() {
    let src = "<?php $a = array($x, $y,\n$z);\n";
    let hits = for_rule(src, 76);
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].2,
        "Array elements should each occupy a single line."
    );
    assert!(for_rule("<?php $a = array($x, $y);\n", 76).is_empty());
    assert!(for_rule("<?php $a = array(\n$x,\n$y,\n);\n", 76).is_empty());
    assert!(for_rule("<?php $a = array(\n);\n", 76).is_empty());
    let src = "<?php $a = [$x, $y,\n$z];\n";
    assert_eq!(for_rule(src, 76).len(), 2);
    assert!(for_rule("<?php echo $a[0];\n", 76).is_empty());
}

#[test]
fn useless_override_is_case_sensitive_and_lenient_on_arity() {
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\nParent::f($x);\n}\n}\n",
            63
        )
        .is_empty()
    );
    assert_eq!(
        for_rule(
            "<?php class B extends A {\npublic function f($x, $y) {\nparent::f($x);\n}\n}\n",
            63
        )
        .len(),
        1
    );
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\nparent::f($x, $y);\n}\n}\n",
            63
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php class B extends A {\npublic function f($x) {\nparent::f(1);\n}\n}\n",
            63
        )
        .is_empty()
    );
}

#[test]
fn invalid_modifiers_repeated_modifiers_each_flag() {
    let hits = for_rule("<?php class A {\nfinal final function f() {}\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Multiple \"final\" modifiers are not allowed.");
    let hits = for_rule(
        "<?php abstract class A {\nabstract abstract function f();\n}\n",
        72,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Multiple \"abstract\" modifiers are not allowed."
    );
    let hits = for_rule("<?php class A {\npublic public $x;\n}\n", 72);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Multiple access type modifiers are not allowed.");
}

#[test]
fn invalid_default_handles_nullable_unions_and_promotion() {
    assert!(for_rule("<?php function f(?array $x = []) {}\n", 70).is_empty());
    let hits = for_rule("<?php function f(Foo|Bar $x = 5) {}\n", 70);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Default value for parameters with a class type hint can only be `null`."
    );
    assert!(for_rule("<?php function f(int|string $x = 5) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f(callable $c = strlen(...)) {}\n", 70).is_empty());
    assert!(
        for_rule(
            "<?php class A {\npublic function __construct(private array $x = []) {}\n}\n",
            70
        )
        .is_empty()
    );
    let hits = for_rule(
        "<?php class A {\npublic function __construct(private Foo $x = 5) {}\n}\n",
        70,
    );
    assert_eq!(hits.len(), 1);
}

#[test]
fn array_value_skips_list_targets_and_destructuring() {
    assert!(for_rule("<?php [$a,\n$b] = $c;\n", 76).is_empty());
    assert!(for_rule("<?php foreach ($x as [$a,\n$b]) {}\n", 76).is_empty());
    assert!(for_rule("<?php $a = $b[\n0];\n", 76).is_empty());
}

#[test]
fn list_assignment_nested_lists_each_flag() {
    let hits = for_rule("<?php list($a, list($b,)) = $c;\n", 77);
    assert_eq!(hits.len(), 1);
    let hits = for_rule("<?php list($a, list($b,),) = $c;\n", 77);
    assert_eq!(hits.len(), 2);
}

#[test]
fn self_reference_class_constant_fetch_flags() {
    assert_eq!(
        for_rule(
            "<?php class A {\npublic function f() {\nreturn A::class;\n}\n}\n",
            57
        )
        .len(),
        1
    );
    assert!(
        for_rule(
            "<?php class A {\npublic function f() {\nreturn $this::class;\n}\n}\n",
            57
        )
        .is_empty()
    );
}

#[test]
fn implicit_visibility_tolerates_attributes() {
    assert!(for_rule("<?php class A {\n#[Attr]\npublic function f() {}\n}\n", 52).is_empty());
    assert!(for_rule("<?php class A {\n#[Attr]\npublic $x;\n}\n", 52).is_empty());
    assert!(for_rule("<?php class A {\nconst X = 1;\n}\n", 52).is_empty());
    assert!(for_rule("<?php class A {\npublic const X = 1;\n}\n", 52).is_empty());
}

#[test]
fn list_assignment_flags_trailing_commas() {
    let hits = for_rule("<?php list($a, $b,) = $c;\n", 77);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Unnecessary comma in list assignment.");
    assert!(for_rule("<?php list($a, $b) = $c;\n", 77).is_empty());
    let hits = for_rule("<?php [$a, $b,] = $c;\n", 77);
    assert_eq!(hits.len(), 1);
    assert!(for_rule("<?php [$a, $b] = $c;\n", 77).is_empty());
}

// -( T1170 COV5: close xhp_rem2.rs )---------------------------------------------

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

fn for_rule_canned(json: &str, rule: u32) -> Vec<(usize, usize, String)> {
    check_canned(json)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

#[test]
fn modifier_scan_skips_brackets_and_stops() {
    // A non-attribute `]` ends the modifier run.
    assert!(for_rule("<?php class A {\n$x = [1] public function f() {}\n}\n", 52).is_empty());
    // Nested brackets unwind first.
    assert!(
        for_rule(
            "<?php class A {\n$x = [[1]] public function f() {}\n}\n",
            52
        )
        .is_empty()
    );
    // A statement boundary stops the attribute scan.
    assert!(
        for_rule(
            "<?php class A {\npublic $x = 1; 2] public function f() {}\n}\n",
            52
        )
        .is_empty()
    );
}

#[test]
fn attribute_scan_past_start_returns_none() {
    // No trivia, brackets, or stops before the `]`: the scan runs
    // off the start of the token stream.
    let hits = for_rule_canned(
        r#"{"source": "C]final class A {}",
          "tokens": [["T_STRING", "C", 0], ["]", "]", 1],
            ["T_WHITESPACE", " ", 2], ["T_FINAL", "final", 3],
            ["T_WHITESPACE", " ", 8], ["T_CLASS", "class", 9],
            ["T_WHITESPACE", " ", 14], ["T_STRING", "A", 15],
            ["T_WHITESPACE", " ", 16], ["{", "{", 17], ["}", "}", 18]]}"#,
        55,
    );
    assert!(hits.is_empty());
}

#[test]
fn duplicate_case_handles_colon_less_arms() {
    // The first arm ends at the next arm.
    let hits = for_rule("<?php switch ($x) {\ncase 1 case 1:\nbreak;\n}\n", 50);
    assert_eq!(hits.len(), 1);
    // The last arm ends at the switch body end.
    let hits = for_rule("<?php switch ($x) {\ncase 1:\nbreak;\ncase 1 }\n", 50);
    assert_eq!(hits.len(), 1);
}

#[test]
fn properties_need_brace_bodies() {
    assert!(for_rule("<?php class A;\n", 52).is_empty());
    assert!(for_rule("<?php class A;\n", 72).is_empty());
}

#[test]
fn unclosed_attribute_abandons_property() {
    assert!(for_rule("<?php class A {\n#[Foo public $x;\n}\n", 52).is_empty());
}

#[test]
fn methods_outside_final_classes_are_ignored() {
    assert!(
        for_rule(
            "<?php final class A {}\nclass B {\nfunction g() {}\n}\n",
            55
        )
        .is_empty()
    );
}

#[test]
fn anonymous_classes_have_no_name() {
    assert!(
        for_rule(
            "<?php $x = new class {\nfunction f() { return 1; }\n};\n",
            57
        )
        .is_empty()
    );
}

#[test]
fn qualified_self_names_walk_back() {
    assert!(
        for_rule(
            "<?php class Foo {\nfunction f() { return Foo\\Bar::baz(); }\n}\n",
            57
        )
        .is_empty()
    );
}

#[test]
fn arrow_without_parens_has_no_lparen() {
    assert!(for_rule("<?php $f = fn => 42;\n", 60).is_empty());
}

#[test]
fn anonymous_method_closures_are_skipped() {
    assert!(for_rule("<?php class A {\npublic $f = function() {};\n}\n", 63).is_empty());
}

#[test]
fn useless_override_rejects_partial_forwards() {
    for body in [
        "{ return; }",
        "{ parent; }",
        "{ parent foo; }",
        "{ parent::; }",
        "{ parent::f; }",
        "{ parent::f x; }",
        "{ parent::f(; }",
        "{ parent::f() + 1; }",
        "{ parent::f($x + 1); }",
    ] {
        let src = format!("<?php class A {{\nfunction f($x) {body}\n}}\n");
        assert!(for_rule(&src, 63).is_empty(), "{body}");
    }
}

#[test]
fn parent_access_skips_other_classes() {
    let hits = for_rule(
        "<?php class A {}\nclass B {\nfunction f() { parent::g(); }\n}\n",
        64,
    );
    assert_eq!(hits.len(), 1);
}

#[test]
fn tostring_without_body_is_skipped() {
    assert!(for_rule("<?php interface I {\nfunction __toString();\n}\n", 67).is_empty());
}

#[test]
fn instanceof_at_end_is_skipped() {
    assert!(for_rule("<?php $x instanceof", 69).is_empty());
}

#[test]
fn instanceof_leading_scalar_flags() {
    let hits = for_rule_canned(
        r#"{"source": "5 instanceof X;",
          "tokens": [["T_LNUMBER", "5", 0], ["T_WHITESPACE", " ", 1],
            ["T_INSTANCEOF", "instanceof", 2], ["T_WHITESPACE", " ", 12],
            ["T_STRING", "X", 13], [";", ";", 14]]}"#,
        69,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "`instanceof` expects an object instance, constant given."
    );
}

#[test]
fn instanceof_absolute_names_walk_back() {
    let hits = for_rule("<?php \\FOO instanceof X;\n", 69);
    assert_eq!(hits.len(), 1);
}

#[test]
fn array_hint_without_semantic_default_records_nothing() {
    // Trivia-only and missing defaults record no range at all.
    assert!(for_rule("<?php function f(array $x = /*c*/) {}\n", 70).is_empty());
    assert!(for_rule("<?php function f(array $x = ) {}\n", 70).is_empty());
}

#[test]
fn list_scan_skips_trivia_before_close() {
    assert!(for_rule("<?php list($a, $b ) = $c;\n", 77).is_empty());
}

fn t1170_tree(source: &str) -> ast::Tree {
    let canned = ast::tokens_to_canned(source, &ast::tokenize(source));
    ast::tree_from_canned(&canned).expect("canned output must parse")
}

#[test]
fn t1170_empty_default_range_is_not_an_array() {
    // T1170 band J: a default range with no tokens cannot be an
    // array default.
    let tree = t1170_tree("<?php 1\n");
    assert!(!super::default_is_array(&tree, (3, 1)));
}
