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
    let close = tokens
        .iter()
        .find(|t| t.kind == kinds::T_CLOSE_TAG)
        .unwrap();
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
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_DNUMBER && t.value == "1.")
    );
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

// -( T1170 coverage: options, lexer edges, calls, blocks )---------------------

#[test]
fn t1170_options_read_all_maps_leniently() {
    use std::collections::HashMap;
    let mut map = HashMap::new();
    map.insert("xhpast.php-version".to_string(), serde_json::json!("7.4.0"));
    map.insert(
        "xhpast.php-version.windows".to_string(),
        serde_json::json!("8.0.0"),
    );
    map.insert(
        "xhpast.blacklisted.function".to_string(),
        serde_json::json!({"eval": "never", "junk": 7}),
    );
    map.insert(
        "xhpast.deprecated.functions".to_string(),
        serde_json::json!({"old": "use new"}),
    );
    map.insert(
        "xhpast.printf-functions".to_string(),
        serde_json::json!({"my_printf": 1, "other": "2", "junk": true}),
    );
    map.insert(
        "xhpast.dynamic-string.classes".to_string(),
        serde_json::json!({"C": 3}),
    );
    map.insert(
        "xhpast.dynamic-string.functions".to_string(),
        serde_json::json!({"f": "4"}),
    );
    map.insert("xhpast.switchhook".to_string(), serde_json::json!("Hook"));
    map.insert("xhpast.naminghook".to_string(), serde_json::json!(9));
    let options = XhpOptions::from_map(&map);
    assert_eq!(options.php_version.as_deref(), Some("7.4.0"));
    assert_eq!(options.php_version_windows.as_deref(), Some("8.0.0"));
    assert_eq!(
        options.blacklisted,
        vec![("eval".to_string(), "never".to_string())]
    );
    assert_eq!(
        options.deprecated,
        vec![("old".to_string(), "use new".to_string())]
    );
    assert_eq!(options.printf.len(), 2);
    assert!(options.printf.contains(&("my_printf".to_string(), 1)));
    assert!(options.printf.contains(&("other".to_string(), 2)));
    assert_eq!(options.dynamic_classes, vec![("C".to_string(), 3)]);
    assert_eq!(options.dynamic_functions, vec![("f".to_string(), 4)]);
    assert_eq!(options.switchhook.as_deref(), Some("Hook"));
    assert_eq!(options.naminghook, None);
    // Wrong-typed maps read as empty.
    let mut bad = HashMap::new();
    bad.insert(
        "xhpast.blacklisted.function".to_string(),
        serde_json::json!(["nope"]),
    );
    bad.insert("xhpast.printf-functions".to_string(), serde_json::json!(42));
    let options = XhpOptions::from_map(&bad);
    assert!(options.blacklisted.is_empty());
    assert!(options.printf.is_empty());
    assert!(XhpOptions::from_map(&HashMap::new()).blacklisted.is_empty());
}

#[test]
fn t1170_tree_slices_cover_edges() {
    let tree = tree_from_source("<?php\nfoo();\n");
    assert_eq!(tree.source_line(3), "");
    assert_eq!(tree.source_line(0), "");
    assert_eq!(tree.prev_semantic(0), None);
    assert_eq!(tree.gap_text(2, 1), "");
    assert_eq!(tree.gap_text(99, 100), "");
    assert_eq!(tree.span_text(2, 1), "");
    assert_eq!(tree.span_text(99, 100), "");
}

