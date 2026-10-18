//! Tests for [`crate::terminal`].

use crate::terminal::{self, PhtArg, RefView, TVal};

fn fmt(pattern: &str, args: &[TVal]) -> String {
    terminal::tsprintf_ansi(pattern, args, false)
        .map(|t| t.to_string())
        .unwrap()
}

#[test]
fn strips_bold_underline_and_invert() {
    assert_eq!(terminal::interpret("**a**", false), "a");
    assert_eq!(terminal::interpret("**a** b **c**", false), "a b c");
    assert_eq!(terminal::interpret("__a__", false), "a");
    assert_eq!(terminal::interpret("##a##", false), "a");
    assert_eq!(terminal::interpret("**a", false), "**a");
    assert_eq!(terminal::interpret("a**", false), "a**");
    assert_eq!(terminal::interpret("***x**", false), "*x");
    assert_eq!(terminal::interpret("****", false), "");
}

#[test]
fn strips_colors() {
    assert_eq!(terminal::interpret("<fg:red>X</fg>", false), "X");
    assert_eq!(terminal::interpret("<bg:blue> X </bg>", false), " X ");
    assert_eq!(
        terminal::interpret("<bg:yellow>** <!> %s **</bg>", false),
        " <!> %s "
    );
    // Not color tags: unknown names, missing closers, mismatched kinds.
    assert_eq!(terminal::interpret("<NUL>", false), "<NUL>");
    assert_eq!(terminal::interpret("<fg:red>X", false), "<fg:red>X");
    assert_eq!(
        terminal::interpret("<fg:red>X</bg>", false),
        "<fg:red>X</bg>"
    );
    assert_eq!(
        terminal::interpret("<fg:orange>X</fg>", false),
        "<fg:orange>X</fg>"
    );
    assert_eq!(terminal::interpret("a</fg>", false), "a</fg>");
}

#[test]
fn strips_combined_log_and_badge_patterns() {
    assert_eq!(
        terminal::interpret("**<bg:blue> %s </bg>** %s\n", false),
        " %s  %s\n"
    );
    assert_eq!(
        terminal::interpret("<bg:cyan>**  *  **</bg> %s%s\n", false),
        "  *   %s%s\n"
    );
    assert_eq!(
        terminal::interpret("<bg:cyan>** :// **</bg>   %s__%s__\n", false),
        " ://    %s%s\n"
    );
    assert_eq!(
        terminal::interpret("**<bg:cyan> %s </bg>** %s %s ", false),
        " %s  %s %s "
    );
}

#[test]
fn drops_backslash_escapes() {
    assert_eq!(terminal::interpret("\\**a**", false), "**a**");
    assert_eq!(terminal::interpret("\\__a__", false), "__a__");
    assert_eq!(terminal::interpret("\\\\**a**", false), "\\**a**");
    assert_eq!(terminal::interpret("\\**a", false), "\\**a");
    assert_eq!(terminal::interpret("a\\b", false), "a\\b");
}

#[test]
fn renders_ansi() {
    assert_eq!(terminal::interpret("**a**", true), "\x1b[1ma\x1b[m");
    assert_eq!(terminal::interpret("__a__", true), "\x1b[4ma\x1b[m");
    assert_eq!(terminal::interpret("##a##", true), "\x1b[7ma\x1b[m");
    assert_eq!(
        terminal::interpret("<bg:green>X</bg>", true),
        "\x1b[42mX\x1b[49m"
    );
    assert_eq!(
        terminal::interpret("<fg:red>X</fg>", true),
        "\x1b[31mX\x1b[39m"
    );
    assert_eq!(
        terminal::interpret("<fg:default>X</fg>", true),
        "\x1b[39mX\x1b[39m"
    );
    assert_eq!(
        terminal::interpret("**<bg:blue> %s </bg>**", true),
        "\x1b[1m\x1b[44m %s \x1b[49m\x1b[m"
    );
}

