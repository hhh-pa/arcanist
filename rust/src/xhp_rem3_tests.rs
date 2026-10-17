use super::super::xhp_ast as ast;

fn check(source: &str) -> Vec<(u32, usize, usize, String)> {
    let canned = ast::tokens_to_canned(source, &ast::tokenize(source));
    let tree = ast::tree_from_canned(&canned).expect("canned output must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options: &ast::XhpOptions::default(),
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
fn wiring_meta_matches_php_names_and_severities() {
    let table = super::super::RULE_TABLE;
    for (id, name, severity) in [
        (87, "Class Not `abstract` Or `final`", "disabled"),
        (88, "Class Not Extending `Phobject`", "disabled"),
        (91, "`$this` Reassignment", "error"),
        (92, "Unexpected `return` Value", "warning"),
        (95, "Self Class Reference", "warning"),
        (
            107,
            "`abstract` Method Cannot Be Declared `private`",
            "error",
        ),
        (108, "`abstract` Method Cannot Contain Body", "error"),
        (
            113,
            "`class` Containing `abstract` Methods Must Be Declared `abstract`",
            "error",
        ),
        (114, "`interface` Method Cannot Contain Body", "error"),
        (
            118,
            "`interface` Methods Cannot Be Marked `abstract`",
            "error",
        ),
        (128, "Continue Inside Switch", "error"),
        (130, "Use of `public` Properties", "advice"),
        (132, "Partial Catch", "warning"),
    ] {
        assert!(
            table.contains(&(id, name, severity)),
            "RULE_TABLE lacks ({id}, {name}, {severity})"
        );
        assert_eq!(super::super::rule_meta(id), Some((name, severity)));
    }
    let blocked = super::super::blocked_rules();
    for id in [87, 88, 91, 92, 95, 107, 108, 113, 114, 118, 128, 130, 132] {
        assert!(
            !blocked.iter().any(|(b, _, _)| *b == id),
            "rule {id} still blocked"
        );
    }
}

#[test]
fn ragged_edge_flags_plain_class_at_name() {
    let hits = for_rule("<?php\nclass A extends Phobject {}\n", 87);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 7));
    assert_eq!(
        hits[0].2,
        "This class is neither `final` nor `abstract`, and does not have a docblock marking it \
         `@concrete-extensible`."
    );
}

#[test]
fn ragged_edge_exempts_final_abstract_and_docblock() {
    assert!(for_rule("<?php\nfinal class C extends Phobject {}\n", 87).is_empty());
    assert!(for_rule("<?php\nabstract class B extends Phobject {}\n", 87).is_empty());
    assert!(
        for_rule(
            "<?php\n/**\n * @concrete-extensible\n */\nclass D extends Phobject {}\n",
            87
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\n/** @concrete-extensible because reasons */\nclass D {}\n",
            87
        )
        .is_empty()
    );
    assert!(for_rule("<?php\n/** plain docblock */\nclass D {}\n", 87).len() == 1);
    assert!(for_rule("<?php\ninterface I {}\n", 87).is_empty());
    assert!(for_rule("<?php\ntrait T {}\n", 87).is_empty());
    assert!(for_rule("<?php\n$c = new class {};\n", 87).is_empty());
}

#[test]
fn ragged_edge_modifier_match_is_case_sensitive_like_php() {
    assert_eq!(for_rule("<?php\nFINAL class C {}\n", 87).len(), 1);
    assert_eq!(for_rule("<?php\nABSTRACT class C {}\n", 87).len(), 1);
}

#[test]
fn extends_object_flags_bare_class_at_node_start() {
    let src = "<?php\n\nabstract class A extends Phobject {}\nfinal class B extends A {}\nfinal class C {}\n";
    let hits = for_rule(src, 88);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (5, 1));
    assert_eq!(
        hits[0].2,
        "Classes should extend from `Phobject` or from some other class. All classes (except \
         for `Phobject` itself) should have a base class."
    );
    assert!(for_rule("<?php\nclass Phobject {}\n", 88).is_empty());
    assert!(for_rule("<?php\nclass phobject {}\n", 88).len() == 1);
    assert!(for_rule("<?php\ninterface I {}\n", 88).is_empty());
    assert!(for_rule("<?php\n$c = new class {};\n", 88).is_empty());
}

