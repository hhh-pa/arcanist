//! Mercurial (`hg`) working-copy API.
//!
//! PHP reference: `src/repository/api/ArcanistMercurialAPI.php`,
//! `src/repository/parser/ArcanistMercurialParser.php`, and the needed
//! `ArcanistRepositoryAPI` base parts
//! (`src/repository/api/ArcanistRepositoryAPI.php`).
//!
//! API shape mirrors `git.rs`: stateless free functions taking the working
//! copy root first, `Result<_, String>` errors, and pure `parse_*` seams
//! for every output shape. Functions that spawn `hg` come in two forms:
//! `name(dir, ...)` uses the system binary, `name_with(dir, hg, ...)`
//! takes an [`HgCommand`] backend so tests can point at a fake executable.
//!
//! Divergences from PHP (behavior limits, all deliberate):
//!
//! - Stateless: no memo caches for branch, commit info, or raw diffs
//!   (`ArcanistMercurialAPI.php:15-17,71-76,191,361-380`); every call
//!   shells out, so `didReloadWorkingCopy` / `didReloadCommitRange`
//!   (`:78-80,346-351`) have no equivalent.
//! - Synchronous: no futures (`ArcanistRepositoryAPI.php:397-410`,
//!   `ArcanistMercurialAPI.php:1041-1048,1173-1217`); feature probes and
//!   parent lookups run inline with identical results.
//! - Arguments pass as argv, never through a shell, so `hgsprintf` / `%`
//!   pattern quoting has no equivalent; the escape-then-raw retry in
//!   `resolveBaseCommitRule` (`:820-828`) collapses to one attempt.
//! - File data is `Vec<u8>`, matching PHP byte strings.
//! - Dates parse to `Option<i64>`; unparseable dates yield `None` where
//!   PHP `strtotime` yields false.
//! - Base-commit rules needing the configuration manager
//!   (`ArcanistMercurialAPI.php:107-118`,
//!   `ArcanistRepositoryAPI.php:642-658`) are excluded; explicit and
//!   automatic base commits are ported.
//! - Scratch `.arc` to `.hg/arc` migration
//!   (`ArcanistRepositoryAPI.php:551-562`) is explicit: [`migrate_scratch_dir`]
//!   moves the files on demand instead of once per process, and it leaves
//!   the old directory alone when it holds anything but plain files (PHP
//!   would crash mid-migration there).
//! - Conduit, marker/remote/graph queries, land/work engines, commit refs,
//!   and `getAllLocalChanges` (needs `ArcanistDiffParser`, which has no
//!   wired equivalent) are excluded; see the crate brief for line refs.
//!
//! Status maps use `BTreeMap`, so iteration order matches PHP `ksort`
//! (`ArcanistRepositoryAPI.php:148,236`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Map from repo-relative path to `FLAG_*` bitmask, as returned by
/// `getUncommittedStatus` (`ArcanistRepositoryAPI.php:145-152`).
pub type StatusMap = BTreeMap<String, u32>;

/// System Mercurial binary name.
pub const HG: &str = "hg";

/// `HGPLAIN=1`, forced on every invocation
/// (`ArcanistMercurialAPI.php:1010-1026`).
pub const HGPLAIN: (&str, &str) = ("HGPLAIN", "1");

/// `--encoding utf-8`, prefixed to every invocation
/// (`ArcanistMercurialAPI.php:13`, `ROOT_HG_COMMAND`).
pub const HG_ENCODING_ARGS: &[&str] = &["--encoding", "utf-8"];

/// How to invoke the `hg` binary: the system binary, or a fake executable
/// for hermetic tests (for example `cmd /C fakehg.bat` on Windows, which
/// needs the `prefix` words before the `hg` arguments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgCommand {
    /// Executable to spawn.
    pub program: String,
    /// Fixed leading arguments, empty for the real binary.
    pub prefix: Vec<String>,
}

impl HgCommand {
    /// Invoke the system `hg` binary.
    #[must_use]
    pub fn system() -> Self {
        Self {
            program: HG.to_string(),
            prefix: Vec::new(),
        }
    }

    /// Invoke a custom program with fixed leading arguments (test seam).
    #[must_use]
    pub fn custom(program: &str, prefix: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            prefix: prefix.iter().map(|item| (*item).to_string()).collect(),
        }
    }
}

impl Default for HgCommand {
    fn default() -> Self {
        Self::system()
    }
}

/// Exit code plus captured output, the `execManualLocal` triple
/// (`ArcanistRepositoryAPI.php:402-405`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    /// Process exit code, or -1 when killed by a signal.
    pub code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

fn command_for(dir: &Path, hg: &HgCommand, args: &[&str]) -> std::process::Command {
    let mut cmd = crate::proc_env::command(&hg.program);
    cmd.current_dir(dir);
    cmd.args(&hg.prefix);
    cmd.args(HG_ENCODING_ARGS);
    cmd.args(args);
    cmd.env(HGPLAIN.0, HGPLAIN.1);
    cmd
}

/// Run `hg` in `dir`, returning the raw output. Spawn failures become
/// errors; non-zero exit statuses are left for the caller to interpret.
/// Mirrors `git::output`, plus the `hg` encoding prefix, `HGPLAIN`, and
/// backend override (`ArcanistMercurialAPI.php:22-26,35-43`). Transient
/// ETXTBSY retries come from the shared [`crate::proc_env::spawn_output`].
pub fn output_with(
    dir: &Path,
    hg: &HgCommand,
    args: &[&str],
) -> Result<std::process::Output, String> {
    crate::proc_env::spawn_output(&mut command_for(dir, hg, args))
        .map_err(|e| format!("failed to run {}: {e}", hg.program))
}

/// Run the system `hg` in `dir`, returning the raw output.
pub fn output(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    output_with(dir, &HgCommand::system(), args)
}

/// Run `hg` in `dir`, returning trimmed stdout, or the combined trimmed
/// output when `hg` exits non-zero. Mirrors `git::stdout_text`.
pub fn stdout_text_with(dir: &Path, hg: &HgCommand, args: &[&str]) -> Result<String, String> {
    let out = output_with(dir, hg, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            Err(format!("{} {} failed", hg.program, args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

/// Run the system `hg` in `dir`, returning trimmed stdout.
pub fn stdout_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    stdout_text_with(dir, &HgCommand::system(), args)
}

/// True when `hg` exits zero in `dir` for `args`. Mirrors `git::ok`.
#[must_use]
pub fn ok_with(dir: &Path, hg: &HgCommand, args: &[&str]) -> bool {
    output_with(dir, hg, args).is_ok_and(|out| out.status.success())
}

/// True when the system `hg` exits zero in `dir` for `args`.
#[must_use]
pub fn ok(dir: &Path, args: &[&str]) -> bool {
    ok_with(dir, &HgCommand::system(), args)
}

/// Run `hg` in `dir` without failing on non-zero exit, returning the exit
/// code plus both streams (`execManualLocal`,
/// `ArcanistRepositoryAPI.php:402-405`).
pub fn manual_with(dir: &Path, hg: &HgCommand, args: &[&str]) -> Result<CommandResult, String> {
    let out = output_with(dir, hg, args)?;
    Ok(CommandResult {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

/// Run the system `hg` in `dir` without failing on non-zero exit.
pub fn manual(dir: &Path, args: &[&str]) -> Result<CommandResult, String> {
    manual_with(dir, &HgCommand::system(), args)
}

/// Run `hg` in `dir` with inherited stdio, returning the exit code
/// (`execPassthru`, `ArcanistRepositoryAPI.php:674-682` with the
/// Mercurial `newPassthru` at `ArcanistMercurialAPI.php:28-33`).
pub fn exec_passthru_with(dir: &Path, hg: &HgCommand, args: &[&str]) -> Result<i32, String> {
    command_for(dir, hg, args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map(|status| status.code().unwrap_or(-1))
        .map_err(|e| format!("failed to run {}: {e}", hg.program))
}

/// Run the system `hg` in `dir` with inherited stdio.
pub fn exec_passthru(dir: &Path, args: &[&str]) -> Result<i32, String> {
    exec_passthru_with(dir, &HgCommand::system(), args)
}

/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_MODIFIED: u32 = 1;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_ADDED: u32 = 2;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_DELETED: u32 = 4;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_UNTRACKED: u32 = 8;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_CONFLICT: u32 = 16;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_MISSING: u32 = 32;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_UNSTAGED: u32 = 64;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_UNCOMMITTED: u32 = 128;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_EXTERNALS: u32 = 256;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_OBSTRUCTED: u32 = 512;
/// Path status bitmask flags, verbatim values from
/// `ArcanistRepositoryAPI.php:10-28`.
pub const FLAG_INCOMPLETE: u32 = 1024;

/// `getSourceControlSystemName` (`ArcanistMercurialAPI.php:45-47`).
#[must_use]
pub fn get_source_control_system_name() -> &'static str {
    "hg"
}

/// `getMetadataPath` (`ArcanistMercurialAPI.php:49-51`).
#[must_use]
pub fn get_metadata_path(root: &Path) -> PathBuf {
    root.join(".hg")
}

/// Walk up from `dir` looking for a Mercurial working copy root (the
/// directory holding `.hg`), or `None` outside one.
#[must_use]
pub fn find_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|dir| dir.join(".hg").exists())
        .map(Path::to_path_buf)
}

/// `getSourceControlPath` (`ArcanistMercurialAPI.php:66-68`).
#[must_use]
pub fn get_source_control_path() -> &'static str {
    "/"
}

/// `getWorkingCopyRevision` (`ArcanistMercurialAPI.php:461-463`).
#[must_use]
pub fn get_working_copy_revision() -> &'static str {
    "."
}

/// `newCurrentCommitSymbol` (`ArcanistMercurialAPI.php:457-459`).
#[must_use]
pub fn new_current_commit_symbol() -> &'static str {
    get_working_copy_revision()
}

/// `isHistoryDefaultImmutable` (`ArcanistMercurialAPI.php:465-467`).
#[must_use]
pub fn is_history_default_immutable() -> bool {
    true
}

/// `supportsCommitRanges` (`ArcanistMercurialAPI.php:478-480`).
#[must_use]
pub fn supports_commit_ranges() -> bool {
    true
}

/// `supportsLocalCommits` (`ArcanistMercurialAPI.php:482-484`).
#[must_use]
pub fn supports_local_commits() -> bool {
    true
}

/// `canStashChanges`, inherited from the base
/// (`ArcanistRepositoryAPI.php:414-416`).
#[must_use]
pub fn can_stash_changes() -> bool {
    false
}

/// `stashChanges`, inherited from the base, which throws
/// (`ArcanistRepositoryAPI.php:418-420`).
pub fn stash_changes() -> Result<(), String> {
    Err("stash is not supported by this repository API".to_string())
}

/// `unstashChanges`, inherited from the base, which throws
/// (`ArcanistRepositoryAPI.php:422-424`).
pub fn unstash_changes() -> Result<(), String> {
    Err("stash is not supported by this repository API".to_string())
}

/// `getRepositoryUUID`: the base default null, which Mercurial does not
/// override (`ArcanistRepositoryAPI.php:660-662`).
#[must_use]
pub fn get_repository_uuid() -> Option<String> {
    None
}

/// `getFinalizedRevisionMessage`
/// (`ArcanistMercurialAPI.php:525-530`).
#[must_use]
pub fn get_finalized_revision_message() -> String {
    "You may now push this commit upstream, as appropriate (e.g. with 'hg push' or by printing and faxing it)."
        .to_string()
}

/// `newSupportedMarkerTypes` (`ArcanistMercurialAPI.php:1219-1224`); values
/// are `ArcanistMarkerRef::TYPE_BRANCH` / `TYPE_BOOKMARK`
/// (`src/repository/marker/ArcanistMarkerRef.php:10-11`).
pub const SUPPORTED_MARKER_TYPES: &[&str] = &["branch", "bookmark"];

/// One entry of `parseMercurialStatusDetails`
/// (`ArcanistMercurialParser.php:24-91`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgStatusDetail {
    /// `FLAG_*` bitmask.
    pub flags: u32,
    /// Copy source from `hg status -C` continuation lines, if any.
    pub from: Option<String>,
}