#[test]
fn escapes_control_characters() {
    // PhutilTsprintfTestCase oracles.
    assert_eq!(fmt("%s", &[TVal::Text("\0")]), "<NUL>");
    assert_eq!(
        fmt("%s", &[TVal::Text("\x1b[31mred\x1b[39m")]),
        "<ESC>[31mred<ESC>[39m"
    );
    assert_eq!(
        fmt("%s", &[TVal::Text("1\r\n2\r3\n4")]),
        "1<CR><LF>2<CR>3<LF>4"
    );
    assert_eq!(fmt("%B", &[TVal::Text("1\r\n2\r3\n4")]), "1\r\n2<CR>3\n4");
    // Full map, plus multibyte passthrough.
    assert_eq!(
        terminal::escape_value("\x00\x01\x07\x08\t\n\r\x1b\x1f\x7fAé", false),
        "<NUL><0x01><BEL><BS><TAB><LF><CR><ESC><0x1F><DEL>Aé"
    );
    assert_eq!(
        terminal::escape_value("\r\nX\rY\tZ\n", true),
        "\r\nX<CR>Y\tZ\n"
    );
    assert_eq!(terminal::escape_value("", true), "");
}

#[test]
fn wraps_php_fixtures() {
    for name in ["plain", "newlines", "long", "trailing-space-prompt", "utf8"] {
        let input = match name {
            "plain" => include_str!("../tests/fixtures/terminal-wrap/plain.txt"),
            "newlines" => include_str!("../tests/fixtures/terminal-wrap/newlines.txt"),
            "long" => include_str!("../tests/fixtures/terminal-wrap/long.txt"),
            "trailing-space-prompt" => {
                include_str!("../tests/fixtures/terminal-wrap/trailing-space-prompt.txt")
            }
            _ => include_str!("../tests/fixtures/terminal-wrap/utf8.txt"),
        };
        let expected = match name {
            "plain" => include_str!("../tests/fixtures/terminal-wrap/plain.txt.expect"),
            "newlines" => {
                include_str!("../tests/fixtures/terminal-wrap/newlines.txt.expect")
            }
            "long" => include_str!("../tests/fixtures/terminal-wrap/long.txt.expect"),
            "trailing-space-prompt" => {
                include_str!("../tests/fixtures/terminal-wrap/trailing-space-prompt.txt.expect")
            }
            _ => include_str!("../tests/fixtures/terminal-wrap/utf8.txt.expect"),
        };
        assert_eq!(terminal::console_wrap(input, 0, true), expected, "{name}");
    }
}

#[test]
fn wraps_with_indent() {
    let turtles = format!("{}turtle", "turtle ".repeat(19));
    let line = "                    turtle turtle turtle turtle turtle turtle turtle turtle";
    assert_eq!(
        terminal::console_wrap(&turtles, 20, true),
        format!("{line}\n{line}\n                    turtle turtle turtle turtle")
    );
    assert_eq!(terminal::console_wrap("", 0, true), "");
    assert_eq!(terminal::console_wrap("ab\n", 4, false), "ab\n");
    assert_eq!(terminal::console_wrap("ab\ncd", 4, true), "    ab\n    cd");
}

#[test]
fn wrap_ignores_ansi_width() {
    let broken = "<bg:red>** %s **</bg> abc abc abc abc abc abc abc abc abc abc \
         abc abc abc abc abc abc abc\nabc abc abc abc abc abc abc abc abc abc abc!";
    let single = "<bg:red>** %s **</bg> abc abc abc abc abc abc abc abc abc abc \
         abc abc abc abc abc abc abc abc abc abc abc abc abc abc abc abc abc abc!";
    let expected = fmt(broken, &[TVal::Text("ERROR")]);
    let wrapped = terminal::console_wrap(&fmt(single, &[TVal::Text("ERROR")]), 0, true);
    assert_eq!(wrapped, expected);
    // The same text with real ANSI codes wraps identically.
    let ansi = terminal::tsprintf_ansi(single, &[TVal::Text("ERROR")], true)
        .map(|t| t.to_string())
        .unwrap();
    let stripped = terminal::console_wrap(&ansi, 0, true)
        .replace("\x1b[1m", "")
        .replace("\x1b[m", "")
        .replace("\x1b[41m", "")
        .replace("\x1b[49m", "");
    assert_eq!(stripped, expected);
}

