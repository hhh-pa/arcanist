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
fn short_tag_flags_first_open_tag_only() {
    let hits = for_rule("<? echo $x;\n", 6);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 1);
    assert_eq!(hits[0].2, "Use the full form of the PHP open tag, `<?php`.");
    assert!(for_rule("<?php echo $x;\n", 6).is_empty());
    assert!(for_rule("<?= $x ?>\n", 6).is_empty());
}

#[test]
fn echo_tag_flags_every_occurrence() {
    assert_eq!(for_rule("<?= $x ?>\n", 7).len(), 1);
    assert_eq!(for_rule("<?= $x ?> <?= $y ?>\n", 7).len(), 2);
    assert!(for_rule("<?php echo $x;\n", 7).is_empty());
}

#[test]
fn close_tag_skipped_with_inline_html() {
    let hits = for_rule("<?php echo $x; ?>\n", 8);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Do not use the PHP closing tag, `?>`.");
    assert!(for_rule("<?php echo $x; ?> tail\n", 8).is_empty());
    assert!(for_rule("<?php echo $x;\n", 8).is_empty());
}

#[test]
fn open_tag_requires_php_first() {
    assert!(for_rule("<?php echo;\n", 15).is_empty());
    assert!(for_rule("#!/usr/bin/php\n<?php echo;\n", 15).is_empty());
    let hits = for_rule("  <?php echo;\n", 15);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "PHP files should start with `<?php`, which may be preceded by a `#!` line for scripts."
    );
}

#[test]
fn todo_comment_matches_per_occurrence() {
    let hits = for_rule("<?php // TODO: fix TODO again\n", 16);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].2, "This comment has a TODO.");
    assert!(for_rule("<?php // nothing here\n", 16).is_empty());
    assert_eq!(for_rule("<?php /** @todo x */\n", 16).len(), 1);
    assert!(for_rule("<?php /* @todo x */\n", 16).is_empty());
}

#[test]
fn comment_style_flags_hash_comments() {
    let hits = for_rule("<?php # hello\n", 18);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Use `//` single-line comments, not `#`.");
    assert!(for_rule("<?php // hello\n", 18).is_empty());
    assert!(for_rule("<?php #[\\ReturnTypeWillChange] function f() {}\n", 18).is_empty());
}

#[test]
fn comment_helpers_tolerate_unlexable_shapes() {
    // Defensive edges the tokenizer never produces: a `#[\...]`
    // comment (always lexed as an attribute) and a `/` that opens no
    // comment marker.
    let tree = ast::Tree {
        source: "#[\\x]".to_string(),
        tokens: vec![ast::Token {
            kind: ast::kinds::T_COMMENT.to_string(),
            value: "#[\\x]".to_string(),
            offset: 0,
        }],
        ..Default::default()
    };
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options: &ast::XhpOptions::default(),
    };
    let mut out = Vec::new();
    super::rule_comment_style(&ctx, &mut out);
    assert!(out.is_empty());
    assert!(!super::needs_comment_space("/ab"));
    assert!(!super::needs_comment_space("/"));
}

#[test]
fn elseif_usage_message_is_exact() {
    let hits = for_rule("<?php if ($a) {} elseif ($b) {}\n", 42);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Usage of `else if` is preferred over `elseif`.");
    assert!(for_rule("<?php if ($a) {} else if ($b) {}\n", 42).is_empty());
}

#[test]
fn logical_operators_flag_and_or_only() {
    assert_eq!(for_rule("<?php if ($a and $b) {}\n", 58).len(), 1);
    assert_eq!(for_rule("<?php if ($a or $b) {}\n", 58).len(), 1);
    assert!(for_rule("<?php if ($a && $b) {}\n", 58).is_empty());
    assert!(for_rule("<?php if ($a xor $b) {}\n", 58).is_empty());
    assert_eq!(
        for_rule("<?php if ($a and $b) {}\n", 58)[0].2,
        "Use `&&` instead of `and`."
    );
}

#[test]
fn inline_html_reports_first_block_only() {
    let hits = for_rule("<?php echo; ?> hello <?php echo; ?> world\n", 78);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "PHP files must only contain PHP code.");
    assert!(for_rule("#!/usr/bin/php\n<?php echo;\n", 78).is_empty());
    assert!(for_rule("<?php echo; ?>\n", 78).is_empty());
}

