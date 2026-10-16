//! Workflow tests for `arc lint`: flags, exit codes, renderers,
//! changed-line filtering, patching, and amend flows against real git
//! repositories.

use arc_rs::lint;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = Path::new("target").join("lint-text-tmp").join(format!(
            "{}-{}-{n}",
            name,
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-c")
        .arg("core.autocrlf=false")
        .arg("-c")
        .arg("core.eol=lf")
        .arg("-c")
        .arg("user.email=lint@test")
        .arg("-c")
        .arg("user.name=lint")
        .arg("-c")
        .arg("init.defaultBranch=main")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A git repo with a text-only `.arclint`.
fn git_repo(name: &str) -> TempDir {
    let tmp = TempDir::new(name);
    git(tmp.path(), &["init"]);
    tmp.write(".arclint", r#"{"linters": {"text": {"type": "text"}}}"#);
    tmp
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn run_in(dir: &Path, words: &[&str]) -> ExitCode {
    let argv = args(words);
    let mut empty = BufReader::new("".as_bytes());
    lint::run_in_with_input(dir, &argv, &mut empty)
}

fn run_in_with(dir: &Path, words: &[&str], stdin_text: &str) -> ExitCode {
    let argv = args(words);
    let mut input = Cursor::new(stdin_text.as_bytes().to_vec());
    lint::run_in_with_input(dir, &argv, &mut input)
}

fn code(value: ExitCode) -> Option<u8> {
    // `ExitCode` exposes no accessor; probe the values we assert.
    (0..=3u8).find(|expected| value == ExitCode::from(*expected))
}

#[test]
fn lint_help_and_usage_errors() {
    let tmp = git_repo("help");
    assert_eq!(run_in(tmp.path(), &["--help"]), ExitCode::SUCCESS);
    assert_eq!(run_in(tmp.path(), &["--bogus"]), ExitCode::FAILURE);
    assert_eq!(
        run_in(tmp.path(), &["--apply-patches", "--never-apply-patches"]),
        ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &["--everything", "file.txt"]),
        ExitCode::FAILURE
    );
    assert_eq!(run_in(tmp.path(), &["--rev"]), ExitCode::FAILURE);
}

#[test]
fn lint_requires_a_working_copy_and_an_engine() {
    let outside = std::env::temp_dir().join(format!("lint-text-outside-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    assert_eq!(run_in(&outside, &["--output", "none"]), ExitCode::FAILURE);
    std::fs::remove_dir_all(&outside).ok();

    // A repo without `.arclint` has no engine.
    let tmp = TempDir::new("noengine");
    git(tmp.path(), &["init"]);
    tmp.write("file.txt", "ok\n");
    assert_eq!(
        run_in(tmp.path(), &["file.txt", "--output", "none"]),
        ExitCode::FAILURE
    );
}

#[test]
fn lint_unknown_engine_and_renderer_fail() {
    let tmp = git_repo("badengine");
    tmp.write("file.txt", "ok\n");
    assert_eq!(
        run_in(tmp.path(), &["file.txt", "--engine", "BogusEngine"]),
        ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &["file.txt", "--output", "bogus"]),
        ExitCode::FAILURE
    );
    assert_eq!(
        run_in(tmp.path(), &["missing.txt", "--output", "none"]),
        ExitCode::FAILURE
    );
}

#[test]
fn lint_exit_codes_follow_severity() {
    let tmp = git_repo("codes");
    tmp.write("error.txt", "ok\nx\ty\n");
    tmp.write("warn.txt", &format!("{}\n", "x".repeat(81)));
    tmp.write("clean.txt", "ok\n");

    assert_eq!(
        code(run_in(tmp.path(), &["error.txt", "--output", "none"])),
        Some(2)
    );
    assert_eq!(
        code(run_in(tmp.path(), &["warn.txt", "--output", "none"])),
        Some(1)
    );
    assert_eq!(
        code(run_in(tmp.path(), &["clean.txt", "--output", "none"])),
        Some(0)
    );
    // Autofixes left unapplied do not fail the run.
    tmp.write("auto.txt", "ok  \n");
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["auto.txt", "--output", "none", "--never-apply-patches"]
        )),
        Some(0)
    );
}

#[test]
fn lint_severity_minimum_filters_and_accepts_anything() {
    let tmp = git_repo("severity");
    tmp.write("warn.txt", &format!("{}\n", "x".repeat(81)));
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["warn.txt", "--output", "none", "--severity", "error"]
        )),
        Some(0)
    );
    // PHP validates nothing: an unknown minimum shows everything.
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["warn.txt", "--output", "none", "--severity", "bogus"]
        )),
        Some(1)
    );
}

