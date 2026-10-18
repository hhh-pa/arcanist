use super::*;
use arc_rs::unit_engine::{RESULT_FAIL, RESULT_PASS, RESULT_SKIP, RESULT_UNSOUND};
use std::collections::HashMap;

#[test]
fn cover_engine_loading_branches() {
    let exists = |p: &str| p == "/r/tools/cscover.exe";
    assert_eq!(
        load_cover_engine("/r", Some("tools/cscover.exe"), None, &exists).unwrap(),
        Some("/r/tools/cscover.exe".to_string())
    );
    assert_eq!(
        load_cover_engine("/r", Some("tools/cscover.exe"), Some(false), &exists).unwrap(),
        None
    );
    let err = load_cover_engine("/r", None, None, &exists).unwrap_err();
    assert!(
        err.message().contains("unit.csharp.coverage.binary"),
        "{}",
        err.message() // LCOV_EXCL_LINE // Reason: assert format arg; evaluated only on failure
    );
    let err = load_cover_engine("/r", Some("missing.exe"), None, &exists).unwrap_err();
    assert!(
        err.message().contains("have you built yet?"),
        "{}",
        err.message() // LCOV_EXCL_LINE // Reason: assert format arg; evaluated only on failure
    );
    assert_eq!(CSCOVER_BINARY_KEY, "unit.csharp.cscover.binary");
    assert_eq!(COVERAGE_MATCH_KEY, "unit.csharp.coverage.match");
    assert_eq!(COVERAGE_EXCLUDED_KEY, "unit.csharp.coverage.excluded");
}

#[test]
fn instrumentation_checks_excluded_then_regex() {
    let mut excluded = HashMap::new();
    excluded.insert("Skip.dll".to_string(), "1".to_string());
    assert!(!assembly_should_be_instrumented(
        "Skip.dll",
        Some(&excluded),
        None
    ));
    assert!(assembly_should_be_instrumented(
        "App.dll",
        Some(&excluded),
        None
    ));
    assert!(assembly_should_be_instrumented(
        "App.dll",
        None,
        Some("App")
    ));
    assert!(!assembly_should_be_instrumented(
        "Other.dll",
        None,
        Some("App")
    ));
    assert!(assembly_should_be_instrumented("Other.dll", None, None));
    assert!(is_assembly_file("a.dll"));
    assert!(is_assembly_file("b.exe"));
    assert!(!is_assembly_file("c.pdb"));
    let found = collect_assemblies("/b", &["a.dll", "c.pdb", "Skip.dll"], Some(&excluded), None);
    assert_eq!(found, vec!["/b/a.dll".to_string()]);
}

#[test]
fn xunit_command_and_cscover_argv() {
    let (cmd, args) = xunit_cmd_and_args("", "xunit", "A.dll", "r.xml");
    assert_eq!(cmd, "xunit");
    assert_eq!(args, vec!["A.dll", "/xml", "r.xml"]);
    let (cmd, args) = xunit_cmd_and_args("mono", "xunit", "A.dll", "r.xml");
    assert_eq!(cmd, "mono");
    assert_eq!(args, vec!["xunit", "A.dll", "/xml", "r.xml"]);

    let argv = build_cscover_argv(
        "mono",
        "/c/cscover",
        "cover.xml",
        "xunit",
        &["A.dll".to_string(), "/xml".to_string(), "r.xml".to_string()],
        "/b",
        &["/b/A.dll".to_string()],
    );
    assert_eq!(
        argv,
        vec![
            "mono",
            "/c/cscover",
            "-o",
            "cover.xml",
            "-c",
            "xunit",
            "-a",
            "A.dll",
            "/xml",
            "r.xml",
            "-w",
            "/b",
            "/b/A.dll",
        ]
    );
    assert!(use_cover_engine(None, &["/b/A.dll".to_string()]));
    assert!(!use_cover_engine(Some(false), &["/b/A.dll".to_string()]));
    assert!(!use_cover_engine(None, &[]));
}