#[test]
fn t1170_token_at_offset_searches_both_directions() {
    let tree = tree_from_source("<?php foo($bar, 'baz');\n");
    let foo = tree.tokens.iter().position(|t| t.value == "foo").unwrap();
    assert_eq!(tree.token_at_offset(tree.tokens[foo].offset + 1), Some(foo));
    assert_eq!(tree.token_at_offset(tree.tokens[foo].offset), Some(foo));
    let last = tree.tokens.len() - 1;
    assert_eq!(tree.token_at_offset(tree.tokens[last].offset), Some(last));
    assert_eq!(tree.token_at_offset(tree.source.len() + 50), None);
    let empty = tree_from_canned(r#"{"source": "x", "tokens": []}"#).unwrap();
    assert_eq!(empty.token_at_offset(0), None);
}

#[test]
fn t1170_bracket_matching_covers_misses() {
    let tree = tree_from_source("<?php foo($a;\n");
    let lparen = tree.tokens.iter().position(|t| t.kind == "(").unwrap();
    assert_eq!(tree.match_close(lparen), None);
    assert_eq!(tree.match_open(0), None);
    assert_eq!(tree.match_open(tree.tokens.len()), None);
}

#[test]
fn t1170_tokenizes_backticks_and_crlf_close() {
    let tokens = tokenize("<?php $a = `ls -l`;\n");
    let tick = tokens.iter().find(|t| t.kind == kinds::T_BACKTICK).unwrap();
    assert_eq!(tick.value, "`ls -l`");
    let tokens = tokenize("<?php echo; ?>\r\nnext");
    let close = tokens
        .iter()
        .find(|t| t.kind == kinds::T_CLOSE_TAG)
        .unwrap();
    assert_eq!(close.value, "?>\r\n");
}

#[test]
fn t1170_shift_left_is_not_a_heredoc() {
    let tokens = tokenize("<?php $a << 2;\n");
    assert!(tokens.iter().any(|t| t.kind == kinds::T_SL));
    assert!(!tokens.iter().any(|t| t.kind == kinds::T_HEREDOC));
}

#[test]
fn t1170_unterminated_strings_run_to_end() {
    let tokens = tokenize("<?php $a = 'abc;");
    let lit = tokens
        .iter()
        .find(|t| t.kind == kinds::T_CONSTANT_ENCAPSED_STRING)
        .unwrap();
    assert_eq!(lit.value, "'abc;");
}

#[test]
fn t1170_dollar_followups_split_interpolation() {
    // `$` before a non-label stays plain; `{$` always interpolates.
    let tokens = tokenize("<?php $a = \"$5\";\n");
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_CONSTANT_ENCAPSED_STRING && t.value == "\"$5\"")
    );
    let tokens = tokenize("<?php $a = \"{$b}\";\n");
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == kinds::T_INTERPOLATED_STRING)
    );
}

#[test]
fn t1170_broken_heredoc_openers_fall_back_to_operators() {
    for source in [
        "<?php $a <<< 2;\n",
        "<?php $a = <<<EOT + 1;\n",
        "<?php $a = <<<'EOT\";\n",
    ] {
        let tokens = tokenize(source);
        assert!(tokens.iter().any(|t| t.kind == kinds::T_SL), "{source}");
    }
}

#[test]
fn t1170_heredoc_edges_scan_to_end_and_indent() {
    // No closing label: the literal runs to end of input.
    let tokens = tokenize("<?php $a = <<<EOT\nhello\n");
    assert_eq!(
        tokens.iter().filter(|t| t.kind == kinds::T_HEREDOC).count(),
        1
    );
    // An indented label still closes; a longer label-like line does not.
    let tokens = tokenize("<?php $a = <<<EOT\nEOTX\n  EOT;\n");
    let docs: Vec<&Token> = tokens
        .iter()
        .filter(|t| t.kind == kinds::T_HEREDOC)
        .collect();
    assert_eq!(docs.len(), 1);
    assert!(docs[0].value.contains("EOTX"));
}

#[test]
fn t1170_numbers_cover_exponents() {
    let tokens = tokenize("<?php $a = 1e5; $b = 1.5e-3; $c = 2E+4; $d = 1ex;\n");
    for (value, kind) in [
        ("1e5", kinds::T_DNUMBER),
        ("1.5e-3", kinds::T_DNUMBER),
        ("2E+4", kinds::T_DNUMBER),
        ("1", kinds::T_LNUMBER),
    ] {
        assert!(
            tokens.iter().any(|t| t.value == value && t.kind == kind),
            "{value} {kind}"
        );
    }
    assert!(
        tokens
            .iter()
            .any(|t| t.value == "ex" && t.kind == kinds::T_STRING)
    );
}

