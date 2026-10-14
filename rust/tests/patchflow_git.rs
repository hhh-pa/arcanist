//! `export`/`patch` flows through the wired library.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{conduit, export, patch};

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-patchflow-{tag}-{}-{}",
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
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
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

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

/// Serve canned response bodies in order; returns the base URI and the
/// observed (request line, body) pairs once all are served.
fn serve_seq(
    canned: Vec<&'static str>,
) -> (String, std::sync::mpsc::Receiver<Vec<(String, String)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        for body in canned {
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
                .filter_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                })
                .next()
                .unwrap_or(0);
            let mut raw = vec![0u8; len];
            stream.read_exact(&mut raw).unwrap();
            seen.push((
                head.lines().next().unwrap_or("").to_string(),
                String::from_utf8(raw).unwrap(),
            ));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        tx.send(seen).unwrap();
    });
    (base, rx)
}

fn seen_requests(rx: &std::sync::mpsc::Receiver<Vec<(String, String)>>) -> Vec<(String, String)> {
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
}

fn client_for(base: &str) -> conduit::Client {
    conduit::Client::new(base, "api-test-token").unwrap()
}

const RAW_DIFF: &str = "diff --git a/f.txt b/f.txt\nindex 257cc56..5716ca5 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-a\n+b\n";

#[test]
fn module_names_match_subcommands() {
    assert_eq!(patch::NAME, "patch");
    assert_eq!(export::NAME, "export");
}

#[test]
fn patch_rejects_bad_args_without_repo() {
    assert_eq!(patch::run(&argv(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(patch::run(&argv(&["--diff"])), ExitCode::FAILURE);
    assert_eq!(patch::run(&argv(&[])), ExitCode::FAILURE);
    assert_eq!(patch::run(&argv(&["D1", "--diff", "2"])), ExitCode::FAILURE);
    assert_eq!(
        patch::run(&argv(&["D1", "--arcbundle", "x"])),
        ExitCode::FAILURE
    );
}

#[test]
fn patch_help_succeeds() {
    let fx = Fixture::named("help");
    assert_eq!(
        patch::run_in(fx.path(), &argv(&["--help"])),
        ExitCode::SUCCESS
    );
    assert_eq!(
        export::run_in(fx.path(), &argv(&["--help"])),
        ExitCode::SUCCESS
    );
    let text = patch::execute(fx.path(), &argv(&["--help"])).unwrap();
    assert!(text.contains("usage: arc-rs patch"), "{text}");
}

#[test]
fn patch_preview_prints_patch_file_without_repo() {
    let fx = Fixture::named("preview");
    fx.write("fix.patch", RAW_DIFF);
    let patch_path = fx.dir.join("fix.patch").to_string_lossy().to_string();
    let out = patch::execute(fx.path(), &argv(&["--patch", &patch_path, "--preview"])).unwrap();
    assert_eq!(out, RAW_DIFF);
}

#[test]
fn patch_preview_reads_stdin_patch() {
    let fx = Fixture::named("stdin");
    let out = patch::execute_with_stdin(
        fx.path(),
        &argv(&["--patch", "-", "--preview"]),
        Some("x\n"),
    )
    .unwrap();
    assert_eq!(out, "x\n");
    let err = patch::execute_with_stdin(fx.path(), &argv(&["--patch", "-", "--preview"]), Some(""))
        .unwrap_err();
    assert!(err.contains("stdin"), "{err}");
}

#[test]
fn patch_applies_patch_file_to_repo() {
    let fx = Fixture::named("apply");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    // The patch file lives outside the repo: untracked files dirty the tree.
    let px = Fixture::named("apply-patch");
    px.write("fix.patch", RAW_DIFF);
    let patch_path = px.dir.join("fix.patch").to_string_lossy().to_string();

    let out = patch::execute(fx.path(), &argv(&["--patch", &patch_path])).unwrap();
    assert!(out.contains("Applied patch"), "{out}");
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("f.txt")).unwrap(),
        "b\n"
    );
}

#[test]
fn patch_refuses_dirty_tree_unless_forced() {
    let fx = Fixture::named("dirty");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.write("g.txt", "g\n");
    fx.commit_all("one");
    fx.write("g.txt", "dirty\n");
    let px = Fixture::named("dirty-patch");
    px.write("fix.patch", RAW_DIFF);
    let patch_path = px.dir.join("fix.patch").to_string_lossy().to_string();

    let err = patch::execute(fx.path(), &argv(&["--patch", &patch_path])).unwrap_err();
    assert!(err.contains("not clean"), "{err}");

    let out = patch::execute(fx.path(), &argv(&["--patch", &patch_path, "--force"])).unwrap();
    assert!(out.contains("Applied patch"), "{out}");
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("f.txt")).unwrap(),
        "b\n"
    );
}

#[test]
fn patch_reports_apply_failure() {
    let fx = Fixture::named("applfail");
    fx.init_repo();
    fx.write("f.txt", "something else\n");
    fx.commit_all("one");
    let px = Fixture::named("applfail-patch");
    px.write("fix.patch", RAW_DIFF);
    let patch_path = px.dir.join("fix.patch").to_string_lossy().to_string();

    let err = patch::execute(fx.path(), &argv(&["--patch", &patch_path])).unwrap_err();
    assert!(err.contains("git apply failed"), "{err}");
}