#[test]
fn newline_after_open_tag_needs_blank_line() {
    assert!(for_rule("<?php\n\n\necho;\n", 81).is_empty());
    assert!(for_rule("<?php ?>\n", 81).is_empty());
    assert_eq!(for_rule("<?php echo;\n", 81).len(), 1);
    assert_eq!(for_rule("<?php\necho;\n", 81).len(), 1);
    assert_eq!(
        for_rule("<?php echo;\n", 81)[0].2,
        "`<?php` should be separated from code by an empty line."
    );
}

#[test]
fn empty_file_fires_on_tags_only() {
    assert_eq!(for_rule("<?php\n", 82).len(), 1);
    assert_eq!(for_rule("<?php ?>\n", 82).len(), 1);
    assert!(for_rule("<?php echo;\n", 82).is_empty());
}

#[test]
fn curly_brace_index_points_at_object() {
    let hits = for_rule("<?php echo $x{'key'};\n", 119);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 1);
    assert_eq!(hits[0].2, "Use `$x['key']` instead of `$x{'key'}`.");
    assert!(for_rule("<?php echo $x['key'];\n", 119).is_empty());
    assert!(for_rule("<?php if ($x) { echo; }\n", 119).is_empty());
    assert!(for_rule("<?php class Foo { public $x; }\n", 119).is_empty());
}

#[test]
fn paren_spacing_hugs_both_sides() {
    assert_eq!(for_rule("<?php foo( $x );\n", 25).len(), 2);
    assert_eq!(for_rule("<?php foo( $x);\n", 25).len(), 1);
    assert!(for_rule("<?php foo($x );\n", 25).is_empty());
    assert!(for_rule("<?php foo($x);\n", 25).is_empty());
    assert_eq!(for_rule("<?php foo( );\n", 25).len(), 1);
    assert!(for_rule("<?php if (\n$x\n) {}\n", 25).is_empty());
}

#[test]
fn paren_spacing_covers_calls_and_skips_declares() {
    assert_eq!(for_rule("<?php $o->foo( $x );\n", 25).len(), 2);
    assert_eq!(for_rule("<?php new Foo( $x );\n", 25).len(), 2);
    assert!(for_rule("<?php declare( ticks=1 );\n", 25).is_empty());
    assert!(for_rule("<?php isset( $x );\n", 25).is_empty());
}

#[test]
fn control_spacing_wants_single_space() {
    assert!(for_rule("<?php if ($x) {}\n", 26).is_empty());
    let hits = for_rule("<?php if($x) {}\n", 26);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Convention: put a space after control statements."
    );
    let hits = for_rule("<?php if  ($x) {}\n", 26);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Convention: put a single space after control statements."
    );
    assert!(for_rule("<?php if ($x) {} else {}\n", 26).is_empty());
}

#[test]
fn index_spacing_flags_gaps() {
    assert_eq!(for_rule("<?php echo $x [0];\n", 28).len(), 1);
    assert!(for_rule("<?php echo $x[0];\n", 28).is_empty());
    assert!(for_rule("<?php $a = [1, 2];\n", 28).is_empty());
    assert_eq!(for_rule("<?php echo foo() [0];\n", 28).len(), 1);
}

#[test]
fn comment_spacing_needs_space() {
    assert_eq!(for_rule("<?php //x\n", 34).len(), 1);
    assert!(for_rule("<?php // x\n", 34).is_empty());
    assert_eq!(for_rule("<?php /*x*/\n", 34).len(), 1);
    assert!(for_rule("<?php /* x */\n", 34).is_empty());
    assert!(for_rule("<?php /** doc */\n", 34).is_empty());
}

#[test]
fn call_parens_hug_outer_and_inner() {
    assert_eq!(for_rule("<?php foo ($x);\n", 37).len(), 1);
    assert_eq!(for_rule("<?php foo($x );\n", 37).len(), 1);
    assert!(for_rule("<?php foo($x);\n", 37).is_empty());
    assert_eq!(for_rule("<?php $o->foo ($x);\n", 37).len(), 1);
    assert_eq!(for_rule("<?php array ($x);\n", 37).len(), 1);
    assert!(for_rule("<?php array($x );\n", 37).is_empty());
}

