//! Tests for [`land`](super): parsing, prompts, selection, discovery,
//! grouping, and the git/Mercurial flows.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "arc_rs_land_{}_{}_{}",
        std::process::id(),
        id,
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    git(&dir, &["init", "-b", "master"]);
    git(&dir, &["config", "user.email", "land@test"]);
    git(&dir, &["config", "user.name", "Land Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("file.txt"), "base\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "base"]);
    dir
}

fn commit_file(dir: &Path, name: &str, body: &str, message: &str) -> String {
    std::fs::write(dir.join(name), body).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn modern_row(
    id: i64,
    status: &str,
    status_name: &str,
    closed: bool,
    author: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "phid": format!("PHID-DREV-{id}"),
        "fields": {
            "status": {"value": status, "name": status_name, "closed": closed},
            "title": format!("Work {id}"),
            "authorPHID": author,
            "diffPHID": format!("PHID-DIFF-{id}"),
        }
    })
}

fn view(
    id: i64,
    status: &str,
    status_name: &str,
    closed: bool,
    author: &str,
) -> land_checks::RevisionView {
    land_checks::RevisionView {
        id,
        monogram: format!("D{id}"),
        name: format!("Work {id}"),
        status: status.to_string(),
        status_name: status_name.to_string(),
        closed,
        author_phid: author.to_string(),
        phid: format!("PHID-DREV-{id}"),
        diff_phid: Some(format!("PHID-DIFF-{id}")),
    }
}

fn land_commit(hash: &str, parents: &[&str], summary: &str) -> LandCommit {
    LandCommit {
        hash: hash.to_string(),
        parents: parents.iter().map(|parent| (*parent).to_string()).collect(),
        summary: summary.to_string(),
        direct_symbols: Vec::new(),
        indirect_symbols: Vec::new(),
        explicit: None,
        related: Vec::new(),
        is_head: false,
        is_implicit: false,
    }
}

fn canned(
    responses: Vec<(&str, serde_json::Value)>,
) -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    move |method, _params| {
        responses
            .iter()
            .find(|(name, _)| *name == method)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("unexpected conduit call {method}"))
    }
}

#[test]
fn parse_defaults_are_empty() {
    let options = parse_options(&args(&[])).unwrap().unwrap();
    assert_eq!(options, Options::default());
    assert!(options.provided.is_empty());
}

#[test]
fn parse_covers_every_flag() {
    let options = parse_options(&args(&[
        "--hold",
        "--keep-branches",
        "--onto-remote",
        "fork",
        "--onto",
        "stable",
        "--onto=next",
        "--strategy",
        "merge",
        "--revision",
        "D89",
        "--preview",
        "--into",
        "release",
        "--into-remote",
        "upstream",
        "--into-local",
        "--into-empty",
        "--incremental",
        "--pick",
        "--conduit-uri",
        "https://phab.example/api/",
        "--",
        "feature",
    ]))
    .unwrap()
    .unwrap();
    assert!(options.hold);
    assert!(options.keep_branches);
    assert_eq!(options.onto_remote.as_deref(), Some("fork"));
    assert_eq!(options.onto, vec!["stable".to_string(), "next".to_string()]);
    assert_eq!(options.strategy.as_deref(), Some("merge"));
    assert_eq!(options.revision.as_deref(), Some("D89"));
    assert!(options.preview);
    assert_eq!(options.into.as_deref(), Some("release"));
    assert_eq!(options.into_remote.as_deref(), Some("upstream"));
    assert!(options.into_local);
    assert!(options.into_empty);
    assert!(options.incremental);
    assert!(options.pick);
    assert_eq!(options.refs, vec!["feature".to_string()]);
    assert_eq!(
        options.conduit_uri.as_deref(),
        Some("https://phab.example/api/")
    );
    assert_eq!(
        options.provided,
        vec![
            "hold",
            "keep-branches",
            "onto-remote",
            "onto",
            "strategy",
            "revision",
            "preview",
            "into",
            "into-remote",
            "into-local",
            "into-empty",
            "incremental",
            "pick",
            "conduit-uri",
        ]
    );
}

