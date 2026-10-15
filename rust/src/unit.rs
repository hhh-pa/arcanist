//! `arc-rs unit` — full-state port of `arc unit` plus a cargo adapter.
//!
//! PHP references: `src/workflow/ArcanistUnitWorkflow.php` (`getArguments`
//! lines 38-107, engine setup lines 151-168, per-result rendering lines
//! 184-199, overall mapping lines 243-253, output formats lines 333-348),
//! `src/workflow/ArcanistWorkflow.php` (`newUnitTestEngine` lines 2107-2147),
//! `src/unit/engine/ArcanistUnitTestEngine.php` (run-all gate lines 21-32),
//! `src/unit/renderer/ArcanistUnitConsoleRenderer.php` (`renderUnitResult`
//! lines 5-30, result labels lines 32-54, time format lines 83-99).
//!
//! Selection mirrors `newUnitTestEngine`: explicit `--engine` (workflow
//! line 151 passes `getArgument('engine)`), then the `unit.engine`
//! `.arcconfig` key (workflow lines 2111-2113), then `.arcunit` in the
//! project root (lines 2115-2119). When nothing is configured this port
//! runs the `cargo` engine; PHP throws `ArcanistNoEngineException`
//! (lines 2121-2129) instead.
//!
//! `--engine` accepts `cargo` plus the PHP engine class names verbatim.
//! `.arcunit` `type` values accept the same class names plus `phutil`
//! (`PhutilUnitTestEngine.php:8-10`); PHP resolves `.arcunit` types through
//! a class map of `getEngineConfigurationName` values
//! (`ArcanistConfigurationDrivenUnitTestEngine.php:176-181`) where only
//! `phutil` is non-null in-tree, so accepting class names there diverges
//! from PHP to make every ported engine selectable.
//!
//! Out-of-process notes: the `cargo` engine treats positional paths as
//! `cargo test` filters and warns (instead of failing) on coverage flags
//! it cannot honor; the `phutil` engine shells out to the user's `php`
//! running a staged driver (see `unit_phutil`); `json`/`ugly` rendering
//! matches PHP byte-for-byte including the pretty numeric-key quirk (see
//! [`results_to_json_text`]).

extern crate self as arc_rs;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::hg::HgCommand;
use arc_rs::svn::SvnCommand;
use arc_rs::unit_config::{self, PlannedEngine};
use arc_rs::unit_engine::{
    CommandRunner, EngineContext, EngineError, UnitResult as EngineUnitResult,
};
use arc_rs::{config, unit_csharp, unit_nose, unit_phpunit, unit_phutil, unit_pytest, unit_xunit};

pub const NAME: &str = "unit";

/// Engines `--engine` accepts: `cargo` plus the PHP engine class names
/// verbatim (`NoseTestEngine.php:8`, `PytestTestEngine.php:6`,
/// `PhpunitTestEngine.php:6`, `PhutilUnitTestEngine.php:6`,
/// `XUnitTestEngine.php:12`, `CSharpToolsTestEngine.php:10`,
/// `ArcanistConfigurationDrivenUnitTestEngine.php:3`).
pub const AVAILABLE_ENGINES: &[&str] = &[
    "cargo",
    "ArcanistConfigurationDrivenUnitTestEngine",
    "CSharpToolsTestEngine",
    "NoseTestEngine",
    "PhpunitTestEngine",
    "PhutilUnitTestEngine",
    "PytestTestEngine",
    "XUnitTestEngine",
];

/// `.arcunit` `type` values: the PHP engine class names plus `phutil`
/// (`PhutilUnitTestEngine.php:9` returns `'phutil'`).
pub const CONFIG_AVAILABLE: &[&str] = &[
    "CSharpToolsTestEngine",
    "NoseTestEngine",
    "PhpunitTestEngine",
    "PhutilUnitTestEngine",
    "PytestTestEngine",
    "XUnitTestEngine",
    "phutil",
];

pub const USAGE: &str = "usage: arc-rs unit [--rev REV] [--engine ENGINE] [--coverage] [--no-coverage] [--detailed-coverage] [--json] [--output FORMAT] [--target PHID] [--everything] [--ugly] [--conduit-uri URI] [paths...] [-- <cargo-test-args>]\n";

/// Number of trailing output lines echoed after the one-line summary.
pub const TAIL_LINES: usize = 20;

/// Test outcome codes, mirroring `ArcanistUnitTestResult::RESULT_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
    Broken,
    Unsound,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Skip => "skip",
            Outcome::Broken => "broken",
            Outcome::Unsound => "unsound",
        }
    }

    pub fn from_pass(passed: bool) -> Self {
        if passed { Outcome::Pass } else { Outcome::Fail }
    }
}

/// One test result: name, outcome, and duration in seconds.
///
/// Mirrors `ArcanistUnitTestResult` (`name`, `result`, `duration`). Until
/// per-test parsing is ported, each engine run yields a single aggregate
/// result named after the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitResult {
    pub name: String,
    pub outcome: Outcome,
    pub duration_secs: f64,
}

impl UnitResult {
    pub fn new(name: &str, outcome: Outcome, duration_secs: f64) -> Self {
        Self {
            name: name.to_string(),
            outcome,
            duration_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Run tests covering changes since revision (`rev`, workflow lines 40-53).
    pub rev: Option<String>,
    /// Override configured engine (`engine`, lines 54-57).
    pub engine: String,
    /// Always enable coverage (`coverage`, lines 58-63).
    pub coverage: bool,
    /// Always disable coverage (`no-coverage`, lines 64-66).
    pub no_coverage: bool,
    /// Detailed coverage report; implies `--coverage` (`detailed-coverage`,
    /// lines 67-71).
    pub detailed_coverage: bool,
    /// Report results in JSON (`json`, lines 72-74).
    pub json: bool,
    /// Output format: `full` (default), `json`, `ugly`, `none` (`output`,
    /// lines 75-86).
    pub output: Option<String>,
    /// Harbormaster build target PHID to record results on (`target`, lines
    /// 87-92).
    pub target: Option<String>,
    /// Run every test (`everything`, lines 93-100).
    pub everything: bool,
    /// Uglier but more efficient JSON (`ugly`, lines 101-105).
    pub ugly: bool,
    /// Conduit URI override (global `--conduit-uri`, threaded like `diff`).
    pub conduit_uri: Option<String>,
    /// Positional paths (`*` maps to `paths`, line 106).
    pub paths: Vec<String>,
    /// Canonical long flag names in first-provision order (PHP `$dict`
    /// insertion order, `ArcanistWorkflow.php:791-884`). Empty for
    /// hand-built options, which skip the order-dependent checks.
    pub provided: Vec<&'static str>,
    /// `--help`/`-h` (compat wrapper only; `parse_options` returns `None`).
    pub help: bool,
    /// Cargo-test args after the first `--` (Rust extension; D1 collects
    /// these into `paths`).
    pub cargo_args: Vec<String>,
    /// True when `--engine` was given explicitly (override like PHP
    /// `newUnitTestEngine($engine_class)`).
    pub engine_explicit: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            rev: None,
            engine: "cargo".to_string(),
            coverage: false,
            no_coverage: false,
            detailed_coverage: false,
            json: false,
            output: None,
            target: None,
            everything: false,
            ugly: false,
            conduit_uri: None,
            paths: Vec::new(),
            provided: Vec::new(),
            help: false,
            cargo_args: Vec::new(),
            engine_explicit: false,
        }
    }
}

/// A value-taking `unit` flag (`--rev REV`, `--engine=NAME`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueFlag {
    Rev,
    Engine,
    Output,
    Target,
    ConduitUri,
}

/// A boolean `unit` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolFlag {
    Coverage,
    NoCoverage,
    DetailedCoverage,
    Json,
    Everything,
    Ugly,
}

/// Any `unit` flag, grouped so each store match stays small.
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

/// One `getArguments()` entry: canonical long name, short alias, and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlagSpec {
    long: &'static str,
    short: Option<char>,
    flag: Flag,
}

/// `unit` flags in `getArguments()` order
/// (`src/workflow/ArcanistUnitWorkflow.php:38-107`), plus the global
/// `--conduit-uri` this port threads through every command.
const FLAG_SPECS: &[FlagSpec] = &[
    FlagSpec {
        long: "rev",
        short: None,
        flag: Flag::Value(ValueFlag::Rev),
    },
    FlagSpec {
        long: "engine",
        short: None,
        flag: Flag::Value(ValueFlag::Engine),
    },
    FlagSpec {
        long: "coverage",
        short: None,
        flag: Flag::Bool(BoolFlag::Coverage),
    },
    FlagSpec {
        long: "no-coverage",
        short: None,
        flag: Flag::Bool(BoolFlag::NoCoverage),
    },
    FlagSpec {
        long: "detailed-coverage",
        short: None,
        flag: Flag::Bool(BoolFlag::DetailedCoverage),
    },
    FlagSpec {
        long: "json",
        short: None,
        flag: Flag::Bool(BoolFlag::Json),
    },
    FlagSpec {
        long: "output",
        short: None,
        flag: Flag::Value(ValueFlag::Output),
    },
    FlagSpec {
        long: "target",
        short: None,
        flag: Flag::Value(ValueFlag::Target),
    },
    FlagSpec {
        long: "everything",
        short: None,
        flag: Flag::Bool(BoolFlag::Everything),
    },
    FlagSpec {
        long: "ugly",
        short: None,
        flag: Flag::Bool(BoolFlag::Ugly),
    },
    FlagSpec {
        long: "conduit-uri",
        short: None,
        flag: Flag::Value(ValueFlag::ConduitUri),
    },
];

/// Find a spec by long name (`--engine`).
fn lookup_long(name: &str) -> Option<FlagSpec> {
    FLAG_SPECS.iter().find(|spec| spec.long == name).copied()
}

/// Find a spec by short spelling (`-x`).
///
/// PHP matches the whole remainder after `-` against the short map
/// (`ArcanistWorkflow.php:852-862`): no combining, no `-m=value`. `unit`
/// declares no shorts, so every `-x` is unknown (except `-h`, handled as
/// help before lookup).
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
        ValueFlag::Rev => options.rev = Some(value),
        ValueFlag::Engine => {
            options.engine = value;
            options.engine_explicit = true;
        }
        ValueFlag::Output => options.output = Some(value),
        ValueFlag::Target => options.target = Some(value),
        ValueFlag::ConduitUri => options.conduit_uri = Some(value),
    }
}

/// Store a parsed boolean flag in `options`.
fn store_bool_flag(options: &mut Options, flag: BoolFlag) {
    match flag {
        BoolFlag::Coverage => options.coverage = true,
        BoolFlag::NoCoverage => options.no_coverage = true,
        BoolFlag::DetailedCoverage => options.detailed_coverage = true,
        BoolFlag::Json => options.json = true,
        BoolFlag::Everything => options.everything = true,
        BoolFlag::Ugly => options.ugly = true,
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
        Flag::Bool(bool_flag) => store_bool_flag(options, bool_flag),
    }
}

/// Parse one `--name` or `--name=value` token.
///
/// Values may be empty and may look like flags (`--engine --json` takes
/// `--json` as the engine); only a trailing `param` flag errors
/// (`ArcanistWorkflow.php:867-883`). PHP's British-spelling corrector
/// (`:834-843`) can never fire here: no `unit` flag name has a
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

/// Parse `argv` (excluding the `unit` subcommand name).
///
/// Ports `ArcanistWorkflow::parseArguments()`
/// (`src/workflow/ArcanistWorkflow.php:788-926`): `--` ends flags (cargo
/// extension: the rest forwards to `cargo test` instead of collecting into
/// `paths`), positionals collect into `paths` (the `*` wildcard), and
/// conflicts are checked after the whole line parses. Returns `Ok(None)`
/// when `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut queue: VecDeque<String> = argv.iter().cloned().collect();
    let mut end_of_flags = false;
    while let Some(arg) = queue.pop_front() {
        if end_of_flags {
            options.cargo_args.push(arg);
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

/// Parse `argv` (excluding the `unit` subcommand name), compat wrapper.
///
/// Maps `parse_options` help (`None`) to `help: true`; all other outcomes
/// pass through. Everything after the first `--` forwards verbatim to
/// `cargo test`; only tokens before it are interpreted as `unit` flags.
pub fn parse_args(argv: &[String]) -> Result<Options, String> {
    match parse_options(argv)? {
        Some(opts) => Ok(opts),
        None => Ok(Options {
            help: true,
            ..Options::default()
        }),
    }
}

/// How a `conflicts` reason renders (`ArcanistWorkflow.php:905-909`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictReason {
    /// `null`: the message ends with a bare `.`.
    Bare,
    /// `pht(...)` text, rendering `: {text}`.
    Text(&'static str),
}

/// One flag's `conflicts` declaration, in declaration order.
struct ConflictDecl {
    flag: &'static str,
    conflicts: &'static [(&'static str, ConflictReason)],
}

