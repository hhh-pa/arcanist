//! `RuboCopLinter`: Ruby checks via the external `rubocop` binary.
//!
//! PHP reference: `src/lint/linter/ArcanistRuboCopLinter.php`.
//! Concrete: `getLinterName()` returns `'RuboCop'`, default binary
//! `rubocop`, with the mandatory flag `--format=json`.
//!
//! Behavior: paths that are not Ruby files (`.rb`/`.rake`/`.gemspec`/
//! `.ru` extensions, or `Rakefile`/`Gemfile` basenames), inputs
//! containing NUL bytes, and runs where `rubocop` is absent from `PATH`
//! yield no messages. Otherwise the input bytes are written to a
//! scratch `target.rb` file and `rubocop --format=json <scratch>`
//! stdout is parsed: each JSON offense becomes a message with the
//! offense's line/column, `cop_name` as code, and severity mapped from
//! the offense severity (`convention`/`refactor`/`warning` warning,
//! `error`/`fatal` error, anything else advice). Unlike PHP, which
//! reports the tool-echoed path, the reported path is always the linted
//! path (the tool only ever sees the scratch file). Invalid JSON or
//! malformed offenses yield no messages.
//!
//! Out of scope (the Rust engine has no per-linter config plumbing):
//! the `rubocop.config` option, custom binaries, extra flags, and
//! version requirements.

extern crate self as arc_rs;

use arc_rs::lint::{LintMessage, Linter};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default binary name, mirroring PHP `getDefaultBinary()`.
pub const BINARY: &str = "rubocop";

const SEVERITY_ERROR: &str = "error";
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ADVICE: &str = "advice";

/// Mandatory command flags, mirroring PHP `getMandatoryFlags()`.
#[must_use]
pub fn mandatory_flags() -> Vec<&'static str> {
    vec!["--format=json"]
}

/// True for paths this linter applies to: Ruby files.
#[must_use]
pub fn is_supported_path(path: &str) -> bool {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if base == "Rakefile" || base == "Gemfile" {
        return true;
    }
    let Some((_, ext)) = base.rsplit_once('.') else {
        return false;
    };
    ext.eq_ignore_ascii_case("rb")
        || ext.eq_ignore_ascii_case("rake")
        || ext.eq_ignore_ascii_case("gemspec")
        || ext.eq_ignore_ascii_case("ru")
}

/// Ruby static code analyzer (external `rubocop` wrapper).
#[derive(Debug, Default)]
pub struct RuboCopLinter;

impl RuboCopLinter {
    /// Create the linter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Linter for RuboCopLinter {
    fn name(&self) -> &'static str {
        "RuboCop"
    }

    fn lint_bytes(&self, path: &str, data: &[u8]) -> Vec<LintMessage> {
        if !is_supported_path(path) || data.contains(&0) {
            return Vec::new();
        }
        let Some(tool) = binary_on_path(BINARY) else {
            // Inactive when the tool is not installed, mirroring PHP.
            return Vec::new();
        };
        let Some(scratch) = write_scratch("rubocop", ".rb", path, data) else {
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

/// Parse `rubocop --format=json` stdout into messages for `path`,
/// mirroring PHP `parseLinterOutput()`. Invalid JSON yields no
/// messages; malformed offenses are skipped.
#[must_use]
pub fn parse_output(path: &str, stdout: &str) -> Vec<LintMessage> {
    let mut out = Vec::new();
    let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return Vec::new();
    };
    let Some(files) = json.get("files").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    for file in files {
        let Some(offenses) = file.get("offenses").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for offense in offenses {
            if let Some(message) = parse_offense(path, offense) {
                out.push(message);
            }
        }
    }
    out
}

fn parse_offense(path: &str, offense: &serde_json::Value) -> Option<LintMessage> {
    let description = offense.get("message")?.as_str()?.to_string();
    let location = offense.get("location")?;
    let line = location.get("line")?.as_u64()? as usize;
    let col = location.get("column")?.as_u64()? as usize;
    let severity_word = offense.get("severity")?.as_str()?;
    let code = offense.get("cop_name")?.as_str()?;
    Some(LintMessage {
        path: path.to_string(),
        line,
        col,
        code: code.to_string(),
        severity: severity_for(severity_word).to_string(),
        name: "RuboCop".to_string(),
        description,
    })
}

fn severity_for(word: &str) -> &'static str {
    match word {
        "convention" | "refactor" | "warning" => SEVERITY_WARNING,
        "error" | "fatal" => SEVERITY_ERROR,
        _ => SEVERITY_ADVICE,
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
#[path = "lint_rubocop_tests.rs"]
mod lint_rubocop_tests;
