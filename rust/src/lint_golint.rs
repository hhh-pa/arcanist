//! `arc-rs lint` GoLint linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistGoLintLinter.php` (an
//! `ArcanistExternalLinter`).
//!
//! Behavior: runs `golint <file>` (no flags) on Go files and parses
//! stdout lines shaped like `<file>:<line>:<col>: <message>`. Every
//! message has code and name `GOLINT`, advice severity, and an
//! uppercased-first-letter description, mirroring `parseLinterOutput()`.
//!
//! The `bin`, `flags`, and `version` linter options apply (N4). A
//! nonzero tool exit yields no messages (the PHP linter raises via
//! `resolvex()` since `shouldExpectCommandErrors()` is false, but the
//! [`Linter`](arc_rs::lint::Linter) trait has no error channel).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// Severity mirroring `ArcanistLintSeverity`; `lint.rs` only defines the
/// severities the text linter uses.
const SEVERITY_ADVICE: &str = "advice";

/// GoLint linter: Go style findings via an external binary.
#[derive(Debug, Default)]
pub struct GolintLinter {
    options: ToolOptions,
}

impl GolintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "golint"
}

/// Mandatory flags, mirroring `getMandatoryFlags()`: none.
fn mandatory_flags() -> Vec<String> {
    Vec::new()
}

/// Default flags, mirroring `getDefaultFlags()`: none.
fn default_flags() -> Vec<String> {
    Vec::new()
}

/// Exact tool argv flags: none (the PHP linter defines neither mandatory
/// nor default flags).
#[must_use]
pub fn command_flags() -> Vec<String> {
    let mut flags = mandatory_flags();
    flags.extend(default_flags());
    flags
}

fn is_supported(path: &str) -> bool {
    extension_of(path).is_some_and(|ext| ext.eq_ignore_ascii_case("go"))
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
            "arc-rs-lint-golint-{}-{nanos}-{n}",
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
        _ => "src.go".to_string(),
    }
}

fn run_tool(binary: &Path, file: &Path, flags: &[String]) -> Option<(i32, String, String)> {
    let mut command = Command::new(binary);
    command.args(flags).arg(file).stdin(Stdio::null());
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
    lint_with_binary_and_flags(path, data, binary, &command_flags())
}

/// Linter body with explicit flags (configured flags replace defaults).
fn lint_with_binary_and_flags(
    path: &str,
    data: &[u8],
    binary: Option<&Path>,
    flags: &[String],
) -> Vec<LintMessage> {
    let Some(binary) = binary else {
        return Vec::new();
    };
    let Ok(staged) = StagedFile::stage(path, data) else {
        return Vec::new();
    };
    let Some((code, stdout, stderr)) = run_tool(binary, staged.path(), flags) else {
        return Vec::new();
    };
    parse_output(path, code, &stdout, &stderr)
}

/// Parse golint stdout, mirroring `parseLinterOutput()`: each
/// `<file>:<line>:<col>: <message>` line becomes one advice message.
/// A nonzero exit means the tool itself failed (PHP raises), so it
/// yields no messages.
#[must_use]
pub fn parse_output(path: &str, exit_code: i32, stdout: &str, _stderr: &str) -> Vec<LintMessage> {
    if exit_code != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, ':');
        let (Some(_file), Some(line_no), Some(col_no), Some(message)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        out.push(LintMessage {
            path: path.to_string(),
            line: parse_num(line_no),
            col: parse_num(col_no),
            code: "GOLINT".to_string(),
            severity: SEVERITY_ADVICE.to_string(),
            name: "GOLINT".to_string(),
            description: ucfirst(message.trim()),
            original: None,
            replacement: None,
        });
    }
    out
}

/// 1-based position component; unparsable or zero values map to 1.
fn parse_num(text: &str) -> usize {
    text.trim().parse::<usize>().unwrap_or(1).max(1)
}

/// Uppercase a leading ASCII lowercase byte, mirroring PHP `ucfirst()`
/// (byte-based: non-ASCII initials pass through unchanged).
fn ucfirst(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    if let Some(first) = bytes.first_mut() {
        first.make_ascii_uppercase();
    }
    // ASCII case mapping preserves UTF-8 validity.
    String::from_utf8(bytes).unwrap_or_else(|_| text.to_string())
}

impl Linter for GolintLinter {
    fn name(&self) -> &'static str {
        "GOLINT"
    }

    fn applies_custom_severities(&self) -> bool {
        // The PHP linter sets severities directly and never consults
        // custom severity maps.
        false
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported(path) {
            return Vec::new();
        }
        if data.contains(&0) {
            return Vec::new();
        }
        let binary = self.options.bin.clone().or_else(binary_on_path);
        let Some(ref binary) = binary else {
            return Vec::new();
        };
        if let Some(requirement) = &self.options.version
            && !version_requirement_satisfied(
                version_pattern_for("golint"),
                binary,
                requirement,
                "ArcanistGoLintLinter",
            )
        {
            return Vec::new();
        }
        let flags = self
            .options
            .command_flags(&mandatory_flags(), &default_flags());
        lint_with_binary_and_flags(path, data, Some(binary), &flags)
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }
}

#[cfg(test)]
#[path = "lint_golint_tests.rs"]
mod lint_golint_tests;