#[test]
fn this_reassignment_flags_plain_assignment() {
    let hits = for_rule("<?php\n$this = null;\n", 91);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 1));
    assert_eq!(
        hits[0].2,
        "`$this` cannot be re-assigned. This construct will cause a PHP fatal error."
    );
    let hits = for_rule(
        "<?php\nclass SomeClass {\n  public function __construct() {\n    $this = null;\n  }\n}\n",
        91,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 5));
    assert!(for_rule("<?php if ($this == $x) {}\n", 91).is_empty());
    assert!(for_rule("<?php if ($this === $x) {}\n", 91).is_empty());
    assert!(for_rule("<?php if ($this != $x) {}\n", 91).is_empty());
    assert!(for_rule("<?php $x = [$this => 1];\n", 91).is_empty());
    assert!(for_rule("<?php $$this = 1;\n", 91).is_empty());
    assert!(for_rule("<?php $this->x = 1;\n", 91).is_empty());
    assert!(for_rule("<?php $this[0] = 1;\n", 91).is_empty());
    assert!(for_rule("<?php foreach ($a as $this) {}\n", 91).is_empty());
    assert!(for_rule("<?php echo \"$this\";\n", 91).is_empty());
}

#[test]
fn unexpected_return_flags_valued_returns_in_ctors() {
    let hits = for_rule(
        "<?php\nclass SomeClass {\n  public function __construct() {\n    return null;\n  }\n}\n",
        92,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 5));
    assert_eq!(
        hits[0].2,
        "Unexpected `return` value in `__construct` method."
    );
    let hits = for_rule(
        "<?php\nclass SomeClass {\n  public function __destruct() {\n    return null;\n  }\n}\n",
        92,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Unexpected `return` value in `__destruct` method."
    );
    assert!(
        for_rule(
            "<?php\nclass SomeClass {\n  private $closure;\n\n  public function __construct() {\n    $this->closure = function() {\n        return null;\n    };\n  }\n}\n",
            92
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass SomeClass {\n  public function __construct() {\n    return;\n  }\n}\n",
            92
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass SomeClass {\n  public function other() {\n    return null;\n  }\n}\n",
            92
        )
        .is_empty()
    );
    assert!(for_rule("<?php\nfunction __construct() {\n  return 1;\n}\n", 92).is_empty());
}

#[test]
fn unexpected_return_includes_nested_named_functions_like_php() {
    let hits = for_rule(
        "<?php\nclass C {\n  public function __construct() {\n    function helper() {\n      return 1;\n    }\n  }\n}\n",
        92,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (5, 7));
}

#[test]
fn unexpected_return_uses_original_method_case() {
    let hits = for_rule(
        "<?php\nclass C {\n  public function __CONSTRUCT() {\n    return 1;\n  }\n}\n",
        92,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Unexpected `return` value in `__CONSTRUCT` method."
    );
}

#[test]
fn self_reference_flags_same_class_instantiation() {
    let hits = for_rule(
        "<?php\n\nclass Foo extends Bar {\n  public static function newInstance() {\n    return new Foo();\n  }\n}\n",
        95,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (5, 16));
    assert_eq!(hits[0].2, "Use `self` to instantiate the current class.");
    assert!(
        for_rule(
            "<?php\nclass Foo {\n  public static function init() {\n    return new self();\n  }\n}\n",
            95
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass Foo {\n  public function f() {\n    return new \\Foo();\n  }\n}\n",
            95
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass Foo {\n  public function f() {\n    return new Foo\\Bar();\n  }\n}\n",
            95
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass Foo {\n  public function f() {\n    return new Other();\n  }\n}\n",
            95
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass Foo {\n  public function f() {\n    return new fOo();\n  }\n}\n",
            95
        )
        .len()
            == 1
    );
    assert!(
        for_rule(
            "<?php\n$c = new class {\n  public function f() {\n    return new self();\n  }\n};\n",
            95
        )
        .is_empty()
    );
}

