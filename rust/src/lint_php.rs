//! `arc lint` PHP linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistPhpLinter.php` (an
//! `ArcanistExternalLinter` wrapping `php -l`).
//!
//! Behavior: runs `php -l <file>` on PHP files and scans the combined
//! stderr/stdout for the first
//! `[(PHP )]<type> error: <message> in <file> on line <line>` diagnostic.
//! `Parse` maps to code `PHP1` ("Parse Error") and `Fatal` to code `PHP2`
//! ("Fatal Error"); both are error severity. Like `php -l`, only the
//! first error is reported.
//!
//! The `bin`, `flags`, and `version` linter options apply (N4). An
//! unrecognized error type yields no messages (the PHP linter raises,
//! but the [`Linter`](arc_rs::lint::Linter) trait has no error
//! channel). `severity` keys are rejected (`canCustomize` is false).

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter, OptionContext, SEVERITY_ERROR};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// PHP linter: syntax errors via `php -l`.
#[derive(Debug, Default)]
pub struct PhpLinter {
    options: ToolOptions,
}

impl PhpLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "php"
}

/// Mandatory flags, mirroring `getMandatoryFlags()`.
fn mandatory_flags() -> Vec<String> {
    vec!["-l".to_string()]
}

/// Default flags, mirroring `getDefaultFlags()`: none.
fn default_flags() -> Vec<String> {
    Vec::new()
}

/// Exact tool argv flags, mirroring `getMandatoryFlags()`.
#[must_use]
pub fn command_flags() -> Vec<String> {
    let mut flags = mandatory_flags();
    flags.extend(default_flags());
    flags
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
            "arc-rs-lint-php-{}-{nanos}-{n}",
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

/// Parse `php -l` output, mirroring `parseLinterOutput()`: stderr and
/// stdout are combined (older PHP prints to stdout, newer splits the
/// streams) and the first
/// `[(PHP )]<type> error: <message> in <file> on line <line>` diagnostic
/// becomes the single message. The exit code is ignored like the PHP
/// parser. Greedy splits mirror the greedy PHP regex groups.
#[must_use]
pub fn parse_output(path: &str, _exit_code: i32, stdout: &str, stderr: &str) -> Vec<LintMessage> {
    let combined = format!("{stderr}\n{stdout}");
    for raw in combined.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let Some(found) = parse_line(line) else {
            continue;
        };
        let (kind, message, line_no) = found;
        let Some((code, name)) = lint_name(&kind) else {
            // Unrecognized error type: the PHP linter throws, which
            // surfaces no messages either.
            return Vec::new();
        };
        return vec![LintMessage {
            path: path.to_string(),
            line: line_no,
            col: 1,
            code: format!("PHP{code}"),
            severity: SEVERITY_ERROR.to_string(),
            name: name.to_string(),
            description: message,
            original: None,
            replacement: None,
        }];
    }
    Vec::new()
}

/// Parse one diagnostic line into `(type, message, line)`.
fn parse_line(line: &str) -> Option<(String, String, usize)> {
    let body = line.strip_prefix("PHP ").unwrap_or(line);
    // Greedy `<type>`: split at the last ` error: ` like `.+ error: +`.
    let marker = " error: ";
    let split = body.rfind(marker)?;
    let kind = body[..split].to_string();
    if kind.is_empty() {
        return None;
    }
    let mut rest = body[split + marker.len()..].trim_start().to_string();
    if rest.is_empty() {
        return None;
    }
    // Trailing ` on line <digits>$`.
    let marker = " on line ";
    let split = rest.rfind(marker)?;
    let digits = rest[split + marker.len()..].trim().to_string();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.truncate(split);
    // Greedy `<message>`: split at the last ` in ` like `.+ in .+`.
    let marker = " in ";
    let split = rest.rfind(marker)?;
    let message = rest[..split].trim().to_string();
    let file = rest[split + marker.len()..].trim();
    if message.is_empty() || file.is_empty() {
        return None;
    }
    // `php -l` only reports real lines, but stay 1-based regardless.
    let line_no = digits.parse::<usize>().unwrap_or(1).max(1);
    Some((kind, message, line_no))
}

/// `(code, name)` for a `php -l` error type, mirroring
/// `getLintCodeFromLinterConfigurationKey()` plus `getLintNameMap()`.
fn lint_name(kind: &str) -> Option<(u8, &'static str)> {
    if kind.eq_ignore_ascii_case("parse") {
        Some((1, "Parse Error"))
    } else if kind.eq_ignore_ascii_case("fatal") {
        Some((2, "Fatal Error"))
    } else {
        None
    }
}

impl Linter for PhpLinter {
    fn name(&self) -> &'static str {
        "PHP"
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
                version_pattern_for("php"),
                binary,
                requirement,
                "ArcanistPhpLinter",
            )
        {
            return Vec::new();
        }
        let flags = self
            .options
            .command_flags(&mandatory_flags(), &default_flags());
        lint_with_binary_and_flags(path, data, Some(binary), &flags)
    }

    fn can_customize_severities(&self) -> bool {
        false
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
#[path = "lint_php_tests.rs"]
mod lint_php_tests;
