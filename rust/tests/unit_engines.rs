//! Bundle U harness: unit engines end to end over canned outputs,
//! through the wired library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use arc_rs::hg::HgCommand;
use arc_rs::svn::SvnCommand;
use arc_rs::{
    unit, unit_config, unit_csharp,
    unit_engine::{self, CapturedOutput, CommandRunner, EngineContext, EngineError, UnitResult},
    unit_nose,
    unit_phpunit::{self, PhpunitFs},
    unit_phutil,
    unit_pytest::{self, PytestFs},
    unit_xunit,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-uniteng-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn root_string(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }

    fn write(&self, rel: &str, contents: &str) -> String {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, contents).unwrap();
        full.to_string_lossy().replace('\\', "/")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn with_root(fixture: &str, root: &str) -> String {
    fixture.replace("COVER_ROOT", root)
}

// --- configuration-driven engine -------------------------------------------

#[test]
fn config_plans_engines_and_routes_paths() {
    let specs = unit_config::parse_engine_specs(
        include_str!("fixtures/uniteng/arcunit_sample.json"),
        "/r/.arcunit",
        &["phutil", "xunit"],
    )
    .unwrap();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].engine_type, "phutil");
    assert_eq!(
        specs[1].include,
        vec!["src/".to_string(), "lib/".to_string()]
    );
    assert_eq!(specs[1].exclude, vec!["gen/".to_string()]);

    let paths = vec![
        "src/a.cs".to_string(),
        "lib/b.cs".to_string(),
        "gen/c.cs".to_string(),
        "docs/d.md".to_string(),
    ];
    let routed = unit_config::match_paths(&paths, &specs[1].include, &specs[1].exclude);
    assert_eq!(routed, vec!["src/a.cs".to_string(), "lib/b.cs".to_string()]);

    let err = unit_config::parse_engine_specs(
        include_str!("fixtures/uniteng/arcunit_badtype.json"),
        "/r/.arcunit",
        &["phutil", "xunit"],
    )
    .unwrap_err();
    assert!(
        err.message().contains("specifies invalid type 'nope'"),
        "{}",
        err.message()
    );

    let tmp = TempDir::new("config");
    let err = unit_config::load_engine_specs(&tmp.root_string(), &["phutil"]).unwrap_err();
    assert!(
        err.message().contains("Unable to find '.arcunit'"),
        "{}",
        err.message()
    );
    tmp.write(
        ".arcunit",
        include_str!("fixtures/uniteng/arcunit_sample.json"),
    );
    let specs = unit_config::load_engine_specs(&tmp.root_string(), &["phutil", "xunit"]).unwrap();
    assert_eq!(specs.len(), 2);

    let mut result = UnitResult::new();
    result.set_name("t").unwrap();
    result.set_result(unit_engine::RESULT_PASS);
    let merged =
        unit_config::merge_engine_runs(vec![Ok(vec![result]), Err(EngineError::no_effect())])
            .unwrap();
    assert_eq!(merged.len(), 1);
    assert!(unit_config::merge_engine_runs(vec![Err(EngineError::no_effect())]).is_err());
}

// --- nose -------------------------------------------------------------------

struct NoseHarnessFs {
    root: String,
}

impl unit_nose::NoseFs for NoseHarnessFs {
    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn is_readable(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        std::fs::read_to_string(path)
            .ok()
            .map(|t| t.lines().count().max(1))
    }
}