#[test]
fn t1170_unquote_covers_truncations_and_escapes() {
    assert_eq!(unquote_plain_string(""), None);
    assert_eq!(unquote_plain_string("x"), None);
    assert_eq!(unquote_plain_string("'abc\""), None);
    assert_eq!(unquote_plain_string("'ab\\'").as_deref(), Some("ab\\"));
    assert_eq!(unquote_plain_string("\"a\\qb\"").as_deref(), Some("a\\qb"));
}

#[test]
fn t1170_string_variables_reject_broken_forms() {
    assert!(string_variables("\"${name\"").is_empty());
    assert!(string_variables("\"${5}\"").is_empty());
    assert!(string_variables("\"$5\"").is_empty());
    let vars = string_variables("\"$ok${also_ok}\"");
    let names: Vec<&str> = vars.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, vec!["ok", "also_ok"]);
}

#[test]
fn t1170_canned_rejects_misshapen_tokens() {
    assert!(tree_from_canned("{\"source\": \"x\", \"tokens\": [[\"a\", \"b\"]]}").is_err());
}

#[test]
fn t1170_call_slices_cover_edges() {
    let tree = tree_from_source("<?php foo($a, bar(1, 2), 'x');\n");
    let calls = tree.calls();
    assert_eq!(tree.arg_text(&calls[0], 99), "");
    assert_eq!(tree.arg_plain_string(&calls[0], 99), None);
    assert_eq!(tree.arg_plain_string(&calls[0], 1), None);
    let (first, _) = calls[0].args[0];
    let (second, _) = calls[0].args[1];
    assert_eq!(tree.static_scalar_text(first, second), None);
}

#[test]
fn t1170_call_at_zero_has_no_exclusion() {
    let tree = tree_from_canned(
        "{\"source\": \"x\", \"tokens\": [[\"T_STRING\", \"foo\", 0], [\"(\", \"(\", 3], [\")\", \")\", 4]]}",
    )
    .unwrap();
    let calls = tree.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "foo");
}

#[test]
fn t1170_by_ref_and_qualified_calls_are_not_plain_calls() {
    // `function &foo()` declares; `A\foo()` qualifies; `\foo()`
    // still calls the global.
    let tree = tree_from_source("<?php function &foo($x) {} A\\foo(1); \\foo(2);\n");
    let calls = tree.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "foo");
    assert_eq!(tree.arg_text(&calls[0], 0), "2");
}

#[test]
fn t1170_body_first_covers_colon_and_missing() {
    let tree = tree_from_source("<?php if ($a): foo(); else: bar(); endif;\n");
    let blocks = tree.blocks();
    assert!(tree.body_first(&blocks[0]).is_some());
    let missing = tree_from_source("<?php if");
    let blocks = missing.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].body, BodySpan::Missing);
    assert_eq!(missing.body_first(&blocks[0]), None);
}

#[test]
fn t1170_method_checks_reject_non_functions() {
    let tree =
        tree_from_source("<?php class Foo { public static function bar($x = 1) { return $x; } }\n");
    let blocks = tree.blocks();
    let class = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Class)
        .unwrap();
    let func = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Function)
        .unwrap();
    assert!(!tree.is_method(&blocks, class));
    assert!(tree.is_method(&blocks, func));
    assert_eq!(tree.enclosing_class(&blocks, func), Some(class));
    assert_eq!(tree.enclosing_class(&blocks, class), None);
    // A closure nested in a method walks through the outer function.
    let nested =
        tree_from_source("<?php class A { function b() { $f = function ($x) { return $x; }; } }\n");
    let blocks = nested.blocks();
    let inner = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Function && b.is_anonymous)
        .unwrap();
    let outer_class = blocks
        .iter()
        .position(|b| b.kind == BlockKind::Class)
        .unwrap();
    assert_eq!(nested.enclosing_class(&blocks, inner), Some(outer_class));
    assert!(!nested.is_method(&blocks, inner));
}

