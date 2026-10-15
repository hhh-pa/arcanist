use super::*;

fn kinds_of(source: &str) -> Vec<String> {
    tokenize(source).iter().map(|t| t.kind.clone()).collect()
}

#[test]
fn tokenizes_open_tags_and_html() {
    let tokens = tokenize("hello <?= $x ?> world <?php echo; ?> tail <? echo;");
    let kinds = tokens.iter().map(|t| t.kind.as_str()).collect::<Vec<_>>();
    assert!(kinds.contains(&kinds::T_INLINE_HTML));
    assert!(kinds.contains(&kinds::T_OPEN_TAG_WITH_ECHO));
    assert!(kinds.contains(&kinds::T_CLOSE_TAG));
    assert!(kinds.contains(&kinds::T_OPEN_TAG));
    assert!(kinds.contains(&kinds::T_VARIABLE));
    assert!(kinds.contains(&kinds::T_ECHO));
    let open: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.kind == kinds::T_OPEN_TAG)
        .collect();
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].value, "<?php");
    assert_eq!(open[1].value, "<?");
}

#[test]
fn close_tag_consumes_one_newline() {
    let tokens = tokenize("<?php echo; ?>\nnext");
    let close = tokens.iter().find(|t| t.kind == kinds::T_CLOSE_TAG).unwrap();
    assert_eq!(close.value, "?>\n");
    let after: Vec<&Token> = tokens
        .iter()
        .skip_while(|t| t.kind != kinds::T_CLOSE_TAG)
        .skip(1)
        .collect();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].value, "next");
}

#[test]
fn tokenizes_comments_and_whitespace() {
    let tokens = tokenize("<?php // line\n# hash\n/* block */ /** doc */");
    let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
    assert!(kinds.contains(&kinds::T_WHITESPACE));
    assert!(kinds.contains(&kinds::T_DOC_COMMENT));
    let comments: Vec<&&str> = kinds.iter().filter(|k| **k == kinds::T_COMMENT).collect();
    assert_eq!(comments.len(), 3);
}

#[test]
fn tokenizes_attribute_not_comment() {
    let tokens = tokenize("<?php #[Foo] class A {}");
    assert!(tokens.iter().any(|t| t.kind == kinds::T_ATTRIBUTE));
    assert!(!tokens.iter().any(is_comment));
}

#[test]
fn tokenizes_strings_numbers_and_variables() {
    let tokens =
        tokenize("<?php $a = 'x'; $b = \"y\"; $c = \"hi $a\"; $d = 08; $e = 0x1F; $f = 1.5;");
    let kinds = kinds_of("<?php $a = 'x';");
    assert!(kinds.contains(&kinds::T_VARIABLE.to_string()));
    assert!(kinds.contains(&kinds::T_CONSTANT_ENCAPSED_STRING.to_string()));
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_INTERPOLATED_STRING)
    );
    let bad_octal: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.kind == kinds::T_LNUMBER && t.value == "08")
        .collect();
    assert_eq!(bad_octal.len(), 1);
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_DNUMBER && t.value == "1.5")
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_LNUMBER && t.value == "0x1F")
    );
}

#[test]
fn tokenizes_keywords_case_insensitively() {
    let tokens = tokenize("<?php IF ($x) { ECHO $y; } ELSEIF ($z) { DIE; }");
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_IF && t.value == "IF")
    );
    assert!(tokens.iter().any(|t| t.kind == kinds::T_ELSEIF));
    assert!(tokens.iter().any(|t| t.kind == kinds::T_ECHO));
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_EXIT && t.value == "DIE")
    );
}

#[test]
fn tokenizes_heredoc_and_nowdoc() {
    let source = "<?php $a = <<<EOT\nhello\nEOT;\n$b = <<<'NOW'\nplain\nNOW;\n";
    let tokens = tokenize(source);
    assert!(tokens.iter().any(|t| t.kind == kinds::T_HEREDOC));
    assert!(tokens.iter().any(|t| t.kind == kinds::T_NOWDOC));
    assert!(tokens.iter().any(|t| t.kind == ";" && t.offset > 0));
}

