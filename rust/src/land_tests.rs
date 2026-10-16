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
    git(&dir, &["config", "core.autocrlf", "false"]);
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
fn prompt_saves_star_suffix_to_user_config() {
    let dir = temp_dir("prompt-save-user");
    let user = dir.join("arcrc");
    let mut io = Io::canned(&["y*"]);
    io.config_writer = Some(ConfigWriter {
        user_path: Some(user.clone()),
        local_path: None,
    });
    assert!(prompt_for_key(&mut io, "k", "q?").is_ok());
    assert!(
        io.out
            .stdout
            .contains("** SAVE PROMPT ** Saving response \"y\" to prompt \"k\".\n")
    );
    let saved = load_saved_prompts_from(&[(user, PromptScope::User)]);
    assert_eq!(
        saved.get("k"),
        Some(&SavedResponse {
            response: "y".to_string(),
            scope: PromptScope::User,
        })
    );
    // Without a writer, saving fails like PHP's missing-source throw.
    let mut io = Io::canned(&["y*"]);
    assert_eq!(
        prompt_for_key(&mut io, "k", "q?").unwrap_err(),
        "Unable to save response to config: no writable configuration source for scope \
         (\"user\")."
    );
    // `!` targets the working-copy config and still declines.
    let dir = temp_dir("prompt-save-local");
    let local = dir.join("nested").join("config");
    let mut io = Io::canned(&["n!"]);
    io.config_writer = Some(ConfigWriter {
        user_path: None,
        local_path: Some(local.clone()),
    });
    assert_eq!(
        prompt_for_key(&mut io, "k", "q?").unwrap_err(),
        "User aborted the workflow."
    );
    assert!(
        io.out
            .stdout
            .contains("** SAVE PROMPT ** Saving response \"n\" to prompt \"k\".\n")
    );
    let saved = load_saved_prompts_from(&[(local, PromptScope::WorkingCopy)]);
    assert_eq!(saved.get("k").map(|s| s.response.as_str()), Some("n"));
}

#[test]
fn prompt_honors_saved_responses() {
    let mut io = Io::canned(&[]);
    io.saved.insert(
        "arc.land.confirm".to_string(),
        SavedResponse {
            response: "y".to_string(),
            scope: PromptScope::User,
        },
    );
    assert!(prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").is_ok());
    assert_eq!(
        io.out.stdout,
        "\n>>> Land these changes? <y>\n>>> (Using saved response to prompt \
         \"arc.land.confirm\".\n\n"
    );
    let mut io = Io::canned(&[]);
    io.saved.insert(
        "arc.land.confirm".to_string(),
        SavedResponse {
            response: "n".to_string(),
            scope: PromptScope::Project,
        },
    );
    assert!(prompt_for_key(&mut io, "arc.land.confirm", "Land these changes?").is_err());
    // Saved garbage falls through to an interactive prompt, like PHP.
    let mut io = Io::canned(&["y"]);
    io.saved.insert(
        "k".to_string(),
        SavedResponse {
            response: "yes".to_string(),
            scope: PromptScope::User,
        },
    );
    assert!(prompt_for_key(&mut io, "k", "q?").is_ok());
    assert_eq!(io.out.stdout, "\n>>> q? [y/N/?] ");
}

#[test]
fn confirm_renders_options_and_defaults() {
    let mut io = Io::canned(&[""]);
    assert!(!console_confirm(&mut io, "Delete?", true).unwrap());
    assert_eq!(io.out.stdout, "\n\n    Delete? [y/N] \n");
    let mut io = Io::canned(&[""]);
    assert!(console_confirm(&mut io, "Apply?", false).unwrap());
    assert_eq!(io.out.stdout, "\n\n    Apply? [Y/n] \n");
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
        serde_json::json!({"constraints": {"phids": ["PHID-DREV-89"]}})
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
                {"id": 42, "fields": {
                    "name": "Lint",
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
    let builds =
        fetch_build_views(&call, &revisions, Some("https://phab.example.com/api/")).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].status, "failed");
    assert_eq!(builds[0].plan_behavior.as_deref(), Some("always"));
    assert_eq!(builds[0].revision_phid, "PHID-DREV-89");
    assert_eq!(builds[0].build_id, 42);
    assert_eq!(builds[0].build_name, "Lint");
    assert_eq!(builds[0].buildable_monogram, "B7");
    assert_eq!(builds[0].buildable_uri, "https://phab.example.com/B7");
}

#[test]
fn build_gate_queries_nest_constraints() {
    let seen: std::rc::Rc<std::cell::RefCell<Vec<(String, serde_json::Value)>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        probe
            .borrow_mut()
            .push((method.to_string(), params.clone()));
        Ok(match method {
            METHOD_BUILDABLE_SEARCH => serde_json::json!({"data": [
                {"id": 7, "phid": "PHID-HMBB-7", "fields": {"objectPHID": "PHID-DIFF-89"}},
            ]}),
            METHOD_BUILD_SEARCH => serde_json::json!({"data": [
                {"id": 42, "fields": {
                    "name": "Lint",
                    "buildStatus": {"value": "passed"},
                    "buildablePHID": "PHID-HMBB-7",
                    "buildPlanPHID": "PHID-HMPA-1",
                }},
            ]}),
            _ => serde_json::json!({"data": [
                {"phid": "PHID-HMPA-1", "fields": {"behaviors": {"arc-land": {"value": "always"}}}},
            ]}),
        })
    };
    let revisions = vec![view(89, "accepted", "Accepted", false, "PHID-USER-a")];
    fetch_build_views(&call, &revisions, None).unwrap();
    assert_eq!(
        *seen.borrow(),
        vec![
            (
                METHOD_BUILDABLE_SEARCH.to_string(),
                serde_json::json!({"constraints": {
                    "objectPHIDs": ["PHID-DIFF-89"],
                    "manual": false,
                }}),
            ),
            (
                METHOD_BUILD_SEARCH.to_string(),
                serde_json::json!({"constraints": {"buildables": ["PHID-HMBB-7"]}}),
            ),
            (
                METHOD_BUILDPLAN_SEARCH.to_string(),
                serde_json::json!({"constraints": {"phids": ["PHID-HMPA-1"]}}),
            ),
        ]
    );
}

#[test]
fn build_views_skip_revisions_without_diff_phids() {
    let call = canned(vec![]);
    let mut revision = view(89, "accepted", "Accepted", false, "PHID-USER-a");
    revision.diff_phid = None;
    let builds = fetch_build_views(&call, &[revision], None).unwrap();
    assert!(builds.is_empty());
}

