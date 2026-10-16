//! `arc cover`: show blame for changed lines.
//!
//! PHP reference: `src/workflow/ArcanistCoverWorkflow.php` with changed
//! lines from `ArcanistDiffHunk::getChangedLines('cover')`
//! (`src/parser/diff/ArcanistDiffHunk.php`), blame from
//! `ArcanistGitAPI::getBlame()`, and path selection from
//! `ArcanistWorkflow::selectPathsForWorkflow()`.
//!
//! Model (git): diff the worktree against the base commit, walk each hunk
//! corpus to find the OLD-side lines a change touches, then blame those
//! lines at the base commit and group them by author. Files added since
//! the base and untracked files are omitted (they carry
//! `FLAG_ADDED`/`FLAG_UNTRACKED`, which cover's omit mask drops); without
//! `--rev` the base is `HEAD`.
//!
//! Divergences from PHP, all deliberate:
//!
//! - No interactive default-commit-range selection: without `--rev` the
//!   base is always `HEAD`, matching historic arc behavior.
//! - Blame failures are skipped silently (unattributed) instead of fatal:
//!   PHP crashes on explicitly passed added files because they cannot be
//!   blamed at the base commit.
//! - Old-side line N attributes the Nth blame entry (1-based). PHP looks
//!   the 1-based line up in a 0-indexed blame list, so it attributes the
//!   next line's author; that off-by-one is not reproduced.
//! - Subversion and Mercurial working copies are not yet ported (T1128).
//!
//! Console output goes through the shared [`arc_rs::terminal`] API:
//! `phutil_console_format()` sites (interpret pattern, substitute raw)
//! use `tsprintf_ansi` with the `%R` (raw string) conversion, `pht()`
//! sites use `pht` (including the real `en_US` plural table), and
//! markup-free `sprintf` sites use plain `format!`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::terminal::{PhtArg, TVal};
use arc_rs::{git, terminal};

/// Subcommand name, matching `arc cover`.
pub const NAME: &str = "cover";

fn usage() -> String {
    "usage: arc cover [--rev revision] [path ...]\n\
     \n\
     Supports: git\n\
     Cover your... professional reputation. Show blame for the lines you\n\
     changed in your working copy (svn) or since some commit (hg, git).\n\
     This will take a minute because blame takes a minute, especially under\n\
     SVN.\n\
     \n\
     Options:\n\
     \x20 --rev REV      Cover changes since a specific revision.\n\
     \x20 -h, --help     Show this help.\n\
     \n\
     Not yet ported (see T1128): Subversion and Mercurial working copies.\n"
        .to_string()
}

/// The empty-tree object PHP accepts as an explicit base
/// (`ArcanistGitAPI::GIT_MAGIC_ROOT_COMMIT`).
const GIT_MAGIC_ROOT_COMMIT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

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

/// Parse `argv` (excluding the subcommand name). Error bytes match
/// PHP's argument parser (`ArcanistWorkflow.php:815-880`).
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
                return ParseOutcome::Error("Option '--rev' requires a parameter.".to_string());
            };
            rev = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--rev=") {
            rev = Some(value.to_string());
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if let Some(long) = arg.strip_prefix("--") {
            let key = long.split('=').next().unwrap_or("");
            return ParseOutcome::Error(format!("Unknown argument '{key}'. Try 'arc help'."));
        } else if let Some(key) = arg.strip_prefix('-') {
            return ParseOutcome::Error(format!("Unknown argument '{key}'. Try 'arc help'."));
        } else {
            paths.push(arg.clone());
        }
    }
    ParseOutcome::Args(Options { rev, paths })
}

/// Stdout text when nothing changed: `ArcanistNoEffectException` renders as
/// `echo $ex->getMessage()."\n";` on stdout with exit 0
/// (`scripts/arcanist.php`).
#[must_use]
pub fn no_changes_message() -> String {
    format!(
        "{}\n",
        terminal::pht("You're covered, you didn't change anything.", &[])
    )
}

