use super::*;
use crate::diff_local;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn defaults() -> Options {
    Options {
        base: None,
        json: false,
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

#[test]
fn parses_defaults() {
    assert_eq!(parse_options(&args(&[])).unwrap(), Some(defaults()));
}

#[test]
fn parses_base_and_json() {
    assert_eq!(
        parse_options(&args(&["--base", "HEAD~1", "--json"])).unwrap(),
        Some(Options {
            base: Some("HEAD~1".to_string()),
            json: true,
            provided: vec!["base", "json"],
            ..defaults()
        })
    );
    assert_eq!(
        parse_options(&args(&["--base=abc123"])).unwrap(),
        Some(Options {
            base: Some("abc123".to_string()),
            provided: vec!["base"],
            ..defaults()
        })
    );
}

#[test]
fn parses_range_and_field_flags() {
    let options = parse_options(&args(&[
        "--head",
        "HEAD",
        "--message",
        "take two",
        "--message-file",
        "msg.txt",
        "--reviewers",
        "alice,bob",
        "--cc",
        "carol",
        "--conduit-uri",
        "https://phab.example.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(
        options,
        Options {
            head: Some("HEAD".to_string()),
            message: Some("take two".to_string()),
            message_file: Some("msg.txt".to_string()),
            reviewers: Some("alice,bob".to_string()),
            cc: Some("carol".to_string()),
            conduit_uri: Some("https://phab.example.test/".to_string()),
            provided: vec![
                "head",
                "message",
                "message-file",
                "reviewers",
                "cc",
                "conduit-uri"
            ],
            ..defaults()
        }
    );
    let options = parse_options(&args(&[
        "--head=abc",
        "--message=hi",
        "--message-file=m.txt",
        "--reviewers=al",
        "--cc=bo",
        "--conduit-uri=https://x.test/",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(options.head.as_deref(), Some("abc"));
    assert_eq!(options.message.as_deref(), Some("hi"));
    assert_eq!(options.message_file.as_deref(), Some("m.txt"));
    assert_eq!(options.reviewers.as_deref(), Some("al"));
    assert_eq!(options.cc.as_deref(), Some("bo"));
    assert_eq!(options.conduit_uri.as_deref(), Some("https://x.test/"));
    // `--update` conflicts with `--reviewers`/`--cc`, so it parses alone.
    let options = parse_options(&args(&["--update=D12"])).unwrap().unwrap();
    assert_eq!(options.update.as_deref(), Some("D12"));
}

#[test]
fn parses_shorts_and_submit_modes() {
    let options = parse_options(&args(&["-m", "msg", "-F", "f.txt", "-a"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("msg"));
    assert_eq!(options.message_file.as_deref(), Some("f.txt"));
    assert!(options.add_all);
    assert_eq!(options.provided, vec!["message", "message-file", "add-all"]);

    let options = parse_options(&args(&["--create"])).unwrap().unwrap();
    assert!(options.create && !options.only && options.update.is_none());
    let options = parse_options(&args(&["--only"])).unwrap().unwrap();
    assert!(options.only && !options.create && options.update.is_none());
    let options = parse_options(&args(&["--update", "123"])).unwrap().unwrap();
    assert_eq!(options.update.as_deref(), Some("123"));
}

#[test]
fn parses_all_spec_flags() {
    // `--json` conflicts with nothing, so it pairs with every flag.
    let options = parse_options(&args(&["--json", "--edit"]))
        .unwrap()
        .unwrap();
    assert!(options.edit);
    let options = parse_options(&args(&["--json", "--raw"])).unwrap().unwrap();
    assert!(options.raw);
    let options = parse_options(&args(&["--json", "--raw-command", "git diff"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.raw_command.as_deref(), Some("git diff"));
    let options = parse_options(&args(&["--json", "--raw-command=git log"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.raw_command.as_deref(), Some("git log"));
    let options = parse_options(&args(&["--json", "--draft"]))
        .unwrap()
        .unwrap();
    assert!(options.draft);
    let options = parse_options(&args(&["--json", "--nounit"]))
        .unwrap()
        .unwrap();
    assert!(options.nounit);
    let options = parse_options(&args(&["--json", "--nolint"]))
        .unwrap()
        .unwrap();
    assert!(options.nolint);
    let options = parse_options(&args(&["--json", "--allow-untracked"]))
        .unwrap()
        .unwrap();
    assert!(options.allow_untracked);
    let options = parse_options(&args(&["--json", "--apply-patches"]))
        .unwrap()
        .unwrap();
    assert!(options.apply_patches);
    let options = parse_options(&args(&["--json", "--never-apply-patches"]))
        .unwrap()
        .unwrap();
    assert!(options.never_apply_patches);
    let options = parse_options(&args(&["--json", "--amend-all"]))
        .unwrap()
        .unwrap();
    assert!(options.amend_all);
    let options = parse_options(&args(&["--json", "--amend-autofixes"]))
        .unwrap()
        .unwrap();
    assert!(options.amend_autofixes);
    let options = parse_options(&args(&["--json", "--add-all"]))
        .unwrap()
        .unwrap();
    assert!(options.add_all);
    let options = parse_options(&args(&["--json", "--no-amend"]))
        .unwrap()
        .unwrap();
    assert!(options.no_amend);
    let options = parse_options(&args(&["--json", "--uncommitted"]))
        .unwrap()
        .unwrap();
    assert!(options.uncommitted);
    let options = parse_options(&args(&["--json", "--verbatim"]))
        .unwrap()
        .unwrap();
    assert!(options.verbatim);
    let options = parse_options(&args(&["--json", "--skip-binaries"]))
        .unwrap()
        .unwrap();
    assert!(options.skip_binaries);
    let options = parse_options(&args(&["--json", "--skip-staging"]))
        .unwrap()
        .unwrap();
    assert!(options.skip_staging);
    let options = parse_options(&args(&["--json", "--coverage"]))
        .unwrap()
        .unwrap();
    assert!(options.coverage);
    let options = parse_options(&args(&["--json", "--no-coverage"]))
        .unwrap()
        .unwrap();
    assert!(options.no_coverage);
    let options = parse_options(&args(&["--json", "--browse"]))
        .unwrap()
        .unwrap();
    assert!(options.browse);
    // Re-providing a flag overwrites without error.
    let options = parse_options(&args(&["--json", "--message", "a", "--message", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("b"));
    assert_eq!(options.provided, vec!["json", "message"]);
}

#[test]
fn parses_positional_paths() {
    // Positionals collect into `paths`; the base commit derives later.
    let options = parse_options(&args(&["HEAD~1"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["HEAD~1".to_string()]);
    assert_eq!(options.commit, None);
    let options = parse_options(&args(&["--", "HEAD~1"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["HEAD~1".to_string()]);
    // Several positionals (and a second `--`) parse; arity errors come later.
    let options = parse_options(&args(&["--", "a", "--", "b"]))
        .unwrap()
        .unwrap();
    assert_eq!(
        options.paths,
        vec!["a".to_string(), "--".to_string(), "b".to_string()]
    );
    let options = parse_options(&args(&["a", "b"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn parses_bool_equals_value_as_next_token() {
    // PHP splices `--bool=value` (`:826-832`): the value parses as the next
    // token, so it lands in `paths` here ...
    let options = parse_options(&args(&["--json=x"])).unwrap().unwrap();
    assert!(options.json);
    assert_eq!(options.paths, vec!["x".to_string()]);
    // ... and a flag-looking value even enables that flag.
    let err = parse_options(&args(&["--create=--only"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--create' and '--only' are mutually exclusive: \
         '--create' can not be used with '--only'."
    );
}

#[test]
fn parses_help() {
    assert_eq!(parse_options(&args(&["--help"])).unwrap(), None);
    assert_eq!(parse_options(&args(&["-h"])).unwrap(), None);
    // Past `--`, help looks like a path.
    let options = parse_options(&args(&["--", "--help"])).unwrap().unwrap();
    assert_eq!(options.paths, vec!["--help".to_string()]);
    // Unknown names error even when they start with a known prefix.
    let err = parse_options(&args(&["--help=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'help'. Try 'arc help'.");
}

#[test]
fn rejects_bad_args() {
    // A trailing `param` flag names its typed spelling.
    for argv in [
        vec!["--base"],
        vec!["--head"],
        vec!["--message"],
        vec!["--message-file"],
        vec!["--raw-command"],
        vec!["--reviewers"],
        vec!["--cc"],
        vec!["--update"],
        vec!["--conduit-uri"],
    ] {
        let err = parse_options(&args(&argv)).unwrap_err();
        assert_eq!(err, format!("Option '{}' requires a parameter.", argv[0]));
    }
    let err = parse_options(&args(&["-m"])).unwrap_err();
    assert_eq!(err, "Option '-m' requires a parameter.");
    let err = parse_options(&args(&["-F"])).unwrap_err();
    assert_eq!(err, "Option '-F' requires a parameter.");
    // Empty values are fine; flag-looking values are consumed, not parsed.
    let options = parse_options(&args(&["--base="])).unwrap().unwrap();
    assert_eq!(options.base.as_deref(), Some(""));
    let options = parse_options(&args(&["--head="])).unwrap().unwrap();
    assert_eq!(options.head.as_deref(), Some(""));
    let options = parse_options(&args(&["--message", "--json"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.message.as_deref(), Some("--json"));
    assert!(!options.json);
    // Unknown names report the dashes-stripped spelling.
    let err = parse_options(&args(&["--bogus"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
    let err = parse_options(&args(&["--bogus=x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
    let err = parse_options(&args(&["-x"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'x'. Try 'arc help'.");
    // Short flags never combine.
    let err = parse_options(&args(&["-mx"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'mx'. Try 'arc help'.");
    let err = parse_options(&args(&["-"])).unwrap_err();
    assert_eq!(err, "Unknown argument ''. Try 'arc help'.");
}

#[test]
fn rejects_conflicting_submit_flags() {
    let err = parse_options(&args(&["--create", "--update", "1"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--create' and '--update' are mutually exclusive: \
         '--create' can not be used with '--update'."
    );
    let err = parse_options(&args(&["--create", "--only"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--create' and '--only' are mutually exclusive: \
         '--create' can not be used with '--only'."
    );
    let err = parse_options(&args(&["--only", "--message", "x"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--only' and '--message' are mutually exclusive: \
         '--only' does not update any revision."
    );
    // `--update` plus `--only` stays legal: `--update` wins.
    let options = parse_options(&args(&["--update", "1", "--only"]))
        .unwrap()
        .unwrap();
    assert_eq!(options.update.as_deref(), Some("1"));
    assert!(options.only);
}

#[test]
fn rejects_every_declared_conflict() {
    // (argv, expected message): one case per table row, in table order.
    let cases: &[(&[&str], &str)] = &[
        (
            &["--raw", "--apply-patches"],
            "Arguments '--raw' and '--apply-patches' are mutually exclusive: '--raw' disables lint.",
        ),
        (
            &["--raw", "--never-apply-patches"],
            "Arguments '--raw' and '--never-apply-patches' are mutually exclusive: '--raw' disables lint.",
        ),
        (
            &["--raw", "--create"],
            "Arguments '--raw' and '--create' are mutually exclusive: '--raw' and '--create' both need stdin. Use '--raw-command'.",
        ),
        (
            &["--raw", "--edit"],
            "Arguments '--raw' and '--edit' are mutually exclusive: '--raw' and '--edit' both need stdin. Use '--raw-command'.",
        ),
        (
            &["--raw", "--raw-command", "cmd"],
            "Arguments '--raw' and '--raw-command' are mutually exclusive.",
        ),
        (
            &["--raw-command", "cmd", "--apply-patches"],
            "Arguments '--raw-command' and '--apply-patches' are mutually exclusive: '--raw-command' disables lint.",
        ),
        (
            &["--raw-command", "cmd", "--never-apply-patches"],
            "Arguments '--raw-command' and '--never-apply-patches' are mutually exclusive: '--raw-command' disables lint.",
        ),
        (
            &["--create", "--edit"],
            "Arguments '--create' and '--edit' are mutually exclusive: '--create' can not be used with '--edit'.",
        ),
        (
            &["--draft", "--edit"],
            "Arguments '--draft' and '--edit' are mutually exclusive.",
        ),
        (
            &["--draft", "--only"],
            "Arguments '--draft' and '--only' are mutually exclusive.",
        ),
        (
            &["--draft", "--update", "1"],
            "Arguments '--draft' and '--update' are mutually exclusive.",
        ),
        (
            &["--nolint", "--apply-patches"],
            "Arguments '--nolint' and '--apply-patches' are mutually exclusive: '--nolint' suppresses lint.",
        ),
        (
            &["--nolint", "--never-apply-patches"],
            "Arguments '--nolint' and '--never-apply-patches' are mutually exclusive: '--nolint' suppresses lint.",
        ),
        (
            &["--only", "--edit"],
            "Arguments '--only' and '--edit' are mutually exclusive: '--only' does affect revisions.",
        ),
        (
            &["--verbatim", "--update", "1"],
            "Arguments '--verbatim' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--update", "1", "--verbatim"],
            "Arguments '--verbatim' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--only"],
            "Arguments '--verbatim' and '--only' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--raw"],
            "Arguments '--verbatim' and '--raw' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--raw-command", "cmd"],
            "Arguments '--verbatim' and '--raw-command' are mutually exclusive: 1",
        ),
        (
            &["--verbatim", "--message-file", "f"],
            "Arguments '--verbatim' and '--message-file' are mutually exclusive: 1",
        ),
        (
            &["--only", "--reviewers", "a"],
            "Arguments '--reviewers' and '--only' are mutually exclusive: 1",
        ),
        (
            &["--update", "1", "--reviewers", "a"],
            "Arguments '--reviewers' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--only", "--cc", "a"],
            "Arguments '--cc' and '--only' are mutually exclusive: 1",
        ),
        (
            &["--update", "1", "--cc", "a"],
            "Arguments '--cc' and '--update' are mutually exclusive: 1",
        ),
        (
            &["--coverage", "--no-coverage"],
            "Arguments '--coverage' and '--no-coverage' are mutually exclusive.",
        ),
    ];
    for (argv, expected) in cases {
        let err = parse_options(&args(argv)).unwrap_err();
        assert_eq!(err, expected.to_string(), "argv: {argv:?}");
    }
    // Symmetric pairs name the earlier flag first.
    let err = parse_options(&args(&["--apply-patches", "--never-apply-patches"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--apply-patches' and '--never-apply-patches' are mutually exclusive: 1"
    );
    let err = parse_options(&args(&["--never-apply-patches", "--apply-patches"])).unwrap_err();
    assert_eq!(
        err,
        "Arguments '--never-apply-patches' and '--apply-patches' are mutually exclusive: 1"
    );
    // Unknown flags beat conflicts: the whole line parses first.
    let err = parse_options(&args(&["--create", "--bogus", "--only"])).unwrap_err();
    assert_eq!(err, "Unknown argument 'bogus'. Try 'arc help'.");
}

#[test]
fn checks_vcs_supports() {
    // Supported combinations pass quietly.
    assert!(check_supports(Vcs::Git, &["head", "json"]).is_ok());
    assert!(check_supports(Vcs::Hg, &["uncommitted", "verbatim"]).is_ok());
    assert!(check_supports(Vcs::Git, &[]).is_ok());
    // Unsupported flags report the VCS plus any `nosupport` note.
    let err = check_supports(Vcs::Git, &["uncommitted"]).unwrap_err();
    assert_eq!(err, "Option '--uncommitted' is not supported under git.");
    let err = check_supports(Vcs::Svn, &["uncommitted"]).unwrap_err();
    assert_eq!(err, "Option '--uncommitted' is not supported under svn.");
    let err = check_supports(Vcs::Svn, &["verbatim"]).unwrap_err();
    assert_eq!(err, "Option '--verbatim' is not supported under svn.");
    let err = check_supports(Vcs::Svn, &["edit"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--edit' is not supported under svn. \
         Edit revisions via the web interface when using SVN."
    );
    let err = check_supports(Vcs::Svn, &["base"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--base' is not supported under svn. Subversion does not use base commits."
    );
    let err = check_supports(Vcs::Svn, &["head"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--head' is not supported under svn. Subversion does not support commit ranges."
    );
    let err = check_supports(Vcs::Hg, &["head"]).unwrap_err();
    assert_eq!(
        err,
        "Option '--head' is not supported under hg. Mercurial does not support --head yet."
    );
    // First unsupported flag in provision order wins.
    let err = check_supports(Vcs::Svn, &["head", "uncommitted"]).unwrap_err();
    assert!(err.contains("'--head'"), "{err}");
    let err = check_supports(Vcs::Svn, &["uncommitted", "head"]).unwrap_err();
    assert!(err.contains("'--uncommitted'"), "{err}");
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("arc-rs-diff-d1-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn git_dir(tag: &str) -> PathBuf {
    let dir = scratch_dir(tag);
    let status = std::process::Command::new("git")
        .arg("init")
        .arg(&dir)
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

#[test]
fn detects_working_copy_vcs() {
    assert_eq!(detect_vcs(&scratch_dir("none")), None);
    let hg = scratch_dir("hg");
    std::fs::create_dir_all(hg.join("sub")).unwrap();
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    assert_eq!(detect_vcs(&hg.join("sub")), Some(Vcs::Hg));
    let svn = scratch_dir("svn");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    assert_eq!(detect_vcs(&svn), Some(Vcs::Svn));
    assert_eq!(detect_vcs(&git_dir("git")), Some(Vcs::Git));
    // Git wins over enclosing markers.
    let nested = git_dir("nested");
    std::fs::create_dir_all(nested.join(".hg")).unwrap();
    assert_eq!(detect_vcs(&nested), Some(Vcs::Git));
}

#[test]
fn prepares_options_per_working_copy() {
    // No working copy: options pass through untouched.
    let options = Options {
        head: Some(String::new()),
        ..defaults()
    };
    let resolved = prepare(&scratch_dir("pass"), &options).unwrap();
    assert_eq!(resolved.head, None);
    assert_eq!(resolved.commit, None);
    // Git derives the base commit from `paths`.
    let options = Options {
        paths: vec!["abc".to_string()],
        provided: vec!["json"],
        ..defaults()
    };
    let resolved = prepare(&git_dir("commit"), &options).unwrap();
    assert_eq!(resolved.commit.as_deref(), Some("abc"));
    // ... and rejects several positionals.
    let options = Options {
        paths: vec!["a".to_string(), "b".to_string()],
        ..defaults()
    };
    let err = prepare(&git_dir("two"), &options).unwrap_err();
    assert!(err.contains("exactly one base commit"), "{err}");
    // Supports errors beat arity errors.
    let options = Options {
        paths: vec!["a".to_string(), "b".to_string()],
        provided: vec!["uncommitted"],
        uncommitted: true,
        ..defaults()
    };
    let err = prepare(&git_dir("order"), &options).unwrap_err();
    assert_eq!(err, "Option '--uncommitted' is not supported under git.");
    // Svn positionals are file paths, not base commits.
    let svn = scratch_dir("paths");
    std::fs::create_dir_all(svn.join(".svn")).unwrap();
    let options = Options {
        paths: vec!["a".to_string()],
        ..defaults()
    };
    let err = prepare(&svn, &options).unwrap_err();
    assert_eq!(
        err,
        "This version control system does not support commit ranges."
    );
    // Detected non-git working copies fail honestly until their flows land.
    let err = prepare(&svn, &defaults()).unwrap_err();
    assert_eq!(err, "svn working copies are not yet ported (see T1128)");
    let hg = scratch_dir("ported");
    std::fs::create_dir_all(hg.join(".hg")).unwrap();
    let err = prepare(&hg, &defaults()).unwrap_err();
    assert_eq!(err, "hg working copies are not yet ported (see T1128)");
}

#[test]
fn resolve_range_rejects_base_with_commit() {
    let options = Options {
        base: Some("abc".to_string()),
        commit: Some("def".to_string()),
        ..defaults()
    };
    let err = resolve_range(Path::new("."), &options).unwrap_err();
    assert_eq!(err, "'--base' can not be used with a base commit.");
}

#[test]
fn derives_submit_mode() {
    assert_eq!(submit_mode(&defaults()).unwrap(), None);
    assert_eq!(
        submit_mode(&Options {
            create: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Create)
    );
    assert_eq!(
        submit_mode(&Options {
            only: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Only)
    );
    assert_eq!(
        submit_mode(&Options {
            update: Some("D12".to_string()),
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Update(12))
    );
    // `--update` wins over `--only`, like shouldOnlyCreateDiff().
    assert_eq!(
        submit_mode(&Options {
            update: Some("7".to_string()),
            only: true,
            ..defaults()
        })
        .unwrap(),
        Some(SubmitMode::Update(7))
    );
    let err = submit_mode(&Options {
        update: Some("nope".to_string()),
        ..defaults()
    })
    .unwrap_err();
    assert!(err.contains("Parameter to --update"), "{err}");
}

#[test]
fn merges_fields_oldest_first_with_faux_flags() {
    let commits = vec![
        "Oldest title\n\nOld body.\n".to_string(),
        "Newest title\n\nReviewers: alice\n".to_string(),
    ];
    let fields = merge_message_fields(&commits, Some("bob"), Some("carol"));
    assert_eq!(
        fields.get(diff_local::FIELD_TITLE),
        Some(&diff_local::FieldValue::Text("Oldest title".to_string()))
    );
    let summary = match fields.get(diff_local::FIELD_SUMMARY) {
        Some(diff_local::FieldValue::Text(text)) => text.clone(),
        other => panic!("missing summary: {other:?}"),
    };
    assert!(summary.contains("Old body"), "{summary}");
    assert!(summary.contains("Newest title"), "{summary}");
    assert_eq!(
        fields.get(diff_local::FIELD_REVIEWERS),
        Some(&diff_local::FieldValue::List(vec![
            "alice".to_string(),
            "bob".to_string()
        ]))
    );
    assert_eq!(
        fields.get(diff_local::FIELD_CCS),
        Some(&diff_local::FieldValue::List(vec!["carol".to_string()]))
    );
}

#[test]
fn renders_fields_json() {
    let mut fields = diff_local::CommitFields::new();
    fields.insert(
        diff_local::FIELD_TITLE.to_string(),
        diff_local::FieldValue::Text("T".to_string()),
    );
    fields.insert(
        diff_local::FIELD_REVIEWERS.to_string(),
        diff_local::FieldValue::List(vec!["a".to_string()]),
    );
    assert_eq!(
        fields_json(&fields),
        serde_json::json!({"title": "T", "reviewerPHIDs": ["a"]})
    );
}

#[test]
fn parses_empty_numstat() {
    assert_eq!(parse_numstat_z(b"").unwrap(), Vec::new());
}

#[test]
fn parses_numstat_entries() {
    let raw = b"2\t1\tf.txt\0-\t-\tbin.dat\0";
    assert_eq!(
        parse_numstat_z(raw).unwrap(),
        vec![
            FileDiff {
                path: "f.txt".to_string(),
                added: 2,
                deleted: 1
            },
            FileDiff {
                path: "bin.dat".to_string(),
                added: 0,
                deleted: 0
            },
        ]
    );
}

#[test]
fn parses_numstat_rename_to_new_path() {
    let raw = b"0\t0\t\0g.txt\0renamed.txt\0";
    assert_eq!(
        parse_numstat_z(raw).unwrap(),
        vec![FileDiff {
            path: "renamed.txt".to_string(),
            added: 0,
            deleted: 0
        }]
    );
}

#[test]
fn rejects_malformed_numstat() {
    assert!(parse_numstat_z(b"no-tabs-here\0").is_err());
    assert!(parse_numstat_z(b"x\t1\tf\0").is_err());
    assert!(parse_numstat_z(b"0\t0\t\0only-old\0").is_err());
}

#[test]
fn builds_json_document() {
    let files = vec![FileDiff {
        path: "f.txt".to_string(),
        added: 2,
        deleted: 1,
    }];
    let doc = diff_json("abc", &files);
    assert_eq!(
        doc,
        serde_json::json!({
            "base": "abc",
            "files": [{"path": "f.txt", "added": 2, "deleted": 1}],
            "insertions": 2,
            "deletions": 1,
        })
    );
    assert_eq!(summarize(&[]), (0, 0));
}
