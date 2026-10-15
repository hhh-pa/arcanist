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
        assert!(
            check_well_formed(data).is_ok(),
            "for {}",
            String::from_utf8_lossy(data)
        );
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
