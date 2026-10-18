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
    // T1153 B21/B23: TTY runs need no `--`.
    let options = parse_options_with_tty(&args(&[]), true).unwrap().unwrap();
    assert_eq!(options, Options::default());
    assert!(options.provided.is_empty());
}

#[test]
fn parse_covers_every_flag() {
    // T1153 B21/B23: TTY runs need no `--` (argv already carries `--`).
    let options = parse_options_with_tty(
        &args(&[
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
        ]),
        true,
    )
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
    // T1153 B21/B23: TTY runs need no `--` (argv already carries `--`).
    let options = parse_options_with_tty(&args(&["a", "--", "b"]), true)
        .unwrap()
        .unwrap();
    assert_eq!(options.refs, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn parse_help_short_circuits() {
    // T1153 B21/B23: TTY runs need no `--`.
    assert!(
        parse_options_with_tty(&args(&["--help"]), true)
            .unwrap()
            .is_none()
    );
    assert!(
        parse_options_with_tty(&args(&["--hold", "-h"]), true)
            .unwrap()
            .is_none()
    );
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
    // T1153 B21/B23: TTY runs need no `--`.
    let options = parse_options_with_tty(&args(&["--hold=x"]), true)
        .unwrap()
        .unwrap();
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
    // LAND-31: the banner shows the viewer monogram (`@alice`).
    assert_eq!(viewer.name, "@alice");
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
    // T1153 CORE-12/T1153 CORE-33: discovery reads the merged config from the target working
    // copy. Shadow options keep the ambient `~/.arcrc` out; the project
    // file only counts at a working-copy root.
    let dir = temp_dir("client-dir");
    let empty = dir.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    let shadow = arc_rs::get_config::LoadOptions {
        config_files: vec![empty],
        ..Default::default()
    };
    let err = match client_for_dir_with(None, &dir, None, &shadow) {
        Ok(_) => panic!("expected no conduit URI"),
        Err(err) => err,
    };
    assert_eq!(
        err,
        "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with \
         .arcconfig"
    );
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"phabricator.uri": "https://phab.example/"}"#,
    )
    .unwrap();
    let err = match client_for_dir_with(None, &dir, None, &shadow) {
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
    // LAND-4: first anchored `D123`-or-URI value only; anything else is
    // invalid (PHP throws, the land message query skips).
    assert_eq!(
        differential_revision_ids("Subject\n\nDifferential Revision: https://phab.test/D123\n"),
        vec![123]
    );
    assert_eq!(
        differential_revision_ids("Differential Revision: D7\n"),
        vec![7]
    );
    assert_eq!(
        differential_revision_ids("differential revision: d7\n"),
        vec![7]
    );
    assert_eq!(
        differential_revision_ids("Differential Revision: https://phab.test/D123/\n"),
        vec![123]
    );
    assert!(differential_revision_ids("differential revision: D7 and D9").is_empty());
    assert_eq!(
        differential_revision_ids("Differential Revision: D7\nquoted Differential Revision: D9\n"),
        vec![7]
    );
    assert!(differential_revision_ids("Differential Revision: 42").is_empty());
    assert!(differential_revision_ids("Differential Revision: D0").is_empty());
    assert!(differential_revision_ids("Differential Revision: D007").is_empty());
    assert!(differential_revision_ids("  Differential Revision: D7").is_empty());
    assert!(differential_revision_ids("Differential Revision:").is_empty());
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
    // LAND-2: message-derived IDs resolve via `differential.revision.search`
    // (rows keep `diffPHID`), not legacy `differential.query`.
    let call = canned(vec![(
        diff_submit::METHOD_REVISION_SEARCH,
        serde_json::json!({"data": [modern_row(12, "accepted", "Accepted", false, "PHID-USER-a")]}),
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
    assert_eq!(
        commits[0].explicit.as_ref().unwrap().diff_phid.as_deref(),
        Some("PHID-DIFF-12")
    );
}

#[test]
fn discovery_rejects_ambiguous_without_forced_revision() {
    // LAND-3: ambiguity comes from the union of message and hash results;
    // here two hash-echoed rows attach to one commit.
    let mut row11 = legacy_row(11);
    row11["hashes"] = serde_json::json!([["gtcm", "aaa"]]);
    let mut row12 = legacy_row(12);
    row12["hashes"] = serde_json::json!([["gtcm", "aaa"]]);
    let call = canned(vec![(
        diff_submit::METHOD_QUERY,
        serde_json::json!([row11, row12]),
    )]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    commits[0].indirect_symbols.push("feature".to_string());
    let mut io = Io::canned(&[]);
    let err = load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("no tag".to_string()),
        &|_| Ok(vec![("gtcm".to_string(), "aaa".to_string())]),
        None,
    )
    .unwrap_err();
    assert!(err.contains("is ambiguous"));
    assert!(io.out.stdout.contains("** AMBIGUOUS REVISION **"));
}

#[test]
fn discovery_queries_hashes_without_message_ids() {
    // LAND-3/LAND-17: one batched `commitHashes` query; rows attach via
    // their echoed `hashes`, not by position.
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_QUERY);
        probe.borrow_mut().push(params.clone());
        let mut row = legacy_row(3);
        row["hashes"] = serde_json::json!([["gtcm", "aaa"]]);
        Ok(serde_json::json!([row]))
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
    assert_eq!(seen.borrow().len(), 1);
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

    // LAND-2: the foreign related revision now arrives via
    // `differential.revision.search`.
    let call = canned(vec![(
        diff_submit::METHOD_REVISION_SEARCH,
        serde_json::json!({"data": [modern_row(4, "accepted", "Accepted", false, "PHID-USER-a")]}),
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
    // LAND-38: PHP's title carries a trailing space.
    assert!(io.out.stdout.contains("** 1 OPEN PARENT REVISION(S)  **"));
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
    // LAND-39: bare strings fail with `Expected a list!`, non-string
    // items with the usage error; only string lists parse.
    assert_eq!(
        parse_onto_config(&serde_json::json!("stable")).unwrap_err(),
        "Expected a list!"
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!(["a", "b"])).unwrap(),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!(["a", 1, "b"])).unwrap_err(),
        "Expected a string (at index \"1\"), found \"integer\"."
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!({"x": "a"})).unwrap_err(),
        "Expected a natural list!"
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!(7)).unwrap_err(),
        "Expected a list!"
    );
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

// NOTE (LAND-12/LAND-43): `split_author_handles_malformed` and
// `merge_approaches_follow_strategy` covered helpers the ceb-flow
// restoration deleted; replacements live in the Wave 2 block at the end.
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
    // LAND-33: the hint shows only with previously merged but unpushed
    // sets, regardless of the `--incremental` flag.
    let mut io = Io::canned(&[]);
    let err = git_merge_conflict(&mut io, &commits, &set, "aaa", "bbb", true);
    assert_eq!(err, "Encountered a merge conflict.");
    assert!(io.out.stdout.contains("** MERGE CONFLICT **"));
    assert!(io.out.stdout.contains("\"feature\""));
    // The hint renders as a `%?` line.
    assert!(
        io.out
            .stdout
            .contains("  ?   Use \"--incremental\" to merge and push changes one by one.\n\n")
    );
    let mut io = Io::canned(&[]);
    git_merge_conflict(&mut io, &commits, &set, "aaa", "bbb", false);
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
    // LAND-34: verbatim banner texts and list titles.
    assert!(io.out.stdout.contains("** UNCOMMITTED CHANGES **"));
    assert!(
        io.out
            .stdout
            .contains("You have uncommitted changes in this working copy.")
    );
    assert!(io.out.stdout.contains("Untracked changes in working copy:"));
    assert!(io.out.stdout.contains("Unstaged changes in working copy:"));
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
    // LAND-34: verbatim banner text and list titles; the banner also
    // lists untracked/unstaged/uncommitted files like PHP.
    let dir = temp_dir("local-conflicts");
    let mut io = Io::canned(&[]);
    let clean = ChangeLists {
        conflicts: Vec::new(),
        externals: Vec::new(),
        untracked: Vec::new(),
        unstaged: Vec::new(),
        uncommitted: Vec::new(),
    };
    assert!(gate_merge_conflicts(&mut io, &dir, &clean).is_ok());
    let mut io = Io::canned(&[]);
    let dirty = ChangeLists {
        conflicts: vec!["a.txt".to_string()],
        externals: Vec::new(),
        untracked: vec!["u.txt".to_string()],
        unstaged: Vec::new(),
        uncommitted: vec!["c.txt".to_string()],
    };
    let err = gate_merge_conflicts(&mut io, &dir, &dirty).unwrap_err();
    assert_eq!(err, "Resolve merge conflicts before proceeding.");
    assert!(io.out.stdout.contains("** MERGE CONFLICTS **"));
    assert!(
        io.out
            .stdout
            .contains("You have merge conflicts in this working copy.")
    );
    assert!(io.out.stdout.contains("Merge conflicts in working copy:"));
    assert!(io.out.stdout.contains("Untracked changes in working copy:"));
    assert!(
        io.out
            .stdout
            .contains("Uncommitted changes in working copy:")
    );
    assert!(io.out.stdout.contains("    a.txt\n"));
    assert!(io.out.stdout.contains("    u.txt\n"));
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
    // T1153 R10: `--help` renders the generated workflow help (exact
    // bytes locked by `r10_land_help_matches_generated_format`).
    let dir = temp_dir("exec-misc");
    let mut io = Io::canned(&[]);
    let out = execute_with_io(&dir, &args(&["--help", "--"]), &mut io).unwrap();
    assert_eq!(out.stdout, usage_text(false));
    // T1153 R4 (LAND-37): PHP renders usage errors with no usage
    // dump; the envelope (`USAGE EXCEPTION` label) applies at `run_in`.
    let err = execute_with_io(&dir, &args(&["--nope"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err, "Unknown argument 'nope'. Try 'arc help'.");
    let err = execute_with_io(&dir, &args(&["--preview", "--"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err, NOT_GIT_OR_HG);
    let out = execute(&dir, &args(&["--help", "--"])).unwrap();
    assert!(out.stdout.contains("LAND WORKFLOW"));
}

#[test]
fn execute_preview_without_conduit_fails() {
    // T1153 B21/B23: piped runs carry `--`.
    // LAND-6: even `--preview` needs Conduit (PHP resolves the viewer
    // before the engine runs); there is no offline land anymore.
    let (repo, _bare) = repo_with_origin("preview");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "f.txt", "f\n", "feature work");
    // T1153 CORE-12: discovery now honors ambient user config, so pin a project
    // URI no host entry can match to force the failure hermetically.
    std::fs::write(
        repo.join(".arcconfig"),
        r#"{"phabricator.uri": "https://127.0.0.1:9/"}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    let err = execute_with_io(&repo, &args(&["--preview", "--", "feature"]), &mut io).unwrap_err();
    assert!(err.contains("no token for"), "{err}");
}

#[test]
fn run_in_reports_success_and_failure() {
    // T1153 B21/B23: piped runs carry `--` (`--nope` stays bare: scan errors report first).
    let dir = temp_dir("run-misc");
    assert!(matches!(
        run_in(&dir, &args(&["--help", "--"])),
        ExitCode::SUCCESS
    ));
    assert!(matches!(
        run_in(&dir, &args(&["--nope"])),
        ExitCode::FAILURE
    ));
    assert!(matches!(
        run_in(&dir, &args(&["--preview", "--"])),
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
    // LAND-34: lists are `"\n\n"` separated (PHP's separator echo).
    let expected = "Added\n\n    a.txt\n\n\nRemoved\n\n    gone.txt\n\n";
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
fn r3_land16_revision_search_follows_after_cursor() {
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_REVISION_SEARCH);
        probe.borrow_mut().push(params.clone());
        if probe.borrow().len() == 1 {
            Ok(serde_json::json!({
                "data": [modern_row(89, "accepted", "Accepted", false, "PHID-USER-a")],
                "cursor": {"after": "page2"},
            }))
        } else {
            Ok(serde_json::json!({
                "data": [modern_row(90, "accepted", "Accepted", false, "PHID-USER-a")],
            }))
        }
    };
    let views = fetch_revision_views(&call, &RevisionSymbol::Id(89)).unwrap();
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].monogram, "D89");
    assert_eq!(views[1].monogram, "D90");
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].get("after").is_none());
    assert_eq!(seen[1]["after"], serde_json::json!("page2"));
    assert_eq!(seen[1]["constraints"], serde_json::json!({"ids": [89]}));
}

#[test]
fn r3_land16_build_searches_paginate() {
    let calls: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = calls.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        let page = if params.get("after").is_some() { 2 } else { 1 };
        probe.borrow_mut().push(format!("{method} p{page}"));
        match (method, page) {
            (METHOD_BUILDABLE_SEARCH, 1) => Ok(serde_json::json!({"data": [
                {"id": 7, "phid": "PHID-HMBB-7", "fields": {"objectPHID": "PHID-DIFF-89"}},
            ], "cursor": {"after": "b2"}})),
            // LAND-19: the second page's buildable belongs to a second
            // diff (one buildable per diff PHID, last wins).
            (METHOD_BUILDABLE_SEARCH, _) => Ok(serde_json::json!({"data": [
                {"id": 8, "phid": "PHID-HMBB-8", "fields": {"objectPHID": "PHID-DIFF-90"}},
            ]})),
            (METHOD_BUILD_SEARCH, 1) => Ok(serde_json::json!({"data": [
                {"id": 42, "fields": {
                    "name": "Lint",
                    "buildStatus": {"value": "failed"},
                    "buildablePHID": "PHID-HMBB-7",
                    "buildPlanPHID": "PHID-HMPA-1",
                }},
            ], "cursor": {"after": "u2"}})),
            (METHOD_BUILD_SEARCH, _) => Ok(serde_json::json!({"data": [
                {"id": 43, "fields": {
                    "name": "Unit",
                    "buildStatus": {"value": "passed"},
                    "buildablePHID": "PHID-HMBB-8",
                    "buildPlanPHID": "PHID-HMPA-2",
                }},
            ]})),
            (METHOD_BUILDPLAN_SEARCH, 1) => Ok(serde_json::json!({"data": [
                {"phid": "PHID-HMPA-1", "fields": {"behaviors": {"arc-land": {"value": "always"}}}},
            ], "cursor": {"after": "p2"}})),
            (METHOD_BUILDPLAN_SEARCH, _) => Ok(serde_json::json!({"data": [
                {"phid": "PHID-HMPA-2", "fields": {"behaviors": {"arc-land": {"value": "never"}}}},
            ]})),
            _ => Err(format!("unexpected conduit call {method}")),
        }
    };
    let revisions = vec![
        view(89, "accepted", "Accepted", false, "PHID-USER-a"),
        view(90, "accepted", "Accepted", false, "PHID-USER-a"),
    ];
    let builds = fetch_build_views(&call, &revisions, None).unwrap();
    assert_eq!(builds.len(), 2);
    assert_eq!(builds[0].buildable_monogram, "B7");
    assert_eq!(builds[0].plan_behavior.as_deref(), Some("always"));
    assert_eq!(builds[1].buildable_monogram, "B8");
    assert_eq!(builds[1].plan_behavior.as_deref(), Some("never"));
    assert_eq!(
        calls.borrow().clone(),
        vec![
            "harbormaster.buildable.search p1",
            "harbormaster.buildable.search p2",
            "harbormaster.build.search p1",
            "harbormaster.build.search p2",
            "harbormaster.buildplan.search p1",
            "harbormaster.buildplan.search p2",
        ]
    );
}

// -( R5 TTY gating: LAND-10 )------------------------------------------------

#[test]
fn r5_land10_prompt_requires_tty_first() {
    // `ArcanistPrompt.php:77-88`: piped stdin echoes the raw query,
    // then throws — before any saved response is consulted.
    let mut io = Io::canned(&["y"]);
    io.stdin_tty = false;
    assert_eq!(
        prompt_for_key(&mut io, "k", "Land these changes?").unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    assert_eq!(io.out.stdout, "Land these changes?\n");
    // A saved response does not bypass the gate.
    let mut io = Io::canned(&[]);
    io.stdin_tty = false;
    io.saved.insert(
        "k".to_string(),
        SavedResponse {
            response: "y".to_string(),
            scope: PromptScope::User,
        },
    );
    assert_eq!(
        prompt_for_key(&mut io, "k", "q?").unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    assert_eq!(io.out.stdout, "q?\n");
}

#[test]
fn r5_land10_console_confirm_requires_tty() {
    // `phutil_console_prompt()` echoes, then `require_tty()` throws
    // (`src/console/format.php:42-52`).
    let mut io = Io::canned(&["y"]);
    io.stdin_tty = false;
    assert_eq!(
        console_confirm(&mut io, "Delete?", true).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
    assert_eq!(io.out.stdout, "\n\n    Delete? [y/N] ");
}

#[test]
fn r6_land13_onto_reads_full_merge() {
    // LAND-13: `arc.land.onto` resolves runtime > local > project > user,
    // not `.arcconfig` alone.
    let repo = make_repo("r6-onto");
    std::fs::write(
        repo.join(".arcconfig"),
        r#"{"arc.land.onto": ["from-project"]}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &[]).unwrap(),
        vec!["from-project".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("reading \"arc.land.onto\" configuration")
    );
    // User source via `--arcrc-file`, below project.
    let user = repo.join("arcrc.json");
    std::fs::write(&user, r#"{"config": {"arc.land.onto": ["from-user"]}}"#).unwrap();
    let user_opts = Options {
        arcrc_file: Some(user.to_string_lossy().to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &user_opts, &[]).unwrap(),
        vec!["from-project".to_string()]
    );
    std::fs::remove_file(repo.join(".arcconfig")).unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &user_opts, &[]).unwrap(),
        vec!["from-user".to_string()]
    );
    // Local beats user; `--config` beats local.
    let local_dir = repo.join(".git").join("arc");
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(
        local_dir.join("config"),
        r#"{"arc.land.onto": ["from-local"]}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &user_opts, &[]).unwrap(),
        vec!["from-local".to_string()]
    );
    let rt_opts = Options {
        arcrc_file: user_opts.arcrc_file.clone(),
        config: vec!["arc.land.onto=from-rt".to_string()],
        ..Options::default()
    };
    // LAND-39: a bare runtime string is not a list (PHP throws
    // `Expected a list!` for the merged string value too).
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &rt_opts, &[]).unwrap_err(),
        "Expected a list!"
    );
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
        // T1153 Wave 1 carry-over: push failures rewrap into usage after
        // the cascade retry.
        "Push failed! Fix the error and run \"arc land\" again.",
        "Submit failed! Fix the error and run \"arc land\" again.",
        // LAND-8/LAND-34/LAND-35/LAND-39 usage errors.
        "Desired merge strategy is ambiguous, choose an explicit strategy.",
        "Resolve merge conflicts before proceeding.",
        "You can not continue with uncommitted changes. Commit or discard them before proceeding.",
        "Expected a string (at index \"0\"), found \"integer\".",
    ] {
        assert!(is_toolset_usage_message(message), "{message}");
    }
    // Operation failures escape into a fatal instead.
    for message in [
        "Fetch of \"m\" from remote \"o\" failed! Fix the error and run \"arc land\" again.",
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
fn r6_land13_strategy_and_remote_merge() {
    // LAND-13: `arc.land.strategy` and `arc.land.onto-remote` resolve
    // through the same merge (strategy via `validate_options`).
    let dir = temp_dir("r6-strategy");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"arc.land.strategy": "merge", "arc.land.onto-remote": "proj-remote"}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    let validated = validate_options(&Options::default(), &dir, &mut io).unwrap();
    assert_eq!(validated.strategy, Strategy::Merge);
    assert!(
        io.out
            .stderr
            .contains("configured with \"arc.land.strategy\"")
    );
    let user = dir.join("arcrc.json");
    std::fs::write(
        &user,
        r#"{"config": {"arc.land.strategy": "squash", "arc.land.onto-remote": "user-remote"}}"#,
    )
    .unwrap();
    let user_opts = Options {
        arcrc_file: Some(user.to_string_lossy().to_string()),
        ..Options::default()
    };
    // Project beats user for both keys.
    let mut io = Io::canned(&[]);
    let validated = validate_options(&user_opts, &dir, &mut io).unwrap();
    assert_eq!(validated.strategy, Strategy::Merge);
    assert_eq!(
        land_config_value(&user_opts, &dir, "arc.land.onto-remote").unwrap(),
        Some(serde_json::json!("proj-remote"))
    );
    std::fs::remove_file(dir.join(".arcconfig")).unwrap();
    let mut io = Io::canned(&[]);
    let validated = validate_options(&user_opts, &dir, &mut io).unwrap();
    assert_eq!(validated.strategy, Strategy::Squash);
    // A stray subdir `.arcconfig` never counts (T1153 CORE-33).
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join(".arcconfig"), r#"{"arc.land.strategy": "merge"}"#).unwrap();
    let mut io = Io::canned(&[]);
    let validated = validate_options(&user_opts, &sub, &mut io).unwrap();
    assert_eq!(validated.strategy, Strategy::Squash);
}

#[test]
fn r6_core34_land_config_flags_parse() {
    // T1153 B21/B23: TTY runs need no `--`.
    let opts = parse_options_with_tty(
        &args(&[
            "--config",
            "a=1",
            "--config=b=2",
            "--config-file",
            "f",
            "--arcrc-file=g",
        ]),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(opts.config, ["a=1", "b=2"]);
    assert_eq!(opts.config_file, ["f"]);
    assert_eq!(opts.arcrc_file.as_deref(), Some("g"));
}

#[test]
fn r4_land37_classified_parse_and_non_repo_are_usage() {
    // T1153 B21/B23: piped runs carry `--` to reach the non-repo arm (`--nope` stays bare: scan first).
    use arc_rs::conduit::EnvelopeKind;
    let dir = temp_dir("r4-land37");
    let err =
        execute_with_io_classified(&dir, &args(&["--nope"]), &mut Io::canned(&[])).unwrap_err();
    assert_eq!(err.kind, EnvelopeKind::ToolsetUsage);
    assert_eq!(err.message, "Unknown argument 'nope'. Try 'arc help'.");
    let err = execute_with_io_classified(&dir, &args(&["--preview", "--"]), &mut Io::canned(&[]))
        .unwrap_err();
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
    // T1153 Wave 1 carry-over: push failures rewrap into usage after
    // the cascade retry (`ArcanistLandEngine.php:1278-1290`).
    assert_eq!(
        classify_engine_error("Push failed! Fix the error and run \"arc land\" again.".to_string())
            .kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_engine_error(
            "Submit failed! Fix the error and run \"arc land\" again.".to_string()
        )
        .kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_engine_error(
            "Desired merge strategy is ambiguous, choose an explicit strategy.".to_string()
        )
        .kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_engine_error("Resolve merge conflicts before proceeding.".to_string()).kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_engine_error(
            "You can not continue with uncommitted changes. Commit or discard them before proceeding."
                .to_string()
        )
        .kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_engine_error("Expected a string (at index \"0\"), found \"integer\".".to_string())
            .kind,
        EnvelopeKind::ToolsetUsage
    );
    // Fetch failures stay fatal (plain `ArcanistUsageException`, which the
    // toolset runtime does not catch).
    assert_eq!(
        classify_engine_error(
            "Fetch of \"master\" from remote \"origin\" failed! Fix the error and run \"arc land\" again."
                .to_string()
        )
        .kind,
        EnvelopeKind::Fatal
    );
}

// -( T1153 Wave 2: LAND findings )--------------------------------------------

#[test]
fn land2_message_ids_batch_into_one_search() {
    // One `differential.revision.search` carries every message-derived ID;
    // rows keep `diffPHID` so build gates can run.
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_REVISION_SEARCH);
        probe.borrow_mut().push(params.clone());
        Ok(serde_json::json!({"data": [
            modern_row(11, "accepted", "Accepted", false, "PHID-USER-a"),
            modern_row(12, "accepted", "Accepted", false, "PHID-USER-a"),
        ]}))
    };
    let mut commits = vec![
        land_commit("aaa", &[], "one"),
        land_commit("bbb", &["aaa"], "two"),
    ];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|hash| {
            Ok(if hash == "aaa" {
                "Differential Revision: D11".to_string()
            } else {
                "Differential Revision: D12".to_string()
            })
        },
        &|_| Ok(Vec::new()),
        None,
    )
    .unwrap();
    assert_eq!(seen.borrow().len(), 1);
    let mut ids = seen.borrow()[0]["constraints"]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_i64().unwrap())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![11, 12]);
    assert_eq!(commits[0].explicit.as_ref().unwrap().monogram, "D11");
    assert_eq!(commits[1].explicit.as_ref().unwrap().monogram, "D12");
    assert!(commits[0].explicit.as_ref().unwrap().diff_phid.is_some());
}

