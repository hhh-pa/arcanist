use super::*;

fn fresh_repo(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-git-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["init", "-q", "-b", "master"])
        .output()
        .unwrap();
    assert!(out.status.success());
    for (key, value) in [("user.email", "t@t"), ("user.name", "t")] {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["config", key, value])
            .output()
            .unwrap();
    }
    dir
}

#[test]
fn detects_work_trees() {
    let repo = fresh_repo("tree");
    assert!(is_inside_work_tree(&repo));
    let plain = std::env::temp_dir().join(format!("arc-rs-git-plain-{}", std::process::id()));
    if plain.exists() {
        std::fs::remove_dir_all(&plain).ok();
    }
    std::fs::create_dir_all(&plain).unwrap();
    assert!(!is_inside_work_tree(&plain));
    std::fs::remove_dir_all(&repo).ok();
    std::fs::remove_dir_all(&plain).ok();
}

#[test]
fn stdout_text_ok_and_err() {
    let repo = fresh_repo("text");
    let top = stdout_text(&repo, &["rev-parse", "--show-toplevel"]).unwrap();
    assert!(!top.is_empty());
    let err = stdout_text(&repo, &["rev-parse", "--verify", "no-such-rev"]).unwrap_err();
    assert!(!err.is_empty());
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn output_and_ok_match_exit_status() {
    let repo = fresh_repo("ok");
    assert!(ok(&repo, &["rev-parse"]));
    assert!(!ok(&repo, &["no-such-subcommand"]));
    let raw = output(&repo, &["rev-parse", "--show-toplevel"]).unwrap();
    assert!(raw.status.success());
    assert!(!raw.stdout.is_empty());
    std::fs::remove_dir_all(&repo).ok();
}
