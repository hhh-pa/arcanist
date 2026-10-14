//! `liberate` workflow: create or update a libphutil library.
//!
//! PHP reference: `src/workflow/ArcanistLiberateWorkflow.php`.
//!
//! With no path, one level of subdirectories is scanned for
//! `__phutil_library_init__.php` files. A path without a library gets a
//! new init file (after an interactive prompt for the directory and the
//! library name); a library at format version 2 gets its map rebuilt by
//! shelling out to `support/lib/rebuild-map.php` with `php`, exactly as
//! the PHP workflow does. Version 1 libraries are rejected as unsupported.
//!
//! Divergence: the PHP workflow always exits 0; this port exits non-zero
//! when any library fails to update.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
                return ParseOutcome::Error(
                    "Provide only one path to \"arc liberate\".".to_string(),
                );
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
                return ParseOutcome::Error(
                    "Provide only one path to \"arc liberate\".".to_string(),
                );
            }
            path = Some(arg.clone());
        }
    }

    ParseOutcome::Args(Options { clean, path })
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
pub fn parse_map_version(map_text: &str) -> Option<u32> {
    let at = map_text.find(VERSION_MARKER)?;
    let rest = &map_text[at + VERSION_MARKER.len()..];
    let digits: String = rest
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<u32>().ok()
    }
}

/// Library format version: marker value, or 1 when the map is absent.
pub fn library_version(path: &Path) -> Result<u32, String> {
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

fn prompt_line(prompt: &str, input: &mut dyn BufRead) -> Option<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    input.read_line(&mut line).ok()?;
    Some(line.trim().to_string())
}

fn confirm_create_directory(path: &Path, input: &mut dyn BufRead) -> Result<bool, String> {
    println!(
        "The directory \"{}\" does not exist. Do you want to create it?",
        path.display()
    );
    match prompt_line("Create new library? [y/N] ", input).as_deref() {
        Some("y" | "Y" | "yes" | "YES") => Ok(true),
        _ => Ok(false),
    }
}

fn ask_library_name(input: &mut dyn BufRead) -> Result<String, String> {
    println!("Choose a name for the new library.");
    loop {
        let name = prompt_line("What do you want to name this library? ", input)
            .ok_or_else(|| "cancelled: no library name given".to_string())?;
        if is_valid_library_name(&name) {
            return Ok(name);
        }
        println!("Library name should contain only lowercase letters and hyphens.");
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
    println!(
        "No library currently exists at the path \"{}\"...",
        path.display()
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
    let name = ask_library_name(input)?;
    println!("Writing '{INIT_FILE}' to '{}'...", path.display());
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
pub fn rebuild_map(path: &Path, clean: bool, script: Option<&Path>) -> Result<String, String> {
    let script = script.map_or_else(
        || find_rebuild_script(path, path),
        |s| Some(s.to_path_buf()),
    );
    let Some(script) = script else {
        return Err(format!(
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
    let status = command.status().map_err(|e| {
        format!(
            "cannot run php '{}': {e} (is PHP installed?)",
            script.display()
        )
    })?;
    if status.success() {
        Ok(format!("Rebuilt library map for '{}'.\n", path.display()))
    } else {
        Err(format!(
            "rebuild-map.php failed for '{}' (exit {status})",
            path.display()
        ))
    }
}

/// Update or create the library at `path`.
pub fn liberate_path(path: &Path, clean: bool, input: &mut dyn BufRead) -> Result<String, String> {
    let mut out = format!("Updating library: {}\n", path.display());
    if ensure_library(path, input)? {
        // Fresh libraries skip the version check and go straight to a v2
        // rebuild, mirroring `liberateCreateLibrary`.
        out.push_str(&rebuild_map(path, clean, None)?);
        return Ok(out);
    }
    match library_version(path)? {
        1 => Err("This very old library is no longer supported.".to_string()),
        2 => {
            out.push_str(&rebuild_map(path, clean, None)?);
            Ok(out)
        }
        other => Err(format!("Unknown library version '{other}'!")),
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
            let found = scan_libraries(dir)?;
            if found.is_empty() {
                return Err(format!(
                    "Unable to find any libraries under '{}'. To create a library, provide a path.",
                    dir.display()
                ));
            }
            found
        }
    };
    let mut out = String::new();
    let mut failures = Vec::new();
    for path in &paths {
        match liberate_path(path, opts.clean, input) {
            Ok(text) => out.push_str(&text),
            Err(message) => {
                failures.push(format!("{}: {message}", path.display()));
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "Failed to update librarie(s):\n  {}",
            failures.join("\n  ")
        ));
    }
    out.push_str(&format!("Updated {} librarie(s).\n", paths.len()));
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
            ExitCode::FAILURE
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