#[test]
fn parse_positionals_collect_into_refs() {
    let options = parse_options(&args(&["a", "--", "b"])).unwrap().unwrap();
    assert_eq!(options.refs, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn parse_help_short_circuits() {
    assert!(parse_options(&args(&["--help"])).unwrap().is_none());
    assert!(parse_options(&args(&["--hold", "-h"])).unwrap().is_none());
}

#[test]
fn parse_unknown_long_errors() {
    let err = parse_options(&args(&["--nope"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'nope'. Try 'arc help'.");
}

#[test]
fn parse_unknown_short_errors() {
    let err = parse_options(&args(&["-x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'x'. Try 'arc help'.");
    let err = parse_options(&args(&["-"])).unwrap_err();
    assert_eq!(err, "Unknown argument ''. Try 'arc help'.");
}

#[test]
fn parse_missing_param_errors() {
    let err = parse_options(&args(&["--onto"])).unwrap_err();
    assert_eq!(err, "Option '--onto' requires a parameter.");
}

#[test]
fn parse_bool_value_splices_back() {
    let options = parse_options(&args(&["--hold=x"])).unwrap().unwrap();
    assert!(options.hold);
    assert_eq!(options.refs, vec!["x".to_string()]);
}

#[test]
fn out_streams_and_status_lines() {
    let mut out = Out::default();
    assert!(out.is_empty());
    out.push_out("o");
    out.push_err("e");
    out.push_status("LABEL", "message");
    assert_eq!(out.stdout, "o");
    assert_eq!(out.stderr, "e** LABEL ** message\n");
    assert!(!out.is_empty());
}

#[test]
fn banner_renders_blank_title_body() {
    let mut io = Io::canned(&[]);
    push_banner(&mut io, "TITLE", "body line");
    assert_eq!(io.out.stdout, "\n** TITLE **\nbody line\n\n");
}

#[test]
fn prompt_accepts_y_and_n() {
    let mut io = Io::canned(&["y"]);
    assert!(prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").is_ok());
    assert_eq!(io.out.stdout, "\n>>> Land these changes? [y/N/?] ");
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").unwrap_err(),
        "User aborted the workflow."
    );
}

#[test]
fn prompt_empty_means_no_and_loops_on_garbage() {
    let mut io = Io::canned(&["", "bogus", "Y"]);
    assert!(prompt_for_key(&mut io, "k", "q?").is_err());
    let mut io = Io::canned(&["bogus", "Y"]);
    assert!(prompt_for_key(&mut io, "k", "q?").is_ok());
    assert_eq!(io.out.stdout, "\n>>> q? [y/N/?] >>> q? [y/N/?] ");
}

#[test]
fn prompt_help_lists_description() {
    let mut io = Io::canned(&["?", "n"]);
    assert!(prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").is_err());
    assert!(io.out.stdout.contains("** PROMPT ** **arc.land.confirm**"));
    assert!(
        io.out
            .stdout
            .contains("Confirms that the correct changes have been selected to land.")
    );
    assert!(io.out.stdout.contains("Run \"arc help prompts\""));
}

#[test]
fn prompt_strips_save_suffixes() {
    let mut io = Io::canned(&["y*"]);
    assert!(prompt_for_key(&mut io, "k", "q?").is_ok());
    let mut io = Io::canned(&["n!"]);
    assert!(prompt_for_key(&mut io, "k", "q?").is_err());
}

#[test]
fn prompt_honors_saved_responses() {
    let mut io = Io::canned(&[]);
    io.saved.insert("arc.land.confirm".to_string(), true);
    assert!(prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").is_ok());
    assert!(io.out.stdout.contains("Using saved response"));
    let mut io = Io::canned(&[]);
    io.saved.insert("arc.land.confirm".to_string(), false);
    assert!(prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").is_err());
}

#[test]
fn confirm_renders_options_and_defaults() {
    let mut io = Io::canned(&[""]);
    assert!(!console_confirm(&mut io, "Delete?", true).unwrap());
    assert_eq!(io.out.stdout, "\n\nDelete? [y/N] \n");
    let mut io = Io::canned(&[""]);
    assert!(console_confirm(&mut io, "Apply?", false).unwrap());
    assert_eq!(io.out.stdout, "\n\nApply? [Y/n] \n");
    let mut io = Io::canned(&["bogus", "N"]);
    assert!(!console_confirm(&mut io, "Delete?", true).unwrap());
}

#[test]
fn revision_symbol_parses_ids_and_phids() {
    assert_eq!(
        parse_revision_symbol("123").unwrap(),
        RevisionSymbol::Id(123)
    );
    assert_eq!(
        parse_revision_symbol("D123").unwrap(),
        RevisionSymbol::Id(123)
    );
    assert_eq!(parse_revision_symbol("d7").unwrap(), RevisionSymbol::Id(7));
    assert_eq!(
        parse_revision_symbol("PHID-DREV-abc").unwrap(),
        RevisionSymbol::Phid("PHID-DREV-abc".to_string())
    );
    assert_eq!(RevisionSymbol::Id(9).display(), "9");
    assert_eq!(
        RevisionSymbol::Phid("PHID-DREV-x".to_string()).display(),
        "PHID-DREV-x"
    );
}

#[test]
fn revision_symbol_rejects_bad_formats() {
    for bad in [
        "",
        "0",
        "007",
        "D0",
        "abc",
        "D",
        "PHID-DREV-",
        "PHID-DIFF-x",
    ] {
        let err = parse_revision_symbol(bad).unwrap_err();
        assert_eq!(
            err,
            format!(
                "The format of symbol \"{bad}\" is unrecognized. Expected a monogram like \
                 \"X123\", or an ID like \"123\", or a PHID."
            ),
            "input {bad:?}"
        );
    }
}

#[test]
fn strategy_selects_flag_config_default() {
    let (strategy, line) = select_strategy(Some("merge"), None).unwrap();
    assert_eq!(strategy, Strategy::Merge);
    assert!(line.contains("\"--strategy\""));
    let (strategy, line) = select_strategy(None, Some("merge")).unwrap();
    assert_eq!(strategy, Strategy::Merge);
    assert!(line.contains("arc.land.strategy"));
    let (strategy, line) = select_strategy(None, None).unwrap();
    assert_eq!(strategy, Strategy::Squash);
    assert!(line.contains("default strategy"));
}

#[test]
fn strategy_rejects_unknown_names() {
    let err = select_strategy(Some("octopus"), None).unwrap_err();
    assert_eq!(
        err,
        "Merge strategy \"octopus\" specified with \"--strategy\" is unknown. Supported merge \
         strategies are: merge, squash."
    );
    let err = select_strategy(None, Some("octopus")).unwrap_err();
    assert_eq!(
        err,
        "Merge strategy \"octopus\" specified in \"arc.land.strategy\" configuration is unknown. \
         Supported merge strategies are: merge, squash."
    );
}

#[test]
fn fetch_revision_views_queries_ids_and_phids() {
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_REVISION_SEARCH);
        probe.borrow_mut().push(params.clone());
        Ok(
            serde_json::json!({"data": [modern_row(89, "accepted", "Accepted", false, "PHID-USER-a")]}),
        )
    };
    let views = fetch_revision_views(&call, &RevisionSymbol::Id(89)).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].monogram, "D89");
    let views =
        fetch_revision_views(&call, &RevisionSymbol::Phid("PHID-DREV-89".to_string())).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(
        seen.borrow()[0],
        serde_json::json!({"constraints": {"ids": [89]}})
    );
    assert_eq!(
        seen.borrow()[1],
        serde_json::json!({"phids": ["PHID-DREV-89"]})
    );
}

#[test]
fn resolve_forced_revision_reports_unknown_symbols() {
    let call = canned(vec![(
        diff_submit::METHOD_REVISION_SEARCH,
        serde_json::json!({"data": []}),
    )]);
    let err = resolve_forced_revision(&call, &RevisionSymbol::Id(89)).unwrap_err();
    assert_eq!(err, "Symbol \"89\" does not identify a valid revision.");
}

#[test]
fn viewer_identity_reads_whoami() {
    let call = canned(vec![(
        METHOD_WHOAMI,
        serde_json::json!({"phid": "PHID-USER-a", "userName": "alice"}),
    )]);
    let viewer = fetch_viewer_identity(&call).unwrap();
    assert_eq!(viewer.phid, "PHID-USER-a");
    assert_eq!(viewer.name, "alice");
    assert_eq!(fetch_viewer_phid(&call).unwrap(), "PHID-USER-a");
    let call = canned(vec![(METHOD_WHOAMI, serde_json::json!({}))]);
    assert!(fetch_viewer_identity(&call).is_err());
}

#[test]
fn parent_links_follow_edges_and_resolve_parents() {
    let call = canned(vec![
        (
            METHOD_EDGE_SEARCH,
            serde_json::json!({"data": [
                {"sourcePHID": "PHID-DREV-2", "destinationPHID": "PHID-DREV-1"},
            ]}),
        ),
        (
            diff_submit::METHOD_REVISION_SEARCH,
            serde_json::json!({"data": [modern_row(1, "accepted", "Accepted", false, "PHID-USER-a")]}),
        ),
    ]);
    let revisions = vec![
        view(1, "accepted", "Accepted", false, "PHID-USER-a"),
        view(2, "accepted", "Accepted", false, "PHID-USER-a"),
    ];
    let links = fetch_parent_links(&call, &revisions).unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].child_index, 1);
    assert_eq!(links[0].parent.monogram, "D1");
}

#[test]
fn parent_edges_follow_pagination() {
    let calls = std::rc::Rc::new(std::cell::RefCell::new(0));
    let probe = calls.clone();
    let call = move |method: &str, _params: &serde_json::Value| {
        assert_eq!(method, METHOD_EDGE_SEARCH);
        *probe.borrow_mut() += 1;
        if *calls.borrow() == 1 {
            Ok(serde_json::json!({
                "data": [{"sourcePHID": "s", "destinationPHID": "d1"}],
                "cursor": {"after": "next"},
            }))
        } else {
            Ok(serde_json::json!({
                "data": [{"sourcePHID": "s", "destinationPHID": "d2"}],
            }))
        }
    };
    let edges = fetch_parent_edges(&call, &["s".to_string()]).unwrap();
    assert_eq!(edges.len(), 2);
}

#[test]
fn build_views_join_buildable_build_plan() {
    let call = canned(vec![
        (
            METHOD_BUILDABLE_SEARCH,
            serde_json::json!({"data": [
                {"id": 7, "phid": "PHID-HMBB-7", "fields": {"objectPHID": "PHID-DIFF-89"}},
            ]}),
        ),
        (
            METHOD_BUILD_SEARCH,
            serde_json::json!({"data": [
                {"fields": {
                    "buildStatus": {"value": "failed"},
                    "buildablePHID": "PHID-HMBB-7",
                    "buildPlanPHID": "PHID-HMPA-1",
                }},
            ]}),
        ),
        (
            METHOD_BUILDPLAN_SEARCH,
            serde_json::json!({"data": [
                {"phid": "PHID-HMPA-1", "fields": {"behaviors": {"arc-land": {"value": "always"}}}},
            ]}),
        ),
    ]);
    let revisions = vec![view(89, "accepted", "Accepted", false, "PHID-USER-a")];
    let builds = fetch_build_views(&call, &revisions).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].status, "failed");
    assert_eq!(builds[0].plan_behavior.as_deref(), Some("always"));
    assert_eq!(builds[0].revision_phid, "PHID-DREV-89");
}

#[test]
fn build_views_skip_revisions_without_diff_phids() {
    let call = canned(vec![]);
    let mut revision = view(89, "accepted", "Accepted", false, "PHID-USER-a");
    revision.diff_phid = None;
    let builds = fetch_build_views(&call, &[revision]).unwrap();
    assert!(builds.is_empty());
}

#[test]
fn commit_message_requires_a_string() {
    let call = canned(vec![(
        diff_submit::METHOD_GET_COMMIT_MESSAGE,
        serde_json::json!("D89: Work\n\nSummary."),
    )]);
    assert_eq!(
        fetch_commit_message(&call, 89).unwrap(),
        "D89: Work\n\nSummary."
    );
    let call = canned(vec![(
        diff_submit::METHOD_GET_COMMIT_MESSAGE,
        serde_json::json!({"message": "nope"}),
    )]);
    assert!(fetch_commit_message(&call, 89).is_err());
}

#[test]
fn gate_preview_lists_findings() {
    assert_eq!(format_gate_preview(&[]), "Pre-land checks passed.\n");
    let findings = land_checks::evaluate_gates(
        &[view(
            1,
            "needs-review",
            "Needs Review",
            false,
            "PHID-USER-b",
        )],
        "PHID-USER-a",
        &[],
        &["PHID-DREV-1"],
        &[],
        Some(1),
    );
    let text = format_gate_preview(&findings);
    assert!(text.starts_with("Pre-land checks:\n"));
    assert!(text.contains("[arc.land.not-accepted]"));
}

#[test]
fn client_for_dir_discovers_from_target() {
    let dir = temp_dir("client-dir");
    let err = match client_for_dir(None, &dir) {
        Ok(_) => panic!("expected no conduit URI"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with \
         .arcconfig"
    );
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phabricator.uri": "https://phab.example/"}"#,
    )
    .unwrap();
    let err = match client_for_dir(None, &dir) {
        Ok(_) => panic!("expected no token"),
        Err(err) => err,
    };
    assert!(err.starts_with("no token for https://phab.example/"));
}

#[test]
fn load_gate_findings_combines_gates() {
    let call = canned(vec![
        (
            METHOD_WHOAMI,
            serde_json::json!({"phid": "PHID-USER-a", "userName": "alice"}),
        ),
        (METHOD_EDGE_SEARCH, serde_json::json!({"data": []})),
        (METHOD_BUILDABLE_SEARCH, serde_json::json!({"data": []})),
    ]);
    let revisions = vec![view(1, "accepted", "Accepted", false, "PHID-USER-a")];
    let findings = load_gate_findings(&call, &revisions, 1).unwrap();
    assert!(findings.is_empty());
}

#[test]
fn display_summary_truncates_long_subjects() {
    assert_eq!(display_summary("short"), "short");
    let long = "x".repeat(100);
    let shown = display_summary(&long);
    assert_eq!(shown.len(), 64 + 3);
    assert!(shown.ends_with("..."));
}

#[test]
fn git_log_lines_parse_and_skip_blanks() {
    let parsed = parse_git_log_lines("aaa\0p1 p2\0subject\n\nbbb\0\0other\n").unwrap();
    assert_eq!(
        parsed,
        vec![
            (
                "aaa".to_string(),
                "p1 p2".to_string(),
                "subject".to_string()
            ),
            ("bbb".to_string(), String::new(), "other".to_string()),
        ]
    );
    assert!(parse_git_log_lines("no-nuls-here\n").is_err());
}

#[test]
fn hg_log_lines_parse_with_three_parts() {
    assert_eq!(
        parse_hg_log_line("aaa-p1 p2-subject").unwrap(),
        (
            "aaa".to_string(),
            "p1 p2".to_string(),
            "subject".to_string()
        )
    );
    assert!(parse_hg_log_line("only-two").is_err());
}

#[test]
fn collect_git_commits_merges_symbols() {
    let mut commits = Vec::new();
    collect_git_commits(&mut commits, "aaa\0\0one\nbbb\0aaa\0two\n", "s1").unwrap();
    collect_git_commits(&mut commits, "bbb\0aaa\0two\n", "s2").unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].direct_symbols, vec!["s1".to_string()]);
    assert_eq!(
        commits[1].indirect_symbols,
        vec!["s1".to_string(), "s2".to_string()]
    );
    assert_eq!(commits[1].direct_symbols, vec!["s2".to_string()]);
    assert_eq!(commits[1].parents, vec!["aaa".to_string()]);
}

