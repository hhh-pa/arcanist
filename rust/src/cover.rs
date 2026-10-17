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
//! `--rev` the base follows PHP's default chain (`base` rules, project
//! default, upstream, scratch file, prompt).
//!
//! Divergences from PHP, all deliberate:
//!
//! - Zero/one-commit repos keep `HEAD` (or nothing) instead of PHP's
//!   empty-tree base, which PHP then fatals on (T1153 COVER-C10 keep).
//! - Blame failures are skipped silently (unattributed) instead of fatal:
//!   PHP crashes on explicitly passed added files because they cannot be
//!   blamed at the base commit.
//! - Old-side line N attributes the Nth blame entry (1-based). PHP looks
//!   the 1-based line up in a 0-indexed blame list, so it attributes the
//!   next line's author; that off-by-one is not reproduced.
//! - Subversion and Mercurial working copies are not yet ported (T1128).
//! - `CommandException` bytes (failed `cat-file`/`merge-base`/`log` calls
//!   on the base chain) render as plain `git <verb> failed:` messages.
//! - Multi-merge-base output resolves to the first base (PHP feeds the
//!   whole blob to later commands, which always fails).
//!
//! Console output goes through the shared [`arc_rs::terminal`] API:
//! `phutil_console_format()` sites (interpret pattern, substitute raw)
//! use `tsprintf_ansi` with the `%R` (raw string) conversion, `pht()`
//! sites use `pht` (including the real `en_US` plural table), and
//! markup-free `sprintf` sites use plain `format!`.

use std::collections::VecDeque;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::terminal::{PhtArg, TVal};
use arc_rs::{config, diff_local, get_config, git, prompt, terminal};

