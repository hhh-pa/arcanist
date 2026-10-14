//! `arc-rs diff`: local change inspection, plus opt-in Differential submit.
//!
//! Read-only half of `src/workflow/ArcanistDiffWorkflow.php` by default:
//! shows the working copy diff against a base revision as text or JSON.
//! Message-field parsing (`--message-file`, `--reviewers`, `--cc`),
//! commit-range selection (positional `[commit]`, `--head`), and the submit
//! flows (`--create`, `--update`, `--only`) reuse `diff_local` and
//! `diff_submit`. Flag names and Conduit payload keys are verbatim from PHP.
//!
//! The argument layer ports `ArcanistWorkflow::parseArguments()`
//! (`src/workflow/ArcanistWorkflow.php:788-926`), the `willRunWorkflow()`
//! VCS-supports check (`:1494-1517`), and `parseBaseCommitArgument()`
//! (`:1825-1846`) exactly, including error strings. Top-level `arc` actually
//! parses through phutil's toolset parser (not vendored here), so its
//! rendering of these same declarations may differ in wording; the legacy
//! parser below is the in-repo contract.
//!
//! Divergences from `ArcanistDiffWorkflow::run()` (`:345-577`):
//! - PHP always submits to Differential; this port stays read-only unless
//!   `--create`, `--update`, or `--only` is given.
//! - Submitted changes carry file metadata only (no hunk corpora): the
//!   unified-diff parser (`generateChanges()`, `:802-1026`) is not ported.
//! - All 30 flags parse, but only the read-only and submit flows run: raw
//!   stdin/command sources, `--edit`, `--verbatim`, `--draft`, lint/unit
//!   execution, commit amend, and browser opening are still pending, as are
//!   hg/svn working copies and `.arcconfig` custom arguments.

extern crate self as arc_rs;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{config, diff_local, diff_submit, svn};

/// Subcommand name, matching `arc diff`.
pub const NAME: &str = "diff";

const USAGE: &str = "usage: arc-rs diff [--message MSG] [--message-file FILE] [--edit] [--raw] [--raw-command CMD] [--create | --update ID] [--draft] [--nounit] [--nolint] [--only] [--allow-untracked] [--apply-patches | --never-apply-patches] [--amend-all] [--amend-autofixes] [-a] [--json] [--no-amend] [--uncommitted] [--verbatim] [--reviewers NAMES] [--cc NAMES] [--skip-binaries] [--skip-staging] [--base REV] [--coverage | --no-coverage] [--browse] [--head REV] [--conduit-uri URI] [commit]\n";

/// Parsed `diff` options.
///
/// Flag names are verbatim from
/// `ArcanistDiffWorkflow::getArguments()` (`:86-339`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Explicit `--base` revision, if given (`base`, `:297-304`).
    pub base: Option<String>,
    /// Whether `--json` output was requested (`json`, `:244-247`).
    pub json: bool,
    /// End of the commit range (`head`, `:325-335`).
    pub head: Option<String>,
    /// Positional base commit (`arc diff [commit]`, `:38-44`; at most one,
    /// via `parseBaseCommitArgument()`,
    /// `src/workflow/ArcanistWorkflow.php:1825-1846`). Derived from
    /// [`Options::paths`] by inspection, not by parsing.
    pub commit: Option<String>,
    /// Update message (`message`, `:88-94`).
    pub message: Option<String>,
    /// Revision message file (`message-file`, `:95-102`).
    pub message_file: Option<String>,
    /// Reviewers for created revisions (`reviewers`, `:275-282`).
    pub reviewers: Option<String>,
    /// CCs for created revisions (`cc`, `:283-290`).
    pub cc: Option<String>,
    /// Revision to update (`update`, `:164-167`).
    pub update: Option<String>,
    /// Always create a new revision (`create`, `:147-163`).
    pub create: bool,
    /// Only create a diff, no revision (`only`, `:190-198`).
    pub only: bool,
    /// Conduit URI override (global `--conduit-uri`,
    /// `src/workflow/ArcanistWorkflow.php:304-310`).
    pub conduit_uri: Option<String>,
    /// Edit revision information before updating (`edit`, `:103-114`).
    pub edit: bool,
    /// Read the diff from stdin (`raw`, `:115-135`).
    pub raw: bool,
    /// Generate the diff via a command (`raw-command`, `:136-146`).
    pub raw_command: Option<String>,
    /// Create a draft revision (`draft`, `:168-179`).
    pub draft: bool,
    /// Do not run unit tests (`nounit`, `:180-182`).
    pub nounit: bool,
    /// Do not run lint (`nolint`, `:183-189`).
    pub nolint: bool,
    /// Skip untracked-file checks (`allow-untracked`, `:199-201`).
    pub allow_untracked: bool,
    /// Apply lint patches without prompting (`apply-patches`, `:202-212`).
    pub apply_patches: bool,
    /// Never apply lint patches (`never-apply-patches`, `:213-221`).
    pub never_apply_patches: bool,
    /// Amend HEAD with all lint patches (`amend-all`, `:222-229`).
    pub amend_all: bool,
    /// Amend HEAD with lint autofixes (`amend-autofixes`, `:230-237`).
    pub amend_autofixes: bool,
    /// Add all files to the commit (`add-all`, `-a`, `:238-243`).
    pub add_all: bool,
    /// Never amend commits with lint patches (`no-amend`, `:248-251`).
    pub no_amend: bool,
    /// Suppress the uncommitted-changes warning (`uncommitted`, `:252-257`).
    pub uncommitted: bool,
    /// Use the commit message verbatim (`verbatim`, `:258-274`).
    pub verbatim: bool,
    /// Do not upload binaries (`skip-binaries`, `:291-293`).
    pub skip_binaries: bool,
    /// Do not copy changes to staging (`skip-staging`, `:294-296`).
    pub skip_staging: bool,
    /// Always enable coverage (`coverage`, `:305-313`).
    pub coverage: bool,
    /// Always disable coverage (`no-coverage`, `:314-319`).
    pub no_coverage: bool,
    /// Open the result in a browser (`browse`, `:320-323`).
    pub browse: bool,
    /// Positional arguments: the wildcard `*` maps to `paths` (`:324`;
    /// `parseArguments()`, `src/workflow/ArcanistWorkflow.php:793-798`).
    pub paths: Vec<String>,
    /// Canonical long flag names in first-provision order (PHP `$dict`
    /// insertion order, `ArcanistWorkflow.php:791-884`). Empty for
    /// hand-built options, which skip the order-dependent checks.
    pub provided: Vec<&'static str>,
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