/// `ArcanistMercurialParser::parseMercurialStatusDetails`
/// (`ArcanistMercurialParser.php:24-91`). Clean (`C`) and ignored (`I`)
/// entries are kept with zero flags, exactly like PHP.
pub fn parse_mercurial_status_details(
    stdout: &str,
) -> Result<BTreeMap<String, HgStatusDetail>, String> {
    let mut result = BTreeMap::new();
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(result);
    }
    let mut last_path: Option<String> = None;
    for line in trimmed.split('\n') {
        // Real `hg` emits LF; tolerate CRLF producers (the Windows
        // test fake shells through `cmd`, whose `echo` appends CRLF).
        let line = line.strip_suffix('\r').unwrap_or(line);
        let bytes = line.as_bytes();
        if bytes.len() < 2 || bytes[1] != b' ' {
            return Err(format!("Unparsable Mercurial status line '{line}'."));
        }
        let code = bytes[0];
        let path = line[2..].to_string();
        if code == b' ' {
            apply_status_copy_source(&mut result, &last_path, &path, line)?;
            continue;
        }
        let flags = status_code_flags(code)?;
        last_path = Some(path.clone());
        result.insert(path, HgStatusDetail { flags, from: None });
    }
    Ok(result)
}

/// Record a `hg status -C` copy-source continuation line on the previous
/// entry; a source with no previous entry is an error.
fn apply_status_copy_source(
    result: &mut BTreeMap<String, HgStatusDetail>,
    last_path: &Option<String>,
    path: &str,
    _line: &str,
) -> Result<(), String> {
    match last_path {
        None => unreachable!("trim() strips leading spaces, so a copy-source line is never first"),
        Some(prev) => {
            if let Some(detail) = result.get_mut(prev) {
                detail.from = Some(path.to_string());
            }
            Ok(())
        }
    }
}

/// Flag mask for one Mercurial status code; clean (`C`) and ignored (`I`)
/// entries carry zero flags, like PHP.
fn status_code_flags(code: u8) -> Result<u32, String> {
    match code {
        b'A' => Ok(FLAG_ADDED),
        b'R' => Ok(FLAG_DELETED),
        b'M' => Ok(FLAG_MODIFIED),
        b'C' => Ok(0),
        b'!' => Ok(FLAG_MISSING),
        b'?' => Ok(FLAG_UNTRACKED),
        b'I' => Ok(0),
        _ => Err(format!("Unknown Mercurial status '{}'.", code as char)),
    }
}

/// `ArcanistMercurialParser::parseMercurialStatus`
/// (`ArcanistMercurialParser.php:103-106`).
pub fn parse_mercurial_status(stdout: &str) -> Result<StatusMap, String> {
    let mut result = StatusMap::new();
    for (path, detail) in parse_mercurial_status_details(stdout)? {
        result.insert(path, detail.flags);
    }
    Ok(result)
}

/// One parent line of a default-style log entry
/// (`ArcanistMercurialParser.php:158-167`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HgLogParent {
    /// Local revision number.
    pub local: String,
    /// Full commit hash, absent when the line has no colon.
    pub rev: Option<String>,
}

/// One changeset of `parseMercurialLog`
/// (`ArcanistMercurialParser.php:117-191`). Fields stay `None` when the
/// entry has no such line, matching PHP's sparse dictionaries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HgLogEntry {
    /// `user` line.
    pub user: Option<String>,
    /// Raw `date` line.
    pub date_raw: Option<String>,
    /// Parsed `date` line; `None` where PHP `strtotime` yields false.
    pub date_unix: Option<i64>,
    /// `summary` line.
    pub summary: Option<String>,
    /// Local revision number from the `changeset` line.
    pub local: Option<String>,
    /// Full hash from the `changeset` line.
    pub rev: Option<String>,
    /// `parent` lines, in order.
    pub parents: Vec<HgLogParent>,
    /// `branch` line.
    pub branch: Option<String>,
    /// `tag` line.
    pub tag: Option<String>,
    /// `bookmark` line.
    pub bookmark: Option<String>,
}

fn split_changeset(value: &str) -> (String, Option<String>) {
    match value.split_once(':') {
        Some((local, rev)) => (local.to_string(), Some(rev.to_string())),
        None => (value.to_string(), None),
    }
}

/// `ArcanistMercurialParser::parseMercurialLog`
/// (`ArcanistMercurialParser.php:117-191`). Lines without a colon behave
/// like PHP `explode` returning one part: the whole line becomes the field
/// name and the value is empty. The `evolve` extension's `obsolete` /
/// `instability` fields are skipped (`:177-181`).
pub fn parse_mercurial_log(stdout: &str) -> Result<Vec<HgLogEntry>, String> {
    let mut result = Vec::new();
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(result);
    }
    for chunk in trimmed.split("\n\n") {
        let mut entry = HgLogEntry::default();
        for line in chunk.split('\n') {
            if is_log_noise_line(line) {
                continue;
            }
            let (name, value) = match line.split_once(':') {
                Some((name, value)) => (name, value.trim().to_string()),
                None => (line, String::new()),
            };
            apply_log_field(&mut entry, name, value)?;
        }
        result.push(entry);
    }
    Ok(result)
}

/// Progress chatter `hg log` may interleave with changesets.
fn is_log_noise_line(line: &str) -> bool {
    line.starts_with("comparing with")
        || line.starts_with("searching for changes")
        || line.starts_with("remote:")
}

/// Apply one `name: value` log line to the entry; the `evolve` extension's
/// `obsolete` / `instability` fields are skipped, unknown fields are errors.
fn apply_log_field(entry: &mut HgLogEntry, name: &str, value: String) -> Result<(), String> {
    match name {
        "user" => entry.user = Some(value),
        "date" => {
            entry.date_unix = parse_hg_default_date(&value);
            entry.date_raw = Some(value);
        }
        "summary" => entry.summary = Some(value),
        "changeset" => {
            let (local, rev) = split_changeset(&value);
            entry.local = Some(local);
            entry.rev = rev;
        }
        "parent" => {
            let (local, rev) = split_changeset(&value);
            entry.parents.push(HgLogParent { local, rev });
        }
        "branch" => entry.branch = Some(value),
        "tag" => entry.tag = Some(value),
        "bookmark" => entry.bookmark = Some(value),
        "obsolete" | "instability" => {}
        _ => return Err(format!("Unknown Mercurial log field '{name}'!")),
    }
    Ok(())
}

/// One branch of `parseMercurialBranches`
/// (`ArcanistMercurialParser.php:201-239`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgBranch {
    /// Local revision number.
    pub local: String,
    /// Short hash.
    pub rev: String,
}

fn parse_mercurial_branch_line(line: &str) -> Option<(String, String, String)> {
    if line.chars().next().is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    // Optional " (inactive)" cue; like the PHP pattern this also strips a
    // genuinely inactive-looking branch name.
    let head = line
        .strip_suffix(" (inactive)")
        .or_else(|| line.strip_suffix("\t(inactive)"))
        .unwrap_or(line);
    let (name, tail) = head.rsplit_once([' ', '\t'])?;
    let name = name.trim_end();
    let (local, rev) = tail.split_once(':')?;
    if local.is_empty() || !local.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if rev.is_empty() || !rev.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    Some((name.to_string(), local.to_string(), rev.to_string()))
}

/// `ArcanistMercurialParser::parseMercurialBranches`
/// (`ArcanistMercurialParser.php:201-239`).
pub fn parse_mercurial_branches(stdout: &str) -> Result<BTreeMap<String, HgBranch>, String> {
    let trimmed = stdout.trim_end_matches('\n');
    if trimmed.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut branches = BTreeMap::new();
    for line in trimmed.split('\n') {
        let Some((name, local, rev)) = parse_mercurial_branch_line(line) else {
            return Err(format!("Failed to parse 'hg branches' output: {line}"));
        };
        branches.insert(name, HgBranch { local, rev });
    }
    Ok(branches)
}

