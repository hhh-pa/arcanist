//! `diff` against local git fixtures through the wired library.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use arc_rs::diff;

/* G1 full-state integration tests: hunk-carrying submits, full inspection. */

fn g1_repo(name: &str) -> (Fixture, String, String) {
    let fx = Fixture::named(name);
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    let first = fx.commit_all("First");
    fx.write("f.txt", "a\nB\nc\n");
    let second = fx.commit_all("Second title");
    fx.write("f.txt", "a\nB\nC\n");
    (fx, first, second)
}

#[test]
fn g1_full_submit_carries_hunks_over_seam() {
    let (fx, _first, second) = g1_repo("g1-full-hunks");
    let opts = options(None, false);
    let input = diff::prepare_submit_full(fx.path(), &opts, &diff::SubmitMode::Only).unwrap();
    assert!(
        input.included_changes.starts_with("Included changes:"),
        "{}",
        input.included_changes
    );
    assert!(
        input.included_changes.contains("f.txt"),
        "{}",
        input.included_changes
    );

    let canned = Canned::default().with("differential.creatediff", wire_fixture("creatediff"));
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let report = diff::submit_over(&call, &input).unwrap();
    assert!(report.contains("Differential diff"), "{report}");

    let sent = canned.params_for("differential.creatediff");
    assert_eq!(sent.len(), 1);
    let changes = sent[0]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["currentPath"], serde_json::json!("f.txt"));
    let hunks = changes[0]["hunks"].as_array().unwrap();
    assert!(!hunks.is_empty(), "{}", sent[0]);
    assert!(
        hunks[0]["corpus"].as_str().unwrap().contains("@@"),
        "{}",
        sent[0]
    );
    assert_eq!(
        sent[0]["sourceControlBaseRevision"],
        serde_json::json!(second)
    );
}

#[test]
fn g1_full_submit_draft_create_fails_legacy() {
    let (fx, _, _) = g1_repo("g1-full-draft");
    let mut opts = options(None, false);
    opts.draft = true;
    let err = diff::prepare_submit_full(fx.path(), &opts, &diff::SubmitMode::Create).unwrap_err();
    assert!(err.contains("too old"), "{err}");
}

#[test]
fn g1_full_submit_update_requires_message() {
    let (fx, _, _) = g1_repo("g1-full-updmsg");
    let err = diff::prepare_submit_full(
        fx.path(),
        &options(None, false),
        &diff::SubmitMode::Update(5),
    )
    .unwrap_err();
    assert!(err.contains("requires --message"), "{err}");
}

#[test]
fn g1_full_submit_verbatim_update_sends_fields() {
    let (fx, first, _) = g1_repo("g1-full-verbatim");
    let mut opts = options(Some(&first), false);
    opts.verbatim = true;
    opts.message = Some("update notes".to_string());
    let input = diff::prepare_submit_full(fx.path(), &opts, &diff::SubmitMode::Update(5)).unwrap();
    assert_eq!(input.fields["title"], serde_json::json!("Second title"));
    assert_eq!(input.message.as_deref(), Some("update notes"));
    assert!(
        input.properties.iter().any(|p| p.name == "local:commits"),
        "{:?}",
        input.properties.iter().map(|p| &p.name).collect::<Vec<_>>()
    );

    // Without `--verbatim`, updates carry no local fields.
    let mut plain = options(Some(&first), false);
    plain.message = Some("update notes".to_string());
    let input = diff::prepare_submit_full(fx.path(), &plain, &diff::SubmitMode::Update(5)).unwrap();
    assert_eq!(input.fields, serde_json::json!({}));
}

#[test]
fn g1_full_submit_message_file_wins() {
    let (fx, _, _) = g1_repo("g1-full-msgfile");
    fx.write("msg.txt", "Filed title\n\nReviewers: alice\n");
    let mut opts = options(None, false);
    opts.message_file = Some(fx.path().join("msg.txt").to_string_lossy().to_string());
    let input = diff::prepare_submit_full(fx.path(), &opts, &diff::SubmitMode::Create).unwrap();
    assert_eq!(input.fields["title"], serde_json::json!("Filed title"));
    assert_eq!(input.fields["reviewerPHIDs"], serde_json::json!(["alice"]));
}

