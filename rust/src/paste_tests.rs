use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn args_of(words: &[&str]) -> Options {
    let ParseOutcome::Args(opts) = parse_args(&argv(words)) else {
        panic!("expected args for {words:?}");
    };
    opts
}

#[test]
fn parses_create_options() {
    let opts = args_of(&["--title", "t", "--lang", "rust", "--input", "f.txt"]);
    assert_eq!(opts.title, Some("t".to_string()));
    assert_eq!(opts.lang, Some("rust".to_string()));
    assert_eq!(opts.input, Some("f.txt".to_string()));
    assert_eq!(opts.symbol, None);
}

#[test]
fn parses_read_symbol() {
    let opts = args_of(&["P123"]);
    assert_eq!(opts.symbol, Some("P123".to_string()));
}

#[test]
fn rejects_read_mode_creation_flags() {
    for words in [
        &["P1", "--lang", "x"][..],
        &["P1", "--title", "x"][..],
        &["P1", "--input", "f"][..],
        &["P1", "--browse"][..],
    ] {
        let ParseOutcome::Error(message) = parse_args(&argv(words)) else {
            panic!("expected error for {words:?}");
        };
        assert!(message.contains("not supported when reading"), "{message}");
    }
    assert!(matches!(
        parse_args(&argv(&["P1", "P2"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn defers_browse_on_create() {
    let ParseOutcome::NotYetPorted(message) = parse_args(&argv(&["--browse"])) else {
        panic!("expected not-yet-ported for --browse");
    };
    assert!(message.contains("not yet ported"), "{message}");
}

#[test]
fn parses_paste_symbols() {
    assert_eq!(
        parse_paste_symbol("P123").unwrap(),
        PasteSymbol::Ids(vec![123])
    );
    assert_eq!(parse_paste_symbol("p4").unwrap(), PasteSymbol::Ids(vec![4]));
    assert_eq!(
        parse_paste_symbol("PHID-PSTE-abc").unwrap(),
        PasteSymbol::Phids(vec!["PHID-PSTE-abc".to_string()])
    );
    assert!(parse_paste_symbol("Px").is_err());
    assert!(parse_paste_symbol("").is_err());
}

#[test]
fn absolutizes_uris() {
    assert_eq!(
        absolute_uri(Some("https://phab.example/api/"), "/P9"),
        "https://phab.example/P9"
    );
    assert_eq!(absolute_uri(None, "/P9"), "/P9");
}

#[test]
fn reads_created_ids_and_phids() {
    assert_eq!(created_id_of(&serde_json::json!(12)), Some(12));
    assert_eq!(created_id_of(&serde_json::json!("34")), Some(34));
    assert_eq!(created_id_of(&serde_json::json!({"id": 5})), Some(5));
    assert_eq!(
        created_id_of(&serde_json::json!({"object": {"id": "6"}})),
        Some(6)
    );
    assert_eq!(created_id_of(&serde_json::json!({"x": 1})), None);
    assert_eq!(
        created_phid_of(&serde_json::json!({"object": {"phid": "PHID-PSTE-1"}})),
        Some("PHID-PSTE-1".to_string())
    );
    assert_eq!(
        created_uri_of(&serde_json::json!({"uri": "/P1"})),
        Some("/P1".to_string())
    );
}
