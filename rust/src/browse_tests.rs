use super::*;
use std::collections::HashMap;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn options_of(words: &[&str]) -> BrowseOptions {
    let Parsed::Options(opts) = parse_args(&argv(words)) else {
        panic!("expected options for {words:?}");
    };
    opts
}

#[test]
#[should_panic(expected = "expected options")]
fn options_of_rejects_non_options() {
    options_of(&["--bogus"]);
}

#[test]
fn parses_flags_targets_and_help() {
    let opts = options_of(&["--branch", "dev", "--types", "path,object", "--", "a:3"]);
    assert_eq!(opts.branch, Some("dev".to_string()));
    assert_eq!(opts.types, vec!["path".to_string(), "object".to_string()]);
    assert!(!opts.force);
    assert_eq!(opts.targets, vec!["a:3".to_string()]);

    let opts = options_of(&["--branch=dev", "--types=commit revision", "--force", "x"]);
    assert_eq!(opts.branch, Some("dev".to_string()));
    assert_eq!(
        opts.types,
        vec!["commit".to_string(), "revision".to_string()]
    );
    assert!(opts.force);

    assert!(matches!(parse_args(&argv(&["-h"])), Parsed::Help));
    assert!(matches!(parse_args(&argv(&["--bogus"])), Parsed::Error(_)));
    assert!(matches!(parse_args(&argv(&["--branch"])), Parsed::Error(_)));
    assert!(matches!(parse_args(&argv(&["--types"])), Parsed::Error(_)));
}

#[test]
fn splits_types_like_preg_split() {
    assert_eq!(split_types("path"), vec!["path".to_string()]);
    assert_eq!(
        split_types("commit,object  path"),
        vec![
            "commit".to_string(),
            "object".to_string(),
            "path".to_string()
        ]
    );
    // Separator runs count once; edge empties are kept, like PHP
    // without PREG_SPLIT_NO_EMPTY.
    assert_eq!(split_types(""), vec![String::new()]);
    assert_eq!(split_types("a,,b"), vec!["a".to_string(), "b".to_string()]);
    assert_eq!(
        split_types(",a,"),
        vec![String::new(), "a".to_string(), String::new()]
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "arc-rs-browse-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn fixture_repo(tag: &str) -> TempDir {
    let tmp = TempDir::new(tag);
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    git(&tmp.path, &["config", "user.email", "t@t"]);
    git(&tmp.path, &["config", "user.name", "t"]);
    git(&tmp.path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(tmp.path.join("f.txt"), "one\n").unwrap();
    git(&tmp.path, &["add", "f.txt"]);
    git(&tmp.path, &["commit", "-qm", "add f"]);
    tmp
}

/// Route-based mock: unknown methods fail the test loudly.
fn mock(
    routes: HashMap<String, serde_json::Value>,
) -> impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> {
    move |method: &str, _params: &serde_json::Value| {
        routes
            .get(method)
            .cloned()
            .ok_or_else(|| format!("unexpected call {method}"))
    }
}

#[test]
fn resolves_commit_hashes_locally() {
    let tmp = fixture_repo("commits");
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    assert_eq!(commit_hashes(&tmp.path, &None), vec![head.clone()]);
    assert_eq!(
        commit_hashes(&tmp.path, &Some("HEAD".to_string())),
        vec![head]
    );
    assert!(commit_hashes(&tmp.path, &Some(".".to_string())).is_empty());
    assert!(commit_hashes(&tmp.path, &Some("no-such-rev".to_string())).is_empty());
}

#[test]
fn absolutizes_uris_against_the_base() {
    assert_eq!(
        absolute_uri("https://phab.example/api/", "https://x.test/y"),
        "https://x.test/y"
    );
    assert_eq!(
        absolute_uri("https://phab.example/api/", "/D123"),
        "https://phab.example/D123"
    );
    assert_eq!(
        absolute_uri("https://phab.example/api/", "D123"),
        "https://phab.example/D123"
    );
}

#[test]
fn builds_diffusion_browse_uris() {
    let repo = RepoInfo {
        phid: "PHID-REPO-1".to_string(),
        browse_uri: "https://phab.example/diffusion/X/".to_string(),
        default_branch: "master".to_string(),
    };
    assert_eq!(
        new_browse_uri(&repo, "src/a b.txt", Some("12"), None),
        "https://phab.example/diffusion/X/browse/master/src/a%20b.txt$12"
    );
    // Branches are double-encoded; empty pieces fall back.
    assert_eq!(
        new_browse_uri(&repo, "", None, Some("feat/x")),
        "https://phab.example/diffusion/X/browse/feat%252Fx/"
    );
    assert_eq!(
        new_browse_uri(&repo, "/f", Some(""), Some("")),
        "https://phab.example/diffusion/X/browse/master/f"
    );
}

#[test]
fn renders_borderless_tables() {
    let out = render_table(
        &["Pick", "Type", "URI"],
        &[vec![
            "1".to_string(),
            "commit".to_string(),
            "https://x/1".to_string(),
        ]],
    );
    assert_eq!(out, "Pick Type   URI\n1    commit https://x/1\n");
}

#[test]
fn selects_picks_and_aborts_on_eof() {
    // BROWSE-B3: scripted sessions inject a TTY; `console_select` gates
    // on the real stdin state.
    let mut input = std::io::Cursor::new(b"nope\n99\n2\n");
    assert_eq!(
        console_select_with_tty(&mut input, "Pick?", 1, 3, true).unwrap(),
        2
    );
    let mut eof = std::io::Cursor::new(b"");
    assert_eq!(
        console_select_with_tty(&mut eof, "Pick?", 1, 3, true).unwrap_err(),
        ABORT
    );
}

#[test]
fn resolves_repositories_by_callsign_then_remote() {
    let tmp = fixture_repo("repo");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"repository.callsign": "X"}"#,
    )
    .unwrap();
    let mut routes = HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        serde_json::json!({"1": {
            "phid": "PHID-REPO-1",
            "uri": "https://phab.example/diffusion/X/",
            "callsign": "X",
        }}),
    );
    let call = mock(routes);
    let repo = resolve_repo(&tmp.path, &call).unwrap().unwrap();
    assert_eq!(repo.phid, "PHID-REPO-1");
    assert_eq!(repo.default_branch, "master");

    // Several matches (or none) mean "no repository".
    let mut routes = HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        serde_json::json!({"1": {"phid": "a", "uri": "u"}, "2": {"phid": "b", "uri": "u"}}),
    );
    let call = mock(routes);
    assert!(resolve_repo(&tmp.path, &call).unwrap().is_none());

    // Old servers without repository.query behave the same.
    let failing = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("ERR-CONDUIT-CALL: unknown method".to_string())
    };
    assert!(resolve_repo(&tmp.path, &failing).unwrap().is_none());
    let broken = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("boom".to_string())
    };
    assert!(resolve_repo(&tmp.path, &broken).is_err());
}