/// Subcommand name, matching `arc cover`.
pub const NAME: &str = "cover";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `arc_rs::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistCoverWorkflow.php:12-46`).
const HELP_SYNOPSIS: &str = "      **cover** [--rev __revision__] [__path__ ...]";
const HELP_TEXT: &str = "          Supports: svn, git, hg\n          Cover your... professional reputation. Show blame for the lines you\n          changed in your working copy (svn) or since some commit (hg, git).\n          This will take a minute because blame takes a minute, especially under\n          SVN.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[(
    "rev",
    None,
    Some("revision"),
    "Cover changes since a specific revision.",
)];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<arc_rs::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    arc_rs::help::usage_text(
        NAME,
        None,
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        false,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(arc_rs::terminal::ansi_enabled())
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
///
/// Design notes for the svn/hg port (T1153 COVER-C13, Rule 3 —
/// blocked on the T1128 git-only limit, refusals locked by test):
/// PHP covers all three systems (`ArcanistCoverWorkflow.php:35-40`
/// at ceb) through per-VCS `getBlame()`. Subversion shells `svn
/// blame <path>` against the worktree (no base) and parses
/// `/^\s*(\d+)\s+(\S+)/` into `(author, revision)` pairs, with an
/// empty blame for empty files and a `Bad blame? ...` throw on
/// unparseable lines (`ArcanistSubversionAPI.php:561-585`); `--rev`
/// is rejected (`cover does not currently support --rev in svn`).
/// Mercurial shells `hg annotate -u -v -c --rev <base> -- <path>`
/// at the resolved base and parses
/// `/^\s*([^:]+?) ([a-f0-9]{12}):/`, throwing `Unable to parse
/// Mercurial blame line: ...` on mismatch
/// (`ArcanistMercurialAPI.php:275-304`). Both pair lists feed the
/// same attribute/render pipeline (no boundary flag exists outside
/// git, so every blamed line attributes).
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

fn resolve_cover_base(
    dir: &Path,
    top: &Path,
    rev: Option<&str>,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<Option<String>, String> {
    match rev {
        // An empty `--rev=` is falsy in PHP, so it falls back to the
        // default base like no `--rev` at all.
        Some(rev) if !rev.is_empty() => resolve_rev_base(dir, rev).map(Some),
        _ => default_base_commit(dir, top, input, stdin_tty, user_file, opts),
    }
}

/// Default base without `--rev` (`buildBaseCommit(null)`, T1153 COVER-C1):
/// `base` rules, then the merge-base of the default relative commit
/// (project setting, upstream, scratch file, prompt) and HEAD.
/// Zero/one-commit repos keep HEAD (T1153 COVER-C10), never the empty tree.
fn default_base_commit(
    dir: &Path,
    top: &Path,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<Option<String>, String> {
    if !git::ok(dir, &["rev-parse", "--verify", "HEAD^"]) {
        return Ok(resolve_commit(dir, "HEAD"));
    }
    let merged = get_config::load_merged(dir, user_file, opts)?;
    if let Some(base) = base_config_commit(dir, top, &merged, input, stdin_tty)? {
        return Ok(Some(base));
    }
    let (relative, from_prompt) = default_relative_commit(dir, &merged, input, stdin_tty)?;
    verify_relative_commit(dir, &relative)?;
    if from_prompt {
        write_scratch_default(dir, &relative);
    }
    merge_base_head(dir, &relative)
}

/// `base` configuration rules (`buildBaseCommit` + `resolveBaseCommit`,
/// T1153 COVER-C1): when the winning `base` value is set, the rule engine
/// resolves it; no match is a usage error. `None` continues the chain.
fn base_config_commit(
    dir: &Path,
    top: &Path,
    merged: &get_config::MergedConfig,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<Option<String>, String> {
    let specs = base_rule_specs(merged)?;
    let winner = specs.iter().find_map(|(_, spec)| spec.as_deref());
    if winner.is_none_or(str::is_empty) {
        return Ok(None);
    }
    let have: Vec<(&str, &str)> = specs
        .iter()
        .map(|(source, spec)| (*source, spec.as_deref().unwrap_or("")))
        .collect();
    match resolve_base_rules(dir, top, &have, input, stdin_tty)? {
        Some(commit) => Ok(Some(commit)),
        None => Err(terminal::pht(
            "None of the rules in your 'base' configuration matched a valid commit. Adjust \
             rules or specify which commit you want to use explicitly.",
            &[],
        )),
    }
}

/// Per-source `base` specs in precedence order (T1153 COVER-C1): every present
/// value is type-checked like `getConfigFromAllSources()`, and explicit
/// nulls read unset like PHP `idx()`.
fn base_rule_specs(
    merged: &get_config::MergedConfig,
) -> Result<Vec<(&'static str, Option<String>)>, String> {
    let mut specs = Vec::new();
    let runtime = merged.runtime.get("base").filter(|v| !v.is_null());
    specs.push((get_config::SOURCE_RUNTIME, base_spec_value(runtime)?));
    for source in [
        get_config::SOURCE_LOCAL,
        get_config::SOURCE_PROJECT,
        get_config::SOURCE_USER,
        get_config::SOURCE_SYSTEM,
    ] {
        let value = get_config::value_at("base", source, &merged.sources).filter(|v| !v.is_null());
        specs.push((source, base_spec_value(value.as_ref())?));
    }
    Ok(specs)
}

/// One validated `base` source value: `None` when unset, else the string
/// (mistyped values error like `willReadValue()`).
fn base_spec_value(value: Option<&serde_json::Value>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let checked = get_config::check_value("base", value)?;
    Ok(checked.as_str().map(str::to_string))
}

/// First default relative commit: project `git.default-relative-commit`,
/// `@{upstream}`, the scratch file, else the interactive prompt (T1153 COVER-C1).
/// The flag reports prompt answers (which persist the scratch file).
fn default_relative_commit(
    dir: &Path,
    merged: &get_config::MergedConfig,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<(String, bool), String> {
    if let Some(name) = project_default_relative(merged) {
        return Ok((name, false));
    }
    if let Some(name) = git_upstream(dir) {
        return Ok((name, false));
    }
    if let Some(name) = read_scratch_default(dir) {
        return Ok((name, false));
    }
    prompt_default_relative(input, stdin_tty).map(|name| (name, true))
}

/// Project `.arcconfig` `git.default-relative-commit` (T1153 COVER-C1): raw
/// project source only (unvalidated scalars render like PHP string use).
fn project_default_relative(merged: &get_config::MergedConfig) -> Option<String> {
    let value = get_config::value_at(
        "git.default-relative-commit",
        get_config::SOURCE_PROJECT,
        &merged.sources,
    )?;
    let name = match &value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(true) => "1".to_string(),
        _ => return None,
    };
    if name.is_empty() { None } else { Some(name) }
}

/// Current branch's upstream short name, if one is configured (T1153 COVER-C1).
fn git_upstream(dir: &Path) -> Option<String> {
    let upstream = git::stdout_text(
        dir,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()?;
    let name = upstream.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Git metadata directory for scratch files (`rev-parse --git-dir`,
/// resolved against `dir` since git may print it relative).
fn git_metadata_dir(dir: &Path) -> Option<PathBuf> {
    let git_dir = git::stdout_text(dir, &["rev-parse", "--git-dir"]).ok()?;
    Some(dir.join(git_dir.trim()))
}

/// Read the `default-relative-commit` scratch file (T1153 COVER-C1): missing or
/// unreadable files read empty like `readScratchFile()`.
fn read_scratch_default(dir: &Path) -> Option<String> {
    let path = git_metadata_dir(dir)?.join("arc/default-relative-commit");
    let name = std::fs::read_to_string(path).ok()?;
    let name = name.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Best-effort scratch write (T1153 COVER-C1): PHP ignores
/// `writeScratchFile` failures, and the legacy `.arc` migration is not
/// reproduced.
fn write_scratch_default(dir: &Path, relative: &str) {
    let Some(arc_dir) = git_metadata_dir(dir).map(|meta| meta.join("arc")) else {
        return;
    };
    if std::fs::create_dir_all(&arc_dir).is_err() {
        return;
    }
    std::fs::write(arc_dir.join("default-relative-commit"), relative).ok();
}

/// The default range essay, with PHP's substitutions applied.
const DEFAULT_RANGE_ESSAY: &str = "You're running a command which operates on a range of \
     revisions (usually, from some revision to HEAD) but have not specified the revision that \
     should determine the start of the range.\n\nPreviously, arc assumed you meant 'HEAD^' \
     when you did not specify a start revision, but this behavior does not make much sense in \
     most workflows outside of Facebook's historic git-svn workflow.\n\narc no longer assumes \
     'HEAD^'. You must specify a relative commit explicitly when you invoke a command (e.g., \
     `arc diff HEAD^`, not just `arc diff`) or select a default for this working copy.\n\nIn \
     most cases, the best default is 'origin/master'. You can also select 'HEAD^' to preserve \
     the old behavior, or some other remote or branch. But you almost certainly want to select \
     'origin/master'.\n\n(Technically: the merge-base of the selected revision and HEAD is \
     used to determine the start of the commit range.)";

/// Banner plus wrapped essay printed before the default-commit prompt
/// (T1153 COVER-C1; pure form for tests).
fn default_range_banner(ansi: bool) -> String {
    let mut out = terminal::console_format(
        "<bg:green>** %s **</bg>\n\n",
        &["Select a Default Commit Range"],
        ansi,
    );
    out.push_str(&terminal::console_wrap(DEFAULT_RANGE_ESSAY, 0, true));
    out
}

/// PHP `trim()` charlist (`" \t\n\r\0\x0B"`, no form feed).
fn trim_answer(text: &str) -> &str {
    text.trim_matches([' ', '\t', '\n', '\r', '\0', '\u{b}'])
}

/// The interactive default-commit prompt (T1153 COVER-C1): an empty answer
/// keeps `origin/master`.
fn prompt_default_relative(input: &mut dyn BufRead, stdin_tty: bool) -> Result<String, String> {
    print!("{}", default_range_banner(terminal::ansi_enabled()));
    let answer = prompt::console_prompt_with_tty(
        input,
        "What default do you want to use? [origin/master]",
        stdin_tty,
    )?;
    if trim_answer(&answer).is_empty() {
        return Ok("origin/master".to_string());
    }
    Ok(trim_answer(&answer).to_string())
}

/// `cat-file -t` must report `commit` (T1153 COVER-C1): unknown names fail the
/// git call itself (plain message; `CommandException` bytes are
/// unportable), while non-commits get the PHP text.
fn verify_relative_commit(dir: &Path, relative: &str) -> Result<(), String> {
    match git::stdout_text(dir, &["cat-file", "-t", relative]) {
        Ok(kind) if kind.trim() == "commit" => Ok(()),
        Ok(_) => Err(terminal::pht(
            "Relative commit '%s' is not the name of a commit!",
            &[PhtArg::Text(relative)],
        )),
        Err(detail) => Err(format!("git cat-file failed: {detail}")),
    }
}

/// Merge-base of a relative commit and HEAD, first base wins (T1153 COVER-C1).
fn merge_base_head(dir: &Path, relative: &str) -> Result<Option<String>, String> {
    let out = git::stdout_text(dir, &["merge-base", relative, "HEAD"])
        .map_err(|detail| format!("git merge-base failed: {detail}"))?;
    Ok(first_output_line(&out))
}

/// First non-empty output line, if any.
fn first_output_line(out: &str) -> Option<String> {
    out.lines()
        .next()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

/// Outcome of one base-commit rule (T1153 COVER-C1): a resolved commit, no
/// match (try the next rule), or a ruleset stop (try the next source).
enum RuleOutcome {
    Matched(String),
    NoMatch,
    StopRuleset,
}

/// `base` rule engine state (`ArcanistBaseCommitParser`, T1153 COVER-C1).
struct BaseRuleEngine<'a> {
    dir: &'a Path,
    top: &'a Path,
    input: &'a mut dyn BufRead,
    stdin_tty: bool,
    verbose: bool,
}

impl BaseRuleEngine<'_> {
    /// Verbose log line on stderr, like `ArcanistBaseCommitParser::log()`.
    fn log(&self, message: &str) {
        if self.verbose {
            eprintln!("{message}");
        }
    }

    /// Prompt for `arc:prompt`: an empty answer moves to the next rule.
    /// (The base-commit explanation is skipped: cover never prints it.)
    fn prompt_base_commit(&mut self) -> Result<Option<String>, String> {
        let answer =
            prompt::console_prompt_with_tty(self.input, "Against which commit?", self.stdin_tty)?;
        if trim_answer(&answer).is_empty() {
            return Ok(None);
        }
        Ok(Some(answer))
    }
}

/// Split one source's `base` spec on commas
/// (`tokenizeBaseCommitSpecification`, T1153 COVER-C1): falsy rules (`""`,
/// `"0"`) drop like PHP `array_filter`, colon-less rules error.
fn tokenize_base_spec(spec: &str) -> Result<Vec<String>, String> {
    if spec.is_empty() || spec == "0" {
        return Ok(Vec::new());
    }
    let mut rules = Vec::new();
    for part in spec.split(',') {
        let rule = part.trim_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r']);
        if rule.is_empty() || rule == "0" {
            continue;
        }
        if !rule.contains(':') {
            return Err(terminal::pht(
                "Rule '%s' is invalid, it must have a type and name like 'arc:upstream'.",
                &[PhtArg::Text(rule)],
            ));
        }
        rules.push(rule.to_string());
    }
    Ok(rules)
}

/// Resolve `base` rules from every source in precedence order
/// (`ArcanistBaseCommitParser::resolveBaseCommit`, T1153 COVER-C1).
fn resolve_base_rules(
    dir: &Path,
    top: &Path,
    specs: &[(&str, &str)],
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<Option<String>, String> {
    // PHP tokenizes every source upfront, so a malformed rule errors
    // even below a resolving source.
    let mut queues: Vec<(&str, VecDeque<String>)> = Vec::new();
    for (source, spec) in specs {
        queues.push((source, tokenize_base_spec(spec)?.into()));
    }
    let mut engine = BaseRuleEngine {
        dir,
        top,
        input,
        stdin_tty,
        verbose: false,
    };
    let mut trying: VecDeque<&str> = queues.iter().map(|(source, _)| *source).collect();
    while let Some(source) = trying.front().copied() {
        if source_queue_empty(&queues, source) {
            engine.log(&format!("No rules left from source '{source}'."));
            trying.pop_front();
            continue;
        }
        engine.log(&format!("Trying rules from source '{source}'."));
        if let Some(commit) = drain_source_queue(&mut engine, &mut queues, source, &mut trying)? {
            return Ok(Some(commit));
        }
    }
    Ok(None)
}

/// True when `source` has no rules left to try.
fn source_queue_empty(queues: &[(&str, VecDeque<String>)], source: &str) -> bool {
    queues
        .iter()
        .find(|(name, _)| *name == source)
        .is_none_or(|(_, queue)| queue.is_empty())
}

/// Try one source's rules in order: the first match resolves the base,
/// a ruleset stop (or exhaustion) returns to the source loop.
fn drain_source_queue(
    engine: &mut BaseRuleEngine,
    queues: &mut [(&str, VecDeque<String>)],
    source: &str,
    trying: &mut VecDeque<&str>,
) -> Result<Option<String>, String> {
    loop {
        let rule = queues
            .iter_mut()
            .find(|(name, _)| *name == source)
            .and_then(|(_, queue)| queue.pop_front());
        let Some(rule) = rule else {
            return Ok(None);
        };
        engine.log(&format!("Trying rule '{rule}'."));
        match resolve_base_rule(engine, &rule, source, trying)? {
            RuleOutcome::Matched(commit) => {
                engine.log(&format!("Resolved commit '{commit}' from rule '{rule}'."));
                return Ok(Some(commit));
            }
            RuleOutcome::NoMatch => {}
            RuleOutcome::StopRuleset => return Ok(None),
        }
    }
}

/// Resolve one rule (`resolveRule`, T1153 COVER-C1): `literal:` answers
/// verbatim, `git:` runs the git API, `hg:` never matches git, `arc:`
/// runs the arc rules, anything else errors.
fn resolve_base_rule(
    engine: &mut BaseRuleEngine,
    rule: &str,
    source: &str,
    trying: &mut VecDeque<&str>,
) -> Result<RuleOutcome, String> {
    let Some((kind, name)) = rule.split_once(':') else {
        return Err(invalid_base_rule(rule));
    };
    match kind {
        "literal" => Ok(RuleOutcome::Matched(name.to_string())),
        "git" => {
            Ok(git_api_rule(engine, "git", name)?
                .map_or(RuleOutcome::NoMatch, RuleOutcome::Matched))
        }
        "hg" => Ok(RuleOutcome::NoMatch),
        "arc" => arc_rule(engine, name, rule, source, trying),
        _ => Err(unrecognized_base_rule(rule, source)),
    }
}

/// `Rule '%s' is invalid` usage text (unreachable: the tokenizer rejects
/// colon-less rules first).
fn invalid_base_rule(rule: &str) -> String {
    terminal::pht(
        "Rule '%s' is invalid, it must have a type and name like 'arc:upstream'.",
        &[PhtArg::Text(rule)],
    )
}

/// `Base commit rule ... is not a recognized rule` usage text.
fn unrecognized_base_rule(rule: &str, source: &str) -> String {
    terminal::pht(
        "Base commit rule '%s' (from source '%s') is not a recognized rule.",
        &[PhtArg::Text(rule), PhtArg::Text(source)],
    )
}

/// `arc:` rules (`resolveArcRule`, T1153 COVER-C1).
fn arc_rule(
    engine: &mut BaseRuleEngine,
    name: &str,
    rule: &str,
    source: &str,
    trying: &mut VecDeque<&str>,
) -> Result<RuleOutcome, String> {
    let name = update_legacy_rule_name(engine, name);
    match name {
        "verbose" => {
            engine.verbose = true;
            engine.log("Enabled verbose mode.");
            Ok(RuleOutcome::NoMatch)
        }
        "prompt" => Ok(engine
            .prompt_base_commit()?
            .map_or(RuleOutcome::NoMatch, RuleOutcome::Matched)),
        "local" | "user" | "project" | "runtime" | "system" => {
            let target: &'static str = static_source_name(name).expect("matched source name");
            trying.push_front(target);
            engine.log(&format!("Switching to source '{target}'."));
            Ok(RuleOutcome::StopRuleset)
        }
        "yield" => {
            if let Some(head) = trying.pop_front() {
                trying.push_back(head);
            }
            engine.log(&format!("Yielding processing of rules from '{source}'."));
            Ok(RuleOutcome::StopRuleset)
        }
        "halt" => {
            trying.clear();
            engine.log("Halting all rule processing.");
            Ok(RuleOutcome::StopRuleset)
        }
        "skip" => Ok(RuleOutcome::NoMatch),
        "empty" | "upstream" | "outgoing" | "bookmark" | "amended" | "this" => {
            Ok(git_api_rule(engine, "arc", name)?
                .map_or(RuleOutcome::NoMatch, RuleOutcome::Matched))
        }
        _ => arc_call_rule(engine, name, rule, source),
    }
}

/// Static source name for `arc:` source-switch rules.
fn static_source_name(name: &str) -> Option<&'static str> {
    match name {
        "local" => Some("local"),
        "user" => Some("user"),
        "project" => Some("project"),
        "runtime" => Some("runtime"),
        "system" => Some("system"),
        _ => None,
    }
}

/// Translate legacy `arc:` source names (`updateLegacyRuleName`).
fn update_legacy_rule_name<'a>(engine: &BaseRuleEngine, name: &'a str) -> &'a str {
    let translated = match name {
        "global" => Some("user"),
        "args" => Some("runtime"),
        _ => None,
    };
    if let Some(new) = translated {
        engine.log(&format!("Translating legacy name '{name}' to '{new}'"));
        new
    } else {
        name
    }
}

/// `arc:exec(...)` / `arc:nodiff(...)` forms, else the unrecognized-rule
/// error (T1153 COVER-C1): a failing command moves to the next rule.
fn arc_call_rule(
    engine: &BaseRuleEngine,
    name: &str,
    rule: &str,
    source: &str,
) -> Result<RuleOutcome, String> {
    if let Some(command) = paren_form(name, "exec") {
        return Ok(exec_rule_command(engine.top, command)
            .map_or(RuleOutcome::NoMatch, RuleOutcome::Matched));
    }
    if paren_form(name, "nodiff").is_some() {
        return Ok(RuleOutcome::NoMatch);
    }
    Err(unrecognized_base_rule(rule, source))
}

/// Inner text of a `func(...)` form, greedy like PHP `(.*)`.
fn paren_form<'a>(name: &'a str, func: &str) -> Option<&'a str> {
    let prefix = format!("{func}(");
    name.strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(')'))
}

/// `func(...)` with a non-empty body (PHP `(.+)`).
fn nonempty_paren_form<'a>(name: &'a str, func: &str) -> Option<&'a str> {
    paren_form(name, func).filter(|inner| !inner.is_empty())
}