fn month_number(name: &str) -> Option<i64> {
    match name {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

fn parse_zone(suffix: &str) -> Option<i64> {
    if suffix.len() != 5 || (!suffix.starts_with('+') && !suffix.starts_with('-')) {
        return None;
    }
    let hours: i64 = suffix[1..3].parse().ok()?;
    let minutes: i64 = suffix[3..5].parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let offset = hours * 3600 + minutes * 60;
    Some(if suffix.starts_with('-') {
        -offset
    } else {
        offset
    })
}

fn parse_clock(text: &str) -> Option<i64> {
    let mut parts = text.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hours > 23 || minutes > 59 || seconds > 60 {
        return None;
    }
    Some(hours * 3600 + minutes * 60 + seconds)
}

/// Parse `{date|rfc822date}` template output such as
/// `Thu, 14 Oct 2026 12:34:56 +0200` (`ArcanistMercurialAPI.php:234` uses
/// PHP `strtotime` on this shape).
#[must_use]
pub fn parse_rfc822_date(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let _weekday = parts.next()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let month = month_number(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let clock = parse_clock(parts.next()?)?;
    let zone = parse_zone(parts.next()?)?;
    if parts.next().is_some() || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 + clock - zone)
}

/// Parse default-style `date` lines such as
/// `Thu Oct 14 12:34:56 2026 +0200`
/// (`ArcanistMercurialParser.php:147-149` uses PHP `strtotime`).
#[must_use]
pub fn parse_hg_default_date(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let _weekday = parts.next()?;
    let month = month_number(parts.next()?)?;
    let day: i64 = parts.next()?.parse().ok()?;
    let clock = parse_clock(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let zone = parse_zone(parts.next()?)?;
    if parts.next().is_some() || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 + clock - zone)
}

fn parse_blame_line(line: &str) -> Option<(String, String)> {
    // `/^\s*([^:]+?) ([a-f0-9]{12}):/` with a lazy author: the first space
    // after a colon-free author that is followed by 12 hex digits and a
    // colon (`ArcanistMercurialAPI.php:296`).
    let rest = line.trim_start();
    for (index, _) in rest.char_indices().filter(|(_, c)| *c == ' ') {
        let author = &rest[..index];
        if author.is_empty() || author.contains(':') {
            continue;
        }
        let after = &rest[index + 1..];
        let bytes = after.as_bytes();
        if bytes.len() >= 13
            && bytes[..12]
                .iter()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            && bytes[12] == b':'
        {
            return Some((author.trim().to_string(), after[..12].to_string()));
        }
    }
    None
}

/// Parse `hg annotate -u -v -c` output into `(author, revision)` pairs
/// (`getBlame`, `ArcanistMercurialAPI.php:281-311`).
pub fn parse_blame(stdout: &str) -> Result<Vec<(String, String)>, String> {
    let mut blame = Vec::new();
    for line in stdout.split_inclusive('\n') {
        let Some(entry) = parse_blame_line(line) else {
            return Err(format!("Unable to parse Mercurial blame line: {line}"));
        };
        blame.push(entry);
    }
    Ok(blame)
}

fn parse_email_address(text: &str) -> (Option<String>, Option<String>) {
    // `PhutilEmailAddress` (`src/parser/PhutilEmailAddress.php:15-41`):
    // `Name <addr>` splits at the last `<` with quote trimming, else the
    // address splits at the last `@`.
    let trimmed = text.trim();
    if let Some(inner) = trimmed.strip_suffix('>')
        && let Some(open) = inner.rfind('<')
    {
        let display = inner[..open].trim_matches(['\'', '"', ' ']);
        let addr = &inner[open + 1..];
        let (local, domain) = match addr.split_once('@') {
            Some((local, domain)) => (local, Some(domain)),
            None => (addr, None),
        };
        let address = match domain {
            Some(domain) if !domain.is_empty() => format!("{local}@{domain}"),
            _ => local.to_string(),
        };
        return (Some(display.to_string()), Some(address));
    }
    match trimmed.rsplit_once('@') {
        Some((local, domain)) => {
            let address = if domain.is_empty() {
                local.to_string()
            } else {
                format!("{local}@{domain}")
            };
            (None, Some(address))
        }
        None => (None, Some(trimmed.to_string())),
    }
}

/// `parseFullAuthor` (`ArcanistMercurialAPI.php:645-656`): names without
/// `@` pass through with no email, otherwise the display name and address
/// split like `PhutilEmailAddress`.
#[must_use]
pub fn parse_full_author(full_author: &str) -> (Option<String>, Option<String>) {
    if !full_author.contains('@') {
        return (Some(full_author.to_string()), None);
    }
    parse_email_address(full_author)
}

/// Split `hg manifest` output into paths (`getAllFiles`,
/// `ArcanistMercurialAPI.php:268-272`). Divergence: PHP returns a lazy
/// iterator, this collects a vector; the newline-in-path caveat is inherited.
#[must_use]
pub fn parse_manifest_lines(stdout: &str) -> Vec<String> {
    stdout.lines().map(str::to_string).collect()
}

/// `getMercurialConfig` value handling
/// (`ArcanistMercurialAPI.php:622-628`): empty output yields the default,
/// otherwise trailing whitespace is stripped.
#[must_use]
pub fn parse_config_value(stdout: &str, default: &str) -> String {
    if stdout.is_empty() {
        default.to_string()
    } else {
        stdout.trim_end().to_string()
    }
}

/// Parse `log --template '{node} ' --rev children(.)` output
/// (`ArcanistMercurialAPI.php:706-710`).
#[must_use]
pub fn parse_children_list(stdout: &str) -> Vec<String> {
    stdout
        .split(' ')
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// `getCommitSummary` (`ArcanistMercurialAPI.php:779-791`): `null` names
/// the empty void, otherwise the first description line is trimmed.
#[must_use]
pub fn parse_commit_summary(commit: &str, stdout: &str) -> String {
    if commit == "null" {
        return "(The Empty Void)".to_string();
    }
    stdout.split('\n').next().unwrap_or("").trim().to_string()
}

/// Parse `{node}\x01{desc}\x02` records newest-first
/// (`getCommitMessageLog`, `ArcanistMercurialAPI.php:532-549`). Like PHP
/// `list()`, extra `\x01` segments are ignored; a missing description
/// becomes empty where PHP yields null.
#[must_use]
pub fn parse_commit_message_log(stdout: &str) -> Vec<(String, String)> {
    let mut records = Vec::new();
    for log in stdout.trim().split('\x02') {
        if log.is_empty() {
            continue;
        }
        let mut parts = log.split('\x01');
        let node = parts.next().unwrap_or("");
        let desc = parts.next().unwrap_or("");
        records.push((node.to_string(), desc.to_string()));
    }
    records.reverse();
    records
}

/// Parse `{node}\x01{desc}\x02` records for `arc:nodiff(...)` rules
/// (`ArcanistMercurialAPI.php:911-918`), newest-first. PHP explodes with
/// limit 2, so the description keeps later `\x01` bytes.
#[must_use]
pub fn parse_nodiff_records(stdout: &str) -> Vec<(String, String)> {
    let mut records = Vec::new();
    for log in stdout.trim().split('\x02').rev() {
        if log.is_empty() {
            continue;
        }
        let (node, desc) = match log.split_once('\x01') {
            Some(pair) => pair,
            None => continue,
        };
        records.push((node.to_string(), desc.to_string()));
    }
    records
}

/// `hg svn info` (hgsubversion) fields (`getSubversionInfo`,
/// `ArcanistMercurialAPI.php:951-979`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HgSubversionInfo {
    /// `URL` line.
    pub base_path: Option<String>,
    /// `Repository UUID` line.
    pub uuid: Option<String>,
    /// `URL@Revision`, when both lines are present and non-empty.
    pub base_revision: Option<String>,
}

/// Parse `hg svn info` output. Lines without `: ` are skipped, where PHP
/// emits a notice and ignores them.
#[must_use]
pub fn parse_subversion_info(stdout: &str) -> HgSubversionInfo {
    let mut info = HgSubversionInfo::default();
    let mut revision: Option<String> = None;
    for line in stdout.trim().split('\n') {
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        match key {
            "URL" => info.base_path = Some(value.to_string()),
            "Repository UUID" => info.uuid = Some(value.to_string()),
            "Revision" => revision = Some(value.to_string()),
            _ => {}
        }
    }
    if let (Some(path), Some(rev)) = (info.base_path.clone(), revision) {
        info.base_revision = Some(format!("{path}@{rev}"));
    }
    info
}

/// Parse `log --rev parents(draft()) - draft()` node lines, skipping blank
/// lines but otherwise keeping them verbatim
/// (`newPublishedCommitHashes`, `ArcanistMercurialAPI.php:1244-1262`).
#[must_use]
pub fn parse_published_hashes(stdout: &str) -> Vec<String> {
    stdout
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Default diff context lines (`ArcanistRepositoryAPI.php:31`).
pub const DEFAULT_DIFF_CONTEXT: u32 = 0x7FFF;

/// `getDiffOptions` as argv (`ArcanistMercurialAPI.php:353-359`).
#[must_use]
pub fn diff_options(context: u32) -> Vec<String> {
    vec!["--git".to_string(), format!("-U{context}")]
}

/// `getDiffOptions` as the joined string PHP interpolates.
#[must_use]
pub fn diff_options_text(context: u32) -> String {
    format!("--git -U{context}")
}

/// Raw-diff cache key (`getRawDiffText`,
/// `ArcanistMercurialAPI.php:366`). The cache itself is excluded as
/// stateless Divergence; the key shape is preserved for callers.
#[must_use]
pub fn raw_diff_cache_key(options: &str, range: &str, path: &str) -> String {
    format!("{options} {range} {path}")
}

/// One step of the `buildBaseCommit` parent walk
/// (`ArcanistMercurialAPI.php:146-175`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseWalkStep {
    /// A non-outgoing ancestor: it is the base commit.
    Found(String),
    /// Keep walking from this outgoing parent.
    Descend(String),
    /// No parents: the whole repository is outgoing (`null`).
    Null,
}

/// One step of the `buildBaseCommit` parent walk. Parents without a hash
/// are skipped, where PHP would select null as the base.
#[must_use]
pub fn base_walk_step(outgoing_revs: &[String], parents: &[HgLogEntry]) -> BaseWalkStep {
    let revs: Vec<&str> = parents
        .iter()
        .filter_map(|parent| parent.rev.as_deref())
        .collect();
    match revs.as_slice() {
        [first, ..] if !outgoing_revs.iter().any(|rev| rev == first) => {
            BaseWalkStep::Found((*first).to_string())
        }
        [_, second, ..] if !outgoing_revs.iter().any(|rev| rev == second) => {
            BaseWalkStep::Found((*second).to_string())
        }
        [first, ..] => BaseWalkStep::Descend((*first).to_string()),
        [] => BaseWalkStep::Null,
    }
}

/// `log` template for `getLocalCommitInformation`
/// (`ArcanistMercurialAPI.php:193-198`).
pub const LOCAL_COMMIT_TEMPLATE: &str = "{node}\x01{rev}\x01{author}\x01{date|rfc822date}\x01{branch}\x01{tag}\x01{parents}\x01{desc}\x02";

/// `log` template for `getCommitMessageLog`
/// (`ArcanistMercurialAPI.php:534-538`) and `arc:nodiff(...)` rules
/// (`:911-914`).
pub const COMMIT_MESSAGE_TEMPLATE: &str = "{node}\x01{desc}\x02";

/// Revset of the commits after the base (`getLocalCommitInformation`,
/// `ArcanistMercurialAPI.php:197`; `getCommitMessageLog`, `:537`).
#[must_use]
pub fn local_commit_revset(base: &str) -> String {
    format!("({base}::. - {base})")
}

/// One commit of `getLocalCommitInformation`
/// (`ArcanistMercurialAPI.php:206-247`). Keys map 1:1 from the PHP
/// dictionary, including the `rev` alias of `commit` (kept despite the
/// PHP follow-up note).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgLocalCommit {
    /// Commit author display name.
    pub author: Option<String>,
    /// Commit timestamp; `None` where PHP `strtotime` yields false.
    pub time: Option<i64>,
    /// Branch name.
    pub branch: String,
    /// Tag.
    pub tag: String,
    /// Full commit hash.
    pub commit: String,
    /// Alias of `commit` (PHP notes it should be removed eventually).
    pub rev: String,
    /// Local revision number.
    pub local: String,
    /// Parent hashes; `None` means the caller must run `hg parents`.
    pub parents: Option<Vec<String>>,
    /// First description line.
    pub summary: String,
    /// Full description.
    pub message: String,
    /// Commit author email.
    pub author_email: Option<String>,
}

/// Parse one `\x01`-separated local-commit record. With empty `{parents}`
/// output and a known predecessor, the predecessor is the parent
/// (`ArcanistMercurialAPI.php:216-221`); otherwise `parents` stays `None`
/// for the caller to resolve. Malformed records are errors, where PHP
/// fills nulls.
pub fn parse_local_commit_record(
    log: &str,
    last_node: Option<&str>,
) -> Result<HgLocalCommit, String> {
    let parts: Vec<&str> = log.split('\x01').collect();
    if parts.len() < 8 {
        return Err(format!("Malformed Mercurial commit record: {log}"));
    }
    let (node, rev, full_author, date, branch, tag, parents, desc) = (
        parts[0], parts[1], parts[2], parts[3], parts[4], parts[5], parts[6], parts[7],
    );
    let (author, author_email) = parse_full_author(full_author);
    let commit_parents = if parents.is_empty() {
        last_node.map(|node| vec![node.to_string()])
    } else {
        None
    };
    Ok(HgLocalCommit {
        author,
        time: parse_rfc822_date(date),
        branch: branch.to_string(),
        tag: tag.to_string(),
        commit: node.to_string(),
        rev: node.to_string(),
        local: rev.to_string(),
        parents: commit_parents,
        summary: desc.split('\n').next().unwrap_or("").to_string(),
        message: desc.to_string(),
        author_email,
    })
}

/// Split a `type:name` base rule (`resolveBaseCommitRule`,
/// `ArcanistMercurialAPI.php:794`). Rules without a colon yield `None`,
/// where PHP emits a notice; both resolve to no match.
#[must_use]
pub fn split_base_rule(rule: &str) -> Option<(String, String)> {
    rule.split_once(':')
        .map(|(kind, name)| (kind.to_string(), name.to_string()))
}

/// `hg:gca(...)` revset (`ArcanistMercurialAPI.php:804-807`).
#[must_use]
pub fn gca_revset(name: &str) -> String {
    format!("ancestor(., {name})")
}

/// `arc:outgoing` revset (`ArcanistMercurialAPI.php:849-851`).
pub const OUTGOING_BASE_REVSET: &str = "limit(reverse(ancestors(.) - outgoing()), 1)";

/// `arc:bookmark` revset (`ArcanistMercurialAPI.php:880-887`).
pub const BOOKMARK_BASE_REVSET: &str = "limit(  sort(    (ancestors(.) and bookmark() - .) or    (ancestors(.) - outgoing()),   -rev),1)";

/// `arc:nodiff(...)` revset (`ArcanistMercurialAPI.php:911-914`).
#[must_use]
pub fn nodiff_revset(name: &str) -> String {
    format!("ancestor(.,{})::.^", name)
}

/// `/^gca\((.+)\)$/` target (`ArcanistMercurialAPI.php:804`).
#[must_use]
pub fn gca_rule_target(name: &str) -> Option<String> {
    let inner = name.strip_prefix("gca(")?.strip_suffix(')')?;
    if inner.is_empty() {
        return None;
    }
    Some(inner.to_string())
}

/// `/^nodiff\((.+)\)$/` target (`ArcanistMercurialAPI.php:910`).
#[must_use]
pub fn nodiff_rule_target(name: &str) -> Option<String> {
    let inner = name.strip_prefix("nodiff(")?.strip_suffix(')')?;
    if inner.is_empty() {
        return None;
    }
    Some(inner.to_string())
}

/// First newest-first record whose message `has_revision_id` reports a
/// `Differential Revision:` trailer (`ArcanistMercurialAPI.php:917-938`).
/// Message parsing itself is caller-supplied: no Differential message
/// parser is wired into this crate.
#[must_use]
pub fn select_nodiff_base(
    records: &[(String, String)],
    has_revision_id: &dyn Fn(&str) -> bool,
) -> Option<String> {
    for (node, desc) in records {
        if has_revision_id(desc) {
            return Some(node.clone());
        }
    }
    None
}

/// `getMercurialExtensionFlag` as `(--config, value)` argv
/// (`ArcanistMercurialAPI.php:1060-1083`). The `arc-hg` script path comes
/// from the caller, where PHP resolves it via the phutil library root.
pub fn extension_config_arg(
    extension: &str,
    arc_hg_script: Option<&Path>,
) -> Result<(String, String), String> {
    let value = match extension {
        "arc-hg" => {
            let Some(script) = arc_hg_script else {
                return Err("arc-hg extension script path is required".to_string());
            };
            format!("extensions.arc-hg={}", script.to_string_lossy())
        }
        "rebase" => "extensions.rebase=".to_string(),
        "shelve" => "extensions.shelve=".to_string(),
        "strip" => "extensions.strip=".to_string(),
        _ => {
            return Err(format!("Unknown Mercurial Extension: \"{extension}\"."));
        }
    };
    Ok(("--config".to_string(), value))
}

/// `buildMercurialExtensionCommand`
/// (`ArcanistMercurialAPI.php:1098-1115`): extension flag argv plus the
/// command argv.
pub fn extension_command(
    extension: &str,
    args: &[&str],
    arc_hg_script: Option<&Path>,
) -> Result<Vec<String>, String> {
    let (flag, value) = extension_config_arg(extension, arc_hg_script)?;
    let mut full = vec![flag, value];
    full.extend(args.iter().map(|item| (*item).to_string()));
    Ok(full)
}

fn argv_refs(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

/// `execxLocalWithExtension` (`ArcanistMercurialAPI.php:1117-1129`).
pub fn exec_with_extension_with(
    dir: &Path,
    hg: &HgCommand,
    extension: &str,
    args: &[&str],
    arc_hg_script: Option<&Path>,
) -> Result<String, String> {
    let full = extension_command(extension, args, arc_hg_script)?;
    stdout_text_with(dir, hg, &argv_refs(&full))
}

/// `execxLocalWithExtension` with the system binary.
pub fn exec_with_extension(
    dir: &Path,
    extension: &str,
    args: &[&str],
    arc_hg_script: Option<&Path>,
) -> Result<String, String> {
    exec_with_extension_with(dir, &HgCommand::system(), extension, args, arc_hg_script)
}

/// `execManualLocalWithExtension`
/// (`ArcanistMercurialAPI.php:1159-1171`). The future and passthru flavors
/// (`:1131-1157`) have no synchronous equivalent.
pub fn exec_manual_with_extension_with(
    dir: &Path,
    hg: &HgCommand,
    extension: &str,
    args: &[&str],
    arc_hg_script: Option<&Path>,
) -> Result<CommandResult, String> {
    let full = extension_command(extension, args, arc_hg_script)?;
    manual_with(dir, hg, &argv_refs(&full))
}

/// `execManualLocalWithExtension` with the system binary.
pub fn exec_manual_with_extension(
    dir: &Path,
    extension: &str,
    args: &[&str],
    arc_hg_script: Option<&Path>,
) -> Result<CommandResult, String> {
    exec_manual_with_extension_with(dir, &HgCommand::system(), extension, args, arc_hg_script)
}

/// Feature probe argv (`newMercurialFeatureFuture`,
/// `ArcanistMercurialAPI.php:1195-1209`).
pub fn feature_probe_argv(feature: &str) -> Result<Vec<String>, String> {
    match feature {
        "shelve" => Ok(vec![
            "--config".to_string(),
            "extensions.shelve=".to_string(),
            "shelve".to_string(),
            "--help".to_string(),
            "--".to_string(),
        ]),
        "evolve" => Ok(vec![
            "prune".to_string(),
            "--help".to_string(),
            "--".to_string(),
        ]),
        _ => Err(format!("Unknown Mercurial feature \"{feature}\".")),
    }
}

/// `getMercurialFeature` (`ArcanistMercurialAPI.php:1046-1048`): the probe
/// succeeds exactly when the capability is present
/// (`resolveMercurialFeatureFuture`, `:1211-1217`). Prefetching
/// (`willTestMercurialFeature`, `:1041-1044`) has no synchronous
/// equivalent.
pub fn mercurial_feature_supported_with(
    dir: &Path,
    hg: &HgCommand,
    feature: &str,
) -> Result<bool, String> {
    let argv = feature_probe_argv(feature)?;
    Ok(manual_with(dir, hg, &argv_refs(&argv))?.code == 0)
}

/// `getMercurialFeature` with the system binary.
pub fn mercurial_feature_supported(dir: &Path, feature: &str) -> Result<bool, String> {
    mercurial_feature_supported_with(dir, &HgCommand::system(), feature)
}

/// Trim `hg branch` output (`getBranchName`,
/// `ArcanistMercurialAPI.php:70-76`). Divergence: PHP memoizes the branch;
/// callers must cache when needed.
#[must_use]
pub fn parse_branch_name(stdout: &str) -> String {
    stdout.trim().to_string()
}

/// `getCanonicalRevisionName` (`ArcanistMercurialAPI.php:57-64`).
pub fn get_canonical_revision_name_with(
    dir: &Path,
    hg: &HgCommand,
    rev: &str,
) -> Result<String, String> {
    stdout_text_with(
        dir,
        hg,
        &["log", "-l", "1", "--template", "{node}", "-r", rev, "--"],
    )
}

/// `getCanonicalRevisionName` with the system binary.
pub fn get_canonical_revision_name(dir: &Path, rev: &str) -> Result<String, String> {
    get_canonical_revision_name_with(dir, &HgCommand::system(), rev)
}

/// `getBranchName` (`ArcanistMercurialAPI.php:70-76`).
pub fn get_branch_name_with(dir: &Path, hg: &HgCommand) -> Result<String, String> {
    Ok(parse_branch_name(&stdout_text_with(dir, hg, &["branch"])?))
}

/// `getBranchName` with the system binary.
pub fn get_branch_name(dir: &Path) -> Result<String, String> {
    get_branch_name_with(dir, &HgCommand::system())
}

/// Raw `hg bookmarks` output: one `[*] <name> <rev>:<node>` entry per
/// line, `*` marking the active bookmark.
///
/// Divergence: PHP lists markers through the `arc-hg` extension
/// (`hg arc-ls-markers`, `ArcanistMercurialRepositoryMarkerQuery.php`),
/// which arc-rs does not require; plain `hg bookmarks` carries the same
/// names, heads, and active mark.
pub fn get_bookmarks_with(dir: &Path, hg: &HgCommand) -> Result<String, String> {
    stdout_text_with(dir, hg, &["bookmarks"])
}

/// Raw `hg bookmarks` output with the system binary.
pub fn get_bookmarks(dir: &Path) -> Result<String, String> {
    get_bookmarks_with(dir, &HgCommand::system())
}

/// Raw `hg branches` output, parsed by [`parse_mercurial_branches`].
///
/// Divergence: same `arc-hg` simplification as [`get_bookmarks_with`].
pub fn get_branches_with(dir: &Path, hg: &HgCommand) -> Result<String, String> {
    stdout_text_with(dir, hg, &["branches"])
}

/// Raw `hg branches` output with the system binary.
pub fn get_branches(dir: &Path) -> Result<String, String> {
    get_branches_with(dir, &HgCommand::system())
}

/// `getSourceControlBaseRevision` for a resolved base commit
/// (`ArcanistMercurialAPI.php:53-55`).
pub fn get_source_control_base_revision_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
) -> Result<String, String> {
    get_canonical_revision_name_with(dir, hg, base)
}

/// `getSourceControlBaseRevision` with the system binary.
pub fn get_source_control_base_revision(dir: &Path, base: &str) -> Result<String, String> {
    get_source_control_base_revision_with(dir, &HgCommand::system(), base)
}

/// `getChangedFiles` (`ArcanistMercurialAPI.php:274-279`).
pub fn get_changed_files_with(
    dir: &Path,
    hg: &HgCommand,
    since_commit: &str,
) -> Result<StatusMap, String> {
    let stdout = stdout_text_with(dir, hg, &["status", "--rev", since_commit])?;
    parse_mercurial_status(&stdout)
}

/// `getChangedFiles` with the system binary.
pub fn get_changed_files(dir: &Path, since_commit: &str) -> Result<StatusMap, String> {
    get_changed_files_with(dir, &HgCommand::system(), since_commit)
}

/// `getBlame` (`ArcanistMercurialAPI.php:281-311`).
pub fn get_blame_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
    path: &str,
) -> Result<Vec<(String, String)>, String> {
    let stdout = stdout_text_with(
        dir,
        hg,
        &["annotate", "-u", "-v", "-c", "--rev", base, "--", path],
    )?;
    parse_blame(&stdout)
}

