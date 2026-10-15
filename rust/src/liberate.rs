//! `liberate` workflow: create or update a libphutil library.
//!
//! PHP reference: `src/workflow/ArcanistLiberateWorkflow.php`.
//!
//! With no path, one level of subdirectories is scanned for
//! `__phutil_library_init__.php` files. A path without a library gets a
//! new init file (after an interactive prompt for the directory and the
//! library name); a library at format version 2 gets its map rebuilt by
//! shelling out to `support/lib/rebuild-map.php` with `php`, exactly as
//! the PHP workflow does.
//!
//! Status lines go to stderr through the log engine (`SCAN`/`WORK` blue,
//! `ERROR` red, `DONE` green); the missing-directory banner renders via
//! `tsprintf("%!\n%W\n", ...)` and the `DONE` count through the `pht`
//! plural variants (`Updated library.` / `Updated %s libraries.`).
//!
//! Like PHP, per-library rebuild failures report `ERROR` lines but still
//! exit 0; only argument-parsing errors fail (raised by the toolset
//! runtime outside the workflow, as in PHP). Every workflow failure --
//! no libraries found, ancient or unknown library versions, declined
//! prompts -- still exits 0 after reporting the error.
//!
//! Divergences:
//! - The rebuild script is found by an upward `support/lib/rebuild-map.php`
//!   search instead of the install-relative bundled path (arc-rs ships no
//!   PHP runtime files).
//! - A declined directory prompt fails with `cancelled: ...` instead of
//!   the PHP prompt abort, and end-of-input while naming a library fails
//!   instead of re-prompting forever.
//! - Prompts print plain text (no `phutil_console_prompt` wrapping or
//!   readline history); see `confirm_create_directory`/`ask_library_name`.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::terminal::{self, PhtArg, TVal};

pub const NAME: &str = "liberate";

const INIT_FILE: &str = "__phutil_library_init__.php";
const MAP_FILE: &str = "__phutil_library_map__.php";
const VERSION_MARKER: &str = "@phutil-library-version";

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--clean] [PATH]\n\
         \n\
         Create or update a libphutil library. Without PATH, libraries one\n\
         level below the current directory are updated.\n\
         \n\
         Options:\n\
         \x20 --clean     perform a clean rebuild, ignoring caches (slow)\n\
         \x20 -h, --help  show this help\n"
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    clean: bool,
    path: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut clean = false;
    let mut path: Option<String> = None;
    let mut end_of_flags = false;

    for arg in argv {
        if end_of_flags {
            if path.is_some() {
                return ParseOutcome::Error(one_path_error());
            }
            path = Some(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--clean" {
            clean = true;
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            if path.is_some() {
                return ParseOutcome::Error(one_path_error());
            }
            path = Some(arg.clone());
        }
    }

    ParseOutcome::Args(Options { clean, path })
}

fn one_path_error() -> String {
    "Provide only one path to \"arc liberate\". The path should identify a directory where you want to create or update a library."
        .to_string()
}

/// Library roots one level below `dir` (dirs holding an init file), sorted.
pub fn scan_libraries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list '{}': {e}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list '{}': {e}", dir.display()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let candidate = entry.path().join(INIT_FILE);
        if candidate.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// Parse the `@phutil-library-version N` marker from map text.
///
/// Mirrors `/@phutil-library-version (\d+)/` plus PHP's `(int)` cast:
/// exactly one space, then digits saturating at `i64::MAX`. Later
/// markers still match when an early one has no digits.
pub fn parse_map_version(map_text: &str) -> Option<i64> {
    let mut search = 0;
    while let Some(at) = map_text[search..].find(VERSION_MARKER) {
        let rest = &map_text[search + at + VERSION_MARKER.len()..];
        let digits: String = rest
            .strip_prefix(' ')
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            let mut value: i64 = 0;
            for byte in digits.bytes() {
                value = value
                    .saturating_mul(10)
                    .saturating_add(i64::from(byte - b'0'));
            }
            return Some(value);
        }
        search += at + VERSION_MARKER.len();
    }
    None
}

/// Library format version: marker value, or 1 when the map is absent.
pub fn library_version(path: &Path) -> Result<i64, String> {
    let map_file = path.join(MAP_FILE);
    if !map_file.exists() {
        return Ok(1);
    }
    let map = std::fs::read_to_string(&map_file)
        .map_err(|e| format!("cannot read '{}': {e}", map_file.display()))?;
    Ok(parse_map_version(&map).unwrap_or(1))
}

/// True for valid new library names (`^[a-z-]+$`).
#[must_use]
pub fn is_valid_library_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