#[test]
fn nose_runs_canned_xunit_and_coverage() {
    let tmp = TempDir::new("nose");
    let root = tmp.root_string();
    tmp.write("pkg/mod.py", "one\ntwo\nthree\nfour\n");
    tmp.write("tests/pkg/test_mod.py", "test\n");
    let cover = with_root(include_str!("fixtures/uniteng/cover_nose.xml"), &root);
    let xunit = include_str!("fixtures/uniteng/xunit_sample.xml").to_string();

    let fs = NoseHarnessFs { root: root.clone() };
    let engine = unit_nose::NoseEngine::with_binary("fake-nose");
    let ctx = EngineContext::new(&root);
    let runner = |argv: &[String], _cwd: &str| {
        let xunit_path = argv
            .iter()
            .find(|a| a.starts_with("--xunit-file="))
            .unwrap();
        let cover_path = argv
            .iter()
            .find(|a| a.starts_with("--cover-xml-file="))
            .unwrap();
        std::fs::write(xunit_path.trim_start_matches("--xunit-file="), &xunit).unwrap();
        std::fs::write(cover_path.trim_start_matches("--cover-xml-file="), &cover).unwrap();
        CapturedOutput::new(1, "", "")
    };
    let tests = unit_nose::select_tests(&["pkg/mod.py".to_string()], &root, &fs);
    assert_eq!(tests.len(), 1);
    let results = engine.run_tests(&tests, "./", &ctx, &runner, &fs).unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].get_result(), unit_engine::RESULT_PASS);
    assert_eq!(results[1].get_result(), unit_engine::RESULT_SKIP);
    assert_eq!(results[2].get_result(), unit_engine::RESULT_FAIL);
    assert_eq!(results[3].get_result(), unit_engine::RESULT_BROKEN);
    let coverage = results[0].get_coverage().unwrap();
    assert_eq!(coverage.len(), 1, "{coverage:?}");
    let report = coverage.values().next().unwrap();
    assert_eq!(report, "CUNC");
    assert_eq!(fs.root, root);
}

// --- pytest -----------------------------------------------------------------

struct PytestHarnessFs {
    files: HashMap<String, String>,
    dirs: HashSet<String>,
}

impl unit_pytest::PytestFs for PytestHarnessFs {
    fn path_exists(&self, path: &str) -> bool {
        self.files.contains_key(path) || Path::new(path).exists()
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
fn pytest_resolves_modules_and_parses_reports() {
    let tmp = TempDir::new("pytest");
    let root = tmp.root_string();
    let mut files = HashMap::new();
    files.insert(format!("{root}/pkg/mod.py"), "one\ntwo\n".to_string());
    let fs = PytestHarnessFs {
        files,
        dirs: HashSet::new(),
    };
    let cover = include_str!("fixtures/uniteng/cover_pytest.xml");
    let reports = unit_pytest::read_coverage(
        cover,
        &root,
        &["pkg/mod.py".to_string()],
        &|p| fs.line_count(p),
        &|p| fs.is_dir(p),
        &|p| fs.is_file(p),
    );
    assert_eq!(reports.get("pkg/mod.py").unwrap(), "CU");

    let engine = unit_pytest::PytestEngine::with_binaries("fake-pytest", "fake-coverage");
    let ctx = EngineContext {
        paths: vec!["pkg/mod.py".to_string()],
        enable_coverage: Some(false),
        ..EngineContext::new(&root)
    };
    let xunit = include_str!("fixtures/uniteng/xunit_sample.xml").to_string();
    let runner = |argv: &[String], _cwd: &str| {
        if argv[0] == "fake-pytest" {
            let junit = argv.iter().find(|a| a.starts_with("--junit-xml=")).unwrap();
            std::fs::write(junit.trim_start_matches("--junit-xml="), &xunit).unwrap();
        }
        CapturedOutput::new(0, "", "")
    };
    let results = engine.run_with(&ctx, &runner, &fs).unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(
        unit_engine::overall_result(&results),
        unit_engine::WORKFLOW_RESULT_FAIL
    );
}

// --- phpunit ----------------------------------------------------------------

struct PhpunitHarnessFs {
    files: HashMap<String, String>,
    dirs: HashSet<String>,
}

impl unit_phpunit::PhpunitFs for PhpunitHarnessFs {
    fn is_dir(&self, path: &str) -> bool {
        self.dirs.contains(path)
    }

    fn path_exists(&self, path: &str) -> bool {
        self.files.contains_key(path) || self.dirs.contains(path) || Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.files
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        self.files.get(path).map(|t| t.lines().count().max(1))
    }
}

#[test]
fn phpunit_selects_tests_and_parses_events() {
    let tmp = TempDir::new("phpunit");
    let root = tmp.root_string();
    let mut files = HashMap::new();
    files.insert(
        format!("{root}/src/Calc.php"),
        "<?php\nline\nline\n".to_string(),
    );
    files.insert(
        format!("{root}/src/CalcTest.php"),
        "<?php\ntest\n".to_string(),
    );
    let fs = PhpunitHarnessFs {
        files,
        dirs: HashSet::new(),
    };
    let affected = unit_phpunit::select_affected_tests(
        &["src/Calc.php".to_string()],
        &root,
        &|p| fs.is_dir(p),
        &|p| fs.path_exists(p),
    )
    .unwrap();
    assert_eq!(
        affected.get(&format!("{root}/src/Calc.php")).unwrap(),
        &format!("{root}/src/CalcTest.php")
    );

    let clover = with_root(include_str!("fixtures/uniteng/clover_sample.xml"), &root);
    let results = unit_phpunit::parse_phpunit_results(
        "CalcTest.php",
        include_str!("fixtures/uniteng/phpunit_events.txt"),
        Some(&clover),
        "",
        None,
        &root,
        &affected,
        &|p| fs.line_count(p),
    )
    .unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].get_result(), unit_engine::RESULT_PASS);
    assert_eq!(results[1].get_result(), unit_engine::RESULT_FAIL);
    assert!(
        results[1]
            .get_user_data()
            .unwrap()
            .contains("CalcTest.php:42")
    );
    assert_eq!(results[2].get_result(), unit_engine::RESULT_SKIP);
    let coverage = results[0].get_coverage().unwrap();
    assert_eq!(coverage.get("src/Calc.php").unwrap(), "CUN");
}