/// `getBlame` with the system binary.
pub fn get_blame(dir: &Path, base: &str, path: &str) -> Result<Vec<(String, String)>, String> {
    get_blame_with(dir, &HgCommand::system(), base, path)
}

/// `buildUncommittedStatus` output handling
/// (`ArcanistMercurialAPI.php:313-329`): tracked files gain
/// `FLAG_UNCOMMITTED`.
pub fn uncommitted_status_from_output(stdout: &str) -> Result<StatusMap, String> {
    let mut result = StatusMap::new();
    for (path, mut mask) in parse_mercurial_status(stdout)? {
        if mask & FLAG_UNTRACKED == 0 {
            mask |= FLAG_UNCOMMITTED;
        }
        result.insert(path, mask);
    }
    Ok(result)
}

/// `buildUncommittedStatus` (`ArcanistMercurialAPI.php:313-329`).
pub fn get_uncommitted_status_with(dir: &Path, hg: &HgCommand) -> Result<StatusMap, String> {
    let stdout = stdout_text_with(dir, hg, &["status"])?;
    uncommitted_status_from_output(&stdout)
}

/// `buildUncommittedStatus` with the system binary.
pub fn get_uncommitted_status(dir: &Path) -> Result<StatusMap, String> {
    get_uncommitted_status_with(dir, &HgCommand::system())
}