#[test]
fn g1_inspect_full_matches_inspect_for_git() {
    let (fx, first, _) = g1_repo("g1-full-inspect");
    let opts = options(Some(&first), false);
    let classic = diff::inspect(fx.path(), &opts).unwrap();
    let full = diff::inspect_full(fx.path(), &opts).unwrap();
    assert_eq!(full, classic);

    let json_opts = options(Some(&first), true);
    let classic: serde_json::Value =
        serde_json::from_slice(&diff::inspect(fx.path(), &json_opts).unwrap()).unwrap();
    let full: serde_json::Value =
        serde_json::from_slice(&diff::inspect_full(fx.path(), &json_opts).unwrap()).unwrap();
    assert_eq!(full, classic);
    assert_eq!(full["base"], serde_json::json!(first));
    assert!(!full["files"].as_array().unwrap().is_empty());
}

#[test]
fn g1_amend_worktree_message_git_rewrites() {
    let fx = Fixture::named("g1-amend");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("old subject");
    diff::amend_worktree_message_git(fx.path(), "new subject").unwrap();
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "new subject");
}

#[test]
fn g1_full_spec_input_git_shape() {
    let (fx, _, second) = g1_repo("g1-full-spec");
    let spec = diff::full_spec_input(fx.path(), Some(diff::VcsKind::Git), &second).unwrap();
    assert_eq!(spec.source_control_system.as_deref(), Some("git"));
    assert_eq!(spec.branch.as_deref(), Some("main"));
    assert_eq!(
        spec.source_control_base_revision.as_deref(),
        Some(second.as_str())
    );
}

/// Unique scratch dir for one test; removed on drop (best effort).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn named(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-diff-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo(&self) {
        self.git(&["init", "-q", "-b", "main", "."]);
        self.git(&["config", "user.email", "test@example.test"]);
        self.git(&["config", "user.name", "test"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["config", "core.autocrlf", "false"]);
    }

    fn write(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn commit_all(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Canned conduit seam: records calls, replays fixture responses, never
/// touches the network.
#[derive(Default)]
struct Canned {
    calls: RefCell<Vec<(String, serde_json::Value)>>,
    responses: HashMap<String, serde_json::Value>,
}

impl Canned {
    fn with(mut self, method: &str, response: &str) -> Self {
        self.responses
            .insert(method.to_string(), serde_json::from_str(response).unwrap());
        self
    }

    fn call(&self, method: &str, params: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.calls
            .borrow_mut()
            .push((method.to_string(), params.clone()));
        self.responses
            .get(method)
            .cloned()
            .ok_or_else(|| format!("no canned response for {method}"))
    }

    fn methods(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|(m, _)| m.clone()).collect()
    }

    fn params_for(&self, method: &str) -> Vec<serde_json::Value> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|(m, p)| if m == method { Some(p.clone()) } else { None })
            .collect()
    }
}

fn wire_fixture(name: &str) -> &'static str {
    match name {
        "creatediff" => include_str!("fixtures/wire_diff/creatediff.json"),
        "updaterevision" => include_str!("fixtures/wire_diff/updaterevision.json"),
        "createrevision" => include_str!("fixtures/wire_diff/createrevision.json"),
        "setdiffproperty" => include_str!("fixtures/wire_diff/setdiffproperty.json"),
        _ => panic!("unknown wire fixture {name}"),
    }
}

