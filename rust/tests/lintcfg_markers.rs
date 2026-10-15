//! `.arclint` config plus `markers` behavior through the wired library.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{arclint, markers};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-lintcfg-{tag}-{}-{n}", std::process::id()));
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(tmp.path(), &["init", "-q", "-b", "master"]);
    git(tmp.path(), &["config", "user.email", "t@t"]);
    git(tmp.path(), &["config", "user.name", "t"]);
    git(tmp.path(), &["config", "commit.gpgsign", "false"]);
    tmp
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn module_names_match() {
    assert_eq!(markers::NAME, "markers");
    assert_eq!(markers::MARKER_TYPE_BRANCH, "branch");
    assert_eq!(markers::MARKER_TYPE_BOOKMARK, "bookmark");
    assert_eq!(markers::UNPUBLISHED_LIMIT, 1000);
    assert_eq!(arclint::CONFIG_FILE_NAME, ".arclint");
}

#[test]
fn arclint_sample_plans_paths() {
    let text = include_str!("fixtures/lintcfg/arclint.sample.json");
    let config = arclint::parse_arclint(text).unwrap();
    let paths = ["src/a.php", "externals/b.php", "data/c.json"]
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    let plans = arclint::plan_for_paths(&config, &paths);
    assert_eq!(plans.len(), 3);
    // The fixture's global `externals/` pattern has no PCRE delimiters,
    // so it never matches (PHP warns and returns false); every path
    // without an include list is kept.
    let text = plans.iter().find(|plan| plan.display == "TXT").unwrap();
    assert_eq!(text.paths, paths);
    let json = plans.iter().find(|plan| plan.display == "JSON").unwrap();
    assert_eq!(json.paths, vec!["data/c.json".to_string()]);
}

#[test]
fn arclint_fixture_errors() {
    let err =
        arclint::parse_arclint(include_str!("fixtures/lintcfg/arclint.invalid.json")).unwrap_err();
    assert!(err.contains("valid JSON"), "unexpected: {err}");

    let err =
        arclint::parse_arclint(include_str!("fixtures/lintcfg/arclint.badtype.json")).unwrap_err();
    assert!(
        err.contains("specifies invalid type 'nope'"),
        "unexpected: {err}"
    );

    let err = arclint::parse_arclint(include_str!("fixtures/lintcfg/arclint.badseverity.json"))
        .unwrap_err();
    assert!(err.contains("Unknown lint severity"), "unexpected: {err}");
}

#[test]
fn arclint_loads_from_disk() {
    let tmp = TempDir::new("load");
    let err = arclint::load_arclint(tmp.path()).unwrap_err();
    assert!(
        err.contains("Unable to find '.arclint' file"),
        "unexpected: {err}"
    );

    std::fs::write(
        tmp.path().join(".arclint"),
        include_str!("fixtures/lintcfg/arclint.sample.json"),
    )
    .unwrap();
    let config = arclint::load_arclint(tmp.path()).unwrap();
    assert_eq!(
        arclint::selected_display_names(&config),
        vec!["JSON", "SPELL", "TXT"]
    );

    std::fs::write(tmp.path().join(".arclint"), "{oops").unwrap();
    let err = arclint::load_arclint(tmp.path()).unwrap_err();
    assert!(err.contains("valid JSON"), "unexpected: {err}");
}

#[test]
fn arclint_type_table_is_complete() {
    let available = arclint::available_types();
    assert_eq!(available.len(), 34);
    let mut sorted = available.clone();
    sorted.sort_unstable();
    assert_eq!(available, sorted);

    let displays = arclint::display_names();
    assert_eq!(displays.len(), 33);
    for name in [
        "TXT",
        "NAME",
        "JSON",
        "XML",
        "MERGECONFLICT",
        "GEN",
        "NOLINT",
        "CHMOD",
        "SPELL",
        "S&RX",
        "COMPOSER",
        "PHL",
        "JSHint",
        "JSCS",
        "CSSLint",
        "LESSC",
        "GJSLINT",
        "COFFEE",
        "flake8",
        "PYFLAKES",
        "PyLint",
        "PEP8",
        "RuboCop",
        "RUBY",
        "cppcheck",
        "C++ Google's Styleguide",
        "GOLINT",
        "HLINT",
        "C#",
        "PHPCS",
        "PHP",
        "PUPPETLINT",
        "XHP",
    ] {
        assert!(displays.contains(&name), "missing {name}");
    }
    assert_eq!(
        arclint::configuration_name_to_display("cpplint"),
        Some("C++ Google's Styleguide")
    );
    assert_eq!(
        arclint::configuration_name_to_display("jsonlint"),
        Some("JSON")
    );
}

