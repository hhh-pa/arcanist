//! `arc diff`: local change inspection, plus opt-in Differential submit.
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
//!   stdin/command sources, `--edit`, `--verbatim`, `--draft`, and commit
//!   amend are still pending, as are hg/svn working copies and
//!   `.arcconfig` custom arguments. Lint/unit children run in-process;
//!   unresolved message dictionaries stay empty (see
//!   [`diff_local::run_lint`]).

extern crate self as arc_rs;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{
    amend, close_revision, conduit, config, diff_local, diff_submit, get_config, git, hg, list,
    svn, terminal,
};

/// Subcommand name, matching `arc diff`.
pub const NAME: &str = "diff";

const USAGE: &str = "usage: arc diff [--message MSG] [--message-file FILE] [--edit] [--raw] [--raw-command CMD] [--create | --update ID] [--draft] [--nounit] [--nolint] [--only] [--allow-untracked] [--apply-patches | --never-apply-patches] [--amend-all] [--amend-autofixes] [-a] [--json] [--no-amend] [--uncommitted] [--verbatim] [--reviewers NAMES] [--cc NAMES] [--skip-binaries] [--skip-staging] [--base REV] [--coverage | --no-coverage] [--browse] [--head REV] [--conduit-uri URI] [commit]\n";

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

/// Local commit messages oldest-first for field/corpus collection.
fn local_commit_messages_oldest_first(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<String>, String> {
    let infos = diff_local::get_local_commit_information(cwd, base, head.unwrap_or("HEAD"))?;
    Ok(infos
        .iter()
        .rev()
        .map(|info| info.message.clone())
        .collect())
}

/// Join oldest-first commit messages plus the `--reviewers`/`--cc` faux
/// message into the create corpus resolved server-side
/// (`parseCommitMessagesIntoFields()` faux handling, `:1786-1811`). The
/// faux message rides last; its `<placeholder>` first line is stripped
/// when real messages precede it (the server takes the first line as the
/// title, and PHP's merge skips placeholder titles). `None` when there is
/// nothing to resolve (no messages, no flags).
#[must_use]
pub fn join_create_corpus(
    oldest_first: &[String],
    reviewers: Option<&str>,
    cc: Option<&str>,
) -> Option<String> {
    let faux = diff_local::faux_flag_message(reviewers, cc);
    if oldest_first.is_empty() {
        return faux;
    }
    let mut parts: Vec<String> = oldest_first.to_vec();
    if let Some(faux) = faux {
        parts.push(strip_placeholder_line(&faux));
    }
    let corpus = parts.join("\n\n");
    if corpus.trim().is_empty() {
        return None;
    }
    Some(corpus)
}

/// Drop a leading `<placeholder>` title line (T12069 filler), keeping the
/// tag lines the server must parse.
fn strip_placeholder_line(faux: &str) -> String {
    let mut lines = faux.splitn(2, '\n');
    let first = lines.next().unwrap_or("");
    let rest = lines.next().unwrap_or("");
    if first.trim() == diff_local::PLACEHOLDER_TITLE {
        return rest.trim_start_matches('\n').to_string();
    }
    faux.to_string()
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
    let oldest_first = local_commit_messages_oldest_first(cwd, base, head)?;
    Ok(merge_message_fields(
        &oldest_first,
        options.reviewers.as_deref(),
        options.cc.as_deref(),
    ))
}

/// Collect the create-mode message corpus for `options`: `--message-file`
/// content wins alone, else the range's commit messages plus flags.
fn collect_create_corpus(
    cwd: &Path,
    options: &Options,
    base: &str,
    head: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(path) = options.message_file.as_deref() {
        let message = diff_local::commit_message_from_file(Path::new(path))?;
        return Ok(Some(message.raw_corpus));
    }
    let oldest_first = local_commit_messages_oldest_first(cwd, base, head)?;
    Ok(join_create_corpus(
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
    /// Whether `--draft` was passed (create mode decides against server
    /// transactions support).
    pub draft: bool,
    /// Create-mode message corpus for server-side field resolution
    /// (`parsecommitmessage`); `None` for update/only modes and for
    /// sourceless (raw/svn) creates, which keep local fields.
    pub message_corpus: Option<String>,
    /// Revision fields (create mode) or `{}` (update mode).
    pub fields: serde_json::Value,
    /// Full `differential.creatediff` params.
    pub spec: serde_json::Value,
    /// `differential.setdiffproperty` writes after the diff exists.
    pub properties: Vec<diff_submit::DiffProperty>,
    /// Lint/unit report: feeds the `arc:lint`/`arc:unit` properties and
    /// the Harbormaster autotarget upload (DIFF-D11).
    pub lint_unit: diff_local::LintUnitReport,
    /// `Included changes:` preview block.
    pub included_changes: String,
    /// Whether `--json` output was requested.
    pub json: bool,
}

/// Convert a local change entry to a submit payload change.
///
/// Hunks, modes, and companions attach in [`submit_changes_with_hunks`]
/// from the parsed range text (T1153 EXPORT-E11/E13/E14).
fn submit_change(entry: &diff_local::ChangeEntry) -> diff_submit::DiffChange {
    // T1153 OLD-PATH: ADD reports `/dev/null` like the parser
    // (`ArcanistDiffParser.php:801-808`); moves/copies keep the old
    // path, deletes/changes keep the current path.
    let old_path = match entry.change_type {
        diff_local::TYPE_MOVE_HERE | diff_local::TYPE_COPY_HERE => {
            entry.old_path.clone().unwrap_or_default()
        }
        diff_local::TYPE_ADD => "/dev/null".to_string(),
        _ => entry.current.path.clone(),
    };
    let mut change = diff_submit::DiffChange::new_text(&old_path, &entry.current.path);
    change.change_type = i64::from(entry.change_type);
    change.file_type = i64::from(entry.file_type);
    change
}

/// Attach a parsed change's hunks and modes to a skeleton (T1153
/// EXPORT-E11/E13: `,length` bits ride alongside the hunks, filemodes
/// ride the properties).
fn attach_parsed_content(change: &mut diff_submit::DiffChange, found: &diff_local::ParsedChange) {
    change.old_properties = filemode_properties(found.old_mode.as_deref());
    change.new_properties = filemode_properties(found.new_mode.as_deref());
    change.hunks = found
        .hunks
        .iter()
        .zip(found.hunk_len_explicit.iter())
        .map(|(hunk, explicit)| submit_hunk_from_parsed(hunk, explicit))
        .collect();
}

/// A folded companion for one here entry (T1153 EXPORT-E14): content
/// (synthetic delete-side hunks for pure moves) attaches from the
/// parsed companion twin by old path.
fn entry_companion(
    companion: &diff_local::AwayCompanion,
    parsed: &[diff_local::ParsedChange],
) -> diff_submit::DiffChange {
    let mut out = diff_submit::DiffChange::new_companion(
        i64::from(companion.change_type),
        &companion.current_path,
        &companion.away_paths,
    );
    if let Some(twin) = parsed.iter().find(|parsed| {
        parsed.current_path == companion.current_path
            && diff_local::is_companion_type(parsed.change_type)
    }) {
        attach_parsed_content(&mut out, twin);
        out.file_type = i64::from(twin.file_type);
    }
    out
}

/// Fold one entry's companion, emitting or updating it in `changes`
/// (T1153 EXPORT-E14: shared fold, PHP's first-reference order).
fn fold_entry_companion(
    fold: &mut diff_local::CompanionFold,
    slots: &mut Vec<usize>,
    changes: &mut Vec<diff_submit::DiffChange>,
    entry: &diff_local::ChangeEntry,
    parsed: &[diff_local::ParsedChange],
) {
    let Some(old) = entry.old_path.as_deref() else {
        return;
    };
    if !matches!(
        entry.change_type,
        diff_local::TYPE_MOVE_HERE | diff_local::TYPE_COPY_HERE
    ) || old.is_empty()
        || old == entry.current.path
    {
        return;
    }
    let (slot, is_new) = fold.fold(entry.change_type, old, &entry.current.path);
    if is_new {
        slots.push(changes.len());
        changes.push(entry_companion(fold.companion(slot), parsed));
    } else if let Some(&at) = slots.get(slot) {
        changes[at] = entry_companion(fold.companion(slot), parsed);
    }
}

/// Build submit changes for the git range: entry skeletons plus hunks
/// parsed from the range diff text (`generateChanges()`, `:802-827`).
/// Entries keep their type/path mapping; hunks attach by current path.
/// A range whose text does not parse fails like PHP's `didFailParse()`.
///
/// The range diff runs with the pinned `getFullGitDiff()` invocation
/// (T1153 EXPORT-E36: `ArcanistGitAPI.php:464-491`), so renames parse
/// as renames and hunks carry whole-file context like PHP's. Pure
/// moves gain fabricated hunks (T1153 EXPORT-E20), and renames/copies
/// gain folded away companions (T1153 EXPORT-E14).
fn submit_changes_with_hunks(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
    entries: &[diff_local::ChangeEntry],
) -> Result<Vec<diff_submit::DiffChange>, String> {
    let options = diff_local::full_diff_options(diff_local::DEFAULT_DIFF_CONTEXT_LINES);
    let mut args: Vec<String> = diff_local::DIFF_CONFIG_OPTIONS
        .iter()
        .map(|option| (*option).to_string())
        .collect();
    args.push("diff".to_string());
    args.extend(options);
    args.push(base.to_string());
    if let Some(rev) = head {
        args.push(rev.to_string());
    }
    args.push("--".to_string());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = git_output(cwd, &arg_refs)?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = crate::patch::decode_bytes(&out.stdout, None)?;
    let mut full = diff_local::with_companions(&diff_local::parse_unified_diff(&text)?);
    let raw_for = |path: &str| diff_local::get_raw_diff_text(cwd, base, path);
    diff_local::attach_synthetic_git_hunks(&mut full, &raw_for)?;
    let mut fold = diff_local::CompanionFold::new();
    let mut slots: Vec<usize> = Vec::new();
    let mut changes = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut change = submit_change(entry);
        // Prefer the here-change twin: a companion for a rename chain
        // (`R a->b`, `R b->c`) shares the middle path (T1153 EXPORT-E14).
        let found = full
            .iter()
            .find(|parsed| {
                parsed.current_path == entry.current.path
                    && !diff_local::is_companion_type(parsed.change_type)
            })
            .or_else(|| {
                full.iter()
                    .find(|parsed| parsed.current_path == entry.current.path)
            });
        if let Some(found) = found {
            attach_parsed_content(&mut change, found);
        }
        changes.push(change);
        fold_entry_companion(&mut fold, &mut slots, &mut changes, entry, &full);
    }
    Ok(changes)
}

/// `arc:lint`/`arc:unit` properties from a lint/unit report
/// (`updateLintDiffProperty()`/`updateUnitDiffProperty()`, `:2286-2311`).
/// Built ungated; `submit_over_with_uri_ansi` drops them when the
/// autotarget upload hits (DIFF-D11).
fn lint_unit_properties(
    report: &diff_local::LintUnitReport,
) -> Result<Vec<diff_submit::DiffProperty>, String> {
    let mut properties = Vec::new();
    if diff_local::should_update_lint_property(false, !report.unresolved_lint.is_empty()) {
        let data =
            diff_submit::php_json_encode(&serde_json::Value::Array(report.unresolved_lint.clone()))
                .map_err(|e| format!("failed to encode lint messages: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_LINT.to_string(),
            data,
        });
    }
    if diff_local::should_update_unit_property(false, !report.test_results.is_empty()) {
        let data =
            diff_submit::php_json_encode(&serde_json::Value::Array(report.test_results.clone()))
                .map_err(|e| format!("failed to encode unit results: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_UNIT.to_string(),
            data,
        });
    }
    Ok(properties)
}

/// Collect the diff properties to store: local commits plus onto targets.
///
/// Lint/unit properties come from [`lint_unit_properties`] instead (they
/// need the report, and the autotarget gate applies at submit time).
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
        let data = diff_submit::php_json_encode(&serde_json::Value::Array(commits))
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
        let data = diff_submit::php_json_encode(&serde_json::Value::Array(rendered))
            .map_err(|e| format!("failed to encode onto targets: {e}"))?;
        properties.push(diff_submit::DiffProperty {
            name: diff_submit::PROP_ONTO.to_string(),
            data,
        });
    }
    Ok(properties)
}

/// User-set passthrough flags for the lint/unit children, in spec order
/// (see [`diff_local::passthru_argv`]).
fn set_child_flags(options: &Options) -> (Vec<String>, Vec<String>) {
    let is_set = |name: &str| match name {
        "apply-patches" => options.apply_patches,
        "never-apply-patches" => options.never_apply_patches,
        "amend-all" => options.amend_all,
        "amend-autofixes" => options.amend_autofixes,
        "coverage" => options.coverage,
        "no-coverage" => options.no_coverage,
        _ => false,
    };
    let lint = diff_local::LINT_PASSTHRU_FLAGS
        .iter()
        .filter(|flag| is_set(flag))
        .map(|flag| flag.to_string())
        .collect();
    let unit = diff_local::UNIT_PASSTHRU_FLAGS
        .iter()
        .filter(|flag| is_set(flag))
        .map(|flag| flag.to_string())
        .collect();
    (lint, unit)
}

/// Run lint and unit over the process stdio (`runLintUnit()`, `:1131-1140`).
///
/// `base` carries `--rev` (commit-range repositories only); `head` and
/// `is_raw` drive the skip checks. Callers resolve the base first so the
/// canonical "not inside a working copy" errors surface before any child
/// runs, like PHP's repository setup preceding `:363`.
///
/// Notices print to stdout even under `--json`: PHP captures them into
/// the `human` field, which in-process runs cannot do (same `human`
/// divergence as the submit report).
/// Default `history.immutable` for `cwd`'s VCS (T1153 DIFF-D22):
/// `isHistoryDefaultImmutable()` when no config value is set.
fn history_default_immutable(cwd: &Path) -> bool {
    match detect_vcs_kind(cwd) {
        Some(VcsKind::Git) => git::is_history_default_immutable(),
        Some(VcsKind::Hg) => hg::is_history_default_immutable(),
        Some(VcsKind::Svn) => svn::is_history_default_immutable(),
        // Unreachable for non-raw runs (base resolution fails outside a
        // working copy first); raw runs never amend.
        None => false,
    }
}

fn run_lint_unit_stdio_ansi(
    cwd: &Path,
    options: &Options,
    base: Option<&str>,
    head: Option<&str>,
    is_raw: bool,
    ansi: bool,
) -> Result<diff_local::LintUnitReport, String> {
    let (lint_flags, unit_flags) = set_child_flags(options);
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut out = std::io::stdout();
    // T1153 DIFF-D22: the lint child amends when `shouldAmend()`
    // (bare `setShouldAmendChanges()`), like PHP's `runLint()`.
    let amend_lint = diff_local::should_amend(
        is_raw,
        options.no_amend,
        head,
        amend::history_is_immutable(cwd, history_default_immutable(cwd))?,
    );
    let run_lint_child = |dir: &Path, argv: &[String]| {
        diff_local::run_lint_in_process_with_amend(dir, argv, amend_lint)
    };
    diff_local::run_lint_unit_ansi(
        cwd,
        base,
        &lint_flags,
        &unit_flags,
        diff_local::should_skip_lint(options.nolint, is_raw, head),
        diff_local::should_skip_unit(options.nounit, is_raw, head),
        &run_lint_child,
        &diff_local::run_unit_in_process,
        &mut input,
        &mut out,
        ansi,
    )
}

/// Collect everything a submit sends, without touching Conduit.
pub fn prepare_submit(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
) -> Result<SubmitInput, String> {
    prepare_submit_with_report(cwd, options, mode, &diff_local::LintUnitReport::empty())
}

/// Mode-dependent submit payload: local fields (create), the update
/// message (update), and the create corpus for server-side field
/// resolution (create).
struct ModePayload {
    fields: serde_json::Value,
    message: Option<String>,
    message_corpus: Option<String>,
}

/// Collect the mode-dependent submit payload.
fn collect_mode_payload(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
    base: &str,
    head: Option<&str>,
) -> Result<ModePayload, String> {
    let fields = match mode {
        SubmitMode::Create => fields_json(&collect_fields(cwd, options, base, head)?),
        // Without `--verbatim` PHP sends no local fields on update (`:648-652`).
        SubmitMode::Update(_) => serde_json::json!({}),
        // Diff-only mode builds no commit message (`:1292-1295`).
        SubmitMode::Only => serde_json::Value::Null,
    };
    // DIFF-D9: the raw `--message` rides through; absent/empty messages
    // resolve via the editor online (`resolve_update_revision`).
    let message = match mode {
        SubmitMode::Update(_) => options.message.clone(),
        _ => None,
    };
    let message_corpus = match mode {
        SubmitMode::Create => collect_create_corpus(cwd, options, base, head)?,
        _ => None,
    };
    Ok(ModePayload {
        fields,
        message,
        message_corpus,
    })
}

/// Collect a submit, stamping the diff spec with `report`'s lint/unit
/// status words (`run()`, `:375-379`).
pub fn prepare_submit_with_report(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
    report: &diff_local::LintUnitReport,
) -> Result<SubmitInput, String> {
    let range = resolve_range(cwd, options)?;
    let head = range.head.as_deref();
    let entries = diff_local::collect_change_entries(cwd, &range.base, head)?;
    diff_submit::require_changes(entries.len())?;
    let changes = submit_changes_with_hunks(cwd, &range.base, head, &entries)?;
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
        diff_local::lint_status(report.lint_result),
        diff_local::unit_status(report.unit_result),
        &spec_input,
    );
    let payload = collect_mode_payload(cwd, options, mode, &range.base, head)?;
    Ok(SubmitInput {
        mode: mode.clone(),
        message: payload.message,
        draft: options.draft,
        message_corpus: payload.message_corpus,
        fields: payload.fields,
        spec,
        properties: {
            let mut properties = diff_properties(cwd, &range.base, head)?;
            properties.extend(lint_unit_properties(report)?);
            properties
        },
        lint_unit: report.clone(),
        included_changes: diff_local::render_included_changes(&entries),
        json: options.json,
    })
}

/// Run a submit over `call`, returning the stdout text.
///
/// Submit report text plus the created object URI for `--browse`
/// (`:431-433`, `:560-562`).
pub struct SubmitReport {
    /// Human-readable report (also the `--json` document when requested).
    pub text: String,
    /// Created diff/revision URI, when the mode yields one.
    pub uri: Option<String>,
    /// Revision id for create/update modes (DIFF-D7 post-create amend).
    pub revision_id: Option<i64>,
    /// Created diff id (DIFF-D13 staging runs after the submit).
    pub diff_id: i64,
}