#[test]
fn land3_message_and_hash_results_union_phid_deduped() {
    // The same revision found via message and via hash attaches once.
    let mut row = modern_row(12, "accepted", "Accepted", false, "PHID-USER-a");
    row["hashes"] = serde_json::json!([["gtcm", "aaa"]]);
    let call = canned(vec![
        (
            diff_submit::METHOD_REVISION_SEARCH,
            serde_json::json!({"data": [modern_row(12, "accepted", "Accepted", false, "PHID-USER-a")]}),
        ),
        (diff_submit::METHOD_QUERY, serde_json::json!([row])),
    ]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("Differential Revision: D12".to_string()),
        &|_| Ok(vec![("gtcm".to_string(), "aaa".to_string())]),
        None,
    )
    .unwrap();
    assert_eq!(commits[0].related.len(), 1);
    assert_eq!(commits[0].explicit.as_ref().unwrap().monogram, "D12");
}

#[test]
fn land3_rows_without_echoed_hashes_attach_nowhere() {
    // A `differential.query` row whose `hashes` echo nothing (or are
    // absent) matches no commit, like PHP's `$map` lookup.
    let mut foreign = legacy_row(7);
    foreign["hashes"] = serde_json::json!([["gtcm", "zzz"]]);
    let call = canned(vec![(
        diff_submit::METHOD_QUERY,
        serde_json::json!([legacy_row(8), foreign]),
    )]);
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
    assert!(commits[0].related.is_empty());
    assert!(commits[0].explicit.is_none());
}

#[test]
fn land4_invalid_field_skips_message_query() {
    // An invalid `Differential Revision:` value (bare digits here) sends
    // no `revision.search`: PHP throws, the message query skips.
    let call = move |method: &str, _params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_QUERY);
        Ok(serde_json::json!([]))
    };
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("Differential Revision: 42".to_string()),
        &|_| Ok(vec![("gtcm".to_string(), "aaa".to_string())]),
        None,
    )
    .unwrap();
    assert!(commits[0].related.is_empty());
}

#[test]
fn land5_git_hash_reader_sends_gtcm_and_gttr() {
    // LAND-5 is CLEAN (Rule 4): PHP sends both `gtcm` and `gttr`
    // (`ArcanistGitWorkingCopyRevisionHardpointQuery.php:28-45`), and so
    // does the port. Lock the pair shape.
    let dir = make_repo("land5-hashes");
    let head = commit_file(&dir, "a.txt", "a\n", "add a");
    let reader = commit_hash_reader(&dir, Vcs::Git);
    let pairs = reader(&head).unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "gtcm");
    assert_eq!(pairs[0].1, head);
    assert_eq!(pairs[1].0, "gttr");
    assert_eq!(pairs[1].1.len(), 40);
    let reader = commit_hash_reader(&dir, Vcs::Hg);
    assert_eq!(
        reader("abc").unwrap(),
        vec![("hgcm".to_string(), "abc".to_string())]
    );
}

#[test]
fn land17_parent_phids_batch_into_one_search() {
    // Distinct parent PHIDs resolve with a single `revision.search`.
    let seen: std::rc::Rc<std::cell::RefCell<Vec<serde_json::Value>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = seen.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        if method == diff_submit::METHOD_REVISION_SEARCH {
            probe.borrow_mut().push(params.clone());
            return Ok(serde_json::json!({"data": [
                modern_row(1, "accepted", "Accepted", false, "PHID-USER-a"),
                modern_row(2, "accepted", "Accepted", false, "PHID-USER-a"),
            ]}));
        }
        assert_eq!(method, METHOD_EDGE_SEARCH);
        Ok(serde_json::json!({"data": [
            {"sourcePHID": "PHID-DREV-3", "destinationPHID": "PHID-DREV-1"},
            {"sourcePHID": "PHID-DREV-4", "destinationPHID": "PHID-DREV-2"},
        ]}))
    };
    let revisions = vec![
        view(3, "accepted", "Accepted", false, "PHID-USER-a"),
        view(4, "accepted", "Accepted", false, "PHID-USER-a"),
    ];
    let links = fetch_parent_links(&call, &revisions).unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(
        seen.borrow()[0],
        serde_json::json!({"constraints": {"phids": ["PHID-DREV-1", "PHID-DREV-2"]}})
    );
}

#[test]
fn land18_malformed_search_data_degrades_to_empty() {
    // Missing or malformed `data` lists yield no rows instead of errors.
    let call = canned(vec![
        (diff_submit::METHOD_REVISION_SEARCH, serde_json::json!({})),
        (METHOD_EDGE_SEARCH, serde_json::json!({"data": {}})),
        (METHOD_BUILDABLE_SEARCH, serde_json::json!({})),
        (METHOD_BUILD_SEARCH, serde_json::json!({"oops": []})),
        (METHOD_BUILDPLAN_SEARCH, serde_json::json!({"data": []})),
    ]);
    let views = fetch_revision_views(&call, &RevisionSymbol::Id(89)).unwrap();
    assert!(views.is_empty());
    let revisions = vec![view(1, "accepted", "Accepted", false, "PHID-USER-a")];
    assert!(fetch_parent_links(&call, &revisions).unwrap().is_empty());
    let builds = fetch_build_views(&call, &revisions, None).unwrap();
    assert!(builds.is_empty());
    assert!(parse_buildable_rows(&serde_json::json!({})).is_empty());
    assert!(parse_build_rows(&serde_json::json!({"data": 7})).is_empty());
    assert!(parse_plan_behaviors(&serde_json::json!({})).is_empty());
}

#[test]
fn land19_last_buildable_per_diff_wins() {
    // Two buildables for one diff PHID: only the last one's builds join.
    let call = canned(vec![
        (
            METHOD_BUILDABLE_SEARCH,
            serde_json::json!({"data": [
                {"id": 7, "phid": "PHID-HMBB-7", "fields": {"objectPHID": "PHID-DIFF-89"}},
                {"id": 8, "phid": "PHID-HMBB-8", "fields": {"objectPHID": "PHID-DIFF-89"}},
            ]}),
        ),
        (
            METHOD_BUILD_SEARCH,
            serde_json::json!({"data": [
                {"id": 42, "fields": {
                    "name": "Lint", "buildStatus": {"value": "failed"},
                    "buildablePHID": "PHID-HMBB-7", "buildPlanPHID": "PHID-HMPA-1"}},
                {"id": 43, "fields": {
                    "name": "Unit", "buildStatus": {"value": "passed"},
                    "buildablePHID": "PHID-HMBB-8", "buildPlanPHID": "PHID-HMPA-2"}},
            ]}),
        ),
        (
            METHOD_BUILDPLAN_SEARCH,
            serde_json::json!({"data": [
                {"phid": "PHID-HMPA-1", "fields": {"behaviors": {"arc-land": {"value": "always"}}}},
                {"phid": "PHID-HMPA-2", "fields": {"behaviors": {"arc-land": {"value": "never"}}}},
            ]}),
        ),
    ]);
    let revisions = vec![view(89, "accepted", "Accepted", false, "PHID-USER-a")];
    let builds = fetch_build_views(&call, &revisions, None).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].buildable_monogram, "B8");
    assert_eq!(builds[0].build_name, "Unit");
}

#[test]
fn land7_whoami_failure_propagates() {
    // No fallback to the first revision's author: the error surfaces.
    let call = canned(vec![
        (METHOD_EDGE_SEARCH, serde_json::json!({"data": []})),
        (METHOD_BUILDABLE_SEARCH, serde_json::json!({"data": []})),
    ]);
    let revisions = vec![view(
        1,
        "needs-review",
        "Needs Review",
        false,
        "PHID-USER-b",
    )];
    let err = load_gate_findings(&call, &revisions, 1).unwrap_err();
    assert!(err.contains("user.whoami"), "{err}");
}

#[test]
fn land31_author_monograms_render_in_unauthored_banner() {
    let call = canned(vec![(
        METHOD_USER_SEARCH,
        serde_json::json!({"data": [
            {"phid": "PHID-USER-b", "fields": {"username": "bob"}},
        ]}),
    )]);
    let authors = fetch_user_monograms(
        &call,
        &["PHID-USER-b".to_string(), "PHID-USER-zzz".to_string()],
    )
    .unwrap();
    assert_eq!(authors.get("PHID-USER-b").unwrap(), "@bob");
    assert!(!authors.contains_key("PHID-USER-zzz"));

    let revisions = vec![view(1, "accepted", "Accepted", false, "PHID-USER-b")];
    let viewer = ViewerIdentity {
        phid: "PHID-USER-a".to_string(),
        name: "@alice".to_string(),
    };
    let findings = land_checks::evaluate_gates(
        &revisions,
        &viewer.phid,
        &[],
        &["PHID-DREV-1"],
        &[],
        Some(1),
    );
    assert_eq!(findings.len(), 1);
    let mut io = Io::canned(&["y"]);
    confirm_revision_gates_with_authors(
        &mut io,
        &revisions,
        &viewer,
        &[],
        &[],
        &findings,
        &authors,
    )
    .unwrap();
    assert!(
        io.out.stdout.contains("(\"@alice\") are not the author"),
        "{}",
        io.out.stdout
    );
    assert!(io.out.stdout.contains("Author: @bob"), "{}", io.out.stdout);
    assert!(!io.out.stdout.contains("PHID-USER-b"), "{}", io.out.stdout);

    // Unknown authors show no `Author:` line (PHP's missing author ref).
    let mut io = Io::canned(&["y"]);
    confirm_revision_gates(&mut io, &revisions, &viewer, &[], &[], &findings).unwrap();
    assert!(!io.out.stdout.contains("Author:"), "{}", io.out.stdout);
}

#[test]
fn land43_squash_rebase_failure_is_a_conflict() {
    // The ceb flow has no reduce fallback: when the rebase fails, the
    // merge fails with `Encountered a merge conflict.` (and no
    // `MERGE Attempting ...` status lines exist).
    let repo = make_repo("land43-conflict");
    let master = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("file.txt"), "feature\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "feature work"]);
    let tip = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "master"]);
    std::fs::write(repo.join("file.txt"), "master\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "master work"]);
    let into = git(&repo, &["rev-parse", "HEAD"]);
    let commits = vec![land_commit(&tip, &[&master], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let err = git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&into),
        true,
        false,
        &|_| Ok("Landed message".to_string()),
    )
    .unwrap_err();
    assert_eq!(err, "Encountered a merge conflict.");
    assert!(io.out.stdout.contains("** MERGE CONFLICT **"));
    assert!(!io.out.stderr.contains("Attempting to rebase changes."));
    assert!(
        !io.out
            .stderr
            .contains("Attempting to reduce and rebase changes.")
    );
    assert!(!io.out.stderr.contains("Merge succeeded."));
}

#[test]
fn land43_merge_commit_range_conflicts_like_ceb() {
    // The merge-commit-range experiment behind the LAND-43 decision: a
    // range whose replay conflicts fails instead of force-landing.
    let repo = make_repo("land43-range");
    let base = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-qb", "feature"]);
    std::fs::write(repo.join("file.txt"), "B1\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "f1"]);
    let min = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("file.txt"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "f2 revert"]);
    let mid = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-qb", "side", &base]);
    std::fs::write(repo.join("other.txt"), "side\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "side1"]);
    git(&repo, &["checkout", "-q", "feature"]);
    git(&repo, &["merge", "--no-ff", "-m", "merge-side", "side"]);
    let max = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "master"]);
    std::fs::write(repo.join("file.txt"), "B2\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "into"]);
    let into = git(&repo, &["rev-parse", "HEAD"]);
    let commits = vec![
        land_commit(&min, &[&base], "f1"),
        land_commit(&mid, &[&min], "f2 revert"),
        land_commit(&max, &[&mid], "merge-side"),
    ];
    let set = CommitSet {
        revision: None,
        commits: vec![0, 1, 2],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    // The range replays `f1` onto the into state, conflicts, and fails:
    // no reduce fallback retries it.
    let err = git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&into),
        true,
        false,
        &|_| Ok("Landed message".to_string()),
    )
    .unwrap_err();
    assert_eq!(err, "Encountered a merge conflict.");
}