#[test]
fn resolves_repositories_by_remote_uri() {
    let tmp = fixture_repo("remoterepo");
    git(
        &tmp.path,
        &["remote", "add", "origin", "git@example.test:X.git"],
    );
    let mut routes = HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        serde_json::json!([{
            "phid": "PHID-REPO-2",
            "uri": "https://phab.example/diffusion/X/",
            "fields": {"defaultBranch": "dev"},
        }]),
    );
    let call = mock(routes);
    let repo = resolve_repo(&tmp.path, &call).unwrap().unwrap();
    assert_eq!(repo.phid, "PHID-REPO-2");
    assert_eq!(repo.default_branch, "dev");

    // Outside repositories there is nothing to resolve (no conduit call:
    // the empty route table errors loudly if `resolve_repo` calls out).
    let bare = TempDir::new("norepo");
    let never = mock(HashMap::new());
    assert!(resolve_repo(&bare.path, &never).unwrap().is_none());
}

fn repo_fixture(tag: &str) -> TempDir {
    let tmp = fixture_repo(tag);
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"repository.callsign": "X"}"#,
    )
    .unwrap();
    tmp
}

fn repo_routes() -> HashMap<String, serde_json::Value> {
    let mut routes = HashMap::new();
    routes.insert(
        "repository.query".to_string(),
        serde_json::json!({"1": {
            "phid": "PHID-REPO-1",
            "uri": "https://phab.example/diffusion/X/",
            "callsign": "X",
        }}),
    );
    routes
}

#[test]
fn resolves_object_uris() {
    let tmp = fixture_repo("object");
    let mut routes = HashMap::new();
    routes.insert(
        "phid.lookup".to_string(),
        serde_json::json!({"T7": {"uri": "https://phab.example/T7"}}),
    );
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![BrowseRef {
        token: Some("T7".to_string()),
        types: vec![BROWSE_OBJECT.to_string()],
        branch: None,
    }];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert_eq!(
        resolved[0].uris,
        vec![BrowseUri {
            uri: "https://phab.example/T7".to_string(),
            kind: "object".to_string(),
        }]
    );
    assert!(resolved[0].commits.is_empty());
}

#[test]
fn resolves_path_uris_with_lines() {
    let tmp = repo_fixture("path");
    let mut routes = repo_routes();
    routes.insert("phid.lookup".to_string(), serde_json::json!({}));
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![BrowseRef {
        token: Some("f.txt:4".to_string()),
        types: Vec::new(),
        branch: None,
    }];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    // Existing files resolve even untyped; only the path kind hits here
    // (the token is neither a commit nor a known object).
    assert_eq!(resolved[0].uris.len(), 1);
    assert_eq!(
        resolved[0].uris[0].uri,
        "https://phab.example/diffusion/X/browse/master/f.txt$4"
    );

    // Missing files resolve only when path-typed.
    let refs = vec![BrowseRef {
        token: Some("missing.txt".to_string()),
        types: Vec::new(),
        branch: None,
    }];
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert!(resolved[0].uris.is_empty());
    let refs = vec![BrowseRef {
        token: Some("missing.txt".to_string()),
        types: vec![BROWSE_PATH.to_string()],
        branch: None,
    }];
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert_eq!(resolved[0].uris.len(), 1);
}

#[test]
fn resolves_commit_and_revision_uris() {
    let tmp = repo_fixture("commitrev");
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let mut routes = repo_routes();
    routes.insert("phid.lookup".to_string(), serde_json::json!({}));
    routes.insert(
        "diffusion.querycommits".to_string(),
        serde_json::json!({
            "identifierMap": {head.clone(): "PHID-CMT-1"},
            "data": {"PHID-CMT-1": {"uri": "/rXabc123"}},
        }),
    );
    routes.insert(
        "differential.query".to_string(),
        serde_json::json!([{
            "statusName": "Accepted",
            "uri": "https://phab.example/D1",
            "hashes": [["gtcm", head]],
        }, {
            "statusName": "Closed",
            "uri": "https://phab.example/D2",
            "hashes": [["gtcm", head]],
        }]),
    );
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![BrowseRef {
        token: Some("HEAD".to_string()),
        types: Vec::new(),
        branch: None,
    }];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert_eq!(resolved[0].commits, vec![head]);
    // Commit URI plus the open revision; the closed one is skipped.
    // (phid.lookup finds nothing, the token is not a file.)
    let kinds: Vec<&str> = resolved[0].uris.iter().map(|u| u.kind.as_str()).collect();
    assert!(kinds.contains(&"commit"), "{kinds:?}");
    assert!(kinds.contains(&"revision"), "{kinds:?}");
    assert_eq!(resolved[0].uris.len(), 2);
}

/// A fake browser command which exits zero without doing anything.
#[cfg(windows)]
fn null_browser() -> Vec<String> {
    vec!["cmd".to_string(), "/c".to_string(), "rem".to_string()]
}

/// A fake browser command which exits zero without doing anything.
#[cfg(not(windows))]
fn null_browser() -> Vec<String> {
    vec!["true".to_string()]
}