fn options(base: Option<&str>, json: bool) -> diff::Options {
    diff::Options {
        base: base.map(str::to_string),
        json,
        head: None,
        commit: None,
        message: None,
        message_file: None,
        reviewers: None,
        cc: None,
        update: None,
        create: false,
        only: false,
        conduit_uri: None,
        edit: false,
        raw: false,
        raw_command: None,
        draft: false,
        nounit: false,
        nolint: false,
        allow_untracked: false,
        apply_patches: false,
        never_apply_patches: false,
        amend_all: false,
        amend_autofixes: false,
        add_all: false,
        no_amend: false,
        uncommitted: false,
        verbatim: false,
        skip_binaries: false,
        skip_staging: false,
        coverage: false,
        no_coverage: false,
        browse: false,
        paths: Vec::new(),
        provided: Vec::new(),
    }
}

fn field_options() -> diff::Options {
    options(None, true)
}

#[test]
fn module_name_matches_subcommand() {
    assert_eq!(diff::NAME, "diff");
}

#[test]
fn errors_outside_git_repo() {
    let fx = Fixture::named("not-a-repo");
    assert!(!diff::is_inside_git_repo(fx.path()));
    let err = diff::inspect(fx.path(), &options(None, false)).unwrap_err();
    assert!(err.contains("not inside a git repository"), "{err}");
}

#[test]
fn errors_in_empty_repo_without_commits() {
    let fx = Fixture::named("empty-repo");
    fx.init_repo();
    let err = diff::inspect(fx.path(), &options(None, true)).unwrap_err();
    assert!(err.contains("HEAD"), "{err}");
}

#[test]
fn rejects_invalid_explicit_base() {
    let fx = Fixture::named("bad-base");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    let err = diff::inspect(fx.path(), &options(Some("does-not-exist"), true)).unwrap_err();
    assert!(err.contains("invalid base revision"), "{err}");
}