#[test]
fn tokenizes_operators() {
    let tokens = tokenize("<?php $a->b; $c::$d; $e === $f; $g <=> $h; $i ?? $j; $k ... $l;");
    let kinds: Vec<&str> = tokens.iter().map(|t| t.kind.as_str()).collect();
    for expected in [
        kinds::T_OBJECT_OPERATOR,
        kinds::T_PAAMAYIM_NEKUDOTAYIM,
        kinds::T_IS_IDENTICAL,
        kinds::T_SPACESHIP,
        kinds::T_COALESCE,
        kinds::T_ELLIPSIS,
    ] {
        assert!(kinds.contains(&expected), "{expected}");
    }
}

#[test]
fn float_member_dot_stays_in_number() {
    let tokens = tokenize("<?php $a = 1.5; $b = 1.foo(); $c = 1 . $d;\n");
    assert!(!tokens.iter().any(|t| t.kind == "." && t.value == ".foo"));
    let dots: Vec<&Token> = tokens.iter().filter(|t| t.kind == ".").collect();
    assert_eq!(dots.len(), 1);
    assert!(tokens.iter().any(|t| t.kind == kinds::T_DNUMBER && t.value == "1."));
}

#[test]
fn token_offsets_cover_source() {
    let source = "<?php foo($bar, 'baz');\n";
    let tokens = tokenize(source);
    let mut pos = 0;
    for token in &tokens {
        assert_eq!(token.offset, pos, "{}", token.value);
        pos += token.value.len();
    }
    assert_eq!(pos, source.len());
}

#[test]
fn line_col_counts_bytes_from_one() {
    let tree = tree_from_source("<?php\nfoo();\n");
    assert_eq!(tree.line_col(0), (1, 1));
    assert_eq!(tree.line_col(6), (2, 1));
    assert_eq!(tree.line_col(8), (2, 3));
    assert_eq!(tree.source_line(2), "foo();");
    assert_eq!(tree.source_line(99), "");
}

#[test]
fn semantic_navigation_skips_trivia() {
    let tree = tree_from_source("<?php foo( /* c */ $x );\n");
    let foo = tree.tokens.iter().position(|t| t.value == "foo").unwrap();
    let lparen = tree.next_semantic(foo).unwrap();
    assert_eq!(tree.tokens[lparen].kind, "(");
    assert_eq!(tree.prev_semantic(lparen), Some(foo));
    assert_eq!(tree.tokens[tree.next_semantic(lparen).unwrap()].value, "$x");
}

#[test]
fn bracket_matching_nests() {
    let tree = tree_from_source("<?php foo(bar(1), [2]);\n");
    let lparen = tree.tokens.iter().position(|t| t.kind == "(").unwrap();
    let close = tree.match_close(lparen).unwrap();
    assert_eq!(tree.tokens[close].kind, ")");
    assert_eq!(tree.match_open(close), Some(lparen));
    assert!(tree.match_close(close).is_none());
}

#[test]
fn gap_and_span_text_slice_source() {
    let tree = tree_from_source("<?php $a  =  1;\n");
    let var = tree.tokens.iter().position(|t| t.value == "$a").unwrap();
    let eq = tree.next_semantic(var).unwrap();
    assert_eq!(tree.gap_text(var, eq), "  ");
    assert_eq!(tree.span_text(var, eq), "$a  =");
}

#[test]
fn calls_skips_methods_statics_and_decls() {
    let tree = tree_from_source("<?php foo($a); $o->foo($b); A::foo($c); function foo($d) {}\n");
    let calls = tree.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "foo");
    assert_eq!(calls[0].args.len(), 1);
    assert_eq!(tree.arg_text(&calls[0], 0), "$a");
}

#[test]
fn calls_split_args_at_top_level() {
    let tree = tree_from_source("<?php foo($a, bar(1, 2), 'x');\n");
    let calls = tree.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "foo");
    assert_eq!(calls[0].args.len(), 3);
    assert_eq!(tree.arg_text(&calls[0], 1), "bar(1, 2)");
    assert_eq!(tree.arg_plain_string(&calls[0], 2).as_deref(), Some("x"));
    assert!(tree.arg_plain_string(&calls[0], 0).is_none());
}