#[test]
fn formats_conversions() {
    assert_eq!(fmt("%s", &[TVal::Text("x")]), "x");
    assert_eq!(fmt("%R", &[TVal::Text("x\0y")]), "x\0y");
    assert_eq!(fmt("%%%s%%", &[TVal::Text("x")]), "%x%");
    assert_eq!(fmt("100%", &[]), "100%");
    assert_eq!(fmt("%s", &[TVal::Int(42)]), "42");
    assert_eq!(fmt("%d", &[TVal::Int(-7)]), "-7");
    assert_eq!(fmt("%d", &[TVal::Text("12x")]), "12");
    assert_eq!(fmt("%d", &[TVal::Text("abc")]), "0");
    assert_eq!(fmt("%d", &[TVal::Text("  -45x")]), "-45");
    assert_eq!(fmt("%d", &[TVal::Text("")]), "0");
    // Values are never markup-interpreted.
    assert_eq!(fmt("%s", &[TVal::Text("a**b")]), "a**b");
    assert_eq!(fmt("**%s**", &[TVal::Text("a**b")]), "a**b");
    // Nested terminal strings pass through unescaped.
    let inner = terminal::tsprintf_ansi("**%s**", &[TVal::Text("x")], false).unwrap();
    assert_eq!(fmt("[%s]", &[TVal::Terminal(&inner)]), "[x]");
    // Block conversions.
    assert_eq!(fmt("%!", &[TVal::Text("T")]), " <!> T ");
    assert_eq!(fmt("%>", &[TVal::Text("cmd")]), "    $ cmd\n");
    assert_eq!(fmt("%?", &[TVal::Text("hint")]), "  ?   hint");
    let wrapped = fmt("%W", &[TVal::Text(&"word ".repeat(30))]);
    assert!(wrapped.contains('\n'), "{wrapped:?}");
    assert!(!wrapped.contains("  "), "{wrapped:?}");
}

#[test]
fn rejects_bad_patterns() {
    assert_eq!(
        terminal::tsprintf_ansi("%5s", &[TVal::Text("x")], false).unwrap_err(),
        "xsprintf() does not support the \"%5\" modifier."
    );
    assert_eq!(
        terminal::tsprintf_ansi("%s", &[], false).unwrap_err(),
        "Too few arguments to xsprintf()."
    );
    assert_eq!(
        terminal::tsprintf_ansi("x", &[TVal::Text("a")], false).unwrap_err(),
        "Too many arguments to xsprintf()."
    );
    assert_eq!(
        terminal::tsprintf_ansi("%s%q", &[TVal::Text("a"), TVal::Text("b")], false).unwrap_err(),
        "Unsupported escape sequence \"q\" found in pattern: %s%q"
    );
}

#[test]
fn translates_identities() {
    assert_eq!(terminal::pht("beer", &[]), "beer");
    assert_eq!(terminal::pht("%d beer(s)", &[PhtArg::Int(1)]), "1 beer(s)");
    assert_eq!(
        terminal::pht("a %s c %s", &[PhtArg::Text("b"), PhtArg::Text("d")]),
        "a b c d"
    );
    assert_eq!(terminal::pht("100%%", &[]), "100%");
    assert_eq!(terminal::pht("100%", &[]), "100%");
    assert_eq!(
        terminal::pht("%s %s", &[PhtArg::Text("a")]),
        "[Invalid Translation!] %s %s"
    );
}