#[test]
fn absolute_buildable_uri_keeps_domains_and_guesses_paths() {
    assert_eq!(
        absolute_buildable_uri(
            Some("https://phab.example.com/api/"),
            "B7",
            Some("https://x.test/B7")
        ),
        "https://x.test/B7"
    );
    assert_eq!(
        absolute_buildable_uri(Some("https://phab.example.com/api/"), "B7", Some("/B7")),
        "https://phab.example.com/B7"
    );
    assert_eq!(
        absolute_buildable_uri(Some("https://phab.example.com/api/"), "B7", None),
        "https://phab.example.com/B7"
    );
    assert_eq!(absolute_buildable_uri(None, "B7", Some("/B7")), "/B7");
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
fn commit_message_requests_no_edit() {
    use std::cell::RefCell;
    let seen = RefCell::new(serde_json::json!(null));
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        *seen.borrow_mut() = params.clone();
        Ok::<serde_json::Value, String>(serde_json::json!("D89: Work"))
    };
    assert_eq!(fetch_commit_message(&call, 89).unwrap(), "D89: Work");
    // PHP asks with `'edit' => false` and never opens an editor (L1).
    assert_eq!(
        *seen.borrow(),
        serde_json::json!({"revision_id": 89, "edit": false})
    );
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
    assert_eq!(shown, format!("{}…", "x".repeat(63)));
    // Word boundaries win over the raw cutoff.
    let words = "Fix the thing that was broken by the other thing completely now";
    assert_eq!(display_summary(words), words);
    let longer = format!("{words} and then some more words to push past sixty-four glyphs total");
    let shown = display_summary(&longer);
    assert!(shown.ends_with('…'));
    assert!(!shown.contains("  "));
    // Sentence stops cut without a terminator when the stop sits
    // right before the last break in range.
    let sentence = format!("{}. {}", "a".repeat(50), "b".repeat(30));
    assert_eq!(display_summary(&sentence), format!("{}.", "a".repeat(50)));
}

#[test]
fn console_wrap_matches_phutil_boundaries() {
    assert_eq!(console_wrap("", 0, true), "");
    assert_eq!(console_wrap("short", 0, true), "short");
    // 79 characters with no space overflow, like PHP.
    let word = "x".repeat(79);
    assert_eq!(console_wrap(&word, 0, true), word);
    // Breaks at the last space past column 78.
    let text = format!("{} {}", "a".repeat(70), "b".repeat(20));
    assert_eq!(
        console_wrap(&text, 0, true),
        format!("{}\n{}", "a".repeat(70), "b".repeat(20))
    );
    // Embedded newlines flush the line and survive.
    assert_eq!(console_wrap("a\n\nb", 0, true), "a\n\nb");
    // Indent prefixes every line but optionally the first.
    assert_eq!(console_wrap("a\nb", 4, true), "    a\n    b");
    assert_eq!(console_wrap("a\nb", 4, false), "a\n    b");
    // ANSI escapes do not count toward the width.
    let colored = format!("\x1b[1m{}\x1b[m {}", "a".repeat(70), "b");
    assert_eq!(console_wrap(&colored, 0, true), colored);
}

#[test]
fn truncate_glyphs_matches_phutil_truncator() {
    assert_eq!(truncate_glyphs("short", 64), "short");
    assert_eq!(truncate_glyphs("", 64), "");
    // Byte fast path: short byte strings return unchanged.
    assert_eq!(truncate_glyphs("é", 4), "é");
    // No usable break: hard cut at max - 1 plus the terminator.
    assert_eq!(truncate_glyphs("abcdef", 4), "abc…");
    // Word boundary backward search.
    assert_eq!(truncate_glyphs("hello world foo", 8), "hello…");
    // Sentence stop cuts without a terminator (the scan stops one
    // word back, so the stop must sit right before the last break).
    assert_eq!(truncate_glyphs("hello. world of friends", 10), "hello.");
    // A stop further back is out of reach: plain word cut instead.
    assert_eq!(truncate_glyphs("hi. there friend", 8), "hi. the…");
    // Combining marks stay glued to their base.
    assert_eq!(truncate_glyphs("e\u{301}tudex", 4), "e\u{301}tu…");
    // Zero room yields just the terminator.
    assert_eq!(truncate_glyphs("abcdef", 1), "…");
}

#[test]
fn console_strlen_counts_glyphs_and_wide_chars() {
    assert_eq!(console_strlen(""), 0);
    assert_eq!(console_strlen("hello"), 5);
    assert_eq!(console_strlen("\x1b[1mhi\x1b[m"), 2);
    assert_eq!(console_strlen("e\u{301}"), 1);
    assert_eq!(console_strlen("日本"), 4);
}

#[test]
fn ref_view_renders_markers_and_children() {
    let revision = view(12, "accepted", "Accepted", false, "PHID-USER-a");
    let rendered = revision_ref_view(&revision);
    assert_eq!(rendered.render(), "  *   D12 Work 12\n");
    // Long titles truncate to the usable width (80 - 6 - name - 1).
    let mut named = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    named.name = "w".repeat(100);
    let rendered = revision_ref_view(&named).render();
    assert!(rendered.starts_with("  *   D1 "));
    assert!(rendered.ends_with("…\n"));
    // Name-only markers keep PHP's `" "` title, trailing space included.
    let marker = marker_ref_view("Branch \"fresh\"", "", "");
    assert_eq!(marker.render(), "  *   Branch \"fresh\"  \n");
    // URIs, lines, and children indent one level deeper.
    let mut parent = revision_ref_view(&revision);
    parent.lines.push("Status: Accepted".to_string());
    parent
        .children
        .push(buildable_ref_view("B7", "https://phab.example.com/B7"));
    let mut expected = String::from("  *   D12 Work 12\n");
    expected.push_str("        Status: Accepted\n");
    expected.push_str("  *     B7 Buildable \"B7\"\n");
    expected.push_str(" ://      https://phab.example.com/B7\n");
    assert_eq!(parent.render(), expected);
    let build = build_ref_view(42, "Lint");
    assert_eq!(build.render(), "  *   Build 42 Lint\n");
}

