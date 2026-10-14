//! `arc-rs lint` XHPAST linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistXHPASTLinter.php` (plus
//! `ArcanistBaseXHPASTLinter.php` and the rules under
//! `src/lint/linter/xhpast/rules/`).
//!
//! Behavior: this bundle ports XHPAST as an external-tool wrapper: when
//! the `xhpast` binary is present on `PATH`, it is run on the file; a
//! zero exit means the file parsed and yields no messages, while a
//! nonzero exit yields one `XHP2` ("Unable to Parse") warning mirroring
//! `ArcanistUnableToParseXHPASTLinterRule` (ID 2, warning severity).
//! When the binary is absent the linter is inactive and reports no
//! messages.
//!
//! Known limits (see T1128): XHPAST rule evaluation (the ~100 rules in
//! `src/lint/linter/xhpast/rules/`) is NOT ported: parsed files yield no
//! rule messages, and a nonzero tool exit cannot distinguish a PHP
//! syntax error (PHP would raise `XHP1` "PHP Syntax Error!" at the error
//! line) from a tool failure, so both surface as the path-level `XHP2`
//! warning. The `xhpast.php-version` options are not supported.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter};

/// Severity mirroring `ArcanistLintSeverity`; `lint.rs` only defines the
/// severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";

/// XHPAST linter: PHP parse check via the `xhpast` binary.
#[derive(Debug, Default)]
pub struct XhpastLinter;

impl XhpastLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Tool binary name looked up on `PATH`.
#[must_use]
pub fn binary_name() -> &'static str {
    "xhpast"
}

/// Exact tool argv flags: none (the file path is the sole argument).
#[must_use]
pub fn command_flags() -> Vec<String> {
    Vec::new()
}

fn is_supported(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["php", "phtml", "inc"];
    let Some(ext) = extension_of(path) else {
        return false;
    };
    EXTENSIONS
        .iter()
        .any(|known| ext.eq_ignore_ascii_case(known))
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
            "arc-rs-lint-xhpast-{}-{nanos}-{n}",
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
        _ => "src.php".to_string(),
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

/// Interpret the `xhpast` run: exit zero means the file parsed (rule
/// evaluation is out of scope, so no messages); any other exit means the
/// file could not be parsed and yields one path-level `XHP2` warning,
/// mirroring the `ArcanistUnableToParseXHPASTLinterRule` branch of the
/// PHP `resolveFuture()`.
#[must_use]
pub fn parse_output(path: &str, exit_code: i32, _stdout: &str, _stderr: &str) -> Vec<LintMessage> {
    if exit_code == 0 {
        return Vec::new();
    }
    vec![LintMessage {
        path: path.to_string(),
        line: 1,
        col: 1,
        code: "XHP2".to_string(),
        severity: SEVERITY_WARNING.to_string(),
        name: "Unable to Parse".to_string(),
        description: format!(
            "xhpast could not parse this file (exit status {exit_code}). XHPAST rule evaluation is not yet ported."
        ),
    }]
}

impl Linter for XhpastLinter {
    fn name(&self) -> &'static str {
        "XHP"
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
#[path = "lint_xhpast_tests.rs"]
mod lint_xhpast_tests;
