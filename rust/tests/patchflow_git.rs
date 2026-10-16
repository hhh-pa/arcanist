//! `export`/`patch` flows through the wired library.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{conduit, export, hg, land, patch, svn};

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

/// Owned HOME with a token for `conduit_base` (T1153 EXPORT-E30 harness
/// for conduit-backed CLI paths).
fn upx_home_with_token(tag: &str, conduit_base: &str) -> (Fixture, common::HomeGuard) {
    let home = Fixture::named(tag);
    let after_scheme = conduit_base.split("://").nth(1).unwrap_or(conduit_base);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let scheme = conduit_base.split("://").next().unwrap_or("http");
    let key = format!("{scheme}://{authority}/api/");
    std::fs::write(
        home.path().join(".arcrc"),
        format!(r#"{{"hosts": {{"{key}": {{"token": "t"}}}}}}"#),
    )
    .unwrap();
    let guard = common::with_owned_home(home.path().to_path_buf());
    (home, guard)
}

fn upx_user_routes() -> Vec<&'static str> {
    vec![
        r#"{"result": {"phid": "PHID-USER-1"}, "error_code": null}"#,
        r#"{"result": {"1": {"realName": "Ada"}}, "error_code": null}"#,
    ]
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

/// Adapt a blocking conduit client to the `patch` call closure.
fn patch_call(
    client: &conduit::Client,
) -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> + '_ {
    |method, params| client.call(method, params).map_err(|e| e.to_string())
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
    let out = patch::execute(fx.path(), &argv(&["--help"])).unwrap();
    assert!(out.stdout.contains("usage: arc patch"), "{}", out.stdout);
}

#[test]
fn patch_preview_prints_patch_file_without_repo() {
    let fx = Fixture::named("preview");
    fx.write("fix.patch", RAW_DIFF);
    let patch_path = fx.dir.join("fix.patch").to_string_lossy().to_string();
    let out = patch::execute(fx.path(), &argv(&["--patch", &patch_path, "--preview"])).unwrap();
    assert_eq!(out.stdout, RAW_DIFF);
}

#[test]
fn patch_preview_reads_stdin_patch() {
    let fx = Fixture::named("stdin");
    let out = patch::execute_with_stdin(
        fx.path(),
        &argv(&["--patch", "-", "--preview"]),
        Some("diff --git a/f b/f\n"),
    )
    .unwrap();
    assert_eq!(out.stdout, "diff --git a/f b/f\n");
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

    let out = patch::execute(
        fx.path(),
        &argv(&["--patch", &patch_path, "--nocommit", "--nobranch"]),
    )
    .unwrap();
    assert!(out.stderr.contains("APPLIED"), "{}", out.stderr);
    assert_eq!(
        std::fs::read_to_string(fx.dir.join("f.txt")).unwrap(),
        "b\n"
    );
    assert_eq!(fx.git(&["branch", "--show-current"]), "main");
}

#[test]
fn patch_branches_by_default() {
    let fx = Fixture::named("branch");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    let px = Fixture::named("branch-patch");
    px.write("fix.patch", RAW_DIFF);
    let patch_path = px.dir.join("fix.patch").to_string_lossy().to_string();

    let out = patch::execute(fx.path(), &argv(&["--patch", &patch_path, "--nocommit"])).unwrap();
    assert!(out.stderr.contains("APPLIED"), "{}", out.stderr);
    assert_eq!(fx.git(&["branch", "--show-current"]), "arcpatch");
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

    let mut io = land::Io::canned(&["n"]);
    let err = patch::execute_with_io(
        fx.path(),
        &argv(&["--patch", &patch_path]),
        &mut io,
        &patch::Backends::default(),
    )
    .unwrap_err();
    assert!(
        err.contains("You can not continue with uncommitted changes"),
        "{err}"
    );

    let out = patch::execute(
        fx.path(),
        &argv(&[
            "--patch",
            &patch_path,
            "--force",
            "--nocommit",
            "--nobranch",
        ]),
    )
    .unwrap();
    assert!(out.stderr.contains("APPLIED"), "{}", out.stderr);
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

    let err = patch::execute(
        fx.path(),
        &argv(&["--patch", &patch_path, "--nocommit", "--nobranch"]),
    )
    .unwrap_err();
    assert_eq!(err, "Unable to apply patch!");
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
fn patch_renders_revision_from_querydiffs_dicts() {
    let query = r#"{"result": {"77": {"id": "77", "changes": [
        {"type": "2", "fileType": "1", "oldPath": "a.txt", "currentPath": "a.txt",
         "hunks": [{"oldOffset": "1", "oldLength": "1", "newOffset": "1",
                    "newLength": "1", "corpus": "-a\n+b\n"}]}
      ]}}, "error_code": null}"#;
    let (base, rx) = serve_seq(vec![query]);
    let client = client_for(&base);
    let call = patch_call(&client);

    let bundle = patch::fetch_revision_bundle(&call, 123).unwrap();
    assert_eq!(
        bundle.text,
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\n"
    );
    assert_eq!(bundle.revision_id, Some(123));

    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].0.starts_with("POST /api/differential.querydiffs "),
        "{}",
        seen[0].0
    );
    assert!(seen[0].1.contains("revisionIDs"), "{}", seen[0].1);
    assert!(seen[0].1.contains("123"), "{}", seen[0].1);
}

