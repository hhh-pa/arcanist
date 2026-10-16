use super::*;
use std::collections::HashMap;

#[test]
fn accessors_round_trip() {
    let mut result = UnitResult::new();
    result.set_namespace("ns");
    result.set_name("my test").unwrap();
    result.set_link("http://example.test/t");
    result.set_result(RESULT_PASS);
    result.set_duration(1.25);
    result.set_user_data("details");
    let mut extra = HashMap::new();
    extra.insert("k".to_string(), "v".to_string());
    result.set_extra_data(extra);
    let mut coverage = HashMap::new();
    coverage.insert("a.php".to_string(), "CCU".to_string());
    result.set_coverage(coverage);

    assert_eq!(result.get_namespace(), Some("ns"));
    assert_eq!(result.get_name(), "my test");
    assert_eq!(result.get_link(), Some("http://example.test/t"));
    assert_eq!(result.get_result(), RESULT_PASS);
    assert_eq!(result.get_duration(), Some(1.25));
    assert_eq!(result.get_user_data(), Some("details"));
    assert_eq!(result.get_extra_data().unwrap().get("k").unwrap(), "v");
    assert_eq!(result.get_coverage().unwrap().get("a.php").unwrap(), "CCU");
}

#[test]
fn name_over_255_bytes_is_rejected() {
    let mut result = UnitResult::new();
    let long = "n".repeat(256);
    let err = result.set_name(&long).unwrap_err();
    assert!(err.contains("setName()"), "{err}");
    assert!(err.contains("255"), "{err}");
    assert!(err.contains("256"), "{err}");
    assert_eq!(result.get_name(), "");
    assert!(result.set_name(&"n".repeat(255)).is_ok());
}

#[test]
fn merge_coverage_prefers_covered_and_keeps_tails() {
    assert_eq!(UnitResult::merge_coverage(&[]), None);
    assert_eq!(
        UnitResult::merge_coverage(&["NNN".to_string()]),
        Some("NNN".to_string())
    );
    assert_eq!(
        UnitResult::merge_coverage(&["NUU".to_string(), "CCN".to_string()]),
        Some("CCU".to_string())
    );
    assert_eq!(
        UnitResult::merge_coverage(&["C".to_string(), "NUCC".to_string()]),
        Some("CUCC".to_string())
    );
}

#[test]
fn dictionary_keys_match_php_order() {
    let mut result = UnitResult::new();
    result.set_name("t").unwrap();
    result.set_result(RESULT_FAIL);
    result.set_duration(0.5);
    result.set_user_data("why");
    let json = result.to_dictionary().to_json_string();
    assert!(json.contains("\"namespace\":null"), "{json}");
    assert!(json.contains("\"name\":\"t\""), "{json}");
    assert!(json.contains("\"result\":\"fail\""), "{json}");
    assert!(json.contains("\"userData\":\"why\""), "{json}");
    assert!(json.contains("\"coverage\":null"), "{json}");
    assert!(json.contains("\"extra\":null"), "{json}");
    let order: Vec<&str> = [
        "namespace",
        "name",
        "link",
        "result",
        "duration",
        "extra",
        "userData",
        "coverage",
    ]
    .iter()
    .map(|k| {
        let _ = k;
        *k
    })
    .collect();
    let mut last = 0;
    for key in order {
        let at = json.find(&format!("\"{key}\"")).unwrap();
        assert!(at >= last, "{json}");
        last = at;
    }
}

#[test]
fn result_code_specs_cover_all_codes() {
    let codes = UnitResult::get_all_result_codes();
    assert_eq!(codes, vec!["pass", "fail", "skip", "broken", "unsound"]);
    assert_eq!(UnitResult::get_result_code_name("pass"), Some("Pass"));
    assert_eq!(
        UnitResult::get_result_code_description("unsound"),
        Some(
            "The test failed, but this change is probably not what broke it. For example, it might have already been failing."
        )
    );
    assert_eq!(UnitResult::get_result_code_name("nope"), None);
    assert_eq!(UnitResult::get_result_code_description("nope"), None);
}

