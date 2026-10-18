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
            std::env::temp_dir().join(format!("arc-rs-setcfg-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn types_cover_known_keys() {
    assert_eq!(setting_type("history.immutable"), "bool");
    assert_eq!(setting_type("arc.autostash"), "bool");
    assert_eq!(setting_type("load"), "list");
    assert_eq!(setting_type("events.listeners"), "list");
    assert_eq!(setting_type("phabricator.uri"), "string");
    assert_eq!(setting_type("aliases"), "aliases");
    assert_eq!(setting_type("whatever"), "wild");
    assert!(is_known("editor"));
    assert!(!is_known("whatever"));
}

#[test]
fn coercion_matches_php_write_rules() {
    for raw in ["true", "yes", "on", "1", "TRUE"] {
        assert_eq!(
            coerce_value("history.immutable", raw).unwrap(),
            serde_json::json!(true)
        );
    }
    for raw in ["false", "no", "off", "0", "NO", ""] {
        assert_eq!(
            coerce_value("history.immutable", raw).unwrap(),
            serde_json::json!(false)
        );
    }
    assert_eq!(
        coerce_value("history.immutable", "perhaps").unwrap_err(),
        "Type of setting 'history.immutable' must be boolean, like 'true' or 'false'."
    );
    assert_eq!(
        coerce_value("load", r#"["a", "b"]"#).unwrap(),
        serde_json::json!(["a", "b"])
    );
    // PHP decodes with assoc=true, so JSON objects are accepted lists.
    assert_eq!(
        coerce_value("load", r#"{"a": 1}"#).unwrap(),
        serde_json::json!({"a": 1})
    );
    assert_eq!(
        coerce_value("load", "a, b").unwrap_err(),
        "Type of setting 'load' must be list. You can specify a list in JSON, like: [\"apple\", \"banana\", \"cherry\"]"
    );
    assert!(coerce_value("load", "7").is_err());
    assert_eq!(
        coerce_value("editor", "nano").unwrap(),
        serde_json::json!("nano")
    );
    assert_eq!(
        coerce_value("custom.key", "v").unwrap(),
        serde_json::json!("v")
    );
    // T1153 CORE-37: the alias error keeps PHP's text (the
    // `Exception` arm renders it at the CLI layer).
    assert_eq!(
        coerce_value("aliases", "x").unwrap_err(),
        "Use \"arc alias\" to configure aliases, not \"arc set-config\"."
    );
}

#[test]
fn display_and_warnings() {
    assert_eq!(format_value(&serde_json::json!(true)), "true");
    assert_eq!(format_value(&serde_json::json!(null)), "null");
    assert_eq!(format_value(&serde_json::json!("x")), "\"x\"");
    assert_eq!(format_value(&serde_json::json!([1])), "[1]");
    assert!(warn_if_unknown_ansi("editor", false).is_none());
    assert_eq!(
        warn_if_unknown_ansi("bogus", false).unwrap(),
        "Warning: The configuration key \"bogus\" is not recognized by arc. It may be misspelled or out of date."
    );
    assert_eq!(
        warn_if_unknown_ansi("bogus", true).unwrap(),
        "\x1b[1mWarning:\x1b[m The configuration key \"bogus\" is not recognized by arc. It may be misspelled or out of date."
    );
}

#[test]
fn user_doc_handles_missing_and_broken_files() {
    let tmp = TempDir::new("doc");
    let path = tmp.path.join(".arcrc");
    assert_eq!(read_user_doc(&path).unwrap(), serde_json::json!({}));
    std::fs::write(&path, "nope").unwrap();
    assert_eq!(
        read_user_doc(&path).unwrap_err(),
        "Your '~/.arcrc' file is not a valid JSON file."
    );
    std::fs::write(&path, "[1]").unwrap();
    assert!(read_user_doc(&path).is_err());
    let doc = serde_json::json!({"config": {"a": "b"}});
    write_user_doc(&path, &doc).unwrap();
    assert_eq!(read_user_doc(&path).unwrap(), doc);
}

#[test]
fn apply_user_sets_deletes_and_preserves_hosts() {
    let tmp = TempDir::new("apply");
    let path = tmp.path.join(".arcrc");
    std::fs::write(&path, r#"{"hosts": {"https://h/api/": {"token": "t"}}}"#).unwrap();

    // PHP always prints `(was ...)`: a missing key renders as `null`.
    assert_eq!(
        apply_user(&path, "editor", "nano").unwrap(),
        "Set key \"editor\" = \"nano\" in user config (was null)."
    );
    assert_eq!(
        apply_user(&path, "editor", "vim").unwrap(),
        "Set key \"editor\" = \"vim\" in user config (was \"nano\")."
    );
    let doc = read_user_doc(&path).unwrap();
    assert_eq!(doc["config"]["editor"], serde_json::json!("vim"));
    assert_eq!(
        doc["hosts"]["https://h/api/"]["token"],
        serde_json::json!("t")
    );

    assert_eq!(
        apply_user(&path, "editor", "").unwrap(),
        "Deleted key \"editor\" from user config (was \"vim\")."
    );
    assert_eq!(
        apply_user(&path, "editor", "").unwrap(),
        "Deleted key \"editor\" from user config (was null)."
    );

    assert!(apply_user(&path, "history.immutable", "perhaps").is_err());
    assert!(apply_user(&path, "aliases", "x").is_err());
    std::fs::write(&path, r#"{"config": 7}"#).unwrap();
    assert!(apply_user(&path, "editor", "x").is_err());
}

#[test]
fn apply_user_opt_stages_unset_default_and_overrides() {
    // `None` with no override stages the unset-HOME arm.
    assert_eq!(
        apply_user_opt(None, None, "editor", "vim").unwrap_err(),
        "cannot locate ~/.arcrc (HOME/USERPROFILE is not set)"
    );
    let tmp = TempDir::new("user-opt");
    // A staged default writes through the seam.
    let path = tmp.path.join(".arcrc");
    let message = apply_user_opt(Some(&path), None, "editor", "vim").unwrap();
    assert_eq!(
        message,
        "Set key \"editor\" = \"vim\" in user config (was null)."
    );
    let doc = read_user_doc(&path).unwrap();
    assert_eq!(doc["config"]["editor"], serde_json::json!("vim"));
    // A hostile staged default (unparseable JSON) fails the write.
    std::fs::write(&path, "{nope").unwrap();
    assert!(apply_user_opt(Some(&path), None, "editor", "x").is_err());
    // A missing `--arcrc-file` override errors like stock PHP.
    let missing = tmp.path.join("missing");
    assert!(
        apply_user_opt(Some(&path), Some(&missing), "editor", "x")
            .unwrap_err()
            .contains("was specified, but it was not found")
    );
}

#[test]
fn local_files_roundtrip_at_explicit_roots() {
    let tmp = TempDir::new("local");
    let root = tmp.path.join("repo");
    let path = local_config_path(&root);
    assert!(path.ends_with(Path::new(".git").join("arc").join("config")));
    assert!(read_local_file(&path).unwrap().is_empty());

    assert_eq!(
        apply_local_at(&root, "history.immutable", "true").unwrap(),
        "Set key \"history.immutable\" = true in local config (was null)."
    );
    assert!(path.is_file());
    let mut map = read_local_file(&path).unwrap();
    assert_eq!(map["history.immutable"], serde_json::json!(true));

    map.insert("editor".to_string(), serde_json::json!("nano"));
    write_local_file(&path, &map).unwrap();
    assert_eq!(
        read_local_file(&path).unwrap()["editor"],
        serde_json::json!("nano")
    );

    std::fs::write(&path, "nope").unwrap();
    let err = read_local_file(&path).unwrap_err();
    assert!(err.starts_with("Failed to parse '"), "{err}");
    assert!(err.ends_with("as JSON."), "{err}");
    std::fs::write(&path, "[1]").unwrap();
    assert!(read_local_file(&path).is_err());

    let deleted = apply_local_at(&root, "editor", "").unwrap_err();
    assert!(deleted.ends_with("is not a JSON object"), "{deleted}");
}

#[cfg(unix)]
#[test]
fn only_user_writes_harden_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("perms");
    let user_file = tmp.path.join(".arcrc");
    apply_user(&user_file, "editor", "nano").unwrap();
    let mode = std::fs::metadata(&user_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "user file must be 0600");

    let root = tmp.path.join("repo");
    apply_local_at(&root, "editor", "nano").unwrap();
    let local = local_config_path(&root);
    let mode = std::fs::metadata(&local).unwrap().permissions().mode() & 0o777;
    assert_ne!(mode, 0o600, "local file must not be hardened");
}

#[test]
fn local_apply_requires_a_repo() {
    let tmp = TempDir::new("norepo");
    assert_eq!(
        apply_local(&tmp.path, "editor", "x").unwrap_err(),
        "This command must be run in a Git, Mercurial or Subversion working copy."
    );
}

#[test]
fn core32_local_writes_cover_hg_and_svn_roots() {
    // T1153 CORE-32 (write half): `--local` resolves the VCS root for
    // git, hg, and svn alike (filesystem markers, no binaries needed),
    // writing `arc/config` under the right metadata dir from subdirs.
    for (meta, kind) in [
        (".hg", get_config::VcsKind::Hg),
        (".svn", get_config::VcsKind::Svn),
        (".git", get_config::VcsKind::Git),
    ] {
        let tmp = TempDir::new("localvcs");
        let root = tmp.path.join("proj");
        std::fs::create_dir_all(root.join(meta)).unwrap();
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let message = apply_local(&sub, "editor", "nano").unwrap();
        assert!(message.contains("local config"), "{message}");
        let stored = read_local_file(&local_config_path_for(kind, &root)).unwrap();
        assert_eq!(stored["editor"], serde_json::json!("nano"));
        // Delete round-trips through the same path.
        apply_local(&sub, "editor", "").unwrap();
        let stored = read_local_file(&local_config_path_for(kind, &root)).unwrap();
        assert!(!stored.contains_key("editor"));
    }
    assert_eq!(
        local_config_path(Path::new("/r")),
        PathBuf::from("/r/.git/arc/config")
    );
}

#[test]
fn args_require_key_and_value() {
    assert!(parse_args(&[]).is_err());
    assert!(parse_args(&["only".to_string()]).is_err());
    assert!(parse_args(&["a".to_string(), "b".to_string(), "c".to_string()]).is_err());
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    assert!(parse_args(&["--bogus".to_string(), "a".to_string(), "b".to_string()]).is_err());
    let opts = parse_args(&["--local".to_string(), "k".to_string(), "v".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(
        opts,
        Options {
            local: true,
            key: "k".to_string(),
            value: "v".to_string(),
            config: vec![],
            config_files: vec![],
            arcrc_file: None,
        }
    );
    let empty = parse_args(&["k".to_string(), String::new()])
        .unwrap()
        .unwrap();
    assert_eq!(empty.value, "");
    let dashed = parse_args(&["--".to_string(), "--x".to_string(), "v".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(dashed.key, "--x");
}

#[test]
fn r6_core34_runtime_config_parses_and_coerces() {
    let map = parse_runtime_config(&[
        "editor=nano".to_string(),
        "history.immutable=yes".to_string(),
        "load=[\"a\"]".to_string(),
        "mystery=raw".to_string(),
    ])
    .unwrap();
    assert_eq!(map["editor"], serde_json::json!("nano"));
    assert_eq!(map["history.immutable"], serde_json::json!(true));
    assert_eq!(map["load"], serde_json::json!(["a"]));
    assert_eq!(map["mystery"], serde_json::json!("raw"));
    // Split on the first `=` only.
    let map = parse_runtime_config(&["editor=a=b".to_string()]).unwrap();
    assert_eq!(map["editor"], serde_json::json!("a=b"));
    // Later pairs win, like `setRuntimeConfig()` overwrites.
    let map = parse_runtime_config(&["editor=a".to_string(), "editor=b".to_string()]).unwrap();
    assert_eq!(map["editor"], serde_json::json!("b"));
    // Missing `=` and bad types error with the stock texts.
    assert_eq!(
        parse_runtime_config(&["lone".to_string()]).unwrap_err(),
        "Argument was 'lone', but must be 'name=value'. For example, history.immutable=true"
    );
    assert_eq!(
        parse_runtime_config(&["history.immutable=maybe".to_string()]).unwrap_err(),
        "Type of setting 'history.immutable' must be boolean, like 'true' or 'false'."
    );
    assert!(
        parse_runtime_config(&["aliases=x".to_string()])
            .unwrap_err()
            .contains("Use \"arc alias\"")
    );
}

#[test]
fn r6_core34_cli_flags_parse() {
    let opts = parse_args(&[
        "--config".to_string(),
        "a=1".to_string(),
        "--config-file=f".to_string(),
        "--arcrc-file".to_string(),
        "g".to_string(),
        "k".to_string(),
        "v".to_string(),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(opts.config, ["a=1"]);
    assert_eq!(opts.config_files, ["f"]);
    assert_eq!(opts.arcrc_file.as_deref(), Some("g"));
    assert_eq!(opts.key, "k");
    assert!(parse_args(&["--arcrc-file".to_string()]).is_err());
}

#[test]
fn r4_core49_set_config_usage_has_no_dump() {
    // Argument errors render `Usage Exception:` with exit 1 and no
    // usage dump (parse fails before any disk access).
    assert_eq!(
        run(&["--bogus".to_string()]),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(run(&[]), std::process::ExitCode::FAILURE);
}

#[test]
fn run_rejects_malformed_runtime_config() {
    // `--config` validates before anything else: a pair without `=`
    // fails the run with exit 1 and no disk access.
    assert_eq!(
        run(&[
            "--config".to_string(),
            "bogus".to_string(),
            "key".to_string(),
            "value".to_string()
        ]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_local_rejects_alias_keys_before_writing() {
    // `aliases` keys always fail coercion, so the `--local` run errors
    // before touching working-copy config: the local arm runs without
    // writing to the real checkout.
    assert_eq!(
        run(&[
            "--local".to_string(),
            "aliases".to_string(),
            "x".to_string()
        ]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn run_rejects_missing_arcrc_file_override() {
    // A `--arcrc-file` that names no file fails user-file resolution
    // before any read or write.
    assert_eq!(
        run(&[
            "--arcrc-file".to_string(),
            "definitely/missing/arcrc".to_string(),
            "k".to_string(),
            "v".to_string()
        ]),
        std::process::ExitCode::FAILURE
    );
}

#[test]
fn local_writes_reject_parentless_paths() {
    // A path with no parent skips directory creation; the write itself
    // then fails because the empty path names no file.
    let map = serde_json::Map::new();
    assert!(write_local_file(Path::new(""), &map).is_err());
}
#[test]
fn r10_set_config_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "SET-CONFIG WORKFLOW\n\
         \n\
         set-config\n\
         \x20     set-config [options] -- name value\n\
         \n\
         \x20         Supports: cli\n\
         \x20         Sets an arc configuration option.\n\
         \n\
         \x20         Options are either user (apply to all arc commands you invoke\n\
         \x20         from the current user) or local (apply only to the current working\n\
         \x20         copy). By default, user configuration is written. Use --local\n\
         \x20         to write local configuration.\n\
         \n\
         \x20         User values are written to '~/.arcrc' on Linux and Mac OS X, and an\n\
         \x20         undisclosed location on Windows. Local values are written to an arc\n\
         \x20         directory under either .git, .hg, or .svn as appropriate.\n\
         \n\
         \x20     --local\n\
         \x20         Set a local config value instead of a user one.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("SET-CONFIG WORKFLOW"), "{}", usage());
}

#[test]
fn b21_set_config_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `set-config`: it is a classic
    // workflow, so PHP runs it via the `scripts/arcanist.php`
    // re-exec, which never requires `--`. Parsing takes no TTY
    // input, so piped-style argv parses directly, and `--help`
    // without `--` still succeeds.
    assert!(
        parse_args(&["k".to_string(), "v".to_string()])
            .unwrap()
            .is_some()
    );
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    let text = usage_text(false);
    assert!(text.contains("SET-CONFIG WORKFLOW"), "{text}");
}

#[test]
fn b23_set_config_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help set-config`.
    // Extensions stay undocumented.
    let text = usage_text(false);
    assert!(text.starts_with("SET-CONFIG WORKFLOW\n"), "{text}");
    assert!(text.contains("Sets an arc configuration option."), "{text}");
    assert!(text.contains("--local"), "{text}");
    assert!(!text.contains("--arcrc-file"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
