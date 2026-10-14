//! `arc-rs lint` HLint linter (T1128).
//!
//! PHP reference: `src/lint/linter/ArcanistHLintLinter.php` (an
//! `ArcanistExternalLinter`).
//!
//! Behavior: runs `hlint --json <file>` on Haskell files and parses the
//! stdout JSON array. Each idea becomes one message with code `HLINT`,
//! name `<hint>`, and description `In module `<m>`, declaration `<d>`.`
//! plus ` **NOTE**: <note>.` suffixes; `Error` maps to `error`,
//! `Warning` to `warning`, and anything else to `advice`, mirroring
//! `parseLinterOutput()`.
//!
//! Known limits (see T1128): the `bin`, `flags`, and `version` linter
//! options are not supported; invalid JSON yields no messages (the PHP
//! linter raises, but the [`Linter`](arc_rs::lint::Linter) trait has no
//! error channel); null array elements are skipped.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use arc_rs::lint::{LintMessage, Linter, SEVERITY_ERROR};

/// Companion severities mirroring `ArcanistLintSeverity`; `lint.rs` only
/// defines the severities the text linter uses.
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";

/// HLint linter: Haskell suggestions via an external binary.
#[derive(Debug, Default)]
pub struct HlintLinter;

impl HlintLinter {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Default binary, mirroring `getDefaultBinary()`.
#[must_use]
pub fn binary_name() -> &'static str {
    "hlint"
}

/// Exact tool argv flags, mirroring `getMandatoryFlags()`.
#[must_use]
pub fn command_flags() -> Vec<String> {
    vec!["--json".to_string()]
}

fn is_supported(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["hs", "lhs"];
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
            "arc-rs-lint-hlint-{}-{nanos}-{n}",
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
        _ => "src.hs".to_string(),
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

/// Parse hlint `--json` stdout, mirroring `parseLinterOutput()`. The exit
/// code is ignored like the PHP parser; invalid JSON yields no messages.
#[must_use]
pub fn parse_output(path: &str, _exit_code: i32, stdout: &str, _stderr: &str) -> Vec<LintMessage> {
    let Ok(serde_json::Value::Array(ideas)) = serde_json::from_str::<serde_json::Value>(stdout)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for idea in &ideas {
        let Some(message) = parse_idea(path, idea) else {
            continue;
        };
        out.push(message);
    }
    out
}

fn parse_idea(path: &str, idea: &serde_json::Value) -> Option<LintMessage> {
    if !idea.is_object() {
        return None;
    }
    let text = |key: &str| {
        idea.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
    let module = text("module");
    let decl = text("decl");
    let hint = text("hint");
    let mut description = format!("In module `{module}`, declaration `{decl}`.");
    if let Some(notes) = idea.get("note").and_then(serde_json::Value::as_array) {
        for note in notes {
            if let Some(note) = note.as_str() {
                description.push_str(&format!(" **NOTE**: {}.", note.trim_matches('"')));
            }
        }
    }
    let severity = match text("severity") {
        "Error" => SEVERITY_ERROR,
        "Warning" => SEVERITY_WARNING,
        _ => SEVERITY_ADVICE,
    };
    Some(LintMessage {
        path: path.to_string(),
        line: json_position(idea.get("startLine")),
        col: json_position(idea.get("startColumn")),
        code: "HLINT".to_string(),
        severity: severity.to_string(),
        name: hint.to_string(),
        description,
        original: None,
        replacement: None,
    })
}

/// 1-based position from a JSON number or string; missing, unparsable,
/// or zero values map to 1.
fn json_position(value: Option<&serde_json::Value>) -> usize {
    let raw = match value {
        Some(serde_json::Value::Number(number)) => {
            if let Some(n) = number.as_u64() {
                return usize::try_from(n).unwrap_or(1).max(1);
            }
            if let Some(n) = number.as_i64() {
                return usize::try_from(n).unwrap_or(1).max(1);
            }
            number.as_f64().unwrap_or(1.0).to_string()
        }
        Some(serde_json::Value::String(text)) => text.to_string(),
        _ => return 1,
    };
    raw.trim().parse::<usize>().unwrap_or(1).max(1)
}

impl Linter for HlintLinter {
    fn name(&self) -> &'static str {
        "HLINT"
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
#[path = "lint_hlint_tests.rs"]
mod lint_hlint_tests;