/// One log-engine line on stderr (`writeStatus` blue, `writeError`
/// red, `writeSuccess` green).
fn push_log(color: &str, label: &str, message: &str) {
    eprint!(
        "{}",
        terminal::format_log(color, label, message, terminal::ansi_enabled())
    );
}

/// `echo tsprintf("%!\n%W\n", ...)` for a missing directory: a banner
/// plus a wrapped paragraph.
#[must_use]
pub fn new_library_block(path: &str, ansi: bool) -> String {
    let title = terminal::pht("NEW LIBRARY", &[]);
    let body = terminal::pht(
        "The directory \"%s\" does not exist. Do you want to create it?",
        &[PhtArg::Text(path)],
    );
    terminal::tsprintf_ansi("%!\n%W\n", &[TVal::Text(&title), TVal::Text(&body)], ansi)
        .expect("fixed tsprintf pattern")
        .as_str()
        .to_string()
}

/// `echo tsprintf("%s\n", ...)` when no library exists at the path.
#[must_use]
pub fn no_library_line(path: &str, ansi: bool) -> String {
    let message = terminal::pht(
        "No library currently exists at the path \"%s\"...",
        &[PhtArg::Text(path)],
    );
    terminal::tsprintf_ansi("%s\n", &[TVal::Text(&message)], ansi)
        .expect("fixed tsprintf pattern")
        .as_str()
        .to_string()
}

/// `pht('Updated %s librarie(s).', ...)` with the US English variants
/// (`Updated library.` / `Updated %s libraries.`).
#[must_use]
pub fn done_message(count: usize) -> String {
    terminal::pht("Updated %s librarie(s).", &[PhtArg::Int(count as i64)])
}

fn prompt_line(prompt: &str, input: &mut dyn BufRead) -> Option<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    input.read_line(&mut line).ok()?;
    Some(line.trim().to_string())
}

fn confirm_create_directory(path: &Path, input: &mut dyn BufRead) -> Result<bool, String> {
    print!(
        "{}",
        new_library_block(&path.display().to_string(), terminal::ansi_enabled())
    );
    match prompt_line("Create new library? [y/N] ", input).as_deref() {
        Some("y" | "Y" | "yes" | "YES") => Ok(true),
        _ => Ok(false),
    }
}

fn ask_library_name(input: &mut dyn BufRead) -> Result<String, String> {
    println!(
        "{}",
        terminal::pht("Choose a name for the new library.", &[])
    );
    loop {
        let name = prompt_line("What do you want to name this library? ", input)
            .ok_or_else(|| "cancelled: no library name given".to_string())?;
        if is_valid_library_name(&name) {
            return Ok(name);
        }
        // `echo phutil_console_format("%s\n", ...)`: substituted raw.
        println!(
            "{}",
            terminal::pht(
                "Library name should contain only lowercase letters and hyphens.",
                &[]
            )
        );
    }
}

/// Create the directory (prompting) and init file when missing.
///
/// Returns `true` when a library was created. Prints the interactive
/// prompts to stdout and reads answers from `input`.
pub fn ensure_library(path: &Path, input: &mut dyn BufRead) -> Result<bool, String> {
    if path.join(INIT_FILE).is_file() {
        return Ok(false);
    }
    print!(
        "{}",
        no_library_line(&path.display().to_string(), terminal::ansi_enabled())
    );
    if path.exists() {
        if !path.is_dir() {
            return Err(
                "Provide a directory to create or update a libphutil library in.".to_string(),
            );
        }
    } else if confirm_create_directory(path, input)? {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("cannot create '{}': {e}", path.display()))?;
    } else {
        return Err(format!(
            "cancelled: directory \"{}\" does not exist",
            path.display()
        ));
    }
    if path.join(INIT_FILE).is_file() {
        return Ok(false);
    }
    // `echo pht(...)."\n"`: substituted raw, newline outside.
    let dir = path.display().to_string();
    println!(
        "{}",
        terminal::pht(
            "Creating new libphutil library in '%s'.",
            &[PhtArg::Text(&dir)]
        )
    );
    let name = ask_library_name(input)?;
    // The trailing newline is inside the `pht` pattern here.
    print!(
        "{}",
        terminal::pht(
            "Writing '%s' to '%s'...\n",
            &[PhtArg::Text(INIT_FILE), PhtArg::Text(&dir)]
        )
    );
    std::fs::write(
        path.join(INIT_FILE),
        format!("<?php\n\nphutil_register_library('{name}', __FILE__);\n"),
    )
    .map_err(|e| format!("cannot write init file: {e}"))?;
    Ok(true)
}