#[test]
fn semicolon_spacing_flags_space_before() {
    assert_eq!(for_rule("<?php echo $x ;\n", 43).len(), 1);
    assert!(for_rule("<?php echo $x;\n", 43).is_empty());
}

#[test]
fn concat_spacing_flags_single_line_gaps() {
    assert_eq!(for_rule("<?php $a . $b;\n", 44).len(), 2);
    assert!(for_rule("<?php $a.$b;\n", 44).is_empty());
    assert_eq!(for_rule("<?php $a\n. $b;\n", 44).len(), 1);
    assert!(for_rule("<?php $a\n.\n$b;\n", 44).is_empty());
    assert!(for_rule("<?php $a. /* c */ $b;\n", 44).is_empty());
}

#[test]
fn empty_block_flags_whitespace_interiors() {
    assert_eq!(for_rule("<?php if ($x) { }\n", 47).len(), 1);
    assert!(for_rule("<?php if ($x) {}\n", 47).is_empty());
    assert!(for_rule("<?php if ($x) { /* c */ }\n", 47).is_empty());
    assert!(for_rule("<?php class C {\n\n}\n", 47).is_empty());
}

#[test]
fn new_parens_required() {
    assert_eq!(for_rule("<?php $a = new Foo;\n", 49).len(), 1);
    assert!(for_rule("<?php $a = new Foo();\n", 49).is_empty());
    assert!(for_rule("<?php $a = new class {};\n", 49).is_empty());
    assert_eq!(for_rule("<?php $a = new $x;\n", 49).len(), 1);
}

#[test]
fn unnecessary_semicolon_flags_lone_semis() {
    assert_eq!(for_rule("<?php foo();;\n", 56).len(), 1);
    assert!(for_rule("<?php foo();\n", 56).is_empty());
    assert!(for_rule("<?php for ($i = 0; $i < 1; $i++) {}\n", 56).is_empty());
    assert!(for_rule("<?php declare(ticks=1);\n", 56).is_empty());
    assert_eq!(for_rule("<?php if ($x);\n", 56).len(), 1);
}

#[test]
fn cast_spacing_hugs_operand() {
    assert_eq!(for_rule("<?php $a = (int) $b;\n", 66).len(), 1);
    assert!(for_rule("<?php $a = (int)$b;\n", 66).is_empty());
    assert!(for_rule("<?php $a = (int )$b;\n", 66).is_empty());
    assert_eq!(for_rule("<?php $a = (INT) $b;\n", 66).len(), 1);
}

#[test]
fn object_op_spacing_hugs_with_newline_before_ok() {
    assert_eq!(for_rule("<?php $a -> $b;\n", 74).len(), 2);
    assert!(for_rule("<?php $a->$b;\n", 74).is_empty());
    assert!(for_rule("<?php $a\n->$b;\n", 74).is_empty());
    assert_eq!(for_rule("<?php $a->\n$b;\n", 74).len(), 1);
}

#[test]
fn paamayim_spacing_flags_each_side() {
    assert_eq!(for_rule("<?php A :: B;\n", 96).len(), 2);
    assert_eq!(for_rule("<?php A:: B;\n", 96).len(), 1);
    assert!(for_rule("<?php A::B;\n", 96).is_empty());
}

#[test]
fn ref_spacing_flags_gap_after_amp() {
    assert_eq!(for_rule("<?php function f(& $x) {}\n", 123).len(), 1);
    assert!(for_rule("<?php function f(&$x) {}\n", 123).is_empty());
    assert!(for_rule("<?php $a = $b & $c;\n", 123).is_empty());
    assert_eq!(for_rule("<?php foreach ($a as & $v) {}\n", 123).len(), 1);
}

#[test]
fn parent_ref_prefers_parent_keyword() {
    let src = "<?php class C extends P { function f() { P::foo(); } }\n";
    let hits = for_rule(src, 83);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Use `parent::` to call parent method.");
    assert!(
        for_rule(
            "<?php class C extends P { function f() { parent::foo(); } }\n",
            83
        )
        .is_empty()
    );
    assert!(for_rule("<?php class C { function f() { P::foo(); } }\n", 83).is_empty());
}