#[test]
fn opens_single_hits_and_reports_misses() {
    let tmp = fixture_repo("open");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        serde_json::json!({"browser": null_browser()}).to_string(),
    )
    .unwrap();
    let mut routes = HashMap::new();
    routes.insert(
        "phid.lookup".to_string(),
        serde_json::json!({"T7": {"uri": "https://phab.example/T7"}}),
    );
    let call = mock(routes);
    let options = options_of(&["--types", "object", "T7"]);
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_prepared(
            &tmp.path,
            &options,
            &call,
            "https://phab.example/api/",
            &mut input
        ),
        std::process::ExitCode::SUCCESS
    );

    // Zero hits still exit zero, like PHP.
    let mut routes = HashMap::new();
    routes.insert("phid.lookup".to_string(), serde_json::json!({}));
    let call = mock(routes);
    let options = options_of(&["--types", "object", "T9"]);
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        run_prepared(
            &tmp.path,
            &options,
            &call,
            "https://phab.example/api/",
            &mut input
        ),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn disambiguates_single_refs_by_pick() {
    let tmp = repo_fixture("pick");
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let mut routes = repo_routes();
    routes.insert(
        "diffusion.querycommits".to_string(),
        serde_json::json!({
            "identifierMap": {head.clone(): "PHID-CMT-1"},
            "data": {"PHID-CMT-1": {"uri": "/rXabc"}},
        }),
    );
    routes.insert(
        "phid.lookup".to_string(),
        serde_json::json!({"HEAD": {"uri": "https://phab.example/object/HEAD"}}),
    );
    routes.insert("differential.query".to_string(), serde_json::json!([]));
    let call = mock(routes);
    let options = options_of(&["HEAD"]);
    // Pick the second URI (object); the footer search finds no revision.
    let mut input = std::io::Cursor::new(b"2\n");
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = build_refs(&options);
    // BROWSE-B3: scripted sessions inject a TTY.
    let uris = execute_refs_with_tty(&mut ctx, &refs, &mut input, false, true).unwrap();
    assert_eq!(uris, vec!["https://phab.example/object/HEAD".to_string()]);
}

fn pick_ref(token: Option<&str>) -> BrowseRef {
    BrowseRef {
        token: token.map(str::to_string),
        types: Vec::new(),
        branch: None,
    }
}

fn pick_hits(pairs: &[(&str, &str)]) -> ResolvedRef {
    ResolvedRef {
        commits: Vec::new(),
        uris: pairs
            .iter()
            .map(|(uri, kind)| BrowseUri {
                uri: (*uri).to_string(),
                kind: (*kind).to_string(),
            })
            .collect(),
    }
}

#[test]
fn renders_pick_tables_with_plain_titles() {
    // Bold titles strip to plain text without ANSI; single refs show
    // pick numbers.
    let refs = vec![pick_ref(Some("HEAD"))];
    let resolved = vec![pick_hits(&[
        ("https://x/1", "commit"),
        ("https://x/2", "object"),
    ])];
    assert_eq!(
        render_pick_table(&refs, &resolved, true, false),
        "Pick Type   URI\n1    commit https://x/1\n2    object https://x/2\n"
    );
}

#[test]
fn pick_tables_show_default_for_empty_tokens() {
    let refs = vec![pick_ref(Some("T7")), pick_ref(None)];
    let resolved = vec![
        pick_hits(&[("u1", "commit"), ("u2", "object")]),
        pick_hits(&[("u3", "commit"), ("u4", "object")]),
    ];
    assert_eq!(
        render_pick_table(&refs, &resolved, false, false),
        "Argument  Type   URI\n\
         T7        commit u1\n\
         T7        object u2\n\
         <default> commit u3\n\
         <default> object u4\n"
    );
}

#[test]
fn pick_tables_escape_values_after_padding() {
    // Widths span raw values; escaping happens afterwards, so control
    // characters widen their cell exactly like PHP's `drawConsoleString`.
    let refs = vec![pick_ref(Some("HEAD"))];
    let resolved = vec![pick_hits(&[
        ("https://x/\u{1}", "commit"),
        ("https://x/2", "object"),
    ])];
    assert_eq!(
        render_pick_table(&refs, &resolved, true, false),
        "Pick Type   URI\n1    commit https://x/<0x01>\n2    object https://x/2\n"
    );
}

#[test]
fn renders_unresolved_reports() {
    let refs = vec![pick_ref(Some("nope")), pick_ref(None)];
    let resolved = vec![pick_hits(&[]), pick_hits(&[])];
    assert_eq!(
        render_unresolved(&refs, &resolved, false),
        "Unable to resolve argument \"nope\".\nUnable to resolve default browse target.\n"
    );
}

#[test]
fn unresolved_reports_escape_control_characters() {
    let refs = vec![pick_ref(Some("a\u{7}b"))];
    let resolved = vec![pick_hits(&[])];
    assert_eq!(
        render_unresolved(&refs, &resolved, false),
        "Unable to resolve argument \"a<BEL>b\".\n"
    );
}

#[test]
fn wraps_select_prompts_at_indent_four() {
    assert_eq!(
        select_prompt("Which URI do you want to open?", 1, 12),
        "    Which URI do you want to open? [1 - 12] "
    );
    assert_eq!(select_prompt("Pick?", 1, 3), "    Pick? [1 - 3] ");
}

#[test]
fn r9_browse_b24_no_browser_text_matches_ceb() {
    // `run_prepared` prints the shared scan error when no browser command
    // resolves; it must be the ceb rendering (PHP
    // `ArcanistWorkflow.php:2204-2206` @ ceb082ef), not the newer-baseline
    // `"browser"` / `your configuration` wording.
    assert_eq!(
        crate::diff::NO_BROWSER_MESSAGE,
        "Unable to find a browser command to run. Set 'browser' in your Arcanist config to specify a command to use."
    );
}

#[test]
fn r4_browseb22_parse_error_exits_1() {
    // Toolset parser errors render `USAGE EXCEPTION` with exit 1
    // and no usage dump.
    let dir = std::env::temp_dir();
    let mut input: &[u8] = &[];
    assert_eq!(
        run_in_with(&dir, &argv(&["--bogus"]), None, &mut input),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn r3_browseb7_revision_search_follows_after_cursor() {
    fn row(id: u32) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "fields": {
                "uri": format!("/D{id}"),
                "status": {"closed": false},
            },
        })
    }
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "differential.revision.search");
        seen.borrow_mut().push(params.clone());
        if seen.borrow().len() == 1 {
            Ok(serde_json::json!({"data": [row(1)], "cursor": {"after": "p2"}}))
        } else {
            Ok(serde_json::json!({"data": [row(2)]}))
        }
    };
    let hits = search_revision_hits(&call, &["1".to_string(), "2".to_string()]).unwrap();
    assert_eq!(
        hits,
        vec![
            ("1".to_string(), "/D1".to_string(), false),
            ("2".to_string(), "/D2".to_string(), false),
        ]
    );
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].get("after").is_none());
    assert_eq!(seen[1]["after"], serde_json::json!("p2"));
}

// -( R5 TTY gating: BROWSE-B3 )------------------------------------------------