/// Format one submit report: plain lead, bold label, underlined URI.
///
/// PHP prints the lead with plain `%s` and the URI line as
/// `"**%s** __%s__"` (`:414-420`, `:554-558`); `ansi=false` strips to
/// the plain shape, byte-identical to the historical `format!` output.
fn format_submit_report_ansi(
    lead: &str,
    label: &str,
    uri: &str,
    included: &str,
    ansi: bool,
) -> String {
    terminal::tsprintf_ansi(
        "%R\n        **%R** __%R__\n\n%R",
        &[
            terminal::TVal::Text(lead),
            terminal::TVal::Text(label),
            terminal::TVal::Text(uri),
            terminal::TVal::Text(included),
        ],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_else(|_| format!("{lead}\n        {label} {uri}\n\n{included}"))
}

/// Call order mirrors `run()` (`src/workflow/ArcanistDiffWorkflow.php:382-410,
/// 463-465,525-527`): `differential.creatediff`, the
/// `differential.setdiffproperty` writes, then the revision call. Output
/// follows `:414-420` (diff-only), `:476,554-558` (update),
/// `:551-558` (create), plus `Included changes:` (`:565-568`).
///
/// Divergence: diff-only `--json` reports `"human": ""` (this port buffers
/// no human output, `:421-429`).
pub fn submit_over_with_uri(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
) -> Result<SubmitReport, String> {
    submit_over_with_uri_ansi(call, input, terminal::ansi_enabled())
}

/// [`submit_over_with_uri`] with an explicit ANSI setting.
pub fn submit_over_with_uri_ansi(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
    ansi: bool,
) -> Result<SubmitReport, String> {
    let created = diff_submit::create_diff(call, &input.spec)?;
    let hit_autotargets =
        upload_submit_autotargets(call, created.phid.as_deref(), &input.lint_unit);
    let properties = submit_diff_properties(&input.properties, hit_autotargets);
    diff_submit::set_diff_properties(call, created.diff_id, &properties)?;
    match &input.mode {
        SubmitMode::Only => report_only_submit(&created, input, ansi),
        SubmitMode::Update(id) => report_update_submit(call, *id, created.diff_id, input, ansi),
        SubmitMode::Create => report_create_submit(call, &created, input, ansi),
    }
}

/// DIFF-D11: autotargets upload first (`:399-406`); a hit gates the
/// legacy `arc:lint`/`arc:unit` properties off (`:2286-2311`). The
/// upload runs only when `creatediff` returned a diff PHID.
fn upload_submit_autotargets(
    call: &diff_submit::ConduitCall<'_>,
    diff_phid: Option<&str>,
    lint_unit: &diff_local::LintUnitReport,
) -> bool {
    diff_phid.is_some_and(|phid| {
        // No unit result fails the unit target, like PHP's `default` arm.
        let unit_result = i64::from(
            lint_unit
                .unit_result
                .unwrap_or(diff_local::UNIT_RESULT_FAIL),
        );
        diff_submit::update_autotargets(
            call,
            phid,
            &lint_unit.unresolved_lint,
            &lint_unit.test_results,
            unit_result,
        )
    })
}

/// Diff properties to set: an autotarget hit drops the legacy
/// `arc:lint`/`arc:unit` properties.
fn submit_diff_properties(
    properties: &[diff_submit::DiffProperty],
    hit_autotargets: bool,
) -> Vec<diff_submit::DiffProperty> {
    properties
        .iter()
        .filter(|property| {
            !hit_autotargets
                || (property.name != diff_submit::PROP_LINT
                    && property.name != diff_submit::PROP_UNIT)
        })
        .cloned()
        .collect()
}

/// `--only` report: the JSON document under `--json`, else the
/// human-readable diff report.
fn report_only_submit(
    created: &diff_submit::CreatedDiff,
    input: &SubmitInput,
    ansi: bool,
) -> Result<SubmitReport, String> {
    let created_diff_id = created.diff_id;
    if input.json {
        let uri = created.uri.clone();
        let doc = serde_json::json!({
            "diffURI": created.uri,
            "diffID": created.diff_id,
            "human": "",
        });
        let mut text = diff_submit::php_json_encode(&doc)
            .map_err(|e| format!("failed to encode JSON: {e}"))?;
        text.push('\n');
        return Ok(SubmitReport {
            text,
            uri: Some(uri),
            revision_id: None,
            diff_id: created_diff_id,
        });
    }
    Ok(SubmitReport {
        text: format_submit_report_ansi(
            "Created a new Differential diff:",
            "Diff URI:",
            &created.uri,
            &input.included_changes,
            ansi,
        ),
        uri: Some(created.uri.clone()),
        revision_id: None,
        diff_id: created_diff_id,
    })
}

/// `--update` report: update the revision, then format the report.
fn report_update_submit(
    call: &diff_submit::ConduitCall<'_>,
    update_id: u64,
    created_diff_id: i64,
    input: &SubmitInput,
    ansi: bool,
) -> Result<SubmitReport, String> {
    let revision_id = i64::try_from(update_id)
        .map_err(|_| format!("Revision ID {update_id} is out of range."))?;
    let params = diff_submit::update_revision_params(
        revision_id,
        input.fields.clone(),
        created_diff_id,
        input.message.as_deref().unwrap_or(""),
    );
    let updated = diff_submit::update_revision(call, &params)?;
    // DIFF-D17: under `--json` PHP discards the whole report for
    // create/update (`ob_get_clean()`, `:570-572`); the URI is
    // still returned for `--browse`.
    let text = if input.json {
        String::new()
    } else {
        format_submit_report_ansi(
            "Updated an existing Differential revision:",
            "Revision URI:",
            &updated.uri,
            &input.included_changes,
            ansi,
        )
    };
    Ok(SubmitReport {
        text,
        uri: Some(updated.uri),
        revision_id: Some(revision_id),
        diff_id: created_diff_id,
    })
}

/// `--create` report: create the revision, then format the report.
fn report_create_submit(
    call: &diff_submit::ConduitCall<'_>,
    created: &diff_submit::CreatedDiff,
    input: &SubmitInput,
    ansi: bool,
) -> Result<SubmitReport, String> {
    let revision = submit_create_revision(
        call,
        input,
        created.diff_id,
        created.phid.as_deref(),
        &created.uri,
    )?;
    // DIFF-D17: see the update branch above (`:570-572`).
    let text = if input.json {
        String::new()
    } else {
        format_submit_report_ansi(
            "Created a new Differential revision:",
            "Revision URI:",
            &revision.uri,
            &input.included_changes,
            ansi,
        )
    };
    let created_id = revision.revision_id;
    Ok(SubmitReport {
        text,
        uri: Some(revision.uri),
        revision_id: Some(created_id),
        diff_id: created.diff_id,
    })
}

/// Run a submit over `call`, returning the report text (see
/// [`submit_over_with_uri`]).
pub fn submit_over(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
) -> Result<String, String> {
    submit_over_ansi(call, input, terminal::ansi_enabled())
}

/// [`submit_over`] with an explicit ANSI setting.
pub fn submit_over_ansi(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
    ansi: bool,
) -> Result<String, String> {
    submit_over_with_uri_ansi(call, input, ansi).map(|report| report.text)
}

/// Stamp `repositoryPHID` into a submit spec for non-raw submits.
///
/// DIFF-D14: PHP sends `repositoryPHID` when the working copy resolves
/// to a unique repository (`buildDiffSpecification()`,
/// `src/workflow/ArcanistDiffWorkflow.php:2275-2280`, via
/// `getRepositoryPHID()`); raw sources send none.
fn stamp_repository_phid(
    client: &conduit::Client,
    cwd: &Path,
    is_raw: bool,
    spec: &mut serde_json::Value,
) -> Result<(), String> {
    if is_raw {
        return Ok(());
    }
    if let Some(phid) = close_revision::resolve_repository_phid(client, cwd)? {
        spec["repositoryPHID"] = serde_json::Value::String(phid);
    }
    Ok(())
}

/// Collect a submit locally, then run it over a real Conduit client.
fn submit_via_client_ansi(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
    ansi: bool,
) -> Result<String, String> {
    let range = resolve_range(cwd, options)?;
    let head = range.head.as_deref();
    let is_raw = diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref());
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    // DIFF-D8: explicit creates resolve their message before lint (PHP's
    // `getCommitMessageFromUser()` precedes the linters); a footer
    // revision redirects the mode to that update instead.
    let CreateRedirect {
        mode,
        corpus: create_corpus,
        footer: footer_redirect,
    } = resolve_submit_create_redirect(cwd, options, &call, mode, &range.base, head)?;
    let report =
        run_lint_unit_stdio_ansi(cwd, options, Some(range.base.as_str()), head, is_raw, ansi)?;
    let mut input = prepare_submit_with_report(cwd, options, &mode, &report)?;
    if let Some(corpus) = create_corpus {
        input.message_corpus = Some(corpus);
    }
    let skip_update_resolution =
        apply_footer_redirect(cwd, options, &call, &mut input, &footer_redirect)?;
    submit_prepared_via_client_ansi(
        cwd,
        &hg::HgCommand::default(),
        options,
        &client,
        &mut input,
        Some(VcsKind::Git),
        is_raw,
        skip_update_resolution,
        ansi,
    )
}

/// DIFF-D8 create redirect: the effective mode plus the create
/// corpus or footer revision to apply after lint.
struct CreateRedirect {
    /// Effective submit mode (a footer revision redirects to update).
    mode: SubmitMode,
    /// Raw corpus for the create submit, absent on redirect.
    corpus: Option<String>,
    /// Footer revision to update instead of creating.
    footer: Option<FooterRedirect>,
}

/// A footer revision redirect: the id plus the pulled message fields.
struct FooterRedirect {
    /// Footer revision id.
    revision_id: u64,
    /// Server-pulled message fields.
    fields: diff_local::CommitFields,
}

/// Resolve the DIFF-D8 create redirect.
fn resolve_submit_create_redirect(
    cwd: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    mode: &SubmitMode,
    base: &str,
    head: Option<&str>,
) -> Result<CreateRedirect, String> {
    if !matches!(mode, SubmitMode::Create) {
        return Ok(CreateRedirect {
            mode: mode.clone(),
            corpus: None,
            footer: None,
        });
    }
    let metadata_dir = update_metadata_dir(cwd, Some(VcsKind::Git))?;
    let branch = git::branch_name(cwd)?;
    // A detached HEAD renders an empty branch name (PHP formats a
    // null branch through `%s`); raw sources never reach this arm.
    let branch = branch.as_deref().or(Some(""));
    let resolved = resolve_create_message_online(
        cwd,
        &hg::HgCommand::default(),
        options,
        call,
        Some(VcsKind::Git),
        &metadata_dir,
        base,
        head,
        branch,
    )?;
    if let Some(footer_id) = resolved.footer_revision_id {
        Ok(CreateRedirect {
            mode: SubmitMode::Update(footer_id),
            corpus: None,
            footer: Some(FooterRedirect {
                revision_id: footer_id,
                fields: resolved.fields,
            }),
        })
    } else {
        Ok(CreateRedirect {
            mode: mode.clone(),
            corpus: Some(resolved.corpus),
            footer: None,
        })
    }
}

/// Apply a footer redirect to a prepared input, returning whether the
/// update was pre-resolved (skipping the standard resolution).
fn apply_footer_redirect(
    cwd: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    input: &mut SubmitInput,
    footer_redirect: &Option<FooterRedirect>,
) -> Result<bool, String> {
    let Some(redirect) = footer_redirect else {
        return Ok(false);
    };
    let metadata_dir = update_metadata_dir(cwd, Some(VcsKind::Git))?;
    let resolved = resolve_footer_update_online(
        cwd,
        &hg::HgCommand::default(),
        options,
        call,
        redirect.revision_id,
        &redirect.fields,
        Some(VcsKind::Git),
        &metadata_dir,
    )?;
    input.fields = resolved.fields;
    input.message = Some(resolved.message);
    Ok(true)
}

/// Submit a prepared input: stamp the repository PHID, resolve a
/// `--update` target (unless the caller pre-resolved a footer
/// redirect), submit, push staging, amend after creates, and clear the
/// update scratch files.
#[allow(clippy::too_many_arguments)]
fn submit_prepared_via_client_ansi(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    client: &crate::conduit::Client,
    input: &mut SubmitInput,
    kind: Option<VcsKind>,
    is_raw: bool,
    skip_update_resolution: bool,
    ansi: bool,
) -> Result<String, String> {
    stamp_repository_phid(client, cwd, is_raw, &mut input.spec)?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    // DIFF-D6/D9/D10: updates resolve fields and the message against
    // Conduit (plus the editor) before the diff exists, like PHP's
    // `buildRevisionFromCommitMessage()` preceding `creatediff`.
    let update_id = match &input.mode {
        SubmitMode::Update(id) => Some(*id),
        _ => None,
    };
    if let Some(update_id) = update_id
        && !skip_update_resolution
    {
        apply_client_update_resolution(cwd, hg_cmd, options, &call, kind, &mut *input, update_id)?;
    }
    let report = submit_over_and_stage(cwd, options, &call, kind, input, ansi)?;
    finalize_client_submit(
        cwd,
        hg_cmd,
        options,
        &call,
        kind,
        &input.mode,
        report.revision_id,
        update_id,
    )?;
    Ok(report.text)
}

/// Resolve an `--update` target's fields and message against Conduit
/// (plus the editor) before the diff exists.
fn apply_client_update_resolution(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    kind: Option<VcsKind>,
    input: &mut SubmitInput,
    update_id: u64,
) -> Result<(), String> {
    let metadata_dir = update_metadata_dir(cwd, kind)?;
    let mut display_arg = String::new();
    let target = standard_update_target(options, &input.fields, update_id, &mut display_arg);
    let resolved = resolve_update_online(cwd, hg_cmd, options, call, &target, kind, &metadata_dir)?;
    input.fields = resolved.fields;
    input.message = Some(resolved.message);
    Ok(())
}

/// Submit over `call`, then push the diff to the staging area.
fn submit_over_and_stage(
    cwd: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    kind: Option<VcsKind>,
    input: &SubmitInput,
    ansi: bool,
) -> Result<SubmitReport, String> {
    let report = submit_over_with_browse_ansi(cwd, options, call, input, ansi)?;
    // DIFF-D13: the staging push records before the post-create amend
    // (both run after the submit here; PHP stages pre-submit).
    let spec_base = input
        .spec
        .get("sourceControlBaseRevision")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    submit_changes_to_staging_area(cwd, options, call, kind, report.diff_id, spec_base, ansi)?;
    Ok(report)
}

/// DIFF-D7/D6: amend the worktree after a create, then forget the
/// update scratch files.
#[allow(clippy::too_many_arguments)]
fn finalize_client_submit(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    kind: Option<VcsKind>,
    mode: &SubmitMode,
    created_id: Option<i64>,
    update_id: Option<u64>,
) -> Result<(), String> {
    // DIFF-D7: a fresh revision's footer amends into the worktree commit.
    if matches!(mode, SubmitMode::Create)
        && let Some(created_id) = created_id
    {
        amend_after_create(cwd, hg_cmd, options, call, created_id, kind)?;
    }
    if let Some(update_id) = update_id
        && let Ok(metadata_dir) = update_metadata_dir(cwd, kind)
    {
        clear_update_scratch(&metadata_dir, update_id);
    }
    Ok(())
}

/// Inspect `cwd` per `options`, returning the bytes for stdout.
///
/// Bare `--json` is the read-only inspection: it gains a `fields`
/// object only when field flags are given, so default output is
/// byte-identical to before. With `--create`, `--update`, or `--only`
/// the diff is submitted over Conduit instead, as is a bare human
/// invocation (DIFF-D8: working-copy detection decides the mode).
/// True when raw sources or non-git working copies take the G1 pipeline.
fn uses_full_pipeline(cwd: &Path, options: &Options) -> bool {
    diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref())
        || matches!(detect_vcs(cwd), Some(vcs) if vcs != Vcs::Git)
}

/// True when field-affecting flags were given.
fn has_field_flags(options: &Options) -> bool {
    options.message_file.is_some() || options.reviewers.is_some() || options.cc.is_some()
}

/// Render the git read-only `--json` document for `range`.
fn inspect_json_git(cwd: &Path, options: &Options, range: &Range) -> Result<Vec<u8>, String> {
    let files = numstat_range(cwd, &range.base, range.head.as_deref())?;
    let mut doc = diff_json(&range.base, &files);
    if has_field_flags(options) {
        let fields = collect_fields(cwd, options, &range.base, range.head.as_deref())?;
        doc["fields"] = fields_json(&fields);
    }
    let mut text = match diff_submit::php_json_encode_pretty(&doc) {
        Ok(text) => text,
        Err(e) => return Err(format!("failed to encode JSON: {e}")),
    };
    text.push('\n');
    Ok(text.into_bytes())
}

pub fn inspect(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    inspect_ansi(cwd, options, terminal::ansi_enabled())
}

