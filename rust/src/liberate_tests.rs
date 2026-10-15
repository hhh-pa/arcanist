use super::*;
use std::io::BufReader;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn args_of(words: &[&str]) -> Options {
    let ParseOutcome::Args(opts) = parse_args(&argv(words)) else {
        panic!("expected args for {words:?}");
    };
    opts
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arc-rs-liberate-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn parses_path_and_clean() {
    let opts = args_of(&[]);
    assert!(!opts.clean);
    assert_eq!(opts.path, None);

    let opts = args_of(&["--clean", "lib/"]);
    assert!(opts.clean);
    assert_eq!(opts.path, Some("lib/".to_string()));

    assert!(matches!(
        parse_args(&argv(&["a", "b"])),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args(&argv(&["--bogus"])),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn rejects_second_path_with_full_text() {
    let ParseOutcome::Error(message) = parse_args(&argv(&["a", "b"])) else {
        panic!("expected an error");
    };
    assert_eq!(message, one_path_error());
    assert!(message.contains("Provide only one path to \"arc liberate\"."));
    assert!(message.contains("create or update a library"));
}

#[test]
fn renders_readable_paths() {
    let dir = scratch("readable");
    assert_eq!(readable_path(&dir, &dir.join("liba")), "liba");
    let outside = std::env::temp_dir().join("arc-rs-liberate-elsewhere");
    assert_eq!(readable_path(&dir, &outside), outside.display().to_string());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parses_map_versions() {
    assert_eq!(parse_map_version("@phutil-library-version 2\n"), Some(2));
    // PHP requires exactly one space before the digits.
    assert_eq!(parse_map_version("x @phutil-library-version   12 y"), None);
    assert_eq!(parse_map_version("no marker here"), None);
    assert_eq!(parse_map_version("@phutil-library-version x"), None);
    // Later markers still match when an early one has no digits.
    assert_eq!(
        parse_map_version("@phutil-library-version x @phutil-library-version 4"),
        Some(4)
    );
    // Huge versions saturate like PHP's `(int)` cast.
    assert_eq!(
        parse_map_version("@phutil-library-version 99999999999999999999999"),
        Some(i64::MAX)
    );
}

#[test]
fn validates_library_names() {
    assert!(is_valid_library_name("my-lib"));
    assert!(!is_valid_library_name("My-Lib"));
    assert!(!is_valid_library_name("my_lib"));
    assert!(!is_valid_library_name(""));
    assert!(!is_valid_library_name("lib 1"));
}

#[test]
fn defaults_missing_map_to_version_one() {
    let dir = scratch("version");
    assert_eq!(library_version(&dir).unwrap(), 1);
    std::fs::write(dir.join(MAP_FILE), "@phutil-library-version 2").unwrap();
    assert_eq!(library_version(&dir).unwrap(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scans_one_level_for_libraries() {
    let dir = scratch("scan");
    std::fs::create_dir_all(dir.join("liba")).unwrap();
    std::fs::write(dir.join("liba").join(INIT_FILE), "<?php\n").unwrap();
    std::fs::create_dir_all(dir.join("plain")).unwrap();
    std::fs::write(dir.join(INIT_FILE), "<?php\n").unwrap();

    let found = scan_libraries(&dir).unwrap();
    assert_eq!(found, vec![dir.join("liba")]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn creates_init_file_in_existing_directory() {
    let dir = scratch("create");
    let lib = dir.join("newlib");
    std::fs::create_dir_all(&lib).unwrap();

    let mut input = BufReader::new(std::io::Cursor::new("Bad Name!\nmy-lib\n"));
    assert!(ensure_library(&lib, &mut input).unwrap());
    let init = std::fs::read_to_string(lib.join(INIT_FILE)).unwrap();
    assert!(init.contains("phutil_register_library('my-lib'"), "{init}");

    // Second call is a no-op.
    let mut input = BufReader::new(std::io::Cursor::new(""));
    assert!(!ensure_library(&lib, &mut input).unwrap());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn declines_missing_directory() {
    let dir = scratch("decline");
    let lib = dir.join("nope");
    let mut input = BufReader::new(std::io::Cursor::new("n\n"));
    assert!(ensure_library(&lib, &mut input).is_err());
    assert!(!lib.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pluralizes_done_messages() {
    assert_eq!(done_message(1), "Updated library.");
    assert_eq!(done_message(2), "Updated 2 libraries.");
    assert_eq!(done_message(12), "Updated 12 libraries.");
}

#[test]
fn renders_missing_directory_banners() {
    // `%!` is `<bg:yellow>** <!> %s **</bg>` stripped: the spaces
    // inside the markers survive; `%W` wraps the paragraph.
    assert_eq!(
        new_library_block("/x", false),
        " <!> NEW LIBRARY \nThe directory \"/x\" does not exist. Do you want to create it?\n"
    );
}

#[test]
fn renders_no_library_lines() {
    assert_eq!(
        no_library_line("/x", false),
        "No library currently exists at the path \"/x\"...\n"
    );
    assert_eq!(
        no_library_line("a\u{1}b", false),
        "No library currently exists at the path \"a<0x01>b\"...\n"
    );
}

#[test]
fn finds_rebuild_script_upwards() {
    let dir = scratch("script");
    let script = dir.join("support/lib/rebuild-map.php");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "<?php\n").unwrap();
    let deep = dir.join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();

    assert_eq!(find_rebuild_script(&deep, &deep), Some(script));
    let bare = scratch("noscript");
    assert_eq!(find_rebuild_script(&bare, &bare), None);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn run_in_exits_zero_on_workflow_errors() {
    // PHP runWorkflow always returns 0: finding no libraries is
    // reported but still exits 0.
    let dir = scratch("exit-zero");
    assert_eq!(run_in(&dir, &argv(&[])), ExitCode::SUCCESS);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_in_fails_only_on_argument_errors() {
    // Argument errors come from the toolset runtime outside the
    // workflow, so they still fail; help still succeeds.
    let dir = scratch("exit-args");
    assert_eq!(run_in(&dir, &argv(&["a", "b"])), ExitCode::FAILURE);
    assert_eq!(run_in(&dir, &argv(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(run_in(&dir, &argv(&["--help"])), ExitCode::SUCCESS);
    std::fs::remove_dir_all(&dir).ok();
}