#[test]
fn binary_spacing_matches_php_fixture() {
    assert!(for_rule("<?php $a + $b;\n", 27).is_empty());
    assert_eq!(for_rule("<?php $a+$b;\n", 27).len(), 1);
    assert_eq!(for_rule("<?php $a +$b;\n", 27).len(), 1);
    assert!(for_rule("<?php $a = -$b;\n", 27).is_empty());
    assert_eq!(for_rule("<?php $a=-$b;\n", 27).len(), 1);
    assert_eq!(for_rule("<?php $a-=$b;\n", 27).len(), 1);
    assert_eq!(for_rule("<?php $a===$b;\n", 27).len(), 1);
    assert!(for_rule("<?php $a.$b;\n", 27).is_empty());
    assert_eq!(for_rule("<?php function x($n=null) {}\n", 27).len(), 1);
    assert!(for_rule("<?php $y /* ! */ += // ?\n$z;\n", 27).is_empty());
    assert_eq!(
        for_rule("<?php if ($x instanceof z &&$w) {}\n", 27).len(),
        1
    );
    assert_eq!(for_rule("<?php f(1,2);\n", 27).len(), 1);
    assert_eq!(
        for_rule("<?php $a+$b;\n", 27)[0].2,
        "Convention: logical and arithmetic operators should be surrounded by whitespace."
    );
    assert_eq!(
        for_rule("<?php f(1,2);\n", 27)[0].2,
        "Convention: comma should be followed by space."
    );
}

#[test]
fn binary_spacing_covers_arrows_assigns_and_declares() {
    assert_eq!(for_rule("<?php array($x=>$y);\n", 27).len(), 1);
    assert!(for_rule("<?php array($x => $y);\n", 27).is_empty());
    assert!(for_rule("<?php declare(strict_types=1);\n", 27).is_empty());
    assert!(for_rule("<?php $a = &$b;\n", 27).is_empty());
    assert_eq!(for_rule("<?php $a=$b;\n", 27).len(), 1);
    assert!(for_rule("<?php $a = $b;\n", 27).is_empty());
}

#[test]
fn binary_spacing_skips_declaration_equals() {
    assert!(for_rule("<?php class C { public $x=1; }\n", 27).is_empty());
    assert!(for_rule("<?php const X=1;\n", 27).is_empty());
    assert!(for_rule("<?php function f() { static $x=1; }\n", 27).is_empty());
    assert_eq!(for_rule("<?php $a=$b;\n", 27).len(), 1);
}

#[test]
fn decl_parens_distinguishes_anonymous() {
    assert_eq!(for_rule("<?php function foo ($x) {}\n", 38).len(), 1);
    assert!(for_rule("<?php function foo($x) {}\n", 38).is_empty());
    assert!(for_rule("<?php $f = function ($x) {};\n", 38).is_empty());
    assert_eq!(for_rule("<?php $f = function($x) {};\n", 38).len(), 1);
    assert_eq!(for_rule("<?php function foo($x ) {}\n", 38).len(), 1);
    assert_eq!(
        for_rule("<?php $f = function ($x)use ($y) {};\n", 38).len(),
        1
    );
    assert_eq!(
        for_rule("<?php $f = function ($x)use($y) {};\n", 38).len(),
        2
    );
    assert!(for_rule("<?php $f = function ($x) use ($y) {};\n", 38).is_empty());
}