/// `buildCommitRangeStatus` output handling
/// (`ArcanistMercurialAPI.php:331-344`).
pub fn commit_range_status_from_output(stdout: &str) -> Result<StatusMap, String> {
    parse_mercurial_status(stdout)
}

/// `buildCommitRangeStatus` (`ArcanistMercurialAPI.php:331-344`).
pub fn get_commit_range_status_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
) -> Result<StatusMap, String> {
    let stdout = stdout_text_with(dir, hg, &["status", "--rev", base, "--rev", "tip"])?;
    commit_range_status_from_output(&stdout)
}

/// `buildCommitRangeStatus` with the system binary.
pub fn get_commit_range_status(dir: &Path, base: &str) -> Result<StatusMap, String> {
    get_commit_range_status_with(dir, &HgCommand::system(), base)
}

/// `getRawDiffText` (`ArcanistMercurialAPI.php:361-380`).
pub fn get_raw_diff_text_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
    path: &str,
    context: u32,
) -> Result<String, String> {
    let options = diff_options(context);
    let args: Vec<&str> = vec!["diff", &options[0], &options[1], "--rev", base, "--", path];
    stdout_text_with(dir, hg, &args)
}

/// `getRawDiffText` with the system binary.
pub fn get_raw_diff_text(
    dir: &Path,
    base: &str,
    path: &str,
    context: u32,
) -> Result<String, String> {
    get_raw_diff_text_with(dir, &HgCommand::system(), base, path, context)
}

/// `getFullMercurialDiff` (`ArcanistMercurialAPI.php:382-384`).
pub fn get_full_mercurial_diff_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
    context: u32,
) -> Result<String, String> {
    get_raw_diff_text_with(dir, hg, base, "", context)
}

/// `getFullMercurialDiff` with the system binary.
pub fn get_full_mercurial_diff(dir: &Path, base: &str, context: u32) -> Result<String, String> {
    get_full_mercurial_diff_with(dir, &HgCommand::system(), base, context)
}

/// `cat --rev` at one revision: failures mean no file at that revision
/// (`getFileDataAtRevision`, `ArcanistMercurialAPI.php:444-455`).
pub fn file_data_at_revision_with(
    dir: &Path,
    hg: &HgCommand,
    path: &str,
    rev: &str,
) -> Result<Option<Vec<u8>>, String> {
    let out = output_with(dir, hg, &["cat", "--rev", rev, "--", path])?;
    if out.status.success() {
        Ok(Some(out.stdout))
    } else {
        Ok(None)
    }
}

/// `getFileDataAtRevision` with the system binary.
pub fn file_data_at_revision(dir: &Path, path: &str, rev: &str) -> Result<Option<Vec<u8>>, String> {
    file_data_at_revision_with(dir, &HgCommand::system(), path, rev)
}

/// `getOriginalFileData` (`ArcanistMercurialAPI.php:386-388`).
pub fn get_original_file_data_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    file_data_at_revision_with(dir, hg, path, base)
}

/// `getOriginalFileData` with the system binary.
pub fn get_original_file_data(
    dir: &Path,
    base: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    get_original_file_data_with(dir, &HgCommand::system(), base, path)
}

/// `getCurrentFileData` (`ArcanistMercurialAPI.php:390-394`).
pub fn get_current_file_data_with(
    dir: &Path,
    hg: &HgCommand,
    working_rev: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    file_data_at_revision_with(dir, hg, path, working_rev)
}

/// `getCurrentFileData` with the system binary.
pub fn get_current_file_data(
    dir: &Path,
    working_rev: &str,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    get_current_file_data_with(dir, &HgCommand::system(), working_rev, path)
}

/// `--output` argument for batched `hg cat`: the temp dir plus a literal
/// `%p` formatting directive, avoiding Windows escaping issues
/// (`ArcanistMercurialAPI.php:420-429`, PHI904).
#[must_use]
pub fn cat_output_arg(tmp: &Path) -> String {
    format!("{}{}%p", tmp.to_string_lossy(), std::path::MAIN_SEPARATOR)
}

fn unique_temp_name(tag: &str, suffix: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "arc-rs-hg-{tag}-{}-{n}{suffix}",
        std::process::id()
    ))
}

fn create_temp_dir(tag: &str) -> Result<PathBuf, String> {
    // `.d` suffix keeps lib temp dirs out of the unsuffixed namespace
    // test helpers use for their own dirs.
    let dir = unique_temp_name(tag, ".d");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create temporary directory: {e}"))?;
    Ok(dir)
}

fn write_temp_file(tag: &str, data: &[u8]) -> Result<PathBuf, String> {
    let path = unique_temp_name(tag, ".tmp");
    std::fs::write(&path, data).map_err(|e| format!("failed to write temporary file: {e}"))?;
    Ok(path)
}

/// `getBulkFileDataAtRevision` into an explicit temp dir
/// (`ArcanistMercurialAPI.php:406-442`): one batched `hg cat` writes every
/// file, then whatever exists is read back. The exit status is ignored,
/// like PHP. Directories can never appear in real output; they are
/// skipped, where PHP would fail reading them.
pub fn bulk_file_data_at_revision_in(
    dir: &Path,
    hg: &HgCommand,
    paths: &[&str],
    rev: &str,
    tmp: &Path,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    for path in paths {
        if let Some(parent) = tmp.join(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create temporary directory: {e}"))?;
        }
    }
    let output_arg = cat_output_arg(tmp);
    let mut args: Vec<&str> = vec!["cat", "--rev", rev, "--output", &output_arg, "--"];
    args.extend(paths.iter().copied());
    let _ = manual_with(dir, hg, &args)?;
    let mut data = BTreeMap::new();
    for path in paths {
        let dest = tmp.join(path);
        if dest.is_file() {
            let bytes = std::fs::read(&dest).map_err(|e| format!("failed to read {path}: {e}"))?;
            data.insert((*path).to_string(), bytes);
        }
    }
    Ok(data)
}

