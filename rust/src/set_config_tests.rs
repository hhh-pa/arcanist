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
    for raw in ["false", "no", "off", "0", "NO"] {
        assert_eq!(
            coerce_value("history.immutable", raw).unwrap(),
            serde_json::json!(false)
        );
    }
    assert!(coerce_value("history.immutable", "perhaps").is_err());
    assert_eq!(
        coerce_value("load", r#"["a", "b"]"#).unwrap(),
        serde_json::json!(["a", "b"])
    );
    assert!(coerce_value("load", "a, b").is_err());
    assert!(coerce_value("load", r#"{"a": 1}"#).is_err());
    assert_eq!(
        coerce_value("editor", "nano").unwrap(),
        serde_json::json!("nano")
    );
    assert_eq!(
        coerce_value("custom.key", "v").unwrap(),
        serde_json::json!("v")
    );
    assert!(
        coerce_value("aliases", "x")
            .unwrap_err()
            .contains("arc alias")
    );
}

#[test]
fn display_and_warnings() {
    assert_eq!(format_value(&serde_json::json!(true)), "true");
    assert_eq!(format_value(&serde_json::json!("x")), "\"x\"");
    assert_eq!(format_value(&serde_json::json!([1])), "[1]");
    assert!(warn_if_unknown("editor").is_none());
    assert!(warn_if_unknown("bogus").unwrap().contains("not recognized"));
}

#[test]
fn user_doc_handles_missing_and_broken_files() {
    let tmp = TempDir::new("doc");
    let path = tmp.path.join(".arcrc");
    assert_eq!(read_user_doc(&path).unwrap(), serde_json::json!({}));
    std::fs::write(&path, "nope").unwrap();
    assert!(read_user_doc(&path).is_err());
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

    let set = apply_user(&path, "editor", "nano").unwrap();
    assert!(
        set.contains("Set key \"editor\" = \"nano\" in user config."),
        "{set}"
    );
    let again = apply_user(&path, "editor", "vim").unwrap();
    assert!(again.contains("(was \"nano\")"), "{again}");
    let doc = read_user_doc(&path).unwrap();
    assert_eq!(doc["config"]["editor"], serde_json::json!("vim"));
    assert_eq!(
        doc["hosts"]["https://h/api/"]["token"],
        serde_json::json!("t")
    );

    let deleted = apply_user(&path, "editor", "").unwrap();
    assert!(deleted.contains("Deleted key \"editor\""), "{deleted}");
    assert!(deleted.contains("(was \"vim\")"), "{deleted}");
    let missing = apply_user(&path, "editor", "").unwrap();
    assert!(!missing.contains("(was"), "{missing}");

    assert!(apply_user(&path, "history.immutable", "perhaps").is_err());
    assert!(apply_user(&path, "aliases", "x").is_err());
    std::fs::write(&path, r#"{"config": 7}"#).unwrap();
    assert!(apply_user(&path, "editor", "x").is_err());
}

#[test]
fn local_files_roundtrip_at_explicit_roots() {
    let tmp = TempDir::new("local");
    let root = tmp.path.join("repo");
    let path = local_config_path(&root);
    assert!(path.ends_with(Path::new(".git").join("arc").join("config")));
    assert!(read_local_file(&path).unwrap().is_empty());

    let set = apply_local_at(&root, "history.immutable", "true").unwrap();
    assert!(set.contains("in local config"), "{set}");
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
    assert!(read_local_file(&path).is_err());
    std::fs::write(&path, "[1]").unwrap();
    assert!(read_local_file(&path).is_err());

    let deleted = apply_local_at(&root, "editor", "").unwrap_err();
    assert!(deleted.contains("JSON"), "{deleted}");
}

#[test]
fn local_apply_requires_a_repo() {
    let tmp = TempDir::new("norepo");
    let err = apply_local(&tmp.path, "editor", "x").unwrap_err();
    assert!(err.contains("working copy"), "{err}");
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
            value: "v".to_string()
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