#[test]
fn abstract_private_method_message_and_offset_are_exact() {
    let hits = for_rule(
        "<?php\nabstract class SomeClass {\n  private abstract function someMethod();\n}\n",
        107,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (3, 3));
    assert_eq!(
        hits[0].2,
        "`abstract` method cannot be declared `private`. This construct will cause a fatal error."
    );
    assert!(
        for_rule(
            "<?php\nabstract class C {\n  abstract public function f();\n}\n",
            107
        )
        .is_empty()
    );
    assert!(for_rule("<?php\nclass C {\n  private function f() {}\n}\n", 107).is_empty());
    assert_eq!(
        for_rule(
            "<?php\nabstract class C {\n  PRIVATE ABSTRACT function f();\n}\n",
            107
        )
        .len(),
        1
    );
    assert!(for_rule("<?php\nabstract private function f() {}\n", 107).is_empty());
}

#[test]
fn abstract_body_flags_braced_abstract_methods() {
    let hits = for_rule(
        "<?php\nabstract class SomeClass {\n  abstract public function someMethod() {}\n}\n",
        108,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (3, 41));
    assert_eq!(
        hits[0].2,
        "`abstract` methods cannot contain a body. This construct will cause a fatal error."
    );
    assert!(
        for_rule(
            "<?php\nabstract class SomeClass {\n  abstract public function someMethod();\n}\n",
            108
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nabstract class SomeClass {\n  public function someMethod() {}\n}\n",
            108
        )
        .is_empty()
    );
}

#[test]
fn must_be_abstract_flags_concrete_class_with_abstract_methods() {
    let hits = for_rule(
        "<?php\nclass SomeClass {\n  abstract public function someMethod();\n}\n",
        113,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 1));
    assert_eq!(
        hits[0].2,
        "Class contains 1 abstract method(s) and must therefore be declared `abstract`."
    );
    assert!(
        for_rule(
            "<?php\nabstract class SomeClass {\n  abstract public function someMethod();\n}\n",
            113
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass SomeClass {\n  public function someMethod() {}\n}\n",
            113
        )
        .is_empty()
    );
    let hits = for_rule(
        "<?php\nclass C {\n  abstract public function f();\n  abstract protected function g();\n}\n",
        113,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Class contains 2 abstract method(s) and must therefore be declared `abstract`."
    );
    assert!(
        for_rule(
            "<?php\n$c = new class {\n  abstract public function f();\n};\n",
            113
        )
        .is_empty()
    );
}

#[test]
fn interface_body_flags_braced_methods() {
    let hits = for_rule(
        "<?php\n\ninterface SomeInterface {\n  public function someMethod() {}\n}\n",
        114,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 32));
    assert_eq!(
        hits[0].2,
        "`interface` methods cannot contain a body. This construct will cause a fatal error."
    );
    assert!(
        for_rule(
            "<?php\n\ninterface SomeInterface {\n  public function someMethod();\n}\n",
            114
        )
        .is_empty()
    );
    assert!(for_rule("<?php\nclass C {\n  public function f() {}\n}\n", 114).is_empty());
}

#[test]
fn interface_abstract_flags_abstract_methods() {
    let hits = for_rule(
        "<?php\n\ninterface SomeInterface {\n  abstract public function someMethod();\n}\n",
        118,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 3));
    assert_eq!(
        hits[0].2,
        "`interface` methods cannot be marked as `abstract`. This construct will cause a fatal \
         error."
    );
    assert!(
        for_rule(
            "<?php\n\ninterface SomeInterface {\n  public function someMethod();\n}\n",
            118
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nabstract class C {\n  abstract public function f();\n}\n",
            118
        )
        .is_empty()
    );
}

