//! `arc-rs diff`: local change inspection.
//!
//! Read-only half of `src/workflow/ArcanistDiffWorkflow.php`: shows the
//! working copy diff against a base revision as text or JSON.
//! Submitting to Differential is explicitly out of scope.

use std::path::Path;
use std::process::{Command, ExitCode};

/// Subcommand name, matching `arc diff`.
pub const NAME: &str = "diff";

const USAGE: &str = "usage: arc-rs diff [--base REV] [--json]\n";

/// Parsed `diff` options.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    /// Explicit `--base` revision, if given.
    pub base: Option<String>,
    /// Whether `--json` output was requested.
    pub json: bool,
}

/// One file's numstat entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path as reported by git (renames report the new path).
    pub path: String,
    /// Added lines (0 for binary files).
    pub added: u64,
    /// Deleted lines (0 for binary files).
    pub deleted: u64,
}

/// Parse `argv` (excluding the subcommand name).
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut base = None;
    let mut json = false;
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--json" {
            json = true;
        } else if arg == "--base" {
            let Some(value) = iter.next() else {
                return Err("--base requires a value".to_string());
            };
            if value.is_empty() {
                return Err("--base requires a non-empty value".to_string());
            }
            base = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--base=") {
            if value.is_empty() {
                return Err("--base requires a non-empty value".to_string());
            }
            base = Some(value.to_string());
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg == "--" {
            return Err("unexpected argument '--': diff takes no paths".to_string());
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(Some(Options { base, json }))
}

/// Run `git` with `args` in `cwd`, capturing output.
fn git_output(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// True when `cwd` is inside a git working tree.
pub fn is_inside_git_repo(cwd: &Path) -> bool {
    let Ok(out) = git_output(cwd, &["rev-parse", "--is-inside-work-tree"]) else {
        return false;
    };
    out.status.success() && out.stdout.trim_ascii() == b"true"
}

/// Resolve the base revision to a commit SHA.
///
/// Uses the explicit revision when given, else the merge-base of `HEAD`
/// with `@{upstream}` when an upstream exists, else `HEAD`.
pub fn resolve_base(cwd: &Path, explicit: Option<&str>) -> Result<String, String> {
    if let Some(rev) = explicit {
        return rev_to_commit(cwd, rev).ok_or_else(|| format!("invalid base revision '{rev}'"));
    }
    if upstream_exists(cwd)
        && let Ok(out) = git_output(cwd, &["merge-base", "HEAD", "@{upstream}"])
        && out.status.success()
    {
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            return Ok(sha);
        }
    }
    rev_to_commit(cwd, "HEAD")
        .ok_or_else(|| "cannot resolve base: HEAD does not exist (empty repository?)".to_string())
}

/// Resolve `rev` to a commit SHA, or `None` when it does not resolve.
fn rev_to_commit(cwd: &Path, rev: &str) -> Option<String> {
    let arg = format!("{rev}^{{commit}}");
    let out = git_output(cwd, &["rev-parse", "--verify", &arg]).ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// True when the current branch has an upstream configured.
fn upstream_exists(cwd: &Path) -> bool {
    git_output(cwd, &["rev-parse", "--verify", "@{upstream}"]).is_ok_and(|out| out.status.success())
}

/// Parse raw `git diff --numstat -z` output.
pub fn parse_numstat_z(raw: &[u8]) -> Result<Vec<FileDiff>, String> {
    let mut files = Vec::new();
    let mut fields: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    if fields.last() == Some(&b"".as_slice()) {
        fields.pop();
    }
    let mut i = 0;
    while i < fields.len() {
        let record = String::from_utf8_lossy(fields[i]);
        let mut parts = record.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(format!("malformed numstat record {}", i + 1));
        };
        let added = parse_count(added)?;
        let deleted = parse_count(deleted)?;
        i += 1;
        let path = if path.is_empty() {
            // Rename/copied entry: next two fields are old and new paths.
            let (Some(_old), Some(new)) = (fields.get(i), fields.get(i + 1)) else {
                return Err(format!("truncated rename record {}", files.len() + 1));
            };
            i += 2;
            String::from_utf8_lossy(new).into_owned()
        } else {
            path.to_string()
        };
        files.push(FileDiff {
            path,
            added,
            deleted,
        });
    }
    Ok(files)
}

/// Parse one numstat count (`-` for binary files means 0).
fn parse_count(text: &str) -> Result<u64, String> {
    if text == "-" {
        return Ok(0);
    }
    text.parse::<u64>()
        .map_err(|_| format!("malformed numstat count '{text}'"))
}

/// Collect numstat entries for the worktree diff against `base`.
pub fn numstat(cwd: &Path, base: &str) -> Result<Vec<FileDiff>, String> {
    let out = git_output(cwd, &["diff", "--numstat", "-z", base, "--"])?;
    if !out.status.success() {
        return Err(format!(
            "git diff --numstat failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_numstat_z(&out.stdout)
}

/// Total insertions and deletions across `files`.
pub fn summarize(files: &[FileDiff]) -> (u64, u64) {
    files
        .iter()
        .fold((0, 0), |(ins, del), f| (ins + f.added, del + f.deleted))
}

/// Build the `--json` document.
pub fn diff_json(base: &str, files: &[FileDiff]) -> serde_json::Value {
    let (insertions, deletions) = summarize(files);
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "added": f.added,
                "deleted": f.deleted,
            })
        })
        .collect();
    serde_json::json!({
        "base": base,
        "files": files,
        "insertions": insertions,
        "deletions": deletions,
    })
}

/// Stat plus full unified diff against `base`, via the git binary.
pub fn default_diff_output(cwd: &Path, base: &str) -> Result<Vec<u8>, String> {
    let mut combined = Vec::new();
    for args in [
        ["--no-pager", "diff", "--no-color", "--stat", base, "--"].as_slice(),
        ["--no-pager", "diff", "--no-color", base, "--"].as_slice(),
    ] {
        let out = git_output(cwd, args)?;
        if !out.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        combined.extend_from_slice(&out.stdout);
    }
    Ok(combined)
}

/// Inspect `cwd` per `options`, returning the bytes for stdout.
pub fn inspect(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    if !is_inside_git_repo(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let base = resolve_base(cwd, options.base.as_deref())?;
    if options.json {
        let files = numstat(cwd, &base)?;
        let mut text = serde_json::to_string_pretty(&diff_json(&base, &files))
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text.into_bytes())
    } else {
        default_diff_output(cwd, &base)
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc-rs diff: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    match inspect(&cwd, &options) {
        Ok(out) => {
            use std::io::Write;
            if std::io::stdout().write_all(&out).is_err() {
                eprintln!("arc-rs diff: failed to write output");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc-rs diff: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod diff_tests;
