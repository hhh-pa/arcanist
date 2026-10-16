//! `.arclint` config plus `markers` behavior through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::conduit::Client;
use arc_rs::{arclint, markers};

/// One-shot mock conduit server answering `body` to a single call.
fn serve_once(body: &'static str) -> (String, std::sync::mpsc::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        let head = String::from_utf8(head).unwrap();
        let len: usize = head
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length: ")
                    .or_else(|| line.strip_prefix("content-length: "))
                    .map(|v| v.trim().parse().unwrap())
            })
            .unwrap_or(0);
        let mut rest = vec![0u8; len];
        stream.read_exact(&mut rest).unwrap();
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(reply.as_bytes()).unwrap();
        tx.send(()).unwrap();
    });
    (base, rx)
}

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
        vec!["TXT", "SPELL", "JSON"]
    );

    std::fs::write(tmp.path().join(".arclint"), "{oops").unwrap();
    let err = arclint::load_arclint(tmp.path()).unwrap_err();
    assert!(err.contains("valid JSON"), "unexpected: {err}");
}

#[test]
fn arclint_type_table_is_complete() {
    let available = arclint::available_types();
    assert_eq!(available.len(), 34);
    // T1153 LINT-39: PHP class-map order (alphabetical by class name),
    // not sorted.
    assert_eq!(
        available,
        vec![
            "csslint",
            "csharp",
            "chmod",
            "gjslint",
            "coffeelint",
            "composer",
            "cppcheck",
            "cpplint",
            "filename",
            "flake8",
            "generated",
            "golint",
            "hlint",
            "jshint",
            "jsonlint",
            "json",
            "jscs",
            "lessc",
            "merge-conflict",
            "nolint",
            "pep8",
            "php",
            "phpcs",
            "phutil-library",
            "puppet-lint",
            "pyflakes",
            "pylint",
            "rubocop",
            "ruby",
            "script-and-regex",
            "spelling",
            "text",
            "xhpast",
            "xml",
        ]
    );

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
    // T1153 LINT-38: `type` takes configuration names only (`C#` is invalid).
    let config = arclint::parse_arclint(
        r#"{"linters": {"a": {"type": "cpplint"}, "b": {"type": "csharp"}}}"#,
    )
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
    // T1153 RB-D9: the grid workflow parses `git remote --verbose`
    // strictly, so this rendering fixture needs a remote (a
    // remote-less checkout fails; see `rbd9_remoteless_checkout_fails`).
    git(
        tmp.path(),
        &["remote", "add", "origin", "https://example.test/repo.git"],
    );

    let found = markers::list_git_branches(tmp.path()).unwrap();
    assert_eq!(found.len(), 2);
    assert!(found[0].active);
    assert_eq!(found[0].name, "feature");

    // Fixed width: message truncation depends on the terminal.
    // T1153 LAND-23: the grid needs a client (`differential.query`
    // answers empty here; no tracking refs, so nothing else fires).
    let (base, _served) = serve_once(r#"{"result": [], "error_code": null}"#);
    let client = Client::new(&base, "test-token").unwrap();
    let text = markers::execute_with_client(tmp.path(), &[], Some(150), Some(&client)).unwrap();
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
    // No `run_in` without a client here: the exit code would depend
    // on ambient ~/.arcrc (T1153 LAND-23 fails conduit-less grids;
    // see land23_grid_fails_without_conduit). The bookmarks test
    // below covers run_in's failure arm.
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

#[test]
fn land23_grid_fails_without_conduit() {
    // Without a client the workflow fails like PHP instead of
    // rendering an unannotated grid.
    let tmp = git_repo("land23-noclient");
    git(
        tmp.path(),
        &["commit", "-q", "--allow-empty", "-m", "first"],
    );
    git(
        tmp.path(),
        &["remote", "add", "origin", "https://example.test/repo.git"],
    );
    let err = markers::execute_with_client(tmp.path(), &[], Some(150), None).unwrap_err();
    assert!(
        err.contains("This command needs to communicate with Phabricator"),
        "unexpected: {err}"
    );
}

#[test]
fn rbd9_remoteless_checkout_fails_like_php() {
    // Empty `git remote --verbose` output throws (the empty line
    // misses the remote pattern), like PHP.
    let tmp = git_repo("rbd9-noremote");
    git(
        tmp.path(),
        &["commit", "-q", "--allow-empty", "-m", "first"],
    );
    let err = markers::query_git_remotes(tmp.path()).unwrap_err();
    assert_eq!(err, "Failed to match remote pattern against line \"\".");
    let err = markers::execute_with_client(tmp.path(), &[], Some(150), None).unwrap_err();
    assert_eq!(err, "Failed to match remote pattern against line \"\".");
}