#[test]
fn continue_switch_flags_bare_continue_in_switch() {
    let src = "<?php\n\nswitch ($x) {\n  case 1:\n    continue;\n}\n\nswitch ($x) {\n  case 1:\n    continue /* keep */ ;\n}\n";
    let hits = for_rule(src, 128);
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].0, hits[0].1), (5, 5));
    assert_eq!((hits[1].0, hits[1].1), (10, 5));
    assert_eq!(
        hits[0].2,
        "In a \"switch\" statement, \"continue;\" is equivalent to \"break;\" but causes compile \
         errors beginning with PHP 7.0.0."
    );
}

#[test]
fn continue_switch_allows_loops_and_levels() {
    assert!(
        for_rule(
            "<?php\nfor ($x = 0; $x < 3; $x++) {\n  continue;\n}\n\nforeach ($x as $y) {\n  continue;\n}\n\nwhile ($x) {\n  continue;\n}\n\ndo {\n  continue;\n} while ($x);\n",
            128
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nwhile ($x) {\n  switch ($y) {\n    case 1:\n      continue 2;\n  }\n}\n",
            128
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nswitch ($x) {\n  case 1:\n    switch ($y) {\n      case 2:\n        continue 2;\n    }\n    break;\n}\n",
            128
        )
        .is_empty()
    );
}

#[test]
fn continue_switch_flags_inside_loop_nested_switches() {
    let hits = for_rule(
        "<?php\nwhile ($a) {\n  switch ($x) {\n    case 1:\n      continue;\n  }\n}\n",
        128,
    );
    assert_eq!(hits.len(), 1);
    let hits = for_rule(
        "<?php\nswitch ($x) {\n  case 1:\n    switch ($y) {\n      case 2:\n        continue;\n    }\n}\n",
        128,
    );
    assert_eq!(hits.len(), 2);
    assert!(for_rule("<?php\nswitch ($x) {\n  case 1:\n    break;\n}\n", 128).is_empty());
}

#[test]
fn continue_switch_covers_alternative_syntax() {
    let hits = for_rule(
        "<?php\nswitch ($x):\n  case 1:\n    continue;\nendswitch;\n",
        128,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 5));
    assert!(for_rule("<?php\nwhile ($x):\n  continue;\nendwhile;\n", 128).is_empty());
}

#[test]
fn public_property_flags_public_members() {
    let hits = for_rule(
        "<?php\n\nclass SomeClass {\n  private $x;\n  protected $y;\n  public $z;\n}\n",
        130,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (6, 3));
    assert_eq!(
        hits[0].2,
        "`public` properties should be avoided. Instead of exposing the property value directly, \
         consider using getter and setter methods."
    );
    assert!(
        for_rule(
            "<?php\nclass C {\n  var $x;\n  private $y;\n  protected $z;\n}\n",
            130
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\nclass C {\n  public function f() {}\n  public const X = 1;\n}\n",
            130
        )
        .is_empty()
    );
    assert_eq!(
        for_rule(
            "<?php\nclass C {\n  public static $x = 1, $y = 2;\n}\n",
            130
        )
        .len(),
        1
    );
    assert_eq!(
        for_rule("<?php\ntrait T {\n  public $x;\n}\n", 130).len(),
        1
    );
    assert!(for_rule("<?php\ninterface I {\n  public function f();\n}\n", 130).is_empty());
}

#[test]
fn partial_catch_flags_exception_without_throwable() {
    let hits = for_rule(
        "<?php\ntry {\n  f();\n} catch (Exception $ex) {\n  g();\n}\n",
        132,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (4, 10));
    assert_eq!(
        hits[0].2,
        "Try/catch block catches \"Exception\", but does not catch \"Throwable\". In PHP7 and \
         newer, some runtime exceptions will escape this block."
    );
    assert!(
        for_rule(
            "<?php\ntry {\n  f();\n} catch (Exception $ex) {\n  g();\n} catch (Throwable $ex) {\n  h();\n}\n",
            132
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\ntry {\n  f();\n} catch (\\Exception $ex) {\n  g();\n}\n",
            132
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\ntry {\n  f();\n} catch (Exception | Throwable $ex) {\n  g();\n}\n",
            132
        )
        .is_empty()
    );
    assert!(
        for_rule(
            "<?php\ntry {\n  f();\n} catch (RuntimeException $ex) {\n  g();\n}\n",
            132
        )
        .is_empty()
    );
    assert!(for_rule("<?php\ntry {\n  f();\n} finally {\n  g();\n}\n", 132).is_empty());
}

