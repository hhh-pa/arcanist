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
//!   stdin/command sources, `--edit`, `--verbatim`, `--draft`, and commit
//!   amend are still pending, as are hg/svn working copies and
//!   `.arcconfig` custom arguments. Lint/unit children run in-process;
//!   unresolved message dictionaries stay empty (see
//!   [`diff_local::run_lint`]).

extern crate self as arc_rs;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use arc_rs::{config, diff_local, diff_submit, get_config, hg, svn, terminal};

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
    diff_local::run_lint_unit_ansi(
        cwd,
        base,
        &lint_flags,
        &unit_flags,
        diff_local::should_skip_lint(options.nolint, is_raw, head),
        diff_local::should_skip_unit(options.nounit, is_raw, head),
        &diff_local::run_lint_in_process,
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
        diff_local::lint_status(report.lint_result),
        diff_local::unit_status(report.unit_result),
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
/// Submit report text plus the created object URI for `--browse`
/// (`:431-433`, `:560-562`).
pub struct SubmitReport {
    /// Human-readable report (also the `--json` document when requested).
    pub text: String,
    /// Created diff/revision URI, when the mode yields one.
    pub uri: Option<String>,
}

/// Format one submit report: plain lead, bold label, underlined URI.
///
/// PHP prints the lead plain; the URI line is `"**%s** __%s__"`
/// `__underline__`); `ansi=false` strips to the plain shape,
/// byte-identical to the historical `format!` output.
fn format_submit_report_ansi(
    lead: &str,
    label: &str,
    uri: &str,
    included: &str,
    ansi: bool,
) -> String {
    terminal::tsprintf_ansi(
        "**%R**\n        %R __%R__\n\n%R",
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
    diff_submit::set_diff_properties(call, created.diff_id, &input.properties)?;
    match &input.mode {
        SubmitMode::Only => {
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
                uri: Some(created.uri),
            })
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
            Ok(SubmitReport {
                text: format_submit_report_ansi(
                    "Updated an existing Differential revision:",
                    "Revision URI:",
                    &updated.uri,
                    &input.included_changes,
                    ansi,
                ),
                uri: Some(updated.uri),
            })
        }
        SubmitMode::Create => {
            // No server transactions are available locally, so the legacy
            // `differential.createrevision` path applies (`:478-531`).
            let params = diff_submit::create_revision_params(input.fields.clone(), created.diff_id);
            let revision = diff_submit::create_revision_legacy(call, &params)?;
            Ok(SubmitReport {
                text: format_submit_report_ansi(
                    "Created a new Differential revision:",
                    "Revision URI:",
                    &revision.uri,
                    &input.included_changes,
                    ansi,
                ),
                uri: Some(revision.uri),
            })
        }
    }
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
    let report =
        run_lint_unit_stdio_ansi(cwd, options, Some(range.base.as_str()), head, is_raw, ansi)?;
    let input = prepare_submit_with_report(cwd, options, mode, &report)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    submit_over_with_browse_ansi(cwd, options, &call, &input, ansi)
}