/// A value-taking `diff` flag (`--base REV`, `--base=REV`, `-m MSG`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueFlag {
    Message,
    MessageFile,
    RawCommand,
    Update,
    Reviewers,
    Cc,
    Base,
    Head,
    ConduitUri,
}

/// First half of the boolean `diff` flags, in spec order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EarlyBoolFlag {
    Edit,
    Raw,
    Create,
    Draft,
    NoUnit,
    NoLint,
    Only,
    AllowUntracked,
    ApplyPatches,
    NeverApplyPatches,
    AmendAll,
}

/// Second half of the boolean `diff` flags, in spec order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LateBoolFlag {
    AmendAutofixes,
    AddAll,
    Json,
    NoAmend,
    Uncommitted,
    Verbatim,
    SkipBinaries,
    SkipStaging,
    Coverage,
    NoCoverage,
    Browse,
}

/// Any `diff` flag, grouped so each store match stays small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Value(ValueFlag),
    Early(EarlyBoolFlag),
    Late(LateBoolFlag),
}

impl Flag {
    /// True for `param` flags, which consume the next token as their value.
    fn takes_param(self) -> bool {
        matches!(self, Flag::Value(_))
    }
}

/// One `getArguments()` entry: canonical long name, short alias, and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlagSpec {
    long: &'static str,
    short: Option<char>,
    flag: Flag,
}

/// `diff` flags in `getArguments()` order
/// (`src/workflow/ArcanistDiffWorkflow.php:86-339`), plus the global
/// `--conduit-uri` this port threads through every command.
const FLAG_SPECS: &[FlagSpec] = &[
    FlagSpec {
        long: "message",
        short: Some('m'),
        flag: Flag::Value(ValueFlag::Message),
    },
    FlagSpec {
        long: "message-file",
        short: Some('F'),
        flag: Flag::Value(ValueFlag::MessageFile),
    },
    FlagSpec {
        long: "edit",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::Edit),
    },
    FlagSpec {
        long: "raw",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::Raw),
    },
    FlagSpec {
        long: "raw-command",
        short: None,
        flag: Flag::Value(ValueFlag::RawCommand),
    },
    FlagSpec {
        long: "create",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::Create),
    },
    FlagSpec {
        long: "update",
        short: None,
        flag: Flag::Value(ValueFlag::Update),
    },
    FlagSpec {
        long: "draft",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::Draft),
    },
    FlagSpec {
        long: "nounit",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::NoUnit),
    },
    FlagSpec {
        long: "nolint",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::NoLint),
    },
    FlagSpec {
        long: "only",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::Only),
    },
    FlagSpec {
        long: "allow-untracked",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::AllowUntracked),
    },
    FlagSpec {
        long: "apply-patches",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::ApplyPatches),
    },
    FlagSpec {
        long: "never-apply-patches",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::NeverApplyPatches),
    },
    FlagSpec {
        long: "amend-all",
        short: None,
        flag: Flag::Early(EarlyBoolFlag::AmendAll),
    },
    FlagSpec {
        long: "amend-autofixes",
        short: None,
        flag: Flag::Late(LateBoolFlag::AmendAutofixes),
    },
    FlagSpec {
        long: "add-all",
        short: Some('a'),
        flag: Flag::Late(LateBoolFlag::AddAll),
    },
    FlagSpec {
        long: "json",
        short: None,
        flag: Flag::Late(LateBoolFlag::Json),
    },
    FlagSpec {
        long: "no-amend",
        short: None,
        flag: Flag::Late(LateBoolFlag::NoAmend),
    },
    FlagSpec {
        long: "uncommitted",
        short: None,
        flag: Flag::Late(LateBoolFlag::Uncommitted),
    },
    FlagSpec {
        long: "verbatim",
        short: None,
        flag: Flag::Late(LateBoolFlag::Verbatim),
    },
    FlagSpec {
        long: "reviewers",
        short: None,
        flag: Flag::Value(ValueFlag::Reviewers),
    },
    FlagSpec {
        long: "cc",
        short: None,
        flag: Flag::Value(ValueFlag::Cc),
    },
    FlagSpec {
        long: "skip-binaries",
        short: None,
        flag: Flag::Late(LateBoolFlag::SkipBinaries),
    },
    FlagSpec {
        long: "skip-staging",
        short: None,
        flag: Flag::Late(LateBoolFlag::SkipStaging),
    },
    FlagSpec {
        long: "base",
        short: None,
        flag: Flag::Value(ValueFlag::Base),
    },
    FlagSpec {
        long: "coverage",
        short: None,
        flag: Flag::Late(LateBoolFlag::Coverage),
    },
    FlagSpec {
        long: "no-coverage",
        short: None,
        flag: Flag::Late(LateBoolFlag::NoCoverage),
    },
    FlagSpec {
        long: "browse",
        short: None,
        flag: Flag::Late(LateBoolFlag::Browse),
    },
    FlagSpec {
        long: "head",
        short: None,
        flag: Flag::Value(ValueFlag::Head),
    },
    FlagSpec {
        long: "conduit-uri",
        short: None,
        flag: Flag::Value(ValueFlag::ConduitUri),
    },
];