#[test]
fn decl_paren_helpers_tolerate_missing_anchors() {
    // Defensive edges the parser never produces (a declaration paren
    // with no predecessor; a closure `use` span with no `use`
    // keyword before its paren).
    let tree = ast::tree_from_source("<?php function foo($x) {}\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    let mut out = Vec::new();
    super::decl_paren_leading(&tree, &blocks[0], 0, &mut out);
    assert!(out.is_empty());

    let tree = ast::tree_from_source("<?php $f = function ($x) use ($y) {};\n");
    let blocks = tree.blocks();
    let found = blocks.into_iter().find(|b| b.closure_use.is_some());
    let mut block = found.unwrap();
    let rparen = block.rparen.unwrap();
    block.closure_use = Some((rparen + 1, rparen + 1));
    let mut out = Vec::new();
    super::decl_use_spacing(&tree, &block, &mut out);
    assert!(out.is_empty());
}

#[test]
fn keyword_casing_flags_uppercase_keywords() {
    let hits = for_rule("<?php IF ($x) {}\n", 40);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Convention: spell keyword `IF` as `if`.");
    assert!(for_rule("<?php if ($x) {}\n", 40).is_empty());
    assert_eq!(for_rule("<?php $a = TRUE;\n", 40).len(), 1);
    assert_eq!(for_rule("<?php $a = true;\n", 40).len(), 0);
    assert!(for_rule("<?php TRUE();\n", 40).is_empty());
    assert_eq!(for_rule("<?php echo __file__;\n", 40).len(), 1);
    assert!(for_rule("<?php echo __FILE__;\n", 40).is_empty());
    assert_eq!(
        for_rule(
            "<?php class C extends P { function f() { SELF::x(); } }\n",
            40
        )
        .len(),
        1
    );
}

#[test]
fn construct_parens_flags_echo_and_include() {
    assert_eq!(for_rule("<?php echo($x);\n", 46).len(), 1);
    assert!(for_rule("<?php echo $x;\n", 46).is_empty());
    assert_eq!(for_rule("<?php include($f);\n", 46).len(), 1);
    assert!(for_rule("<?php include $f;\n", 46).is_empty());
    assert!(for_rule("<?php exit(1);\n", 46).is_empty());
    assert_eq!(
        for_rule("<?php echo($x);\n", 46)[0].2,
        "Language constructs do not require parentheses."
    );
}

#[test]
fn array_separator_enforces_trailing_commas() {
    assert!(for_rule("<?php array($x => $y);\n", 48).is_empty());
    assert_eq!(for_rule("<?php array($x, $y,);\n", 48).len(), 1);
    assert_eq!(
        for_rule("<?php array($x, $y,);\n", 48)[0].2,
        "Single lined arrays should not have a trailing comma."
    );
    assert_eq!(for_rule("<?php $a = [\n1,\n2\n];\n", 48).len(), 1);
    assert_eq!(
        for_rule("<?php $a = [\n1,\n2\n];\n", 48)[0].2,
        "Multi-lined arrays should have trailing commas."
    );
    assert!(for_rule("<?php $a = [\n1,\n2,\n];\n", 48).is_empty());
    assert_eq!(for_rule("<?php $a = [\n1,\n2, ];\n", 48).len(), 1);
    assert_eq!(
        for_rule("<?php $a = [\n1,\n2, ];\n", 48)[0].2,
        "Closing parenthesis should be on a new line."
    );
    assert!(for_rule("<?php $a = [];\n", 48).is_empty());
    assert!(for_rule("<?php echo $x[0];\n", 48).is_empty());
}

#[test]
fn modifier_ordering_flags_bad_order() {
    assert_eq!(
        for_rule("<?php class C { static public function f() {} }\n", 71).len(),
        1
    );
    assert!(for_rule("<?php class C { public static function f() {} }\n", 71).is_empty());
    assert_eq!(
        for_rule("<?php class C { static public $x; }\n", 71).len(),
        1
    );
    assert!(for_rule("<?php class C { public static $x; }\n", 71).is_empty());
    assert!(for_rule("<?php class C { var $x; }\n", 71).is_empty());
    assert_eq!(
        for_rule("<?php class C { static public function f() {} }\n", 71)[0].2,
        "Non-conventional modifier ordering."
    );
}

#[test]
fn unary_prefix_spacing_hugs_operand() {
    assert_eq!(for_rule("<?php $a = - $b;\n", 73).len(), 1);
    assert!(for_rule("<?php $a = -$b;\n", 73).is_empty());
    assert_eq!(for_rule("<?php $a = ! $b;\n", 73).len(), 1);
    assert_eq!(for_rule("<?php ++ $i;\n", 73).len(), 1);
    assert!(for_rule("<?php ++$i;\n", 73).is_empty());
    assert!(for_rule("<?php $a - $b;\n", 73).is_empty());
    assert_eq!(
        for_rule("<?php $a = - $b;\n", 73)[0].2,
        "Unary prefix operators should not be followed by whitespace."
    );
}

#[test]
fn unary_postfix_spacing_hugs_operand() {
    assert_eq!(for_rule("<?php $i ++;\n", 75).len(), 1);
    assert!(for_rule("<?php $i++;\n", 75).is_empty());
    assert!(for_rule("<?php ++$i;\n", 75).is_empty());
    assert_eq!(
        for_rule("<?php $i ++;\n", 75)[0].2,
        "Unary postfix operators should not be prefixed by whitespace."
    );
}

#[test]
fn hand_written_canned_json_parses() {
    let json = r#"{"source": "<?php if ($a) {} elseif ($b) {}",
      "tokens": [["T_OPEN_TAG", "<?php", 0], ["T_WHITESPACE", " ", 5],
        ["T_IF", "if", 6], ["T_WHITESPACE", " ", 8], ["(", "(", 9],
        ["T_VARIABLE", "$a", 10], [")", ")", 12], ["T_WHITESPACE", " ", 13],
        ["{", "{", 14], ["}", "}", 15], ["T_WHITESPACE", " ", 16],
        ["T_ELSEIF", "elseif", 17]]}"#;
    let tree = ast::tree_from_canned(json).expect("hand-written JSON must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options: &ast::XhpOptions::default(),
    };
    let mut out = Vec::new();
    super::apply(&ctx, &mut out);
    assert!(out.iter().any(|f| f.rule == 42));
}

