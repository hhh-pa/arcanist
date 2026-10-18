//! `arc lint` PuppetLint linter (T1128).
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
//! The `bin`, `flags`, `version`, and `puppet-lint.config` linter
//! options apply (N4). Lines that do not parse are skipped.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::Stdio;

use arc_rs::lint::{LintMessage, Linter, OptionContext, SEVERITY_ERROR};
use arc_rs::lint_external::{ToolOptions, apply_tool_option, version_requirement_satisfied};
use arc_rs::linters::version_pattern_for;

/// Companion severities mirroring `ArcanistLintSeverity`; `lint.rs` only
/// defines the severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";

/// PuppetLint linter: Puppet style-guide findings via an external binary.
#[derive(Debug, Default)]
pub struct PuppetlintLinter {
    options: ToolOptions,
    config: Option<String>,
}

impl PuppetlintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "puppet-lint"
}

/// Exact tool argv flags, mirroring `getMandatoryFlags()` without a
/// configured config file.
#[must_use]
pub fn command_flags() -> Vec<String> {
    let mut flags = mandatory_flags();
    flags.extend(default_flags(None));
    flags
}

/// Mandatory flags, mirroring `getMandatoryFlags()`.
fn mandatory_flags() -> Vec<String> {
    vec![
        "--error-level=all".to_string(),
        "--log-format=%{linenumber}|%{column}|%{kind}|%{check}|%{message}".to_string(),
    ]
}

/// Default flags, mirroring `getDefaultFlags()`: `--config=` when a
/// config file is configured (custom `flags` replace this).
fn default_flags(config: Option<&str>) -> Vec<String> {
    config
        .map(|path| vec![format!("--config={path}")])
        .unwrap_or_default()
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
        #[cfg(windows)]
        {
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

/// Uppercase ASCII initials after whitespace, mirroring PHP `ucwords()`
/// (byte-based: only `a-z` change, so non-ASCII bytes pass through).
fn ucwords(text: &str) -> String {
    let mut out = text.as_bytes().to_vec();
    let mut start = true;
    for byte in &mut out {
        if start {
            byte.make_ascii_uppercase();
        }
        start = matches!(*byte, b' ' | b'\t' | b'\r' | b'\n' | b'\x0C' | b'\x0B');
    }
    // ASCII case mapping preserves UTF-8 validity.
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

impl Linter for PuppetlintLinter {
    fn name(&self) -> &'static str {
        "PUPPETLINT"
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
                version_pattern_for("puppet-lint"),
                binary,
                requirement,
                "ArcanistPuppetLintLinter",
            )
        {
            return Vec::new();
        }
        let flags = self
            .options
            .command_flags(&mandatory_flags(), &default_flags(self.config.as_deref()));
        lint_with_binary_and_flags(path, data, Some(binary), &flags)
    }

    fn set_option(
        &mut self,
        key: &str,
        value: &serde_json::Value,
        ctx: &OptionContext,
    ) -> Result<(), String> {
        if key == "puppet-lint.config" {
            let Some(config) = value.as_str() else {
                return Err("'puppet-lint.config' must be a string".to_string());
            };
            self.config = Some(config.to_string());
            return Ok(());
        }
        apply_tool_option(&mut self.options, key, value, ctx.project_root)
    }
}

#[cfg(test)]
#[path = "lint_puppetlint_tests.rs"]
mod lint_puppetlint_tests;
