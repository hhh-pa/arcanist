use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-version-{tag}-{}-{n}", std::process::id()));
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

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn git(dir: &Path, args: &[&str]) {
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

fn git_output(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string()
}

fn fresh_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    git(&tmp.path, &["config", "user.email", "t@t"]);
    git(&tmp.path, &["config", "user.name", "t"]);
    git(&tmp.path, &["config", "commit.gpgsign", "false"]);
    tmp
}

/// Commit `name` with a fixed UTC timestamp (deterministic `%ct`).
fn commit_at(dir: &Path, name: &str, date: &str) {
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-qm", name])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn parses_empty_and_help() {
    assert_eq!(parse_args(&args(&[])), Ok(Some(())));
    assert_eq!(parse_args(&args(&["--help"])), Ok(None));
    assert_eq!(parse_args(&args(&["-h"])), Ok(None));
}

#[test]
fn rejects_arguments_and_flags() {
    assert_eq!(
        parse_args(&args(&["--"])),
        Err("unexpected argument '--': version takes no arguments".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus"])),
        Err("unknown flag '--bogus'".to_string())
    );
    assert_eq!(
        parse_args(&args(&["extra"])),
        Err("unexpected argument 'extra'".to_string())
    );
}

#[test]
fn usage_names_command_and_synopsis() {
    let text = usage();
    assert!(text.starts_with("usage: arc-rs version\n"), "{text}");
    assert!(text.contains("Show toolset version information."), "{text}");
    assert!(text.contains("Shows the current version."), "{text}");
}

#[test]
fn error_messages_match_php_text() {
    assert_eq!(
        missing_git_message(),
        "Cannot display current version without \"git\" installed."
    );
    assert_eq!(
        not_git_message(Path::new("/tmp/nonexistent-wc")),
        "Library \"arcanist\" (at \"/tmp/nonexistent-wc\") is not a Git working copy, so no version information can be provided."
    );
}

#[test]
fn splits_log_output_on_unit_separator() {
    let (timestamp, commit) = split_log_line("1724925600\x01abcdef\n").unwrap();
    assert_eq!(timestamp, 1_724_925_600);
    assert_eq!(commit, "abcdef");
    assert!(split_log_line("no-separator-here").is_err());
    assert!(split_log_line("").is_err());
}

#[test]
fn formats_php_day_month_year_dates() {
    // Oracle: `date -u -r <epoch> +'%-d %b %Y'`.
    assert_eq!(format_version_date(0), "1 Jan 1970");
    assert_eq!(format_version_date(-1), "31 Dec 1969");
    assert_eq!(format_version_date(946_684_800), "1 Jan 2000");
    assert_eq!(format_version_date(1_791_158_400), "5 Oct 2026");
    assert_eq!(format_version_date(1_798_675_200), "31 Dec 2026");
}

#[test]
fn renders_php_version_line() {
    assert_eq!(
        render_version_line("abc123", 0),
        "arcanist abc123 (1 Jan 1970)\n"
    );
}

#[test]
fn reports_fixture_checkout_version() {
    let repo = fresh_repo("happy");
    commit_at(&repo.path, "a.txt", "2026-10-05 12:00:00 +0000");
    let hash = git_output(&repo.path, &["rev-parse", "HEAD"]);
    assert_eq!(hash.len(), 40);
    assert_eq!(
        version_text(&repo.path),
        Ok(format!("arcanist {hash} (5 Oct 2026)\n"))
    );
}

#[test]
fn resolves_library_root_from_subdirectory() {
    let repo = fresh_repo("subdir");
    commit_at(&repo.path, "a.txt", "2026-10-05 12:00:00 +0000");
    let nested = repo.path.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    let root = library_root(&nested).unwrap();
    let expected = git_output(&repo.path, &["rev-parse", "--show-toplevel"]);
    assert_eq!(root, PathBuf::from(expected.as_str()));
    let hash = git_output(&repo.path, &["rev-parse", "HEAD"]);
    assert_eq!(
        version_text(&nested),
        Ok(format!("arcanist {hash} (5 Oct 2026)\n"))
    );
}

#[test]
fn rejects_directories_outside_any_checkout() {
    let tmp = TempDir::new("plain");
    assert_eq!(library_root(tmp.path()), Err(not_git_message(tmp.path())));
    // Either precondition may fail depending on the machine, but a bare
    // temp dir can never report a version.
    let err = version_text(tmp.path()).unwrap_err();
    assert!(
        err == not_git_message(tmp.path()) || err == missing_git_message(),
        "{err}"
    );
    assert_eq!(
        run_in(tmp.path(), &args(&[])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn fails_without_commits() {
    let repo = fresh_repo("empty");
    assert!(head_commit(&repo.path).is_err());
    assert!(version_text(&repo.path).is_err());
}

#[test]
fn run_in_reports_help_and_usage_errors() {
    let repo = fresh_repo("runin");
    commit_at(&repo.path, "a.txt", "2026-10-05 12:00:00 +0000");
    assert_eq!(
        run_in(&repo.path, &args(&[])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(&repo.path, &args(&["--help"])),
        std::process::ExitCode::SUCCESS
    );
    assert_eq!(
        run_in(&repo.path, &args(&["--bogus"])),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_reports_help_and_usage_errors() {
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn git_is_available_on_test_machines() {
    // Every fixture here shells out to git, like the other suites.
    assert!(git_binary_available());
}