#[test]
fn r5_browse_b3_non_tty_throws_after_echo() {
    // `phutil_console_select()` funnels through
    // `phutil_console_prompt()`, which echoes, then `require_tty()`
    // throws (`src/console/format.php:26-52`).
    let mut piped = std::io::Cursor::new(b"2\n");
    assert_eq!(
        console_select_with_tty(&mut piped, "Pick?", 1, 3, false).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
}

#[test]
fn r4_browseb20_workflow_error_is_fatal() {
    // `browse` runs in the toolset runtime: workflow errors escape
    // into a fatal (255). An unresolvable target with an injected
    // failing caller exercises the fatal arm without network.
    let dir = std::env::temp_dir();
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("boom".to_string())
    };
    let mut input: &[u8] = &[];
    let options = options_of(&["T999"]);
    assert_eq!(
        run_prepared(&dir, &options, &call, "", &mut input),
        std::process::ExitCode::from(255)
    );
}

// -( T1153 Wave 2: browse retrocompat )---------------------------------------

#[test]
fn browse_b1_force_yields_to_explicit_types() {
    // `--types commit --force` resolves the commit kind (PHP
    // `ArcanistBrowseWorkflow.php:88-97`), so the missing
    // `diffusion.querycommits` route fails loudly (fatal); bare
    // `--force` still resolves the path kind (exit 0).
    let tmp = repo_fixture("forcewins");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        serde_json::json!({
            "repository.callsign": "X",
            "browser": null_browser(),
        })
        .to_string(),
    )
    .unwrap();
    let call = mock(repo_routes());
    let mut input: &[u8] = &[];
    assert_eq!(
        run_in_with(
            &tmp.path,
            &argv(&["--types", "commit", "--force", "--", "HEAD"]),
            Some(&call),
            &mut input
        ),
        std::process::ExitCode::from(255)
    );
    let mut input: &[u8] = &[];
    assert_eq!(
        run_in_with(
            &tmp.path,
            &argv(&["--force", "--", "HEAD"]),
            Some(&call),
            &mut input
        ),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn browse_b2_default_token_decided_per_ref() {
    // PHP reuses the last warning-loop `$token` for every row (a
    // leftover loop variable, kept like LAND-41); Rust decides per
    // ref. Reversed order pins it: stale state would show `T7` on
    // all four rows.
    let refs = vec![pick_ref(None), pick_ref(Some("T7"))];
    let resolved = vec![
        pick_hits(&[("u1", "commit"), ("u2", "object")]),
        pick_hits(&[("u3", "commit"), ("u4", "object")]),
    ];
    assert_eq!(
        render_pick_table(&refs, &resolved, false, false),
        "Argument  Type   URI\n\
         <default> commit u1\n\
         <default> object u2\n\
         T7        commit u3\n\
         T7        object u4\n"
    );
}

#[test]
fn browse_b4_eof_during_pick_aborts() {
    // PHP loops forever on EOF; Rust aborts (kept: PHP never
    // terminates, like the T1153 CORE-46/T1153 CORE-54 keep).
    let mut input = std::io::Cursor::new(b"");
    assert_eq!(
        console_select_with_tty(&mut input, "Pick?", 1, 3, true).unwrap_err(),
        ABORT
    );
}

#[test]
fn browse_b5_absolute_uri_drops_query_and_fragment() {
    assert_eq!(
        absolute_uri("https://phab.example/api/", "/D123?x=1#frag"),
        "https://phab.example/D123"
    );
    // Protocol-relative URIs already have a domain: kept whole.
    assert_eq!(
        absolute_uri("https://phab.example/api/", "//other.test/x?y=1"),
        "//other.test/x?y=1"
    );
    // Absolute URIs (query included) still pass through.
    assert_eq!(
        absolute_uri("https://phab.example/api/", "https://x.test/y?z"),
        "https://x.test/y?z"
    );
}

#[test]
fn browse_b6_legacy_hit_without_uri_falls_back_to_monogram() {
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        serde_json::json!([
            {"id": 12, "statusName": "Needs Review", "hashes": [["gtcm", "abc"]]},
            {"id": "13", "statusName": "Accepted", "hashes": [["gtcm", "def"]]},
        ]),
    );
    let call = mock(routes);
    let pairs = vec![
        vec!["gtcm".to_string(), "abc".to_string()],
        vec!["gtcm".to_string(), "def".to_string()],
    ];
    let hits = hash_revision_hits(&call, &pairs).unwrap();
    assert_eq!(hits.get("abc").unwrap(), &vec![("/D12".to_string(), false)]);
    // String ids coerce like PHP's `(int)` cast.
    assert_eq!(hits.get("def").unwrap(), &vec![("/D13".to_string(), false)]);
}

#[test]
fn browse_b8_top_level_default_branch_ignored() {
    // PHP reads only `fields.defaultBranch`.
    assert_eq!(
        default_branch_of(&serde_json::json!({"defaultBranch": "top"})),
        "master"
    );
    assert_eq!(
        default_branch_of(&serde_json::json!({"fields": {"defaultBranch": "dev"}})),
        "dev"
    );
    assert_eq!(default_branch_of(&serde_json::json!({})), "master");
}

#[test]
fn browse_b9_lines_zero_dropped() {
    let repo = RepoInfo {
        phid: "PHID-REPO-1".to_string(),
        browse_uri: "https://phab.example/diffusion/X/".to_string(),
        default_branch: "master".to_string(),
    };
    // PHP's `if ($params['lines'])` drops `'0'` (falsy); any other
    // string, including `'00'`, stays.
    assert_eq!(
        new_browse_uri(&repo, "f", Some("0"), None),
        "https://phab.example/diffusion/X/browse/master/f"
    );
    assert_eq!(
        new_browse_uri(&repo, "f", Some("00"), None),
        "https://phab.example/diffusion/X/browse/master/f$00"
    );
    assert_eq!(
        new_browse_uri(&repo, "f", Some("12"), None),
        "https://phab.example/diffusion/X/browse/master/f$12"
    );
}

#[test]
fn browse_b10_null_token_head_failure_throws() {
    let tmp = TempDir::new("nohead");
    git(&tmp.path, &["init", "-q", "-b", "master"]);
    // A missing token resolves `HEAD` outside the per-token
    // try/catch, so an unresolvable `HEAD` throws.
    let rf = pick_ref(None);
    assert_eq!(
        commits_for_one_ref(&tmp.path, &rf).unwrap_err(),
        "Unable to find any git commit named 'HEAD' in this repository."
    );
    // Explicit bad tokens still resolve to nothing, and path-only
    // refs never touch `HEAD` (the path query wants no commitrefs).
    let rf = pick_ref(Some("no-such-rev"));
    assert!(commits_for_one_ref(&tmp.path, &rf).unwrap().is_empty());
    let rf = BrowseRef {
        token: None,
        types: vec![BROWSE_PATH.to_string()],
        branch: None,
    };
    assert!(commits_for_one_ref(&tmp.path, &rf).unwrap().is_empty());
}