#[test]
fn translates_every_table_entry() {
    let cases: &[(&str, i64, &str)] = &[
        (
            "%s locally modified path(s) are not included in this revision:",
            1,
            "A locally modified path is not included in this revision:",
        ),
        (
            "%s locally modified path(s) are not included in this revision:",
            2,
            "Locally modified paths are not included in this revision:",
        ),
        (
            "These %s path(s) will NOT be committed. Commit this revision anyway?",
            1,
            "This path will NOT be committed. Commit this revision anyway?",
        ),
        (
            "These %s path(s) will NOT be committed. Commit this revision anyway?",
            5,
            "These paths will NOT be committed. Commit this revision anyway?",
        ),
        (
            "Revision includes changes to %s path(s) that do not exist:",
            1,
            "Revision includes changes to a path that does not exist:",
        ),
        (
            "Revision includes changes to %s path(s) that do not exist:",
            3,
            "Revision includes changes to paths that do not exist:",
        ),
        (
            "This diff includes %s file(s) which are not valid UTF-8 (they contain invalid byte \
             sequences). You can either stop this workflow and fix these files, or continue. If \
             you continue, these files will be marked as binary.",
            1,
            "This diff includes a file which is not valid UTF-8 (it has invalid byte sequences). \
             You can either stop this workflow and fix it, or continue. If you continue, this \
             file will be marked as binary.",
        ),
        (
            "This diff includes %s file(s) which are not valid UTF-8 (they contain invalid byte \
             sequences). You can either stop this workflow and fix these files, or continue. If \
             you continue, these files will be marked as binary.",
            2,
            "This diff includes files which are not valid UTF-8 (they contain invalid byte \
             sequences). You can either stop this workflow and fix these files, or continue. If \
             you continue, these files will be marked as binary.",
        ),
        ("%s AFFECTED FILE(S)", 1, "AFFECTED FILE"),
        ("%s AFFECTED FILE(S)", 4, "AFFECTED FILES"),
        (
            "Do you want to mark these %s file(s) as binary and continue?",
            1,
            "Do you want to mark this file as binary and continue?",
        ),
        (
            "Do you want to mark these %s file(s) as binary and continue?",
            2,
            "Do you want to mark these files as binary and continue?",
        ),
        (
            "Do you want to amend these %s change(s) to the current commit?",
            1,
            "Do you want to amend this change to the current commit?",
        ),
        (
            "Do you want to amend these %s change(s) to the current commit?",
            2,
            "Do you want to amend these changes to the current commit?",
        ),
        (
            "Do you want to create a new commit with these %s change(s)?",
            1,
            "Do you want to create a new commit with this change?",
        ),
        (
            "Do you want to create a new commit with these %s change(s)?",
            2,
            "Do you want to create a new commit with these changes?",
        ),
        ("%s line(s)", 1, "line"),
        ("%s line(s)", 0, "lines"),
        ("%s line(s)", 2, "lines"),
        (
            "Ignore these %s untracked file(s) and continue?",
            1,
            "Ignore this untracked file and continue?",
        ),
        (
            "Ignore these %s untracked file(s) and continue?",
            2,
            "Ignore these untracked files and continue?",
        ),
        (
            "These %s commit(s) will be landed:",
            1,
            "This commit will be landed:",
        ),
        (
            "These %s commit(s) will be landed:",
            2,
            "These commits will be landed:",
        ),
        (
            "Daemon was idle for more than %s second(s), scaling pool down.",
            1,
            "Daemon was idle for more than one second, scaling pool down.",
        ),
        (
            "Daemon was idle for more than %s second(s), scaling pool down.",
            2,
            "Daemon was idle for more than 2 seconds, scaling pool down.",
        ),
        ("... (%s more byte(s)) ...", 1, "... (1 more byte) ..."),
        ("... (%s more byte(s)) ...", 5, "... (5 more bytes) ..."),
    ];
    for (pattern, count, expected) in cases {
        assert_eq!(
            terminal::pht(pattern, &[PhtArg::Int(*count)]),
            (*expected).to_string(),
            "{pattern} / {count}"
        );
    }
}

#[test]
fn translates_numbers_positions_and_nesting() {
    assert_eq!(
        terminal::pht("%s assertion(s) passed.", &[PhtArg::Number(1, 0)]),
        "1 assertion passed."
    );
    assert_eq!(
        terminal::pht("%s assertion(s) passed.", &[PhtArg::Number(1000, 0)]),
        "1,000 assertions passed."
    );
    assert_eq!(
        terminal::pht("%s assertion(s) passed.", &[PhtArg::Number(8, 1)]),
        "8.0 assertions passed."
    );
    assert_eq!(
        terminal::pht(
            "%s submodule(s) have uncommitted or untracked changes:",
            &[PhtArg::Number(1, 0)]
        ),
        "A submodule has uncommitted or untracked changes:"
    );
    assert_eq!(
        terminal::pht("Updated %s librarie(s).", &[PhtArg::Number(1234, 0)]),
        "Updated 1,234 libraries."
    );
    assert_eq!(
        terminal::pht("Updated %s librarie(s).", &[PhtArg::Number(1, 0)]),
        "Updated library."
    );
    assert_eq!(
        terminal::pht(
            "(To ignore these %s change(s), add them to \"%s\".)",
            &[PhtArg::Int(1), PhtArg::Text("/x")]
        ),
        "(To ignore this change, add it to \"/x\".)"
    );
    assert_eq!(
        terminal::pht(
            "(To ignore these %s change(s), add them to \"%s\".)",
            &[PhtArg::Int(3), PhtArg::Text("/x")]
        ),
        "(To ignore these changes, add them to \"/x\".)"
    );
    assert_eq!(
        terminal::pht(
            "Analyzing %s file(s) with %s subprocess(es)...",
            &[PhtArg::Int(1), PhtArg::Int(1)]
        ),
        "Analyzing one file with a single subprocess..."
    );
    assert_eq!(
        terminal::pht(
            "Analyzing %s file(s) with %s subprocess(es)...",
            &[PhtArg::Int(1), PhtArg::Int(2)]
        ),
        "Analyzing 1 file with 2 subprocesses..."
    );
    assert_eq!(
        terminal::pht(
            "Analyzing %s file(s) with %s subprocess(es)...",
            &[PhtArg::Int(3), PhtArg::Int(2)]
        ),
        "Analyzing 3 files with 2 subprocesses..."
    );
}

