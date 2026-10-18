//! Shared `--browse` support, mirroring `openURIsInBrowser()` and
//! `getBrowserCommand()` (`src/workflow/ArcanistWorkflow.php:2148-2207`
//! at ceb).
//!
//! The browser command comes from the `browser` config value (a truthy
//! scalar is one argument, a non-empty list is the full argv, each item
//! string-cast like PHP); without one, Windows uses `cmd /c start` and
//! other systems scan `sensible-browser`, `xdg-open`, then `open --`.
//! Each URI is opened in turn; the first failure aborts with PHP's error.

use std::path::Path;

use crate::{config, diff, get_config};

/// PHP truthiness for a `browser` config value
/// (`getBrowserCommand()`, `:2169-2173` at ceb, T1153 CORE-63):
/// `null`, `false`, `0`, `""`, `"0"`, and empty arrays/objects are
/// falsy (unconfigured); everything else runs.
fn is_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => {
            number.as_i64().is_none_or(|n| n != 0)
                && number.as_u64().is_none_or(|n| n != 0)
                && number.as_f64().is_none_or(|n| n != 0.0)
        }
        serde_json::Value::String(text) => !text.is_empty() && text != "0",
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}

/// PHP string cast for one argv element: strings pass through, numbers
/// render (`42`, `4.5`), `true` is `"1"`, `null` is `""`; nested
/// arrays/objects have no sane spelling and are dropped.
fn cast_element(item: &serde_json::Value) -> Option<String> {
    match item {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(true) => Some("1".to_string()),
        serde_json::Value::Bool(false) => Some(String::new()),
        serde_json::Value::Null => Some(String::new()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
    }
}

/// A `browser` config value as argv (`openURIsInBrowser()`, `:2148-2154`
/// at ceb, T1153 CORE-63): a truthy scalar is one argument, a
/// non-empty list is the full command; falsy values (including empty
/// ones, which are falsy upstream) mean "unconfigured".
#[must_use]
pub fn configured_argv(value: &serde_json::Value) -> Option<Vec<String>> {
    if !is_truthy(value) {
        return None;
    }
    match value {
        serde_json::Value::Array(items) => {
            let argv: Vec<String> = items.iter().filter_map(cast_element).collect();
            if argv.is_empty() { None } else { Some(argv) }
        }
        serde_json::Value::Object(map) => {
            let argv: Vec<String> = map.values().filter_map(cast_element).collect();
            if argv.is_empty() { None } else { Some(argv) }
        }
        scalar => cast_element(scalar).map(|single| vec![single]),
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

/// True for a plain file that the OS would execute.
///
/// T1153 BROWSE-B19: PHP resolves via `which %s`
/// (`Filesystem::resolveBinary()`), which only finds executables;
/// `is_file()` alone also matches unmarked files.
fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// True when `name` resolves to a file on `PATH`, mirroring
/// `Filesystem::binaryExists()`.
fn binary_exists(name: &str) -> bool {
    if name.contains('/') || name.contains('\\') {
        return is_executable_file(Path::new(name));
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            if is_executable_file(&dir.join(name)) {
                return true;
            }
            cfg!(windows)
                && ["exe", "cmd", "bat", "com"] // LCOV_EXCL_START // Reason: windows-only extension probe; cfg!(windows) is false on ubuntu CI
                    .iter()
                    .any(|ext| is_executable_file(&dir.join(format!("{name}.{ext}")))) // LCOV_EXCL_STOP
        })
    })
}

/// Open one URI, resolving the browser command first.
fn open_one(uri: &str, configured: Option<&[String]>) -> Result<(), String> {
    // `browser_open_argv` returns the full command with the URI appended;
    // the browser itself is everything before it.
    let full = diff::browser_open_argv(uri, configured, &binary_exists)?;
    let browser = &full[..full.len() - 1];
    let status = crate::proc_env::command(&full[0])
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