/// `getBulkFileDataAtRevision` (`ArcanistMercurialAPI.php:406-442`),
/// managing its own temp dir.
pub fn bulk_file_data_at_revision_with(
    dir: &Path,
    hg: &HgCommand,
    paths: &[&str],
    rev: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let tmp = create_temp_dir("hgcat")?;
    let result = bulk_file_data_at_revision_in(dir, hg, paths, rev, &tmp);
    std::fs::remove_dir_all(&tmp).ok();
    result
}

/// `getBulkFileDataAtRevision` with the system binary.
pub fn bulk_file_data_at_revision(
    dir: &Path,
    paths: &[&str],
    rev: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    bulk_file_data_at_revision_with(dir, &HgCommand::system(), paths, rev)
}

/// `getBulkOriginalFileData` (`ArcanistMercurialAPI.php:396-398`).
pub fn get_bulk_original_file_data_with(
    dir: &Path,
    hg: &HgCommand,
    paths: &[&str],
    base: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    bulk_file_data_at_revision_with(dir, hg, paths, base)
}

/// `getBulkOriginalFileData` with the system binary.
pub fn get_bulk_original_file_data(
    dir: &Path,
    paths: &[&str],
    base: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    get_bulk_original_file_data_with(dir, &HgCommand::system(), paths, base)
}

/// `getBulkCurrentFileData` (`ArcanistMercurialAPI.php:400-404`).
pub fn get_bulk_current_file_data_with(
    dir: &Path,
    hg: &HgCommand,
    paths: &[&str],
    working_rev: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    bulk_file_data_at_revision_with(dir, hg, paths, working_rev)
}

/// `getBulkCurrentFileData` with the system binary.
pub fn get_bulk_current_file_data(
    dir: &Path,
    paths: &[&str],
    working_rev: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    get_bulk_current_file_data_with(dir, &HgCommand::system(), paths, working_rev)
}

/// `getAllFiles` (`ArcanistMercurialAPI.php:268-272`).
pub fn get_all_files_with(dir: &Path, hg: &HgCommand) -> Result<Vec<String>, String> {
    Ok(parse_manifest_lines(&stdout_text_with(
        dir,
        hg,
        &["manifest"],
    )?))
}

/// `getAllFiles` with the system binary.
pub fn get_all_files(dir: &Path) -> Result<Vec<String>, String> {
    get_all_files_with(dir, &HgCommand::system())
}

/// `getCommitMessage` (`ArcanistMercurialAPI.php:509-514`). Note the
/// attached `--template={desc}` form, which differs from the summary call.
pub fn get_commit_message_with(dir: &Path, hg: &HgCommand, commit: &str) -> Result<String, String> {
    stdout_text_with(dir, hg, &["log", "--template={desc}", "--rev", commit])
}

/// `getCommitMessage` with the system binary.
pub fn get_commit_message(dir: &Path, commit: &str) -> Result<String, String> {
    get_commit_message_with(dir, &HgCommand::system(), commit)
}

/// `getCommitSummary` (`ArcanistMercurialAPI.php:779-791`). Note the
/// separate `--template {desc}` form, which differs from the message call.
pub fn get_commit_summary_with(dir: &Path, hg: &HgCommand, commit: &str) -> Result<String, String> {
    if commit == "null" {
        return Ok(parse_commit_summary(commit, ""));
    }
    let stdout = stdout_text_with(
        dir,
        hg,
        &[
            "log",
            "--template",
            "{desc}",
            "--limit",
            "1",
            "--rev",
            commit,
        ],
    )?;
    Ok(parse_commit_summary(commit, &stdout))
}

/// `getCommitSummary` with the system binary.
pub fn get_commit_summary(dir: &Path, commit: &str) -> Result<String, String> {
    get_commit_summary_with(dir, &HgCommand::system(), commit)
}

/// `hasLocalCommit` (`ArcanistMercurialAPI.php:500-507`).
pub fn has_local_commit_with(dir: &Path, hg: &HgCommand, commit: &str) -> Result<bool, String> {
    match get_canonical_revision_name_with(dir, hg, commit) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// `hasLocalCommit` with the system binary.
pub fn has_local_commit(dir: &Path, commit: &str) -> Result<bool, String> {
    has_local_commit_with(dir, &HgCommand::system(), commit)
}

/// `getCommitMessageLog` (`ArcanistMercurialAPI.php:532-549`), newest-first.
pub fn get_commit_message_log_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
    branch: &str,
) -> Result<Vec<(String, String)>, String> {
    let revset = local_commit_revset(base);
    let stdout = stdout_text_with(
        dir,
        hg,
        &[
            "log",
            "--template",
            COMMIT_MESSAGE_TEMPLATE,
            "--rev",
            &revset,
            "--branch",
            branch,
            "--",
        ],
    )?;
    Ok(parse_commit_message_log(&stdout))
}

/// `getCommitMessageLog` with the system binary.
pub fn get_commit_message_log(
    dir: &Path,
    base: &str,
    branch: &str,
) -> Result<Vec<(String, String)>, String> {
    get_commit_message_log_with(dir, &HgCommand::system(), base, branch)
}

/// `getLocalCommitInformation` (`ArcanistMercurialAPI.php:190-266`),
/// newest-first. Divergence: parent lookups run sequentially instead of
/// through a 4-wide future iterator; results are identical.
pub fn get_local_commit_information_with(
    dir: &Path,
    hg: &HgCommand,
    base: &str,
    branch: &str,
) -> Result<Vec<HgLocalCommit>, String> {
    let revset = local_commit_revset(base);
    let info = stdout_text_with(
        dir,
        hg,
        &[
            "log",
            "--template",
            LOCAL_COMMIT_TEMPLATE,
            "--rev",
            &revset,
            "--branch",
            branch,
            "--",
        ],
    )?;
    let mut commits = Vec::new();
    let mut last_node: Option<String> = None;
    for log in info.split('\x02').filter(|item| !item.is_empty()) {
        let mut commit = parse_local_commit_record(log, last_node.as_deref())?;
        if commit.parents.is_none() {
            let node = commit.commit.clone();
            let parents_out = stdout_text_with(
                dir,
                hg,
                &["parents", "--template", "{node}\\n", "--rev", &node],
            )?;
            commit.parents = Some(
                parents_out
                    .split('\n')
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        }
        last_node = Some(commit.commit.clone());
        commits.push(commit);
    }
    commits.reverse();
    Ok(commits)
}

/// `getLocalCommitInformation` with the system binary.
pub fn get_local_commit_information(
    dir: &Path,
    base: &str,
    branch: &str,
) -> Result<Vec<HgLocalCommit>, String> {
    get_local_commit_information_with(dir, &HgCommand::system(), base, branch)
}

/// `updateWorkingCopy` (`ArcanistMercurialAPI.php:617-620`).
pub fn update_working_copy_with(dir: &Path, hg: &HgCommand) -> Result<(), String> {
    stdout_text_with(dir, hg, &["up"])?;
    Ok(())
}

/// `updateWorkingCopy` with the system binary.
pub fn update_working_copy(dir: &Path) -> Result<(), String> {
    update_working_copy_with(dir, &HgCommand::system())
}

/// `getMercurialConfig` (`ArcanistMercurialAPI.php:622-628`).
pub fn get_mercurial_config_with(
    dir: &Path,
    hg: &HgCommand,
    key: &str,
    default: &str,
) -> Result<String, String> {
    Ok(parse_config_value(
        &stdout_text_with(dir, hg, &["showconfig", key])?,
        default,
    ))
}

/// `getMercurialConfig` with the system binary.
pub fn get_mercurial_config(dir: &Path, key: &str, default: &str) -> Result<String, String> {
    get_mercurial_config_with(dir, &HgCommand::system(), key, default)
}

/// `getAuthor` (`ArcanistMercurialAPI.php:630-634`): the display-name part
/// of `ui.username`, or `None` when unconfigured.
pub fn get_author_with(dir: &Path, hg: &HgCommand) -> Result<Option<String>, String> {
    let out = manual_with(dir, hg, &["showconfig", "ui.username"])?;
    if out.code != 0 || out.stdout.is_empty() {
        return Ok(None);
    }
    Ok(parse_full_author(out.stdout.trim_end()).0)
}

/// `getAuthor` with the system binary.
pub fn get_author(dir: &Path) -> Result<Option<String>, String> {
    get_author_with(dir, &HgCommand::system())
}

/// `addToCommit` (`ArcanistMercurialAPI.php:658-663`).
pub fn add_to_commit_with(dir: &Path, hg: &HgCommand, paths: &[&str]) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["addremove", "--"];
    args.extend(paths.iter().copied());
    stdout_text_with(dir, hg, &args)?;
    Ok(())
}

/// `addToCommit` with the system binary.
pub fn add_to_commit(dir: &Path, paths: &[&str]) -> Result<(), String> {
    add_to_commit_with(dir, &HgCommand::system(), paths)
}

/// `doCommit` (`ArcanistMercurialAPI.php:665-670`).
pub fn do_commit_with(dir: &Path, hg: &HgCommand, message: &str) -> Result<(), String> {
    let tmp = write_temp_file("hgcommit", message.as_bytes())?;
    let path = tmp.to_string_lossy().to_string();
    let result = stdout_text_with(dir, hg, &["commit", "--logfile", &path]);
    std::fs::remove_file(&tmp).ok();
    result?;
    Ok(())
}

/// `doCommit` with the system binary.
pub fn do_commit(dir: &Path, message: &str) -> Result<(), String> {
    do_commit_with(dir, &HgCommand::system(), message)
}

