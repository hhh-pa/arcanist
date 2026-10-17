//! Subversion (`svn`) working-copy API.
//!
//! PHP reference: `src/repository/api/ArcanistSubversionAPI.php` and the
//! needed `ArcanistRepositoryAPI` base parts
//! (`src/repository/api/ArcanistRepositoryAPI.php`).
//!
//! API shape mirrors `git.rs`: stateless free functions taking the working
//! copy root first, `Result<_, String>` errors, and pure `parse_*` seams
//! for every output shape. Functions that spawn `svn` come in two forms:
//! `name(dir, ...)` uses the system binary, `name_with(dir, svn, ...)`
//! takes an [`SvnCommand`] backend so tests can point at a fake executable.
//!
//! `dir` is both the subprocess working directory and the root that paths
//! join onto, like PHP `getPath()` serving both roles.
//!
//! Divergences from PHP (behavior limits, all deliberate):
//!
//! - Stateless: no `svnStatus` / `svnInfo` / `svnDiffRaw` caches and no
//!   `primeSVNInfoResult` / `primeSVNDiffResult` seams
//!   (`ArcanistSubversionAPI.php:8-16,316-324`); every call shells out and
//!   the pure `parse_*` functions are the test seams.
//! - Backslash status paths normalize to slashes on every platform, where
//!   PHP only does so on Windows (`:89-91`).
//! - `getLocalCommitInformation` always returns null in PHP (`:609-611`);
//!   it is excluded and covered by `supports_local_commits() == false`.
//! - The base-revision override (`:235-238`) is a caller-passed argument
//!   instead of object state.
//! - `addToCommit` existence checks resolve against `dir`, where PHP checks
//!   the process working directory (`:186`); the two agree when arc runs at
//!   the working-copy root.
//! - The `diff` platform branch (`:295-313`) is an explicit `diff_cmd`
//!   parameter; the unified-`diff` program for synthetic additions is an
//!   explicit parameter defaulting to `diff`.
//! - File and diff data are `Vec<u8>`, matching PHP byte strings.
//! - Dates parse to `Option<i64>`; unparseable dates yield `None` where
//!   PHP `strtotime` yields false.
//! - Scratch `.arc` migration (`ArcanistRepositoryAPI.php:551-562`) is
//!   explicit: [`migrate_scratch_dir`] moves the files on demand instead of
//!   once per process, and it leaves the old directory alone when it holds
//!   anything but plain files (PHP would crash mid-migration there).
//! - Conduit revision lookup (`:643-660`) is excluded; see the crate brief.
//!
//! Status maps use `BTreeMap`, so iteration order matches PHP `ksort`
//! (`ArcanistRepositoryAPI.php:148,236`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Map from repo-relative path to `FLAG_*` bitmask, as returned by
/// `getUncommittedStatus` (`ArcanistRepositoryAPI.php:145-152`).
pub type StatusMap = BTreeMap<String, u32>;

/// System Subversion binary name.
pub const SVN: &str = "svn";

/// How to invoke the `svn` binary: the system binary, or a fake executable
/// for hermetic tests (for example `cmd /C fakesvn.bat` on Windows, which
/// needs the `prefix` words before the `svn` arguments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvnCommand {
    /// Executable to spawn.
    pub program: String,
    /// Fixed leading arguments, empty for the real binary.
    pub prefix: Vec<String>,
}

