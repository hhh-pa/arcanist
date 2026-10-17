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
            std::env::temp_dir().join(format!("arc-rs-getcfg-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

fn empty_sources() -> Sources {
    Sources {
        local: serde_json::Map::new(),
        project: serde_json::Map::new(),
        user: serde_json::Map::new(),
        system: serde_json::Map::new(),
        default: default_config(),
        project_file: None,
    }
}

#[test]
fn keys_cover_builtins_in_sorted_order() {
    let keys = known_keys();
    assert_eq!(keys.len(), 15);
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
    for key in [
        "aliases",
        "arc.autostash",
        "history.immutable",
        "load",
        "phabricator.uri",
        "repository.callsign",
    ] {
        assert!(keys.contains(&key), "missing {key}");
    }
}

#[test]
fn settings_table_matches_php_verbatim() {
    // (key, type, help, example, legacy) traced to
    // `ArcanistSettings::getOptions()`.
    type ExpectedSetting<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, Option<&'a str>);
    let expected: &[ExpectedSetting] = &[
        (
            "aliases",
            "aliases",
            "Configured command aliases. Use \"arc alias\" to define aliases.",
            None,
            None,
        ),
        (
            "arc.autostash",
            "bool",
            "Whether arc should permit the automatic stashing of changes in the working directory when requiring a clean working copy. This option should only be used when users understand how to restore their working directory from the local stash if an operation causes an unrecoverable error.",
            Some("false"),
            None,
        ),
        (
            "arc.land.onto.default",
            "string",
            "The name of the default branch to land changes onto when `arc land` is run.",
            Some("\"develop\""),
            None,
        ),
        (
            "base",
            "string",
            "Base commit ruleset to invoke when determining the start of a commit range. See \"Arcanist User Guide: Commit Ranges\" for details.",
            Some("\"arc:amended, arc:prompt\""),
            None,
        ),
        (
            "browser",
            "string",
            "Command to use to invoke a web browser.",
            Some("\"gnome-www-browser\""),
            None,
        ),
        (
            "default",
            "string",
            "The URI of a server to connect to by default, if arc is run in a project without a configured URI or run outside of a project.",
            Some("\"http://devtools.example.com/\""),
            None,
        ),
        (
            "editor",
            "string",
            "Command to use to invoke an interactive editor, like `nano` or `vim`. This setting overrides the EDITOR environmental variable.",
            Some("\"nano\""),
            None,
        ),
        (
            "events.listeners",
            "list",
            "List of event listener classes to install at startup.",
            Some("[\"ExampleEventListener\"]"),
            None,
        ),
        (
            "history.immutable",
            "bool",
            "If true, arc will never change repository history (e.g., through amending or rebasing). Defaults to true in Mercurial and false in Git. This setting has no effect in Subversion.",
            Some("false"),
            Some("immutable_history"),
        ),
        (
            "https.cabundle",
            "string",
            "Path to a custom CA bundle file to be used for cURL calls. This is used primarily when your conduit endpoint is behind HTTPS signed by your organization's internal CA.",
            Some("support/yourca.pem"),
            None,
        ),
        (
            "lint.engine",
            "string",
            "The name of a default lint engine to use, if no lint engine is specified by the current project.",
            Some("\"ExampleLintEngine\""),
            Some("lint_engine"),
        ),
        (
            "load",
            "list",
            "A list of paths to phutil libraries that should be loaded at startup. This can be used to make classes available, like lint or unit test engines.",
            Some("[\"/var/arc/customlib/src\"]"),
            Some("phutil_libraries"),
        ),
        (
            "phabricator.uri",
            "string",
            "Associates this working copy with a specific server.",
            Some("\"https://devtools.example.com/\""),
            Some("conduit_uri"),
        ),
        (
            "repository.callsign",
            "string",
            "Associate the working copy with a specific repository. Normally, arc can figure this association out on its own, but if your setup is unusual you can use this option to tell it what the desired value is.",
            Some("\"X\""),
            None,
        ),
        (
            "unit.engine",
            "string",
            "The name of a default unit test engine to use, if no unit test engine is specified by the current project.",
            Some("\"ExampleUnitTestEngine\""),
            Some("unit_engine"),
        ),
    ];
    assert_eq!(expected.len(), known_keys().len());
    for (key, type_name, help, example, legacy) in expected {
        assert_eq!(setting_type(key), *type_name, "type of {key}");
        assert_eq!(help_for(key), Some(*help), "help of {key}");
        assert_eq!(example_for(key), *example, "example of {key}");
        assert_eq!(legacy_name(key), *legacy, "legacy of {key}");
    }
    assert!(help_for("nope").is_none());
    assert_eq!(example_for("nope"), None);
    assert_eq!(setting_type("nope"), "wild");
    assert_eq!(legacy_name("editor"), None);
}

#[test]
fn defaults_match_php() {
    let defaults = default_config();
    assert_eq!(defaults.len(), 3);
    assert_eq!(defaults["load"], serde_json::json!([]));
    assert_eq!(defaults["events.listeners"], serde_json::json!([]));
    assert_eq!(defaults["arc.autostash"], serde_json::json!(false));
}

#[test]
fn values_format_like_php() {
    assert_eq!(format_value("k", &serde_json::json!(null)), "null");
    assert_eq!(format_value("k", &serde_json::json!(true)), "true");
    assert_eq!(format_value("k", &serde_json::json!(false)), "false");
    assert_eq!(format_value("k", &serde_json::json!("x")), "\"x\"");
    assert_eq!(
        format_value("k", &serde_json::json!(["a", "b"])),
        "[\"a\",\"b\"]"
    );
    assert_eq!(format_value("k", &serde_json::json!({"a": 1})), "{\"a\":1}");
    assert_eq!(format_value("k", &serde_json::json!(7)), "7");
    // Slashes stay literal, like PHP's display-time `\/` unescaping.
    assert_eq!(format_value("k", &serde_json::json!(["a/b"])), "[\"a/b\"]");
}

#[test]
fn verbose_help_goes_through_console_wrap() {
    use arc_rs::terminal;
    // Hand-traced through the PHP algorithm: 89 chars at width 74 break
    // after "misspelled", consuming the space.
    assert_eq!(
        terminal::console_wrap(
            "(This configuration value is not recognized by arc. It may be misspelled or out of date.)",
            4,
            true,
        ),
        "    (This configuration value is not recognized by arc. It may be misspelled\n    or out of date.)"
    );
    // Hand-traced: the editor help breaks after "`vim`." (69 + 57 chars).
    assert_eq!(
        terminal::console_wrap(help_for("editor").unwrap(), 4, true),
        "    Command to use to invoke an interactive editor, like `nano` or `vim`.\n    This setting overrides the EDITOR environmental variable."
    );
    // Every builtin help fits 78 columns once wrapped at indent 4.
    for key in known_keys() {
        for line in terminal::console_wrap(help_for(key).unwrap(), 4, true).lines() {
            assert!(line.len() <= 78, "{key}: {line}");
            assert!(line.starts_with("    "), "{key}: {line}");
        }
    }
}

#[test]
fn project_config_reads_vcs_root_and_validates() {
    // T1153 CORE-33: git reads `.arcconfig` only at the working-copy root; a
    // subdir file is invisible, and so is any file outside a repository.
    let tmp = TempDir::new("project");
    let child = tmp.path.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(
        tmp.path.join("a").join(".arcconfig"),
        r#"{"phabricator.uri": "https://sub.example/"}"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://x.example/"}"#,
    )
    .unwrap();
    let (file, map) = read_project_config(&child).unwrap();
    assert_eq!(file.unwrap().file_name().unwrap(), ".arcconfig");
    assert_eq!(
        map["phabricator.uri"],
        serde_json::json!("https://x.example/")
    );
    let plain = TempDir::new("plain");
    std::fs::write(
        plain.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://plain.example/"}"#,
    )
    .unwrap();
    let (none, empty) = read_project_config(&plain.path).unwrap();
    assert!(none.is_none() && empty.is_empty());

    std::fs::write(tmp.path.join(".arcconfig"), "not json").unwrap();
    let err = read_project_config(&tmp.path).unwrap_err();
    assert!(
        err.starts_with("Unable to parse '.arcconfig' file '"),
        "{err}"
    );
    assert!(err.ends_with(".arcconfig'."));
    std::fs::write(tmp.path.join(".arcconfig"), "[1]").unwrap();
    assert!(read_project_config(&tmp.path).is_err());
}

#[test]
fn local_config_is_empty_outside_repos() {
    let tmp = TempDir::new("local");
    assert!(read_local_config(&tmp.path).unwrap().is_empty());
}

#[test]
fn user_file_reads_config_and_aliases() {
    let tmp = TempDir::new("user");
    let missing = tmp.path.join(".arcrc");
    assert!(read_user_file(&missing).unwrap().is_empty());

    std::fs::write(
        &missing,
        r#"{"hosts": {"https://h/api/": {"token": "t"}}, "config": {"editor": "nano"}, "aliases": {"a": "b"}}"#,
    )
    .unwrap();
    let map = read_user_file(&missing).unwrap();
    assert_eq!(map["editor"], serde_json::json!("nano"));
    // T1153 CORE-30: with a `config` section, classic `readUserArcConfig()`
    // returns ONLY the section; top-level `aliases` siblings are dropped.
    assert!(!map.contains_key("aliases"));
    assert!(!map.contains_key("hosts"));

    // Without a `config` section, stock PHP returns the whole file --
    // including `hosts` -- as the user config.
    std::fs::write(&missing, r#"{"hosts": {}}"#).unwrap();
    let whole = read_user_file(&missing).unwrap();
    assert_eq!(whole.len(), 1);
    assert_eq!(whole["hosts"], serde_json::json!({}));

    // PHP `isset()` is false for null too, so a null section also falls
    // back to the whole file.
    std::fs::write(&missing, r#"{"config": null, "editor": "top"}"#).unwrap();
    let whole = read_user_file(&missing).unwrap();
    assert_eq!(whole["editor"], serde_json::json!("top"));
    assert_eq!(whole["config"], serde_json::Value::Null);
    std::fs::write(&missing, "nope").unwrap();
    // PHP hardcodes the filename in this error.
    assert_eq!(
        read_user_file(&missing).unwrap_err(),
        "Your '~/.arcrc' file is not a valid JSON file."
    );
    std::fs::write(&missing, "[1]").unwrap();
    assert!(read_user_file(&missing).is_err());
    std::fs::write(&missing, r#"{"config": 7}"#).unwrap();
    assert!(read_user_file(&missing).is_err());
}

#[test]
fn user_config_without_section_matches_stock_php() {
    // A legacy `~/.arcrc` with credentials but no `config` section:
    // stock PHP treats the whole file as user config, hosts included.
    let tmp = TempDir::new("wholefile");
    let arcrc = tmp.path.join(".arcrc");
    std::fs::write(
        &arcrc,
        r#"{"hosts": {"https://h/api/": {"token": "t"}}, "editor": "top", "aliases": {"a": "b"}}"#,
    )
    .unwrap();
    let sources = load_sources(&tmp.path, Some(&arcrc)).unwrap();
    assert_eq!(
        current_value("hosts", &sources).unwrap(),
        (
            "user",
            serde_json::json!({"https://h/api/": {"token": "t"}})
        )
    );
    assert_eq!(
        current_value("editor", &sources).unwrap(),
        ("user", serde_json::json!("top"))
    );
    assert!(all_keys(&sources).contains(&"hosts".to_string()));
    assert_eq!(
        render_ansi("hosts", &sources, false, false),
        "hosts\n\n       Current Value: {\"https://h/api/\":{\"token\":\"t\"}}\n      Current Source: user\n"
    );
}

#[test]
fn system_file_missing_is_empty_and_invalid_fails() {
    let tmp = TempDir::new("system");
    let missing = tmp.path.join("arcconfig");
    assert!(read_system_config_from(&missing).unwrap().is_empty());
    std::fs::write(&missing, "nope").unwrap();
    let err = read_system_config_from(&missing).unwrap_err();
    assert!(err.starts_with("Your '"), "{err}");
    assert!(err.ends_with("file is not a valid JSON file."), "{err}");
    let path = system_config_path();
    if cfg!(windows) {
        assert!(path.ends_with(Path::new("Phabricator").join("Arcanist").join("config")));
    } else {
        assert_eq!(path, PathBuf::from("/etc/arcconfig"));
    }
    assert!(read_system_config().is_ok());
}

#[test]
fn resolution_follows_source_precedence() {
    let tmp = TempDir::new("resolve");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"editor": "proj", "browser": "proj"}"#,
    )
    .unwrap();
    let user_file = tmp.path.join(".arcrc");
    std::fs::write(&user_file, r#"{"config": {"editor": "user"}}"#).unwrap();
    let sources = load_sources(&tmp.path, Some(&user_file)).unwrap();
    assert!(sources.project_file.is_some());
    assert!(sources.local.is_empty());
    assert_eq!(
        current_value("editor", &sources).unwrap(),
        ("project", serde_json::json!("proj"))
    );
    assert_eq!(
        current_value("browser", &sources).unwrap(),
        ("project", serde_json::json!("proj"))
    );
    assert_eq!(
        value_at("editor", "user", &sources).unwrap(),
        serde_json::json!("user")
    );
    assert!(value_at("editor", "local", &sources).is_none());
    assert!(current_value("never-set", &sources).is_none());
    assert_eq!(
        current_value("arc.autostash", &sources).unwrap(),
        ("default", serde_json::json!(false))
    );
    let keys = all_keys(&sources);
    assert!(keys.contains(&"editor".to_string()));
    assert!(keys.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn project_legacy_names_apply() {
    let tmp = TempDir::new("legacy");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"conduit_uri": "https://old.example/"}"#,
    )
    .unwrap();
    let sources = load_sources(&tmp.path, None).unwrap();
    assert_eq!(
        current_value("phabricator.uri", &sources).unwrap(),
        ("project", serde_json::json!("https://old.example/"))
    );
    assert!(value_at("phabricator.uri", "user", &sources).is_none());
}

#[test]
fn brief_render_matches_php_bytes() {
    let tmp = TempDir::new("brief");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(tmp.path.join(".arcconfig"), r#"{"editor": "nano"}"#).unwrap();
    let sources = load_sources(&tmp.path, None).unwrap();
    assert_eq!(
        render_ansi("editor", &sources, false, false),
        "editor\n\n       Current Value: \"nano\"\n      Current Source: project\n"
    );
    assert_eq!(
        render_ansi("never-set", &sources, false, false),
        "never-set\n\n       Current Value: -\n      Current Source: -\n"
    );
    let all = render_all_ansi(&["editor".to_string()], &sources, false, false);
    assert_eq!(
        all,
        "editor\n\n       Current Value: \"nano\"\n      Current Source: project\n(Run with --verbose for more details.)\n"
    );
}

#[test]
fn verbose_render_matches_php_bytes() {
    let tmp = TempDir::new("verbose");
    // T1153 CORE-33: the project file only counts at a working-copy root.
    std::fs::create_dir_all(tmp.path.join(".git")).unwrap();
    std::fs::write(tmp.path.join(".arcconfig"), r#"{"editor": "nano"}"#).unwrap();
    let sources = load_sources(&tmp.path, None).unwrap();
    // Source labels keep PHP's lowercase names; the example row prints
    // empty (not `-`) when the key has no example.
    assert_eq!(
        render_ansi("editor", &sources, true, false),
        "editor\n\
         \n\
         \x20   Command to use to invoke an interactive editor, like `nano` or `vim`.\n\
         \x20   This setting overrides the EDITOR environmental variable.\n\
         \n\
         \x20      Example Value: \"nano\"\n\
         \n\
         \x20      Current Value: \"nano\"\n\
         \x20     Current Source: project\n\
         \n\
         \x20        local Value: -\n\
         \x20      project Value: \"nano\"\n\
         \x20         user Value: -\n\
         \x20       system Value: -\n\
         \x20      default Value: -\n"
    );
    assert_eq!(
        render_ansi("bogus.key", &sources, true, false),
        "bogus.key\n\
         \n\
         \x20   (This configuration value is not recognized by arc. It may be misspelled\n\
         \x20   or out of date.)\n\
         \n\
         \x20      Example Value: \n\
         \n\
         \x20      Current Value: -\n\
         \x20     Current Source: -\n\
         \n\
         \x20        local Value: -\n\
         \x20      project Value: -\n\
         \x20         user Value: -\n\
         \x20       system Value: -\n\
         \x20      default Value: -\n"
    );
    assert!(!render_all_ansi(&["a".to_string()], &sources, true, false).contains("Run with"));
}

#[test]
fn ansi_render_bolds_only_the_header() {
    let sources = empty_sources();
    assert_eq!(
        render_ansi("editor", &sources, false, true),
        "\x1b[1meditor\x1b[m\n\n       Current Value: -\n      Current Source: -\n"
    );
}

#[test]
fn multi_key_output_separates_blocks() {
    let sources = empty_sources();
    let multi = render_all_ansi(&["a".to_string(), "b".to_string()], &sources, false, false);
    assert!(
        multi.ends_with("(Run with --verbose for more details.)\n"),
        "{multi}"
    );
    // Each brief block ends with a blank line separator when multi.
    assert_eq!(
        multi,
        "a\n\n       Current Value: -\n      Current Source: -\n\nb\n\n       Current Value: -\n      Current Source: -\n\n(Run with --verbose for more details.)\n"
    );
}

#[test]
fn args_accept_flags_separator_and_help() {
    assert_eq!(
        parse_args(&[]).unwrap().unwrap(),
        Options {
            show_source: false,
            keys: vec![],
            config: vec![],
            config_files: vec![],
            arcrc_file: None,
        }
    );
    assert!(
        parse_args(&["--show-source".to_string()])
            .unwrap()
            .unwrap()
            .show_source
    );
    assert!(
        parse_args(&["--verbose".to_string()])
            .unwrap()
            .unwrap()
            .show_source
    );
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    assert!(parse_args(&["-h".to_string()]).unwrap().is_none());
    let opts = parse_args(&["--".to_string(), "--weird".to_string()])
        .unwrap()
        .unwrap();
    assert_eq!(opts.keys, ["--weird"]);
    assert!(parse_args(&["--bogus".to_string()]).is_err());
}

#[test]
fn r6_core33_find_vcs_root_prefers_innermost_git_first() {
    let tmp = TempDir::new("vcsroot");
    assert_eq!(find_vcs_root(&tmp.path), None);
    // Innermost level wins.
    let outer = tmp.path.join("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(inner.join(".hg")).unwrap();
    std::fs::create_dir_all(outer.join(".git")).unwrap();
    assert_eq!(find_vcs_root(&inner), Some((VcsKind::Hg, inner.clone())));
    assert_eq!(find_vcs_root(&outer), Some((VcsKind::Git, outer.clone())));
    // `.git` wins over `.hg`/`.svn` in one directory, like PHP's `$try`.
    std::fs::create_dir_all(inner.join(".git")).unwrap();
    std::fs::create_dir_all(inner.join(".svn")).unwrap();
    assert_eq!(find_vcs_root(&inner), Some((VcsKind::Git, inner.clone())));
    // A `.git` file (worktree/submodule pointer) counts like PHP's
    // `pathExists()`.
    let work = tmp.path.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(".git"), "gitdir: /elsewhere\n").unwrap();
    assert_eq!(find_vcs_root(&work), Some((VcsKind::Git, work)));
}

#[test]
fn r6_core34_merged_value_precedence() {
    let mut sources = empty_sources();
    sources
        .system
        .insert("k".to_string(), serde_json::json!("system"));
    sources
        .user
        .insert("k".to_string(), serde_json::json!("user"));
    sources
        .project
        .insert("k".to_string(), serde_json::json!("project"));
    sources
        .local
        .insert("k".to_string(), serde_json::json!("local"));
    let plain = MergedConfig {
        sources,
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        merged_value("k", &plain).unwrap(),
        ("local", serde_json::json!("local"))
    );
    let mut runtime = serde_json::Map::new();
    runtime.insert("k".to_string(), serde_json::json!("runtime"));
    let with_runtime = MergedConfig {
        sources: plain.sources,
        runtime,
    };
    assert_eq!(
        merged_value("k", &with_runtime).unwrap(),
        (SOURCE_RUNTIME, serde_json::json!("runtime"))
    );
    assert!(merged_value("never-set", &with_runtime).is_none());
    // Project legacy names still resolve through the merge.
    let mut legacy = empty_sources();
    legacy.project.insert(
        "conduit_uri".to_string(),
        serde_json::json!("https://old.example/"),
    );
    let merged = MergedConfig {
        sources: legacy,
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        merged_value("phabricator.uri", &merged).unwrap().0,
        "project"
    );
}

#[test]
fn r6_core34_checked_value_type_errors() {
    let mut sources = empty_sources();
    sources
        .project
        .insert("editor".to_string(), serde_json::json!(42));
    let merged = MergedConfig {
        sources,
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        checked_value("editor", &merged).unwrap_err(),
        "Type of setting 'editor' must be string."
    );
    let mut sources = empty_sources();
    sources
        .user
        .insert("history.immutable".to_string(), serde_json::json!("yes"));
    let merged = MergedConfig {
        sources,
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        checked_value("history.immutable", &merged).unwrap_err(),
        "Type of setting 'history.immutable' must be boolean."
    );
    // A bad lower value errors even below a valid winner, like PHP's
    // `getConfigFromAllSources()`.
    let mut sources = empty_sources();
    sources
        .local
        .insert("load".to_string(), serde_json::json!(["ok"]));
    sources
        .user
        .insert("load".to_string(), serde_json::json!("nope"));
    let merged = MergedConfig {
        sources,
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        checked_value("load", &merged).unwrap_err(),
        "Type of setting 'load' must be list."
    );
    // JSON objects pass `list`: PHP decodes to associative arrays.
    let mut sources = empty_sources();
    sources
        .project
        .insert("load".to_string(), serde_json::json!({"a": "b"}));
    let merged = MergedConfig {
        sources,
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        checked_value("load", &merged).unwrap().unwrap().0,
        "project"
    );
    // Builtin defaults read unchecked.
    let merged = MergedConfig {
        sources: empty_sources(),
        runtime: serde_json::Map::new(),
    };
    assert_eq!(
        checked_value("arc.autostash", &merged).unwrap().unwrap(),
        ("default", serde_json::json!(false))
    );
}

#[test]
fn r6_core34_config_file_shadows_system_user() {
    let tmp = TempDir::new("conffile");
    let user_file = tmp.path.join("arcrc.json");
    std::fs::write(&user_file, r#"{"config": {"editor": "user"}}"#).unwrap();
    let first = tmp.path.join("first.json");
    let second = tmp.path.join("second.json");
    std::fs::write(&first, r#"{"editor": "first"}"#).unwrap();
    std::fs::write(&second, r#"{"editor": "second"}"#).unwrap();
    // Without `--config-file`, the user value shows.
    let merged = load_merged(&tmp.path, Some(&user_file), &LoadOptions::default()).unwrap();
    assert_eq!(merged.sources.user["editor"], serde_json::json!("user"));
    // With `--config-file`s, user+system go empty and later files win.
    let opts = LoadOptions {
        config_files: vec![first, second, tmp.path.join("missing.json")],
        ..Default::default()
    };
    let merged = load_merged(&tmp.path, Some(&user_file), &opts).unwrap();
    assert!(merged.sources.user.is_empty());
    assert_eq!(
        merged_value("editor", &merged).unwrap(),
        ("system", serde_json::json!("second"))
    );
}

#[test]
fn r6_core34_arcrc_file_redirects_user_source() {
    let tmp = TempDir::new("arcrcfile");
    let custom = tmp.path.join("custom.json");
    std::fs::write(&custom, r#"{"config": {"editor": "custom"}}"#).unwrap();
    let opts = LoadOptions {
        arcrc_file: Some(custom),
        ..Default::default()
    };
    let merged = load_merged(&tmp.path, None, &opts).unwrap();
    assert_eq!(
        merged_value("editor", &merged).unwrap(),
        ("user", serde_json::json!("custom"))
    );
    // A missing custom file errors, like PHP's
    // `setUserConfigurationFileLocation()`.
    let missing = LoadOptions {
        arcrc_file: Some(tmp.path.join("nope.json")),
        ..Default::default()
    };
    assert_eq!(
        load_merged(&tmp.path, None, &missing).unwrap_err(),
        "Custom arcrc file was specified, but it was not found!"
    );
    assert_eq!(
        resolve_user_file(None, Some(Path::new("nope.json"))).unwrap_err(),
        "Custom arcrc file was specified, but it was not found!"
    );
}

#[test]
fn r6_core34_runtime_excluded_from_display() {
    // Stock `get-config` never shows `--config` values: display reads the
    // static sources only.
    let mut runtime = serde_json::Map::new();
    runtime.insert("editor".to_string(), serde_json::json!("rt"));
    runtime.insert("zzz-unknown".to_string(), serde_json::json!("rt"));
    let merged = MergedConfig {
        sources: empty_sources(),
        runtime,
    };
    assert_eq!(merged_value("editor", &merged).unwrap().0, SOURCE_RUNTIME);
    assert!(current_value("editor", &merged.sources).is_none());
    assert!(!all_keys(&merged.sources).contains(&"zzz-unknown".to_string()));
    let text = render_ansi("editor", &merged.sources, true, false);
    assert!(text.contains("Current Value: -"), "{text}");
    assert!(!text.contains("\"rt\""), "{text}");
}

#[test]
fn r6_core34_cli_flags_parse_both_forms() {
    let opts = parse_args(&[
        "--config".to_string(),
        "a=1".to_string(),
        "--config=b=2".to_string(),
        "--config-file".to_string(),
        "f".to_string(),
        "--arcrc-file=g".to_string(),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(opts.config, ["a=1", "b=2"]);
    assert_eq!(opts.config_files, ["f"]);
    assert_eq!(opts.arcrc_file.as_deref(), Some("g"));
    assert!(parse_args(&["--config".to_string()]).is_err());
    let cli = load_options_from_cli(&opts.config, &opts.config_files, opts.arcrc_file.as_deref())
        .unwrap();
    assert_eq!(cli.runtime["a"], serde_json::json!("1"));
    assert_eq!(cli.config_files, [PathBuf::from("f")]);
    assert_eq!(cli.arcrc_file, Some(PathBuf::from("g")));
    assert!(load_options_from_cli(&["lone".to_string()], &[], None).is_err());
}

#[test]
fn core28_toolset_user_file_merges_section_and_top_level() {
    let tmp = TempDir::new("core28");
    let file = tmp.path.join(".arcrc");
    std::fs::write(
        &file,
        r#"{
            "hosts": {"https://h/api/": {"token": "t"}},
            "config": {"editor": "nano", "aliases": ["section"]},
            "aliases": ["top"],
            "prompts": []
        }"#,
    )
    .unwrap();
    let map = read_user_file_toolset(&file).unwrap();
    // Section wins collisions; other top-level keys merge in.
    assert_eq!(map["editor"], serde_json::json!("nano"));
    assert_eq!(map["aliases"], serde_json::json!(["section"]));
    assert_eq!(map["prompts"], serde_json::json!([]));
    // `hosts` and `config` are subtracted like PHP's `$subtract`.
    assert!(!map.contains_key("hosts"));
    assert!(!map.contains_key("config"));

    // Without a section, the whole file minus `hosts`/`config` merges.
    std::fs::write(&file, r#"{"hosts": {}, "aliases": ["top"]}"#).unwrap();
    let map = read_user_file_toolset(&file).unwrap();
    assert_eq!(map["aliases"], serde_json::json!(["top"]));
    assert!(!map.contains_key("hosts"));

    assert!(
        read_user_file_toolset(&tmp.path.join("missing"))
            .unwrap()
            .is_empty()
    );
    std::fs::write(&file, "broken").unwrap();
    assert!(
        read_user_file_toolset(&file)
            .unwrap_err()
            .contains("~/.arcrc")
    );
}

#[test]
fn core30_classic_user_file_drops_all_top_level_siblings() {
    // `user_file_reads_config_and_aliases` pins `aliases`/`hosts`; this
    // pins every other top-level sibling next to a `config` section.
    let tmp = TempDir::new("core30");
    let file = tmp.path.join(".arcrc");
    std::fs::write(
        &file,
        r#"{"config": {"editor": "nano"}, "editor": "top", "prompts": [], "other": 1}"#,
    )
    .unwrap();
    let map = read_user_file(&file).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map["editor"], serde_json::json!("nano"));
}

#[test]
fn core31_project_empty_key_answers_keys_without_legacy() {
    let mut sources = empty_sources();
    sources
        .project
        .insert("".to_string(), serde_json::json!("fallback"));
    // `editor` has no legacy name, so PHP's null-key lookup hits `""`.
    assert_eq!(
        value_at("editor", "project", &sources).unwrap(),
        serde_json::json!("fallback")
    );
    // Unknown keys behave the same (no setting row, no legacy name).
    assert_eq!(
        value_at("whatever.custom", "project", &sources).unwrap(),
        serde_json::json!("fallback")
    );
    // Other sources never consult `""`.
    sources.user.insert("".to_string(), serde_json::json!("u"));
    assert!(value_at("editor", "user", &sources).is_none());
    assert!(value_at("editor", "local", &sources).is_none());
    // A canonical value wins over `""`.
    sources
        .project
        .insert("editor".to_string(), serde_json::json!("vi"));
    assert_eq!(
        value_at("editor", "project", &sources).unwrap(),
        serde_json::json!("vi")
    );
    // A key WITH a legacy name consults only that name, never `""`.
    let mut sources = empty_sources();
    sources
        .project
        .insert("".to_string(), serde_json::json!("fallback"));
    assert!(value_at("phabricator.uri", "project", &sources).is_none());
    sources.project.insert(
        "conduit_uri".to_string(),
        serde_json::json!("https://legacy.example/"),
    );
    assert_eq!(
        value_at("phabricator.uri", "project", &sources).unwrap(),
        serde_json::json!("https://legacy.example/")
    );
}

#[test]
fn core32_local_config_reads_git_hg_and_svn() {
    for meta in [".git", ".hg", ".svn"] {
        let tmp = TempDir::new("core32");
        let child = tmp.path.join("sub");
        std::fs::create_dir_all(&child).unwrap();
        // The marker sits at the working-copy root, like PHP's
        // `$localMetaDir`; reads from a subdirectory find it.
        std::fs::create_dir_all(tmp.path.join(meta).join("arc")).unwrap();
        std::fs::write(
            tmp.path.join(meta).join("arc").join("config"),
            r#"{"editor": "local"}"#,
        )
        .unwrap();
        let map = read_local_config(&child).unwrap();
        assert_eq!(map["editor"], serde_json::json!("local"), "{meta}");
    }
    // Present marker without a config file still reads empty.
    let tmp = TempDir::new("core32empty");
    std::fs::create_dir_all(tmp.path.join(".hg")).unwrap();
    assert!(read_local_config(&tmp.path).unwrap().is_empty());
}

#[test]
fn core33_project_discovery_svn_nearest_hg_root() {
    // Subversion: nearest `.arcconfig` from `dir` up to the root wins.
    let tmp = TempDir::new("core33svn");
    let child = tmp.path.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(tmp.path.join(".svn")).unwrap();
    std::fs::write(
        tmp.path.join(".arcconfig"),
        r#"{"phabricator.uri": "https://root.example/"}"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path.join("a").join(".arcconfig"),
        r#"{"phabricator.uri": "https://near.example/"}"#,
    )
    .unwrap();
    let (file, map) = read_project_config(&child).unwrap();
    assert_eq!(file.unwrap().parent().unwrap(), tmp.path.join("a"));
    assert_eq!(
        map["phabricator.uri"],
        serde_json::json!("https://near.example/")
    );
    // Mercurial: root-only, like git; a subdir file is invisible.
    let tmp = TempDir::new("core33hg");
    let child = tmp.path.join("a");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::create_dir_all(tmp.path.join(".hg")).unwrap();
    std::fs::write(
        tmp.path.join("a").join(".arcconfig"),
        r#"{"phabricator.uri": "https://sub.example/"}"#,
    )
    .unwrap();
    let (none, empty) = read_project_config(&child).unwrap();
    assert!(none.is_none() && empty.is_empty());
}
#[test]
fn r10_get_config_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "GET-CONFIG WORKFLOW\n\
         \n\
         get-config\n\
         \x20     get-config [options] -- [name ...]\n\
         \n\
         \x20         Supports: cli\n\
         \x20         Reads an arc configuration option. With no argument, reads all\n\
         \x20         options.\n\
         \n\
         \x20         With --verbose, shows detailed information about one or more\n\
         \x20         options.\n\
         \n\
         \x20     --verbose\n\
         \x20         Show detailed information about options.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("GET-CONFIG WORKFLOW"), "{}", usage());
}

#[test]
fn b21_get_config_needs_no_terminator() {
    // T1153 B21/B23 VERIFIED for `get-config`: it is a classic
    // workflow, so PHP runs it via the `scripts/arcanist.php`
    // re-exec, which never requires `--`. Parsing takes no TTY
    // input, so piped-style argv parses directly, and `--help`
    // without `--` still succeeds.
    assert!(
        parse_args(&["arc.land.onto".to_string()])
            .unwrap()
            .is_some()
    );
    assert!(parse_args(&["--help".to_string()]).unwrap().is_none());
    let text = usage_text(false);
    assert!(text.contains("GET-CONFIG WORKFLOW"), "{text}");
}

#[test]
fn b23_get_config_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help get-config`.
    let text = usage_text(false);
    assert!(text.starts_with("GET-CONFIG WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Reads an arc configuration option."),
        "{text}"
    );
    assert!(text.contains("--verbose"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}
