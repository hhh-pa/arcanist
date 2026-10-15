//! Shared `--browse` support, mirroring `openURIsInBrowser()` and
//! `getBrowserCommand()` (`src/workflow/ArcanistWorkflow.php:2150-2208`).
//!
//! The browser command comes from the `browser` config value (a string is
//! one argument, a list is the full argv); without one, Windows uses
//! `cmd /c start` and other systems scan `sensible-browser`, `xdg-open`,
//! then `open --`. Each URI is opened in turn; the first failure aborts
//! with PHP's error.

use std::path::Path;

use crate::{config, diff, get_config};

/// A `browser` config value as argv: a truthy string is one argument, a
/// list is the full command; anything else (including empty values, which
/// are falsy upstream) means "unconfigured".
#[must_use]
pub fn configured_argv(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(command) if !command.is_empty() && command != "0" => {
            Some(vec![command.clone()])
        }
        serde_json::Value::Array(items) => {
            let argv: Vec<String> = items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect();
            if argv.is_empty() { None } else { Some(argv) }
        }
        _ => None,
    }
}

/// The `browser` command for `dir` from any config source, or `None` when
/// unset (or when config can not be read, which behaves the same).
#[must_use]
pub fn configured_browser(dir: &Path) -> Option<Vec<String>> {
    let sources = get_config::load_sources(dir, config::arcrc_path().as_deref()).ok()?;
    let (_, value) = get_config::current_value("browser", &sources)?;
    configured_argv(&value)
}

/// True when `name` resolves to a file on `PATH`, mirroring
/// `Filesystem::binaryExists()`.
fn binary_exists(name: &str) -> bool {
    if name.contains('/') || name.contains('\\') {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            if dir.join(name).is_file() {
                return true;
            }
            cfg!(windows)
                && ["exe", "cmd", "bat", "com"]
                    .iter()
                    .any(|ext| dir.join(format!("{name}.{ext}")).is_file())
        })
    })
}

/// Open one URI, resolving the browser command first.
fn open_one(uri: &str, configured: Option<&[String]>) -> Result<(), String> {
    // `browser_open_argv` returns the full command with the URI appended;
    // the browser itself is everything before it.
    let full = diff::browser_open_argv(uri, configured, &binary_exists)?;
    let browser = &full[..full.len() - 1];
    let status = std::process::Command::new(&full[0])
        .args(&full[1..])
        .status()
        .map_err(|_| diff::browser_open_error(uri, browser))?;
    if status.success() {
        Ok(())
    } else {
        Err(diff::browser_open_error(uri, browser))
    }
}

/// Open each URI in turn; the first failure aborts, like PHP.
pub fn open_uris_in_browser(uris: &[String], configured: Option<&[String]>) -> Result<(), String> {
    for uri in uris {
        open_one(uri, configured)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod browser_tests;