#[test]
fn t1170_do_blocks_cover_broken_shapes() {
    // `do` at end of input, with a non-brace body, and with an
    // unclosed brace each parse leniently.
    for source in ["<?php do", "<?php do foo();", "<?php do {"] {
        let tree = tree_from_source(source);
        assert_eq!(tree.blocks().len(), 1, "{source}");
        assert_eq!(tree.blocks()[0].kind, BlockKind::DoWhile);
    }
    // A braced block without the trailing `while` keeps no tail.
    let tree = tree_from_source("<?php do { foo(); }\n");
    let blocks = tree.blocks();
    assert_eq!(blocks[0].tail_while, None);
    assert!(matches!(blocks[0].body, BodySpan::Brace { .. }));
}

#[test]
fn t1170_class_at_zero_is_a_declaration() {
    let tree = tree_from_canned(
        "{\"source\": \"x\", \"tokens\": [[\"T_CLASS\", \"class\", 0], \
         [\"T_STRING\", \"A\", 6], [\"{\", \"{\", 7], [\"}\", \"}\", 8]]}",
    )
    .unwrap();
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].name.as_deref(), Some("A"));
}

#[test]
fn t1170_paren_blocks_cover_bodiless_shapes() {
    // No parens: the body starts at the next token.
    let tree = tree_from_source("<?php if $x;\n");
    assert!(matches!(tree.blocks()[0].body, BodySpan::Single { .. }));
    // Parens but no body at all.
    let tree = tree_from_source("<?php if ($x)");
    assert_eq!(tree.blocks()[0].body, BodySpan::Missing);
}

#[test]
fn t1170_single_bodies_stop_at_close_and_tag() {
    let tree = tree_from_source("<?php { if ($a) foo() }\n");
    assert!(matches!(tree.blocks()[0].body, BodySpan::Single { .. }));
    let tree = tree_from_source("<?php if ($a) foo() ?>html");
    assert!(matches!(tree.blocks()[0].body, BodySpan::Single { .. }));
}

#[test]
fn t1170_colon_enders_nest_and_terminate() {
    // Nested same-opener colon blocks deepen the ender search.
    let tree = tree_from_source("<?php if ($a): if ($b): foo(); endif; endif;\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(blocks[0].body, BodySpan::Colon { .. }));
    // An unterminated colon block has no ender.
    let tree = tree_from_source("<?php if ($a): foo();\n");
    assert!(matches!(
        tree.blocks()[0].body,
        BodySpan::Colon { end: None, .. }
    ));
    // A nested opener at end of input is not colon form.
    let tree = tree_from_source("<?php if ($a): if");
    assert!(matches!(
        tree.blocks()[0].body,
        BodySpan::Colon { end: None, .. }
    ));
    // A paren-less nested opener still counts when followed by `:`.
    let tree = tree_from_source("<?php if ($a): if: foo(); endif; endif;\n");
    assert_eq!(tree.blocks().len(), 2);
}

#[test]
fn t1170_switch_cases_cover_bodiless_and_bracketed() {
    // A `;` body holds no cases.
    let tree = tree_from_source("<?php switch ($x);\n");
    assert!(tree.blocks()[0].cases.is_empty());
    // Brackets inside a case head nest the colon scan.
    let tree = tree_from_source("<?php switch ($x) { case foo(1): bar(); }\n");
    let blocks = tree.blocks();
    assert_eq!(blocks[0].cases.len(), 1);
    assert!(blocks[0].cases[0].colon.is_some());
    // A colon-less case has neither colon nor body.
    let tree = tree_from_source("<?php switch ($x) { case 1 }\n");
    let cases = &tree.blocks()[0].cases;
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].colon, None);
    assert_eq!(cases[0].body_first, None);
    // Trailing trivia before the boundary still finds the body end.
    let tree = tree_from_source("<?php switch ($x) { case 1: foo();   }\n");
    let cases = &tree.blocks()[0].cases;
    assert!(cases[0].body_first.is_some());
    assert!(cases[0].body_last.is_some());
}

