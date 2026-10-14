//! `PyLintLinter`: Python checks via the external `pylint` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistPyLintLinter.php`. Concrete:
//! `getLinterName()` returns `'PyLint'`, default binary `pylint`, with
//! mandatory flags `--reports=no` and
//! `--msg-template={line}|{column}|{msg_id}|{symbol}|{msg}`.
//!
//! Behavior: paths not ending in `.py`/`.pyw`, inputs containing NUL
//! bytes, and runs where `pylint` is absent from `PATH` yield no
//! messages. Otherwise the input bytes are written to a scratch
//! `target.py` file and `pylint <flags> <scratch>` stdout lines of the
//! form `line|column|msg_id|symbol|message` are parsed. Severity follows
//! the PHP linter: `R*`/`C*` advice, `W*` warning, `E*`/`F*` error, and
//! anything else disabled (dropped, since the Rust engine has no
//! severity filtering). The tool's column is passed through like PHP
//! (pylint reports 0-based columns; `-1` becomes 0 per T9257).
//!
//! Known limits: pylint exit status 32 (usage error) makes PHP raise;
//! without an error channel the linter stays silent instead. Out of
//! scope (no per-linter config plumbing): the `pylint.config` rcfile
//! option, custom binaries, extra flags, and version requirements.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default binary name, mirroring PHP `getDefaultBinary()`.
pub const BINARY: &str = "pylint";

const SEVERITY_ERROR: &str = "error";
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";

/// Mandatory command flags, mirroring PHP `getMandatoryFlags()`.
#[must_use]
pub fn mandatory_flags() -> Vec<&'static str> {
    vec![
        "--reports=no",
        "--msg-template={line}|{column}|{msg_id}|{symbol}|{msg}",
    ]
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

/// PyLint analyzer (external `pylint` wrapper).
#[derive(Debug, Default)]
pub struct PyLintLinter;

impl PyLintLinter {
    /// Create the linter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Linter for PyLintLinter {
    fn name(&self) -> &'static str {
        "PyLint"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported_path(path) || data.contains(&0) {
            return Vec::new();
        }
        let tool = binary_on_path(BINARY);
        lint_with_binary(path, data, tool.as_deref())
    }
}

/// Lint with an explicit tool binary (test seam mirroring bundle E);
/// `None` means the tool is not installed and reports nothing.
pub fn lint_with_binary(path: &str, data: &[u8], binary: Option<&Path>) -> Vec<LintMessage> {
    let Some(tool) = binary else {
        // Inactive when the tool is not installed, mirroring PHP.
        return Vec::new();
    };
    let Some(scratch) = write_scratch("pylint", ".py", path, data) else {
        return Vec::new();
    };
    let mut command = Command::new(tool);
    command
        .args(mandatory_flags())
        .arg(&scratch.file)
        .current_dir(&scratch.cwd);
    let output = arc_rs::lint::spawn_with_retry(&mut command);
    let mut messages = Vec::new();
    if let Ok(output) = output {
        let code = output.status.code().unwrap_or_default();
        messages = parse_output(path, code, &String::from_utf8_lossy(&output.stdout));
    }
    scratch.cleanup();
    messages
}

/// Parse `pylint` stdout into messages for `path`, mirroring PHP
/// `parseLinterOutput()`. Exit status 32 (usage error) and unparseable
/// lines yield no messages.
#[must_use]
pub fn parse_output(path: &str, exit_code: i32, stdout: &str) -> Vec<LintMessage> {
    if exit_code == 32 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for raw in stdout.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(message) = parse_line(path, line) {
            out.push(message);
        }
    }
    out
}

fn parse_line(path: &str, line: &str) -> Option<LintMessage> {
    let mut parts = line.splitn(5, '|');
    let (Some(line_s), Some(col_s), Some(code), Some(symbol), Some(message)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    let line_no = line_s.parse::<usize>().ok()?;
    // PHP casts the column with `(int)` (junk becomes 0) and clamps
    // negative values to 0 (T9257).
    let col = col_s.parse::<i64>().unwrap_or(0).max(0) as usize;
    let severity = severity_for(code)?;
    Some(LintMessage {
        path: path.to_string(),
        line: line_no,
        col,
        code: code.to_string(),
        severity: severity.to_string(),
        name: display_name(symbol),
        description: message.to_string(),
        original: None,
        replacement: None,
    })
}

/// Mirror PHP `ucwords(str_replace('-', ' ', $symbol))`.
fn display_name(symbol: &str) -> String {
    let mut out = String::with_capacity(symbol.len());
    let mut upper = true;
    for ch in symbol.replace('-', " ").chars() {
        if ch.is_whitespace() {
            upper = true;
            out.push(ch);
        } else if upper {
            upper = false;
            for upper_ch in ch.to_uppercase() {
                out.push(upper_ch);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Severity for a pylint message id. `None` means PHP's
/// `SEVERITY_DISABLED`: the engine would drop the message.
fn severity_for(code: &str) -> Option<&'static str> {
    match code.as_bytes().first() {
        Some(b'R') | Some(b'C') => Some(SEVERITY_ADVICE),
        Some(b'W') => Some(SEVERITY_WARNING),
        Some(b'E') | Some(b'F') => Some(SEVERITY_ERROR),
        _ => None,
    }
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
    std::env::var_os("PATH").and_then(|path| binary_in(&path))
}

/// Look the tool up in `path_env` (test seam; production passes `$PATH`).
fn binary_in(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join(BINARY);
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
#[path = "lint_pylint_tests.rs"]
mod lint_pylint_tests;