#[test]
fn partial_catch_reports_last_duplicate_and_scopes_per_try() {
    let hits = for_rule(
        "<?php\ntry {\n  f();\n} catch (Exception $a) {\n  g();\n} catch (Exception $b) {\n  h();\n}\n",
        132,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (6, 10));
    let hits = for_rule(
        "<?php\ntry {\n  f();\n} catch (Exception $a) {\n  g();\n} catch (Throwable $b) {\n  h();\n}\ntry {\n  f();\n} catch (Exception $c) {\n  g();\n}\n",
        132,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (11, 10));
}

#[test]
fn node_start_skips_a_balanced_subscript() {
    // A `]` directly before the class keyword walks back over its
    // matcher; rule 87 still reports at the class name.
    let hits = for_rule("<?php $a[0]\nclass C extends Phobject {}\n", 87);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 7));
}

#[test]
fn node_start_stops_at_an_unbalanced_bracket() {
    let hits = for_rule("<?php ]\nclass C extends Phobject {}\n", 87);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1), (2, 7));
}

#[test]
fn truncated_class_declaration_is_skipped_safely() {
    // `class` at EOF yields a nameless, bodyless block: rules 87/95
    // skip it, rule 113 has no body to scan, rule 88 still fires.
    assert!(for_rule("<?php class", 87).is_empty());
    assert!(for_rule("<?php class", 95).is_empty());
    assert!(for_rule("<?php class", 113).is_empty());
    assert_eq!(for_rule("<?php class", 88).len(), 1);
}

#[test]
fn docblock_marker_needs_word_boundaries() {
    // A glued marker neither exempts (rule 87 fires) nor matches.
    let hits = for_rule(
        "<?php\n/**x@concrete-extensible*/\nclass C extends Phobject {}\n",
        87,
    );
    assert_eq!(hits.len(), 1);
    // A rejected first hit falls through to a valid second marker.
    assert!(
        for_rule(
            "<?php\n/** x@concrete-extensible @concrete-extensible */\nclass C extends Phobject {}\n",
            87
        )
        .is_empty()
    );
}

#[test]
fn bodiless_constructor_has_no_returns_to_flag() {
    // A `__construct` terminated by `}` (no `;`/body) has no span.
    assert!(for_rule("<?php interface I { function __construct() }\n", 92).is_empty());
}

#[test]
fn closures_outside_the_constructor_do_not_mask_returns() {
    let hits = for_rule(
        "<?php class A extends Phobject { function __construct() { return 1; } function m() { $f = function () {}; } }\n",
        92,
    );
    assert_eq!(hits.len(), 1);
}

#[test]
fn truncated_return_counts_as_bare() {
    assert!(
        for_rule(
            "<?php class A extends Phobject { function __construct() { return",
            92
        )
        .is_empty()
    );
}

#[test]
fn continue_walk_crosses_function_boundaries() {
    let hits = for_rule(
        "<?php function f($x) { switch ($x) { case 1: continue; } }\n",
        128,
    );
    assert_eq!(hits.len(), 1);
}

#[test]
fn public_property_with_nested_brackets_still_flags() {
    let hits = for_rule(
        "<?php class A extends Phobject { public $a = [1, 2]; }\n",
        130,
    );
    assert_eq!(hits.len(), 1);
}

#[test]
fn paren_less_catch_has_no_names_to_collect() {
    assert!(for_rule("<?php try {} catch {}\n", 132).is_empty());
}

#[test]
fn glued_marker_at_end_of_doc_matches_nothing() {
    // The needle ending the doc with a glued prefix advances past the
    // end without matching (the scan's fallthrough arm).
    assert!(!super::has_concrete_extensible("x@concrete-extensible"));
    assert!(super::has_concrete_extensible(" @concrete-extensible "));
}
