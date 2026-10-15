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

fn never_prompt(_: &str) -> Option<String> {
    None
}

fn empty_loaded() -> get_config::Sources {
    get_config::Sources {
        local: serde_json::Map::new(),
        project: serde_json::Map::new(),
        user: serde_json::Map::new(),
        system: serde_json::Map::new(),
        default: get_config::default_config(),
        project_file: None,
    }
}

fn base_inputs<'a>(
    runtime_rules: &'a str,
    prompt: &'a dyn Fn(&str) -> Option<String>,
) -> BaseInputs<'a> {
    BaseInputs {
        runtime_rules,
        prompt,
    }
}

fn no_matching() -> MatchConfig<'static> {
    MatchConfig::Unavailable("no conduit in test".to_string())
}

fn test_resolution<'a>(
    runtime_rules: &'a str,
    prompt: &'a dyn Fn(&str) -> Option<String>,
) -> Resolution<'a> {
    Resolution {
        runtime_rules,
        prompt,
        match_config: no_matching(),
    }
}

#[test]
fn base_prefers_explicit_then_upstream() {
    let repo = fresh_repo("base");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    assert_eq!(head.len(), 40);
    let loaded = empty_loaded();

    let inputs = base_inputs("", &never_prompt);
    let (explicit, why) =
        resolve_base(&repo.path, Some("HEAD~1"), "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(explicit, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("explicitly specified base commit"), "{why}");

    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(
        &repo.path,
        Some("no-such-rev"),
        "HEAD",
        false,
        &loaded,
        &inputs,
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Unable to find any git commit named 'no-such-rev' in this repository."
    );

    // No upstream, no rules, no scratch: the prompt default
    // (`origin/master`) does not exist here, so resolution fails like PHP.
    let inputs = base_inputs("", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert_eq!(
        err,
        "Relative commit 'origin/master' is not the name of a commit!"
    );

    let origin = TempDir::new("worigin");
    git(&origin.path, &["init", "-q", "--bare"]);
    git(
        &repo.path,
        &["remote", "add", "origin", &origin.path.to_string_lossy()],
    );
    git(&repo.path, &["push", "-q", "-u", "origin", "master"]);
    commit(&repo.path, "c.txt");
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
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
fn base_rules_cover_literal_empty_and_refs() {
    let repo = fresh_repo("rules");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let loaded = empty_loaded();

    let inputs = base_inputs("literal:abc123", &never_prompt);
    assert_eq!(
        resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap(),
        ("abc123".to_string(), String::new())
    );

    let inputs = base_inputs("arc:empty", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("arc:empty"), "{why}");

    // Unknown refs fall through to the next rule.
    let inputs = base_inputs("git:no-such-ref, git:HEAD~1", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, "HEAD~1");

    let inputs = base_inputs("git:merge-base(HEAD~1)", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, rev_to_commit(&repo.path, "HEAD~1").unwrap());
    assert!(why.contains("merge-base of 'HEAD~1' and HEAD"), "{why}");

    // Nothing matched: the PHP usage error.
    let inputs = base_inputs("git:no-such-ref", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");

    // Malformed and unknown rules fail like PHP.
    let inputs = base_inputs("arcupstream", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("must have a type and name"), "{err}");

    let inputs = base_inputs("bogus:rule", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("not a recognized rule"), "{err}");
}

#[test]
fn base_engine_switches_skips_and_halts() {
    let repo = fresh_repo("engine");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let mut loaded = empty_loaded();
    loaded.project.insert(
        "base".to_string(),
        serde_json::json!("literal:from-project"),
    );

    // `arc:project` switches to the project source mid-stream.
    let inputs = base_inputs("arc:project, literal:unreached", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, "from-project");

    // `arc:skip` moves to the next rule; `arc:halt` stops everything.
    let inputs = base_inputs("arc:skip, literal:kept", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "kept");

    let inputs = base_inputs("arc:halt, literal:unreached", &never_prompt);
    let err = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap_err();
    assert!(err.contains("None of the rules"), "{err}");

    // `arc:prompt` uses the answer, or falls through when empty.
    let inputs = base_inputs("arc:prompt", &|_: &str| Some("typed-base".to_string()));
    let (base, why) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "typed-base");
    assert!(why.contains("what you typed"), "{why}");

    let inputs = base_inputs("arc:prompt, literal:after-prompt", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "after-prompt");
}

#[test]
fn base_exec_runs_in_project_root() {
    let repo = fresh_repo("exec");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    std::fs::write(repo.path.join(".arcconfig"), r#"{"base": ""}"#).unwrap();
    let loaded = empty_loaded();

    let inputs = base_inputs("arc:exec(echo hi)", &never_prompt);
    let (base, _) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, "hi");

    // A failing command falls through to the next rule.
    let inputs = base_inputs("arc:exec(exit 3), literal:fallback", &never_prompt);
    let (base, _) =
        resolve_base(&repo.path, None, "HEAD", false, &empty_loaded(), &inputs).unwrap();
    assert_eq!(base, "fallback");
}

#[test]
fn base_default_chain_uses_project_and_scratch() {
    let repo = fresh_repo("defaultchain");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head_minus_1 = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let mut loaded = empty_loaded();
    loaded.project.insert(
        "git.default-relative-commit".to_string(),
        serde_json::json!("HEAD~1"),
    );

    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, head_minus_1);
    assert!(why.contains("git.default-relative-commit"), "{why}");
}

#[test]
fn shallow_histories_use_the_empty_tree() {
    let repo = fresh_repo("shallow");
    let loaded = empty_loaded();
    let inputs = base_inputs("", &never_prompt);
    // Zero commits.
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("no commits"), "{why}");

    commit(&repo.path, "a.txt");
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(&repo.path, None, "HEAD", false, &loaded, &inputs).unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("only one commit"), "{why}");
}