#[test]
fn rejects_invalid_variants() {
    assert_eq!(
        terminal::pht("%s line(s)", &[PhtArg::Text("x")]),
        "[Invalid Translation!] The \"en_US\" language data offers variant translations for the \
         plurality or gender of argument 1, but the value for that argument is not an integer, \
         PhutilNumber, or PhutilPerson (it is a value of type \"string\"). Raw input: <%s \
         line(s)>."
    );
    assert_eq!(
        terminal::pht("%s line(s)", &[]),
        "[Invalid Translation!] The \"en_US\" language data offers variant translations for the \
         plurality or gender of argument , but the value for that argument is not an integer, \
         PhutilNumber, or PhutilPerson (it is a value of type \"boolean\"). Raw input: <%s \
         line(s)>."
    );
}

#[test]
fn renders_ref_views() {
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some("Fix it".to_string()),
        ..RefView::default()
    };
    assert_eq!(view.render(0, 80, false), "  *   D123 Fix it\n");
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some(String::new()),
        ..RefView::default()
    };
    assert_eq!(view.render(0, 80, false), "  *   D123\n");
    let view = RefView {
        title: Some("Title only".to_string()),
        ..RefView::default()
    };
    assert_eq!(view.render(0, 80, false), "  *   Title only\n");
    let view = RefView {
        object_name: Some("D1".to_string()),
        title: Some("a\nb".to_string()),
        ..RefView::default()
    };
    assert_eq!(view.render(0, 80, false), "  *   D1 a<LF>b\n");
    let view = RefView {
        object_name: Some("D1".to_string()),
        title: Some("t".to_string()),
        uri: Some("http://x/D1".to_string()),
        lines: vec!["extra".to_string()],
        children: vec![RefView {
            object_name: Some("C1".to_string()),
            title: Some("child".to_string()),
            ..RefView::default()
        }],
    };
    assert_eq!(
        view.render(0, 80, false),
        "  *   D1 t\n ://    http://x/D1\n        extra\n  *     C1 child\n"
    );
}

#[test]
fn truncates_long_titles() {
    // 100 x's at usable 69: hard cut at 68 plus the terminator.
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some("x".repeat(100)),
        ..RefView::default()
    };
    assert_eq!(
        view.render(0, 80, false),
        format!("  *   D123 {}\u{2026}\n", "x".repeat(68))
    );
    // Word boundary: cut before the last run of breaks.
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some("0123456789 ".repeat(10)),
        ..RefView::default()
    };
    assert_eq!(
        view.render(0, 80, false),
        format!(
            "  *   D123 {}\u{2026}\n",
            "0123456789 ".repeat(5) + "0123456789"
        )
    );
    // Stop characters cut cleanly with no terminator.
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some(format!("{}. {}", "x".repeat(40), "y".repeat(60))),
        ..RefView::default()
    };
    assert_eq!(
        view.render(0, 80, false),
        format!("  *   D123 {}.\n", "x".repeat(40))
    );
    // Boundaries past the search floor are ignored: hard cut instead.
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some(format!("Ends here. {}", "x".repeat(100))),
        ..RefView::default()
    };
    assert_eq!(
        view.render(0, 80, false),
        format!("  *   D123 Ends here. {}\u{2026}\n", "x".repeat(57))
    );
    // Narrow consoles floor the usable width at 16.
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: Some("x".repeat(30)),
        ..RefView::default()
    };
    assert_eq!(
        view.render(0, 10, false),
        format!("  *   D123 {}\u{2026}\n", "x".repeat(15))
    );
}

#[test]
fn formats_console_format() {
    assert_eq!(
        terminal::console_format("<fg:green>**%s**</fg>", &["a\nb"], false),
        "a\nb"
    );
    assert_eq!(
        terminal::console_format("<fg:green>**%s**</fg>", &["x"], true),
        "\x1b[32m\x1b[1mx\x1b[m\x1b[39m"
    );
    assert_eq!(
        terminal::format_status_message("yellow", "OPEN REVISION", "left open.", false),
        " OPEN REVISION  left open.\n"
    );
    assert_eq!(
        terminal::format_status_message("blue", "CLOSE", "done.", true),
        "\x1b[44m\x1b[1m CLOSE \x1b[m\x1b[49m done.\n"
    );
}

