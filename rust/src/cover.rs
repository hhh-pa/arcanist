//! `arc-rs cover`: show blame for changed lines.
//!
//! PHP reference: `src/workflow/ArcanistCoverWorkflow.php`: blame the lines
//! changed in the working copy (or since `--rev`) and group them by author,
//! so reviewers for touched code can be found.
//!
//! Scope: git working copies. Changed lines come from
//! `git diff --unified=0 <base>`; blame comes from
//! `git blame --line-porcelain`. Untracked files have no blame, so their
//! lines stay unattributed (matching `arc`, where no author is found for
//! them). Subversion and Mercurial working copies are not yet ported.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::git;

/// Subcommand name, matching `arc cover`.
pub const NAME: &str = "cover";

fn usage() -> String {
    "usage: arc-rs cover [--rev REV] [PATH...]\n\
     \n\
     Cover your professional reputation. Show blame for the lines you changed\n\
     in your working copy (or since REV), grouped by author.\n\
     \n\
     Options:\n\
     \x20 --rev REV      Cover changes since a specific revision.\n\
     \x20 -h, --help     Show this help.\n\
     \n\
     Not yet ported (see T1128): Subversion and Mercurial working copies.\n"
        .to_string()
}

/// Parsed `cover` options.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub rev: Option<String>,
    pub paths: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// Parse `argv` (excluding the subcommand name).
pub fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut rev = None;
    let mut paths = Vec::new();
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            paths.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--rev" {
            let Some(value) = iter.next() else {
                return ParseOutcome::Error("--rev requires a value".to_string());
            };
            rev = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--rev=") {
            rev = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            paths.push(arg.clone());
        }
    }
    ParseOutcome::Args(Options { rev, paths })
}

/// Resolve `rev` to a commit SHA, or `None` when it does not resolve.
fn resolve_commit(dir: &Path, rev: &str) -> Option<String> {
    let arg = format!("{rev}^{{commit}}");
    git::stdout_text(dir, &["rev-parse", "--verify", &arg])
        .ok()
        .filter(|sha| !sha.is_empty())
}

/// Reject non-git working copies: Mercurial and Subversion report
/// not-yet-ported, anything else is an error.
fn check_working_copy(dir: &Path) -> Result<(), String> {
    if git::is_inside_work_tree(dir) {
        return Ok(());
    }
    for ancestor in dir.ancestors() {
        if ancestor.join(".hg").is_dir() {
            return Err("Mercurial working copies are not yet ported (see T1128)".to_string());
        }
        if ancestor.join(".svn").is_dir() {
            return Err("Subversion working copies are not yet ported (see T1128)".to_string());
        }
    }
    Err("\"arc-rs cover\" must be run in a Git, Mercurial, or Subversion working copy.".to_string())
}

/// True when `path` exists in the worktree or at `base` (which covers files
/// deleted from the worktree but present in the diff).
fn path_known(dir: &Path, path: &str, base: Option<&str>) -> bool {
    if dir.join(path).exists() {
        return true;
    }
    if let Some(base) = base {
        let spec = format!("{base}:./{path}");
        if git::ok(dir, &["cat-file", "-e", &spec]) {
            return true;
        }
    }
    false
}

/// Changed new-side line numbers for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChanges {
    pub path: String,
    pub lines: Vec<u64>,
}

/// Parse the new-side `+start[,count]` range from a hunk header body such as
/// `-1,3 +5,7 @@`. Returns `None` for empty ranges (pure deletions).
fn parse_hunk(header: &str) -> Option<(u64, u64)> {
    let plus = header
        .split_whitespace()
        .find(|part| part.starts_with('+'))?;
    let range: Vec<&str> = plus.trim_start_matches('+').split(',').collect();
    let start: u64 = range.first()?.parse().ok()?;
    let count: u64 = match range.get(1) {
        None => 1,
        Some(text) => text.parse().ok()?,
    };
    if count == 0 {
        return None;
    }
    Some((start, count))
}

