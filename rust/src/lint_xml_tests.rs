use super::*;

#[test]
fn linter_name_is_xml() {
    assert_eq!(XmlLinter::new().name(), "XML");
}

#[test]
fn path_filter_matches_dot_xml_only() {
    assert!(is_xml_path("a.xml"));
    assert!(is_xml_path("dir/b.XML"));
    assert!(!is_xml_path("a.xml5"));
    assert!(!is_xml_path("a.txt"));
    assert!(!is_xml_path("xml"));
    assert!(!is_xml_path(""));
}

#[test]
fn well_formed_documents_pass() {
    let docs: &[&[u8]] = &[
        b"<r/>",
        b"<root></root>",
        b"<?xml version=\"1.0\"?><r/>",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<r a=\"1\" b='2'>t</r>\n",
        b"<!-- lead --><?pi data?><r><!-- c --><![CDATA[<not>&markup]]><?p?><e/></r><!-- tail -->",
        b"<!DOCTYPE r><r/>",
        b"<!DOCTYPE r [ <!ENTITY e \"v\"> ]><r/>",
        b"<!DOCTYPE r SYSTEM \"http://x/y.dtd\"><r/>",
        b"<r>&amp;&lt;&gt;&apos;&quot;&#65;&#x41;</r>",
        b"<a:b xmlns:c=\"d\"><c:e f:g=\"h\"/></a:b>",
        b"\xef\xbb\xbf<r/>",
        b"<r>\xc3\xa9</r>",
        b"<r>a</r>   \n  <!-- ok -->\n",
    ];
    for data in docs {
        let text = String::from_utf8_lossy(data);
        assert!(check_well_formed(data).is_ok(), "for {text}");
        assert!(XmlLinter::new().lint_bytes("f.xml", data).is_empty());
    }
}

#[test]
fn malformed_documents_fail_with_positions() {
    let cases: &[(&[u8], &str)] = &[
        (b"", "no root element"),
        (b"   \n  ", "no root element"),
        (b"<r>", "unclosed"),
        (b"<r></s>", "mismatched"),
        (b"<r><a/></r></extra>", "only one root"),
        (b"<r/>junk", "content after root"),
        (b"text<r/>", "expected root"),
        (b"<r a=1/>", "quoted"),
        (b"<r a=\"1\" a=\"2\"/>", "duplicate attribute"),
        (b"<r a=\"<\"/>", "'<' in attribute"),
        (b"<r>&bogus;</r>", "unknown entity"),
        (b"<r>&</r>", "entity reference"),
        (b"<r>&#;</r>", "character reference"),
        (b"<r>&#xD800;</r>", "out of range"),
        (b"<r>text</r", "expected '>'"),
        (b"<!-- unterminated", "unterminated comment"),
        (b"<r><!-- a -- b --></r>", "'--' is not allowed"),
        (b"<r><![CDATA[never ends</r>", "unterminated CDATA"),
        (b"<?pi never ends", "unterminated processing"),
        (b"<?xml version=\"2.0\"?><r/>", "unsupported XML version"),
        (b"<?xml encoding=\"x\"?><r/>", "must start with version"),
        (
            b"<r/><?xml version=\"1.0\"?>",
            "reserved processing instruction",
        ),
        (b"<!DOCTYPE r [ <!ENTITY e \"v\">", "unterminated DOCTYPE"),
        (b"<r/><!DOCTYPE r>", "only allowed before"),
        (b"<>", "invalid start tag"),
        (b"<r>\x01</r>", "invalid character"),
    ];
    for (data, fragment) in cases {
        let error = check_well_formed(data).expect_err(&format!("should fail: {data:?}"));
        assert!(
            error.message.contains(fragment),
            "for {data:?}: got '{}', want '{fragment}'",
            error.message
        );
        assert!(error.line >= 1 && error.col >= 1);
    }
}

#[test]
fn error_positions_point_at_the_problem() {
    let error = check_well_formed(b"<root>\n  <child>oops</root>\n").unwrap_err();
    assert!(error.message.contains("mismatched"), "{}", error.message);
    assert_eq!((error.line, error.col), (2, 14));

    let error = check_well_formed(b"<r>\ntext\nmore\n").unwrap_err();
    assert!(error.message.contains("unclosed"), "{}", error.message);
    assert_eq!(error.line, 4);
}