#[test]
fn browse_b11_remote_uri_applies_insteadof() {
    let tmp = fixture_repo("instead");
    git(&tmp.path, &["remote", "add", "origin", "orig:X.git"]);
    git(
        &tmp.path,
        &["config", "url.git@example.test:.insteadOf", "orig:"],
    );
    // `git remote get-url` rewrites; `git config` would echo `orig:`.
    assert_eq!(
        remote_uri(&tmp.path).as_deref(),
        Some("git@example.test:X.git")
    );
}

#[test]
#[cfg(unix)]
fn browse_b12_leaf_symlink_stays_unresolved() {
    let tmp = TempDir::new("leaf");
    std::fs::write(tmp.path.join("target.txt"), "x").unwrap();
    std::os::unix::fs::symlink("target.txt", tmp.path.join("link.txt")).unwrap();
    let link = tmp.path.join("link.txt");
    let resolved = canonicalize_lax(&link);
    // The parent resolves but the leaf name survives, like PHP's
    // `resolvePath()`; resolving through would end at `target.txt`.
    assert_eq!(resolved.file_name().unwrap(), "link.txt");
    assert_eq!(
        resolved.parent().unwrap(),
        tmp.path.canonicalize().unwrap().as_path()
    );
}

#[test]
#[cfg(unix)]
fn browse_b13_broken_symlink_counts_as_existing() {
    let tmp = TempDir::new("broken");
    std::fs::write(tmp.path.join("real.txt"), "x").unwrap();
    std::os::unix::fs::symlink("missing.txt", tmp.path.join("link.txt")).unwrap();
    assert!(path_exists(&tmp.path.join("real.txt")));
    assert!(path_exists(&tmp.path.join("link.txt")));
    assert!(!path_exists(&tmp.path.join("missing.txt")));
}

#[test]
fn browse_b14_status_lines_use_writewarn_format() {
    // `push_status` routes through `format_status_message`
    // (`writeWarn`/`writeInfo`), not the log engine: bold nests
    // inside the background color, and values pass through raw.
    let status = terminal::format_status_message("yellow", "AMBIGUOUS", "m", true);
    assert!(status.contains("\x1b[43m\x1b[1m"), "{status:?}");
    let log = terminal::format_log("yellow", "AMBIGUOUS", "m", true);
    assert!(log.contains("\x1b[1m\x1b[43m"), "{log:?}");
    let raw = terminal::format_status_message("blue", "L", "a\x01b", false);
    assert!(raw.contains("a\x01b"), "{raw:?}");
}

#[test]
fn browse_b15_wide_chars_pad_by_columns() {
    // `日本` spans 4 console columns (2 chars); `x` pads to 4, not 2.
    let out = render_table(
        &["K", "V"],
        &[
            vec!["日本".to_string(), "1".to_string()],
            vec!["x".to_string(), "2".to_string()],
        ],
    );
    assert_eq!(out, "K    V\n日本 1\nx    2\n");
}

#[test]
fn browse_b16_kind_order_is_fixed() {
    // PHP follows future-resolution order (nondeterministic); Rust
    // resolves commit/object/path/revision (kept: one valid order,
    // deterministic). A token that is a branch, a file, and an
    // object exercises all four kinds at once.
    let tmp = repo_fixture("kindorder");
    std::fs::write(tmp.path.join("myfile"), "x\n").unwrap();
    git(&tmp.path, &["add", "myfile"]);
    git(&tmp.path, &["commit", "-qm", "add myfile"]);
    git(&tmp.path, &["branch", "myfile"]);
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let mut routes = repo_routes();
    routes.insert(
        "diffusion.querycommits".to_string(),
        serde_json::json!({
            "identifierMap": {head.clone(): "PHID-CMT-1"},
            "data": {"PHID-CMT-1": {"uri": "/rXabc"}},
        }),
    );
    routes.insert(
        "phid.lookup".to_string(),
        serde_json::json!({"myfile": {"uri": "https://phab.example/object/myfile"}}),
    );
    routes.insert(
        "differential.query".to_string(),
        serde_json::json!([{
            "id": 7,
            "statusName": "Accepted",
            "uri": "https://phab.example/D7",
            "hashes": [["gtcm", head]],
        }]),
    );
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![pick_ref(Some("myfile"))];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    let kinds: Vec<&str> = resolved[0].uris.iter().map(|u| u.kind.as_str()).collect();
    assert_eq!(kinds, vec!["commit", "object", "path", "revision"]);
}

#[test]
fn browse_b17_querycommits_names_deduped() {
    // Two tokens resolving to one hash send one name, like PHP's
    // `array_keys($commit_map)`.
    let tmp = repo_fixture("dedupe");
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let routes = repo_routes();
    let call = |method: &str, params: &serde_json::Value| {
        if method == "diffusion.querycommits" {
            seen.borrow_mut().push(params.clone());
            return Ok(serde_json::json!({
                "identifierMap": {head.clone(): "PHID-CMT-1"},
                "data": {"PHID-CMT-1": {"uri": "/rXabc"}},
            }));
        }
        routes
            .get(method)
            .cloned()
            .ok_or_else(|| format!("unexpected call {method}"))
    };
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![
        BrowseRef {
            token: Some("HEAD".to_string()),
            types: vec![BROWSE_COMMIT.to_string()],
            branch: None,
        },
        BrowseRef {
            token: Some("master".to_string()),
            types: vec![BROWSE_COMMIT.to_string()],
            branch: None,
        },
    ];
    resolve_refs(&mut ctx, &refs).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["names"], serde_json::json!([head]));
}