/// `unit` conflict declarations, verbatim from `getArguments()`
/// (`src/workflow/ArcanistUnitWorkflow.php:60-62,82-85,97-99`).
const CONFLICTS: &[ConflictDecl] = &[
    ConflictDecl {
        flag: "coverage",
        conflicts: &[("no-coverage", ConflictReason::Bare)],
    },
    ConflictDecl {
        flag: "output",
        conflicts: &[
            (
                "json",
                ConflictReason::Text("Only one output format allowed"),
            ),
            (
                "ugly",
                ConflictReason::Text("Only one output format allowed"),
            ),
        ],
    },
    ConflictDecl {
        flag: "everything",
        conflicts: &[("rev", ConflictReason::Text("--everything runs all tests."))],
    },
];

/// Reject flag combinations PHP declares conflicting
/// (`ArcanistWorkflow.php:899-919`).
///
/// The outer order is first-provision order, so symmetric pairs name the
/// earlier flag first.
fn check_conflicts(provided: &[&'static str]) -> Result<(), String> {
    for key in provided {
        let Some(decl) = CONFLICTS.iter().find(|decl| decl.flag == *key) else {
            continue;
        };
        for (conflict, reason) in decl.conflicts {
            if provided.contains(conflict) {
                let suffix = match reason {
                    ConflictReason::Bare => ".".to_string(),
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

/// `unit` VCS-support declarations, verbatim from `getArguments()`
/// (`src/workflow/ArcanistUnitWorkflow.php:44-52`).
const SUPPORTS: &[SupportDecl] = &[SupportDecl {
    flag: "rev",
    systems: &["git", "hg"],
    nosupport: &[("svn", "Arc unit does not currently support --rev in SVN.")],
}];

/// Working-copy VCS, named like `getSourceControlSystemName()`.
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

/// Output format (`ArcanistUnitWorkflow.php:333-348`): `--ugly` wins, then
/// `--json`, then `--output` (`none`/`json`/`ugly`/`full`, unknown maps to
/// `full` like PHP `idx(..., 'full')`).
#[must_use]
pub fn output_format(opts: &Options) -> String {
    if opts.ugly {
        return "ugly".to_string();
    }
    if opts.json {
        return "json".to_string();
    }
    match opts.output.as_deref() {
        Some("none") => "none".to_string(),
        Some("json") => "json".to_string(),
        Some("ugly") => "ugly".to_string(),
        Some("full") => "full".to_string(),
        _ => "full".to_string(),
    }
}

/// Coverage flag (`ArcanistUnitWorkflow.php:161-168`): `None` means "default"
/// like PHP `null`; `--coverage` or `--detailed-coverage` (which implies
/// `--coverage`) enables, else `--no-coverage` disables.
#[must_use]
pub fn enable_coverage(opts: &Options) -> Option<bool> {
    if opts.coverage || opts.detailed_coverage {
        Some(true)
    } else if opts.no_coverage {
        Some(false)
    } else {
        None
    }
}

/// Harbormaster message type (`ArcanistUnitWorkflow.php:388-400`): `OKAY` and
/// `SKIP` map to `pass`, everything else to `fail`.
#[must_use]
pub fn harbormaster_type(overall: i32) -> &'static str {
    if overall == arc_rs::unit_engine::WORKFLOW_RESULT_OKAY
        || overall == arc_rs::unit_engine::WORKFLOW_RESULT_SKIP
    {
        "pass"
    } else {
        "fail"
    }
}

/// `--everything` plus explicit paths is a usage error
/// (`ArcanistUnitWorkflow.php:136-143`).
pub fn check_everything_paths(everything: bool, paths: &[String]) -> Result<(), String> {
    if everything && !paths.is_empty() {
        return Err(
            "You can not specify paths with --everything. The --everything flag runs every \
             test associated with a tracked file in the working copy."
                .to_string(),
        );
    }
    Ok(())
}

/// Run `git` with `args` in `cwd`, capturing output.
fn git_output(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

/// True when `cwd` is inside a git working tree.
#[must_use]
pub fn is_inside_git_repo(cwd: &Path) -> bool {
    let Ok(out) = git_output(cwd, &["rev-parse", "--is-inside-work-tree"]) else {
        return false;
    };
    out.status.success() && out.stdout.trim_ascii() == b"true"
}

/// Walk toward the filesystem root for a VCS marker dir.
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
    if find_dot_dir(cwd, ".svn").is_some() {
        return Some(Vcs::Svn);
    }
    None
}

/// Uncommitted paths excluding untracked (`getWorkingCopyStatus` minus the
/// commit range, `ArcanistWorkflow.php:1627-1660` with the default
/// `FLAG_UNTRACKED` mask). Parses `git status --porcelain -z`.
pub fn uncommitted_paths(cwd: &Path) -> Result<Vec<String>, String> {
    let out = git_output(cwd, &["status", "--porcelain", "-z"])?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut paths = Vec::new();
    for record in out.stdout.split(|b| *b == 0) {
        if record.len() < 4 {
            continue;
        }
        let status = &record[..2];
        if status == b"??" {
            continue;
        }
        let mut path = String::from_utf8_lossy(&record[3..]).into_owned();
        // Renames report as `old -> new`; keep the new path.
        if let Some(arrow) = path.find(" -> ") {
            path = path[arrow + 4..].to_string();
        }
        // Quoted paths (spaces, unicode) gain double quotes; strip them.
        if path.len() >= 2 && path.starts_with('"') && path.ends_with('"') {
            path = path[1..path.len() - 1].to_string();
        }
        if !path.is_empty() {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Resolve an explicit git base revision to its merge-base hash with `HEAD`
/// (`ArcanistGitAPI::buildBaseCommit`, explicit branch,
/// `ArcanistGitAPI.php:208-243`): the empty tree passes through, anything
/// else resolves via `git merge-base`, and unknown names fail with the PHP
/// usage message.
fn git_base_commit(cwd: &Path, rev: &str) -> Result<String, String> {
    arc_rs::git::base_commit_for_explicit_symbol(cwd, rev, "HEAD")
}

/// Paths changed between `rev` and `HEAD` (`getCommitRangeStatus` half of
/// `getWorkingCopyStatus`, `ArcanistRepositoryAPI.php:253-265`): the rev
/// resolves to its merge-base first (like PHP), then `git diff --name-only`
/// lists the range. `--name-only` without rename detection reports the same
/// path set as PHP's `--raw` plus `parseGitRawDiff`.
pub fn commit_range_paths(cwd: &Path, rev: &str) -> Result<Vec<String>, String> {
    let base = git_base_commit(cwd, rev)?;
    let out = git_output(cwd, &["diff", "--name-only", "-z", &base, "HEAD"])?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut paths: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Working-copy status paths: commit-range union uncommitted
/// (`ArcanistRepositoryAPI.php:253-265`), untracked excluded.
pub fn working_copy_paths(cwd: &Path, rev: Option<&str>) -> Result<Vec<String>, String> {
    let mut paths = uncommitted_paths(cwd)?;
    if let Some(rev) = rev {
        let mut ranged = commit_range_paths(cwd, rev)?;
        paths.append(&mut ranged);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Every tracked file (`getAllFiles`, `ArcanistGitAPI.php:932-936` via
/// `git ls-files -z`). Used for `--everything`
/// (`ArcanistUnitWorkflow.php:145-146`).
pub fn all_tracked_files(cwd: &Path) -> Result<Vec<String>, String> {
    let out = git_output(cwd, &["ls-files", "-z"])?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut paths: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    paths.sort();
    Ok(paths)
}

/// Escape an hg symbol for revset use (`hgsprintf` `%s`,
/// `src/xsprintf/hgsprintf.php:26-35`): alphanumeric-only symbols pass
/// through, anything else is single-quoted with `'` and `\` escaped.
#[must_use]
pub fn hg_symbol_escape(symbol: &str) -> String {
    let plain = !symbol.is_empty() && symbol.bytes().all(|b| b.is_ascii_alphanumeric());
    if plain {
        symbol.to_string()
    } else {
        format!("'{}'", symbol.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// `ancestor(<rev>,.)` revsets for an explicit hg base revision, quoted
/// first then raw (`ArcanistMercurialAPI::buildBaseCommit`,
/// `ArcanistMercurialAPI.php:82-98`).
#[must_use]
pub fn hg_base_revsets(rev: &str) -> [String; 2] {
    [
        format!("ancestor({},.)", hg_symbol_escape(rev)),
        format!("ancestor({rev},.)"),
    ]
}

/// Resolve an explicit hg base revision to a node hash: the quoted revset
/// first, then the raw revset, else the PHP usage error
/// (`ArcanistMercurialAPI.php:93-97`).
fn hg_base_commit_with(dir: &Path, hg: &HgCommand, rev: &str) -> Result<String, String> {
    for revset in hg_base_revsets(rev) {
        if let Ok(node) = arc_rs::hg::get_canonical_revision_name_with(dir, hg, &revset) {
            return Ok(node);
        }
    }
    Err(format!(
        "Commit '{rev}' is not a valid Mercurial commit identifier."
    ))
}

/// Parse `hg status` output into sorted paths, omitting untracked files
/// (the default `FLAG_UNTRACKED` omit mask,
/// `ArcanistWorkflow.php:1627-1629`). Clean/ignored entries carry zero
/// flags and pass through like PHP, but plain `hg status` never emits them.
fn hg_status_paths(stdout: &str) -> Result<Vec<String>, String> {
    let status = arc_rs::hg::parse_mercurial_status(stdout)?;
    Ok(status
        .iter()
        .filter(|(_, mask)| *mask & arc_rs::hg::FLAG_UNTRACKED == 0)
        .map(|(path, _)| path.clone())
        .collect())
}

/// Run `hg` with `args` in `dir`, mapping failures to `<command> failed`
/// errors carrying stderr, like the git helpers above.
fn hg_output_checked(
    dir: &Path,
    hg: &HgCommand,
    args: &[&str],
    command: &str,
) -> Result<String, String> {
    let out = arc_rs::hg::output_with(dir, hg, args)?;
    if !out.status.success() {
        return Err(format!(
            "{command} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Uncommitted hg paths (`hg status` minus untracked,
/// `ArcanistMercurialAPI.php:313-329`).
pub fn hg_uncommitted_paths_with(dir: &Path, hg: &HgCommand) -> Result<Vec<String>, String> {
    let stdout = hg_output_checked(dir, hg, &["status"], "hg status")?;
    hg_status_paths(&stdout)
}

/// hg commit-range paths (`hg status --rev <base> --rev tip`,
/// `ArcanistMercurialAPI.php:331-344`).
pub fn hg_commit_range_paths_with(
    dir: &Path,
    hg: &HgCommand,
    rev: &str,
) -> Result<Vec<String>, String> {
    let base = hg_base_commit_with(dir, hg, rev)?;
    let args = ["status", "--rev", base.as_str(), "--rev", "tip"];
    let stdout = hg_output_checked(dir, hg, &args, "hg status")?;
    hg_status_paths(&stdout)
}

/// hg working-copy status: commit-range union uncommitted, untracked
/// excluded (`ArcanistRepositoryAPI.php:253-265`).
pub fn hg_working_copy_paths_with(
    dir: &Path,
    hg: &HgCommand,
    rev: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut paths = hg_uncommitted_paths_with(dir, hg)?;
    if let Some(rev) = rev {
        paths.append(&mut hg_commit_range_paths_with(dir, hg, rev)?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Every tracked file (`hg manifest`,
/// `ArcanistMercurialAPI.php:268-272`).
pub fn hg_all_tracked_files_with(dir: &Path, hg: &HgCommand) -> Result<Vec<String>, String> {
    let stdout = hg_output_checked(dir, hg, &["manifest"], "hg manifest")?;
    let mut paths = arc_rs::hg::parse_manifest_lines(&stdout);
    paths.sort();
    Ok(paths)
}

/// Uncommitted svn paths (`svn --xml status` minus externals and untracked,
/// `ArcanistSubversionAPI.php:53-55,69-149` with the default
/// `FLAG_UNTRACKED` omit mask).
pub fn svn_uncommitted_paths_with(dir: &Path, svn: &SvnCommand) -> Result<Vec<String>, String> {
    let argv = arc_rs::svn::status_argv(&[]);
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = arc_rs::svn::output_with(dir, svn, &args)?;
    if !out.status.success() {
        return Err(format!(
            "svn --xml status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let parsed = arc_rs::svn::parse_svn_status_xml(&String::from_utf8_lossy(&out.stdout))?;
    let visible = arc_rs::svn::filter_externals(&parsed.files, false);
    Ok(visible
        .iter()
        .filter(|(_, mask)| *mask & arc_rs::svn::FLAG_UNTRACKED == 0)
        .map(|(path, _)| path.clone())
        .collect())
}

/// svn working-copy status: uncommitted only, since svn has no commit
/// ranges (`ArcanistSubversionAPI.php:47-51`). An explicit rev is the PHP
/// usage error (`ArcanistWorkflow.php:1831-1834`); the `unit` flag check
/// rejects `--rev` under svn before this runs.
pub fn svn_working_copy_paths_with(
    dir: &Path,
    svn: &SvnCommand,
    rev: Option<&str>,
) -> Result<Vec<String>, String> {
    if rev.is_some() {
        return Err("This version control system does not support commit ranges.".to_string());
    }
    svn_uncommitted_paths_with(dir, svn)
}

/// Every tracked file (`svn list -R` minus directories,
/// `ArcanistSubversionAPI.php:525-531,553-559`).
pub fn svn_all_tracked_files_with(dir: &Path, svn: &SvnCommand) -> Result<Vec<String>, String> {
    let out = arc_rs::svn::output_with(dir, svn, &["list", "-R"])?;
    if !out.status.success() {
        return Err(format!(
            "svn list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut paths: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(arc_rs::svn::filter_files)
        .collect();
    paths.sort();
    Ok(paths)
}

/// Validate explicit paths (`selectPathsForWorkflow`,
/// `ArcanistWorkflow.php:1631-1645`): each must exist, then is relativized
/// against the project root like `Filesystem::readablePath`.
pub fn validate_paths(
    paths: &[String],
    cwd: &Path,
    project_root: &Path,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for path in paths {
        let candidate = cwd.join(path);
        let full = if candidate.exists() {
            candidate
        } else {
            let absolute = PathBuf::from(path);
            if absolute.is_absolute() && absolute.exists() {
                absolute
            } else {
                return Err(format!("Path '{path}' does not exist!"));
            }
        };
        let display = full.to_string_lossy().replace('\\', "/");
        let root = project_root.to_string_lossy().replace('\\', "/");
        let root = root.trim_end_matches('/');
        let relative = display
            .strip_prefix(&format!("{root}/"))
            .unwrap_or(display.as_str())
            .to_string();
        out.push(relative);
    }
    Ok(out)
}

/// Select workflow paths with injectable `hg`/`svn` backends
/// (`ArcanistUnitWorkflow.php:136-149` plus `selectPathsForWorkflow`):
/// `--everything` lists every tracked file, else explicit paths validate,
/// else working-copy status (range plus uncommitted, untracked excluded).
/// Without a detectable repo an empty selection passes through and the
/// engine reports no tests. `hg`/`svn` run at the project root like PHP
/// (`execxLocal` uses the project path).
pub fn select_paths_with(
    opts: &Options,
    cwd: &Path,
    project_root: &Path,
    hg: &HgCommand,
    svn: &SvnCommand,
) -> Result<Vec<String>, String> {
    check_everything_paths(opts.everything, &opts.paths)?;
    if opts.everything {
        let Some(vcs) = detect_vcs(cwd) else {
            return Ok(Vec::new());
        };
        return match vcs {
            Vcs::Git => all_tracked_files(cwd),
            Vcs::Hg => hg_all_tracked_files_with(project_root, hg),
            Vcs::Svn => svn_all_tracked_files_with(project_root, svn),
        };
    }
    if !opts.paths.is_empty() {
        return validate_paths(&opts.paths, cwd, project_root);
    }
    let Some(vcs) = detect_vcs(cwd) else {
        return Ok(Vec::new());
    };
    match vcs {
        Vcs::Git => working_copy_paths(cwd, opts.rev.as_deref()),
        Vcs::Hg => hg_working_copy_paths_with(project_root, hg, opts.rev.as_deref()),
        Vcs::Svn => svn_working_copy_paths_with(project_root, svn, opts.rev.as_deref()),
    }
}

/// Select workflow paths with the system `hg`/`svn` binaries.
pub fn select_paths(
    opts: &Options,
    cwd: &Path,
    project_root: &Path,
) -> Result<Vec<String>, String> {
    select_paths_with(
        opts,
        cwd,
        project_root,
        &HgCommand::system(),
        &SvnCommand::system(),
    )
}

/// Walk up from `dir` looking for `Cargo.toml`; returns the directory that
/// contains it, if any.
pub fn find_crate_root_from(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find_map(|dir| {
        if dir.join("Cargo.toml").is_file() {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

/// Walk up from `dir` looking for `.arcunit`; returns the directory that
/// contains it, if any (`ArcanistWorkflow.php:2116` checks
/// `getProjectPath('.arcunit')`).
#[must_use]
pub fn find_arcunit_from(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find_map(|dir| {
        if dir.join(".arcunit").is_file() {
            Some(dir.to_path_buf())
        } else {
            None
        }
    })
}

/// Read the `unit.engine` key (`ArcanistWorkflow.php:2112`
/// `getConfigFromAnySource('unit.engine')`) from the nearest `.arcconfig`
/// walking up from `dir`.
#[must_use]
pub fn find_unit_engine_from(dir: &Path) -> Option<String> {
    for ancestor in dir.ancestors() {
        let Ok(text) = std::fs::read_to_string(ancestor.join(".arcconfig")) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(name) = value.get("unit.engine").and_then(|v| v.as_str())
            && !name.is_empty()
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Aggregate report for one engine run.
#[derive(Debug, Clone)]
pub struct EngineReport {
    pub engine: String,
    pub passed: u32,
    pub failed: u32,
    pub duration_ms: u64,
    pub success: bool,
    pub tail: String,
    pub results: Vec<UnitResult>,
}

impl EngineReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": self.engine,
            "passed": self.passed,
            "failed": self.failed,
            "duration_ms": self.duration_ms,
        })
    }

    pub fn summary_line(&self) -> String {
        let status = if self.success { "PASS" } else { "FAIL" };
        format!(
            "{} {} ({} passed, {} failed, {} ms)",
            status, self.engine, self.passed, self.failed, self.duration_ms
        )
    }
}

/// Aggregate report for a configuration-driven or single-engine run over
/// per-test results.
#[derive(Debug, Clone)]
pub struct ConfigReport {
    pub engine: String,
    pub passed: u32,
    pub failed: u32,
    pub duration_ms: u64,
    pub success: bool,
    pub results: Vec<EngineUnitResult>,
}

impl ConfigReport {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": self.engine,
            "passed": self.passed,
            "failed": self.failed,
            "duration_ms": self.duration_ms,
        })
    }
}

/// Last `n` lines of `text` (without a trailing partial-line artifact).
pub fn tail(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// `cargo test` arguments for an options set (Rust extension; PHP has no
/// cargo engine): the cargo-test args after the unit `--` separator first,
/// then the positional unit paths as test-name filters. Filters after a
/// `--` in the cargo args still filter: the test binary treats its first
/// positional the same way.
#[must_use]
pub fn cargo_test_argv(opts: &Options) -> Vec<String> {
    let mut argv = vec!["test".to_string()];
    argv.extend(opts.cargo_args.iter().cloned());
    argv.extend(opts.paths.iter().cloned());
    argv
}

/// Coverage warning for the cargo engine (Rust extension): cargo has no
/// coverage support, so an explicit `--coverage` (or `--detailed-coverage`,
/// which implies it) warns on stderr and the run continues without
/// coverage. `--no-coverage` needs no warning: it is already the effective
/// behavior.
#[must_use]
pub fn cargo_coverage_warning(opts: &Options) -> Option<String> {
    if opts.detailed_coverage {
        Some(
            "arc-rs unit: warning: the cargo engine does not support coverage; \
             ignoring --detailed-coverage"
                .to_string(),
        )
    } else if opts.coverage {
        Some(
            "arc-rs unit: warning: the cargo engine does not support coverage; \
             ignoring --coverage"
                .to_string(),
        )
    } else {
        None
    }
}

/// Run the configured engine in the crate found by walking up from `cwd`.
///
/// `target_dir` overrides `CARGO_TARGET_DIR` for the child process only;
/// pass `None` for normal runs (cargo default) and `Some` in tests to keep
/// build output out of the repo. The child always runs with
/// `CARGO_NET_OFFLINE=true`.
///
/// Cargo-extension behavior: positional paths become `cargo test` filters
/// (see [`cargo_test_argv`]), `--everything` is the default full-suite run,
/// `--rev` is ignored, and coverage flags warn (see
/// [`cargo_coverage_warning`]).
///
/// Per-test parsing is out of scope: `passed`/`failed` are 1/0 or 0/1 based
/// on the `cargo test` exit status.
pub fn execute(
    opts: &Options,
    cwd: &Path,
    target_dir: Option<&Path>,
) -> Result<EngineReport, String> {
    if !AVAILABLE_ENGINES.contains(&opts.engine.as_str()) {
        return Err(format!(
            "unknown engine '{}' (available engines: {})",
            opts.engine,
            AVAILABLE_ENGINES.join(", ")
        ));
    }
    if opts.engine != "cargo" {
        return Err(format!(
            "engine '{}' is not the cargo engine (execute() runs cargo only)",
            opts.engine
        ));
    }
    let Some(root) = find_crate_root_from(cwd) else {
        return Err(format!(
            "no Cargo.toml found walking up from '{}'",
            cwd.display()
        ));
    };

    let mut cmd = std::process::Command::new("cargo");
    cmd.args(cargo_test_argv(opts))
        .current_dir(&root)
        .env("CARGO_NET_OFFLINE", "true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = target_dir {
        cmd.env("CARGO_TARGET_DIR", dir);
    }
    let started = std::time::Instant::now();
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run 'cargo test': {e}"))?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);

    let success = output.status.success();
    let (passed, failed) = if success { (1, 0) } else { (0, 1) };
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let results = vec![UnitResult::new(
        &opts.engine,
        Outcome::from_pass(success),
        duration_ms as f64 / 1000.0,
    )];
    Ok(EngineReport {
        engine: opts.engine.clone(),
        passed,
        failed,
        duration_ms,
        success,
        tail: tail(&combined, TAIL_LINES),
        results,
    })
}

/// Engine selection mirroring `newUnitTestEngine`
/// (`ArcanistWorkflow.php:2107-2147`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Default `cargo` run (PHP throws `ArcanistNoEngineException` here).
    Cargo,
    /// One engine named by `--engine` or `unit.engine`.
    Single(String),
    /// Configuration-driven run over `.arcunit` engines.
    Config {
        engines: Vec<PlannedEngine>,
        root: PathBuf,
    },
}

fn validate_engine(engine: &str) -> Result<(), String> {
    if !AVAILABLE_ENGINES.contains(&engine) {
        return Err(format!(
            "unknown engine '{engine}' (available engines: {})",
            AVAILABLE_ENGINES.join(", ")
        ));
    }
    Ok(())
}

fn load_config_selection(cwd: &Path) -> Result<Selection, String> {
    let Some(dir) = find_arcunit_from(cwd) else {
        return Err(
            "Unable to find '.arcunit' file to configure test engines. Create an \
            '.arcunit' file in the root directory of the working copy."
                .to_string(),
        );
    };
    let path = dir.join(".arcunit");
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let display = path.to_string_lossy().to_string();
    let engines = unit_config::parse_engine_specs(&data, &display, CONFIG_AVAILABLE)
        .map_err(|e| e.message().to_string())?;
    Ok(Selection::Config { engines, root: dir })
}

/// Select engines: explicit `--engine`, then `unit.engine` from `.arcconfig`,
/// then `.arcunit`, else the `cargo` default.
pub fn discover_selection(opts: &Options, cwd: &Path) -> Result<Selection, String> {
    if opts.engine_explicit {
        validate_engine(&opts.engine)?;
        if opts.engine == "cargo" {
            return Ok(Selection::Cargo);
        }
        if opts.engine == "ArcanistConfigurationDrivenUnitTestEngine" {
            return load_config_selection(cwd);
        }
        return Ok(Selection::Single(opts.engine.clone()));
    }
    if let Some(class) = find_unit_engine_from(cwd) {
        validate_engine(&class)?;
        if class == "cargo" {
            return Ok(Selection::Cargo);
        }
        if class == "ArcanistConfigurationDrivenUnitTestEngine" {
            return load_config_selection(cwd);
        }
        return Ok(Selection::Single(class));
    }
    if find_arcunit_from(cwd).is_some() {
        return load_config_selection(cwd);
    }
    Ok(Selection::Cargo)
}

/// Whether an engine supports `--everything`: nose
/// (`NoseTestEngine.php:12-14`), xunit (`XUnitTestEngine.php:24-26`),
/// phutil (`PhutilUnitTestEngine.php:12-14`) and csharp (inherits xunit)
/// return true; pytest and phpunit inherit the false default
/// (`ArcanistUnitTestEngine.php:38-40`).
#[must_use]
pub fn supports_run_all_tests(engine_type: &str) -> bool {
    matches!(
        engine_type,
        "NoseTestEngine"
            | "XUnitTestEngine"
            | "CSharpToolsTestEngine"
            | "PhutilUnitTestEngine"
            | "phutil"
    )
}

/// Real command runner: spawn `argv` with `cwd`, capturing output.
struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, argv: &[String], cwd: &str) -> arc_rs::unit_engine::CapturedOutput {
        if argv.is_empty() {
            return arc_rs::unit_engine::CapturedOutput::new(1, "", "empty command");
        }
        match std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .output()
        {
            Ok(output) => arc_rs::unit_engine::CapturedOutput::new(
                output.status.code().unwrap_or(1),
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            ),
            Err(e) => arc_rs::unit_engine::CapturedOutput::new(
                1,
                "",
                &format!("failed to run '{}': {e}", argv.join(" ")),
            ),
        }
    }
}

/// Real-filesystem probes for the pytest engine.
struct RealPytestFs;

impl unit_pytest::PytestFs for RealPytestFs {
    fn path_exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn is_file(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.lines().count().max(1))
    }
}

/// Real-filesystem probes for the phpunit engine.
struct RealPhpunitFs;

impl unit_phpunit::PhpunitFs for RealPhpunitFs {
    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn path_exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn line_count(&self, path: &str) -> Option<usize> {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.lines().count().max(1))
    }
}

/// Search `PATH` for `name` (plus `.exe` on Windows), mirroring
/// `Filesystem::binaryExists`.
#[must_use]
pub fn binary_exists(name: &str) -> bool {
    resolve_binary_path(name).is_some()
}

/// Resolve `name` via `PATH`, mirroring `Filesystem::resolveBinary`.
/// Returns the full path when found, else `name` unchanged.
#[must_use]
pub fn resolve_binary_path(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".bat", ".cmd"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_var) {
        for ext in extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    None
}

/// Read `.arcconfig` JSON from `project_root`, if present and valid.
#[must_use]
pub fn read_arcconfig(project_root: &str) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(format!("{project_root}/.arcconfig")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Fetch one `.arcconfig` string key (`getConfigFromAnySource` subset for
/// unit engines; walks only the project root, not user/global configs).
#[must_use]
pub fn arcconfig_string(project_root: &str, key: &str) -> Option<String> {
    read_arcconfig(project_root)?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Parse `unit.csharp.discovery` (`XUnitTestEngine.php:64-74`): map of regex
/// to list of `[project, assembly]` pairs.
#[must_use]
pub fn parse_discovery_rules(value: &serde_json::Value) -> Option<Vec<unit_xunit::DiscoveryRule>> {
    let obj = value.as_object()?;
    let mut rules = Vec::new();
    for (regex, targets) in obj {
        let arr = targets.as_array()?;
        let mut parsed = Vec::new();
        for target in arr {
            let pair = target.as_array()?;
            if pair.len() < 2 {
                return None;
            }
            let project = pair[0].as_str()?.to_string();
            let assembly = pair[1].as_str()?.to_string();
            parsed.push(unit_xunit::DiscoveryTarget { project, assembly });
        }
        rules.push(unit_xunit::DiscoveryRule {
            regex: regex.clone(),
            targets: parsed,
        });
    }
    Some(rules)
}

/// Every file under `root` (relative, forward slashes), mirroring
/// `FileFinder($projectRoot)->find()` for `--everything`
/// (`XUnitTestEngine.php:107`).
#[must_use]
pub fn all_files_under(root: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_files_under(Path::new(root), Path::new(root), &mut out);
    out.sort();
    out
}

fn collect_files_under(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.sort();
    for path in names {
        if path.is_dir() {
            collect_files_under(base, &path, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Random 10-char hex stem for xUnit report names, standing in for
/// `Filesystem::readRandomCharacters(10)` (`XUnitTestEngine.php:326`).
fn random_xml_stem() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}{:x}", std::process::id(), nanos, n)
        .chars()
        .take(10)
        .collect::<String>()
        .chars()
        .chain(std::iter::repeat('0'))
        .take(10)
        .collect()
}

/// xUnit environment plus the test projects selected for a run: the
/// shared half of the xUnit and C# engines (the C# engine inherits the
/// parent stages and only overrides the test future).
pub struct XunitPlan {
    /// Loaded build/runtime/test engines, root, and discovery rules.
    pub env: unit_xunit::XunitEnvironment,
    /// Test projects mapped from the run paths.
    pub projects: Vec<unit_xunit::TestProject>,
    /// Host platform flag for report paths.
    pub is_windows: bool,
}

/// Load the xUnit environment and map the run paths to test projects
/// (`XUnitTestEngine.php:36-95,103-113`) with real filesystem probes.
pub fn prepare_xunit_run(ctx: &EngineContext) -> Result<XunitPlan, EngineError> {
    let is_windows = cfg!(windows);
    let arcconfig = read_arcconfig(&ctx.project_root);
    let discovery = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_xunit::DISCOVERY_KEY))
        .and_then(parse_discovery_rules);
    let hint = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_xunit::XUNIT_BINARY_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let env = unit_xunit::load_environment(
        &ctx.project_root,
        is_windows,
        &binary_exists,
        &|b| resolve_binary_path(b).unwrap_or_else(|| b.to_string()),
        discovery,
        hint.as_deref(),
        &|p| Path::new(p).exists(),
    )?;
    let paths = if ctx.run_all_tests {
        all_files_under(&ctx.project_root)
    } else {
        ctx.paths.clone()
    };
    let projects = unit_xunit::map_paths_to_results(&paths, &env, &|p| Path::new(p).exists());
    Ok(XunitPlan {
        env,
        projects,
        is_windows,
    })
}

/// Run generate, build, then test over a plan, stopping after the first
/// failing stage (`runAllTests`, `XUnitTestEngine.php:167-184`).
fn run_xunit_plan(
    plan: &XunitPlan,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let mut results = run_xunit_generate(&plan.env, runner)?;
    if unit_xunit::results_contain_failures(&results) {
        return Ok(results);
    }
    let mut built = run_xunit_build(&plan.env, &plan.projects, runner)?;
    results.append(&mut built);
    if unit_xunit::results_contain_failures(&results) {
        return Ok(results);
    }
    let mut tested = run_xunit_test(&plan.env, &plan.projects, runner, plan.is_windows)?;
    results.append(&mut tested);
    Ok(results)
}

/// Run the xUnit engine (`XUnitTestEngine.php:103-113,167-184`) with real
/// filesystem probes and the injected command runner.
pub fn run_xunit_with_runner(
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let plan = prepare_xunit_run(ctx)?;
    if plan.projects.is_empty() {
        return Ok(Vec::new());
    }
    run_xunit_plan(&plan, runner)
}

/// xUnit generate stage (`generateProjects`, lines 215-264).
fn run_xunit_generate(
    env: &unit_xunit::XunitEnvironment,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let build_dir = Path::new(&env.project_root).join("Build");
    let proto = Path::new(&env.project_root).join("Protobuild.exe");
    if !unit_xunit::should_generate(build_dir.is_dir(), proto.is_file()) {
        return Ok(Vec::new());
    }
    let files: Vec<String> = std::fs::read_dir(&env.project_root)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let is_windows = cfg!(windows);
    let platform = unit_xunit::detect_platform(is_windows, &refs);
    let argv = unit_xunit::build_regenerate_argv(&env.runtime_engine, &platform);
    let started = std::time::Instant::now();
    let output = runner.run(&argv, &env.project_root);
    let name = unit_xunit::regenerate_result_name(&platform);
    let duration = started.elapsed().as_secs_f64();
    match unit_xunit::stage_outcome(output.exit_code) {
        unit_xunit::StageOutcome::Pass | unit_xunit::StageOutcome::Fail => {
            unit_xunit::stage_result(&name, output.exit_code, &output.stdout, duration)
                .map(|r| vec![r])
                .map_err(EngineError::Failed)
        }
        unit_xunit::StageOutcome::HardError => Err(EngineError::Failed(format!(
            "command '{}' failed with exit code {}: {}",
            argv.join(" "),
            output.exit_code,
            output.stderr.trim()
        ))),
    }
}

/// xUnit build stage (`buildProjects`, lines 277-312), sequential.
fn run_xunit_build(
    env: &unit_xunit::XunitEnvironment,
    projects: &[unit_xunit::TestProject],
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let mut results = Vec::new();
    for project in projects {
        let argv = unit_xunit::build_build_argv(&env.build_engine);
        let cwd = Path::new(&project.project)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| env.project_root.clone());
        let started = std::time::Instant::now();
        let output = runner.run(&argv, &cwd);
        let name = unit_xunit::build_result_name(&project.project);
        let duration = started.elapsed().as_secs_f64();
        match unit_xunit::stage_outcome(output.exit_code) {
            unit_xunit::StageOutcome::Pass | unit_xunit::StageOutcome::Fail => {
                let result =
                    unit_xunit::stage_result(&name, output.exit_code, &output.stdout, duration)
                        .map_err(EngineError::Failed)?;
                results.push(result);
            }
            unit_xunit::StageOutcome::HardError => {
                return Err(EngineError::Failed(format!(
                    "command '{}' failed with exit code {}: {}",
                    argv.join(" "),
                    output.exit_code,
                    output.stderr.trim()
                )));
            }
        }
    }
    Ok(results)
}

/// xUnit test stage (`testAssemblies`, lines 351-391), sequential. Missing
/// XML reports contribute no results (Mono segfault tolerance, lines
/// 372-387).
fn run_xunit_test(
    env: &unit_xunit::XunitEnvironment,
    projects: &[unit_xunit::TestProject],
    runner: &dyn CommandRunner,
    is_windows: bool,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let mut merged = Vec::new();
    for project in projects {
        let xml_name = unit_xunit::xml_report_name(&random_xml_stem());
        let argv = unit_xunit::build_test_argv(
            &env.runtime_engine,
            &env.test_engine,
            &project.assembly,
            &xml_name,
        );
        runner.run(&argv, &env.project_root);
        let output = unit_xunit::xunit_output_path(&env.project_root, &xml_name, is_windows);
        if !unit_xunit::should_parse_output(Path::new(&output).exists()) {
            continue;
        }
        let xml = std::fs::read_to_string(&output)
            .map_err(|e| EngineError::Failed(format!("cannot read '{output}': {e}")))?;
        let mut results =
            unit_xunit::parse_assembly_results(&xml, None).map_err(EngineError::Failed)?;
        merged.append(&mut results);
        std::fs::remove_file(&output).ok();
    }
    Ok(merged)
}

/// C# cover configuration: resolved cscover engine plus the
/// instrumentation filters (`CSharpToolsTestEngine.php:22-55`).
#[derive(Debug)]
struct CsharpCover {
    cover_engine: String,
    match_regex: Option<String>,
    excluded: Option<HashMap<String, String>>,
}

/// Load the C# cover configuration from `.arcconfig`: the cscover hint
/// plus the match/excluded filters. `None` when coverage is disabled
/// (then every assembly runs the plain xUnit future).
fn load_csharp_cover(ctx: &EngineContext) -> Result<Option<CsharpCover>, EngineError> {
    let arcconfig = read_arcconfig(&ctx.project_root);
    let hint = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_csharp::CSCOVER_BINARY_KEY))
        .and_then(|v| v.as_str());
    let cover_engine =
        unit_csharp::load_cover_engine(&ctx.project_root, hint, ctx.enable_coverage, &|p| {
            Path::new(p).exists()
        })?;
    let Some(cover_engine) = cover_engine else {
        return Ok(None);
    };
    let match_regex = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_csharp::COVERAGE_MATCH_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let excluded = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_csharp::COVERAGE_EXCLUDED_KEY))
        .and_then(parse_excluded_files);
    Ok(Some(CsharpCover {
        cover_engine,
        match_regex,
        excluded,
    }))
}

/// Parse `unit.csharp.coverage.excluded` (a map; PHP checks key existence,
/// so only keys matter and non-string values become empty).
fn parse_excluded_files(value: &serde_json::Value) -> Option<HashMap<String, String>> {
    let obj = value.as_object()?;
    Some(
        obj.iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
            .collect(),
    )
}

/// One planned C# test run: the argv plus the report files it produces
/// (`buildTestFuture`, `CSharpToolsTestEngine.php:88-143`).
struct CsharpRun {
    argv: Vec<String>,
    xunit_output: String,
    cover_output: Option<String>,
}

/// Parent directory of an assembly path (forward slashes), defaulting to
/// the project root for bare names.
fn assembly_parent_dir(assembly: &str, project_root: &str) -> String {
    let parent = Path::new(assembly)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if parent.is_empty() {
        project_root.to_string()
    } else {
        parent
    }
}

/// Bare file names directly under `dir`, sorted for a deterministic
/// assembly scan.
fn dir_file_names(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Assemblies in `assembly_dir` selected for instrumentation (`.dll`/`.exe`
/// passing the excluded/match filters, lines 117-125).
fn instrumentable_assemblies(
    assembly_dir: &str,
    match_regex: &Option<String>,
    excluded: Option<&HashMap<String, String>>,
) -> Vec<String> {
    let files = dir_file_names(assembly_dir);
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    unit_csharp::collect_assemblies(assembly_dir, &refs, excluded, match_regex.as_deref())
}

/// Stage an empty cscover output file next to the project root (unique
/// stem). PHP uses a system `TempFile`; we stage next to the destination.
fn stage_cover_file(project_root: &str) -> Result<String, EngineError> {
    let path = format!("{project_root}/.cscover-{}.xml", random_xml_stem());
    std::fs::write(&path, "")
        .map_err(|e| EngineError::Failed(format!("cannot stage cover file '{path}': {e}")))?;
    Ok(path)
}

/// Remove a stale report file, if any (best-effort).
fn remove_report_file(path: &str) {
    std::fs::remove_file(path).ok();
}

/// Plan one assembly's test run: with coverage and at least one
/// instrumentable assembly, wrap xUnit in cscover (`-o/-c/-a/-w`);
/// otherwise run the plain xUnit future (lines 89-91, 126-128).
fn plan_csharp_assembly(
    env: &unit_xunit::XunitEnvironment,
    assembly: &str,
    cover: Option<&CsharpCover>,
    enable_coverage: Option<bool>,
    is_windows: bool,
) -> Result<CsharpRun, EngineError> {
    let xml_name = unit_xunit::xml_report_name(&random_xml_stem());
    let (xunit_cmd, xunit_args) =
        unit_csharp::xunit_cmd_and_args(&env.runtime_engine, &env.test_engine, assembly, &xml_name);
    let assembly_dir = assembly_parent_dir(assembly, &env.project_root);
    let assemblies = match cover {
        Some(cover) => {
            instrumentable_assemblies(&assembly_dir, &cover.match_regex, cover.excluded.as_ref())
        }
        None => Vec::new(),
    };
    if let Some(cover) = cover
        && unit_csharp::use_cover_engine(enable_coverage, &assemblies)
    {
        let cover_output = stage_cover_file(&env.project_root)?;
        let argv = unit_csharp::build_cscover_argv(
            &env.runtime_engine,
            &cover.cover_engine,
            &cover_output,
            &xunit_cmd,
            &xunit_args,
            &assembly_dir,
            &assemblies,
        );
        // cscover runs xUnit with `-w <assembly_dir>`, so the report lands
        // next to the assemblies.
        let xunit_output = format!("{assembly_dir}/{xml_name}");
        remove_report_file(&xunit_output);
        return Ok(CsharpRun {
            argv,
            xunit_output,
            cover_output: Some(cover_output),
        });
    }
    let argv =
        unit_xunit::build_test_argv(&env.runtime_engine, &env.test_engine, assembly, &xml_name);
    let xunit_output = unit_xunit::xunit_output_path(&env.project_root, &xml_name, is_windows);
    remove_report_file(&xunit_output);
    Ok(CsharpRun {
        argv,
        xunit_output,
        cover_output: None,
    })
}

/// Read one run's cscover report through the cache (`readCoverage`,
/// lines 216-285). Line counts come from the real filesystem, matching
/// PHP `count(file(...))`.
fn read_run_coverage(
    cover_output: Option<&str>,
    project_root: &str,
    cache: &mut unit_csharp::CoverageCache,
) -> Result<Option<HashMap<String, String>>, EngineError> {
    let Some(cover_file) = cover_output else {
        return Ok(None);
    };
    let xml = std::fs::read_to_string(cover_file)
        .map_err(|e| EngineError::Failed(format!("cannot read '{cover_file}': {e}")))?;
    cache
        .read_cached(cover_file, &xml, project_root, &|p| {
            std::fs::read_to_string(p)
                .ok()
                .map(|text| text.lines().count())
        })
        .map(Some)
}

/// Execute one planned C# run and parse its results. A missing xUnit
/// report contributes nothing (Mono segfault tolerance, like the parent
/// stage); staged files are removed best-effort either way.
fn execute_csharp_run(
    run: &CsharpRun,
    project_root: &str,
    runner: &dyn CommandRunner,
    cache: &mut unit_csharp::CoverageCache,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    runner.run(&run.argv, project_root);
    if !Path::new(&run.xunit_output).exists() {
        if let Some(cover) = run.cover_output.as_deref() {
            remove_report_file(cover);
        }
        return Ok(Vec::new());
    }
    let xml = std::fs::read_to_string(&run.xunit_output)
        .map_err(|e| EngineError::Failed(format!("cannot read '{}': {e}", run.xunit_output)))?;
    let coverage = read_run_coverage(run.cover_output.as_deref(), project_root, cache)?;
    let results = unit_csharp::parse_assembly_results(&xml, coverage.as_ref())?;
    remove_report_file(&run.xunit_output);
    if let Some(cover) = run.cover_output.as_deref() {
        remove_report_file(cover);
    }
    Ok(results)
}

/// C# test stage over all projects (`testAssemblies` with the overridden
/// future, `XUnitTestEngine.php:351-391`).
fn run_csharp_test(
    plan: &XunitPlan,
    cover: Option<&CsharpCover>,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let mut cache = unit_csharp::CoverageCache::new();
    let mut merged = Vec::new();
    for project in &plan.projects {
        let run = plan_csharp_assembly(
            &plan.env,
            &project.assembly,
            cover,
            enable_coverage,
            plan.is_windows,
        )?;
        let mut results = execute_csharp_run(&run, &plan.env.project_root, runner, &mut cache)?;
        merged.append(&mut results);
    }
    Ok(merged)
}

/// C# tools run: shared xUnit stages plus the cscover test future when
/// coverage applies, else the plain xUnit path
/// (`CSharpToolsTestEngine.php:22-55,88-143,152-157,216-285`).
pub fn run_csharp_with_runner(
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let plan = prepare_xunit_run(ctx)?;
    // The cover engine resolves before any stage runs (like the parent
    // `loadEnvironment` plus the cover half throwing first), even when no
    // projects mapped.
    let cover = load_csharp_cover(ctx)?;
    if plan.projects.is_empty() {
        return Ok(Vec::new());
    }
    let mut results = run_xunit_generate(&plan.env, runner)?;
    if unit_xunit::results_contain_failures(&results) {
        return Ok(results);
    }
    let mut built = run_xunit_build(&plan.env, &plan.projects, runner)?;
    results.append(&mut built);
    if unit_xunit::results_contain_failures(&results) {
        return Ok(results);
    }
    let mut tested = run_csharp_test(&plan, cover.as_ref(), ctx.enable_coverage, runner)?;
    results.append(&mut tested);
    Ok(results)
}

/// Build a phpunit engine from `.arcconfig` (`prepareConfigFile`,
/// `PhpunitTestEngine.php:254-278`): `phpunit_config` plus
/// `unit.phpunit.binary`.
#[must_use]
pub fn phpunit_engine_from_config(project_root: &str) -> unit_phpunit::PhpunitEngine {
    let arcconfig = read_arcconfig(project_root);
    let config_value = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_phpunit::CONFIG_KEY))
        .and_then(|v| v.as_str());
    let config_file = match config_value {
        None => None,
        Some(value) => {
            let full = format!("{project_root}/{value}");
            if Path::new(&full).exists() {
                Some(full)
            } else {
                // Defer the "not found" error to run time via the engine's
                // resolver path: store nothing here and let `run_with` use
                // defaults; the explicit resolver is tested separately.
                None
            }
        }
    };
    let configured = arcconfig
        .as_ref()
        .and_then(|v| v.get(unit_phpunit::BINARY_KEY))
        .and_then(|v| v.as_str());
    let binary = match configured {
        None => unit_phpunit::PHPUNIT_BINARY.to_string(),
        Some(bin) if binary_exists(bin) => bin.to_string(),
        Some(bin) => format!("{project_root}/{bin}"),
    };
    unit_phpunit::PhpunitEngine {
        binary,
        config_file,
    }
}

/// Staged phutil driver files (all next to the project root, one stem).
struct PhutilStaging {
    driver: String,
    input: String,
    output: String,
}

/// Stage the driver script plus its JSON input next to the project root.
fn stage_phutil_files(project_root: &str, input_json: &str) -> Result<PhutilStaging, EngineError> {
    let stem = random_xml_stem();
    let driver = format!("{project_root}/.arc-unit-phutil-driver-{stem}.php");
    let input = format!("{project_root}/.arc-unit-phutil-input-{stem}.json");
    let output = format!("{project_root}/.arc-unit-phutil-output-{stem}.json");
    std::fs::write(&driver, unit_phutil::phutil_driver_source())
        .map_err(|e| EngineError::Failed(format!("cannot stage phutil driver '{driver}': {e}")))?;
    std::fs::write(&input, input_json)
        .map_err(|e| EngineError::Failed(format!("cannot stage phutil input '{input}': {e}")))?;
    Ok(PhutilStaging {
        driver,
        input,
        output,
    })
}

/// Remove staged phutil files (best-effort).
fn cleanup_phutil_files(staging: &PhutilStaging) {
    remove_report_file(&staging.driver);
    remove_report_file(&staging.input);
    remove_report_file(&staging.output);
}

/// Failure message for a nonzero driver exit: stderr when present, else
/// stdout, else the bare exit code.
fn phutil_failure(outcome: &arc_rs::unit_engine::CapturedOutput) -> EngineError {
    let stderr = outcome.stderr.trim();
    if !stderr.is_empty() {
        return EngineError::Failed(format!("phutil driver failed: {stderr}"));
    }
    let stdout = outcome.stdout.trim();
    if !stdout.is_empty() {
        return EngineError::Failed(format!("phutil driver failed: {stdout}"));
    }
    EngineError::Failed(format!(
        "phutil driver failed with exit code {}",
        outcome.exit_code
    ))
}

/// Default `php` invocation prefix: the interpreter on the real `PATH`,
/// if any (anoid's `python3` pattern).
fn resolve_php_prefix() -> Option<Vec<String>> {
    unit_phutil::php_argv_prefix()
}

/// Run the phutil engine out-of-process: stage the driver plus its JSON
/// input next to the project root, run `php` through `runner`, and parse
/// the JSON output. `php` resolves the interpreter prefix argv; `None` is
/// the clear php-absent error.
pub fn run_phutil_with_runner(
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
    engine: &str,
    php: &dyn Fn() -> Option<Vec<String>>,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let Some(prefix) = php() else {
        return Err(EngineError::Usage(unit_phutil::missing_php_message(engine)));
    };
    let init_script = read_arcconfig(&ctx.project_root)
        .as_ref()
        .and_then(|v| v.get(unit_phutil::INIT_SCRIPT_KEY))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let input = unit_phutil::PhutilRunInput {
        project_root: ctx.project_root.clone(),
        run_all_tests: ctx.run_all_tests,
        paths: ctx.paths.clone(),
        enable_coverage: ctx.enable_coverage,
        init_script,
    };
    let staging = stage_phutil_files(&ctx.project_root, &unit_phutil::phutil_input_json(&input))?;
    let argv =
        unit_phutil::build_php_argv(&prefix, &staging.driver, &staging.input, &staging.output);
    let outcome = runner.run(&argv, &ctx.project_root);
    if outcome.exit_code != 0 {
        cleanup_phutil_files(&staging);
        return Err(phutil_failure(&outcome));
    }
    let text = std::fs::read_to_string(&staging.output).map_err(|e| {
        EngineError::Failed(format!(
            "phutil driver did not write '{}': {e}",
            staging.output
        ))
    });
    cleanup_phutil_files(&staging);
    unit_phutil::parse_phutil_output(&text?, &staging.output)
}

/// Run one engine by `.arcunit` type or `--engine` class name.
///
/// Every command goes through `runner` so tests inject canned outputs.
/// The phutil engine resolves `php` on the real `PATH`; see
/// [`run_single_with_php`] for the injectable seam.
pub fn run_single_with_runner(
    engine_type: &str,
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    run_single_with_php(engine_type, ctx, runner, &resolve_php_prefix)
}

/// Run one engine by `.arcunit` type or `--engine` class name, with an
/// injectable `php` prefix resolver for the phutil engine.
///
/// Every command goes through `runner` so tests inject canned outputs.
pub fn run_single_with_php(
    engine_type: &str,
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
    php: &dyn Fn() -> Option<Vec<String>>,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    if !CONFIG_AVAILABLE.contains(&engine_type) {
        return Err(EngineError::Usage(format!(
            "unknown engine '{engine_type}' (available engines: {})",
            AVAILABLE_ENGINES.join(", ")
        )));
    }
    EngineContext::check_set_run_all_tests(
        supports_run_all_tests(engine_type),
        engine_type,
        ctx.run_all_tests,
    )
    .map_err(EngineError::Usage)?;
    match engine_type {
        "NoseTestEngine" => {
            unit_nose::NoseEngine::new().run_with(ctx, runner, &unit_nose::RealNoseFs)
        }
        "PytestTestEngine" => unit_pytest::PytestEngine::new().run_with(ctx, runner, &RealPytestFs),
        "PhpunitTestEngine" => {
            let engine = phpunit_engine_from_config(&ctx.project_root);
            // Surface a configured-but-missing phpunit config verbatim
            // (`PhpunitTestEngine.php:263-266`).
            if let Some(config_value) = read_arcconfig(&ctx.project_root)
                .as_ref()
                .and_then(|v| v.get(unit_phpunit::CONFIG_KEY))
                .and_then(|v| v.as_str())
            {
                let full = format!("{}/{config_value}", ctx.project_root);
                if !Path::new(&full).exists() {
                    return Err(EngineError::Failed(format!(
                        "PHPUnit configuration file was not found in {full}"
                    )));
                }
            }
            engine.run_with(ctx, runner, &RealPhpunitFs)
        }
        "PhutilUnitTestEngine" | "phutil" => run_phutil_with_runner(ctx, runner, engine_type, php),
        "XUnitTestEngine" => run_xunit_with_runner(ctx, runner),
        "CSharpToolsTestEngine" => run_csharp_with_runner(ctx, runner),
        _ => Err(EngineError::Usage(format!(
            "unknown engine '{engine_type}' (available engines: {})",
            AVAILABLE_ENGINES.join(", ")
        ))),
    }
}

/// Count per-test results: `pass` counts as passed, `skip` counts as
/// neither, every other code counts as failed (fail-closed).
fn count_results(results: &[EngineUnitResult]) -> (u32, u32) {
    let mut passed = 0u32;
    let mut failed = 0u32;
    for result in results {
        match result.get_result() {
            arc_rs::unit_engine::RESULT_PASS => passed += 1,
            arc_rs::unit_engine::RESULT_SKIP => {}
            _ => failed += 1,
        }
    }
    (passed, failed)
}

/// Run one engine and aggregate its per-test results.
pub fn run_single_report(
    engine_type: &str,
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<ConfigReport, String> {
    let ctx = EngineContext {
        project_root: project_root.to_string(),
        paths: paths.to_vec(),
        enable_coverage,
        run_all_tests,
    };
    let started = std::time::Instant::now();
    let results =
        run_single_with_runner(engine_type, &ctx, runner).map_err(|e| e.message().to_string())?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let (passed, failed) = count_results(&results);
    let success =
        arc_rs::unit_engine::overall_result(&results) == arc_rs::unit_engine::WORKFLOW_RESULT_OKAY;
    Ok(ConfigReport {
        engine: engine_type.to_string(),
        passed,
        failed,
        duration_ms,
        success,
        results,
    })
}

/// Run `.arcunit` engines and merge their results, mirroring
/// `ArcanistConfigurationDrivenUnitTestEngine::run`
/// (`ArcanistConfigurationDrivenUnitTestEngine.php:117-170`). The phutil
/// engine resolves `php` on the real `PATH`; see [`run_config_with_php`].
pub fn run_config_with_runner(
    planned: &[PlannedEngine],
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<ConfigReport, String> {
    run_config_with_php(
        planned,
        project_root,
        paths,
        run_all_tests,
        enable_coverage,
        runner,
        &resolve_php_prefix,
    )
}

/// Run `.arcunit` engines with an injectable `php` prefix resolver for
/// the phutil engine.
pub fn run_config_with_php(
    planned: &[PlannedEngine],
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
    php: &dyn Fn() -> Option<Vec<String>>,
) -> Result<ConfigReport, String> {
    let child_supports: Vec<bool> = planned
        .iter()
        .map(|engine| supports_run_all_tests(&engine.engine_type))
        .collect();
    if run_all_tests && !unit_config::supports_run_all_tests(&child_supports) {
        return Err(
            "Engine 'ArcanistConfigurationDrivenUnitTestEngine' does not support --everything."
                .to_string(),
        );
    }
    let top = EngineContext {
        project_root: project_root.to_string(),
        paths: paths.to_vec(),
        enable_coverage,
        run_all_tests,
    };
    let started = std::time::Instant::now();
    let mut runs = Vec::new();
    for engine in planned {
        let child =
            unit_config::child_context(&top, engine, supports_run_all_tests(&engine.engine_type));
        runs.push(run_single_with_php(
            &engine.engine_type,
            &child,
            runner,
            php,
        ));
    }
    let results = unit_config::merge_engine_runs(runs).map_err(|e| e.message().to_string())?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let (passed, failed) = count_results(&results);
    let success =
        arc_rs::unit_engine::overall_result(&results) == arc_rs::unit_engine::WORKFLOW_RESULT_OKAY;
    Ok(ConfigReport {
        engine: "ArcanistConfigurationDrivenUnitTestEngine".to_string(),
        passed,
        failed,
        duration_ms,
        success,
        results,
    })
}

/// Format a duration in seconds, mirroring `formatTime`
/// (`ArcanistUnitConsoleRenderer.php:83-99`).
#[must_use]
pub fn format_time(seconds: f64) -> String {
    if seconds >= 60.0 {
        let minutes = (seconds / 60.0).floor() as i64;
        let rest = seconds as i64 % 60;
        return format!("{minutes}m{rest:02}s");
    }
    if seconds >= 1.0 {
        return format!("{seconds:4.1}s");
    }
    let milliseconds = seconds * 1000.0;
    if milliseconds >= 1.0 {
        return format!("{:3}ms", milliseconds.round() as i64);
    }
    " <1ms".to_string()
}

/// Render one result, mirroring `renderUnitResult`
/// (`ArcanistUnitConsoleRenderer.php:5-30`) in plain text: `  <LABEL>[ time]
/// name`, plus `userData` for non-passing results. Labels match
/// `getFormattedResult` (lines 32-54) without console colors.
#[must_use]
pub fn render_unit_result(result: &EngineUnitResult) -> String {
    let code = result.get_result();
    let label = match code {
        arc_rs::unit_engine::RESULT_PASS => "PASS",
        arc_rs::unit_engine::RESULT_FAIL => "FAIL",
        arc_rs::unit_engine::RESULT_SKIP => "SKIP",
        arc_rs::unit_engine::RESULT_BROKEN => "BROKEN",
        arc_rs::unit_engine::RESULT_UNSOUND => "UNSOUND",
        _ => "",
    };
    let duration = if code == arc_rs::unit_engine::RESULT_PASS {
        format!(" {}", format_time(result.get_duration().unwrap_or(0.0)))
    } else {
        String::new()
    };
    let name = match result.get_namespace() {
        Some(namespace) if !namespace.is_empty() => {
            format!("{}::{}", namespace, result.get_name())
        }
        _ => result.get_name().to_string(),
    };
    let mut out = format!("  {label}{duration} {name}\n");
    if code != arc_rs::unit_engine::RESULT_PASS
        && let Some(user_data) = result.get_user_data()
        && !user_data.is_empty()
    {
        out.push_str(user_data);
        out.push('\n');
    }
    out
}

/// Project root for single-engine runs: nearest `.arcunit`/`.arcconfig`
/// ancestor, else `cwd` (mirrors `getWorkingCopy()->getProjectRoot()`).
fn project_root_path(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".arcunit").is_file() || dir.join(".arcconfig").is_file())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Collect per-file coverage reports across results
/// (`ArcanistUnitWorkflow.php:194-199`): `coverage[file][] = report`.
#[must_use]
pub fn collect_coverage(results: &[EngineUnitResult]) -> HashMap<String, Vec<String>> {
    let mut coverage: HashMap<String, Vec<String>> = HashMap::new();
    for result in results {
        if let Some(map) = result.get_coverage() {
            for (file, report) in map {
                coverage
                    .entry(file.clone())
                    .or_default()
                    .push(report.clone());
            }
        }
    }
    coverage
}

/// Merge per-file reports and compute fractions
/// (`ArcanistUnitWorkflow.php:201-217`): `C / (C + U)` else `0`.
/// Returns `(sorted_file_coverage, file_reports)`; `paths` seeds every
/// requested file at `0` like PHP `array_fill_keys($paths, 0)`.
#[must_use]
pub fn merge_file_coverage(
    paths: &[String],
    coverage: &HashMap<String, Vec<String>>,
) -> (Vec<(String, f64)>, HashMap<String, String>) {
    let mut file_coverage: HashMap<String, f64> = HashMap::new();
    for path in paths {
        file_coverage.insert(path.clone(), 0.0);
    }
    let mut file_reports: HashMap<String, String> = HashMap::new();
    for (file, reports) in coverage {
        let merged = EngineUnitResult::merge_coverage(reports).unwrap_or_default();
        let fraction = arc_rs::unit_engine::coverage_fraction(&merged);
        file_coverage.insert(file.clone(), fraction);
        file_reports.insert(file.clone(), merged);
    }
    let mut sorted: Vec<(String, f64)> = file_coverage.into_iter().collect();
    sorted.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    (sorted, file_reports)
}

/// Render the coverage report (`ArcanistUnitWorkflow.php:218-239`) in plain
/// text: `\nCOVERAGE REPORT\n` plus `    %3d%%     file` lines (PHP wraps
/// both in `__`/`**` console markup) and optional detailed reports.
#[must_use]
pub fn render_coverage_report(
    sorted: &[(String, f64)],
    file_reports: &HashMap<String, String>,
    detailed: bool,
    project_root: &Path,
) -> String {
    let mut out = String::from("\nCOVERAGE REPORT\n");
    for (file, fraction) in sorted {
        let pct = (100.0 * fraction) as i32;
        out.push_str(&format!("    {pct:>3}%     {file}\n"));
        if detailed {
            let full = project_root.join(file);
            if let Some(report) = file_reports.get(file)
                && full.is_file()
                && let Ok(data) = std::fs::read_to_string(&full)
            {
                out.push_str(&render_detailed_coverage_report(&data, report));
            }
        }
    }
    out
}

/// Render one file's detailed coverage (`ArcanistUnitWorkflow.php:292-331`)
/// in plain text: `%5d  [C] <80-col line>` per source line. PHP paints
/// `<bg:green/red/magenta>` backgrounds for `C`/`U`/`X` (default `N` plain);
/// the bracketed mark keeps the stripped colors observable.
#[must_use]
pub fn render_detailed_coverage_report(data: &str, report: &str) -> String {
    let report_bytes = report.as_bytes();
    let mut out = String::new();
    for (n, line) in data.split('\n').enumerate() {
        let mark = report_bytes.get(n).copied().unwrap_or(b'N') as char;
        let padded = format!("{line:<80}");
        // Only C/U/X/N appear in real reports; anything else echoes raw.
        let shown = match mark {
            'C' | 'U' | 'X' | 'N' => mark,
            other => other,
        };
        out.push_str(&format!("{:>5}  [{shown}] {padded}\n", n + 1));
    }
    out
}

/// Escape one string like PHP `json_encode`.
///
/// Both unit JSON formats escape `"`, `\`, controls (short forms `\b \f
/// `\n \r \t`, other C0 as lowercase `\u00xx`) and non-ASCII (lowercase
/// `\uXXXX`, surrogate pairs past the BMP). Only the ugly format escapes
/// `/` as `\/`: pretty values use `JSON_UNESCAPED_SLASHES`
/// (`src/parser/PhutilJSON.php:133-140`). `<`, `>`, `&` stay raw in both
/// (PHP escapes those only with `JSON_HEX_*`).
fn php_json_escape_into(out: &mut String, text: &str, escape_slashes: bool) {
    for ch in text.chars() {
        if !escape_short_form(out, ch, escape_slashes) {
            escape_fallback_char(out, ch);
        }
    }
}

/// Push a short escape for `ch`; true when handled.
fn escape_short_form(out: &mut String, ch: char, escape_slashes: bool) -> bool {
    match ch {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '/' => {
            if escape_slashes {
                out.push_str("\\/");
            } else {
                out.push('/');
            }
        }
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{08}' => out.push_str("\\b"),
        '\u{0C}' => out.push_str("\\f"),
        _ => return false,
    }
    true
}

/// Push a control, ASCII, or unicode escape for a non-short-form char.
fn escape_fallback_char(out: &mut String, ch: char) {
    if (ch as u32) < 0x20 {
        out.push_str(&format!("\\u{:04x}", ch as u32));
    } else if (ch as u32) < 0x80 {
        out.push(ch);
    } else {
        append_json_unicode_escape(out, ch);
    }
}

/// Append a lowercase `\uXXXX` (or surrogate pair) escape for `ch`.
fn append_json_unicode_escape(out: &mut String, ch: char) {
    let n = ch as u32;
    if n < 0x1_0000 {
        out.push_str(&format!("\\u{n:04x}"));
    } else {
        let v = n - 0x1_0000;
        let hi = 0xD800 + (v >> 10);
        let lo = 0xDC00 + (v & 0x3FF);
        out.push_str(&format!("\\u{hi:04x}\\u{lo:04x}"));
    }
}

/// Quote one string like PHP `json_encode` renders a scalar.
fn php_json_string(text: &str, escape_slashes: bool) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    php_json_escape_into(&mut out, text, escape_slashes);
    out.push('"');
    out
}

/// Render a float like `json_encode` (shortest round-trip, `.0` for
/// integrals); non-finite becomes `null` (durations are always finite).
fn php_json_number(value: f64) -> String {
    serde_json::Number::from_f64(value).map_or_else(|| "null".to_string(), |n| n.to_string())
}

/// Render one `extra`/`coverage` map: `null` when absent, `[]` when empty
/// (PHP encodes an empty array as a list), else an object. Nested keys are
/// sorted; PHP keeps insertion order (see the audit note on
/// [`results_to_json_text`]).
fn append_json_map(
    out: &mut String,
    pairs: Option<&[(String, String)]>,
    escape_slashes: bool,
    pretty: bool,
    depth: usize,
) {
    let Some(pairs) = pairs else {
        out.push_str("null");
        return;
    };
    if pairs.is_empty() {
        out.push_str("[]");
        return;
    }
    if !pretty {
        out.push('{');
        for (i, (key, value)) in pairs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&php_json_string(key, escape_slashes));
            out.push(':');
            out.push_str(&php_json_string(value, escape_slashes));
        }
        out.push('}');
        return;
    }
    append_json_map_pretty(out, pairs, depth);
}

/// Render one non-empty map in `PhutilJSON` pretty form at `depth`.
fn append_json_map_pretty(out: &mut String, pairs: &[(String, String)], depth: usize) {
    out.push_str("{\n");
    for (i, (key, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        append_json_indent(out, depth + 1);
        out.push_str(&php_json_string(key, false));
        out.push_str(": ");
        out.push_str(&php_json_string(value, false));
    }
    out.push('\n');
    append_json_indent(out, depth);
    out.push('}');
}

/// Append `depth` levels of two-space `PhutilJSON` indentation.
fn append_json_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Render one result dict in `toDictionary` key order (compact).
fn append_result_ugly(out: &mut String, result: &EngineUnitResult) {
    let dict = result.to_dictionary();
    out.push_str("{\"namespace\":");
    append_json_string_or_null(out, dict.namespace.as_deref(), true);
    out.push_str(",\"name\":");
    out.push_str(&php_json_string(&dict.name, true));
    out.push_str(",\"link\":");
    append_json_string_or_null(out, dict.link.as_deref(), true);
    out.push_str(",\"result\":");
    out.push_str(&php_json_string(&dict.result, true));
    out.push_str(",\"duration\":");
    append_json_duration(out, dict.duration);
    out.push_str(",\"extra\":");
    append_json_map(out, dict.extra.as_deref(), true, false, 0);
    out.push_str(",\"userData\":");
    append_json_string_or_null(out, dict.user_data.as_deref(), true);
    out.push_str(",\"coverage\":");
    append_json_map(out, dict.coverage.as_deref(), true, false, 0);
    out.push('}');
}

/// Render one result dict in `PhutilJSON` pretty form at `depth`.
fn append_result_pretty(out: &mut String, result: &EngineUnitResult, depth: usize) {
    let dict = result.to_dictionary();
    let mut extra = String::new();
    append_json_map(&mut extra, dict.extra.as_deref(), false, true, depth + 1);
    let mut coverage = String::new();
    append_json_map(
        &mut coverage,
        dict.coverage.as_deref(),
        false,
        true,
        depth + 1,
    );
    let fields = [
        (
            "namespace",
            php_json_string_or_null(dict.namespace.as_deref(), false),
        ),
        ("name", php_json_string(&dict.name, false)),
        ("link", php_json_string_or_null(dict.link.as_deref(), false)),
        ("result", php_json_string(&dict.result, false)),
        ("duration", php_json_duration(dict.duration)),
        ("extra", extra),
        (
            "userData",
            php_json_string_or_null(dict.user_data.as_deref(), false),
        ),
        ("coverage", coverage),
    ];
    out.push('{');
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('\n');
        append_json_indent(out, depth + 1);
        out.push_str(&php_json_string(key, false));
        out.push_str(": ");
        out.push_str(value);
    }
    out.push('\n');
    append_json_indent(out, depth);
    out.push('}');
}

/// Render an optional string as a JSON string or `null`.
fn append_json_string_or_null(out: &mut String, value: Option<&str>, escape_slashes: bool) {
    match value {
        Some(text) => out.push_str(&php_json_string(text, escape_slashes)),
        None => out.push_str("null"),
    }
}

/// Render an optional string as a JSON string or `null` (owned form).
fn php_json_string_or_null(value: Option<&str>, escape_slashes: bool) -> String {
    value.map_or_else(
        || "null".to_string(),
        |text| php_json_string(text, escape_slashes),
    )
}

/// Render an optional duration as a JSON number or `null`.
fn append_json_duration(out: &mut String, duration: Option<f64>) {
    match duration {
        Some(value) => out.push_str(&php_json_number(value)),
        None => out.push_str("null"),
    }
}

/// Render an optional duration as a JSON number or `null` (owned form).
fn php_json_duration(duration: Option<f64>) -> String {
    duration.map_or_else(|| "null".to_string(), php_json_number)
}

/// Encode results exactly like PHP renders them
/// (`ArcanistUnitWorkflow.php:258-266`).
///
/// Ugly is `json_encode($data)`: a compact array, `/` escaped as `\/`,
/// no trailing newline. Pretty is `PhutilJSON::encodeFormatted($data)`:
/// note it formats the result *list* as an object with numeric keys
/// (`encodeFormatted` always calls `encodeFormattedObject`), slashes
/// unescaped, one trailing newline; empty results render `{}`.
///
/// Audit note (serde vs PHP): serde_json sorts keys (PHP keeps
/// `toDictionary` insertion order), never escapes `/`, and emits raw
/// UTF-8 (PHP `\uXXXX`-escapes non-ASCII) and `{}` for empty maps (PHP
/// emits `[]` for empty arrays), so both formats render here by hand.
/// Remaining known divergences: nested `extra`/`coverage` key order
/// (sorted here, first-seen in PHP) and Harbormaster upload encoding
/// (the shared conduit client serializes with serde).
#[must_use]
pub fn results_to_json_text(results: &[EngineUnitResult], pretty: bool) -> String {
    if pretty {
        results_to_json_pretty(results)
    } else {
        let mut out = String::from("[");
        for (i, result) in results.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            append_result_ugly(&mut out, result);
        }
        out.push(']');
        out
    }
}

/// Pretty results: an object with numeric keys plus one trailing newline.
fn results_to_json_pretty(results: &[EngineUnitResult]) -> String {
    if results.is_empty() {
        return "{}\n".to_string();
    }
    let mut out = String::from("{");
    for (i, result) in results.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('\n');
        append_json_indent(&mut out, 1);
        out.push_str(&php_json_string(&i.to_string(), false));
        out.push_str(": ");
        append_result_pretty(&mut out, result, 1);
    }
    out.push_str("\n}\n");
    out
}

/// Encode results as a JSON array of `toDictionary` objects
/// (`ArcanistUnitWorkflow.php:258`, `ArcanistUnitTestResult.php:164-175`),
/// in `toDictionary` key order with `null` for missing values.
#[must_use]
pub fn results_to_json(results: &[EngineUnitResult]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            let d = r.to_dictionary();
            let mut map = serde_json::Map::new();
            map.insert(
                "namespace".to_string(),
                d.namespace
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
            map.insert("name".to_string(), serde_json::Value::String(d.name));
            map.insert(
                "link".to_string(),
                d.link
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
            map.insert("result".to_string(), serde_json::Value::String(d.result));
            map.insert(
                "duration".to_string(),
                d.duration.map_or(serde_json::Value::Null, |v| {
                    serde_json::Number::from_f64(v)
                        .map_or(serde_json::Value::Null, serde_json::Value::Number)
                }),
            );
            map.insert(
                "extra".to_string(),
                d.extra.map_or(serde_json::Value::Null, |pairs| {
                    let mut obj = serde_json::Map::new();
                    for (k, v) in pairs {
                        obj.insert(k, serde_json::Value::String(v));
                    }
                    serde_json::Value::Object(obj)
                }),
            );
            map.insert(
                "userData".to_string(),
                d.user_data
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
            map.insert(
                "coverage".to_string(),
                d.coverage.map_or(serde_json::Value::Null, |pairs| {
                    let mut obj = serde_json::Map::new();
                    for (k, v) in pairs {
                        obj.insert(k, serde_json::Value::String(v));
                    }
                    serde_json::Value::Object(obj)
                }),
            );
            serde_json::Value::Object(map)
        })
        .collect();
    serde_json::Value::Array(items)
}

/// Modern unit dictionary (`ArcanistWorkflow.php:2244-2254`): drop `null`
/// values, rename non-empty `userData` to `details`.
#[must_use]
pub fn modern_unit_value(value: &serde_json::Value) -> serde_json::Value {
    let Some(obj) = value.as_object() else {
        return value.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if v.is_null() {
            continue;
        }
        if k == "userData" {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    if let Some(details) = obj.get("userData").and_then(|v| v.as_str())
        && !details.is_empty()
    {
        out.insert(
            "details".to_string(),
            serde_json::Value::String(details.to_string()),
        );
    }
    serde_json::Value::Object(out)
}

/// Upload results to Harbormaster (`ArcanistUnitWorkflow.php:406-428`):
/// `harbormaster.sendmessage` with `buildTargetPHID`, modern `unit` dicts,
/// and the `pass`/`fail` type from [`harbormaster_type`].
pub fn upload_results(
    target_phid: &str,
    overall: i32,
    results: &[EngineUnitResult],
    conduit_uri: Option<&str>,
) -> Result<(), String> {
    let client = config::client_for(conduit_uri)?;
    let raw = results_to_json(results);
    let modern: Vec<serde_json::Value> = raw
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(modern_unit_value)
        .collect();
    let params = serde_json::json!({
        "buildTargetPHID": target_phid,
        "unit": modern,
        "type": harbormaster_type(overall),
    });
    client
        .call("harbormaster.sendmessage", &params)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Render a full workflow report to stdout text (pure helper for tests).
///
/// `full` echoes per-result lines plus the coverage report when coverage is
/// present; `json` pretty-prints the `toDictionary` list (as a
/// numeric-keyed object, like `PhutilJSON`); `ugly` compacts it to a JSON
/// array; `none` prints nothing (`ArcanistUnitWorkflow.php:178-273`).
#[must_use]
pub fn render_workflow_output(
    results: &[EngineUnitResult],
    paths: &[String],
    output_format: &str,
    detailed: bool,
    project_root: &Path,
) -> String {
    match output_format {
        "json" => results_to_json_text(results, true),
        "ugly" => results_to_json_text(results, false),
        "none" => String::new(),
        _ => {
            let mut out = String::new();
            for result in results {
                out.push_str(&render_unit_result(result));
            }
            let coverage = collect_coverage(results);
            if !coverage.is_empty() {
                let (sorted, reports) = merge_file_coverage(paths, &coverage);
                out.push_str(&render_coverage_report(
                    &sorted,
                    &reports,
                    detailed,
                    project_root,
                ));
            }
            out
        }
    }
}

/// Exit code for a workflow result (`RESULT_OKAY` 0, `RESULT_UNSOUND` 1,
/// `RESULT_FAIL` 2; `ArcanistUnitWorkflow.php:8-11,243-253`).
#[must_use]
pub fn exit_code_for_overall(overall: i32) -> ExitCode {
    ExitCode::from(overall as u8)
}

fn print_cargo_output(report: &EngineReport, format: &str) {
    match format {
        "json" => {
            let text =
                serde_json::to_string_pretty(&report.to_json()).unwrap_or_else(empty_json_object);
            println!("{text}");
        }
        "ugly" => {
            println!("{}", report.to_json());
        }
        "none" => {}
        _ => {
            println!("{}", report.summary_line());
            if !report.tail.is_empty() {
                println!("{}", report.tail);
            }
        }
    }
}

fn cargo_outcome_code(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Pass => arc_rs::unit_engine::RESULT_PASS,
        Outcome::Fail => arc_rs::unit_engine::RESULT_FAIL,
        Outcome::Skip => arc_rs::unit_engine::RESULT_SKIP,
        Outcome::Broken => arc_rs::unit_engine::RESULT_BROKEN,
        Outcome::Unsound => arc_rs::unit_engine::RESULT_UNSOUND,
    }
}

fn convert_cargo_result(result: &UnitResult) -> EngineUnitResult {
    let mut out = EngineUnitResult::new();
    let _ = out.set_name(&result.name);
    out.set_result(cargo_outcome_code(result.outcome));
    out.set_duration(result.duration_secs);
    out
}

fn converted_cargo_results(report: &EngineReport) -> Vec<EngineUnitResult> {
    report.results.iter().map(convert_cargo_result).collect()
}

fn upload_cargo_target(opts: &Options, report: &EngineReport) -> Result<(), String> {
    let Some(target) = opts.target.as_deref() else {
        return Ok(());
    };
    let overall = if report.success {
        arc_rs::unit_engine::WORKFLOW_RESULT_OKAY
    } else {
        arc_rs::unit_engine::WORKFLOW_RESULT_FAIL
    };
    let converted = converted_cargo_results(report);
    upload_results(target, overall, &converted, opts.conduit_uri.as_deref())
}

fn empty_json_object(_: serde_json::Error) -> String {
    "{}".to_string()
}

/// Run the default `cargo` selection and report its outcome.
///
/// `full` prints the one-line summary plus the trailing output tail;
/// `json` pretty-prints the aggregate; `ugly` compacts it; `none` prints
/// nothing. Failures exit `2` (`RESULT_FAIL`); successes exit `0`.
fn run_cargo_selection(opts: &Options, cwd: &Path) -> ExitCode {
    if let Err(msg) = check_everything_paths(opts.everything, &opts.paths) {
        eprintln!("arc-rs unit: {msg}");
        return ExitCode::FAILURE;
    }
    if let Some(warning) = cargo_coverage_warning(opts) {
        eprintln!("{warning}");
    }
    match execute(opts, cwd, None) {
        Ok(report) => {
            let format = output_format(opts);
            print_cargo_output(&report, &format);
            if let Err(msg) = upload_cargo_target(opts, &report) {
                eprintln!("arc-rs unit: {msg}");
                return ExitCode::FAILURE;
            }
            if report.success {
                ExitCode::SUCCESS
            } else {
                exit_code_for_overall(arc_rs::unit_engine::WORKFLOW_RESULT_FAIL)
            }
        }
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Run one engine named by `--engine` or `unit.engine`.
fn run_single_selection(opts: &Options, cwd: &Path, class: &str, paths: &[String]) -> ExitCode {
    if !opts.cargo_args.is_empty() {
        eprintln!("arc-rs unit: cargo test args after '--' require the cargo engine");
        return ExitCode::FAILURE;
    }
    let root = project_root_path(cwd);
    let root_string = root.to_string_lossy().replace('\\', "/");
    let runner = RealRunner;
    let coverage = enable_coverage(opts);
    match run_single_report(
        class,
        &root_string,
        paths,
        opts.everything,
        coverage,
        &runner,
    ) {
        Ok(report) => finish_php_report(opts, &root, paths, &report),
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Run a configuration-driven (`.arcunit`) selection.
fn run_config_selection(
    opts: &Options,
    engines: &[PlannedEngine],
    root: &Path,
    paths: &[String],
) -> ExitCode {
    if !opts.cargo_args.is_empty() {
        eprintln!("arc-rs unit: cargo test args after '--' require the cargo engine");
        return ExitCode::FAILURE;
    }
    let root_string = root.to_string_lossy().replace('\\', "/");
    let runner = RealRunner;
    let coverage = enable_coverage(opts);
    match run_config_with_runner(
        engines,
        &root_string,
        paths,
        opts.everything,
        coverage,
        &runner,
    ) {
        Ok(report) => finish_php_report(opts, root, paths, &report),
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Print a PHP-engine report per the output format, upload to `target` when
/// set, and map the overall result to the process exit code
/// (`ArcanistUnitWorkflow.php:184-282`).
fn finish_php_report(
    opts: &Options,
    project_root: &Path,
    paths: &[String],
    report: &ConfigReport,
) -> ExitCode {
    let format = output_format(opts);
    let text = render_workflow_output(
        &report.results,
        paths,
        &format,
        opts.detailed_coverage,
        project_root,
    );
    print!("{text}");
    let overall = arc_rs::unit_engine::overall_result(&report.results);
    if let Some(target) = opts.target.as_deref()
        && let Err(msg) = upload_results(
            target,
            overall,
            &report.results,
            opts.conduit_uri.as_deref(),
        )
    {
        eprintln!("arc-rs unit: {msg}");
        return ExitCode::FAILURE;
    }
    exit_code_for_overall(overall)
}

fn parse_unit_options(argv: &[String]) -> Result<Options, ExitCode> {
    match parse_options(argv) {
        Ok(Some(opts)) => Ok(opts),
        Ok(None) => {
            print!("{USAGE}");
            Err(ExitCode::SUCCESS)
        }
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}\n\n{USAGE}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn unit_working_dir() -> Result<PathBuf, ExitCode> {
    match std::env::current_dir() {
        Ok(dir) => Ok(dir),
        Err(e) => {
            eprintln!("arc-rs unit: cannot read current directory: {e}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn check_unit_supports(cwd: &Path, opts: &Options) -> Result<(), ExitCode> {
    if let Some(vcs) = detect_vcs(cwd)
        && let Err(msg) = check_supports(vcs, &opts.provided)
    {
        eprintln!("arc-rs unit: {msg}");
        return Err(ExitCode::FAILURE);
    }
    Ok(())
}

fn discover_unit_selection(opts: &Options, cwd: &Path) -> Result<Selection, ExitCode> {
    // Cargo ignores paths/rev (it runs `cargo test` in the crate root), but
    // the `--everything` plus paths check still applies workflow-wide.
    match discover_selection(opts, cwd) {
        Ok(selection) => Ok(selection),
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn run_single_paths(opts: &Options, cwd: &Path, class: &str) -> ExitCode {
    let root = project_root_path(cwd);
    let paths = match select_paths(opts, cwd, &root) {
        Ok(paths) => paths,
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            return ExitCode::FAILURE;
        }
    };
    run_single_selection(opts, cwd, class, &paths)
}

fn run_config_paths(
    opts: &Options,
    cwd: &Path,
    engines: &[PlannedEngine],
    root: &Path,
) -> ExitCode {
    let paths = match select_paths(opts, cwd, root) {
        Ok(paths) => paths,
        Err(msg) => {
            eprintln!("arc-rs unit: {msg}");
            return ExitCode::FAILURE;
        }
    };
    run_config_selection(opts, engines, root, &paths)
}

fn run_unit_selection(opts: &Options, cwd: &Path, selection: Selection) -> ExitCode {
    match selection {
        Selection::Cargo => run_cargo_selection(opts, cwd),
        Selection::Single(class) => run_single_paths(opts, cwd, &class),
        Selection::Config { engines, root } => run_config_paths(opts, cwd, &engines, &root),
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let opts = match parse_unit_options(argv) {
        Ok(opts) => opts,
        Err(code) => return code,
    };
    let cwd = match unit_working_dir() {
        Ok(dir) => dir,
        Err(code) => return code,
    };
    if let Err(code) = check_unit_supports(&cwd, &opts) {
        return code;
    }
    let selection = match discover_unit_selection(&opts, &cwd) {
        Ok(selection) => selection,
        Err(code) => return code,
    };
    run_unit_selection(&opts, &cwd, selection)
}

#[cfg(test)]
#[path = "unit_tests.rs"]
mod unit_tests;