#[test]
fn land12_raw_commit_round_trips_and_rebuilds() {
    // Blob parse/render fidelity plus the into-empty rebuild: no parents
    // under squash, author/committer/message bytes preserved.
    let blob = "tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\n\
        parent 1111111111111111111111111111111111111111\n\
        author Ada <ada@test> 1 +0000\n\
        committer Ron <ron@test> 2 +0000\n\
        \n\
        subject\n\
        \n\
        body\n";
    let raw = parse_raw_commit_blob(blob).unwrap();
    assert_eq!(
        raw.tree.as_deref(),
        Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
    );
    assert_eq!(
        raw.parents,
        vec!["1111111111111111111111111111111111111111".to_string()]
    );
    assert_eq!(raw.author.as_deref(), Some("Ada <ada@test> 1 +0000"));
    assert_eq!(raw.committer.as_deref(), Some("Ron <ron@test> 2 +0000"));
    assert_eq!(raw.message.as_deref(), Some("subject\n\nbody\n"));
    assert_eq!(render_raw_commit_blob(&raw), blob);
    assert_eq!(
        render_raw_commit_blob(&RawCommit::empty()),
        "tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\n\n"
    );
    assert!(parse_raw_commit_blob("tree abc\ngpgsig xyz\n\nm\n").is_err());
    assert!(parse_raw_commit_blob("tree a\ntree b\n\nm\n").is_err());
    assert!(parse_raw_commit_blob("bogus\n").is_err());

    // Into-empty squash: parentless, author from the max commit,
    // committer left to the runner, message intact.
    let repo = make_repo("land12-empty");
    git(&repo, &["checkout", "-b", "feature"]);
    let tip = commit_file(&repo, "f.txt", "f\n", "feature work");
    let commits = vec![land_commit(&tip, &[], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let cursor = git_execute_merge(&repo, &mut io, &set, &commits, None, true, false, &|_| {
        Ok("Landed message".to_string())
    })
    .unwrap();
    assert_eq!(
        git(&repo, &["rev-list", "--parents", "-n1", &cursor])
            .split_whitespace()
            .count(),
        1
    );
    assert_eq!(
        git(&repo, &["log", "--format=%s", "-n1", &cursor]),
        "Landed message"
    );
    assert_eq!(
        git(&repo, &["log", "--format=%an <%ae>", "-n1", &cursor]),
        "Land Test <land@test>"
    );
    // The committer is the runner, not the original author@date: with a
    // distinct identity the commit still lands byte-identically.
    let raw = git_read_raw_commit(&repo, &cursor).unwrap();
    assert!(raw.parents.is_empty());
    assert_eq!(raw.message.as_deref(), Some("Landed message\n"));
}

#[test]
fn land33_hint_tracks_unpushed_sets() {
    // `git_execute_merge` shows the hint exactly when earlier sets are
    // still unpushed (first-set conflicts never hint, even unflagged).
    let repo = make_repo("land33-hint");
    let master = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("file.txt"), "feature\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "feature work"]);
    let tip = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "master"]);
    std::fs::write(repo.join("file.txt"), "master\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "master work"]);
    let into = git(&repo, &["rev-parse", "HEAD"]);
    let commits = vec![land_commit(&tip, &[&master], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&into),
        true,
        false,
        &|_| Ok("Landed".to_string()),
    )
    .unwrap_err();
    assert!(!io.out.stdout.contains("--incremental"));
    let mut io = Io::canned(&[]);
    git_execute_merge(
        &repo,
        &mut io,
        &set,
        &commits,
        Some(&into),
        true,
        true,
        &|_| Ok("Landed".to_string()),
    )
    .unwrap_err();
    assert!(io.out.stdout.contains("--incremental"));
}

#[test]
fn land8_immutable_without_strategy_is_ambiguous() {
    // Legacy `history.immutable` with no explicit strategy fails at merge
    // time; `--strategy` or `arc.land.strategy` bypasses the check.
    let dir = make_repo("land8-immutable");
    std::fs::write(dir.join(".arcconfig"), r#"{"history.immutable": true}"#).unwrap();
    let options = Options::default();
    let mut io = Io::canned(&[]);
    let err = confirm_legacy_strategy_configuration(&dir, &options, &mut io).unwrap_err();
    assert_eq!(
        err,
        "Desired merge strategy is ambiguous, choose an explicit strategy."
    );
    assert!(io.out.stdout.contains("** MERGE STRATEGY IS AMBIGUOUS **"));

    let flagged = Options {
        strategy: Some("squash".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert!(confirm_legacy_strategy_configuration(&dir, &flagged, &mut io).is_ok());

    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"history.immutable": true, "arc.land.strategy": "merge"}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert!(confirm_legacy_strategy_configuration(&dir, &options, &mut io).is_ok());

    std::fs::write(dir.join(".arcconfig"), r#"{"history.immutable": false}"#).unwrap();
    let mut io = Io::canned(&[]);
    assert!(confirm_legacy_strategy_configuration(&dir, &options, &mut io).is_ok());

    assert!(is_truthy_config(&serde_json::json!(true)));
    assert!(!is_truthy_config(&serde_json::json!(false)));
    assert!(is_truthy_config(&serde_json::json!("yes")));
    assert!(!is_truthy_config(&serde_json::json!("")));
    assert!(!is_truthy_config(&serde_json::json!("0")));
    assert!(!is_truthy_config(&serde_json::json!(0)));
    assert!(is_truthy_config(&serde_json::json!(1)));
    assert!(!is_truthy_config(&serde_json::Value::Null));
}

#[test]
fn land11_set_messages_prefetch_in_order() {
    // Messages load for every set up front, in order; a later failure
    // aborts the prefetch (no lazy per-set fetch mid-loop).
    let calls: std::rc::Rc<std::cell::RefCell<Vec<i64>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let probe = calls.clone();
    let call = move |method: &str, params: &serde_json::Value| {
        assert_eq!(method, diff_submit::METHOD_GET_COMMIT_MESSAGE);
        let id = params["revision_id"].as_i64().unwrap();
        probe.borrow_mut().push(id);
        if id == 12 {
            return Err("boom".to_string());
        }
        Ok(serde_json::json!(format!("message {id}")))
    };
    let sets = vec![
        CommitSet {
            revision: Some(view(11, "accepted", "Accepted", false, "PHID-USER-a")),
            commits: vec![0],
            is_pick: false,
        },
        CommitSet {
            revision: None,
            commits: vec![1],
            is_pick: false,
        },
        CommitSet {
            revision: Some(view(12, "accepted", "Accepted", false, "PHID-USER-a")),
            commits: vec![2],
            is_pick: false,
        },
    ];
    let commits = vec![
        land_commit("aaa", &[], "one"),
        land_commit("bbb", &[], "two"),
        land_commit("ccc", &[], "three"),
    ];
    let raw = vec!["feature".to_string()];
    let onto = vec!["master".to_string()];
    let err = prefetch_set_messages(&raw, &onto, &commits, Some(&call), &sets).unwrap_err();
    assert_eq!(err, "boom");
    assert_eq!(*calls.borrow(), vec![11, 12]);

    let ok = canned(vec![(
        diff_submit::METHOD_GET_COMMIT_MESSAGE,
        serde_json::json!("fetched"),
    )]);
    let messages = prefetch_set_messages(&raw, &onto, &commits, Some(&ok), &sets[..2]).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], "fetched");
    assert!(messages[1].contains("Land feature"));
    // Without a call every set synthesizes the local message.
    let messages = prefetch_set_messages(&raw, &onto, &commits, None, &sets[..2]).unwrap();
    assert!(messages[0].contains("Land feature"));
}

#[test]
fn land36_prune_sorts_branches_globally_natural() {
    // One `natcasesort()` over every collected branch: `b9` sorts before
    // `b10` however the sets order them.
    let branches = parse_for_each_ref("refs/heads/b10 h1\nrefs/heads/b9 h2\n").unwrap();
    assert_eq!(
        branches
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["b9", "b10"]
    );

    let repo = make_repo("land36-prune");
    git(&repo, &["checkout", "-b", "b10"]);
    let tip10 = commit_file(&repo, "f10.txt", "x\n", "ten");
    git(&repo, &["checkout", "-q", "master"]);
    git(&repo, &["checkout", "-b", "b9"]);
    let tip9 = commit_file(&repo, "f9.txt", "x\n", "nine");
    git(&repo, &["checkout", "-q", "master"]);
    let commits = vec![
        land_commit(&tip10, &[], "ten"),
        land_commit(&tip9, &[], "nine"),
    ];
    let sets = vec![
        CommitSet {
            revision: None,
            commits: vec![0],
            is_pick: false,
        },
        CommitSet {
            revision: None,
            commits: vec![1],
            is_pick: false,
        },
    ];
    let mut io = Io::canned(&[]);
    let deleted = git_prune_branches(&repo, &mut io, &sets, &commits);
    assert_eq!(deleted, vec!["b9".to_string(), "b10".to_string()]);
}

#[test]
#[cfg(unix)]
fn land9_push_reports_failure_without_capturing() {
    // The push inherits stdio and maps failure to `Push failed! ...`.
    // Unix-only: the stub programs (`true`/`false`/`sh`) have no
    // Windows counterparts.
    let dir = temp_dir("land9-push");
    let mut io = Io::canned(&[]);
    assert!(
        git_push_change_with(
            &dir,
            "true",
            &[],
            &mut io,
            "origin",
            &["master".to_string()],
            "aaa",
        )
        .is_ok()
    );
    let err = git_push_change_with(
        &dir,
        "false",
        &[],
        &mut io,
        "origin",
        &["master".to_string()],
        "aaa",
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Push failed! Fix the error and run \"arc land\" again."
    );
    assert!(io.out.stderr.contains("** PUSHING **"));

    let hg_cmd = hg::HgCommand::custom("true", &[]);
    assert!(hg_passthru_with(&dir, &hg_cmd, &["push"]));
    let hg_cmd = hg::HgCommand::custom("false", &[]);
    assert!(!hg_passthru_with(&dir, &hg_cmd, &["push"]));
    // Prefix and encoding args ride along to custom programs.
    let seen = temp_dir("land9-argv");
    let marker = seen.join("argv");
    let script = format!("echo \"$@\" > {}", marker.display());
    let hg_cmd = hg::HgCommand::custom("sh", &["-c", &script, "ignored"]);
    assert!(hg_passthru_with(&dir, &hg_cmd, &["push"]));
    let argv = std::fs::read_to_string(&marker).unwrap();
    assert!(argv.contains("--encoding utf-8 push"), "{argv}");
}

#[test]
fn land34_externals_block_uses_php_texts() {
    // The dirty-externals block and prompt match `saveLocalState()`
    // verbatim; declining aborts.
    let mut io = Io::canned(&["y"]);
    confirm_dirty_externals(&mut io, &["sub/mod".to_string(), "lib/ext".to_string()]).unwrap();
    assert!(
        io.out.stdout.contains(
            "2 submodule(s) have uncommitted or untracked changes:\n\n      sub/mod\n      lib/ext\n"
        ),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out
            .stdout
            .contains("Ignore the changes to these 2 submodule(s) and continue? [Y/n]"),
        "{}",
        io.out.stdout
    );
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        confirm_dirty_externals(&mut io, &["sub/mod".to_string()]).unwrap_err(),
        ABORT
    );
}

#[test]
fn land34_ignore_hints_need_untracked_files() {
    // Ignore hints print only when untracked files exist.
    let dir = temp_dir("land34-hints");
    let tracked_only = ChangeLists {
        conflicts: Vec::new(),
        externals: Vec::new(),
        untracked: Vec::new(),
        unstaged: vec!["edit.txt".to_string()],
        uncommitted: Vec::new(),
    };
    let mut io = Io::canned(&[]);
    assert!(push_uncommitted_banner(
        &mut io,
        &dir,
        &tracked_only,
        "HINT"
    ));
    assert!(!io.out.stdout.contains("HINT"));
    let with_untracked = ChangeLists {
        untracked: vec!["new.txt".to_string()],
        ..tracked_only
    };
    let mut io = Io::canned(&[]);
    assert!(push_uncommitted_banner(
        &mut io,
        &dir,
        &with_untracked,
        "HINT"
    ));
    assert!(io.out.stdout.contains("HINT"));
}

#[test]
fn land35_hg_without_shelve_matches_php_error() {
    // Without shelve support, hg fails with the verbatim PHP usage error
    // (no prompt is shown first).
    let dir = temp_dir("land35-hg");
    let script = dir.join("fakehg.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nfor a in \"$@\"; do\n  case \"$a\" in\n    status) echo '? new.txt'; exit 0;;\n  esac\ndone\nexit 1\n",
    )
    .unwrap();
    let hg_cmd = hg::HgCommand::custom("sh", &[script.to_str().unwrap()]);
    let mut io = Io::canned(&["y"]);
    let err = save_local_state_hg_with(&dir, &hg_cmd, &mut io).unwrap_err();
    assert_eq!(
        err,
        "You can not continue with uncommitted changes. Commit or discard them before proceeding."
    );
}

#[test]
fn land14_saved_prompts_read_repo_root_config() {
    // Saved prompts load from the repo-root `.git/arc/config` even when
    // land runs in a subdirectory.
    let repo = make_repo("land14-prompts");
    let sub = repo.join("sub").join("dir");
    std::fs::create_dir_all(&sub).unwrap();
    let local = repo.join(".git").join("arc").join("config");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(
        &local,
        r#"{"prompts": [{"prompt": "k", "response": "yes"}]}"#,
    )
    .unwrap();
    let saved = load_saved_prompts(&sub);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved["k"].response, "yes");
}

#[test]
fn land39_onto_config_validates_list_shape() {
    // Natural-list objects parse in key order; anything else fails.
    assert_eq!(
        parse_onto_config(&serde_json::json!({"1": "b", "0": "a"})).unwrap(),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!({"0": "a", "2": "c"})).unwrap_err(),
        "Expected a natural list!"
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!([true])).unwrap_err(),
        "Expected a string (at index \"0\"), found \"boolean\"."
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!([1.5])).unwrap_err(),
        "Expected a string (at index \"0\"), found \"double\"."
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!([null])).unwrap_err(),
        "Expected a string (at index \"0\"), found \"NULL\"."
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!([])).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn land40_unresolved_revision_echoes_raw_symbol() {
    // `--revision D123` failures echo `D123`, not the digits.
    let call = canned(vec![(
        diff_submit::METHOD_REVISION_SEARCH,
        serde_json::json!({"data": []}),
    )]);
    let err =
        resolve_forced_revision_with_raw(&call, &RevisionSymbol::Id(123), "D123").unwrap_err();
    assert_eq!(err, "Symbol \"D123\" does not identify a valid revision.");
    let err = resolve_forced_revision(&call, &RevisionSymbol::Id(123)).unwrap_err();
    assert_eq!(err, "Symbol \"123\" does not identify a valid revision.");
}

#[test]
fn land15_invalid_symbol_fails_before_onto_output() {
    // T1153 B21/B23: piped runs carry `--`.
    // Resolution runs before onto/into selection: an invalid symbol
    // fails with no target output at all.
    let (repo, _bare) = repo_with_origin("land15-order");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "f.txt", "f\n", "feature work");
    let mut io = Io::canned(&[]);
    let err = execute_with_io(&repo, &args(&["--", "nope"]), &mut io).unwrap_err();
    assert!(err.contains("nope"), "{err}");
    assert!(!io.out.stderr.contains("ONTO"), "{}", io.out.stderr);
    assert!(!io.out.stderr.contains("INTO"), "{}", io.out.stderr);
}
#[test]
fn r10_land_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "LAND WORKFLOW\n\
         \n\
         land [options] -- [ref ...]\n\
         Publish reviewed changes.\n\
         \n\
         Supports: git, git/p4, git/svn, hg\n\
         \n\
         Publish accepted revisions after review. This command is the last step in the\n\
         standard Differential code review workflow.\n\
         \n\
         To publish changes in local branch or bookmark \"feature1\", you will usually\n\
         run this command:\n\
         \n\
         \x20 $ arc land feature1\n\
         \n\
         This workflow merges and pushes changes associated with revisions that are\n\
         ancestors of ref. Without ref, the current working copy state will be used.\n\
         You can specify multiple ref arguments to publish multiple changes at once.\n\
         \n\
         A ref can be any symbol which identifies a commit: a branch name, a tag name,\n\
         a bookmark name, a topic name, a raw commit hash, a symbolic reference, etc.\n\
         \n\
         When you provide a ref, all unpublished changes which are present in\n\
         ancestors of that ref will be selected for publishing. (With the --pick flag,\n\
         only the unpublished changes you directly reference will be selected.)\n\
         \n\
         For example, if you provide local branch \"feature3\" as a ref argument, that\n\
         may also select the changes in \"feature1\" and \"feature2\" (if they are\n\
         ancestors of \"feature3\"). If you stack changes in a single local branch, all\n\
         commits in the stack may be selected.\n\
         \n\
         The workflow merges unpublished changes reachable from ref \"into\" some\n\
         intermediate branch, then pushes the combined state \"onto\" some destination\n\
         branch (or list of branches).\n\
         \n\
         (In Mercurial, the \"into\" and \"onto\" branches may be bookmarks instead.)\n\
         \n\
         In the most common case, there is only one \"onto\" branch (often \"master\" or\n\
         \"default\" or some similar branch) and the \"into\" branch is the same branch.\n\
         For example, it is common to merge local feature branch \"feature1\" into\n\
         \"origin/master\", then push it onto \"origin/master\".\n\
         \n\
         The list of \"onto\" branches is selected by examining these sources in order:\n\
         \n\
         \x20 - the --onto flags;\n\
         \x20 - the arc.land.onto configuration setting;\n\
         \x20 - (in Git) the upstream of the branch targeted by the land operation,\n\
         \x20   recursively;\n\
         \x20 - or by falling back to a standard default:\n\
         \x20   - (in Git) \"master\";\n\
         \x20   - (in Mercurial) \"default\".\n\
         \n\
         The remote to push \"onto\" is selected by examining these sources in order:\n\
         \n\
         \x20 - the --onto-remote flag;\n\
         \x20 - the arc.land.onto-remote configuration setting;\n\
         \x20 - (in Git) the upstream of the current branch, recursively;\n\
         \x20 - (in Git) the special \"p4\" remote which indicates a repository has\n\
         \x20   been synchronized with Perforce;\n\
         \x20 - or by falling back to a standard default:\n\
         \x20   - (in Git) \"origin\";\n\
         \x20   - (in Mercurial) \"default\".\n\
         \n\
         The branch to merge \"into\" is selected by examining these sources in order:\n\
         \n\
         \x20 - the --into flag;\n\
         \x20 - the --into-empty flag;\n\
         \x20 - or by falling back to the first \"onto\" branch.\n\
         \n\
         The remote to merge \"into\" is selected by examining these sources in order:\n\
         \n\
         \x20 - the --into-remote flag;\n\
         \x20 - the --into-local flag (which disables fetching before merging);\n\
         \x20 - or by falling back to the \"onto\" remote.\n\
         \n\
         After selecting remotes and branches, the commits which will land are\n\
         printed.\n\
         \n\
         With --preview, execution stops here, before the change is merged.\n\
         \n\
         The \"into\" branch is fetched from the \"into\" remote (unless --into-local or\n\
         --into-empty are specified) and the changes are merged into the state in the\n\
         \"into\" branch according to the selected merge strategy.\n\
         \n\
         The default merge strategy is \"squash\", which produces a single commit from\n\
         all local commits for each change. A different strategy can be selected with\n\
         the --strategy flag.\n\
         \n\
         The resulting merged change will be given an up-to-date commit message\n\
         describing the final state of the revision in Differential.\n\
         \n\
         With --hold, execution stops here, before the change is pushed.\n\
         \n\
         The change is pushed onto all of the \"onto\" branches in the \"onto\" remote.\n\
         \n\
         If you are landing multiple changes, they are normally all merged locally and\n\
         then all pushed in a single operation. Instead, you can merge and push them\n\
         one at a time with --incremental.\n\
         \n\
         Under merge strategies which mutate history (including the default \"squash\"\n\
         strategy), local refs which descend from commits that were published are now\n\
         updated. For example, if you land \"feature4\", local branches \"feature5\" and\n\
         \"feature6\" may now be rebased on the published version of the change.\n\
         \n\
         Once everything has been pushed, cleanup occurs. Consulting mystical sources\n\
         of power, the workflow makes a guess about what state you wanted to end up in\n\
         after the process finishes. The working copy is put into that state.\n\
         \n\
         Any obsolete refs that point at commits which were published are deleted,\n\
         unless the --keep-branches flag is passed.\n\
         \n\
         \x20     --hold\n\
         \x20         Prepare the changes to be pushed, but do not actually push them.\n\
         \n\
         \x20     --incremental\n\
         \x20         When landing multiple revisions at once, push and rebase after each\n\
         \x20         merge completes instead of waiting until all merges are completed\n\
         \x20         to push.\n\
         \x20         \n\
         \x20         This is slower than the default behavior and not atomic, but may\n\
         \x20         make it easier to resolve conflicts and land complicated changes by\n\
         \x20         allowing you to make progress one step at a time.\n\
         \n\
         \x20     --into commit-ref\n\
         \x20         Specify the state to merge into. By default, this is the same as\n\
         \x20         the \"onto\" ref.\n\
         \n\
         \x20     --into-empty\n\
         \x20         Merge into the empty state instead of an existing state. This mode\n\
         \x20         is primarily useful when creating a new repository, and selected\n\
         \x20         automatically if the \"onto\" ref does not exist and the \"into\" state\n\
         \x20         is not specified.\n\
         \n\
         \x20     --into-local\n\
         \x20         Use the local \"into\" ref state instead of fetching it from a remote.\n\
         \n\
         \x20     --into-remote remote-name\n\
         \x20         Specifies the remote to fetch the \"into\" ref from. By default, this\n\
         \x20         is the same as the \"onto\" remote.\n\
         \n\
         \x20     --keep-branches\n\
         \x20         Keep local branches around after changes are pushed. By default,\n\
         \x20         local branches are deleted after the changes they contain are\n\
         \x20         published.\n\
         \n\
         \x20     --onto branch-name\n\
         \x20         After merging, push changes onto a specified branch.\n\
         \x20         \n\
         \x20         Specifying this flag multiple times will push to multiple branches.\n\
         \x20         \n\
         \x20         Related configuration: arc.land.onto\n\
         \n\
         \x20     --onto-remote remote-name\n\
         \x20         Push to a remote other than the default.\n\
         \x20         \n\
         \x20         Related configuration: arc.land.onto-remote\n\
         \n\
         \x20     --pick\n\
         \x20         Land only the changes directly named by arguments, instead of all\n\
         \x20         reachable ancestors.\n\
         \n\
         \x20     --preview\n\
         \x20         Show the changes that will land. Does not modify the working copy\n\
         \x20         or the remote.\n\
         \n\
         \x20     --revision revision-identifier\n\
         \x20         Land a specific revision, rather than determining revisions\n\
         \x20         automatically from the commits that are landing.\n\
         \n\
         \x20     --strategy strategy-name\n\
         \x20         Merge using a particular strategy. Supported strategies are\n\
         \x20         \"squash\" and \"merge\".\n\
         \x20         \n\
         \x20         The \"squash\" strategy collapses multiple local commits into a\n\
         \x20         single commit when publishing. It produces a linear published\n\
         \x20         history (but discards local checkpoint commits). This is the\n\
         \x20         default strategy.\n\
         \x20         \n\
         \x20         The \"merge\" strategy generates a merge commit when publishing that\n\
         \x20         retains local checkpoint commits (but produces a nonlinear\n\
         \x20         published history). Select this strategy if you do not want \"arc\n\
         \x20         land\" to discard checkpoint commits.\n\
         \x20         \n\
         \x20         Related configuration: arc.land.strategy\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `execute_with_io` pushes the same text with the IO ANSI mode.
    assert!(usage_text(true).contains("LAND WORKFLOW"));
}