// -( T1170 coverage: boundary tokens )----------------------------------------

#[test]
fn open_tag_skips_empty_files() {
    assert!(for_rule("", 15).is_empty());
}

#[test]
fn newline_after_open_tag_accepts_blank_line() {
    assert!(for_rule("<?php\n\necho;\n", 81).is_empty());
}

#[test]
fn curly_index_ignores_plain_blocks() {
    // `{` after `)` is a control block, not an index.
    assert!(for_rule("<?php if ($x) { echo; }\n", 119).is_empty());
    // A bare block has no indexable object before it.
    assert!(for_rule("<?php { echo; }\n", 119).is_empty());
    // Trait adaptation braces are legal.
    assert!(for_rule("<?php class C { use T { foo as bar; } }\n", 119).is_empty());
}

fn for_rule_json(tokens: &str, rule: u32) -> Vec<(usize, usize, String)> {
    // Degenerate token streams (a leading operator with no previous
    // token) never survive the real tokenizer, which emits
    // `T_INLINE_HTML` without an open tag; hand-written canned JSON
    // pins the defensive arms directly (as for rule 42 above).
    let json = format!(r#"{{"source": "xxxxxxxxxx", "tokens": [{tokens}]}}"#);
    let tree = ast::tree_from_canned(&json).expect("hand-written JSON must parse");
    let ctx = ast::RuleCtx {
        tree: &tree,
        path: "test.php",
        options: &ast::XhpOptions::default(),
    };
    let mut out = Vec::new();
    super::apply(&ctx, &mut out);
    out.iter()
        .filter(|f| f.rule == rule)
        .map(|f| {
            let (line, col) = tree.line_col(f.offset);
            (line, col, f.message.clone())
        })
        .collect()
}

#[test]
fn degenerate_streams_skip_missing_neighbors() {
    // A leading `{` has no previous token at all.
    assert!(for_rule_json(r#"["{", "{", 0]"#, 119).is_empty());
    // A leading `[` has no previous token at all.
    assert!(for_rule_json(r#"["[", "[", 0]"#, 28).is_empty());
    // A leading `T_STRING` has no previous token at all.
    assert!(for_rule_json(r#"["T_STRING", "foo", 0]"#, 37).is_empty());
    // A leading `++` has no previous token at all.
    assert!(for_rule_json(r#"["T_INC", "++", 0]"#, 75).is_empty());
    // A leading `->` has no previous token at all.
    assert!(
        for_rule_json(
            r#"["T_OBJECT_OPERATOR", "->", 0], ["T_STRING", "foo", 2]"#,
            74
        )
        .is_empty()
    );
    // A leading `&` before a variable is still adjacent.
    assert!(for_rule_json(r#"["&", "&", 0], ["T_VARIABLE", "$x", 1]"#, 123).is_empty());
}

#[test]
fn degenerate_streams_flag_lone_operators() {
    // A leading `;` is an unnecessary semicolon.
    assert_eq!(for_rule_json(r#"[";", ";", 0]"#, 56).len(), 1);
    // A leading `=` has no previous operand to compare gaps with.
    assert_eq!(for_rule_json(r#"["=", "=", 0]"#, 27).len(), 1);
}

#[test]
fn curly_index_flags_string_offsets() {
    let hits = for_rule("<?php $x = foo{'k'};\n", 119);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Use `$x['key']` instead of `$x{'key'}`.");
}

#[test]
fn paren_rules_tolerate_truncated_array() {
    // `array` at EOF (or not followed by parens) contributes no group.
    assert!(for_rule("<?php array", 25).is_empty());
    assert!(for_rule("<?php array;\n", 25).is_empty());
}

#[test]
fn new_parens_tolerates_truncated_new() {
    assert!(for_rule("<?php new", 49).is_empty());
    // A name at EOF still misses its constructor parens.
    assert_eq!(for_rule("<?php new Foo", 49).len(), 1);
}

#[test]
fn comment_spacing_ignores_bare_marker() {
    assert!(for_rule("<?php //\n", 34).is_empty());
}

#[test]
fn call_parens_tolerates_unclosed_call() {
    assert!(for_rule("<?php foo($x;\n", 37).is_empty());
}

#[test]
fn method_sites_tolerate_truncated_fetches() {
    // A method name at EOF has no paren to check.
    assert!(for_rule("<?php $x->foo", 37).is_empty());
    // A property fetch is not a call.
    assert!(for_rule("<?php $x->foo;\n", 37).is_empty());
}

#[test]
fn concat_spacing_handles_dot_near_eof() {
    let hits = for_rule("<?php $a .$b", 44);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Convention: no spaces around string concatenation operator."
    );
    // A trailing `.` has no right-hand operand to check.
    let hits = for_rule("<?php $a .", 44);
    assert_eq!(hits.len(), 1);
}

#[test]
fn empty_block_ignores_unclosed_brace() {
    assert!(for_rule("<?php if ($x) {", 47).is_empty());
}

#[test]
fn empty_block_exempts_alternate_switch() {
    assert!(for_rule("<?php switch ($x): case 1: break; endswitch;\n", 47).is_empty());
}

// -( T1170 coverage: offsets, refs, operators )--------------------------------

#[test]
fn curly_index_walks_back_over_subscripts() {
    let hits = for_rule("<?php $a[0]{'k'};\n", 119);
    assert_eq!(hits.len(), 1);
    // An unmatchable `]` stops the walk at the brace itself.
    assert_eq!(for_rule("<?php $a]{'k'};\n", 119).len(), 1);
}

#[test]
fn new_parens_ignores_non_names() {
    assert!(for_rule("<?php new 'x';\n", 49).is_empty());
}

#[test]
fn new_parens_flags_missing_constructor_call() {
    let hits = for_rule("<?php new Foo;\n", 49);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Use parentheses when invoking a constructor.");
    // A qualified name ends at the first non-name token.
    let hits = for_rule("<?php new\\Foo;\n", 49);
    assert_eq!(hits.len(), 1);
}

#[test]
fn unnecessary_semicolon_ignores_unclosed_for() {
    assert!(for_rule("<?php for ($i = 0;", 56).is_empty());
}

#[test]
fn cast_spacing_ignores_truncated_cast() {
    assert!(for_rule("<?php (in", 66).is_empty());
}

#[test]
fn object_op_spacing_tolerates_truncated_operator() {
    // A trailing `->` has no next token, so no gap to check.
    assert!(for_rule("<?php $x->", 74).is_empty());
}

#[test]
fn ref_spacing_ignores_non_references() {
    // `&` at EOF or before a non-variable is never a reference.
    assert!(for_rule("<?php $x = &", 123).is_empty());
    assert!(for_rule("<?php $a & 3;\n", 123).is_empty());
}

#[test]
fn parent_ref_ignores_braceless_classes() {
    assert!(for_rule("<?php class C extends P;\n", 83).is_empty());
}

#[test]
fn parent_ref_ignores_variable_receivers() {
    assert!(
        for_rule(
            "<?php class C extends P { function f() { $x::foo(); } }\n",
            83
        )
        .is_empty()
    );
}

#[test]
fn parent_ref_walks_qualified_names() {
    assert!(
        for_rule(
            "<?php class C extends P { function f() { Foo\\Bar::baz(); } }\n",
            83
        )
        .is_empty()
    );
}

fn for_rule_with_options(
    source: &str,
    rule: u32,
    options: &ast::XhpOptions,
) -> Vec<(usize, usize, String)> {
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
        .filter(|f| f.rule == rule)
        .map(|f| {
            let (line, col) = tree.line_col(f.offset);
            (line, col, f.message.clone())
        })
        .collect()
}

#[test]
fn parent_ref_keeps_old_php_closure_calls() {
    // Before PHP 5.4, `parent::` cannot spell a closure call, so the
    // old-version run stays silent while the default run flags it.
    let source = "<?php class C extends P { function f() { $o = new class { }; $g = function () { P::foo(); }; } }\n";
    let legacy = ast::XhpOptions {
        php_version: Some("5.3.0".to_string()),
        ..ast::XhpOptions::default()
    };
    assert!(for_rule_with_options(source, 83, &legacy).is_empty());
    assert_eq!(
        for_rule_with_options(source, 83, &ast::XhpOptions::default()).len(),
        1
    );
}

#[test]
fn binary_spacing_tolerates_unclosed_declare() {
    assert!(for_rule("<?php declare(ticks = 1;\n", 27).is_empty());
}

#[test]
fn binary_spacing_ignores_open_tag_equals() {
    // `=` after the open tag is not a declaration.
    assert!(for_rule("<?php = $b;\n", 27).is_empty());
}

#[test]
fn comma_spacing_ignores_trailing_comma() {
    assert!(for_rule("<?php foo($a,", 27).is_empty());
}

// -( T1170 coverage: declarations, arrays, modifiers )-------------------------

#[test]
fn decl_parens_flags_spaced_empty_params() {
    let hits = for_rule("<?php function foo( ) {}\n", 38);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Convention: no spaces before closing parenthesis in function and method declarations."
    );
}

#[test]
fn decl_parens_ignores_invoked_closure() {
    // An immediately-invoked closure's second paren group is not a
    // `use` list, so the `use` scan finds nothing.
    assert!(for_rule("<?php (function () {})();\n", 38).is_empty());
}

#[test]
fn keyword_casing_ignores_capitalized_names() {
    assert!(for_rule("<?php function True() {}\n", 40).is_empty());
}

#[test]
fn call_parens_tolerates_heredoc_argument() {
    assert!(for_rule("<?php foo(<<<EOT\ntext\nEOT\n);\n", 37).is_empty());
}

#[test]
fn call_parens_checks_multi_token_arguments() {
    let hits = for_rule("<?php foo($x + $y );\n", 37);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Convention: no spaces before closing parentheses."
    );
    let hits = for_rule("<?php foo( $x );\n", 37);
    assert_eq!(hits.len(), 1);
}

#[test]
fn array_separator_exempts_heredoc_values() {
    assert!(for_rule("<?php $a = [\n  <<<EOT\ntext\nEOT\n];\n", 48).is_empty());
}

#[test]
fn array_separator_scans_nested_last_values() {
    let hits = for_rule("<?php $a = [\n  \"k\" => foo(1)\n];\n", 48);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Multi-lined arrays should have trailing commas.");
    let hits = for_rule("<?php $a = [\n  \"k\" => $x\n];\n", 48);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "Multi-lined arrays should have trailing commas.");
}

#[test]
fn modifier_ordering_skips_attributes() {
    assert!(for_rule("<?php class C { #[Foo] public function bar() {} }\n", 71).is_empty());
}

#[test]
fn modifier_ordering_ignores_stray_bracket() {
    assert!(
        for_rule(
            "<?php class C { function a() {} ] public function b() {} }\n",
            71
        )
        .is_empty()
    );
}

#[test]
fn modifier_ordering_skips_matched_bracket() {
    assert!(
        for_rule(
            "<?php class C { function a() {} [0] public function b() {} }\n",
            71
        )
        .is_empty()
    );
}

#[test]
fn modifier_ordering_ignores_unknown_words() {
    // `var` is not a method modifier, so the run cannot be ordered.
    assert!(for_rule("<?php class C { var public function f() {} }\n", 71).is_empty());
}

#[test]
fn modifier_ordering_tolerates_stray_paren() {
    assert!(for_rule("<?php class C { public $x = BAR); }\n", 71).is_empty());
}

#[test]
fn modifier_ordering_ignores_bare_variables() {
    assert!(for_rule("<?php class C { $x = 1; }\n", 71).is_empty());
}

#[test]
fn modifier_ordering_stops_runs_at_keywords() {
    assert!(
        for_rule(
            "<?php class C { public function f() {} public $y = 2; }\n",
            71
        )
        .is_empty()
    );
}