#[test]
fn run_rejects_bad_args_without_repo() {
    use std::process::ExitCode;
    let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(diff::run(&argv(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(diff::run(&argv(&["--base"])), ExitCode::FAILURE);
    assert_eq!(diff::run(&argv(&["stray-path"])), ExitCode::FAILURE);
    assert_eq!(
        diff::run(&argv(&["--create", "--update", "1"])),
        ExitCode::FAILURE
    );
    assert_eq!(
        diff::run(&argv(&["--only", "--message", "x"])),
        ExitCode::FAILURE
    );
}

#[test]
fn json_diff_against_explicit_base() {
    let fx = Fixture::named("json-base");
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    fx.write("g.txt", "x\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nB2\nB3\nc\n");
    fx.git(&["mv", "g.txt", "renamed.txt"]);

    let out = diff::inspect(fx.path(), &options(Some(&first), true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(first));
    assert_eq!(doc["insertions"], serde_json::json!(2));
    assert_eq!(doc["deletions"], serde_json::json!(1));
    let files = doc["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{doc}");
    assert!(
        files.contains(&serde_json::json!({
            "path": "f.txt",
            "added": 2,
            "deleted": 1,
        })),
        "{doc}"
    );
    assert!(
        files.contains(&serde_json::json!({
            "path": "renamed.txt",
            "added": 0,
            "deleted": 0,
        })),
        "{doc}"
    );
}

#[test]
fn default_output_has_stat_and_patch() {
    let fx = Fixture::named("default-out");
    fx.init_repo();
    fx.write("f.txt", "a\nb\nc\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nB2\nB3\nc\n");

    let out = diff::inspect(fx.path(), &options(None, false)).unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("f.txt"), "{text}");
    assert!(text.contains("diff --git"), "{text}");
    assert!(
        text.contains("files changed") || text.contains("file changed"),
        "{text}"
    );
}

#[test]
fn default_base_is_head_without_upstream() {
    let fx = Fixture::named("head-base");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    assert_eq!(diff::resolve_base(fx.path(), None).unwrap(), head);
    let out = diff::inspect(fx.path(), &options(None, true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(head));
    assert_eq!(doc["insertions"], serde_json::json!(1));
}

#[test]
fn default_base_prefers_upstream_merge_base() {
    let fx = Fixture::named("upstream-base");
    fx.init_repo();
    fx.write("f.txt", "one\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "one\ntwo\n");
    fx.commit_all("two");
    // Local branch used as the configured upstream (no network involved).
    fx.git(&["branch", "frozen", &first]);
    fx.git(&["branch", "--set-upstream-to=frozen"]);

    assert_eq!(diff::resolve_base(fx.path(), None).unwrap(), first);
    let out = diff::inspect(fx.path(), &options(None, true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(first));
    assert_eq!(doc["insertions"], serde_json::json!(1));
}

#[test]
fn json_diff_is_empty_on_clean_tree() {
    let fx = Fixture::named("clean-tree");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("one");

    let out = diff::inspect(fx.path(), &options(None, true)).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(head));
    assert_eq!(doc["files"], serde_json::json!([]));
    assert_eq!(doc["insertions"], serde_json::json!(0));
    assert_eq!(doc["deletions"], serde_json::json!(0));
    // No field flags: no `fields` key, byte-identical default document.
    assert!(doc.get("fields").is_none(), "{doc}");
}

#[test]
fn positional_commit_selects_merge_base() {
    let fx = Fixture::named("positional");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    fx.commit_all("two");
    fx.write("f.txt", "a\nb\nc\n");

    let mut opts = options(None, true);
    opts.commit = Some(first.clone());
    let out = diff::inspect(fx.path(), &opts).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["base"], serde_json::json!(first));
    assert_eq!(doc["insertions"], serde_json::json!(2));

    let mut opts = options(None, true);
    opts.commit = Some("does-not-exist".to_string());
    let err = diff::inspect(fx.path(), &opts).unwrap_err();
    assert!(err.contains("Unable to find any git commit named"), "{err}");
}

#[test]
fn head_bounds_the_commit_range() {
    let fx = Fixture::named("head-range");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    let second = fx.commit_all("two");
    fx.write("f.txt", "a\nb\nc\n");

    // Uncommitted worktree changes are outside a base..head range.
    let mut opts = options(Some(&first), true);
    opts.head = Some(second.clone());
    let out = diff::inspect(fx.path(), &opts).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["insertions"], serde_json::json!(1));

    let mut opts = options(Some(&second), true);
    opts.head = Some(first.clone());
    let err = diff::inspect(fx.path(), &opts).unwrap_err();
    assert!(err.contains("is not an ancestor of"), "{err}");

    let mut opts = options(Some(&first), true);
    opts.head = Some("does-not-exist".to_string());
    let err = diff::inspect(fx.path(), &opts).unwrap_err();
    assert!(err.contains("Unable to find any git commit named"), "{err}");
}

#[test]
fn range_wrappers_match_plain_calls() {
    let fx = Fixture::named("range-equiv");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let head = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    assert_eq!(
        diff::numstat(fx.path(), &head).unwrap(),
        diff::numstat_range(fx.path(), &head, None).unwrap()
    );
    assert_eq!(
        diff::default_diff_output(fx.path(), &head).unwrap(),
        diff::default_diff_output_range(fx.path(), &head, None).unwrap()
    );
}

#[test]
fn json_fields_merge_message_file_and_flags() {
    let fx = Fixture::named("json-fields");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    fx.write(
        "revision.txt",
        "Fix off-by-one in pager\n\nTest Plan: cargo test pager\nReviewers: dave\n",
    );

    let mut opts = field_options();
    opts.message_file = Some(fx.path().join("revision.txt").to_string_lossy().to_string());
    let out = diff::inspect(fx.path(), &opts).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        doc["fields"]["title"],
        serde_json::json!("Fix off-by-one in pager")
    );
    assert_eq!(doc["fields"]["reviewerPHIDs"], serde_json::json!(["dave"]));

    // Without a message file, local commits merge with the faux flag message.
    // Here the range holds no commits (base is HEAD), so only flag fields show.
    let mut opts = field_options();
    opts.reviewers = Some("alice".to_string());
    opts.cc = Some("carol".to_string());
    let out = diff::inspect(fx.path(), &opts).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(doc["fields"].get("title").is_none(), "{}", doc["fields"]);
    assert_eq!(doc["fields"]["reviewerPHIDs"], serde_json::json!(["alice"]));
    assert_eq!(doc["fields"]["ccPHIDs"], serde_json::json!(["carol"]));

    // Against an older base the range commits contribute their fields too.
    fx.commit_all("Add feature\n\nReviewers: bob");
    let mut opts = field_options();
    opts.base = Some(fx.git(&["rev-parse", "HEAD~1"]));
    opts.reviewers = Some("alice".to_string());
    let out = diff::inspect(fx.path(), &opts).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["fields"]["title"], serde_json::json!("Add feature"));
    assert_eq!(
        doc["fields"]["reviewerPHIDs"],
        serde_json::json!(["bob", "alice"])
    );
}

#[test]
fn only_submit_creates_diff_over_seam() {
    let fx = Fixture::named("submit-only");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    let opts = options(None, false);
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Only).unwrap();
    assert_eq!(input.mode, diff::SubmitMode::Only);
    assert!(
        input.included_changes.starts_with("Included changes:\n"),
        "{}",
        input.included_changes
    );

    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let text = diff::submit_over(&call, &input).unwrap();
    assert!(text.contains("Created a new Differential diff:"), "{text}");
    assert!(
        text.contains("Diff URI: https://phab.example.test/differential/diff/42/"),
        "{text}"
    );
    assert!(text.contains("Included changes:"), "{text}");

    let methods = canned.methods();
    assert_eq!(methods[0], "differential.creatediff");
    assert!(
        methods[1..]
            .iter()
            .all(|m| m == "differential.setdiffproperty")
    );
    let specs = canned.params_for("differential.creatediff");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0]["lintStatus"], serde_json::json!("none"));
    assert_eq!(specs[0]["unitStatus"], serde_json::json!("none"));
    assert_eq!(specs[0]["creationMethod"], serde_json::json!("arc"));
    assert!(!specs[0]["changes"].as_array().unwrap().is_empty());
}