#[test]
fn b21_land_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv. Unknown flags still report first.
    assert_eq!(
        parse_options_with_tty(&args(&[]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--help"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    assert_eq!(
        parse_options_with_tty(&args(&["--preview"]), false),
        Err(crate::help::MISSING_TERMINATOR.to_string())
    );
    let err = parse_options_with_tty(&args(&["--nope"]), false).unwrap_err();
    assert_eq!(err, "Unknown argument 'nope'. Try 'arc help'.");
    assert!(
        parse_options_with_tty(&args(&["--"]), false)
            .unwrap()
            .is_some()
    );
    assert!(
        parse_options_with_tty(&args(&["--preview", "--", "feature"]), false)
            .unwrap()
            .is_some()
    );
    assert!(parse_options_with_tty(&args(&[]), true).unwrap().is_some());
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

/// Write a fake-`hg` shell script and return an `HgCommand` invoking it
/// via `sh`. The script sees the full argv (`--encoding utf-8` first, plus
/// `--config ...` for extension calls); `body` dispatches with the `has
/// WORD` probe over the space-joined argv. Unix-only: a Windows fake
/// would need a `.bat` instead.
#[cfg(unix)]
fn fake_hg(dir: &Path, name: &str, body: &str) -> hg::HgCommand {
    let script = dir.join(name);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nARGS=\"$*\"\nhas() {{ case \" $ARGS \" in *\" $1 \"*) return 0;; *) return 1;; esac; }}\n{body}"
        ),
    )
    .unwrap();
    hg::HgCommand::custom("sh", &[script.to_str().unwrap()])
}

/// Extract the `-r`/`--rev` value from the fake-`hg` argv into `$rev`.
#[cfg(unix)]
const FAKE_HG_REV_LOOP: &str = r#"prev=""; rev=""; for a in "$@"; do case "$prev" in -r|--rev) rev="$a";; esac; prev="$a"; done"#;