/// `arc:exec(command)` in the project root (T1153 COVER-C1).
fn exec_rule_command(top: &Path, command: &str) -> Option<String> {
    let output = shell_command(top, command).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Platform shell command with `top` as the working directory.
fn shell_command(top: &Path, command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = crate::proc_env::command("cmd");
        cmd.args(["/C"]);
        <std::process::Command as std::os::windows::process::CommandExt>::raw_arg(
            &mut cmd, command,
        );
        cmd.current_dir(top);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = crate::proc_env::command("sh");
        cmd.args(["-c", command]);
        cmd.current_dir(top);
        cmd
    }
}

/// The git API's rules (`resolveBaseCommitRule`, T1153 COVER-C1): `git:`
/// `merge-base()` / `branch-unique()` / plain names, and the `arc:` API
/// names (`empty`, `amended`, `upstream`, `this`; the rest never match).
fn git_api_rule(engine: &BaseRuleEngine, kind: &str, name: &str) -> Result<Option<String>, String> {
    if kind == "git" {
        return git_name_rule(engine, name);
    }
    match name {
        "empty" => Ok(Some(GIT_MAGIC_ROOT_COMMIT.to_string())),
        "amended" => head_amended(engine.dir),
        "upstream" => upstream_rule_base(engine.dir),
        "this" => Ok(Some("HEAD^".to_string())),
        _ => Ok(None),
    }
}