/// Find `support/lib/rebuild-map.php` above `path`, then above `fallback`.
pub fn find_rebuild_script(path: &Path, fallback: &Path) -> Option<PathBuf> {
    for root in [path, fallback] {
        for ancestor in root.ancestors() {
            let candidate = ancestor.join("support/lib/rebuild-map.php");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Rebuild one v2 library map via `php rebuild-map.php`.
///
/// The script streams to the console directly (`phutil_passthru`); only
/// the exit status comes back. Returns the failure message when the
/// rebuild fails, or `None` on success.
pub fn rebuild_map(path: &Path, clean: bool, script: Option<&Path>) -> Option<String> {
    let script = script.map_or_else(
        || find_rebuild_script(path, path),
        |s| Some(s.to_path_buf()),
    );
    let Some(script) = script else {
        return Some(format!(
            "cannot rebuild '{}': support/lib/rebuild-map.php was not found above the library",
            path.display()
        ));
    };
    let mut command = std::process::Command::new("php");
    command.arg("-f").arg(&script).arg("--");
    if clean {
        command.arg("--drop-cache");
    }
    command.arg(path);
    match command.status() {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!(
            "rebuild-map.php failed for '{}' (exit {status})",
            path.display()
        )),
        Err(e) => Some(format!(
            "cannot run php '{}': {e} (is PHP installed?)",
            script.display()
        )),
    }
}

/// Update or create the library at `path`.
///
/// Returns `Ok(None)` on success and `Ok(Some(message))` when the rebuild
/// step fails (a per-library failure: reported, but the workflow still
/// exits 0). Usage errors (ancient or unknown versions, declined
/// prompts, unreadable maps) return `Err` and abort the workflow.
pub fn liberate_path(
    path: &Path,
    clean: bool,
    input: &mut dyn BufRead,
) -> Result<Option<String>, String> {
    if ensure_library(path, input)? {
        // Fresh libraries skip the version check and go straight to a v2
        // rebuild, mirroring `liberateCreateLibrary`.
        return Ok(rebuild_map(path, clean, None));
    }
    match library_version(path)? {
        1 => Err("This very old library is no longer supported.".to_string()),
        2 => Ok(rebuild_map(path, clean, None)),
        other => Err(format!("Unknown library version '{other}'!")),
    }
}

/// Render `path` relative to the working copy root when it sits under
/// `dir`, else unchanged (`Filesystem::readablePath()`).
pub fn readable_path(dir: &Path, path: &Path) -> String {
    let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
    let text = path.display().to_string();
    match text.strip_prefix(&prefix) {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// Run the workflow; reads interactive answers from `input`.
pub fn execute_with_input(
    dir: &Path,
    argv: &[String],
    input: &mut dyn BufRead,
) -> Result<String, String> {
    let opts = match parse_args(argv) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        ParseOutcome::Error(message) => return Err(format!("{message}\n\n{}", usage())),
    };
    let paths = match opts.path {
        Some(path) => vec![dir.join(&path)],
        None => {
            push_log(
                "blue",
                &terminal::pht("SCAN", &[]),
                &terminal::pht(
                    "Searching for libraries in the current working directory...",
                    &[],
                ),
            );
            let found = scan_libraries(dir)?;
            if found.is_empty() {
                return Err(
                    "Unable to find any libraries under the current working directory. To create a library, provide a path."
                        .to_string(),
                );
            }
            found
        }
    };
    let mut out = String::new();
    let mut any_errors = false;
    for path in &paths {
        let entry = format!("{}{}", readable_path(dir, path), std::path::MAIN_SEPARATOR);
        push_log(
            "blue",
            &terminal::pht("WORK", &[]),
            &terminal::pht("Updating library: %s", &[PhtArg::Text(&entry)]),
        );
        match liberate_path(path, opts.clean, input) {
            Ok(None) => {}
            Ok(Some(message)) => {
                any_errors = true;
                let failed = path.display().to_string();
                push_log(
                    "red",
                    &terminal::pht("ERROR", &[]),
                    &terminal::pht("Failed to update library: %s", &[PhtArg::Text(&failed)]),
                );
                out.push_str(&format!("{message}\n"));
            }
            Err(message) => return Err(message),
        }
    }
    if !any_errors {
        push_log(
            "green",
            &terminal::pht("DONE", &[]),
            &done_message(paths.len()),
        );
    }
    Ok(out)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    execute_with_input(dir, argv, &mut locked)
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            // PHP `runWorkflow` always returns 0; only argument errors
            // (raised by the toolset runtime, outside the workflow) fail.
            if matches!(parse_args(argv), ParseOutcome::Error(_)) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "liberate_tests.rs"]
mod liberate_tests;
