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

    let opts = args_of(&["--as", "-", "--", "F7"]);
    assert_eq!(opts.file_id, 7);
    assert_eq!(opts.symbol, "F7");
    assert_eq!(opts.save_as, Some("-".to_string()));

    let opts = args_of(&["99"]);
    assert_eq!(opts.file_id, 99);
    assert_eq!(opts.symbol, "99");
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
    assert_eq!(parse_file_symbol("99").unwrap(), 99);
    assert_eq!(parse_file_symbol("F007").unwrap(), 7);
    // PHP `/^F?\d+$/`: uppercase `F` only, nothing else.
    assert!(parse_file_symbol("f4").is_err());
    assert!(parse_file_symbol(" 99").is_err());
    assert!(parse_file_symbol("99 ").is_err());
    assert!(parse_file_symbol("F").is_err());
    assert!(parse_file_symbol("Fx").is_err());
    assert!(parse_file_symbol("").is_err());
    assert_eq!(
        parse_file_symbol("xyz").unwrap_err(),
        "Specify file by ID, e.g. F123."
    );
    // PHP `(int)` casts saturate instead of failing.
    assert_eq!(
        parse_file_symbol("99999999999999999999999").unwrap(),
        i64::MAX as u64
    );
}

#[test]
fn counts_format_with_thousands_separators() {
    assert_eq!(format_count(0), "0");
    assert_eq!(format_count(999), "999");
    assert_eq!(format_count(1000), "1,000");
    assert_eq!(format_count(1234567), "1,234,567");
}

#[test]
fn timeouts_grow_with_expected_size() {
    assert_eq!(download_timeout(0), std::time::Duration::from_secs(120));
    assert_eq!(
        download_timeout(32 * 1024),
        std::time::Duration::from_secs(121)
    );
    assert_eq!(
        download_timeout(4 * 1024 * 1024),
        std::time::Duration::from_secs(248)
    );
}

#[test]
fn readable_paths_show_relative_under_dir() {
    let dir = std::env::temp_dir().join("arc-rs-download-readable");
    assert_eq!(readable_path(&dir, &dir.join("report.txt")), "report.txt");
    let outside = std::env::temp_dir().join("elsewhere.bin");
    assert_eq!(readable_path(&dir, &outside), outside.display().to_string());
}

#[test]
fn size_mismatch_uses_grouped_counts() {
    assert_eq!(
        size_mismatch_error(5, 100000),
        "Downloaded file size (5 bytes) does not match expected file size (100,000 bytes). \
         This download may be incomplete or corrupt."
    );
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

#[test]
fn status_lines_carry_no_markup() {
    let downloading = format_downloading("report.txt", 12_345, "report.txt");
    assert_eq!(
        downloading,
        "Downloading \"report.txt\" (12,345 byte(s)) to \"report.txt\"...\n"
    );
    let saved = format_saved("report.txt", "report.txt");
    assert_eq!(saved, "Saved \"report.txt\" as \"report.txt\".\n");
    for line in [&downloading, &saved] {
        assert!(!line.contains('\x1b'), "{line}");
        assert!(!line.contains("**"), "{line}");
    }
}