/// Find a spec by long name (`--message`).
fn lookup_long(name: &str) -> Option<FlagSpec> {
    FLAG_SPECS.iter().find(|spec| spec.long == name).copied()
}

/// Find a spec by short spelling (`-m`).
///
/// PHP matches the whole remainder after `-` against the short map
/// (`ArcanistWorkflow.php:852-862`): no combining, no `-m=value`.
fn lookup_short(spelling: &str) -> Option<FlagSpec> {
    let short = spelling.parse::<char>().ok()?;
    FLAG_SPECS
        .iter()
        .find(|spec| spec.short == Some(short))
        .copied()
}

/// Store a parsed value-flag value in `options`.
fn store_value_flag(options: &mut Options, flag: ValueFlag, value: String) {
    match flag {
        ValueFlag::Message => options.message = Some(value),
        ValueFlag::MessageFile => options.message_file = Some(value),
        ValueFlag::RawCommand => options.raw_command = Some(value),
        ValueFlag::Update => options.update = Some(value),
        ValueFlag::Reviewers => options.reviewers = Some(value),
        ValueFlag::Cc => options.cc = Some(value),
        ValueFlag::Base => options.base = Some(value),
        ValueFlag::Head => options.head = Some(value),
        ValueFlag::ConduitUri => options.conduit_uri = Some(value),
    }
}

/// Store one of the first-half boolean flags in `options`.
fn store_early_bool_flag(options: &mut Options, flag: EarlyBoolFlag) {
    match flag {
        EarlyBoolFlag::Edit => options.edit = true,
        EarlyBoolFlag::Raw => options.raw = true,
        EarlyBoolFlag::Create => options.create = true,
        EarlyBoolFlag::Draft => options.draft = true,
        EarlyBoolFlag::NoUnit => options.nounit = true,
        EarlyBoolFlag::NoLint => options.nolint = true,
        EarlyBoolFlag::Only => options.only = true,
        EarlyBoolFlag::AllowUntracked => options.allow_untracked = true,
        EarlyBoolFlag::ApplyPatches => options.apply_patches = true,
        EarlyBoolFlag::NeverApplyPatches => options.never_apply_patches = true,
        EarlyBoolFlag::AmendAll => options.amend_all = true,
    }
}

/// Store one of the second-half boolean flags in `options`.
fn store_late_bool_flag(options: &mut Options, flag: LateBoolFlag) {
    match flag {
        LateBoolFlag::AmendAutofixes => options.amend_autofixes = true,
        LateBoolFlag::AddAll => options.add_all = true,
        LateBoolFlag::Json => options.json = true,
        LateBoolFlag::NoAmend => options.no_amend = true,
        LateBoolFlag::Uncommitted => options.uncommitted = true,
        LateBoolFlag::Verbatim => options.verbatim = true,
        LateBoolFlag::SkipBinaries => options.skip_binaries = true,
        LateBoolFlag::SkipStaging => options.skip_staging = true,
        LateBoolFlag::Coverage => options.coverage = true,
        LateBoolFlag::NoCoverage => options.no_coverage = true,
        LateBoolFlag::Browse => options.browse = true,
    }
}

/// Record one parsed flag in `options` (`$dict[$arg_key] = ...`,
/// `ArcanistWorkflow.php:867-883`), tracking first-provision order.
///
/// Re-providing a flag overwrites (PHP has no "provided twice" error).
fn store_flag(options: &mut Options, spec: FlagSpec, value: Option<String>) {
    if !options.provided.contains(&spec.long) {
        options.provided.push(spec.long);
    }
    match spec.flag {
        Flag::Value(value_flag) => {
            store_value_flag(options, value_flag, value.unwrap_or_default());
        }
        Flag::Early(bool_flag) => store_early_bool_flag(options, bool_flag),
        Flag::Late(bool_flag) => store_late_bool_flag(options, bool_flag),
    }
}

