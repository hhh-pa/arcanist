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
fn parses_id_and_as() {
    let opts = args_of(&["F123"]);
    assert_eq!(opts.file_id, 123);
    assert_eq!(opts.save_as, None);

    let opts = args_of(&["--as", "-", "--", "f7"]);
    assert_eq!(opts.file_id, 7);
    assert_eq!(opts.save_as, Some("-".to_string()));
}

#[test]
fn rejects_missing_extra_and_bad_symbols() {
    let ParseOutcome::Error(message) = parse_args(&argv(&[])) else {
        panic!("expected error for no file");
    };
    assert!(message.contains("Specify a file"), "{message}");

    assert!(matches!(
        parse_args(&argv(&["F1", "F2"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["xyz"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["F1", "--bogus"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn parses_file_symbols() {
    assert_eq!(parse_file_symbol("F123").unwrap(), 123);
    assert_eq!(parse_file_symbol("f4").unwrap(), 4);
    assert_eq!(parse_file_symbol("99").unwrap(), 99);
    assert!(parse_file_symbol("F").is_err());
    assert!(parse_file_symbol("Fx").is_err());
    assert!(parse_file_symbol("").is_err());
}

#[test]
fn monogram_and_display_name() {
    let file = FileRef {
        id: 5,
        name: String::new(),
        size: 0,
        data_uri: String::new(),
    };
    assert_eq!(file.monogram(), "F5");
    assert_eq!(file.display_name(), "F5");
    let named = FileRef {
        name: "a.txt".to_string(),
        ..file.clone()
    };
    assert_eq!(named.display_name(), "a.txt");
}

#[test]
fn resolves_targets_and_uniquifies() {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-download-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = FileRef {
        id: 1,
        name: "report.txt".to_string(),
        size: 3,
        data_uri: String::new(),
    };

    let first = resolve_target(&dir, None, &file).unwrap();
    assert_eq!(first.file_name().unwrap(), "report.txt");
    std::fs::write(&first, "abc").unwrap();
    let second = resolve_target(&dir, None, &file).unwrap();
    assert_eq!(second.file_name().unwrap(), "report.txt.1");

    let explicit = resolve_target(&dir, Some("out.bin"), &file).unwrap();
    assert_eq!(explicit.file_name().unwrap(), "out.bin");
    std::fs::write(&explicit, "x").unwrap();
    assert!(resolve_target(&dir, Some("out.bin"), &file).is_err());

    std::fs::remove_dir_all(&dir).ok();
}