#[test]
fn formats_log_lines() {
    assert_eq!(
        terminal::format_log("blue", "SCAN", "msg", false),
        " SCAN  msg\n"
    );
    assert_eq!(
        terminal::format_log("red", "USAGE EXCEPTION", "bad\nnews", false),
        " USAGE EXCEPTION  bad<LF>news\n"
    );
    assert_eq!(
        terminal::format_log("green", "DONE", "ok", true),
        "\x1b[1m\x1b[42m DONE \x1b[49m\x1b[m ok\n"
    );
}

#[test]
fn detects_process_settings() {
    assert!(!terminal::ansi_enabled());
    let _ = terminal::terminal_width();
    assert_eq!(
        terminal::tsprintf("%s", &[TVal::Text("x")])
            .unwrap()
            .to_string(),
        "x"
    );
}

// -( T1170 COV5: close terminal.rs )----------------------------------------------

#[test]
fn pht_int_conversions_render_numbers() {
    assert_eq!(terminal::pht("%d", &[PhtArg::Int(-7)]), "-7");
    assert_eq!(terminal::pht("%d", &[PhtArg::Number(7, 2)]), "7");
    assert_eq!(terminal::pht("%d", &[PhtArg::Text("42x")]), "42");
    assert_eq!(terminal::pht("%d", &[PhtArg::Text("abc")]), "0");
}

#[test]
fn pht_rejects_bad_conversions() {
    assert_eq!(
        terminal::pht("100%x done", &[]),
        "[Invalid Translation!] 100%x done"
    );
}

#[test]
fn pht_nested_variants_select_and_reject() {
    const PATTERN: &str = "Analyzing %s file(s) with %s subprocess(es)...";
    assert_eq!(
        terminal::pht(PATTERN, &[PhtArg::Int(1), PhtArg::Int(1)]),
        "Analyzing one file with a single subprocess..."
    );
    assert_eq!(
        terminal::pht(PATTERN, &[PhtArg::Int(2), PhtArg::Int(3)]),
        "Analyzing 2 files with 3 subprocesses..."
    );
    let bad_first = terminal::pht(PATTERN, &[PhtArg::Text("x"), PhtArg::Int(2)]);
    assert!(bad_first.contains("[Invalid Translation!]"), "{bad_first}");
    assert!(bad_first.contains("argument 1"), "{bad_first}");
    let bad_second = terminal::pht(PATTERN, &[PhtArg::Int(1), PhtArg::Text("y")]);
    assert!(
        bad_second.contains("[Invalid Translation!]"),
        "{bad_second}"
    );
    assert!(bad_second.contains("argument 2"), "{bad_second}");
}

#[test]
fn format_number_groups_and_signs() {
    assert_eq!(super::format_number(-1_234_567, 2), "-1,234,567.00");
    assert_eq!(super::format_number(1_234_567, 0), "1,234,567");
    assert_eq!(super::format_number(0, 3), "0.000");
}

#[test]
fn combine_glyphs_handles_empty_and_combiners() {
    assert!(super::combine_glyphs("").is_empty());
    assert_eq!(super::combine_glyphs("\u{301}ab"), [" \u{301}", "a", "b"]);
    assert_eq!(super::combine_glyphs("e\u{301}x"), ["e\u{301}", "x"]);
}

#[test]
fn console_width_skips_ansi_and_counts_wide() {
    assert_eq!(super::console_strlen("\x1b[1mab\x1b[0m"), 2);
    assert_eq!(super::console_strlen("\x1b[31X"), 5);
    assert_eq!(super::console_strlen("e\u{301}"), 1);
    assert_eq!(super::console_strlen("\u{1100}"), 2);
    assert_eq!(super::console_strlen("\u{1160}"), 1);
    assert_eq!(super::console_strlen("\u{2329}"), 2);
    assert_eq!(super::console_strlen("\u{4e00}ab"), 4);
    // NUL glyphs contribute no width.
    assert_eq!(super::console_strlen("\u{0}ab"), 2);
}

