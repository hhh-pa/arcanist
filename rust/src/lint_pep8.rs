//! `Pep8Linter`: Python style checks via the external `pep8` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistPEP8Linter.php`. Concrete:
//! `getLinterName()` returns `'PEP8'`, default binary `pep8`, no
//! mandatory flags.
//!
//! Behavior: paths not ending in `.py`/`.pyw`, inputs containing NUL
//! bytes, and runs where `pep8` is absent from `PATH` yield no
//! messages. Otherwise the input bytes are written to a scratch
//! `target.py` file and `pep8 <scratch>` stdout lines of the form
//! `path:line:col: CODE message` are parsed. Severity follows the PHP
//! linter: codes starting with `W` are warnings, anything else errors.
//!
//! Out of scope (the Rust engine has no per-linter config plumbing):
//! custom binaries, extra flags, version requirements, and per-code
//! severity overrides.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default binary name, mirroring PHP `getDefaultBinary()`.
pub const BINARY: &str = "pep8";

const SEVERITY_ERROR: &str = "error";
const SEVERITY_WARNING: &str = "warning";

/// Mandatory command flags, mirroring PHP `getMandatoryFlags()`.
#[must_use]
pub fn mandatory_flags() -> Vec<&'static str> {
    Vec::new()
}

/// True for paths this linter applies to: `.py`/`.pyw` files.
#[must_use]
pub fn is_supported_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((_, ext)) = base.rsplit_once('.') else {
        return false;
    };
    ext.eq_ignore_ascii_case("py") || ext.eq_ignore_ascii_case("pyw")
}

/// Python PEP 8 linter (external `pep8` wrapper).
#[derive(Debug, Default)]
pub struct Pep8Linter;

impl Pep8Linter {
    /// Create the linter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Linter for Pep8Linter {
    fn name(&self) -> &'static str {
        "PEP8"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported_path(path) || data.contains(&0) {
            return Vec::new();
        }
        let Some(tool) = binary_on_path(BINARY) else {
            // Inactive when the tool is not installed, mirroring PHP.
            return Vec::new();
        };
        let Some(scratch) = write_scratch("pep8", ".py", path, data) else {
            return Vec::new();
        };
        let output = Command::new(&tool)
            .args(mandatory_flags())
            .arg(&scratch.file)
            .current_dir(&scratch.cwd)
            .output();
        let mut messages = Vec::new();
        if let Ok(output) = output {
            messages = parse_output(path, &String::from_utf8_lossy(&output.stdout));
        }
        scratch.cleanup();
        messages
    }
}

/// Parse `pep8` stdout into messages for `path`, mirroring PHP
/// `parseLinterOutput()`. Unparseable lines are skipped.
#[must_use]
pub fn parse_output(path: &str, stdout: &str) -> Vec<LintMessage> {
    let mut out = Vec::new();
    for raw in stdout.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(message) = parse_line(path, line) {
            out.push(message);
        }
    }
    out
}

/// Try each `:` as the path/line boundary (the PHP path group is lazy,
/// so the first boundary that yields a full match wins).
fn parse_line(path: &str, line: &str) -> Option<LintMessage> {
    for (index, _) in line.char_indices().filter(|(_, c)| *c == ':') {
        if let Some(message) = parse_at(path, line, index) {
            return Some(message);
        }
    }
    None
}

fn parse_at(path: &str, line: &str, colon: usize) -> Option<LintMessage> {
    let after_path = line.get(colon + 1..)?;
    let (line_no, rest) = take_digits(after_path)?;
    let rest = rest.strip_prefix(':')?;
    let (col, rest) = take_digits(rest)?;
    let rest = rest.strip_prefix(':')?;
    let rest = rest.strip_prefix(' ')?;
    let (code, rest) = take_token(rest)?;
    let description = rest.strip_prefix(' ')?.trim().to_string();
    Some(LintMessage {
        path: path.to_string(),
        line: line_no,
        col,
        code: code.to_string(),
        severity: severity_for(code).to_string(),
        name: format!("PEP8 {code}"),
        description,
    })
}

fn severity_for(code: &str) -> &'static str {
    match code.as_bytes().first() {
        Some(b'W') => SEVERITY_WARNING,
        _ => SEVERITY_ERROR,
    }
}

/// Split leading ASCII digits into a number plus the remainder.
fn take_digits(text: &str) -> Option<(usize, &str)> {
    let end = text.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    let value = text[..end].parse::<usize>().ok()?;
    Some((value, &text[end..]))
}

/// Split a leading non-whitespace token (PHP `\S+`) plus the remainder.
fn take_token(text: &str) -> Option<(&str, &str)> {
    let end = text
        .bytes()
        .take_while(|b| !b.is_ascii_whitespace())
        .count();
    if end == 0 {
        return None;
    }
    Some((&text[..end], &text[end..]))
}

/// Look `name` up on `PATH`, returning the tool path when present.
fn binary_on_path(name: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(name);
    if direct.components().count() > 1 {
        if direct.is_file() {
            return Some(direct);
        }
        return None;
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let pathext = std::env::var_os("PATHEXT")
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".EXE;.BAT;.CMD;.COM".to_string());
            for ext in pathext.split(';') {
                let ext = ext.trim();
                if ext.is_empty() {
                    continue;
                }
                let mut probed = candidate.clone();
                probed.as_mut_os_string().push(ext);
                if probed.is_file() {
                    return Some(probed);
                }
            }
        }
    }
    None
}

struct Scratch {
    dir: PathBuf,
    file: PathBuf,
    cwd: PathBuf,
}

impl Scratch {
    fn cleanup(self) {
        std::fs::remove_file(&self.file).ok();
        std::fs::remove_dir(&self.dir).ok();
    }
}

/// Write `data` to a unique temp file with the tool's expected suffix.
/// The tool runs with the linted file's directory as its working
/// directory when that directory exists (so tool config files apply,
/// mirroring PHP running from the project root), else the temp dir.
fn write_scratch(slug: &str, suffix: &str, display: &str, data: &[u8]) -> Option<Scratch> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("arc-rs-lint-{slug}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let file = dir.join(format!("target{suffix}"));
    std::fs::write(&file, data).ok()?;
    let cwd = Path::new(display)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty() && parent.is_dir())
        .map_or_else(|| dir.clone(), Path::to_path_buf);
    Some(Scratch { dir, file, cwd })
}

#[cfg(test)]
#[path = "lint_pep8_tests.rs"]
mod lint_pep8_tests;