/// `amendNonHeadCommit` (`ArcanistMercurialAPI.php:738-777`, 2023-only:
/// absent at ceb, where `amendCommit` always runs plain `commit --amend`
/// — T1153 LINT-23): amend via the bundled `arc-hg` extension, rebase
/// every child onto the new commit (aborting the rebase on failure,
/// where a failed abort masks the original error, exactly like PHP),
/// then strip the old commit.
pub fn amend_non_head_commit_with(
    dir: &Path,
    hg: &HgCommand,
    child_nodes: &[String],
    message_file: &str,
    arc_hg_script: Option<&Path>,
) -> Result<(), String> {
    let current = stdout_text_with(
        dir,
        hg,
        &["log", "--template", "{node}", "--rev", ".", "--"],
    )?;
    exec_with_extension_with(
        dir,
        hg,
        "arc-hg",
        &["arc-amend", "--logfile", message_file],
        arc_hg_script,
    )?;
    let new_commit = stdout_text_with(
        dir,
        hg,
        &["log", "--rev", "tip", "--template", "{node}", "--"],
    )?;
    let mut rebase = vec!["rebase".to_string(), "--dest".to_string(), new_commit];
    for child in child_nodes {
        rebase.push("--source".to_string());
        rebase.push(child.clone());
    }
    rebase.push("--".to_string());
    if let Err(first) =
        exec_with_extension_with(dir, hg, "rebase", &argv_refs(&rebase), arc_hg_script)
    {
        exec_with_extension_with(
            dir,
            hg,
            "rebase",
            &["rebase", "--abort", "--"],
            arc_hg_script,
        )?;
        return Err(first);
    }
    exec_with_extension_with(
        dir,
        hg,
        "strip",
        &["strip", "--rev", &current, "--"],
        arc_hg_script,
    )?;
    Ok(())
}

/// `amendCommit` (`ArcanistMercurialAPI.php:662-687` at ceb, T1153
/// LINT-23): always run `commit --amend`, swallowing only a
/// `/nothing changed/` failure (Mercurial treats a no-op amend as an
/// error; workflows assume no-op amends are fine). Every other failure
/// propagates. There is no early return, no `evolve` path, and no
/// children probe: those are the 2023 flow, not ceb. There is no
/// in-memory working-copy state to reload. `arc_hg_script` is unused
/// (ceb amends through plain `execxLocal`, no extension); it stays in
/// the signature for the established callers.
pub fn amend_commit_with(
    dir: &Path,
    hg: &HgCommand,
    message: Option<&str>,
    arc_hg_script: Option<&Path>,
) -> Result<(), String> {
    let _ = arc_hg_script;
    let message = match message {
        Some(text) => text.to_string(),
        None => get_commit_message_with(dir, hg, ".")?,
    };
    let tmp = write_temp_file("hgamend", message.as_bytes())?;
    let path = tmp.to_string_lossy().to_string();
    let out = output_with(dir, hg, &["commit", "--amend", "--logfile", &path])?;
    std::fs::remove_file(&tmp).ok();
    if out.status.success() {
        return Ok(());
    }
    // PHP matches `/nothing changed/` against stdout only
    // (`$ex->getStdout()`), not the combined streams.
    if String::from_utf8_lossy(&out.stdout).contains("nothing changed") {
        return Ok(());
    }
    Err(amend_failure_message(hg, &out))
}

/// Failure text for [`amend_commit_with`], shaped like
/// [`stdout_text_with`]: the trimmed combined output, else the bare
/// failing argv (without the temp message path).
fn amend_failure_message(hg: &HgCommand, out: &std::process::Output) -> String {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        format!("{} commit --amend failed", hg.program)
    } else {
        trimmed.to_string()
    }
}

/// `amendCommit` with the system binary.
pub fn amend_commit(
    dir: &Path,
    message: Option<&str>,
    arc_hg_script: Option<&Path>,
) -> Result<(), String> {
    amend_commit_with(dir, &HgCommand::system(), message, arc_hg_script)
}

/// Explicit-commit branch of `buildBaseCommit`
/// (`ArcanistMercurialAPI.php:82-105`): the base is the greatest common
/// ancestor of the working copy and the given commit. Both the quoted and
/// the raw revset attempts run, like PHP; under argv they are identical.
pub fn resolve_explicit_base_with(
    dir: &Path,
    hg: &HgCommand,
    symbolic: &str,
) -> Result<(String, String), String> {
    let ancestor = format!("ancestor({symbolic},.)");
    let commit = match get_canonical_revision_name_with(dir, hg, &ancestor) {
        Ok(commit) => commit,
        Err(_) => get_canonical_revision_name_with(dir, hg, &ancestor).map_err(|_| {
            format!("Commit '{symbolic}' is not a valid Mercurial commit identifier.")
        })?,
    };
    Ok((
        commit,
        "it is the greatest common ancestor of the working directory and the commit you specified explicitly."
            .to_string(),
    ))
}

/// Explicit-commit branch of `buildBaseCommit` with the system binary.
pub fn resolve_explicit_base(dir: &Path, symbolic: &str) -> Result<(String, String), String> {
    resolve_explicit_base_with(dir, &HgCommand::system(), symbolic)
}

/// Automatic branch of `buildBaseCommit`
/// (`ArcanistMercurialAPI.php:120-188`): no outgoing commits means the
/// working-copy revision, otherwise walk parents to the first non-outgoing
/// ancestor (`null` when everything is outgoing). A failed `draft()` log
/// means no outgoing commits, exactly like PHP.
pub fn auto_base_commit_with(
    dir: &Path,
    hg: &HgCommand,
    branch: &str,
    working_rev: &str,
) -> Result<(String, String), String> {
    let out = manual_with(
        dir,
        hg,
        &[
            "log", "--branch", branch, "-r", "draft()", "--style", "default",
        ],
    )?;
    let logs = if out.code == 0 {
        parse_mercurial_log(&out.stdout)?
    } else {
        Vec::new()
    };
    if logs.is_empty() {
        return Ok((
            working_rev.to_string(),
            "you have no outgoing commits, so arc assumes you intend to submit uncommitted changes in the working copy."
                .to_string(),
        ));
    }
    let outgoing: Vec<String> = logs.iter().filter_map(|log| log.rev.clone()).collect();
    let mut against = working_rev.to_string();
    loop {
        let parents_out = stdout_text_with(
            dir,
            hg,
            &["parents", "--style", "default", "--rev", &against],
        )?;
        let parents = parse_mercurial_log(&parents_out)?;
        match base_walk_step(&outgoing, &parents) {
            BaseWalkStep::Found(rev) => {
                against = rev;
                break;
            }
            BaseWalkStep::Descend(rev) => {
                against = rev;
            }
            BaseWalkStep::Null => {
                against = "null".to_string();
                break;
            }
        }
    }
    if against == "null" {
        Ok((
            against,
            "this is a new repository (all changes are outgoing).".to_string(),
        ))
    } else {
        Ok((
            against,
            "it is the first commit reachable from the working copy state which is not outgoing."
                .to_string(),
        ))
    }
}

/// Automatic branch of `buildBaseCommit` with the system binary.
pub fn auto_base_commit(
    dir: &Path,
    branch: &str,
    working_rev: &str,
) -> Result<(String, String), String> {
    auto_base_commit_with(dir, &HgCommand::system(), branch, working_rev)
}

/// `hg:` rules of `resolveBaseCommitRule`
/// (`ArcanistMercurialAPI.php:802-837`). Divergence: the escaped-then-raw
/// retry collapses to one `log` call under argv.
pub fn resolve_hg_base_rule_with(
    dir: &Path,
    hg: &HgCommand,
    name: &str,
    rule: &str,
    source: &str,
) -> Result<Option<(String, String)>, String> {
    if let Some(target) = gca_rule_target(name) {
        let revset = gca_revset(&target);
        let out = manual_with(dir, hg, &["log", "--template={node}", "--rev", &revset])?;
        if out.code != 0 {
            return Ok(None);
        }
        return Ok(Some((
            out.stdout.trim().to_string(),
            format!(
                "it is the greatest common ancestor of '{target}' and ., as specified by '{rule}' in your {source} 'base' configuration."
            ),
        )));
    }
    let out = manual_with(dir, hg, &["log", "--template", "{node}", "--rev", name])?;
    if out.code != 0 {
        return Ok(None);
    }
    Ok(Some((
        out.stdout.trim().to_string(),
        format!("it is specified by '{rule}' in your {source} 'base' configuration."),
    )))
}

/// `arc:` rules of `resolveBaseCommitRule`
/// (`ArcanistMercurialAPI.php:839-941`). Like PHP, a failed `outgoing`
/// probe falls through to the `amended` check. `has_revision_id` reports
/// `Differential Revision:` trailers; no message parser is wired in.
pub fn resolve_arc_base_rule_with(
    dir: &Path,
    hg: &HgCommand,
    name: &str,
    rule: &str,
    source: &str,
    has_revision_id: &dyn Fn(&str) -> bool,
) -> Result<Option<(String, String)>, String> {
    match name {
        "empty" => Ok(Some((
            "null".to_string(),
            format!("you specified '{rule}' in your {source} 'base' configuration."),
        ))),
        "outgoing" => {
            let out = manual_with(
                dir,
                hg,
                &["log", "--template={node}", "--rev", OUTGOING_BASE_REVSET],
            )?;
            if out.code == 0 {
                return Ok(Some((
                    out.stdout.trim().to_string(),
                    format!(
                        "it is the first ancestor of the working copy that is not outgoing, and it matched the rule {rule} in your {source} 'base' configuration."
                    ),
                )));
            }
            resolve_amended_base_with(dir, hg, rule, source, has_revision_id)
        }
        "amended" => resolve_amended_base_with(dir, hg, rule, source, has_revision_id),
        "bookmark" => {
            let out = manual_with(
                dir,
                hg,
                &["log", "--template={node}", "--rev", BOOKMARK_BASE_REVSET],
            )?;
            if out.code != 0 {
                return Ok(None);
            }
            Ok(Some((
                out.stdout.trim().to_string(),
                format!(
                    "it is the first ancestor of . that either has a bookmark, or is already in the remote and it matched the rule {rule} in your {source} 'base' configuration"
                ),
            )))
        }
        "this" => {
            let commit = get_canonical_revision_name_with(dir, hg, ".^")?;
            Ok(Some((
                commit,
                format!("you specified '{rule}' in your {source} 'base' configuration."),
            )))
        }
        _ => {
            let Some(target) = nodiff_rule_target(name) else {
                return Ok(None);
            };
            let revset = nodiff_revset(&target);
            let stdout = stdout_text_with(
                dir,
                hg,
                &[
                    "log",
                    "--template",
                    COMMIT_MESSAGE_TEMPLATE,
                    "--rev",
                    &revset,
                ],
            )?;
            let records = parse_nodiff_records(&stdout);
            Ok(select_nodiff_base(&records, has_revision_id).map(|node| {
                (
                    node,
                    format!(
                        "it is the first ancestor of . that has a diff and is the gca or a descendant of the gca with '{target}', specified by '{rule}' in your {source} 'base' configuration."
                    ),
                )
            }))
        }
    }
}

