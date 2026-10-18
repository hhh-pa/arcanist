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

/// Run the rules over a hand-written canned-JSON blob.
fn check_blob(canned: &str) -> Vec<(u32, usize, usize, String)> {
    let tree = ast::tree_from_canned(canned).expect("canned output must parse");
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
fn clean_file_has_no_findings() {
    assert!(check("<?php echo 1;\n").is_empty());
}

#[test]
fn canned_blob_literal_path_reports_extract() {
    let findings = check_blob("{\"source\": \"<?php extract($x);\\n\"}");
    assert_eq!(
        findings,
        vec![(
            4,
            1,
            7,
            "Avoid `extract`. It is confusing and hinders static analysis.".to_string(),
        )],
    );
}

#[test]
fn extract_case_insensitive_but_not_method_call() {
    // Uppercase `EXTRACT` also trips the lowercase-builtin rule, as in PHP.
    assert_eq!(
        check("<?php EXTRACT($x);\n"),
        vec![
            (
                4,
                1,
                7,
                "Avoid `extract`. It is confusing and hinders static analysis.".to_string(),
            ),
            (
                61,
                1,
                7,
                "Calls to built-in PHP functions should be lowercase.".to_string(),
            ),
        ],
    );
    assert!(check("<?php $o->extract($x);\n").is_empty());
}

#[test]
fn dynamic_define_requires_string_literal() {
    assert_eq!(
        check("<?php define($name, 1);\n"),
        vec![(
            12,
            1,
            14,
            "First argument to `define` must be a string literal.".to_string(),
        )],
    );
    assert!(check("<?php define('NAME', 1);\n").is_empty());
    // Uppercase name still parses as `define` (no XHP12 on a literal) but
    // trips the lowercase-builtin rule.
    assert_eq!(
        check("<?php DEFINE('NAME', 1);\n"),
        vec![(
            61,
            1,
            7,
            "Calls to built-in PHP functions should be lowercase.".to_string(),
        )],
    );
}

#[test]
fn plus_on_string_literal() {
    assert_eq!(
        check("<?php $x = 'a' + $b;\n"),
        vec![(
            21,
            1,
            16,
            "In PHP, `.` is the string concatenation operator, not `+`. \
             This expression uses `+` with a string literal as an operand."
                .to_string(),
        )],
    );
    // The double-quoted operand also trips the double-quote rule, as in PHP.
    assert_eq!(
        check("<?php $x = $a + \"b\";\n"),
        vec![
            (
                21,
                1,
                15,
                "In PHP, `.` is the string concatenation operator, not `+`. \
                 This expression uses `+` with a string literal as an operand."
                    .to_string(),
            ),
            (
                41,
                1,
                17,
                "String does not require double quotes. For consistency, \
                 prefer single quotes."
                    .to_string(),
            ),
        ],
    );
    assert!(check("<?php $x = $a + $b;\n").is_empty());
    assert!(check("<?php $x = 'a' . $b;\n").is_empty());
    assert!(check("<?php $x = +'a';\n").is_empty());
}

#[test]
fn slowness_strstr_against_false() {
    assert_eq!(
        check("<?php if (false === strstr($a, 'b')) {}\n"),
        vec![(
            36,
            1,
            21,
            "Use `strpos` for checking if the string contains something.".to_string(),
        )],
    );
    // `strchr` is also an alias function, so both rules fire, as in PHP.
    assert_eq!(
        check("<?php if (strchr($a, 'b') !== false) {}\n"),
        vec![
            (
                36,
                1,
                11,
                "Use `strpos` for checking if the string contains something.".to_string(),
            ),
            (65, 1, 11, "Alias functions should be avoided.".to_string(),),
        ],
    );
    assert_eq!(
        check("<?php if (false === stristr($a, 'b')) {}\n"),
        vec![(
            36,
            1,
            21,
            "Use `stripos` for checking if the string contains something.".to_string(),
        )],
    );
    // Loose comparison and uppercase FALSE do not trigger (PHP-exact).
    assert!(check("<?php if (false == strstr($a, 'b')) {}\n").is_empty());
    assert!(check("<?php if (FALSE === strstr($a, 'b')) {}\n").is_empty());
}

#[test]
fn slowness_strpos_against_zero() {
    assert_eq!(
        check("<?php if (strpos($a, 'b') !== 0) {}\n"),
        vec![(
            36,
            1,
            11,
            "Use `strncmp` for checking if the string starts with something.".to_string(),
        )],
    );
    assert_eq!(
        check("<?php if (0 === stripos($a, 'b')) {}\n"),
        vec![(
            36,
            1,
            17,
            "Use `strncasecmp` for checking if the string starts with something.".to_string(),
        )],
    );
    assert!(check("<?php if (strpos($a, 'b') != 0) {}\n").is_empty());
}

#[test]
fn double_quote_plain_string() {
    assert_eq!(
        check("<?php $x = \"abc\";\n"),
        vec![(
            41,
            1,
            12,
            "String does not require double quotes. For consistency, \
             prefer single quotes."
                .to_string(),
        )],
    );
    assert!(check("<?php $x = 'abc';\n").is_empty());
    assert!(check("<?php $x = \"a\\n\";\n").is_empty());
    assert!(check("<?php $x = \"a$b\";\n").is_empty());
}

#[test]
fn double_quote_concat_group_shares_validity() {
    // Lone plain member of a concatenation is still reported.
    assert_eq!(check("<?php $x = \"a\" . $y;\n").len(), 1,);
    // An interpolated member validates the whole group.
    assert!(check("<?php $x = \"a\" . \"$y\";\n").is_empty());
    // An escape-bearing member validates the whole group.
    assert!(check("<?php $x = \"a\" . \"b\\t\";\n").is_empty());
}

#[test]
fn blacklisted_function_uses_option_message() {
    let options = ast::XhpOptions {
        blacklisted: vec![("myfunc".to_string(), "Do not use myfunc().".to_string())],
        ..ast::XhpOptions::default()
    };
    assert_eq!(
        check_with_options("<?php myfunc(1);\n", &options),
        vec![(51, 1, 7, "Do not use myfunc().".to_string())],
    );
    // Exact-case match only, like PHP `idx()`.
    assert!(check_with_options("<?php MyFunc(1);\n", &options).is_empty());
    assert!(check("<?php myfunc(1);\n").is_empty());
}

#[test]
fn call_time_pass_by_reference() {
    assert_eq!(
        check("<?php foo(&$x);\n"),
        vec![(
            53,
            1,
            11,
            "Call-time pass-by-reference calls are prohibited.".to_string(),
        )],
    );
    assert_eq!(check("<?php $o->foo($a, &$b);\n").len(), 1,);
    // Declarations and binary `&` are not call-time passing.
    assert!(check("<?php function foo(&$x) {}\n").is_empty());
    assert!(check("<?php foo($a & $b);\n").is_empty());
    assert!(check("<?php foo($x);\n").is_empty());
}

#[test]
fn formatted_string_missing_format() {
    assert_eq!(
        check("<?php printf();\n"),
        vec![(
            54,
            1,
            7,
            "This function is expected to have a format string.".to_string(),
        )],
    );
    assert!(check("<?php printf('%s', $x);\n").is_empty());
    assert_eq!(check("<?php fprintf($fh);\n").len(), 1);
    assert!(check("<?php fprintf($fh, '%s');\n").is_empty());
    // Custom printf-style function via configuration.
    let options = ast::XhpOptions {
        printf: vec![("myfmt".to_string(), 0)],
        ..ast::XhpOptions::default()
    };
    assert_eq!(check_with_options("<?php myfmt();\n", &options).len(), 1,);
    assert!(check_with_options("<?php myfmt('%s');\n", &options).is_empty());
}

#[test]
fn lowercase_builtin_functions() {
    assert_eq!(
        check("<?php STRLEN($x);\n"),
        vec![(
            61,
            1,
            7,
            "Calls to built-in PHP functions should be lowercase.".to_string(),
        )],
    );
    assert!(check("<?php strlen($x);\n").is_empty());
    assert!(check("<?php MyFunc($x);\n").is_empty());
}

#[test]
fn class_name_literal_prefers_class_constant() {
    assert_eq!(
        check("<?php class Foo { public function m() { return 'Foo'; } }\n"),
        vec![(
            62,
            1,
            48,
            "Prefer \"__CLASS__\" over hard-coded class names.".to_string(),
        )],
    );
    assert!(check("<?php class Foo { public function m() { return 'Bar'; } }\n").is_empty());
    assert!(check("<?php class Foo { public function m() { return 'FooBar'; } }\n").is_empty());
}

#[test]
fn alias_functions_case_insensitive() {
    assert_eq!(
        check("<?php sizeof($x);\n"),
        vec![(65, 1, 7, "Alias functions should be avoided.".to_string())],
    );
    // Uppercase alias also trips the lowercase-builtin rule, as in PHP.
    assert_eq!(
        check("<?php SIZEOF($x);\n"),
        vec![
            (
                61,
                1,
                7,
                "Calls to built-in PHP functions should be lowercase.".to_string(),
            ),
            (65, 1, 7, "Alias functions should be avoided.".to_string()),
        ],
    );
    assert!(check("<?php count($x);\n").is_empty());
}

#[test]
fn lambda_func_declaration() {
    assert_eq!(
        check("<?php function __lambda_func() {}\n"),
        vec![(
            68,
            1,
            7,
            "Declaring a function named `__lambda_func` causes any call to \
             create_function to fail. This is because `create_function` \
             eval-declares the function `__lambda_func`, then modifies the \
             symbol table so that the function is instead named \
             `\"\\0lambda_\".(++$i)`, and returns that name."
                .to_string(),
        )],
    );
    // Methods and other casings do not trigger.
    assert!(check("<?php class A { function __lambda_func() {} }\n").is_empty());
    assert!(check("<?php function __LAMBDA_FUNC() {}\n").is_empty());
}

#[test]
fn parse_str_needs_second_parameter() {
    assert_eq!(
        check("<?php parse_str($s);\n"),
        vec![(
            80,
            1,
            7,
            "Avoid `parse_str` unless the second parameter is specified. \
             It is confusing and hinders static analysis."
                .to_string(),
        )],
    );
    assert!(check("<?php parse_str($s, $out);\n").is_empty());
}

#[test]
fn array_combine_identical_arguments() {
    assert_eq!(
        check("<?php array_combine($x, $x);\n"),
        vec![(
            84,
            1,
            7,
            "Prior to PHP 5.4, `array_combine()` fails when given empty arrays. \
             Prefer to write `array_combine($x, $x)` as `array_fuse($x)`."
                .to_string(),
        )],
    );
    assert!(check("<?php array_combine($x, $y);\n").is_empty());
    assert!(check("<?php array_combine($x);\n").is_empty());
}

#[test]
fn deprecation_uses_option_message() {
    let options = ast::XhpOptions {
        deprecated: vec![("oldfunc".to_string(), "Use newfunc() instead.".to_string())],
        ..ast::XhpOptions::default()
    };
    assert_eq!(
        check_with_options("<?php oldfunc(1);\n", &options),
        vec![(85, 1, 7, "Use newfunc() instead.".to_string())],
    );
    // Case-insensitive call match, mirroring `getFunctionCalls()`.
    assert_eq!(check_with_options("<?php OLDFUNC(1);\n", &options).len(), 1,);
    // Empty messages are skipped like PHP `empty()`.
    let empty = ast::XhpOptions {
        deprecated: vec![("oldfunc".to_string(), String::new())],
        ..ast::XhpOptions::default()
    };
    assert!(check_with_options("<?php oldfunc(1);\n", &empty).is_empty());
}

#[test]
fn unsafe_dynamic_string_functions() {
    let options = ast::XhpOptions {
        dynamic_functions: vec![("query".to_string(), 0)],
        ..ast::XhpOptions::default()
    };
    assert_eq!(
        check_with_options("<?php query($sql);\n", &options),
        vec![(
            86,
            1,
            7,
            "Parameter 1 of `query` should be a scalar string, \
             otherwise it's not safe."
                .to_string(),
        )],
    );
    assert!(check_with_options("<?php query('SELECT 1');\n", &options).is_empty());
    assert!(check_with_options("<?php query();\n", &options).is_empty());
}

#[test]
fn unsafe_dynamic_string_classes() {
    let options = ast::XhpOptions {
        dynamic_classes: vec![("sql".to_string(), 0)],
        ..ast::XhpOptions::default()
    };
    assert_eq!(
        check_with_options("<?php new Sql($q);\n", &options),
        vec![(
            86,
            1,
            7,
            "Parameter 1 of `Sql` should be a scalar string, \
             otherwise it's not safe."
                .to_string(),
        )],
    );
    assert!(check_with_options("<?php new Sql('x');\n", &options).is_empty());
}

#[test]
fn cast_function_calls() {
    assert_eq!(
        check("<?php intval($x);\n"),
        vec![(
            105,
            1,
            7,
            "For consistency, use `(int)` (a type cast) instead of `intval` \
             (a function call). Function calls impose additional overhead."
                .to_string(),
        )],
    );
    // Uppercase `INTVAL` also trips the lowercase-builtin rule, as in PHP.
    assert_eq!(
        check("<?php INTVAL($x);\n"),
        vec![
            (
                61,
                1,
                7,
                "Calls to built-in PHP functions should be lowercase.".to_string(),
            ),
            (
                105,
                1,
                7,
                "For consistency, use `(int)` (a type cast) instead of `INTVAL` \
                 (a function call). Function calls impose additional overhead."
                    .to_string(),
            ),
        ],
    );
    assert!(check("<?php intval($x, 8);\n").is_empty());
    assert_eq!(check("<?php intval($x, 10);\n").len(), 1);
    // PHP `break` quirk: a non-numeric base stops the whole rule scan.
    assert!(check("<?php intval($x, $b); strval($y);\n").is_empty());
}

#[test]
fn is_a_should_be_instanceof() {
    assert_eq!(
        check("<?php is_a($o, 'Foo');\n"),
        vec![(
            111,
            1,
            7,
            "Use `instanceof` instead of `is_a`. The former is a language \
             construct whereas the latter is a function call, which has \
             additional overhead."
                .to_string(),
        )],
    );
    assert!(check("<?php is_a($o, 'Foo', true);\n").is_empty());
    assert_eq!(check("<?php is_a($o, 'Foo', false);\n").len(), 1);
    assert!(check("<?php is_a($o);\n").is_empty());
}

#[test]
fn invalid_octal_scalars() {
    assert_eq!(
        check("<?php $x = 08;\n"),
        vec![(
            125,
            1,
            12,
            "Invalid octal numeric scalar. `08` is not a valid octal and \
             will be interpreted as `0`."
                .to_string(),
        )],
    );
    assert_eq!(
        check("<?php $x = 018;\n")[0].3,
        "Invalid octal numeric scalar. `018` is not a valid octal and \
         will be interpreted as `1`."
            .to_string(),
    );
    assert!(check("<?php $x = 077;\n").is_empty());
    assert!(check("<?php $x = 8;\n").is_empty());
}

#[test]
fn hex_casing_must_be_uppercase() {
    assert_eq!(
        check("<?php $x = 0xff;\n"),
        vec![(
            127,
            1,
            12,
            "For consistency, write hexadecimals integers in uppercase with a \
             leading `0x`."
                .to_string(),
        )],
    );
    assert_eq!(check("<?php $x = 0XFF;\n").len(), 1);
    assert!(check("<?php $x = 0xFF;\n").is_empty());
}

#[test]
fn implode_glue_first() {
    assert_eq!(
        check("<?php implode($arr, ',');\n"),
        vec![(
            129,
            1,
            7,
            "When calling \"implode()\", pass the \"glue\" argument first. (The \
             other parameter order is deprecated in PHP 7.4 and raises a \
             warning.)"
                .to_string(),
        )],
    );
    assert!(check("<?php implode(',', $arr);\n").is_empty());
    assert_eq!(
        check("<?php implode($arr, DIRECTORY_SEPARATOR);\n").len(),
        1
    );
    assert!(check("<?php implode($a);\n").is_empty());
}

#[test]
fn binary_casing_must_be_lowercase_prefix() {
    assert_eq!(
        check("<?php $x = 0B101;\n"),
        vec![(
            131,
            1,
            12,
            "For consistency, write binary integers with a leading `0b`.".to_string(),
        )],
    );
    assert!(check("<?php $x = 0b101;\n").is_empty());
}

#[test]
fn each_use_is_removed() {
    assert_eq!(
        check("<?php each($arr);\n"),
        vec![(
            133,
            1,
            7,
            "Do not use \"each()\". This function was deprecated in PHP 7.2 \
             and removed in PHP 8.0"
                .to_string(),
        )],
    );
    // Uppercase `EACH` also trips the lowercase-builtin rule, as in PHP.
    assert_eq!(check("<?php EACH($arr);\n").len(), 2);
}

#[test]
fn product_name_literals_are_not_ported() {
    // XHP134 needs libphutil `PlatformSymbols` at runtime, so `pht()`
    // contents are never reported here.
    assert!(check("<?php pht('Phabricator rocks');\n").is_empty());
}

// -( T1170 COV5: close xhp_calls.rs )--------------------------------------------

fn for_rule(source: &str, rule: u32) -> Vec<(usize, usize, String)> {
    check(source)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

fn for_rule_with_options(
    source: &str,
    rule: u32,
    options: &ast::XhpOptions,
) -> Vec<(usize, usize, String)> {
    check_with_options(source, options)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

fn for_rule_blob(canned: &str, rule: u32) -> Vec<(usize, usize, String)> {
    check_blob(canned)
        .into_iter()
        .filter(|(r, _, _, _)| *r == rule)
        .map(|(_, line, col, message)| (line, col, message))
        .collect()
}

#[test]
fn constructs_skip_call_name_rules() {
    // `isset` is a construct: every `is_construct` early-out runs.
    assert!(check("<?php isset($x);\n").is_empty());
}

#[test]
fn arg_scans_trim_trivia_and_nesting() {
    assert!(for_rule("<?php f($x );\n", 53).is_empty());
    assert!(for_rule("<?php f( );\n", 53).is_empty());
    assert!(for_rule("<?php f(g($x));\n", 53).is_empty());
}

#[test]
fn arg_less_define_is_skipped() {
    assert!(for_rule("<?php define();\n", 12).is_empty());
}

#[test]
fn unclosed_strpos_call_is_skipped() {
    assert!(for_rule("<?php strpos($a\n", 36).is_empty());
}

#[test]
fn double_quote_chains_cross_brackets() {
    let hits = for_rule("<?php $x = \"a\" . ($y);\n", 41);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "String does not require double quotes. For consistency, prefer single quotes."
    );
    let hits = for_rule("<?php $x = ($y) . \"b\";\n", 41);
    assert_eq!(hits.len(), 1);
}

#[test]
fn single_quote_literal_has_no_inner_text() {
    let hits = for_rule_blob(
        "{\"source\": \"\\\"\", \"tokens\": [[\"T_CONSTANT_ENCAPSED_STRING\", \"\\\"\", 0]]}",
        41,
    );
    assert_eq!(hits.len(), 1);
}

#[test]
fn leading_paren_has_no_call_context() {
    assert!(
        for_rule_blob(
            "{\"source\": \"($x);\", \"tokens\": [[\"(\", \"(\", 0],[\"T_VARIABLE\", \"$x\", 1],[\";\", \";\", 3]]}",
            53
        )
        .is_empty()
    );
}

#[test]
fn leading_call_name_is_not_a_declaration() {
    assert!(
        for_rule_blob(
            "{\"source\": \"f()\", \"tokens\": [[\"T_STRING\", \"f\", 0],[\"(\", \"(\", 1],[\")\", \")\", 2]]}",
            53
        )
        .is_empty()
    );
}

#[test]
fn by_ref_declaration_names_are_not_calls() {
    assert!(for_rule("<?php function &foo() {}\n", 53).is_empty());
}

#[test]
fn custom_printf_functions_take_effect() {
    let options = ast::XhpOptions {
        printf: vec![("myprintf".to_string(), 0)],
        ..ast::XhpOptions::default()
    };
    let hits = for_rule_with_options("<?php myprintf();\n", 54, &options);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "This function is expected to have a format string."
    );
    assert!(for_rule_with_options("<?php myprintf('x', $y);\n", 54, &options).is_empty());
    assert!(for_rule_with_options("<?php otherfunc();\n", 54, &options).is_empty());
}

#[test]
fn anonymous_classes_have_no_name_index() {
    assert!(for_rule("<?php $x = new class {};\n", 62).is_empty());
}

#[test]
fn body_less_classes_have_no_end() {
    assert!(for_rule("<?php class A $x;\n", 62).is_empty());
    assert!(for_rule("<?php class A extends\n", 62).is_empty());
}

#[test]
fn dynamic_function_keys_match_case_insensitively() {
    let options = ast::XhpOptions {
        dynamic_functions: vec![("MYFUNC".to_string(), 0)],
        ..ast::XhpOptions::default()
    };
    let hits = for_rule_with_options("<?php myfunc($x);\n", 86, &options);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].2,
        "Parameter 1 of `myfunc` should be a scalar string, otherwise it's not safe."
    );
    assert!(for_rule_with_options("<?php myfunc('lit');\n", 86, &options).is_empty());
    assert!(for_rule_with_options("<?php otherfunc($x);\n", 86, &options).is_empty());
}

#[test]
fn negative_dynamic_params_are_safe() {
    let options = ast::XhpOptions {
        dynamic_functions: vec![("f".to_string(), -1)],
        ..ast::XhpOptions::default()
    };
    assert!(for_rule_with_options("<?php f($x);\n", 86, &options).is_empty());
}

#[test]
fn dynamic_class_names_scan_and_skip() {
    for src in [
        "<?php $x = new",
        "<?php $x = new $y;\n",
        "<?php $x = new Foo",
        "<?php $x = new Foo;\n",
        "<?php $x = new Foo($x);\n",
        "<?php $x = new Foo\\",
        "<?php $x = new Foo\\5;\n",
        "<?php $x = new A\\B($x);\n",
    ] {
        assert!(for_rule(src, 86).is_empty(), "{src}");
    }
    // A watched class with a dynamic argument reports.
    let options = ast::XhpOptions {
        dynamic_classes: vec![("a\\b".to_string(), 0)],
        ..ast::XhpOptions::default()
    };
    let hits = for_rule_with_options("<?php $x = new A\\B($y);\n", 86, &options);
    assert_eq!(hits.len(), 1);
}