#[test]
fn truncate_glyphs_handles_multibyte_and_tiny_max() {
    assert_eq!(super::truncate_glyphs("éééé", 5), "éééé");
    assert_eq!(super::truncate_glyphs("abcdef", 1), "…");
    assert_eq!(super::truncate_glyphs("abcdef", 0), "…");
}

// -( T1170 coverage wave 6g band G, batch E )----------------------------------

#[test]
fn cov6g_question_conversion_renders_badge() {
    for ansi in [false, true] {
        let text = terminal::tsprintf_ansi("%?", &[TVal::Text("hi")], ansi)
            .unwrap()
            .to_string();
        assert!(text.contains("hi"), "{text:?} (ansi={ansi})");
    }
}

#[test]
fn cov6g_php_truthy_matrix() {
    assert!(!super::php_truthy(None));
    assert!(!super::php_truthy(Some("")));
    assert!(!super::php_truthy(Some("0")));
    assert!(super::php_truthy(Some("1")));
    assert!(super::php_truthy(Some("xterm")));
}

#[test]
fn cov6g_tput_columns_is_stable() {
    // Whatever the environment resolves (success, failure, or odd
    // output), two back-to-back probes agree.
    assert_eq!(super::tput_columns(), super::tput_columns());
}

// -( T1176 PC-2: demark tty-commands )-----------------------------------------

#[cfg(unix)]
struct TempDir {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-terminal-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(unix)]
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[cfg(unix)]
fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn question_conversion_survives_hostile_values() {
    // The `%?` nested format substitutes values; hostile text that looks
    // like patterns, markup, or ANSI can never fail it (white-box pin
    // for the infallible nested `expect`, formerly `?`). Each value
    // renders literally (escaped), proving values never re-parse.
    let hostile: &[(&str, &str)] = &[
        ("%s", "%s"),
        ("%q", "%q"),
        ("%5s", "%5s"),
        ("%", "%"),
        ("%%", "%%"),
        ("%?%!%>", "%?%!%>"),
        ("100%", "100%"),
        ("**x**", "**x**"),
        ("__y__", "__y__"),
        ("##z##", "##z##"),
        ("<bg:red>w</bg>", "<bg:red>w</bg>"),
        ("<fg:blue>v</fg>", "<fg:blue>v</fg>"),
        ("\x1b[31mred\x1b[0m", "<ESC>[31mred<ESC>[0m"),
        ("a\0b", "a<NUL>b"),
        ("\r\n", "<CR><LF>"),
        ("", ""),
        (" ", " "),
    ];
    for (value, expected) in hostile {
        let plain = terminal::tsprintf_ansi("%?", &[TVal::Text(value)], false)
            .unwrap()
            .to_string();
        assert!(plain.starts_with("  ?   "), "{plain:?} for {value:?}");
        assert!(plain.contains(expected), "{plain:?} for {value:?}");
        let ansi = terminal::tsprintf_ansi("%?", &[TVal::Text(value)], true)
            .unwrap()
            .to_string();
        assert!(
            ansi.contains('?') && ansi.contains("\x1b["),
            "{ansi:?} for {value:?}"
        );
        assert!(ansi.contains(expected), "{ansi:?} for {value:?}");
    }
    // Long values wrap but still succeed.
    let big = "x".repeat(500);
    for ansi in [false, true] {
        let text = terminal::tsprintf_ansi("%?", &[TVal::Text(&big)], ansi)
            .unwrap()
            .to_string();
        assert!(text.contains('?'), "{text:?} (ansi={ansi})");
    }
    // Non-text values take the same nested path.
    for ansi in [false, true] {
        assert!(terminal::tsprintf_ansi("%?", &[TVal::Int(-42)], ansi).is_ok());
        let inner = terminal::tsprintf_ansi("%s", &[TVal::Text("i%q")], ansi).unwrap();
        assert!(terminal::tsprintf_ansi("%?", &[TVal::Terminal(&inner)], ansi).is_ok());
        assert!(terminal::tsprintf_ansi("%?", &[TVal::Owned("%5s".to_string())], ansi).is_ok());
    }
}

#[cfg(unix)]
#[test]
fn tput_spawn_failure_yields_zero() {
    // No `tput` at the seam path: the spawn-failure arm returns 0.
    let tmp = TempDir::new("tput-missing");
    assert_eq!(super::tput_columns_with(tmp.path().join("tput")), 0);
}