#[test]
fn static_scalar_text_covers_scalars() {
    let tree = tree_from_source("<?php foo('a', 12, TRUE, $x);\n");
    let calls = tree.calls();
    assert_eq!(calls.len(), 1);
    let (a, _) = calls[0].args[0];
    assert_eq!(tree.static_scalar_text(a, a).as_deref(), Some("a"));
    let (b, _) = calls[0].args[1];
    assert_eq!(tree.static_scalar_text(b, b).as_deref(), Some("12"));
    let (c, _) = calls[0].args[2];
    assert_eq!(tree.static_scalar_text(c, c).as_deref(), Some("TRUE"));
    let (d, _) = calls[0].args[3];
    assert!(tree.static_scalar_text(d, d).is_none());
}

#[test]
fn unquote_handles_escapes() {
    assert_eq!(unquote_plain_string("'a\\'b'").as_deref(), Some("a'b"));
    assert_eq!(unquote_plain_string("'a\\\\b'").as_deref(), Some("a\\b"));
    assert_eq!(unquote_plain_string("'a\\nb'").as_deref(), Some("a\\nb"));
    assert_eq!(unquote_plain_string("\"a\\\"b\"").as_deref(), Some("a\"b"));
    assert!(unquote_plain_string("\"a$b\"").is_none());
    assert!(unquote_plain_string("nope").is_none());
}

#[test]
fn string_variables_finds_interpolations() {
    let vars = string_variables("\"hi $name and ${other} and {$third}!\"");
    let names: Vec<&str> = vars.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, vec!["name", "other", "third"]);
    assert!(string_variables("\"escaped \\$not\"").is_empty());
}

#[test]
fn canned_round_trip_preserves_tokens() {
    let source = "<?php foo($x);\n";
    let canned = tokens_to_canned(source, &tokenize(source));
    let tree = tree_from_canned(&canned).expect("round trip");
    assert_eq!(tree.source, source);
    assert_eq!(tree.tokens, tokenize(source));
}

