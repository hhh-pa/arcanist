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

#[path = "../src/set_config.rs"]
mod set_config;

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
            "arc-rs-cfgsys-set-{tag}-{}-{n}",
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
    assert_eq!(set_config::NAME, "set-config");
}

#[test]
fn run_help_and_bad_args() {
    assert_eq!(set_config::run(&["--help".to_string()]), ExitCode::SUCCESS);
    assert_eq!(set_config::run(&[]), ExitCode::FAILURE);
    assert_eq!(set_config::run(&["only".to_string()]), ExitCode::FAILURE);
    assert_eq!(
        set_config::run(&["--bogus".to_string(), "k".to_string(), "v".to_string()]),
        ExitCode::FAILURE
    );
}

#[test]
fn run_writes_user_config_under_temp_home() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("run");
    let _home = with_temp_home(tmp.path());
    assert_eq!(
        set_config::run(&["editor".to_string(), "nano".to_string()]),
        ExitCode::SUCCESS
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join(".arcrc")).unwrap()).unwrap();
    assert_eq!(doc["config"]["editor"], serde_json::json!("nano"));
    assert_eq!(
        set_config::run(&["editor".to_string(), String::new()]),
        ExitCode::SUCCESS
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join(".arcrc")).unwrap()).unwrap();
    assert!(!doc["config"].as_object().unwrap().contains_key("editor"));
    // Unknown keys warn but still write.
    assert_eq!(
        set_config::run(&["custom.key".to_string(), "v".to_string()]),
        ExitCode::SUCCESS
    );
    // Bad types fail without writing.
    assert_eq!(
        set_config::run(&["history.immutable".to_string(), "perhaps".to_string()]),
        ExitCode::FAILURE
    );
}

#[test]
fn local_writes_land_in_repo_meta_dir() {
    let tmp = TempDir::new("local");
    run_git(tmp.path(), &["init", "-q", "-b", "master"]);
    let message = set_config::apply_local(tmp.path(), "editor", "nano").unwrap();
    assert!(message.contains("local config"), "{message}");
    let stored = set_config::read_local_file(&set_config::local_config_path(tmp.path())).unwrap();
    assert_eq!(stored["editor"], serde_json::json!("nano"));
    // Coercion applies to local writes too.
    set_config::apply_local(tmp.path(), "history.immutable", "true").unwrap();
    let stored = set_config::read_local_file(&set_config::local_config_path(tmp.path())).unwrap();
    assert_eq!(stored["history.immutable"], serde_json::json!(true));
}