/// Stdout text when changes touch no one else's code:
/// `echo pht("You're covered, your changes didn't touch anyone else's
/// code.\n");`.
#[must_use]
pub fn untouched_message() -> String {
    terminal::pht(
        "You're covered, your changes didn't touch anyone else's code.\n",
        &[],
    )
}

/// Resolve `rev` to a commit SHA, or `None` when it does not resolve.
fn resolve_commit(dir: &Path, rev: &str) -> Option<String> {
    let arg = format!("{rev}^{{commit}}");
    git::stdout_text(dir, &["rev-parse", "--verify", &arg])
        .ok()
        .filter(|sha| !sha.is_empty())
}

/// Resolve `--rev` to the merge-base with `HEAD`, like
/// `ArcanistGitAPI::buildBaseCommit()`. On ties git prints several bases;
/// the first wins (PHP would feed the whole blob to later commands).
/// The empty-tree magic commit is accepted directly, like PHP.
fn resolve_rev_base(dir: &Path, rev: &str) -> Result<String, String> {
    if rev == GIT_MAGIC_ROOT_COMMIT {
        return Ok(rev.to_string());
    }
    let failure = || {
        terminal::pht(
            "Unable to find any git commit named '%s' in this repository.",
            &[PhtArg::Text(rev)],
        )
    };
    let out = git::stdout_text(dir, &["merge-base", rev, "HEAD"]).map_err(|_| failure())?;
    out.lines()
        .next()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .ok_or_else(failure)
}

/// Reject non-git working copies: Mercurial and Subversion report
/// not-yet-ported, anything else is the stock outside-a-working-copy
/// usage error from `scripts/arcanist.php`.
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
    Err(terminal::pht(
        "This command must be run in a Git, Mercurial or Subversion working copy.",
        &[],
    ))
}

/// Resolve `.` and `..` lexically (no filesystem access), so
/// `..` segments cannot defeat prefix stripping.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Express absolute `target` relative to `dir` (both cleaned
/// lexically), for git pathspecs. Returns `None` when the two share
/// no common prefix.
fn relative_to_dir(dir: &Path, target: &Path) -> Option<String> {
    let dir = lexical_clean(dir);
    let target = lexical_clean(target);
    if let Ok(relative) = target.strip_prefix(&dir) {
        if relative.as_os_str().is_empty() {
            return Some(".".to_string());
        }
        return Some(relative.to_string_lossy().into_owned());
    }
    let mut dir_parts: Vec<_> = dir.components().collect();
    let mut target_parts: Vec<_> = target.components().collect();
    let mut common = 0;
    while common < dir_parts.len()
        && common < target_parts.len()
        && dir_parts[common] == target_parts[common]
    {
        common += 1;
    }
    if common == 0 {
        return None;
    }
    dir_parts.drain(..common);
    target_parts.drain(..common);
    let mut out = PathBuf::new();
    for _ in &dir_parts {
        out.push("..");
    }
    for part in target_parts {
        out.push(part.as_os_str());
    }
    Some(out.to_string_lossy().into_owned())
}

/// Explicit paths must exist in the worktree, like
/// `selectPathsForWorkflow()`: `pht("Path '%s' does not exist!", $path)`.
/// Returns usable git pathspecs: absolute paths resolve to
/// dir-relative form like PHP's `readablePath` normalization, so they
/// select files instead of matching nothing.
fn check_cover_paths(dir: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let candidate = Path::new(path);
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            dir.join(candidate)
        };
        if !absolute.exists() {
            return Err(terminal::pht(
                "Path '%s' does not exist!",
                &[PhtArg::Text(path)],
            ));
        }
        if candidate.is_absolute() {
            match relative_to_dir(dir, candidate) {
                Some(relative) => out.push(relative),
                None => out.push(path.clone()),
            }
        } else {
            out.push(path.clone());
        }
    }
    Ok(out)
}

