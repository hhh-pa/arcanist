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
    // The fake's line counts back the coverage readers.
    assert_eq!(fs.line_count("/r/pkg/mod.py"), Some(1));
    assert_eq!(fs.line_count("/r/missing.py"), None);
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
      <class filename="/r/pkg/other.py"><lines>
        <line number="1" hits="1"/>
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

// -( T1170 wave 5d )-----------------------------------------------------------

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-unit-nose-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn default_engine_uses_nose_binary() {
    assert_eq!(NoseEngine::default().binary, NOSE_BINARY);
    assert_eq!(NoseEngine::new().binary, NOSE_BINARY);
}

#[test]
fn real_fs_line_count_reads_files() {
    let tmp = TempDir::new("linecount");
    let file = tmp.path().join("mod.py");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let fs = RealNoseFs;
    let name = file.to_string_lossy().to_string();
    assert_eq!(fs.line_count(&name), Some(3));
    let missing = tmp.path().join("missing.py").to_string_lossy().to_string();
    assert_eq!(fs.line_count(&missing), None);
    // Empty files still report one line, like the PHP count.
    let empty = tmp.path().join("empty.py").to_string_lossy().to_string();
    std::fs::write(&empty, "").unwrap();
    assert_eq!(fs.line_count(&empty), Some(1));
}

#[test]
fn run_all_discovers_test_files_recursively() {
    let tmp = TempDir::new("discover");
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("tests").join("sub")).unwrap();
    std::fs::write(root.join("tests").join("test_a.py"), "x\n").unwrap();
    std::fs::write(root.join("tests").join("sub").join("test_b.py"), "x\n").unwrap();
    std::fs::write(root.join("tests").join("helper.py"), "x\n").unwrap();
    std::fs::write(root.join("tests").join("notes.txt"), "x\n").unwrap();
    std::fs::write(root.join("top.py"), "x\n").unwrap();
    let root_name = root.to_string_lossy().to_string();
    let found = expand_run_all_tests(&root_name);
    let base = root.to_string_lossy().replace('\\', "/");
    assert_eq!(
        found,
        vec![
            format!("{base}/tests/sub/test_b.py"),
            format!("{base}/tests/test_a.py"),
        ]
    );
    // A missing `tests` tree discovers nothing.
    let bare = TempDir::new("discover-bare");
    let bare_name = bare.path().to_string_lossy().to_string();
    assert!(expand_run_all_tests(&bare_name).is_empty());
}

#[test]
fn coverage_skips_nameless_and_uncounted_files() {
    let xml = r#"<coverage><package><classes>
      <class><lines><line number="1" hits="1"/></lines></class>
      <class filename="/r/unknown.py"><lines><line number="1" hits="1"/></lines></class>
      <class filename="/r/known.py"><lines><line number="1" hits="1"/></lines></class>
    </classes></package></coverage>"#;
    let counts = |path: &str| {
        if path == "/r/known.py" { Some(1) } else { None }
    };
    let reports = read_coverage_ordered(xml, "/r", &counts);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, "/r/known.py");
}

#[test]
fn join_root_passes_absolute_paths_through() {
    assert_eq!(join_root("/r", "/abs/x.py"), "/abs/x.py");
    assert_eq!(join_root("/r", "C:\\proj\\x.py"), "C:/proj/x.py");
    assert_eq!(join_root("/r", "pkg/x.py"), "/r/pkg/x.py");
}

#[test]
fn run_tests_propagates_unparseable_xunit() {
    let fs = FakeFs::new();
    let engine = NoseEngine::with_binary("fake-nose");
    let ctx = EngineContext::new("/r");
    let runner = |argv: &[String], _cwd: &str| {
        let xunit = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        let cover = argv
            .iter()
            .find(|a| a.starts_with("--cover-xml-file="))
            .unwrap();
        std::fs::write(xunit.trim_start_matches("--xunit-file="), "").unwrap();
        std::fs::write(cover.trim_start_matches("--cover-xml-file="), "").unwrap();
        CapturedOutput::new(0, "", "")
    };
    let err = engine
        .run_tests(&["t.py".to_string()], "./", &ctx, &runner, &fs)
        .unwrap_err();
    assert!(
        err.message().contains("must not be empty"),
        "{}",
        err.message()
    );
}