/// Parse one `--name` or `--name=value` token.
///
/// Values may be empty and may look like flags (`--message --json` takes
/// `--json` as the message); only a trailing `param` flag errors
/// (`ArcanistWorkflow.php:867-883`). PHP's British-spelling corrector
/// (`:834-843`) can never fire here: no `diff` flag name has a
/// British-variant pair, so unknown names always error.
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
            // `--bool=value`: PHP splices the value back as the next token
            // (`:826-832`), so it parses as a positional or even a flag.
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

/// Parse one `-x` token (`ArcanistWorkflow.php:852-862`).
fn parse_short_flag(
    options: &mut Options,
    queue: &mut VecDeque<String>,
    typed: &str,
    spelling: &str,
) -> Result<(), String> {
    let Some(spec) = lookup_short(spelling) else {
        return Err(format!("Unknown argument '{spelling}'. Try 'arc help'."));
    };
    if !spec.flag.takes_param() {
        store_flag(options, spec, None);
        return Ok(());
    }
    let value = queue
        .pop_front()
        .ok_or_else(|| format!("Option '{typed}' requires a parameter."))?;
    store_flag(options, spec, Some(value));
    Ok(())
}

/// Parse `argv` (excluding the subcommand name).
///
/// Ports `ArcanistWorkflow::parseArguments()`
/// (`src/workflow/ArcanistWorkflow.php:788-926`): `--` ends flags, positionals
/// collect into `paths` (the `*` wildcard), and conflicts are checked after
/// the whole line parses. Returns `Ok(None)` when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut queue: VecDeque<String> = argv.iter().cloned().collect();
    let mut end_of_flags = false;
    while let Some(arg) = queue.pop_front() {
        if end_of_flags {
            options.paths.push(arg);
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
            parse_short_flag(&mut options, &mut queue, &arg, spelling)?;
            continue;
        }
        options.paths.push(arg);
    }
    check_conflicts(&options.provided)?;
    Ok(Some(options))
}

/// How a `conflicts` reason renders (`ArcanistWorkflow.php:905-909`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictReason {
    /// `null`: the message ends with a bare `.`.
    Bare,
    /// `true`: PHP stringifies it to `1`, rendering `: 1`.
    One,
    /// `pht(...)` text, rendering `: {text}`.
    Text(&'static str),
}

/// One flag's `conflicts` declaration, in declaration order.
struct ConflictDecl {
    flag: &'static str,
    conflicts: &'static [(&'static str, ConflictReason)],
}

/// `diff` conflict declarations, verbatim from `getArguments()`.
const CONFLICTS: &[ConflictDecl] = &[
    ConflictDecl {
        flag: "raw",
        conflicts: &[
            (
                "apply-patches",
                ConflictReason::Text("'--raw' disables lint."),
            ),
            (
                "never-apply-patches",
                ConflictReason::Text("'--raw' disables lint."),
            ),
            (
                "create",
                ConflictReason::Text(
                    "'--raw' and '--create' both need stdin. Use '--raw-command'.",
                ),
            ),
            (
                "edit",
                ConflictReason::Text("'--raw' and '--edit' both need stdin. Use '--raw-command'."),
            ),
            ("raw-command", ConflictReason::Bare),
        ],
    },
    ConflictDecl {
        flag: "raw-command",
        conflicts: &[
            (
                "apply-patches",
                ConflictReason::Text("'--raw-command' disables lint."),
            ),
            (
                "never-apply-patches",
                ConflictReason::Text("'--raw-command' disables lint."),
            ),
        ],
    },
    ConflictDecl {
        flag: "create",
        conflicts: &[
            (
                "edit",
                ConflictReason::Text("'--create' can not be used with '--edit'."),
            ),
            (
                "only",
                ConflictReason::Text("'--create' can not be used with '--only'."),
            ),
            (
                "update",
                ConflictReason::Text("'--create' can not be used with '--update'."),
            ),
        ],
    },
    ConflictDecl {
        flag: "draft",
        conflicts: &[
            ("edit", ConflictReason::Bare),
            ("only", ConflictReason::Bare),
            ("update", ConflictReason::Bare),
        ],
    },
    ConflictDecl {
        flag: "nolint",
        conflicts: &[
            (
                "apply-patches",
                ConflictReason::Text("'--nolint' suppresses lint."),
            ),
            (
                "never-apply-patches",
                ConflictReason::Text("'--nolint' suppresses lint."),
            ),
        ],
    },
    ConflictDecl {
        flag: "only",
        conflicts: &[
            (
                "edit",
                ConflictReason::Text("'--only' does affect revisions."),
            ),
            (
                "message",
                ConflictReason::Text("'--only' does not update any revision."),
            ),
        ],
    },
    ConflictDecl {
        flag: "apply-patches",
        conflicts: &[("never-apply-patches", ConflictReason::One)],
    },
    ConflictDecl {
        flag: "never-apply-patches",
        conflicts: &[("apply-patches", ConflictReason::One)],
    },
    ConflictDecl {
        flag: "verbatim",
        conflicts: &[
            ("update", ConflictReason::One),
            ("only", ConflictReason::One),
            ("raw", ConflictReason::One),
            ("raw-command", ConflictReason::One),
            ("message-file", ConflictReason::One),
        ],
    },
    ConflictDecl {
        flag: "reviewers",
        conflicts: &[
            ("only", ConflictReason::One),
            ("update", ConflictReason::One),
        ],
    },
    ConflictDecl {
        flag: "cc",
        conflicts: &[
            ("only", ConflictReason::One),
            ("update", ConflictReason::One),
        ],
    },
    ConflictDecl {
        flag: "coverage",
        conflicts: &[("no-coverage", ConflictReason::Bare)],
    },
];

