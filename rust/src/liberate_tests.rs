use super::*;
use std::io::BufReader;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

fn args_of(words: &[&str]) -> Options {
    // T1153 B21/B23: TTY runs need no `--`.
    let ParseOutcome::Args(opts) = parse_args_with_tty(&argv(words), true) else {
        panic!("expected args for {words:?}");
    };
    opts
}

fn expect_lib_error(words: &[&str]) -> String {
    // T1153 B21/B23: TTY runs need no `--`.
    let ParseOutcome::Error(message) = parse_args_with_tty(&argv(words), true) else {
        panic!("expected an error");
    };
    message
}

fn expect_lib_args(words: &[&str]) -> Options {
    // T1153 B21/B23: TTY runs need no `--`.
    let ParseOutcome::Args(opts) = parse_args_with_tty(&argv(words), true) else {
        panic!("expected autocorrected args");
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
    // T1153 B21/B23: TTY runs need no `--`.
    let opts = args_of(&[]);
    assert!(!opts.clean);
    assert_eq!(opts.path, None);

    let opts = args_of(&["--clean", "lib/"]);
    assert!(opts.clean);
    assert_eq!(opts.path, Some("lib/".to_string()));

    assert!(matches!(
        parse_args_with_tty(&argv(&["a", "b"]), true),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--bogus"]), true),
        ParseOutcome::Error(_)
    ));
}

#[test]
fn rejects_second_path_with_full_text() {
    // T1153 B21/B23: TTY runs need no `--`.
    let message = expect_lib_error(&["a", "b"]);
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
    // T1153 CORE-51: a decline aborts with the `cancelled:` abort
    // text (the `---` arm strips the prefix at the CLI layer).
    let dir = scratch("decline");
    let lib = dir.join("nope");
    let mut input = BufReader::new(std::io::Cursor::new("n\n"));
    let err = ensure_library(&lib, &mut input).unwrap_err();
    assert!(err.contains("User aborted the workflow."), "{err}");
    assert!(!lib.exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core52_create_prompt_requires_a_tty() {
    // T1153 B21/B23: piped runs carry `--` to reach the prompt gate.
    // T1153 CORE-52: off-TTY the create-directory prompt echoes its
    // query and throws (`ArcanistPrompt.php:77-86` at ceb), even with
    // a piped answer waiting.
    let dir = scratch("tty-gate");
    let lib = dir.join("nope");
    let mut input = BufReader::new(std::io::Cursor::new("y\n"));
    let mut noop = |_: crate::prompts::PromptScope, _: &str| Ok(());
    let err = ensure_library_with(&lib, &mut input, false, None, &mut noop).unwrap_err();
    assert!(err.contains("attempting to read user input"), "{err}");
    assert!(!lib.exists());
    // Same through the workflow seam.
    let mut input = BufReader::new(std::io::Cursor::new("y\nmy-lib\n"));
    let err = execute_with_input(&dir, &argv(&["--", "nope"]), &mut input, false).unwrap_err();
    assert!(err.contains("attempting to read user input"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core55_synopsis_matches_php() {
    // T1153 CORE-55: the synopsis transcribes PHP verbatim
    // (`ArcanistLiberateWorkflow.php:14-20` at ceb).
    assert!(usage().contains("Create or update an Arcanist library."));
}

#[test]
fn eof_while_naming_library_cancels() {
    // T1153 CORE-54 (kept): a 0-byte `read_line` (EOF) must surface
    // the `cancelled` error, not map to `Some("")` and re-prompt
    // forever like PHP.
    let mut input = BufReader::new(std::io::Cursor::new(""));
    let err = ask_library_name(&mut input).expect_err("EOF must cancel, not loop");
    assert!(err.contains("cancelled"), "{err}");

    // Same after one invalid name: the loop must still terminate.
    let mut input = BufReader::new(std::io::Cursor::new("Bad Name!\n"));
    let err = ask_library_name(&mut input).expect_err("EOF must cancel, not loop");
    assert!(err.contains("cancelled"), "{err}");
}

#[test]
fn ancient_and_unknown_versions_are_usage_errors() {
    // T1153 CORE-53: ancient (v1, including a missing map) and unknown
    // versions abort with `Err`, which the `run_in` layer maps to a
    // fatal (255).
    let dir = scratch("versions");
    let old = dir.join("oldlib");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join(INIT_FILE), "<?php\n").unwrap();
    let weird = dir.join("weirdlib");
    std::fs::create_dir_all(&weird).unwrap();
    std::fs::write(weird.join(INIT_FILE), "<?php\n").unwrap();
    std::fs::write(weird.join(MAP_FILE), "@phutil-library-version 7\n").unwrap();

    let mut input = BufReader::new(std::io::Cursor::new(""));
    let err = execute_with_input(&dir, &argv(&["oldlib"]), &mut input, true).unwrap_err();
    assert!(err.contains("no longer supported"), "{err}");
    let mut input = BufReader::new(std::io::Cursor::new(""));
    let err = execute_with_input(&dir, &argv(&["weirdlib"]), &mut input, true).unwrap_err();
    assert!(err.contains("Unknown library version '7'"), "{err}");
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
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn exe_ancestor_search_finds_script_in_fake_tree() {
    // Hermetic: the ancestor walk must not depend on where the test
    // binary lives (`CARGO_TARGET_DIR` may sit outside the repo, as in
    // CI's gates image), so exercise it against a fake install tree.
    let dir = scratch("exe-search");
    let script = dir.join("support/lib/rebuild-map.php");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "<?php\n").unwrap();
    let fake_exe = dir.join("bin").join("arc");
    assert_eq!(find_in_ancestors(&fake_exe), Some(script));

    let bare = scratch("exe-search-bare");
    std::fs::create_dir_all(bare.join("bin")).unwrap();
    assert_eq!(find_in_ancestors(&bare.join("bin").join("arc")), None);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bare).ok();
}

#[test]
fn out_of_tree_lookup_never_returns_garbage() {
    // The public lookup may or may not resolve via the real binary
    // location depending on the environment; either way it must never
    // return a path that does not exist.
    let bare = scratch("out-of-tree");
    let found = find_rebuild_script(&bare, &bare);
    assert!(found.is_none_or(|path| path.is_file()));
    std::fs::remove_dir_all(&bare).ok();
}

/// Repo root above `rust/` (compile-time manifest dir), holding `support/`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("rust/ lives one level below the repo root")
        .to_path_buf()
}

#[test]
fn rebuild_script_ships_in_tree() {
    // Temp-stub tests alone cannot catch a deleted script: assert the
    // real `support/lib/rebuild-map.php` plus the bits it needs at
    // runtime (the init bootstrap it requires, and the symbol extractor
    // `PhutilLibraryMapBuilder` shells out to) ship in the source tree.
    let root = repo_root();
    let script = root.join("support/lib/rebuild-map.php");
    assert!(script.is_file(), "missing {}", script.display());
    let body = std::fs::read_to_string(&script).unwrap();
    assert!(
        body.contains("PhutilLibraryMapBuilder"),
        "unexpected rebuild-map.php body"
    );
    assert!(body.contains("buildAndWriteMap"), "unexpected body");
    assert!(root.join("support/init/init-script.php").is_file());
    assert!(root.join("support/lib/extract-symbols.php").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["rebuild-map.php", "extract-symbols.php"] {
            let mode = std::fs::metadata(root.join("support/lib").join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(mode & 0o100, 0, "{name} lost its owner-execute bit");
        }
    }
}

#[test]
#[cfg(unix)]
fn php_lint_checks_shipped_scripts() {
    // The `php -l` loop over the shipped scripts, hermetic: a fake
    // `php` that succeeds only for `-l <existing file>`, so the loop
    // must pass the flag plus the real shipped paths; a missing file
    // propagates the failure.
    let root = repo_root();
    let dir = TempDir::new("php-lint");
    let php = dir.path().join("php");
    std::fs::write(
        &php,
        "#!/bin/sh\nif [ \"$1\" = \"-l\" ] && [ -f \"$2\" ]; then exit 0; fi\nexit 1\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&php, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for name in [
        "support/lib/rebuild-map.php",
        "support/lib/extract-symbols.php",
        "support/init/init-script.php",
    ] {
        let target = root.join(name);
        let output = std::process::Command::new(&php)
            .arg("-l")
            .arg(&target)
            .output()
            .expect("php -l failed to spawn");
        let target_display = target.display().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "php -l {target_display} failed: {stderr}"
        );
    }
    let missing = dir.path().join("no-such-file.php");
    let output = std::process::Command::new(&php)
        .arg("-l")
        .arg(&missing)
        .output()
        .expect("php -l failed to spawn");
    assert!(!output.status.success());
}

#[test]
fn finds_shipped_rebuild_script_from_tree() {
    // The upward search resolves the SHIPPED script (not a temp stub)
    // for libraries inside the checkout.
    let root = repo_root();
    let deep = root.join("rust").join("src");
    assert_eq!(
        find_rebuild_script(&deep, &deep),
        Some(root.join("support/lib/rebuild-map.php"))
    );
}

#[test]
fn rebuild_failure_stays_per_library_failure() {
    // A failing rebuild (here: an explicit script stub exiting 3, or a
    // missing `php` binary) yields per-library failure text, with or
    // without `php` installed; the `run_in` layer still exits 0, as in
    // PHP where a failing passthru still returns 0 from `runWorkflow`
    // (`$any_errors` never fails the run).
    let dir = scratch("rebuild-msg");
    let stub = dir.join("rebuild-map.php");
    std::fs::write(&stub, "<?php exit(3);\n").unwrap();
    let message = rebuild_map(&dir, false, Some(&stub)).expect("expected failure text");
    assert!(message.contains("rebuild-map.php"), "{message}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_in_fails_on_usage_errors() {
    // T1153 B21/B23: piped runs carry `--` to reach each error arm;
    // `--bogus` stays bare to lock scan-first ordering.
    // T1153 CORE-21 / T1153 CORE-53 (R4): parser errors exit 1 under the
    // `USAGE EXCEPTION` label, but no-libraries-found throws
    // `ArcanistUsageException`, which escapes the toolset runtime
    // into a PHP fatal (255). Help still succeeds.
    let dir = scratch("exit-usage");
    assert_eq!(run_in(&dir, &argv(&["--"])), ExitCode::from(255));
    assert_eq!(run_in(&dir, &argv(&["a", "b", "--"])), ExitCode::FAILURE);
    assert_eq!(run_in(&dir, &argv(&["--bogus"])), ExitCode::FAILURE);
    assert_eq!(run_in(&dir, &argv(&["--help", "--"])), ExitCode::SUCCESS);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_in_exits_zero_on_rebuild_failures() {
    // T1153 B21/B23: piped runs carry `--`.
    // Per-library rebuild failures (missing script, missing `php`, a
    // failing rebuild) still exit 0, per PHP's `$any_errors` policy. The
    // rebuild runs for real here when `php` is present, and fails
    // cleanly when it is absent; either way the exit code is 0.
    let dir = scratch("exit-rebuild-ok");
    let lib = dir.join("v2lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join(INIT_FILE), "<?php\n").unwrap();
    std::fs::write(lib.join(MAP_FILE), "@phutil-library-version 2\n").unwrap();
    assert_eq!(run_in(&dir, &argv(&["--", "v2lib"])), ExitCode::SUCCESS);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn r4_core53_error_kinds() {
    use crate::conduit::EnvelopeKind;
    // T1153 B21/B23: non-parse messages carry `--` to model a successful parse.
    // Parser errors are toolset usage; declines are `---`; the rest
    // (no libraries, versions, IO) escape into a fatal.
    assert_eq!(
        classify_error(&argv(&["a", "b", "--"]), "x".to_string()).kind,
        EnvelopeKind::ToolsetUsage
    );
    assert_eq!(
        classify_error(
            &argv(&["--", "lib"]),
            "cancelled: directory \"d\" does not exist".to_string()
        )
        .kind,
        EnvelopeKind::UserAbort
    );
    assert_eq!(
        classify_error(
            &argv(&["--"]),
            "Unable to find any libraries under the current working directory. To create a library, provide a path.".to_string()
        )
        .kind,
        EnvelopeKind::Fatal
    );
    assert_eq!(
        classify_error(
            &argv(&["--", "lib"]),
            "Unknown library version '9'!".to_string()
        )
        .kind,
        EnvelopeKind::Fatal
    );
}

#[test]
fn r4_core21_decline_reports_user_abort() {
    // T1153 B21/B23: `classify_error` argv carries `--` to model a successful parse.
    // T1153 CORE-51: declining the create-directory prompt fails
    // with `cancelled:`, routed to the `---` arm (exit 1).
    let dir = scratch("r4-decline");
    let input = "n\n";
    let mut reader = BufReader::new(input.as_bytes());
    let err = execute_with_input(&dir, &argv(&["newlib"]), &mut reader, true).unwrap_err();
    assert!(err.starts_with("cancelled: "), "{err}");
    assert_eq!(
        classify_error(&argv(&["--", "newlib"]), err).kind,
        crate::conduit::EnvelopeKind::UserAbort
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core50_rebuild_failure_still_succeeds_silently() {
    // T1153 CORE-50: no `support/` tree above scratch, so the rebuild fails, but the
    // run still succeeds with no failure text (DONE prints always).
    let dir = scratch("core50");
    let lib = dir.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join(INIT_FILE), "<?php\n").unwrap();
    std::fs::write(lib.join(MAP_FILE), "@phutil-library-version 2\n").unwrap();
    let mut input = BufReader::new(std::io::Cursor::new(""));
    let out = execute_with_input(&dir, &argv(&["lib"]), &mut input, true).unwrap();
    assert!(out.is_empty(), "{out:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core51_decline_renders_php_abort_text() {
    // T1153 B21/B23: `classify_error` argv carries `--` to model a successful parse.
    let error = classify_error(
        &argv(&["--", "newlib"]),
        format!("cancelled: {}", crate::prompt::ABORT),
    );
    assert_eq!(error.kind, crate::conduit::EnvelopeKind::UserAbort);
    assert_eq!(error.message, crate::prompt::ABORT);
}

#[test]
fn core52_prompt_engine_drives_liberate_confirms() {
    let dir = scratch("core52");
    let lib = dir.join("newlib");
    // Decline through the engine.
    let mut input = BufReader::new(std::io::Cursor::new("n\n"));
    let mut noop = |_: crate::prompts::PromptScope, _: &str| Ok(());
    let err = ensure_library_with(&lib, &mut input, true, None, &mut noop).unwrap_err();
    assert!(err.starts_with("cancelled: "), "{err}");
    assert!(!lib.exists());
    // Naming an existing directory never touches the save channel.
    std::fs::create_dir_all(&lib).unwrap();
    let mut input = BufReader::new(std::io::Cursor::new("my-lib\n"));
    let created = ensure_library_with(&lib, &mut input, true, None, &mut noop).unwrap();
    assert!(created);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn core52_liberate_create_save_suffix_persists() {
    // Sibling of `core52_prompt_engine_drives_liberate_confirms`: a `*`
    // suffix on the create-directory confirm fires the save channel with
    // the stripped response under the user scope.
    let dir = TempDir::new("core52-save");
    let lib = dir.path().join("newlib");
    let mut input = BufReader::new(std::io::Cursor::new("y*\nmy-lib\n"));
    let mut saved_calls = Vec::new();
    let created = ensure_library_with(
        &lib,
        &mut input,
        true,
        None,
        &mut |scope, response: &str| {
            saved_calls.push((scope, response.to_string()));
            Ok(())
        },
    )
    .unwrap();
    assert!(created);
    assert_eq!(
        saved_calls,
        vec![(crate::prompts::PromptScope::User, "y".to_string())]
    );
    assert!(lib.join(INIT_FILE).is_file());
}

#[test]
fn core53_parse_errors_keep_red_arm_with_parser_texts() {
    // T1153 B21/B23: TTY runs need no `--`.
    // The arm stays `ToolsetUsage` (R4-locked exit mapping); only the
    // texts follow the toolset parser.
    let error = classify_error(&argv(&["--bogus"]), "x".to_string());
    assert_eq!(error.kind, crate::conduit::EnvelopeKind::ToolsetUsage);
    let message = expect_lib_error(&["--bogus"]);
    assert_eq!(
        message,
        "Argument \"--bogus\" is unrecognized. Use \"--\" to indicate the end of flags."
    );
    assert!(matches!(
        parse_args_with_tty(&argv(&["--clean", "--clean"]), true),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--clean=x"]), true),
        ParseOutcome::Error(_)
    ));
    // `--help` after `--` is a positional path, not help.
    assert!(matches!(
        parse_args(&argv(&["--", "--help"])),
        ParseOutcome::Args(_)
    ));
}
#[test]
fn r10_liberate_help_matches_generated_format() {
    // T1153 R10: `--help` renders the generated workflow help
    // (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
    // byte-identical to `arc help <name>` for one name.
    assert_eq!(
        usage_text(false),
        "LIBERATE WORKFLOW\n\
         \n\
         liberate\n\
         liberate [path]\n\
         Create or update an Arcanist library.\n\
         \n\
         Create or update an Arcanist library.\n\
         \n\
         \x20     --clean\n\
         \x20         Perform a clean rebuild, ignoring caches. Thorough, but slow.\n\
         \n\
         \n"
    );
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
    assert!(ansi.contains("\x1b[4m"), "{ansi:?}");
    // `run()` prints `usage()` (same text, ambient ANSI mode).
    assert!(usage().contains("LIBERATE WORKFLOW"), "{}", usage());
}

#[test]
fn b21_liberate_requires_terminator_when_piped() {
    // T1153 B21/B23: piped without `--` reports the terminator, even
    // for `--help` and empty argv. Unknown flags still report first;
    // path-count validation runs after the terminator check.
    assert!(matches!(
        parse_args_with_tty(&argv(&["lib"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--help"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&[]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["a", "b"]), false),
        ParseOutcome::MissingTerminator
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--bogus"]), false),
        ParseOutcome::Error(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["--", "lib"]), false),
        ParseOutcome::Args(_)
    ));
    assert!(matches!(
        parse_args_with_tty(&argv(&["lib"]), true),
        ParseOutcome::Args(_)
    ));
    assert_eq!(
        crate::help::MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn b23_liberate_help_matches_generated_format() {
    // T1153 B21/B23: `--help` renders the generated workflow help via
    // the shared renderer, byte-identical to `arc help liberate`.
    let text = usage_text(false);
    assert!(text.starts_with("LIBERATE WORKFLOW\n"), "{text}");
    assert!(
        text.contains("Create or update an Arcanist library."),
        "{text}"
    );
    assert!(text.contains("--clean"), "{text}");
    let ansi = usage_text(true);
    assert!(ansi.contains("\x1b[1m"), "{ansi:?}");
}

// -( T1170 wave 5d )-----------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "arc-rs-liberate-tests-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

#[test]
fn parse_corrects_near_miss_flag_with_notice() {
    // T1153 B21/B23: TTY runs need no `--`; autocorrect is on, so the
    // near-miss `--clea` corrects to `--clean` with a stderr notice.
    let opts = expect_lib_args(&["--clea"]);
    assert!(opts.clean);
    assert_eq!(opts.path, None);
}

#[test]
fn ensure_library_rejects_files() {
    let dir = TempDir::new("notdir");
    let file = dir.path().join("plain-file");
    std::fs::write(&file, "x\n").unwrap();
    let mut input = std::io::Cursor::new(Vec::new());
    let mut save = |_: crate::prompts::PromptScope, _: &str| Ok(());
    let err = ensure_library_with(&file, &mut input, true, None, &mut save).unwrap_err();
    assert_eq!(
        err,
        "Provide a directory to create or update a libphutil library in."
    );
}

#[test]
fn rebuild_map_passes_drop_cache_when_clean() {
    // The `--drop-cache` flag composes before spawning; the spawn
    // itself fails either way (php or the script is missing), so the
    // outcome only pins the failure shape.
    let dir = TempDir::new("rebuild-clean");
    let missing = dir.path().join("no-such-rebuild-map.php");
    let message = rebuild_map(dir.path(), true, Some(&missing)).unwrap();
    assert!(message.contains("php"), "{message}");
}

#[test]
#[cfg(unix)]
fn rebuild_map_reports_php_outcomes() {
    // Both `php` outcome arms, hermetic: a fake `php` exiting 0 except
    // for the failing script (exit 3), injected through the override.
    let dir = TempDir::new("rebuild-php");
    let php = dir.path().join("php");
    std::fs::write(
        &php,
        "#!/bin/sh\ncase \" $* \" in\n*\"fail.php\"*) exit 3;;\n*) exit 0;;\nesac\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&php, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let clean = dir.path().join("ok.php");
    std::fs::write(&clean, "<?php exit(0);\n").unwrap();
    assert_eq!(
        rebuild_map_with_php(dir.path(), false, Some(&clean), Some(&php)),
        None
    );
    let failing = dir.path().join("fail.php");
    std::fs::write(&failing, "<?php exit(3);\n").unwrap();
    let message = rebuild_map_with_php(dir.path(), false, Some(&failing), Some(&php)).unwrap();
    assert!(message.contains("rebuild-map.php failed"), "{message}");
}

#[test]
fn rebuild_lookup_miss_without_support_tree_reports_not_found() {
    // Lookup-miss arm, deterministic: a clean temp root carries no support
    // tree above it, and the `None` binary override skips the exe-ancestor
    // search, so the lookup misses regardless of where the test binary lives.
    let dir = TempDir::new("rebuild-miss");
    assert_eq!(
        find_rebuild_script_with_exe(dir.path(), dir.path(), None),
        None
    );
    let message =
        rebuild_map_with_php_and_exe(dir.path(), false, None, None, None).expect("miss text");
    assert!(
        message.contains("support/lib/rebuild-map.php was not found above"),
        "{message}"
    );
}

#[test]
fn liberate_path_delegates_to_prompt_channel() {
    let dir = TempDir::new("delegate");
    let file = dir.path().join("plain-file");
    std::fs::write(&file, "x\n").unwrap();
    let mut input = std::io::Cursor::new(Vec::new());
    let err = liberate_path(&file, false, &mut input).unwrap_err();
    assert_eq!(
        err,
        "Provide a directory to create or update a libphutil library in."
    );
}

#[test]
fn execute_with_input_reports_missing_terminator() {
    // Piped runs without `--` fail before any prompt or scan.
    let dir = TempDir::new("terminator");
    let mut input = std::io::Cursor::new(Vec::new());
    let err = execute_with_input(dir.path(), &argv(&[]), &mut input, false).unwrap_err();
    assert_eq!(err, crate::help::MISSING_TERMINATOR);
}

// -( T1170 coverage wave 6g band G, batch E )----------------------------------

/// `BufRead` that plants the library init file on first read, so the
/// post-confirm recheck in `ensure_library_with` observes a library
/// that did not exist at entry.
struct Cov6gPlantOnRead {
    lib: PathBuf,
    planted: bool,
    inner: std::io::Cursor<Vec<u8>>,
}

impl Cov6gPlantOnRead {
    fn new(lib: PathBuf, answer: &str) -> Self {
        Self {
            lib,
            planted: false,
            inner: std::io::Cursor::new(answer.as_bytes().to_vec()),
        }
    }

    fn plant(&mut self) {
        if self.planted {
            return;
        }
        self.planted = true;
        std::fs::create_dir_all(&self.lib).unwrap();
        std::fs::write(self.lib.join(INIT_FILE), "<?php\n").unwrap();
    }
}

impl std::io::Read for Cov6gPlantOnRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.plant();
        self.inner.read(buf)
    }
}

impl std::io::BufRead for Cov6gPlantOnRead {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.plant();
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt);
    }
}

#[test]
fn cov6g_ensure_library_sees_init_created_during_confirm() {
    let dir = TempDir::new("cov6g-plant");
    let lib = dir.path().join("newlib");
    let mut input = Cov6gPlantOnRead::new(lib.clone(), "y\n");
    let mut noop = |_: crate::prompts::PromptScope, _: &str| Ok(());
    assert_eq!(
        ensure_library_with(&lib, &mut input, true, None, &mut noop),
        Ok(false)
    );
    assert!(lib.join(INIT_FILE).is_file());
    // Second read trips the plant-once guard (the confirm already planted).
    std::io::BufRead::fill_buf(&mut input).unwrap();
}

#[test]
fn cov6g_plant_on_read_read_plants_and_returns_bytes() {
    // Direct `Read::read` unit test for the `BufRead` supertrait stub: the
    // first read plants the init file and streams the queued answer bytes.
    let dir = TempDir::new("cov6g-read");
    let lib = dir.path().join("newlib");
    let mut input = Cov6gPlantOnRead::new(lib.clone(), "y\n");
    let mut buf = [0u8; 4];
    let n = std::io::Read::read(&mut input, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"y\n");
    assert!(lib.join(INIT_FILE).is_file());
    // Second read trips the plant-once guard through the `Read` path too.
    let n = std::io::Read::read(&mut input, &mut buf).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn cov6g_run_in_two_paths_is_usage_failure() {
    // Piped (no `--`): the terminator failure prints the usage hints,
    // then reports toolset usage. On a TTY the same argv fails one-path
    // validation instead. Both exit 1 before any prompt, so this never
    // blocks on stdin in either state.
    let dir = TempDir::new("cov6g-two-paths");
    let code = run_in(dir.path(), &argv(&["a", "b"]));
    assert_eq!(code, std::process::ExitCode::FAILURE);
}
// -( T1170 wave 6h: helper-arm coverage )--------------------------------------

#[test]
#[should_panic(expected = "expected args for")]
fn args_of_panics_on_invalid_argv() {
    // T1153 B21/B23: TTY runs need no `--`; two paths still error.
    args_of(&["a", "b"]);
}

#[test]
#[should_panic(expected = "expected an error")]
fn rejects_second_path_with_full_text_defensive_args_arm() {
    // `expect_lib_error` unwraps `Error`: a single path parses.
    expect_lib_error(&["lib"]);
}

#[test]
#[should_panic(expected = "expected autocorrected args")]
fn parse_corrects_near_miss_flag_with_notice_defensive_error_arm() {
    // `expect_lib_args` unwraps `Args`: an unknown flag errors.
    expect_lib_args(&["--bogus"]);
}