/// `git:` names: `merge-base()` / `branch-unique()` forms, else a plain
/// name verified with `cat-file` (returned symbolically, like PHP).
fn git_name_rule(engine: &BaseRuleEngine, name: &str) -> Result<Option<String>, String> {
    if let Some(inner) = nonempty_paren_form(name, "merge-base") {
        let out = git::stdout_text(engine.dir, &["merge-base", inner, "HEAD"]).ok();
        return Ok(out.and_then(|text| first_output_line(&text)));
    }
    if let Some(inner) = nonempty_paren_form(name, "branch-unique") {
        return branch_unique_base(engine.dir, inner);
    }
    if git::ok(engine.dir, &["cat-file", "-t", name]) {
        return Ok(Some(name.to_string()));
    }
    Ok(None)
}

/// `arc:amended`: `HEAD^` when HEAD carries a revision ID, else no match
/// (an invalid field errors like PHP's throw).
fn head_amended(dir: &Path) -> Result<Option<String>, String> {
    let message = git::stdout_text(dir, &["log", "-n1", "--format=%B", "HEAD"])
        .map_err(|detail| format!("git log failed: {detail}"))?;
    let revision = diff_local::parse_revision_id_from_raw_corpus(&message)?;
    Ok(revision.map(|_| "HEAD^".to_string()))
}