#[test]
fn overall_result_matches_workflow_mapping() {
    let mut pass = UnitResult::new();
    pass.set_result(RESULT_PASS);
    let mut unsound = UnitResult::new();
    unsound.set_result(RESULT_UNSOUND);
    let mut fail = UnitResult::new();
    fail.set_result(RESULT_FAIL);
    let mut broken = UnitResult::new();
    broken.set_result(RESULT_BROKEN);
    assert_eq!(overall_result(&[pass.clone()]), WORKFLOW_RESULT_OKAY);
    assert_eq!(
        overall_result(&[pass.clone(), unsound.clone()]),
        WORKFLOW_RESULT_UNSOUND
    );
    assert_eq!(overall_result(&[unsound, fail]), WORKFLOW_RESULT_FAIL);
    assert_eq!(overall_result(&[pass, broken]), WORKFLOW_RESULT_FAIL);
    assert_eq!(overall_result(&[]), WORKFLOW_RESULT_OKAY);
}

#[test]
fn coverage_fraction_counts_covered_over_known() {
    assert_eq!(coverage_fraction("CCUU"), 0.5);
    assert_eq!(coverage_fraction("NNNN"), 0.0);
    assert_eq!(coverage_fraction(""), 0.0);
}

#[test]
fn coverage_string_matches_nose_loop() {
    // Note the PHP `<` quirk: line 5 stays unreported, so the string is short.
    assert_eq!(coverage_string_from_lines(5, &[(2, 1), (4, 0)]), "NCNU");
    assert_eq!(coverage_string_from_lines(3, &[]), "NNN");
    assert_eq!(coverage_string_from_lines(3, &[(3, 1)]), "NNC");
    // Same `<` quirk: with start == count, no trailing fill is added.
    assert_eq!(coverage_string_from_lines(2, &[(1, 0)]), "U");
}

#[test]
fn run_all_guard_matches_php_message() {
    let err = EngineContext::check_set_run_all_tests(false, "PytestTestEngine", true).unwrap_err();
    assert_eq!(
        err,
        "Engine 'PytestTestEngine' does not support --everything."
    );
    assert!(EngineContext::check_set_run_all_tests(true, "X", true).is_ok());
}

#[test]
fn mini_xml_finds_elements_and_text() {
    let xml = "<root><!-- hi --><a x=\"1\">A&amp;B</a><a x='2'/></root>";
    let found = find_elements(xml, "a");
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].attr("x"), Some("1"));
    assert_eq!(text_content(&found[0].inner), "A&B");
    assert_eq!(found[1].attr("x"), Some("2"));
    assert_eq!(find_elements(xml, "missing").len(), 0);
}

#[test]
fn mini_xml_handles_cdata_and_entities() {
    let xml = "<t><m><![CDATA[a<b]]></m><m>&#65;&#x42;</m></t>";
    let found = find_elements(xml, "m");
    assert_eq!(text_content(&found[0].inner), "a<b");
    assert_eq!(text_content(&found[1].inner), "AB");
}

#[test]
fn xunit_parser_maps_statuses_and_names() {
    let xml = r#"<testsuite>
      <testcase classname="a.B" name="one" time="0.5"/>
      <testcase classname="a.B" name="two" time="1"><skipped>  why
</skipped></testcase>
      <testcase classname="a.B" name="three" time="2"><failure>boom</failure></testcase>
      <testcase classname="a.B" name="four" time="3"><error>kaboom</error></testcase>
    </testsuite>"#;
    let results = parse_xunit_testcases(xml).unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].get_name(), "a.B.one");
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert_eq!(results[0].get_duration(), Some(0.5));
    assert_eq!(results[1].get_result(), RESULT_SKIP);
    assert_eq!(results[1].get_user_data(), Some("why"));
    assert_eq!(results[2].get_result(), RESULT_FAIL);
    assert_eq!(results[2].get_user_data(), Some("boom\n"));
    assert_eq!(results[3].get_result(), RESULT_BROKEN);
    assert_eq!(results[3].get_user_data(), Some("kaboom\n"));
}