// --- phutil -----------------------------------------------------------------

#[test]
fn phutil_paths_and_coverage_gate() {
    let root_for = |p: &str| {
        if p.starts_with("/lib/") {
            Some("/lib".to_string())
        } else {
            None
        }
    };
    let name_for = |_: &str| Some("libphutil".to_string());
    let is_file = |_: &str| true;
    let paths = unit_phutil::get_test_paths(
        &["/lib/src/Thing.php".to_string()],
        "/",
        &root_for,
        &name_for,
        &is_file,
    )
    .unwrap();
    assert!(
        paths.iter().any(|p| p.path == "src/__tests__/"),
        "{paths:?}"
    );
    assert!(paths.iter().any(|p| p.path == "__tests__/"), "{paths:?}");

    let names = unit_phutil::collect_test_names(&[
        vec!["A".to_string()],
        vec!["A".to_string(), "B".to_string()],
    ]);
    let selected = unit_phutil::select_tests(false, Vec::new(), names).unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(
        unit_phutil::resolve_coverage(None, true).unwrap(),
        Some(true)
    );
    assert_eq!(unit_phutil::ENGINE_CONFIGURATION_NAME, "phutil");
}

// --- xunit ------------------------------------------------------------------

#[test]
fn xunit_discovers_builds_and_parses() {
    let env = unit_xunit::load_environment(
        "/r",
        false,
        &|b| matches!(b, "xbuild" | "mono" | "xunit.console.clr4.exe"),
        &|b| format!("/bin/{b}"),
        Some(vec![unit_xunit::DiscoveryRule {
            regex: "src/(.*)/".to_string(),
            targets: vec![unit_xunit::DiscoveryTarget {
                project: "$1.Tests.csproj".to_string(),
                assembly: "$1.Tests.dll".to_string(),
            }],
        }]),
        None,
        &|_: &str| false,
    )
    .unwrap();
    let projects = unit_xunit::map_paths_to_results(&["src/App/File.cs".to_string()], &env, &|p| {
        p == "/r/App.Tests.csproj"
    });
    assert_eq!(projects.len(), 1);

    let results = unit_xunit::parse_assembly_results(
        include_str!("fixtures/uniteng/assembly_sample.xml"),
        None,
    )
    .unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].get_result(), unit_engine::RESULT_PASS);
    assert_eq!(results[1].get_result(), unit_engine::RESULT_FAIL);
    assert_eq!(
        results[1].get_user_data(),
        Some("Expected 1 but got 0\nat App.Tests.Calc.Divides()")
    );
    assert_eq!(results[2].get_result(), unit_engine::RESULT_SKIP);

    let mut pass = UnitResult::new();
    pass.set_name("g").unwrap();
    pass.set_result(unit_engine::RESULT_PASS);
    let merged = unit_xunit::run_all_tests(
        &projects,
        &|| vec![pass.clone()],
        &|_| vec![pass.clone()],
        &|_| vec![pass.clone()],
    );
    assert_eq!(merged.len(), 3);

    let dict = results[0].to_dictionary().to_json_string();
    assert!(dict.contains("\"name\":\"App.Tests.Calc.Adds\""), "{dict}");
    assert!(dict.contains("\"result\":\"pass\""), "{dict}");
}

