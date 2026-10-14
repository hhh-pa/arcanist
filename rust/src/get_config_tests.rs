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
fn metadata_matches_php_settings() {
    assert!(
        help_for("phabricator.uri")
            .unwrap()
            .contains("specific server")
    );
    assert!(help_for("nope").is_none());
    assert_eq!(example_for("editor"), Some("\"nano\""));
    assert_eq!(example_for("aliases"), None);
    assert_eq!(example_for("nope"), None);
    assert_eq!(setting_type("history.immutable"), "bool");
    assert_eq!(setting_type("load"), "list");
    assert_eq!(setting_type("editor"), "string");
    assert_eq!(setting_type("aliases"), "aliases");
    assert_eq!(setting_type("nope"), "wild");
    assert_eq!(legacy_name("phabricator.uri"), Some("conduit_uri"));
    assert_eq!(legacy_name("load"), Some("phutil_libraries"));
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
    assert_eq!(format_value("k", &serde_json::json!(7)), "7");
}

#[test]
fn project_config_walks_up_and_validates() {
    let tmp = TempDir::new("project");
    let child = tmp.path.join("a").join("b");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(
        tmp.path.join("a").join(".arcconfig"),
        r#"{"phabricator.uri": "https://x.example/"}"#,
    )
    .unwrap();
    let (file, map) = read_project_config(&child).unwrap();
    assert_eq!(file.unwrap().file_name().unwrap(), ".arcconfig");
    assert_eq!(
        map["phabricator.uri"],
        serde_json::json!("https://x.example/")
    );
    let (none, empty) = read_project_config(&tmp.path).unwrap();
    assert!(none.is_none() && empty.is_empty());

    std::fs::write(tmp.path.join(".arcconfig"), "not json").unwrap();
    assert!(read_project_config(&tmp.path).is_err());
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
    assert_eq!(map["aliases"], serde_json::json!({"a": "b"}));
    assert!(!map.contains_key("hosts"));

    std::fs::write(&missing, r#"{"hosts": {}}"#).unwrap();
    assert!(read_user_file(&missing).unwrap().is_empty());
    std::fs::write(&missing, "nope").unwrap();
    assert!(read_user_file(&missing).is_err());
    std::fs::write(&missing, "[1]").unwrap();
    assert!(read_user_file(&missing).is_err());
    std::fs::write(&missing, r#"{"config": 7}"#).unwrap();
    assert!(read_user_file(&missing).is_err());
}

#[test]
fn system_file_missing_is_empty_and_invalid_fails() {
    let tmp = TempDir::new("system");
    let missing = tmp.path.join("arcconfig");
    assert!(read_system_config_from(&missing).unwrap().is_empty());
    std::fs::write(&missing, "nope").unwrap();
    assert!(read_system_config_from(&missing).is_err());
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
fn rendering_shows_values_sources_and_details() {
    let tmp = TempDir::new("render");
    std::fs::write(tmp.path.join(".arcconfig"), r#"{"editor": "nano"}"#).unwrap();
    let sources = load_sources(&tmp.path, None).unwrap();
    let brief = render("editor", &sources, false);
    assert!(brief.contains("editor"), "{brief}");
    assert!(brief.contains("\"nano\""), "{brief}");
    assert!(brief.contains("project"), "{brief}");
    let detailed = render("editor", &sources, true);
    assert!(detailed.contains("interactive editor"), "{detailed}");
    assert!(detailed.contains("Local Value"), "{detailed}");
    assert!(detailed.contains("Project Value"), "{detailed}");
    let unknown = render("bogus.key", &sources, true);
    assert!(unknown.contains("not recognized"), "{unknown}");
    let all = render_all(&["editor".to_string()], &sources, false);
    assert!(all.contains("--show-source"), "{all}");
    let multi = render_all(&["a".to_string(), "b".to_string()], &sources, false);
    assert!(
        multi.ends_with("(Run with --show-source for more details.)\n"),
        "{multi}"
    );
    assert!(!render_all(&["a".to_string()], &sources, true).contains("Run with"));
}

#[test]
fn args_accept_flags_separator_and_help() {
    assert_eq!(
        parse_args(&[]).unwrap().unwrap(),
        Options {
            show_source: false,
            keys: vec![]
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