#[test]
fn cscover_coverage_paints_spans() {
    let xml = r#"<coverage>
      <instrumented file="/r/src/A.cs" start="1" end="3"/>
      <instrumented file="/r/src/A.cs" start="5" end="5"/>
      <executed file="/r/src/A.cs" start="2" end="2"/>
    </coverage>"#;
    let counts = |p: &str| if p == "/r/src/A.cs" { Some(5) } else { None };
    let reports = read_cscover_coverage(xml, "/r", &counts).unwrap();
    assert_eq!(reports.get("src/A.cs").unwrap(), "UCUNU");
    assert_eq!(relative_to_root("/r/src/A.cs", "/r"), "src/A.cs");
}

#[test]
fn coverage_result_and_cache() {
    let xml = "<coverage></coverage>";
    let counts = |_: &str| Some(1);
    assert_eq!(
        parse_coverage_result(Some(xml), Some(false), "/r", &counts).unwrap(),
        None
    );
    let some = parse_coverage_result(Some(xml), None, "/r", &counts).unwrap();
    assert!(some.is_some());
    assert_eq!(
        parse_coverage_result(None, None, "/r", &counts).unwrap(),
        None
    );

    let mut cache = CoverageCache::new();
    assert!(cache.get_cached("c.xml").is_none());
    let first = cache.read_cached("c.xml", xml, "/r", &counts).unwrap();
    let second = cache
        .read_cached("c.xml", "<other/>", "/r", &counts)
        .unwrap();
    assert_eq!(first, second);
    assert!(cache.get_cached("c.xml").is_some());
}

#[test]
fn assembly_results_mirror_parent_mapping() {
    let xml = r#"<assembly>
      <test name="t1" time="1" result="Pass"/>
      <test name="t2" time="0" result="Fail"/>
      <test name="t3" time="0" result="Skip"/>
      <test name="t4" time="0" result="Bogus"/>
    </assembly>"#;
    let mut coverage = HashMap::new();
    coverage.insert("A.cs".to_string(), "C".to_string());
    let results = parse_assembly_results(xml, Some(&coverage)).unwrap();
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert_eq!(results[1].get_result(), RESULT_FAIL);
    assert_eq!(results[2].get_result(), RESULT_SKIP);
    assert_eq!(results[3].get_result(), RESULT_UNSOUND);
    assert!(results[0].get_coverage().is_some());
}

#[test]
fn unit_u20_cscover_cache_and_parse_keep_file_order() {
    let xml = r#"<coverage>
      <instrumented file="/r/Z.cs" start="1" end="1"/>
      <instrumented file="/r/A.cs" start="1" end="1"/>
      <instrumented file="/r/Q.cs" start="1" end="1"/>
      <executed file="/r/Z.cs" start="1" end="1"/>
    </coverage>"#;
    let counts = |p: &str| {
        if p == "/r/Z.cs" || p == "/r/A.cs" {
            Some(1)
        } else {
            None
        }
    };
    let reports = read_cscover_coverage_ordered(xml, "/r", &counts).unwrap();
    let names: Vec<&str> = reports.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["Z.cs", "A.cs"]);
    // Through the cache and the assembly parse into the dictionary.
    let mut cache = CoverageCache::new();
    let cached = cache
        .read_cached_ordered("c.xml", xml, "/r", &counts)
        .unwrap();
    assert_eq!(cached, reports);
    let assembly = r#"<assembly><test name="t" time="1" result="Pass"/></assembly>"#;
    let results = parse_assembly_results_ordered(assembly, Some(&cached)).unwrap();
    let dict = results[0].to_dictionary();
    let names: Vec<&str> = dict
        .coverage
        .as_deref()
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, vec!["Z.cs", "A.cs"]);
}

// -( T1170 wave 5d )-----------------------------------------------------------