#[cfg(unix)]
#[test]
fn tput_failing_exit_yields_zero() {
    let tmp = TempDir::new("tput-fail");
    let fake = write_script(tmp.path(), "tput", "#!/bin/sh\nexit 1\n");
    assert_eq!(super::tput_columns_with(&fake), 0);
}

#[cfg(unix)]
#[test]
fn tput_success_parses_widths_end_to_end() {
    let tmp = TempDir::new("tput-ok");
    for (body, expected) in [
        ("#!/bin/sh\necho 80\n", 80),
        ("#!/bin/sh\nprintf '  120\\t\\n'\n", 120),
        ("#!/bin/sh\nprintf '%s' '+42'\n", 42),
        ("#!/bin/sh\nprintf '%s' '80x'\n", 80),
        ("#!/bin/sh\nprintf '%s' '-80'\n", 0),
        ("#!/bin/sh\nprintf '%s' '0'\n", 0),
        ("#!/bin/sh\nprintf '%s' ''\n", 0),
    ] {
        let fake = write_script(tmp.path(), "tput", body);
        assert_eq!(super::tput_columns_with(&fake), expected, "{body:?}");
    }
}

#[cfg(not(windows))]
#[test]
fn parse_tput_width_matrix() {
    assert_eq!(super::parse_tput_width("80\n"), 80);
    assert_eq!(super::parse_tput_width("  120\t\n"), 120);
    assert_eq!(super::parse_tput_width("\0\x0b80\x0b\0"), 80);
    assert_eq!(super::parse_tput_width("+42"), 42);
    assert_eq!(super::parse_tput_width("007"), 7);
    assert_eq!(super::parse_tput_width("80x"), 80);
    assert_eq!(super::parse_tput_width("8 0"), 8);
    assert_eq!(super::parse_tput_width("-80"), 0);
    assert_eq!(super::parse_tput_width("-"), 0);
    assert_eq!(super::parse_tput_width("+"), 0);
    assert_eq!(super::parse_tput_width("0"), 0);
    assert_eq!(super::parse_tput_width(""), 0);
    assert_eq!(super::parse_tput_width("   "), 0);
    assert_eq!(super::parse_tput_width("x80"), 0);
    assert_eq!(
        super::parse_tput_width("99999999999999999999999"),
        usize::MAX
    );
}

#[test]
fn resolve_term_applies_ansicon_fallback() {
    assert_eq!(super::resolve_term(String::new(), Some("1")), "ansi");
    assert_eq!(super::resolve_term("0".to_string(), Some("x")), "ansi");
    assert_eq!(super::resolve_term("xterm".to_string(), Some("1")), "xterm");
    assert_eq!(super::resolve_term(String::new(), None), "");
    assert_eq!(super::resolve_term(String::new(), Some("")), "");
    assert_eq!(super::resolve_term(String::new(), Some("0")), "");
    assert_eq!(super::resolve_term("0".to_string(), None), "0");
}

#[test]
fn ansi_decision_follows_term_gate_then_tty_probe() {
    // Hermetic pin for the TERM gate: no env mutation (a global TERM
    // write would race parallel `tput` probes) and no TTY needed.
    #[cfg(not(windows))]
    {
        assert!(super::ansi_enabled_for("", true));
        assert!(super::ansi_enabled_for("ansi", true));
        assert!(!super::ansi_enabled_for("ansi", false));
        assert!(!super::ansi_enabled_for("dumb", false));
    }
    #[cfg(windows)]
    {
        assert!(super::ansi_enabled_for("ansi", true));
        assert!(super::ansi_enabled_for("cygwin", true));
        assert!(!super::ansi_enabled_for("ansi", false));
        assert!(!super::ansi_enabled_for("dumb", true));
        assert!(!super::ansi_enabled_for("", true));
    }
}

#[test]
fn ansicon_fallback_enables_ansi_end_to_end() {
    // TERM unset + ANSICON set resolves to `ansi`, which passes the
    // gate and defers to the TTY probe.
    let term = super::resolve_term(String::new(), Some("1"));
    assert_eq!(term, "ansi");
    assert!(super::ansi_enabled_for(&term, true));
    assert!(!super::ansi_enabled_for(&term, false));
    // Without ANSICON the empty TERM fails the Windows gate (off
    // Windows TERM never gates, so the probe alone decides).
    let term = super::resolve_term(String::new(), None);
    #[cfg(windows)]
    assert!(!super::ansi_enabled_for(&term, true));
    #[cfg(not(windows))]
    assert!(super::ansi_enabled_for(&term, true));
}