#[test]
fn t1170_else_try_and_function_end_shapes() {
    let tree = tree_from_source("<?php if ($a) {} else");
    assert_eq!(tree.blocks()[1].body, BodySpan::Missing);
    let tree = tree_from_source("<?php try");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].body, BodySpan::Missing);
    assert!(blocks[0].linked.is_empty());
    // A named function without parens declares nothing.
    let tree = tree_from_source("<?php function foo;\n");
    assert!(tree.blocks().is_empty());
    // Parens without a body leave the body missing.
    let tree = tree_from_source("<?php function foo($x)");
    assert_eq!(tree.blocks()[0].body, BodySpan::Missing);
    // A non-name, non-paren head declares nothing.
    let tree = tree_from_source("<?php function 123;\n");
    assert!(tree.blocks().is_empty());
    // A `use` without a paren list keeps the bare keyword.
    let tree = tree_from_source("<?php $f = function ($x) use ;\n");
    assert_eq!(tree.blocks()[0].closure_use, None);
}

#[test]
fn t1170_functions_cover_by_ref_and_union_returns() {
    let tree = tree_from_source("<?php function &foo(int $x, string ...$rest): A|B {}\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].name.as_deref(), Some("foo"));
    assert_eq!(blocks[0].params.len(), 2);
    assert!(blocks[0].params[1].variadic);
    assert!(blocks[0].params[0].ty.is_some());
}

#[test]
fn t1170_params_skip_leading_trivia_in_type_range() {
    // Whitespace and comments between the opener (or comma) and the
    // type name are not part of the type range.
    let tree = tree_from_source("<?php function f( int $x, /*c*/ string $y) {}\n");
    let blocks = tree.blocks();
    assert_eq!(blocks[0].params.len(), 2);
    assert!(blocks[0].params[0].ty.is_some());
    assert!(blocks[0].params[1].ty.is_some());
}

#[test]
fn t1170_param_type_range_validates_trivia_led_ranges() {
    // Split ranges can start on trivia (the parser trims them, so
    // only a direct call exercises the skip): the range starts at
    // the first semantic token.
    let tree = Tree {
        source: " int $x".to_string(),
        tokens: vec![
            Token {
                kind: kinds::T_WHITESPACE.to_string(),
                value: " ".to_string(),
                offset: 0,
            },
            Token {
                kind: kinds::T_STRING.to_string(),
                value: "int".to_string(),
                offset: 1,
            },
            Token {
                kind: kinds::T_VARIABLE.to_string(),
                value: "$x".to_string(),
                offset: 5,
            },
        ],
        ..Default::default()
    };
    assert_eq!(param_ty_range(&tree, 0, 2), Some((1, 1)));
}

#[test]
fn t1170_arrow_functions_parse_bodies() {
    let tree = tree_from_source("<?php $f = fn($x) => foo($x) + 1;\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, BlockKind::ArrowFn);
    assert!(blocks[0].is_anonymous);
    assert_eq!(blocks[0].params.len(), 1);
    assert!(matches!(blocks[0].body, BodySpan::Single { .. }));
    // An arrow nested in a call stops at the closing paren.
    let tree = tree_from_source("<?php foo(fn($x) => $x);\n");
    assert!(matches!(tree.blocks()[0].body, BodySpan::Single { .. }));
    // A body running to end of input ends at the last token.
    let tree = tree_from_source("<?php $f = fn($x) => $x");
    assert!(matches!(tree.blocks()[0].body, BodySpan::Single { .. }));
}

#[test]
fn t1170_arrow_functions_cover_broken_shapes() {
    for source in [
        "<?php fn",
        "<?php fn foo;",
        "<?php $f = fn($x)",
        "<?php $f = fn($x) {}",
        "<?php $f = fn($x) =>",
    ] {
        let tree = tree_from_source(source);
        assert_eq!(tree.blocks().len(), 1, "{source}");
        assert_eq!(tree.blocks()[0].kind, BlockKind::ArrowFn);
    }
}

#[test]
fn t1170_class_headers_cover_anonymous_and_junk() {
    // Anonymous classes skip their constructor parens.
    let tree = tree_from_source("<?php $o = new class($x) {};\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].is_anonymous);
    let tree = tree_from_source("<?php $o = new class($");
    assert!(tree.blocks()[0].is_anonymous);
    // Stray header words either continue or stop the scan.
    let tree = tree_from_source("<?php class Foo Bar {}\n");
    assert_eq!(tree.blocks()[0].name.as_deref(), Some("Foo"));
    let tree = tree_from_source("<?php class Foo 123 {}\n");
    assert_eq!(tree.blocks()[0].name.as_deref(), Some("Foo"));
    // An extends at end of input still names the parent.
    let tree = tree_from_source("<?php class Foo extends Bar");
    assert_eq!(
        tree.blocks()[0].extends.as_ref().map(|(n, _)| n.as_str()),
        Some("Bar")
    );
    let tree = tree_from_source("<?php class");
    assert_eq!(tree.blocks()[0].body, BodySpan::Missing);
}

#[test]
fn t1170_namespaces_cover_bodiless_and_braced() {
    let tree = tree_from_source("<?php namespace");
    assert_eq!(tree.blocks()[0].body, BodySpan::Missing);
    let tree = tree_from_source("<?php namespace Foo");
    assert_eq!(tree.blocks()[0].name.as_deref(), Some("Foo"));
    let tree = tree_from_source("<?php namespace { foo(); }\n");
    let blocks = tree.blocks();
    assert_eq!(blocks[0].name, None);
    assert!(matches!(blocks[0].body, BodySpan::Brace { .. }));
}

#[test]
fn t1170_modifiers_skip_bracketed_prefixes() {
    let tree = tree_from_source("<?php $a[0] function foo() {}\n");
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].modifiers.is_empty());
}

#[test]
fn t1170_params_cover_trailing_equals() {
    let tree = tree_from_source("<?php function foo($x =) {}\n");
    let params = &tree.blocks()[0].params;
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].default, None);
}