fn resolve_amended_base_with(
    dir: &Path,
    hg: &HgCommand,
    rule: &str,
    source: &str,
    has_revision_id: &dyn Fn(&str) -> bool,
) -> Result<Option<(String, String)>, String> {
    let text = get_commit_message_with(dir, hg, ".")?;
    if !has_revision_id(&text) {
        return Ok(None);
    }
    let commit = get_canonical_revision_name_with(dir, hg, ".^")?;
    Ok(Some((
        commit,
        format!(
            "'.' has been amended with 'Differential Revision:', as specified by '{rule}' in your {source} 'base' configuration."
        ),
    )))
}

/// `resolveBaseCommitRule` (`ArcanistMercurialAPI.php:793-949`): the
/// `(commit, explanation)` for one `type:name` rule, or `None` for unknown
/// rule types and non-matching rules.
pub fn resolve_base_commit_rule_with(
    dir: &Path,
    hg: &HgCommand,
    rule: &str,
    source: &str,
    has_revision_id: &dyn Fn(&str) -> bool,
) -> Result<Option<(String, String)>, String> {
    let Some((kind, name)) = split_base_rule(rule) else {
        return Ok(None);
    };
    match kind.as_str() {
        "hg" => resolve_hg_base_rule_with(dir, hg, &name, rule, source),
        "arc" => resolve_arc_base_rule_with(dir, hg, &name, rule, source, has_revision_id),
        _ => Ok(None),
    }
}

/// `resolveBaseCommitRule` with the system binary.
pub fn resolve_base_commit_rule(
    dir: &Path,
    rule: &str,
    source: &str,
    has_revision_id: &dyn Fn(&str) -> bool,
) -> Result<Option<(String, String)>, String> {
    resolve_base_commit_rule_with(dir, &HgCommand::system(), rule, source, has_revision_id)
}

/// First matching `(rule, source)` pair, else the configured-rules error
/// (`buildBaseCommit`, `ArcanistMercurialAPI.php:107-118`). This is a small
/// caller-side helper, not a PHP method: rule sources come from the
/// configuration manager, which is excluded.
pub fn first_matching_base_rule_with(
    dir: &Path,
    hg: &HgCommand,
    rules: &[(&str, &str)],
    has_revision_id: &dyn Fn(&str) -> bool,
) -> Result<(String, String), String> {
    for (rule, source) in rules {
        if let Some(found) = resolve_base_commit_rule_with(dir, hg, rule, source, has_revision_id)?
        {
            return Ok(found);
        }
    }
    Err("None of the rules in your 'base' configuration matched a valid commit. Adjust rules or specify which commit you want to use explicitly."
        .to_string())
}

/// `supportsAmend` (`ArcanistMercurialAPI.php:469-476`): `help commit`
/// must succeed and mention `amend`.
pub fn supports_amend_with(dir: &Path, hg: &HgCommand) -> Result<bool, String> {
    let out = manual_with(dir, hg, &["help", "commit"])?;
    Ok(out.code == 0 && out.stdout.contains("amend"))
}

/// `supportsAmend` with the system binary.
pub fn supports_amend(dir: &Path) -> Result<bool, String> {
    supports_amend_with(dir, &HgCommand::system())
}

/// `getSubversionInfo` (`ArcanistMercurialAPI.php:951-979`): failed
/// `hg svn info` yields empty info.
pub fn get_subversion_info_with(dir: &Path, hg: &HgCommand) -> Result<HgSubversionInfo, String> {
    let out = manual_with(dir, hg, &["svn", "info"])?;
    if out.code != 0 {
        return Ok(HgSubversionInfo::default());
    }
    Ok(parse_subversion_info(&out.stdout))
}

/// `getSubversionInfo` with the system binary.
pub fn get_subversion_info(dir: &Path) -> Result<HgSubversionInfo, String> {
    get_subversion_info_with(dir, &HgCommand::system())
}

/// `getRemoteURI` (`ArcanistMercurialAPI.php:997-1008`).
pub fn get_remote_uri_with(dir: &Path, hg: &HgCommand) -> Result<Option<String>, String> {
    let uri = stdout_text_with(dir, hg, &["paths", "default"])?;
    let uri = uri.trim().to_string();
    if uri.is_empty() {
        return Ok(None);
    }
    Ok(Some(uri))
}

/// `getRemoteURI` with the system binary.
pub fn get_remote_uri(dir: &Path) -> Result<Option<String>, String> {
    get_remote_uri_with(dir, &HgCommand::system())
}

/// `getPublishedCommitHashes` (`ArcanistRepositoryAPI.php:820-826` via
/// `newPublishedCommitHashes`, `ArcanistMercurialAPI.php:1244-1262`).
/// Like PHP, the exit status is ignored.
pub fn get_published_commit_hashes_with(dir: &Path, hg: &HgCommand) -> Result<Vec<String>, String> {
    let out = manual_with(
        dir,
        hg,
        &[
            "log",
            "--rev",
            "parents(draft()) - draft()",
            "--template",
            "{node}\\n",
        ],
    )?;
    Ok(parse_published_hashes(&out.stdout))
}

/// `getPublishedCommitHashes` with the system binary.
pub fn get_published_commit_hashes(dir: &Path) -> Result<Vec<String>, String> {
    get_published_commit_hashes_with(dir, &HgCommand::system())
}

/// `getPath` (`ArcanistRepositoryAPI.php:105-113`): the root, or the root
/// joined with a file whose leading separators are stripped. Both slash
/// styles strip on every platform, where PHP strips only the platform
/// separator.
#[must_use]
pub fn join_path(root: &Path, file: Option<&str>) -> PathBuf {
    match file {
        None => root.to_path_buf(),
        Some(file) => root.join(file.trim_start_matches(['/', '\\'])),
    }
}

/// `getUncommittedPathsWithMask`
/// (`ArcanistRepositoryAPI.php:214-222`).
#[must_use]
pub fn paths_with_mask(status: &StatusMap, mask: u32) -> Vec<String> {
    status
        .iter()
        .filter(|(_, flags)| *flags & mask != 0)
        .map(|(path, _)| path.clone())
        .collect()
}

/// `getUntrackedChanges` (`ArcanistRepositoryAPI.php:158-160`).
#[must_use]
pub fn get_untracked_changes(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_UNTRACKED)
}

/// `getUnstagedChanges` (`ArcanistRepositoryAPI.php:166-168`).
#[must_use]
pub fn get_unstaged_changes(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_UNSTAGED)
}

/// `getUncommittedChanges` (`ArcanistRepositoryAPI.php:174-176`).
#[must_use]
pub fn get_uncommitted_changes(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_UNCOMMITTED)
}

/// `getMergeConflicts` (`ArcanistRepositoryAPI.php:182-184`).
#[must_use]
pub fn get_merge_conflicts(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_CONFLICT)
}

/// `getIncompleteChanges` (`ArcanistRepositoryAPI.php:190-192`).
#[must_use]
pub fn get_incomplete_changes(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_INCOMPLETE)
}

/// `getMissingChanges` (`ArcanistRepositoryAPI.php:198-200`).
#[must_use]
pub fn get_missing_changes(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_MISSING)
}

/// `getDirtyExternalChanges` (`ArcanistRepositoryAPI.php:206-208`).
#[must_use]
pub fn get_dirty_external_changes(status: &StatusMap) -> Vec<String> {
    paths_with_mask(status, FLAG_EXTERNALS)
}

/// `getWorkingCopyStatus` merging (`ArcanistRepositoryAPI.php:253-265`):
/// range flags OR-ed with uncommitted flags.
#[must_use]
pub fn merge_status(range: &StatusMap, uncommitted: &StatusMap) -> StatusMap {
    let mut result = range.clone();
    for (path, mask) in uncommitted {
        *result.entry(path.clone()).or_insert(0) |= mask;
    }
    result
}

/// `getDisplayHash` (`ArcanistRepositoryAPI.php:806-808`).
#[must_use]
pub fn get_display_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// `getScratchFilePath` (`ArcanistRepositoryAPI.php:538-565`).
///
/// The one-time `.arc` migration there is explicit here: call
/// [`migrate_scratch_dir`] before the first scratch access.
#[must_use]
pub fn scratch_file_path(metadata: &Path, name: &str) -> PathBuf {
    metadata.join("arc").join(name)
}

/// Top-level file names under `old`, or `None` when the directory cannot
/// be listed or holds anything but plain files.
fn migration_file_names(old: &Path) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(old).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_file() {
            return None;
        }
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Some(names)
}

/// Move the legacy `<root>/.arc` scratch directory to `<metadata>/arc`.
///
/// PHP: the one-time migration inside `getScratchFilePath`
/// (`ArcanistRepositoryAPI.php:538-565`). When the old directory exists
/// and the new one does not, every top-level file moves over and the old
/// directory is removed; otherwise nothing happens. Returns true when the
/// new location is usable afterwards (migrated, or nothing to migrate).
///
/// Divergence: PHP runs this once per process behind a static flag; the
/// `<new> missing` check makes repeats no-ops anyway, so this runs on
/// demand. PHP also crashes when the old directory holds a subdirectory;
/// this leaves the old directory untouched instead, never deleting data
/// the migration did not move.
pub fn migrate_scratch_dir(work_root: &Path, metadata: &Path) -> bool {
    let old = work_root.join(".arc");
    let new = metadata.join("arc");
    if !old.is_dir() || new.exists() {
        return true;
    }
    let Some(names) = migration_file_names(&old) else {
        return false;
    };
    if std::fs::create_dir_all(&new).is_err() {
        return false;
    }
    for name in &names {
        if std::fs::copy(old.join(name), new.join(name)).is_err() {
            return false;
        }
    }
    std::fs::remove_dir_all(&old).is_ok() && !old.exists()
}

/// `readScratchFile` (`ArcanistRepositoryAPI.php:436-453`).
#[must_use]
pub fn read_scratch_file(metadata: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(scratch_file_path(metadata, name)).ok()
}

/// `writeScratchFile` (`ArcanistRepositoryAPI.php:465-486`).
pub fn write_scratch_file(metadata: &Path, name: &str, data: &str) -> bool {
    let dir = scratch_file_path(metadata, "");
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    std::fs::write(scratch_file_path(metadata, name), data).is_ok()
}

/// `removeScratchFile` (`ArcanistRepositoryAPI.php:496-509`).
pub fn remove_scratch_file(metadata: &Path, name: &str) -> bool {
    let path = scratch_file_path(metadata, name);
    std::fs::remove_file(&path)
        .or_else(|_| std::fs::remove_dir_all(&path).map(|_| ()))
        .is_ok()
        && !path.exists()
}

#[cfg(test)]
#[path = "hg_tests.rs"]
mod hg_tests;