/// Reject flag combinations PHP declares conflicting
/// (`ArcanistWorkflow.php:899-919`).
///
/// The outer order is first-provision order, so symmetric pairs name the
/// earlier flag first; `--update` plus `--only` is allowed:
/// `shouldOnlyCreateDiff()` (`ArcanistDiffWorkflow.php:724-738`) lets
/// `--update` win.
fn check_conflicts(provided: &[&'static str]) -> Result<(), String> {
    for key in provided {
        let Some(decl) = CONFLICTS.iter().find(|decl| decl.flag == *key) else {
            continue;
        };
        for (conflict, reason) in decl.conflicts {
            if provided.contains(conflict) {
                let suffix = match reason {
                    ConflictReason::Bare => ".".to_string(),
                    ConflictReason::One => ": 1".to_string(),
                    ConflictReason::Text(text) => format!(": {text}"),
                };
                return Err(format!(
                    "Arguments '--{key}' and '--{conflict}' are mutually exclusive{suffix}"
                ));
            }
        }
    }
    Ok(())
}

/// One flag's `supports`/`nosupport` declaration.
struct SupportDecl {
    flag: &'static str,
    systems: &'static [&'static str],
    nosupport: &'static [(&'static str, &'static str)],
}

/// `diff` VCS-support declarations, verbatim from `getArguments()`.
const SUPPORTS: &[SupportDecl] = &[
    SupportDecl {
        flag: "edit",
        systems: &["git", "hg"],
        nosupport: &[(
            "svn",
            "Edit revisions via the web interface when using SVN.",
        )],
    },
    SupportDecl {
        flag: "uncommitted",
        systems: &["hg"],
        nosupport: &[],
    },
    SupportDecl {
        flag: "verbatim",
        systems: &["hg", "git"],
        nosupport: &[],
    },
    SupportDecl {
        flag: "base",
        systems: &["git", "hg"],
        nosupport: &[("svn", "Subversion does not use base commits.")],
    },
    SupportDecl {
        flag: "head",
        systems: &["git"],
        nosupport: &[
            ("svn", "Subversion does not support commit ranges."),
            ("hg", "Mercurial does not support --head yet."),
        ],
    },
];

/// Working-copy VCS, named like `getSourceControlSystemName()`
/// (`src/repository/api/ArcanistGitAPI.php:49-51`,
/// `ArcanistMercurialAPI.php:45-47`, `ArcanistSubversionAPI.php:18-20`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vcs {
    Git,
    Hg,
    Svn,
}

impl Vcs {
    fn name(self) -> &'static str {
        match self {
            Vcs::Git => "git",
            Vcs::Hg => "hg",
            Vcs::Svn => "svn",
        }
    }
}

