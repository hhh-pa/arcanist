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
    let mut input = std::io::Cursor::new(b"nope\n99\n2\n");
    assert_eq!(console_select(&mut input, "Pick?", 1, 3).unwrap(), 2);
    let mut eof = std::io::Cursor::new(b"");
    assert_eq!(console_select(&mut eof, "Pick?", 1, 3).unwrap_err(), ABORT);
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

    // Outside repositories there is nothing to resolve (no conduit call).
    let bare = TempDir::new("norepo");
    let never = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        panic!("must not call conduit outside a repo")
    };
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
    let uris = execute_refs(&mut ctx, &refs, &mut input, false).unwrap();
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
fn r4_browseb20_workflow_error_is_fatal() {
    // `browse` runs in the toolset runtime: workflow errors escape
    // into a fatal (255). An unresolvable target with an injected
    // failing caller exercises the fatal arm without network.
    let dir = std::env::temp_dir();
    let call = |_: &str, _: &serde_json::Value| -> Result<serde_json::Value, String> {
        Err("boom".to_string())
    };
    let mut input: &[u8] = &[];
    let options = match parse_args(&argv(&["T999"])) {
        Parsed::Options(options) => options,
        _ => panic!("expected options"),
    };
    assert_eq!(
        run_prepared(&dir, &options, &call, "", &mut input),
        std::process::ExitCode::from(255)
    );
}