/// [`inspect`] with an explicit ANSI setting for submit reports.
pub fn inspect_ansi(cwd: &Path, options: &Options, ansi: bool) -> Result<Vec<u8>, String> {
    // Full-state routing (merge): raw sources and non-git working copies
    // take the parallel G1 pipeline; git and vcs-less shells keep the
    // locked baseline contract below.
    if uses_full_pipeline(cwd, options) {
        return inspect_full_ansi(cwd, options, ansi);
    }
    let resolved = prepare(cwd, options)?;
    let options = &resolved;
    if !is_inside_git_repo(cwd) {
        return Err("not inside a git repository".to_string());
    }
    if let Some(mode) = submit_mode(options)? {
        return submit_via_client_ansi(cwd, options, &mode, ansi).map(String::into_bytes);
    }
    let range = resolve_range(cwd, options)?;
    if options.json {
        return inspect_json_git(cwd, options, &range);
    }
    // DIFF-D8: a bare human invocation submits like PHP (working-copy
    // detection decides create vs update); only `--json` stays
    // read-only.
    submit_bare_git_ansi(cwd, options, &range, ansi).map(String::into_bytes)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{}", help_text());
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc diff: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let ansi = terminal::ansi_enabled();
    match inspect_ansi(&cwd, &options, ansi) {
        Ok(out) => {
            use std::io::Write;
            if std::io::stdout().write_all(&out).is_err() {
                eprintln!("arc diff: failed to write output");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("arc diff: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Full `--help` text: usage line plus the long help.
#[must_use]
pub fn help_text() -> String {
    format!("{USAGE}\n{FULL_HELP}")
}

/* -(  Full-State Prompts and Raw Sources (G1)  )--------------------------------- */

/// Full `arc diff` help text: supports line plus command help.
///
/// PHP: `getCommandHelp()` (`src/workflow/ArcanistDiffWorkflow.php:46-60`)
/// with console markup stripped. `run()` still prints [`USAGE`]; this is
/// the merge target for `--help` parity.
pub const FULL_HELP: &str = "Supports: git, svn, hg\nGenerate a Differential diff or revision from local changes.\n\nUnder git and mercurial, you can specify a commit (like HEAD^^^ or master) and Differential will generate a diff against the merge base of that commit and your current working directory parent.\n\nUnder svn, you can choose to include only some of the modified files in the working copy in the diff by specifying their paths. If you omit paths, all changes are included in the diff.\n";

/// Wrap console text to 78 columns with an `indent`-space prefix.
///
/// PHP: `phutil_console_wrap()` (`src/console/format.php:102-191`).
/// Delegates to [`terminal::console_wrap`] so all wrapping renders
/// through the shared ANSI-aware helper.
#[must_use]
pub fn console_wrap(text: &str, indent: usize, with_prefix: bool) -> String {
    terminal::console_wrap(text, indent, with_prefix)
}

/// Render a `phutil_console_confirm()` prompt: blank lines plus the wrapped
/// `{prompt} {options}` line.
///
/// PHP: `phutil_console_confirm()` / `phutil_console_prompt()`
/// (`src/console/format.php:11-25,42-52`). `default_no` selects `[y/N]`
/// (default) or `[Y/n]`.
#[must_use]
pub fn format_confirm_prompt(prompt: &str, default_no: bool) -> String {
    let options = if default_no { "[y/N]" } else { "[Y/n]" };
    format!(
        "\n\n{}",
        terminal::console_wrap(&format!("{prompt} {options} "), 4, true)
    )
}

/// Interpret one confirm answer: `y`/`n`/empty decide, anything else
/// reprompts (`None`).
///
/// PHP: the `do/while` in `phutil_console_confirm()`
/// (`src/console/format.php:14-24`): `$c = trim(strtolower($response))`.
/// `strtolower()` is ASCII-only and `trim()` strips `" \t\n\r\0\x0B"`
/// only, so Unicode whitespace (e.g. NBSP) is NOT trimmed and reprompts.
#[must_use]
pub fn interpret_confirm_answer(response: &str, default_no: bool) -> Option<bool> {
    let lowered = response.to_ascii_lowercase();
    let trimmed = lowered.trim_matches([' ', '\t', '\n', '\r', '\0', '\x0B']);
    match trimmed {
        "y" => Some(true),
        "n" => Some(false),
        "" => Some(!default_no),
        _ => None,
    }
}

/// Error when stdin is not a TTY.
///
/// PHP: `PhutilConsoleStdinNotInteractiveException`
/// (`src/console/PhutilConsoleStdinNotInteractiveException.php:11-16`).
pub const STDIN_NOT_TTY: &str = "The program is attempting to read user input, but stdin is being \
    piped from some other source (not a TTY).";

/// Run a confirm prompt over `reader`/`writer` until it resolves.
///
/// PHP: `phutil_console_confirm()` (`src/console/format.php:11-25`). EOF
/// answers with the default, like PHP's `fgets()` failure falling through
/// to the empty answer.
pub fn prompt_confirm_from<R: std::io::BufRead, W: std::io::Write>(
    reader: &mut R,
    writer: &mut W,
    prompt: &str,
    default_no: bool,
) -> Result<bool, String> {
    loop {
        let rendered = format_confirm_prompt(prompt, default_no);
        std::io::Write::write_all(writer, rendered.as_bytes())
            .map_err(|e| format!("failed to write prompt: {e}"))?;
        std::io::Write::flush(writer).map_err(|e| format!("failed to write prompt: {e}"))?;
        let mut response = String::new();
        let read = std::io::BufRead::read_line(reader, &mut response)
            .map_err(|e| format!("failed to read user input: {e}"))?;
        if read == 0 {
            response.clear();
        }
        if let Some(answer) = interpret_confirm_answer(&response, default_no) {
            std::io::Write::write_all(writer, b"\n")
                .map_err(|e| format!("failed to write prompt: {e}"))?;
            return Ok(answer);
        }
    }
}

/// Whether the TTY gate can block prompting right now.
///
/// PHP: `phutil_console_require_tty()` (`src/console/format.php:194-198`)
/// throws only when `posix_isatty` exists AND stdin is not a TTY. On
/// Windows (and any platform without POSIX) there is no check at all,
/// so prompting proceeds to read stdin.
fn tty_gate_blocks() -> bool {
    cfg!(unix) && !std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// Run a confirm prompt on the real stdio, requiring a TTY where PHP
/// does (Unix with piped stdin).
///
/// PHP: `phutil_console_prompt()` (`:42-52`) echoes the prompt before the
/// TTY check throws, which this mirrors on stdout.
pub fn prompt_confirm(prompt: &str, default_no: bool) -> Result<bool, String> {
    if tty_gate_blocks() {
        print!("{}", format_confirm_prompt(prompt, default_no));
        let _ = std::io::Write::flush(&mut std::io::stdout());
        return Err(STDIN_NOT_TTY.to_string());
    }
    let mut reader = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    prompt_confirm_from(&mut reader, &mut stdout, prompt, default_no)
}

/// Where a raw diff comes from (`generateChanges()`, `:805-818`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawSource {
    /// `--raw`: read from stdin.
    Stdin,
    /// `--raw-command`: generate via a shell command.
    Command(String),
}

/// Decide the raw diff source, or fail outside raw mode (`:817`).
pub fn raw_diff_source(options: &Options) -> Result<RawSource, String> {
    if options.raw {
        Ok(RawSource::Stdin)
    } else if let Some(command) = options.raw_command.as_deref() {
        Ok(RawSource::Command(command.to_string()))
    } else {
        Err(diff_local::UNKNOWN_RAW_DIFF_SOURCE.to_string())
    }
}

/// Read a raw diff: the stdin notice plus body, or the command output.
///
/// PHP: `generateChanges()` (`:805-818`).
pub fn read_raw_diff_text(options: &Options) -> Result<String, String> {
    match raw_diff_source(options)? {
        RawSource::Stdin => {
            eprint!("{}", diff_local::RAW_STDIN_NOTICE);
            diff_local::read_raw_stdin_from(std::io::stdin())
        }
        RawSource::Command(command) => diff_local::run_raw_command_text(&command),
    }
}

/* -(  Full-State Change Collection (G1)  )---------------------------------------- */

/// Convert a parsed hunk to a submit payload hunk (`toDictionary`
/// fields). `explicit` is the hunk's `,length` spelling bits (T1153
/// EXPORT-E11); counts and missing-newline flags come from the shared
/// length-driven loop, where a `\ No newline` marker flags the old side
/// while new lines remain and the new side once exhausted (T1153
/// EXPORT-E15: `ArcanistDiffParser.php:939-997`).
pub fn submit_hunk_from_parsed(
    hunk: &diff_local::ParsedHunk,
    explicit: &diff_local::HunkLenExplicit,
) -> diff_submit::DiffHunk {
    // Counts honor the synthetic pre-rewrite override (T1153 EXPORT-E20).
    let (adds, dels) = diff_local::hunk_counts(hunk);
    let (_, _, missing_old, missing_new) =
        diff_local::hunk_stats(&hunk.corpus, hunk.old_len, hunk.new_len);
    diff_submit::DiffHunk {
        old_offset: i64::try_from(hunk.old_start).unwrap_or(i64::MAX),
        new_offset: i64::try_from(hunk.new_start).unwrap_or(i64::MAX),
        old_length: i64::try_from(hunk.old_len).unwrap_or(i64::MAX),
        new_length: i64::try_from(hunk.new_len).unwrap_or(i64::MAX),
        old_len_explicit: explicit.old_explicit,
        new_len_explicit: explicit.new_explicit,
        add_lines: i64::try_from(adds).unwrap_or(i64::MAX),
        del_lines: i64::try_from(dels).unwrap_or(i64::MAX),
        is_missing_old_newline: missing_old,
        is_missing_new_newline: missing_new,
        corpus: hunk.corpus.clone(),
    }
}

/// One `unix:filemode` property value (T1153 EXPORT-E13): PHP sets the
/// property from the diff's mode lines
/// (`ArcanistDiffParser.php:622-627`) and `toDictionary()` passes it
/// through; absent modes stay `[]`.
fn filemode_properties(mode: Option<&str>) -> serde_json::Value {
    match mode {
        Some(mode) => serde_json::json!({"unix:filemode": mode}),
        None => serde_json::Value::Array(Vec::new()),
    }
}

/// Convert a parsed change to a submit payload change, hunks included.
///
/// Added files report `/dev/null` like the parser (T1153 OLD-PATH).
pub fn submit_change_from_parsed(change: &diff_local::ParsedChange) -> diff_submit::DiffChange {
    let mut out = diff_submit::DiffChange::new_text(&change.old_path, &change.current_path);
    // T1153 EXPORT-E14: companions report a null old path (PHP never
    // sets it on the `buildChange($oldPath)` twin).
    if diff_local::is_companion_type(change.change_type) {
        out.old_path = None;
    }
    out.change_type = i64::from(change.change_type);
    out.file_type = i64::from(change.file_type);
    out.away_paths = change.away_paths.clone();
    out.commit_hash = change.commit_hash.clone();
    // T1153 EXPORT-E13: filemodes ride the properties, like PHP's
    // `setOldProperty()`/`setNewProperty()`.
    out.old_properties = filemode_properties(change.old_mode.as_deref());
    out.new_properties = filemode_properties(change.new_mode.as_deref());
    // `hunk_len_explicit` runs parallel to `hunks` (T1153 EXPORT-E11).
    out.hunks = change
        .hunks
        .iter()
        .zip(change.hunk_len_explicit.iter())
        .map(|(hunk, explicit)| submit_hunk_from_parsed(hunk, explicit))
        .collect();
    out
}

/// Submit payload changes from parsed changes, companions included
/// (T1153 EXPORT-E14: PHP's parser emits the folded away changes, so
/// every parsed submit path routes through here).
pub fn submit_changes_from_parsed(
    changes: &[diff_local::ParsedChange],
) -> Vec<diff_submit::DiffChange> {
    diff_local::with_companions(changes)
        .iter()
        .map(submit_change_from_parsed)
        .collect()
}

/// Parse raw diff text into submit payload changes, dropping `TYPE_MESSAGE`
/// pseudo-changes (`generateChanges()`, `:820-827`).
pub fn parse_raw_diff_changes(text: &str) -> Result<Vec<diff_submit::DiffChange>, String> {
    let parsed = diff_local::parse_unified_diff(text)?;
    Ok(submit_changes_from_parsed(
        &diff_local::without_message_changes(&parsed),
    ))
}

/// Build an `Included changes:` entry from a parsed change, counting
/// added/deleted lines from its hunks.
pub fn change_entry_from_parsed(change: &diff_local::ParsedChange) -> diff_local::ChangeEntry {
    let mut added = 0u64;
    let mut deleted = 0u64;
    for hunk in &change.hunks {
        // Counts come from the shared loop, honoring the synthetic
        // pre-rewrite override (T1153 EXPORT-E15/E20).
        let (adds, dels) = diff_local::hunk_counts(hunk);
        added += adds;
        deleted += dels;
    }
    diff_local::ChangeEntry {
        change_type: change.change_type,
        file_type: change.file_type,
        old_path: match change.change_type {
            moved if moved == diff_local::TYPE_MOVE_HERE || moved == diff_local::TYPE_COPY_HERE => {
                Some(change.old_path.clone())
            }
            _ => None,
        },
        away_paths: change.away_paths.clone(),
        current: FileDiff {
            path: change.current_path.clone(),
            added,
            deleted,
        },
    }
}

/// Working-copy VCS for the full-state flows (public mirror of [`Vcs`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    /// Git working copy.
    Git,
    /// Mercurial working copy.
    Hg,
    /// Subversion working copy.
    Svn,
}

impl VcsKind {
    /// Short name (`git`/`hg`/`svn`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            VcsKind::Git => "git",
            VcsKind::Hg => "hg",
            VcsKind::Svn => "svn",
        }
    }
}

/// Detect the working-copy VCS for the full-state flows, git first.
pub fn detect_vcs_kind(cwd: &Path) -> Option<VcsKind> {
    match detect_vcs(cwd) {
        Some(Vcs::Git) => Some(VcsKind::Git),
        Some(Vcs::Hg) => Some(VcsKind::Hg),
        Some(Vcs::Svn) => Some(VcsKind::Svn),
        None => None,
    }
}

/// Resolve an hg revision to its node hash.
///
/// PHP: `getCanonicalRevisionName()`-style `log -l 1 --template {node} -r`
/// (`ArcanistMercurialAPI.php:58-64`). `None` when unresolvable.
pub fn hg_rev_to_commit(cwd: &Path, rev: &str) -> Option<String> {
    hg_rev_to_commit_with(cwd, &hg::HgCommand::default(), rev)
}

/// [`hg_rev_to_commit`] over an injected `hg` backend.
pub fn hg_rev_to_commit_with(cwd: &Path, hg_cmd: &hg::HgCommand, rev: &str) -> Option<String> {
    let out = hg::output_with(
        cwd,
        hg_cmd,
        &["log", "-l", "1", "--template", "{node}", "-r", rev],
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let node = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if node.is_empty() { None } else { Some(node) }
}

/// Resolve the hg ancestor of the working copy and `commit`.
///
/// PHP: the `hg:` bookmark rule's `ancestor(., X)` lookup
/// (`ArcanistMercurialAPI.php:805-807`), mirroring git's merge-base for an
/// explicit base commit.
pub fn hg_ancestor(cwd: &Path, commit: &str, working_rev: &str) -> Result<String, String> {
    hg_ancestor_with(cwd, &hg::HgCommand::default(), commit, working_rev)
}

/// [`hg_ancestor`] over an injected `hg` backend.
pub fn hg_ancestor_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    commit: &str,
    working_rev: &str,
) -> Result<String, String> {
    let revset = format!("ancestor({working_rev},{commit})");
    let out = hg::output_with(
        cwd,
        hg_cmd,
        &["log", "--template", "{node}", "--rev", &revset],
    )
    .map_err(|e| format!("failed to run hg: {e}"))?;
    if !out.status.success() {
        return Err(format!("Unable to find any hg commit named '{commit}'."));
    }
    let node = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if node.is_empty() {
        return Err(format!("Unable to find any hg commit named '{commit}'."));
    }
    Ok(node)
}

/// Resolve the hg diff base: explicit `--base`, the positional commit's
/// ancestor, or the automatic outgoing base.
///
/// Divergence: `--base` is read as a plain revision; the `base` rule engine
/// (`resolveBaseCommitRule()`) needs `.arcconfig` configuration sources,
/// which are not ported.
pub fn resolve_hg_base(cwd: &Path, options: &Options) -> Result<String, String> {
    resolve_hg_base_with(cwd, &hg::HgCommand::default(), options)
}

/// [`resolve_hg_base`] over an injected `hg` backend.
pub fn resolve_hg_base_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
) -> Result<String, String> {
    if options.base.is_some() && options.commit.is_some() {
        return Err("'--base' can not be used with a base commit.".to_string());
    }
    if let Some(rev) = options.base.as_deref() {
        return hg_rev_to_commit_with(cwd, hg_cmd, rev)
            .ok_or_else(|| format!("Unable to find any hg commit named '{rev}'."));
    }
    if let Some(commit) = options.commit.as_deref() {
        return hg_ancestor_with(cwd, hg_cmd, commit, ".");
    }
    let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
    hg::auto_base_commit_with(cwd, hg_cmd, &branch, ".")
        .map(|(base, _)| base)
        .map_err(|e| format!("failed to determine hg base commit: {e}"))
}

/// Collect submit payload changes with hunks for any VCS or raw source.
///
/// Collect svn submit changes through the per-path fan-out.
///
/// PHP: the Subversion branch of `generateChanges()`
/// (`:832-884`): argv-limited status, externals warn/confirm, the
/// base-revision check, and one `--depth empty` diff per path.
fn collect_svn_submit_changes<R: std::io::BufRead, W: std::io::Write>(
    cwd: &Path,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    io: &mut diff_local::SvnPromptIo<'_, R, W>,
) -> Result<Vec<diff_submit::DiffChange>, String> {
    let script = diff_local::default_svn_diff_cmd().map(|path| path.to_string_lossy().into_owned());
    let config = svn::SvnDiffConfig {
        context: diff_local::DEFAULT_DIFF_CONTEXT_LINES,
        diff_cmd: script.as_deref(),
        diff_program: "diff",
        diff_prefix: &[],
    };
    let parsed =
        diff_local::collect_svn_parsed_changes_with_io(cwd, svn_cmd, &options.paths, &config, io)?;
    Ok(submit_changes_from_parsed(&parsed))
}

/// Real-stdio prompt state for the svn externals confirm.
fn stdio_prompt_io<'a>(
    reader: &'a mut std::io::BufReader<std::io::Stdin>,
    writer: &'a mut std::io::Stdout,
) -> diff_local::SvnPromptIo<'a, std::io::BufReader<std::io::Stdin>, std::io::Stdout> {
    diff_local::SvnPromptIo {
        reader,
        writer,
        warned_externals: false,
    }
}