#[test]
fn b23_land_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help land`.
    let text = usage_text(false);
    assert!(
        text.starts_with("LAND WORKFLOW\n"),
        "{}",
        &text[..200.min(text.len())]
    );
    assert!(text.contains("Publish reviewed changes."), "{text}");
    assert!(text.contains("--strategy"), "{text}");
    assert!(text.contains("--onto"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// T1170 coverage: prompt scopes, glyph/width helpers, and the
// Mercurial select-state seams.

#[test]
fn t1170_prompt_save_reports_working_copy_scope_without_writer() {
    // The `!` suffix targets the working-copy scope, whose missing
    // writer reports its own scope name (like the `*`/user case).
    let mut io = Io::canned(&["y!"]);
    assert_eq!(
        prompt_for_key(&mut io, "k", "q?").unwrap_err(),
        "Unable to save response to config: no writable configuration source for scope \
         (\"working-copy\")."
    );
}

#[test]
fn t1170_split_glyphs_handles_empty_and_leading_mark() {
    assert!(split_glyphs("").is_empty());
    // A leading combining mark attaches to a synthetic space.
    assert_eq!(split_glyphs("\u{301}ab"), vec![" \u{301}", "a", "b"]);
}

#[test]
fn t1170_truncate_glyphs_keeps_short_strings() {
    assert_eq!(truncate_glyphs("abc", 5), "abc");
    assert_eq!(truncate_glyphs("", 0), "");
}

#[test]
fn t1170_console_strlen_counts_nul_and_broken_escapes() {
    // NUL contributes no columns.
    assert_eq!(console_strlen("\0"), 0);
    // ESC not followed by `[` is literal text.
    assert_eq!(console_strlen("a\x1bXb"), 4);
    // ESC `[` without a closing `m` is literal text too.
    assert_eq!(console_strlen("\x1b[1X"), 4);
    // A complete color escape still strips.
    assert_eq!(console_strlen("\x1b[1mab"), 2);
}

#[test]
fn t1170_console_strlen_counts_every_wide_range() {
    // One double-column representative per `is_wide_codepoint` range.
    let chars = [
        '\u{AC00}',                       // Hangul syllables
        '\u{F900}',                       // CJK compatibility ideographs
        '\u{FE10}',                       // vertical forms
        '\u{FE30}',                       // CJK compatibility forms
        '\u{FF21}',                       // fullwidth ASCII
        '\u{FFE1}',                       // fullwidth symbols
        char::from_u32(0x20000).unwrap(), // CJK ext B plane
        char::from_u32(0x30000).unwrap(), // CJK ext G plane
    ];
    for ch in chars {
        assert_eq!(console_strlen(&ch.to_string()), 2, "{ch:?}");
    }
}

#[test]
fn t1170_ref_view_renders_name_only_when_title_empty() {
    let view = RefView {
        object_name: Some("D1".to_string()),
        title: String::new(),
        ..Default::default()
    };
    assert_eq!(view.render_with_ansi(false), "  *   D1\n");
}

#[test]
fn t1170_save_prompt_rejects_invalid_json() {
    // Saving over an unparseable config reports the file, like PHP's
    // `phutil_json_decode()`.
    let dir = temp_dir("t1170-badjson");
    let user = dir.join("arcrc");
    std::fs::write(&user, "{not json").unwrap();
    let mut io = Io::canned(&["y*"]);
    io.config_writer = Some(ConfigWriter {
        user_path: Some(user.clone()),
        local_path: None,
    });
    assert_eq!(
        prompt_for_key(&mut io, "k", "q?").unwrap_err(),
        format!(
            "Unable to read configuration file \"{}\": invalid JSON.",
            user.display()
        )
    );
}

#[test]
fn t1170_save_prompt_keeps_malformed_entries() {
    // Entries without a `prompt`/`response` pair have no key, so saving
    // keeps them alongside the new answer.
    let dir = temp_dir("t1170-malformed");
    let user = dir.join("arcrc");
    std::fs::write(&user, r#"{"prompts": [{"prompt": "k"}]}"#).unwrap();
    let mut io = Io::canned(&["y*"]);
    io.config_writer = Some(ConfigWriter {
        user_path: Some(user.clone()),
        local_path: None,
    });
    assert!(prompt_for_key(&mut io, "k2", "q?").is_ok());
    let saved = load_saved_prompts_from(&[(user.clone(), PromptScope::User)]);
    assert_eq!(saved.get("k2").map(|s| s.response.as_str()), Some("y"));
    let stored = std::fs::read_to_string(&user).unwrap();
    assert!(stored.contains("\"prompt\": \"k\""), "{stored}");
    assert!(stored.contains("\"prompt\": \"k2\""), "{stored}");
    assert!(stored.contains("\"response\": \"y\""), "{stored}");
}

#[test]
fn t1170_onto_config_reports_array_items() {
    assert_eq!(
        parse_onto_config(&serde_json::json!([[1]])).unwrap_err(),
        "Expected a string (at index \"0\"), found \"array\"."
    );
    assert_eq!(
        parse_onto_config(&serde_json::json!([{"a": 1}])).unwrap_err(),
        "Expected a string (at index \"0\"), found \"array\"."
    );
}

#[test]
fn t1170_format_skip_note_mentions_reason() {
    let note = format_skip_note("conduit unreachable");
    assert!(note.contains("conduit unreachable"), "{note}");
    assert_eq!(
        note,
        format_skip_note_with_ansi("conduit unreachable", false)
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_default_symbols_prefers_active_bookmark() {
    let dir = temp_dir("t1170-hg-bookmark");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has bookmarks; then printf '   old  0:aaa\n * feat  1:bbb\\n'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_default_symbols_with(&dir, &hg_cmd, &mut io).unwrap(),
        vec!["feat".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("Landing the active bookmark, \"feat\"."),
        "{}",
        io.out.stderr
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_default_symbols_falls_back_to_branch_then_commit() {
    let dir = temp_dir("t1170-hg-branch");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has bookmarks; then exit 0; fi\nif has branch; then printf 'stable\\n'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_default_symbols_with(&dir, &hg_cmd, &mut io).unwrap(),
        vec!["stable".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("Landing the active branch, \"stable\"."),
        "{}",
        io.out.stderr
    );
    // Without a branch either, the active commit lands under its short hash.
    let hg_cmd = fake_hg(
        &dir,
        "fakehg2.sh",
        "if has bookmarks; then exit 0; fi\nif has branch; then exit 1; fi\nif has log; then printf 'abc123def4567890'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_default_symbols_with(&dir, &hg_cmd, &mut io).unwrap(),
        vec!["abc123def4567890".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("Landing the active commit, \"abc123def456\"."),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_hg_default_symbols_reports_missing_binary() {
    // Outside a repository the system `hg` fails every probe, with or
    // without the binary installed.
    let dir = temp_dir("t1170-hg-nosys");
    let mut io = Io::canned(&[]);
    assert!(hg_default_symbols(&dir, &mut io).is_err());
}

#[test]
#[cfg(unix)]
fn t1170_hg_resolve_symbols_prefers_markers_rejects_unknown() {
    let dir = temp_dir("t1170-hg-resolve");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has bookmarks; then printf '   feat  1:abc123\\n'; exit 0; fi\n\
             if has branches; then printf 'default  5:def456\\n'; exit 0; fi\n\
             if has log; then\n  {rev}\n  if [ \"$rev\" = \"feat\" ]; then printf 'abc123'; exit 0; fi\n  \
             if [ \"$rev\" = \"default\" ]; then printf 'def456'; exit 0; fi\n  exit 1\nfi\nexit 1\n",
            rev = FAKE_HG_REV_LOOP,
        ),
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols_with(&dir, &hg_cmd, &mut io, &["feat".to_string()]).unwrap(),
        vec!["abc123".to_string()]
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols_with(&dir, &hg_cmd, &mut io, &["default".to_string()]).unwrap(),
        vec!["def456".to_string()]
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols_with(&dir, &hg_cmd, &mut io, &["nope".to_string()]).unwrap_err(),
        "Symbol \"nope\" does not identify a bookmark, branch, or commit."
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_resolve_symbols_rejects_ambiguous_markers() {
    let dir = temp_dir("t1170-hg-ambiguous");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has bookmarks; then printf '   dup  1:aaa\\n   dup  2:bbb\\n'; exit 0; fi\n\
         if has branches; then printf 'solo  1:ccc\\n'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols_with(&dir, &hg_cmd, &mut io, &["dup".to_string()]).unwrap_err(),
        "Symbol \"dup\" is ambiguous."
    );
    assert!(
        io.out.stdout.contains("** AMBIGUOUS SYMBOL **"),
        "{}",
        io.out.stdout
    );
    assert_eq!(
        io.out.stdout.matches("Bookmark \"dup\"").count(),
        2,
        "{}",
        io.out.stdout
    );
    let hg_cmd = fake_hg(
        &dir,
        "fakehg2.sh",
        "if has bookmarks; then exit 0; fi\n\
         if has branches; then printf 'dup  1:aaa\\ndup  2:bbb\\n'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols_with(&dir, &hg_cmd, &mut io, &["dup".to_string()]).unwrap_err(),
        "Symbol \"dup\" is ambiguous."
    );
    assert_eq!(
        io.out.stdout.matches("Branch \"dup\"").count(),
        2,
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_hg_resolve_symbols_reports_unknown_without_repo() {
    let dir = temp_dir("t1170-hg-noresolve");
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols(&dir, &mut io, &["x".to_string()]).unwrap_err(),
        "Symbol \"x\" does not identify a bookmark, branch, or commit."
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_select_onto_remote_reads_config() {
    let dir = temp_dir("t1170-hg-remote");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"arc.land.onto-remote": "upstream"}"#,
    )
    .unwrap();
    let hg_cmd = fake_hg(&dir, "fakehg.sh", "if has paths; then exit 0; fi\nexit 1\n");
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_onto_remote_with(&dir, &hg_cmd, &mut io, &Options::default()).unwrap(),
        "upstream"
    );
    assert!(
        io.out.stderr.contains(
            "Remote \"upstream\" was selected by reading \"arc.land.onto-remote\" configuration."
        ),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_hg_select_onto_refs_flag_and_config() {
    let options = Options {
        onto: vec!["stable".to_string(), "next".to_string()],
        ..Options::default()
    };
    let dir = temp_dir("t1170-hg-onto");
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_onto_refs(&dir, &mut io, &options).unwrap(),
        vec!["stable".to_string(), "next".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("Refs were selected with the \"--onto\" flag"),
        "{}",
        io.out.stderr
    );
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(
        dir.join(".arcconfig"),
        r#"{"arc.land.onto": ["from-config"]}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_onto_refs(&dir, &mut io, &Options::default()).unwrap(),
        vec!["from-config".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("Refs were selected by reading \"arc.land.onto\" configuration"),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_hg_confirm_rejects_empty_ref() {
    let dir = temp_dir("t1170-hg-empty");
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_confirm_onto_refs(&dir, &mut io, "default", &[String::new()], false).unwrap_err(),
        "Selected \"onto\" ref \"\" is invalid: the empty string is not a valid ref."
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_confirm_rejects_multiple_branches() {
    let dir = temp_dir("t1170-hg-multi");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has branches; then printf 'b1  1:aaa\\nb2  2:bbb\\n'; exit 0; fi\n\
             if has log; then\n  if has \"{{desc}}\"; then printf 'do the thing\\nsecond line\\n'; exit 0; fi\n  \
             {rev}\n  printf 'aaaabbbbccccddddeeeeffff'; exit 0\nfi\nexit 1\n",
            rev = FAKE_HG_REV_LOOP,
        ),
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_confirm_onto_refs_with(
            &dir,
            &hg_cmd,
            &mut io,
            "default",
            &["b1".to_string(), "b2".to_string()],
            false
        )
        .unwrap_err(),
        "Landing onto multiple branches at once is not supported in Mercurial."
    );
    assert!(
        io.out.stdout.contains("** MULTIPLE \"ONTO\" BRANCHES **"),
        "{}",
        io.out.stdout
    );
    // Each branch renders with its 12-char tip hash and first summary line.
    assert!(io.out.stdout.contains("Branch \"b1\""), "{}", io.out.stdout);
    assert!(io.out.stdout.contains("aaaabbbbcccc"), "{}", io.out.stdout);
    assert!(io.out.stdout.contains("do the thing"), "{}", io.out.stdout);
    assert!(!io.out.stdout.contains("second line"), "{}", io.out.stdout);
}

#[test]
#[cfg(unix)]
fn t1170_hg_confirm_hold_note_before_create_prompt() {
    let dir = temp_dir("t1170-hg-hold");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has branches; then printf 'default  1:aaa\\n'; exit 0; fi\n\
         if has identify; then exit 1; fi\nexit 0\n",
    );
    let mut io = Io::canned(&["y"]);
    assert_eq!(
        hg_confirm_onto_refs_with(
            &dir,
            &hg_cmd,
            &mut io,
            "default",
            &["fresh".to_string()],
            true
        )
        .unwrap(),
        None
    );
    assert!(
        io.out.stdout.contains("** CREATE 1 BOOKMARK(S) **"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("You are using \"--hold\""),
        "{}",
        io.out.stdout
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_into_commit_resolves_local_and_remote() {
    let dir = temp_dir("t1170-hg-into");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has pull; then exit 0; fi\n\
             if has log; then\n  {rev}\n  if [ \"$rev\" = \"gone\" ]; then exit 1; fi\n  \
             printf 'abc123def4567890'; exit 0\nfi\nexit 1\n",
            rev = FAKE_HG_REV_LOOP,
        ),
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_into_commit_with(
            &dir,
            &hg_cmd,
            &mut io,
            &IntoState::Local("stable".to_string()),
            None
        )
        .unwrap(),
        Some("abc123def4567890".to_string())
    );
    assert!(
        io.out
            .stderr
            .contains("Preparing merge into local target \"stable\""),
        "{}",
        io.out.stderr
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_into_commit_with(
            &dir,
            &hg_cmd,
            &mut io,
            &IntoState::Remote {
                remote: "default".to_string(),
                name: "stable".to_string()
            },
            None
        )
        .unwrap(),
        Some("abc123def4567890".to_string())
    );
    assert!(
        io.out
            .stderr
            .contains("Preparing merge into \"stable\" from remote \"default\""),
        "{}",
        io.out.stderr
    );
    // A missing remote ref without `--into` implies the empty state.
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_into_commit_with(
            &dir,
            &hg_cmd,
            &mut io,
            &IntoState::Remote {
                remote: "default".to_string(),
                name: "gone".to_string()
            },
            None
        )
        .unwrap(),
        None
    );
    assert!(
        io.out
            .stderr
            .contains("Preparing merge into the empty state to create target \"gone\""),
        "{}",
        io.out.stderr
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_select_commits_reads_revsets() {
    let dir = temp_dir("t1170-hg-commits");
    let marker = dir.join("revsets");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has log; then\n  {rev}\n  printf '%s\\n' \"$rev\" >> \"{marker}\"\n  \
             printf 'aaa--root summary\\nbbb-aaa -child summary\\n'; exit 0\nfi\nexit 1\n",
            rev = FAKE_HG_REV_LOOP,
            marker = marker.display(),
        ),
    );
    let symbols = vec![GitSymbol {
        symbol: "feat".to_string(),
        commit: "abc".to_string(),
    }];
    let commits = hg_select_commits_with(&dir, &hg_cmd, &symbols, None).unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].hash, "aaa");
    assert!(commits[0].parents.is_empty());
    assert_eq!(commits[0].summary, "root summary");
    assert_eq!(commits[0].direct_symbols, vec!["feat".to_string()]);
    assert_eq!(commits[1].hash, "bbb");
    assert_eq!(commits[1].parents, vec!["aaa".to_string()]);
    assert!(commits[1].direct_symbols.is_empty());
    assert_eq!(commits[1].indirect_symbols, vec!["feat".to_string()]);
    hg_select_commits_with(&dir, &hg_cmd, &symbols, Some("zzz")).unwrap();
    let revsets = std::fs::read_to_string(&marker).unwrap();
    assert!(revsets.contains("reverse(ancestors(abc))"), "{revsets}");
    assert!(
        revsets.contains("reverse(ancestors(abc) - ancestors(zzz))"),
        "{revsets}"
    );
}

#[test]
fn t1170_hg_select_commits_reports_log_failure() {
    let dir = temp_dir("t1170-hg-nolog");
    let symbols = vec![GitSymbol {
        symbol: "x".to_string(),
        commit: "abc".to_string(),
    }];
    assert!(hg_select_commits(&dir, &symbols, None).is_err());
}

#[test]
fn t1170_hg_hold_text_without_bookmarks_pushes_rev() {
    let mut io = Io::canned(&[]);
    hg_hold_text(&mut io, &[], "default", "abc123", &[]);
    assert!(
        io.out.stdout.contains("** HOLD CHANGES **"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("hg push --rev abc123 -- default"),
        "{}",
        io.out.stdout
    );
}

// T1170 coverage: Mercurial merge/push/cascade/prune/reconcile and the
// save/restore local-state seams.

#[test]
#[cfg(unix)]
fn t1170_hg_execute_merge_rebases_and_restores_bookmarks() {
    let dir = temp_dir("t1170-hg-merge");
    let calls = dir.join("calls");
    let message_file = dir.join("message");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has rebase; then\n  if has --abort; then printf 'abort\\n' >> \"{calls}\"; exit 0; fi\n  \
             cat > \"{message}\"; printf 'rebase\\n' >> \"{calls}\"; exit 0\nfi\n\
             if has bookmarks; then printf '   keep  1:aaa\\n'; exit 0; fi\n\
             if has bookmark; then printf 'bookmark %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has update; then printf 'update\\n' >> \"{calls}\"; exit 0; fi\n\
             if has branch; then printf 'branch\\n' >> \"{calls}\"; exit 0; fi\n\
             if has log; then\n  {rev}\n  if [ \"$rev\" = \"tip\" ]; then printf 'cccnewtip'; exit 0; fi\n  \
             printf 'aaa'; exit 0\nfi\nexit 1\n",
            calls = calls.display(),
            message = message_file.display(),
            rev = FAKE_HG_REV_LOOP,
        ),
    );
    let commits = vec![
        land_commit("aaa", &[], "first"),
        land_commit("bbb", &["aaa"], "second"),
    ];
    let set = CommitSet {
        revision: None,
        commits: vec![0, 1],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let tip = hg_execute_merge_with(
        &dir,
        &hg_cmd,
        &mut io,
        &set,
        &commits,
        Some("iii"),
        true,
        Some("stable"),
        &|_| Ok("landed message".to_string()),
    )
    .unwrap();
    assert_eq!(tip, "cccnewtip");
    // The landed message reached the rebase over stdin.
    assert_eq!(
        std::fs::read_to_string(&message_file).unwrap(),
        "landed message"
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("update"), "{calls}");
    assert!(calls.contains("branch"), "{calls}");
    assert!(calls.contains("rebase"), "{calls}");
    // The bookmark sitting on the obsolete range was moved back.
    assert!(calls.contains("--force --rev aaa -- keep"), "{calls}");
}

#[test]
#[cfg(unix)]
fn t1170_hg_execute_merge_rebase_failure_aborts() {
    let dir = temp_dir("t1170-hg-mergefail");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has rebase; then\n  if has --abort; then printf 'abort\\n' >> \"{calls}\"; exit 0; fi\n  \
             exit 1\nfi\n\
             if has bookmarks; then exit 0; fi\n\
             if has update; then exit 0; fi\n\
             if has log; then printf 'tip'; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let commits = vec![land_commit("aaa", &[], "first")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_execute_merge_with(
            &dir,
            &hg_cmd,
            &mut io,
            &set,
            &commits,
            Some("iii"),
            true,
            None,
            &|_| Ok("msg".to_string()),
        )
        .unwrap_err(),
        "rebase failed"
    );
    assert!(std::fs::read_to_string(&calls).unwrap().contains("abort"));
}

#[test]
#[cfg(unix)]
fn t1170_hg_write_stdin_reports_failures() {
    // An unspawnable program and a failing one both report false; `cat`
    // consumes stdin and succeeds.
    let dir = temp_dir("t1170-hg-stdin");
    let missing = hg::HgCommand::custom("/nonexistent-hg-xyz", &[]);
    assert!(!hg_write_stdin_with(&dir, &missing, &["rebase"], "msg"));
    let failing = hg::HgCommand::custom("false", &[]);
    assert!(!hg_write_stdin_with(&dir, &failing, &["rebase"], "msg"));
    let sink = hg::HgCommand::custom("sh", &["-c", "cat > /dev/null", "ignored"]);
    assert!(hg_write_stdin_with(&dir, &sink, &["rebase"], "msg"));
}

#[test]
#[cfg(unix)]
fn t1170_hg_push_change_moves_bookmarks_and_pulls_back() {
    let dir = temp_dir("t1170-hg-push");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has push; then exit 0; fi\n\
             if has pull; then printf 'pull %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has bookmark; then printf 'bookmark %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has log; then printf 'oldhash'; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let mut io = Io::canned(&[]);
    hg_push_change_with(
        &dir,
        &hg_cmd,
        &mut io,
        "default",
        &["b1".to_string()],
        "newhash",
    )
    .unwrap();
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("--force --rev newhash -- b1"), "{calls}");
    assert!(
        calls.contains("pull") && calls.contains("--bookmark b1"),
        "{calls}"
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_push_change_restores_bookmarks_on_failure() {
    let dir = temp_dir("t1170-hg-pushfail");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has push; then exit 1; fi\n\
             if has bookmark; then printf 'bookmark %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has log; then printf 'oldhash'; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_push_change_with(
            &dir,
            &hg_cmd,
            &mut io,
            "default",
            &["b1".to_string()],
            "newhash"
        )
        .unwrap_err(),
        "Push failed! Fix the error and run \"arc land\" again."
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("--force --rev newhash -- b1"), "{calls}");
    assert!(calls.contains("--force --rev oldhash"), "{calls}");
}

#[test]
#[cfg(unix)]
fn t1170_hg_push_change_skips_bookmark_already_at_cursor() {
    let dir = temp_dir("t1170-hg-pushskip");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has push; then exit 0; fi\n\
             if has pull; then printf 'pull\\n' >> \"{calls}\"; exit 0; fi\n\
             if has bookmark; then printf 'bookmark\\n' >> \"{calls}\"; exit 0; fi\n\
             if has log; then printf 'newhash'; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let mut io = Io::canned(&[]);
    hg_push_change_with(
        &dir,
        &hg_cmd,
        &mut io,
        "default",
        &["b1".to_string()],
        "newhash",
    )
    .unwrap();
    // No bookmark move and no pull-back: nothing was displaced.
    assert!(
        !calls.exists(),
        "unexpected calls: {:?}",
        std::fs::read_to_string(&calls)
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_cascade_state_rebase_failure_aborts() {
    let dir = temp_dir("t1170-hg-cascade");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has rebase; then\n  if has --abort; then printf 'abort\\n' >> \"{calls}\"; exit 0; fi\n  \
             printf 'rebase %s\\n' \"$*\" >> \"{calls}\"; exit 1\nfi\n\
             if has log; then printf 'child1\\n'; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let commits = vec![land_commit("old", &[], "landed")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    assert_eq!(
        hg_cascade_state_with(&dir, &hg_cmd, &set, &commits, "new", true).unwrap_err(),
        "rebase of \"child1\" onto landed state failed"
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("--source child1 --dest new"), "{calls}");
    assert!(calls.contains("abort"), "{calls}");
    // No children: nothing to rebase.
    let hg_cmd = fake_hg(&dir, "empty.sh", "if has log; then exit 0; fi\nexit 1\n");
    assert!(hg_cascade_state_with(&dir, &hg_cmd, &set, &commits, "new", true).is_ok());
}

#[test]
#[cfg(unix)]
fn t1170_hg_prune_branches_deletes_obsolete_and_strips() {
    let dir = temp_dir("t1170-hg-prune");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has strip; then printf 'strip %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has bookmark; then printf 'bookmark %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has bookmarks; then printf '   gone  1:landed\\n   stay  2:other\\n'; exit 0; fi\n\
             if has log; then\n  {rev}\n  if [ \"$rev\" = \"gone\" ]; then printf 'landed'; exit 0; fi\n  \
             printf 'other'; exit 0\nfi\nexit 1\n",
            calls = calls.display(),
            rev = FAKE_HG_REV_LOOP,
        ),
    );
    let commits = vec![land_commit("landed", &[], "work")];
    let sets = vec![CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = Io::canned(&[]);
    hg_prune_branches_with(&dir, &hg_cmd, &mut io, &sets, &commits).unwrap();
    assert!(
        io.out.stderr.contains("Deleting bookmark \"gone\""),
        "{}",
        io.out.stderr
    );
    assert!(!io.out.stderr.contains("stay"), "{}", io.out.stderr);
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("--delete -- gone"), "{calls}");
    assert!(
        calls.contains("strip") && calls.contains("(landed::landed)"),
        "{calls}"
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_reconcile_updates_to_bookmark_or_commit() {
    let dir = temp_dir("t1170-hg-reconcile");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "printf 'update %s\\n' \"$*\" >> \"{calls}\"\nexit 0\n",
            calls = calls.display()
        ),
    );
    hg_reconcile_with(&dir, &hg_cmd, "abc123", Some("work"));
    hg_reconcile_with(&dir, &hg_cmd, "def456", None);
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("update -- work"), "{calls}");
    assert!(calls.contains("update -- def456"), "{calls}");
}

#[test]
#[cfg(unix)]
fn t1170_save_local_state_hg_shelves_and_records_bookmark() {
    let dir = temp_dir("t1170-hg-save");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has --help; then exit 0; fi\n\
             if has unshelve; then printf 'unshelve\\n' >> \"{calls}\"; exit 0; fi\n\
             if has --delete; then printf 'delete\\n' >> \"{calls}\"; exit 0; fi\n\
             if has shelve; then printf 'shelve %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has status; then printf '? new.txt\\n'; exit 0; fi\n\
             if has bookmarks; then printf ' * work  1:x\\n'; exit 0; fi\n\
             if has branch; then printf 'default\\n'; exit 0; fi\n\
             if has log; then printf 'commitnode'; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let mut io = Io::canned(&["y"]);
    let state = save_local_state_hg_with(&dir, &hg_cmd, &mut io).unwrap();
    let stash = state.stash.expect("changes were shelved");
    assert!(stash.starts_with("arc-"), "{stash}");
    assert_eq!(state.local_commit, "commitnode");
    assert_eq!(state.local_branch, "default");
    assert_eq!(state.local_bookmark.as_deref(), Some("work"));
    assert!(
        io.out.stderr.contains("Shelving uncommitted changes"),
        "{}",
        io.out.stderr
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(
        calls.contains("--unknown") && calls.contains(&stash),
        "{calls}"
    );
}

#[test]
#[cfg(unix)]
fn t1170_save_local_state_hg_decline_aborts_stash() {
    let dir = temp_dir("t1170-hg-decline");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has --help; then exit 0; fi\n\
         if has shelve; then exit 0; fi\n\
         if has status; then printf 'M edit.txt\\n'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&["n"]);
    assert_eq!(
        save_local_state_hg_with(&dir, &hg_cmd, &mut io).unwrap_err(),
        ABORT
    );
}

#[test]
#[cfg(unix)]
fn t1170_save_local_state_hg_reports_shelve_failure() {
    let dir = temp_dir("t1170-hg-shelvefail");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has --help; then exit 0; fi\n\
         if has shelve; then exit 1; fi\n\
         if has status; then printf 'M edit.txt\\n'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&["y"]);
    let err = save_local_state_hg_with(&dir, &hg_cmd, &mut io).unwrap_err();
    assert!(err.contains("failed"), "{err}");
}

#[test]
#[cfg(unix)]
fn t1170_save_local_state_hg_reports_log_failure() {
    let dir = temp_dir("t1170-hg-logfail");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has status; then exit 0; fi\n\
         if has log; then if has \"-l\"; then exit 1; fi; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert!(save_local_state_hg_with(&dir, &hg_cmd, &mut io).is_err());
}

#[test]
fn t1170_save_local_state_hg_reports_status_failure() {
    let dir = temp_dir("t1170-hg-nostatus");
    let mut io = Io::canned(&["y"]);
    assert!(save_local_state_hg(&dir, &mut io).is_err());
}

#[test]
#[cfg(unix)]
fn t1170_restore_local_state_hg_updates_branch_and_unshelves() {
    let dir = temp_dir("t1170-hg-restore");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has unshelve; then printf 'unshelve %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has --delete; then printf 'delete %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has shelve; then printf 'shelve %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has bookmark; then printf 'bookmark %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has branch; then printf 'branch %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\n\
             if has update; then printf 'update %s\\n' \"$*\" >> \"{calls}\"; exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let state = HgLocalState {
        local_commit: "abc123def456".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: Some("work".to_string()),
        stash: Some("arc-test".to_string()),
    };
    let mut io = Io::canned(&[]);
    restore_local_state_hg_with(&dir, &hg_cmd, &mut io, &state).unwrap();
    assert!(
        io.out.stderr.contains("bookmarked as \"work\""),
        "{}",
        io.out.stderr
    );
    assert!(
        io.out.stderr.contains("Restoring uncommitted changes"),
        "{}",
        io.out.stderr
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("update -- abc123def456"), "{calls}");
    assert!(calls.contains("branch --force -- default"), "{calls}");
    assert!(calls.contains("bookmark --force -- work"), "{calls}");
    assert!(calls.contains("unshelve --keep --name arc-test"), "{calls}");
    assert!(calls.contains("shelve --delete arc-test"), "{calls}");
}

#[test]
#[cfg(unix)]
fn t1170_restore_local_state_hg_without_bookmark_or_stash() {
    let dir = temp_dir("t1170-hg-restorebare");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "printf '%s\\n' \"$*\" >> \"{calls}\"\nexit 0\n",
            calls = calls.display()
        ),
    );
    let state = HgLocalState {
        local_commit: "abc123def4567890".to_string(),
        local_branch: "stable".to_string(),
        local_bookmark: None,
        stash: None,
    };
    let mut io = Io::canned(&[]);
    restore_local_state_hg_with(&dir, &hg_cmd, &mut io, &state).unwrap();
    assert!(
        io.out
            .stderr
            .contains("(at \"abc123def456\" on branch \"stable\")"),
        "{}",
        io.out.stderr
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(!calls.contains("bookmark"), "{calls}");
    assert!(!calls.contains("unshelve"), "{calls}");
}

#[test]
#[cfg(unix)]
fn t1170_restore_local_state_hg_reports_branch_failure() {
    let dir = temp_dir("t1170-hg-branchfail");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has update; then exit 0; fi\nif has branch; then exit 1; fi\nexit 1\n",
    );
    let state = HgLocalState {
        local_commit: "abc".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: None,
    };
    let mut io = Io::canned(&[]);
    assert!(restore_local_state_hg_with(&dir, &hg_cmd, &mut io, &state).is_err());
}

#[test]
fn t1170_restore_local_state_hg_reports_update_failure() {
    let dir = temp_dir("t1170-hg-noupdate");
    let state = HgLocalState {
        local_commit: "abc".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: None,
    };
    let mut io = Io::canned(&[]);
    assert!(restore_local_state_hg(&dir, &mut io, &state).is_err());
}

#[test]
#[cfg(unix)]
fn t1170_discard_local_state_hg_restores_on_unshelve_failure() {
    let dir = temp_dir("t1170-hg-discardfail");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has unshelve; then exit 1; fi\n\
             if has shelve; then exit 0; fi\n\
             if has update; then printf 'update\\n' >> \"{calls}\"; exit 0; fi\n\
             if has branch; then exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let state = HgLocalState {
        local_commit: "abc".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: Some("arc-test".to_string()),
    };
    let mut io = Io::canned(&[]);
    assert!(discard_local_state_hg_with(&dir, &hg_cmd, &mut io, &state).is_err());
    // The failed discard restored the saved state before reporting.
    assert!(std::fs::read_to_string(&calls).unwrap().contains("update"));
}

#[test]
fn t1170_discard_local_state_hg_applies_nothing_without_stash() {
    let dir = temp_dir("t1170-hg-discardok");
    let state = HgLocalState {
        local_commit: "abc".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: None,
    };
    let mut io = Io::canned(&[]);
    assert!(discard_local_state_hg(&dir, &mut io, &state).is_ok());
}

#[test]
fn t1170_land_hg_reports_save_failure() {
    let dir = temp_dir("t1170-hg-landsave");
    let mut io = Io::canned(&[]);
    assert!(
        land_hg(
            &dir,
            &mut io,
            &Options::default(),
            true,
            &[],
            "default",
            &["default".to_string()],
            None,
            None,
            &[],
            &[],
            None
        )
        .is_err()
    );
}

#[test]
fn t1170_should_push_hg_set_holds_and_increments() {
    let hold = Options {
        hold: true,
        ..Options::default()
    };
    assert!(!should_push_hg_set(&hold, 0, 0));
    let incremental = Options {
        incremental: true,
        ..Options::default()
    };
    assert!(should_push_hg_set(&incremental, 0, 2));
    assert!(!should_push_hg_set(&Options::default(), 0, 2));
    assert!(should_push_hg_set(&Options::default(), 2, 2));
}

#[test]
#[cfg(unix)]
fn t1170_push_and_cascade_hg_reports_push_failure() {
    let dir = temp_dir("t1170-hg-pushcascade");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has push; then exit 1; fi\n\
         if has bookmark; then exit 0; fi\n\
         if has log; then printf 'oldpos'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        push_and_cascade_hg(
            &dir,
            &hg_cmd,
            &mut io,
            &Options::default(),
            "default",
            &["bm".to_string()],
            Some("cursor"),
            &[],
            true,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap_err(),
        "Push failed! Fix the error and run \"arc land\" again."
    );
}

#[test]
#[cfg(unix)]
fn t1170_push_and_cascade_hg_skips_prune_when_keeping() {
    let dir = temp_dir("t1170-hg-keepbranches");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has push; then exit 0; fi\n\
             if has strip; then printf 'strip\\n' >> \"{calls}\"; exit 0; fi\n\
             if has bookmark; then exit 0; fi\n\
             if has bookmarks; then exit 0; fi\n\
             if has log; then exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let options = Options {
        keep_branches: true,
        ..Options::default()
    };
    let commits = vec![land_commit("aaa", &[], "work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    push_and_cascade_hg(
        &dir,
        &hg_cmd,
        &mut io,
        &options,
        "default",
        &[],
        Some("cursor"),
        &commits,
        true,
        &mut vec![set.clone()],
        &mut vec![set],
    )
    .unwrap();
    assert!(!calls.exists(), "prune ran despite --keep-branches");
}

#[test]
#[cfg(unix)]
fn t1170_land_hg_inner_with_reports_push_failure() {
    let dir = temp_dir("t1170-hg-innerpush");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has push; then exit 1; fi\n\
         if has pull; then exit 0; fi\n\
         if has rebase; then if has --abort; then exit 0; fi; cat > /dev/null; exit 0; fi\n\
         if has bookmark; then exit 0; fi\n\
         if has bookmarks; then exit 0; fi\n\
         if has update; then exit 0; fi\n\
         if has branch; then exit 0; fi\n\
         if has log; then\n  PREV=\"\"; REV=\"\"; for a in \"$@\"; do case \"$PREV\" in -r|--rev) REV=\"$a\";; esac; PREV=\"$a\"; done\n  \
         if [ \"$REV\" = \"tip\" ]; then printf 'newtip'; exit 0; fi\n  printf 'oldpos'; exit 0\nfi\nexit 1\n",
    );
    let commits = vec![land_commit("aaa", &[], "work")];
    let sets = vec![CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let state = HgLocalState {
        local_commit: "aaa".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: None,
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        land_hg_inner_with(
            &dir,
            &hg_cmd,
            &mut io,
            &Options::default(),
            true,
            &["feat".to_string()],
            "default",
            &["bm".to_string()],
            Some("iii"),
            None,
            &sets,
            &commits,
            None,
            &state,
        )
        .unwrap_err(),
        "Push failed! Fix the error and run \"arc land\" again."
    );
}

// T1170 coverage: git onto/into selection, target fetching, and target
// resolution.

#[test]
fn t1170_git_onto_remote_reads_config() {
    let (repo, _bare) = repo_with_origin("t1170-onto-remote");
    std::fs::write(
        repo.join(".arcconfig"),
        r#"{"arc.land.onto-remote": "origin"}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_remote(&repo, &mut io, &Options::default(), &["master".to_string()])
            .unwrap(),
        "origin"
    );
    assert!(
        io.out.stderr.contains(
            "Remote \"origin\" was selected by reading \"arc.land.onto-remote\" configuration."
        ),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_git_onto_refs_ignores_empty_config() {
    // An empty `arc.land.onto` list selects nothing: selection falls
    // through to the default target.
    let repo = make_repo("t1170-empty-onto");
    std::fs::write(repo.join(".arcconfig"), r#"{"arc.land.onto": []}"#).unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &[]).unwrap(),
        vec!["master".to_string()]
    );
    assert!(
        io.out
            .stderr
            .contains("Landing onto target \"master\", the default target under Git."),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_git_onto_refs_ignores_unconnected_upstream() {
    // A branch tracking a local branch has no path to a remote: warn and
    // fall back to the default target.
    let repo = make_repo("t1170-no-path");
    git(&repo, &["branch", "--track", "child", "master"]);
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &["child".to_string()]).unwrap(),
        vec!["master".to_string()]
    );
    assert!(
        io.out.stderr.contains("NO PATH TO REMOTE"),
        "{}",
        io.out.stderr
    );
    assert!(
        io.out.stderr.contains("there is no path to a remote"),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_git_onto_refs_warns_on_local_cycle() {
    // Branches tracking each other form a local cycle: warn and fall back
    // to the default target.
    let repo = make_repo("t1170-cycle");
    git(&repo, &["branch", "a"]);
    git(&repo, &["branch", "b"]);
    git(&repo, &["config", "branch.a.remote", "."]);
    git(&repo, &["config", "branch.a.merge", "refs/heads/b"]);
    git(&repo, &["config", "branch.b.remote", "."]);
    git(&repo, &["config", "branch.b.merge", "refs/heads/a"]);
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &["a".to_string()]).unwrap(),
        vec!["master".to_string()]
    );
    assert!(io.out.stderr.contains("LOCAL CYCLE"), "{}", io.out.stderr);
    assert!(io.out.stderr.contains("a -> b"), "{}", io.out.stderr);
}

#[test]
fn t1170_git_onto_multi_remote_errors() {
    // Two sources tracking different remotes (or remote branches) fail
    // with the verbatim disambiguation errors.
    let repo = make_repo("t1170-multi");
    git(&repo, &["remote", "add", "origin", "/nonexistent-origin"]);
    git(&repo, &["remote", "add", "fork", "/nonexistent-fork"]);
    git(&repo, &["branch", "a"]);
    git(&repo, &["branch", "b"]);
    git(&repo, &["config", "branch.a.remote", "origin"]);
    git(&repo, &["config", "branch.a.merge", "refs/heads/x"]);
    git(&repo, &["config", "branch.b.remote", "fork"]);
    git(&repo, &["config", "branch.b.merge", "refs/heads/y"]);
    // Upstream paths resolve through real tracking refs.
    let head = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["update-ref", "refs/remotes/origin/x", &head]);
    git(&repo, &["update-ref", "refs/remotes/fork/y", &head]);
    let symbols = vec!["a".to_string(), "b".to_string()];
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_remote(&repo, &mut io, &Options::default(), &symbols).unwrap_err(),
        "The \"onto\" refs you have selected are connected to multiple different remotes via Git \
         branch upstreams. Use \"--onto-remote\" to select a single remote."
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_onto_refs(&repo, &mut io, &Options::default(), &symbols).unwrap_err(),
        "The branches you are landing are connected to multiple different remote branches via Git \
         branch upstreams. Use \"--onto\" to select the refs you want to push to."
    );
}

#[test]
fn t1170_select_into_state_covers_remote_and_local_defaults() {
    // `--into-remote` plus `--into` selects a remote state and reports the
    // flag; the default `onto` target stays local under `--into-local`.
    let options = Options {
        into_remote: Some("upstream".to_string()),
        into: Some("stable".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        select_into_state(
            &mut io,
            &options,
            "origin",
            &["master".to_string()],
            &|_| true
        )
        .unwrap(),
        IntoState::Remote {
            remote: "upstream".to_string(),
            name: "stable".to_string()
        }
    );
    assert!(
        io.out
            .stderr
            .contains("Will merge into remote \"upstream\", selected with the \"--into\" flag."),
        "{}",
        io.out.stderr
    );
    let options = Options {
        into_local: true,
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        select_into_state(
            &mut io,
            &options,
            "origin",
            &["master".to_string()],
            &|_| true
        )
        .unwrap(),
        IntoState::Local("master".to_string())
    );
}

#[test]
fn t1170_git_select_targets_resolves_remote_into() {
    // A missing local copy fetches from the remote and reports FETCHED;
    // `--into-remote` runs the fetchable probe.
    let (repo, _bare) = repo_with_origin("t1170-targets");
    git(&repo, &["update-ref", "-d", "refs/remotes/origin/master"]);
    let options = Options {
        into_remote: Some("origin".to_string()),
        into: Some("master".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    let (onto_remote, onto_refs, into_commit, into_ref, branch) =
        select_git_targets(&repo, &options, &mut io, &["master".to_string()]).unwrap();
    assert_eq!(onto_remote, "origin");
    assert_eq!(onto_refs, vec!["master".to_string()]);
    assert_eq!(into_ref.as_deref(), Some("master"));
    assert_eq!(branch, None);
    let master = git(&repo, &["rev-parse", "master"]);
    assert_eq!(into_commit.as_deref(), Some(master.as_str()));
    assert!(io.out.stderr.contains("FETCHED"), "{}", io.out.stderr);
    assert!(
        io.out
            .stderr
            .contains("Will merge into remote \"origin\", selected with the \"--into\" flag."),
        "{}",
        io.out.stderr
    );
}

#[test]
fn t1170_git_select_targets_empty_into_has_no_ref() {
    let (repo, _bare) = repo_with_origin("t1170-targets-empty");
    let options = Options {
        into_empty: true,
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    let (_, _, into_commit, into_ref, _) =
        select_git_targets(&repo, &options, &mut io, &["master".to_string()]).unwrap();
    assert_eq!(into_commit, None);
    assert_eq!(into_ref, None);
}

#[test]
fn t1170_git_fetch_target_failing_refetch_errors() {
    // A stale local copy refetches, and a failed refetch is fatal.
    let repo = make_repo("t1170-refetch");
    let master = git(&repo, &["rev-parse", "master"]);
    git(
        &repo,
        &["remote", "add", "origin", "/nonexistent-remote-xyz"],
    );
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/master", &master],
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_into_commit(
            &repo,
            &mut io,
            &IntoState::Remote {
                remote: "origin".to_string(),
                name: "master".to_string()
            },
            None
        )
        .unwrap_err(),
        "Fetch of \"master\" from remote \"origin\" failed! Fix the error and run \"arc land\" again."
    );
    assert!(io.out.stderr.contains("** FETCH **"), "{}", io.out.stderr);
}

#[test]
fn t1170_git_fetch_target_perforce_syncs() {
    let repo = make_repo("t1170-p4fetch");
    std::fs::create_dir_all(repo.join(".git").join("refs").join("remotes").join("p4")).unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_select_into_commit(
            &repo,
            &mut io,
            &IntoState::Remote {
                remote: "p4".to_string(),
                name: "master".to_string()
            },
            None
        )
        .unwrap_err(),
        "Perforce sync failed! Fix the error and run \"arc land\" again."
    );
    assert!(io.out.stderr.contains("** P4 SYNC **"), "{}", io.out.stderr);
}

#[test]
fn t1170_git_is_perforce_remote_resolves_absolute_git_dir() {
    // Linked worktrees report an absolute `--git-dir`, which resolves
    // directly instead of joining onto the working copy.
    let repo = make_repo("t1170-worktree");
    let wt = temp_dir("t1170-wt").join("wt");
    git(
        &repo,
        &["worktree", "add", wt.to_str().unwrap(), "-b", "wtbranch"],
    );
    let git_dir = git(&wt, &["rev-parse", "--git-dir"]);
    assert!(Path::new(&git_dir).is_absolute(), "{git_dir}");
    assert!(!git_is_perforce_remote(&wt, "p4"));
}

// T1170 coverage: git merge/commit/raw-commit handling, the p4 seams,
// cascade/prune/reconcile, the land loop, and git local state.

/// A repo whose `feature` tip conflicts with its `master` tip, plus the
/// base commit both descend from.
fn conflicting_repo(name: &str) -> (PathBuf, String, String, String) {
    let repo = make_repo(name);
    let base = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    let feature = commit_file(&repo, "file.txt", "feature\n", "feature work");
    git(&repo, &["checkout", "master"]);
    let master = commit_file(&repo, "file.txt", "master\n", "master work");
    (repo, base, feature, master)
}

#[test]
fn t1170_git_execute_merge_conflict_reports_symbols() {
    // A failing merge aborts, resets, and reports the conflict with the
    // max commit's indirect symbols plus the incremental hint.
    let (repo, base, feature, master) = conflicting_repo("t1170-conflict");
    let mut commit = land_commit(&feature, &[&base], "feature work");
    commit.indirect_symbols = vec!["feature".to_string()];
    let commits = vec![commit];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_execute_merge(
            &repo,
            &mut io,
            &set,
            &commits,
            Some(&master),
            false,
            true,
            &|_| { Ok("msg".to_string()) }
        )
        .unwrap_err(),
        "Encountered a merge conflict."
    );
    assert!(
        io.out.stdout.contains("** MERGE CONFLICT **"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("(reachable from: \"feature\")"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("Use \"--incremental\""),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_git_execute_merge_into_empty_merge_strategy() {
    // Under the merge strategy into the empty state, the scratch parent
    // drops and the merged max becomes the single parent.
    let repo = make_repo("t1170-empty-merge");
    let base = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    let feature = commit_file(&repo, "extra.txt", "new\n", "feature work");
    let commits = vec![land_commit(&feature, &[&base], "feature work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    let cursor = git_execute_merge(&repo, &mut io, &set, &commits, None, false, false, &|_| {
        Ok("landed".to_string())
    })
    .unwrap();
    let parents = git(&repo, &["rev-list", "--parents", "-n1", &cursor]);
    assert_eq!(parents, format!("{cursor} {feature}"), "{parents}");
}

#[test]
fn t1170_git_apply_commit_writes_and_reports_failure() {
    let repo = make_repo("t1170-apply-commit");
    std::fs::write(repo.join("file.txt"), "changed\n").unwrap();
    git(&repo, &["add", "."]);
    let hash = git_apply_commit(&repo, "hello", None, None, false).unwrap();
    assert_eq!(git(&repo, &["log", "-n1", "--format=%s", &hash]), "hello");
    // `--allow-empty` commits a clean tree.
    let empty = git_apply_commit(&repo, "empty", None, None, true).unwrap();
    assert_eq!(git(&repo, &["log", "-n1", "--format=%s", &empty]), "empty");
    // A commit git refuses reports git's own detail.
    let err = git_apply_commit(&repo, "", None, None, false).unwrap_err();
    assert!(!err.is_empty(), "expected git's failure detail");
}

#[test]
fn t1170_git_merge_diff_query_failure() {
    // Outside a repository the already-landed diff probe fails outright.
    let dir = temp_dir("t1170-norepo-diff");
    let commits = vec![land_commit("abc", &[], "work")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    assert!(
        git_execute_merge(
            &dir,
            &mut io,
            &set,
            &commits,
            Some("def"),
            false,
            false,
            &|_| { Ok("msg".to_string()) }
        )
        .is_err()
    );
}

#[test]
fn t1170_git_read_raw_commit_reports_git_detail() {
    let repo = make_repo("t1170-cat-file");
    let err = git_read_raw_commit(&repo, "deadbeef").unwrap_err();
    assert!(err.contains("deadbeef"), "{err}");
}

#[test]
fn t1170_git_write_raw_commit_rejects_unwritable() {
    // Outside a repository there is no object store to write to.
    let dir = temp_dir("t1170-norepo-write");
    assert_eq!(
        git_write_raw_commit(&dir, &RawCommit::empty()).unwrap_err(),
        "git hash-object failed"
    );
}

#[test]
fn t1170_parse_raw_commit_blob_rejects_malformed() {
    // A header line without `label value` shape fails the pattern.
    let err = parse_raw_commit_blob("garbage\n\nmsg").unwrap_err();
    assert!(err.contains("Expected to match pattern"), "{err}");
    // Repeated non-parent labels fail.
    let err = parse_raw_commit_blob("tree t\nauthor a\nauthor b\n\n").unwrap_err();
    assert!(err.contains("Encountered two \"author\" lines"), "{err}");
    // Unknown labels fail.
    let err = parse_raw_commit_blob("tree t\nfoo bar\n\n").unwrap_err();
    assert!(err.contains("Unknown attribute label \"foo\""), "{err}");
    // Repeated parents parse fine.
    let raw = parse_raw_commit_blob("tree t\nparent a\nparent b\n\ndesc\n").unwrap();
    assert_eq!(raw.parents, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(raw.message.as_deref(), Some("desc\n"));
}

#[test]
fn t1170_git_p4_wrappers_report_status_and_failure() {
    // Outside a repository `git p4` fails, and the wrappers map that to
    // the verbatim submit/sync errors.
    let dir = temp_dir("t1170-p4wrap");
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_p4_submit(&dir, &mut io, "p4", "aaa").unwrap_err(),
        "Submit failed! Fix the error and run \"arc land\" again."
    );
    assert!(io.out.stderr.contains("SUBMITTING"), "{}", io.out.stderr);
    assert_eq!(
        git_p4_sync(&dir, "p4", "master").unwrap_err(),
        "Perforce sync failed! Fix the error and run \"arc land\" again."
    );
}

#[test]
#[cfg(unix)]
fn t1170_git_p4_seams_report_success() {
    let dir = temp_dir("t1170-p4ok");
    let mut io = Io::canned(&[]);
    assert!(git_p4_submit_with(&dir, "true", &[], &mut io, "p4", "aaa").is_ok());
    assert!(git_p4_sync_with(&dir, "true", &[], "p4", "master").is_ok());
}

#[test]
fn t1170_parse_for_each_ref_rejects_extra_field() {
    assert_eq!(
        parse_for_each_ref("refs/heads/a h1 extra\n").unwrap_err(),
        "Failed to explode line \"refs/heads/a h1 extra\"."
    );
}

#[test]
fn t1170_git_cascade_state_pick_rebases_onto_range_parent() {
    // `--pick` sets rebase descendants onto the range start's parent,
    // dropping the landed range from the descendant.
    let repo = make_repo("t1170-pick");
    git(&repo, &["checkout", "-b", "feature"]);
    let f1 = commit_file(&repo, "a.txt", "f1\n", "first");
    let f2 = commit_file(&repo, "a.txt", "f1\nf2\n", "second");
    git(&repo, &["checkout", "-b", "child"]);
    commit_file(&repo, "b.txt", "c1\n", "child work");
    git(&repo, &["checkout", "master"]);
    let master = git(&repo, &["rev-parse", "master"]);
    let commits = vec![
        land_commit(&f1, &[], "first"),
        land_commit(&f2, &[&f1], "second"),
    ];
    let set = CommitSet {
        revision: None,
        commits: vec![0, 1],
        is_pick: true,
    };
    let mut io = Io::canned(&[]);
    git_cascade_state(&repo, &mut io, &set, &commits, &master, true);
    assert!(io.out.stderr.contains("CASCADE"), "{}", io.out.stderr);
    assert!(
        !io.out.stderr.contains("REBASE CONFLICT"),
        "{}",
        io.out.stderr
    );
    let log = git(&repo, &["log", "--format=%s", "child"]);
    assert!(log.contains("child work"), "{log}");
    assert!(!log.lines().any(|line| line == "second"), "{log}");
}

#[test]
fn t1170_git_cascade_state_conflict_warns_and_skips() {
    // A descendant that does not rebase cleanly aborts, resets, and warns
    // instead of failing the land.
    let repo = make_repo("t1170-cascade-conflict");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "file.txt", "F1\n", "first");
    let f2 = commit_file(&repo, "file.txt", "F1\nF2\n", "second");
    git(&repo, &["checkout", "-b", "child"]);
    commit_file(&repo, "file.txt", "F1\nC1\n", "child work");
    git(&repo, &["checkout", "master"]);
    let master = commit_file(&repo, "file.txt", "M1\n", "master work");
    let before = git(&repo, &["rev-parse", "child"]);
    let commits = vec![land_commit(&f2, &[], "second")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    git_cascade_state(&repo, &mut io, &set, &commits, &master, true);
    assert!(
        io.out.stderr.contains("REBASE CONFLICT"),
        "{}",
        io.out.stderr
    );
    assert!(io.out.stderr.contains("\"child\""), "{}", io.out.stderr);
    // The failed rebase aborted: the child still points at its old tip.
    assert_eq!(git(&repo, &["rev-parse", "child"]), before);
}

#[test]
fn t1170_git_reconcile_perforce_updates_resets_and_detaches() {
    // At-or-behind the cursor: switch to the branch and reset it.
    let repo = make_repo("t1170-p4-update");
    git(&repo, &["checkout", "-b", "feature"]);
    let f1 = commit_file(&repo, "extra.txt", "new\n", "feature work");
    git(&repo, &["checkout", "master"]);
    let master = git(&repo, &["rev-parse", "master"]);
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: master,
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    git_reconcile_perforce(&repo, &mut io, &state, None, &[], &[], &f1).unwrap();
    assert!(io.out.stderr.contains("UPDATE"), "{}", io.out.stderr);
    assert_eq!(git(&repo, &["rev-parse", "master"]), f1);
    assert_eq!(git(&repo, &["branch", "--show-current"]), "master");

    // Ahead of the cursor: check the branch out but leave it in place.
    let repo = make_repo("t1170-p4-checkout");
    git(&repo, &["checkout", "-b", "feature"]);
    let f1 = commit_file(&repo, "extra.txt", "new\n", "feature work");
    git(&repo, &["checkout", "master"]);
    let master = git(&repo, &["rev-parse", "master"]);
    let state = GitLocalState {
        local_ref: Some("feature".to_string()),
        local_commit: f1.clone(),
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    git_reconcile_perforce(&repo, &mut io, &state, None, &[], &[], &master).unwrap();
    assert!(io.out.stderr.contains("CHECKOUT"), "{}", io.out.stderr);
    assert!(
        io.out.stderr.contains("has unpublished changes"),
        "{}",
        io.out.stderr
    );
    assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
    assert_eq!(git(&repo, &["rev-parse", "feature"]), f1);

    // No surviving branches: stay on a detached head.
    let repo = make_repo("t1170-p4-detached");
    let master = git(&repo, &["rev-parse", "master"]);
    let state = GitLocalState {
        local_ref: None,
        local_commit: master,
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    git_reconcile_perforce(&repo, &mut io, &state, None, &[], &[], "abc").unwrap();
    assert!(io.out.stderr.contains("DETACHED HEAD"), "{}", io.out.stderr);
}

#[test]
fn t1170_git_should_push_holds_and_increments() {
    let hold = Options {
        hold: true,
        ..Options::default()
    };
    assert!(!git_should_push(&hold, 0, 0));
    let incremental = Options {
        incremental: true,
        ..Options::default()
    };
    assert!(git_should_push(&incremental, 0, 2));
    assert!(!git_should_push(&Options::default(), 0, 2));
    assert!(git_should_push(&Options::default(), 2, 2));
}

#[test]
fn t1170_git_push_and_update_submits_perforce() {
    let repo = make_repo("t1170-p4submit");
    std::fs::create_dir_all(repo.join(".git").join("refs").join("remotes").join("p4")).unwrap();
    let mut io = Io::canned(&[]);
    assert_eq!(
        git_push_and_update(
            &repo,
            &mut io,
            "p4",
            &["master".to_string()],
            "aaa",
            true,
            false,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &[],
        )
        .unwrap_err(),
        "Submit failed! Fix the error and run \"arc land\" again."
    );
    assert!(io.out.stderr.contains("SUBMITTING"), "{}", io.out.stderr);
}

#[test]
fn t1170_git_finish_landing_reconciles_perforce_and_plain() {
    // Perforce mode reconciles the Perforce way, then reports DONE.
    let repo = make_repo("t1170-finish-p4");
    std::fs::create_dir_all(repo.join(".git").join("refs").join("remotes").join("p4")).unwrap();
    let master = git(&repo, &["rev-parse", "master"]);
    let state = GitLocalState {
        local_ref: None,
        local_commit: master,
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    git_finish_landing(&repo, &mut io, false, &state, "p4", &[], "abc", None, &[]).unwrap();
    assert!(io.out.stderr.contains("DETACHED HEAD"), "{}", io.out.stderr);
    assert!(io.out.stderr.contains("DONE"), "{}", io.out.stderr);

    // Plain mode reconciles through the connected branches.
    let repo = make_repo("t1170-finish-plain");
    let master = git(&repo, &["rev-parse", "master"]);
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: master.clone(),
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    git_finish_landing(
        &repo,
        &mut io,
        false,
        &state,
        "origin",
        &[],
        &master,
        None,
        &[],
    )
    .unwrap();
    assert!(io.out.stderr.contains("RETURN"), "{}", io.out.stderr);
    assert!(io.out.stderr.contains("DONE"), "{}", io.out.stderr);
}

#[test]
fn t1170_land_git_inner_reports_merge_failure() {
    let (repo, base, feature, master) = conflicting_repo("t1170-inner-merge");
    let commits = vec![land_commit(&feature, &[&base], "feature work")];
    let sets = vec![CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: master.clone(),
        local_path: None,
        stashed: false,
    };
    let options = Options {
        strategy: Some("merge".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        land_git_inner(
            &repo,
            &mut io,
            &options,
            false,
            &["feature".to_string()],
            "origin",
            &["master".to_string()],
            Some(&master),
            None,
            &sets,
            &commits,
            None,
            &state,
        )
        .unwrap_err(),
        "Encountered a merge conflict."
    );
}

#[test]
fn t1170_land_git_inner_reports_push_failure() {
    // The merge succeeds but the push to a missing remote fails.
    let repo = make_repo("t1170-inner-push");
    let base = git(&repo, &["rev-parse", "master"]);
    git(&repo, &["checkout", "-b", "feature"]);
    let feature = commit_file(&repo, "extra.txt", "new\n", "feature work");
    git(&repo, &["checkout", "master"]);
    let master = git(&repo, &["rev-parse", "master"]);
    let commits = vec![land_commit(&feature, &[&base], "feature work")];
    let sets = vec![CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    }];
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: master.clone(),
        local_path: None,
        stashed: false,
    };
    let options = Options {
        strategy: Some("squash".to_string()),
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        land_git_inner(
            &repo,
            &mut io,
            &options,
            true,
            &["feature".to_string()],
            "nonexistent-remote",
            &["master".to_string()],
            Some(&master),
            None,
            &sets,
            &commits,
            None,
            &state,
        )
        .unwrap_err(),
        "Push failed! Fix the error and run \"arc land\" again."
    );
}

#[test]
fn t1170_truthy_config_reads_collections() {
    assert!(is_truthy_config(&serde_json::json!([0])));
    assert!(!is_truthy_config(&serde_json::json!([])));
    assert!(is_truthy_config(&serde_json::json!({"a": false})));
    assert!(!is_truthy_config(&serde_json::json!({})));
}

#[test]
fn t1170_save_local_state_git_confirms_dirty_externals() {
    // A submodule with modified content counts as a dirty external: the
    // save confirms past it, then stashes and records the state.
    let sub = make_repo("t1170-sub");
    let repo = make_repo("t1170-super");
    git(
        &repo,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub.to_str().unwrap(),
            "sub",
        ],
    );
    git(&repo, &["commit", "-m", "add submodule"]);
    std::fs::write(repo.join("sub").join("file.txt"), "dirty\n").unwrap();
    // Dirty externals confirm past without stashing (nothing stashable).
    let mut io = Io::canned(&["y"]);
    let state = save_local_state_git(&repo, &mut io).unwrap();
    assert!(
        io.out.stdout.contains("submodule(s) have uncommitted"),
        "{}",
        io.out.stdout
    );
    assert_eq!(state.local_ref.as_deref(), Some("master"));
    assert!(!state.stashed);
}

#[test]
fn t1170_restore_local_state_git_detached_and_failure() {
    let repo = make_repo("t1170-restore-git");
    let master = git(&repo, &["rev-parse", "master"]);
    let detached = GitLocalState {
        local_ref: None,
        local_commit: master.clone(),
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    restore_local_state_git(&repo, &mut io, &detached).unwrap();
    assert!(
        io.out.stderr.contains("to detached commit"),
        "{}",
        io.out.stderr
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), master);
    // A bogus commit fails the ref restore.
    let bogus = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    assert!(restore_local_state_git(&repo, &mut io, &bogus).is_err());
}

#[test]
fn t1170_discard_local_state_git_restores_on_stash_failure() {
    // With no stash entry to apply, the discard restores the saved ref
    // first, then reports the original stash error.
    let repo = make_repo("t1170-discard-git");
    let master = git(&repo, &["rev-parse", "master"]);
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: master,
        local_path: None,
        stashed: true,
    };
    let mut io = Io::canned(&[]);
    assert!(discard_local_state_git(&repo, &mut io, &state).is_err());
    assert!(io.out.stderr.contains("LOAD STASH"), "{}", io.out.stderr);
    assert!(io.out.stderr.contains("LOAD STATE"), "{}", io.out.stderr);
}

// T1170 coverage: revision discovery/grouping, build views and gates,
// the engine seams, and workflow validation.

#[test]
fn t1170_json_id_reads_numbers_and_strings() {
    assert_eq!(json_id(&serde_json::json!(7)), Some(7));
    assert_eq!(json_id(&serde_json::json!("12")), Some(12));
    assert_eq!(json_id(&serde_json::json!(u64::MAX)), None);
    assert_eq!(json_id(&serde_json::json!("x")), None);
}

#[test]
fn t1170_build_views_skip_phidless_rows_and_absolutize_uris() {
    // Buildable and plan rows without a PHID are skipped; a schemeless
    // conduit URI defaults to https; a build whose buildable belongs to no
    // landing revision is dropped.
    let call = canned(vec![
        (
            METHOD_BUILDABLE_SEARCH,
            serde_json::json!({"data": [
                {"fields": {"objectPHID": "PHID-DIFF-89"}},
                {"id": "7", "phid": "PHID-HMBB-7", "fields": {"objectPHID": "PHID-DIFF-89"}},
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
                {"id": 43, "fields": {
                    "name": "Ghost",
                    "buildStatus": {"value": "failed"},
                    "buildablePHID": "PHID-HMBB-gone",
                    "buildPlanPHID": "PHID-HMPA-1",
                }},
            ]}),
        ),
        (
            METHOD_BUILDPLAN_SEARCH,
            serde_json::json!({"data": [
                {"fields": {"behaviors": {"arc-land": {"value": "always"}}}},
                {"phid": "PHID-HMPA-1", "fields": {"behaviors": {"arc-land": {"value": "always"}}}},
            ]}),
        ),
    ]);
    let revisions = vec![view(89, "accepted", "Accepted", false, "PHID-USER-a")];
    let builds = fetch_build_views(&call, &revisions, Some("phab.example.com/api/")).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].buildable_monogram, "B7");
    assert_eq!(builds[0].buildable_uri, "https://phab.example.com/B7");
}

#[test]
fn t1170_build_views_skip_plan_search_without_plans() {
    // Builds without a plan ref skip the plan search entirely (the canned
    // conduit has no plan entry, so any query would fail the test).
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
                }},
            ]}),
        ),
    ]);
    let revisions = vec![view(89, "accepted", "Accepted", false, "PHID-USER-a")];
    let builds = fetch_build_views(&call, &revisions, None).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].plan_behavior, None);
}

#[test]
fn t1170_differential_revision_ids_skips_short_lines() {
    assert_eq!(
        differential_revision_ids("x\nDifferential Revision: D7\n"),
        vec![7]
    );
    assert!(differential_revision_ids("short").is_empty());
}

#[test]
fn t1170_collect_hg_commits_merges_duplicate_hash() {
    // A hash seen under two symbols attaches once, direct under both.
    let mut commits = Vec::new();
    collect_hg_commits(&mut commits, "aaa--root summary\n", "s1").unwrap();
    collect_hg_commits(&mut commits, "aaa--root summary\n", "s2").unwrap();
    assert_eq!(commits.len(), 1);
    // Each caller's first line attaches direct; both attach indirect.
    assert_eq!(
        commits[0].direct_symbols,
        vec!["s1".to_string(), "s2".to_string()]
    );
    assert_eq!(
        commits[0].indirect_symbols,
        vec!["s1".to_string(), "s2".to_string()]
    );
}

#[test]
fn t1170_attach_message_revisions_ignores_unrequested() {
    // Search rows for revisions no commit asked about attach nowhere.
    let call = canned(vec![(
        diff_submit::METHOD_REVISION_SEARCH,
        serde_json::json!({"data": [
            modern_row(11, "accepted", "Accepted", false, "PHID-USER-a"),
            modern_row(99, "accepted", "Accepted", false, "PHID-USER-a"),
        ]}),
    )]);
    let mut commits = vec![land_commit("aaa", &[], "one")];
    let mut io = Io::canned(&[]);
    load_revision_refs(
        &mut io,
        &call,
        &mut commits,
        &|_| Ok("Differential Revision: D11".to_string()),
        &|_| Ok(Vec::new()),
        None,
    )
    .unwrap();
    assert_eq!(commits[0].explicit.as_ref().unwrap().monogram, "D11");
    assert_eq!(commits[0].related.len(), 1);
}

#[test]
fn t1170_apply_forced_revision_adopts_matching_related() {
    let force = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let mut other = land_commit("b", &[], "other");
    other.related = vec![view(2, "accepted", "Accepted", false, "PHID-USER-a")];
    let mut matching = land_commit("c", &[], "match");
    matching.related = vec![force.clone()];
    let mut commits = vec![land_commit("a", &[], "free"), other, matching];
    apply_forced_revision(&mut commits, &force).unwrap();
    // Free commits and commits already related to the forced revision are
    // adopted; unrelated commits stay unforced without failing.
    assert_eq!(commits[0].explicit.as_ref().unwrap().phid, force.phid);
    assert!(commits[1].explicit.is_none());
    assert_eq!(commits[2].explicit.as_ref().unwrap().phid, force.phid);
}

#[test]
fn t1170_resolve_commit_revision_follows_parents_and_cycles() {
    // Cyclic parents resolve to no revision instead of recursing forever.
    let commits = vec![land_commit("a", &["b"], "a"), land_commit("b", &["a"], "b")];
    assert_eq!(resolve_commit_revision(&commits, 0).unwrap(), None);
    // Parents outside the landing range are skipped.
    let commits = vec![land_commit("c", &["zzz"], "c")];
    assert_eq!(resolve_commit_revision(&commits, 0).unwrap(), None);
    // Two parents resolving to the same revision agree.
    let d1 = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let mut pa = land_commit("pa", &[], "pa");
    pa.explicit = Some(d1.clone());
    let mut pb = land_commit("pb", &[], "pb");
    pb.explicit = Some(d1.clone());
    let commits = vec![pa, pb, land_commit("m", &["pa", "pb"], "merge")];
    assert_eq!(
        resolve_commit_revision(&commits, 2).unwrap().as_ref(),
        Some(&d1)
    );
}

#[test]
fn t1170_ancestor_revision_phids_dedups_shared_parents() {
    // Cycles terminate with no revisions.
    let commits = vec![land_commit("a", &["b"], "a"), land_commit("b", &["a"], "b")];
    assert!(ancestor_revision_phids(&commits, 0).is_empty());
    // Shared parent revisions list once.
    let d1 = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let mut pa = land_commit("pa", &[], "pa");
    pa.explicit = Some(d1.clone());
    let mut pb = land_commit("pb", &[], "pb");
    pb.explicit = Some(d1.clone());
    let commits = vec![pa, pb, land_commit("m", &["pa", "pb"], "merge")];
    assert_eq!(ancestor_revision_phids(&commits, 2), vec![d1.phid.clone()]);
}

#[test]
fn t1170_ordered_commit_sets_skips_missing_groups() {
    let commits = vec![land_commit("a", &[], "a")];
    let sets = ordered_commit_sets(&commits, &[], &["PHID-DREV-9".to_string()]).unwrap();
    assert!(sets.is_empty());
}

#[test]
fn t1170_push_unordered_groups_appends_unassembled() {
    let d1 = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let mut commit = land_commit("a", &[], "a");
    commit.explicit = Some(d1.clone());
    let commits = vec![commit];
    let groups = vec![(d1.phid.clone(), vec![0])];
    let mut sets = Vec::new();
    push_unordered_groups(&mut sets, &commits, &groups).unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].revision.as_ref(), Some(&d1));
    // Already-assembled groups are skipped.
    push_unordered_groups(&mut sets, &commits, &groups).unwrap();
    assert_eq!(sets.len(), 1);
}

#[test]
fn t1170_mark_implicit_commits_flags_foreign() {
    let d1 = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let d2 = view(2, "accepted", "Accepted", false, "PHID-USER-a");
    let mut foreign = land_commit("a", &[], "a");
    foreign.explicit = Some(d2);
    let mut owned = land_commit("b", &[], "b");
    owned.explicit = Some(d1.clone());
    let mut commits = vec![foreign, owned, land_commit("c", &[], "c")];
    let sets = vec![CommitSet {
        revision: Some(d1),
        commits: vec![0, 1, 2],
        is_pick: false,
    }];
    mark_implicit_commits(&mut commits, &sets);
    assert!(commits[0].is_implicit);
    assert!(!commits[1].is_implicit);
    assert!(commits[2].is_implicit);
}

#[test]
fn t1170_filter_commit_sets_keeps_open_under_squash() {
    let open = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let commits = vec![land_commit("a", &[], "a")];
    let sets = vec![CommitSet {
        revision: Some(open),
        commits: vec![0],
        is_pick: false,
    }];
    let mut io = Io::canned(&[]);
    let kept = filter_commit_sets(&mut io, sets, &commits, true, false, &|_| String::new());
    assert_eq!(kept.len(), 1);
    assert!(
        !io.out.stdout.contains("DISCARDING ANCESTORS"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_fetch_user_monograms_skips_phidless_rows() {
    let call = canned(vec![(
        METHOD_USER_SEARCH,
        serde_json::json!({"data": [
            {"fields": {"username": "ghost"}},
            {"phid": "PHID-USER-1", "fields": {"username": "alice"}},
        ]}),
    )]);
    let monograms = fetch_user_monograms(&call, &["PHID-USER-1".to_string()]).unwrap();
    assert_eq!(monograms.len(), 1);
    assert_eq!(monograms["PHID-USER-1"], "@alice");
}

#[test]
fn t1170_confirm_gates_prompt_unknown_keys() {
    // Findings with no dedicated banner prompt their query directly.
    let revisions = vec![view(1, "accepted", "Accepted", false, "PHID-USER-a")];
    let viewer = ViewerIdentity {
        phid: "PHID-USER-a".to_string(),
        name: "alice".to_string(),
    };
    let findings = vec![land_checks::GateFinding {
        prompt: "arc.land.custom",
        title: "CUSTOM".to_string(),
        query: "Proceed with custom gate?".to_string(),
        count: 1,
    }];
    let mut io = Io::canned(&["y"]);
    confirm_revision_gates(&mut io, &revisions, &viewer, &[], &[], &findings).unwrap();
    assert!(
        io.out.stdout.contains("Proceed with custom gate?"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_group_problem_builds_skips_missing_appends_shared() {
    let build = |id: i64, phid: &str| land_checks::BuildView {
        status: "failed".to_string(),
        plan_behavior: Some("always".to_string()),
        revision_phid: phid.to_string(),
        build_id: id,
        build_name: format!("Build {id}"),
        buildable_monogram: "B7".to_string(),
        buildable_uri: "https://phab.example.com/B7".to_string(),
    };
    let builds = vec![build(1, "PHID-DREV-1"), build(2, "PHID-DREV-1")];
    // Index 99 names no build and is skipped; both real builds share one
    // revision group.
    let groups = group_problem_builds(&builds, &[0, 99, 1]);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].builds.len(), 2);
}

#[test]
fn t1170_confirm_builds_reports_mixed_and_ongoing() {
    let child = view(2, "accepted", "Accepted", false, "PHID-USER-a");
    let build = |id: i64, status: &str, phid: &str| land_checks::BuildView {
        status: status.to_string(),
        plan_behavior: Some("always".to_string()),
        revision_phid: phid.to_string(),
        build_id: id,
        build_name: format!("Build {id}"),
        buildable_monogram: "B7".to_string(),
        buildable_uri: "https://phab.example.com/B7".to_string(),
    };
    let finding = land_checks::GateFinding {
        prompt: land_checks::PROMPT_FAILED_BUILDS,
        title: "BUILD FAILURES".to_string(),
        query: "Land despite builds?".to_string(),
        count: 1,
    };
    // Failures plus ongoing builds combine into one message; builds for
    // revisions that are not landing are skipped.
    let builds = vec![
        build(1, "failed", "PHID-DREV-2"),
        build(2, "building", "PHID-DREV-2"),
        build(3, "failed", "PHID-DREV-9"),
    ];
    let mut io = Io::canned(&["y"]);
    confirm_builds(&mut io, std::slice::from_ref(&child), &builds, &finding).unwrap();
    assert!(
        io.out
            .stdout
            .contains("have build failures or ongoing builds:"),
        "{}",
        io.out.stdout
    );
    assert!(!io.out.stdout.contains("Build 3"), "{}", io.out.stdout);
    // Ongoing builds alone take the ongoing title.
    let finding = land_checks::GateFinding {
        prompt: land_checks::PROMPT_ONGOING_BUILDS,
        title: "ONGOING BUILDS".to_string(),
        query: "Land despite builds?".to_string(),
        count: 1,
    };
    let mut io = Io::canned(&["y"]);
    confirm_builds(
        &mut io,
        &[child],
        &[build(4, "building", "PHID-DREV-2")],
        &finding,
    )
    .unwrap();
    assert!(
        io.out.stdout.contains("** ONGOING BUILDS **"),
        "{}",
        io.out.stdout
    );
    assert!(
        io.out.stdout.contains("1 revision(s) have ongoing builds:"),
        "{}",
        io.out.stdout
    );
}

#[test]
fn t1170_into_display_string_splits_vcs() {
    assert_eq!(
        into_display_string(Vcs::Hg, Some("abc123def4567890")).as_deref(),
        Some("abc123def456")
    );
    assert_eq!(into_display_string(Vcs::Hg, None), None);
    assert_eq!(
        into_display_string(Vcs::Git, Some("abc123def4567890")).as_deref(),
        Some("abc123def456")
    );
}

#[test]
fn t1170_confirm_gates_if_online_skips_offline() {
    let dir = temp_dir("t1170-offline-gates");
    let mut io = Io::canned(&[]);
    assert!(confirm_gates_if_online(&dir, &Options::default(), &mut io, None, &[], &[]).is_ok());
}

#[test]
fn t1170_hash_display_for_matches_vcs() {
    assert_eq!(
        hash_display_for(Vcs::Hg)("abc123def4567890"),
        "abc123def456"
    );
    assert_eq!(
        hash_display_for(Vcs::Git)("abc123def4567890"),
        "abc123def456"
    );
}

#[test]
fn t1170_render_for_matches_vcs() {
    let commits = vec![land_commit(
        "abc123def4567890abc123def4567890abc12345",
        &[],
        "work",
    )];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    assert!(render_for(Vcs::Git, &commits, false)(&set).contains("abc123def456"));
    assert!(render_for(Vcs::Hg, &commits, false)(&set).contains("abc123def456"));
}

#[test]
fn t1170_default_source_symbols_hg_reports_failure() {
    let dir = temp_dir("t1170-hg-nosource");
    let mut io = Io::canned(&[]);
    assert!(default_source_symbols(&dir, &Options::default(), Vcs::Hg, &mut io).is_err());
}

#[test]
fn t1170_select_hg_targets_reports_missing_remote() {
    let dir = temp_dir("t1170-hg-notargets");
    let mut io = Io::canned(&[]);
    assert_eq!(
        select_hg_targets(&dir, &Options::default(), &mut io, &[]).unwrap_err(),
        "No remote \"default\" exists in this repository."
    );
}

#[test]
fn t1170_resolve_sources_hg_reports_failure() {
    let dir = temp_dir("t1170-hg-nosources");
    let mut io = Io::canned(&[]);
    assert!(resolve_sources(&dir, &mut io, Vcs::Hg, &["x".to_string()]).is_err());
}

#[test]
fn t1170_select_source_commits_hg_reports_failure() {
    let dir = temp_dir("t1170-hg-noselect");
    let symbols = vec![GitSymbol {
        symbol: "x".to_string(),
        commit: "abc".to_string(),
    }];
    assert!(select_source_commits(&dir, Vcs::Hg, &symbols, None).is_err());
}

#[test]
fn t1170_build_sets_offline_groups_unassociated() {
    let dir = temp_dir("t1170-offline-sets");
    let mut io = Io::canned(&[]);
    let validated = Validated {
        strategy: Strategy::Squash,
        revision: None,
        revision_raw: None,
    };
    let mut commits = vec![land_commit("a", &[], "a"), land_commit("b", &["a"], "b")];
    let sets = build_sets(
        &dir,
        &mut io,
        Vcs::Git,
        &validated,
        &mut commits,
        None,
        &|hash| hash.to_string(),
    )
    .unwrap();
    assert_eq!(sets.len(), 1);
    assert!(sets[0].revision.is_none());
    assert_eq!(sets[0].commits, vec![0, 1]);
    assert!(!sets[0].is_pick);
}

#[test]
fn t1170_commit_message_reader_hg_reports_failure() {
    let dir = temp_dir("t1170-hg-nomessage");
    assert!(commit_message_reader(&dir, Vcs::Hg)("abc").is_err());
}

#[test]
fn t1170_validate_options_rejects_unparseable_project_config() {
    let repo = make_repo("t1170-badarcconfig");
    std::fs::write(repo.join(".arcconfig"), "{not json").unwrap();
    let mut io = Io::canned(&[]);
    let err = validate_options(&Options::default(), &repo, &mut io).unwrap_err();
    assert!(err.contains("Unable to parse"), "{err}");
}

#[test]
fn t1170_load_options_for_run_rejects_bare_config() {
    let options = Options {
        config: vec!["bogus".to_string()],
        ..Options::default()
    };
    let err = load_options_for_run(&options).unwrap_err();
    assert!(err.contains("name=value"), "{err}");
}

#[test]
fn t1170_execute_classified_merges_saved_prompts() {
    // Saved project prompts land in `io.saved` before the engine runs
    // (which then fails on the missing pushable remote).
    let repo = make_repo("t1170-exec-prompts");
    std::fs::write(
        repo.join(".arcconfig"),
        r#"{"prompts": [{"prompt": "k", "response": "y"}]}"#,
    )
    .unwrap();
    let mut io = Io::canned(&[]);
    assert!(execute_with_io_classified(&repo, &args(&["--", "master"]), &mut io).is_err());
    assert_eq!(io.saved.get("k").map(|s| s.response.as_str()), Some("y"));
}

#[test]
fn t1170_parse_hg_bookmarks_skips_plain_lines() {
    assert_eq!(
        parse_hg_bookmarks("not a bookmark\n * feat  1:abc\n"),
        vec![("feat".to_string(), true)]
    );
    assert_eq!(parse_hg_bookmarks("   \n"), Vec::new());
}

#[test]
#[cfg(unix)]
fn t1170_hg_push_change_reports_move_failure() {
    // A bookmark that cannot move fails the push before any push runs.
    let dir = temp_dir("t1170-hg-movefail");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has push; then exit 0; fi\n\
         if has bookmark; then exit 1; fi\n\
         if has log; then printf 'oldhash'; exit 0; fi\nexit 1\n",
    );
    let mut io = Io::canned(&[]);
    assert!(
        hg_push_change_with(
            &dir,
            &hg_cmd,
            &mut io,
            "default",
            &["b1".to_string()],
            "newhash"
        )
        .is_err()
    );
}

#[test]
#[cfg(unix)]
fn t1170_hg_push_change_missing_position_moves_without_restore() {
    // A bookmark with no recorded position still moves, but with nothing
    // to restore there is no pull-back.
    let dir = temp_dir("t1170-hg-nopos");
    let calls = dir.join("calls");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has push; then exit 0; fi\n\
             if has pull; then printf 'pull\\n' >> \"{calls}\"; exit 0; fi\n\
             if has bookmark; then printf 'bookmark\\n' >> \"{calls}\"; exit 0; fi\n\
             if has log; then exit 0; fi\nexit 1\n",
            calls = calls.display(),
        ),
    );
    let mut io = Io::canned(&[]);
    hg_push_change_with(
        &dir,
        &hg_cmd,
        &mut io,
        "default",
        &["b1".to_string()],
        "newhash",
    )
    .unwrap();
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert!(calls.contains("bookmark"), "{calls}");
    assert!(!calls.contains("pull"), "{calls}");
}

#[test]
fn t1170_build_sets_online_reports_conduit_failure() {
    // Forced-revision resolution runs over conduit, which fails against a
    // refused localhost endpoint without touching the network.
    let dir = temp_dir("t1170-online-fail");
    let client = arc_rs::conduit::Client::new("http://127.0.0.1:9/api/", "token").unwrap();
    let validated = Validated {
        strategy: Strategy::Squash,
        revision: Some(RevisionSymbol::Id(9)),
        revision_raw: None,
    };
    let mut commits = vec![land_commit("a", &[], "a")];
    let mut io = Io::canned(&[]);
    let err = build_sets(
        &dir,
        &mut io,
        Vcs::Git,
        &validated,
        &mut commits,
        Some(&client),
        &|hash| hash.to_string(),
    )
    .unwrap_err();
    assert!(!err.is_empty(), "expected the transport failure");
}

#[test]
#[cfg(unix)]
fn t1170_land_hg_inner_with_reports_prefetch_failure() {
    // Set-message prefetch over a refused conduit endpoint fails before
    // any merge runs.
    let dir = temp_dir("t1170-hg-prefetch");
    let client = arc_rs::conduit::Client::new("http://127.0.0.1:9/api/", "token").unwrap();
    let d1 = view(1, "accepted", "Accepted", false, "PHID-USER-a");
    let commits = vec![land_commit("aaa", &[], "work")];
    let sets = vec![CommitSet {
        revision: Some(d1),
        commits: vec![0],
        is_pick: false,
    }];
    let state = HgLocalState {
        local_commit: "aaa".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: None,
    };
    let hg_cmd = hg::HgCommand::custom("false", &[]);
    let mut io = Io::canned(&[]);
    let err = land_hg_inner_with(
        &dir,
        &hg_cmd,
        &mut io,
        &Options::default(),
        true,
        &["feat".to_string()],
        "default",
        &["bm".to_string()],
        Some("iii"),
        None,
        &sets,
        &commits,
        Some(&client),
        &state,
    )
    .unwrap_err();
    assert!(!err.is_empty(), "expected the transport failure");
}

#[test]
fn t1170_execute_engine_reports_discovery_failure() {
    // With targets selected and a client built from `--arcrc-file`, set
    // discovery fails against the refused conduit endpoint.
    let (repo, _bare) = repo_with_origin("t1170-engine-fail");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "extra.txt", "new\n", "feature work");
    let arcrc = repo.join("arcrc.json");
    std::fs::write(
        &arcrc,
        r#"{"hosts": {"http://127.0.0.1:9/api/": {"token": "test-token"}}}"#,
    )
    .unwrap();
    let options = Options {
        refs: vec!["feature".to_string()],
        conduit_uri: Some("http://127.0.0.1:9/api/".to_string()),
        arcrc_file: Some(arcrc.to_string_lossy().to_string()),
        ..Options::default()
    };
    let validated = Validated {
        strategy: Strategy::Squash,
        revision: None,
        revision_raw: None,
    };
    let mut io = Io::canned(&[]);
    let err = execute_engine(&repo, &options, Vcs::Git, &validated, &mut io).unwrap_err();
    assert!(!err.is_empty(), "expected the discovery failure");
    assert!(io.out.stderr.contains("ONTO REMOTE"), "{}", io.out.stderr);
}

#[test]
fn t1170_prompts_abort_on_exhausted_answers() {
    // Piped stdin is never a TTY under `cargo test` and reads EOF, so
    // an answerless TTY session aborts; bail out (rather than block
    // on a real terminal) if that ever changes.
    if crate::prompt::stdin_is_terminal() {
        return;
    }
    assert_eq!(
        prompt_for_key(&mut Io::canned(&[]), "k", "q?").unwrap_err(),
        ABORT
    );
    assert_eq!(
        console_confirm(&mut Io::canned(&[]), "q?", true).unwrap_err(),
        ABORT
    );
}

#[test]
fn t1170_truncate_prefers_glyph_counts() {
    // Multibyte text past the byte limit but within the glyph limit
    // returns unchanged.
    assert_eq!(truncate_glyphs("éé", 3), "éé");
}

#[test]
fn t1170_write_saved_prompt_rejects_parentless_paths() {
    // A path without a parent skips directory creation, then the
    // sibling staging write cannot rename into place.
    let staging = std::env::current_dir()
        .unwrap()
        .join(format!(".tmp-{}", std::process::id()));
    std::fs::remove_file(&staging).ok();
    assert!(write_saved_prompt(std::path::Path::new(""), "k", "v").is_err());
    std::fs::remove_file(&staging).ok();
}

#[test]
fn t1170_land_config_reports_bad_runtime_config() {
    let options = Options {
        config: vec!["noequals".to_string()],
        ..Options::default()
    };
    let dir = temp_dir("t1170-landcfg");
    assert!(land_config_value(&options, &dir, "arc.land.strategy").is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_client_for_dir_reports_missing_uris() {
    // Without a flag, `ARC_CONDUIT_URI`, or discovery the client
    // reports the missing URI. Ambient configuration that resolves a
    // URI skips the arm (CI has none).
    let dir = temp_dir("t1170-nouri");
    if resolve_conduit_uri(None, &dir).is_some() {
        std::fs::remove_dir_all(&dir).ok();
        return;
    }
    let err = client_for_dir(None, &dir).unwrap_err();
    assert!(err.contains("no conduit URI"), "unexpected: {err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_saved_prompts_load_without_vcs() {
    // Only the load path is exercised here; the merged answers
    // depend on ambient configuration files.
    let dir = temp_dir("t1170-noprompts");
    let _ = load_saved_prompts(&dir);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_select_strategy_errors_surface_in_validation() {
    // A shadowed config keeps discovery deterministic: only the
    // bogus `--strategy` can fail selection.
    let dir = temp_dir("t1170-strategy");
    let options = Options {
        strategy: Some("bogus".to_string()),
        config_file: vec!["/nonexistent-arc-land-xyz".to_string()],
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    let err = validate_options(&options, &dir, &mut io).unwrap_err();
    assert!(err.contains("unknown"), "unexpected: {err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_buildable_display_handles_missing_rows() {
    assert_eq!(
        buildable_display_for(&[], "PHID-x", None),
        (String::new(), String::new())
    );
}

#[test]
fn t1170_filter_keeps_everything_without_squash() {
    let mut io = Io::canned(&[]);
    let kept = filter_commit_sets(&mut io, Vec::new(), &[], false, false, &|_| String::new());
    assert!(kept.is_empty());
}

#[test]
fn t1170_split_header_lines_reject_bad_shapes() {
    assert_eq!(split_header_line("tree abc"), Some(("tree", "abc")));
    assert!(split_header_line("tree ").is_none());
    assert!(split_header_line("a-b x").is_none());
    assert!(split_header_line(" x").is_none());
}

#[test]
fn t1170_reconcile_candidates_skip_missing_local_refs() {
    let dir = temp_dir("t1170-reconcile");
    let state = GitLocalState {
        local_ref: None,
        local_commit: "aaa".to_string(),
        local_path: None,
        stashed: false,
    };
    assert!(git_reconcile_candidates(&dir, &state, None, &[], &[]).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_reconcile_perforce_walks_local_paths() {
    let repo = make_repo("t1170-perforce");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: head.clone(),
        local_path: Some(git::UpstreamPath::new()),
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    git_reconcile_perforce(&repo, &mut io, &state, None, &[], &[], &head).unwrap();
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
#[cfg(unix)]
fn t1170_hg_default_symbols_skips_blank_branches() {
    let dir = temp_dir("t1170-hgblank");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has bookmarks; then exit 0; fi\nif has branch; then printf '  \\n'; exit 0; fi\nprintf 'nodehash'; exit 0\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_default_symbols_with(&dir, &hg_cmd, &mut io).unwrap(),
        vec!["nodehash".to_string()]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn t1170_hg_resolve_symbols_falls_back_to_commits() {
    let dir = temp_dir("t1170-hgcommit");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has bookmarks; then printf '   bm1 1:x\\n'; exit 0; fi\nif has branches; then printf 'default\\n'; exit 0; fi\nprintf 'nodeabc'; exit 0\n",
    );
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_resolve_symbols_with(&dir, &hg_cmd, &mut io, &["abc".to_string()]).unwrap(),
        vec!["nodeabc".to_string()]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_ambiguous_markers_fall_back_to_generic_views() {
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_ambiguous_symbol(&mut io, "s", "tag", 2),
        "Symbol \"s\" is ambiguous."
    );
    assert!(io.out.stdout.contains("Marker \"s\""), "{}", io.out.stdout);
}

#[test]
fn t1170_hg_onto_refs_default_on_empty_config() {
    // A shadowing config file pins `arc.land.onto` to empty
    // regardless of ambient configuration.
    let dir = temp_dir("t1170-hgonto");
    let file = dir.join("onto.json");
    std::fs::write(&file, r#"{"arc.land.onto": []}"#).unwrap();
    let options = Options {
        config_file: vec![file.to_string_lossy().to_string()],
        ..Options::default()
    };
    let mut io = Io::canned(&[]);
    assert_eq!(
        hg_select_onto_refs(&dir, &mut io, &options).unwrap(),
        vec!["default".to_string()]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn t1170_hg_merge_reports_tip_failures() {
    let dir = temp_dir("t1170-hgmerge-tip");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has rebase; then cat > /dev/null; exit 0; fi\nif has log; then\n  {rev}\n  if [ \"$rev\" = \"tip\" ]; then exit 1; fi\n  printf 'aaa'; exit 0\nfi\nexit 0\n",
            rev = FAKE_HG_REV_LOOP,
        ),
    );
    let commits = vec![land_commit("aaa", &[], "first")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    let mut io = Io::canned(&[]);
    assert!(
        hg_execute_merge_with(
            &dir,
            &hg_cmd,
            &mut io,
            &set,
            &commits,
            Some("iii"),
            true,
            None,
            &|_| Ok("landed message".to_string()),
        )
        .is_err()
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn t1170_hg_stdin_write_failures_are_fatal() {
    // Unix-only: the stub program (`false`) has no Windows counterpart.
    // A megabyte past an instantly-exiting reader deterministically
    // breaks the pipe on every platform.
    let dir = temp_dir("t1170-hgepipe");
    let hg_cmd = hg::HgCommand::custom("false", &[]);
    assert!(!hg_write_stdin_with(
        &dir,
        &hg_cmd,
        &["whatever"],
        &"x".repeat(1 << 20)
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn t1170_hg_cascade_keeps_successful_rebases() {
    let dir = temp_dir("t1170-hgcascade-ok");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has rebase; then exit 0; fi\nif has log; then printf 'child1\\n'; exit 0; fi\nexit 1\n",
    );
    let commits = vec![land_commit("old", &[], "landed")];
    let set = CommitSet {
        revision: None,
        commits: vec![0],
        is_pick: false,
    };
    assert!(hg_cascade_state_with(&dir, &hg_cmd, &set, &commits, "new", true).is_ok());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_select_targets_dispatches_by_vcs() {
    // Without an `hg` binary the Mercurial selection fails before any
    // ref work (and a bare directory fails it when one exists).
    let dir = temp_dir("t1170-targets");
    let mut io = Io::canned(&[]);
    assert!(select_targets(&dir, &Options::default(), Vcs::Hg, &mut io, &[]).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn t1170_apply_shelve_reports_delete_failures() {
    let dir = temp_dir("t1170-shelve-del");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        "if has --delete; then exit 1; fi\nexit 0\n",
    );
    let mut io = Io::canned(&[]);
    let state = HgLocalState {
        local_commit: "aaa".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: Some("s1".to_string()),
    };
    assert!(apply_shelve_with(&dir, &hg_cmd, &mut io, &state).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(unix)]
fn t1170_discard_hg_restores_before_reporting() {
    // The first unshelve fails but the restore's retry succeeds, so
    // the original error reports after the state is back.
    let dir = temp_dir("t1170-shelve-state");
    let marker = dir.join("ate");
    let hg_cmd = fake_hg(
        &dir,
        "fakehg.sh",
        &format!(
            "if has unshelve; then if [ -e \"{m}\" ]; then exit 0; else touch \"{m}\"; exit 1; fi; fi\nexit 0\n",
            m = marker.display(),
        ),
    );
    let mut io = Io::canned(&[]);
    let state = HgLocalState {
        local_commit: "aaa".to_string(),
        local_branch: "default".to_string(),
        local_bookmark: None,
        stash: Some("s1".to_string()),
    };
    assert!(discard_local_state_hg_with(&dir, &hg_cmd, &mut io, &state).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_finish_landing_reports_reconcile_failures() {
    // The pull branch cannot be an ancestor of a bogus cursor, so the
    // reconcile (all local git reads) fails into the finish arm. The
    // upstream tracking ref comes from a local-path remote (no network).
    let upstream = make_repo("t1170-finish-up");
    let repo = make_repo("t1170-finish-fail");
    git(
        &repo,
        &["remote", "add", "origin", upstream.to_str().unwrap()],
    );
    git(&repo, &["fetch", "-q", "origin"]);
    git(
        &repo,
        &["branch", "--set-upstream-to=origin/master", "master"],
    );
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let state = GitLocalState {
        local_ref: Some("master".to_string()),
        local_commit: head,
        local_path: None,
        stashed: false,
    };
    let mut io = Io::canned(&[]);
    assert!(
        git_finish_landing(
            &repo,
            &mut io,
            false,
            &state,
            "origin",
            &["master".to_string()],
            "bogus-not-a-commit",
            None,
            &[],
        )
        .is_err()
    );
    std::fs::remove_dir_all(&repo).ok();
    std::fs::remove_dir_all(&upstream).ok();
}

#[test]
fn t1170_write_raw_commit_handles_epipe() {
    // Outside a repository `git hash-object` exits before reading
    // stdin; a megabyte past the dead reader deterministically
    // breaks the pipe on every platform.
    let dir = temp_dir("t1170-hashobject");
    let raw = RawCommit {
        tree: None,
        parents: Vec::new(),
        author: None,
        committer: None,
        message: Some("x".repeat(1 << 20)),
    };
    assert_eq!(
        git_write_raw_commit(&dir, &raw).unwrap_err(),
        "git hash-object failed"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_classified_runs_skip_configured_writers() {
    // A preset config writer skips installation; the mutually
    // exclusive into flags then fail validation deterministically.
    let repo = make_repo("t1170-classified");
    let mut io = Io::canned(&[]);
    io.config_writer = Some(ConfigWriter {
        user_path: None,
        local_path: None,
    });
    let err = execute_with_io_classified(
        &repo,
        &args(&["--into", "x", "--into-empty", "--"]),
        &mut io,
    )
    .unwrap_err();
    assert!(
        err.message.contains("mutually exclusive"),
        "{}",
        err.message
    );
    std::fs::remove_dir_all(&repo).ok();
}

#[test]
fn t1170_run_in_reports_missing_terminator() {
    // Piped stdin is never a TTY under `cargo test`; bail out (rather
    // than parse differently) if that ever changes.
    if crate::prompt::stdin_is_terminal() {
        return;
    }
    let dir = temp_dir("t1170-terminator");
    assert_eq!(
        run_in(&dir, &args(&["--preview"])),
        std::process::ExitCode::FAILURE
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn t1170_execute_classified_uses_hg_metadata() {
    let dir = temp_dir("t1170-exec-hg");
    std::fs::create_dir(dir.join(".hg")).unwrap();
    let mut io = Io::canned(&[]);
    assert!(execute_with_io_classified(&dir, &args(&["--", "x"]), &mut io).is_err());
    let writer = io.config_writer.as_ref().expect("writer is installed");
    assert!(
        writer
            .local_path
            .as_ref()
            .is_some_and(|path| path.ends_with(".hg/arc/config")),
        "{writer:?}"
    );
}

#[test]
fn t1170_describe_json_type_names_all_shapes() {
    assert_eq!(describe_json_type(&serde_json::json!(null)), "NULL");
    assert_eq!(describe_json_type(&serde_json::json!(true)), "boolean");
    assert_eq!(describe_json_type(&serde_json::json!(7)), "integer");
    assert_eq!(describe_json_type(&serde_json::json!(7.5)), "double");
    assert_eq!(describe_json_type(&serde_json::json!("x")), "string");
    assert_eq!(describe_json_type(&serde_json::json!([1])), "array");
    assert_eq!(describe_json_type(&serde_json::json!({"a": 1})), "array");
}

// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

/// `temp_dir` with cleanup on drop, so `should_panic` tests do not leak.
struct W6hDir(PathBuf);

impl Drop for W6hDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
#[should_panic(expected = "failed:")]
fn git_helper_reports_failing_commands() {
    // A failing git invocation panics with the command and stderr.
    let dir = W6hDir(temp_dir("w6h-git-fail"));
    git(&dir.0, &["--bogus-flag-xyz"]);
}