#[test]
fn only_submit_json_reports_diff_identity() {
    let fx = Fixture::named("submit-only-json");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    let opts = options(None, true);
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Only).unwrap();
    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let text = diff::submit_over(&call, &input).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        doc["diffURI"],
        serde_json::json!("https://phab.example.test/differential/diff/42/")
    );
    assert_eq!(doc["diffID"], serde_json::json!(42));
}

#[test]
fn update_submit_requires_message_then_updates_over_seam() {
    let fx = Fixture::named("submit-update");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    let opts = options(None, false);
    let err = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Update(123)).unwrap_err();
    assert!(err.contains("requires --message"), "{err}");

    let mut opts = options(None, false);
    opts.message = Some("take two".to_string());
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Update(123)).unwrap();
    assert_eq!(input.fields, serde_json::json!({}));

    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        )
        .with(
            "differential.updaterevision",
            wire_fixture("updaterevision"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let text = diff::submit_over(&call, &input).unwrap();
    assert!(
        text.contains("Updated an existing Differential revision:"),
        "{text}"
    );
    assert!(
        text.contains("Revision URI: https://phab.example.test/D123"),
        "{text}"
    );

    let updates = canned.params_for("differential.updaterevision");
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["id"], serde_json::json!(123));
    assert_eq!(updates[0]["diffid"], serde_json::json!(42));
    assert_eq!(updates[0]["message"], serde_json::json!("take two"));
}

#[test]
fn create_submit_sends_fields_over_seam() {
    let fx = Fixture::named("submit-create");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    fx.commit_all("Add feature\n\nReviewers: alice");
    fx.write("f.txt", "a\nb\nc\n");

    let opts = options(Some(&first), false);
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Create).unwrap();
    assert_eq!(input.fields["title"], serde_json::json!("Add feature"));

    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        )
        .with(
            "differential.createrevision",
            wire_fixture("createrevision"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let text = diff::submit_over(&call, &input).unwrap();
    assert!(
        text.contains("Created a new Differential revision:"),
        "{text}"
    );
    assert!(
        text.contains("Revision URI: https://phab.example.test/D124"),
        "{text}"
    );

    let creates = canned.params_for("differential.createrevision");
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0]["diffid"], serde_json::json!(42));
    assert_eq!(
        creates[0]["fields"]["title"],
        serde_json::json!("Add feature")
    );
}