/// Repository root for resolving diff output paths (which git reports
/// relative to the root no matter where the command runs).
fn repo_top(dir: &Path) -> PathBuf {
    git::stdout_text(dir, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.to_path_buf())
}

/// Changed OLD-side line numbers for one file: the `cover` map of
/// `ArcanistDiffHunk::getChangedLines('cover')`, in corpus-walk order.
/// `0` entries are the new-file quirk (`cover[0]`) and never attributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChanges {
    pub path: String,
    pub lines: Vec<u64>,
}

/// Live counters while walking one hunk corpus. `old_rem`/`new_rem` count
/// down exactly like `ArcanistDiffParser`, so corpus lines that look like
/// section headers (an added `+++ foo` line) stay corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HunkWalk {
    old_line: u64,
    new_line: u64,
    old_rem: u64,
    new_rem: u64,
}

/// Parse one `-start[,count]`/`+start[,count]` range. A missing count
/// implies one line, like `ArcanistDiffParser`.
fn parse_range(range: &str) -> Option<(u64, u64)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Parse a hunk header body (`-1,3 +5,7 @@ ...`) into a fresh walk.
/// Only the ranges before the closing `@@` count, so trailing context
/// like `@@ func +x` cannot poison the parse.
fn parse_hunk_header(header: &str) -> Option<HunkWalk> {
    let ranges = header.split("@@").next()?;
    let mut old = None;
    let mut new = None;
    for part in ranges.split_whitespace() {
        if let Some(range) = part.strip_prefix('+') {
            new = parse_range(range);
        } else if let Some(range) = part.strip_prefix('-') {
            old = parse_range(range);
        }
    }
    let ((old_start, old_len), (new_start, new_len)) = old.zip(new)?;
    if old_len == 0 && new_len == 0 {
        return None;
    }
    Some(HunkWalk {
        old_line: old_start,
        new_line: new_start,
        old_rem: old_len,
        new_rem: new_len,
    })
}

/// True for lines that can appear in a hunk corpus.
fn is_corpus_start(line: &str) -> bool {
    matches!(line.chars().next(), Some('-' | '+' | ' ' | '\\') | None)
}

/// Apply one corpus line to the walk, mirroring
/// `ArcanistDiffHunk::getChangedLines('cover')`: `-` covers the old line,
/// `+` covers the old line and its predecessor, context advances both
/// counters. `\ No newline` markers also advance both counters (the PHP
/// `default` quirk) but consume no hunk length, like `ArcanistDiffParser`.
fn walk_corpus_line(walk: &mut HunkWalk, lines: &mut Vec<u64>, line: &str) {
    match line.chars().next() {
        Some('-') => {
            lines.push(walk.old_line);
            walk.old_line += 1;
            walk.old_rem = walk.old_rem.saturating_sub(1);
        }
        Some('+') => {
            if walk.old_line > 1 {
                lines.push(walk.old_line - 1);
            }
            lines.push(walk.old_line);
            walk.new_line += 1;
            walk.new_rem = walk.new_rem.saturating_sub(1);
        }
        Some('\\') => {
            walk.old_line += 1;
            walk.new_line += 1;
        }
        _ => {
            walk.old_line += 1;
            walk.new_line += 1;
            walk.old_rem = walk.old_rem.saturating_sub(1);
            walk.new_rem = walk.new_rem.saturating_sub(1);
        }
    }
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

struct DiffParse {
    files: Vec<FileChanges>,
    walk: Option<HunkWalk>,
}

fn apply_diff_line(state: &mut DiffParse, line: &str) {
    if state.walk.is_some() && is_corpus_start(line) {
        let walk = state.walk.as_mut().expect("checked walk");
        if let Some(file) = state.files.last_mut() {
            walk_corpus_line(walk, &mut file.lines, line);
        }
        if walk.old_rem == 0 && walk.new_rem == 0 {
            state.walk = None;
        }
        return;
    }
    state.walk = None;
    if line.starts_with("diff --git ") {
        let path = fallback_path(line).unwrap_or_default();
        state.files.push(FileChanges {
            path,
            lines: Vec::new(),
        });
    } else if let Some(path) = line.strip_prefix("+++ ") {
        apply_plusplus_line(&mut state.files, path);
    } else if let Some(path) = line.strip_prefix("--- ") {
        apply_minusminus_line(&mut state.files, path);
    } else if let Some(header) = line.strip_prefix("@@ ") {
        state.walk = parse_hunk_header(header);
    }
}

/// Parse `git diff --unified=0` output into per-file OLD-side cover lines.
/// Files without cover lines (deletions of nothing, binary files, mode
/// changes) are kept with empty line lists so callers can tell "changed
/// but unattributable" from "unchanged". Lines are deduplicated in
/// first-seen order, like PHP's `cover_map` keys.
pub fn parse_diff_unified0(text: &str) -> Vec<FileChanges> {
    let mut state = DiffParse {
        files: Vec::new(),
        walk: None,
    };
    for line in text.lines() {
        apply_diff_line(&mut state, line);
    }
    state.files.retain(|file| !file.path.is_empty());
    for file in &mut state.files {
        dedupe_cover_lines(&mut file.lines);
    }
    state.files
}

/// Drop duplicate cover lines, keeping first-seen order (PHP's
/// `cover_map` is keyed, so `array_keys()` never repeats).
fn dedupe_cover_lines(lines: &mut Vec<u64>) {
    let mut seen = std::collections::HashSet::new();
    lines.retain(|line| seen.insert(*line));
}

/// One blamed line: the author git reports, plus whether the line sits on
/// the blame boundary. Boundary lines predate the base revision and stay
/// unattributed, like `ArcanistGitAPI::getBlame()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub author: String,
    pub boundary: bool,
}

