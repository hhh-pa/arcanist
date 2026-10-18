//! `arc lint` Cpplint linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistCpplintLinter.php` (an
//! `ArcanistExternalLinter` wrapping Google's `cpplint.py`).
//!
//! Behavior: runs `cpplint.py <file>` (no flags) on C/C++ files and
//! parses stderr lines shaped like
//! `<line>: <message> [<category>] [<confidence>]`. Every message has
//! warning severity with code and name set to the cpplint category.
//! Cpplint reports whole-file findings (such as a missing `#ifndef`
//! guard) on line 0; those map to line 1 because engine messages are
//! 1-based.
//!
//! The `bin`, `flags`, and `version` linter options apply (N4):
//! configured binaries are resolved at config time, configured flags
//! replace the (empty) defaults, and version requirements gate the run
//! (an undeterminable version, like here where PHP defines no
//! `getVersion()`, keeps the linter inactive). Unparsable tool output
//! yields no messages (the PHP linter raises, but the
//! [`Linter`](arc_rs::lint::Linter) trait has no error channel).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use arc_rs::lint::{LintMessage, Linter, OptionContext};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// Severity mirroring `ArcanistLintSeverity`; `lint.rs` only defines the
/// severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";

/// Cpplint linter: Google's C++ style guide via an external binary.
#[derive(Debug, Default)]
pub struct CpplintLinter {
    options: ToolOptions,
}

impl CpplintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "cpplint.py"
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

/// C/C++ extensions this linter applies to. (PHP scopes linters through
/// `.arclint` `include` patterns instead; these defaults keep the
/// run-everything engine fast.)
fn is_supported(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["c", "h", "cpp", "hpp", "cc", "hh", "cxx", "hxx"];
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
            let exe = dir.join(format!("{binary}.exe")); // LCOV_EXCL_START // Reason: windows-only .exe probe; cfg!(windows) is false on ubuntu CI
            if exe.is_file() {
                return Some(exe);
            } // LCOV_EXCL_STOP
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
            "arc-rs-lint-cpplint-{}-{nanos}-{n}",
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
        _ => "src.cpp".to_string(),
    }
}

fn run_tool(binary: &Path, file: &Path, flags: &[String]) -> Option<(i32, String, String)> {
    let mut command = crate::proc_env::command(binary);
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
        return Vec::new(); // LCOV_EXCL_LINE // Reason: staging failure; hermetic tests cannot fail temp-dir writes
    };
    let Some((code, stdout, stderr)) = run_tool(binary, staged.path(), flags) else {
        return Vec::new();
    };
    parse_output(path, code, &stdout, &stderr)
}

/// Parse cpplint stderr, mirroring `parseLinterOutput()`:
/// `<line>: <message> [<category>] [<confidence>]`. Lines that do not
/// match are skipped; the exit code is ignored like the PHP parser.
#[must_use]
pub fn parse_output(path: &str, _exit_code: i32, _stdout: &str, stderr: &str) -> Vec<LintMessage> {
    let mut out = Vec::new();
    for raw in stderr.lines() {
        let Some((line, message, category)) = parse_line(raw.trim()) else {
            continue;
        };
        out.push(LintMessage {
            path: path.to_string(),
            line,
            col: 1,
            code: category.clone(),
            severity: SEVERITY_WARNING.to_string(),
            name: category,
            description: message,
            original: None,
            replacement: None,
            granularity: None,
            locations: Vec::new(),
            bypass_changed_line_filtering: None,
            original_bytes: None,
            replacement_bytes: None,
        });
    }
    out
}

/// Parse one cpplint finding line into `(line, message, category)`.
fn parse_line(line: &str) -> Option<(usize, String, String)> {
    let bracketed = line.strip_suffix(']')?;
    let confidence_open = bracketed.rfind('[')?;
    let confidence = bracketed[confidence_open + 1..].trim();
    if confidence.is_empty() || !confidence.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let head = bracketed[..confidence_open].trim_end();
    let head = head.strip_suffix(']')?;
    let category_open = head.rfind('[')?;
    let category = head[category_open + 1..].trim().to_string();
    let head = head[..category_open].trim_end();
    let (line, message) = split_line_number(head)?;
    Some((line, message, category))
}

/// Split `<file>:<line>: <message>` at the leftmost `<digits>:` run,
/// mirroring the unanchored PHP `/(\d+):.../` match (file paths may
/// contain digits and colons). Like the PHP parser, an empty message is
/// accepted. Cpplint line 0 means whole-file; engine messages are
/// 1-based.
fn split_line_number(head: &str) -> Option<(usize, String)> {
    let bytes = head.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b':' {
            let line = head[i..j].parse::<usize>().unwrap_or(1).max(1);
            return Some((line, head[j + 1..].trim().to_string()));
        }
        i = j;
    }
    None
}

impl Linter for CpplintLinter {
    fn name(&self) -> &'static str {
        "C++ Google's Styleguide"
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
                version_pattern_for("cpplint"),
                binary,
                requirement,
                "ArcanistCpplintLinter",
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

    fn validate_severity_code(&self, code: &str) -> Result<String, String> {
        // `@^[a-z_]+/[a-z0-9_+]+$@`, like PHP.
        let mut parts = code.split('/');
        let ok = match (parts.next(), parts.next(), parts.next()) {
            (Some(head), Some(tail), None) => {
                !head.is_empty()
                    && head.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                    && !tail.is_empty()
                    && tail.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'+'
                    })
            }
            _ => false,
        };
        if ok {
            return Ok(code.to_string());
        }
        Err(format!(
            "Unrecognized lint message code \"{code}\". Expected a valid cpplint lint code like \
             \"build/include_order\" or \"whitespace/braces\"."
        ))
    }
}

#[cfg(test)]
#[path = "lint_cpplint_tests.rs"]
mod lint_cpplint_tests;