#[test]
fn canned_accepts_object_tokens_and_rejects_garbage() {
    let json = r#"{"source": "<?php echo;",
      "tokens": [{"t": "T_OPEN_TAG", "v": "<?php", "o": 0}]}"#;
    let tree = tree_from_canned(json).expect("object form");
    assert_eq!(tree.tokens.len(), 1);
    assert!(tree_from_canned("not json").is_err());
    assert!(tree_from_canned("{}").is_err());
    assert!(tree_from_canned(r#"{"source": 1}"#).is_err());
    assert!(tree_from_canned(r#"{"source": "x", "tokens": [1]}"#).is_err());
    assert!(tree_from_canned(r#"{"source": "x", "nodes": [1]}"#).is_err());
}

#[test]
fn canned_without_tokens_tokenizes_locally() {
    let tree = tree_from_canned(r#"{"source": "<?php $a;"}"#).expect("local tokenize");
    assert!(tree.tokens.iter().any(|t| t.kind == kinds::T_VARIABLE));
}

#[test]
fn canned_nodes_are_queryable() {
    let json = r#"{"source": "<?php;",
      "tokens": [],
      "nodes": [{"t": "n_ROOT", "s": 0, "e": 6,
        "c": [{"t": "n_ECHO", "s": 6, "e": 10}]}]}"#;
    let tree = tree_from_canned(json).expect("nodes parse");
    assert_eq!(tree.nodes_of_type("n_ECHO").len(), 1);
    assert!(tree.nodes_of_type("n_MISSING").is_empty());
}

#[test]
fn version_compare_orders_dotted_versions() {
    use std::cmp::Ordering;
    assert_eq!(version_compare("5.3.0", "5.4.0"), Ordering::Less);
    assert_eq!(version_compare("5.4.0", "5.3.0"), Ordering::Greater);
    assert_eq!(version_compare("5.3.0", "5.3.0"), Ordering::Equal);
    assert_eq!(version_compare("5.10.0", "5.9.0"), Ordering::Greater);
    assert_eq!(version_compare("7.4", "7.4.0"), Ordering::Equal);
}

#[test]
fn keyword_table_is_sorted_and_complete() {
    let mut words: Vec<&str> = KEYWORDS.iter().map(|(w, _)| *w).collect();
    let sorted = {
        let mut copy = words.clone();
        copy.sort_unstable();
        copy
    };
    assert_eq!(words, sorted);
    words.dedup();
    assert_eq!(words.len(), KEYWORDS.len());
    for (word, kind) in KEYWORDS {
        assert_eq!(keyword_kind(word), Some(*kind));
    }
    assert_eq!(keyword_kind("notakeyword"), None);
    assert_eq!(keyword_kind("true"), None);
    assert_eq!(keyword_kind("self"), None);
}

#[test]
fn blocks_parse_if_else_chain() {
    let tree = tree_from_source("<?php if ($a) { foo(); } else { bar(); }\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].kind, BlockKind::If);
    assert!(blocks[0].lparen.is_some());
    assert!(matches!(blocks[0].body, BodySpan::Brace { .. }));
    assert_eq!(blocks[1].kind, BlockKind::Else);
    assert!(matches!(blocks[1].body, BodySpan::Brace { .. }));
}

#[test]
fn blocks_parse_functions_and_methods() {
    let source = "<?php class Foo { public static function bar($x = 1) { return $x; } }\n";
    let tree = tree_from_source(source);
    let blocks = tree.blocks();
    let class = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Class)
        .unwrap();
    assert_eq!(blocks[class].name.as_deref(), Some("Foo"));
    let func = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Function)
        .unwrap();
    assert_eq!(blocks[func].name.as_deref(), Some("bar"));
    assert!(tree.is_method(&blocks, func));
    assert_eq!(blocks[func].parent, Some(class));
    assert_eq!(blocks[func].params.len(), 1);
    assert_eq!(blocks[func].params[0].name, "$x");
    assert!(blocks[func].params[0].default.is_some());
    assert!(blocks[func].modifiers.contains(&"public".to_string()));
    assert!(blocks[func].modifiers.contains(&"static".to_string()));
}

#[test]
fn blocks_parse_switch_cases() {
    let tree = tree_from_source("<?php switch ($x) { case 1: foo(); break; default: bar(); }\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, BlockKind::Switch);
    assert_eq!(blocks[0].cases.len(), 2);
    assert!(!blocks[0].cases[0].is_default);
    assert!(blocks[0].cases[1].is_default);
    assert!(blocks[0].cases[0].colon.is_some());
    assert!(blocks[0].cases[0].body_first.is_some());
}

#[test]
fn blocks_link_try_catch_finally() {
    let tree = tree_from_source(
        "<?php try { foo(); } catch (Exception $e) { bar(); } finally { baz(); }\n",
    );
    let blocks = tree.blocks();
    let tr = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Try)
        .unwrap();
    assert_eq!(blocks[tr].linked.len(), 2);
    assert_eq!(blocks[blocks[tr].linked[0]].kind, BlockKind::Catch);
    assert_eq!(blocks[blocks[tr].linked[1]].kind, BlockKind::Finally);
}

#[test]
fn blocks_parse_do_while_tail() {
    let tree = tree_from_source("<?php do { foo(); } while ($x);\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, BlockKind::DoWhile);
    assert!(blocks[0].tail_while.is_some());
    assert!(blocks[0].tail_rparen.is_some());
}

#[test]
fn blocks_parse_alternative_syntax() {
    let tree = tree_from_source("<?php if ($a): foo(); else: bar(); endif;\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(blocks[0].body, BodySpan::Colon { .. }));
    assert!(matches!(blocks[1].body, BodySpan::Colon { .. }));
}

#[test]
fn blocks_parse_class_header() {
    let tree = tree_from_source("<?php abstract class Foo extends Bar implements A, B {}\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].name.as_deref(), Some("Foo"));
    assert_eq!(
        blocks[0].extends.as_ref().map(|(n, _)| n.as_str()),
        Some("Bar")
    );
    assert_eq!(blocks[0].implements.len(), 2);
    assert!(blocks[0].modifiers.contains(&"abstract".to_string()));
}