#[test]
fn explicit_magic_base_passes_through() {
    let repo = fresh_repo("magicexplicit");
    commit(&repo.path, "a.txt");
    let loaded = empty_loaded();
    let inputs = base_inputs("", &never_prompt);
    let (base, why) = resolve_base(
        &repo.path,
        Some(GIT_MAGIC_ROOT_COMMIT),
        "HEAD",
        false,
        &loaded,
        &inputs,
    )
    .unwrap();
    assert_eq!(base, GIT_MAGIC_ROOT_COMMIT);
    assert!(why.contains("empty tree"), "{why}");
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
    // Single commit: the empty-tree base, no prompt needed.
    let resolution = test_resolution("", &never_prompt);
    let ctx = load_context(
        &repo.path,
        Some(&user_file),
        None,
        None,
        None,
        "HEAD",
        false,
        &resolution,
    )
    .unwrap();
    assert_eq!(ctx.vcs, "git");
    assert!(ctx.vcs_root.is_some());
    assert!(ctx.project_file.is_some());
    assert_eq!(ctx.branch.as_deref(), Some("master"));
    assert!(ctx.head.is_some());
    assert_eq!(ctx.conduit_uri.as_deref(), Some("https://h.example/"));
    assert_eq!(ctx.conduit_source, "project (.arcconfig)");
    assert_eq!(ctx.token_installed, Some(true));
    assert_eq!(ctx.callsign.as_deref(), Some("X"));
    assert_eq!(ctx.base.as_deref(), Some(GIT_MAGIC_ROOT_COMMIT));

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
    assert!(text.contains("Revision matching unavailable"), "{text}");
    let json = report_json(&ctx);
    assert_eq!(json["vcs"], serde_json::json!("git"));
    assert_eq!(json["callsign"], serde_json::json!("X"));
    assert!(json["matching_note"].is_string());

    let plain = TempDir::new("plain");
    let resolution = test_resolution("", &never_prompt);
    let outside = load_context(
        &plain.path,
        None,
        None,
        None,
        None,
        "HEAD",
        false,
        &resolution,
    )
    .unwrap();
    assert_eq!(outside.vcs, "none");
    assert!(outside.base.is_none());
    assert!(outside.commits.is_empty());
    assert!(render(&outside).contains("not inside a working copy"));
    assert!(matches!(outside.matching, Matching::OutsideRepo));
}