/// Reject flags the working-copy VCS does not support, in provision order
/// (`willRunWorkflow()`, `ArcanistWorkflow.php:1494-1517`).
fn check_supports(vcs: Vcs, provided: &[&'static str]) -> Result<(), String> {
    for name in provided {
        let Some(decl) = SUPPORTS.iter().find(|decl| decl.flag == *name) else {
            continue;
        };
        if decl.systems.contains(&vcs.name()) {
            continue;
        }
        let mut message = format!("Option '--{name}' is not supported under {}.", vcs.name());
        if let Some((_, extra)) = decl
            .nosupport
            .iter()
            .find(|(system, _)| *system == vcs.name())
        {
            message.push(' ');
            message.push_str(extra);
        }
        return Err(message);
    }
    Ok(())
}

/// Walk toward the filesystem root for a VCS marker dir, mirroring
/// [`svn::find_metadata_path`].
fn find_dot_dir(start: &Path, name: &str) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Detect the working-copy VCS for the supports check, git first.
fn detect_vcs(cwd: &Path) -> Option<Vcs> {
    if is_inside_git_repo(cwd) {
        return Some(Vcs::Git);
    }
    if find_dot_dir(cwd, ".hg").is_some() {
        return Some(Vcs::Hg);
    }
    if svn::find_metadata_path(cwd).is_some() {
        return Some(Vcs::Svn);
    }
    None
}

/// Run the pre-inspection argument checks: the `willRunWorkflow()` supports
/// check plus the `run()` base-commit derivation
/// (`ArcanistDiffWorkflow.php:586-593`).
///
/// Without a detectable working copy the options pass through untouched;
/// `inspect()` reports the working-copy error. An empty `--head=` normalizes
/// to absent: PHP tests head truthiness, and only the unported amend check
/// (`:1111`) distinguishes empty from missing.
fn prepare(cwd: &Path, options: &Options) -> Result<Options, String> {
    let mut resolved = options.clone();
    if resolved.head.as_deref() == Some("") {
        resolved.head = None;
    }
    let Some(vcs) = detect_vcs(cwd) else {
        return Ok(resolved);
    };
    check_supports(vcs, &resolved.provided)?;
    if !resolved.paths.is_empty() {
        resolved.commit = diff_local::parse_base_commit_argument(vcs != Vcs::Svn, &resolved.paths)?;
    }
    if vcs != Vcs::Git {
        return Err(format!(
            "{} working copies are not yet ported (see T1128)",
            vcs.name()
        ));
    }
    Ok(resolved)
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
    numstat_range(cwd, base, None)
}

/// Collect numstat entries for `base` against `head` (or the worktree).
pub fn numstat_range(cwd: &Path, base: &str, head: Option<&str>) -> Result<Vec<FileDiff>, String> {
    let mut args = vec!["diff", "--numstat", "-z", base];
    if let Some(rev) = head {
        args.push(rev);
    }
    args.push("--");
    let out = git_output(cwd, &args)?;
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

/// Render merged commit-message fields as a JSON object.
pub fn fields_json(fields: &diff_local::CommitFields) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in fields {
        let rendered = match value {
            diff_local::FieldValue::Text(text) => serde_json::Value::String(text.clone()),
            diff_local::FieldValue::List(items) => serde_json::Value::Array(
                items
                    .iter()
                    .map(|item| serde_json::Value::String(item.clone()))
                    .collect(),
            ),
        };
        map.insert(key.clone(), rendered);
    }
    serde_json::Value::Object(map)
}

/// Stat plus full unified diff against `base`, via the git binary.
pub fn default_diff_output(cwd: &Path, base: &str) -> Result<Vec<u8>, String> {
    default_diff_output_range(cwd, base, None)
}

/// Stat plus full unified diff for `base` against `head` (or the worktree).
pub fn default_diff_output_range(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut stat_args = vec!["--no-pager", "diff", "--no-color", "--stat", base];
    if let Some(rev) = head {
        stat_args.push(rev);
    }
    stat_args.push("--");
    let mut patch_args = vec!["--no-pager", "diff", "--no-color", base];
    if let Some(rev) = head {
        patch_args.push(rev);
    }
    patch_args.push("--");
    let mut combined = Vec::new();
    for args in [stat_args, patch_args] {
        let out = git_output(cwd, &args)?;
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

/// Resolved diff range: base commit plus optional head commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    /// Resolved base commit SHA.
    pub base: String,
    /// Symbolic head revision (`--head`), if given.
    pub head: Option<String>,
}

/// Resolve the base commit and validate the head of the range.
///
/// Explicit `--base` resolves as before; the positional commit resolves to
/// its merge-base with the head (`ArcanistGitAPI::buildBaseCommit()`,
/// `src/repository/api/ArcanistGitAPI.php:208-243`); otherwise the default
/// base applies. `--head` must descend from the base
/// (`ArcanistGitAPI.php:132-157`).
pub fn resolve_range(cwd: &Path, options: &Options) -> Result<Range, String> {
    if options.base.is_some() && options.commit.is_some() {
        return Err("'--base' can not be used with a base commit.".to_string());
    }
    let head_symbolic = options.head.as_deref().unwrap_or("HEAD");
    let base = if let Some(rev) = options.base.as_deref() {
        rev_to_commit(cwd, rev).ok_or_else(|| format!("invalid base revision '{rev}'"))?
    } else if let Some(commit) = options.commit.as_deref() {
        diff_local::resolve_explicit_base(cwd, commit, head_symbolic)?
    } else {
        resolve_base(cwd, None)?
    };
    if let Some(head) = options.head.as_deref() {
        diff_local::require_head_descends_base(cwd, &base, head)?;
    }
    Ok(Range {
        base,
        head: options.head.clone(),
    })
}

/// Merge commit messages (oldest-first) plus the `--reviewers`/`--cc` faux
/// tags into one field dictionary.
///
/// Ports the merge in `parseCommitMessagesIntoFields()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1890-1969`): the faux flags
/// message parses last, like PHP's prepend-then-reverse (`:1839-1904`).
pub fn merge_message_fields(
    oldest_first: &[String],
    reviewers: Option<&str>,
    cc: Option<&str>,
) -> diff_local::CommitFields {
    let mut parsed: Vec<diff_local::CommitFields> = oldest_first
        .iter()
        .map(|text| diff_local::parse_field_tags(text))
        .collect();
    if let Some(faux) = diff_local::faux_flag_message(reviewers, cc) {
        parsed.push(diff_local::parse_field_tags(&faux));
    }
    diff_local::merge_fields_oldest_first(&parsed)
}

/// Collect the merged revision fields for `options`.
///
/// `--message-file` wins alone (`buildCommitMessage()`,
/// `src/workflow/ArcanistDiffWorkflow.php:1331-1338`); otherwise the local
/// commits in the range merge with the `--reviewers`/`--cc` faux message.
pub fn collect_fields(
    cwd: &Path,
    options: &Options,
    base: &str,
    head: Option<&str>,
) -> Result<diff_local::CommitFields, String> {
    if let Some(path) = options.message_file.as_deref() {
        let message = diff_local::commit_message_from_file(Path::new(path))?;
        return Ok(diff_local::parse_field_tags(&message.raw_corpus));
    }
    let infos = diff_local::get_local_commit_information(cwd, base, head.unwrap_or("HEAD"))?;
    let oldest_first: Vec<String> = infos
        .iter()
        .rev()
        .map(|info| info.message.clone())
        .collect();
    Ok(merge_message_fields(
        &oldest_first,
        options.reviewers.as_deref(),
        options.cc.as_deref(),
    ))
}

/// How a submit reaches Differential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitMode {
    /// `--only`: create a diff, no revision
    /// (`src/workflow/ArcanistDiffWorkflow.php:724-738`).
    Only,
    /// `--create`: create a new revision (`:147-163`).
    Create,
    /// `--update`: update a revision (`:164-167`).
    Update(u64),
}

/// Derive the submit mode from `options`, or `None` to stay read-only.
///
/// `--update` wins over `--only`, like `shouldOnlyCreateDiff()`
/// (`src/workflow/ArcanistDiffWorkflow.php:724-738`).
pub fn submit_mode(options: &Options) -> Result<Option<SubmitMode>, String> {
    if options.create {
        return Ok(Some(SubmitMode::Create));
    }
    if let Some(target) = options.update.as_deref() {
        return Ok(Some(SubmitMode::Update(
            diff_local::parse_update_revision_id(target)?,
        )));
    }
    if options.only {
        return Ok(Some(SubmitMode::Only));
    }
    Ok(None)
}

/// Everything a submit sends, collected without touching Conduit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitInput {
    /// Submit mode.
    pub mode: SubmitMode,
    /// Update message (`--message`), update mode only.
    pub message: Option<String>,
    /// Revision fields (create mode) or `{}` (update mode).
    pub fields: serde_json::Value,
    /// Full `differential.creatediff` params.
    pub spec: serde_json::Value,
    /// `differential.setdiffproperty` writes after the diff exists.
    pub properties: Vec<diff_submit::DiffProperty>,
    /// `Included changes:` preview block.
    pub included_changes: String,
    /// Whether `--json` output was requested.
    pub json: bool,
}