#[test]
fn gap2_browse_opens_created_uri_through_fake_opener() {
    let fx = Fixture::named("gap2-browse");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    let opts = options(None, false);
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Only).unwrap();
    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let report = diff::submit_over_with_uri(&call, &input).unwrap();
    assert_eq!(
        report.uri.as_deref(),
        Some("https://phab.example.test/differential/diff/42/")
    );
    assert!(
        report.text.contains("Created a new Differential diff:"),
        "{}",
        report.text
    );

    // Fake opener records instead of spawning a real browser.
    let seen: RefCell<Vec<(String, Vec<String>)>> = RefCell::new(Vec::new());
    let fake_open = |uri: &str, browser: &[String]| {
        seen.borrow_mut().push((uri.to_string(), browser.to_vec()));
        Ok(())
    };
    let never = |_: &str| false;
    let configured = vec!["fake-browser".to_string()];
    diff::open_submit_result_in_browser(
        report.uri.as_deref(),
        true,
        Some(&configured),
        &never,
        &fake_open,
    )
    .unwrap();
    assert_eq!(
        *seen.borrow(),
        vec![(
            "https://phab.example.test/differential/diff/42/".to_string(),
            configured.clone()
        )]
    );
    // Without --browse the opener stays shut.
    diff::open_submit_result_in_browser(
        report.uri.as_deref(),
        false,
        Some(&configured),
        &never,
        &fake_open,
    )
    .unwrap();
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn gap2_submit_report_uris_per_mode() {
    let fx = Fixture::named("gap2-report-uris");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");

    let mut opts = options(None, false);
    opts.message = Some("take two".to_string());
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Update(123)).unwrap();
    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        )
        .with(
            "differential.updaterevision",
            wire_fixture("updaterevision"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let report = diff::submit_over_with_uri(&call, &input).unwrap();
    assert_eq!(
        report.uri.as_deref(),
        Some("https://phab.example.test/D123")
    );

    let fx = Fixture::named("gap2-report-uris-create");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    let first = fx.commit_all("one");
    fx.write("f.txt", "a\nb\n");
    fx.commit_all("Add feature");
    fx.write("f.txt", "a\nb\nc\n");
    let opts = options(Some(&first), false);
    let input = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Create).unwrap();
    let canned = Canned::default()
        .with("differential.creatediff", wire_fixture("creatediff"))
        .with(
            "differential.setdiffproperty",
            wire_fixture("setdiffproperty"),
        )
        .with(
            "differential.createrevision",
            wire_fixture("createrevision"),
        );
    let call = |method: &str, params: &serde_json::Value| canned.call(method, params);
    let report = diff::submit_over_with_uri(&call, &input).unwrap();
    assert_eq!(
        report.uri.as_deref(),
        Some("https://phab.example.test/D124")
    );
}

#[test]
fn gap2_browser_open_failure_names_uri_and_browser() {
    let err = diff::open_uri_in_browser(
        "http://x/D1",
        &["definitely-not-a-browser-gap2".to_string()],
    )
    .unwrap_err();
    assert_eq!(
        err,
        "Failed to open URI \"http://x/D1\" in browser (\"definitely-not-a-browser-gap2\"). Check your \"browser\" config option."
    );
}

#[test]
fn submit_with_no_changes_fails() {
    let fx = Fixture::named("submit-empty");
    fx.init_repo();
    fx.write("f.txt", "a\n");
    fx.commit_all("one");

    let opts = options(None, false);
    let err = diff::prepare_submit(fx.path(), &opts, &diff::SubmitMode::Only).unwrap_err();
    assert_eq!(err, "There are no changes to generate a diff from!");
}