#[test]
fn lint_summary_renderer_writes_exact_bytes() {
    let tmp = git_repo("summary");
    tmp.write("tab.txt", "ok\nx\ty\n");
    let out = tmp.path().join("out.txt");
    let out_arg = out.to_string_lossy().to_string();
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["tab.txt", "--output", "summary", "--outfile", &out_arg]
        )),
        Some(2)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "tab.txt:2:Error: Tab Literal\n"
    );
    // Clean files append the OKAY line through the same sink.
    tmp.write("clean.txt", "ok\n");
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["clean.txt", "--output", "summary", "--outfile", &out_arg]
        )),
        Some(0)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "   OKAY  No lint messages.\n"
    );
}

#[test]
fn lint_compiler_json_xml_none_renderers() {
    let tmp = git_repo("renderers");
    tmp.write("tab.txt", "ok\nx\ty\n");
    let out = tmp.path().join("out.txt");
    let out_arg = out.to_string_lossy().to_string();

    assert_eq!(
        code(run_in(
            tmp.path(),
            &["tab.txt", "--output", "compiler", "--outfile", &out_arg]
        )),
        Some(2)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "tab.txt:2:Error (TXT2) Configure your editor to use spaces for indentation.\n"
    );

    assert_eq!(
        code(run_in(
            tmp.path(),
            &["tab.txt", "--output", "json", "--outfile", &out_arg]
        )),
        Some(2)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(parsed["tab.txt"][0]["code"], "TXT2");
    assert_eq!(parsed["tab.txt"][0]["line"], 2);
    assert_eq!(parsed["tab.txt"][0]["char"], 2);
    assert_eq!(parsed["tab.txt"][0]["severity"], "error");
    assert_eq!(parsed["tab.txt"][0]["context"], "ok\nx\ty\n");
    assert!(std::fs::read_to_string(&out).unwrap().contains("tab.txt"));

    assert_eq!(
        code(run_in(
            tmp.path(),
            &["tab.txt", "--output", "xml", "--outfile", &out_arg]
        )),
        Some(2)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <checkstyle version=\"4.3\">\n\
         \x20\x20<file name=\"tab.txt\">\n\
         \x20\x20\x20\x20<error line=\"2\" column=\"2\" severity=\"error\" message=\"Configure your editor to use spaces for indentation.\" source=\"TXT2\"/>\n\
         \x20\x20</file>\n\
         </checkstyle>\n"
    );

    assert_eq!(
        code(run_in(
            tmp.path(),
            &["tab.txt", "--output", "none", "--outfile", &out_arg]
        )),
        Some(2)
    );
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "");
}

#[test]
fn lint_console_renderer_writes_exact_bytes() {
    let tmp = git_repo("console");
    tmp.write("c.txt", "ok  \n");
    let out = tmp.path().join("out.txt");
    let out_arg = out.to_string_lossy().to_string();
    assert_eq!(
        code(run_in(
            tmp.path(),
            &[
                "c.txt",
                "--output",
                "console",
                "--outfile",
                &out_arg,
                "--never-apply-patches"
            ]
        )),
        Some(0)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        ">>> Lint for c.txt:\n\
         \n\
         \n\
         \x20\x20\x20Auto-Fix  (TXT6) Trailing Whitespace\n\
         \x20\x20\x20\x20This line contains trailing whitespace. Consider setting up your editor to\n\
         \x20\x20\x20\x20automatically remove trailing whitespace.\n\
         \n\
         \x20\x20\x20\x20>>> -      1 ok  \n\
         \x20\x20\x20\x20\x20\x20\x20\x20+        ok\n\
         \x20\x20\x20OKAY  No lint messages.\n"
    );
}