/// Fallback path from a `diff --git a/old b/new` line, for sections without
/// `+++` lines (binary files, mode-only changes, pure renames).
fn fallback_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git ")?;
    let marker = rest.rfind(" b/")?;
    Some(rest[marker + 3..].to_string())
}

fn apply_plusplus_line(files: &mut [FileChanges], path: &str) {
    if path == "/dev/null" {
        return;
    }
    let path = path.strip_prefix("b/").unwrap_or(path).to_string();
    if let Some(file) = files.last_mut() {
        file.path = path;
    }
}

fn apply_minusminus_line(files: &mut [FileChanges], path: &str) {
    if path == "/dev/null" {
        return;
    }
    let path = path.strip_prefix("a/").unwrap_or(path).to_string();
    if let Some(file) = files.last_mut()
        && file.path.is_empty()
    {
        file.path = path;
    }
}

fn apply_hunk_line(files: &mut [FileChanges], header: &str) {
    let Some((start, count)) = parse_hunk(header) else {
        return;
    };
    if let Some(file) = files.last_mut() {
        file.lines.extend(start..start + count);
    }
}

fn apply_diff_line(files: &mut Vec<FileChanges>, line: &str) {
    if line.starts_with("diff --git ") {
        let path = fallback_path(line).unwrap_or_default();
        files.push(FileChanges {
            path,
            lines: Vec::new(),
        });
    } else if let Some(path) = line.strip_prefix("+++ ") {
        apply_plusplus_line(files, path);
    } else if let Some(path) = line.strip_prefix("--- ") {
        apply_minusminus_line(files, path);
    } else if let Some(header) = line.strip_prefix("@@ ") {
        apply_hunk_line(files, header);
    }
}

/// Parse `git diff --unified=0` output into per-file changed lines. Files
/// without new-side lines (deletions, binary files, mode changes) are kept
/// with empty line lists so callers can tell "changed but unattributable"
/// from "unchanged".
pub fn parse_diff_unified0(text: &str) -> Vec<FileChanges> {
    let mut files: Vec<FileChanges> = Vec::new();
    for line in text.lines() {
        apply_diff_line(&mut files, line);
    }
    files.retain(|file| !file.path.is_empty());
    files
}

/// Parse `git blame --line-porcelain` output into the author of each final
/// line, in order (index 0 is line 1).
pub fn parse_blame_porcelain(text: &str) -> Vec<String> {
    let mut authors = Vec::new();
    let mut author = String::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("author ") {
            author = name.to_string();
        } else if line.starts_with('\t') {
            authors.push(author.clone());
        }
    }
    authors
}

/// Blame `path` in `dir`, returning the author of each line (index 0 is
/// line 1).
pub fn blame_file(dir: &Path, path: &str) -> Result<Vec<String>, String> {
    let text = git::stdout_text(dir, &["blame", "--line-porcelain", "--", path])
        .map_err(|detail| format!("cannot blame '{path}': {detail}"))?;
    Ok(parse_blame_porcelain(&text))
}

/// Format line numbers as readable ranges (`1-3, 5`), mirroring
/// `readableSequenceFromLineNumbers`.
pub fn readable_sequence(lines: &[u64]) -> String {
    let mut sorted: Vec<u64> = lines.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < sorted.len() {
        let first = sorted[index];
        let mut last = first;
        while index + 1 < sorted.len() && sorted[index + 1] == last + 1 {
            index += 1;
            last += 1;
        }
        if last > first {
            parts.push(format!("{first}-{last}"));
        } else {
            parts.push(format!("{first}"));
        }
        index += 1;
    }
    parts.join(", ")
}

fn git_args(refs: &[String]) -> Vec<&str> {
    refs.iter().map(String::as_str).collect()
}

/// Collect changed lines for tracked files via `git diff`.
fn diff_changes(dir: &Path, base: &str, paths: &[String]) -> Result<Vec<FileChanges>, String> {
    let mut owned: Vec<String> = vec![
        "-c".to_string(),
        "core.quotepath=false".to_string(),
        "diff".to_string(),
        "--no-color".to_string(),
        "--unified=0".to_string(),
        base.to_string(),
        "--".to_string(),
    ];
    owned.extend(paths.iter().cloned());
    let args = git_args(&owned);
    let text =
        git::stdout_text(dir, &args).map_err(|detail| format!("git diff failed: {detail}"))?;
    Ok(parse_diff_unified0(&text))
}