#[test]
fn collect_hg_commits_marks_first_direct() {
    let mut commits = Vec::new();
    collect_hg_commits(&mut commits, "bbb-aaa-two\naaa--one\n", "tip").unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].direct_symbols, vec!["tip".to_string()]);
    assert!(commits[1].direct_symbols.is_empty());
}

#[test]
fn link_commits_reverses_and_marks_heads() {
    let mut commits = vec![
        land_commit("new", &["old"], "new"),
        land_commit("old", &[], "old"),
    ];
    link_commits(&mut commits);
    assert_eq!(commits[0].hash, "old");
    assert_eq!(commits[1].hash, "new");
    assert!(!commits[0].is_head);
    assert!(commits[1].is_head);
}

#[test]
fn confirm_commit_count_rejects_empty() {
    let mut io = Io::canned(&[]);
    let err = confirm_commit_count(&mut io, &[], "\"a\"", Some("mstr"), &|hash| {
        hash.to_string()
    })
    .unwrap_err();
    assert_eq!(err, "There are no commits to land.");
    assert!(io.out.stdout.contains("** NOTHING TO LAND **"));
}

#[test]
fn confirm_commit_count_prompts_large_sets() {
    let commits: Vec<LandCommit> = (0..51)
        .map(|index| land_commit(&format!("h{index:04}"), &[], &format!("c{index}")))
        .collect();
    let mut io = Io::canned(&["y"]);
    confirm_commit_count(&mut io, &commits, "\"a\"", None, &|hash| hash.to_string()).unwrap();
    assert!(io.out.stdout.contains("** LARGE WORKING SET **"));
    assert!(io.out.stdout.contains("< ... 41 more commits ... >"));
    assert!(io.out.stdout.contains("Land 51 commit(s)?"));
    let mut io = Io::canned(&["n"]);
    assert!(
        confirm_commit_count(&mut io, &commits, "\"a\"", Some("mstr"), &|hash| {
            hash.to_string()
        })
        .is_err()
    );
}

#[test]
fn confirm_commit_count_passes_small_sets_quietly() {
    let commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    confirm_commit_count(&mut io, &commits, "\"a\"", Some("mstr"), &|hash| {
        hash.to_string()
    })
    .unwrap();
    assert!(io.out.is_empty());
}