#[test]
fn lint_autofixes_apply_without_prompting() {
    let tmp = git_repo("autofix");
    let file = tmp.write("auto.txt", "ok  \n");
    // Only the console renderer patches; autofixes apply silently.
    assert_eq!(code(run_in_with(tmp.path(), &["auto.txt"], "")), Some(0));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ok\n");
    // No `.linted` temp files are left behind.
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".linted"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn lint_patch_prompts_accept_and_decline() {
    // TXT4 is patchable but not an autofix: the console renderer asks.
    let tmp = git_repo("prompt");
    let file = tmp.write("noeol.txt", "hello\nworld");
    // `--output none` never patches: the error stays unresolved.
    assert_eq!(
        code(run_in_with(
            tmp.path(),
            &["noeol.txt", "--output", "none"],
            "n\n"
        )),
        Some(2)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\nworld");

    assert_eq!(
        code(run_in_with(tmp.path(), &["noeol.txt"], "n\n")),
        Some(2)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\nworld");

    assert_eq!(
        code(run_in_with(tmp.path(), &["noeol.txt"], "y\n")),
        Some(0)
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\nworld\n");

    // `--output none` never patches, even with `--apply-patches`,
    // so the error stays unresolved.
    tmp.write("noeol2.txt", "hello\nworld");
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["noeol2.txt", "--apply-patches", "--output", "none"]
        )),
        Some(2)
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("noeol2.txt")).unwrap(),
        "hello\nworld"
    );
    assert_eq!(
        code(run_in(tmp.path(), &["noeol2.txt", "--apply-patches"])),
        Some(0)
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("noeol2.txt")).unwrap(),
        "hello\nworld\n"
    );
}

#[test]
fn lint_amend_all_amends_and_decline_aborts() {
    let tmp = git_repo("amend");
    // The amend flow shells out to `git commit --amend`, which needs an
    // identity the `-c` flags on the helper cannot provide.
    git(tmp.path(), &["config", "user.email", "lint@test"]);
    git(tmp.path(), &["config", "user.name", "lint"]);
    tmp.write("fix.txt", "ok  \n");
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-m", "base"]);

    assert_eq!(
        code(run_in(
            tmp.path(),
            &["fix.txt", "--apply-patches", "--amend-all"]
        )),
        Some(0)
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("fix.txt")).unwrap(),
        "ok\n"
    );
    assert_eq!(git(tmp.path(), &["status", "--porcelain"]).trim(), "");
    assert_eq!(
        git(tmp.path(), &["show", "HEAD:fix.txt"]),
        "ok\n".to_string()
    );

    // A declined amend keeps the patch in the worktree and fails.
    tmp.write("fix2.txt", "hello\nworld");
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-m", "second"]);
    assert_eq!(
        code(run_in_with(
            tmp.path(),
            &["fix2.txt", "--apply-patches", "--amend-autofixes"],
            "n\n"
        )),
        Some(1)
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("fix2.txt")).unwrap(),
        "hello\nworld\n"
    );
    assert_eq!(
        git(tmp.path(), &["show", "HEAD:fix2.txt"]),
        "hello\nworld".to_string()
    );
}

#[test]
fn lint_everything_covers_tracked_only() {
    let tmp = git_repo("everything");
    tmp.write("tracked.txt", "ok\nx\ty\n");
    tmp.write("ignored.txt", "ok\n");
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-m", "base"]);
    // Untracked files are invisible to `--everything`.
    tmp.write("untracked.txt", "ok\nx\ty\n");
    assert_eq!(
        code(run_in(tmp.path(), &["--everything", "--output", "none"])),
        Some(2)
    );
    // ...and to the status selection (which is empty here).
    // T1153 R4 (LINT-1): "No paths are lintable." is
    // `ArcanistNoEffectException`: stdout + exit 0, not exit 1.
    assert_eq!(
        code(run_in_with(
            tmp.path(),
            &["--rev", "HEAD", "--output", "none"],
            ""
        )),
        Some(0)
    );
}