#[test]
fn match_rule_strips_delimiter_pairs() {
    // A non-alphanumeric opener with a matching closer unwraps.
    assert!(match_rule("/App/", "bin/App.dll"));
    assert!(!match_rule("/App/", "bin/Other.dll"));
    assert!(match_rule("#a\\/b#", "xa/by"));
    // Brackets, backslashes, and alphanumerics never unwrap.
    assert!(!match_rule("(App)", "bin/App.dll"));
    assert!(!match_rule("\\App", "bin/App.dll"));
    assert!(match_rule("App", "bin/App.dll"));
    assert!(!match_rule("App", "bin/Other.dll"));
}

#[test]
fn match_rule_empty_pattern_matches_everything() {
    assert!(match_rule("^$", "bin/anything.dll"));
    assert!(match_rule("//", "bin/anything.dll"));
    assert!(match_rule("", "bin/anything.dll"));
}

#[test]
fn paint_span_skips_zero_and_out_of_range_starts() {
    let mut coverage = vec![b'N'; 3];
    paint_span(&mut coverage, 0, 3, b'C');
    assert_eq!(coverage, vec![b'N'; 3]);
    paint_span(&mut coverage, 9, 10, b'C');
    assert_eq!(coverage, vec![b'N'; 3]);
    paint_span(&mut coverage, 2, 1, b'C');
    assert_eq!(coverage, vec![b'N'; 3]);
    paint_span(&mut coverage, 1, 2, b'C');
    assert_eq!(coverage, vec![b'C', b'C', b'N']);
}

#[test]
fn legacy_cache_round_trip() {
    let mut cache = CoverageCache::new();
    let mut reports = HashMap::new();
    reports.insert("B.cs".to_string(), "C".to_string());
    reports.insert("A.cs".to_string(), "U".to_string());
    cache.add_cached("c.xml", reports);
    let stored = cache.get_cached("c.xml").unwrap();
    assert_eq!(stored.get("A.cs").map(String::as_str), Some("U"));
    assert_eq!(stored.len(), 2);
    // Legacy entries without order read back sorted.
    assert_eq!(
        cache.get_cached_ordered("c.xml").unwrap(),
        vec![
            ("A.cs".to_string(), "U".to_string()),
            ("B.cs".to_string(), "C".to_string()),
        ]
    );
    assert!(cache.get_cached("missing.xml").is_none());
}

#[test]
fn ordered_cache_preserves_first_seen_order() {
    let mut cache = CoverageCache::new();
    cache.add_cached_ordered(
        "c.xml",
        vec![
            ("Z.cs".to_string(), "C".to_string()),
            ("A.cs".to_string(), "U".to_string()),
        ],
    );
    assert_eq!(
        cache.get_cached_ordered("c.xml").unwrap(),
        vec![
            ("Z.cs".to_string(), "C".to_string()),
            ("A.cs".to_string(), "U".to_string()),
        ]
    );
}

#[test]
fn legacy_parse_keeps_sorted_fallback_coverage() {
    let mut map = HashMap::new();
    map.insert("A.cs".to_string(), "C".to_string());
    // Empty assemblies keep the loop honest with zero results.
    assert!(parse_assembly_results("", Some(&map)).unwrap().is_empty());
    let results = parse_assembly_results(
        r#"<assembly><test name="t" time="1" result="Pass"/></assembly>"#,
        Some(&map),
    )
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0]
            .get_coverage()
            .unwrap()
            .get("A.cs")
            .map(String::as_str),
        Some("C")
    );
}

#[test]
fn coverage_skips_files_with_unknown_line_counts() {
    let xml = r#"<coverage>
      <instrumented file="/r/src/Unknown.cs" start="1" end="2"/>
      <instrumented file="/r/src/Known.cs" start="1" end="1"/>
    </coverage>"#;
    let counts = |p: &str| {
        if p == "/r/src/Known.cs" {
            Some(2)
        } else {
            None
        }
    };
    let reports = read_cscover_coverage_ordered(xml, "/r", &counts).unwrap();
    assert_eq!(
        reports,
        vec![("src/Known.cs".to_string(), "UN".to_string())]
    );
}