#[test]
fn lint_message_shape() {
    let found = XmlLinter::new().lint_bytes("f.xml", b"<r>");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "f.xml");
    assert_eq!(found[0].code, "XML1");
    assert_eq!(found[0].severity, "error");
    assert_eq!(found[0].name, "LibXML Error");
    assert!(found[0].description.contains("unclosed"));
}

#[test]
fn general_entities_are_rejected() {
    // Known limit: entity declarations are skipped, never resolved.
    let error = check_well_formed(b"<!DOCTYPE r [ <!ENTITY e \"v\"> ]><r>&e;</r>").unwrap_err();
    assert!(
        error.message.contains("unknown entity"),
        "{}",
        error.message
    );
}

#[test]
fn non_xml_and_binary_inputs_are_silent() {
    let linter = XmlLinter::new();
    assert!(linter.lint_bytes("f.txt", b"<r>").is_empty());
    assert!(linter.lint_bytes("f.xml", b"<r\x00>").is_empty());
}

#[test]
fn deep_nesting_is_rejected_not_stack_overflowing() {
    let mut data = Vec::new();
    for _ in 0..MAX_DEPTH + 10 {
        data.extend_from_slice(b"<a>");
    }
    let error = check_well_formed(&data).unwrap_err();
    assert!(error.message.contains("too deep"), "{}", error.message);
}

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-lint-xml-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn directories_named_xml_are_skipped() {
    let tmp = TempDir::new("dirskip");
    let dir = tmp.path().join("sub.xml");
    std::fs::create_dir_all(&dir).unwrap();
    let linter = XmlLinter::new();
    assert!(
        linter
            .lint_bytes(dir.to_string_lossy().as_ref(), b"<r/>")
            .is_empty()
    );
    // Missing display paths still check content.
    assert!(!linter.lint_bytes("ghost.xml", b"<r>").is_empty());
}

#[test]
fn lowercase_hex_and_malformed_char_refs() {
    // Lowercase hex digits decode.
    assert!(check_well_formed(b"<r>&#xab;</r>").is_ok());
    // Non-hex `x` digits are malformed.
    let error = check_well_formed(b"<r>&#xzz;</r>").unwrap_err();
    assert!(
        error.message.contains("invalid character reference"),
        "{}",
        error.message
    );
    // The hex helper maps only hex digits.
    assert_eq!(hex_val(b'a'), 10);
    assert_eq!(hex_val(b'F'), 15);
    assert_eq!(hex_val(b'z'), 0);
}

#[test]
fn declaration_lookalikes_without_space_are_not_declarations() {
    // `<?xml` not followed by whitespace is a PI (reserved target).
    let error = check_well_formed(b"<?xml?><r/>").unwrap_err();
    assert!(
        error.message.contains("reserved processing instruction"),
        "{}",
        error.message
    );
    // `<!DOCTYPE` not followed by whitespace is not a doctype.
    let error = check_well_formed(b"<!DOCTYPEfoo><r/>").unwrap_err();
    assert!(
        error.message.contains("expected root element"),
        "{}",
        error.message
    );
}