/// Convert a local change entry to a submit payload change.
///
/// Divergence: hunks are empty and away paths unpaired. Faithful hunks need
/// the unified-diff parser (`generateChanges()`,
/// `src/workflow/ArcanistDiffWorkflow.php:802-1026`), which is not ported.
fn submit_change(entry: &diff_local::ChangeEntry) -> diff_submit::DiffChange {
    let old_path = match entry.change_type {
        diff_local::TYPE_MOVE_HERE | diff_local::TYPE_COPY_HERE => {
            entry.old_path.clone().unwrap_or_default()
        }
        _ => entry.current.path.clone(),
    };
    let mut change = diff_submit::DiffChange::new_text(&old_path, &entry.current.path);
    change.change_type = i64::from(entry.change_type);
    change.file_type = i64::from(entry.file_type);
    change
}

/// Require a `--message` update message.
///
/// PHP prompts interactively when `--message` is absent
/// (`src/workflow/ArcanistDiffWorkflow.php:701-716`); this port has no
/// interactive editor, so the flag is required.
fn update_message(message: Option<&str>) -> Result<String, String> {
    match message {
        None => Err(
            "Updating a revision requires --message (interactive update prompts are not supported)."
                .to_string(),
        ),
        Some(text) if text.trim().is_empty() => Err("Update message must not be empty.".to_string()),
        Some(text) => Ok(text.to_string()),
    }
}