/// PHP: `generateChanges()` (`:802-901`): raw text parses directly
/// (`:820-827`); git/hg diffs require non-empty output (`:884-898`); svn
/// fans out to per-path `--depth empty` diffs (`:832-884`).
pub fn collect_submit_changes(
    cwd: &Path,
    options: &Options,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffChange>, String> {
    collect_submit_changes_with(
        cwd,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        options,
        base,
        head,
    )
}

/// [`collect_submit_changes`] over injected `hg`/`svn` backends (the git
/// arm always uses the system binary). The svn externals prompt uses real
/// stdio; [`collect_submit_changes_with_io`] injects it.
pub fn collect_submit_changes_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffChange>, String> {
    let mut reader = std::io::BufReader::new(std::io::stdin());
    let mut writer = std::io::stdout();
    let mut io = stdio_prompt_io(&mut reader, &mut writer);
    collect_submit_changes_with_io(cwd, hg_cmd, svn_cmd, options, base, head, &mut io)
}

/// [`collect_submit_changes_with`] with injected prompt stdio for the svn
/// externals confirm.
/// Collect git submit changes, attaching synthetic hunks for pure moves.
///
/// T1153 EXPORT-E20: `loadSyntheticData()` runs on git submits in PHP.
/// Split from [`collect_submit_changes_with_io`] for the complexity
/// ratchet (cyclomatic ≤ 15).
fn collect_git_submit_changes(
    cwd: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffChange>, String> {
    let text =
        diff_local::get_full_git_diff(cwd, base, head, diff_local::DEFAULT_DIFF_CONTEXT_LINES)?;
    if text.is_empty() {
        return Err(diff_local::NO_CHANGES_FOUND.to_string());
    }
    let mut full = diff_local::with_companions(&diff_local::parse_unified_diff(&text)?);
    let raw_for = |path: &str| diff_local::get_raw_diff_text(cwd, base, path);
    diff_local::attach_synthetic_git_hunks(&mut full, &raw_for)?;
    Ok(submit_changes_from_parsed(&full))
}

pub fn collect_submit_changes_with_io<R: std::io::BufRead, W: std::io::Write>(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    base: &str,
    head: Option<&str>,
    io: &mut diff_local::SvnPromptIo<'_, R, W>,
) -> Result<Vec<diff_submit::DiffChange>, String> {
    if diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref()) {
        return parse_raw_diff_changes(&read_raw_diff_text(options)?);
    }
    let convert = |text: &str| -> Result<Vec<diff_submit::DiffChange>, String> {
        let parsed = diff_local::parse_unified_diff(text)?;
        Ok(submit_changes_from_parsed(&parsed))
    };
    match detect_vcs_kind(cwd) {
        Some(VcsKind::Git) => collect_git_submit_changes(cwd, base, head),
        Some(VcsKind::Hg) => {
            let text = diff_local::get_full_hg_diff_with(
                cwd,
                hg_cmd,
                base,
                diff_local::DEFAULT_DIFF_CONTEXT_LINES,
            )?;
            if text.is_empty() {
                return Err(diff_local::NO_CHANGES_FOUND.to_string());
            }
            Ok(convert(&text)?)
        }
        Some(VcsKind::Svn) => collect_svn_submit_changes(cwd, svn_cmd, options, io),
        None => Err("not inside a git, hg, or svn working copy".to_string()),
    }
}

/// Collect a full-state submit: hunk-carrying changes for any VCS or raw
/// source, `--verbatim` update fields, and the `--draft` create guard.
///
/// PHP: `run()` plus `generateChanges()` and `buildRevisionFromCommitMessage()`
/// (`:345-577,648-652,802-901`). Unlike [`prepare_submit`], changes carry
/// hunk corpora and hg/svn/raw sources work; `--draft --create` without
/// server transactions fails like the legacy fallback (`:515-521`).
pub fn prepare_submit_full(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
) -> Result<SubmitInput, String> {
    prepare_submit_full_with_report(cwd, options, mode, &diff_local::LintUnitReport::empty())
}

/// Collect a full-state submit, stamping the diff spec with `report`'s
/// lint/unit status words (`run()`, `:375-379`).
pub fn prepare_submit_full_with_report(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
    report: &diff_local::LintUnitReport,
) -> Result<SubmitInput, String> {
    prepare_submit_full_with_commands(
        cwd,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        options,
        mode,
        report,
    )
}

/// [`prepare_submit_full_with_report`] over injected `hg`/`svn` backends
/// (fake binaries in tests; the git arms always use the system binary).
/// The svn externals prompt uses real stdio; the `_io` variant injects it.
pub fn prepare_submit_full_with_commands(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    mode: &SubmitMode,
    report: &diff_local::LintUnitReport,
) -> Result<SubmitInput, String> {
    let mut reader = std::io::BufReader::new(std::io::stdin());
    let mut writer = std::io::stdout();
    let mut io = stdio_prompt_io(&mut reader, &mut writer);
    prepare_submit_full_with_commands_io(cwd, hg_cmd, svn_cmd, options, mode, report, &mut io)
}

/// Resolve the hg base for a full-state submit, rejecting `--head`.
fn submit_full_hg_base(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
) -> Result<String, String> {
    if options.head.is_some() {
        return Err(
            "Option '--head' is not supported under hg. Mercurial does not support --head yet."
                .to_string(),
        );
    }
    let mut resolved = options.clone();
    if !resolved.paths.is_empty() {
        resolved.commit = diff_local::parse_base_commit_argument(true, &resolved.paths)?;
    }
    resolve_hg_base_with(cwd, hg_cmd, &resolved)
}

/// Base commit plus head for a full-state submit.
fn submit_full_base_head(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    is_raw: bool,
) -> Result<(String, Option<String>), String> {
    if is_raw {
        return Ok((String::new(), None));
    }
    match detect_vcs_kind(cwd) {
        Some(VcsKind::Git) => {
            let range = resolve_range(cwd, options)?;
            Ok((range.base, range.head))
        }
        Some(VcsKind::Hg) => Ok((submit_full_hg_base(cwd, hg_cmd, options)?, None)),
        Some(VcsKind::Svn) => Ok((String::new(), None)),
        None => Err("not inside a git, hg, or svn working copy".to_string()),
    }
}

/// Working-copy kind for a full-state submit (`None` for raw sources).
fn submit_full_kind(cwd: &Path, is_raw: bool) -> Option<VcsKind> {
    if is_raw { None } else { detect_vcs_kind(cwd) }
}

/// Render an svn diff-command path for [`svn::SvnDiffConfig`].
fn svn_diff_cmd_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

/// Subversion parsed changes for a full-state submit, collected once.
fn submit_svn_parsed<R: std::io::BufRead, W: std::io::Write>(
    cwd: &Path,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    is_raw: bool,
    kind: Option<VcsKind>,
    io: &mut diff_local::SvnPromptIo<'_, R, W>,
) -> Result<Option<Vec<diff_local::ParsedChange>>, String> {
    if is_raw || kind != Some(VcsKind::Svn) {
        return Ok(None);
    }
    let script = diff_local::default_svn_diff_cmd().map(svn_diff_cmd_string);
    let config = svn::SvnDiffConfig {
        context: diff_local::DEFAULT_DIFF_CONTEXT_LINES,
        diff_cmd: script.as_deref(),
        diff_program: "diff",
        diff_prefix: &[],
    };
    Ok(Some(diff_local::collect_svn_parsed_changes_with_io(
        cwd,
        svn_cmd,
        &options.paths,
        &config,
        io,
    )?))
}

/// Submit payload changes from collected svn parses.
fn changes_from_parsed(parsed: &[diff_local::ParsedChange]) -> Vec<diff_submit::DiffChange> {
    submit_changes_from_parsed(parsed)
}

/// Submit payload changes, reusing the collected svn parse when present.
#[allow(clippy::too_many_arguments)]
fn submit_full_changes<R: std::io::BufRead, W: std::io::Write>(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    base: &str,
    head: Option<&str>,
    svn_parsed: &Option<Vec<diff_local::ParsedChange>>,
    io: &mut diff_local::SvnPromptIo<'_, R, W>,
) -> Result<Vec<diff_submit::DiffChange>, String> {
    if let Some(parsed) = svn_parsed {
        return Ok(changes_from_parsed(parsed));
    }
    collect_submit_changes_with_io(cwd, hg_cmd, svn_cmd, options, base, head, io)
}

/// Create a revision from `input` (`run()`, `:478-531`): resolve the
/// create corpus server-side (`parsecommitmessage` fields plus
/// transactions), then take `differential.revision.edit` when the server
/// sent transactions, else legacy `differential.createrevision` (where
/// `--draft` errors). Without a corpus (raw/svn sourceless creates) the
/// local fields go legacy, as before.
fn submit_create_revision(
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
    diff_id: i64,
    diff_phid: Option<&str>,
    diff_uri: &str,
) -> Result<diff_submit::RevisionResult, String> {
    let Some(corpus) = input.message_corpus.as_deref() else {
        diff_submit::check_draft_create(input.draft, false)?;
        let params = diff_submit::create_revision_params(input.fields.clone(), diff_id);
        return diff_submit::create_revision_legacy(call, &params);
    };
    let mut message = diff_local::CommitMessage::new_from_raw_corpus(corpus)?;
    message
        .pull_data_from_conduit(call, false)
        .map_err(diff_local::PullDataError::into_message)?;
    let fields = fields_json(&scrub_placeholder_fields(message.get_fields()));
    let xactions = message
        .xactions
        .as_ref()
        .and_then(|value| value.as_array())
        .map(|items| scrub_placeholder_xactions(items))
        .unwrap_or_default();
    if xactions.is_empty() {
        diff_submit::check_draft_create(input.draft, false)?;
        let params = diff_submit::create_revision_params(fields, diff_id);
        return diff_submit::create_revision_legacy(call, &params);
    }
    let params = diff_submit::revision_edit_params(&xactions, diff_phid, input.draft);
    let revision_id = diff_submit::create_revision_via_edit(call, &params)?;
    Ok(diff_submit::RevisionResult {
        revision_id,
        uri: revision_uri_from_diff_uri(diff_uri, revision_id),
    })
}

/// Drop a `<placeholder>` faux title from server-parsed fields, mirroring
/// PHP's field merge skip (`parseCommitMessagesIntoFields()`, `:1846-1852`).
fn scrub_placeholder_fields(fields: &diff_local::CommitFields) -> diff_local::CommitFields {
    let mut scrubbed = fields.clone();
    if matches!(
        scrubbed.get("title"),
        Some(diff_local::FieldValue::Text(title)) if title == diff_local::PLACEHOLDER_TITLE
    ) {
        scrubbed.remove("title");
    }
    scrubbed
}

/// Drop `<placeholder>` title transactions, the xactions half of
/// [`scrub_placeholder_fields`].
fn scrub_placeholder_xactions(xactions: &[serde_json::Value]) -> Vec<serde_json::Value> {
    xactions
        .iter()
        .filter(|xaction| {
            let is_title = xaction.get("type").and_then(|v| v.as_str()) == Some("title");
            let is_placeholder = xaction.get("value").and_then(|v| v.as_str())
                == Some(diff_local::PLACEHOLDER_TITLE);
            !(is_title && is_placeholder)
        })
        .cloned()
        .collect()
}

/* -(  Update Resolution (D6/D9/D10)  )--------------------------------------- */

/// `edit-messages.json` scratch file (`:657,681`).
pub const SCRATCH_EDIT_MESSAGES: &str = "edit-messages.json";
/// `update-messages.json` scratch file (`:703,715`).
pub const SCRATCH_UPDATE_MESSAGES: &str = "update-messages.json";
/// Editor name for revision-info edits (`:676-678`).
pub const EDITOR_DIFF_EDIT: &str = "differential-edit-revision-info";
/// Editor name for update comments (`:1753-1755`).
pub const EDITOR_DIFF_UPDATE: &str = "differential-update-comments";

/// Seams for resolving an update's fields and message
/// (`buildRevisionFromCommitMessage()` + `getCommitMessageFromRevision()`
/// + `getUpdateMessage()`, `:632-722,:1572-1610,:1715-1758`).
pub struct UpdateResolver<'a, R: std::io::BufRead, W: std::io::Write> {
    /// Prompt input.
    pub reader: &'a mut R,
    /// Prompt output.
    pub writer: &'a mut W,
    /// Conduit caller.
    pub call: &'a diff_submit::ConduitCall<'a>,
    /// VCS metadata dir (scratch files).
    pub metadata_dir: &'a Path,
    /// Interactive editor: template plus name to edited text.
    pub edit: &'a dyn Fn(&str, &str) -> Result<String, String>,
    /// `getDefaultUpdateMessage()` prefill, when the template is empty.
    pub prefill: &'a dyn Fn() -> Result<Option<String>, String>,
}

/// An update's remote-resolved payload: full parsed fields (DIFF-D6)
/// plus the update message (DIFF-D9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUpdate {
    /// Remote-parsed revision fields for `updaterevision`.
    pub fields: serde_json::Value,
    /// Update message (`--message` or the editor).
    pub message: String,
}

/// Which revision an update targets, and how its base corpus resolves.
pub struct UpdateTarget<'a> {
    /// Normalized revision id.
    pub revision_id: u64,
    /// Display text for the unknown-revision error (the normalized
    /// `--update` argument, or the id).
    pub revision_arg: &'a str,
    /// Local fields object (`{}` without `--verbatim`): nonempty fields
    /// defeat the edit-message cache.
    pub verbatim_fields: &'a serde_json::Value,
    /// Run the ownership check (`getCommitMessageFromRevision()` does;
    /// footer revisions were checked during message validation).
    pub check_ownership: bool,
    /// Footer path (DIFF-D8): base the corpus on the `edit` template
    /// with these fields instead of the plain revision corpus
    /// (`:667-670`, `commitMessageFromRevision` unset).
    pub edit_fields: Option<&'a serde_json::Value>,
}

/// Read a scratch JSON object, coercing missing/corrupt files to `{}`.
fn read_update_scratch(metadata_dir: &Path, name: &str) -> serde_json::Value {
    let value = read_scratch_json(metadata_dir, name);
    if value.is_object() {
        value
    } else {
        serde_json::json!({})
    }
}

/// Viewer PHID via `user.whoami` (PHP reads this from workflow auth
/// state, which has no Rust equivalent, so one explicit call stands in).
fn fetch_viewer_phid(call: &diff_submit::ConduitCall<'_>) -> Result<String, String> {
    let result = call("user.whoami", &serde_json::json!({}))?;
    result
        .get("phid")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "conduit user.whoami response has no phid".to_string())
}

/// Resolve an update's fields and message over `resolver`.
///
/// DIFF-D6: fields come from the server — the `edit:false` corpus (or
/// the `edit` template on the DIFF-D8 footer path) re-parsed via
/// `parsecommitmessage` — never `{}`. DIFF-D10: `--edit` opens the
/// corpus in the editor with an `edit-messages.json` cache. DIFF-D9: an
/// absent or empty `--message` opens the editor on the update template
/// (raw sources keep the ceb error); a blank result aborts.
pub fn resolve_update_revision<R: std::io::BufRead, W: std::io::Write>(
    resolver: &mut UpdateResolver<'_, R, W>,
    options: &Options,
    target: &UpdateTarget<'_>,
) -> Result<ResolvedUpdate, String> {
    let revision_id = target.revision_id;
    let id_arg = i64::try_from(revision_id)
        .map_err(|_| format!("Revision ID {revision_id} is out of range."))?;
    if target.check_ownership {
        check_update_revision_ownership(&mut *resolver, id_arg, target.revision_arg)?;
    }
    let (base_corpus, base_message) =
        fetch_base_update_message(resolver.call, id_arg, target.edit_fields)?;
    let id_key = revision_id.to_string();
    let remote_corpus = resolve_remote_corpus(
        resolver,
        &base_corpus,
        &id_key,
        options.edit,
        target.verbatim_fields,
    )?;
    let new_message =
        rebuild_update_message(resolver.call, base_message, &base_corpus, &remote_corpus)?;
    let fields = fields_json(new_message.get_fields());
    let message = match options.message.as_deref() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => prompt_update_message(&mut *resolver, options, &id_key, &new_message, revision_id)?,
    };
    Ok(ResolvedUpdate { fields, message })
}

/// Check the viewer owns the updated revision, erroring on unknown
/// revisions first.
fn check_update_revision_ownership<R: std::io::BufRead, W: std::io::Write>(
    resolver: &mut UpdateResolver<'_, R, W>,
    id_arg: i64,
    revision_arg: &str,
) -> Result<(), String> {
    let row = diff_submit::query_revision_row(resolver.call, id_arg)?
        .ok_or_else(|| diff_submit::unknown_revision_message(revision_arg))?;
    let viewer = fetch_viewer_phid(resolver.call)?;
    diff_local::check_revision_ownership(resolver.reader, resolver.writer, &row, &viewer)
}

/// Fetch the update's base corpus and its conduit-pulled message: the
/// `edit` template under footer fields, else the plain revision corpus.
fn fetch_base_update_message(
    call: &diff_submit::ConduitCall<'_>,
    id_arg: i64,
    edit_fields: Option<&serde_json::Value>,
) -> Result<(String, diff_local::CommitMessage), String> {
    let params = match edit_fields {
        Some(fields) => diff_submit::commit_message_update_params(id_arg, fields.clone()),
        None => diff_submit::commit_message_from_revision_params(id_arg),
    };
    let base_corpus = diff_submit::fetch_commit_message(call, &params)?;
    let mut base_message = diff_local::CommitMessage::new_from_raw_corpus(&base_corpus)?;
    base_message
        .pull_data_from_conduit(call, false)
        .map_err(diff_local::PullDataError::into_message)?;
    Ok((base_corpus, base_message))
}

/// Resolve the remote corpus: the cached edit or base corpus, run
/// through the revision-info editor.
fn resolve_remote_corpus<R: std::io::BufRead, W: std::io::Write>(
    resolver: &UpdateResolver<'_, R, W>,
    base_corpus: &str,
    id_key: &str,
    edit: bool,
    verbatim_fields: &serde_json::Value,
) -> Result<String, String> {
    let mut edit_messages = read_update_scratch(resolver.metadata_dir, SCRATCH_EDIT_MESSAGES);
    let cached = edit_messages
        .get(id_key)
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let use_fields = verbatim_fields
        .as_object()
        .is_some_and(|fields| !fields.is_empty());
    let remote_corpus = select_remote_corpus(base_corpus, cached, edit, use_fields);
    maybe_edit_remote_corpus(
        edit,
        resolver.edit,
        resolver.metadata_dir,
        id_key,
        &mut edit_messages,
        remote_corpus,
    )
}

/// Pick the remote corpus: the cached edit, unless editing is off,
/// nothing is cached, or verbatim fields defeat the cache.
fn select_remote_corpus(
    base_corpus: &str,
    cached: Option<String>,
    edit: bool,
    use_fields: bool,
) -> String {
    if !edit || cached.is_none() || use_fields {
        base_corpus.to_string()
    } else {
        cached.unwrap_or_default()
    }
}

/// Run the revision-info editor over the remote corpus, caching edits
/// under the revision id.
fn maybe_edit_remote_corpus(
    edit: bool,
    editor: &dyn Fn(&str, &str) -> Result<String, String>,
    metadata_dir: &Path,
    id_key: &str,
    edit_messages: &mut serde_json::Value,
    remote_corpus: String,
) -> Result<String, String> {
    if !edit {
        return Ok(remote_corpus);
    }
    let edited = editor(&remote_corpus, EDITOR_DIFF_EDIT)?;
    if edited == remote_corpus {
        return Ok(remote_corpus);
    }
    edit_messages[id_key] = serde_json::Value::String(edited.clone());
    write_scratch_json(metadata_dir, SCRATCH_EDIT_MESSAGES, edit_messages);
    Ok(edited)
}

/// Rebuild the update message from an edited corpus, reusing the base
/// message when nothing changed.
fn rebuild_update_message(
    call: &diff_submit::ConduitCall<'_>,
    base_message: diff_local::CommitMessage,
    base_corpus: &str,
    remote_corpus: &str,
) -> Result<diff_local::CommitMessage, String> {
    if base_corpus == remote_corpus {
        return Ok(base_message);
    }
    let stripped = diff_local::remove_comments(remote_corpus);
    let mut fresh = diff_local::CommitMessage::new_from_raw_corpus(&stripped)?;
    fresh
        .pull_data_from_conduit(call, false)
        .map_err(diff_local::PullDataError::into_message)?;
    Ok(fresh)
}

/// Prompt the update message through the editor, caching comments under
/// the revision id. An empty message aborts the update.
fn prompt_update_message<R: std::io::BufRead, W: std::io::Write>(
    resolver: &mut UpdateResolver<'_, R, W>,
    options: &Options,
    id_key: &str,
    new_message: &diff_local::CommitMessage,
    revision_id: u64,
) -> Result<String, String> {
    let is_raw = diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref());
    diff_local::require_non_raw_update_message(is_raw)?;
    let mut update_messages = read_update_scratch(resolver.metadata_dir, SCRATCH_UPDATE_MESSAGES);
    let cached_message = update_messages
        .get(id_key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let template = if cached_message.is_empty() {
        update_message_template(resolver.prefill, new_message, revision_id)?
    } else {
        cached_message
    };
    let comments = (resolver.edit)(&template, EDITOR_DIFF_UPDATE)?;
    update_messages[id_key] = serde_json::Value::String(comments.clone());
    write_scratch_json(
        resolver.metadata_dir,
        SCRATCH_UPDATE_MESSAGES,
        &update_messages,
    );
    let stripped = diff_local::remove_comments(&comments);
    if stripped.trim().is_empty() {
        return Err(diff_local::USER_ABORT.to_string());
    }
    Ok(stripped)
}

/// Build the update template from the prefill comments and the
/// message's title.
fn update_message_template(
    prefill: &dyn Fn() -> Result<Option<String>, String>,
    new_message: &diff_local::CommitMessage,
    revision_id: u64,
) -> Result<String, String> {
    let comments = prefill()?.unwrap_or_default();
    let title = update_message_title(new_message);
    let template_id = new_message.revision_id.unwrap_or(revision_id);
    Ok(diff_local::default_update_message_template(
        &comments,
        template_id,
        &title,
    ))
}

/// The update template's title: the message's text title, if any.
fn update_message_title(new_message: &diff_local::CommitMessage) -> String {
    new_message
        .get_fields()
        .get("title")
        .and_then(|value| match value {
            diff_local::FieldValue::Text(title) => Some(title.clone()),
            diff_local::FieldValue::List(_) => None,
        })
        .unwrap_or_default()
}

/// Forget one revision's cached edit/update messages after a successful
/// update (`:470-475`). Write failures are ignored, like PHP.
fn clear_update_scratch(metadata_dir: &Path, revision_id: u64) {
    let id_key = revision_id.to_string();
    for name in [SCRATCH_EDIT_MESSAGES, SCRATCH_UPDATE_MESSAGES] {
        let mut saved = read_update_scratch(metadata_dir, name);
        if saved
            .as_object()
            .is_some_and(|map| map.contains_key(&id_key))
        {
            saved.as_object_mut().map(|map| map.remove(&id_key));
            write_scratch_json(metadata_dir, name, &saved);
        }
    }
}

/// VCS metadata dir for update scratch files (DIFF-D6/D9/D10).
fn update_metadata_dir(cwd: &Path, kind: Option<VcsKind>) -> Result<PathBuf, String> {
    let kind = kind.or_else(|| detect_vcs_kind(cwd));
    match kind {
        Some(VcsKind::Hg) => list::find_hg_root(cwd)
            .map(|root| root.join(".hg"))
            .ok_or_else(|| "not inside a Mercurial working copy".to_string()),
        Some(VcsKind::Svn) => svn::find_metadata_path(cwd)
            .ok_or_else(|| "not inside a Subversion working copy".to_string()),
        Some(VcsKind::Git) | None => git_metadata_dir(cwd),
    }
}