/// Inspect `cwd` per `options`, returning the bytes for stdout.
///
/// Without a submit mode this is the read-only inspection: `--json`
/// gains a `fields` object only when field flags are given, so default
/// output is byte-identical to before. With `--create`, `--update`, or
/// `--only`, the diff is submitted over Conduit instead.
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
        inspect_json_git(cwd, options, &range)
    } else {
        default_diff_output_range(cwd, &range.base, range.head.as_deref())
    }
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
            eprintln!("arc-rs diff: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let ansi = terminal::ansi_enabled();
    match inspect_ansi(&cwd, &options, ansi) {
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

/// Count added/deleted body lines in a hunk corpus (header skipped).
fn count_hunk_lines(corpus: &str) -> (i64, i64) {
    let mut adds = 0;
    let mut dels = 0;
    for (number, line) in corpus.lines().enumerate() {
        if number == 0 {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => adds += 1,
            Some(b'-') => dels += 1,
            _ => {}
        }
    }
    (adds, dels)
}

/// Missing-newline flags from `\ No newline` markers: the marker inherits
/// the side of the line it follows.
fn missing_newlines(corpus: &str) -> (bool, bool) {
    let mut missing_old = false;
    let mut missing_new = false;
    let mut previous: u8 = 0;
    for (number, line) in corpus.lines().enumerate() {
        if number == 0 {
            continue;
        }
        if line.starts_with("\\ No newline") {
            if previous == b'-' {
                missing_old = true;
            }
            if previous == b'+' {
                missing_new = true;
            }
        }
        previous = line.as_bytes().first().copied().unwrap_or(0);
    }
    (missing_old, missing_new)
}

/// Convert a parsed hunk to a submit payload hunk (`toDictionary` fields).
pub fn submit_hunk_from_parsed(hunk: &diff_local::ParsedHunk) -> diff_submit::DiffHunk {
    let (adds, dels) = count_hunk_lines(&hunk.corpus);
    let (missing_old, missing_new) = missing_newlines(&hunk.corpus);
    diff_submit::DiffHunk {
        old_offset: i64::try_from(hunk.old_start).unwrap_or(i64::MAX),
        new_offset: i64::try_from(hunk.new_start).unwrap_or(i64::MAX),
        old_length: i64::try_from(hunk.old_len).unwrap_or(i64::MAX),
        new_length: i64::try_from(hunk.new_len).unwrap_or(i64::MAX),
        add_lines: adds,
        del_lines: dels,
        is_missing_old_newline: missing_old,
        is_missing_new_newline: missing_new,
        corpus: hunk.corpus.clone(),
    }
}

/// Convert a parsed change to a submit payload change, hunks included.
///
/// Divergence: added files report an empty `oldPath` (normalized from
/// `/dev/null` by the parser).
pub fn submit_change_from_parsed(change: &diff_local::ParsedChange) -> diff_submit::DiffChange {
    let mut out = diff_submit::DiffChange::new_text(&change.old_path, &change.current_path);
    out.change_type = i64::from(change.change_type);
    out.file_type = i64::from(change.file_type);
    out.away_paths = change.away_paths.clone();
    out.commit_hash = change.commit_hash.clone();
    out.hunks = change.hunks.iter().map(submit_hunk_from_parsed).collect();
    out
}

/// Parse raw diff text into submit payload changes, dropping `TYPE_MESSAGE`
/// pseudo-changes (`generateChanges()`, `:820-827`).
pub fn parse_raw_diff_changes(text: &str) -> Result<Vec<diff_submit::DiffChange>, String> {
    let parsed = diff_local::parse_unified_diff(text)?;
    Ok(diff_local::without_message_changes(&parsed)
        .iter()
        .map(submit_change_from_parsed)
        .collect())
}

/// Build an `Included changes:` entry from a parsed change, counting
/// added/deleted lines from its hunks.
pub fn change_entry_from_parsed(change: &diff_local::ParsedChange) -> diff_local::ChangeEntry {
    let mut added = 0u64;
    let mut deleted = 0u64;
    for hunk in &change.hunks {
        let (adds, dels) = count_hunk_lines(&hunk.corpus);
        added += u64::try_from(adds).unwrap_or(0);
        deleted += u64::try_from(dels).unwrap_or(0);
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
    Ok(parsed.iter().map(submit_change_from_parsed).collect())
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
        Ok(parsed.iter().map(submit_change_from_parsed).collect())
    };
    match detect_vcs_kind(cwd) {
        Some(VcsKind::Git) => {
            let text = diff_local::get_full_git_diff(
                cwd,
                base,
                head,
                diff_local::DEFAULT_DIFF_CONTEXT_LINES,
            )?;
            if text.is_empty() {
                return Err(diff_local::NO_CHANGES_FOUND.to_string());
            }
            Ok(convert(&text)?)
        }
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
    let mut changes = Vec::new();
    for change in parsed {
        changes.push(submit_change_from_parsed(change));
    }
    changes
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

/// Reject `--draft --create` without server transactions (`:515-521`).
fn check_submit_draft(mode: &SubmitMode, draft: bool) -> Result<(), String> {
    if matches!(mode, SubmitMode::Create) {
        diff_submit::check_draft_create(draft, false)?;
    }
    Ok(())
}

/// Update message for a full-state submit, if the mode needs one.
fn submit_full_message(options: &Options, mode: &SubmitMode) -> Result<Option<String>, String> {
    match mode {
        SubmitMode::Update(_) => Ok(Some(update_message(options.message.as_deref())?)),
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
    check_submit_draft(mode, options.draft)?;
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
        fields,
        spec,
        properties: full_submit_properties_with(cwd, hg_cmd, kind, &base, head_ref)?,
        included_changes: diff_local::render_included_changes(&entries),
        json: options.json,
    })
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
    // Outside a working copy the prepare step reports the canonical
    // error; lint/unit stay unrunnable (PHP's repository setup throws
    // before `:363` just the same).
    let report = if !is_raw && kind.is_none() {
        diff_local::LintUnitReport::empty()
    } else {
        let (base, head) = lint_unit_base(cwd, options, kind, is_raw)?;
        let head_ref = head.as_deref();
        run_lint_unit_stdio_ansi(cwd, options, base.as_deref(), head_ref, is_raw, ansi)?
    };
    let input = prepare_submit_full_with_report(cwd, options, mode, &report)?;
    let client = config::client_for(options.conduit_uri.as_deref())?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    submit_over_with_browse_ansi(cwd, options, &call, &input, ansi)
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

/// Error when no browser command can be found (`:2204-2207`).
pub const NO_BROWSER_MESSAGE: &str = "Unable to find a browser command to run. Set \"browser\" in \
    your configuration to specify a command to use.";

/// Browser argv for `uri`: explicit config, `cmd /c start` on Windows, else
/// the `sensible-browser`/`xdg-open`/`open --` scan (`:2171-2208`).
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

/// Error when the browser command fails (`:2161-2166`).
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
) -> Result<String, String> {
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
    Ok(report.text)
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
            Ok(diff_submit::DiffSpecInput {
                source_machine: machine,
                source_path: Some(root),
                branch: Some(branch),
                bookmark: None,
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