/// Collect the diff properties to store: local commits plus onto targets.
///
/// Lint/unit properties are skipped: no engines ran, so there are no results
/// (`should_update_lint_property` / `should_update_unit_property`,
/// `src/workflow/ArcanistDiffWorkflow.php:2296-2320`).
fn diff_properties(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffProperty>, String> {
    let mut properties = Vec::new();
    let infos = diff_local::get_local_commit_information(cwd, base, head.unwrap_or("HEAD"))?;
    if diff_local::should_update_local_property(false, !infos.is_empty()) {
        let commits: Vec<serde_json::Value> = infos
            .iter()
            .map(diff_local::LocalCommitInfo::to_json)
            .collect();
        let data = serde_json::to_string(&commits)
            .map_err(|e| format!("failed to encode local commits: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_LOCAL_COMMITS.to_string(),
            data,
        });
    }
    let upstream = diff_local::resolve_upstream_branch(cwd);
    let targets = diff_local::diff_onto_targets(upstream.as_deref(), None);
    if diff_local::should_update_onto_property(!targets.is_empty()) {
        let rendered: Vec<serde_json::Value> = targets
            .iter()
            .map(diff_local::OntoTarget::to_json)
            .collect();
        let data = serde_json::to_string(&rendered)
            .map_err(|e| format!("failed to encode onto targets: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_ONTO.to_string(),
            data,
        });
    }
    Ok(properties)
}

/// Collect everything a submit sends, without touching Conduit.
pub fn prepare_submit(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
) -> Result<SubmitInput, String> {
    let range = resolve_range(cwd, options)?;
    let head = range.head.as_deref();
    let entries = diff_local::collect_change_entries(cwd, &range.base, head)?;
    diff_submit::require_changes(entries.len())?;
    let changes: Vec<diff_submit::DiffChange> = entries.iter().map(submit_change).collect();
    let local = diff_local::load_diff_specification(
        cwd,
        &range.base,
        &diff_local::default_source_machine_name(),
        None,
        false,
    )?;
    let spec_input = diff_submit::DiffSpecInput {
        source_machine: local.source_machine,
        source_path: local.source_path,
        branch: local.branch,
        bookmark: local.bookmark,
        source_control_system: local.source_control_system,
        source_control_path: local.source_control_path,
        source_control_base_revision: local.source_control_base_revision,
        repository_phid: None,
    };
    let spec = diff_submit::diff_spec_json(
        &changes,
        diff_local::lint_status(None),
        diff_local::unit_status(None),
        &spec_input,
    );
    let fields = match mode {
        SubmitMode::Create => fields_json(&collect_fields(cwd, options, &range.base, head)?),
        // Without `--verbatim` PHP sends no local fields on update (`:648-652`).
        SubmitMode::Update(_) => serde_json::json!({}),
        // Diff-only mode builds no commit message (`:1292-1295`).
        SubmitMode::Only => serde_json::Value::Null,
    };
    let message = match mode {
        SubmitMode::Update(_) => Some(update_message(options.message.as_deref())?),
        _ => None,
    };
    Ok(SubmitInput {
        mode: mode.clone(),
        message,
        fields,
        spec,
        properties: diff_properties(cwd, &range.base, head)?,
        included_changes: diff_local::render_included_changes(&entries),
        json: options.json,
    })
}

/// Run a submit over `call`, returning the stdout text.
///
/// Call order mirrors `run()` (`src/workflow/ArcanistDiffWorkflow.php:382-410,
/// 463-465,525-527`): `differential.creatediff`, the
/// `differential.setdiffproperty` writes, then the revision call. Output
/// follows `:414-420` (diff-only), `:476,554-558` (update),
/// `:551-558` (create), plus `Included changes:` (`:565-568`).
///
/// Divergence: console markup (`**bold**`, `__underline__`) renders as plain
/// text, and diff-only `--json` reports `"human": ""` (this port buffers no
/// human output, `:421-429`).
pub fn submit_over(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
) -> Result<String, String> {
    let created = diff_submit::create_diff(call, &input.spec)?;
    diff_submit::set_diff_properties(call, created.diff_id, &input.properties)?;
    match &input.mode {
        SubmitMode::Only => {
            if input.json {
                let doc = serde_json::json!({
                    "diffURI": created.uri,
                    "diffID": created.diff_id,
                    "human": "",
                });
                let mut text = serde_json::to_string(&doc)
                    .map_err(|e| format!("failed to encode JSON: {e}"))?;
                text.push('\n');
                return Ok(text);
            }
            Ok(format!(
                "Created a new Differential diff:\n        Diff URI: {}\n\n{}",
                created.uri, input.included_changes
            ))
        }
        SubmitMode::Update(id) => {
            let revision_id =
                i64::try_from(*id).map_err(|_| format!("Revision ID {id} is out of range."))?;
            let params = diff_submit::update_revision_params(
                revision_id,
                input.fields.clone(),
                created.diff_id,
                input.message.as_deref().unwrap_or(""),
            );
            let updated = diff_submit::update_revision(call, &params)?;
            Ok(format!(
                "Updated an existing Differential revision:\n        Revision URI: {}\n\n{}",
                updated.uri, input.included_changes
            ))
        }
        SubmitMode::Create => {
            // No server transactions are available locally, so the legacy
            // `differential.createrevision` path applies (`:478-531`).
            let params = diff_submit::create_revision_params(input.fields.clone(), created.diff_id);
            let revision = diff_submit::create_revision_legacy(call, &params)?;
            Ok(format!(
                "Created a new Differential revision:\n        Revision URI: {}\n\n{}",
                revision.uri, input.included_changes
            ))
        }
    }
}

/// Collect a submit locally, then run it over a real Conduit client.
fn submit_via_client(cwd: &Path, options: &Options, mode: &SubmitMode) -> Result<String, String> {
    let input = prepare_submit(cwd, options, mode)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    submit_over(&call, &input)
}

/// Inspect `cwd` per `options`, returning the bytes for stdout.
///
/// Without a submit mode this is the read-only inspection: `--json`
/// gains a `fields` object only when field flags are given, so default
/// output is byte-identical to before. With `--create`, `--update`, or
/// `--only`, the diff is submitted over Conduit instead.
pub fn inspect(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    let resolved = prepare(cwd, options)?;
    let options = &resolved;
    if !is_inside_git_repo(cwd) {
        return Err("not inside a git repository".to_string());
    }
    if let Some(mode) = submit_mode(options)? {
        return submit_via_client(cwd, options, &mode).map(String::into_bytes);
    }
    let range = resolve_range(cwd, options)?;
    if options.json {
        let files = numstat_range(cwd, &range.base, range.head.as_deref())?;
        let mut doc = diff_json(&range.base, &files);
        if options.message_file.is_some() || options.reviewers.is_some() || options.cc.is_some() {
            let fields = collect_fields(cwd, options, &range.base, range.head.as_deref())?;
            doc["fields"] = fields_json(&fields);
        }
        let mut text = serde_json::to_string_pretty(&doc)
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        Ok(text.into_bytes())
    } else {
        default_diff_output_range(cwd, &range.base, range.head.as_deref())
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