/// Usable-log prefill for a git update (`getGitUpdateMessage()`).
fn git_update_prefill(
    cwd: &Path,
    call: &diff_submit::ConduitCall<'_>,
    revision_id: i64,
    base: &str,
    head: Option<&str>,
) -> Result<Option<String>, String> {
    let messages = diff_local::get_git_update_messages(cwd, base, head.unwrap_or("HEAD"))?;
    let hashes: std::collections::HashSet<String> =
        diff_submit::active_diff_hashes(call, revision_id)?
            .into_iter()
            .collect();
    let usable = diff_local::select_usable_messages(&messages, &hashes)?;
    if usable.is_empty() {
        return Ok(None);
    }
    Ok(Some(diff_local::format_usable_logs(&usable)))
}

/// Usable-log prefill for an hg update (`getMercurialUpdateMessage()`).
fn hg_update_prefill(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    call: &diff_submit::ConduitCall<'_>,
    revision_id: i64,
    base: &str,
) -> Result<Option<String>, String> {
    let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
    let argv = diff_local::hg_commit_log_argv(base, &branch);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let text = hg::stdout_text_with(cwd, hg_cmd, &refs)?;
    let mut pairs = diff_local::parse_hg_commit_log(&text);
    pairs.reverse();
    let hashes: std::collections::HashSet<String> =
        diff_submit::active_diff_hashes(call, revision_id)?
            .into_iter()
            .collect();
    let usable = diff_local::select_usable_hg_messages(&pairs, &hashes);
    if usable.is_empty() {
        return Ok(None);
    }
    Ok(Some(diff_local::format_usable_logs(&usable)))
}

/// `getDefaultUpdateMessage()` (`:1962-1977`): usable-log prefills for
/// git/hg, none for svn and raw sources.
fn default_update_prefill(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    kind: Option<VcsKind>,
    call: &diff_submit::ConduitCall<'_>,
    revision_id: i64,
) -> Result<Option<String>, String> {
    match kind {
        Some(VcsKind::Git) => {
            let range = resolve_range(cwd, options)?;
            git_update_prefill(cwd, call, revision_id, &range.base, range.head.as_deref())
        }
        Some(VcsKind::Hg) => {
            let base = submit_full_hg_base(cwd, hg_cmd, options)?;
            hg_update_prefill(cwd, hg_cmd, call, revision_id, &base)
        }
        Some(VcsKind::Svn) | None => Ok(None),
    }
}

/// Resolve an update over real stdio, the interactive editor, and
/// log-based prefills.
#[allow(clippy::too_many_arguments)]
fn resolve_update_online(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    target: &UpdateTarget<'_>,
    kind: Option<VcsKind>,
    metadata_dir: &Path,
) -> Result<ResolvedUpdate, String> {
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin);
    let mut writer = std::io::stdout();
    let id_arg = i64::try_from(target.revision_id)
        .map_err(|_| format!("Revision ID {} is out of range.", target.revision_id))?;
    let prefill = || default_update_prefill(cwd, hg_cmd, options, kind, call, id_arg);
    let mut resolver = UpdateResolver {
        reader: &mut reader,
        writer: &mut writer,
        call,
        metadata_dir,
        edit: &|template, name| edit_interactively(template, name, None, None, 0),
        prefill: &prefill,
    };
    resolve_update_revision(&mut resolver, options, target)
}

/// The `--update` target: normalized display arg, prepared fields, and
/// the ownership check.
fn standard_update_target<'a>(
    options: &Options,
    fields: &'a serde_json::Value,
    revision_id: u64,
    display_arg: &'a mut String,
) -> UpdateTarget<'a> {
    *display_arg = options
        .update
        .as_deref()
        .map(diff_local::normalize_revision_id)
        .filter(|arg| !arg.is_empty())
        .unwrap_or(&revision_id.to_string())
        .to_string();
    UpdateTarget {
        revision_id,
        revision_arg: display_arg,
        verbatim_fields: fields,
        check_ownership: true,
        edit_fields: None,
    }
}

/* -(  Staging Area (D13)  )------------------------------------------------ */

/// Unique `repository.query` row (`loadRepositoryInformation()`,
/// `ArcanistWorkflow.php:1927-1966`): configured callsign, then the VCS
/// UUID (svn), then the remote URI; `None` without a query or a unique
/// match. `ERR-CONDUIT-CALL` (old server) also yields None; other
/// conduit failures propagate.
fn query_unique_repository_row(
    call: &diff_submit::ConduitCall<'_>,
    cwd: &Path,
    kind: Option<VcsKind>,
) -> Result<Option<serde_json::Value>, String> {
    let user_file = config::arcrc_path();
    let sources = get_config::load_sources(cwd, user_file.as_deref())?;
    let mut query = callsign_repository_query(&sources);
    if query.is_none() {
        query = vcs_repository_query(cwd, kind)?;
    }
    let Some(query) = query else {
        return Ok(None);
    };
    run_unique_repository_query(call, &query)
}

/// The `repository.query` constraint from the configured callsign, if any.
fn callsign_repository_query(sources: &get_config::Sources) -> Option<serde_json::Value> {
    if let Some((_, value)) = get_config::current_value("repository.callsign", sources)
        && let Some(callsign) = value.as_str().map(str::trim)
        && !callsign.is_empty()
    {
        Some(serde_json::json!({"callsigns": [callsign]}))
    } else {
        None
    }
}

/// The `repository.query` constraint from the working copy: the VCS
/// UUID (svn), then the remote URI.
fn vcs_repository_query(
    cwd: &Path,
    kind: Option<VcsKind>,
) -> Result<Option<serde_json::Value>, String> {
    let (uuid, remote) = match kind {
        Some(VcsKind::Hg) | None => (None, hg::get_remote_uri(cwd)?),
        Some(VcsKind::Svn) => (svn::get_repository_uuid(cwd)?, svn::get_remote_uri(cwd)?),
        Some(VcsKind::Git) => (None, git::remote_uri(cwd)?),
    };
    if let Some(uuid) = uuid {
        Ok(Some(serde_json::json!({"uuids": [uuid]})))
    } else if let Some(remote) = remote {
        Ok(Some(serde_json::json!({"remoteURIs": [remote]})))
    } else {
        Ok(None)
    }
}

/// Run a `repository.query` constraint, yielding the row on a unique
/// match. `ERR-CONDUIT-CALL` (old server) and non-unique matches yield
/// `None`; other conduit failures propagate.
fn run_unique_repository_query(
    call: &diff_submit::ConduitCall<'_>,
    query: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let results = match call("repository.query", query) {
        Ok(results) => results,
        Err(error) if is_old_server_error(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let matches = results.as_array().cloned().unwrap_or_default();
    if matches.len() != 1 {
        return Ok(None);
    }
    Ok(Some(matches[0].clone()))
}

/// True for `ERR-CONDUIT-CALL` failures: the server predates the method.
fn is_old_server_error(error: &str) -> bool {
    error == "ERR-CONDUIT-CALL" || error.starts_with("ERR-CONDUIT-CALL:")
}

/// A staging status notice on stderr (`writeInfo`/`writeOkay`/`writeWarn`
/// render `<bg:color>** %s **</bg> %s\n` to stderr, `:1564-1590`).
fn staging_notice(color: &str, label: &str, message: &str, ansi: bool) {
    if ansi {
        eprint!(
            "{}",
            terminal::format_status_message(color, label, message, true)
        );
    } else {
        eprintln!("** {label} ** {message}");
    }
}

/// Push the diff to the staging area and record the outcome
/// (`submitChangesToStagingArea()`/`pushChangesToStagingArea()`,
/// `:2588-2746`).
///
/// DIFF-D13: the skip ladder (flag, raw, unknown/unavailable/
/// unsupported/unconfigured repository, non-git client) records a skip
/// status; otherwise the base and diff tags push and the refs record.
/// Notices go to stderr, ungated: PHP's output buffer only captures
/// stdout, so stderr notices survive `--json` too. `spec_base` is the
/// spec's `sourceControlBaseRevision`.
///
/// Divergence: PHP pushes between `creatediff` and the revision submit;
/// this runs after the submit (the diff id is only known then), so a
/// staging failure surfaces after the revision exists.
#[allow(clippy::too_many_arguments)]
fn submit_changes_to_staging_area(
    cwd: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    kind: Option<VcsKind>,
    diff_id: i64,
    spec_base: &str,
    ansi: bool,
) -> Result<(), String> {
    let record = |status: &str, refs: &[serde_json::Value]| {
        let params = diff_submit::set_diff_property_params(
            diff_id,
            diff_submit::PROP_STAGING,
            &diff_submit::staging_property_data(status, refs),
        );
        call(diff_submit::METHOD_SET_DIFF_PROPERTY, &params).map(|_| ())
    };
    match plan_staging_push(options, call, cwd, kind)? {
        StagingPlan::Skip { message, status } => {
            staging_notice("blue", "SKIP STAGING", message, ansi);
            record(status, &[])
        }
        StagingPlan::Push { uri, prefix } => {
            push_staging_refs(cwd, &uri, &prefix, diff_id, spec_base, ansi, &record)
        }
    }
}

/// A staging decision: skip with a notice and status, or push to a URI.
enum StagingPlan {
    /// Skip the push, recording `status`.
    Skip {
        /// Stderr notice message.
        message: &'static str,
        /// Recorded staging status.
        status: &'static str,
    },
    /// Push the base and diff tags.
    Push {
        /// Staging area URI.
        uri: String,
        /// Tag prefix.
        prefix: String,
    },
}

/// Resolve the staging plan through the skip ladder (flag, raw,
/// unknown/unavailable/unsupported/unconfigured repository, non-git
/// client).
fn plan_staging_push(
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    cwd: &Path,
    kind: Option<VcsKind>,
) -> Result<StagingPlan, String> {
    if options.skip_staging {
        return Ok(StagingPlan::Skip {
            message: "Flag --skip-staging was specified.",
            status: diff_submit::STAGING_USER_SKIP,
        });
    }
    if diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref()) {
        return Ok(StagingPlan::Skip {
            message: "Raw changes can not be pushed to a staging area.",
            status: diff_submit::STAGING_DIFF_RAW,
        });
    }
    let row = query_unique_repository_row(call, cwd, kind)?;
    let Some(row) = row else {
        return Ok(StagingPlan::Skip {
            message: "Unable to determine repository for this change.",
            status: diff_submit::STAGING_REPOSITORY_UNKNOWN,
        });
    };
    let Some(staging) = row.get("staging") else {
        return Ok(StagingPlan::Skip {
            message: "The server does not support staging areas.",
            status: diff_submit::STAGING_REPOSITORY_UNAVAILABLE,
        });
    };
    if staging.get("supported").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(StagingPlan::Skip {
            message: "Phabricator does not support staging areas for this repository.",
            status: diff_submit::STAGING_REPOSITORY_UNSUPPORTED,
        });
    }
    let staging_uri = staging.get("uri").and_then(|v| v.as_str()).unwrap_or("");
    if staging_uri.is_empty() {
        return Ok(StagingPlan::Skip {
            message: "No staging area is configured for this repository.",
            status: diff_submit::STAGING_REPOSITORY_UNCONFIGURED,
        });
    }
    if !matches!(kind, Some(VcsKind::Git)) {
        return Ok(StagingPlan::Skip {
            message: "This client version does not support staging this repository.",
            status: diff_submit::STAGING_CLIENT_UNSUPPORTED,
        });
    }
    Ok(StagingPlan::Push {
        uri: staging_uri.to_string(),
        prefix: staging
            .get("prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("phabricator")
            .to_string(),
    })
}

/// Records a staging status plus refs (`setdiffproperty`).
type StagingRecorder<'a> = dyn Fn(&str, &[serde_json::Value]) -> Result<(), String> + 'a;

/// Push the base and diff tags to the staging area, recording the refs.
#[allow(clippy::too_many_arguments)]
fn push_staging_refs(
    cwd: &Path,
    staging_uri: &str,
    prefix: &str,
    diff_id: i64,
    spec_base: &str,
    ansi: bool,
    record: &StagingRecorder<'_>,
) -> Result<(), String> {
    let head = git::head_commit(cwd)?;
    let is_lfs = git::is_git_lfs_working_copy(cwd);
    let refs =
        diff_submit::staging_push_refs(staging_uri, prefix, diff_id, spec_base, &head, is_lfs);
    staging_notice(
        "green",
        "PUSH STAGING",
        "Pushing changes to staging area...",
        ansi,
    );
    let mut argv: Vec<&str> = vec!["push"];
    if git::version_at_least(&git::git_version(cwd)?, "1.8.2") {
        argv.push("--no-verify");
    }
    argv.push("--");
    argv.push(staging_uri);
    let refspecs: Vec<String> = refs
        .iter()
        .map(|row| {
            let commit = row.get("commit").and_then(|v| v.as_str()).unwrap_or("");
            let tag = row.get("ref").and_then(|v| v.as_str()).unwrap_or("");
            format!("{commit}:{tag}")
        })
        .collect();
    argv.extend(refspecs.iter().map(String::as_str));
    let out = git::output(cwd, &argv)?;
    if !out.status.success() {
        staging_notice(
            "yellow",
            "STAGING FAILED",
            "Unable to push changes to the staging area.",
            ansi,
        );
        return Err(
            "Failed to push changes to staging area. Correct the issue, or use --skip-staging to skip this step."
                .to_string(),
        );
    }
    if is_lfs {
        let forced = format!("+{head}:refs/tags/{prefix}/diff/{diff_id}");
        let _ = git::output(cwd, &["push", "--", staging_uri, &forced]);
    }
    record(diff_submit::STAGING_PUSHED, &refs)
}

/* -(  Bare Submit + Create Message (D8)  )--------------------------------- */

/// Editor name for new-revision messages (`:1477-1479`).
pub const EDITOR_NEW_COMMIT: &str = "new-commit";

/// Local commits for `getDefaultCreateFields()` (`:1404-1412`): git and
/// hg history, none for svn (`ArcanistSubversionAPI.php:609-611`).
fn create_local_commits(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    kind: Option<VcsKind>,
    base: &str,
    head: Option<&str>,
    branch: Option<&str>,
) -> Result<Vec<diff_local::LocalCommitInfo>, String> {
    match kind {
        Some(VcsKind::Git) => {
            diff_local::get_local_commit_information(cwd, base, head.unwrap_or("HEAD"))
        }
        Some(VcsKind::Hg) => {
            let branch = match branch {
                Some(branch) => branch.to_string(),
                None => diff_local::get_hg_branch_name_with(cwd, hg_cmd)?,
            };
            let commits = diff_local::get_hg_local_commits_with(cwd, hg_cmd, base, &branch)?;
            Ok(commits
                .into_iter()
                .map(|commit| diff_local::LocalCommitInfo {
                    commit: commit.node,
                    tree: String::new(),
                    parents: Vec::new(),
                    time: String::new(),
                    author: commit.author.unwrap_or_default(),
                    summary: commit.summary,
                    message: commit.message,
                    author_email: commit.author_email.unwrap_or_default(),
                })
                .collect())
        }
        Some(VcsKind::Svn) | None => Ok(Vec::new()),
    }
}

/// Seams for the interactive create-message cycle.
pub struct CreateMessageContext<'a, R: std::io::BufRead, W: std::io::Write> {
    /// Prompt input.
    pub reader: &'a mut R,
    /// Echo plus prompt output.
    pub writer: &'a mut W,
    /// Conduit caller.
    pub call: &'a diff_submit::ConduitCall<'a>,
    /// VCS metadata dir (scratch files).
    pub metadata_dir: &'a Path,
    /// Work root (readable scratch paths).
    pub work_root: &'a Path,
    /// Interactive editor: template text to edited text.
    pub edit: &'a dyn Fn(&str) -> Result<String, String>,
    /// Amend hook when one git commit absorbs the message; `None`
    /// saves to scratch instead.
    pub amend_commit: Option<diff_local::MessageHook<'a>>,
    /// Viewer PHID (ownership checks).
    pub user_phid: &'a str,
}

/// Interactively build a create message (`getCommitMessageFromUser()`,
/// `:1356-1559`): default fields always resolve first (their conduit
/// pulls run even when a saved template resumes, `:1398-1403`), then
/// the saved template (fields/notes cleared), the head-commit message
/// when parse notes exist, or the server `create` template — followed
/// by the edit/validate/re-edit cycle. `branch` is `None` for raw
/// sources. Returns the pulled, validated message.
pub fn create_message_from_user<R: std::io::BufRead, W: std::io::Write>(
    context: &mut CreateMessageContext<'_, R, W>,
    options: &Options,
    local: &[diff_local::LocalCommitInfo],
    branch: Option<&str>,
) -> Result<diff_local::CommitMessage, String> {
    let (fields, included, notes) = diff_local::get_default_create_fields_with(
        context.call,
        local,
        true,
        options.reviewers.as_deref(),
        options.cc.as_deref(),
        options.verbatim,
    )?;
    let saved = diff_local::resolve_saved_template(
        context.reader,
        context.writer,
        context.metadata_dir,
        context.work_root,
        options.verbatim,
    )?;
    let block = if included.is_empty() {
        Vec::new()
    } else {
        diff_local::included_commits_block(branch, &included)
    };
    let (template, template_is_default, notes) = match saved {
        Some(saved) => (saved, false, Vec::new()),
        None => {
            let template_is_default = fields.is_empty();
            let template = if notes.is_empty() {
                let params = diff_submit::commit_message_template_params(fields_json(&fields));
                diff_submit::fetch_commit_message(context.call, &params)?
            } else {
                // PHP takes the head commit message (`:1411-1412`); no
                // local commits (faux-only notes) degrades to `""`
                // instead of PHP's null-index notice.
                local
                    .first()
                    .map(|commit| commit.message.clone())
                    .unwrap_or_default()
            };
            (template, template_is_default, notes)
        }
    };
    let issues = diff_local::new_revision_issues(&block, &notes);
    let mut editor = diff_local::CreateMessageEditor {
        reader: &mut *context.reader,
        writer: &mut *context.writer,
        call: context.call,
        metadata_dir: context.metadata_dir,
        work_root: context.work_root,
        edit: context.edit,
        amend_commit: context.amend_commit,
        user_phid: context.user_phid,
        away_offset: 0,
    };
    diff_local::edit_create_message(
        &mut editor,
        &template,
        &template,
        template_is_default,
        options.verbatim,
        &issues,
    )
}

/// Detect the working-copy revisions for bare-submit dispatch
/// (`buildCommitMessage()`, `:1303-1310`).
fn load_working_copy_revisions(
    call: &diff_submit::ConduitCall<'_>,
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    kind: Option<VcsKind>,
    base: &str,
    head: Option<&str>,
    user_phid: &str,
) -> Result<Vec<diff_local::WorkingCopyRevision>, String> {
    match kind {
        Some(VcsKind::Git) => diff_local::load_working_copy_revisions_git(
            call,
            cwd,
            base,
            head.unwrap_or("HEAD"),
            user_phid,
        ),
        Some(VcsKind::Hg) => {
            let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
            diff_local::load_working_copy_revisions_hg(call, cwd, hg_cmd, base, &branch, user_phid)
        }
        Some(VcsKind::Svn) => diff_local::load_working_copy_revisions_svn(call, cwd, user_phid),
        None => Ok(Vec::new()),
    }
}

/// A create-destined message, resolved and validated.
pub struct ResolvedCreateMessage {
    /// Raw corpus for the create submit.
    pub corpus: String,
    /// Server-pulled fields (footer `use_fields` when verbatim).
    pub fields: diff_local::CommitFields,
    /// Footer revision id: update it instead of creating.
    pub footer_revision_id: Option<u64>,
}