/// True for `--porcelain`/`--line-porcelain` per-line commit headers
/// (`<40-hex> <orig> <final> <count>`).
fn is_blame_header(line: &str) -> bool {
    line.len() > 41
        && line.as_bytes()[40] == b' '
        && line.as_bytes()[..40].iter().all(|b| b.is_ascii_hexdigit())
}

/// Parse `git blame --porcelain` (grouped or `--line-porcelain`) output
/// into one entry per blamed line, in order.
pub fn parse_blame_porcelain(text: &str) -> Vec<BlameLine> {
    let mut out = Vec::new();
    let mut author = String::new();
    let mut boundary = false;
    for line in text.lines() {
        if is_blame_header(line) {
            boundary = false;
        } else if let Some(name) = line.strip_prefix("author ") {
            author = name.to_string();
        } else if line == "boundary" {
            boundary = true;
        } else if line.starts_with('\t') {
            out.push(BlameLine {
                author: author.clone(),
                boundary,
            });
        }
    }
    out
}

/// Blame `path` (repository-relative) at `HEAD`, like `getBlame()` with
/// the default base commit.
pub fn blame_file(dir: &Path, path: &str) -> Result<Vec<BlameLine>, String> {
    match resolve_commit(dir, "HEAD") {
        Some(head) => blame_file_at(dir, &head, path),
        None => Err(format!("cannot blame '{path}': no HEAD commit")),
    }
}