#[test]
fn browse_b18_hash_match_compares_type_and_value() {
    // PHP keys hashes as `type:value`; a tree hash never matches a
    // commit-hash pair.
    let mut routes = HashMap::new();
    routes.insert(
        "differential.query".to_string(),
        serde_json::json!([
            {"id": 1, "statusName": "Accepted", "uri": "u-tree",
             "hashes": [["gttr", "abc"]]},
            {"id": 2, "statusName": "Accepted", "uri": "u-commit",
             "hashes": [["gtcm", "abc"]]},
        ]),
    );
    let call = mock(routes);
    let pairs = vec![vec!["gtcm".to_string(), "abc".to_string()]];
    let hits = hash_revision_hits(&call, &pairs).unwrap();
    assert_eq!(
        hits.get("abc").unwrap(),
        &vec![("u-commit".to_string(), false)]
    );
}

#[test]
fn browse_b21_terminator_required_when_piped() {
    assert!(crate::help::terminator_missing(&argv(&["HEAD"]), false));
    assert!(!crate::help::terminator_missing(
        &argv(&["--", "HEAD"]),
        false
    ));
    assert!(!crate::help::terminator_missing(&argv(&["HEAD"]), true));
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
    // The failure renders toolset usage (exit 1); `--` proceeds.
    assert_eq!(
        terminator_exit(&argv(&["HEAD"]), false),
        Some(std::process::ExitCode::FAILURE)
    );
    assert_eq!(terminator_exit(&argv(&["--", "HEAD"]), false), None);
    assert_eq!(terminator_exit(&argv(&["HEAD"]), true), None);
}