/// List untracked files under `paths` (worktree-relative display paths).
fn untracked_files(dir: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    let mut owned: Vec<String> = vec![
        "-c".to_string(),
        "core.quotepath=false".to_string(),
        "ls-files".to_string(),
        "--others".to_string(),
        "--exclude-standard".to_string(),
        "--".to_string(),
    ];
    owned.extend(paths.iter().cloned());
    let args = git_args(&owned);
    let text =
        git::stdout_text(dir, &args).map_err(|detail| format!("git ls-files failed: {detail}"))?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Changed lines grouped by author, then by file.
type Covers = BTreeMap<String, BTreeMap<String, Vec<u64>>>;

fn resolve_cover_base(dir: &Path, rev: Option<&str>) -> Result<Option<String>, String> {
    match rev {
        Some(rev) => match resolve_commit(dir, rev) {
            Some(sha) => Ok(Some(sha)),
            None => Err(format!("invalid revision '{rev}'.")),
        },
        None => Ok(resolve_commit(dir, "HEAD")),
    }
}

fn check_cover_paths(dir: &Path, paths: &[String], base: Option<&str>) -> Result<(), String> {
    for path in paths {
        if !path_known(dir, path, base) {
            return Err(format!("path does not exist: {path}"));
        }
    }
    Ok(())
}

fn collect_cover_changes(
    dir: &Path,
    base: Option<&str>,
    paths: &[String],
) -> Result<(Vec<FileChanges>, Vec<String>), String> {
    let tracked = match base {
        Some(base) => diff_changes(dir, base, paths)?,
        None => Vec::new(),
    };
    let untracked = untracked_files(dir, paths)?;
    Ok((tracked, untracked))
}

fn attribute_lines(authors: &[String], changes: &FileChanges, covers: &mut Covers) {
    for line in &changes.lines {
        let index = (*line as usize).saturating_sub(1);
        let Some(author) = authors.get(index) else {
            continue;
        };
        if author.is_empty() {
            continue;
        }
        covers
            .entry(author.clone())
            .or_default()
            .entry(changes.path.clone())
            .or_default()
            .push(*line);
    }
}

fn blame_cover_lines(dir: &Path, tracked: &[FileChanges]) -> Result<Covers, String> {
    let mut covers: Covers = BTreeMap::new();
    for changes in tracked {
        if changes.lines.is_empty() {
            continue;
        }
        let authors = blame_file(dir, &changes.path)?;
        attribute_lines(&authors, changes, &mut covers);
    }
    Ok(covers)
}

fn print_covers(covers: &Covers) {
    for (author, files) in covers {
        println!("{author}");
        for (file, lines) in files {
            println!(
                "  {file}: {} line(s) {}",
                lines.len(),
                readable_sequence(lines)
            );
        }
    }
}

fn execute_cover(dir: &Path, options: &Options) -> Result<(), String> {
    check_working_copy(dir)?;
    let base = resolve_cover_base(dir, options.rev.as_deref())?;
    check_cover_paths(dir, &options.paths, base.as_deref())?;
    let (tracked, untracked) = collect_cover_changes(dir, base.as_deref(), &options.paths)?;
    if tracked.is_empty() && untracked.is_empty() {
        println!("You're covered, you didn't change anything.");
        return Ok(());
    }
    let covers = blame_cover_lines(dir, &tracked)?;
    // Untracked files carry no blame, so their lines stay unattributed.
    if covers.is_empty() {
        println!("You're covered, your changes didn't touch anyone else's code.");
        return Ok(());
    }
    print_covers(&covers);
    Ok(())
}

/// Cover changed lines in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(options) => options,
    };
    match execute_cover(dir, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "cover_tests.rs"]
mod cover_tests;