#[test]
fn patch_fetches_single_diff() {
    let query = r#"{"result": {"9": {"id": "9", "changes": [
        {"type": "2", "fileType": "1", "oldPath": "a.txt", "currentPath": "a.txt",
         "hunks": [{"oldOffset": "1", "oldLength": "1", "newOffset": "1",
                    "newLength": "1", "corpus": "-a\n+b\n"}]}
      ]}}, "error_code": null}"#;
    let (base, rx) = serve_seq(vec![query]);
    let client = client_for(&base);
    let call = patch_call(&client);

    let bundle = patch::fetch_diff_bundle(&call, 9).unwrap();
    assert!(
        bundle.text.contains("@@ -1 +1 @@\n-a\n+b\n"),
        "{}",
        bundle.text
    );

    let seen = seen_requests(&rx);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].0.starts_with("POST /api/differential.querydiffs "),
        "{}",
        seen[0].0
    );
}

#[test]
fn patch_rejects_unknown_revision() {
    let (base, _rx) = serve_seq(vec![r#"{"result": {}, "error_code": null}"#]);
    let client = client_for(&base);
    let call = patch_call(&client);
    let err = patch::fetch_revision_bundle(&call, 404).unwrap_err();
    assert!(
        err.contains("invalid or you don't have permission"),
        "{err}"
    );
}

#[test]
fn export_git_and_unified_from_working_copy() {
    // T1153 EXPORT-E30/E2/E5/E6: local text exports need conduit, cover
    // the committed base..HEAD range, and render from the bundle.
    let fx = Fixture::named("export");
    fx.init_repo();
    fx.write("uf.txt", "a\nb\n");
    fx.commit_all("one");
    fx.write("uf.txt", "a\nB\n");
    fx.commit_all("two");
    let mut canned = upx_user_routes();
    canned.extend(upx_user_routes());
    let (base, rx) = serve_seq(canned);
    let (_home, _guard) = upx_home_with_token("export-home", &base);

    let git_out = export::execute(
        fx.path(),
        &argv(&["--git", "--conduit-uri", &base, "HEAD~1"]),
    )
    .unwrap();
    assert!(
        git_out.contains("diff --git a/uf.txt b/uf.txt"),
        "{git_out}"
    );
    assert!(git_out.contains("-b\n+B"), "{git_out}");

    let uni_out = export::execute(
        fx.path(),
        &argv(&["--unified", "--conduit-uri", &base, "HEAD~1"]),
    )
    .unwrap();
    assert!(uni_out.contains("Index: uf.txt"), "{uni_out}");
    assert!(
        uni_out.contains("==================================================================="),
        "{uni_out}"
    );
    assert!(uni_out.contains("--- uf.txt"), "{uni_out}");
    assert!(uni_out.contains("+++ uf.txt"), "{uni_out}");
    assert!(!uni_out.contains("diff --git"), "{uni_out}");
    assert_eq!(seen_requests(&rx).len(), 4);
}

#[test]
fn export_honors_explicit_base() {
    let fx = Fixture::named("base");
    fx.init_repo();
    fx.write("f.txt", "one\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "one\ntwo\n");
    fx.commit_all("two");

    // T1153 EXPORT-E30/E3: conduit first; explicit bases merge with HEAD.
    let (base, rx) = serve_seq(upx_user_routes());
    let (_home, _guard) = upx_home_with_token("base-home", &base);
    let out =
        export::execute(fx.path(), &argv(&["--git", "--conduit-uri", &base, &first])).unwrap();
    assert!(out.contains("+two"), "{out}");

    let err = export::execute(
        fx.path(),
        &argv(&["--git", "--conduit-uri", &base, "does-not-exist"]),
    )
    .unwrap_err();
    assert!(
        err.contains("Unable to find any git commit named 'does-not-exist'"),
        "{err}"
    );
    assert_eq!(seen_requests(&rx).len(), 2);
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
    // T1153 EXPORT-E30: without conduit config the conduit error wins...
    let fx = Fixture::named("exnorepo");
    let err = export::execute(
        fx.path(),
        &argv(&["--git", "--conduit-uri", "https://export-e30.invalid/"]),
    )
    .unwrap_err();
    assert!(err.contains("no token"), "{err}");
    // ...while configured conduit reaches the working-copy check.
    let (base, _rx) = serve_seq(vec![]);
    let (_home, _guard) = upx_home_with_token("exnorepo-home", &base);
    let err = export::execute(fx.path(), &argv(&["--git", "--conduit-uri", &base])).unwrap_err();
    assert!(err.contains("working copy"), "{err}");
}

fn fake_hg_for_patch(dir: &Path) -> hg::HgCommand {
    #[cfg(windows)]
    let (name, content) = ("fakehg-patch.bat", include_str!("fixtures/vcs/fakehg.bat"));
    #[cfg(not(windows))]
    let (name, content) = ("fakehg-patch.sh", include_str!("fixtures/vcs/fakehg.sh"));
    let script = dir.join(name);
    std::fs::write(&script, content).unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    #[cfg(windows)]
    {
        hg::HgCommand::custom("cmd", &["/C", &script.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        hg::HgCommand::custom(&script.to_string_lossy(), &[])
    }
}

fn fake_svn_for_patch(dir: &Path) -> svn::SvnCommand {
    #[cfg(windows)]
    let (name, content) = (
        "fakesvn-patch.bat",
        include_str!("fixtures/vcs/fakesvn.bat"),
    );
    #[cfg(not(windows))]
    let (name, content) = ("fakesvn-patch.sh", include_str!("fixtures/vcs/fakesvn.sh"));
    let script = dir.join(name);
    std::fs::write(&script, content).unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    #[cfg(windows)]
    {
        svn::SvnCommand::custom("cmd", &["/C", &script.to_string_lossy()])
    }
    #[cfg(not(windows))]
    {
        svn::SvnCommand::custom(&script.to_string_lossy(), &[])
    }
}

#[test]
fn patch_hg_flow_via_fake_binary() {
    let fx = Fixture::named("hg-fake");
    std::fs::create_dir(fx.dir.join(".hg")).unwrap();
    let hg_cmd = fake_hg_for_patch(fx.path());
    let backends = patch::Backends {
        hg: hg_cmd,
        svn: svn::SvnCommand::system(),
    };
    fx.write("fix.patch", RAW_DIFF);
    let patch_path = fx.dir.join("fix.patch").to_string_lossy().to_string();
    let mut io = land::Io::buffered();
    let out = patch::execute_with_io(
        fx.path(),
        &argv(&[
            "--patch",
            &patch_path,
            "--force",
            "--nocommit",
            "--nobranch",
        ]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert!(out.stderr.contains("OKAY"), "{}", out.stderr);
    let log = std::fs::read_to_string(fx.dir.join(".fakehg.log")).unwrap();
    assert!(log.contains("import --no-commit"), "{log}");
}

#[test]
fn patch_svn_mode_change_via_fake_binary() {
    let fx = Fixture::named("svn-fake");
    std::fs::create_dir(fx.dir.join(".svn")).unwrap();
    let svn_cmd = fake_svn_for_patch(fx.path());
    let backends = patch::Backends {
        hg: hg::HgCommand::system(),
        svn: svn_cmd,
    };
    fx.write(
        "fix.patch",
        "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n",
    );
    let patch_path = fx.dir.join("fix.patch").to_string_lossy().to_string();
    let mut io = land::Io::buffered();
    let out = patch::execute_with_io(
        fx.path(),
        &argv(&["--patch", &patch_path, "--force"]),
        &mut io,
        &backends,
    )
    .unwrap();
    assert!(out.stdout.contains("OKAY"), "{}", out.stdout);
    let log = std::fs::read_to_string(fx.dir.join(".fakesvn.log")).unwrap();
    assert!(log.contains("propset"), "{log}");
    assert!(log.contains("svn:executable"), "{log}");
}

#[test]
fn patch_binary_renders_and_applies() {
    assert_eq!(
        patch::sha1_hex(b"abc"),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(patch::base85_encode(b""), "@\n");
    assert_eq!(patch::base64_decode("aGk=").unwrap(), b"hi");
    let fx = Fixture::named("bin-int");
    fx.init_repo();
    let old_bytes: &[u8] = &[0, 1, 2, 3, 4, 5, 250, 251, 252];
    let new_bytes: &[u8] = &[0, 1, 2, 3, 9, 9, 9, 250, 251, 252, 10, 11];
    std::fs::write(fx.dir.join("a.bin"), old_bytes).unwrap();
    fx.git(&["add", "."]);
    fx.git(&["commit", "-m", "bin"]);
    let mut changes = vec![patch::BundleChange {
        change_type: 2,
        file_type: 3,
        old_path: Some("a.bin".to_string()),
        current_path: Some("a.bin".to_string()),
        old_mode: None,
        new_mode: None,
        hunks: Vec::new(),
        old_binary_phid: None,
        new_binary_phid: None,
        away_paths: Vec::new(),
        original_file_data: Some(old_bytes.to_vec()),
        current_file_data: Some(new_bytes.to_vec()),
    }];
    let text =
        patch::render_git_patch_with_blobs(&mut changes, &|_: &str| panic!("inline")).unwrap();
    assert!(text.contains("GIT binary patch\n"), "{text}");
    let bundle = patch::Bundle {
        text,
        ..Default::default()
    };
    let mut io = land::Io::buffered();
    patch::git_apply_patch(fx.path(), &bundle, &mut io).unwrap();
    assert_eq!(std::fs::read(fx.dir.join("a.bin")).unwrap(), new_bytes);
}

#[test]
fn patch_rejects_malformed_patch_file() {
    let fx = Fixture::named("malformed");
    fx.init_repo();
    fx.write("bad.patch", "not a patch\n");
    let bad = fx.dir.join("bad.patch").to_string_lossy().to_string();
    let err = patch::execute(fx.path(), &argv(&["--patch", &bad, "--force"])).unwrap_err();
    assert!(
        err.starts_with("Diff Parse Exception: Expected a hunk header"),
        "{err}"
    );
    fx.write("empty.patch", "");
    let empty = fx.dir.join("empty.patch").to_string_lossy().to_string();
    assert_eq!(
        patch::execute(fx.path(), &argv(&["--patch", &empty, "--force"])).unwrap_err(),
        "Can't parse an empty diff!"
    );
}

#[test]
fn patch_gap2_helpers_cover_all_cases() {
    for valid in [
        "Index: /path/to/file.ext\n",
        "Modified: foo\n",
        "Property changes on: foo\n",
        "commit 59bcc3ad6775562f845953cf01624225\n",
        "diff --git a/f b/f\n",
        "rcsdiff -u a b\n",
        "--- a/old.txt\t2024-01-02 03:04:05\n",
        "Binary files a 2024-01-02 and b 2024-01-02 differ\n",
        "diff -r abc123 file.txt\n",
        "# c\ndiff --git a/f b/f\n",
    ] {
        assert!(patch::validate_patch_text(valid).is_ok(), "{valid:?}");
    }
    for invalid in ["commit XYZ\n", "--- no-date\n", "hello\n"] {
        assert!(patch::validate_patch_text(invalid).is_err(), "{invalid:?}");
    }
    let err = patch::validate_patch_text("not a patch\n").unwrap_err();
    let message = "Expected a hunk header, like 'Index: /path/to/file.ext' (svn), \
         'Property changes on: /path/to/file.ext' (svn properties), \
         'commit 59bcc3ad6775562f845953cf01624225' (git show), 'diff --git' \
         (git diff), '--- filename' (unified diff), or 'diff -r' (hg diff or patch).";
    let full = format!("Diff Parse Exception: {message}\n\n   >>>        1   not a patch\n");
    assert_eq!(err, full);
    let fx = Fixture::named("with-seam");
    let hg_cmd = fake_hg_for_patch(fx.path());
    let backends = patch::Backends {
        hg: hg_cmd,
        svn: svn::SvnCommand::system(),
    };
    let bundle = patch::Bundle {
        base_revision: Some("abc123".to_string()),
        ..Default::default()
    };
    let mut io = land::Io::buffered();
    assert!(
        patch::ensure_base_present_with(fx.path(), patch::Vcs::Hg, &bundle, &mut io, &backends)
            .unwrap()
    );
}