#[test]
fn t1170_colon_enders_cover_loops_and_declare() {
    for (source, kind) in [
        ("<?php for ($i = 0;;): foo(); endfor;\n", BlockKind::For),
        (
            "<?php foreach ($a as $b): foo(); endforeach;\n",
            BlockKind::Foreach,
        ),
        (
            "<?php declare (ticks=1): foo(); enddeclare;\n",
            BlockKind::Declare,
        ),
    ] {
        let tree = tree_from_source(source);
        assert_eq!(tree.blocks().len(), 1, "{source}");
        assert_eq!(tree.blocks()[0].kind, kind);
        assert!(matches!(tree.blocks()[0].body, BodySpan::Colon { .. }));
    }
}

#[test]
fn t1170_kind_tables_cover_all_variants() {
    // Every block kind resolves an ender pair (or none) and a colon
    // answer; the map itself is the contract.
    for kind in [
        BlockKind::If,
        BlockKind::ElseIf,
        BlockKind::Else,
        BlockKind::While,
        BlockKind::DoWhile,
        BlockKind::For,
        BlockKind::Foreach,
        BlockKind::Switch,
        BlockKind::Try,
        BlockKind::Catch,
        BlockKind::Finally,
        BlockKind::Function,
        BlockKind::ArrowFn,
        BlockKind::Class,
        BlockKind::Interface,
        BlockKind::Trait,
        BlockKind::Enum,
        BlockKind::Declare,
        BlockKind::Namespace,
    ] {
        let _ = ender_pair(kind);
        let _ = takes_colon(kind);
    }
    assert!(ender_pair(BlockKind::If).is_some());
    assert_eq!(ender_pair(BlockKind::Try), None);
    assert!(takes_colon(BlockKind::While));
    assert!(!takes_colon(BlockKind::Try));
    // A `catch` colon still parses through the non-colon path.
    let tree = tree_from_source("<?php try {} catch (Exception $e): foo();\n");
    assert_eq!(tree.blocks().len(), 2);
}

#[test]
fn t1170_try_links_skip_nested_handlers() {
    // A `catch` inside the `try` body belongs to the inner block;
    // only the outer handler links to the outer `try`.
    let tree = tree_from_source(
        "<?php try { try { foo(); } catch (Exception $e) { bar(); } } \
         catch (Exception $e) { baz(); }\n",
    );
    let blocks = tree.blocks();
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0].kind, BlockKind::Try);
    assert_eq!(blocks[0].linked.len(), 1);
    assert_eq!(blocks[blocks[0].linked[0]].kind, BlockKind::Catch);
    assert!(blocks[1].linked.contains(&2));
}