// --- csharp -----------------------------------------------------------------

#[test]
fn csharp_instruments_and_reads_cscover() {
    let tmp = TempDir::new("csharp");
    let root = tmp.root_string();
    tmp.write("tools/cscover.exe", "fake");
    let cover = unit_csharp::load_cover_engine(&root, Some("tools/cscover.exe"), None, &|p| {
        Path::new(p).exists()
    })
    .unwrap();
    assert!(cover.is_some());

    let excluded: HashMap<String, String> =
        HashMap::from([("Skip.dll".to_string(), "1".to_string())]);
    let assemblies = unit_csharp::collect_assemblies(
        "bin",
        &["App.dll", "Skip.dll", "notes.txt"],
        Some(&excluded),
        None,
    );
    assert_eq!(assemblies, vec!["bin/App.dll".to_string()]);
    assert!(unit_csharp::use_cover_engine(None, &assemblies));

    let xml = with_root(include_str!("fixtures/uniteng/cscover_sample.xml"), &root);
    let reports = unit_csharp::read_cscover_coverage(&xml, &root, &|p| {
        if p == format!("{root}/src/App.cs") {
            Some(4)
        } else {
            None
        }
    })
    .unwrap();
    assert_eq!(reports.get("src/App.cs").unwrap(), "CCUU");

    let results = unit_csharp::parse_assembly_results(
        include_str!("fixtures/uniteng/assembly_sample.xml"),
        Some(&reports),
    )
    .unwrap();
    assert_eq!(results.len(), 3);
    assert!(results[0].get_coverage().is_some());
}

// --- shared result model ----------------------------------------------------

#[test]
fn shared_result_model_merges_and_reports() {
    let merged = UnitResult::merge_coverage(&["NU".to_string(), "CN".to_string()]);
    assert_eq!(merged, Some("CU".to_string()));
    assert_eq!(unit_engine::coverage_fraction("CCU"), 2.0 / 3.0);
    let mut fail = UnitResult::new();
    fail.set_name("t").unwrap();
    fail.set_result(unit_engine::RESULT_BROKEN);
    assert_eq!(
        unit_engine::overall_result(&[fail]),
        unit_engine::WORKFLOW_RESULT_FAIL
    );
    assert_eq!(UnitResult::get_all_result_codes().len(), 5);
}

// --- gap2: hg/svn selection over fake binaries (U1/U2) -----------------------

#[cfg(windows)]
const FAKEHG: &str = include_str!("fixtures/vcs/fakehg.bat");
#[cfg(not(windows))]
const FAKEHG: &str = include_str!("fixtures/vcs/fakehg.sh");