#[test]
fn display_symbols_quotes_and_escapes() {
    assert_eq!(
        display_symbols(&["a".to_string(), "b\"c\\".to_string()]),
        "\"a\", \"b\\\"c\\\\\""
    );
}

#[test]
fn differential_ids_parse_messages() {
    assert_eq!(
        differential_revision_ids("Subject\n\nDifferential Revision: https://phab.test/D123\n"),
        vec![123]
    );
    assert_eq!(
        differential_revision_ids("differential revision: D7 and D9"),
        vec![7]
    );
    assert_eq!(
        differential_revision_ids("Differential Revision: D7\nquoted Differential Revision: D9\n"),
        vec![7, 9]
    );
    assert_eq!(
        differential_revision_ids("Differential Revision: 42"),
        vec![42]
    );
    assert!(differential_revision_ids("no tag here").is_empty());
    assert!(differential_revision_ids("Differential Revision: none").is_empty());
}

#[test]
fn legacy_queries_send_ids_and_hashes() {
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_QUERY);
        probe.borrow_mut().push(params.clone());
        Ok(serde_json::json!([
            {"id": 5, "phid": "PHID-DREV-5", "title": "t", "statusName": "Accepted",
             "authorPHID": "PHID-USER-a"},
        ]))
    };
    let views = query_revisions_by_ids(&call, &[5]).unwrap();
    assert_eq!(views[0].monogram, "D5");
    let views =
        query_revisions_by_hashes(&call, &[("gtcm".to_string(), "abc".to_string())]).unwrap();
    assert_eq!(views[0].monogram, "D5");
    assert_eq!(seen.borrow()[0], serde_json::json!({"ids": [5]}));
    assert_eq!(
        seen.borrow()[1],
        serde_json::json!({"commitHashes": [["gtcm", "abc"]]})
    );
}

fn legacy_row(id: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "phid": format!("PHID-DREV-{id}"),
        "title": format!("Work {id}"),
        "statusName": "Accepted",
        "authorPHID": "PHID-USER-a",
    })
}

#[test]
fn discovery_selects_single_related_revisions() {
    let call = canned(vec![(
        diff_submit::METHOD_QUERY,
        serde_json::json!([legacy_row(12)]),
    )]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("Differential Revision: D12".to_string()),
        &|_| Ok(Vec::new()),
        None,
    )
    .unwrap();
    assert_eq!(commits[0].explicit.as_ref().unwrap().monogram, "D12");
}

#[test]
fn discovery_rejects_ambiguous_without_forced_revision() {
    let call = canned(vec![(
        diff_submit::METHOD_QUERY,
        serde_json::json!([legacy_row(11), legacy_row(12)]),
    )]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    commits[0].indirect_symbols.push("feature".to_string());
    let mut io = Io::canned(&[]);
    let err = load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("Differential Revision: D12".to_string()),
        &|_| Ok(Vec::new()),
        None,
    )
    .unwrap_err();
    assert!(err.contains("is ambiguous"));
    assert!(io.out.stdout.contains("** AMBIGUOUS REVISION **"));
}

#[test]
fn discovery_queries_hashes_without_message_ids() {
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |_method: &str, params: &serde_json::Value| {
        probe.borrow_mut().push(params.clone());
        Ok(serde_json::json!([legacy_row(3)]))
    };
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("no tag".to_string()),
        &|_| Ok(vec![("gtcm".to_string(), "aaa".to_string())]),
        None,
    )
    .unwrap();
    assert_eq!(commits[0].explicit.as_ref().unwrap().monogram, "D3");
    assert_eq!(
        seen.borrow()[0],
        serde_json::json!({"commitHashes": [["gtcm", "aaa"]]})
    );
}

#[test]
fn discovery_force_selects_and_rejects_foreign() {
    let force = view(9, "accepted", "Accepted", false, "PHID-USER-a");
    let call = canned(vec![(diff_submit::METHOD_QUERY, serde_json::json!([]))]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("no tag".to_string()),
        &|_| Ok(Vec::new()),
        Some(&force),
    )
    .unwrap();
    assert_eq!(commits[0].explicit.as_ref().unwrap().monogram, "D9");

    let call = canned(vec![(
        diff_submit::METHOD_QUERY,
        serde_json::json!([legacy_row(4)]),
    )]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    let err = load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("Differential Revision: D4".to_string()),
        &|_| Ok(Vec::new()),
        Some(&force),
    )
    .unwrap_err();
    assert!(err.starts_with("TODO: You are forcing a revision"));
}

#[test]
fn resolve_commit_revision_walks_parents() {
    let rev = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let mut old = land_commit("old", &[], "old");
    old.explicit = Some(rev.clone());
    let commits = vec![old, land_commit("new", &["old"], "new")];
    let resolved = resolve_commit_revision(&commits, 1).unwrap().unwrap();
    assert_eq!(resolved.monogram, "D1");
    assert!(
        resolve_commit_revision(&[land_commit("x", &[], "x")], 0)
            .unwrap()
            .is_none()
    );
}

#[test]
fn resolve_commit_revision_rejects_distinct_parents() {
    let mut left = land_commit("left", &[], "left");
    left.explicit = Some(view(1, "accepted", "Accepted", false, "PHID-USER-a"));
    let mut right = land_commit("right", &[], "right");
    right.explicit = Some(view(2, "accepted", "Accepted", false, "PHID-USER-a"));
    let commits = vec![
        left,
        right,
        land_commit("merge", &["left", "right"], "merge"),
    ];
    assert_eq!(
        resolve_commit_revision(&commits, 2).unwrap_err(),
        "Too many distinct parent refs!"
    );
}

#[test]
fn ancestor_phids_list_parents_first() {
    let mut old = land_commit("old", &[], "old");
    old.explicit = Some(view(1, "accepted", "Accepted", false, "PHID-USER-a"));
    let mut new = land_commit("new", &["old"], "new");
    new.explicit = Some(view(2, "accepted", "Accepted", false, "PHID-USER-a"));
    let commits = vec![old, new];
    assert_eq!(
        ancestor_revision_phids(&commits, 1),
        vec!["PHID-DREV-1".to_string(), "PHID-DREV-2".to_string()]
    );
}

#[test]
fn grouping_orders_sets_and_marks_implicit() {
    let mut old = land_commit("old", &[], "old");
    old.explicit = Some(view(1, "accepted", "Accepted", false, "PHID-USER-a"));
    old.is_head = false;
    let mut new = land_commit("new", &["old"], "new");
    new.explicit = Some(view(2, "accepted", "Accepted", false, "PHID-USER-a"));
    new.is_head = true;
    let mut commits = vec![old, new];
    let mut io = Io::canned(&[]);
    let sets = group_commit_sets(&mut io, &mut commits, &|hash| hash.to_string()).unwrap();
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0].revision.as_ref().unwrap().monogram, "D1");
    assert_eq!(sets[1].revision.as_ref().unwrap().monogram, "D2");
    assert!(!commits[0].is_implicit);
}

#[test]
fn grouping_reports_unknown_revisions() {
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    let err = group_commit_sets(&mut io, &mut commits, &|hash| hash.to_string()).unwrap_err();
    assert_eq!(err, "Unable to determine revision for commit \"aaa\".");
    assert!(io.out.stdout.contains("** UNKNOWN REVISION **"));
}