#[test]
fn xunit_parser_rejects_empty_and_garbage() {
    let err = parse_xunit_testcases("").unwrap_err();
    assert_eq!(
        err,
        "test_results argument to parseTestResults() must not be empty"
    );
    let err = parse_xunit_testcases("not xml at all").unwrap_err();
    assert!(
        err.contains("Failed to load XUnit report; Input starts with:"),
        "{err}"
    );
    assert!(err.contains("not xml at all"), "{err}");
    // Well-formed but result-free input parses to nothing.
    let results = parse_xunit_testcases("<testsuite></testsuite>").unwrap();
    assert!(results.is_empty());
}

#[test]
fn json_parser_reads_objects_arrays_and_escapes() {
    let value = parse_json(r#"{"a": [1, true, null, "x\ny"], "b": {"c": -2.5e2}}"#).unwrap();
    let arr = value.get("a").and_then(JsonValue::as_arr).unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0].as_num(), Some(1.0));
    assert_eq!(arr[2], JsonValue::Null);
    assert_eq!(arr[3].as_str(), Some("x\ny"));
    assert_eq!(
        value.get("b").unwrap().get("c").unwrap().as_num(),
        Some(-250.0)
    );
    assert!(parse_json("{bad}").is_err());
    assert!(parse_json("{} trailing").is_err());
    assert_eq!(
        parse_json(r#""\u0041""#).unwrap(),
        JsonValue::Str("A".to_string())
    );
}

#[test]
fn temp_file_is_removed_on_drop() {
    let path = {
        let tmp = TempFile::create("uniteng", ".tmp").unwrap();
        let path = tmp.path().to_path_buf();
        assert!(path.exists());
        path
    };
    assert!(!path.exists());
}

#[test]
fn dict_json_escapes_like_php_json_encode() {
    let mut result = UnitResult::new();
    result.set_name("a/b\"c\\d\u{8}\u{c}é").unwrap();
    result.set_result(RESULT_PASS);
    result.set_user_data("x/y");
    let json = result.to_dictionary().to_json_string();
    assert!(
        json.contains("\"name\":\"a\\/b\\\"c\\\\d\\b\\f\\u00e9\""),
        "{json}"
    );
    assert!(json.contains("\"userData\":\"x\\/y\""), "{json}");
    // Empty maps encode as lists, like PHP's empty arrays.
    result.set_extra_data(std::collections::HashMap::new());
    let json = result.to_dictionary().to_json_string();
    assert!(json.contains("\"extra\":[]"), "{json}");
}

// --- T1153 Wave 2: UNIT findings ---

#[test]
fn unit_u18_int_flag_transitions() {
    let mut result = UnitResult::new();
    assert!(!result.duration_is_int());
    result.set_duration_int(3);
    assert!(result.duration_is_int());
    assert_eq!(result.get_duration(), Some(3.0));
    // A float set clears the flag (PHP `json_encode(3.0)` keeps `.0`).
    result.set_duration(3.0);
    assert!(!result.duration_is_int());
}

#[test]
fn unit_u19_dictionary_nulls_for_unset() {
    let dict = UnitResult::new().to_dictionary();
    assert_eq!(dict.name, None);
    assert_eq!(dict.result, None);
    assert!(!dict.duration_is_int);
    let mut result = UnitResult::new();
    result.set_name("t").unwrap();
    result.set_result(RESULT_PASS);
    let dict = result.to_dictionary();
    assert_eq!(dict.name.as_deref(), Some("t"));
    assert_eq!(dict.result.as_deref(), Some(RESULT_PASS));
}

#[test]
fn unit_u25_counts_use_thousands_separators() {
    assert_eq!(format_count(255), "255");
    assert_eq!(format_count(1234), "1,234");
    assert_eq!(format_count(1234567), "1,234,567");
    let mut result = UnitResult::new();
    let err = result.set_name(&"n".repeat(1234)).unwrap_err();
    assert!(err.contains("1,234 bytes"), "{err}");
    assert!(err.contains("255 bytes"), "{err}");
}
