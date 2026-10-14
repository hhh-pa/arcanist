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
            std::env::temp_dir().join(format!("arc-rs-which-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
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

fn fresh_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    git(&tmp.path, &["config", "user.email", "t@t"]);
    git(&tmp.path, &["config", "user.name", "t"]);
    git(&tmp.path, &["config", "commit.gpgsign", "false"]);
    tmp
}

fn commit(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), "x\n").unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-qm", name]);
}

#[test]
fn conduit_resolution_reports_its_source() {
    let tmp = TempDir::new("conduit");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://cfg.example/"}"#,
    )
    .unwrap();
    assert_eq!(
        resolve_conduit_in(
            &tmp.path,
            Some("https://f.example/"),
            Some("https://e.example/")
        ),
        (
            Some("https://f.example/".to_string()),
            "command line (--conduit-uri)"
        )
    );
    assert_eq!(
        resolve_conduit_in(&tmp.path, None, Some("https://e.example/")),
        (
            Some("https://e.example/".to_string()),
            "environment (ARC_CONDUIT_URI)"
        )
    );
    assert_eq!(
        resolve_conduit_in(&tmp.path, None, None),
        (
            Some("https://cfg.example/".to_string()),
            "project (.arcconfig)"
        )
    );
    let bare = TempDir::new("noconduit");
    assert_eq!(resolve_conduit_in(&bare.path, None, None), (None, "none"));
    assert_eq!(
        resolve_conduit_in(&tmp.path, Some("  "), None).1,
        "project (.arcconfig)"
    );
}

#[test]
fn token_check_is_lenient() {
    let tmp = TempDir::new("token");
    assert!(!token_installed(None, "https://h.example/"));
    assert!(!token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://h.example/"
    ));
    std::fs::write(tmp.path.join(".arcrc"), "broken").unwrap();
    assert!(!token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://h.example/"
    ));
    std::fs::write(
        tmp.path.join(".arcrc"),
        r#"{"hosts": {"https://h.example/api/": {"token": "t"}}}"#,
    )
    .unwrap();
    assert!(token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://h.example/"
    ));
    assert!(!token_installed(
        Some(&tmp.path.join(".arcrc")),
        "https://other.example/"
    ));
}

#[test]
fn project_and_callsign_discovery() {
    let tmp = TempDir::new("proj");
    let child = tmp.path.join("sub");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"repository.callsign": "X", "phabricator.uri": "https://h.example/"}"#,
    )
    .unwrap();
    let (root, file) = find_project(&child);
    assert_eq!(root.unwrap(), tmp.path);
    assert!(file.unwrap().ends_with(".arcconfig"));
    assert_eq!(find_project(&TempDir::new("noroot").path), (None, None));

    let callsign = read_callsign(Some(&tmp.path.join(".arcconfig")), None).unwrap();
    assert_eq!(callsign, ("X".to_string(), "project (.arcconfig)"));
    std::fs::write(
        tmp.path.join(".arcrc"),
        r#"{"config": {"repository.callsign": "U"}}"#,
    )
    .unwrap();
    let from_user = read_callsign(None, Some(&tmp.path.join(".arcrc"))).unwrap();
    assert_eq!(from_user, ("U".to_string(), "user (~/.arcrc)"));
    assert!(read_callsign(None, None).is_none());
}

#[test]
fn base_prefers_explicit_then_upstream_then_head() {
    let repo = fresh_repo("base");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    assert_eq!(head.len(), 40);

    let (explicit, why) = resolve_base(&repo.path, Some("HEAD~1"), "HEAD").unwrap();
    assert_eq!(explicit, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("explicit"), "{why}");
    assert!(resolve_base(&repo.path, Some("no-such-rev"), "HEAD").is_err());

    let (no_upstream, why) = resolve_base(&repo.path, None, "HEAD").unwrap();
    assert_eq!(no_upstream, head);
    assert!(why.contains("no upstream"), "{why}");

    let origin = TempDir::new("worigin");
    git(&origin.path, &["init", "-q", "--bare"]);
    git(
        &repo.path,
        &["remote", "add", "origin", &origin.path.to_string_lossy()],
    );
    git(&repo.path, &["push", "-q", "-u", "origin", "master"]);
    commit(&repo.path, "c.txt");
    let (base, why) = resolve_base(&repo.path, None, "HEAD").unwrap();
    assert_eq!(base, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("merge-base"), "{why}");

    let commits = commits_between(&repo.path, &base, "HEAD").unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "c.txt");
    assert!(
        commits_between(&repo.path, &head, &head)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn context_covers_repos_and_plain_dirs() {
    let repo = fresh_repo("ctx");
    commit(&repo.path, "a.txt");
    std::fs::write(
        repo.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://h.example/", "repository.callsign": "X"}"#,
    )
    .unwrap();
    let user_file = repo.path.join(".arcrc");
    std::fs::write(
        &user_file,
        r#"{"hosts": {"https://h.example/api/": {"token": "t"}}}"#,
    )
    .unwrap();
    let ctx = load_context(&repo.path, Some(&user_file), None, None, None, "HEAD").unwrap();
    assert_eq!(ctx.vcs, "git");
    assert!(ctx.vcs_root.is_some());
    assert!(ctx.project_file.is_some());
    assert_eq!(ctx.branch.as_deref(), Some("master"));
    assert!(ctx.head.is_some());
    assert_eq!(ctx.conduit_uri.as_deref(), Some("https://h.example/"));
    assert_eq!(ctx.conduit_source, "project (.arcconfig)");
    assert_eq!(ctx.token_installed, Some(true));
    assert_eq!(ctx.callsign.as_deref(), Some("X"));
    assert!(ctx.base.is_some());

    let text = render(&ctx);
    for section in [
        "WORKING COPY",
        "REPOSITORY",
        "CONDUIT",
        "COMMIT RANGE",
        "MATCHING REVISIONS",
    ] {
        assert!(text.contains(section), "{text}");
    }
    assert!(text.contains("Not yet ported"), "{text}");
    let json = report_json(&ctx);
    assert_eq!(json["vcs"], serde_json::json!("git"));
    assert_eq!(json["callsign"], serde_json::json!("X"));

    let plain = TempDir::new("plain");
    let outside = load_context(&plain.path, None, None, None, None, "HEAD").unwrap();
    assert_eq!(outside.vcs, "none");
    assert!(outside.base.is_none());
    assert!(outside.commits.is_empty());
    assert!(render(&outside).contains("not inside a working copy"));
}

#[test]
fn args_cover_flags_ranges_and_unported() {
    assert!(matches!(parse_args(&[]), Parsed::Options(_)));
    assert!(matches!(parse_args(&["--help".to_string()]), Parsed::Help));
    assert!(matches!(
        parse_args(&["--base".to_string()]),
        Parsed::NotYetPorted(_)
    ));
    assert!(matches!(
        parse_args(&["--any-status".to_string()]),
        Parsed::NotYetPorted(_)
    ));
    assert!(matches!(
        parse_args(&["--bogus".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(
        parse_args(&["--head".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(
        parse_args(&["--conduit-uri".to_string()]),
        Parsed::Error(_)
    ));
    assert!(matches!(
        parse_args(&["a".to_string(), "b".to_string()]),
        Parsed::Error(_)
    ));
    match parse_args(&[
        "abc123".to_string(),
        "--head".to_string(),
        "HEAD".to_string(),
    ]) {
        Parsed::Options(opts) => {
            assert_eq!(opts.commit.as_deref(), Some("abc123"));
            assert_eq!(opts.head, "HEAD");
        }
        _ => panic!("expected options"),
    }
}