#[test]
fn arclint_retain_selects_exact_names() {
    struct Fake {
        name: &'static str,
    }
    let config =
        arclint::parse_arclint(r#"{"linters": {"a": {"type": "cpplint"}, "b": {"type": "C#"}}}"#)
            .unwrap();
    assert_eq!(
        arclint::selected_display_names(&config),
        vec!["C++ Google's Styleguide", "C#"]
    );
    let mut linters = vec![
        Fake { name: "TXT" },
        Fake {
            name: "C++ Google's Styleguide",
        },
        Fake {
            name: "c++ google's styleguide",
        },
        Fake { name: "C#" },
    ];
    arclint::retain_selected(&mut linters, |linter| linter.name, &config);
    let names: Vec<&str> = linters.iter().map(|linter| linter.name).collect();
    assert_eq!(names, vec!["C++ Google's Styleguide", "C#"]);
}

#[test]
fn markers_rejects_bad_arguments() {
    assert_eq!(markers::run(&args(&["--help"])), ExitCode::SUCCESS);
    assert_eq!(markers::run(&args(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(markers::run(&args(&["--type"])), ExitCode::FAILURE);
    assert_eq!(markers::run(&args(&["--type", "tag"])), ExitCode::FAILURE);
    assert_eq!(markers::run(&args(&["extra"])), ExitCode::FAILURE);
}

#[test]
fn markers_lists_git_branches_as_grid() {
    let tmp = git_repo("grid");
    std::fs::write(tmp.path().join("a.txt"), "first\n").unwrap();
    git(tmp.path(), &["add", "a.txt"]);
    git(tmp.path(), &["commit", "-qm", "first"]);
    git(tmp.path(), &["checkout", "-qb", "feature"]);
    std::fs::write(tmp.path().join("b.txt"), "second\n").unwrap();
    git(tmp.path(), &["add", "b.txt"]);
    git(tmp.path(), &["commit", "-qm", "add thing"]);

    let found = markers::list_git_branches(tmp.path()).unwrap();
    assert_eq!(found.len(), 2);
    assert!(found[0].active);
    assert_eq!(found[0].name, "feature");

    // Fixed width: message truncation depends on the terminal.
    let text = markers::execute_with_client(tmp.path(), &[], Some(150), None).unwrap();
    let feature = git(tmp.path(), &["rev-parse", "--short", "feature"]);
    // The tree grid follows graph order: master (first commit) renders
    // first, the active feature marker second.
    assert!(text.contains("• master"), "got: {text}");
    assert!(text.contains("└  *  feature"), "got: {text}");
    assert!(text.contains(&feature), "got: {text}");
    assert!(text.contains("add thing"), "got: {text}");
    let feature_at = text.find("feature").unwrap();
    let master_at = text.find("master").unwrap();
    assert!(master_at < feature_at, "got: {text}");
    assert!(text.ends_with('\n'));

    let json = markers::execute(tmp.path(), &args(&["--json"])).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 2);
    assert_eq!(value[0]["active"], serde_json::json!(true));

    assert_eq!(markers::run_in(tmp.path(), &[]), ExitCode::SUCCESS);
}

#[test]
fn markers_bookmarks_fail_under_git_like_arc() {
    let tmp = git_repo("bookmark");
    std::fs::write(tmp.path().join("a.txt"), "x\n").unwrap();
    git(tmp.path(), &["add", "a.txt"]);
    git(tmp.path(), &["commit", "-qm", "first"]);

    let err = markers::execute(tmp.path(), &args(&["--type", "bookmark"])).unwrap_err();
    assert_eq!(
        err,
        "The version control system (\"git\") in the current working copy does not support bookmarks."
    );
    assert_eq!(
        markers::run_in(tmp.path(), &args(&["--type", "bookmark"])),
        ExitCode::FAILURE
    );
}

#[test]
fn markers_fails_outside_a_working_copy() {
    let tmp = TempDir::new("nowhere");
    let err = markers::execute(tmp.path(), &[]).unwrap_err();
    assert!(
        err.contains("not inside a working copy"),
        "unexpected: {err}"
    );
}