#[test]
fn lint_changed_line_filtering_hides_off_line_warnings() {
    let tmp = git_repo("changedlines");
    let long = "x".repeat(81);
    tmp.write("lines.txt", &format!("{long}\n{long}\n"));
    git(tmp.path(), &["add", "-A"]);
    git(tmp.path(), &["commit", "-m", "base"]);
    // Shorten line 1 (changed); line 2 stays long (unchanged).
    tmp.write("lines.txt", &format!("short\n{long}\n"));

    // Errors always show; off-line warnings hide.
    assert_eq!(
        code(run_in(tmp.path(), &["--rev", "HEAD", "--output", "none"])),
        Some(0)
    );
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["--rev", "HEAD", "--output", "none", "--lintall"]
        )),
        Some(1)
    );
    // Unknown revisions fail.
    assert_eq!(
        code(run_in(
            tmp.path(),
            &["--rev", "no-such-rev", "--output", "none"]
        )),
        Some(1)
    );
}

#[test]
fn lint_explicit_paths_do_not_recurse() {
    let tmp = git_repo("dirs");
    tmp.write("sub/inner.txt", "ok\nx\ty\n");
    // Directories are not walked, and directory paths only reach
    // directory-aware linters (the filename linter opts in; the text
    // linter does not), so explicit directories stay silent here.
    assert_eq!(
        code(run_in(tmp.path(), &["sub", "--output", "none"])),
        Some(0)
    );
    std::fs::create_dir_all(tmp.path().join("bad dir")).unwrap();
    assert_eq!(
        code(run_in(tmp.path(), &["bad dir", "--output", "none"])),
        Some(0)
    );
    // Binaries skip text rules.
    std::fs::write(tmp.path().join("bin.dat"), b"has\0nul\t\n").unwrap();
    assert_eq!(
        code(run_in(tmp.path(), &["bin.dat", "--output", "none"])),
        Some(0)
    );
}

#[test]
fn lint_comprehensive_and_single_engines() {
    let tmp = TempDir::new("engines");
    git(tmp.path(), &["init"]);
    tmp.write("a.php", "ok\nx\ty\n");

    // Comprehensive needs no `.arclint`.
    assert_eq!(
        code(run_in(
            tmp.path(),
            &[
                "a.php",
                "--engine",
                "ArcanistComprehensiveLintEngine",
                "--output",
                "none"
            ]
        )),
        Some(2)
    );
    // `externals/` is excluded: nothing lintable.
    // T1153 R4 (LINT-1): "No paths are lintable." is
    // `ArcanistNoEffectException`: stdout + exit 0, not exit 1.
    tmp.write("externals/a.php", "ok\nx\ty\n");
    assert_eq!(
        code(run_in(
            tmp.path(),
            &[
                "externals/a.php",
                "--engine",
                "ArcanistComprehensiveLintEngine",
                "--output",
                "none"
            ]
        )),
        Some(0)
    );

    // Single runs one linter class.
    tmp.write(
        ".arcconfig",
        r#"{"lint.engine.single.linter": "ArcanistTextLinter"}"#,
    );
    assert_eq!(
        code(run_in(
            tmp.path(),
            &[
                "a.php",
                "--engine",
                "ArcanistSingleLintEngine",
                "--output",
                "none"
            ]
        )),
        Some(2)
    );
}

#[test]
fn write_patch_to_disk_roundtrip() {
    let tmp = git_repo("patchdisk");
    let file = tmp.write("p.txt", "ok  \n");
    let engine = lint::Engine::new();
    let messages = engine.lint_bytes("p.txt", b"ok  \n");
    assert!(!messages.is_empty());
    let mut result = lint::LintResult::new("p.txt".to_string(), file.clone(), b"ok  \n".to_vec());
    for message in messages {
        result.add_message(message);
    }
    assert!(result.is_patchable());
    assert!(result.is_all_autofix());
    let applied = lint::write_patch_to_disk(&result).unwrap();
    assert!(applied.iter().any(|flag| *flag));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ok\n");
}