#[test]
fn attribute_value_edges() {
    let cases: &[(&[u8], &str)] = &[
        (b"<r a>", "expected '='"),
        (b"<r a=\"x>", "unterminated attribute value"),
    ];
    for (data, fragment) in cases {
        let error = check_well_formed(data).expect_err("should fail");
        let text = String::from_utf8_lossy(data);
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
    // References inside values validate.
    assert!(check_well_formed(b"<r a=\"&amp;\"/>").is_ok());
    let error = check_well_formed(b"<r a=\"&bogus;\"/>").unwrap_err();
    assert!(
        error.message.contains("unknown entity"),
        "{}",
        error.message
    );
}

#[test]
fn quoted_string_edges_in_declarations() {
    // Unquoted version value.
    let error = check_well_formed(b"<?xml version=1.0?><r/>").unwrap_err();
    assert!(
        error.message.contains("invalid XML declaration"),
        "{}",
        error.message
    );
    // Truncated declaration (the unterminated-quote error surfaces
    // mapped as an invalid declaration).
    let error = check_well_formed(b"<?xml version=\"1.0").unwrap_err();
    assert!(
        error.message.contains("invalid XML declaration"),
        "{}",
        error.message
    );
    // Truncated reference.
    let error = check_well_formed(b"<r>&amp").unwrap_err();
    assert!(
        error.message.contains("unterminated entity reference"),
        "{}",
        error.message
    );
}

#[test]
fn processing_instruction_edges() {
    let cases: &[(&[u8], &str)] = &[
        (b"<? ?><r/>", "invalid processing instruction"),
        (
            b"<?pi!><r/>",
            "expected whitespace in processing instruction",
        ),
    ];
    for (data, fragment) in cases {
        let error = check_well_formed(data).expect_err("should fail");
        let text = String::from_utf8_lossy(data);
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
}

#[test]
fn xml_declaration_attribute_edges() {
    let cases: &[(&[u8], &str)] = &[
        // Missing version opener.
        (b"<?xml ?><r/>", "invalid XML declaration"),
        // Truncated after version.
        (b"<?xml version=\"1.0\"", "unterminated XML declaration"),
        // Junk where whitespace belongs.
        (
            b"<?xml version=\"1.0\"!>",
            "expected whitespace in XML declaration",
        ),
        // Nameless attribute.
        (b"<?xml version=\"1.0\" !>", "invalid XML declaration"),
        // Repeated encoding.
        (
            b"<?xml version=\"1.0\" encoding=\"a\" encoding=\"b\"?><r/>",
            "invalid encoding declaration",
        ),
        // Bad standalone value.
        (
            b"<?xml version=\"1.0\" standalone=\"maybe\"?><r/>",
            "invalid standalone declaration",
        ),
        // Unknown attribute.
        (
            b"<?xml version=\"1.0\" foo=\"bar\"?><r/>",
            "unknown XML declaration attribute",
        ),
    ];
    for (data, fragment) in cases {
        let error = check_well_formed(data).expect_err("should fail");
        let text = String::from_utf8_lossy(data);
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
    // Trailing space before `?>` is valid.
    assert!(check_well_formed(b"<?xml version=\"1.0\" ?><r/>").is_ok());
}

#[test]
fn doctype_edges() {
    let cases: &[(&[u8], &str)] = &[
        (b"<!DOCTYPE ><r/>", "invalid DOCTYPE"),
        (b"<!DOCTYPE root", "unterminated DOCTYPE"),
        (b"<!DOCTYPE root !>", "invalid DOCTYPE"),
    ];
    for (data, fragment) in cases {
        let error = check_well_formed(data).expect_err("should fail");
        let text = String::from_utf8_lossy(data);
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
    // Internal subsets skip comments and PIs; a subset without the
    // closing `>` is unterminated.
    let error = check_well_formed(b"<!DOCTYPE r [ <!-- c --> <?pi d?> ]").unwrap_err();
    assert!(
        error.message.contains("unterminated DOCTYPE"),
        "{}",
        error.message
    );
    assert!(check_well_formed(b"<!DOCTYPE r [ <!-- c --> <?pi d?> ]><r/>").is_ok());
}

#[test]
fn start_tag_and_content_edges() {
    let cases: &[(&[u8], &str)] = &[
        (b"<r/ >", "expected '>'"),
        (
            b"<r a=\"1\"b=\"2\"/>",
            "expected whitespace before attribute",
        ),
        (b"<r", "unterminated start tag"),
        (b"<r><!FOO></r>", "markup declaration is not allowed here"),
        (b"<r></>", "invalid end tag"),
    ];
    for (data, fragment) in cases {
        let error = check_well_formed(data).expect_err("should fail");
        let text = String::from_utf8_lossy(data);
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
}

// -( T1153 Wave 2: lint retrocompat )--------------------------------------

#[test]
fn lint21_xml_linter_always_runs_without_extension_checks() {
    // PHP's `canRun()` gates on the libxml/simplexml extensions; the
    // port is a dependency-free parser, so it always runs (and orsay's
    // PHP ships both extensions, so PHP always runs there too).
    let linter = XmlLinter::new();
    assert!(!linter.lint_bytes("f.xml", b"<a>").is_empty());
}
