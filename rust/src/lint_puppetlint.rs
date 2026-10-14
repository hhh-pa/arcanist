//! `arc-rs lint` PuppetLint linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistPuppetLintLinter.php` (an
//! `ArcanistExternalLinter`).
//!
//! Behavior: runs
//! `puppet-lint --error-level=all
//! --log-format=%{linenumber}|%{column}|%{kind}|%{check}|%{message} <file>`
//! on Puppet manifests and parses stdout lines shaped like
//! `<line>|<col>|<kind>|<check>|<message>`. Every message has code
//! `PUPPETLINT`, a humanized check name, and an uppercased-first-letter
//! description; `warning`/`error` kinds map to those severities and
//! anything else to advice, mirroring `parseLinterOutput()`.
//!
//! Known limits (see T1128): the `puppet-lint.config` linter option is
//! not supported; lines that do not parse are skipped.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter, SEVERITY_ERROR};

/// Companion severities mirroring `ArcanistLintSeverity`; `lint.rs` only
/// defines the severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";

/// PuppetLint linter: Puppet style-guide findings via an external binary.
#[derive(Debug, Default)]
pub struct PuppetlintLinter;

impl PuppetlintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "puppet-lint"
}

/// Exact tool argv flags, mirroring `getMandatoryFlags()`.
#[must_use]
pub fn command_flags() -> Vec<String> {
    vec![
        "--error-level=all".to_string(),
        "--log-format=%{linenumber}|%{column}|%{kind}|%{check}|%{message}".to_string(),
    ]
}

fn is_supported(path: &str) -> bool {
    extension_of(path).is_some_and(|ext| ext.eq_ignore_ascii_case("pp"))
}

fn extension_of(path: &str) -> Option<&str> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let dot = base.rfind('.')?;
    if dot + 1 >= base.len() {
        return None;
    }
    Some(&base[dot + 1..])
}

/// Locate `binary` on a `$PATH`-style search string (mirrors
/// `anoid::find_on_path` so this module stays standalone-includeable).
fn find_on_path(path_env: &std::ffi::OsStr, binary: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{binary}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn binary_in(path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    find_on_path(path_env, binary_name())
}

/// Locate the tool on the real `$PATH`; `None` means the linter is
/// inactive and reports no messages.
#[must_use]
pub fn binary_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    binary_in(&path)
}

/// Staged copy of the linted bytes: the external tool needs a real file.
struct StagedFile {
    dir: PathBuf,
    file: PathBuf,
}

impl StagedFile {
    fn stage(display_path: &str, data: &[u8]) -> Result<Self, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "arc-rs-lint-puppetlint-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create temp dir: {e}"))?;
        let file = dir.join(staged_name(display_path));
        std::fs::write(&file, data).map_err(|e| format!("cannot stage lint input: {e}"))?;
        Ok(Self { dir, file })
    }

    fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Temp file name preserving a safe source extension so the tool sniffs
/// the file type correctly.
fn staged_name(display_path: &str) -> String {
    match extension_of(display_path) {
        Some(ext)
            if ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+') =>
        {
            format!("src.{ext}")
        }
        _ => "src.pp".to_string(),
    }
}

fn run_tool(binary: &Path, file: &Path) -> Option<(i32, String, String)> {
    let mut command = Command::new(binary);
    command.args(command_flags()).arg(file).stdin(Stdio::null());
    let output = arc_rs::lint::spawn_with_retry(&mut command).ok()?;
    let code = output.status.code().unwrap_or(1);
    Some((
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Linter body with an explicit tool path (mirrors the PHP `bin` option).
/// `None` (tool absent) reports no messages.
pub fn lint_with_binary(path: &str, data: &[u8], binary: Option<&Path>) -> Vec<LintMessage> {
    let Some(binary) = binary else {
        return Vec::new();
    };
    let Ok(staged) = StagedFile::stage(path, data) else {
        return Vec::new();
    };
    let Some((code, stdout, stderr)) = run_tool(binary, staged.path()) else {
        return Vec::new();
    };
    parse_output(path, code, &stdout, &stderr)
}

/// Parse puppet-lint `--log-format` stdout, mirroring
/// `parseLinterOutput()`: `<line>|<col>|<kind>|<check>|<message>` lines
/// (split into at most 5 parts so messages may contain `|`) become
/// messages. Lines with fewer than 5 parts are skipped; the exit code is
/// ignored like the PHP parser.
#[must_use]
pub fn parse_output(path: &str, _exit_code: i32, stdout: &str, _stderr: &str) -> Vec<LintMessage> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() < 5 {
            continue;
        }
        let severity = match parts[2].trim() {
            "warning" => SEVERITY_WARNING,
            "error" => SEVERITY_ERROR,
            _ => SEVERITY_ADVICE,
        };
        let check = parts[3].trim().replace('_', " ");
        out.push(LintMessage {
            path: path.to_string(),
            line: parse_num(parts[0]),
            col: parse_num(parts[1]),
            code: "PUPPETLINT".to_string(),
            severity: severity.to_string(),
            name: ucwords(&check),
            description: ucfirst(parts[4].trim()),
        });
    }
    out
}

/// 1-based position component; unparsable or zero values map to 1.
fn parse_num(text: &str) -> usize {
    text.trim().parse::<usize>().unwrap_or(1).max(1)
}

/// Uppercase the first character, mirroring PHP `ucfirst()` (Unicode-aware).
fn ucfirst(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = String::with_capacity(text.len());
    for upper in first.to_uppercase() {
        out.push(upper);
    }
    out.push_str(chars.as_str());
    out
}

/// Uppercase the first character of each word, mirroring PHP `ucwords()`
/// (Unicode-aware).
fn ucwords(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if start {
            for upper in c.to_uppercase() {
                out.push(upper);
            }
        } else {
            out.push(c);
        }
        start = c.is_whitespace();
    }
    out
}

impl Linter for PuppetlintLinter {
    fn name(&self) -> &'static str {
        "PUPPETLINT"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported(path) {
            return Vec::new();
        }
        if data.contains(&0) {
            return Vec::new();
        }
        lint_with_binary(path, data, binary_on_path().as_deref())
    }
}

#[cfg(test)]
#[path = "lint_puppetlint_tests.rs"]
mod lint_puppetlint_tests;
