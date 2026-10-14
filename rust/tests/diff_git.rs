//! `diff` against local git fixtures through the wired library.

use std::path::{Path, PathBuf};
use std::process::Command;

use arc_rs::diff;

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo(&self) {
        self.git(&["init", "-q", "-b", "main", "."]);
        self.git(&["config", "user.email", "test@example.test"]);
        self.git(&["config", "user.name", "test"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["config", "core.autocrlf", "false"]);
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn options(base: Option<&str>, json: bool) -> diff::Options {
    diff::Options {
        base: base.map(str::to_string),
        json,
    }
}

#[test]
fn module_name_matches_subcommand() {
    assert_eq!(diff::NAME, "diff");
}

#[test]
fn errors_outside_git_repo() {
    let fx = Fixture::named("not-a-repo");
    assert!(!diff::is_inside_git_repo(fx.path()));
    let err = diff::inspect(fx.path(), &options(None, false)).unwrap_err();
    assert!(err.contains("not inside a git repository"), "{err}");
}

#[test]
fn errors_in_empty_repo_without_commits() {
    let fx = Fixture::named("empty-repo");
    fx.init_repo();
    let err = diff::inspect(fx.path(), &options(None, true)).unwrap_err();
    assert!(err.contains("HEAD"), "{err}");
}

#[test]
fn rejects_invalid_explicit_base() {
    let fx = Fixture::named("bad-base");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    let err = diff::inspect(fx.path(), &options(Some("does-not-exist"), true)).unwrap_err();
    assert!(err.contains("invalid base revision"), "{err}");
}

#[test]
fn run_rejects_bad_args_without_repo() {
    use std::process::ExitCode;
    let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(diff::run(&argv(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(diff::run(&argv(&["--base"])), ExitCode::FAILURE);
    assert_eq!(diff::run(&argv(&["stray-path"])), ExitCode::FAILURE);
}

#[test]
fn json_diff_against_explicit_base() {
    let fx = Fixture::named("json-base");
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    fx.write("g.txt", "x\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nB2\nB3\nc\n");
    fx.git(&["mv", "g.txt", "renamed.txt"]);

    let out = diff::inspect(fx.path(), &options(Some(&first), true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(first));
    assert_eq!(doc["insertions"], serde_json::json!(2));
    assert_eq!(doc["deletions"], serde_json::json!(1));
    let files = doc["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{doc}");
    assert!(
        files.contains(&serde_json::json!({
            "path": "f.txt",
            "added": 2,
            "deleted": 1,
        })),
        "{doc}"
    );
    assert!(
        files.contains(&serde_json::json!({
            "path": "renamed.txt",
            "added": 0,
            "deleted": 0,
        })),
        "{doc}"
    );
}

#[test]
fn default_output_has_stat_and_patch() {
    let fx = Fixture::named("default-out");
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nB2\nB3\nc\n");

    let out = diff::inspect(fx.path(), &options(None, false)).unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("f.txt"), "{text}");
    assert!(text.contains("diff --git"), "{text}");
    assert!(
        text.contains("files changed") || text.contains("file changed"),
        "{text}"
    );
}

#[test]
fn default_base_is_head_without_upstream() {
    let fx = Fixture::named("head-base");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    assert_eq!(diff::resolve_base(fx.path(), None).unwrap(), head);
    let out = diff::inspect(fx.path(), &options(None, true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(head));
    assert_eq!(doc["insertions"], serde_json::json!(1));
}

#[test]
fn default_base_prefers_upstream_merge_base() {
    let fx = Fixture::named("upstream-base");
    fx.init_repo();
    fx.write("f.txt", "one\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "one\ntwo\n");
    fx.commit_all("two");
    // Local branch used as the configured upstream (no network involved).
    fx.git(&["branch", "frozen", &first]);
    fx.git(&["branch", "--set-upstream-to=frozen"]);

    assert_eq!(diff::resolve_base(fx.path(), None).unwrap(), first);
    let out = diff::inspect(fx.path(), &options(None, true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(first));
    assert_eq!(doc["insertions"], serde_json::json!(1));
}

#[test]
fn json_diff_is_empty_on_clean_tree() {
    let fx = Fixture::named("clean-tree");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("one");

    let out = diff::inspect(fx.path(), &options(None, true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(head));
    assert_eq!(doc["files"], serde_json::json!([]));
    assert_eq!(doc["insertions"], serde_json::json!(0));
    assert_eq!(doc["deletions"], serde_json::json!(0));
}
