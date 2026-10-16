use super::*;
use arc_rs::unit_engine::{CapturedOutput, EngineContext, RESULT_FAIL, RESULT_PASS};
use std::collections::{HashMap, HashSet};

struct FakeFs {
    dirs: HashSet<String>,
    files: HashMap<String, String>,
}

impl FakeFs {
    fn new() -> Self {
        Self {
            dirs: HashSet::new(),
            files: HashMap::new(),
        }
    }
}

impl NoseFs for FakeFs {
    fn is_dir(&self, path: &str) -> bool {
        self.dirs.contains(path)
    }

    fn is_readable(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        self.files.get(path).map(|t| t.lines().count().max(1))
    }
}

#[test]
fn test_for_source_layout() {
    assert_eq!(test_for_source("pkg/mod.py"), "tests/pkg/test_mod.py");
    assert_eq!(test_for_source("mod.py"), "tests/test_mod.py");
}

#[test]
fn select_tests_maps_dirs_and_files() {
    let mut fs = FakeFs::new();
    fs.dirs.insert("/r/pkg".to_string());
    fs.files
        .insert("/r/pkg/mod.py".to_string(), "x\n".to_string());
    fs.files
        .insert("/r/tests/pkg".to_string(), "x\n".to_string());
    fs.files
        .insert("/r/tests/pkg/test_mod.py".to_string(), "x\n".to_string());
    // Directory input maps to tests/<path>.
    let found = select_tests(&["pkg".to_string()], "/r", &fs);
    assert_eq!(found, vec!["/r/tests/pkg".to_string()]);
    // File input maps through tests/<dir>/test_<file>.
    let found = select_tests(&["pkg/mod.py".to_string()], "/r", &fs);
    assert_eq!(found, vec!["/r/tests/pkg/test_mod.py".to_string()]);
    // Unmapped and unreadable inputs yield nothing.
    let found = select_tests(&["nope.py".to_string()], "/r", &fs);
    assert!(found.is_empty());
}

#[test]
fn build_argv_toggles_coverage_flags() {
    let with = build_nose_argv("nosetests", "x.xml", Some("c.xml"), "t.py");
    assert_eq!(
        with,
        vec![
            "nosetests",
            "--with-xunit",
            "--xunit-file=x.xml",
            "--with-coverage",
            "--cover-xml",
            "--cover-xml-file=c.xml",
            "t.py",
        ]
    );
    let without = build_nose_argv("nosetests", "x.xml", None, "t.py");
    assert_eq!(
        without,
        vec!["nosetests", "--with-xunit", "--xunit-file=x.xml", "t.py"]
    );
}

#[test]
fn exit_gate_keeps_one_and_rejects_two() {
    let argv = vec!["nosetests".to_string()];
    assert!(check_nose_exit(&argv, &CapturedOutput::new(0, "", "")).is_ok());
    assert!(check_nose_exit(&argv, &CapturedOutput::new(1, "", "")).is_ok());
    assert!(check_nose_exit(&argv, &CapturedOutput::new(2, "", "bad")).is_err());
}

#[test]
fn coverage_skips_empty_and_foreign_files() {
    let xml = r#"<coverage><package><classes>
      <class filename="/r/pkg/mod.py"><lines>
        <line number="1" hits="1"/><line number="3" hits="0"/>
      </lines></class>
      <class filename="/elsewhere/x.py"><lines>
        <line number="1" hits="1"/>
      </lines></class>
    </classes></package></coverage>"#;
    let counts = |path: &str| {
        if path == "/r/pkg/mod.py" {
            Some(3)
        } else {
            None
        }
    };
    let reports = read_coverage(xml, "/r", &counts);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports.get("/r/pkg/mod.py").unwrap(), "CNU");
    assert!(read_coverage("", "/r", &counts).is_empty());
}

#[test]
fn parse_attaches_coverage_unless_disabled() {
    let xunit = r#"<testsuite><testcase classname="a" name="t" time="1"/></testsuite>"#;
    let cover = r#"<coverage><package><classes>
      <class filename="/r/a.py"><lines><line number="1" hits="1"/></lines></class>
    </classes></package></coverage>"#;
    let ctx = EngineContext::new("/r");
    let counts = |_: &str| Some(1);
    let results = parse_test_results("./", xunit, Some(cover), None, &ctx, &counts).unwrap();
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert!(results[0].get_coverage().is_some());
    let results = parse_test_results("./", xunit, Some(cover), Some(false), &ctx, &counts).unwrap();
    assert!(results[0].get_coverage().is_none());
}

#[test]
fn run_with_uses_canned_outputs() {
    let fs = FakeFs::new();
    let engine = NoseEngine::with_binary("fake-nose");
    let ctx = EngineContext::new("/r");
    let runner = |argv: &[String], _cwd: &str| {
        assert_eq!(argv[0], "fake-nose");
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        let cover = argv
            .iter()
            .find(|a| a.starts_with("--cover-xml-file="))
            .unwrap();
        std::fs::write(
            xunit.trim_start_matches("--xunit-file="),
            "<testsuite><testcase classname=\"a\" name=\"t\" time=\"1\"><failure>bad</failure></testcase></testsuite>",
        )
        .unwrap();
        std::fs::write(cover.trim_start_matches("--cover-xml-file="), "").unwrap();
        CapturedOutput::new(1, "", "")
    };
    let results = engine
        .run_tests(&["t.py".to_string()], "./", &ctx, &runner, &fs)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), RESULT_FAIL);
    assert!(
        engine
            .run_tests(&[], "./", &ctx, &runner, &fs)
            .unwrap()
            .is_empty()
    );
    assert!(engine.supports_run_all_tests());
    assert_eq!(engine.engine_configuration_name(), None);
}

#[test]
fn unit_u20_coverage_keeps_document_order() {
    let cover = r#"<coverage><package><classes>
      <class filename="/r/z.py"><lines><line number="1" hits="1"/></lines></class>
      <class filename="/r/a.py"><lines><line number="1" hits="0"/></lines></class>
    </classes></package></coverage>"#;
    let counts = |_: &str| Some(1);
    let reports = read_coverage_ordered(cover, "/r", &counts);
    let names: Vec<&str> = reports.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["/r/z.py", "/r/a.py"]);
    // Through the parse path into the dictionary.
    let xunit = r#"<testsuite><testcase classname="a" name="t" time="1"/></testsuite>"#;
    let ctx = EngineContext::new("/r");
    let results = parse_test_results("./", xunit, Some(cover), None, &ctx, &counts).unwrap();
    let dict = results[0].to_dictionary();
    let names: Vec<&str> = dict
        .coverage
        .as_deref()
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, vec!["/r/z.py", "/r/a.py"]);
}
