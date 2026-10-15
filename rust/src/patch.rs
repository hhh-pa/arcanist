//! `arc patch`: apply a Differential change to the working copy.
//!
//! Full-state port of `src/workflow/ArcanistPatchWorkflow.php`
//! (arguments, source selection, the git/hg/svn flows) plus the pieces of
//! `src/workflow/ArcanistWorkflow.php` it uses (`requireCleanWorkingCopy`,
//! bundle loaders) and the arcbundle reader
//! (`src/parser/ArcanistBundle.php`).
//!
//! Flag names, source-selection errors, sanity/dependency prompts, branch
//! and bookmark handling, and the per-VCS command sequences are verbatim
//! from PHP; each item cites its PHP source lines. Argument parsing
//! mirrors the D1 pattern (`diff.rs`): `--` ends flags, positionals feed
//! the `*` wildcard, `--bool=value` splices back, `--help`
//! short-circuits, conflicts render `: 1` for boolean reasons, and
//! unsupported VCS options fail like
//! `Option '--nocommit' is not supported under svn.`
//!
//! Divergences from PHP (all deliberate, all documented):
//! - `--conduit-uri` is threaded through every command (like the diff
//!   port); PHP reads endpoint configuration instead.
//! - `--preview` prints the patch instead of applying it. PHP has no such
//!   flag; the port keeps it for dry runs and tests.
//! - Conduit bundles load hunks via `differential.getrawdiff` instead of
//!   rendering change dictionaries (`toGitPatch()`), but still query
//!   `differential.querydiffs` for the base revision, revision ID, and
//!   author metadata. The applied result is equivalent for text and
//!   binary changes (the server renders `GIT binary` patches); local
//!   binary rendering via `file.download` blobs is available through
//!   `fetch_file_blob()` plus `render_git_patch_with_blobs()`.
//! - Arcbundles reconstruct a git patch from `changes.json` plus `hunks/N`
//!   and `blobs/<phid>` members via the system `tar` binary (like PHP's
//!   `ExecFuture('tar ...')`). Text and `GIT binary` (binary/image,
//!   including multicopy decomposes) changes render; binary bodies use
//!   stored-zlib compression (PHP uses `gzcompress()`), which inflates
//!   identically for `git apply`.
//! - `--patch` text is pre-parsed like `ArcanistDiffParser::parseDiff()`
//!   (empty + header validation with byte-for-byte `didFailParse()`
//!   errors); valid patches pass through verbatim (PHP re-renders via
//!   `toGitPatch()`, equivalent for git patches). Hunk-body line counts
//!   and unified-to-git conversion are left to the VCS binaries.
//! - Non-UTF-8 `--encoding` values decode bytes through a Windows-1252
//!   table. PHP's `phutil_utf8_convert()` (`src/utils/utf8.php:683-733`)
//!   is a thin `mb_convert_encoding()` wrapper with no vendored table;
//!   the port's 256-entry table matches the Windows-1252 standard
//!   byte-for-byte (27 defined `0x80..0x9F` scalars, 5 undefined bytes
//!   to U+FFFD, all others to the identical Latin-1 scalar). Other
//!   source encodings still decode as Windows-1252 (PHP supports any
//!   `mbstring` encoding). UTF-8 input passes through. The
//!   repository-encoding lookup (`getRepositoryEncoding()`, UTF-8 by
//!   default) is not ported: conduit sources always decode as UTF-8
//!   unless `--encoding` says otherwise.
//! - Interactive editors are not ported: commit-message editing (patch
//!   commits, and the clean-check "create a commit" yes-path) fails with
//!   a guidance error naming `--nocommit` instead of opening `$EDITOR`.
//! - The clean-check amend detection (`calculateShouldAmend()`) is not
//!   ported: the prompt always offers "create a new commit", never
//!   "amend".
//! - `arc patch` outside a working copy fails with the established
//!   port message (`not inside a git, hg, or svn working copy`); the PHP
//!   runner error was not found in the vendored sources.
//! - `phutil_console_wrap()` line wrapping is not ported; prompt and
//!   banner text renders unwrapped.
//! - Prompt answers ending in `*` or `!` are accepted with the suffix
//!   stripped (see `land`); persisting is not ported.
//! - Revision text (including three-dot `...` revsets) passes verbatim
//!   to `hg`/`git`: PHP formats revisions with `%s`/`hgsprintf` and
//!   never interprets them (`getBranchName():224-226`,
//!   `getBookmarkName():265-267`, `createBranch():302-305,328-335`,
//!   `checkout %s --`, `cherry-pick -- %s`, `rebase --dest %s --rev %s`
//!   with `hgsprintf`, `getCanonicalRevisionName(hgsprintf('%s^', …))`),
//!   so the binary alone gives `...` meaning. The `dotencode`
//!   extension details are likewise untouched.
//! - Patch output is plain text on both sides: PHP's patch workflow
//!   never calls `json_encode` (0 hits in `ArcanistPatchWorkflow.php`;
//!   the 2 hits in `ArcanistBundle.php` are `writeToDisk()` export
//!   only), and this port has no JSON emit sites on the patch path
//!   (only `from_slice`/`from_str` parsing and conduit `json!` params),
//!   so no `\/` escaping applies anywhere on the patch path.
//! - Dependency child runs execute in-process with
//!   `--force --skip-dependencies --nobranch` (plus `--nocommit` when the
//!   parent used it), like PHP's passthru `arc patch`.
//! - `arc.autostash` configuration and `git stash` / `git stash pop`
//!   around the flow are ported; the stash pop runs even when patching
//!   fails.

extern crate self as arc_rs;

use std::collections::VecDeque;
use std::path::Path;
use std::process::ExitCode;

use arc_rs::land::{self, Io, Out};
use arc_rs::{diff_submit, git, hg, svn, terminal};

/// Subcommand name, matching `arc patch`.
pub const NAME: &str = "patch";

const USAGE: &str = "usage: arc patch [--revision ID] [--diff ID] [--arcbundle FILE] [--patch FILE] [--encoding NAME] [--update] [--nocommit] [--skip-dependencies] [--nobranch] [--force] [--preview] [--conduit-uri URI] [D12345]\n";

/// Error when no working copy is detected (established port message; see
/// the module docs).
pub const NOT_A_WORKING_COPY: &str = "not inside a git, hg, or svn working copy";

/// Error when Conduit reports no usable diff
/// (`ArcanistWorkflow.php:1390-1394`).
pub const NO_DIFF_OR_REVISION: &str =
    "The diff or revision you specified is either invalid or you don't have permission to view it.";

/// Error when `--patch -` reads no patch text
/// (`ArcanistPatchWorkflow.php:619-623`).
pub const NO_STDIN_PATCH: &str = "Failed to read patch from stdin!";

/// Parsed `patch` options.
///
/// Flag names are verbatim from
/// `ArcanistPatchWorkflow::getWorkflowArguments()` (`:26-112`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Apply a revision (`revision`, `:57-61`).
    pub revision: Option<String>,
    /// Apply a diff (`diff`, `:62-66`).
    pub diff: Option<String>,
    /// Apply an arcbundle file (`arcbundle`, `:67-71`).
    pub arcbundle: Option<String>,
    /// Apply a patch file or `-` (`patch`, `:72-76`).
    pub patch: Option<String>,
    /// Patch encoding (`encoding`, `:77-82`).
    pub encoding: Option<String>,
    /// Update the working copy first (`update`, `:31-36`).
    pub update: bool,
    /// Apply without committing (`nocommit`, `:37-42`).
    pub nocommit: bool,
    /// Skip dependency patching (`skip-dependencies`, `:43-47`).
    pub skip_dependencies: bool,
    /// Apply without branching (`nobranch`, `:48-52`).
    pub nobranch: bool,
    /// Skip sanity checks (`force`, `:53-56`).
    pub force: bool,
    /// Positional patch source (the `*` wildcard, `:83-111`).
    pub name: Vec<String>,
    /// Print the patch instead of applying it (port extension).
    pub preview: bool,
    /// Conduit URI override (global `--conduit-uri`).
    pub conduit_uri: Option<String>,
    /// Canonical long flag names in first-provision order.
    pub provided: Vec<&'static str>,
}

/// A value-taking `patch` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueFlag {
    Revision,
    Diff,
    Arcbundle,
    Patch,
    Encoding,
    ConduitUri,
}

/// A boolean `patch` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolFlag {
    Update,
    Nocommit,
    SkipDependencies,
    Nobranch,
    Force,
    Preview,
}

/// Any `patch` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Value(ValueFlag),
    Bool(BoolFlag),
}

impl Flag {
    /// True for `param` flags, which consume the next token as their value.
    fn takes_param(self) -> bool {
        matches!(self, Flag::Value(_))
    }
}

/// Conflict reason rendering: boolean `true` renders as `1`, like PHP
/// string interpolation of `true` (mirrors the D1 pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictReason {
    /// Reason `true`.
    One,
}

impl ConflictReason {
    fn suffix(self) -> &'static str {
        match self {
            ConflictReason::One => ": 1",
        }
    }
}

/// One `getWorkflowArguments()` entry, in PHP order. Patch declares no
/// short flags. Options without a `supports` entry run under every VCS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlagSpec {
    long: &'static str,
    flag: Flag,
    /// Mutually exclusive flags with reasons (`update` lists `nobranch`
    /// and the nonexistent `bookmark`; `nobranch` lists `update`).
    conflicts: &'static [(&'static str, ConflictReason)],
    /// VCS names this option supports, if restricted.
    supports: Option<&'static [&'static str]>,
}

const FLAG_SPECS: &[FlagSpec] = &[
    FlagSpec {
        long: "update",
        flag: Flag::Bool(BoolFlag::Update),
        conflicts: &[("nobranch", ConflictReason::One)],
        supports: Some(&["git", "svn", "hg"]),
    },
    FlagSpec {
        long: "nocommit",
        flag: Flag::Bool(BoolFlag::Nocommit),
        conflicts: &[],
        supports: Some(&["git", "hg"]),
    },
    FlagSpec {
        long: "skip-dependencies",
        flag: Flag::Bool(BoolFlag::SkipDependencies),
        conflicts: &[],
        supports: Some(&["git", "hg"]),
    },
    FlagSpec {
        long: "nobranch",
        flag: Flag::Bool(BoolFlag::Nobranch),
        conflicts: &[("update", ConflictReason::One)],
        supports: Some(&["git", "hg"]),
    },
    FlagSpec {
        long: "force",
        flag: Flag::Bool(BoolFlag::Force),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "revision",
        flag: Flag::Value(ValueFlag::Revision),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "diff",
        flag: Flag::Value(ValueFlag::Diff),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "arcbundle",
        flag: Flag::Value(ValueFlag::Arcbundle),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "patch",
        flag: Flag::Value(ValueFlag::Patch),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "encoding",
        flag: Flag::Value(ValueFlag::Encoding),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "preview",
        flag: Flag::Bool(BoolFlag::Preview),
        conflicts: &[],
        supports: None,
    },
    FlagSpec {
        long: "conduit-uri",
        flag: Flag::Value(ValueFlag::ConduitUri),
        conflicts: &[],
        supports: None,
    },
];

/// Find a spec by long name.
fn lookup_long(name: &str) -> Option<FlagSpec> {
    FLAG_SPECS.iter().find(|spec| spec.long == name).copied()
}

/// Store a parsed value-flag value in `options`.
fn store_value_flag(options: &mut Options, flag: ValueFlag, value: String) {
    match flag {
        ValueFlag::Revision => options.revision = Some(value),
        ValueFlag::Diff => options.diff = Some(value),
        ValueFlag::Arcbundle => options.arcbundle = Some(value),
        ValueFlag::Patch => options.patch = Some(value),
        ValueFlag::Encoding => options.encoding = Some(value),
        ValueFlag::ConduitUri => options.conduit_uri = Some(value),
    }
}

/// Store a parsed boolean flag in `options`.
fn store_bool_flag(options: &mut Options, flag: BoolFlag) {
    match flag {
        BoolFlag::Update => options.update = true,
        BoolFlag::Nocommit => options.nocommit = true,
        BoolFlag::SkipDependencies => options.skip_dependencies = true,
        BoolFlag::Nobranch => options.nobranch = true,
        BoolFlag::Force => options.force = true,
        BoolFlag::Preview => options.preview = true,
    }
}

/// Record one parsed flag in `options`, tracking first-provision order.
fn store_flag(options: &mut Options, spec: FlagSpec, value: Option<String>) {
    if !options.provided.contains(&spec.long) {
        options.provided.push(spec.long);
    }
    match spec.flag {
        Flag::Value(value_flag) => {
            store_value_flag(options, value_flag, value.unwrap_or_default());
        }
        Flag::Bool(bool_flag) => store_bool_flag(options, bool_flag),
    }
}

/// Parse one `--name` or `--name=value` token.
fn parse_long_flag(
    options: &mut Options,
    queue: &mut VecDeque<String>,
    typed: &str,
    name: &str,
) -> Result<(), String> {
    let (key, inline) = match name.split_once('=') {
        Some((key, value)) => (key, Some(value)),
        None => (name, None),
    };
    let Some(spec) = lookup_long(key) else {
        return Err(format!("Unknown argument '{key}'. Try 'arc help'."));
    };
    if !spec.flag.takes_param() {
        store_flag(options, spec, None);
        if let Some(value) = inline {
            queue.push_front(value.to_string());
        }
        return Ok(());
    }
    let value = match inline {
        Some(value) => value.to_string(),
        None => queue
            .pop_front()
            .ok_or_else(|| format!("Option '{typed}' requires a parameter."))?,
    };
    store_flag(options, spec, Some(value));
    Ok(())
}

/// Parse `argv` (excluding the subcommand name), mirroring the D1
/// pattern. Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut queue: VecDeque<String> = argv.iter().cloned().collect();
    let mut end_of_flags = false;
    while let Some(arg) = queue.pop_front() {
        if end_of_flags {
            options.name.push(arg);
            continue;
        }
        if arg == "--" {
            end_of_flags = true;
            continue;
        }
        if arg == "--help" || arg == "-h" {
            return Ok(None);
        }
        if let Some(name) = arg.strip_prefix("--") {
            parse_long_flag(&mut options, &mut queue, &arg, name)?;
            continue;
        }
        if let Some(spelling) = arg.strip_prefix('-') {
            return Err(format!("Unknown argument '{spelling}'. Try 'arc help'."));
        }
        options.name.push(arg);
    }
    check_conflicts(&options)?;
    Ok(Some(options))
}

/// Enforce mutual exclusion in first-provision order
/// (`ArcanistWorkflow.php:899-919`, mirrored from the D1 pattern).
fn check_conflicts(options: &Options) -> Result<(), String> {
    for provided in &options.provided {
        let Some(spec) = lookup_long(provided) else {
            continue;
        };
        for (conflict, reason) in spec.conflicts {
            if options.provided.contains(conflict) {
                return Err(format!(
                    "Arguments '--{provided}' and '--{conflict}' are mutually exclusive{}",
                    reason.suffix()
                ));
            }
        }
    }
    Ok(())
}

/// Enforce per-option VCS support in first-provision order (D1 pattern).
///
/// `vcs` is `git`, `hg`, or `svn`.
pub fn check_supports(options: &Options, vcs: &str) -> Result<(), String> {
    for provided in &options.provided {
        let Some(spec) = lookup_long(provided) else {
            continue;
        };
        if let Some(supported) = spec.supports
            && !supported.contains(&vcs)
        {
            return Err(format!(
                "Option '--{provided}' is not supported under {vcs}."
            ));
        }
    }
    Ok(())
}

/// Selected patch source (`ArcanistPatchWorkflow::didParseArguments()`,
/// `:205-276`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A positional name, `--revision`, or `--diff` value.
    Named(String),
    /// `--arcbundle` path.
    Arcbundle(String),
    /// `--patch` path or `-`.
    Patch(String),
    /// `--diff` ID (normalized).
    Diff(String),
    /// `--revision` ID (normalized).
    Revision(String),
}

/// Select the patch source, enforcing the verbatim source rules.
pub fn select_source(options: &Options) -> Result<Source, String> {
    if options.name.len() > 1 {
        return Err("Specify at most one revision name.".to_string());
    }
    let mut sources: Vec<Source> = Vec::new();
    if let Some(name) = options.name.first() {
        sources.push(Source::Named(normalize_revision_id(name)?));
    }
    if let Some(path) = options.arcbundle.as_deref() {
        sources.push(Source::Arcbundle(path.to_string()));
    }
    if let Some(path) = options.patch.as_deref() {
        sources.push(Source::Patch(path.to_string()));
    }
    if let Some(diff) = options.diff.as_deref() {
        sources.push(Source::Diff(normalize_numeric_id("diff", diff)?));
    }
    if let Some(revision) = options.revision.as_deref() {
        sources.push(Source::Revision(normalize_numeric_id(
            "revision", revision,
        )?));
    }
    if sources.is_empty() {
        return Err(
            "You must specify changes to apply to the working copy with \"D12345\", \
             \"--revision\", \"--diff\", \"--arcbundle\", or \"--patch\"."
                .to_string(),
        );
    }
    if sources.len() > 1 {
        return Err(
            "Options \"D12345\", \"--revision\", \"--diff\", \"--arcbundle\" and \"--patch\" are \
             mutually exclusive. Choose exactly one patch source."
                .to_string(),
        );
    }
    Ok(sources.into_iter().next().expect("one source"))
}

/// Normalize a revision monogram: strip one leading `D` (either case),
/// like `ArcanistWorkflow::normalizeRevisionID()` (`:1586-1588`), then
/// require digits (a fail-fast port extension; PHP would fail later at
/// Conduit with a passthrough error).
pub fn normalize_revision_id(value: &str) -> Result<String, String> {
    normalize_numeric_id("revision", value)
}

/// Normalize a `--diff` ID the same way (`:256-258` normalizes both).
pub fn normalize_diff_id(value: &str) -> Result<String, String> {
    normalize_numeric_id("diff", value)
}

/// Strip one leading `D`/`d` and require an all-digit remainder.
fn normalize_numeric_id(kind: &str, value: &str) -> Result<String, String> {
    let stripped = value
        .strip_prefix('D')
        .or_else(|| value.strip_prefix('d'))
        .unwrap_or(value);
    if stripped.is_empty() || !stripped.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid {kind} '{value}': expected D12345"));
    }
    Ok(stripped.to_string())
}

/// A loaded patch bundle: the patch text plus metadata.
///
/// Mirrors the `ArcanistBundle` surface `arc patch` uses: the rendered
/// text (`toGitPatch()` / file bytes), the base revision, the revision
/// ID, and the author identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bundle {
    /// Patch text to apply.
    pub text: String,
    /// Base commit the change applies against, if known.
    pub base_revision: Option<String>,
    /// Differential revision ID, if known.
    pub revision_id: Option<u64>,
    /// Author display name, if known.
    pub author_name: Option<String>,
    /// Author email, if known.
    pub author_email: Option<String>,
}

impl Bundle {
    /// True when a base revision is known
    /// (`ArcanistBundle::getBaseRevision()` is non-empty,
    /// `src/parser/ArcanistBundle.php:90-97`).
    #[must_use]
    pub fn has_base_revision(&self) -> bool {
        self.base_revision
            .as_deref()
            .is_some_and(|base| !base.is_empty())
    }

