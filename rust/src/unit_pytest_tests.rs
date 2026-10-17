use super::*;
use arc_rs::unit_engine::{CapturedOutput, EngineContext, RESULT_PASS};
use std::collections::{HashMap, HashSet};

struct FakeFs {
    dirs: HashSet<String>,
    files: HashMap<String, String>,
}

impl PytestFs for FakeFs {
    fn path_exists(&self, path: &str) -> bool {
        self.files.contains_key(path) || std::path::Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.files
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
    }

    fn is_dir(&self, path: &str) -> bool {
        self.dirs.contains(path)
    }

    fn is_file(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        self.files.get(path).map(|t| t.lines().count().max(1))
    }
}

#[test]
fn module_stem_conversion() {
    assert_eq!(module_to_relative("tornado.web.py"), "tornado/web");
    assert_eq!(module_to_relative("pkg.sub.mod.py"), "pkg/sub/mod");
}

#[test]
fn module_resolution_prefers_package_then_module() {
    let is_dir = |p: &str| p == "/r/pkg";
    let is_file = |p: &str| p == "/r/pkg.py" || p == "/r/other.py";
    assert_eq!(
        resolve_module_path("/r", "pkg", &is_dir, &is_file),
        Some("pkg/__init__.py".to_string())
    );
    assert_eq!(
        resolve_module_path("/r", "other", &is_dir, &is_file),
        Some("other.py".to_string())
    );
    assert_eq!(
        resolve_module_path("/r", "missing", &is_dir, &is_file),
        None
    );
}

#[test]
fn argv_wraps_coverage_unless_disabled() {
    let engine = PytestEngine::new();
    let ctx = EngineContext::new("/r");
    assert_eq!(
        engine.build_test_argv(&ctx, "j.xml"),
        vec![
            "coverage",
            "run",
            "--source",
            "/r",
            "-m",
            "py.test",
            "--junit-xml=j.xml"
        ]
    );
    let ctx = EngineContext {
        enable_coverage: Some(false),
        ..EngineContext::new("/r")
    };
    assert_eq!(
        engine.build_test_argv(&ctx, "j.xml"),
        vec!["py.test", "--junit-xml=j.xml"]
    );
    assert_eq!(
        build_coverage_xml_argv("coverage", "c.xml"),
        vec!["coverage", "xml", "-o", "c.xml"]
    );
}

#[test]
fn missing_junit_fails_with_php_message() {
    let argv = vec!["py.test".to_string()];
    let err = require_junit_output(false, 3, &argv, "o", "e").unwrap_err();
    assert!(
        err.message().contains("Command failed with error #3!"),
        "{}",
        err.message()
    );
    assert!(require_junit_output(true, 0, &argv, "", "").is_ok());
}

#[test]
fn coverage_filters_to_run_paths() {
    let xml = r#"<coverage><packages><package><classes>
      <class filename="pkg.mod.py"><lines>
        <line number="1" hits="1"/><line number="2" hits="0"/>
      </class></lines>
    </classes></package></packages></coverage>"#;
    let xml = xml.replace("</class></lines>", "</lines></class>");
    let is_dir = |_: &str| false;
    let is_file = |p: &str| p == "/r/pkg/mod.py";
    let counts = |p: &str| if p == "/r/pkg/mod.py" { Some(2) } else { None };
    let reports = read_coverage(
        &xml,
        "/r",
        &["pkg/mod.py".to_string()],
        &counts,
        &is_dir,
        &is_file,
    );
    assert_eq!(reports.get("pkg/mod.py").unwrap(), "CU");
    let reports = read_coverage(
        &xml,
        "/r",
        &["other.py".to_string()],
        &counts,
        &is_dir,
        &is_file,
    );
    assert!(reports.is_empty());
    assert!(read_coverage("", "/r", &[], &counts, &is_dir, &is_file).is_empty());
}

#[test]
fn parse_results_attach_coverage() {
    let junit = r#"<testsuite><testcase classname="a" name="t" time="1"/></testsuite>"#;
    let cover = r#"<coverage><packages><package><classes>
      <class filename="pkg.mod.py"><lines><line number="1" hits="1"/></lines></class>
    </classes></package></packages></coverage>"#;
    let is_dir = |_: &str| false;
    let is_file = |p: &str| p == "/r/pkg/mod.py";
    let counts = |_: &str| Some(1);
    let results = parse_test_results(
        junit,
        Some(cover),
        None,
        "/r",
        &["pkg/mod.py".to_string()],
        &counts,
        &is_dir,
        &is_file,
    )
    .unwrap();
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert!(results[0].get_coverage().is_some());
}