impl SvnCommand {
    /// Invoke the system `svn` binary.
    #[must_use]
    pub fn system() -> Self {
        Self {
            program: SVN.to_string(),
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

impl Default for SvnCommand {
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

fn command_for(dir: &Path, svn: &SvnCommand, args: &[&str]) -> std::process::Command {
    let mut cmd = crate::proc_env::command(&svn.program);
    cmd.current_dir(dir);
    cmd.args(&svn.prefix);
    cmd.args(args);
    cmd
}

/// Spawn `cmd`, retrying briefly on ETXTBSY ("Text file busy"). A
/// freshly written executable can still be settling when it is spawned
/// (parallel tests, an editor saving a hook); the error is transient by
/// nature, so retry bounded, then return the last error unchanged.
fn spawn_output(cmd: &mut std::process::Command) -> Result<std::process::Output, std::io::Error> {
    let mut attempt = 0;
    loop {
        match cmd.output() {
            Err(error) if is_text_file_busy(&error) && attempt < 50 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

/// True for ETXTBSY (errno 26 on Linux and the BSDs, including macOS).
/// Windows has no such spawn error.
#[cfg(unix)]
fn is_text_file_busy(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(26)
}

/// True for ETXTBSY (errno 26 on Linux and the BSDs, including macOS).
/// Windows has no such spawn error.
#[cfg(not(unix))]
fn is_text_file_busy(_error: &std::io::Error) -> bool {
    false
}

/// Run `svn` in `dir`, returning the raw output. Spawn failures become
/// errors; non-zero exit statuses are left for the caller to interpret.
/// Mirrors `git::output` (`ArcanistSubversionAPI.php:39-45`).
pub fn output_with(
    dir: &Path,
    svn: &SvnCommand,
    args: &[&str],
) -> Result<std::process::Output, String> {
    spawn_output(&mut command_for(dir, svn, args))
        .map_err(|e| format!("failed to run {}: {e}", svn.program))
}

/// Run the system `svn` in `dir`, returning the raw output.
pub fn output(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    output_with(dir, &SvnCommand::system(), args)
}

/// Run `svn` in `dir`, returning trimmed stdout, or the combined trimmed
/// output when `svn` exits non-zero. Mirrors `git::stdout_text`.
pub fn stdout_text_with(dir: &Path, svn: &SvnCommand, args: &[&str]) -> Result<String, String> {
    let out = output_with(dir, svn, args)?;
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
            Err(format!("{} {} failed", svn.program, args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

/// Run the system `svn` in `dir`, returning trimmed stdout.
pub fn stdout_text(dir: &Path, args: &[&str]) -> Result<String, String> {
    stdout_text_with(dir, &SvnCommand::system(), args)
}

/// True when `svn` exits zero in `dir` for `args`. Mirrors `git::ok`.
#[must_use]
pub fn ok_with(dir: &Path, svn: &SvnCommand, args: &[&str]) -> bool {
    output_with(dir, svn, args).is_ok_and(|out| out.status.success())
}

/// True when the system `svn` exits zero in `dir` for `args`.
#[must_use]
pub fn ok(dir: &Path, args: &[&str]) -> bool {
    ok_with(dir, &SvnCommand::system(), args)
}

/// Run `svn` in `dir` without failing on non-zero exit, returning the exit
/// code plus both streams (`execManualLocal`,
/// `ArcanistRepositoryAPI.php:402-405`).
pub fn manual_with(dir: &Path, svn: &SvnCommand, args: &[&str]) -> Result<CommandResult, String> {
    let out = output_with(dir, svn, args)?;
    Ok(CommandResult {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

/// Run the system `svn` in `dir` without failing on non-zero exit.
pub fn manual(dir: &Path, args: &[&str]) -> Result<CommandResult, String> {
    manual_with(dir, &SvnCommand::system(), args)
}

fn argv_refs(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
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

/// `getSourceControlSystemName` (`ArcanistSubversionAPI.php:18-20`).
#[must_use]
pub fn get_source_control_system_name() -> &'static str {
    "svn"
}

/// `isHistoryDefaultImmutable` (`ArcanistSubversionAPI.php:613-615`).
#[must_use]
pub fn is_history_default_immutable() -> bool {
    true
}

/// `supportsAmend` (`ArcanistSubversionAPI.php:617-619`).
#[must_use]
pub fn supports_amend() -> bool {
    false
}

/// `supportsCommitRanges` (`ArcanistSubversionAPI.php:621-623`).
#[must_use]
pub fn supports_commit_ranges() -> bool {
    false
}

/// `supportsLocalCommits` (`ArcanistSubversionAPI.php:625-627`).
#[must_use]
pub fn supports_local_commits() -> bool {
    false
}

/// `hasLocalCommit` (`ArcanistSubversionAPI.php:629-631`).
#[must_use]
pub fn has_local_commit(_commit: &str) -> bool {
    false
}

/// `getFinalizedRevisionMessage` (`ArcanistSubversionAPI.php:637-641`).
#[must_use]
pub fn get_finalized_revision_message() -> &'static str {
    "Done."
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

/// `buildCommitRangeStatus` (`ArcanistSubversionAPI.php:47-51`): SVN never
/// has previous commits in the range.
#[must_use]
pub fn get_commit_range_status() -> StatusMap {
    StatusMap::new()
}

/// Walk toward the filesystem root for the working-copy `.svn` dir
/// (`getMetadataPath`, `ArcanistSubversionAPI.php:22-37`). Divergence: no
/// static cache.
#[must_use]
pub fn find_metadata_path(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(".svn");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// `getMetadataPath` (`ArcanistSubversionAPI.php:22-37`).
#[must_use]
pub fn get_metadata_path(root: &Path) -> Option<PathBuf> {
    find_metadata_path(root)
}

/// `parseSVNStatus` (`ArcanistSubversionAPI.php:151-183`). `replaced`
/// maps to modified, exactly like PHP.
pub fn parse_svn_status_item(item: &str) -> Result<u32, String> {
    match item {
        "none" | "normal" => Ok(0),
        "external" => Ok(FLAG_EXTERNALS),
        "unversioned" => Ok(FLAG_UNTRACKED),
        "obstructed" => Ok(FLAG_OBSTRUCTED),
        "missing" => Ok(FLAG_MISSING),
        "added" => Ok(FLAG_ADDED),
        "replaced" => Ok(FLAG_MODIFIED),
        "modified" => Ok(FLAG_MODIFIED),
        "deleted" => Ok(FLAG_DELETED),
        "conflicted" => Ok(FLAG_CONFLICT),
        "incomplete" => Ok(FLAG_INCOMPLETE),
        _ => Err(format!("Unrecognized item status '{item}'.")),
    }
}

fn decode_entity(entity: &str) -> Option<String> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string());
    }
    match entity {
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" => Some("'".to_string()),
        _ => None,
    }
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let entity = &rest[1..semi];
        if entity.contains('<') || entity.contains('&') {
            out.push('&');
            rest = &rest[1..];
            continue;
        }
        match decode_entity(entity) {
            Some(decoded) => {
                out.push_str(&decoded);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse one tag's attributes from its inner text (between `<name` and
/// `>`), supporting both quote styles and a trailing `/`.
fn parse_tag_attrs(tag: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' {
        i += 1;
    }
    while i < bytes.len() {
        let Some(((name, value), next)) = scan_tag_attr(tag, bytes, i) else {
            break;
        };
        attrs.insert(name, value);
        i = next;
    }
    attrs
}

/// Scan one attribute at `start`: the `(name, value)` pair plus the resume
/// offset. Anything malformed halts the whole tag, like the PHP parser.
fn scan_tag_attr(tag: &str, bytes: &[u8], start: usize) -> Option<((String, String), usize)> {
    let i = skip_tag_gap(bytes, start);
    if i >= bytes.len() {
        return None;
    }
    let (name, i) = scan_tag_name(tag, bytes, i);
    let (value, next) = scan_tag_value(tag, bytes, &name, i)?;
    Some(((name, value), next))
}

/// Scan an attribute name plus the whitespace before `=`: the name and the
/// offset past the gap.
fn scan_tag_name(tag: &str, bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let name = tag[start..i].to_string();
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    (name, i)
}

/// Scan the `= "value"` half of the attribute named `name` at `start`.
fn scan_tag_value(tag: &str, bytes: &[u8], name: &str, start: usize) -> Option<(String, usize)> {
    let mut i = start;
    if i >= bytes.len() || bytes[i] != b'=' || name.is_empty() {
        return None;
    }
    i += 1;
    i = skip_tag_ws(bytes, i);
    if i >= bytes.len() {
        return None;
    }
    let quote = bytes[i];
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    i += 1;
    let value_start = i;
    while i < bytes.len() && bytes[i] != quote {
        i += 1;
    }
    let value = decode_entities(&tag[value_start..i.min(bytes.len())]);
    if i < bytes.len() {
        i += 1;
    }
    Some((value, i))
}

/// Skip inter-attribute padding (whitespace and `/`).
fn skip_tag_gap(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
        i += 1;
    }
    i
}

/// Skip whitespace between an attribute name, `=`, and its value.
fn skip_tag_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn is_tag_boundary(byte: Option<u8>) -> bool {
    matches!(
        byte,
        Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/') | Some(b'>') | None
    )
}

/// `(entry attributes, first wc-status attributes)` of one `<target>`
/// chunk of `svn --xml status` output.
type StatusEntry = (BTreeMap<String, String>, Option<BTreeMap<String, String>>);

fn find_status_entries(chunk: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut rest = chunk;
    while let Some(start) = rest.find("<entry") {
        if !is_tag_boundary(rest.as_bytes().get(start + 6).copied()) {
            rest = &rest[start + 6..];
            continue;
        }
        let Some(rel) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start + 1..start + rel];
        let entry_attrs = parse_tag_attrs(tag);
        if tag.trim_end().ends_with('/') {
            entries.push((entry_attrs, None));
            rest = &rest[start + rel + 1..];
            continue;
        }
        let inner_start = start + rel + 1;
        let Some(end_rel) = rest[inner_start..].find("</entry>") else {
            break;
        };
        let inner = &rest[inner_start..inner_start + end_rel];
        entries.push((entry_attrs, find_first_wc_status(inner)));
        rest = &rest[inner_start + end_rel + "</entry>".len()..];
    }
    entries
}

fn find_first_wc_status(inner: &str) -> Option<BTreeMap<String, String>> {
    let open = "<wc-status";
    let mut rest = inner;
    loop {
        let start = rest.find(open)?;
        if is_tag_boundary(rest.as_bytes().get(start + open.len()).copied()) {
            let rel = rest[start..].find('>')?;
            return Some(parse_tag_attrs(&rest[start + 1..start + rel]));
        }
        rest = &rest[start + open.len()..];
    }
}

/// Status backslashes normalize to slashes
/// (`ArcanistSubversionAPI.php:86-91`). Divergence: PHP only does this on
/// Windows.
#[must_use]
pub fn normalize_status_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// `getSVNStatus` result (`ArcanistSubversionAPI.php:69-149`): status flags
/// plus per-path base revisions (`getSVNBaseRevisions`, `:57-62`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SvnStatus {
    /// Map from path to `FLAG_*` bitmask.
    pub files: StatusMap,
    /// Map from path to `wc-status` revision.
    pub base_revisions: BTreeMap<String, String>,
}

/// Parse `svn --xml status` output
/// (`ArcanistSubversionAPI.php:69-149`). Base revisions reset per
/// `<target>`, like PHP (`:84`); with no targets both maps stay empty,
/// where PHP leaves revisions null.
pub fn parse_svn_status_xml(stdout: &str) -> Result<SvnStatus, String> {
    let mut files = StatusMap::new();
    let mut base_revisions = BTreeMap::new();
    let mut externals: Vec<String> = Vec::new();
    let mut chunks = stdout.split("<target");
    chunks.next();
    for chunk in chunks {
        base_revisions = BTreeMap::new();
        for (entry, wc) in find_status_entries(chunk) {
            let path = normalize_status_path(entry.get("path").map(String::as_str).unwrap_or(""));
            let wc = wc.unwrap_or_default();
            let props = wc.get("props").map(String::as_str).unwrap_or("");
            let item = wc.get("item").map(String::as_str).unwrap_or("");
            let base = wc.get("revision").map(String::as_str).unwrap_or("");
            base_revisions.insert(path.clone(), base.to_string());
            let mut mask = match props {
                "none" | "normal" => 0,
                "modified" => FLAG_MODIFIED,
                _ => return Err(format!("Unrecognized property status '{props}'.")),
            };
            mask |= parse_svn_status_item(item)?;
            if item == "external" {
                externals.push(path.clone());
            }
            if !wc
                .get("tree-conflicted")
                .map(String::as_str)
                .unwrap_or("")
                .is_empty()
            {
                mask |= FLAG_CONFLICT;
            }
            files.insert(path, mask);
        }
    }
    apply_externals(&mut files, &externals);
    Ok(SvnStatus {
        files,
        base_revisions,
    })
}

/// Flag every path under an external dir
/// (`ArcanistSubversionAPI.php:128-134`), including the external itself,
/// like the PHP prefix comparison.
pub fn apply_externals(files: &mut StatusMap, externals: &[String]) {
    let paths: Vec<String> = files.keys().cloned().collect();
    for path in paths {
        for external in externals {
            if format!("{path}/").starts_with(&format!("{external}/")) {
                if let Some(mask) = files.get_mut(&path) {
                    *mask |= FLAG_EXTERNALS;
                }
                break;
            }
        }
    }
}

/// Drop external-flagged paths unless requested (`getSVNStatus`,
/// `ArcanistSubversionAPI.php:139-148`).
#[must_use]
pub fn filter_externals(status: &StatusMap, with_externals: bool) -> StatusMap {
    if with_externals {
        return status.clone();
    }
    status
        .iter()
        .filter(|(_, mask)| *mask & FLAG_EXTERNALS == 0)
        .map(|(path, mask)| (path.clone(), *mask))
        .collect()
}

/// `status` argv with `limitStatusToPaths` paths appended
/// (`ArcanistSubversionAPI.php:64-77`).
#[must_use]
pub fn status_argv(paths: &[&str]) -> Vec<String> {
    let mut argv = vec!["--xml".to_string(), "status".to_string()];
    argv.extend(paths.iter().map(|path| (*path).to_string()));
    argv
}

/// Parse `svn --xml diff --summarize` output (`getChangedFiles`,
/// `ArcanistSubversionAPI.php:540-551`). Entry text is not trimmed, like
/// PHP.
pub fn parse_svn_diff_summarize_xml(stdout: &str) -> Result<StatusMap, String> {
    let mut result = StatusMap::new();
    let mut rest = stdout;
    while let Some(start) = rest.find("<path") {
        if !is_tag_boundary(rest.as_bytes().get(start + 5).copied()) {
            rest = &rest[start + 5..];
            continue;
        }
        let Some(rel) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start + 1..start + rel];
        let attrs = parse_tag_attrs(tag);
        let item = attrs.get("item").map(String::as_str).unwrap_or("");
        if tag.trim_end().ends_with('/') {
            result.insert(String::new(), parse_svn_status_item(item)?);
            rest = &rest[start + rel + 1..];
            continue;
        }
        let inner_start = start + rel + 1;
        let Some(end_rel) = rest[inner_start..].find("</path>") else {
            break;
        };
        let text = decode_entities(&rest[inner_start..inner_start + end_rel]);
        result.insert(text, parse_svn_status_item(item)?);
        rest = &rest[inner_start + end_rel + "</path>".len()..];
    }
    Ok(result)
}

/// Greedy `url@revision` split (`getChangedFiles`,
/// `ArcanistSubversionAPI.php:534-538`): the split happens at the last
/// `@`, and unmatched input keeps an empty URL.
#[must_use]
pub fn split_url_at_revision(since: &str) -> (String, String) {
    match since.rfind('@') {
        Some(index) => (since[..index].to_string(), since[index + 1..].to_string()),
        None => (String::new(), since.to_string()),
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

/// Parse `Last Changed Date` values such as
/// `2026-10-13 09:00:00 +0000` (`getSVNInfo`,
/// `ArcanistSubversionAPI.php:363-365`, uses PHP `strtotime`).
#[must_use]
pub fn parse_svn_date(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let date = parts.next()?;
    let clock = parse_clock(parts.next()?)?;
    let zone = parse_zone(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    let (year, month, day) = parse_iso_date(date)?;
    Some(days_from_civil(year, month, day) * 86400 + clock - zone)
}

/// Parse a `YYYY-MM-DD` date with range-checked month and day.
fn parse_iso_date(date: &str) -> Option<(i64, i64, i64)> {
    let mut fields = date.split('-');
    let year: i64 = fields.next()?.parse().ok()?;
    let month: i64 = fields.next()?.parse().ok()?;
    let day: i64 = fields.next()?.parse().ok()?;
    if fields.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Single-token `svn info` keys (`getSVNInfo`,
/// `ArcanistSubversionAPI.php:342-353`).
const SIMPLE_INFO_KEYS: &[&str] = &[
    "URL",
    "Revision",
    "Last Changed Author",
    "Last Changed Rev",
    "Copied From URL",
    "Copied From Rev",
    "Repository Root",
    "Repository UUID",
    "Node Kind",
];

fn parse_info_date_value(rest: &str) -> Option<String> {
    // `/(.+) \(.+\)$/`: the value runs to the last ` (...)` group.
    if !rest.ends_with(')') {
        return None;
    }
    let open = rest.rfind(" (")?;
    let value = &rest[..open];
    let inner = &rest[open + 2..rest.len() - 1];
    if value.is_empty() || inner.is_empty() {
        return None;
    }
    Some(value.to_string())
}

/// Parse `svn info` output into verbatim keys (`getSVNInfo`,
/// `ArcanistSubversionAPI.php:326-375`). Like PHP, the first match per key
/// wins. The `Last Changed Date` keeps its raw value; use
/// [`parse_svn_date`] for the timestamp PHP `strtotime` computes.
pub fn parse_svn_info(stdout: &str) -> Result<BTreeMap<String, String>, String> {
    let normalized = stdout.replace("\r\n", "\n");
    let mut result = BTreeMap::new();
    for line in normalized.split('\n') {
        let mut matched = false;
        for key in SIMPLE_INFO_KEYS {
            let prefix = format!("{key}: ");
            if let Some(rest) = line.strip_prefix(&prefix) {
                if !rest.is_empty() && !rest.chars().any(|c| c.is_whitespace()) {
                    result
                        .entry(key.to_string())
                        .or_insert_with(|| rest.to_string());
                }
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Last Changed Date: ")
            && let Some(value) = parse_info_date_value(rest)
        {
            result
                .entry("Last Changed Date".to_string())
                .or_insert(value);
        }
    }
    if result.is_empty() {
        return Err("Unable to parse SVN info.".to_string());
    }
    Ok(result)
}

/// `info` argv (`buildInfoFuture`, `ArcanistSubversionAPI.php:255-279`).
/// `/` runs `info .` in the root, working around the Subversion symlink bug
/// where `svn info` fails on a symlinked working copy; other paths gain a
/// trailing `@` so literal `@` names parse correctly.
#[must_use]
pub fn info_argv(root: &Path, path: &str) -> Vec<String> {
    if path == "/" {
        vec!["info".to_string(), ".".to_string()]
    } else {
        vec![
            "info".to_string(),
            format!("{}@", join_path(root, Some(path)).to_string_lossy()),
        ]
    }
}

/// `getSourceControlPath` (`ArcanistSubversionAPI.php:208-210`).
pub fn get_source_control_path_with(
    dir: &Path,
    svn: &SvnCommand,
) -> Result<Option<String>, String> {
    Ok(get_svn_info_with(dir, svn, "/")?.get("URL").cloned())
}

/// `getSourceControlPath` with the system binary.
pub fn get_source_control_path(dir: &Path) -> Result<Option<String>, String> {
    get_source_control_path_with(dir, &SvnCommand::system())
}

/// `getSourceControlBaseRevision`
/// (`ArcanistSubversionAPI.php:212-215`): `URL@revision-number`.
#[must_use]
pub fn get_source_control_base_revision(
    info: &BTreeMap<String, String>,
    base_number: Option<&str>,
) -> Option<String> {
    Some(format!("{}@{}", info.get("URL")?, base_number?))
}

/// `getSVNBaseRevisionNumber` with the `overrideSVNBaseRevisionNumber`
/// value passed by the caller (`ArcanistSubversionAPI.php:227-238`).
#[must_use]
pub fn svn_base_revision_number(
    info: &BTreeMap<String, String>,
    override_number: Option<&str>,
) -> Option<String> {
    if let Some(number) = override_number {
        return Some(number.to_string());
    }
    info.get("Revision").cloned()
}

/// `getBranchName` (`ArcanistSubversionAPI.php:240-249`): the URL below the
/// repository root, or `svn` when the URL escapes the root.
#[must_use]
pub fn branch_name_from_info(info: &BTreeMap<String, String>) -> String {
    let root = info
        .get("Repository Root")
        .map(String::as_str)
        .unwrap_or("");
    let url = info.get("URL").map(String::as_str).unwrap_or("");
    match url.strip_prefix(root) {
        Some(rest) => rest.to_string(),
        None => "svn".to_string(),
    }
}

/// `getRemoteURI` (`ArcanistSubversionAPI.php:251-253`). Missing keys
/// yield `None`, where PHP emits a notice.
pub fn get_remote_uri_with(dir: &Path, svn: &SvnCommand) -> Result<Option<String>, String> {
    Ok(get_svn_info_with(dir, svn, "/")?
        .get("Repository Root")
        .cloned())
}

/// `getRemoteURI` with the system binary.
pub fn get_remote_uri(dir: &Path) -> Result<Option<String>, String> {
    get_remote_uri_with(dir, &SvnCommand::system())
}

/// `getRepositoryUUID` (`ArcanistSubversionAPI.php:604-607`). Missing keys
/// yield `None`, where PHP emits a notice.
pub fn get_repository_uuid_with(dir: &Path, svn: &SvnCommand) -> Result<Option<String>, String> {
    Ok(get_svn_info_with(dir, svn, "/")?
        .get("Repository UUID")
        .cloned())
}

/// `getRepositoryUUID` with the system binary.
pub fn get_repository_uuid(dir: &Path) -> Result<Option<String>, String> {
    get_repository_uuid_with(dir, &SvnCommand::system())
}

fn is_php_numeric(text: &str) -> bool {
    let trimmed = text.trim();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[0] == b'+' || bytes[0] == b'-' {
        i = 1;
    }
    let (digits, after) = scan_php_mantissa(bytes, i);
    if digits == 0 {
        return false;
    }
    match scan_php_exponent(bytes, after) {
        Some(end) => end == bytes.len(),
        None => false,
    }
}

/// Scan the mantissa (`digits[.digits]`) at `start`: the digit count plus
/// the offset past it.
fn scan_php_mantissa(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut i = start;
    let mut digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        digits += 1;
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            digits += 1;
            i += 1;
        }
    }
    (digits, i)
}

/// Scan the optional `e[+-]digits` exponent at `start`: the offset past it,
/// or `None` for an `e` with no digits.
fn scan_php_exponent(bytes: &[u8], start: usize) -> Option<usize> {
    if start >= bytes.len() || (bytes[start] != b'e' && bytes[start] != b'E') {
        return Some(start);
    }
    let mut i = start + 1;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let mut exponent = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        exponent += 1;
        i += 1;
    }
    if exponent == 0 {
        return None;
    }
    Some(i)
}

/// `getCanonicalRevisionName` (`ArcanistSubversionAPI.php:217-225`):
/// numeric strings pass through, everything else yields `None`. The PHP
/// accuracy caveat is inherited.
#[must_use]
pub fn get_canonical_revision_name(text: &str) -> Option<String> {
    if is_php_numeric(text) {
        Some(text.to_string())
    } else {
        None
    }
}

/// `diff` argv (`buildDiffFuture`, `ArcanistSubversionAPI.php:281-313`).
/// `Some` diff command is the Unix shape with `binary_safe_diff.sh`;
/// `None` is the Windows shape. `--depth empty` keeps directory diffs from
/// recursing; the extra T5555 filtering PHP mentions (`:291-293`) happens
/// in diff parsing, which is out of scope for this API port.
#[must_use]
pub fn diff_argv(path: &str, context: u32, diff_cmd: Option<&str>) -> Vec<String> {
    match diff_cmd {
        None => vec![
            "diff".to_string(),
            "--depth".to_string(),
            "empty".to_string(),
            path.to_string(),
        ],
        Some(cmd) => vec![
            "diff".to_string(),
            "--depth".to_string(),
            "empty".to_string(),
            "--diff-cmd".to_string(),
            cmd.to_string(),
            "-x".to_string(),
            format!("-U{context}"),
            path.to_string(),
        ],
    }
}

/// `binary_safe_diff.sh` next to the arcanist checkout
/// (`ArcanistSubversionAPI.php:304-305`). The `..` segment stays literal,
/// which spawns identically.
#[must_use]
pub fn default_diff_cmd(arcanist_root: &Path) -> PathBuf {
    arcanist_root
        .join("..")
        .join("scripts")
        .join("repository")
        .join("binary_safe_diff.sh")
}

/// Diff behavior inputs for [`get_raw_diff_text_with`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvnDiffConfig<'a> {
    /// Unified context lines (`getDiffLinesOfContext`).
    pub context: u32,
    /// `--diff-cmd` script, or `None` for the Windows argv shape.
    pub diff_cmd: Option<&'a str>,
    /// Plain unified-`diff` program for synthetic additions (`diff`).
    pub diff_program: &'a str,
    /// Fixed leading arguments for `diff_program` (fake-binary seam).
    pub diff_prefix: &'a [&'a str],
}

impl<'a> SvnDiffConfig<'a> {
    /// Unix shape with `binary_safe_diff.sh` and system `diff`.
    #[must_use]
    pub fn unix(context: u32, diff_cmd: &'a str) -> Self {
        Self {
            context,
            diff_cmd: Some(diff_cmd),
            diff_program: "diff",
            diff_prefix: &[],
        }
    }

    /// Windows shape with system `diff` for synthetic additions.
    #[must_use]
    pub fn windows(context: u32) -> Self {
        Self {
            context,
            diff_cmd: None,
            diff_program: "diff",
            diff_prefix: &[],
        }
    }
}

impl Default for SvnDiffConfig<'_> {
    fn default() -> Self {
        Self {
            context: 0x7FFF,
            diff_cmd: None,
            diff_program: "diff",
            diff_prefix: &[],
        }
    }
}

fn parse_svn_blame_line(line: &str) -> Option<(String, String)> {
    // `/^\s*(\d+)\s+(\S+)/` (`ArcanistSubversionAPI.php:574`).
    let rest = line.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &rest[digits.len()..];
    if !rest.chars().next().is_some_and(|c| c.is_whitespace()) {
        return None;
    }
    let author: String = rest
        .trim_start()
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    if author.is_empty() {
        return None;
    }
    Some((digits, author))
}

/// Parse `svn blame` output into `(author, revision)` pairs (`getBlame`,
/// `ArcanistSubversionAPI.php:561-583`). Empty output is an empty file.
pub fn parse_svn_blame(stdout: &str) -> Result<Vec<(String, String)>, String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut blame = Vec::new();
    for line in trimmed.split('\n') {
        let Some((revision, author)) = parse_svn_blame_line(line) else {
            return Err(format!("Bad blame? `{line}'"));
        };
        blame.push((author, revision));
    }
    Ok(blame)
}

/// `filterFiles` (`ArcanistSubversionAPI.php:553-559`): drop empty paths
/// and directories (trailing slash; SVN uses `/` even on Windows).
#[must_use]
pub fn filter_files(path: &str) -> Option<String> {
    if path.is_empty() || path.ends_with('/') {
        None
    } else {
        Some(path.to_string())
    }
}

/// `escapeFileNameForSVN` (`ArcanistSubversionAPI.php:673-686`): names
/// containing `@` gain a trailing `@`. The `.` special case from the
/// comment is not implemented in PHP either.
#[must_use]
pub fn escape_file_name_for_svn(file: &str) -> String {
    if file.contains('@') {
        format!("{file}@")
    } else {
        file.to_string()
    }
}

/// `escapeFileNamesForSVN` (`ArcanistSubversionAPI.php:666-671`).
#[must_use]
pub fn escape_file_names_for_svn(files: &[&str]) -> Vec<String> {
    files
        .iter()
        .map(|file| escape_file_name_for_svn(file))
        .collect()
}

/// Binary filename extensions (`getRawDiffText`,
/// `ArcanistSubversionAPI.php:403`): `/\.(gif|png|jpe?g|swf|pdf|ico)$/i`.
#[must_use]
pub fn is_binary_filename(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["gif", "png", "jpg", "jpeg", "swf", "pdf", "ico"]
        .iter()
        .any(|ext| {
            lower.len() > ext.len()
                && lower.ends_with(ext)
                && lower.as_bytes()[lower.len() - ext.len() - 1] == b'.'
        })
}

/// Binary-file diff body (`getRawDiffText` and
/// `buildSyntheticAdditionDiff`, `ArcanistSubversionAPI.php:407-414` and
/// `:460-467`).
#[must_use]
pub fn binary_diff_text(path: &str) -> String {
    format!(
        "Index: {path}\n===================================================================\nCannot display: file marked as a binary type.\nsvn:mime-type = application/octet-stream\n\n"
    )
}

/// `buildSyntheticUnchangedDiff` body
/// (`ArcanistSubversionAPI.php:497-523`): every source line gains a leading
/// space, and the hunk counts `newline` segments (so trailing newlines add
/// an empty line, like PHP `explode`).
#[must_use]
pub fn synthetic_unchanged_diff_bytes(path: &str, data: &[u8]) -> Vec<u8> {
    let parts: Vec<&[u8]> = data.split(|byte| *byte == b'\n').collect();
    let mut body: Vec<u8> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            body.push(b'\n');
        }
        body.push(b' ');
        body.extend_from_slice(part);
    }
    let mut out = format!(
        "Index: {path}\n===================================================================\n--- {path} (synthetic)\n+++ {path} (synthetic)\n@@ -1,{} +1,{} @@\n",
        parts.len(),
        parts.len()
    )
    .into_bytes();
    out.extend(body);
    out.extend(b"\n\n");
    out
}

/// Strip the source-control path prefix from a copy source
/// (`buildSyntheticAdditionDiff`, `ArcanistSubversionAPI.php:479`).
/// `str_replace` semantics: every occurrence is stripped.
#[must_use]
pub fn strip_source_prefix(source: &str, source_control_path: &str) -> String {
    source.replace(&format!("{source_control_path}/"), "")
}

/// `buildSyntheticAdditionDiff` body
/// (`ArcanistSubversionAPI.php:486-491`).
#[must_use]
pub fn addition_diff_bytes(path: &str, diff: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "Index: {path}\n===================================================================\n"
    )
    .into_bytes();
    out.extend_from_slice(diff);
    out.extend(b"\n\n");
    out
}

/// `getSVNStatus` (`ArcanistSubversionAPI.php:69-149`).
pub fn get_svn_status_with(
    dir: &Path,
    svn: &SvnCommand,
    paths: &[&str],
    with_externals: bool,
) -> Result<SvnStatus, String> {
    let argv = status_argv(paths);
    let stdout = stdout_text_with(dir, svn, &argv_refs(&argv))?;
    let mut status = parse_svn_status_xml(&stdout)?;
    status.files = filter_externals(&status.files, with_externals);
    Ok(status)
}

/// `getSVNStatus` with the system binary.
pub fn get_svn_status(
    dir: &Path,
    paths: &[&str],
    with_externals: bool,
) -> Result<SvnStatus, String> {
    get_svn_status_with(dir, &SvnCommand::system(), paths, with_externals)
}

/// `buildUncommittedStatus` (`ArcanistSubversionAPI.php:53-55`): the status
/// files without externals.
pub fn get_uncommitted_status_with(
    dir: &Path,
    svn: &SvnCommand,
    paths: &[&str],
) -> Result<StatusMap, String> {
    Ok(get_svn_status_with(dir, svn, paths, false)?.files)
}

/// `buildUncommittedStatus` with the system binary.
pub fn get_uncommitted_status(dir: &Path, paths: &[&str]) -> Result<StatusMap, String> {
    get_uncommitted_status_with(dir, &SvnCommand::system(), paths)
}

/// `getSVNInfo` (`ArcanistSubversionAPI.php:326-375`).
pub fn get_svn_info_with(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
) -> Result<BTreeMap<String, String>, String> {
    let argv = info_argv(dir, path);
    let out = manual_with(dir, svn, &argv_refs(&argv))?;
    if out.code != 0 {
        return Err(format!(
            "Error #{} executing svn info against '{path}'.",
            out.code
        ));
    }
    parse_svn_info(&out.stdout)
}

/// `getSVNInfo` with the system binary.
pub fn get_svn_info(dir: &Path, path: &str) -> Result<BTreeMap<String, String>, String> {
    get_svn_info_with(dir, &SvnCommand::system(), path)
}

/// `getSVNProperty` (`ArcanistSubversionAPI.php:200-206`).
pub fn get_svn_property_with(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
    property: &str,
) -> Result<String, String> {
    let target = format!("{}@", join_path(dir, Some(path)).to_string_lossy());
    Ok(stdout_text_with(dir, svn, &["propget", property, &target])?
        .trim()
        .to_string())
}

/// `getSVNProperty` with the system binary.
pub fn get_svn_property(dir: &Path, path: &str, property: &str) -> Result<String, String> {
    get_svn_property_with(dir, &SvnCommand::system(), path, property)
}

/// `getWorkingCopyRevision` (`ArcanistSubversionAPI.php:633-635`): the base
/// revision `URL@number`.
pub fn get_working_copy_revision_with(
    dir: &Path,
    svn: &SvnCommand,
    base_override: Option<&str>,
) -> Result<Option<String>, String> {
    let info = get_svn_info_with(dir, svn, "/")?;
    let number = svn_base_revision_number(&info, base_override);
    Ok(get_source_control_base_revision(&info, number.as_deref()))
}

/// `getWorkingCopyRevision` with the system binary.
pub fn get_working_copy_revision(
    dir: &Path,
    base_override: Option<&str>,
) -> Result<Option<String>, String> {
    get_working_copy_revision_with(dir, &SvnCommand::system(), base_override)
}

/// `getBranchName` (`ArcanistSubversionAPI.php:240-249`).
pub fn get_branch_name_with(dir: &Path, svn: &SvnCommand) -> Result<String, String> {
    Ok(branch_name_from_info(&get_svn_info_with(dir, svn, "/")?))
}

/// `getBranchName` with the system binary.
pub fn get_branch_name(dir: &Path) -> Result<String, String> {
    get_branch_name_with(dir, &SvnCommand::system())
}

/// `buildSyntheticUnchangedDiff`
/// (`ArcanistSubversionAPI.php:497-523`): directories and missing files
/// yield `None`. Unreadable files also yield `None`, where PHP throws.
pub fn build_synthetic_unchanged_diff(dir: &Path, path: &str) -> Option<Vec<u8>> {
    let full = join_path(dir, Some(path));
    if full.is_dir() {
        return None;
    }
    let data = std::fs::read(&full).ok()?;
    Some(synthetic_unchanged_diff_bytes(path, &data))
}

fn write_temp_file(tag: &str, data: &[u8]) -> Result<PathBuf, String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path =
        std::env::temp_dir().join(format!("arc-rs-svn-{tag}-{}-{n}.tmp", std::process::id()));
    std::fs::write(&path, data).map_err(|e| format!("failed to write temporary file: {e}"))?;
    Ok(path)
}

struct RawOutput {
    code: i32,
    stdout: Vec<u8>,
}

/// Run a plain program (the system unified `diff`) in `dir`.
fn run_program(
    dir: &Path,
    program: &str,
    prefix: &[&str],
    args: &[&str],
) -> Result<RawOutput, String> {
    let out = crate::proc_env::command(program)
        .current_dir(dir)
        .args(prefix)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {program}: {e}"))?;
    Ok(RawOutput {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
    })
}

/// `buildSyntheticAdditionDiff` (`ArcanistSubversionAPI.php:453-495`):
/// directories yield `None`, octet-stream files yield the binary body,
/// otherwise the copy source is diffed against the working file with plain
/// `diff`. Exit code 1 (differences) yields the synthetic diff; any other
/// code, including diff crashes, yields the unchanged diff, like PHP.
pub fn build_synthetic_addition_diff_with(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
    source: &str,
    rev: &str,
    source_control_path: &str,
    config: &SvnDiffConfig,
) -> Result<Option<Vec<u8>>, String> {
    let full = join_path(dir, Some(path));
    if full.is_dir() {
        return Ok(None);
    }
    if get_svn_property_with(dir, svn, path, "svn:mime-type")? == "application/octet-stream" {
        return Ok(Some(binary_diff_text(path).into_bytes()));
    }
    let data = std::fs::read(&full).map_err(|e| format!("failed to read {path}: {e}"))?;
    let target = format!("{source}@{rev}");
    let orig = output_with(dir, svn, &["cat", &target])?;
    if !orig.status.success() {
        return Err(format!("{} cat {target} failed", svn.program));
    }
    let src_tmp = write_temp_file("svndiff-src", &orig.stdout)?;
    let dst_tmp = write_temp_file("svndiff-dst", &data)?;
    let relative = strip_source_prefix(source, source_control_path);
    let src = src_tmp.to_string_lossy().to_string();
    let dst = dst_tmp.to_string_lossy().to_string();
    let diff = run_program(
        dir,
        config.diff_program,
        config.diff_prefix,
        &[
            "-L",
            &format!("a/{relative}"),
            "-L",
            &format!("b/{path}"),
            &format!("-U{}", config.context),
            &src,
            &dst,
        ],
    )?;
    std::fs::remove_file(&src_tmp).ok();
    std::fs::remove_file(&dst_tmp).ok();
    if diff.code == 1 {
        Ok(Some(addition_diff_bytes(path, &diff.stdout)))
    } else {
        Ok(build_synthetic_unchanged_diff(dir, path))
    }
}

/// `buildSyntheticAdditionDiff` with the system binary and default config.
pub fn build_synthetic_addition_diff(
    dir: &Path,
    path: &str,
    source: &str,
    rev: &str,
    source_control_path: &str,
    context: u32,
) -> Result<Option<Vec<u8>>, String> {
    build_synthetic_addition_diff_with(
        dir,
        &SvnCommand::system(),
        path,
        source,
        rev,
        source_control_path,
        &SvnDiffConfig {
            context,
            diff_cmd: None,
            diff_program: "diff",
            diff_prefix: &[],
        },
    )
}

/// `getRawDiffText` (`ArcanistSubversionAPI.php:378-451`): unknown paths
/// yield `None`; copies yield synthetic addition diffs; binary filenames
/// gain the octet-stream mime type (deleted ones yield the binary body); GNU diff
/// exit code 2 via `svn diff` exit code 1 with the exact stderr line is
/// tolerated; empty diffs yield synthetic unchanged diffs.
pub fn get_raw_diff_text_with(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
    status: &StatusMap,
    config: &SvnDiffConfig,
) -> Result<Option<Vec<u8>>, String> {
    let Some(flags) = status.get(path).copied() else {
        return Ok(None);
    };
    if let Some(diff) = copied_addition_diff(dir, svn, path, flags, config)? {
        return Ok(diff);
    }
    if let Some(diff) = binary_filename_diff(dir, svn, path, flags)? {
        return Ok(Some(diff));
    }
    let argv = diff_argv(path, config.context, config.diff_cmd);
    let out = output_with(dir, svn, &argv_refs(&argv))?;
    let code = out.status.code().unwrap_or(-1);
    if code != 0 && out.stderr != b"svn: 'diff' returned 2\n" {
        return Err(format!(
            "svn diff returned unexpected error code: {code}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if code == 0 && out.stdout.is_empty() {
        return Ok(build_synthetic_unchanged_diff(dir, path));
    }
    Ok(Some(out.stdout))
}

/// Synthetic addition diff for an added path copied from elsewhere;
/// `None` when the path is not an added copy.
fn copied_addition_diff(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
    flags: u32,
    config: &SvnDiffConfig,
) -> Result<Option<Option<Vec<u8>>>, String> {
    if flags & FLAG_ADDED == 0 {
        return Ok(None);
    }
    let info = get_svn_info_with(dir, svn, path)?;
    if let Some(source) = info.get("Copied From URL")
        && !source.is_empty()
    {
        let rev = info.get("Copied From Rev").cloned().unwrap_or_default();
        let root_info = get_svn_info_with(dir, svn, "/")?;
        let source_control_path = root_info.get("URL").cloned().unwrap_or_default();
        let diff = build_synthetic_addition_diff_with(
            dir,
            svn,
            path,
            source,
            &rev,
            &source_control_path,
            config,
        )?;
        return Ok(Some(diff));
    }
    Ok(None)
}

/// Binary-filename handling: deleted paths yield the binary body;
/// otherwise the octet-stream mime type is enforced. `None` means "no
/// early diff, run `svn diff`".
fn binary_filename_diff(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
    flags: u32,
) -> Result<Option<Vec<u8>>, String> {
    if !is_binary_filename(path) {
        return Ok(None);
    }
    if flags & FLAG_DELETED != 0 {
        return Ok(Some(binary_diff_text(path).into_bytes()));
    }
    let mime = get_svn_property_with(dir, svn, path, "svn:mime-type")?;
    if mime != "application/octet-stream" {
        let absolute = join_path(dir, Some(path)).to_string_lossy().to_string();
        stdout_text_with(
            dir,
            svn,
            &[
                "propset",
                "svn:mime-type",
                "application/octet-stream",
                &escape_file_name_for_svn(&absolute),
            ],
        )?;
    }
    Ok(None)
}

/// `getRawDiffText` with the system binary and default config.
pub fn get_raw_diff_text(
    dir: &Path,
    path: &str,
    status: &StatusMap,
    context: u32,
) -> Result<Option<Vec<u8>>, String> {
    get_raw_diff_text_with(
        dir,
        &SvnCommand::system(),
        path,
        status,
        &SvnDiffConfig {
            context,
            diff_cmd: None,
            diff_program: "diff",
            diff_prefix: &[],
        },
    )
}

/// `getAllFiles` (`ArcanistSubversionAPI.php:525-531`).
pub fn get_all_files_with(dir: &Path, svn: &SvnCommand) -> Result<Vec<String>, String> {
    let stdout = stdout_text_with(dir, svn, &["list", "-R"])?;
    Ok(stdout.lines().filter_map(filter_files).collect())
}

/// `getAllFiles` with the system binary.
pub fn get_all_files(dir: &Path) -> Result<Vec<String>, String> {
    get_all_files_with(dir, &SvnCommand::system())
}

/// `getChangedFiles` (`ArcanistSubversionAPI.php:533-551`). The URL argument
/// stays in argv even when empty, like PHP.
pub fn get_changed_files_with(
    dir: &Path,
    svn: &SvnCommand,
    since_commit: &str,
) -> Result<StatusMap, String> {
    let (url, revision) = split_url_at_revision(since_commit);
    let range = format!("{revision}:HEAD");
    let stdout = stdout_text_with(
        dir,
        svn,
        &["--xml", "diff", "--revision", &range, "--summarize", &url],
    )?;
    parse_svn_diff_summarize_xml(&stdout)
}

/// `getChangedFiles` with the system binary.
pub fn get_changed_files(dir: &Path, since_commit: &str) -> Result<StatusMap, String> {
    get_changed_files_with(dir, &SvnCommand::system(), since_commit)
}

/// `getBlame` (`ArcanistSubversionAPI.php:561-583`).
pub fn get_blame_with(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
) -> Result<Vec<(String, String)>, String> {
    parse_svn_blame(&stdout_text_with(dir, svn, &["blame", path])?)
}

/// `getBlame` with the system binary.
pub fn get_blame(dir: &Path, path: &str) -> Result<Vec<(String, String)>, String> {
    get_blame_with(dir, &SvnCommand::system(), path)
}

/// `getOriginalFileData` (`ArcanistSubversionAPI.php:585-594`): failures
/// mean the original file does not exist. The `@` suffix applies to the
/// path as given, like PHP.
pub fn get_original_file_data_with(
    dir: &Path,
    svn: &SvnCommand,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let target = format!("{path}@");
    let out = output_with(dir, svn, &["cat", &target])?;
    if out.status.success() {
        Ok(Some(out.stdout))
    } else {
        Ok(None)
    }
}

/// `getOriginalFileData` with the system binary.
pub fn get_original_file_data(dir: &Path, path: &str) -> Result<Option<Vec<u8>>, String> {
    get_original_file_data_with(dir, &SvnCommand::system(), path)
}

/// `getCurrentFileData` (`ArcanistSubversionAPI.php:596-602`).
#[must_use]
pub fn get_current_file_data(dir: &Path, path: &str) -> Option<Vec<u8>> {
    let full = join_path(dir, Some(path));
    if full.exists() {
        std::fs::read(full).ok()
    } else {
        None
    }
}

/// `getBulkOriginalFileData` base loop
/// (`ArcanistRepositoryAPI.php:303-310`).
pub fn get_bulk_original_file_data_with(
    dir: &Path,
    svn: &SvnCommand,
    paths: &[&str],
) -> Result<BTreeMap<String, Option<Vec<u8>>>, String> {
    let mut data = BTreeMap::new();
    for path in paths {
        data.insert(
            (*path).to_string(),
            get_original_file_data_with(dir, svn, path)?,
        );
    }
    Ok(data)
}

/// `getBulkOriginalFileData` with the system binary.
pub fn get_bulk_original_file_data(
    dir: &Path,
    paths: &[&str],
) -> Result<BTreeMap<String, Option<Vec<u8>>>, String> {
    get_bulk_original_file_data_with(dir, &SvnCommand::system(), paths)
}

/// `getBulkCurrentFileData` base loop
/// (`ArcanistRepositoryAPI.php:317-324`).
#[must_use]
pub fn get_bulk_current_file_data(dir: &Path, paths: &[&str]) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut data = BTreeMap::new();
    for path in paths {
        data.insert((*path).to_string(), get_current_file_data(dir, path));
    }
    data
}

/// `addToCommit` (`ArcanistSubversionAPI.php:185-198`): existing paths are
/// added, missing paths are deleted.
pub fn add_to_commit_with(dir: &Path, svn: &SvnCommand, paths: &[&str]) -> Result<(), String> {
    let mut add: Vec<&str> = Vec::new();
    let mut delete: Vec<&str> = Vec::new();
    for path in paths {
        if join_path(dir, Some(path)).exists() {
            add.push(path);
        } else {
            delete.push(path);
        }
    }
    if !add.is_empty() {
        let mut args: Vec<&str> = vec!["add", "--"];
        args.extend(add.iter().copied());
        stdout_text_with(dir, svn, &args)?;
    }
    if !delete.is_empty() {
        let mut args: Vec<&str> = vec!["delete", "--"];
        args.extend(delete.iter().copied());
        stdout_text_with(dir, svn, &args)?;
    }
    Ok(())
}

/// `addToCommit` with the system binary.
pub fn add_to_commit(dir: &Path, paths: &[&str]) -> Result<(), String> {
    add_to_commit_with(dir, &SvnCommand::system(), paths)
}

/// `updateWorkingCopy` (`ArcanistSubversionAPI.php:662-664`).
pub fn update_working_copy_with(dir: &Path, svn: &SvnCommand) -> Result<(), String> {
    stdout_text_with(dir, svn, &["up"])?;
    Ok(())
}

/// `updateWorkingCopy` with the system binary.
pub fn update_working_copy(dir: &Path) -> Result<(), String> {
    update_working_copy_with(dir, &SvnCommand::system())
}

/// Pick the `svn commit` `LANG` value from `locale -a` output:
/// `en_US.UTF-8` when listed, else `en_US.utf8`.
///
/// PHP: `ArcanistCommitWorkflow::getSVNLangEnvVar`
/// (`src/workflow/ArcanistCommitWorkflow.php:297-310`).
#[must_use]
pub fn select_svn_locale(locales_output: &str) -> &'static str {
    if locales_output.lines().any(|line| line == "en_US.UTF-8") {
        "en_US.UTF-8"
    } else {
        "en_US.utf8"
    }
}

/// Probe `locale -a` for the `svn commit` `LANG` value, defaulting to
/// `en_US.utf8` when the probe fails. PHP swallows probe errors the same
/// way; Windows callers pass no locale at all (PHP skips `LANG` there).
pub fn svn_lang_env_var() -> &'static str {
    let output = crate::proc_env::command("locale").arg("-a").output();
    match output {
        Ok(out) if out.status.success() => select_svn_locale(&String::from_utf8_lossy(&out.stdout)),
        _ => "en_US.utf8",
    }
}

/// Failure message for `svn commit`: the bare PHP sentence
/// (`ArcanistCommitWorkflow.php:163` at ceb). The child streams live,
/// so its output is already on the console; nothing rides along here.
fn commit_failed_message() -> String {
    "Executing 'svn commit' failed!".to_string()
}

/// Commit `files` with `message`: `svn commit <files> --encoding utf-8
/// -F <tmp>`, with `LANG` set to `locale` when given.
///
/// PHP: the `svn commit` call in `ArcanistCommitWorkflow::run`
/// (`src/workflow/ArcanistCommitWorkflow.php:148-164`), which runs
/// `phutil_passthru` (live stdio). Both pipes stream live like PHP's
/// (T1153 CORE-65): the `Sending ... Committed revision N` progress
/// lands on the console during the commit, and the caller gets only
/// the status.
pub fn commit_with_locale(
    dir: &Path,
    svn: &SvnCommand,
    files: &[&str],
    message: &str,
    locale: Option<&str>,
) -> Result<(), String> {
    let tmp = write_temp_file("svncommit", message.as_bytes())?;
    let tmp_name = tmp.to_string_lossy().into_owned();
    let mut argv: Vec<&str> = Vec::with_capacity(files.len() + 5);
    argv.push("commit");
    argv.extend(files.iter().copied());
    argv.extend(["--encoding", "utf-8", "-F", tmp_name.as_str()]);
    let mut cmd = command_for(dir, svn, &argv);
    if let Some(locale) = locale {
        cmd.env("LANG", locale);
    }
    // T1153 CORE-65: PHP's `phutil_passthru` streams both pipes live,
    // so both inherit the terminal here; nothing is captured.
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    let out = spawn_output(&mut cmd).map_err(|e| format!("failed to run {}: {e}", svn.program))?;
    std::fs::remove_file(&tmp).ok();
    if !out.status.success() {
        return Err(commit_failed_message());
    }
    Ok(())
}

/// Commit `files` with `message`, probing `LANG` on non-Windows.
///
/// PHP skips `LANG` on Windows (`ArcanistCommitWorkflow.php:155`).
pub fn commit_with(
    dir: &Path,
    svn: &SvnCommand,
    files: &[&str],
    message: &str,
) -> Result<(), String> {
    #[cfg(windows)]
    let locale = None;
    #[cfg(not(windows))]
    let locale = Some(svn_lang_env_var());
    commit_with_locale(dir, svn, files, message, locale)
}

/// Commit `files` with `message` using the system binary.
pub fn commit(dir: &Path, files: &[&str], message: &str) -> Result<(), String> {
    commit_with(dir, &SvnCommand::system(), files, message)
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
#[path = "svn_tests.rs"]
mod svn_tests;