/// Blame `path` (repository-relative) at `base`, like `getBlame()`:
/// `blame --porcelain -w -M <base>`. `--line-porcelain` reports the same
/// per-line authors without grouping.
pub fn blame_file_at(dir: &Path, base: &str, path: &str) -> Result<Vec<BlameLine>, String> {
    let text = git::stdout_text(
        dir,
        &["blame", "--line-porcelain", "-w", "-M", base, "--", path],
    )
    .map_err(|detail| format!("cannot blame '{path}' at '{base}': {detail}"))?;
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

/// Collect the worktree-vs-base diff, mirroring
/// `getFullGitDiff()`/`getDiffFullOptions()` plus machine-parseable paths.
fn diff_changes(dir: &Path, base: &str, paths: &[String]) -> Result<Vec<FileChanges>, String> {
    let mut owned: Vec<String> = vec![
        "-c".to_string(),
        "core.quotepath=false".to_string(),
        "-c".to_string(),
        "diff.suppressBlankEmpty=false".to_string(),
        "diff".to_string(),
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        "--no-textconv".to_string(),
        "--submodule=short".to_string(),
        "--src-prefix=a/".to_string(),
        "--dst-prefix=b/".to_string(),
        "--unified=0".to_string(),
        "-M".to_string(),
        "-C".to_string(),
        base.to_string(),
        "--".to_string(),
    ];
    owned.extend(paths.iter().cloned());
    let args = git_args(&owned);
    let text =
        git::stdout_text(dir, &args).map_err(|detail| format!("git diff failed: {detail}"))?;
    Ok(parse_diff_unified0(&text))
}

/// True when `path` (repository-relative) exists at `base`. Files missing
/// at the base are PHP's `FLAG_ADDED` set (staged-new and committed-new);
/// cover omits them because they cannot be blamed at the base.
fn exists_at_base(dir: &Path, base: &str, path: &str) -> bool {
    let spec = format!("{base}:{path}");
    git::ok(dir, &["cat-file", "-e", &spec])
}

/// Changed lines grouped by author in first-seen order (PHP's `$covers`
/// is insertion-ordered), then by file in first-seen order (PHP keeps
/// CLI order, not alphabetical).
pub type Covers = Vec<(String, Vec<(String, Vec<u64>)>)>;

fn covers_files_for<'a>(covers: &'a mut Covers, author: &str) -> &'a mut Vec<(String, Vec<u64>)> {
    if !covers.iter().any(|(name, _)| name == author) {
        covers.push((author.to_string(), Vec::new()));
    }
    let entry = covers
        .iter_mut()
        .find(|(name, _)| name == author)
        .expect("cover author just inserted");
    &mut entry.1
}

/// Attribute old-side `changes` lines to their base-blame authors.
/// Boundary lines, missing lines, and empty authors stay unattributed,
/// like the PHP loop; line `0` (the new-file quirk) never attributes.
fn attribute_lines(blame: &[BlameLine], changes: &FileChanges, covers: &mut Covers) {
    for line in &changes.lines {
        if *line == 0 {
            continue;
        }
        let index = (*line as usize).saturating_sub(1);
        let Some(blamed) = blame.get(index) else {
            continue;
        };
        if blamed.boundary || blamed.author.is_empty() {
            continue;
        }
        let files = covers_files_for(covers, &blamed.author);
        if let Some(entry) = files.iter_mut().find(|(path, _)| *path == changes.path) {
            entry.1.push(*line);
        } else {
            files.push((changes.path.clone(), vec![*line]));
        }
    }
}

/// Blame every attributable file. Directories, files gone from the
/// worktree (PHP's `is_dir`/`file_exists` skips), and blame failures
/// (notably added files, which PHP fatals on) contribute nothing.
fn blame_cover_lines(top: &Path, base: Option<&str>, files: &[FileChanges]) -> Covers {
    let Some(base) = base else {
        return Vec::new();
    };
    let mut covers: Covers = Vec::new();
    for changes in files {
        if changes.lines.is_empty() {
            continue;
        }
        let full = top.join(&changes.path);
        if !full.is_file() {
            continue;
        }
        let Ok(blame) = blame_file_at(top, base, &changes.path) else {
            continue;
        };
        attribute_lines(&blame, changes, &mut covers);
    }
    covers
}

/// Render attributed lines with the process ANSI setting.
#[must_use]
pub fn render_covers(covers: &Covers) -> String {
    render_covers_ansi(covers, terminal::ansi_enabled())
}