#[test]
fn browse_b23_help_matches_generated_format() {
    assert_eq!(
        help_text(false),
        "BROWSE WORKFLOW\n\
         \n\
         browse [options] -- target ...\n\
         browse -- file-name\n\
         browse -- object-name\n\
         Open a file or object in a local web browser.\n\
         \n\
         Open a file or object (like a task or revision) in a local web browser.\n\
         \n\
         \x20 $ arc browse README   # Open a file in Diffusion.\n\
         \x20 $ arc browse T123     # View a task.\n\
         \x20 $ arc browse HEAD     # View a symbolic commit.\n\
         \n\
         To choose a browser binary to invoke, use:\n\
         \n\
         \x20 $ arc set-config browser browser-binary\n\
         \n\
         If no browser is set, the command will try to guess which browser to use.\n\
         \n\
         \x20     --branch branch-name\n\
         \x20         Default branch name to view on server. Defaults to \"master\".\n\
         \n\
         \x20     --force\n\
         \x20         (DEPRECATED) Obsolete, use \"--types path\" instead.\n\
         \n\
         \x20     --types type-list\n\
         \x20         Force targets to be interpreted as naming particular types of\n\
         \x20         resources.\n\
         \n\
         \n"
    );
    // ANSI mode carries bold/underline codes.
    let ansi = help_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `--help` still exits zero once `--` satisfies the terminator.
    let dir = std::env::temp_dir();
    let mut input: &[u8] = &[];
    assert_eq!(
        run_in_with(&dir, &argv(&["--help", "--"]), None, &mut input),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn browse_b26_phid_lookup_keeps_ref_order() {
    // PHP sends `$token_set` in ref order; the old `BTreeMap` sorted
    // the keys (here `T9` must precede `T7` on the wire).
    let tmp = fixture_repo("phidorder");
    let seen: std::cell::RefCell<Vec<serde_json::Value>> = std::cell::RefCell::new(Vec::new());
    let call = |method: &str, params: &serde_json::Value| {
        assert_eq!(method, "phid.lookup");
        seen.borrow_mut().push(params.clone());
        Ok(serde_json::json!({
            "T9": {"uri": "https://phab.example/T9"},
            "T7": {"uri": "https://phab.example/T7"},
        }))
    };
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![
        BrowseRef {
            token: Some("T9".to_string()),
            types: vec![BROWSE_OBJECT.to_string()],
            branch: None,
        },
        BrowseRef {
            token: Some("T7".to_string()),
            types: vec![BROWSE_OBJECT.to_string()],
            branch: None,
        },
    ];
    resolve_refs(&mut ctx, &refs).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        serde_json::to_string(&seen[0]["names"]).unwrap(),
        r#"{"T9":"T9","T7":"T7"}"#
    );
}

// -( T1170 coverage: query helpers, remotes, paths )--------------------------

#[test]
fn query_rows_falls_back_to_empty_for_scalars() {
    assert!(query_rows(&serde_json::Value::Null).is_empty());
    assert!(query_rows(&serde_json::json!(7)).is_empty());
    assert!(query_rows(&serde_json::json!("x")).is_empty());
    assert_eq!(query_rows(&serde_json::json!([1])).len(), 1);
    assert_eq!(query_rows(&serde_json::json!({"a": 1})).len(), 1);
}

#[test]
fn upstream_remote_prefers_longest_matching_remote() {
    // A tracking branch resolves through `@{upstream}`; the remote is
    // the longest remote name matching the upstream's first segment.
    let tmp = TempDir::new("upstream");
    let bare = tmp.path.join("bare.git");
    git(&tmp.path, &["init", "--bare", "-q", "bare.git"]);
    let work = tmp.path.join("w");
    std::fs::create_dir(&work).unwrap();
    git(&work, &["init", "-q", "-b", "master"]);
    git(&work, &["config", "user.email", "t@t"]);
    git(&work, &["config", "user.name", "t"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    std::fs::write(work.join("f.txt"), "one\n").unwrap();
    git(&work, &["add", "f.txt"]);
    git(&work, &["commit", "-qm", "add f"]);
    git(&work, &["remote", "add", "origin", &bare.to_string_lossy()]);
    git(&work, &["push", "-q", "-u", "origin", "master"]);
    assert_eq!(upstream_remote(&work).as_deref(), Some("origin"));
    let uri = remote_uri(&work).unwrap();
    assert!(uri.contains("bare"), "{uri}");
    // Without an upstream there is no remote to resolve.
    let bare_repo = fixture_repo("noupstream");
    assert!(upstream_remote(&bare_repo.path).is_none());
}

#[test]
fn remote_uri_falls_back_to_ls_remote_via_insteadof() {
    // `git remote get-url` fails for unknown remotes while `ls-remote
    // --get-url` still expands `insteadOf`: the fallback arm answers.
    // (`ls-remote --get-url` prints without any network traffic.)
    let tmp = fixture_repo("insteadof");
    git(
        &tmp.path,
        &[
            "config",
            "url.https://phab.example/rewritten.insteadOf",
            "origin",
        ],
    );
    assert_eq!(
        remote_uri(&tmp.path).as_deref(),
        Some("https://phab.example/rewritten")
    );
}

#[test]
fn configured_callsign_rejects_empty_and_non_string() {
    let tmp = fixture_repo("callsignguard");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"repository.callsign": ""}"#,
    )
    .unwrap();
    assert!(configured_callsign(&tmp.path).is_none());
    std::fs::write(tmp.path.join(".arcconfig"), r#"{"repository.callsign": 7}"#).unwrap();
    assert!(configured_callsign(&tmp.path).is_none());
}

#[test]
fn repo_info_rejects_rows_without_phid_or_uri() {
    for row in [
        serde_json::json!({"uri": "https://phab.example/diffusion/X/"}),
        serde_json::json!({"phid": "PHID-REPO-1"}),
    ] {
        let tmp = repo_fixture("badrow");
        let mut routes = HashMap::new();
        routes.insert("repository.query".to_string(), serde_json::json!([row]));
        let call = mock(routes);
        assert!(resolve_repo(&tmp.path, &call).unwrap().is_none());
    }
}

#[test]
fn resolve_path_handles_absolute_and_parent_segments() {
    let tmp = fixture_repo("resolvepath");
    let abs = tmp.path.join("sub").join("f.txt");
    assert_eq!(resolve_path(&tmp.path, &abs.to_string_lossy()), abs);
    assert_eq!(
        resolve_path(&tmp.path, "a/../b"),
        tmp.path.join("b"),
        "parent segments pop lexically"
    );
}

#[test]
fn canonicalize_lax_climbs_missing_ancestors() {
    // Nothing on this chain exists, so every ancestor lookup fails and
    // the lexical path survives.
    let missing = Path::new("definitely-missing-t1170-a").join("definitely-missing-t1170-b");
    assert_eq!(canonicalize_lax(&missing), missing);
}

// -( T1170 coverage: default targets, footers, id shapes )--------------------

#[test]
fn path_uri_resolves_default_target_at_repo_root() {
    // A missing token on a path-typed ref browses "." (the root itself,
    // which canonicalizes equal to the toplevel).
    let tmp = repo_fixture("rootdefault");
    let call = mock(repo_routes());
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![BrowseRef {
        token: None,
        types: vec![BROWSE_PATH.to_string()],
        branch: None,
    }];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert!(resolved[0].commits.is_empty());
    assert_eq!(resolved[0].uris.len(), 1);
    assert_eq!(
        resolved[0].uris[0].uri,
        "https://phab.example/diffusion/X/browse/master/"
    );
}

#[test]
fn untyped_default_target_resolves_nothing() {
    // Untyped with no token: HEAD commits resolve, but there is no
    // object token to look up and no path to browse.
    let tmp = repo_fixture("emptydefault");
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let mut routes = repo_routes();
    routes.insert("diffusion.querycommits".to_string(), serde_json::json!({}));
    routes.insert("differential.query".to_string(), serde_json::json!([]));
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![BrowseRef {
        token: None,
        types: Vec::new(),
        branch: None,
    }];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert_eq!(resolved[0].commits, vec![head]);
    assert!(resolved[0].uris.is_empty());
}

#[test]
fn hash_revision_hits_short_circuits_empty_input() {
    let call = mock(HashMap::new());
    assert!(hash_revision_hits(&call, &[]).unwrap().is_empty());
}

#[test]
fn footer_ids_skips_unreadable_commits_and_finds_footers() {
    let tmp = fixture_repo("footers");
    git(
        &tmp.path,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "second",
            "-m",
            "Differential Revision: D5",
        ],
    );
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let by_id = footer_ids(&tmp.path, &["bogus-hash-t1170".to_string(), head]);
    assert_eq!(by_id, BTreeMap::from([("5".to_string(), vec![1])]));
}

#[test]
fn id_param_and_row_helpers_cover_scalar_shapes() {
    assert_eq!(id_param("7"), serde_json::json!(7));
    assert_eq!(id_param("D7"), serde_json::json!("D7"));
    assert_eq!(row_id_string(&serde_json::json!({"id": "D7"})), "D7");
    assert_eq!(row_id_string(&serde_json::json!({"id": -3})), "-3");
    assert_eq!(row_id_string(&serde_json::json!({"id": 1.5})), "");
    assert_eq!(row_id_string(&serde_json::json!({})), "");
    assert!(!row_closed(&serde_json::json!({})));
    assert!(!row_closed(&serde_json::json!({"fields": {}})));
    assert!(!row_closed(&serde_json::json!({"fields": {"status": {}}})));
    assert!(row_closed(
        &serde_json::json!({"fields": {"status": {"closed": true}}})
    ));
}

#[test]
fn revision_search_tolerates_bare_rows_and_missing_data() {
    // Rows without `fields` fall back to `/D{id}`; pages without `data`
    // contribute no rows.
    let bare = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "differential.revision.search");
        Ok(serde_json::json!({"data": [{"id": 3}]}))
    };
    assert_eq!(
        search_revision_hits(&bare, &["3".to_string()]).unwrap(),
        vec![("3".to_string(), "/D3".to_string(), false)]
    );
    let dataless = |method: &str, _: &serde_json::Value| {
        assert_eq!(method, "differential.revision.search");
        Ok(serde_json::json!({"cursor": {}}))
    };
    assert!(
        search_revision_hits(&dataless, &["3".to_string()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn revision_uris_skips_closed_and_unknown_ids() {
    let tmp = repo_fixture("revuris");
    git(
        &tmp.path,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "second",
            "-m",
            "Differential Revision: D5",
        ],
    );
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let mut routes = HashMap::new();
    routes.insert("differential.query".to_string(), serde_json::json!([]));
    routes.insert(
        "differential.revision.search".to_string(),
        serde_json::json!({"data": [
            {"id": 9, "fields": {"uri": "https://phab.example/D9", "status": {"closed": true}}},
            {"id": 10, "fields": {"uri": "https://phab.example/D10", "status": {"closed": false}}},
            {"id": 5, "fields": {"uri": "https://phab.example/D5", "status": {"closed": false}}},
        ]}),
    );
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let uris = revision_uris(&mut ctx, std::slice::from_ref(&head)).unwrap();
    assert_eq!(
        uris,
        BTreeMap::from([(head, vec!["https://phab.example/D5".to_string()])])
    );
}

#[test]
fn commits_for_untyped_default_resolves_head() {
    let tmp = fixture_repo("defaulthead");
    let head = git(&tmp.path, &["rev-parse", "HEAD"]);
    let rf = BrowseRef {
        token: None,
        types: Vec::new(),
        branch: None,
    };
    assert_eq!(commits_for_one_ref(&tmp.path, &rf).unwrap(), vec![head]);
}

#[test]
fn explicit_non_path_type_skips_path_resolution() {
    let tmp = repo_fixture("skippath");
    let call = mock(repo_routes());
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![
        BrowseRef {
            token: Some("f.txt".to_string()),
            types: vec![BROWSE_PATH.to_string()],
            branch: None,
        },
        BrowseRef {
            token: Some("f.txt".to_string()),
            types: vec![BROWSE_COMMIT.to_string()],
            branch: None,
        },
    ];
    let resolved = resolve_refs(&mut ctx, &refs).unwrap();
    assert_eq!(resolved[0].uris.len(), 1);
    assert!(resolved[1].uris.is_empty());
}

// -( T1170 coverage: tables, prompts, run paths )------------------------------

#[test]
fn render_pick_table_skips_single_uri_refs() {
    let refs = vec![pick_ref(Some("HEAD"))];
    let resolved = vec![pick_hits(&[("https://x/1", "commit")])];
    assert_eq!(
        render_pick_table(&refs, &resolved, true, false),
        "Pick Type URI\n"
    );
}

#[test]
fn console_select_uses_real_stdin_state() {
    // The wrapper passes the ambient TTY state through: piped runs get
    // the non-interactive error, interactive runs read the pick.
    let outcome = console_select(&mut std::io::Cursor::new(b"2\n"), "Pick?", 1, 3);
    assert!(
        outcome == Ok(2) || outcome == Err(crate::prompt::NOT_INTERACTIVE.to_string()),
        "{outcome:?}"
    );
}

#[test]
fn absolute_uri_keeps_schemeless_bases_relative() {
    assert_eq!(
        absolute_uri("phab.example/api", "/D1"),
        "phab.example/api/D1"
    );
    assert_eq!(
        absolute_uri("https://phab.example/api/", "/D1?x=1#y"),
        "https://phab.example/D1"
    );
}

#[test]
fn disambiguate_lists_table_for_many_refs() {
    // Several refs: the table prints and the caller picks via `--types`
    // (no prompt); a missing token names the default behavior.
    let refs = vec![pick_ref(None), pick_ref(Some("x"))];
    let resolved = vec![
        pick_hits(&[("u1", "commit"), ("u2", "object")]),
        pick_hits(&[("u3", "commit")]),
    ];
    let mut input: &[u8] = &[];
    assert_eq!(
        disambiguate_with_tty(&refs, &resolved, &mut input, false, false).unwrap(),
        Vec::new()
    );
}

#[test]
fn execute_refs_propagates_ambiguous_pick_failure() {
    // One ref resolving two URIs needs a pick; piped runs fail with
    // the non-interactive error through the `?` arm.
    let tmp = repo_fixture("ambigpick");
    let mut routes = repo_routes();
    routes.insert(
        "phid.lookup".to_string(),
        serde_json::json!({"f.txt": {"uri": "https://phab.example/F1"}}),
    );
    let call = mock(routes);
    let mut ctx = BrowseCtx::new(&tmp.path, &call, "https://phab.example/api/");
    let refs = vec![BrowseRef {
        token: Some("f.txt".to_string()),
        types: Vec::new(),
        branch: None,
    }];
    let mut input: &[u8] = &[];
    assert_eq!(
        execute_refs_with_tty(&mut ctx, &refs, &mut input, false, false).unwrap_err(),
        crate::prompt::NOT_INTERACTIVE
    );
}

#[test]
fn build_refs_defaults_to_bare_target() {
    assert_eq!(
        build_refs(&options_of(&[])),
        vec![BrowseRef {
            token: None,
            types: Vec::new(),
            branch: None,
        }]
    );
}

#[test]
fn run_without_force_skips_deprecation() {
    // No `--force`: past the deprecation gate to the injected caller;
    // outside a repository nothing resolves and there is nothing to open.
    let dir = TempDir::new("noforce");
    let call = mock(HashMap::new());
    let mut input: &[u8] = &[];
    assert_eq!(
        run_in_with(
            &dir.path,
            &argv(&["--types", "path", "--"]),
            Some(&call),
            &mut input
        ),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn run_without_caller_fails_before_browser() {
    // No injected caller: a real client is built (missing credentials
    // fail fast), and outside a repository there is no HEAD to resolve,
    // so both arms fail before any conduit traffic or browser launch.
    let dir = TempDir::new("nocaller");
    let mut input: &[u8] = &[];
    assert_eq!(
        run_in_with(&dir.path, &argv(&["--"]), None, &mut input),
        std::process::ExitCode::from(255)
    );
}

#[test]
fn run_prepared_reports_unopenable_browser() {
    let tmp = repo_fixture("badbrowser");
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"repository.callsign": "X", "browser": "definitely-not-a-browser-t1170"}"#,
    )
    .unwrap();
    let mut routes = repo_routes();
    routes.insert("phid.lookup".to_string(), serde_json::json!({}));
    let call = mock(routes);
    let mut input: &[u8] = &[];
    assert_eq!(
        run_prepared(&tmp.path, &options_of(&["f.txt"]), &call, "", &mut input),
        std::process::ExitCode::from(255)
    );
}

#[test]
fn run_dispatches_through_real_cwd() {
    assert_eq!(run(&argv(&["--bogus"])), std::process::ExitCode::FAILURE);
}

#[test]
fn run_in_with_enforces_terminator_when_piped() {
    // Piped runs without `--` fail at the terminator gate (both the
    // help and options arms); interactive runs proceed instead. The
    // expectations stay branchless so neither stdin state leaves a
    // dead arm behind.
    use std::process::ExitCode;
    let piped = !crate::prompt::stdin_is_terminal();
    let dir = TempDir::new("terminator");
    let call = mock(HashMap::new());
    let mut input: &[u8] = &[];
    let help = run_in_with(&dir.path, &argv(&["--help"]), Some(&call), &mut input);
    let help_want = [ExitCode::SUCCESS, ExitCode::FAILURE][usize::from(piped)];
    assert_eq!(help, help_want);
    let options = run_in_with(&dir.path, &argv(&["f.txt"]), Some(&call), &mut input);
    let options_want = [ExitCode::from(255), ExitCode::FAILURE][usize::from(piped)];
    assert_eq!(options, options_want);
}
