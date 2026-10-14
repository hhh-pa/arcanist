// Shared-helper shims: the module under test addresses the library as
// `arc_rs::...` (via its crate alias), which resolves to these includes
// in test builds and to the real library once the module is wired in.
#[allow(dead_code)]
#[path = "../src/conduit.rs"]
mod conduit;
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/git.rs"]
mod git;

#[path = "../src/get_config.rs"]
mod get_config;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-cfgsys-get-{tag}-{}-{n}",
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

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct HomeGuard {
    saved_home: Option<std::ffi::OsString>,
    saved_profile: Option<std::ffi::OsString>,
}

fn with_temp_home(dir: &Path) -> HomeGuard {
    let guard = HomeGuard {
        saved_home: std::env::var_os("HOME"),
        saved_profile: std::env::var_os("USERPROFILE"),
    };
    unsafe {
        std::env::set_var("HOME", dir);
        std::env::set_var("USERPROFILE", dir);
    }
    guard
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.saved_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.saved_profile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }
}

fn run_git(dir: &Path, args: &[&str]) {
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
}

#[test]
fn module_name_matches_arc() {
    assert_eq!(get_config::NAME, "get-config");
}

#[test]
fn run_help_and_bad_flags() {
    assert_eq!(get_config::run(&["--help".to_string()]), ExitCode::SUCCESS);
    assert_eq!(get_config::run(&["--bogus".to_string()]), ExitCode::FAILURE);
}

#[test]
fn run_reads_empty_home() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("run");
    let _home = with_temp_home(tmp.path());
    assert_eq!(get_config::run(&[]), ExitCode::SUCCESS);
    assert_eq!(
        get_config::run(&["editor".to_string(), "--show-source".to_string()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn fixtures_drive_expected_resolution() {
    let tmp = TempDir::new("fixtures");
    std::fs::write(
        tmp.path().join(".arcconfig"),
        include_str!("fixtures/cfgsys/arcconfig.sample.json"),
    )
    .unwrap();
    let user_file = tmp.path().join(".arcrc");
    std::fs::write(
        &user_file,
        include_str!("fixtures/cfgsys/arcrc.sample.json"),
    )
    .unwrap();
    let sources = get_config::load_sources(tmp.path(), Some(&user_file)).unwrap();
    assert_eq!(
        get_config::current_value("phabricator.uri", &sources).unwrap(),
        ("project", serde_json::json!("https://phabricator.example/"))
    );
    assert_eq!(
        get_config::current_value("editor", &sources).unwrap(),
        ("user", serde_json::json!("nano"))
    );
    assert_eq!(
        get_config::current_value("history.immutable", &sources)
            .unwrap()
            .0,
        "project"
    );
    assert!(get_config::all_keys(&sources).contains(&"editor".to_string()));
}

#[test]
fn local_config_beats_project_in_git_fixture() {
    let tmp = TempDir::new("localwins");
    run_git(tmp.path(), &["init", "-q", "-b", "master"]);
    std::fs::write(tmp.path().join(".arcconfig"), r#"{"editor": "proj-ed"}"#).unwrap();
    let local_dir = tmp.path().join(".git").join("arc");
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(
        local_dir.join("config"),
        include_str!("fixtures/cfgsys/local-config.sample.json"),
    )
    .unwrap();

    let local = get_config::read_local_config(tmp.path()).unwrap();
    assert_eq!(local["editor"], serde_json::json!("local-ed"));
    let sources = get_config::load_sources(tmp.path(), None).unwrap();
    assert_eq!(
        get_config::current_value("editor", &sources).unwrap(),
        ("local", serde_json::json!("local-ed"))
    );
    let text = get_config::render("editor", &sources, true);
    assert!(text.contains("local-ed"), "{text}");
    assert!(text.contains("proj-ed"), "{text}");
}