    /// `Name <email>` author line, when both parts are known.
    #[must_use]
    pub fn full_author(&self) -> Option<String> {
        match (&self.author_name, &self.author_email) {
            (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
            _ => None,
        }
    }
}

/// True for UTF-8 encoding names (case-insensitive, dash-optional).
fn is_utf8_name(encoding: &str) -> bool {
    matches!(encoding.to_lowercase().as_str(), "utf-8" | "utf8")
}

/// Decode `bytes` with `encoding` (`ArcanistBundle::setEncoding()` /
/// `convertNonUTF8Diff()`, `src/parser/ArcanistBundle.php:72-88,493-498`).
///
/// UTF-8 (or no encoding) decodes lossily; anything else decodes through
/// a Windows-1252 table (see the module docs). The table is complete
/// and verified byte-for-byte against the Windows-1252 standard:
/// bytes outside `0x80..0xA0` map to the identical Latin-1 scalar, the
/// 27 defined `0x80..0xA0` bytes map per the standard, and the 5
/// undefined bytes (`0x81,0x8D,0x8F,0x90,0x9D`) become U+FFFD (PHP
/// delegates to `mb_convert_encoding()` with no vendored table;
/// U+FFFD matches Rust lossy conversion and `phutil_utf8ize()`).
pub fn decode_bytes(bytes: &[u8], encoding: Option<&str>) -> String {
    match encoding {
        None => String::from_utf8_lossy(bytes).into_owned(),
        Some(name) if is_utf8_name(name) => String::from_utf8_lossy(bytes).into_owned(),
        Some(_) => bytes.iter().map(|byte| decode_cp1252(*byte)).collect(),
    }
}

/// Windows-1252 scalars for `0x80..0xA0` in byte order; `None` marks the
/// 5 undefined bytes (`0x81,0x8D,0x8F,0x90,0x9D`), which decode as U+FFFD.
const CP1252_80_9F: [Option<char>; 32] = [
    Some('€'),
    None,
    Some('‚'),
    Some('ƒ'),
    Some('„'),
    Some('…'),
    Some('†'),
    Some('‡'),
    Some('ˆ'),
    Some('‰'),
    Some('Š'),
    Some('‹'),
    Some('Œ'),
    None,
    Some('Ž'),
    None,
    None,
    Some('‘'),
    Some('’'),
    Some('“'),
    Some('”'),
    Some('•'),
    Some('–'),
    Some('—'),
    Some('˜'),
    Some('™'),
    Some('š'),
    Some('›'),
    Some('œ'),
    None,
    Some('ž'),
    Some('Ÿ'),
];

/// Map one byte through Windows-1252; undefined bytes become U+FFFD.
fn decode_cp1252(byte: u8) -> char {
    if (0x80..0xA0).contains(&byte) {
        return CP1252_80_9F[(byte - 0x80) as usize].unwrap_or('\u{FFFD}');
    }
    byte as char
}

/// Read patch bytes from `path` or `-` (stdin).
///
/// `-` reads `io.patch_stdin` when scripted, else real stdin.
pub fn read_patch_bytes(path: &str, io: &mut Io) -> Result<Vec<u8>, String> {
    if path == "-" {
        if let Some(scripted) = io.patch_stdin.clone() {
            return Ok(scripted.into_bytes());
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Failed to read patch from stdin: {e}"))?;
        return Ok(bytes);
    }
    std::fs::read(path).map_err(|e| format!("Failed to read patch file '{path}': {e}"))
}

/// Empty-diff parse error (`ArcanistDiffParser.php:189-192`).
pub const EMPTY_DIFF_ERROR: &str = "Can't parse an empty diff!";

/// Malformed header error (`ArcanistDiffParser.php:275-287`).
const EXPECTED_HUNK_HEADER: &str = "Expected a hunk header, like 'Index: /path/to/file.ext' \
     (svn), 'Property changes on: /path/to/file.ext' (svn properties), 'commit \
     59bcc3ad6775562f845953cf01624225' (git show), 'diff --git' (git diff), '--- \
     filename' (unified diff), or 'diff -r' (hg diff or patch).";

/// Split like `phutil_split_lines($text, true)`: after each `\n`,
/// keeping endings; `""` yields `[""]`.
fn split_retain_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    text.split_inclusive('\n').map(str::to_string).collect()
}

/// True for PCRE `\s` (ASCII whitespace incl. vertical tab).
fn is_pcre_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x0B)
}

/// True when any line starts with an ANSI color code
/// (`ArcanistDiffParser.php:1101-1104`).
fn has_leading_ansi(text: &str) -> bool {
    for line in text.split('\n') {
        let bytes = line.as_bytes();
        if bytes.len() >= 3 && bytes[0] == 0x1B && bytes[1] == b'[' {
            let mut i = 2;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b';') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'm' {
                return true;
            }
        }
    }
    false
}