/// Render attributed lines: `echo phutil_console_format("**%s**\\n",
/// $author);` per author (interpreted pattern, raw substitution, hence
/// `%R`) plus `echo "  {$file}: {$line_noun} {$lines}\\n";` per file,
/// where `$line_noun = pht('%s line(s)', count)` pluralizes through the
/// `en_US` table to `line`/`lines` (the count itself is dropped).
#[must_use]
pub fn render_covers_ansi(covers: &Covers, ansi: bool) -> String {
    let mut out = String::new();
    for (author, files) in covers {
        let head = terminal::tsprintf_ansi("**%R**\n", &[TVal::Text(author)], ansi)
            .map(|t| t.to_string())
            .unwrap_or_default();
        out.push_str(&head);
        for (file, lines) in files {
            let noun = terminal::pht("%s line(s)", &[PhtArg::Number(lines.len() as i64, 0)]);
            out.push_str(&format!("  {file}: {noun} {}\n", readable_sequence(lines)));
        }
    }
    out
}

fn resolve_cover_base(dir: &Path, rev: Option<&str>) -> Result<Option<String>, String> {
    match rev {
        // An empty `--rev=` is falsy in PHP, so it falls back to the
        // default base like no `--rev` at all.
        Some(rev) if !rev.is_empty() => resolve_rev_base(dir, rev).map(Some),
        _ => Ok(resolve_commit(dir, "HEAD")),
    }
}

/// Diff files minus the added-at-base set, i.e. the paths cover acts on
/// (`FLAG_ADDED` is in the omit mask).
fn prune_added(dir: &Path, base: &str, files: Vec<FileChanges>) -> Vec<FileChanges> {
    files
        .into_iter()
        .filter(|file| exists_at_base(dir, base, &file.path))
        .collect()
}

/// Explicit file pathspecs for the diff. Explicit directories are dropped:
/// PHP's `is_dir` skip means they contribute nothing.
fn explicit_file_spec(dir: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !dir.join(path).is_dir())
        .cloned()
        .collect()
}

fn collect_cover_changes(
    dir: &Path,
    base: Option<&str>,
    options: &Options,
) -> Result<Vec<FileChanges>, String> {
    let Some(base) = base else {
        return Ok(Vec::new());
    };
    if !options.paths.is_empty() {
        // Explicit directories contribute nothing (PHP's `is_dir` skip):
        // when no file paths remain there is nothing to diff for.
        let spec = explicit_file_spec(dir, &options.paths);
        if spec.is_empty() {
            return Ok(Vec::new());
        }
        let tracked = diff_changes(dir, base, &spec)?;
        return Ok(prune_added(dir, base, tracked));
    }
    let tracked = diff_changes(dir, base, &[])?;
    Ok(prune_added(dir, base, tracked))
}

/// Cover changed lines in `dir`, returning the exact stdout text.
pub fn execute_cover_text(dir: &Path, options: &Options) -> Result<String, String> {
    execute_cover_text_ansi(dir, options, terminal::ansi_enabled())
}

/// Cover changed lines in `dir` with an explicit ANSI setting
/// (deterministic under test).
pub fn execute_cover_text_ansi(
    dir: &Path,
    options: &Options,
    ansi: bool,
) -> Result<String, String> {
    check_working_copy(dir)?;
    // Explicit paths are validated before the revision resolves: PHP's
    // `parseBaseCommitArgument()` only stores the rev, while
    // `selectPathsForWorkflow()` throws the path error first.
    let paths = check_cover_paths(dir, &options.paths)?;
    let base = resolve_cover_base(dir, options.rev.as_deref())?;
    let top = repo_top(dir);
    let normalized = Options {
        rev: options.rev.clone(),
        paths,
    };
    let files = collect_cover_changes(dir, base.as_deref(), &normalized)?;
    if options.paths.is_empty() && files.is_empty() {
        return Ok(no_changes_message());
    }
    let covers = blame_cover_lines(&top, base.as_deref(), &files);
    if covers.is_empty() {
        return Ok(untouched_message());
    }
    Ok(render_covers_ansi(&covers, ansi))
}

/// Cover changed lines in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(options) => options,
    };
    match execute_cover_text(dir, &options) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
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