#[cfg(windows)]
const FAKESVN: &str = include_str!("fixtures/vcs/fakesvn.bat");
#[cfg(not(windows))]
const FAKESVN: &str = include_str!("fixtures/vcs/fakesvn.sh");

#[cfg(windows)]
const FAKEPHP: &str = include_str!("fixtures/unit/fakephp.bat");
#[cfg(not(windows))]
const FAKEPHP: &str = include_str!("fixtures/unit/fakephp.sh");

/// Install a fake script into `dir` (executable on unix) and return its path.
fn install_fake(dir: &TempDir, name: &str, contents: &str) -> String {
    let file = format!("{name}.{}", if cfg!(windows) { "bat" } else { "sh" });
    let path = dir.write(&file, contents);
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn hg_backend(dir: &TempDir) -> HgCommand {
    let script = install_fake(dir, "fakehg", FAKEHG);
    #[cfg(windows)]
    {
        HgCommand::custom("cmd", &["/C", script.as_str()])
    }
    #[cfg(not(windows))]
    {
        HgCommand::custom(script.as_str(), &[])
    }
}

fn svn_backend(dir: &TempDir) -> SvnCommand {
    let script = install_fake(dir, "fakesvn", FAKESVN);
    #[cfg(windows)]
    {
        SvnCommand::custom("cmd", &["/C", script.as_str()])
    }
    #[cfg(not(windows))]
    {
        SvnCommand::custom(script.as_str(), &[])
    }
}

#[test]
fn hg_selection_excludes_untracked_and_resolves_rev() {
    let dir = TempDir::new("hg-sel");
    dir.write(".fakehg-status", "M mod.py\nA add.py\n? new.py\n");
    let hg = hg_backend(&dir);
    let root = dir.root_string();
    let root_path = Path::new(&root);
    assert_eq!(
        unit::hg_uncommitted_paths_with(root_path, &hg).unwrap(),
        vec!["add.py".to_string(), "mod.py".to_string()]
    );
    // The range resolves the quoted revset first, then diffs base..tip.
    dir.write(".fakehg-status", "M old.py\n");
    let paths = unit::hg_commit_range_paths_with(root_path, &hg, "my-branch").unwrap();
    assert_eq!(paths, vec!["old.py".to_string()]);
    let log = std::fs::read_to_string(dir.path.join(".fakehg.log")).unwrap();
    assert!(log.contains("ancestor('my-branch',.)"), "{log}");
    assert!(
        log.contains("--rev 0123456789abcdef0123456789abcdef01234567 --rev tip"),
        "{log}"
    );
}

#[test]
fn hg_invalid_rev_and_failures_surface() {
    let dir = TempDir::new("hg-err");
    dir.write(".fakehg-status", "");
    let hg = hg_backend(&dir);
    let root = dir.root_string();
    let root_path = Path::new(&root);
    let err = unit::hg_commit_range_paths_with(root_path, &hg, "nope-xyz").unwrap_err();
    assert_eq!(
        err,
        "Commit 'nope-xyz' is not a valid Mercurial commit identifier."
    );
    dir.write(".fakehg-manifest", "b.py\na.py\n");
    assert_eq!(
        unit::hg_all_tracked_files_with(root_path, &hg).unwrap(),
        vec!["a.py".to_string(), "b.py".to_string()]
    );
    dir.write(".fakehg-fail-status", "x");
    let err = unit::hg_uncommitted_paths_with(root_path, &hg).unwrap_err();
    assert!(err.contains("hg status failed"), "{err}");
}

#[test]
fn svn_selection_excludes_untracked_and_externals() {
    let dir = TempDir::new("svn-sel");
    dir.write(
        ".fakesvn-status",
        "<status><target path=\".\">\
         <entry path=\"mod.py\"><wc-status item=\"modified\" props=\"none\"/></entry>\
         <entry path=\"new.py\"><wc-status item=\"unversioned\" props=\"none\"/></entry>\
         <entry path=\"ext\"><wc-status item=\"external\" props=\"none\"/></entry>\
         <entry path=\"ext/e.py\"><wc-status item=\"modified\" props=\"none\"/></entry>\
         </target></status>",
    );
    let svn = svn_backend(&dir);
    let root = dir.root_string();
    let root_path = Path::new(&root);
    assert_eq!(
        unit::svn_uncommitted_paths_with(root_path, &svn).unwrap(),
        vec!["mod.py".to_string()]
    );
    dir.write(".fakesvn-list", "b.py\na.py\nsub/\n\n");
    assert_eq!(
        unit::svn_all_tracked_files_with(root_path, &svn).unwrap(),
        vec!["a.py".to_string(), "b.py".to_string()]
    );
    dir.write(".fakesvn-fail-status", "x");
    let err = unit::svn_uncommitted_paths_with(root_path, &svn).unwrap_err();
    assert!(err.contains("svn --xml status failed"), "{err}");
    dir.write(".fakesvn-fail-list", "x");
    let err = unit::svn_all_tracked_files_with(root_path, &svn).unwrap_err();
    assert!(err.contains("svn list failed"), "{err}");
}

#[test]
fn select_paths_routes_hg_and_svn() {
    use arc_rs::unit::Options;

    // hg repo: everything lists the manifest, default unions status,
    // explicit paths bypass VCS, rev flows through.
    let dir = TempDir::new("sel-hg");
    std::fs::create_dir_all(dir.path.join(".hg")).unwrap();
    dir.write(".fakehg-manifest", "a.py\n");
    dir.write(".fakehg-status", "M a.py\n? u.py\n");
    dir.write("a.py", "x\n");
    let hg = hg_backend(&dir);
    let svn = svn_backend(&dir);
    let root = dir.root_string();
    let root_path = Path::new(&root);
    let everything = Options {
        everything: true,
        ..Options::default()
    };
    assert_eq!(
        unit::select_paths_with(&everything, root_path, root_path, &hg, &svn).unwrap(),
        vec!["a.py".to_string()]
    );
    assert_eq!(
        unit::select_paths_with(&Options::default(), root_path, root_path, &hg, &svn).unwrap(),
        vec!["a.py".to_string()]
    );
    let explicit = Options {
        paths: vec!["a.py".to_string()],
        ..Options::default()
    };
    assert_eq!(
        unit::select_paths_with(&explicit, root_path, root_path, &hg, &svn).unwrap(),
        vec!["a.py".to_string()]
    );
    let rev = Options {
        rev: Some("my-branch".to_string()),
        ..Options::default()
    };
    assert_eq!(
        unit::select_paths_with(&rev, root_path, root_path, &hg, &svn).unwrap(),
        vec!["a.py".to_string()]
    );

    // svn repo: same, except rev is a usage error.
    let dir = TempDir::new("sel-svn");
    std::fs::create_dir_all(dir.path.join(".svn")).unwrap();
    dir.write(".fakesvn-list", "s.py\n");
    dir.write(
        ".fakesvn-status",
        "<status><target path=\".\">\
         <entry path=\"s.py\"><wc-status item=\"modified\" props=\"none\"/></entry>\
         </target></status>",
    );
    let hg = hg_backend(&dir);
    let svn = svn_backend(&dir);
    let root = dir.root_string();
    let root_path = Path::new(&root);
    assert_eq!(
        unit::select_paths_with(&everything, root_path, root_path, &hg, &svn).unwrap(),
        vec!["s.py".to_string()]
    );
    assert_eq!(
        unit::select_paths_with(&Options::default(), root_path, root_path, &hg, &svn).unwrap(),
        vec!["s.py".to_string()]
    );
    let err = unit::select_paths_with(&rev, root_path, root_path, &hg, &svn).unwrap_err();
    assert_eq!(
        err,
        "This version control system does not support commit ranges."
    );

    // No VCS: the empty selection passes through.
    let dir = TempDir::new("sel-none");
    let root = dir.root_string();
    let root_path = Path::new(&root);
    let hg = hg_backend(&dir);
    let svn = svn_backend(&dir);
    assert!(
        unit::select_paths_with(&everything, root_path, root_path, &hg, &svn)
            .unwrap()
            .is_empty()
    );
    assert!(
        unit::select_paths_with(&Options::default(), root_path, root_path, &hg, &svn)
            .unwrap()
            .is_empty()
    );
}

// --- gap2: phutil fake-php end to end (U5) -----------------------------------

/// Spawn-argv runner for the fake-php tests (the real `php` never runs).
struct SpawnRunner;

impl CommandRunner for SpawnRunner {
    fn run(&self, argv: &[String], cwd: &str) -> CapturedOutput {
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .output()
            .expect("spawn fake php");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        CapturedOutput::new(out.status.code().unwrap_or(-1), &stdout, &stderr)
    }
}

fn php_prefix(dir: &TempDir) -> Vec<String> {
    let script = install_fake(dir, "fakephp", FAKEPHP);
    #[cfg(windows)]
    {
        vec!["cmd".to_string(), "/C".to_string(), script]
    }
    #[cfg(not(windows))]
    {
        vec![script]
    }
}

#[test]
fn phutil_fake_php_end_to_end() {
    let dir = TempDir::new("php-e2e");
    let php = php_prefix(&dir);
    dir.write(
        ".fakephp-output",
        r#"{"results": [{"namespace": "C", "name": "testIt", "result": "pass", "duration": 0.5}]}"#,
    );
    let root = dir.root_string();
    let ctx = EngineContext {
        project_root: root.clone(),
        paths: vec!["lib/A.php".to_string()],
        enable_coverage: Some(false),
        run_all_tests: false,
    };
    let results = unit::run_single_with_php("PhutilUnitTestEngine", &ctx, &SpawnRunner, &|| {
        Some(php.clone())
    })
    .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_name(), "testIt");
    assert_eq!(results[0].get_duration(), Some(0.5));

    // Fixture-pinned argv: driver, input, output in order (the fake logs
    // its own argv, so the interpreter prefix is not in the log).
    let log = std::fs::read_to_string(dir.path.join(".fakephp.log")).unwrap();
    let line = log.lines().next().unwrap();
    let driver_at = line.find(".arc-unit-phutil-driver-").unwrap();
    let input_at = line.find(".arc-unit-phutil-input-").unwrap();
    let output_at = line.find(".arc-unit-phutil-output-").unwrap();
    assert!(driver_at < input_at && input_at < output_at, "{line}");
    assert!(line[input_at..].contains(".json"), "{line}");

    // The staged input carried the run parameters.
    let input_text = std::fs::read_to_string(dir.path.join(".fakephp-input-seen")).unwrap();
    let input: serde_json::Value = serde_json::from_str(&input_text).unwrap();
    assert_eq!(input["projectRoot"], root);
    assert_eq!(input["paths"], serde_json::json!(["lib/A.php"]));
    assert_eq!(input["enableCoverage"], false);
    assert!(input["initScript"].is_null());

    // Staged files are cleaned; only fake control files remain.
    let leftovers: Vec<String> = std::fs::read_dir(&dir.path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".arc-unit-phutil-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn phutil_fake_php_failure_surfaces() {
    let dir = TempDir::new("php-fail");
    let php = php_prefix(&dir);
    dir.write(".fakephp-fail", "x");
    let root = dir.root_string();
    let ctx = EngineContext {
        project_root: root,
        paths: Vec::new(),
        enable_coverage: None,
        run_all_tests: true,
    };
    let err =
        unit::run_single_with_php("phutil", &ctx, &SpawnRunner, &|| Some(php.clone())).unwrap_err();
    assert_eq!(err.message(), "phutil driver failed: fake php exploded");
}
