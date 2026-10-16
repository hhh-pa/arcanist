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
    assert!(text.starts_with("usage: arc version\n"), "{text}");
    assert!(text.contains("Show toolset version information."), "{text}");
    assert!(text.contains("Shows the current version."), "{text}");
}

#[test]
fn error_messages_match_php_text() {
    assert_eq!(
        missing_git_message(),
        "Cannot display current version without \"git\" installed."
    );
    // T1153 CORE-61: outside the CWD the path stays absolute, like
    // `Filesystem::readablePath()`.
    assert_eq!(
        not_git_message(Path::new("/var"), Path::new("/tmp/nonexistent-wc")),
        "Library \"arcanist\" (at \"/tmp/nonexistent-wc\") is not a Git working copy, so no version information can be provided."
    );
    // Under the CWD it relativizes.
    assert_eq!(
        not_git_message(Path::new("/tmp"), Path::new("/tmp/nonexistent-wc")),
        "Library \"arcanist\" (at \"nonexistent-wc\") is not a Git working copy, so no version information can be provided."
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
    assert_eq!(format_version_date_in(0, 0), "1 Jan 1970");
    assert_eq!(format_version_date_in(-1, 0), "31 Dec 1969");
    assert_eq!(format_version_date_in(946_684_800, 0), "1 Jan 2000");
    assert_eq!(format_version_date_in(1_791_158_400, 0), "5 Oct 2026");
    assert_eq!(format_version_date_in(1_798_675_200, 0), "31 Dec 2026");
}

#[test]
fn fixed_offsets_shift_across_day_boundaries() {
    // 2026-10-05 23:30:00 UTC rolls to Oct 6 at +1h, stays Oct 5 at -1h.
    assert_eq!(format_version_date_in(1_791_243_000, 3600), "6 Oct 2026");
    assert_eq!(format_version_date_in(1_791_243_000, -3600), "5 Oct 2026");
    // 2026-10-05 00:30:00 UTC rolls back to Oct 4 at -1h.
    assert_eq!(format_version_date_in(1_791_160_200, -3600), "4 Oct 2026");
    assert_eq!(format_version_date_in(1_791_160_200, 3600), "5 Oct 2026");
}

#[test]
fn live_date_matches_fixed_offset_rendering() {
    // Zone-independent: the live render equals the fixed-offset render
    // at the live offset, whatever the machine zone is.
    for timestamp in [0, 1_791_243_000, 1_791_158_400] {
        let live = format_version_date(timestamp);
        let now = u64::try_from(timestamp).unwrap_or(0);
        let offset = look::local_utc_offset(now);
        assert_eq!(live, format_version_date_in(timestamp, offset));
    }
}

#[test]
fn renders_php_version_line() {
    assert_eq!(
        render_version_line_in("abc123", 0, 0),
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
        version_text_in(&repo.path, &repo.path, 0),
        Ok(format!("arcanist {hash} (5 Oct 2026)\n"))
    );
    // Live render keeps the PHP line shape in any zone.
    let live = version_text(&repo.path, &repo.path).unwrap();
    assert!(live.starts_with(&format!("arcanist {hash} (")));
    assert!(live.ends_with(")\n"));
}

#[test]
fn resolves_library_root_from_subdirectory() {
    let repo = fresh_repo("subdir");
    commit_at(&repo.path, "a.txt", "2026-10-05 12:00:00 +0000");
    let nested = repo.path.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    let root = library_root(&repo.path, &nested).unwrap();
    let expected = git_output(&repo.path, &["rev-parse", "--show-toplevel"]);
    assert_eq!(root, PathBuf::from(expected.as_str()));
    let hash = git_output(&repo.path, &["rev-parse", "HEAD"]);
    assert_eq!(
        version_text_in(&repo.path, &nested, 0),
        Ok(format!("arcanist {hash} (5 Oct 2026)\n"))
    );
}

#[test]
fn rejects_directories_outside_any_checkout() {
    // T1153 CORE-60 (kept): with no library root to resolve, an
    // installed binary outside any checkout reports the not-a-git-copy
    // error (exit 1 via the `USAGE EXCEPTION` arm, T1153 CORE-49).
    let tmp = TempDir::new("plain");
    assert_eq!(
        library_root(tmp.path(), tmp.path()),
        Err(not_git_message(tmp.path(), tmp.path()))
    );
    // Either precondition may fail depending on the machine, but a bare
    // temp dir can never report a version.
    let err = version_text(tmp.path(), tmp.path()).unwrap_err();
    assert!(
        err == not_git_message(tmp.path(), tmp.path()) || err == missing_git_message(),
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
    assert!(version_text(&repo.path, &repo.path).is_err());
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