#[test]
fn run_with_uses_canned_outputs() {
    let fs = FakeFs {
        dirs: HashSet::new(),
        files: HashMap::new(),
    };
    let engine = PytestEngine::with_binaries("fake-pytest", "fake-coverage");
    let ctx = EngineContext {
        enable_coverage: Some(false),
        ..EngineContext::new("/r")
    };
    let runner = |argv: &[String], _cwd: &str| {
        if argv[0] == "fake-pytest" {
            let junit = argv.iter().find(|a| a.starts_with("--junit-xml=")).unwrap();
            std::fs::write(
                junit.trim_start_matches("--junit-xml="),
                "<testsuite><testcase classname=\"a\" name=\"t\" time=\"1\"/></testsuite>",
            )
            .unwrap();
        }
        CapturedOutput::new(0, "", "")
    };
    let results = engine.run_with(&ctx, &runner, &fs).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert!(results[0].get_coverage().is_none());

    let missing = |_: &[String], _: &str| CapturedOutput::new(3, "", "");
    let fs = FakeFs {
        dirs: HashSet::new(),
        files: HashMap::new(),
    };
    let err = engine.run_with(&ctx, &missing, &fs).unwrap_err();
    assert!(
        err.message().contains("Command failed with error #3!"),
        "{}",
        err.message()
    );
}

#[test]
fn binaries_and_flags() {
    let engine = PytestEngine::with_binaries("fake-pytest", "fake-coverage");
    assert_eq!(engine.binary, "fake-pytest");
    assert_eq!(engine.coverage_binary, "fake-coverage");
    assert!(!engine.supports_run_all_tests());
    assert_eq!(engine.engine_configuration_name(), None);
    let out = CapturedOutput::new(1, "", "oops");
    assert_eq!(describe_output(&out), "exit 1: oops");
}

#[test]
fn unit_u20_coverage_keeps_document_order() {
    let cover = r#"<coverage><packages><package><classes>
      <class filename="zeta.py"><lines><line number="1" hits="1"/></lines></class>
      <class filename="alpha.py"><lines><line number="1" hits="0"/></lines></class>
    </classes></package></packages></coverage>"#;
    let is_dir = |_: &str| false;
    let is_file = |p: &str| p == "/r/zeta.py" || p == "/r/alpha.py";
    let counts = |_: &str| Some(1);
    let paths = ["zeta.py".to_string(), "alpha.py".to_string()];
    let reports = read_coverage_ordered(cover, "/r", &paths, &counts, &is_dir, &is_file);
    let names: Vec<&str> = reports.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["zeta.py", "alpha.py"]);
}

#[test]
fn default_engine_matches_new() {
    let engine = PytestEngine::default();
    assert_eq!(engine.binary, PYTEST_BINARY);
    assert_eq!(engine.coverage_binary, COVERAGE_BINARY);
}

#[test]
fn run_with_reads_coverage_unless_disabled() {
    let mut files = HashMap::new();
    files.insert("/r/pkg/mod.py".to_string(), "x\ny\n".to_string());
    let fs = FakeFs {
        dirs: HashSet::new(),
        files,
    };
    let engine = PytestEngine::with_binaries("fake-pytest", "fake-coverage");
    let ctx = EngineContext {
        paths: vec!["pkg/mod.py".to_string()],
        ..EngineContext::new("/r")
    };
    let cover_xml = "<coverage><packages><package><classes>\
      <class filename=\"pkg.mod.py\"><lines><line number=\"1\" hits=\"1\"/></lines></class>\
    </classes></package></packages></coverage>";
    let runner = |argv: &[String], _cwd: &str| {
        if argv[0] == "fake-coverage" && argv[1] == "run" {
            let junit = argv.iter().find(|a| a.starts_with("--junit-xml=")).unwrap();
            std::fs::write(
                junit.trim_start_matches("--junit-xml="),
                "<testsuite><testcase classname=\"a\" name=\"t\" time=\"1\"/></testsuite>",
            )
            .unwrap();
        }
        if argv[0] == "fake-coverage" && argv[1] == "xml" {
            let out = argv.iter().find(|a| a.ends_with(".xml")).unwrap();
            std::fs::write(out, cover_xml).unwrap();
        }
        CapturedOutput::new(0, "", "")
    };
    let results = engine.run_with(&ctx, &runner, &fs).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_result(), RESULT_PASS);
    let coverage = results[0].get_coverage().unwrap();
    assert!(coverage.contains_key("pkg/mod.py"));
}

#[test]
fn coverage_skips_classes_without_filenames() {
    let cover = r#"<coverage><packages><package><classes>
      <class><lines><line number="1" hits="1"/></lines></class>
    </classes></package></packages></coverage>"#;
    let is_dir = |_: &str| false;
    let is_file = |_: &str| true;
    let counts = |_: &str| Some(1);
    let reports = read_coverage_ordered(cover, "/r", &[], &counts, &is_dir, &is_file);
    assert!(reports.is_empty());
}

#[test]
fn coverage_skips_unmeasurable_files() {
    let cover = r#"<coverage><packages><package><classes>
      <class filename="pkg.mod.py"><lines><line number="1" hits="1"/></lines></class>
    </classes></package></packages></coverage>"#;
    let is_dir = |_: &str| false;
    let is_file = |p: &str| p == "/r/pkg/mod.py";
    let counts = |_: &str| None;
    let reports = read_coverage_ordered(
        cover,
        "/r",
        &["pkg/mod.py".to_string()],
        &counts,
        &is_dir,
        &is_file,
    );
    assert!(reports.is_empty());
}