/// Resolve a create-destined message: `--message-file` validates
/// without editing (`getCommitMessageFromFile()`, `:1357-1373`),
/// otherwise the interactive cycle runs (`getCommitMessageFromUser()`).
/// `branch` is `None` for raw sources.
pub fn resolve_create_message<R: std::io::BufRead, W: std::io::Write>(
    context: &mut CreateMessageContext<'_, R, W>,
    options: &Options,
    local: &[diff_local::LocalCommitInfo],
    branch: Option<&str>,
) -> Result<ResolvedCreateMessage, String> {
    if let Some(path) = options.message_file.as_deref() {
        let message = diff_local::get_commit_message_from_file_with(
            &mut *context.reader,
            &mut *context.writer,
            context.call,
            Path::new(path),
            context.user_phid,
            0,
        )?;
        return Ok(ResolvedCreateMessage {
            corpus: message.raw_corpus.clone(),
            fields: message.get_fields().clone(),
            footer_revision_id: message.revision_id,
        });
    }
    let message = create_message_from_user(context, options, local, branch)?;
    Ok(ResolvedCreateMessage {
        corpus: message.raw_corpus.clone(),
        fields: message.get_fields().clone(),
        footer_revision_id: message.revision_id,
    })
}

/// Resolve a create-destined message over stdio (`:1357-1559`).
///
/// The amend hook applies only to the single-git-commit workflow
/// (`count($included_commits) == 1`, git, `shouldAmend()`; hg keeps the
/// T2011 exclusion by never hooking).
#[allow(clippy::too_many_arguments)]
fn resolve_create_message_online(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    kind: Option<VcsKind>,
    metadata_dir: &Path,
    base: &str,
    head: Option<&str>,
    branch: Option<&str>,
) -> Result<ResolvedCreateMessage, String> {
    let user_phid = fetch_viewer_phid(call)?;
    let local = create_local_commits(cwd, hg_cmd, kind, base, head, branch)?;
    let included_count = usize::from(
        diff_local::faux_flag_message(options.reviewers.as_deref(), options.cc.as_deref())
            .is_some(),
    ) + local.len();
    let is_raw = diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref());
    let git_amend = |message: &str| amend_worktree_message_git(cwd, message);
    let amend_commit: Option<diff_local::MessageHook<'_>> = if included_count == 1
        && matches!(kind, Some(VcsKind::Git))
        && diff_local::should_amend(
            is_raw,
            options.no_amend,
            options.head.as_deref(),
            history_immutable_for(cwd, kind),
        ) {
        Some(&git_amend)
    } else {
        None
    };
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin);
    let mut writer = std::io::stdout();
    let mut context = CreateMessageContext {
        reader: &mut reader,
        writer: &mut writer,
        call,
        metadata_dir,
        work_root: cwd,
        edit: &|template| edit_interactively(template, EDITOR_NEW_COMMIT, None, None, 0),
        amend_commit,
        user_phid: &user_phid,
    };
    resolve_create_message(&mut context, options, &local, branch)
}

/// Resolve a footer-redirected update: the pulled message fields seed
/// the `edit` template under `--verbatim` (`{}` otherwise), ownership
/// was already checked during message validation (`:667-670`).
#[allow(clippy::too_many_arguments)]
fn resolve_footer_update_online(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    footer_id: u64,
    pulled_fields: &diff_local::CommitFields,
    kind: Option<VcsKind>,
    metadata_dir: &Path,
) -> Result<ResolvedUpdate, String> {
    let use_fields = if options.verbatim {
        fields_json(pulled_fields)
    } else {
        serde_json::json!({})
    };
    let display_arg = footer_id.to_string();
    let target = UpdateTarget {
        revision_id: footer_id,
        revision_arg: &display_arg,
        verbatim_fields: &use_fields,
        check_ownership: false,
        edit_fields: Some(&use_fields),
    };
    resolve_update_online(cwd, hg_cmd, options, call, &target, kind, metadata_dir)
}

/// True when a bare submit skips working-copy detection: `--verbatim`
/// always takes the create-message path (`:1298-1300`).
fn bare_submit_skips_detection(options: &Options) -> bool {
    options.verbatim
}

/// Submit a bare (flagless) git invocation: `--verbatim` creates
/// without detection, otherwise working-copy detection decides between
/// create, update, and the chooser error.
fn submit_bare_git_ansi(
    cwd: &Path,
    options: &Options,
    range: &Range,
    ansi: bool,
) -> Result<String, String> {
    if bare_submit_skips_detection(options) {
        return submit_via_client_ansi(cwd, options, &SubmitMode::Create, ansi);
    }
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    let user_phid = fetch_viewer_phid(&call)?;
    let mode = resolve_bare_submit_mode(
        &call,
        cwd,
        &hg::HgCommand::default(),
        Some(VcsKind::Git),
        &range.base,
        range.head.as_deref(),
        &user_phid,
    )?;
    submit_via_client_ansi(cwd, options, &mode, ansi)
}

/// Resolve a bare (flagless) submit mode via working-copy detection
/// (DIFF-D8, `:1303-1334`): no open revision creates, exactly one
/// updates, several raise the chooser error.
pub fn resolve_bare_submit_mode(
    call: &diff_submit::ConduitCall<'_>,
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    kind: Option<VcsKind>,
    base: &str,
    head: Option<&str>,
    user_phid: &str,
) -> Result<SubmitMode, String> {
    let revisions = load_working_copy_revisions(call, cwd, hg_cmd, kind, base, head, user_phid)?;
    match revisions.as_slice() {
        [] => Ok(SubmitMode::Create),
        [revision] => Ok(SubmitMode::Update(revision.id)),
        revisions => {
            let pairs: Vec<(i64, &str)> = revisions
                .iter()
                .map(|revision| {
                    (
                        i64::try_from(revision.id).unwrap_or(i64::MAX),
                        revision.title.as_str(),
                    )
                })
                .collect();
            Err(diff_submit::several_revisions_error(
                &diff_submit::render_revision_list(&pairs),
            ))
        }
    }
}

/// Revision web URI from the created diff's URI origin plus `/D{id}`,
/// mirroring PHP's conduit-URI hack (`:508-511`): both URIs are
/// server-absolute on the same host.
fn revision_uri_from_diff_uri(diff_uri: &str, revision_id: i64) -> String {
    let origin = diff_uri.split_once("://").and_then(|(scheme, rest)| {
        let host = rest.split('/').next().unwrap_or("");
        if scheme.is_empty() || host.is_empty() {
            return None;
        }
        Some(format!("{scheme}://{host}"))
    });
    match origin {
        Some(origin) => format!("{origin}/D{revision_id}"),
        None => format!("/D{revision_id}"),
    }
}

/// Update message for a full-state submit, if the mode needs one.
///
/// DIFF-D9: raw `--message` passthrough; absent/empty messages resolve
/// via the editor online (`resolve_update_revision`).
fn submit_full_message(options: &Options, mode: &SubmitMode) -> Result<Option<String>, String> {
    match mode {
        SubmitMode::Update(_) => Ok(options.message.clone()),
        _ => Ok(None),
    }
}

/// `Included changes:` entries from parsed changes.
fn entries_from_parsed(parsed: &[diff_local::ParsedChange]) -> Vec<diff_local::ChangeEntry> {
    let mut entries = Vec::new();
    for change in parsed {
        entries.push(change_entry_from_parsed(change));
    }
    entries
}

/// `Included changes:` entries for a raw full-state submit.
fn submit_raw_entries(options: &Options) -> Result<Vec<diff_local::ChangeEntry>, String> {
    let parsed = diff_local::parse_unified_diff(&read_raw_diff_text(options)?)?;
    let changes = diff_local::without_message_changes(&parsed);
    Ok(entries_from_parsed(&changes))
}

/// `Included changes:` entries for git or the hg/svn fallback.
fn submit_repo_entries(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_local::ChangeEntry>, String> {
    if detect_vcs_kind(cwd) == Some(VcsKind::Git) {
        return diff_local::collect_change_entries(cwd, base, head);
    }
    let text = svn_or_hg_diff_text_with(cwd, hg_cmd, svn_cmd, options, base)?;
    let parsed = diff_local::parse_unified_diff(&text)?;
    Ok(entries_from_parsed(&parsed))
}

/// `Included changes:` entries for a full-state submit.
#[allow(clippy::too_many_arguments)]
fn submit_full_entries(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    base: &str,
    head: Option<&str>,
    is_raw: bool,
    svn_parsed: &Option<Vec<diff_local::ParsedChange>>,
) -> Result<Vec<diff_local::ChangeEntry>, String> {
    if is_raw {
        return submit_raw_entries(options);
    }
    if let Some(parsed) = svn_parsed {
        return Ok(entries_from_parsed(parsed));
    }
    submit_repo_entries(cwd, hg_cmd, svn_cmd, options, base, head)
}

/// [`prepare_submit_full_with_commands`] with injected prompt stdio.
///
/// Subversion collects its parsed changes once and derives both the
/// submit payload and the included-changes entries from them, like PHP's
/// single `generateChanges()` result.
pub fn prepare_submit_full_with_commands_io<R: std::io::BufRead, W: std::io::Write>(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    mode: &SubmitMode,
    report: &diff_local::LintUnitReport,
    io: &mut diff_local::SvnPromptIo<'_, R, W>,
) -> Result<SubmitInput, String> {
    let is_raw = diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref());
    let (base, head) = submit_full_base_head(cwd, hg_cmd, options, is_raw)?;
    let kind = submit_full_kind(cwd, is_raw);
    let head_ref = head.as_deref();
    let svn_parsed = submit_svn_parsed(cwd, svn_cmd, options, is_raw, kind, io)?;
    let changes = submit_full_changes(
        cwd,
        hg_cmd,
        svn_cmd,
        options,
        &base,
        head_ref,
        &svn_parsed,
        io,
    )?;
    diff_submit::require_changes(changes.len())?;
    let spec_input = full_spec_input_with(cwd, hg_cmd, svn_cmd, kind, &base)?;
    let spec = diff_submit::diff_spec_json(
        &changes,
        diff_local::lint_status(report.lint_result),
        diff_local::unit_status(report.unit_result),
        &spec_input,
    );
    let fields = full_submit_fields_with(cwd, hg_cmd, options, kind, mode, &base, head_ref)?;
    let message_corpus = match mode {
        SubmitMode::Create => full_create_corpus_with(cwd, hg_cmd, options, kind, &base, head_ref)?,
        _ => None,
    };
    let message = submit_full_message(options, mode)?;
    let entries = submit_full_entries(
        cwd,
        hg_cmd,
        svn_cmd,
        options,
        &base,
        head_ref,
        is_raw,
        &svn_parsed,
    )?;
    Ok(SubmitInput {
        mode: mode.clone(),
        message,
        draft: options.draft,
        message_corpus,
        fields,
        spec,
        properties: {
            let mut properties = full_submit_properties_with(cwd, hg_cmd, kind, &base, head_ref)?;
            properties.extend(lint_unit_properties(report)?);
            properties
        },
        lint_unit: report.clone(),
        included_changes: diff_local::render_included_changes(&entries),
        json: options.json,
    })
}

/// Create-mode message corpus for the full-state pipeline: `--message-file`
/// wins alone, hg joins its log descriptions, git reuses the locked
/// collector, svn/raw fall back to flags-only (or nothing).
fn full_create_corpus_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    kind: Option<VcsKind>,
    base: &str,
    head: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(path) = options.message_file.as_deref() {
        let message = diff_local::commit_message_from_file(Path::new(path))?;
        return Ok(Some(message.raw_corpus));
    }
    match kind {
        Some(VcsKind::Git) => collect_create_corpus(cwd, options, base, head),
        Some(VcsKind::Hg) => {
            let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
            let argv = diff_local::hg_commit_log_argv(base, &branch);
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let text = hg::stdout_text_with(cwd, hg_cmd, &refs)?;
            let mut oldest_first: Vec<String> = diff_local::parse_hg_commit_log(&text)
                .into_iter()
                .map(|(_, desc)| desc)
                .collect();
            oldest_first.reverse();
            Ok(join_create_corpus(
                &oldest_first,
                options.reviewers.as_deref(),
                options.cc.as_deref(),
            ))
        }
        None | Some(VcsKind::Svn) => Ok(join_create_corpus(
            &[],
            options.reviewers.as_deref(),
            options.cc.as_deref(),
        )),
    }
}

/// Read the unified diff text for hg/svn entry rendering.
fn svn_or_hg_diff_text(cwd: &Path, options: &Options, base: &str) -> Result<String, String> {
    svn_or_hg_diff_text_with(
        cwd,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        options,
        base,
    )
}

