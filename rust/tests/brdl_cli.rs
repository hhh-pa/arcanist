//! T1153 browse/download CLI behavior through the wired library:
//! noninteractive `--` enforcement (BROWSE-B21) and generated
//! `--help` (BROWSE-B23).

use std::process::ExitCode;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn piped_stdin() -> bool {
    use std::io::IsTerminal;
    !std::io::stdin().is_terminal()
}

#[test]
fn b21_download_requires_terminator_when_piped() {
    if !piped_stdin() {
        return;
    }
    let dir = std::env::temp_dir();
    // Missing `--` reports (exit 1), even for `--help` and missing
    // files; `--` proceeds to normal handling.
    assert_eq!(
        arc_rs::download::run_in(&dir, &args(&["--help"])),
        ExitCode::FAILURE
    );
    assert_eq!(
        arc_rs::download::run_in(&dir, &args(&["F1"])),
        ExitCode::FAILURE
    );
    assert_eq!(
        arc_rs::download::run_in(&dir, &args(&["--help", "--"])),
        ExitCode::SUCCESS
    );
}

#[test]
fn b21_browse_requires_terminator_when_piped() {
    if !piped_stdin() {
        return;
    }
    let dir = std::env::temp_dir();
    assert_eq!(
        arc_rs::browse::run_in(&dir, &args(&["--help"])),
        ExitCode::FAILURE
    );
    assert_eq!(
        arc_rs::browse::run_in(&dir, &args(&["HEAD"])),
        ExitCode::FAILURE
    );
}

#[test]
fn b23_help_exits_zero_with_terminator() {
    // TTY-independent: `--` satisfies the check everywhere. Help
    // bodies are pinned byte for byte in the module unit tests.
    let dir = std::env::temp_dir();
    assert_eq!(
        arc_rs::browse::run_in(&dir, &args(&["--help", "--"])),
        ExitCode::SUCCESS
    );
    assert_eq!(
        arc_rs::download::run_in(&dir, &args(&["--help", "--"])),
        ExitCode::SUCCESS
    );
}