/// `arc:upstream`: the merge-base of the branch upstream and HEAD, or no
/// match without an upstream (a failing merge-base aborts like PHP's
/// `execxLocal` throw).
fn upstream_rule_base(dir: &Path) -> Result<Option<String>, String> {
    let upstream = git::stdout_text(
        dir,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    let Ok(upstream) = upstream else {
        return Ok(None);
    };
    let upstream = upstream.trim().to_string();
    let out = git::stdout_text(dir, &["merge-base", &upstream, "HEAD"])
        .map_err(|detail| format!("git merge-base failed: {detail}"))?;
    Ok(first_output_line(&out))
}

/// `git:branch-unique(ref)`: the first commit after the merge-base that
/// sits on more branches than HEAD does (T1153 COVER-C1).
fn branch_unique_base(dir: &Path, upstream_ref: &str) -> Result<Option<String>, String> {
    let out = git::stdout_text(dir, &["merge-base", upstream_ref, "HEAD"]).ok();
    let Some(merge_base) = out.and_then(|text| first_output_line(&text)) else {
        return Ok(None);
    };
    let range = format!("{merge_base}..HEAD");
    let log = git::stdout_text(dir, &["log", "--format=%H", &range, "--"])
        .map_err(|detail| format!("git log failed: {detail}"))?;
    let mut commits: Vec<String> = log.lines().map(str::to_string).collect();
    if commits.is_empty() {
        return Ok(None);
    }
    commits.push(merge_base);
    let branches = local_branch_names(dir)?;
    let mut head_count: Option<usize> = None;
    for commit in commits {
        let count = branches_containing(dir, &commit, &branches)?.len();
        match head_count {
            None => head_count = Some(count),
            Some(first) if count > first => return Ok(Some(commit)),
            _ => {}
        }
    }
    Ok(None)
}

/// Local branch short names (`getAllBranches()` names, T1153 COVER-C1).
fn local_branch_names(dir: &Path) -> Result<Vec<String>, String> {
    let out = git::stdout_text(
        dir,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .map_err(|detail| format!("git for-each-ref failed: {detail}"))?;
    Ok(out.lines().map(str::to_string).collect())
}

/// Branches containing `commit`, filtered to known branch names like PHP
/// (`branch --contains`, trimmed of spaces and `*` markers).
fn branches_containing(
    dir: &Path,
    commit: &str,
    branches: &[String],
) -> Result<Vec<String>, String> {
    let out = git::stdout_text(
        dir,
        &["branch", "--no-color", "--no-column", "--contains", commit],
    )
    .map_err(|detail| format!("git branch failed: {detail}"))?;
    Ok(out
        .lines()
        .map(|line| line.trim_matches([' ', '*']).to_string())
        .filter(|name| branches.contains(name))
        .collect())
}

/// Diff files minus the added-at-base set, i.e. the paths cover acts on
/// (`FLAG_ADDED` is in the omit mask).
fn prune_added(dir: &Path, base: &str, files: Vec<FileChanges>) -> Vec<FileChanges> {
    files
        .into_iter()
        .filter(|file| exists_at_base(dir, base, &file.path))
        .collect()
}

/// True when `path` (as given) resolves inside the repository: paths
/// outside the root can never match diff output, so they contribute
/// nothing like PHP's hunk-map lookup miss (T1153 COVER-C11).
fn path_within_top(dir: &Path, top: &Path, path: &str) -> bool {
    let (Ok(full), Ok(root)) = (dir.join(path).canonicalize(), top.canonicalize()) else {
        return false;
    };
    full.starts_with(&root)
}

/// Explicit file pathspecs for the diff. Explicit directories are dropped
/// (PHP's `is_dir` skip means they contribute nothing), as are paths
/// outside the repository.
fn explicit_file_spec(dir: &Path, top: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !dir.join(path).is_dir() && path_within_top(dir, top, path))
        .cloned()
        .collect()
}

fn collect_cover_changes(
    dir: &Path,
    top: &Path,
    base: Option<&str>,
    options: &Options,
) -> Result<Vec<FileChanges>, String> {
    let Some(base) = base else {
        return Ok(Vec::new());
    };
    if !options.paths.is_empty() {
        // Explicit directories contribute nothing (PHP's `is_dir` skip):
        // when no file paths remain there is nothing to diff for.
        let spec = explicit_file_spec(dir, top, &options.paths);
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
    let user_file = config::arcrc_path();
    execute_cover_text_ansi_with_input_and_config(
        dir,
        options,
        ansi,
        &mut std::io::stdin().lock(),
        prompt::stdin_is_terminal(),
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
}

/// [`execute_cover_text_ansi`] with injectable prompt input (T1153 COVER-C1).
pub fn execute_cover_text_ansi_with_input(
    dir: &Path,
    options: &Options,
    ansi: bool,
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<String, String> {
    let user_file = config::arcrc_path();
    execute_cover_text_ansi_with_input_and_config(
        dir,
        options,
        ansi,
        input,
        stdin_tty,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
}

/// [`execute_cover_text_ansi`] with injectable prompt input and config
/// sources (T1153 COVER-C1; tests pass `user_file: None` for isolation).
pub fn execute_cover_text_ansi_with_input_and_config(
    dir: &Path,
    options: &Options,
    ansi: bool,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<String, String> {
    check_working_copy(dir)?;
    // Explicit paths are validated before the revision resolves: PHP's
    // `parseBaseCommitArgument()` only stores the rev, while
    // `selectPathsForWorkflow()` throws the path error first.
    let paths = check_cover_paths(dir, &options.paths)?;
    let top = repo_top(dir);
    let base = resolve_cover_base(
        dir,
        &top,
        options.rev.as_deref(),
        input,
        stdin_tty,
        user_file,
        opts,
    )?;
    let normalized = Options {
        rev: options.rev.clone(),
        paths,
    };
    let files = collect_cover_changes(dir, &top, base.as_deref(), &normalized)?;
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
    run_in_with_input_and_tty(
        dir,
        argv,
        &mut std::io::stdin().lock(),
        prompt::stdin_is_terminal(),
    )
}

/// [`run_in`] with injectable prompt input (T1153 COVER-C1).
pub fn run_in_with_input_and_tty(
    dir: &Path,
    argv: &[String],
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> ExitCode {
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
    match execute_cover_text_ansi_with_input(
        dir,
        &options,
        terminal::ansi_enabled(),
        input,
        stdin_tty,
    ) {
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