/// [`svn_or_hg_diff_text`] over injected `hg`/`svn` backends.
fn svn_or_hg_diff_text_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    options: &Options,
    base: &str,
) -> Result<String, String> {
    match detect_vcs_kind(cwd) {
        Some(VcsKind::Hg) => diff_local::get_full_hg_diff_with(
            cwd,
            hg_cmd,
            base,
            diff_local::DEFAULT_DIFF_CONTEXT_LINES,
        ),
        Some(VcsKind::Svn) => {
            let mut argv = vec!["diff".to_string()];
            if !options.paths.is_empty() {
                argv.push("--".to_string());
                argv.extend(options.paths.iter().cloned());
            }
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = svn::output_with(cwd, svn_cmd, &refs)?;
            if !out.status.success() {
                return Err(format!(
                    "svn diff failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        _ => Err("not inside an hg or svn working copy".to_string()),
    }
}

/// Run the full-state pre-inspection checks: supports plus base-commit
/// derivation for every VCS, without the git-only gate.
///
/// PHP: `willRunWorkflow()` plus `runRepositoryAPISetup()`
/// (`ArcanistWorkflow.php:1494-1517`, `ArcanistDiffWorkflow.php:586-598`).
/// Raw sources skip detection (`requiresWorkingCopy()`, `:62-64`).
pub fn prepare_full(cwd: &Path, options: &Options) -> Result<(Options, Option<VcsKind>), String> {
    let mut resolved = options.clone();
    if resolved.head.as_deref() == Some("") {
        resolved.head = None;
    }
    if diff_local::is_raw_diff_source(resolved.raw, resolved.raw_command.as_deref()) {
        return Ok((resolved, None));
    }
    let Some(kind) = detect_vcs_kind(cwd) else {
        return Ok((resolved, None));
    };
    check_supports(
        match kind {
            VcsKind::Git => Vcs::Git,
            VcsKind::Hg => Vcs::Hg,
            VcsKind::Svn => Vcs::Svn,
        },
        &resolved.provided,
    )?;
    // Svn positionals are file filters, not base commits (`:786-796`); every
    // other VCS derives at most one base commit from them.
    if !resolved.paths.is_empty() && kind != VcsKind::Svn {
        resolved.commit = diff_local::parse_base_commit_argument(true, &resolved.paths)?;
    }
    Ok((resolved, Some(kind)))
}

/// Inspect with full-state flows: raw/hg/svn sources plus hunk-carrying
/// submits. Read-only output matches [`inspect`] for git; hg/svn/raw emit
/// unified diff text (or a hunk-counted `--json` document).
/// Base commit plus head for a full-state `--json` document, by VCS kind.
fn inspect_full_json_base(
    cwd: &Path,
    options: &Options,
    kind: VcsKind,
) -> Result<(String, Option<String>), String> {
    match kind {
        VcsKind::Git => {
            let range = resolve_range(cwd, options)?;
            Ok((range.base, range.head))
        }
        VcsKind::Hg => Ok((resolve_hg_base(cwd, options)?, None)),
        VcsKind::Svn => Ok((String::new(), None)),
    }
}

/// Change entries for a full-state `--json` document, by VCS kind.
fn inspect_full_json_entries(
    cwd: &Path,
    options: &Options,
    kind: VcsKind,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_local::ChangeEntry>, String> {
    if kind == VcsKind::Git {
        return diff_local::collect_change_entries(cwd, base, head);
    }
    let text = svn_or_hg_diff_text(cwd, options, base)?;
    let parsed = diff_local::parse_unified_diff(&text)?;
    let mut entries = Vec::new();
    for change in &parsed {
        entries.push(change_entry_from_parsed(change));
    }
    Ok(entries)
}

/// Project change entries to their numstat rows.
fn inspect_full_json_files(entries: &[diff_local::ChangeEntry]) -> Vec<FileDiff> {
    let mut files = Vec::new();
    for entry in entries {
        files.push(entry.current.clone());
    }
    files
}

/// Attach `fields` to a full-state `--json` document when field flags exist.
fn attach_inspect_full_fields(
    cwd: &Path,
    options: &Options,
    kind: VcsKind,
    base: &str,
    head: Option<&str>,
    doc: &mut serde_json::Value,
) -> Result<(), String> {
    if !has_field_flags(options) {
        return Ok(());
    }
    if kind == VcsKind::Git {
        let fields = collect_fields(cwd, options, base, head)?;
        doc["fields"] = fields_json(&fields);
    } else {
        doc["fields"] = serde_json::json!({});
    }
    Ok(())
}

/// Encode a `--json` document with its trailing newline.
fn encode_json_doc(doc: &serde_json::Value) -> Result<Vec<u8>, String> {
    let mut text = match diff_submit::php_json_encode_pretty(doc) {
        Ok(text) => text,
        Err(e) => return Err(format!("failed to encode JSON: {e}")),
    };
    text.push('\n');
    Ok(text.into_bytes())
}

/// Render a full-state read-only `--json` document for `kind`.
fn inspect_full_json(cwd: &Path, options: &Options, kind: VcsKind) -> Result<Vec<u8>, String> {
    let (base, head) = inspect_full_json_base(cwd, options, kind)?;
    let head_ref = head.as_deref();
    let entries = inspect_full_json_entries(cwd, options, kind, &base, head_ref)?;
    let files = inspect_full_json_files(&entries);
    let mut doc = diff_json(&base, &files);
    attach_inspect_full_fields(cwd, options, kind, &base, head_ref, &mut doc)?;
    encode_json_doc(&doc)
}

/// Read-only git text output for [`inspect_full`].
fn inspect_full_text_git(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    let range = resolve_range(cwd, options)?;
    default_diff_output_range(cwd, &range.base, range.head.as_deref())
}

/// Read-only hg text output for [`inspect_full`].
fn inspect_full_text_hg(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    let base = resolve_hg_base(cwd, options)?;
    let text = svn_or_hg_diff_text(cwd, options, &base)?;
    if text.is_empty() {
        return Err(diff_local::NO_CHANGES_FOUND.to_string());
    }
    Ok(text.into_bytes())
}

/// Read-only svn text output for [`inspect_full`].
fn inspect_full_text_svn(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    let text = svn_or_hg_diff_text(cwd, options, "")?;
    if text.is_empty() {
        return Err(diff_local::NO_CHANGES_FOUND.to_string());
    }
    Ok(text.into_bytes())
}

/// Read-only text output for [`inspect_full`], by VCS kind.
fn inspect_full_text(cwd: &Path, options: &Options, kind: VcsKind) -> Result<Vec<u8>, String> {
    match kind {
        VcsKind::Git => inspect_full_text_git(cwd, options),
        VcsKind::Hg => inspect_full_text_hg(cwd, options),
        VcsKind::Svn => inspect_full_text_svn(cwd, options),
    }
}

pub fn inspect_full(cwd: &Path, options: &Options) -> Result<Vec<u8>, String> {
    inspect_full_ansi(cwd, options, terminal::ansi_enabled())
}

/// [`inspect_full`] with an explicit ANSI setting for submit reports.
pub fn inspect_full_ansi(cwd: &Path, options: &Options, ansi: bool) -> Result<Vec<u8>, String> {
    let (resolved, kind) = prepare_full(cwd, options)?;
    let options = &resolved;
    if let Some(mode) = submit_mode(options)? {
        return submit_full_via_client_ansi(cwd, options, &mode, ansi).map(String::into_bytes);
    }
    if diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref()) {
        return read_raw_diff_text(options).map(String::into_bytes);
    }
    let Some(kind) = kind else {
        return Err("not inside a git, hg, or svn working copy".to_string());
    };
    if options.json {
        inspect_full_json(cwd, options, kind)
    } else {
        inspect_full_text(cwd, options, kind)
    }
}

/// Collect a full-state submit locally, then run it over a real client.
fn submit_full_via_client_ansi(
    cwd: &Path,
    options: &Options,
    mode: &SubmitMode,
    ansi: bool,
) -> Result<String, String> {
    let is_raw = diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref());
    let kind = if is_raw { None } else { detect_vcs_kind(cwd) };
    let lint_report = full_lint_unit_report(cwd, options, kind, is_raw, ansi)?;
    let mut input = prepare_submit_full_with_report(cwd, options, mode, &lint_report)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    stamp_repository_phid(&client, cwd, is_raw, &mut input.spec)?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    // DIFF-D6/D9/D10: see the baseline path above.
    let update_id = maybe_apply_full_update(cwd, options, &call, kind, &mut input)?;
    let report = submit_over_and_stage(cwd, options, &call, kind, &input, ansi)?;
    // DIFF-D13/DIFF-D7: see the baseline path above.
    finalize_client_submit(
        cwd,
        &hg::HgCommand::default(),
        options,
        &call,
        kind,
        &input.mode,
        report.revision_id,
        update_id,
    )?;
    Ok(report.text)
}

/// Resolve an `--update` target for a full submit, returning the id.
fn maybe_apply_full_update(
    cwd: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    kind: Option<VcsKind>,
    input: &mut SubmitInput,
) -> Result<Option<u64>, String> {
    let update_id = match &input.mode {
        SubmitMode::Update(id) => Some(*id),
        _ => None,
    };
    if let Some(update_id) = update_id {
        apply_client_update_resolution(
            cwd,
            &hg::HgCommand::default(),
            options,
            call,
            kind,
            input,
            update_id,
        )?;
    }
    Ok(update_id)
}

/// Lint/unit report for a full submit. Outside a working copy the
/// prepare step reports the canonical error; lint/unit stay unrunnable
/// (PHP's repository setup throws before `:363` just the same).
fn full_lint_unit_report(
    cwd: &Path,
    options: &Options,
    kind: Option<VcsKind>,
    is_raw: bool,
    ansi: bool,
) -> Result<diff_local::LintUnitReport, String> {
    if !is_raw && kind.is_none() {
        return Ok(diff_local::LintUnitReport::empty());
    }
    let (base, head) = lint_unit_base(cwd, options, kind, is_raw)?;
    let head_ref = head.as_deref();
    run_lint_unit_stdio_ansi(cwd, options, base.as_deref(), head_ref, is_raw, ansi)
}

/// Base commit plus head for lint/unit `--rev`, by repository kind.
///
/// Git resolves the range, hg its base, and svn/raw sources yield no
/// base (svn supports no commit ranges; raw sources skip the children).
fn lint_unit_base(
    cwd: &Path,
    options: &Options,
    kind: Option<VcsKind>,
    is_raw: bool,
) -> Result<(Option<String>, Option<String>), String> {
    if is_raw {
        return Ok((None, None));
    }
    match kind {
        Some(VcsKind::Git) => {
            let range = resolve_range(cwd, options)?;
            Ok((Some(range.base), range.head))
        }
        Some(VcsKind::Hg) => {
            let mut resolved = options.clone();
            if !resolved.paths.is_empty() {
                resolved.commit = diff_local::parse_base_commit_argument(true, &resolved.paths)?;
            }
            Ok((Some(resolve_hg_base(cwd, &resolved)?), None))
        }
        _ => Ok((None, None)),
    }
}

/* -(  Full-State Editor and Scratch Files (G1)  )-------------------------------- */

/// Strip editor-name characters outside `[A-Z0-9._-]`.
///
/// PHP: `PhutilInteractiveEditor::setName()`
/// (`src/console/PhutilInteractiveEditor.php:206-210`).
#[must_use]
pub fn sanitize_editor_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}

/// Editor document file name, defaulting to `untitled` (`:220-225`).
#[must_use]
pub fn editor_file_name(name: &str) -> String {
    let clean = sanitize_editor_name(name);
    if clean.is_empty() {
        "untitled".to_string()
    } else {
        clean
    }
}

/// `Launching editor "{basename}"...` notice (`:83-85`).
#[must_use]
pub fn launching_editor_notice(editor: &str) -> String {
    let binary = editor.rsplit(['/', '\\']).next().unwrap_or(editor);
    format!("Launching editor \"{binary}\"...")
}

/// Error when no editor binary can be found (`:342-346`).
pub const NO_EDITOR_MESSAGE: &str = "Unable to launch an interactive text editor. Set the EDITOR \
    environment variable to an appropriate editor.";

/// Resolve the editor binary: preferred, `$EDITOR`, fallback, then the
/// `editor`/`nano`/`sensible-editor`/`vi` scan (`:320-347`).
pub fn resolve_editor(
    preferred: Option<&str>,
    editor_env: Option<&str>,
    fallback: Option<&str>,
    binary_exists: &dyn Fn(&str) -> bool,
) -> Result<String, String> {
    if let Some(editor) = preferred.filter(|e| !e.is_empty()) {
        return Ok(editor.to_string());
    }
    if let Some(editor) = editor_env.filter(|e| !e.is_empty()) {
        return Ok(editor.to_string());
    }
    if let Some(editor) = fallback.filter(|e| !e.is_empty()) {
        return Ok(editor.to_string());
    }
    for candidate in ["editor", "nano", "sensible-editor", "vi"] {
        if binary_exists(candidate) {
            return Ok(candidate.to_string());
        }
    }
    Err(NO_EDITOR_MESSAGE.to_string())
}

/// True when `cmd` names an executable on `PATH` (a `binaryExists()` port).
///
/// Divergence: Windows consults `exe`/`bat`/`cmd` extensions only, not the
/// full `PATHEXT` list.
#[must_use]
pub fn binary_exists(cmd: &str) -> bool {
    if cmd.contains('/') || cmd.contains('\\') {
        return Path::new(cmd).is_file();
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    let separator = if cfg!(windows) { ';' } else { ':' };
    path.split(separator).any(|dir| {
        let candidate = Path::new(dir).join(cmd);
        if candidate.is_file() {
            return true;
        }
        cfg!(windows)
            && ["exe", "bat", "cmd"]
                .iter()
                .any(|ext| candidate.with_extension(ext).is_file())
    })
}

/// Quote one shell argument, `escapeshellarg()`-style.
#[must_use]
pub fn shell_quote(arg: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", arg.replace('"', "\"\""))
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Editor cursor-offset flag: empty on Windows and for offset 0, `-l N`
/// for `mate*`, `+N` otherwise (`:146-154`).
#[must_use]
pub fn editor_offset_flag(editor: &str, offset: u32) -> String {
    if offset == 0 || cfg!(windows) {
        return String::new();
    }
    if editor.starts_with("mate") {
        format!("-l {offset}")
    } else {
        format!("+{offset}")
    }
}

/// True when a nonzero editor exit should be forgiven: a `vi`/`vim`
/// binary whose `+Q +q` diagnostic also fails (`:94-117`).
#[must_use]
pub fn forgive_editor_exit(binary: &str, diagnostic_failed: bool) -> bool {
    (binary == "vi" || binary == "vim") && diagnostic_failed
}

/// Error when the editor exits nonzero (`:119-122`).
#[must_use]
pub fn editor_exit_error(code: i32) -> String {
    format!("Editor exited with an error code (#{code}).")
}

/// Create a unique temporary subdirectory of the system temp dir.
pub fn unique_temp_subdir(prefix: &str) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}{}_{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create temporary directory: {e}"))?;
    Ok(dir)
}

/// Edit `initial` text interactively and return the result.
///
/// PHP: `PhutilInteractiveEditor::editInteractively()` (`:57-139`): the
/// document is written to `edit.*/{name}`, the editor runs through the
/// shell with stdio inherited, and the file is read back. Newlines convert
/// to CRLF and back on Windows (`:61-63,132-134`).
/// Editor-bound newlines: CRLF on Windows, verbatim elsewhere (`:61-63`).
fn edit_content_for_platform(initial: &str) -> String {
    if cfg!(windows) {
        initial.replace('\n', "\r\n")
    } else {
        initial.to_string()
    }
}

/// Convert editor output newlines back (`:132-134`).
fn edit_result_for_platform(result: String) -> String {
    if cfg!(windows) {
        result.replace("\r\n", "\n")
    } else {
        result
    }
}

/// Write the editor document to `edit.*/{name}`, returning `(dir, path)`.
fn create_edit_document(initial: &str, name: &str) -> Result<(PathBuf, PathBuf), String> {
    let content = edit_content_for_platform(initial);
    let dir = unique_temp_subdir("edit.")?;
    let path = dir.join(editor_file_name(name));
    if std::fs::write(&path, &content).is_err() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("failed to write editor temporary file".to_string());
    }
    Ok((dir, path))
}

/// Shell command running `editor` over `path` at `line_offset`.
fn editor_shell_command(editor: &str, line_offset: u32, path: &Path) -> String {
    let flag = editor_offset_flag(editor, line_offset);
    let path_text = path.to_string_lossy();
    if flag.is_empty() {
        format!("{} {}", editor, shell_quote(&path_text))
    } else {
        format!("{} {} {}", editor, flag, shell_quote(&path_text))
    }
}

/// Forgive a nonzero editor exit for `vi`/`vim` when the `+Q +q`
/// diagnostic also fails (`:94-117`).
fn maybe_forgive_editor_exit(editor: &str, code: i32) -> i32 {
    if code == 0 {
        return code;
    }
    let binary = editor.rsplit(['/', '\\']).next().unwrap_or(editor);
    if binary != "vi" && binary != "vim" {
        return code;
    }
    let diagnostic_failed = match diff_local::shell_command_status(&format!("{binary} +Q +q")) {
        Ok(status) => !status.success(),
        Err(_) => true,
    };
    if forgive_editor_exit(binary, diagnostic_failed) {
        0
    } else {
        code
    }
}

/// Read the editor result back, converting newlines and cleaning up.
fn finish_edit_document(path: &Path, dir: &Path) -> Result<String, String> {
    let result = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => {
            let _ = std::fs::remove_dir_all(dir);
            return Err("failed to read editor temporary file".to_string());
        }
    };
    let _ = std::fs::remove_dir_all(dir);
    Ok(edit_result_for_platform(result))
}

pub fn edit_interactively(
    initial: &str,
    name: &str,
    task_message: Option<&str>,
    preferred_editor: Option<&str>,
    line_offset: u32,
) -> Result<String, String> {
    let (dir, path) = create_edit_document(initial, name)?;
    let editor_env = std::env::var("EDITOR").ok();
    let editor = resolve_editor(
        preferred_editor,
        editor_env.as_deref(),
        None,
        &binary_exists,
    )?;
    println!("{}", launching_editor_notice(&editor));
    if let Some(message) = task_message {
        println!("{message}");
    }
    let command = editor_shell_command(&editor, line_offset, &path);
    let status = diff_local::shell_command_status(&command)?;
    let code = maybe_forgive_editor_exit(&editor, status.code().unwrap_or(1));
    if code != 0 {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(editor_exit_error(code));
    }
    finish_edit_document(&path, &dir)
}

/// Scratch file path under a VCS metadata dir: `<metadata>/arc/<name>`.
///
/// PHP: `ArcanistRepositoryAPI::getScratchFilePath()`
/// (`src/repository/api/ArcanistRepositoryAPI.php:538-541`). The one-time
/// `.arc` migration (`:543-554`) is not ported.
#[must_use]
pub fn scratch_file_path(metadata_dir: &Path, name: &str) -> PathBuf {
    metadata_dir.join("arc").join(name)
}

/// Collapse `.` and `..` segments lexically (the `resolvePath()` subset
/// `readablePath()` needs for its second attempt).
fn lexically_normal_root(root: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in root.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Human-readable scratch path: relative to the work root when under it,
/// else absolute.
///
/// PHP: `ArcanistRepositoryAPI::getReadableScratchFilePath()`
/// (`src/repository/api/ArcanistRepositoryAPI.php:519-529`) via
/// `Filesystem::readablePath()` (`src/filesystem/Filesystem.php`): strip
/// a trailing-separator root prefix, trying the root as given and then
/// resolved.
#[must_use]
pub fn readable_scratch_file_path(metadata_dir: &Path, work_root: &Path, name: &str) -> String {
    let full = scratch_file_path(metadata_dir, name);
    if let Ok(relative) = full.strip_prefix(work_root) {
        return relative.to_string_lossy().into_owned();
    }
    let normal = lexically_normal_root(work_root);
    if let Ok(relative) = full.strip_prefix(&normal) {
        return relative.to_string_lossy().into_owned();
    }
    full.to_string_lossy().into_owned()
}

/// Read a scratch file, `None` on any failure (`:436-453`).
#[must_use]
pub fn read_scratch_file(metadata_dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(scratch_file_path(metadata_dir, name)).ok()
}

/// Write a scratch file, creating the `arc` dir (`:465-494`).
pub fn write_scratch_file(metadata_dir: &Path, name: &str, data: &str) -> bool {
    let path = scratch_file_path(metadata_dir, name);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    std::fs::write(path, data).is_ok()
}

/// Remove a scratch file (`:496-509`).
pub fn remove_scratch_file(metadata_dir: &Path, name: &str) -> bool {
    std::fs::remove_file(scratch_file_path(metadata_dir, name)).is_ok()
}

/// Read a scratch JSON file, empty object on any failure
/// (`ArcanistWorkflow.php:1699-1705`).
#[must_use]
pub fn read_scratch_json(metadata_dir: &Path, name: &str) -> serde_json::Value {
    read_scratch_file(metadata_dir, name)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Write a scratch JSON file (`:1734-1736`).
pub fn write_scratch_json(metadata_dir: &Path, name: &str, data: &serde_json::Value) -> bool {
    match diff_submit::php_json_encode(data) {
        Ok(text) => write_scratch_file(metadata_dir, name, &text),
        Err(_) => false,
    }
}

/// Resolve the git metadata dir (`.git`) for `cwd`.
pub fn git_metadata_dir(cwd: &Path) -> Result<PathBuf, String> {
    let out = git_output(cwd, &["rev-parse", "--git-dir"])?;
    if !out.status.success() {
        return Err("not inside a git repository".to_string());
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir.is_empty() {
        return Err("not inside a git repository".to_string());
    }
    let path = Path::new(&dir).to_path_buf();
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/* -(  Full-State Browser, Amend, Worktree Checks (G1)  )----------------------- */

/// Error when no browser command can be found
/// (`ArcanistWorkflow.php:2202-2206` at ceb, T1153 CORE-62).
pub const NO_BROWSER_MESSAGE: &str = "Unable to find a browser command to run. Set 'browser' in your \
    Arcanist config to specify a command to use.";

/// Browser argv for `uri`: explicit config, `cmd /c start` on Windows, else
/// the `sensible-browser`/`xdg-open`/`open --` scan (`:2169-2207` at ceb).
pub fn browser_open_argv(
    uri: &str,
    configured: Option<&[String]>,
    binary_exists: &dyn Fn(&str) -> bool,
) -> Result<Vec<String>, String> {
    if let Some(argv) = configured.filter(|argv| !argv.is_empty()) {
        let mut full = argv.to_vec();
        full.push(uri.to_string());
        return Ok(full);
    }
    if cfg!(windows) {
        return Ok(vec![
            "cmd".to_string(),
            "/c".to_string(),
            "start".to_string(),
            uri.to_string(),
        ]);
    }
    for (candidate, argv) in [
        ("sensible-browser", vec!["sensible-browser".to_string()]),
        ("xdg-open", vec!["xdg-open".to_string()]),
        ("open", vec!["open".to_string(), "--".to_string()]),
    ] {
        if binary_exists(candidate) {
            let mut full = argv;
            full.push(uri.to_string());
            return Ok(full);
        }
    }
    Err(NO_BROWSER_MESSAGE.to_string())
}

/// Error when the browser command fails (`:2159-2166` at ceb).
#[must_use]
pub fn browser_open_error(uri: &str, browser_argv: &[String]) -> String {
    format!(
        "Failed to open URI \"{uri}\" in browser (\"{}\"). Check your \"browser\" config option.",
        browser_argv.join(" ")
    )
}

/// Open `uri` with `browser_argv` (without the URI), checking the exit.
pub fn open_uri_in_browser(uri: &str, browser_argv: &[String]) -> Result<(), String> {
    let mut argv = browser_argv.to_vec();
    argv.push(uri.to_string());
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map_err(|_| browser_open_error(uri, browser_argv))?;
    if status.success() {
        Ok(())
    } else {
        Err(browser_open_error(uri, browser_argv))
    }
}

/// Read the `browser` command from any config source (`:2172-2175`).
///
/// Scalar strings become one argv word; lists of strings become argv;
/// empty or mistyped values are falsy in PHP and yield `None`.
/// `user_file` is the `~/.arcrc` path (`None` skips the user source).
pub fn configured_browser_command_in(cwd: &Path, user_file: Option<&Path>) -> Option<Vec<String>> {
    let sources = get_config::load_sources(cwd, user_file).ok()?;
    let (_, value) = get_config::current_value("browser", &sources)?;
    match value {
        serde_json::Value::String(word) if !word.is_empty() => Some(vec![word]),
        serde_json::Value::Array(words) => {
            let argv: Vec<String> = words
                .iter()
                .filter_map(|word| word.as_str().map(str::to_string))
                .collect();
            if argv.is_empty() { None } else { Some(argv) }
        }
        _ => None,
    }
}

/// Read the `browser` command for `cwd`, including the user source.
pub fn configured_browser_command(cwd: &Path) -> Option<Vec<String>> {
    let user_file = config::arcrc_path();
    configured_browser_command_in(cwd, user_file.as_deref())
}

/// True when `name` names a runnable file under one of `dirs`.
///
/// Unix requires an execute bit; elsewhere any plain file counts.
/// Backs [`binary_exists_on_path`]; split out so tests need no `PATH`
/// mutation.
fn binary_exists_in_dirs(name: &str, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() && is_runnable(&candidate)
    })
}

#[cfg(unix)]
fn is_runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_runnable(_path: &Path) -> bool {
    true
}

/// True when `name` resolves to a runnable file on `PATH`, standing in
/// for `Filesystem::binaryExists()` in the browser scan (`:2199-2203`).
///
/// Only the fixed `sensible-browser` / `xdg-open` / `open` probes use
/// this (Windows takes `cmd /c start` without probing), so `PATHEXT`
/// handling is intentionally absent.
pub fn binary_exists_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let dirs: Vec<PathBuf> = std::env::split_paths(&paths).collect();
    binary_exists_in_dirs(name, &dirs)
}

/// Browser opener seam: open `uri` with `browser_argv` (without the
/// URI), like [`open_uri_in_browser`].
pub type BrowserRunner<'a> = &'a dyn Fn(&str, &[String]) -> Result<(), String>;

/// Open the created object URI after submit (`:431-433`, `:560-562`).
///
/// No-op without a URI or without `--browse`
/// ([`diff_local::should_open_in_browser`]); otherwise resolves the
/// browser via [`browser_open_argv`] and opens through `run`, which
/// spawns like [`open_uri_in_browser`]. Mirrors `openURIsInBrowser()`
/// over the single created URI, including the failure text.
pub fn open_submit_result_in_browser(
    uri: Option<&str>,
    browse: bool,
    configured: Option<&[String]>,
    binary_exists: &dyn Fn(&str) -> bool,
    run: BrowserRunner<'_>,
) -> Result<(), String> {
    let Some(uri) = uri else {
        return Ok(());
    };
    if !diff_local::should_open_in_browser(browse) {
        return Ok(());
    }
    let full = browser_open_argv(uri, configured, binary_exists)?;
    let browser = &full[..full.len() - 1];
    run(uri, browser)
}

/// Run `submit_over_with_uri` over `call`, then `--browse` handling.
///
/// PHP echoes the success report before a browser failure throws
/// (`:431-433`, `:560-562`), so the report prints to stdout on that
/// path and the browser error still propagates.
fn submit_over_with_browse_ansi(
    cwd: &Path,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    input: &SubmitInput,
    ansi: bool,
) -> Result<SubmitReport, String> {
    let report = submit_over_with_uri_ansi(call, input, ansi)?;
    if let Err(error) = open_submit_result_in_browser(
        report.uri.as_deref(),
        options.browse,
        configured_browser_command(cwd).as_deref(),
        &binary_exists_on_path,
        &open_uri_in_browser,
    ) {
        print!("{}", report.text);
        return Err(error);
    }
    Ok(report)
}

/// Amend the hg worktree message (`ArcanistMercurialAPI::amendCommit()`
/// with a message file, `:715`).
pub fn amend_worktree_message_hg_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    message: &str,
) -> Result<(), String> {
    let scratch = unique_temp_subdir("amend.")?;
    let file = scratch.join("message.txt");
    if std::fs::write(&file, message).is_err() {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err("failed to write amend message file".to_string());
    }
    let argv = diff_local::hg_amend_argv(&file.to_string_lossy());
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = hg::output_with(dir, hg_cmd, &refs)?;
    let _ = std::fs::remove_dir_all(&scratch);
    if !out.status.success() {
        return Err(format!(
            "hg amend failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// `isHistoryImmutable()` (`ArcanistWorkflow.php:1592-1600`): the merged
/// `history.immutable` config wins, else the VCS default (hg/svn true,
/// git false). Config load failures fall back to the default.
fn history_immutable_for(cwd: &Path, kind: Option<VcsKind>) -> bool {
    let default = !matches!(kind, Some(VcsKind::Git) | None);
    let user_file = config::arcrc_path();
    let merged = get_config::load_merged(
        cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    );
    let Ok(merged) = merged else {
        return default;
    };
    get_config::merged_value("history.immutable", &merged)
        .and_then(|(_, value)| value.as_bool())
        .unwrap_or(default)
}

/// Fetch the post-create commit message and amend it into the worktree
/// (`run()`, `:535-551`; DIFF-D7).
///
/// No-op unless [`diff_local::should_amend`] holds. Notices echo to
/// stdout ahead of the report like PHP, except under `--json`, where
/// PHP discards them with the report (DIFF-D17).
fn amend_after_create(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    call: &diff_submit::ConduitCall<'_>,
    revision_id: i64,
    kind: Option<VcsKind>,
) -> Result<(), String> {
    // The fetch precedes the gate, like PHP (`:535-539` runs before
    // `shouldAmend()`).
    let params = diff_submit::commit_message_after_create_params(revision_id);
    let revised = diff_submit::fetch_commit_message(call, &params)?;
    let is_raw = diff_local::is_raw_diff_source(options.raw, options.raw_command.as_deref());
    if !diff_local::should_amend(
        is_raw,
        options.no_amend,
        options.head.as_deref(),
        history_immutable_for(cwd, kind),
    ) {
        return Ok(());
    }
    let supported = match kind {
        Some(VcsKind::Git) => true,
        Some(VcsKind::Hg) => hg_supports_amend_with(cwd, hg_cmd),
        Some(VcsKind::Svn) | None => false,
    };
    if !supported {
        if !options.json {
            // No trailing newline, like PHP (`:546-548` echoes bare).
            print!("{}", diff_local::AMEND_UNSUPPORTED);
        }
        return Ok(());
    }
    if !options.json {
        println!("{}", diff_local::UPDATING_COMMIT_MESSAGE);
    }
    match kind {
        Some(VcsKind::Hg) => amend_worktree_message_hg_with(cwd, hg_cmd, &revised),
        _ => amend_worktree_message_git(cwd, &revised),
    }
}

/// Amend the git worktree message (`ArcanistGitAPI::amendCommit()` with a
/// message, `:829-841`).
pub fn amend_worktree_message_git(dir: &Path, message: &str) -> Result<(), String> {
    let scratch = unique_temp_subdir("amend.")?;
    let file = scratch.join("message.txt");
    if std::fs::write(&file, message).is_err() {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err("failed to write amend message file".to_string());
    }
    let out = git_output(
        dir,
        &[
            "commit",
            "--amend",
            "--allow-empty",
            "-F",
            &file.to_string_lossy(),
        ],
    )?;
    let _ = std::fs::remove_dir_all(&scratch);
    if !out.status.success() {
        return Err(format!(
            "git amend failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// True when hg supports `commit --amend` here
/// (`ArcanistMercurialAPI::supportsAmend()`, `:469-476`).
#[must_use]
pub fn hg_supports_amend(dir: &Path) -> bool {
    hg_supports_amend_with(dir, &hg::HgCommand::default())
}

/// [`hg_supports_amend`] over an injected `hg` backend.
#[must_use]
pub fn hg_supports_amend_with(dir: &Path, hg_cmd: &hg::HgCommand) -> bool {
    hg::output_with(dir, hg_cmd, &["help", "commit"])
        .map(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).contains("amend"))
        .unwrap_or(false)
}

/// Fatal working-copy states in `requireCleanWorkingCopy()`
/// (`ArcanistWorkflow.php:1020-1072`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanWorktreeIssue {
    /// Incompletely checked-out directories (svn).
    Incomplete,
    /// Unresolved merge conflicts.
    Conflicts,
    /// Missing files.
    Missing,
}

/// `  Working copy: {path}` block for worktree errors (`:1025-1028`).
///
/// PHP wraps the path in `__...__` (underline on TTYs, stripped
/// otherwise); `ansi=false` renders byte-identical to the plain shape.
#[must_use]
pub fn working_copy_desc(path: &str) -> String {
    working_copy_desc_ansi(path, terminal::ansi_enabled())
}

/// [`working_copy_desc`] with an explicit ANSI setting.
#[must_use]
pub fn working_copy_desc_ansi(path: &str, ansi: bool) -> String {
    terminal::tsprintf_ansi(
        "  Working copy: __%R__\n\n",
        &[terminal::TVal::Text(path)],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_else(|_| format!("  Working copy: {path}\n\n"))
}

/// Usage error for a fatal working-copy state (`:1032-1072`).
#[must_use]
pub fn clean_worktree_error(issue: CleanWorktreeIssue, workdir: &str, paths: &[String]) -> String {
    clean_worktree_error_ansi(issue, workdir, paths, terminal::ansi_enabled())
}

/// [`clean_worktree_error`] with an explicit ANSI setting.
#[must_use]
pub fn clean_worktree_error_ansi(
    issue: CleanWorktreeIssue,
    workdir: &str,
    paths: &[String],
    ansi: bool,
) -> String {
    let desc = working_copy_desc_ansi(workdir, ansi);
    let list = paths.join("\n    ");
    match issue {
        CleanWorktreeIssue::Incomplete => format!(
            "You have incompletely checked out directories in this working copy. Fix them before \
             proceeding.'\n\n{desc}  Incomplete directories in working copy:\n    {list}\n\nYou can fix \
             these paths by running 'svn update' on them."
        ),
        CleanWorktreeIssue::Conflicts => format!(
            "You have merge conflicts in this working copy. Resolve merge conflicts before \
             proceeding.\n\n{desc}  Conflicts in working copy:\n    {list}"
        ),
        CleanWorktreeIssue::Missing => format!(
            "You have missing files in this working copy. Revert or formally remove them (with \
             `svn rm`) before proceeding.\n\n{desc}  Missing files in working copy:\n    {list}\n"
        ),
    }
}

/// Hint file for ignoring untracked files per VCS (`:1127-1142`).
#[must_use]
pub fn untracked_hint(vcs_name: &str) -> &'static str {
    match vcs_name {
        "git" => ".git/info/exclude",
        "svn" => "svn:ignore",
        _ => ".hgignore",
    }
}

/// Full untracked-files notice block (`:1121-1149`).
#[must_use]
pub fn untracked_block(workdir: &str, vcs_name: &str, untracked: &[String]) -> String {
    untracked_block_ansi(workdir, vcs_name, untracked, terminal::ansi_enabled())
}

/// [`untracked_block`] with an explicit ANSI setting.
#[must_use]
pub fn untracked_block_ansi(
    workdir: &str,
    vcs_name: &str,
    untracked: &[String],
    ansi: bool,
) -> String {
    let desc = working_copy_desc_ansi(workdir, ansi);
    let hint = format!(
        "(To ignore these {} change(s), add them to \"{}\".",
        untracked.len(),
        untracked_hint(vcs_name)
    );
    // NOTE: the closing paren below balances the hint's open paren; the
    // format pieces mirror the two `sprintf()` calls exactly.
    let list = untracked.join("\n    ");
    format!(
        "You have untracked files in this working copy.\n\n{desc}  Untracked changes in working \
         copy:\n  {hint})\n    {list}"
    )
}

/// Prompt to ignore untracked files (`:1151-1153`).
#[must_use]
pub fn untracked_prompt(count: usize) -> String {
    format!("Ignore these {count} untracked file(s) and continue?")
}

/// Full uncommitted-changes notice block (`:1169-1192`).
#[must_use]
pub fn uncommitted_block(workdir: &str, unstaged: &[String], uncommitted: &[String]) -> String {
    uncommitted_block_ansi(workdir, unstaged, uncommitted, terminal::ansi_enabled())
}

/// [`uncommitted_block`] with an explicit ANSI setting.
#[must_use]
pub fn uncommitted_block_ansi(
    workdir: &str,
    unstaged: &[String],
    uncommitted: &[String],
    ansi: bool,
) -> String {
    let desc = working_copy_desc_ansi(workdir, ansi);
    let mut lists = Vec::new();
    if !unstaged.is_empty() {
        lists.push(format!(
            "  Unstaged changes in working copy:\n    {}",
            unstaged.join("\n    ")
        ));
    }
    if !uncommitted.is_empty() {
        lists.push(format!(
            "Uncommitted changes in working copy:\n    {}",
            uncommitted.join("\n    ")
        ));
    }
    format!(
        "You have uncommitted changes in this working copy.\n\n{}{}\n",
        desc,
        lists.join("\n\n")
    )
}

/// Commit-message template for committing through the workflow (`:1222-1233`).
#[must_use]
pub fn commit_message_template(paths: &[String]) -> String {
    let mut sorted = paths.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut template = "\n\n# Enter a commit message.\n#\n# Changes:\n#\n".to_string();
    for path in &sorted {
        template.push_str(&format!("#     {path}\n"));
    }
    template
}

/// Editor task for the workflow commit message (`:1237-1239`).
pub const TASK_COMMIT_MESSAGE: &str =
    "Supply commit message for uncommitted changes, then save and exit.";
/// Error when the workflow commit template is unchanged (`:1243`).
pub const MUST_PROVIDE_MESSAGE: &str = "You must provide a commit message.";
/// Error when the workflow commit message is empty (`:1251`).
pub const MUST_PROVIDE_NONEMPTY_MESSAGE: &str = "You must provide a nonempty commit message.";
/// Error when uncommitted changes block the workflow (`:1209-1213`).
pub const CANNOT_CONTINUE_UNCOMMITTED: &str =
    "You can not continue with uncommitted changes. Commit or discard them before proceeding.";
/// Notice when autostash runs (`:1203-1205`).
pub const STASHING_NOTICE: &str =
    "Stashing uncommitted changes. (You can restore them with `git stash pop`).";

/// Notice that submodules carry changes (`:1083-1085`).
#[must_use]
pub fn externals_message(count: usize) -> String {
    format!("{count} submodule(s) have uncommitted or untracked changes:")
}

/// Prompt to ignore submodule changes (`:1087-1089`).
#[must_use]
pub fn externals_prompt(count: usize) -> String {
    format!("Ignore the changes to these {count} submodule(s) and continue?")
}

/* -(  Full-State Submit Assembly (G1)  )------------------------------------------- */

/// Build the repository-identity half of the diff spec for `kind`
/// (`None` for raw sources).
///
/// PHP: `buildDiffSpecification()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2216-2283`). Raw sources leave
/// every repository field unset (`:2227`); hg reports `sourceControlPath`
/// `/` (`ArcanistMercurialAPI.php:66-68`); svn reports the info URL and
/// `URL@REV` base (`ArcanistSubversionAPI.php:208-215`).
pub fn full_spec_input(
    cwd: &Path,
    kind: Option<VcsKind>,
    base: &str,
) -> Result<diff_submit::DiffSpecInput, String> {
    full_spec_input_with(
        cwd,
        &hg::HgCommand::default(),
        &svn::SvnCommand::default(),
        kind,
        base,
    )
}

/// [`full_spec_input`] over injected `hg`/`svn` backends.
pub fn full_spec_input_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    svn_cmd: &svn::SvnCommand,
    kind: Option<VcsKind>,
    base: &str,
) -> Result<diff_submit::DiffSpecInput, String> {
    let machine = diff_local::default_source_machine_name();
    match kind {
        None => {
            let local = diff_local::load_diff_specification(cwd, base, &machine, None, true)?;
            Ok(diff_submit::DiffSpecInput {
                source_machine: local.source_machine,
                source_path: local.source_path,
                branch: local.branch,
                bookmark: local.bookmark,
                source_control_system: local.source_control_system,
                source_control_path: local.source_control_path,
                source_control_base_revision: local.source_control_base_revision,
                repository_phid: None,
            })
        }
        Some(VcsKind::Git) => {
            let local = diff_local::load_diff_specification(cwd, base, &machine, None, false)?;
            Ok(diff_submit::DiffSpecInput {
                source_machine: local.source_machine,
                source_path: local.source_path,
                branch: local.branch,
                bookmark: local.bookmark,
                source_control_system: local.source_control_system,
                source_control_path: local.source_control_path,
                source_control_base_revision: local.source_control_base_revision,
                repository_phid: None,
            })
        }
        Some(VcsKind::Hg) => {
            let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
            let root = hg::stdout_text_with(cwd, hg_cmd, &["root"])?;
            // DIFF-D19: the active bookmark rides the spec (`:2231-2235`).
            let bookmark = diff_local::get_hg_active_bookmark_with(cwd, hg_cmd)?;
            Ok(diff_submit::DiffSpecInput {
                source_machine: machine,
                source_path: Some(root),
                branch: Some(branch),
                bookmark,
                source_control_system: Some("hg".to_string()),
                source_control_path: Some("/".to_string()),
                source_control_base_revision: Some(base.to_string()),
                repository_phid: None,
            })
        }
        Some(VcsKind::Svn) => {
            let info = diff_local::get_svn_info_with(cwd, svn_cmd, "/")?;
            let url = info.get("URL").cloned().unwrap_or_default();
            let root = info.get("Repository Root").cloned().unwrap_or_default();
            let rev = info.get("Revision").cloned().unwrap_or_default();
            Ok(diff_submit::DiffSpecInput {
                source_machine: machine,
                source_path: Some(cwd.to_string_lossy().into_owned()),
                branch: Some(diff_local::svn_branch_name(&url, &root)),
                bookmark: None,
                source_control_system: Some("svn".to_string()),
                source_control_path: Some(url.clone()),
                source_control_base_revision: Some(format!("{url}@{rev}")),
                repository_phid: None,
            })
        }
    }
}

/// Build the revision fields for a full-state submit.
///
/// PHP: `buildCommitMessage()` plus `buildRevisionFromCommitMessage()`
/// (`:1292-1353,648-652`): `--message-file` wins for creates on every VCS;
/// `--verbatim` passes local fields on update; raw and svn sources have no
/// local commits (svn merges only the faux `--reviewers`/`--cc` message).
pub fn full_submit_fields(
    cwd: &Path,
    options: &Options,
    kind: Option<VcsKind>,
    mode: &SubmitMode,
    base: &str,
    head: Option<&str>,
) -> Result<serde_json::Value, String> {
    full_submit_fields_with(
        cwd,
        &hg::HgCommand::default(),
        options,
        kind,
        mode,
        base,
        head,
    )
}

/// [`full_submit_fields`] over an injected `hg` backend.
pub fn full_submit_fields_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    options: &Options,
    kind: Option<VcsKind>,
    mode: &SubmitMode,
    base: &str,
    head: Option<&str>,
) -> Result<serde_json::Value, String> {
    match mode {
        SubmitMode::Only => Ok(serde_json::Value::Null),
        SubmitMode::Update(_) if !options.verbatim => Ok(serde_json::json!({})),
        _ => {
            if let Some(path) = options.message_file.as_deref() {
                let message = diff_local::commit_message_from_file(Path::new(path))?;
                return Ok(fields_json(&diff_local::parse_field_tags(
                    &message.raw_corpus,
                )));
            }
            match kind {
                None => Ok(serde_json::json!({})),
                Some(VcsKind::Git) => Ok(fields_json(&collect_fields(cwd, options, base, head)?)),
                Some(VcsKind::Hg) => {
                    let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
                    let argv = diff_local::hg_commit_log_argv(base, &branch);
                    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                    let text = hg::stdout_text_with(cwd, hg_cmd, &refs)?;
                    let mut oldest_first: Vec<String> = diff_local::parse_hg_commit_log(&text)
                        .into_iter()
                        .map(|(_, desc)| desc)
                        .collect();
                    oldest_first.reverse();
                    Ok(fields_json(&merge_message_fields(
                        &oldest_first,
                        options.reviewers.as_deref(),
                        options.cc.as_deref(),
                    )))
                }
                Some(VcsKind::Svn) => Ok(fields_json(&merge_message_fields(
                    &[],
                    options.reviewers.as_deref(),
                    options.cc.as_deref(),
                ))),
            }
        }
    }
}

/// Collect the diff properties for a full-state submit.
///
/// PHP: `updateLintDiffProperty()` and friends (`:2296-2360`): raw and svn
/// sources send nothing (no local commits, no onto targets); hg sends
/// `local:commits` only (onto targets are git-only); git reuses
/// [`diff_properties`].
pub fn full_submit_properties(
    cwd: &Path,
    kind: Option<VcsKind>,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffProperty>, String> {
    full_submit_properties_with(cwd, &hg::HgCommand::default(), kind, base, head)
}

/// [`full_submit_properties`] over an injected `hg` backend.
pub fn full_submit_properties_with(
    cwd: &Path,
    hg_cmd: &hg::HgCommand,
    kind: Option<VcsKind>,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<diff_submit::DiffProperty>, String> {
    match kind {
        None | Some(VcsKind::Svn) => Ok(Vec::new()),
        Some(VcsKind::Git) => diff_properties(cwd, base, head),
        Some(VcsKind::Hg) => {
            let branch = diff_local::get_hg_branch_name_with(cwd, hg_cmd)?;
            let commits = diff_local::get_hg_local_commits_with(cwd, hg_cmd, base, &branch)?;
            let mut properties = Vec::new();
            if diff_local::should_update_local_property(false, !commits.is_empty()) {
                let rendered: Vec<serde_json::Value> =
                    commits.iter().map(|commit| commit.to_json()).collect();
                let data = diff_submit::php_json_encode(&serde_json::Value::Array(rendered))
                    .map_err(|e| format!("failed to encode local commits: {e}"))?;
                properties.push(diff_submit::DiffProperty {
                    name: diff_submit::PROP_LOCAL_COMMITS.to_string(),
                    data,
                });
            }
            Ok(properties)
        }
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod diff_tests;