#[test]
fn render_hint_prefixes_and_wraps() {
    assert_eq!(render_hint("Use x."), "  ?   Use x.");
    let long = "word ".repeat(30);
    let rendered = render_hint(long.trim());
    assert!(rendered.starts_with("  ?   word"));
    assert!(rendered.contains("\n      word"));
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
fn confirm_gates_name_example_status() {
    let planned = view(
        3,
        "changes-planned",
        "Changes Planned",
        false,
        "PHID-USER-a",
    );
    let published = view(4, "published", "Published", true, "PHID-USER-a");
    let viewer = ViewerIdentity {
        phid: "PHID-USER-a".to_string(),
        name: "alice".to_string(),
    };
    for (revision, status) in [(&planned, "Changes Planned"), (&published, "Published")] {
        let findings = land_checks::evaluate_gates(
            std::slice::from_ref(revision),
            &viewer.phid,
            &[],
            &[revision.phid.as_str()],
            &[],
            Some(1),
        );
        assert_eq!(findings.len(), 1);
        let mut io = Io::canned(&["y"]);
        confirm_revision_gates(
            &mut io,
            std::slice::from_ref(revision),
            &viewer,
            &[],
            &[],
            &findings,
        )
        .unwrap();
        // The banner wraps, so compare with newlines flattened.
        let flat = io.out.stdout.replace('\n', " ");
        assert!(flat.contains(&format!("\"{status}\"")), "{}", io.out.stdout);
    }
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
        build_id: 42,
        build_name: "Lint".to_string(),
        buildable_monogram: "B7".to_string(),
        buildable_uri: "https://phab.example.com/B7".to_string(),
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
    // The builds banner has no leading blank line; each affected revision
    // renders as a nested revision > buildable > builds tree.
    assert!(
        io.out
            .stdout
            .contains("** BUILD FAILURES **\n1 revision(s) have build failures:\n\n")
    );
    assert!(io.out.stdout.contains("  *   D2 Work 2\n"));
    assert!(io.out.stdout.contains("  *     B7 Buildable \"B7\"\n"));
    assert!(
        io.out
            .stdout
            .contains(" ://      https://phab.example.com/B7\n")
    );
    assert!(io.out.stdout.contains("  *       Build 42 Lint\n"));
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
    let project = dir.join(".arcconfig");
    std::fs::write(
        &project,
        r#"{"prompts": {"arc.land.confirm": "y", "arc.land.create": "n"}}"#,
    )
    .unwrap();
    let saved = load_saved_prompts_from(&[(project, PromptScope::Project)]);
    assert_eq!(
        saved.get("arc.land.confirm").map(|s| s.response.as_str()),
        Some("y")
    );
    assert_eq!(
        saved.get("arc.land.confirm").map(|s| s.scope),
        Some(PromptScope::Project)
    );
    let dir = temp_dir("prompts-list");
    let project = dir.join(".arcconfig");
    std::fs::write(
        &project,
        r#"{"prompts": [{"prompt": "k", "response": "YES"}]}"#,
    )
    .unwrap();
    let saved = load_saved_prompts_from(&[(project, PromptScope::Project)]);
    assert_eq!(saved.get("k").map(|s| s.response.as_str()), Some("YES"));
    assert!(load_saved_prompts_from(&[]).is_empty());
}

#[test]
fn saved_prompts_last_match_wins_across_sources() {
    let dir = temp_dir("prompts-order");
    let system = dir.join("system");
    let user = dir.join("user");
    let project = dir.join("project");
    let local = dir.join("local");
    std::fs::write(
        &system,
        r#"{"prompts": [{"prompt": "k", "response": "n"}]}"#,
    )
    .unwrap();
    std::fs::write(&user, r#"{"prompts": [{"prompt": "k", "response": "y"}]}"#).unwrap();
    std::fs::write(&project, r#"{"other": true}"#).unwrap();
    std::fs::write(&local, "not json").unwrap();
    let saved = load_saved_prompts_from(&[
        (system, PromptScope::System),
        (user, PromptScope::User),
        (project, PromptScope::Project),
        (local, PromptScope::WorkingCopy),
    ]);
    // Later sources win; missing keys and invalid files read empty.
    assert_eq!(saved.get("k").map(|s| s.response.as_str()), Some("y"));
    assert_eq!(saved.get("k").map(|s| s.scope), Some(PromptScope::User));
}

#[test]
fn write_saved_prompt_replaces_and_appends() {
    let dir = temp_dir("prompts-write");
    // The parent directory does not exist yet; writing creates it.
    let path = dir.join("sub").join("arcrc");
    write_saved_prompt(&path, "k", "y").unwrap();
    write_saved_prompt(&path, "other", "n").unwrap();
    write_saved_prompt(&path, "k", "n").unwrap();
    // No staging files leak next to the destination.
    let mut entries: Vec<String> = std::fs::read_dir(dir.join("sub"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, vec!["arcrc".to_string()]);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json.get("prompts").unwrap(),
        &serde_json::json!([
            {"prompt": "other", "response": "n"},
            {"prompt": "k", "response": "n"},
        ])
    );
    // Map-form values convert to lists; other keys survive.
    std::fs::write(&path, r#"{"hosts": {"x": 1}, "prompts": {"k": "y"}}"#).unwrap();
    write_saved_prompt(&path, "z", "n").unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        json.get("prompts").unwrap(),
        &serde_json::json!([
            {"prompt": "k", "response": "y"},
            {"prompt": "z", "response": "n"},
        ])
    );
    assert!(json.get("hosts").is_some());
    // Malformed `prompts` values fail like PHP.
    std::fs::write(&path, r#"{"prompts": 42}"#).unwrap();
    assert_eq!(
        write_saved_prompt(&path, "k", "y").unwrap_err(),
        "Expected a natural list!"
    );
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
fn git_onto_remote_selects_perforce_mode() {
    let repo = make_repo("onto-p4");
    // A `p4` remote without `refs/remotes/p4` is not a Perforce remote.
    assert!(!git_is_perforce_remote(&repo, "p4"));
    assert!(!git_is_perforce_remote(&repo, "origin"));
    std::fs::create_dir_all(repo.join(".git").join("refs").join("remotes").join("p4")).unwrap();
    assert!(git_is_perforce_remote(&repo, "p4"));
    let mut io = Io::canned(&[]);
    let options = Options {
        onto_remote: Some("p4".to_string()),
        ..Options::default()
    };
    assert_eq!(
        git_select_onto_remote(&repo, &mut io, &options, &["master".to_string()]).unwrap(),
        "p4"
    );
    assert!(io.out.stderr.contains("** P4 MODE **"));
    // Merge strategy is banned in Perforce mode.
    let mut io = Io::canned(&[]);
    let options = Options {
        onto_remote: Some("p4".to_string()),
        strategy: Some("merge".to_string()),
        ..Options::default()
    };
    assert_eq!(
        git_select_onto_remote(&repo, &mut io, &options, &["master".to_string()]).unwrap_err(),
        "Perforce mode does not support the \"merge\" land strategy. Use the \"squash\" land \
         strategy when landing to a Perforce remote (you can use \"--squash\" to select this \
         strategy)."
    );
}

#[test]
fn git_p4_submit_argv_matches_php_passthru() {
    assert_eq!(
        git_p4_submit_argv("aaa"),
        vec![
            "-c",
            "git-p4.skipSubmitEdit=true",
            "-c",
            "git-p4.skipSubmitEditCheck=true",
            "p4",
            "submit",
            "--disable-rebase",
            "-M",
            "--conflict=quit",
            "--commit",
            "aaa",
            "--",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>()
    );
}

#[test]
fn git_p4_submit_maps_failures_through_seam() {
    // Spawning a directory always fails, on every platform: the seam maps
    // spawn failures to the verbatim submit error, like PHP passthru.
    let dir = temp_dir("p4-missing");
    let program = dir.to_string_lossy().into_owned();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_p4_submit_with(&dir, &program, &[], &mut io, "p4", "aaa",).unwrap_err(),
        "Submit failed! Fix the error and run \"arc land\" again."
    );
    assert!(io.out.stderr.contains("** SUBMITTING **"));
    assert_eq!(
        git_p4_sync_with(&dir, &program, &[], "p4", "master",).unwrap_err(),
        "Perforce sync failed! Fix the error and run \"arc land\" again."
    );
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
    // New markers render as ref views outside the banner body.
    assert!(io.out.stdout.contains("  *   Branch \"fresh\"  \n"));
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

fn saved_state(repo: &Path) -> GitLocalState {
    save_local_state_git(repo, &mut Io::canned(&[])).unwrap()
}

fn repo_with_tracking_master(name: &str) -> PathBuf {
    let repo = make_repo(name);
    let bare = temp_dir(&format!("{name}-bare"));
    git(&bare, &["init", "--bare", "-q"]);
    git(&repo, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&repo, &["push", "-q", "origin", "master"]);
    git(&repo, &["fetch", "-q", "origin"]);
    git(
        &repo,
        &["branch", "--set-upstream-to=origin/master", "master"],
    );
    repo
}

#[test]
fn git_reconcile_updates_tracking_branches_to_landed_commit() {
    let repo = repo_with_tracking_master("reconcile-update");
    git(&repo, &["checkout", "-qb", "feature"]);
    let state = saved_state(&repo);
    // Simulated landed squash on a detached head, then the pruned branch.
    git(&repo, &["checkout", "-q", "--detach", "master"]);
    let landed = commit_file(&repo, "landed.txt", "landed\n", "squash");
    git(&repo, &["branch", "-D", "feature"]);
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &landed,
        Some("master"),
        "origin",
        &["master".to_string(), "missing".to_string()],
        &["feature".to_string()],
    )
    .unwrap();
    assert!(io.out.stderr.contains("** UPDATE **"), "{}", io.out.stderr);
    assert!(
        io.out.stderr.contains("** CHECKOUT **"),
        "{}",
        io.out.stderr
    );
    assert_eq!(git(&repo, &["rev-parse", "master"]), landed);
    assert_eq!(git(&repo, &["branch", "--show-current"]), "master");
}

#[test]
fn git_reconcile_returns_to_unimpacted_original_branch() {
    let repo = make_repo("reconcile-return");
    git(&repo, &["checkout", "-qb", "feature"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let state = saved_state(&repo);
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &head,
        None,
        "origin",
        &["master".to_string()],
        &[],
    )
    .unwrap();
    assert!(io.out.stderr.contains("** RETURN **"), "{}", io.out.stderr);
    assert!(
        io.out.stderr.contains("** LOAD STATE **"),
        "{}",
        io.out.stderr
    );
    assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
}

#[test]
fn git_reconcile_stays_detached_without_pull_path() {
    let repo = make_repo("reconcile-detached");
    git(&repo, &["checkout", "-qb", "feature"]);
    let state = saved_state(&repo);
    git(&repo, &["checkout", "-q", "--detach", "master"]);
    let landed = commit_file(&repo, "landed.txt", "landed\n", "squash");
    git(&repo, &["branch", "-D", "feature"]);
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &landed,
        Some("master"),
        "origin",
        &["master".to_string()],
        &["feature".to_string()],
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("** DETACHED HEAD **"),
        "{}",
        io.out.stderr
    );
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), landed);
    // Nothing tracks the pushed ref, so local master stays stale.
    assert_ne!(git(&repo, &["rev-parse", "master"]), landed);
}

#[test]
fn git_reconcile_stops_at_branch_with_unpublished_changes() {
    let repo = repo_with_tracking_master("reconcile-local-changes");
    // Local-only commit on master, past the landed state.
    let local = commit_file(&repo, "local.txt", "local\n", "local only");
    git(&repo, &["checkout", "-qb", "feature"]);
    let state = saved_state(&repo);
    // Landed squash based on the pushed base, without the local commit.
    git(&repo, &["checkout", "-q", "--detach", "origin/master"]);
    let landed = commit_file(&repo, "landed.txt", "landed\n", "squash");
    git(&repo, &["branch", "-D", "feature"]);
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &landed,
        Some("master"),
        "origin",
        &["master".to_string()],
        &["feature".to_string()],
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("** LOCAL CHANGES **"),
        "{}",
        io.out.stderr
    );
    assert!(
        io.out.stderr.contains("** DETACHED HEAD **"),
        "{}",
        io.out.stderr
    );
    assert!(!io.out.stderr.contains("** UPDATE **"));
    assert!(!io.out.stderr.contains("** CHECKOUT **"));
    assert_eq!(git(&repo, &["rev-parse", "master"]), local);
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
}

#[test]
fn git_reconcile_ignores_cyclic_upstreams() {
    let repo = make_repo("reconcile-cycle");
    git(&repo, &["checkout", "-qb", "feature"]);
    git(&repo, &["config", "branch.feature.remote", "."]);
    git(
        &repo,
        &["config", "branch.feature.merge", "refs/heads/master"],
    );
    git(&repo, &["config", "branch.master.remote", "."]);
    git(
        &repo,
        &["config", "branch.master.merge", "refs/heads/feature"],
    );
    let state = saved_state(&repo);
    git(&repo, &["checkout", "-q", "--detach", "master"]);
    let landed = commit_file(&repo, "landed.txt", "landed\n", "squash");
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &landed,
        Some("master"),
        "origin",
        &["master".to_string()],
        &[],
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("** LOCAL CYCLE **"),
        "{}",
        io.out.stderr
    );
    // Every candidate cycles, so the surviving original branch is restored.
    assert!(io.out.stderr.contains("** RETURN **"), "{}", io.out.stderr);
    assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
}

#[test]
fn git_reconcile_ignores_branches_tracking_other_remotes_or_refs() {
    let repo = repo_with_tracking_master("reconcile-foreign");
    git(&repo, &["checkout", "-qb", "feature"]);
    let state = saved_state(&repo);
    git(&repo, &["checkout", "-q", "--detach", "master"]);
    let landed = commit_file(&repo, "landed.txt", "landed\n", "squash");
    git(&repo, &["branch", "-D", "feature"]);
    // Pushed to another remote: master tracks origin, not other.
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &landed,
        Some("master"),
        "other",
        &["master".to_string()],
        &["feature".to_string()],
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("** DETACHED HEAD **"),
        "{}",
        io.out.stderr
    );
    // Pushed to another ref: master tracks origin/master, not stable.
    let mut io = Io::canned(&[]);
    git_reconcile(
        &repo,
        &mut io,
        &state,
        &landed,
        Some("master"),
        "origin",
        &["stable".to_string()],
        &["feature".to_string()],
    )
    .unwrap();
    assert!(
        io.out.stderr.contains("** DETACHED HEAD **"),
        "{}",
        io.out.stderr
    );
    assert_ne!(git(&repo, &["rev-parse", "master"]), landed);
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
        false,
    );
    assert!(io.out.stdout.contains("** HOLD CHANGES **"));
    assert!(
        io.out
            .stdout
            .contains("git push -- origin abcdef123456:refs/heads/master")
    );
    assert!(io.out.stdout.contains("git checkout feature --"));
    assert!(io.out.stdout.contains("same state as before"));
    // Perforce mode: "submitted", and the manual `git p4 submit` command.
    let mut io = Io::canned(&[]);
    git_hold_text(
        &mut io,
        "p4",
        &["master".to_string()],
        "abcdef1234567890",
        &[],
        true,
    );
    assert!(io.out.stdout.contains("they have not been submitted."));
    assert!(
        io.out
            .stdout
            .contains("git p4 submit -M --commit abcdef1234567890 --")
    );
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
    let err = git_merge_conflict(&mut io, &commits, &set, "aaa", "bbb", false);
    assert_eq!(err, "Encountered a merge conflict.");
    assert!(io.out.stdout.contains("** MERGE CONFLICT **"));
    assert!(io.out.stdout.contains("\"feature\""));
    // The hint renders as a `%?` line, and only without `--incremental`.
    assert!(
        io.out
            .stdout
            .contains("  ?   Use \"--incremental\" to merge and push changes one by one.\n\n")
    );
    let mut io = Io::canned(&[]);
    git_merge_conflict(&mut io, &commits, &set, "aaa", "bbb", true);
    assert!(!io.out.stdout.contains("--incremental"));
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
fn save_local_state_captures_clean_git_tree() {
    let repo = make_repo("local-clean");
    git(&repo, &["checkout", "-b", "feature"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let mut io = Io::canned(&[]);
    let state = save_local_state_git(&repo, &mut io).unwrap();
    assert_eq!(
        state,
        GitLocalState {
            local_ref: Some("feature".to_string()),
            local_commit: head,
            local_path: Some(git::UpstreamPath::new()),
            stashed: false,
        }
    );
    assert!(io.out.is_empty());
}

#[test]
fn save_local_state_stashes_dirty_git_tree() {
    let repo = make_repo("local-dirty");
    git(&repo, &["checkout", "-b", "feature"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    let mut io = Io::canned(&["y"]);
    let state = save_local_state_git(&repo, &mut io).unwrap();
    assert!(state.stashed);
    assert_eq!(state.local_ref.as_deref(), Some("feature"));
    assert_eq!(state.local_commit, head);
    assert!(io.out.stdout.contains("** UNCOMMITTED CHANGES **"));
    assert!(io.out.stdout.contains("Untracked Changes"));
    assert!(io.out.stdout.contains("Unstaged Changes"));
    assert!(io.out.stdout.contains(".git/info/exclude"));
    assert!(io.out.stderr.contains("** SAVE STASH **"));
    // The tree is clean again; discarding applies the stash back.
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    discard_local_state_git(&repo, &mut io, &state).unwrap();
    assert!(io.out.stderr.contains("** LOAD STASH **"));
    assert_eq!(
        std::fs::read_to_string(repo.join("file.txt")).unwrap(),
        "dirty\n"
    );
    assert!(repo.join("new.txt").is_file());
    // Declining the stash prompt aborts.
    std::fs::write(repo.join("file.txt"), "dirty\n").unwrap();
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        save_local_state_git(&repo, &mut io).unwrap_err(),
        "User aborted the workflow."
    );
}

#[test]
fn restore_local_state_checks_out_saved_git_ref() {
    let repo = make_repo("local-restore");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "f.txt", "f\n", "feature work");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let mut io = Io::canned(&[]);
    let state = save_local_state_git(&repo, &mut io).unwrap();
    git(&repo, &["checkout", "master"]);
    restore_local_state_git(&repo, &mut io, &state).unwrap();
    assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
    assert!(io.out.stderr.contains("** LOAD STATE **"));
    assert_eq!(
        git_restore_commands(&state),
        vec![format!(
            "git checkout -B feature {} --",
            head.chars().take(12).collect::<String>()
        )]
    );
    // Detached HEAD restores the commit and renders without `-B`.
    git(&repo, &["checkout", "--detach", "HEAD"]);
    let detached = save_local_state_git(&repo, &mut Io::canned(&[])).unwrap();
    assert_eq!(detached.local_ref, None);
    assert_eq!(
        git_restore_commands(&detached),
        vec![format!(
            "git checkout {} --",
            head.chars().take(12).collect::<String>()
        )]
    );
}

#[test]
fn change_lists_split_status_masks() {
    let mut status = std::collections::BTreeMap::new();
    status.insert("clean.txt".to_string(), 0);
    status.insert("added.txt".to_string(), git::FLAG_UNTRACKED);
    status.insert(
        "edit.txt".to_string(),
        git::FLAG_UNCOMMITTED | git::FLAG_UNSTAGED,
    );
    status.insert("stage.txt".to_string(), git::FLAG_UNCOMMITTED);
    let lists = change_lists(&status, &GIT_CHANGE_FLAGS);
    assert_eq!(lists.conflicts, Vec::<String>::new());
    assert_eq!(lists.untracked, vec!["added.txt".to_string()]);
    assert_eq!(lists.unstaged, vec!["edit.txt".to_string()]);
    assert_eq!(lists.uncommitted, vec!["stage.txt".to_string()]);
}

#[test]
fn gate_merge_conflicts_lists_files_and_fails() {
    let dir = temp_dir("local-conflicts");
    let mut io = Io::canned(&[]);
    assert!(gate_merge_conflicts(&mut io, &dir, &[]).is_ok());
    let mut io = Io::canned(&[]);
    let err = gate_merge_conflicts(&mut io, &dir, &["a.txt".to_string()]).unwrap_err();
    assert_eq!(err, "Resolve merge conflicts before proceeding.");
    assert!(io.out.stdout.contains("** MERGE CONFLICTS **"));
    assert!(io.out.stdout.contains("Untracked Changes"));
    assert!(io.out.stdout.contains("    a.txt\n"));
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
    // T1153 R4 (LAND-37): PHP renders usage errors with no usage
    // dump; the envelope (`USAGE EXCEPTION` label) applies at `run_in`.
    let err = execute_with_io(&dir, &args(&["--nope"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err, "Unknown argument 'nope'. Try 'arc help'.");
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

fn ansi_io() -> Io {
    let mut io = Io::canned(&[]);
    io.ansi = true;
    io
}

#[test]
fn status_line_renders_blue_writer() {
    assert_eq!(
        status_line("OK", "done", "blue", true),
        "\x1b[1m\x1b[44m OK \x1b[49m\x1b[m done\n"
    );
    assert_eq!(status_line("OK", "done", "blue", false), "** OK ** done\n");
}

#[test]
fn out_push_status_with_ansi_writes_stderr() {
    let mut out = Out::default();
    out.push_status_with_ansi("OK", "done", "blue", true);
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "\x1b[1m\x1b[44m OK \x1b[49m\x1b[m done\n");
    let mut plain = Out::default();
    plain.push_status_with_ansi("OK", "done", "blue", false);
    assert_eq!(plain.stderr, "** OK ** done\n");
}

#[test]
fn io_push_status_follows_ansi_flag() {
    let mut io = ansi_io();
    io.push_status("OK", "done");
    assert_eq!(io.out.stderr, "\x1b[1m\x1b[44m OK \x1b[49m\x1b[m done\n");
    let mut plain = Io::canned(&[]);
    assert!(!plain.ansi);
    plain.push_status("OK", "done");
    assert_eq!(plain.out.stderr, "** OK ** done\n");
}

#[test]
fn push_banner_renders_badge() {
    let mut io = ansi_io();
    push_banner(&mut io, "HELLO", "body text");
    assert_eq!(
        io.out.stdout,
        "\n\x1b[43m\x1b[1m <!> HELLO \x1b[m\x1b[49m\nbody text\n\n"
    );
    let mut plain = Io::canned(&[]);
    push_banner(&mut plain, "HELLO", "body text");
    assert_eq!(plain.out.stdout, "\n** HELLO **\nbody text\n\n");
}

#[test]
fn push_banner_continued_keeps_single_trailing_newline() {
    let mut io = ansi_io();
    push_banner_continued(&mut io, "HELLO", "body text");
    assert_eq!(
        io.out.stdout,
        "\n\x1b[43m\x1b[1m <!> HELLO \x1b[m\x1b[49m\nbody text\n"
    );
    let mut plain = Io::canned(&[]);
    push_banner_continued(&mut plain, "HELLO", "body text");
    assert_eq!(plain.out.stdout, "\n** HELLO **\nbody text\n");
}

#[test]
fn push_banner_mode_unwrapped_keeps_long_body_verbatim_on_tty() {
    let body = "word ".repeat(20);
    let mut io = ansi_io();
    push_banner_mode(&mut io, "HOLD CHANGES", &body, false);
    assert_eq!(
        io.out.stdout,
        format!("\n\x1b[43m\x1b[1m <!> HOLD CHANGES \x1b[m\x1b[49m\n{body}\n\n")
    );
    let mut plain = Io::canned(&[]);
    push_banner_mode(&mut plain, "HOLD CHANGES", &body, false);
    assert_eq!(
        plain.out.stdout,
        format!("\n** HOLD CHANGES **\n{}\n\n", console_wrap(&body, 0, true))
    );
}

#[test]
fn render_hint_with_ansi_badges_question() {
    assert_eq!(
        render_hint_with_ansi("Use X.", true),
        "\x1b[42m\x1b[1m  ?  \x1b[m\x1b[49m Use X."
    );
    assert_eq!(render_hint_with_ansi("Use X.", false), "  ?   Use X.");
}

#[test]
fn ref_view_render_with_ansi_styles_badge_and_name() {
    let view = RefView {
        object_name: Some("D123".to_string()),
        title: "Work".to_string(),
        ..Default::default()
    };
    assert_eq!(
        view.render_with_ansi(true),
        "\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \x1b[1mD123\x1b[m Work\n"
    );
    assert_eq!(view.render_with_ansi(false), "  *   D123 Work\n");
}

#[test]
fn ref_view_render_with_ansi_underlines_uri() {
    let view = RefView {
        uri: Some("https://phab.example/D123".to_string()),
        ..Default::default()
    };
    assert_eq!(
        view.render_with_ansi(true),
        "\x1b[46m\x1b[1m  *  \x1b[m\x1b[49m \n\
         \x1b[46m\x1b[1m :// \x1b[m\x1b[49m   \x1b[4mhttps://phab.example/D123\x1b[m\n"
    );
    assert_eq!(
        view.render_with_ansi(false),
        "  *   \n ://    https://phab.example/D123\n"
    );
}

#[test]
fn push_prompt_query_styles_marker() {
    let mut io = ansi_io();
    push_prompt_query(&mut io, "Land?");
    assert_eq!(
        io.out.stdout,
        "\x1b[1m\x1b[46m >>> \x1b[49m\x1b[m Land? [y/N/?] "
    );
    let mut plain = Io::canned(&[]);
    push_prompt_query(&mut plain, "Land?");
    assert_eq!(plain.out.stdout, ">>> Land? [y/N/?] ");
}

#[test]
fn push_saved_notice_closes_paren_on_tty() {
    let mut io = ansi_io();
    push_saved_notice(&mut io, "arc.land.confirm", "Land?", "y");
    assert_eq!(
        io.out.stdout,
        "\x1b[46m\x1b[1m >>> \x1b[m\x1b[49m Land? \x1b[1m<y>\x1b[m\n\
         \x1b[46m\x1b[1m >>> \x1b[m\x1b[49m \
         (Using saved response to prompt \"arc.land.confirm\".)\n\n"
    );
    let mut plain = Io::canned(&[]);
    push_saved_notice(&mut plain, "arc.land.confirm", "Land?", "y");
    assert_eq!(
        plain.out.stdout,
        ">>> Land? <y>\n>>> (Using saved response to prompt \"arc.land.confirm\".\n\n"
    );
}

#[test]
fn push_save_notice_uses_green_writer() {
    let mut io = ansi_io();
    push_save_notice(&mut io, "k", "y");
    assert_eq!(
        io.out.stdout,
        "\x1b[42m\x1b[1m SAVE PROMPT \x1b[m\x1b[49m Saving response \"y\" to prompt \"k\".\n"
    );
    let mut plain = Io::canned(&[]);
    push_save_notice(&mut plain, "k", "y");
    assert_eq!(
        plain.out.stdout,
        "** SAVE PROMPT ** Saving response \"y\" to prompt \"k\".\n"
    );
}

#[test]
fn push_prompt_help_renders_header_and_hints() {
    let mut io = ansi_io();
    push_prompt_help(&mut io, "arc.land.confirm");
    assert_eq!(
        io.out.stdout,
        "\n\x1b[42m\x1b[1m PROMPT \x1b[m\x1b[49m \x1b[1marc.land.confirm\x1b[m\n\n\
         Confirms that the correct changes have been selected to land.\n\n\
         The default response to this prompt is \"n\".\n\n\
         \x1b[42m\x1b[1m  ?  \x1b[m\x1b[49m \
         Use \"*\" after a response to save it in user configuration.\n\
         \x1b[42m\x1b[1m  ?  \x1b[m\x1b[49m \
         Use \"!\" after a response to save it in working copy configuration.\n\
         \x1b[42m\x1b[1m  ?  \x1b[m\x1b[49m \
         Run \"arc help prompts\" for detailed help on configuring responses.\n\n"
    );
    let mut plain = Io::canned(&[]);
    push_prompt_help(&mut plain, "arc.land.confirm");
    assert_eq!(
        plain.out.stdout,
        "\n** PROMPT ** **arc.land.confirm**\n\n\
         Confirms that the correct changes have been selected to land.\n\n\
         The default response to this prompt is \"n\".\n\n\
         \x20\x20?   Use \"*\" after a response to save it in user configuration.\n\
         \x20\x20?   Use \"!\" after a response to save it in working copy configuration.\n\
         \x20\x20?   Run \"arc help prompts\" for detailed help on configuring responses.\n\n"
    );
}

#[test]
fn select_strategy_with_ansi_reports_default() {
    let (strategy, line) = select_strategy_with_ansi(None, None, true).unwrap();
    assert_eq!(strategy, Strategy::Squash);
    assert_eq!(
        line,
        "\x1b[1m\x1b[44m STRATEGY \x1b[49m\x1b[m \
         Merging with \"squash\" strategy, the default strategy.\n"
    );
    let (_, plain) = select_strategy_with_ansi(None, None, false).unwrap();
    assert_eq!(
        plain,
        "** STRATEGY ** Merging with \"squash\" strategy, the default strategy.\n"
    );
}

#[test]
fn format_skip_note_with_ansi_carries_no_markup() {
    let expected =
        "Note: revision info unavailable (boom); landing without revision association.\n";
    assert_eq!(format_skip_note_with_ansi("boom", true), expected);
    assert_eq!(format_skip_note_with_ansi("boom", false), expected);
    assert!(!format_skip_note_with_ansi("boom", true).contains('\x1b'));
}

#[test]
fn format_gate_preview_with_ansi_lists_findings_plain() {
    assert_eq!(
        format_gate_preview_with_ansi(&[], true),
        "Pre-land checks passed.\n"
    );
    let findings = [land_checks::GateFinding {
        prompt: "arc.land.confirm",
        title: "T".to_string(),
        query: "Q".to_string(),
        count: 1,
    }];
    let expected = "Pre-land checks:\n  [arc.land.confirm] T: Q\n";
    assert_eq!(format_gate_preview_with_ansi(&findings, true), expected);
    assert_eq!(format_gate_preview_with_ansi(&findings, false), expected);
}

#[test]
fn commit_set_line_highlights_implicit_on_tty() {
    assert_eq!(
        commit_set_line("abc", "subj", true, true),
        "       ! \x1b[43m abc \x1b[49m subj\n"
    );
    assert_eq!(
        commit_set_line("abc", "subj", true, false),
        "       ! abc subj\n"
    );
    assert_eq!(
        commit_set_line("abc", "subj", false, true),
        "          abc  subj\n"
    );
    assert_eq!(
        commit_set_line("abc", "subj", false, false),
        "          abc  subj\n"
    );
}

#[test]
fn render_commit_set_with_ansi_styles_implicit() {
    let mut commit = land_commit("abcdef123456", &[], "Do work");
    commit.is_implicit = true;
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    assert_eq!(
        render_commit_set_with_ansi(&set, &[commit], &|hash| hash[..7].to_string(), true),
        "\n  (no associated revision)\n       ! \x1b[43m abcdef1 \x1b[49m Do work\n"
    );
    let mut commit = land_commit("abcdef123456", &[], "Do work");
    commit.is_implicit = true;
    assert_eq!(
        render_commit_set_with_ansi(&set, &[commit], &|hash| hash[..7].to_string(), false),
        "\n  (no associated revision)\n       ! abcdef1 Do work\n"
    );
}

#[test]
fn shell_line_bolds_command_on_tty() {
    assert_eq!(
        shell_line("git push", true),
        "    \x1b[1m$ git push\x1b[m\n"
    );
    assert_eq!(shell_line("git push", false), "    $ git push\n");
}

#[test]
fn status_line_renders_warning_and_success_colors() {
    assert_eq!(
        status_line("TARGET", "fetching", "yellow", true),
        "\x1b[1m\x1b[43m TARGET \x1b[49m\x1b[m fetching\n"
    );
    assert_eq!(
        status_line("DONE", "Landed changes.", "green", true),
        "\x1b[1m\x1b[42m DONE \x1b[49m\x1b[m Landed changes.\n"
    );
    assert_eq!(
        status_line("TARGET", "fetching", "yellow", false),
        "** TARGET ** fetching\n"
    );
    assert_eq!(
        status_line("DONE", "Landed changes.", "green", false),
        "** DONE ** Landed changes.\n"
    );
}

#[test]
fn io_push_status_with_ansi_colors_warnings() {
    let mut io = ansi_io();
    io.push_status_with_ansi("P4 MODE", "perforce", "yellow", io.ansi);
    assert_eq!(
        io.out.stderr,
        "\x1b[1m\x1b[43m P4 MODE \x1b[49m\x1b[m perforce\n"
    );
    let mut plain = Io::canned(&[]);
    plain.push_status_with_ansi("P4 MODE", "perforce", "yellow", plain.ansi);
    assert_eq!(plain.out.stderr, "** P4 MODE ** perforce\n");
}

#[test]
fn push_builds_banner_styles_title_on_tty() {
    let mut io = ansi_io();
    push_builds_banner(
        &mut io,
        "BUILD FAILURES",
        "1 revision(s) have build failures:",
    );
    assert_eq!(
        io.out.stdout,
        "\x1b[43m\x1b[1m <!> BUILD FAILURES \x1b[m\x1b[49m\n1 revision(s) have build \
         failures:\n\n"
    );
    let mut plain = Io::canned(&[]);
    push_builds_banner(
        &mut plain,
        "BUILD FAILURES",
        "1 revision(s) have build failures:",
    );
    assert_eq!(
        plain.out.stdout,
        "** BUILD FAILURES **\n1 revision(s) have build failures:\n\n"
    );
}

#[test]
fn push_state_banner_renders_working_copy_line() {
    let files = vec!["a.txt".to_string()];
    let lists = vec![("Added", files.as_slice())];
    // `Path::display` uses the platform separator (`\tmp\wc` on Windows).
    let dir_text = Path::new("/tmp/wc").display().to_string();
    let mut io = ansi_io();
    push_state_banner(&mut io, "TITLE", "message", Path::new("/tmp/wc"), &lists);
    assert_eq!(
        io.out.stdout,
        format!(
            "\n\x1b[43m\x1b[1m <!> TITLE \x1b[m\x1b[49m\nmessage\n\n    Working Copy: \
             {dir_text}\n\nAdded\n\n    a.txt\n\n"
        )
    );
    let mut plain = Io::canned(&[]);
    push_state_banner(&mut plain, "TITLE", "message", Path::new("/tmp/wc"), &lists);
    assert_eq!(
        plain.out.stdout,
        format!("\n** TITLE **\nmessage\n\n    Working Copy: {dir_text}\n\nAdded\n\n    a.txt\n\n")
    );
}

#[test]
fn render_file_lists_renders_titles_and_paths_in_both_modes() {
    let added = vec!["a.txt".to_string()];
    let empty: Vec<String> = Vec::new();
    let removed = vec!["gone.txt".to_string()];
    let lists = vec![
        ("Added", added.as_slice()),
        ("Empty", empty.as_slice()),
        ("Removed", removed.as_slice()),
    ];
    let expected = "Added\n\n    a.txt\n\nRemoved\n\n    gone.txt\n\n";
    assert_eq!(render_file_lists(&lists, true), expected);
    assert_eq!(render_file_lists(&lists, false), expected);
}

#[test]
fn console_wrap_at_hint_indent_matches_phutil_wrap() {
    // Hand-traced `phutil_console_wrap($text, 6, false)`: width 72, the
    // first line keeps no prefix and trims to "  ?", continuation
    // lines gain six spaces.
    let text = format!("  ?   {} {}", "a".repeat(66), "b".repeat(10));
    assert_eq!(
        console_wrap(&text, 6, false),
        format!("  ?\n      {}\n      {}", "a".repeat(66), "b".repeat(10))
    );
    assert_eq!(console_wrap("  ?   Use X.", 6, false), "  ?   Use X.");
}

#[test]
fn r4_land37_toolset_usage_matcher() {
    use arc_rs::conduit::EnvelopeKind;
    // Engine errors PHP raises as `PhutilArgumentUsageException`.
    for message in [
        "There are no commits to land.",
        "Encountered a merge conflict.",
        "Landing onto multiple branches at once is not supported in Mercurial.",
        "TODO: You are forcing a revision, but ...",
        "Symbol \"D1\" does not identify a valid revision.",
        "Symbol \"x\" does not identify a bookmark, branch, or commit.",
        "Symbol \"x\" is ambiguous.",
        "Revision for commit \"abc\" is ambiguous. Use \"--revision\" to force selection of a particular revision.",
        "Unable to determine revision for commit \"abc\".",
        "Merging local \"abc\" into \"def\" produces an empty diff. This usually means these changes have already landed.",
        "Branch \"b\" does not exist in the local working copy.",
        "No pushable remote \"r\" exists. Use the \"--onto-remote\" flag to choose a valid, pushable remote to land changes onto.",
        "Perforce mode does not support the \"merge\" land strategy. Use the \"squash\" land strategy when landing to a Perforce remote (you can use \"--squash\" to select this strategy).",
        "The \"onto\" refs you have selected are connected to multiple different remotes via Git branch upstreams. Use \"--onto-remote\" to select a single remote.",
        "The branches you are landing are connected to multiple different remote branches via Git branch upstreams. Use \"--onto\" to select the refs you want to push to.",
        "Selected \"onto\" ref \"\" is invalid: the empty string is not a valid ref.",
        "Remote \"r\", specified with \"--into\", is not a valid fetchable remote.",
        "Local ref \"l\" does not exist.",
        "Ref \"r\" does not exist in remote \"o\".",
        "No remote \"r\" exists in this repository.",
    ] {
        assert!(is_toolset_usage_message(message), "{message}");
    }
    // Operation failures escape into a fatal instead.
    for message in [
        "Fetch of \"m\" from remote \"o\" failed! Fix the error and run \"arc land\" again.",
        "Push failed! Fix the error and run \"arc land\" again.",
        "Perforce sync failed! Fix the error and run \"arc land\" again.",
        "Branch \"b\" does not rebase cleanly from \"a\" onto \"c\", halting.",
        "transport exploded",
    ] {
        assert!(!is_toolset_usage_message(message), "{message}");
    }
    assert_ne!(
        EnvelopeKind::ToolsetUsage.exit_code(),
        EnvelopeKind::Fatal.exit_code()
    );
}

#[test]
fn r4_land37_classified_parse_and_non_repo_are_usage() {
    use arc_rs::conduit::EnvelopeKind;
    let dir = temp_dir("r4-land37");
    let err =
        execute_with_io_classified(&dir, &args(&["--nope"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::ToolsetUsage);
    assert_eq!(err.message, "Unknown argument 'nope'. Try 'arc help'.");
    let err =
        execute_with_io_classified(&dir, &args(&["--preview"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::ToolsetUsage);
    assert_eq!(err.message, NOT_GIT_OR_HG);
}

#[test]
fn r4_land37_engine_error_kinds() {
    use arc_rs::conduit::EnvelopeKind;
    // Declines surface as exactly `ABORT` (e.g. `prompt_for_key`
    // with "n"), routed to the `---` arm (exit 1).
    let mut io = Io::canned(&["n"]);
    assert_eq!(prompt_for_key(&mut io, "k", "Land?").unwrap_err(), ABORT);
    assert_eq!(
        classify_engine_error(ABORT.to_string()).kind,
        EnvelopeKind::UserAbort
    );
    assert_eq!(
        classify_engine_error("There are no commits to land.".to_string()).kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_engine_error("Push failed! Fix the error and run \"arc land\" again.".to_string())
            .kind,
        EnvelopeKind::Fatal
    );
}