/// Strip all `\x1B\[[\d;]*m` sequences.
fn strip_ansi_codes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B && bytes.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'm' {
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// PHP `ltrim()` without charlist: space, tab, newline, CR, NUL, VT.
fn ltrim_php(text: &str) -> &str {
    text.trim_start_matches([' ', '\t', '\n', '\r', '\0', '\x0B'])
}

/// True for `YYYY-MM-DD` at the start of `text` (10 ASCII chars).
fn is_date_prefix(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 10
        && b[0..4].iter().all(|d| d.is_ascii_digit())
        && b[4] == b'-'
        && b[5..7].iter().all(|d| d.is_ascii_digit())
        && b[7] == b'-'
        && b[8..10].iter().all(|d| d.is_ascii_digit())
}

/// True when `rest` (after `--- `) holds `<old><ws><date>` with
/// non-empty old (`ArcanistDiffParser.php:244`).
fn has_unified_date(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i + 10 <= bytes.len() {
        if is_date_prefix(&rest[i..]) {
            let mut ws_start = i;
            while ws_start > 0 && is_pcre_space(bytes[ws_start - 1]) {
                ws_start -= 1;
            }
            if ws_start > 0 && ws_start < i {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// True for `commit <hex>[ (<any>)]` (`ArcanistDiffParser.php:237`).
fn is_commit_header(rest: &str) -> bool {
    let mut hex_end = 0;
    for b in rest.bytes() {
        if b.is_ascii_digit() || (b'a'..=b'f').contains(&b) {
            hex_end += 1;
        } else {
            break;
        }
    }
    if hex_end == 0 {
        return false;
    }
    let tail = &rest[hex_end..];
    if tail.is_empty() {
        return true;
    }
    tail.starts_with(" (") && tail.ends_with(')')
}

/// True for `diff -r <hex> [-r <hex> ]<cur>`
/// (`ArcanistDiffParser.php:250`).
fn is_hg_header(rest: &str) -> bool {
    let mut hex_end = 0;
    for b in rest.bytes() {
        if b.is_ascii_digit() || (b'a'..=b'f').contains(&b) {
            hex_end += 1;
        } else {
            break;
        }
    }
    if hex_end == 0 {
        return false;
    }
    let tail = &rest[hex_end..];
    if !tail.starts_with(' ') {
        return false;
    }
    let after = &tail[1..];
    if let Some(second) = after.strip_prefix("-r ") {
        let mut hex2 = 0;
        for b in second.bytes() {
            if b.is_ascii_digit() || (b'a'..=b'f').contains(&b) {
                hex2 += 1;
            } else {
                break;
            }
        }
        if hex2 == 0 {
            return false;
        }
        let tail2 = &second[hex2..];
        return tail2.starts_with(' ') && tail2.len() > 1;
    }
    !after.is_empty()
}

/// True for binary unified headers (`ArcanistDiffParser.php:245-247`).
fn is_binary_unified(line: &str) -> bool {
    let rest = if let Some(tail) = line.strip_prefix("Binary files ") {
        tail
    } else if let Some(tail) = line.strip_prefix("Files ") {
        tail
    } else {
        return false;
    };
    let Some(and_at) = rest.find(" and ") else {
        return false;
    };
    let (old_part, after_and) = (&rest[..and_at], &rest[and_at + 5..]);
    let Some(differ_at) = after_and.rfind(" differ") else {
        return false;
    };
    let new_part = &after_and[..differ_at];
    has_unified_date(old_part) && has_unified_date(new_part)
}

/// True when `line` (trimmed of `\r\n`) matches any diff header
/// (`ArcanistDiffParser.php:228-251`).
fn is_diff_header(line: &str) -> bool {
    if line.starts_with("Index: ") && line.len() > 7 {
        return true;
    }
    for prefix in ["Modified: ", "Added: ", "Deleted: ", "Copied: "] {
        if line.starts_with(prefix) && line.len() > prefix.len() {
            return true;
        }
    }
    if line.starts_with("Property changes on: ") && line.len() > 21 {
        return true;
    }
    if let Some(rest) = line.strip_prefix("commit ") {
        return is_commit_header(rest);
    }
    if line.starts_with("diff --git ") {
        return true;
    }
    if line.starts_with("rcsdiff -u ") {
        return true;
    }
    if let Some(rest) = line.strip_prefix("--- ") {
        return has_unified_date(rest);
    }
    if is_binary_unified(line) {
        return true;
    }
    if let Some(rest) = line.strip_prefix("diff -r ") {
        return is_hg_header(rest);
    }
    false
}

/// Format a `didFailParse()` error (`ArcanistDiffParser.php:1198-1227`).
///
/// `lines` retain endings; `idx` is the current line (may equal
/// `lines.len()` at EOF, yielding no `>>>` marker, like PHP).
/// `writeDiffOnFailure` is false for `--patch` (`newFromDiff()` never
/// sets it), so no raw-file line is emitted.
pub fn format_parse_error(message: &str, lines: &[String], idx: usize) -> String {
    let min = idx.saturating_sub(5);
    let max = (idx + 5).min(lines.len().saturating_sub(1));
    let mut context = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i < min || i > max {
            continue;
        }
        let marker = if i == idx { ">>>  " } else { "" };
        context.push_str(&format!("{:>8} {:>6}   {}", marker, i + 1, line));
    }
    format!("Diff Parse Exception: {message}\n\n{context}")
}

/// Validate `--patch` text like `ArcanistBundle::newFromDiff()`
/// (`ArcanistPatchWorkflow.php:376`, `ArcanistDiffParser.php:189-287`).
///
/// Rejects empty diffs, strips ANSI colors and leading whitespace,
/// skips `#` comments, and requires a diff header; malformed input
/// fails with byte-for-byte `didFailParse()` errors. Successful
/// patches pass through verbatim (PHP re-renders via `toGitPatch()`,
/// which is equivalent for git patches; unified-to-git conversion and
/// hunk-body line-count checks are left to `git apply`/`hg import`).
pub fn validate_patch_text(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(EMPTY_DIFF_ERROR.to_string());
    }
    let lines = preprocess_patch_lines(text);
    let idx = skip_patch_comments(&lines);
    if idx >= lines.len() {
        return Err(format_parse_error(EXPECTED_HUNK_HEADER, &lines, idx));
    }
    let first = lines[idx].trim_end_matches(['\r', '\n']);
    if is_diff_header(first) {
        return Ok(());
    }
    if probe_hg_export_header(&lines, idx) {
        return Ok(());
    }
    Err(format_parse_error(EXPECTED_HUNK_HEADER, &lines, idx))
}

/// Strip ANSI colors and leading whitespace, then split into lines that
/// retain their endings.
fn preprocess_patch_lines(text: &str) -> Vec<String> {
    let mut owned = text.to_string();
    if has_leading_ansi(&owned) {
        owned = strip_ansi_codes(&owned);
    }
    split_retain_lines(ltrim_php(&owned))
}

/// Skip leading `#` comment lines, returning the first content line index.
fn skip_patch_comments(lines: &[String]) -> usize {
    let mut idx = 0;
    while idx < lines.len() {
        let cur = lines[idx].trim_end_matches(['\r', '\n']);
        if cur.starts_with('#') {
            idx += 1;
        } else {
            break;
        }
    }
    idx
}

/// True when a later `diff -r`/`diff --git` opener excuses the failing
/// first line (`hg export` extended diffs).
///
/// Skips the failing line itself: a malformed `diff -r`/`diff --git`
/// opener must still fail; only a later opener excuses the preamble.
fn probe_hg_export_header(lines: &[String], idx: usize) -> bool {
    let mut only_first = true;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_end_matches(['\r', '\n']);
        if t.trim().is_empty() || t.starts_with('#') {
            continue;
        }
        if i != idx {
            only_first = false;
        }
        break;
    }
    if !only_first {
        return false;
    }
    let mut probe = idx + 1;
    while probe < lines.len() {
        let t = lines[probe].trim_end_matches(['\r', '\n']);
        if t.starts_with("diff -r ") || t.starts_with("diff --git ") {
            return true;
        }
        // Match PHP's `^\s*diff\s+-(?:r|-git)` loosely.
        let trimmed_start = t.trim_start();
        if trimmed_start.starts_with("diff -r") || trimmed_start.starts_with("diff --git") {
            return true;
        }
        probe += 1;
    }
    false
}

/// Load a `--patch` bundle from a file or `-`
/// (`ArcanistPatchWorkflow.php:361-377` via `ArcanistBundle::newFromDiff()`).
///
/// Empty stdin fails with [`NO_STDIN_PATCH`]; decoded text is
/// pre-parsed like `ArcanistDiffParser::parseDiff()` (empty and
/// malformed headers fail with byte-for-byte parse errors) and
/// carries no metadata. Valid patches pass through verbatim.
pub fn load_file_bundle(path: &str, encoding: Option<&str>, io: &mut Io) -> Result<Bundle, String> {
    let bytes = read_patch_bytes(path, io)?;
    if path == "-" && bytes.is_empty() {
        return Err(NO_STDIN_PATCH.to_string());
    }
    let text = decode_bytes(&bytes, encoding);
    validate_patch_text(&text)?;
    Ok(Bundle {
        text,
        ..Bundle::default()
    })
}

/// List `tar` members of `bundle` (`tar tfO`, via `tar -tf`).
fn tar_list(bundle: &str) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("tar")
        .args(["-tf", bundle])
        .output()
        .map_err(|e| format!("failed to read arcbundle '{bundle}': {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to read arcbundle '{bundle}': {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\n')
        .filter_map(|line| {
            let trimmed = line.trim().trim_start_matches("./");
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect())
}

/// Extract one `tar` member of `bundle` to bytes (`tar xfO`).
///
/// Tries `member` then `./{member}`: GNU tar stores `./`-prefixed names
/// (from `tar -cf ... .`) and requires an exact match, while bsdtar
/// matches either spelling.
fn tar_extract(bundle: &str, member: &str) -> Result<Vec<u8>, String> {
    for candidate in [member.to_string(), format!("./{member}")] {
        let output = std::process::Command::new("tar")
            .args(["-xOf", bundle])
            .arg(&candidate)
            .output()
            .map_err(|e| format!("failed to read arcbundle '{bundle}': {e}"))?;
        if output.status.success() {
            return Ok(output.stdout);
        }
    }
    Err(format!(
        "failed to read arcbundle '{bundle}': member '{member}' is missing"
    ))
}

/// Bundle metadata from `meta.json`
/// (`ArcanistBundle::newFromArcBundle()`,
/// `src/parser/ArcanistBundle.php:141-159`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BundleMeta {
    base_revision: Option<String>,
    revision_id: Option<u64>,
    encoding: Option<String>,
    author_name: Option<String>,
    author_email: Option<String>,
}

/// Parse `meta.json` bytes; missing or partial metadata yields `None`
/// fields (old bundles predate `meta.json`).
fn parse_bundle_meta(bytes: &[u8]) -> BundleMeta {
    let mut meta = BundleMeta::default();
    let doc: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(doc) => doc,
        Err(_) => return meta,
    };
    meta.base_revision = doc
        .get("baseRevision")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    meta.revision_id = doc.get("revisionID").and_then(|v| v.as_u64());
    meta.encoding = doc
        .get("encoding")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    meta.author_name = doc
        .get("authorName")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    meta.author_email = doc
        .get("authorEmail")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    meta
}

/// Change types (`ArcanistDiffChangeType`, `src/parser/diff/ArcanistDiffChangeType.php:8-17`).
const TYPE_ADD: u32 = 1;
const TYPE_CHANGE: u32 = 2;
const TYPE_DELETE: u32 = 3;
const TYPE_MOVE_AWAY: u32 = 4;
const TYPE_COPY_AWAY: u32 = 5;
const TYPE_MOVE_HERE: u32 = 6;
const TYPE_COPY_HERE: u32 = 7;
const TYPE_MULTICOPY: u32 = 8;

/// File types (`ArcanistDiffChangeType.php:19-25`).
const FILE_TEXT: u32 = 1;
const FILE_IMAGE: u32 = 2;
const FILE_BINARY: u32 = 3;
const FILE_DIRECTORY: u32 = 4;

/// Default `unix:filemode` when properties omit it
/// (`ArcanistBundle.php:402-403`).
const DEFAULT_MODE: &str = "100644";

/// One bundle change with substituted hunk text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleChange {
    /// Change type (`TYPE_*`).
    pub change_type: u32,
    /// File type (`FILE_*`).
    pub file_type: u32,
    /// Old path (`None` for additions).
    pub old_path: Option<String>,
    /// Current path (`None` for deletions).
    pub current_path: Option<String>,
    /// `unix:filemode` before, when properties carry it.
    pub old_mode: Option<String>,
    /// `unix:filemode` after, when properties carry it.
    pub new_mode: Option<String>,
    /// Decoded hunk corpora.
    pub hunks: Vec<String>,
    /// `old:binary-phid` metadata for binary/image changes.
    pub old_binary_phid: Option<String>,
    /// `new:binary-phid` metadata for binary/image changes.
    pub new_binary_phid: Option<String>,
    /// Away paths for move/copy sources (`awayPaths`).
    pub away_paths: Vec<String>,
    /// Original file bytes when carried inline (rare; conduit and
    /// arcbundles use phids instead).
    pub original_file_data: Option<Vec<u8>>,
    /// Current file bytes when carried inline.
    pub current_file_data: Option<Vec<u8>>,
}

impl BundleChange {
    fn defaulted_old_mode(&self) -> &str {
        self.old_mode.as_deref().unwrap_or(DEFAULT_MODE)
    }

    fn defaulted_new_mode(&self) -> &str {
        self.new_mode.as_deref().unwrap_or(DEFAULT_MODE)
    }
}

/// Parse one `changes.json` change dictionary with decoded hunk corpora.
fn parse_bundle_change(value: &serde_json::Value, hunks: Vec<String>) -> BundleChange {
    let mode = |key: &str| {
        value
            .get(key)
            .and_then(|props| props.get("unix:filemode"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    BundleChange {
        change_type: value
            .get("type")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::from(TYPE_CHANGE)) as u32,
        file_type: value
            .get("fileType")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::from(FILE_TEXT)) as u32,
        old_path: value
            .get("oldPath")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        current_path: value
            .get("currentPath")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        old_mode: mode("oldProperties"),
        new_mode: mode("newProperties"),
        hunks,
        old_binary_phid: value
            .get("metadata")
            .and_then(|m| m.get("old:binary-phid"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        new_binary_phid: value
            .get("metadata")
            .and_then(|m| m.get("new:binary-phid"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        away_paths: value
            .get("awayPaths")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        original_file_data: None,
        current_file_data: None,
    }
}

/// Quote a git target path (`encodeGitTargetPath()`): plain paths pass
/// through; paths with quotes, backslashes, or control bytes render
/// C-quoted.
pub fn quote_git_target(path: &str) -> String {
    let needs_quotes = path
        .bytes()
        .any(|byte| byte == b'"' || byte == b'\\' || byte < 0x20 || byte == 0x7f);
    if !needs_quotes {
        return path.to_string();
    }
    let mut quoted = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b'\n' => quoted.push_str("\\n"),
            b'\t' => quoted.push_str("\\t"),
            other if other < 0x20 || other == 0x7f => {
                quoted.push_str(&format!("\\{other:03o}"));
            }
            other => quoted.push(other as char),
        }
    }
    quoted.push('"');
    quoted
}

/// Rotate `v` left by `n` bits (SHA-1 helper).
fn rotl32(v: u32, n: u32) -> u32 {
    v.rotate_left(n)
}

/// Compress one 64-byte SHA-1 block into `state`.
fn sha1_block(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut w = [0u32; 80];
    for (i, word) in w.iter_mut().enumerate().take(16) {
        let o = i * 4;
        *word = (u32::from(block[o]) << 24)
            | (u32::from(block[o + 1]) << 16)
            | (u32::from(block[o + 2]) << 8)
            | u32::from(block[o + 3]);
    }
    for i in 16..80 {
        w[i] = rotl32(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
    }
    let (mut a, mut b, mut c, mut d, mut e) = (state[0], state[1], state[2], state[3], state[4]);
    for (i, word) in w.iter().enumerate() {
        let (f, k) = if i < 20 {
            ((b & c) | ((!b) & d), 0x5A827999u32)
        } else if i < 40 {
            (b ^ c ^ d, 0x6ED9EBA1u32)
        } else if i < 60 {
            ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32)
        } else {
            (b ^ c ^ d, 0xCA62C1D6u32)
        };
        let temp = rotl32(a, 5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = rotl32(b, 30);
        b = a;
        a = temp;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

/// SHA-1 hex digest of `data` (40 lowercase hex chars).
pub fn sha1_hex(data: &[u8]) -> String {
    let mut state = [
        0x67452301u32,
        0xEFCDAB89,
        0x98BADCFE,
        0x10325476,
        0xC3D2E1F0,
    ];
    let mut padded = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.as_chunks::<64>().0 {
        sha1_block(&mut state, block);
    }
    let mut out = String::with_capacity(40);
    for word in state {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

/// Git blob SHA-1 for `data`: `sha1("blob <len>\\0" + data)`.
pub fn git_blob_sha1(data: &[u8]) -> String {
    let mut prefixed = format!("blob {}\0", data.len()).into_bytes();
    prefixed.extend_from_slice(data);
    sha1_hex(&prefixed)
}

/// Adler-32 checksum (zlib trailer).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for byte in data {
        s1 = (s1 + u32::from(*byte)) % MOD;
        s2 = (s2 + s1) % MOD;
    }
    (s2 << 16) | s1
}

/// Zlib-encode `data` with stored (uncompressed) deflate blocks.
///
/// PHP uses `gzcompress()` (compressed deflate); stored blocks are
/// larger but equally valid zlib, so `git apply` accepts them. Exact
/// compressed bytes need not match PHP; the inflated result does.
pub fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 11);
    out.push(0x78);
    out.push(0x01);
    let chunks: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(65535).collect()
    };
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.iter().enumerate() {
        out.push(u8::from(i == last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Git base85 alphabet (`base85.c`, `ArcanistBundle.php:893-979`).
const BASE85_MAP: &[u8; 85] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWX\
    YZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";

/// Length character for one base85 line (`ArcanistBundle.php:981-988`):
/// 0 maps to `@`, 1-26 to `A`-`Z`, 27-52 to `a`-`z`.
fn base85_len_char(len: usize) -> char {
    if len <= 26 {
        (len as u8 + b'A' - 1) as char
    } else {
        (len as u8 - 26 + b'a' - 1) as char
    }
}

/// Encode 4-byte groups of `chunk` (last group zero-padded) as base85.
fn base85_encode_groups(chunk: &[u8], buf: &mut String) {
    let mut pos = 0;
    let mut remaining = chunk.len();
    while remaining > 0 {
        let mut accum: u32 = 0;
        for shift in [24u32, 16, 8, 0] {
            if remaining == 0 {
                break;
            }
            accum += u32::from(chunk[pos]) << shift;
            pos += 1;
            remaining -= 1;
            if remaining == 0 {
                break;
            }
        }
        let mut slice = [0u8; 5];
        let mut value = accum;
        for slot in &mut slice {
            *slot = BASE85_MAP[(value % 85) as usize];
            value /= 85;
        }
        slice.reverse();
        buf.push_str(std::str::from_utf8(&slice).unwrap_or(""));
    }
}

/// Base85-encode `input` like `ArcanistBundle::newBase85Data()`.
///
/// Splits into 52-byte lines, prefixes each with its length char,
/// encodes groups, and terminates every line with `\n`. Empty input
/// encodes to `"@\n"`, like PHP's `str_split('', 52)` path.
pub fn base85_encode(input: &[u8]) -> String {
    if input.is_empty() {
        return "@\n".to_string();
    }
    let mut buf = String::new();
    for chunk in input.chunks(52) {
        buf.push(base85_len_char(chunk.len()));
        base85_encode_groups(chunk, &mut buf);
        buf.push('\n');
    }
    buf
}

/// Emit one `GIT binary patch` literal body for `data`.
///
/// Compresses with [`zlib_store`] (PHP uses `gzcompress()`; both
/// inflate identically) then [`base85_encode`]s. The result ends with
/// `\n` per line; callers append one more `\n` for the blank line.
pub fn emit_binary_body(data: &[u8]) -> String {
    base85_encode(&zlib_store(data))
}

/// Map one base64 character to its 6-bit value (`=` is padding).
fn base64_val(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'=' => None,
        _ => None,
    }
}

/// Decode standard base64 (RFC 4648) like PHP's `base64_decode()`.
///
/// ASCII whitespace is ignored; other invalid bytes fail. Padding
/// (`=`/`==`) is allowed only at the very end.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return Err("invalid base64 length".to_string());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (i, quad) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let is_last = i == bytes.len() / 4 - 1;
        let pad = quad.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || (!is_last && pad > 0) {
            return Err("invalid base64 padding".to_string());
        }
        if quad[..4 - pad].contains(&b'=') {
            return Err("invalid base64 padding".to_string());
        }
        let mut accum: u32 = 0;
        for (j, byte) in quad.iter().enumerate() {
            if j >= 4 - pad {
                accum <<= 6;
                continue;
            }
            let val = base64_val(*byte).ok_or_else(|| "invalid base64 byte".to_string())?;
            accum = (accum << 6) | u32::from(val);
        }
        out.push((accum >> 16) as u8);
        if pad < 2 {
            out.push((accum >> 8) as u8);
        }
        if pad == 0 {
            out.push(accum as u8);
        }
    }
    Ok(out)
}

/// Fetch one binary blob via `file.download`
/// (`ArcanistBundle::getBlob()`, `src/parser/ArcanistBundle.php:709-738`).
///
/// PHP decodes the base64 response with `base64_decode()`; conduit
/// serves file bytes only through `file.download` (querydiffs returns
/// change dictionaries with `old:binary-phid`/`new:binary-phid`).
pub fn fetch_file_blob(call: &diff_submit::ConduitCall<'_>, phid: &str) -> Result<Vec<u8>, String> {
    let result = call("file.download", &serde_json::json!({"phid": phid}))?;
    match result.as_str() {
        Some(data) => base64_decode(data),
        None => Err("file.download returned an unexpected response".to_string()),
    }
}

/// Resolve binary bytes from inline data or a blob phid.
fn resolve_blob_bytes(
    direct: &Option<Vec<u8>>,
    phid: &Option<String>,
    loader: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<Vec<u8>>, String> {
    if let Some(data) = direct {
        return Ok(Some(data.clone()));
    }
    match phid {
        Some(id) => Ok(Some(loader(id)?)),
        None => Ok(None),
    }
}

/// Blob bytes plus git SHA-1: missing blobs become empty with zero SHA
/// (`ArcanistBundle.php:781-786,801-806`).
fn blob_sha_pair(data: Option<Vec<u8>>) -> (Vec<u8>, String) {
    match data {
        None => (Vec::new(), "0".repeat(40)),
        Some(bytes) => {
            let sha = git_blob_sha1(&bytes);
            (bytes, sha)
        }
    }
}

/// Old blob inputs for [`build_binary_change`].
fn old_blob_inputs(
    change: &BundleChange,
    old_source: Option<&BundleChange>,
) -> (Option<Vec<u8>>, Option<String>) {
    match old_source {
        Some(src) if src.original_file_data.is_some() => (src.original_file_data.clone(), None),
        Some(src) => (None, src.old_binary_phid.clone()),
        None => (
            change.original_file_data.clone(),
            change.old_binary_phid.clone(),
        ),
    }
}

/// Render one binary/image change (`ArcanistBundle::buildBinaryChange()`,
/// `src/parser/ArcanistBundle.php:740-819`).
///
/// `old_source` is the away change for moves/copies (from
/// [`collect_binary_sources`]); `loader` fetches blob bytes by phid
/// (local `blobs/` tar members or `file.download`). Emits `index`,
/// `GIT binary patch`, and both `literal` blocks with base85 bodies.
pub fn build_binary_change(
    change: &BundleChange,
    old_source: Option<&BundleChange>,
    loader: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    let (old_direct, old_phid) = old_blob_inputs(change, old_source);
    let old_data = resolve_blob_bytes(&old_direct, &old_phid, loader)?;
    let (old_bytes, old_sha) = blob_sha_pair(old_data);
    let new_data = resolve_blob_bytes(&change.current_file_data, &change.new_binary_phid, loader)?;
    let (new_bytes, new_sha) = blob_sha_pair(new_data);
    let mut content = String::new();
    content.push_str(&format!("index {old_sha}..{new_sha}\n"));
    content.push_str("GIT binary patch\n");
    content.push_str(&format!("literal {}\n", new_bytes.len()));
    content.push_str(&emit_binary_body(&new_bytes));
    content.push('\n');
    content.push_str(&format!("literal {}\n", old_bytes.len()));
    content.push_str(&emit_binary_body(&old_bytes));
    content.push('\n');
    Ok(content)
}

/// True for binary/image file types (`ArcanistBundle::isGitBinaryChange()`,
/// `src/parser/ArcanistBundle.php:487-491`).
fn is_binary_change(change: &BundleChange) -> bool {
    change.file_type == FILE_BINARY || change.file_type == FILE_IMAGE
}

/// Map away destinations to their away change for binary moves/copies
/// (`ArcanistBundle.php:321-335`). Built before multicopy decomposition.
pub fn collect_binary_sources(
    changes: &[BundleChange],
) -> std::collections::BTreeMap<String, BundleChange> {
    let mut sources = std::collections::BTreeMap::new();
    for change in changes {
        if !is_binary_change(change) {
            continue;
        }
        let away = matches!(
            change.change_type,
            x if x == TYPE_MOVE_AWAY || x == TYPE_COPY_AWAY || x == TYPE_MULTICOPY
        );
        if !away {
            continue;
        }
        for path in &change.away_paths {
            sources.insert(path.clone(), change.clone());
        }
    }
    sources
}

/// Git index/target paths for one change (shared by text and binary).
fn git_patch_paths(change: &BundleChange) -> (String, String, String, String) {
    let (old_index, old_target) = match &change.old_path {
        None => (
            format!("a/{}", change.current_path.clone().unwrap_or_default()),
            "/dev/null".to_string(),
        ),
        Some(old) => (format!("a/{old}"), format!("a/{old}")),
    };
    let (cur_index, cur_target) = match &change.current_path {
        None => (
            format!("b/{}", change.old_path.clone().unwrap_or_default()),
            "/dev/null".to_string(),
        ),
        Some(cur) => (format!("b/{cur}"), format!("b/{cur}")),
    };
    (old_index, old_target, cur_index, cur_target)
}

/// Append mode/copy/rename/delete headers for one change
/// (`ArcanistBundle.php:446-472`).
fn push_change_headers(change: &BundleChange, result: &mut String) {
    if change.change_type == TYPE_ADD {
        result.push_str(&format!("new file mode {}\n", change.defaulted_new_mode()));
    }
    if matches!(
        change.change_type,
        x if x == TYPE_COPY_HERE || x == TYPE_MOVE_HERE || x == TYPE_COPY_AWAY || x == TYPE_CHANGE
    ) && change.defaulted_old_mode() != change.defaulted_new_mode()
    {
        result.push_str(&format!("old mode {}\n", change.defaulted_old_mode()));
        result.push_str(&format!("new mode {}\n", change.defaulted_new_mode()));
    }
    if change.change_type == TYPE_COPY_HERE {
        result.push_str(&format!(
            "copy from {}\ncopy to {}\n",
            change.old_path.clone().unwrap_or_default(),
            change.current_path.clone().unwrap_or_default()
        ));
    } else if change.change_type == TYPE_MOVE_HERE {
        result.push_str(&format!(
            "rename from {}\nrename to {}\n",
            change.old_path.clone().unwrap_or_default(),
            change.current_path.clone().unwrap_or_default()
        ));
    } else if (change.change_type == TYPE_DELETE || change.change_type == TYPE_MULTICOPY)
        && let Some(old_mode) = &change.old_mode
    {
        result.push_str(&format!("deleted file mode {old_mode}\n"));
    }
}

/// Render bundle changes as a git patch with binary blobs
/// (`ArcanistBundle::toGitPatch()` full form, `:313-485`).
///
/// Like [`render_git_patch`] but binary/image changes render `GIT
/// binary` patches via `loader` (local `blobs/` members or
/// `file.download`) instead of failing. Text rendering is identical.
pub fn render_git_patch_with_blobs(
    changes: &mut Vec<BundleChange>,
    loader: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    let sources = collect_binary_sources(changes);
    decompose_multicopies(changes)?;
    let mut result = String::new();
    for change in changes.iter() {
        if change.file_type == FILE_DIRECTORY {
            continue;
        }
        if change.change_type == TYPE_MOVE_AWAY {
            continue;
        }
        let binary = is_binary_change(change);
        let body = if binary {
            let old_source = change
                .current_path
                .as_deref()
                .and_then(|cur| sources.get(cur));
            build_binary_change(change, old_source, loader)?
        } else {
            change.hunks.concat()
        };
        if change.change_type == TYPE_COPY_AWAY
            && body.is_empty()
            && change.defaulted_old_mode() == change.defaulted_new_mode()
        {
            continue;
        }
        let (old_index, old_target, cur_index, cur_target) = git_patch_paths(change);
        result.push_str(&format!("diff --git {old_index} {cur_index}\n"));
        push_change_headers(change, &mut result);
        if body.is_empty() {
            continue;
        }
        if !binary {
            result.push_str(&format!("--- {}\n", quote_git_target(&old_target)));
            result.push_str(&format!("+++ {}\n", quote_git_target(&cur_target)));
            result.push_str(&body);
            if !body.ends_with('\n') {
                result.push('\n');
            }
        } else {
            result.push_str(&body);
        }
    }
    result.push('\n');
    Ok(result)
}

/// Render bundle changes as a git patch (`ArcanistBundle::toGitPatch()`,
/// `src/parser/ArcanistBundle.php:313-485`).
///
/// Directories are skipped, move-aways ride with their move-here, and
/// multicopies decompose (T419); binary and image changes fail, since
/// `GIT binary` rendering needs conduit blobs (see the module docs).
pub fn render_git_patch(changes: &mut Vec<BundleChange>) -> Result<String, String> {
    decompose_multicopies(changes)?;
    let mut result = String::new();
    for change in changes.iter() {
        render_text_change(change, &mut result)?;
    }
    result.push('\n');
    Ok(result)
}

/// Render one text change into `result` (directories and move-aways are
/// skipped; binary and image changes fail).
fn render_text_change(change: &BundleChange, result: &mut String) -> Result<(), String> {
    if change.file_type == FILE_DIRECTORY {
        return Ok(());
    }
    if change.change_type == TYPE_MOVE_AWAY {
        return Ok(());
    }
    if change.file_type == FILE_IMAGE || change.file_type == FILE_BINARY {
        return Err("binary patches are not yet ported by arc patch".to_string());
    }
    let body: String = change.hunks.concat();
    if change.change_type == TYPE_COPY_AWAY
        && body.is_empty()
        && change.defaulted_old_mode() == change.defaulted_new_mode()
    {
        return Ok(());
    }
    let (old_index, old_target, cur_index, cur_target) = git_patch_paths(change);
    result.push_str(&format!("diff --git {old_index} {cur_index}\n"));
    push_change_headers(change, result);
    if body.is_empty() {
        return Ok(());
    }
    push_text_body(result, &old_target, &cur_target, &body);
    Ok(())
}

/// Append the `---`/`+++` headers and hunk body for one text change.
fn push_text_body(result: &mut String, old_target: &str, cur_target: &str, body: &str) {
    result.push_str(&format!("--- {}\n", quote_git_target(old_target)));
    result.push_str(&format!("+++ {}\n", quote_git_target(cur_target)));
    result.push_str(body);
    if !body.ends_with('\n') {
        result.push('\n');
    }
}

/// Decompose `MULTICOPY` changes into one `MOVE_HERE` plus `COPY_HERE`
/// siblings (T419, `ArcanistBundle.php:337-380`).
fn decompose_multicopies(changes: &mut Vec<BundleChange>) -> Result<(), String> {
    let multicopy_paths: Vec<Option<String>> = changes
        .iter()
        .filter(|change| change.change_type == TYPE_MULTICOPY)
        .map(|change| change.current_path.clone())
        .collect();
    for current in multicopy_paths {
        let sibling = changes
            .iter_mut()
            .find(|change| change.change_type == TYPE_COPY_HERE && change.old_path == current);
        let Some(sibling) = sibling else {
            return Err(
                "Failed to decompose multicopy changeset in order to generate diff.".to_string(),
            );
        };
        sibling.change_type = TYPE_MOVE_HERE;
    }
    changes.retain(|change| change.change_type != TYPE_MULTICOPY);
    Ok(())
}

/// Load an `--arcbundle` file (`ArcanistBundle::newFromArcBundle()`,
/// `src/parser/ArcanistBundle.php:132-185`).
///
/// Reads `meta.json` (when present) and `changes.json` plus `hunks/N`
/// members via system `tar`, then renders the git patch. `encoding`
/// overrides the bundle metadata encoding.
pub fn load_arcbundle(path: &str, encoding: Option<&str>) -> Result<Bundle, String> {
    let members = tar_list(path)?;
    let meta = arcbundle_meta(path, &members)?;
    require_arcbundle_member(path, &members, "changes.json")?;
    let raw = tar_extract(path, "changes.json")?;
    let dicts: Vec<serde_json::Value> = serde_json::from_slice(&raw)
        .map_err(|e| format!("failed to read arcbundle '{path}': {e}"))?;
    let decode_as = encoding.or(meta.encoding.as_deref());
    let mut changes = arcbundle_changes(path, &dicts, decode_as)?;
    let text = render_git_patch_with_blobs(&mut changes, &|phid: &str| {
        tar_extract(path, &format!("blobs/{phid}"))
    })?;
    let text = decode_bytes(text.as_bytes(), decode_as);
    Ok(Bundle {
        text,
        base_revision: meta.base_revision,
        revision_id: meta.revision_id,
        author_name: meta.author_name,
        author_email: meta.author_email,
    })
}

/// Read `meta.json` from an arcbundle, or defaults when absent (old
/// bundles predate `meta.json`).
fn arcbundle_meta(path: &str, members: &[String]) -> Result<BundleMeta, String> {
    if members.iter().any(|member| member == "meta.json") {
        Ok(parse_bundle_meta(&tar_extract(path, "meta.json")?))
    } else {
        Ok(BundleMeta::default())
    }
}

/// Fail when an arcbundle member is absent.
fn require_arcbundle_member(path: &str, members: &[String], member: &str) -> Result<(), String> {
    if members.iter().any(|candidate| candidate == member) {
        Ok(())
    } else {
        Err(format!(
            "failed to read arcbundle '{path}': member '{member}' is missing"
        ))
    }
}

/// Parse one [`BundleChange`] per dict, decoding each hunk corpus.
fn arcbundle_changes(
    path: &str,
    dicts: &[serde_json::Value],
    decode_as: Option<&str>,
) -> Result<Vec<BundleChange>, String> {
    let mut changes = Vec::new();
    for dict in dicts {
        changes.push(parse_bundle_change(
            dict,
            arcbundle_corpora(path, dict, decode_as)?,
        ));
    }
    Ok(changes)
}

/// Decode one change dict's hunk corpora from the `hunks/N` members.
fn arcbundle_corpora(
    path: &str,
    dict: &serde_json::Value,
    decode_as: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut hunks = Vec::new();
    if let Some(items) = dict.get("hunks").and_then(|v| v.as_array()) {
        for hunk in items {
            let index = hunk.get("corpus").and_then(|v| v.as_u64()).ok_or_else(|| {
                format!("failed to read arcbundle '{path}': hunk corpus is not an index")
            })?;
            let bytes = tar_extract(path, &format!("hunks/{index}"))?;
            hunks.push(decode_bytes(&bytes, decode_as));
        }
    }
    Ok(hunks)
}

/// One `differential.querydiffs` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    /// Diff ID.
    pub id: u64,
    /// Revision ID, when the row carries one.
    pub revision_id: Option<u64>,
    /// `sourceControlBaseRevision`.
    pub base_revision: String,
    /// Author display name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
}

/// Parse `differential.querydiffs` rows, which arrive as a map keyed by
/// diff ID (or, defensively, as a list).
pub fn parse_diff_rows(result: &serde_json::Value) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let values: Vec<&serde_json::Value> = match result {
        serde_json::Value::Object(map) => map.values().collect(),
        serde_json::Value::Array(items) => items.iter().collect(),
        _ => Vec::new(),
    };
    for value in values {
        let id = value.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        rows.push(DiffRow {
            id,
            revision_id: value.get("revisionID").and_then(|v| v.as_u64()),
            base_revision: value
                .get("sourceControlBaseRevision")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            author_name: value
                .get("authorName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            author_email: value
                .get("authorEmail")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    rows.sort_by_key(|row| row.id);
    rows
}

/// Conduit method returning the raw diff text.
const METHOD_GET_RAW_DIFF: &str = "differential.getrawdiff";

/// Extract raw-diff text: a bare string, or `.rawDiff`/`.diff` (the
/// established lenient shapes).
pub fn raw_diff_text(result: &serde_json::Value) -> Option<String> {
    if let Some(text) = result.as_str() {
        return Some(text.to_string());
    }
    result
        .get("rawDiff")
        .or_else(|| result.get("diff"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Load a Conduit bundle: resolve diffs via `differential.querydiffs`,
/// take the latest, and fetch its text via `differential.getrawdiff`.
///
/// Ports the metadata half of
/// `ArcanistWorkflow::loadRevisionBundleFromConduit()` /
/// `loadDiffBundleFromConduit()` (`:1361-1400`); hunks come from
/// `getrawdiff` instead of rendered change dictionaries (see the module
/// docs). Empty diff lists fail with [`NO_DIFF_OR_REVISION`].
pub fn fetch_conduit_bundle(
    call: &diff_submit::ConduitCall<'_>,
    params: &serde_json::Value,
    revision_id: Option<u64>,
) -> Result<Bundle, String> {
    let result = call(diff_submit::METHOD_QUERY_DIFFS, params)?;
    let rows = parse_diff_rows(&result);
    let Some(diff) = rows.into_iter().next_back() else {
        return Err(NO_DIFF_OR_REVISION.to_string());
    };
    let text = call(
        METHOD_GET_RAW_DIFF,
        &serde_json::json!({"diffID": diff.id.to_string()}),
    )
    .map_err(|e| e.to_string())
    .and_then(|result| {
        raw_diff_text(&result)
            .ok_or_else(|| "differential.getrawdiff returned an unexpected response".to_string())
    })?;
    Ok(Bundle {
        text,
        base_revision: Some(diff.base_revision).filter(|base| !base.is_empty()),
        revision_id: revision_id.or(diff.revision_id),
        author_name: Some(diff.author_name).filter(|name| !name.is_empty()),
        author_email: Some(diff.author_email).filter(|email| !email.is_empty()),
    })
}

/// Load the bundle for `--revision` / positional `D123`
/// (`loadRevisionBundleFromConduit()`, `:1361-1370`).
pub fn fetch_revision_bundle(
    call: &diff_submit::ConduitCall<'_>,
    revision_id: u64,
) -> Result<Bundle, String> {
    fetch_conduit_bundle(
        call,
        &serde_json::json!({"revisionIDs": [revision_id]}),
        Some(revision_id),
    )
}

/// Load the bundle for `--diff` (`loadDiffBundleFromConduit()`,
/// `:1372-1400`).
pub fn fetch_diff_bundle(
    call: &diff_submit::ConduitCall<'_>,
    diff_id: u64,
) -> Result<Bundle, String> {
    fetch_conduit_bundle(call, &serde_json::json!({"ids": [diff_id]}), None)
}

/// Working-copy VCS for `patch` (all three are patchable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vcs {
    /// Git.
    Git,
    /// Mercurial.
    Hg,
    /// Subversion.
    Svn,
}

impl Vcs {
    /// VCS name, as used in `supports` errors.
    pub fn name(self) -> &'static str {
        match self {
            Vcs::Git => "git",
            Vcs::Hg => "hg",
            Vcs::Svn => "svn",
        }
    }
}

/// Custom hg/svn backends for tests; git always uses the system binary.
#[derive(Debug, Clone, Default)]
pub struct Backends {
    /// Mercurial command builder.
    pub hg: hg::HgCommand,
    /// Subversion command builder.
    pub svn: svn::SvnCommand,
}

/// Detect the patchable VCS: git work tree, `.hg` dir, then svn metadata.
fn detect_vcs(dir: &Path) -> Option<Vcs> {
    if git::is_inside_work_tree(dir) {
        return Some(Vcs::Git);
    }
    if dir
        .ancestors()
        .any(|ancestor| ancestor.join(".hg").exists())
    {
        return Some(Vcs::Hg);
    }
    if svn::find_metadata_path(dir).is_some() {
        return Some(Vcs::Svn);
    }
    None
}

/// Working-tree status buckets for the clean check
/// (`ArcanistWorkflow::requireCleanWorkingCopy()`,
/// `src/workflow/ArcanistWorkflow.php:1020-1258`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeStatus {
    /// Incompletely checked out directories (svn).
    pub incomplete: Vec<String>,
    /// Merge conflicts.
    pub conflicts: Vec<String>,
    /// Missing files (svn).
    pub missing: Vec<String>,
    /// Dirty externals/submodules (git/hg).
    pub externals: Vec<String>,
    /// Untracked files.
    pub untracked: Vec<String>,
    /// Staged but uncommitted files (git/hg), minus externals/unstaged.
    pub uncommitted: Vec<String>,
    /// Unstaged files (git/hg), minus externals.
    pub unstaged: Vec<String>,
}

/// Read the git working-tree status via `git status --porcelain=2 -z`
/// (`ArcanistGitAPI::buildUncommittedStatusViaStatus()`, `:645-706`).
///
/// Unmerged (`u`) entries surface as uncommitted/unstaged, never as
/// conflicts — PHP never sets `FLAG_CONFLICT` on that path either.
pub fn git_tree_status(dir: &Path) -> Result<TreeStatus, String> {
    let output = git::output(dir, &["status", "--porcelain=2", "-z"])?;
    parse_porcelain_v2(&output.stdout)
}

/// Parse `git status --porcelain=2 -z` bytes into status buckets.
pub fn parse_porcelain_v2(output: &[u8]) -> Result<TreeStatus, String> {
    let text = String::from_utf8_lossy(output);
    let mut parts: VecDeque<&str> = text.split('\0').collect();
    let mut status = TreeStatus::default();
    while parts.len() > 1 {
        let entry = parts.pop_front().unwrap_or("");
        if entry.is_empty() {
            continue;
        }
        parse_porcelain_entry(entry, &mut parts, &mut status);
    }
    sort_status(&mut status);
    Ok(status)
}

/// Parse one porcelain entry into the status buckets, consuming the rename
/// origin for `2` entries.
fn parse_porcelain_entry(entry: &str, parts: &mut VecDeque<&str>, status: &mut TreeStatus) {
    let kind = entry.as_bytes().first().copied().unwrap_or(b' ');
    if kind == b'?' {
        if let Some(path) = entry.split(' ').nth(1) {
            status.untracked.push(path.to_string());
        }
        return;
    }
    let fields: Vec<&str> = entry.split(' ').collect();
    if fields.len() < 3 {
        return;
    }
    let Some((path, xy, subm)) = porcelain_indexed_parts(kind, &fields) else {
        return;
    };
    let xy = xy.as_bytes();
    let unstaged = xy.get(1).is_some_and(|state| *state != b'.');
    let subm = subm.as_bytes();
    let external = subm.get(2) == Some(&b'M') || subm.get(3) == Some(&b'U');
    push_porcelain_entry(status, path, unstaged, external);
    if kind == b'2'
        && let Some(orig) = parts.pop_front()
        && !orig.is_empty()
    {
        push_porcelain_entry(status, orig, unstaged, external);
    }
}

/// Extract the `(path, xy, submodule)` fields of a `1`/`2`/`u` entry.
fn porcelain_indexed_parts<'a>(
    kind: u8,
    fields: &[&'a str],
) -> Option<(&'a str, &'a str, &'a str)> {
    match kind {
        b'1' if fields.len() >= 9 => Some((fields[8], fields[1], fields[2])),
        b'2' if fields.len() >= 10 => Some((fields[9], fields[1], fields[2])),
        b'u' if fields.len() >= 11 => Some((fields[10], fields[1], fields[2])),
        _ => None,
    }
}

/// Record one path as external and unstaged or uncommitted.
fn push_porcelain_entry(status: &mut TreeStatus, path: &str, unstaged: bool, external: bool) {
    if external {
        status.externals.push(path.to_string());
    }
    if unstaged {
        status.unstaged.push(path.to_string());
    } else {
        status.uncommitted.push(path.to_string());
    }
}

/// Sort every status bucket (PHP `ksort`s the status map).
fn sort_status(status: &mut TreeStatus) {
    status.incomplete.sort();
    status.conflicts.sort();
    status.missing.sort();
    status.externals.sort();
    status.untracked.sort();
    status.uncommitted.sort();
    status.unstaged.sort();
}

/// Split `U ` (unresolved) lines out of `hg status` output.
///
/// The `hg` port rejects `U` lines (`Unknown Mercurial status 'U'`);
/// PHP maps them to conflicts, so collect them here and parse the rest.
pub fn split_hg_conflicts(output: &str) -> (Vec<String>, String) {
    let mut conflicts = Vec::new();
    let mut rest = Vec::new();
    for line in output.split('\n') {
        if let Some(path) = line.strip_prefix("U ") {
            conflicts.push(path.to_string());
        } else {
            rest.push(line);
        }
    }
    (conflicts, rest.join("\n"))
}

/// Read the Mercurial working-tree status via `hg status`
/// (`ArcanistMercurialAPI::buildUncommittedStatus()`, `:313-329`).
///
/// Tracked non-untracked files count as uncommitted, like PHP.
pub fn hg_tree_status(dir: &Path, hg: &hg::HgCommand, args: &[&str]) -> Result<TreeStatus, String> {
    let output = hg::stdout_text_with(dir, hg, args)?;
    let (conflicts, rest) = split_hg_conflicts(&output);
    let map = hg::parse_mercurial_status(&rest)?;
    let mut status = TreeStatus {
        conflicts,
        ..TreeStatus::default()
    };
    for (path, mut mask) in map {
        if mask & hg::FLAG_UNTRACKED == 0 {
            mask |= hg::FLAG_UNCOMMITTED;
        }
        if mask & hg::FLAG_CONFLICT != 0 {
            status.conflicts.push(path.clone());
        }
        if mask & hg::FLAG_MISSING != 0 {
            status.missing.push(path.clone());
        }
        if mask & hg::FLAG_UNSTAGED != 0 {
            status.unstaged.push(path.clone());
        }
        if mask & hg::FLAG_UNCOMMITTED != 0 {
            status.uncommitted.push(path.clone());
        }
        if mask & hg::FLAG_EXTERNALS != 0 {
            status.externals.push(path.clone());
        }
        if mask & hg::FLAG_UNTRACKED != 0 {
            status.untracked.push(path);
        }
    }
    sort_status(&mut status);
    Ok(status)
}

/// Read one `<entry path="...">` block's `wc-status` attributes from
/// `svn --xml status` output.
fn svn_entry_status(block: &str) -> Option<(String, String, String, bool)> {
    let path = svn_attr(block, "<entry", "path")?;
    let tag = block.find("<wc-status")?;
    let tag = &block[tag..];
    let end = tag.find('>')?;
    let tag = &tag[..end];
    let props = svn_attr(tag, "<wc-status", "props").unwrap_or_else(|| "none".to_string());
    let item = svn_attr(tag, "<wc-status", "item").unwrap_or_else(|| "none".to_string());
    let conflicted = tag.contains("tree-conflicted=");
    Some((path, props, item, conflicted))
}

/// Read one XML attribute value after `tag` in `text`.
fn svn_attr(text: &str, tag: &str, attr: &str) -> Option<String> {
    let start = text.find(tag)?;
    let rest = &text[start + tag.len()..];
    let key = format!("{attr}=\"");
    let value_at = rest.find(&key)? + key.len();
    let end = rest[value_at..].find('"')?;
    Some(rest[value_at..value_at + end].to_string())
}

/// Map one svn `item` status to mask flags
/// (`ArcanistSubversionAPI::parseSVNStatus()`, `:151-182`).
fn svn_item_flags(item: &str) -> Result<(Vec<&'static str>, bool), String> {
    let mut buckets = Vec::new();
    let mut external = false;
    match item {
        "none" | "normal" => {}
        "external" => {
            buckets.push("externals");
            external = true;
        }
        "unversioned" => buckets.push("untracked"),
        "missing" => buckets.push("missing"),
        "conflicted" => buckets.push("conflicts"),
        "incomplete" => buckets.push("incomplete"),
        "added" | "replaced" | "modified" | "deleted" | "obstructed" => {}
        _ => return Err(format!("Unrecognized item status '{item}'.")),
    }
    Ok((buckets, external))
}

/// Parse `svn --xml status` output into status buckets
/// (`ArcanistSubversionAPI::getSVNStatus()`, `:69-149`).
///
/// Like PHP, plain modifications never set uncommitted/unstaged buckets,
/// so the uncommitted check passes silently under svn.
pub fn parse_svn_status_xml(output: &str) -> Result<TreeStatus, String> {
    let mut status = TreeStatus::default();
    let mut externals: Vec<String> = Vec::new();
    let mut rest = output;
    while let Some(start) = rest.find("<entry") {
        let block_end = rest[start..]
            .find("</entry>")
            .map(|end| start + end)
            .unwrap_or(rest.len());
        let block = &rest[start..block_end];
        rest = &rest[block_end.min(rest.len())..];
        parse_svn_entry_block(block, &mut status, &mut externals)?;
    }
    prune_svn_externals(&mut status, &externals);
    sort_status(&mut status);
    Ok(status)
}

/// Parse one `<entry>` block into the status buckets.
fn parse_svn_entry_block(
    block: &str,
    status: &mut TreeStatus,
    externals: &mut Vec<String>,
) -> Result<(), String> {
    let Some((mut path, props, item, conflicted)) = svn_entry_status(block) else {
        return Ok(());
    };
    path = path.replace('\\', "/");
    if !matches!(props.as_str(), "none" | "normal" | "modified") {
        return Err(format!("Unrecognized property status '{props}'."));
    }
    let (mut buckets, external) = svn_item_flags(&item)?;
    if external {
        externals.push(path.clone());
    }
    if conflicted {
        buckets.push("conflicts");
    }
    for bucket in buckets {
        push_svn_bucket(status, bucket, &path);
    }
    Ok(())
}

/// Push one path into the named status bucket.
fn push_svn_bucket(status: &mut TreeStatus, bucket: &str, path: &str) {
    match bucket {
        "externals" => status.externals.push(path.to_string()),
        "untracked" => status.untracked.push(path.to_string()),
        "missing" => status.missing.push(path.to_string()),
        "conflicts" => status.conflicts.push(path.to_string()),
        "incomplete" => status.incomplete.push(path.to_string()),
        _ => {}
    }
}

/// Drop external paths from the plain buckets (externals are reported
/// separately, not as untracked/missing/conflicted/incomplete).
fn prune_svn_externals(status: &mut TreeStatus, externals: &[String]) {
    for path in externals {
        for bucket in [
            &mut status.untracked,
            &mut status.missing,
            &mut status.conflicts,
            &mut status.incomplete,
        ] {
            bucket.retain(|candidate| candidate != path);
        }
    }
}

/// Read the svn working-tree status via `svn --xml status`.
pub fn svn_tree_status(
    dir: &Path,
    svn: &svn::SvnCommand,
    args: &[&str],
) -> Result<TreeStatus, String> {
    let output = svn::stdout_text_with(dir, svn, args)?;
    parse_svn_status_xml(&output)
}

/// `  Working copy: <path>` block (`:1025-1028`) with an explicit ANSI
/// setting.
///
/// PHP wraps the path in `__...__` (underline on TTYs, stripped
/// otherwise); `ansi=false` renders byte-identical to the plain shape.
fn working_copy_desc_ansi(dir: &Path, ansi: bool) -> String {
    let path = dir.display().to_string();
    terminal::tsprintf_ansi(
        "  Working copy: __%R__\n\n",
        &[terminal::TVal::Text(&path)],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_else(|_| format!("  Working copy: {path}\n\n"))
}

/// Whether `arc.autostash` configuration permits autostashing
/// (`$this->getConfigFromAnySource('arc.autostash')`, `:1201`).
///
/// Reads `.arcconfig` from `dir` upward; truthy values enable.
pub fn autostash_permitted(dir: &Path) -> bool {
    let value = dir.ancestors().find_map(|ancestor| {
        let text = std::fs::read_to_string(ancestor.join(".arcconfig")).ok()?;
        let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
        doc.get("arc.autostash").cloned()
    });
    match value {
        Some(serde_json::Value::Bool(enabled)) => enabled,
        Some(serde_json::Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
        Some(serde_json::Value::String(text)) => {
            !text.is_empty() && !matches!(text.to_lowercase().as_str(), "false" | "0" | "no")
        }
        _ => false,
    }
}

/// Stash state after the clean check: pop when stashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashState {
    /// Nothing stashed.
    Clean,
    /// `git stash` ran; pop it when the flow ends.
    Stashed,
}

/// Require a clean working copy
/// (`ArcanistWorkflow::requireCleanWorkingCopy()`, `:1020-1258`).
///
/// `--force` skips the whole check (`ArcanistPatchWorkflow.php:312-314`).
/// Incomplete directories, conflicts, and missing files fail with the
/// verbatim errors; dirty externals and untracked files prompt (declining
/// aborts); uncommitted/unstaged changes prompt to create a commit —
/// answering yes fails with port guidance (no interactive editor), and
/// answering no autostashes under git with `arc.autostash`, else fails
/// with the verbatim error. Returns whether a stash must be popped.
pub fn require_clean_tree(
    dir: &Path,
    vcs: Vcs,
    backends: &Backends,
    io: &mut Io,
    force: bool,
) -> Result<StashState, String> {
    require_clean_tree_ansi(dir, vcs, backends, io, force, terminal::ansi_enabled())
}

/// [`require_clean_tree`] with an explicit ANSI setting.
pub fn require_clean_tree_ansi(
    dir: &Path,
    vcs: Vcs,
    backends: &Backends,
    io: &mut Io,
    force: bool,
    ansi: bool,
) -> Result<StashState, String> {
    if force {
        return Ok(StashState::Clean);
    }
    let mut status = match vcs {
        Vcs::Git => git_tree_status(dir)?,
        Vcs::Hg => hg_tree_status(dir, &backends.hg, &["status"])?,
        Vcs::Svn => svn_tree_status(dir, &backends.svn, &["--xml", "status"])?,
    };
    let desc = working_copy_desc_ansi(dir, ansi);
    fail_on_fatal_tree_state(&status, &desc)?;
    // Subversion externals are handled elsewhere; never prompt (`:1076-1080`).
    if vcs == Vcs::Svn {
        status.externals.clear();
    }
    confirm_tree_externals(io, &status)?;
    confirm_tree_untracked(io, vcs, &status, &desc)?;
    prune_tree_overlaps(&mut status);
    if status.unstaged.is_empty() && status.uncommitted.is_empty() {
        return Ok(StashState::Clean);
    }
    resolve_uncommitted_tree(dir, vcs, io, &status, &desc)
}

/// Fail on incomplete directories, conflicts, and missing files with the
/// verbatim errors.
fn fail_on_fatal_tree_state(status: &TreeStatus, desc: &str) -> Result<(), String> {
    if !status.incomplete.is_empty() {
        // The trailing `')` is verbatim from PHP (`:1036-1038`).
        return Err(format!(
            "You have incompletely checked out directories in this working copy. Fix them \
             before proceeding.')\n\n{}  Incomplete directories in working copy:\n    {}\n\nYou \
             can fix these paths by running 'svn update' on them.",
            desc,
            status.incomplete.join("\n    ")
        ));
    }
    if !status.conflicts.is_empty() {
        return Err(format!(
            "You have merge conflicts in this working copy. Resolve merge conflicts before \
             proceeding.\n\n{}  Conflicts in working copy:\n    {}",
            desc,
            status.conflicts.join("\n    ")
        ));
    }
    if !status.missing.is_empty() {
        return Err(format!(
            "You have missing files in this working copy. Revert or formally remove them (with \
             `svn rm`) before proceeding.\n\n{}  Missing files in working copy:\n    {}\n",
            desc,
            status.missing.join("\n    ")
        ));
    }
    Ok(())
}

/// Prompt to ignore dirty externals/submodules (declining aborts).
fn confirm_tree_externals(io: &mut Io, status: &TreeStatus) -> Result<(), String> {
    if status.externals.is_empty() {
        return Ok(());
    }
    let count = status.externals.len();
    io.push_out(&format!(
        "{count} submodule(s) have uncommitted or untracked changes:\n"
    ));
    for external in &status.externals {
        io.push_out(&format!("    {external}\n"));
    }
    let query = format!("Ignore the changes to these {count} submodule(s) and continue?");
    if !land::console_confirm(io, &query, false)? {
        return Err(land::ABORT.to_string());
    }
    Ok(())
}

/// Prompt to ignore untracked files (declining aborts).
fn confirm_tree_untracked(
    io: &mut Io,
    vcs: Vcs,
    status: &TreeStatus,
    desc: &str,
) -> Result<(), String> {
    if status.untracked.is_empty() {
        return Ok(());
    }
    let count = status.untracked.len();
    let place = match vcs {
        Vcs::Git => ".git/info/exclude",
        Vcs::Svn => "svn:ignore",
        Vcs::Hg => ".hgignore",
    };
    io.push_out(&format!(
        "You have untracked files in this working copy.\n\n{desc}"
    ));
    io.push_out(&format!(
        "  Untracked changes in working copy:\n  (To ignore these {count} change(s), add them \
         to \"{place}\".)\n    {}\n",
        status.untracked.join("\n    ")
    ));
    let query = format!("Ignore these {count} untracked file(s) and continue?");
    if !land::console_confirm(io, &query, true)? {
        return Err(land::ABORT.to_string());
    }
    Ok(())
}

/// Drop externals from the unstaged bucket and unstaged/externals from the
/// uncommitted bucket.
fn prune_tree_overlaps(status: &mut TreeStatus) {
    status
        .unstaged
        .retain(|path| !status.externals.contains(path));
    status
        .uncommitted
        .retain(|path| !status.unstaged.contains(path) && !status.externals.contains(path));
}

/// Print the uncommitted/unstaged lists and resolve them: offering a new
/// commit (yes fails without an interactive editor), autostashing under
/// git with `arc.autostash`, else failing with the verbatim error.
fn resolve_uncommitted_tree(
    dir: &Path,
    vcs: Vcs,
    io: &mut Io,
    status: &TreeStatus,
    desc: &str,
) -> Result<StashState, String> {
    io.push_out(&format!(
        "You have uncommitted changes in this working copy.\n\n{desc}"
    ));
    let mut lists = Vec::new();
    if !status.unstaged.is_empty() {
        lists.push(format!(
            "  Unstaged changes in working copy:\n    {}",
            status.unstaged.join("\n    ")
        ));
    }
    if !status.uncommitted.is_empty() {
        lists.push(format!(
            "Uncommitted changes in working copy:\n    {}",
            status.uncommitted.join("\n    ")
        ));
    }
    io.push_out(&format!("{}\n", lists.join("\n\n")));
    let count = status.unstaged.len() + status.uncommitted.len();
    // Amend detection is not ported (see the module docs): always offer a
    // new commit (`getAskForAddPrompt()`, `:1348-1359`).
    let query = format!("Do you want to create a new commit with these {count} change(s)?");
    if land::console_confirm(io, &query, true)? {
        return Err(
            "Can not commit changes without an interactive editor. Commit, stash, or discard \
             them manually, then run \"arc patch\" again."
                .to_string(),
        );
    }
    if autostash_permitted(dir) && vcs == Vcs::Git {
        io.push_out("Stashing uncommitted changes. (You can restore them with `git stash pop`).\n");
        git::stdout_text(dir, &["stash"])?;
        return Ok(StashState::Stashed);
    }
    Err(
        "You can not continue with uncommitted changes. Commit or discard them before \
         proceeding."
            .to_string(),
    )
}

/// Pop an autostash taken by [`require_clean_tree`]
/// (`ArcanistGitAPI::unstashChanges()`, `:1442-1444`).
pub fn unstash_changes(dir: &Path, stash: StashState) -> Result<(), String> {
    if stash == StashState::Stashed {
        git::stdout_text(dir, &["stash", "pop"])?;
    }
    Ok(())
}

/// Update the working copy before patching
/// (`ArcanistPatchWorkflow.php:309-320`).
///
/// Prints `Updating working copy...`, runs `git pull` / `hg up` /
/// `svn up`, then prints `Done.`.
pub fn update_working_copy(
    dir: &Path,
    vcs: Vcs,
    backends: &Backends,
    io: &mut Io,
) -> Result<(), String> {
    io.push_out("Updating working copy...\n");
    match vcs {
        Vcs::Git => {
            git::stdout_text(dir, &["pull"])?;
        }
        Vcs::Hg => {
            hg::stdout_text_with(dir, &backends.hg, &["up"])?;
        }
        Vcs::Svn => {
            svn::stdout_text_with(dir, &backends.svn, &["up"])?;
        }
    }
    io.push_out("Done.\n");
    Ok(())
}

/// True when the VCS can branch and commit (git and hg, not svn).
#[must_use]
pub fn can_branch(vcs: Vcs) -> bool {
    !matches!(vcs, Vcs::Svn)
}

/// True when patching should branch (`!--nobranch`).
///
/// Callers gate on [`can_branch`] separately, like PHP
/// (`shouldBranch()`, `:203-209`).
#[must_use]
pub fn should_branch(options: &Options) -> bool {
    !options.nobranch
}

/// True when patching should commit (`!--nocommit`).
///
/// Only the git/hg flows commit, so svn never commits structurally
/// (`shouldCommit()`, `:193-195`).
#[must_use]
pub fn should_commit(options: &Options) -> bool {
    !options.nocommit
}

/// True when patching should follow dependencies: no
/// `--skip-dependencies`, a revision ID on the bundle, and never under
/// svn (`shouldApplyDependencies()`, `:166-179`, plus
/// `buildDependencyGraph()`, `:1117-1119`).
#[must_use]
pub fn should_apply_dependencies(bundle: &Bundle, options: &Options, vcs: Vcs) -> bool {
    !options.skip_dependencies && bundle.revision_id.is_some() && !matches!(vcs, Vcs::Svn)
}

/// One revision node for dependency ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyNode {
    /// Revision PHID.
    pub phid: String,
    /// Revision ID.
    pub id: u64,
    /// `phabricator:depends-on` PHIDs.
    pub depends_on: Vec<String>,
}

/// Parse a legacy `differential.query` row into a dependency node.
pub fn parse_dependency_node(row: &serde_json::Value) -> Option<DependencyNode> {
    let phid = row.get("phid")?.as_str()?.to_string();
    let id = row.get("id")?.as_u64()?;
    let depends_on = row
        .get("auxiliary")
        .and_then(|aux| aux.get("phabricator:depends-on"))
        .and_then(|deps| deps.as_array())
        .map(|deps| {
            deps.iter()
                .filter_map(|dep| dep.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some(DependencyNode {
        phid,
        id,
        depends_on,
    })
}

/// Load the transitive dependency graph for `start_id`
/// (`buildDependencyGraph()`, `:1115-1144`, plus
/// `ArcanistDifferentialDependencyGraph::loadEdges()`, `:33-56`).
///
/// Expands `phabricator:depends-on` edges breadth-first via
/// `differential.query`, pruning revisions whose commit hash is already
/// local (`has_local_commit`). Returns nodes in discovery order.
pub fn load_dependency_nodes(
    call: &diff_submit::ConduitCall<'_>,
    start_id: u64,
    has_local_commit: &dyn Fn(Option<&str>) -> bool,
    hash_key: &str,
) -> Result<(Vec<DependencyNode>, String), String> {
    let result = call(
        diff_submit::METHOD_QUERY,
        &serde_json::json!({"ids": [start_id]}),
    )?;
    let rows = result.as_array().cloned().unwrap_or_default();
    let first = rows.first().and_then(parse_dependency_node);
    let Some(start) = first else {
        return Ok((Vec::new(), String::new()));
    };
    let start_phid = start.phid.clone();
    let mut nodes: Vec<DependencyNode> = vec![start];
    let mut seen = vec![start_phid.clone()];
    let mut queue: VecDeque<String> = nodes[0].depends_on.iter().cloned().collect();
    while let Some(phid) = queue.pop_front() {
        if seen.contains(&phid) {
            continue;
        }
        seen.push(phid.clone());
        let result = call(
            diff_submit::METHOD_QUERY,
            &serde_json::json!({"phids": [phid]}),
        )?;
        let rows = result.as_array().cloned().unwrap_or_default();
        let Some(node) = rows.first().and_then(parse_dependency_node) else {
            continue;
        };
        let hash = rows
            .first()
            .and_then(|row| row.get("hashes"))
            .and_then(|hashes| hashes.get(hash_key))
            .and_then(|hash| hash.as_str());
        if has_local_commit(hash) {
            continue;
        }
        for dep in &node.depends_on {
            if !seen.contains(dep) {
                queue.push_back(dep.clone());
            }
        }
        nodes.push(node);
    }
    Ok((nodes, start_phid))
}

/// Order dependency nodes parents-first, reporting cycles.
///
/// Ports the `AbstractDirectedGraph` half of dependency application:
/// Kahn's algorithm over the depends-on edges, with leftover nodes (a
/// cycle) appended in discovery order and `cyclic` set. The start node
/// sorts last but the caller skips it when applying.
pub fn order_dependencies(nodes: &[DependencyNode]) -> (Vec<usize>, bool) {
    let index_of = |phid: &str| nodes.iter().position(|node| node.phid == phid);
    let mut applied = vec![false; nodes.len()];
    let mut order = Vec::new();
    loop {
        let mut progressed = false;
        for (index, node) in nodes.iter().enumerate() {
            if applied[index] {
                continue;
            }
            let ready = node
                .depends_on
                .iter()
                .all(|dep| index_of(dep).is_none_or(|dep_index| applied[dep_index]));
            if ready {
                applied[index] = true;
                order.push(index);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    let mut cyclic = false;
    for (index, done) in applied.iter().enumerate() {
        if !done {
            cyclic = true;
            order.push(index);
        }
    }
    (order, cyclic)
}

/// Legacy `CLOSED` revision status
/// (`ArcanistDifferentialRevisionStatus::CLOSED`, `:8`).
const STATUS_CLOSED: i64 = 3;

/// Find the latest closed revision matching a commit hash
/// (`loadRevisionFromHash()`, `:1088-1113`).
///
/// Queries `differential.query` with `commitHashes`, sorts by
/// `dateModified`, and returns the latest `Closed` row's monogram suffix
/// (` \ D123`), if any.
pub fn revision_suffix_for_hash(
    call: &diff_submit::ConduitCall<'_>,
    hash_type: &str,
    hash: &str,
) -> Option<String> {
    let result = call(
        diff_submit::METHOD_QUERY,
        &serde_json::json!({"commitHashes": [[hash_type, hash]]}),
    )
    .ok()?;
    let mut rows = result.as_array().cloned().unwrap_or_default();
    rows.sort_by_key(|row| {
        row.get("dateModified")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    });
    let mut found = None;
    for row in &rows {
        if row.get("status").and_then(|v| v.as_i64()) == Some(STATUS_CLOSED)
            && let Some(id) = row.get("id").and_then(|v| v.as_u64())
        {
            found = Some(format!(" \\ D{id}"));
        }
    }
    found
}

/// Check the bundle base revision against the working copy
/// (`sanityCheck()`, `:994-1062`).
///
/// Skips when the VCS has no local commits (svn), when the bundle has no
/// base, or when the base commit is local. Otherwise resolves ` \ D123`
/// suffixes for the bundle base and the working-copy revision through
/// `call` (when authenticated) and prompts
/// `This diff is against commit %s, but the commit is nowhere in the
/// working copy. Try to apply it against the current working copy state?
/// (%s)` (default yes); declining aborts.
pub fn sanity_check(
    dir: &Path,
    vcs: Vcs,
    backends: &Backends,
    io: &mut Io,
    call: Option<&diff_submit::ConduitCall<'_>>,
    bundle: &Bundle,
    has_local_commit: bool,
) -> Result<(), String> {
    let Some(hash_type) = sanity_hash_type(vcs) else {
        return Ok(());
    };
    if !bundle.has_base_revision() || has_local_commit {
        return Ok(());
    }
    let base = bundle.base_revision.clone().unwrap_or_default();
    let source_base = working_source_revision(dir, vcs, backends)?;
    let bundle_str = suffixed_revision(call, hash_type, &base);
    let source_str = suffixed_revision(call, hash_type, &source_base);
    let query = format!(
        "This diff is against commit {bundle_str}, but the commit is nowhere in the working \
         copy. Try to apply it against the current working copy state? ({source_str})"
    );
    if !land::console_confirm(io, &query, false)? {
        return Err(land::ABORT.to_string());
    }
    Ok(())
}

/// Commit-hash query key for the sanity check (`None` under svn, which has
/// no local commits to check).
fn sanity_hash_type(vcs: Vcs) -> Option<&'static str> {
    match vcs {
        Vcs::Git => Some("gtcm"),
        Vcs::Hg => Some("hgcm"),
        Vcs::Svn => None,
    }
}

/// Current working-copy revision (git `HEAD` or the hg `.` node).
fn working_source_revision(dir: &Path, vcs: Vcs, backends: &Backends) -> Result<String, String> {
    match vcs {
        Vcs::Git => git::working_copy_revision(dir),
        Vcs::Hg => hg::stdout_text_with(
            dir,
            &backends.hg,
            &["log", "-r", ".", "--template", "{node}"],
        ),
        // Unreachable: svn returns early via [`sanity_hash_type`].
        Vcs::Svn => Ok(String::new()),
    }
}

/// A commit hash plus its ` \ D123` suffix when Conduit reports a closed
/// revision for it.
fn suffixed_revision(
    call: Option<&diff_submit::ConduitCall<'_>>,
    hash_type: &str,
    hash: &str,
) -> String {
    let mut text = hash.to_string();
    if let Some(call) = call
        && let Some(suffix) = revision_suffix_for_hash(call, hash_type, hash)
    {
        text.push_str(&suffix);
    }
    text
}

/// Exact `$EDITOR` template for patch commit messages
/// (`ArcanistPatchWorkflow.php:954-965`).
pub const COMMIT_TEMPLATE: &str = "\n\n# Enter a commit message.\n#\n# This message will be associated with a Differential revision.\n#\n# Differential Revision: <URL or ID of the Differential Revision>\n#\n";

/// Load a patch commit message via `differential.getcommitmessage`
/// with `{revision_id}` (`getCommitMessage()`, `:924-940`).
pub fn fetch_patch_commit_message(
    call: &diff_submit::ConduitCall<'_>,
    revision_id: u64,
) -> Result<String, String> {
    let result = call(
        diff_submit::METHOD_GET_COMMIT_MESSAGE,
        &serde_json::json!({"revision_id": revision_id}),
    )?;
    result
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "differential.getcommitmessage response is not a string".to_string())
}

/// Temporary patch file, removed on drop (like PHP `TempFile`).
pub struct TempPatchFile {
    /// File path.
    pub path: std::path::PathBuf,
}

impl TempPatchFile {
    /// Write `text` to a unique temp file.
    pub fn write(text: &str) -> Result<Self, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("arc-rs-patch-{}-{id}.patch", std::process::id()));
        std::fs::write(&path, text)
            .map_err(|e| format!("Failed to write temporary patch file: {e}"))?;
        Ok(Self { path })
    }
}

impl Drop for TempPatchFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// Pick a git branch name: `arcpatch`, plus `-D{id}` when the bundle has
/// a revision ID, plus the first free suffix (`getBranchName()`,
/// `:211-250`).
///
/// `exists` reports whether a name is taken (`git rev-parse --verify`).
/// Taken names print `Branch name %s already exists; trying a new name.`;
/// exhausting `null, _1, _2, _3` fails with the verbatim error.
pub fn git_branch_name(
    bundle: &Bundle,
    exists: &dyn Fn(&str) -> bool,
    io: &mut Io,
) -> Result<String, String> {
    let mut base = "arcpatch".to_string();
    if let Some(id) = bundle.revision_id {
        base.push_str(&format!("-D{id}"));
    }
    for suffix in ["", "_1", "_2", "_3"] {
        let proposed = format!("{base}{suffix}");
        if exists(&proposed) {
            io.push_out(&format!(
                "Branch name {proposed} already exists; trying a new name.\n"
            ));
            continue;
        }
        return Ok(proposed);
    }
    Err(
        "Arc was unable to automagically make a name for this patch. Please clean up your \
         working copy and try again."
            .to_string(),
    )
}

/// Create and check out the git patch branch (`createBranch()`,
/// `:293-322`).
///
/// Checks out `-b NAME [BASE]` (base only when `use_base` and the bundle
/// has one), updates submodules, and prints
/// `Created and checked out branch %s.`.
pub fn git_create_branch(
    dir: &Path,
    bundle: &Bundle,
    use_base: bool,
    io: &mut Io,
) -> Result<String, String> {
    let name = git_branch_name(
        bundle,
        &|proposed| {
            git::output(dir, &["rev-parse", "--verify", proposed])
                .is_ok_and(|out| out.status.success())
        },
        io,
    )?;
    if use_base && let Some(base) = bundle.base_revision.as_deref() {
        let canonical = git::canonical_revision_name(dir, base)?;
        git::stdout_text(dir, &["checkout", "-b", &name, &canonical])?;
    } else {
        git::stdout_text(dir, &["checkout", "-b", &name])?;
    }
    git_submodule_update(dir, io)?;
    io.push_out(&format!("Created and checked out branch {name}.\n"));
    Ok(name)
}

/// Run `git submodule update --init --recursive`, echoing output
/// (`execPassthru('submodule update --init --recursive')`).
pub fn git_submodule_update(dir: &Path, io: &mut Io) -> Result<(), String> {
    let output = git::output(dir, &["submodule", "update", "--init", "--recursive"])?;
    if !output.status.success() {
        return Err(format!(
            "git submodule update failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    io.push_out(&String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// Apply the bundle with `git apply --whitespace nowarn --index --reject`
/// (`:703-723`).
///
/// Failures print the verbatim `\n** Patch Failed! **\n` banner and fail
/// with `Unable to apply patch!`.
pub fn git_apply_patch(dir: &Path, bundle: &Bundle, io: &mut Io) -> Result<(), String> {
    git_apply_patch_ansi(dir, bundle, io, terminal::ansi_enabled())
}

/// [`git_apply_patch`] with an explicit ANSI setting for the failure
/// banner.
pub fn git_apply_patch_ansi(
    dir: &Path,
    bundle: &Bundle,
    io: &mut Io,
    ansi: bool,
) -> Result<(), String> {
    let patchfile = TempPatchFile::write(&bundle.text)?;
    let path = patchfile.path.to_string_lossy().to_string();
    let output = git::output(
        dir,
        &[
            "apply",
            "--whitespace",
            "nowarn",
            "--index",
            "--reject",
            "--",
            &path,
        ],
    )?;
    if !output.status.success() {
        io.push_out(&String::from_utf8_lossy(&output.stdout));
        io.push_out(&patch_failed_banner_ansi(ansi));
        return Err("Unable to apply patch!".to_string());
    }
    io.push_out(&String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// Commit the applied patch: `commit -a [--author] -F - --no-verify`
/// with the message on stdin (`:743-755`).
pub fn git_commit_patch(dir: &Path, bundle: &Bundle, message: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = std::process::Command::new("git");
    command.current_dir(dir).arg("commit").arg("-a");
    if let Some(author) = bundle.full_author() {
        command.arg(format!("--author={author}"));
    }
    command
        .args(["-F", "-", "--no-verify"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "failed to write commit message".to_string())?
        .write_all(message.as_bytes())
        .map_err(|e| format!("failed to write commit message: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Format one `** TITLE ** message` status line with a background.
///
/// PHP renders `"<bg:color>** %s **</bg> %s\n"`
/// (`ArcanistWorkflow.php:1571-1593`, `ArcanistPatchWorkflow.php:436-440,
/// 677-681, 865-868`): TTYs gain the background plus the bold label
/// (the `**` markers become codes), while `ansi=false` keeps the
/// legacy `**`-marked plain bytes.
fn status_line_ansi(color: &str, title: &str, message: &str, ansi: bool) -> String {
    if !ansi {
        return format!("** {title} ** {message}\n");
    }
    terminal::format_status_message(color, title, message, true)
}

/// Print a git success line to stderr (`writeOkay()`,
/// `ArcanistWorkflow.php:1587-1593`).
fn git_write_okay_ansi(io: &mut Io, title: &str, message: &str, ansi: bool) {
    io.push_err(&status_line_ansi("green", title, message, ansi));
}

/// Cherry-pick failure banner (`"\n<bg:red>** %s**</bg>\n"`, no space
/// before the closers, as in PHP `:787-789`).
fn cherry_pick_failed_banner_ansi(ansi: bool) -> String {
    if !ansi {
        return "\n** Cherry Pick Failed!**\n".to_string();
    }
    terminal::console_format("\n<bg:red>** %s**</bg>\n", &["Cherry Pick Failed!"], true)
}

/// Cherry-pick the temp branch onto the original branch, then delete it
/// (`:766-792`).
///
/// The temp branch is always deleted; cherry-pick failures print the
/// verbatim `\n** Cherry Pick Failed!**\n` banner (no space before the
/// closers, as in PHP) and propagate the cherry-pick error.
pub fn git_cherry_pick_temp(
    dir: &Path,
    io: &mut Io,
    original_branch: &str,
    new_branch: &str,
) -> Result<(), String> {
    git_cherry_pick_temp_ansi(
        dir,
        io,
        original_branch,
        new_branch,
        terminal::ansi_enabled(),
    )
}

/// [`git_cherry_pick_temp`] with an explicit ANSI setting.
pub fn git_cherry_pick_temp_ansi(
    dir: &Path,
    io: &mut Io,
    original_branch: &str,
    new_branch: &str,
    ansi: bool,
) -> Result<(), String> {
    git::stdout_text(dir, &["checkout", original_branch, "--"])?;
    git_submodule_update(dir, io)?;
    let mut failure: Option<String> = None;
    match git::output(dir, &["cherry-pick", "--", new_branch]) {
        Ok(output) if output.status.success() => {
            if let Err(error) = git_submodule_update(dir, io) {
                failure = Some(error);
            }
        }
        Ok(output) => {
            failure = Some(format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Err(error) => {
            failure = Some(error);
        }
    }
    git::output(dir, &["branch", "-D", "--", new_branch]).ok();
    if let Some(detail) = failure {
        io.push_out(&cherry_pick_failed_banner_ansi(ansi));
        return Err(detail.trim().to_string());
    }
    Ok(())
}

/// Resolve the commit message: Differential messages for bundles with a
/// revision ID, else the port guidance error (no interactive editor; see
/// the module docs).
pub fn resolve_commit_message(
    bundle: &Bundle,
    call: Option<&diff_submit::ConduitCall<'_>>,
) -> Result<String, String> {
    if let (Some(id), Some(call)) = (bundle.revision_id, call) {
        return fetch_patch_commit_message(call, id);
    }
    Err(
        "Unable to determine a commit message for this patch. Re-run with --nocommit to apply \
         without committing."
            .to_string(),
    )
}

/// Apply one bundle under git: apply, submodule update, optional commit
/// with the `COMMITTED`/`APPLIED` line, and the nobranch cherry-pick.
#[allow(clippy::too_many_arguments)]
pub fn apply_git(
    dir: &Path,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: Option<&diff_submit::ConduitCall<'_>>,
    has_base_local: bool,
    original_branch: Option<&str>,
    new_branch: Option<&str>,
) -> Result<(), String> {
    apply_git_ansi(
        dir,
        bundle,
        options,
        io,
        call,
        has_base_local,
        original_branch,
        new_branch,
        terminal::ansi_enabled(),
    )
}

/// [`apply_git`] with an explicit ANSI setting.
#[allow(clippy::too_many_arguments)]
pub fn apply_git_ansi(
    dir: &Path,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: Option<&diff_submit::ConduitCall<'_>>,
    has_base_local: bool,
    original_branch: Option<&str>,
    new_branch: Option<&str>,
    ansi: bool,
) -> Result<(), String> {
    git_apply_patch_ansi(dir, bundle, io, ansi)?;
    git_submodule_update(dir, io)?;
    if should_commit(options) {
        let message = resolve_commit_message(bundle, call)?;
        git_commit_patch(dir, bundle, &message)?;
        git_write_okay_ansi(io, "COMMITTED", "Successfully committed patch.", ansi);
    } else {
        git_write_okay_ansi(io, "APPLIED", "Successfully applied patch.", ansi);
    }
    if !should_branch(options)
        && should_commit(options)
        && has_base_local
        && let (Some(original), Some(new)) = (original_branch, new_branch)
    {
        git_cherry_pick_temp_ansi(dir, io, original, new, ansi)?;
    }
    Ok(())
}

/// Pick a Mercurial bookmark name: `arcpatch`, plus `-D{id}` when the
/// bundle has a revision ID, plus the first free suffix
/// (`getBookmarkName()`, `:252-291`).
///
/// `exists` reports whether a name is taken (`hg log -r NAME`).
pub fn hg_bookmark_name(
    bundle: &Bundle,
    exists: &dyn Fn(&str) -> bool,
    io: &mut Io,
) -> Result<String, String> {
    let mut base = "arcpatch".to_string();
    if let Some(id) = bundle.revision_id {
        base.push_str(&format!("-D{id}"));
    }
    for suffix in ["", "-1", "-2", "-3"] {
        let proposed = format!("{base}{suffix}");
        if exists(&proposed) {
            io.push_out(&format!(
                "Bookmark name {proposed} already exists; trying a new name.\n"
            ));
            continue;
        }
        return Ok(proposed);
    }
    Err(
        "Arc was unable to automagically make a name for this patch. Please clean up your \
         working copy and try again."
            .to_string(),
    )
}

/// Resolve one Mercurial revision to its node via `backends`.
fn hg_canonical(dir: &Path, hg: &hg::HgCommand, revision: &str) -> Result<String, String> {
    hg::stdout_text_with(
        dir,
        hg,
        &[
            "log",
            "-l",
            "1",
            "--template",
            "{node}",
            "-r",
            revision,
            "--",
        ],
    )
}

/// Create the Mercurial patch bookmark (`createBranch()`, `:323-342`).
///
/// Updates to the base (printing `Updating to the revision's base
/// commit`) when `use_base` and the bundle has one, creates the
/// bookmark, and prints `Created and checked out bookmark %s.`.
pub fn hg_create_bookmark(
    dir: &Path,
    hg: &hg::HgCommand,
    bundle: &Bundle,
    use_base: bool,
    io: &mut Io,
) -> Result<String, String> {
    let name = hg_bookmark_name(
        bundle,
        &|proposed| hg::ok_with(dir, hg, &["log", "-r", proposed]),
        io,
    )?;
    if use_base && let Some(base) = bundle.base_revision.as_deref() {
        let canonical = hg_canonical(dir, hg, base)?;
        io.push_out("Updating to the revision's base commit\n");
        let output = hg::output_with(dir, hg, &["update", &canonical])?;
        if !output.status.success() {
            return Err(format!(
                "hg update failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        io.push_out(&String::from_utf8_lossy(&output.stdout));
    }
    hg::stdout_text_with(dir, hg, &["bookmark", &name])?;
    io.push_out(&format!("Created and checked out bookmark {name}.\n"));
    Ok(name)
}

/// Patch-failure banner (`"\n<bg:red>** %s **</bg>\n"`, `:713-715` for
/// git, `:801-803` for hg).
fn patch_failed_banner_ansi(ansi: bool) -> String {
    if !ansi {
        return "\n** Patch Failed! **\n".to_string();
    }
    terminal::console_format("\n<bg:red>** %s **</bg>\n", &["Patch Failed!"], true)
}

/// Rebase-failure banner (`"\n<bg:red>** %s**</bg>\n"`, no space before
/// the closers, as in PHP `:854-857`).
fn rebase_failed_banner_ansi(original_branch: &str, ansi: bool) -> String {
    if !ansi {
        return format!("\n** Rebase onto {original_branch} failed!**\n");
    }
    let message = format!("Rebase onto {original_branch} failed!");
    terminal::console_format("\n<bg:red>** %s**</bg>\n", &[&message], true)
}

/// Case-folding warning (plain shape, verbatim from PHP).
const HG_CASE_WARNING_PLAIN: &str = "\n** WARNING ** This patch may have failed because it attempts to change the case \
     of a filename (for instance, from 'example.c' to 'Example.c'). Mercurial cannot \
     apply patches like this on case-insensitive filesystems. You must apply this patch \
     manually.\n";

/// Case-folding warning with an explicit ANSI setting.
///
/// PHP renders `"\n<bg:yellow>** %s **</bg> %s\n"` (`:807-815`); the
/// `**` markers stay in the pattern so TTYs gain the bold label,
/// while `ansi=false` keeps the legacy `**`-marked plain bytes.
fn hg_case_warning_ansi(ansi: bool) -> String {
    if !ansi {
        return HG_CASE_WARNING_PLAIN.to_string();
    }
    let rest = HG_CASE_WARNING_PLAIN
        .strip_prefix("\n** WARNING ** ")
        .unwrap_or(HG_CASE_WARNING_PLAIN);
    terminal::console_format("\n<bg:yellow>** %s **</bg> %s", &["WARNING", rest], true)
}

/// Import the bundle with `hg import --no-commit -` (`:794-819`).
///
/// Failures print the verbatim `\n** Patch Failed! **\n` banner, plus
/// the case-folding `WARNING` block when stderr mentions a
/// `case-folding collision`, then propagate the hg error.
pub fn hg_import_patch(
    dir: &Path,
    hg: &hg::HgCommand,
    bundle: &Bundle,
    io: &mut Io,
) -> Result<(), String> {
    hg_import_patch_ansi(dir, hg, bundle, io, terminal::ansi_enabled())
}

/// [`hg_import_patch`] with an explicit ANSI setting.
pub fn hg_import_patch_ansi(
    dir: &Path,
    hg: &hg::HgCommand,
    bundle: &Bundle,
    io: &mut Io,
    ansi: bool,
) -> Result<(), String> {
    // The patch travels via temp file instead of `-` stdin so custom
    // backends work; `hg import` accepts a file the same way.
    let patchfile = TempPatchFile::write(&bundle.text)?;
    let path = patchfile.path.to_string_lossy().to_string();
    let output = hg::output_with(dir, hg, &["import", "--no-commit", &path])?;
    if output.status.success() {
        io.push_out(&String::from_utf8_lossy(&output.stdout));
        return Ok(());
    }
    io.push_out(&patch_failed_banner_ansi(ansi));
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if stderr.contains("case-folding collision") {
        io.push_out(&hg_case_warning_ansi(ansi));
    }
    Err(
        format!("{}{}", String::from_utf8_lossy(&output.stdout), stderr)
            .trim()
            .to_string(),
    )
}

/// Commit the imported patch: `commit [-u AUTHOR] -l -` with the message
/// on stdin (`:821-834`).
pub fn hg_commit_patch(
    dir: &Path,
    hg: &hg::HgCommand,
    bundle: &Bundle,
    message: &str,
) -> Result<(), String> {
    let author = bundle.full_author().or_else(|| bundle.author_name.clone());
    // The message travels via temp file instead of `-` stdin so custom
    // backends work; `hg commit -l` accepts a file the same way.
    let logfile = TempPatchFile::write(message)?;
    let path = logfile.path.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["commit"];
    if let Some(author) = author.as_deref() {
        args.push("-u");
        args.push(author);
    }
    args.push("-l");
    args.push(&path);
    let output = hg::output_with(dir, hg, &args)?;
    if !output.status.success() {
        return Err(format!(
            "hg commit failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Rebase the temp bookmark onto the original bookmark and delete it
/// (`:836-858`).
///
/// Skips the rebase when the original revision already parents the new
/// commit; rebase failures abort and fail with the verbatim banner
/// message `\n** Rebase onto %s failed!**\n` (no space, as in PHP).
pub fn hg_rebase_temp(
    dir: &Path,
    hg: &hg::HgCommand,
    original_branch: &str,
    new_branch: &str,
) -> Result<(), String> {
    hg_rebase_temp_ansi(
        dir,
        hg,
        original_branch,
        new_branch,
        terminal::ansi_enabled(),
    )
}

/// [`hg_rebase_temp`] with an explicit ANSI setting for the failure
/// banner (PHP formats the banner into the thrown message, `:854-857`).
pub fn hg_rebase_temp_ansi(
    dir: &Path,
    hg: &hg::HgCommand,
    original_branch: &str,
    new_branch: &str,
    ansi: bool,
) -> Result<(), String> {
    let original_rev = hg_canonical(dir, hg, original_branch)?;
    let current_parent = hg_canonical(dir, hg, &format!("{new_branch}^"))?;
    let mut failed = false;
    if original_rev != current_parent {
        failed = !hg::ok_with(
            dir,
            hg,
            &["rebase", "--dest", original_branch, "--rev", new_branch],
        );
    }
    hg::stdout_text_with(dir, hg, &["bookmark", "--delete", new_branch])?;
    if failed {
        let _ = hg::ok_with(dir, hg, &["rebase", "--abort"]);
        return Err(rebase_failed_banner_ansi(original_branch, ansi));
    }
    Ok(())
}

/// Apply one bundle under Mercurial: import, optional commit, the
/// nobranch rebase, and the `OKAY` line (to stderr, like `writeOkay()`).
#[allow(clippy::too_many_arguments)]
pub fn apply_hg(
    dir: &Path,
    hg: &hg::HgCommand,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: Option<&diff_submit::ConduitCall<'_>>,
    has_base_local: bool,
    original_branch: Option<&str>,
    new_branch: Option<&str>,
) -> Result<(), String> {
    apply_hg_ansi(
        dir,
        hg,
        bundle,
        options,
        io,
        call,
        has_base_local,
        original_branch,
        new_branch,
        terminal::ansi_enabled(),
    )
}

/// [`apply_hg`] with an explicit ANSI setting.
#[allow(clippy::too_many_arguments)]
pub fn apply_hg_ansi(
    dir: &Path,
    hg: &hg::HgCommand,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: Option<&diff_submit::ConduitCall<'_>>,
    has_base_local: bool,
    original_branch: Option<&str>,
    new_branch: Option<&str>,
    ansi: bool,
) -> Result<(), String> {
    hg_import_patch_ansi(dir, hg, bundle, io, ansi)?;
    if should_commit(options) {
        let message = resolve_commit_message(bundle, call)?;
        hg_commit_patch(dir, hg, bundle, &message)?;
        if !should_branch(options)
            && has_base_local
            && let (Some(original), Some(new)) = (original_branch, new_branch)
        {
            hg_rebase_temp_ansi(dir, hg, original, new, ansi)?;
        }
        io.push_err(&status_line_ansi(
            "green",
            "OKAY",
            "Successfully committed patch.",
            ansi,
        ));
    } else {
        io.push_err(&status_line_ansi(
            "green",
            "OKAY",
            "Successfully applied patch.",
            ansi,
        ));
    }
    Ok(())
}

/// Symlink file type (`ArcanistDiffChangeType::FILE_SYMLINK`, `:23`).
const FILE_SYMLINK: u32 = 5;

/// One change for the svn flow, from an arcbundle dictionary or parsed
/// git-patch text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SvnChange {
    /// Change type (`TYPE_*`).
    pub change_type: u32,
    /// File type (`FILE_*`).
    pub file_type: u32,
    /// Old path (`None` for additions).
    pub old_path: Option<String>,
    /// Current path (`None` for deletions).
    pub current_path: Option<String>,
    /// Hunk body lines (from `@@` on), without file headers.
    pub hunks: String,
    /// Old properties.
    pub old_props: std::collections::HashMap<String, String>,
    /// New properties.
    pub new_props: std::collections::HashMap<String, String>,
    /// Symlink target for added/changed symlinks.
    pub symlink_target: Option<String>,
}

/// Build svn changes from arcbundle change dictionaries (with decoded
/// hunk corpora).
pub fn svn_changes_from_dicts(
    dicts: &[serde_json::Value],
    corpora: &[Vec<String>],
) -> Vec<SvnChange> {
    dicts
        .iter()
        .zip(corpora.iter())
        .map(|(dict, hunks)| {
            let props = |key: &str| {
                dict.get(key)
                    .and_then(|v| v.as_object())
                    .map(|map| {
                        map.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let file_type = dict
                .get("fileType")
                .and_then(|v| v.as_u64())
                .unwrap_or(u64::from(FILE_TEXT)) as u32;
            SvnChange {
                change_type: dict
                    .get("type")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(u64::from(TYPE_CHANGE)) as u32,
                file_type,
                old_path: dict
                    .get("oldPath")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                current_path: dict
                    .get("currentPath")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                hunks: hunks.concat(),
                old_props: props("oldProperties"),
                new_props: props("newProperties"),
                symlink_target: symlink_target_from_hunks(file_type, &hunks.concat()),
            }
        })
        .collect()
}

/// Symlink target from hunk bodies: the single added line, when the
/// change is a symlink.
fn symlink_target_from_hunks(file_type: u32, body: &str) -> Option<String> {
    if file_type != FILE_SYMLINK {
        return None;
    }
    body.split('\n').find_map(|line| {
        line.strip_prefix('+')
            .filter(|target| !target.starts_with('+'))
            .map(str::to_string)
    })
}

/// Unquote a git C-quoted path (`"a\"b"` -> `a"b`); plain paths pass
/// through.
pub fn unquote_git_path(path: &str) -> String {
    if !(path.starts_with('"') && path.ends_with('"') && path.len() >= 2) {
        return path.to_string();
    }
    let inner = &path[1..path.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => {
                if other.is_ascii_digit() {
                    let mut octal = String::from(other);
                    octal.push(chars.next().unwrap_or('0'));
                    octal.push(chars.next().unwrap_or('0'));
                    if let Ok(byte) = u8::from_str_radix(&octal, 8) {
                        out.push(byte as char);
                    }
                } else {
                    out.push(other);
                }
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Strip one `a/` or `b/` prefix from a `---`/`+++` path.
fn strip_git_prefix(path: &str) -> String {
    let unquoted = unquote_git_path(path);
    unquoted
        .strip_prefix("a/")
        .or_else(|| unquoted.strip_prefix("b/"))
        .unwrap_or(&unquoted)
        .to_string()
}

/// Parse git-patch text into svn changes.
///
/// Splits on `diff --git` boundaries; headers yield the type, paths,
/// modes, and renames, and everything from the first `@@` on is the hunk
/// body. `GIT binary` / `Binary files` sections carry no hunks (PHP
/// skips hunkless changes the same way). Renames emit a `MOVE_AWAY`
/// (deleted old path) plus a `MOVE_HERE`, like the diff parser.
pub fn parse_git_patch_for_svn(text: &str) -> Vec<SvnChange> {
    let lines: Vec<&str> = text.split('\n').collect();
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("diff --git ").then_some(index))
        .collect();
    let mut changes = Vec::new();
    for (position, start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(lines.len());
        changes.extend(parse_svn_section(&lines[*start..end].join("\n")));
    }
    changes
}

/// Split a `diff --git OLD NEW` header into its two paths, honoring
/// C-quoting.
fn split_git_header(header: &str) -> Option<(String, String)> {
    let rest = header.strip_prefix("diff --git ")?;
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            current.push(ch);
            for quoted in chars.by_ref() {
                current.push(quoted);
                if quoted == '"' {
                    break;
                }
            }
        } else if ch == ' ' {
            if !current.is_empty() {
                paths.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        paths.push(current);
    }
    if paths.len() == 2 {
        Some((paths.remove(0), paths.remove(0)))
    } else {
        None
    }
}

/// Parse one `diff --git` section into svn changes.
fn parse_svn_section(section: &str) -> Vec<SvnChange> {
    let lines: Vec<&str> = section.split('\n').collect();
    let mut change = SvnChange {
        change_type: TYPE_CHANGE,
        file_type: FILE_TEXT,
        ..SvnChange::default()
    };
    let (headers, body_start) = parse_svn_section_headers(&lines, &mut change);
    svn_section_fallback_paths(&lines, &mut change);
    change.hunks = svn_section_body(&lines, body_start, headers.is_binary);
    svn_section_modes(&mut change, &headers);
    svn_section_symlink(&mut change);
    svn_section_classify(change, &headers)
}

/// Parsed `diff --git` section headers (before the first `@@`).
#[derive(Debug, Default)]
struct SvnSectionHeaders {
    old_mode: Option<String>,
    new_mode: Option<String>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    copy_from: Option<String>,
    copy_to: Option<String>,
    is_new: bool,
    is_deleted: bool,
    is_binary: bool,
}

/// Parse the section headers, filling `---`/`+++` paths into `change`.
/// Returns the headers plus the first `@@` line index, if any.
fn parse_svn_section_headers(
    lines: &[&str],
    change: &mut SvnChange,
) -> (SvnSectionHeaders, Option<usize>) {
    let mut headers = SvnSectionHeaders::default();
    let mut body_start = None;
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with("@@") {
            body_start = Some(index);
            break;
        }
        if apply_svn_mode_line(&mut headers, line) {
            continue;
        }
        if apply_svn_copy_line(&mut headers, line) {
            continue;
        }
        if apply_svn_path_line(change, line) {
            continue;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            headers.is_binary = true;
        }
    }
    (headers, body_start)
}

/// Apply one mode header line; true when the line was a mode header.
fn apply_svn_mode_line(headers: &mut SvnSectionHeaders, line: &str) -> bool {
    if let Some(rest) = line.strip_prefix("new file mode ") {
        headers.is_new = true;
        headers.new_mode = Some(rest.to_string());
    } else if let Some(rest) = line.strip_prefix("deleted file mode ") {
        headers.is_deleted = true;
        headers.old_mode = Some(rest.to_string());
    } else if let Some(rest) = line.strip_prefix("old mode ") {
        headers.old_mode = Some(rest.to_string());
    } else if let Some(rest) = line.strip_prefix("new mode ") {
        headers.new_mode = Some(rest.to_string());
    } else {
        return false;
    }
    true
}

/// Apply one rename/copy header line; true when the line matched.
fn apply_svn_copy_line(headers: &mut SvnSectionHeaders, line: &str) -> bool {
    if let Some(rest) = line.strip_prefix("rename from ") {
        headers.rename_from = Some(unquote_git_path(rest));
    } else if let Some(rest) = line.strip_prefix("rename to ") {
        headers.rename_to = Some(unquote_git_path(rest));
    } else if let Some(rest) = line.strip_prefix("copy from ") {
        headers.copy_from = Some(unquote_git_path(rest));
    } else if let Some(rest) = line.strip_prefix("copy to ") {
        headers.copy_to = Some(unquote_git_path(rest));
    } else {
        return false;
    }
    true
}

/// Apply one `---`/`+++` path line into `change`; true when matched.
fn apply_svn_path_line(change: &mut SvnChange, line: &str) -> bool {
    if let Some(rest) = line.strip_prefix("--- ") {
        let path = rest.split('\t').next().unwrap_or("");
        if path != "/dev/null" {
            change.old_path = Some(strip_git_prefix(path));
        }
    } else if let Some(rest) = line.strip_prefix("+++ ") {
        let path = rest.split('\t').next().unwrap_or("");
        if path != "/dev/null" {
            change.current_path = Some(strip_git_prefix(path));
        }
    } else {
        return false;
    }
    true
}

/// Fall back to the `diff --git` line when `---`/`+++` are missing
/// (mode-only changes).
fn svn_section_fallback_paths(lines: &[&str], change: &mut SvnChange) {
    if change.old_path.is_none()
        && change.current_path.is_none()
        && let Some(header) = lines.first()
        && let Some((old, new)) = split_git_header(header)
    {
        change.old_path = Some(strip_git_prefix(&old));
        change.current_path = Some(strip_git_prefix(&new));
    }
}

/// Collect the hunk body (everything from the first `@@` on), dropping it
/// for binary sections.
fn svn_section_body(lines: &[&str], body_start: Option<usize>, is_binary: bool) -> String {
    let mut body = match body_start {
        Some(start) => lines[start..].join("\n"),
        None => String::new(),
    };
    if is_binary {
        body.clear();
    } else if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

/// Record parsed modes as `unix:filemode` properties.
fn svn_section_modes(change: &mut SvnChange, headers: &SvnSectionHeaders) {
    if let Some(mode) = headers.old_mode.clone() {
        change.old_props.insert("unix:filemode".to_string(), mode);
    }
    if let Some(mode) = headers.new_mode.clone() {
        change.new_props.insert("unix:filemode".to_string(), mode);
    }
}

/// Mark `120000`-mode changes as symlinks, resolving the target.
fn svn_section_symlink(change: &mut SvnChange) {
    if change
        .new_props
        .get("unix:filemode")
        .is_some_and(|mode| mode == "120000")
        || change
            .old_props
            .get("unix:filemode")
            .is_some_and(|mode| mode == "120000")
    {
        change.file_type = FILE_SYMLINK;
        change.symlink_target = symlink_target_from_hunks(FILE_SYMLINK, &change.hunks);
    }
}

/// Classify the change type from the headers, expanding renames into a
/// `MOVE_AWAY` plus a `MOVE_HERE`.
fn svn_section_classify(mut change: SvnChange, headers: &SvnSectionHeaders) -> Vec<SvnChange> {
    if headers.is_new {
        change.change_type = TYPE_ADD;
        change.old_path = None;
    } else if headers.is_deleted {
        change.change_type = TYPE_DELETE;
        change.current_path = None;
    } else if headers.rename_from.is_some() || headers.rename_to.is_some() {
        return svn_section_rename(change, headers);
    } else if headers.copy_from.is_some() || headers.copy_to.is_some() {
        change.change_type = TYPE_COPY_HERE;
        change.old_path = headers
            .copy_from
            .clone()
            .or_else(|| change.old_path.clone());
        change.current_path = headers
            .copy_to
            .clone()
            .or_else(|| change.current_path.clone());
    }
    vec![change]
}

/// Expand a rename into a `MOVE_AWAY` (deleted old path) plus a `MOVE_HERE`.
fn svn_section_rename(change: SvnChange, headers: &SvnSectionHeaders) -> Vec<SvnChange> {
    let mut change = change;
    let old = headers
        .rename_from
        .clone()
        .or_else(|| change.old_path.clone());
    let new = headers
        .rename_to
        .clone()
        .or_else(|| change.current_path.clone());
    let mut away = SvnChange {
        change_type: TYPE_MOVE_AWAY,
        file_type: change.file_type,
        current_path: old.clone(),
        ..SvnChange::default()
    };
    away.old_path = old.clone();
    change.change_type = TYPE_MOVE_HERE;
    change.old_path = old;
    change.current_path = new;
    vec![away, change]
}

/// Render one svn change as a `-p0` unified diff
/// (`ArcanistBundle::toUnifiedDiff()` for a single change,
/// `src/parser/ArcanistBundle.php:254-311`).
///
/// Emits the `Index:` header, `---`/`+++` lines (with the move
/// old-equals-current handling and `/dev/null` for added/deleted sides),
/// and the hunk body. Empty when the change has no hunks.
pub fn unified_for_svn(change: &SvnChange) -> String {
    if change.hunks.is_empty() {
        return String::new();
    }
    let index_path = change
        .current_path
        .clone()
        .or_else(|| change.old_path.clone())
        .unwrap_or_default();
    let mut old = change.old_path.clone().unwrap_or("/dev/null".to_string());
    let current = change
        .current_path
        .clone()
        .unwrap_or("/dev/null".to_string());
    if current != "/dev/null" && old != "/dev/null" {
        old = current.clone();
    }
    format!(
        "Index: {index_path}\n{}\n--- {old}\n+++ {current}\n{}",
        "=".repeat(67),
        change.hunks
    )
}

/// Escape `@` in svn paths (`ArcanistSubversionAPI::escapeFileNameForSVN()`,
/// `:637-647`): append `@` when the name contains one.
pub fn escape_svn_path(path: &str) -> String {
    if path.contains('@') {
        format!("{path}@")
    } else {
        path.to_string()
    }
}

/// Create parent directories one at a time, `svn add`ing each
/// (`createParentDirectoryOf()`, `:1064-1086`).
pub fn svn_create_parents(dir: &Path, svn: &svn::SvnCommand, path: &str) -> Result<(), String> {
    let mut current = std::path::PathBuf::new();
    let parent = std::path::Path::new(path).parent();
    let Some(parent) = parent else {
        return Ok(());
    };
    for component in parent.components() {
        current.push(component);
        let full = dir.join(&current);
        if !full.exists() {
            std::fs::create_dir(&full)
                .map_err(|e| format!("Failed to create directory '{}': {e}", full.display()))?;
            let name = current.to_string_lossy().to_string();
            svn::stdout_text_with(dir, svn, &["add", &escape_svn_path(&name)])?;
        }
    }
    Ok(())
}

/// Applies one per-file unified diff; returns `(patched, patch_stdout)`.
type PatchRunner<'a> = &'a mut dyn FnMut(&str, &str) -> Result<(bool, String), String>;

/// Outcome of the svn flow: applied, or hunks failed (`$patch_err`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvnOutcome {
    /// Everything applied.
    Applied,
    /// Some `patch -p0` runs failed (warning printed).
    HunksFailed,
}

/// Svn hunks-failed warning (plain shape, verbatim from PHP).
const SVN_HUNKS_WARNING_PLAIN: &str = "\n\n** WARNING ** Some hunks could not be applied cleanly by the unix 'patch' utility. \
     Your working copy may be different from the revision's base, or you may be in the wrong \
     subdirectory. You can export the raw patch file using 'arc export --unified', and then \
     try to apply it by fiddling with options to 'patch' (particularly, -p), or manually. \
     The output above, from 'patch', may be helpful in figuring out what went wrong.\n";

/// Svn hunks-failed warning with an explicit ANSI setting.
///
/// PHP renders `"\n\n<bg:yellow>** %s **</bg> %s\n"` (`:683-700`); the
/// `**` markers stay in the pattern so TTYs gain the bold label,
/// while `ansi=false` keeps the legacy `**`-marked plain bytes.
fn svn_hunks_warning_ansi(ansi: bool) -> String {
    if !ansi {
        return SVN_HUNKS_WARNING_PLAIN.to_string();
    }
    let rest = SVN_HUNKS_WARNING_PLAIN
        .strip_prefix("\n\n** WARNING ** ")
        .unwrap_or(SVN_HUNKS_WARNING_PLAIN);
    terminal::console_format("\n\n<bg:yellow>** %s **</bg> %s", &["WARNING", rest], true)
}

/// Apply bundle changes under svn (`:476-700`).
///
/// Collects copies/deletes/patches/propsets/adds/symlinks (prompting
/// before skipping missing delete/copy sources), creates parents, runs
/// `svn cp`/`svn rm`, links symlinks, applies each per-file unified diff
/// with `patch -p0` (or `touch` when empty), runs `svn add`, applies
/// property changes (`unix:filemode` becomes `svn:executable`), and
/// prints the `OKAY` line or the `WARNING` block.
#[allow(clippy::too_many_arguments)]
pub fn apply_svn(
    dir: &Path,
    svn: &svn::SvnCommand,
    changes: &[SvnChange],
    io: &mut Io,
    run_patch: PatchRunner<'_>,
    run_touch: &mut dyn FnMut(&str) -> Result<(), String>,
    run_link: &mut dyn FnMut(&str, &str) -> Result<(), String>,
) -> Result<SvnOutcome, String> {
    apply_svn_ansi(
        dir,
        svn,
        changes,
        io,
        run_patch,
        run_touch,
        run_link,
        terminal::ansi_enabled(),
    )
}

/// [`apply_svn`] with an explicit ANSI setting.
#[allow(clippy::too_many_arguments)]
pub fn apply_svn_ansi(
    dir: &Path,
    svn: &svn::SvnCommand,
    changes: &[SvnChange],
    io: &mut Io,
    run_patch: PatchRunner<'_>,
    run_touch: &mut dyn FnMut(&str) -> Result<(), String>,
    run_link: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    ansi: bool,
) -> Result<SvnOutcome, String> {
    let plan = plan_svn_changes(dir, changes, io)?;
    create_svn_parents(dir, svn, &plan)?;
    run_svn_copies_deletes(dir, svn, &plan)?;
    run_svn_symlinks(&plan.symlinks, run_link)?;
    let patch_err = run_svn_patches(&plan, io, run_patch, run_touch)?;
    run_svn_adds_props(dir, svn, &plan)?;
    if !patch_err {
        io.push_out(&status_line_ansi(
            "green",
            "OKAY",
            "Successfully applied patch to the working copy.",
            ansi,
        ));
        return Ok(SvnOutcome::Applied);
    }
    io.push_out(&svn_hunks_warning_ansi(ansi));
    Ok(SvnOutcome::HunksFailed)
}

/// Collected svn apply operations.
#[derive(Debug, Default)]
struct SvnPlan {
    copies: Vec<(String, String)>,
    deletes: Vec<String>,
    patches: Vec<(String, String)>,
    propset: Vec<(String, String, Option<String>)>,
    adds: Vec<String>,
    symlinks: Vec<SvnChange>,
}

/// Collect copies/deletes/patches/propsets/adds/symlinks for the svn flow,
/// prompting before skipping missing delete/copy sources.
fn plan_svn_changes(dir: &Path, changes: &[SvnChange], io: &mut Io) -> Result<SvnPlan, String> {
    let mut plan = SvnPlan::default();
    for change in changes {
        plan_one_svn_change(dir, change, io, &mut plan)?;
    }
    Ok(plan)
}

/// Collect the operations for one change.
fn plan_one_svn_change(
    dir: &Path,
    change: &SvnChange,
    io: &mut Io,
    plan: &mut SvnPlan,
) -> Result<(), String> {
    let mut should_patch = true;
    if change.file_type == FILE_SYMLINK {
        should_patch = false;
        plan.symlinks.push(change.clone());
    }
    match change.change_type {
        x if x == TYPE_MOVE_AWAY || x == TYPE_MULTICOPY || x == TYPE_DELETE => {
            plan_svn_delete(dir, change, io, plan)?;
            should_patch = false;
        }
        x if x == TYPE_COPY_HERE || x == TYPE_MOVE_HERE => {
            plan_svn_copy(dir, change, io, plan)?;
        }
        x if x == TYPE_ADD => {
            plan.adds
                .push(change.current_path.clone().unwrap_or_default());
        }
        _ => {}
    }
    if should_patch {
        plan_svn_patch(change, plan);
    }
    Ok(())
}

/// Plan a delete, prompting when the file is already missing.
fn plan_svn_delete(
    dir: &Path,
    change: &SvnChange,
    io: &mut Io,
    plan: &mut SvnPlan,
) -> Result<(), String> {
    let path = change.current_path.clone().unwrap_or_default();
    if !dir.join(&path).exists() {
        let query = format!(
            "Patch deletes file '{path}', but the file does not exist in the working \
             copy. Continue anyway?"
        );
        if !land::console_confirm(io, &query, true)? {
            return Err(land::ABORT.to_string());
        }
    } else {
        plan.deletes.push(path);
    }
    Ok(())
}

/// Plan a copy/move, prompting when the source path is missing.
fn plan_svn_copy(
    dir: &Path,
    change: &SvnChange,
    io: &mut Io,
    plan: &mut SvnPlan,
) -> Result<(), String> {
    let path = change.old_path.clone().unwrap_or_default();
    if !dir.join(&path).exists() {
        let current = change.current_path.clone().unwrap_or_default();
        let verbs = if change.change_type == TYPE_COPY_HERE {
            "copies"
        } else {
            "moves"
        };
        let query = format!(
            "Patch {verbs} '{path}' to '{current}', but source path does not exist \
             in the working copy. Continue anyway?"
        );
        if !land::console_confirm(io, &query, true)? {
            return Err(land::ABORT.to_string());
        }
    } else {
        plan.copies
            .push((path, change.current_path.clone().unwrap_or_default()));
    }
    Ok(())
}

/// Plan the per-file unified diff and property changes for one change.
fn plan_svn_patch(change: &SvnChange, plan: &mut SvnPlan) {
    let path = change.current_path.clone().unwrap_or_default();
    plan.patches.push((path.clone(), unified_for_svn(change)));
    let mut keys: Vec<String> = change.old_props.keys().cloned().collect();
    for key in change.new_props.keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    for key in keys {
        if change.old_props.get(&key) != change.new_props.get(&key) {
            plan.propset.push((
                path.clone(),
                key.clone(),
                change.new_props.get(&key).cloned(),
            ));
        }
    }
}

/// Create parent directories for every copy destination, patch path, and
/// added path.
fn create_svn_parents(dir: &Path, svn: &svn::SvnCommand, plan: &SvnPlan) -> Result<(), String> {
    for (_, dst) in &plan.copies {
        svn_create_parents(dir, svn, dst)?;
    }
    for (path, _) in &plan.patches {
        svn_create_parents(dir, svn, path)?;
    }
    for add in &plan.adds {
        svn_create_parents(dir, svn, add)?;
    }
    Ok(())
}

/// Run the `svn cp` and `svn rm` operations.
fn run_svn_copies_deletes(dir: &Path, svn: &svn::SvnCommand, plan: &SvnPlan) -> Result<(), String> {
    for (src, dst) in &plan.copies {
        svn::stdout_text_with(
            dir,
            svn,
            &["cp", &escape_svn_path(src), &escape_svn_path(dst)],
        )?;
    }
    for delete in &plan.deletes {
        svn::stdout_text_with(dir, svn, &["rm", &escape_svn_path(delete)])?;
    }
    Ok(())
}

/// Link added/changed symlinks.
fn run_svn_symlinks(
    symlinks: &[SvnChange],
    run_link: &mut dyn FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    for symlink in symlinks {
        match symlink.change_type {
            x if x == TYPE_ADD
                || x == TYPE_CHANGE
                || x == TYPE_MOVE_HERE
                || x == TYPE_COPY_HERE =>
            {
                if let Some(target) = symlink.symlink_target.as_deref() {
                    run_link(target, &symlink.current_path.clone().unwrap_or_default())?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Apply each per-file unified diff (`touch` when empty); returns whether
/// any hunks failed.
fn run_svn_patches(
    plan: &SvnPlan,
    io: &mut Io,
    run_patch: PatchRunner<'_>,
    run_touch: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<bool, String> {
    let mut patch_err = false;
    for (path, patch) in &plan.patches {
        if patch.is_empty() {
            if run_touch(path).is_err() {
                patch_err = true;
            }
        } else {
            match run_patch(path, patch) {
                Ok((ok, out)) => {
                    io.push_out(&out);
                    if !ok {
                        patch_err = true;
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(patch_err)
}

/// Run the `svn add` and property operations (`unix:filemode` becomes
/// `svn:executable`).
fn run_svn_adds_props(dir: &Path, svn: &svn::SvnCommand, plan: &SvnPlan) -> Result<(), String> {
    for add in &plan.adds {
        svn::stdout_text_with(dir, svn, &["add", &escape_svn_path(add)])?;
    }
    for (path, prop, value) in &plan.propset {
        let (prop, value) = svn_prop_mapping(prop, value.clone());
        match value {
            None => {
                svn::stdout_text_with(dir, svn, &["propdel", &prop, &escape_svn_path(path)])?;
            }
            Some(value) => {
                svn::stdout_text_with(
                    dir,
                    svn,
                    &["propset", &prop, &value, &escape_svn_path(path)],
                )?;
            }
        }
    }
    Ok(())
}

/// Map an svn property change: `unix:filemode` becomes `svn:executable`
/// (`on` when any execute bit survives, else deleted); other properties
/// pass through (`:650-674`).
pub fn svn_prop_mapping(prop: &str, value: Option<String>) -> (String, Option<String>) {
    if prop != "unix:filemode" {
        return (prop.to_string(), value);
    }
    let executable = value.as_deref().is_some_and(|mode| {
        u32::from_str_radix(mode.trim_start_matches('0'), 8).is_ok_and(|bits| bits & 0o111 != 0)
    });
    (
        "svn:executable".to_string(),
        executable.then(|| "on".to_string()),
    )
}

/// Apply one per-file unified diff with `patch -p0 -i FILE`
/// (`(cd %s; patch -p0 < %s)`; `-i` is equivalent and needs no shell).
///
/// Streams `patch` output to stdout (like `passthru`) and returns
/// whether it applied.
pub fn run_patch_file(dir: &Path, patch_text: &str) -> Result<(bool, String), String> {
    run_patch_file_with(dir, patch_text, "patch")
}

/// [`run_patch_file`] with an explicit `patch` binary (test seam: whether
/// the system binary exists varies by machine).
pub fn run_patch_file_with(
    dir: &Path,
    patch_text: &str,
    patch_bin: &str,
) -> Result<(bool, String), String> {
    let patchfile = TempPatchFile::write(patch_text)?;
    let file = patchfile.path.to_string_lossy().to_string();
    let output = std::process::Command::new(patch_bin)
        .current_dir(dir)
        .args(["-p0", "-i", &file])
        .output()
        .map_err(|e| format!("failed to run patch: {e}"))?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
    ))
}

/// Touch one svn file (`(cd %s; touch %s)`): create it when missing,
/// else bump its mtime without truncating.
pub fn run_touch_file(dir: &Path, path: &str) -> Result<(), String> {
    let full = dir.join(path);
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&full)
        .map_err(|e| format!("Failed to touch '{path}': {e}"))?;
    Ok(())
}

/// Link one svn symlink (`(cd %s && ln -sf %s %s)`).
pub fn run_symlink(dir: &Path, target: &str, path: &str) -> Result<(), String> {
    let full = dir.join(path);
    if full.exists() || full.is_symlink() {
        std::fs::remove_file(&full).map_err(|e| format!("Failed to replace '{path}': {e}"))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &full)
        .map_err(|e| format!("Failed to link '{path}': {e}"))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, &full)
        .map_err(|e| format!("Failed to link '{path}': {e}"))?;
    Ok(())
}

/// A loaded bundle plus arcbundle change dictionaries for the svn
/// flow (text sources parse `bundle.text` instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBundle {
    /// The bundle.
    pub bundle: Bundle,
    /// Arcbundle change dicts with decoded hunk corpora, when the source
    /// is an arcbundle.
    pub arcbundle_dicts: Option<(Vec<serde_json::Value>, Vec<Vec<String>>)>,
}

/// Load the bundle for `source`.
///
/// Revision, diff, and arcbundle sources require Conduit
/// (`requiresConduit()`, `:173-175`); `--patch` files work offline.
pub fn load_bundle(
    source: &Source,
    options: &Options,
    io: &mut Io,
    call: Option<&diff_submit::ConduitCall<'_>>,
) -> Result<LoadedBundle, String> {
    match source {
        Source::Named(id) | Source::Revision(id) => load_revision_source_bundle(id, call),
        Source::Diff(id) => load_diff_source_bundle(id, call),
        Source::Arcbundle(path) => {
            load_arcbundle_source_bundle(path, options.encoding.as_deref(), call)
        }
        Source::Patch(path) => Ok(LoadedBundle {
            bundle: load_file_bundle(path, options.encoding.as_deref(), io)?,
            arcbundle_dicts: None,
        }),
    }
}

/// Load the bundle for a `--revision` / positional `D123` source.
fn load_revision_source_bundle(
    id: &str,
    call: Option<&diff_submit::ConduitCall<'_>>,
) -> Result<LoadedBundle, String> {
    let call = require_call(call)?;
    let revision_id = id
        .parse::<u64>()
        .map_err(|_| format!("invalid revision '{id}': expected D12345"))?;
    Ok(LoadedBundle {
        bundle: fetch_revision_bundle(call, revision_id)?,
        arcbundle_dicts: None,
    })
}

/// Load the bundle for a `--diff` source.
fn load_diff_source_bundle(
    id: &str,
    call: Option<&diff_submit::ConduitCall<'_>>,
) -> Result<LoadedBundle, String> {
    let call = require_call(call)?;
    let diff_id = id
        .parse::<u64>()
        .map_err(|_| format!("invalid diff '{id}': expected D12345"))?;
    Ok(LoadedBundle {
        bundle: fetch_diff_bundle(call, diff_id)?,
        arcbundle_dicts: None,
    })
}

/// Load the bundle plus raw change dicts for an `--arcbundle` source.
fn load_arcbundle_source_bundle(
    path: &str,
    encoding: Option<&str>,
    call: Option<&diff_submit::ConduitCall<'_>>,
) -> Result<LoadedBundle, String> {
    require_call(call)?;
    let (bundle, dicts, corpora) = load_arcbundle_with_dicts(path, encoding)?;
    Ok(LoadedBundle {
        bundle,
        arcbundle_dicts: Some((dicts, corpora)),
    })
}

/// Require an authenticated Conduit call, or fail like the runner would
/// for `requiresConduit()` workflows.
fn require_call<'a>(
    call: Option<&'a diff_submit::ConduitCall<'_>>,
) -> Result<&'a diff_submit::ConduitCall<'a>, String> {
    call.ok_or_else(|| {
        "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with \
         .arcconfig"
            .to_string()
    })
}

/// An arcbundle plus raw change dicts and decoded hunk corpora.
type ArcbundleWithDicts = (Bundle, Vec<serde_json::Value>, Vec<Vec<String>>);

/// Load an arcbundle plus its raw change dicts and decoded corpora.
fn load_arcbundle_with_dicts(
    path: &str,
    encoding: Option<&str>,
) -> Result<ArcbundleWithDicts, String> {
    let bundle = load_arcbundle(path, encoding)?;
    let members = tar_list(path)?;
    let meta = arcbundle_meta(path, &members)?;
    let raw = tar_extract(path, "changes.json")?;
    let dicts: Vec<serde_json::Value> = serde_json::from_slice(&raw)
        .map_err(|e| format!("failed to read arcbundle '{path}': {e}"))?;
    let decode_as = encoding.or(meta.encoding.as_deref());
    let mut corpora = Vec::new();
    for dict in &dicts {
        corpora.push(arcbundle_corpora(path, dict, decode_as)?);
    }
    Ok((bundle, dicts, corpora))
}

/// Ensure the bundle base is local, fetching under git when it is not
/// (`:431-444`).
///
/// Prints the verbatim `** INFO ** Base commit is not in local
/// repository; trying to fetch.` line (stdout, markup stripped) and runs
/// `fetch --quiet --all` (failures ignored) before rechecking. Returns
/// whether the base is local now.
pub fn ensure_base_present(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    io: &mut Io,
) -> Result<bool, String> {
    ensure_base_present_ansi(dir, vcs, bundle, io, terminal::ansi_enabled())
}

/// [`ensure_base_present`] with an explicit ANSI setting.
pub fn ensure_base_present_ansi(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    io: &mut Io,
    ansi: bool,
) -> Result<bool, String> {
    ensure_base_present_with_ansi(dir, vcs, bundle, io, &Backends::default(), ansi)
}

/// [`ensure_base_present`] with injectable hg/svn backends (test seam).
///
/// Mercurial presence checks run through `backends.hg`; git uses the
/// system binary and svn is never local, like PHP.
pub fn ensure_base_present_with(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    io: &mut Io,
    backends: &Backends,
) -> Result<bool, String> {
    ensure_base_present_with_ansi(dir, vcs, bundle, io, backends, terminal::ansi_enabled())
}

/// [`ensure_base_present_with`] with an explicit ANSI setting.
pub fn ensure_base_present_with_ansi(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    io: &mut Io,
    backends: &Backends,
    ansi: bool,
) -> Result<bool, String> {
    let mut present = match vcs {
        Vcs::Git => git::has_local_commit(dir, bundle.base_revision.as_deref()),
        Vcs::Hg => bundle
            .base_revision
            .as_deref()
            .is_some_and(|base| hg::ok_with(dir, &backends.hg, &["log", "-r", base])),
        Vcs::Svn => false,
    };
    if !present && vcs == Vcs::Git {
        io.push_out(&status_line_ansi(
            "blue",
            "INFO",
            "Base commit is not in local repository; trying to fetch.",
            ansi,
        ));
        git::output(dir, &["fetch", "--quiet", "--all"]).ok();
        present = git::has_local_commit(dir, bundle.base_revision.as_deref());
    }
    Ok(present)
}

/// Branch state captured around patching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchState {
    /// Original branch/bookmark/ref, when a branch was created.
    pub original: Option<String>,
    /// New branch/bookmark, when a branch was created.
    pub new: Option<String>,
}

/// Create the patch branch when
/// `canBranch() && (shouldBranch() || (shouldCommit() && has_base))`
/// (`:446-467`).
///
/// Captures the original branch (git) or active bookmark (hg), falling
/// back to the canonical `HEAD` / `.` revision.
pub fn maybe_create_branch(
    dir: &Path,
    vcs: Vcs,
    backends: &Backends,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    has_base_local: bool,
) -> Result<BranchState, String> {
    let want =
        can_branch(vcs) && (should_branch(options) || (should_commit(options) && has_base_local));
    if !want {
        return Ok(BranchState::default());
    }
    match vcs {
        Vcs::Git => {
            let original = git::branch_name(dir)
                .ok()
                .flatten()
                .or_else(|| git::canonical_revision_name(dir, "HEAD").ok());
            let new = git_create_branch(dir, bundle, has_base_local, io)?;
            Ok(BranchState {
                original,
                new: Some(new),
            })
        }
        Vcs::Hg => {
            let bookmarks =
                hg::stdout_text_with(dir, &backends.hg, &["bookmarks"]).unwrap_or_default();
            let original = land::parse_hg_bookmarks(&bookmarks)
                .into_iter()
                .find(|(_, active)| *active)
                .map(|(name, _)| name)
                .or_else(|| hg_canonical(dir, &backends.hg, ".").ok());
            let new = hg_create_bookmark(dir, &backends.hg, bundle, has_base_local, io)?;
            Ok(BranchState {
                original,
                new: Some(new),
            })
        }
        Vcs::Svn => Ok(BranchState::default()),
    }
}

/// Plan revision-dependency child options oldest-first (the planning
/// half of `applyDependencies()`, `:930-989`).
///
/// Loads the transitive graph (pruning locally-present commits), prompts
/// on cycles (declining silently skips), resolves PHIDs to `D123`
/// monograms, and returns one child option set per non-start dependency:
/// `--force --skip-dependencies --nobranch` (plus `--nocommit` when the
/// parent would not commit).
pub fn plan_dependency_children(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
) -> Result<Vec<Options>, String> {
    plan_dependency_children_with(dir, vcs, bundle, options, io, call, &Backends::default())
}

/// [`plan_dependency_children`] with injectable hg/svn backends.
///
/// Mercurial local-commit probes run through `backends.hg`; git uses
/// the system binary, like PHP.
pub fn plan_dependency_children_with(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
    backends: &Backends,
) -> Result<Vec<Options>, String> {
    let Some(start_id) = bundle.revision_id else {
        return Ok(Vec::new());
    };
    let Some(hash_key) = dependency_hash_key(vcs) else {
        return Ok(Vec::new());
    };
    let (nodes, start_phid) = load_dependency_nodes(
        call,
        start_id,
        &|hash: Option<&str>| dependency_is_local(dir, vcs, backends, hash),
        hash_key,
    )?;
    if nodes.is_empty() {
        return Ok(Vec::new());
    }
    let (phids, cyclic) = order_dependency_phids(&nodes);
    if !confirm_dependency_cycle(io, cyclic)? {
        return Ok(Vec::new());
    }
    let monograms = fetch_dependency_monograms(call, &phids)?;
    Ok(build_dependency_children(
        options,
        &phids,
        &start_phid,
        &monograms,
    ))
}

/// Commit-hash query key for dependency pruning (`None` under svn, which
/// never applies dependencies).
fn dependency_hash_key(vcs: Vcs) -> Option<&'static str> {
    match vcs {
        Vcs::Git => Some("gtcm"),
        Vcs::Hg => Some("hgcm"),
        Vcs::Svn => None,
    }
}

/// True when the commit for `hash` is already local.
fn dependency_is_local(dir: &Path, vcs: Vcs, backends: &Backends, hash: Option<&str>) -> bool {
    match vcs {
        Vcs::Git => git::has_local_commit(dir, hash),
        _ => hash.is_some_and(|hash| hg::ok_with(dir, &backends.hg, &["log", "-r", hash])),
    }
}

/// Order dependency PHIDs parents-first (discovery order on cycles),
/// reporting whether the graph is cyclic.
fn order_dependency_phids(nodes: &[DependencyNode]) -> (Vec<String>, bool) {
    let (order, cyclic) = order_dependencies(nodes);
    let phids: Vec<String> = if cyclic {
        nodes.iter().map(|node| node.phid.clone()).collect()
    } else {
        order
            .iter()
            .map(|index| nodes[*index].phid.clone())
            .collect()
    };
    (phids, cyclic)
}

/// Confirm past a dependency cycle (true when acyclic); declining means
/// silently skipping the dependencies.
fn confirm_dependency_cycle(io: &mut Io, cyclic: bool) -> Result<bool, String> {
    if !cyclic {
        return Ok(true);
    }
    let issue = "The dependencies for this patch have a cycle. Applying them is not \
         guaranteed to work. Continue anyway?";
    land::console_confirm(io, issue, true)
}

/// Resolve dependency PHIDs to `D123` monograms via `differential.query`.
fn fetch_dependency_monograms(
    call: &diff_submit::ConduitCall<'_>,
    phids: &[String],
) -> Result<std::collections::HashMap<String, String>, String> {
    let result = call(
        diff_submit::METHOD_QUERY,
        &serde_json::json!({"phids": phids}),
    )?;
    let rows = result.as_array().cloned().unwrap_or_default();
    let mut monograms: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in &rows {
        if let (Some(phid), Some(id)) = (
            row.get("phid").and_then(|v| v.as_str()),
            row.get("id").and_then(|v| v.as_u64()),
        ) {
            monograms.insert(phid.to_string(), format!("D{id}"));
        }
    }
    Ok(monograms)
}

/// Build one child option set per non-start dependency:
/// `--force --skip-dependencies --nobranch` (plus `--nocommit` when the
/// parent would not commit).
fn build_dependency_children(
    options: &Options,
    phids: &[String],
    start_phid: &str,
    monograms: &std::collections::HashMap<String, String>,
) -> Vec<Options> {
    let mut children = Vec::new();
    for phid in phids {
        if phid.as_str() == start_phid {
            continue;
        }
        let Some(monogram) = monograms.get(phid) else {
            continue;
        };
        children.push(Options {
            force: true,
            skip_dependencies: true,
            nobranch: true,
            nocommit: !should_commit(options),
            revision: Some(monogram.clone()),
            conduit_uri: options.conduit_uri.clone(),
            ..Options::default()
        });
    }
    children
}

/// Apply revision dependencies oldest-first (`applyDependencies()`,
/// `:930-989`): plan the children, then run each through `child`.
pub fn apply_dependencies(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
    child: &mut dyn FnMut(&Options) -> Result<(), String>,
) -> Result<(), String> {
    apply_dependencies_with(
        dir,
        vcs,
        bundle,
        options,
        io,
        call,
        &Backends::default(),
        child,
    )
}

/// [`apply_dependencies`] with injectable hg/svn backends (test seam).
#[allow(clippy::too_many_arguments)]
pub fn apply_dependencies_with(
    dir: &Path,
    vcs: Vcs,
    bundle: &Bundle,
    options: &Options,
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
    backends: &Backends,
    child: &mut dyn FnMut(&Options) -> Result<(), String>,
) -> Result<(), String> {
    for child_options in
        plan_dependency_children_with(dir, vcs, bundle, options, io, call, backends)?
    {
        child(&child_options)?;
    }
    Ok(())
}

/// Outcome of one patch flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowOutcome {
    /// Applied (or committed).
    Done,
    /// Svn hunks failed (warning printed; exit 1).
    SvnHunksFailed,
}

/// Run one patch flow: clean check, base fetch, branch, dependencies,
/// sanity, and the VCS apply.
fn run_patch_flow(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    backends: &Backends,
    ansi: bool,
) -> Result<FlowOutcome, String> {
    let source = select_source(options)?;
    let Some(vcs) = detect_vcs(dir) else {
        return Err(NOT_A_WORKING_COPY.to_string());
    };
    check_supports(options, vcs.name())?;
    if options.update {
        update_working_copy(dir, vcs, backends, io)?;
    }
    let stash = require_clean_tree_ansi(dir, vcs, backends, io, options.force, ansi)?;
    let outcome = run_patch_flow_inner(dir, options, io, backends, &source, vcs, ansi);
    let pop = unstash_changes(dir, stash);
    match (outcome, pop) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), _) => Err(error),
    }
}

/// The patch flow between the clean check and the stash pop.
fn run_patch_flow_inner(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    backends: &Backends,
    source: &Source,
    vcs: Vcs,
    ansi: bool,
) -> Result<FlowOutcome, String> {
    let client = match source {
        Source::Patch(_) => None,
        _ => Some(land::client_for_dir(options.conduit_uri.as_deref(), dir)?),
    };
    let call = |method: &str, params: &serde_json::Value| {
        client
            .as_ref()
            .expect("conduit client")
            .call(method, params)
            .map_err(|e| e.to_string())
    };
    let call_option: Option<&diff_submit::ConduitCall<'_>> = client
        .as_ref()
        .map(|_| &call as &dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String>);
    let loaded = load_bundle(source, options, io, call_option)?;
    let has_base_local =
        ensure_base_present_with_ansi(dir, vcs, &loaded.bundle, io, backends, ansi)?;
    let branch = maybe_create_branch(
        dir,
        vcs,
        backends,
        &loaded.bundle,
        options,
        io,
        has_base_local,
    )?;
    run_patch_dependencies(
        dir,
        options,
        io,
        backends,
        &loaded.bundle,
        vcs,
        has_base_local,
        call_option,
        ansi,
    )?;
    if !options.force {
        sanity_check(
            dir,
            vcs,
            backends,
            io,
            call_option,
            &loaded.bundle,
            has_base_local,
        )?;
    }
    apply_patch_bundle(
        dir,
        options,
        io,
        backends,
        &loaded.bundle,
        loaded.arcbundle_dicts.as_ref(),
        vcs,
        &branch,
        has_base_local,
        call_option,
        ansi,
    )
}

/// Apply revision dependencies oldest-first when the base is not local and
/// the bundle/options/VCS call for it.
#[allow(clippy::too_many_arguments)]
fn run_patch_dependencies(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    backends: &Backends,
    bundle: &Bundle,
    vcs: Vcs,
    has_base_local: bool,
    call_option: Option<&diff_submit::ConduitCall<'_>>,
    ansi: bool,
) -> Result<(), String> {
    if !has_base_local && should_apply_dependencies(bundle, options, vcs) {
        let call_ref = call_option.expect("conduit for dependencies");
        let children =
            plan_dependency_children_with(dir, vcs, bundle, options, io, call_ref, backends)?;
        for child_options in &children {
            match run_patch_flow(dir, child_options, io, backends, ansi)? {
                FlowOutcome::Done => {}
                FlowOutcome::SvnHunksFailed => return Err(String::new()),
            }
        }
    }
    Ok(())
}

/// Apply the loaded bundle under `vcs`.
#[allow(clippy::too_many_arguments)]
fn apply_patch_bundle(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    backends: &Backends,
    bundle: &Bundle,
    arcbundle_dicts: Option<&(Vec<serde_json::Value>, Vec<Vec<String>>)>,
    vcs: Vcs,
    branch: &BranchState,
    has_base_local: bool,
    call_option: Option<&diff_submit::ConduitCall<'_>>,
    ansi: bool,
) -> Result<FlowOutcome, String> {
    match vcs {
        Vcs::Git => {
            apply_git_ansi(
                dir,
                bundle,
                options,
                io,
                call_option,
                has_base_local,
                branch.original.as_deref(),
                branch.new.as_deref(),
                ansi,
            )?;
            Ok(FlowOutcome::Done)
        }
        Vcs::Hg => {
            apply_hg_ansi(
                dir,
                &backends.hg,
                bundle,
                options,
                io,
                call_option,
                has_base_local,
                branch.original.as_deref(),
                branch.new.as_deref(),
                ansi,
            )?;
            Ok(FlowOutcome::Done)
        }
        Vcs::Svn => apply_patch_svn(dir, backends, bundle, arcbundle_dicts, io, ansi),
    }
}

/// Apply the loaded bundle under svn, from arcbundle dicts (with decoded
/// corpora) or parsed git-patch text.
fn apply_patch_svn(
    dir: &Path,
    backends: &Backends,
    bundle: &Bundle,
    arcbundle_dicts: Option<&(Vec<serde_json::Value>, Vec<Vec<String>>)>,
    io: &mut Io,
    ansi: bool,
) -> Result<FlowOutcome, String> {
    let changes = match arcbundle_dicts {
        Some((dicts, corpora)) => svn_changes_from_dicts(dicts, corpora),
        None => parse_git_patch_for_svn(&bundle.text),
    };
    let outcome = apply_svn_ansi(
        dir,
        &backends.svn,
        &changes,
        io,
        &mut |_, patch| run_patch_file(dir, patch),
        &mut |path| run_touch_file(dir, path),
        &mut |target, path| run_symlink(dir, target, path),
        ansi,
    )?;
    Ok(match outcome {
        SvnOutcome::Applied => FlowOutcome::Done,
        SvnOutcome::HunksFailed => FlowOutcome::SvnHunksFailed,
    })
}

/// Run the workflow with scripted or live IO, returning collected output.
///
/// Parse errors append the usage text; `--help` returns the usage text;
/// `--preview` prints the patch without touching the working copy (no
/// repository needed for file/stdin sources). Svn hunks failures return
/// `Err("")`: the warning is already printed, so callers must exit 1
/// without printing more.
pub fn execute_with_io(
    dir: &Path,
    argv: &[String],
    io: &mut Io,
    backends: &Backends,
) -> Result<Out, String> {
    execute_with_io_ansi(dir, argv, io, backends, terminal::ansi_enabled())
}

/// [`execute_with_io`] with an explicit ANSI setting.
pub fn execute_with_io_ansi(
    dir: &Path,
    argv: &[String],
    io: &mut Io,
    backends: &Backends,
    ansi: bool,
) -> Result<Out, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            io.push_out(USAGE);
            return Ok(std::mem::take(&mut io.out));
        }
        Err(message) => return Err(format!("{message}\n\n{USAGE}")),
    };
    if options.preview {
        return execute_preview(dir, &options, io);
    }
    match run_patch_flow(dir, &options, io, backends, ansi)? {
        FlowOutcome::Done => Ok(std::mem::take(&mut io.out)),
        FlowOutcome::SvnHunksFailed => Err(String::new()),
    }
}

/// Print the patch for `--preview` without touching the working copy.
fn execute_preview(dir: &Path, options: &Options, io: &mut Io) -> Result<Out, String> {
    let source = select_source(options)?;
    let client = match &source {
        Source::Patch(_) => None,
        _ => Some(land::client_for_dir(options.conduit_uri.as_deref(), dir)?),
    };
    let call = |method: &str, params: &serde_json::Value| {
        client
            .as_ref()
            .expect("conduit client")
            .call(method, params)
            .map_err(|e| e.to_string())
    };
    let call_option: Option<&diff_submit::ConduitCall<'_>> = client
        .as_ref()
        .map(|_| &call as &dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String>);
    let loaded = load_bundle(&source, options, io, call_option)?;
    io.push_out(&loaded.bundle.text);
    Ok(std::mem::take(&mut io.out))
}

/// Run the workflow with scripted stdin patch text.
pub fn execute_with_stdin(dir: &Path, argv: &[String], stdin: Option<&str>) -> Result<Out, String> {
    execute_with_stdin_ansi(dir, argv, stdin, terminal::ansi_enabled())
}

/// [`execute_with_stdin`] with an explicit ANSI setting.
pub fn execute_with_stdin_ansi(
    dir: &Path,
    argv: &[String],
    stdin: Option<&str>,
    ansi: bool,
) -> Result<Out, String> {
    let mut io = Io::buffered();
    io.patch_stdin = stdin.map(str::to_string);
    execute_with_io_ansi(dir, argv, &mut io, &Backends::default(), ansi)
}

/// Run the workflow rooted at `dir` with buffered IO.
pub fn execute(dir: &Path, argv: &[String]) -> Result<Out, String> {
    execute_ansi(dir, argv, terminal::ansi_enabled())
}

/// [`execute`] with an explicit ANSI setting.
pub fn execute_ansi(dir: &Path, argv: &[String], ansi: bool) -> Result<Out, String> {
    execute_with_io_ansi(dir, argv, &mut Io::buffered(), &Backends::default(), ansi)
}

/// Run the workflow rooted at `dir` with live console IO.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let ansi = terminal::ansi_enabled();
    match execute_with_io_ansi(dir, argv, &mut Io::live(), &Backends::default(), ansi) {
        Ok(_) => ExitCode::SUCCESS,
        Err(message) if message.is_empty() => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("arc patch: {message}");
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
#[path = "patch_tests.rs"]
mod patch_tests;