#[test]
fn args_cover_flags_ranges_and_unported() {
    assert!(matches!(parse_args(&[]), Parsed::Options(_)));
    assert!(matches!(parse_args(&["--help".to_string()]), Parsed::Help));
    match parse_args(&["--base".to_string(), "arc:upstream".to_string()]) {
        Parsed::Options(opts) => assert_eq!(opts.base_rules.as_deref(), Some("arc:upstream")),
        _ => panic!("expected --base options"),
    }
    match parse_args(&["--base=arc:empty".to_string()]) {
        Parsed::Options(opts) => assert_eq!(opts.base_rules.as_deref(), Some("arc:empty")),
        _ => panic!("expected --base= options"),
    }
    assert!(matches!(
        parse_args(&["--base".to_string()]),
        Parsed::Error(_)
    ));
    match parse_args(&["--any-status".to_string()]) {
        Parsed::Options(opts) => assert!(opts.any_status),
        _ => panic!("expected --any-status options"),
    }
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

#[test]
fn tokenizes_base_specifications() {
    assert!(tokenize_base_rules("").unwrap().is_empty());
    assert_eq!(
        tokenize_base_rules("arc:upstream, git:master").unwrap(),
        vec!["arc:upstream".to_string(), "git:master".to_string()]
    );
    assert_eq!(
        tokenize_base_rules("arc:upstream,, ,git:master").unwrap(),
        vec!["arc:upstream".to_string(), "git:master".to_string()]
    );
    assert_eq!(
        tokenize_base_rules("arcupstream").unwrap_err(),
        "Rule 'arcupstream' is invalid, it must have a type and name like 'arc:upstream'."
    );
}

#[test]
fn extracts_revision_ids_from_corpora() {
    assert_eq!(revision_id_from_corpus("no footer here").unwrap(), None);
    assert_eq!(
        revision_id_from_corpus("Subject\n\nDifferential Revision: D123\n").unwrap(),
        Some("123".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("differential revision: d7").unwrap(),
        Some("7".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: https://x.test/D45").unwrap(),
        Some("45".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: https://x.test/D45/").unwrap(),
        Some("45".to_string())
    );
    // `\s*` crosses newlines, so an empty footer consumes the next line
    // as its value (here: not a monogram, so it throws).
    assert!(
        revision_id_from_corpus("Differential Revision:   \nDifferential Revision: D9").is_err()
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:\nD9").unwrap(),
        Some("9".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:\n\n  D10  \n").unwrap(),
        Some("10".to_string())
    );
    // A bare prefix at end of input matches nothing and throws nothing.
    assert_eq!(
        revision_id_from_corpus("Differential Revision:\n").unwrap(),
        None
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision:").unwrap(),
        None
    );
    // Whitespace-only values throw with an empty reported value.
    assert_eq!(
        revision_id_from_corpus("Differential Revision:   ").unwrap_err(),
        "Invalid \"Differential Revision\" field in commit message. This field should have a \
         revision identifier like \"D123\" or a server URI like \
         \"https://devtools.example.com/D123\", but has \"\"."
    );
    // The prefix must sit at a line start; matching is ASCII-only.
    assert_eq!(
        revision_id_from_corpus("xDifferential Revision: D1").unwrap(),
        None
    );
    assert_eq!(
        revision_id_from_corpus("DIFFERENTIAL REVISION: D5").unwrap(),
        Some("5".to_string())
    );
    assert_eq!(
        revision_id_from_corpus("Differential Revision: D8\r\n").unwrap(),
        Some("8".to_string())
    );
    assert!(revision_id_from_corpus("Differential Revision: \u{a0}D9").is_err());
    // Query strings and fragments are stripped before path matching.
    assert_eq!(
        revision_id_from_corpus("Differential Revision: D77?view=all").unwrap(),
        Some("77".to_string())
    );
    // Malformed footers fail like PHP.
    for bad in [
        "Differential Revision: 123",
        "Differential Revision: D0123",
        "Differential Revision: D",
        "Differential Revision: https://x.test/foo/D1",
        "Differential Revision: foo/D1",
    ] {
        assert!(revision_id_from_corpus(bad).is_err(), "{bad}");
    }
    assert!(
        revision_id_from_corpus("Differential Revision: nope")
            .unwrap_err()
            .contains("Invalid \"Differential Revision\" field")
    );
}

#[test]
fn rejects_reversed_ranges() {
    let repo = fresh_repo("reversed");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let head = rev_to_commit(&repo.path, "HEAD").unwrap();
    let parent = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    assert!(check_range_direction(&repo.path, &parent, &head, "HEAD").is_ok());
    let err = check_range_direction(&repo.path, &head, &parent, "HEAD~1").unwrap_err();
    assert!(err.contains("empty, backward or invalid"), "{err}");
    assert!(err.contains(&head), "{err}");
    // An empty range is invalid too.
    assert!(check_range_direction(&repo.path, &head, &head, "HEAD").is_err());
}

#[test]
fn local_commits_carry_trees_and_messages() {
    let repo = fresh_repo("localinfo");
    commit(&repo.path, "a.txt");
    commit(&repo.path, "b.txt");
    let parent = rev_to_commit(&repo.path, "HEAD~1").unwrap();
    let info = local_commit_information(&repo.path, &parent, "HEAD").unwrap();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].summary, "b.txt");
    assert_eq!(info[0].tree.len(), 40);
    assert!(info[0].message.contains("b.txt"));

    let messages = git_commit_messages(&repo.path, &parent, "HEAD").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0.len(), 40);

    let all = git_commit_messages(&repo.path, GIT_MAGIC_ROOT_COMMIT, "HEAD").unwrap();
    assert_eq!(all.len(), 2);
}