#[test]
fn filter_discards_closed_ancestors_under_squash() {
    let closed = CommitSet {
        revision: Some(view(1, "published", "Closed", true, "PHID-USER-a")),
        commits: vec![0],
        is_pick: false,
    };
    let commits = vec![land_commit("old", &[], "old")];
    let mut io = Io::canned(&[]);
    let kept = filter_commit_sets(&mut io, vec![closed], &commits, true, false, &|_| {
        "rendered\n".to_string()
    });
    assert!(kept.is_empty());
    assert!(io.out.stdout.contains("** DISCARDING ANCESTORS **"));
}

#[test]
fn filter_pick_keeps_direct_sets() {
    let mut direct = land_commit("new", &[], "new");
    direct.direct_symbols.push("feature".to_string());
    let sets = vec![
        CommitSet {
            revision: Some(view(1, "accepted", "Accepted", false, "PHID-USER-a")),
            commits: vec![0],
            is_pick: false,
        },
        CommitSet {
            revision: Some(view(2, "accepted", "Accepted", false, "PHID-USER-a")),
            commits: vec![1],
            is_pick: false,
        },
    ];
    let commits = vec![land_commit("old", &[], "old"), direct];
    let mut io = Io::canned(&[]);
    let kept = filter_commit_sets(&mut io, sets, &commits, true, true, &|_| String::new());
    assert_eq!(kept.len(), 1);
    assert!(kept[0].is_pick);
    assert_eq!(kept[0].revision.as_ref().unwrap().monogram, "D2");
}

#[test]
fn render_commit_set_marks_implicit() {
    let mut implicit = land_commit("aaa", &[], "implicit work");
    implicit.is_implicit = true;
    let commits = vec![land_commit("bbb", &[], "plain work"), implicit];
    let set = CommitSet {
        revision: Some(view(1, "accepted", "Accepted", false, "PHID-USER-a")),
        commits: vec![0, 1],
        is_pick: false,
    };
    let text = render_commit_set(&set, &commits, &|hash| hash.to_string());
    assert!(text.contains("D1 Work 1"));
    assert!(text.contains("! aaa implicit work"));
    assert!(text.contains("bbb  plain work"));
}

#[test]
fn confirm_implicit_skips_without_implicit_sets() {
    let commits = vec![land_commit("aaa", &[], "one")];
    let sets = vec![CommitSet {
        revision: Some(view(1, "accepted", "Accepted", false, "PHID-USER-a")),
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = Io::canned(&[]);
    confirm_implicit_sets(&mut io, &sets, &commits, "\"a\"", &|hash| hash.to_string()).unwrap();
    assert!(io.out.is_empty());
}

#[test]
fn confirm_implicit_prompts_with_banner() {
    let mut implicit = land_commit("aaa", &[], "implicit work");
    implicit.is_implicit = true;
    let commits = vec![implicit];
    let sets = vec![CommitSet {
        revision: Some(view(1, "accepted", "Accepted", false, "PHID-USER-a")),
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = Io::canned(&["y"]);
    confirm_implicit_sets(&mut io, &sets, &commits, "\"a\"", &|hash| hash.to_string()).unwrap();
    assert!(io.out.stdout.contains("** IMPLICIT COMMITS **"));
    assert!(io.out.stdout.contains("Continue with this mapping"));
}

#[test]
fn confirm_gates_cover_every_prompt() {
    let revisions = vec![view(
        1,
        "needs-review",
        "Needs Review",
        false,
        "PHID-USER-b",
    )];
    let viewer = ViewerIdentity {
        phid: "PHID-USER-a".to_string(),
        name: "alice".to_string(),
    };
    let findings = land_checks::evaluate_gates(
        &revisions,
        &viewer.phid,
        &[],
        &["PHID-DREV-1"],
        &[],
        Some(1),
    );
    assert_eq!(findings.len(), 2);
    let mut io = Io::canned(&["y", "y"]);
    confirm_revision_gates(&mut io, &revisions, &viewer, &[], &[], &findings).unwrap();
    assert!(io.out.stdout.contains("** NOT REVISION AUTHOR **"));
    assert!(
        io.out
            .stdout
            .contains("** 1 REVISION(S) ARE NOT ACCEPTED **")
    );
    let mut io = Io::canned(&["n"]);
    assert!(confirm_revision_gates(&mut io, &revisions, &viewer, &[], &[], &findings).is_err());
}

#[test]
fn confirm_gates_render_parents_and_builds() {
    let child = view(2, "accepted", "Accepted", false, "PHID-USER-a");
    let parent = view(1, "needs-review", "Needs Review", false, "PHID-USER-a");
    let revisions = vec![child];
    let viewer = ViewerIdentity {
        phid: "PHID-USER-a".to_string(),
        name: "alice".to_string(),
    };
    let links = vec![land_checks::ParentLink {
        child_index: 0,
        parent: parent.clone(),
    }];
    let builds = vec![land_checks::BuildView {
        status: "failed".to_string(),
        plan_behavior: Some("always".to_string()),
        revision_phid: "PHID-DREV-2".to_string(),
    }];
    let groups = land_checks::open_parent_groups(&links, &["PHID-DREV-2"]);
    assert_eq!(groups.len(), 1);
    let verdict = land_checks::classify_builds(&builds);
    assert_eq!(verdict.failures, 1);
    let findings = land_checks::evaluate_gates(
        &revisions,
        &viewer.phid,
        &links,
        &["PHID-DREV-2"],
        &builds,
        Some(1),
    );
    assert_eq!(findings.len(), 2);
    let mut io = Io::canned(&["y", "y"]);
    confirm_revision_gates(&mut io, &revisions, &viewer, &links, &builds, &findings).unwrap();
    assert!(io.out.stdout.contains("** 1 OPEN PARENT REVISION(S) **"));
    assert!(io.out.stdout.contains("Parent of: D2 Work 2"));
    assert!(io.out.stdout.contains("** BUILD FAILURES **"));
}

#[test]
fn name_is_land() {
    assert_eq!(NAME, "land");
}

#[test]
fn detect_vcs_classifies_working_copies() {
    let repo = make_repo("detect-git");
    assert_eq!(detect_vcs(&repo), Some(Vcs::Git));
    let plain = temp_dir("detect-plain");
    assert_eq!(detect_vcs(&plain), None);
    let hg_dir = temp_dir("detect-hg");
    std::fs::create_dir(hg_dir.join(".hg")).unwrap();
    assert_eq!(detect_vcs(&hg_dir), Some(Vcs::Hg));
}

#[test]
fn onto_config_parses_string_and_list() {
    assert_eq!(
        parse_onto_config(&serde_json::json!("stable")),
        vec!["stable".to_string()]
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!(["a", 1, "b"])),
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(parse_onto_config(&serde_json::json!(7)).is_empty());
    assert_eq!(
        parse_string_config(&serde_json::json!("origin")),
        Some("origin".to_string())
    );
    assert_eq!(parse_string_config(&serde_json::json!(7)), None);
}

#[test]
fn saved_prompts_read_map_and_list() {
    let dir = temp_dir("prompts");
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"prompts": {"arc.land.confirm": "y", "arc.land.create": "n"}}"#,
    )
    .unwrap();
    let saved = load_saved_prompts(&dir);
    assert_eq!(saved.get("arc.land.confirm"), Some(&true));
    assert_eq!(saved.get("arc.land.create"), Some(&false));
    let dir = temp_dir("prompts-list");
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"prompts": [{"prompt": "k", "response": "YES"}]}"#,
    )
    .unwrap();
    assert_eq!(load_saved_prompts(&dir).get("k"), Some(&true));
    assert!(load_saved_prompts(&temp_dir("prompts-none")).is_empty());
}