#[test]
fn patch_apply_requires_repo() {
    let fx = Fixture::named("norepo");
    fx.write("fix.patch", RAW_DIFF);
    let patch_path = fx.dir.join("fix.patch").to_string_lossy().to_string();
    // --force skips the clean check so the missing repo is what fails.
    let err = patch::execute(fx.path(), &argv(&["--patch", &patch_path, "--force"])).unwrap_err();
    assert!(err.contains("working copy"), "{err}");
}

#[test]
fn patch_fetches_revision_through_querydiffs_then_getrawdiff() {
    let query = r#"{"result": {"77": {"id": 77}}, "error_code": null}"#;
    let raw = r#"{"result": "diff --git a/x\n", "error_code": null}"#;
    let (base, rx) = serve_seq(vec![query, raw]);
    let client = client_for(&base);

    let text = patch::fetch_raw_diff_for_revision(&client, 123).unwrap();
    assert_eq!(text, "diff --git a/x\n");

    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].0.starts_with("POST /api/differential.querydiffs "),
        "{}",
        seen[0].0
    );
    assert!(seen[0].1.contains("revisionIDs"), "{}", seen[0].1);
    assert!(seen[0].1.contains("123"), "{}", seen[0].1);
    assert!(
        seen[1].0.starts_with("POST /api/differential.getrawdiff "),
        "{}",
        seen[1].0
    );
    assert!(seen[1].1.contains("77"), "{}", seen[1].1);
}

#[test]
fn patch_fetches_single_diff() {
    let raw = r#"{"result": "patch-bytes\n", "error_code": null}"#;
    let (base, rx) = serve_seq(vec![raw]);
    let client = client_for(&base);

    let text = patch::fetch_raw_diff_for_diff(&client, "9").unwrap();
    assert_eq!(text, "patch-bytes\n");

    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].0.starts_with("POST /api/differential.getrawdiff "),
        "{}",
        seen[0].0
    );
}

#[test]
fn patch_rejects_unknown_revision() {
    let (base, _rx) = serve_seq(vec![r#"{"result": {}, "error_code": null}"#]);
    let client = client_for(&base);
    let err = patch::fetch_raw_diff_for_revision(&client, 404).unwrap_err();
    assert!(
        err.contains("invalid or you don't have permission"),
        "{err}"
    );
}

#[test]
fn export_git_and_unified_from_working_copy() {
    let fx = Fixture::named("export");
    fx.init_repo();
    fx.write("uf.txt", "a\nb\n");
    fx.commit_all("one");
    fx.write("uf.txt", "a\nB\n");

    let git_out = export::execute(fx.path(), &argv(&["--git"])).unwrap();
    assert!(
        git_out.contains("diff --git a/uf.txt b/uf.txt"),
        "{git_out}"
    );
    assert!(git_out.contains("-b\n+B"), "{git_out}");

    let uni_out = export::execute(fx.path(), &argv(&["--unified"])).unwrap();
    assert!(uni_out.contains("diff --git uf.txt uf.txt"), "{uni_out}");
    assert!(!uni_out.contains("a/uf.txt"), "{uni_out}");
    assert!(uni_out.contains("--- uf.txt"), "{uni_out}");
}

#[test]
fn export_honors_explicit_base() {
    let fx = Fixture::named("base");
    fx.init_repo();
    fx.write("f.txt", "one\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "one\ntwo\n");
    fx.commit_all("two");

    let out = export::execute(fx.path(), &argv(&["--git", &first])).unwrap();
    assert!(out.contains("+two"), "{out}");

    let err = export::execute(fx.path(), &argv(&["--git", "does-not-exist"])).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn export_requires_exactly_one_format() {
    let fx = Fixture::named("format");
    assert_eq!(export::run(&argv(&[])), ExitCode::FAILURE);
    assert_eq!(
        export::run(&argv(&["--git", "--unified"])),
        ExitCode::FAILURE
    );
    assert_eq!(
        export::run(&argv(&["--git", "--arcbundle", "x"])),
        ExitCode::FAILURE
    );
    let err = export::execute(fx.path(), &argv(&[])).unwrap_err();
    assert!(err.contains("Specify one of"), "{err}");
}

#[test]
fn export_revision_through_conduit() {
    let query = r#"{"result": [{"id": 31}], "error_code": null}"#;
    let raw = r#"{"result": "raw-patch\n", "error_code": null}"#;
    let (base, rx) = serve_seq(vec![query, raw]);
    let client = client_for(&base);

    let text = export::fetch_raw_diff_for_revision(&client, 7).unwrap();
    assert_eq!(text, "raw-patch\n");
    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].0.contains("differential.querydiffs"),
        "{}",
        seen[0].0
    );
    assert!(
        seen[1].0.contains("differential.getrawdiff"),
        "{}",
        seen[1].0
    );
}

#[test]
fn export_single_diff_through_conduit() {
    let (base, rx) = serve_seq(vec![r#"{"result": "d\n", "error_code": null}"#]);
    let client = client_for(&base);
    assert_eq!(
        export::fetch_raw_diff_for_diff(&client, "5").unwrap(),
        "d\n"
    );
    assert_eq!(seen_requests(&rx).len(), 1);
}

#[test]
fn export_local_requires_repo() {
    let fx = Fixture::named("exnorepo");
    let err = export::execute(fx.path(), &argv(&["--git"])).unwrap_err();
    assert!(err.contains("working copy"), "{err}");
}