#[test]
fn revision_id_and_phid_classifiers() {
    assert!(is_revision_id("1"));
    assert!(is_revision_id("123"));
    assert!(!is_revision_id(""));
    assert!(!is_revision_id("0"));
    assert!(!is_revision_id("007"));
    assert!(!is_revision_id("12a"));
    assert!(is_revision_phid("PHID-DREV-abc"));
    assert!(!is_revision_phid("PHID-DREV-"));
    assert!(!is_revision_phid("PHID-DREV-a b"));
    assert!(!is_revision_phid("PHID-DIFF-x"));
}

#[test]
fn split_author_handles_malformed() {
    assert_eq!(
        split_author("Ada <ada@test>"),
        ("Ada".to_string(), "ada@test".to_string())
    );
    assert_eq!(split_author("bogus"), (String::new(), String::new()));
}

#[test]
fn merge_approaches_follow_strategy() {
    assert_eq!(
        merge_approaches(true, false),
        vec![MergeApproach::RebaseMerge, MergeApproach::ReduceRebaseMerge]
    );
    assert_eq!(merge_approaches(true, true), vec![MergeApproach::Merge]);
    assert_eq!(merge_approaches(false, false), vec![MergeApproach::Merge]);
}

#[test]
fn validate_options_rejects_bad_combinations() {
    let dir = temp_dir("validate");
    let options = Options {
        into_local: true,
        into_remote: Some("origin".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        validate_options(&options, &dir, &mut io).unwrap_err(),
        "Arguments \"--into-local\", \"--into-remote\", and \"--into-empty\" are mutually \
         exclusive."
    );
    let options = Options {
        into: Some("x".to_string()),
        into_empty: true,
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        validate_options(&options, &dir, &mut io).unwrap_err(),
        "Arguments \"--into\" and \"--into-empty\" are mutually exclusive."
    );
    let options = Options {
        pick: true,
        strategy: Some("merge".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        validate_options(&options, &dir, &mut io).unwrap_err(),
        "You can not \"--pick\" changes under the \"merge\" strategy."
    );
    let options = Options {
        revision: Some("bogus".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert!(
        validate_options(&options, &dir, &mut io)
            .unwrap_err()
            .contains("unrecognized")
    );
    let options = Options {
        strategy: Some("merge".to_string()),
        revision: Some("D5".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    let validated = validate_options(&options, &dir, &mut io).unwrap();
    assert_eq!(validated.strategy, Strategy::Merge);
    assert_eq!(validated.revision, Some(RevisionSymbol::Id(5)));
    assert!(io.out.stderr.contains("** STRATEGY **"));
}

#[test]
fn git_default_symbols_use_branch_then_head() {
    let repo = make_repo("default-symbols");
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_default_symbols(&repo, &mut io).unwrap(),
        vec!["master".to_string()]
    );
    assert!(io.out.stderr.contains("** SOURCE **"));
    git(&repo, &["checkout", "--detach", "HEAD"]);
    let mut io = Io::canned(&[]);
    let symbols = git_default_symbols(&repo, &mut io).unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].len(), 40);
}

#[test]
fn git_resolve_symbols_checks_branches() {
    let repo = make_repo("resolve");
    assert_eq!(
        git_resolve_symbols(&repo, &["master".to_string()])
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        git_resolve_symbols(&repo, &["missing".to_string()]).unwrap_err(),
        "Branch \"missing\" does not exist in the local working copy."
    );
}

fn repo_with_origin(name: &str) -> (PathBuf, PathBuf) {
    let repo = make_repo(name);
    let bare = temp_dir(&format!("{name}-bare"));
    git(&bare, &["init", "--bare"]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&repo, &["push", "-u", "origin", "master"]);
    (repo, bare)
}

#[test]
fn git_onto_remote_prefers_flag_config_upstream_origin() {
    let (repo, _bare) = repo_with_origin("onto-remote");
    let mut io = Io::canned(&[]);
    let options = Options {
        onto_remote: Some("origin".to_string()),
        ..Options::default()
    };
    assert_eq!(
        git_select_onto_remote(&repo, &mut io, &options, &["master".to_string()]).unwrap(),
        "origin"
    );
    assert!(io.out.stderr.contains("\"--onto-remote\" flag"));
    let options = Options {
        onto_remote: Some("missing".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert!(git_select_onto_remote(&repo, &mut io, &options, &["master".to_string()]).is_err());
    let mut io = Io::canned(&[]);
    let options = Options::default();
    assert_eq!(
        git_select_onto_remote(&repo, &mut io, &options, &["master".to_string()]).unwrap(),
        "origin"
    );
    assert!(io.out.stderr.contains("tracking branches upstream"));
}

#[test]
fn git_onto_refs_follow_flag_config_upstream_default() {
    let (repo, _bare) = repo_with_origin("onto-refs");
    let options = Options {
        onto: vec!["stable".to_string()],
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &options, &["master".to_string()]).unwrap(),
        vec!["stable".to_string()]
    );
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.land.onto": ["cfg"]}"#).unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &["master".to_string()]).unwrap(),
        vec!["cfg".to_string()]
    );
    std::fs::remove_file(repo.join(".arcconfig")).unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &["master".to_string()]).unwrap(),
        vec!["master".to_string()]
    );
}

#[test]
fn ls_remote_heads_parse_branches() {
    let output = "aaa\trefs/heads/master\nbbb\trefs/heads/next\nccc\trefs/tags/v1\n";
    assert_eq!(
        parse_ls_remote_heads(output),
        vec!["master".to_string(), "next".to_string()]
    );
}

#[test]
fn git_confirm_onto_refs_prompts_for_new_branches() {
    let (repo, _bare) = repo_with_origin("confirm-onto");
    let mut io = Io::canned(&[]);
    git_confirm_onto_refs(&repo, &mut io, "origin", &["master".to_string()], false).unwrap();
    assert!(io.out.is_empty());
    let mut io = Io::canned(&["y"]);
    git_confirm_onto_refs(&repo, &mut io, "origin", &["fresh".to_string()], true).unwrap();
    assert!(io.out.stdout.contains("** CREATE 1 BRANCHE(S) **"));
    assert!(io.out.stdout.contains("--hold"));
    let mut io = Io::canned(&["n"]);
    assert!(
        git_confirm_onto_refs(&repo, &mut io, "origin", &["fresh".to_string()], false).is_err()
    );
    let mut io = Io::canned(&[]);
    assert!(git_confirm_onto_refs(&repo, &mut io, "origin", &[String::new()], false).is_err());
}

#[test]
fn into_state_selection_covers_modes() {
    let fetchable = |remote: &str| remote == "origin";
    let mut io = Io::canned(&[]);
    let options = Options {
        into_empty: true,
        ..Options::default()
    };
    assert_eq!(
        select_into_state(&mut io, &options, "origin", &["m".to_string()], &fetchable).unwrap(),
        IntoState::Empty
    );
    let options = Options {
        into_local: true,
        into: Some("local".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        select_into_state(&mut io, &options, "origin", &["m".to_string()], &fetchable).unwrap(),
        IntoState::Local("local".to_string())
    );
    let options = Options {
        into_remote: Some("bogus".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert!(
        select_into_state(&mut io, &options, "origin", &["m".to_string()], &fetchable).is_err()
    );
    let options = Options::default();
    let mut io = Io::canned(&[]);
    assert_eq!(
        select_into_state(
            &mut io,
            &options,
            "origin",
            &["one".to_string(), "two".to_string()],
            &fetchable
        )
        .unwrap(),
        IntoState::Remote {
            remote: "origin".to_string(),
            name: "one".to_string(),
        }
    );
    assert!(io.out.stderr.contains("first \"onto\" target"));
}

#[test]
fn git_into_commit_resolves_empty_local_remote() {
    let (repo, _bare) = repo_with_origin("into-commit");
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_into_commit(&repo, &mut io, &IntoState::Empty, None).unwrap(),
        None
    );
    let mut io = Io::canned(&[]);
    let local = git_select_into_commit(
        &repo,
        &mut io,
        &IntoState::Local("master".to_string()),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(local.len(), 40);
    let mut io = Io::canned(&[]);
    assert!(
        git_select_into_commit(&repo, &mut io, &IntoState::Local("nope".to_string()), None)
            .is_err()
    );
    let mut io = Io::canned(&[]);
    let remote = git_select_into_commit(
        &repo,
        &mut io,
        &IntoState::Remote {
            remote: "origin".to_string(),
            name: "master".to_string(),
        },
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(remote, local);
}

#[test]
fn git_into_commit_implies_empty_or_errors() {
    let repo = make_repo("into-missing");
    let bare = temp_dir("into-missing-bare");
    git(&bare, &["init", "--bare"]);
    git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let mut io = Io::canned(&[]);
    let implied = git_select_into_commit(
        &repo,
        &mut io,
        &IntoState::Remote {
            remote: "origin".to_string(),
            name: "master".to_string(),
        },
        None,
    )
    .unwrap();
    assert_eq!(implied, None);
    assert!(io.out.stderr.contains("empty state to create"));
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_into_commit(
            &repo,
            &mut io,
            &IntoState::Remote {
                remote: "origin".to_string(),
                name: "master".to_string(),
            },
            Some("master"),
        )
        .unwrap_err(),
        "Ref \"master\" does not exist in remote \"origin\"."
    );
}

#[test]
fn git_select_commits_lists_ranges() {
    let repo = make_repo("select-commits");
    git(&repo, &["checkout", "-b", "feature"]);
    let tip = commit_file(&repo, "f.txt", "f\n", "feature work");
    let symbols = vec![GitSymbol {
        symbol: "feature".to_string(),
        commit: tip.clone(),
    }];
    let master = git(&repo, &["rev-parse", "master"]);
    let commits = git_select_commits(&repo, &symbols, Some(&master)).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].hash, tip);
    assert_eq!(commits[0].direct_symbols, vec!["feature".to_string()]);
    let all = git_select_commits(&repo, &symbols, None).unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn git_author_and_date_read_log() {
    let repo = make_repo("author-date");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let (author, date) = git_author_and_date(&repo, &head).unwrap();
    assert_eq!(author, "Land Test <land@test>");
    assert!(!date.is_empty());
}

#[test]
fn git_refspecs_and_ref_parsing() {
    let specs = git_onto_refspecs(&["a".to_string(), "b".to_string()], "abcdef1234567890");
    assert_eq!(
        specs,
        vec![
            "abcdef123456:refs/heads/a".to_string(),
            "abcdef123456:refs/heads/b".to_string(),
        ]
    );
    let branches =
        parse_for_each_ref("refs/heads/b bbb\nrefs/heads/a aaa\nrefs/tags/v1 ccc\n").unwrap();
    assert_eq!(
        branches,
        vec![
            ("a".to_string(), "aaa".to_string()),
            ("b".to_string(), "bbb".to_string()),
        ]
    );
    assert!(parse_for_each_ref("bogus-line\n").is_err());
}

#[test]
fn git_prune_deletes_landed_branches() {
    let repo = make_repo("prune");
    git(&repo, &["checkout", "-b", "feature"]);
    let tip = commit_file(&repo, "f.txt", "f\n", "feature work");
    git(&repo, &["checkout", "master"]);
    let commits = vec![land_commit(&tip, &[], "feature work")];
    let sets = vec![CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = Io::canned(&[]);
    let deleted = git_prune_branches(&repo, &mut io, &sets, &commits);
    assert_eq!(deleted, vec!["feature".to_string()]);
    assert!(io.out.stderr.contains("** CLEANUP **"));
    assert!(io.out.stdout.contains("git checkout -b feature"));
    let branches = git(&repo, &["branch", "--list", "feature"]);
    assert!(branches.is_empty());
}

#[test]
fn git_cascade_rebases_descendants() {
    let repo = make_repo("cascade");
    git(&repo, &["checkout", "-b", "feature"]);
    let tip = commit_file(&repo, "f.txt", "f\n", "feature work");
    git(&repo, &["checkout", "-b", "child"]);
    commit_file(&repo, "c.txt", "c\n", "child work");
    let master = git(&repo, &["rev-parse", "master"]);
    let commits = vec![land_commit(&tip, &[], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    git_cascade_state(&repo, &mut io, &set, &commits, &master, true);
    assert!(io.out.stderr.contains("** CASCADE **"));
    let child_head = git(&repo, &["rev-parse", "child"]);
    let child_parent = git(&repo, &["rev-parse", &format!("{child_head}^")]);
    assert_ne!(child_parent, tip);
    // Squash off: no-op.
    let mut io = Io::canned(&[]);
    git_cascade_state(&repo, &mut io, &set, &commits, &master, false);
    assert!(io.out.is_empty());
}

#[test]
fn git_reconcile_prefers_original_then_onto() {
    let repo = make_repo("reconcile");
    git(&repo, &["checkout", "-b", "feature"]);
    git_reconcile(&repo, Some("feature"), None, &["master".to_string()], &[]);
    assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
    git_reconcile(
        &repo,
        Some("feature"),
        None,
        &["master".to_string()],
        &["feature".to_string()],
    );
    assert_eq!(git(&repo, &["branch", "--show-current"]), "master");
}

#[test]
fn git_hold_text_lists_push_and_restore() {
    let mut io = Io::canned(&[]);
    git_hold_text(
        &mut io,
        "origin",
        &["master".to_string()],
        "abcdef1234567890",
        &["git checkout feature --".to_string()],
    );
    assert!(io.out.stdout.contains("** HOLD CHANGES **"));
    assert!(
        io.out
            .stdout
            .contains("git push -- origin abcdef123456:refs/heads/master")
    );
    assert!(io.out.stdout.contains("git checkout feature --"));
    assert!(io.out.stdout.contains("same state as before"));
}

#[test]
fn git_execute_merge_squashes_and_merges() {
    let repo = make_repo("merge-squash");
    let master = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    let tip = commit_file(&repo, "f.txt", "f\n", "feature work");
    let commits = vec![land_commit(&tip, &[&master], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let cursor = git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&master),
        true,
        false,
        &|_| Ok("Landed message".to_string()),
    )
    .unwrap();
    assert!(io.out.stderr.contains("** MERGING **"));
    assert_eq!(
        git(&repo, &["log", "--format=%s", "-n1", &cursor]),
        "Landed message"
    );
    assert_eq!(
        git(&repo, &["log", "--format=%an <%ae>", "-n1", &cursor]),
        "Land Test <land@test>"
    );
    // Already landed: empty diff.
    let mut io = Io::canned(&[]);
    let err = git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&cursor),
        true,
        false,
        &|_| Ok("again".to_string()),
    )
    .unwrap_err();
    assert!(err.contains("produces an empty diff"));

    let repo = make_repo("merge-merge");
    let master = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    let tip = commit_file(&repo, "g.txt", "g\n", "feature work");
    let commits = vec![land_commit(&tip, &[&master], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let cursor = git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&master),
        false,
        false,
        &|_| Ok("Merge message".to_string()),
    )
    .unwrap();
    let parents = git(&repo, &["rev-list", "--parents", "-n1", &cursor]);
    assert_eq!(parents.split_whitespace().count(), 3);
}

#[test]
fn merge_conflict_banner_names_symbols_and_hints() {
    let mut direct = land_commit("aaa", &[], "work");
    direct.direct_symbols.push("feature".to_string());
    let commits = vec![direct];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let err = git_merge_conflict(&mut io, &commits, &set, "aaa", "bbb", true);
    assert_eq!(err, "Encountered a merge conflict.");
    assert!(io.out.stdout.contains("** MERGE CONFLICT **"));
    assert!(io.out.stdout.contains("\"feature\""));
    assert!(io.out.stdout.contains("--incremental"));
}

#[test]
fn hg_bookmark_and_branch_parsers() {
    let bookmarks = parse_hg_bookmarks(" * active  1:aaa\n   other  0:bbb\ngarbage\n");
    assert_eq!(
        bookmarks,
        vec![("active".to_string(), true), ("other".to_string(), false),]
    );
    assert_eq!(
        parse_hg_branches("default  5:aaa\nstable  3:bbb\n"),
        vec!["default".to_string(), "stable".to_string()]
    );
    assert_eq!(display_hash_12("abcdef1234567890"), "abcdef123456");
}

#[test]
fn hg_selection_errors_without_binary() {
    let dir = temp_dir("hg-missing");
    let mut io = Io::canned(&[]);
    let options = Options {
        onto_remote: Some("default".to_string()),
        ..Options::default()
    };
    assert_eq!(
        hg_select_onto_remote(&dir, &mut io, &options).unwrap_err(),
        "No remote \"default\" exists in this repository."
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_onto_refs(&dir, &mut io, &Options::default()).unwrap(),
        vec!["default".to_string()]
    );
}

#[test]
fn hg_confirm_prompts_for_new_bookmarks() {
    let dir = temp_dir("hg-confirm");
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        hg_confirm_onto_refs(&dir, &mut io, "default", &["fresh".to_string()], false).unwrap(),
        None
    );
    assert!(io.out.stdout.contains("** CREATE 1 BOOKMARK(S) **"));
    let mut io = Io::canned(&["n"]);
    assert!(hg_confirm_onto_refs(&dir, &mut io, "default", &["fresh".to_string()], false).is_err());
}

#[test]
fn hg_into_commit_covers_empty_local_remote() {
    let dir = temp_dir("hg-into");
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_into_commit(&dir, &mut io, &IntoState::Empty, None).unwrap(),
        None
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_into_commit(
            &dir,
            &mut io,
            &IntoState::Local("default".to_string()),
            None
        )
        .unwrap_err(),
        "Local ref \"default\" does not exist."
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_into_commit(
            &dir,
            &mut io,
            &IntoState::Remote {
                remote: "default".to_string(),
                name: "x".to_string(),
            },
            Some("x"),
        )
        .unwrap_err(),
        "Ref \"x\" does not exist in remote \"default\"."
    );
}

#[test]
fn hg_merge_and_push_surface_limits() {
    let dir = temp_dir("hg-merge");
    let commits = vec![land_commit("aaa", &[], "work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_execute_merge(
            &dir,
            &mut io,
            &set,
            &commits,
            Some("bbb"),
            false,
            None,
            &|_| { Ok("m".to_string()) }
        )
        .unwrap_err(),
        "TODO: Support merge strategies"
    );
    let mut io = Io::canned(&[]);
    assert!(
        hg_execute_merge(&dir, &mut io, &set, &commits, None, true, None, &|_| {
            Ok("m".to_string())
        })
        .is_err()
    );
    let mut io = Io::canned(&[]);
    assert!(hg_push_change(&dir, &mut io, "default", &[], "aaa").is_err());
    assert!(hg_cascade_state(&dir, &set, &commits, "bbb", false).is_ok());
    assert!(hg_prune_branches(&dir, &mut Io::canned(&[]), &[set], &commits).is_err());
    hg_reconcile(&dir, "aaa", None);
    let mut io = Io::canned(&[]);
    hg_hold_text(&mut io, &["b".to_string()], "default", "aaa", &[]);
    assert!(io.out.stdout.contains("** HOLD CHANGES **"));
    assert!(io.out.stdout.contains("hg push --bookmark b -- default"));
}

#[test]
fn ensure_clean_tree_checks_git() {
    let repo = make_repo("clean");
    ensure_clean_tree(&repo, Vcs::Git).unwrap();
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    assert_eq!(
        ensure_clean_tree(&repo, Vcs::Git).unwrap_err(),
        "working tree is not clean; commit or stash your changes first (or use --preview)"
    );
}

#[test]
fn offline_message_lists_commits() {
    let commits = vec![land_commit("abcdef1234567890", &[], "feature work")];
    let message = offline_message(&["feature".to_string()], &["master".to_string()], &commits);
    assert!(message.starts_with("Land feature\n"));
    assert!(message.contains("Squashed 1 commit(s)"));
    assert!(message.contains("abcdef1 feature work"));
}

#[test]
fn execute_help_parse_error_and_non_repo() {
    let dir = temp_dir("exec-misc");
    let mut io = Io::canned(&[]);
    let out = execute_with_io(&dir, &args(&["--help"]), &mut io).unwrap();
    assert_eq!(out.stdout, USAGE);
    let err = execute_with_io(&dir, &args(&["--nope"]), &mut Io::canned(&[])).unwrap_err();
    assert!(err.starts_with("Unknown argument 'nope'."));
    assert!(err.contains("usage: arc-rs land"));
    let err = execute_with_io(&dir, &args(&["--preview"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err, NOT_GIT_OR_HG);
    let out = execute(&dir, &args(&["--help"])).unwrap();
    assert_eq!(out.stdout, USAGE);
}

#[test]
fn execute_previews_offline_land() {
    let (repo, _bare) = repo_with_origin("preview");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "f.txt", "f\n", "feature work");
    let mut io = Io::canned(&[]);
    let out = execute_with_io(&repo, &args(&["--preview", "feature"]), &mut io).unwrap();
    assert!(out.stderr.contains("** STRATEGY **"));
    assert!(out.stderr.contains("** ONTO TARGET **"));
    assert!(out.stderr.contains("** LANDING **"));
    assert!(out.stderr.contains("** PREVIEW **"));
    assert!(out.stderr.contains("revision info unavailable"));
    assert!(out.stdout.contains("feature work"));
}

#[test]
fn run_in_reports_success_and_failure() {
    let dir = temp_dir("run-misc");
    assert!(matches!(
        run_in(&dir, &args(&["--help"])),
        ExitCode::SUCCESS
    ));
    assert!(matches!(
        run_in(&dir, &args(&["--nope"])),
        ExitCode::FAILURE
    ));
    assert!(matches!(
        run_in(&dir, &args(&["--preview"])),
        ExitCode::FAILURE
    ));
}
