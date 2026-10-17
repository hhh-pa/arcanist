//! `arc unit` — full-state port of `arc unit` plus a cargo adapter.
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
//! project root (lines 2115-2119), else `ArcanistNoEngineException`
//! (lines 2121-2129). The `cargo` engine runs only when named explicitly
//! or via `unit.engine`.
//!
//! `--engine` accepts `cargo` plus the PHP engine class names verbatim.
//! `.arcunit` `type` values resolve through the class map of
//! `getEngineConfigurationName` values
//! (`ArcanistConfigurationDrivenUnitTestEngine.php:176-181`) where only
//! `phutil` is non-null in-tree (`PhutilUnitTestEngine.php:8-10`), so only
//! `phutil` validates there; single-engine runs accept the class names
//! plus `phutil`.
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
use arc_rs::{
    config, get_config, unit_csharp, unit_nose, unit_phpunit, unit_phutil, unit_pytest, unit_xunit,
};

use crate::conduit::{self, ClassifiedError, EnvelopeKind};
use crate::terminal;

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

/// `.arcunit` `type` values (UNIT-U15): the `getEngineConfigurationName`
/// class map (`ArcanistConfigurationDrivenUnitTestEngine.php:176-181`),
/// where only `phutil` is non-null in-tree
/// (`PhutilUnitTestEngine.php:8-10`).
pub const CONFIG_AVAILABLE: &[&str] = &["phutil"];

/// Engine types the single-engine runners accept: the ported PHP engine
/// class names plus `phutil` (`.arcunit`'s class-map spelling of
/// `PhutilUnitTestEngine`, which `--engine` itself rejects like PHP's
/// `class_exists` check).
pub const SINGLE_AVAILABLE: &[&str] = &[
    "CSharpToolsTestEngine",
    "NoseTestEngine",
    "PhpunitTestEngine",
    "PhutilUnitTestEngine",
    "PytestTestEngine",
    "XUnitTestEngine",
    "phutil",
];

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistUnitWorkflow.php:21-110`).
const HELP_SYNOPSIS: &str =
    "      **unit** [__options__] [__paths__]\n      **unit** [__options__] --rev [__rev__]";
const HELP_TEXT: &str = "          Supports: git, svn, hg\n          Run unit tests that cover specified paths. If no paths are specified,\n          unit tests covering all modified files will be run.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "rev",
        None,
        Some("revision"),
        "Run unit tests covering changes since a specific revision.",
    ),
    (
        "engine",
        None,
        Some("classname"),
        "Override configured unit engine for this project.",
    ),
    (
        "coverage",
        None,
        None,
        "Always enable coverage information.",
    ),
    (
        "no-coverage",
        None,
        None,
        "Always disable coverage information.",
    ),
    (
        "detailed-coverage",
        None,
        None,
        "Show a detailed coverage report on the CLI. Implies --coverage.",
    ),
    ("json", None, None, "Report results in JSON format."),
    (
        "output",
        None,
        Some("format"),
        "With 'full', show full pretty report (Default). With 'json', report results in JSON format. With 'ugly', use uglier (but more efficient) JSON formatting. With 'none', don't print results.",
    ),
    (
        "target",
        None,
        Some("phid"),
        "(PROTOTYPE) Record a copy of the test results on the specified Harbormaster build target.",
    ),
    (
        "everything",
        None,
        None,
        "Run every test associated with a tracked file in the working copy.",
    ),
    (
        "ugly",
        None,
        None,
        "With --json, use uglier (but more efficient) formatting.",
    ),
];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
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
    usage_text(terminal::ansi_enabled())
}

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
    /// Runtime `--config key=value` pairs (highest-precedence source).
    pub config: Vec<String>,
    /// `--config-file` paths (replace the system/user sources).
    pub config_file: Vec<String>,
    /// `--arcrc-file` path (replaces `~/.arcrc`).
    pub arcrc_file: Option<String>,
    /// Positional paths (`*` maps to `paths`, line 106).
    pub paths: Vec<String>,
    /// Canonical long flag names in first-provision order (PHP `$dict`
    /// insertion order, `ArcanistWorkflow.php:791-884`). Empty for
    /// hand-built options, which skip the order-dependent checks.
    pub provided: Vec<&'static str>,
    /// `--help`/`-h` (compat wrapper only; `parse_options` returns `None`).
    pub help: bool,
    /// Extra `cargo test` args for hand-built options (Rust extension):
    /// the CLI collects everything after `--` into `paths` like PHP
    /// (`ArcanistWorkflow.php:818-823`), and the cargo engine forwards
    /// paths as test-name filters.
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
            config: Vec::new(),
            config_file: Vec::new(),
            arcrc_file: None,
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
    Config,
    ConfigFile,
    ArcrcFile,
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
    FlagSpec {
        long: "config",
        short: None,
        flag: Flag::Value(ValueFlag::Config),
    },
    FlagSpec {
        long: "config-file",
        short: None,
        flag: Flag::Value(ValueFlag::ConfigFile),
    },
    FlagSpec {
        long: "arcrc-file",
        short: None,
        flag: Flag::Value(ValueFlag::ArcrcFile),
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
        ValueFlag::Config => options.config.push(value),
        ValueFlag::ConfigFile => options.config_file.push(value),
        ValueFlag::ArcrcFile => options.arcrc_file = Some(value),
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
/// (`src/workflow/ArcanistWorkflow.php:788-926`): `--` ends flags and the
/// rest collects into `paths` like any positional (the `*` wildcard;
/// UNIT-U6), and conflicts are checked after the whole line parses.
/// Returns `Ok(None)` when `--help` was requested.
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

/// Parse `argv` (excluding the `unit` subcommand name), compat wrapper.
///
/// Maps `parse_options` help (`None`) to `help: true`; all other outcomes
/// pass through. Everything after the first `--` collects into `paths`;
/// only tokens before it are interpreted as `unit` flags.
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
    crate::proc_env::command("git")
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
    // UNIT-U26: an empty `--rev` is falsy in PHP (`if ($rev)`), so it
    // takes the plain status path instead of resolving a range.
    let rev = opts.rev.as_deref().filter(|rev| !rev.is_empty());
    let Some(vcs) = detect_vcs(cwd) else {
        return Ok(Vec::new());
    };
    match vcs {
        Vcs::Git => working_copy_paths(cwd, rev),
        Vcs::Hg => hg_working_copy_paths_with(project_root, hg, rev),
        Vcs::Svn => svn_working_copy_paths_with(project_root, svn, rev),
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
/// `getConfigFromAnySource('unit.engine')`) from any config source for
/// `dir` (local > project > user > system > default). Lenient:
/// unreadable sources and non-string values read empty, like the old
/// walk-up; [`unit_engine_from_merged`] surfaces type errors instead.
#[must_use]
pub fn find_unit_engine_from(dir: &Path) -> Option<String> {
    let user_file = config::arcrc_path();
    find_unit_engine_with(
        dir,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
}

/// [`find_unit_engine_from`] with an explicit user file and
/// [`get_config::LoadOptions`].
#[must_use]
pub fn find_unit_engine_with(
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Option<String> {
    let merged = get_config::load_merged(dir, user_file, opts).ok()?;
    get_config::merged_value("unit.engine", &merged).and_then(|(_, value)| {
        value
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    })
}

/// Read `unit.engine` from a merged load with `willReadValue()`
/// validation: a mistyped value errors (like stock PHP) instead of
/// reading empty. An empty string reads as unset (PHP's falsy
/// fallthrough to `.arcunit`).
pub fn unit_engine_from_merged(
    merged: &get_config::MergedConfig,
) -> Result<Option<String>, String> {
    match get_config::checked_value("unit.engine", merged)? {
        Some((_, value)) => Ok(value
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_string)),
        None => Ok(None),
    }
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
/// cargo engine): hand-built `cargo_args` first, then the positional unit
/// paths as test-name filters. The CLI collects post-`--` tokens into
/// `paths` like PHP (UNIT-U6), so they reach cargo as filters too.
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
            "arc unit: warning: the cargo engine does not support coverage; \
             ignoring --detailed-coverage"
                .to_string(),
        )
    } else if opts.coverage {
        Some(
            "arc unit: warning: the cargo engine does not support coverage; \
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
        return Err(not_a_subclass_message(&opts.engine));
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

    let mut cmd = crate::proc_env::command("cargo");
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
    /// `cargo` run (only when named explicitly or via `unit.engine`).
    Cargo,
    /// One engine named by `--engine` or `unit.engine`.
    Single(String),
    /// Configuration-driven run over `.arcunit` engines.
    Config {
        engines: Vec<PlannedEngine>,
        root: PathBuf,
    },
}

/// Bad-engine message (`ArcanistWorkflow.php:2130-2137`, UNIT-U8):
/// `Configured unit test engine "%s" is not a subclass of "%s", but must
/// be.` Custom PHP engine classes are unportable (Rule 3), so names outside
/// the ported set always take this arm.
fn not_a_subclass_message(engine: &str) -> String {
    format!(
        "Configured unit test engine \"{engine}\" is not a subclass of \
         \"ArcanistUnitTestEngine\", but must be."
    )
}

/// No-engine message (`ArcanistWorkflow.php:2121-2129`, UNIT-U7).
fn no_engine_message() -> String {
    "No unit test engine is configured for this project. Create an '.arcunit' file, or \
     configure an advanced engine with 'unit.engine' in '.arcconfig'."
        .to_string()
}

fn validate_engine(engine: &str) -> Result<(), String> {
    if !AVAILABLE_ENGINES.contains(&engine) {
        return Err(not_a_subclass_message(engine));
    }
    Ok(())
}

/// Map an engine error onto the R4 envelope: `NoEffect` prints on
/// stdout with exit 0, `Usage` renders `Usage Exception:`, `Failed`
/// renders the generic banner.
fn classify_engine_error(error: EngineError) -> ClassifiedError {
    match error {
        EngineError::NoEffect(message) => ClassifiedError::no_effect(message),
        EngineError::Usage(message) => ClassifiedError::usage(message),
        EngineError::Failed(message) => ClassifiedError::exception(message),
    }
}

/// String-error entry kept for API stability; production uses
/// [`load_config_selection_classified`].
#[allow(dead_code)]
fn load_config_selection(cwd: &Path) -> Result<Selection, String> {
    load_config_selection_classified(cwd).map_err(|error| error.message)
}

/// [`load_config_selection`] with errors classified for the R4
/// envelope: a missing `.arcunit` is a usage error (PHP throws
/// `ArcanistNoEngineException`, an `ArcanistUsageException`
/// subclass); spec errors keep their engine class.
fn load_config_selection_classified(cwd: &Path) -> Result<Selection, ClassifiedError> {
    let Some(dir) = find_arcunit_from(cwd) else {
        return Err(ClassifiedError::usage(
            "Unable to find '.arcunit' file to configure test engines. Create an \
            '.arcunit' file in the root directory of the working copy."
                .to_string(),
        ));
    };
    let path = dir.join(".arcunit");
    let data = std::fs::read_to_string(&path).map_err(|e| {
        ClassifiedError::exception(format!("cannot read '{}': {e}", path.display()))
    })?;
    let display = path.to_string_lossy().to_string();
    let engines = unit_config::parse_engine_specs(&data, &display, CONFIG_AVAILABLE)
        .map_err(classify_engine_error)?;
    Ok(Selection::Config { engines, root: dir })
}

/// Select engines: explicit `--engine`, then `unit.engine` from any config
/// source, then `.arcunit`, else `ArcanistNoEngineException` (UNIT-U7).
pub fn discover_selection(opts: &Options, cwd: &Path) -> Result<Selection, String> {
    discover_selection_classified(opts, cwd).map_err(|error| error.message)
}

/// [`discover_selection`] with errors classified for the R4 envelope:
/// unknown engine names are usage errors (PHP throws
/// `ArcanistUsageException` for a bad `--engine`).
pub fn discover_selection_classified(
    opts: &Options,
    cwd: &Path,
) -> Result<Selection, ClassifiedError> {
    // UNIT-U14: an empty `--engine` is falsy in PHP (`if (!$engine_class)`)
    // and falls through to config, like an omitted flag.
    if opts.engine_explicit && !opts.engine.is_empty() {
        validate_engine(&opts.engine).map_err(ClassifiedError::usage)?;
        if opts.engine == "cargo" {
            return Ok(Selection::Cargo);
        }
        if opts.engine == "ArcanistConfigurationDrivenUnitTestEngine" {
            return load_config_selection_classified(cwd);
        }
        return Ok(Selection::Single(opts.engine.clone()));
    }
    let cli = get_config::load_options_from_cli(
        &opts.config,
        &opts.config_file,
        opts.arcrc_file.as_deref(),
    )
    .map_err(ClassifiedError::usage)?;
    let user_file = config::arcrc_path();
    let merged =
        get_config::load_merged(cwd, user_file.as_deref(), &cli).map_err(ClassifiedError::usage)?;
    if let Some(class) = unit_engine_from_merged(&merged).map_err(ClassifiedError::usage)? {
        validate_engine(&class).map_err(ClassifiedError::usage)?;
        if class == "cargo" {
            return Ok(Selection::Cargo);
        }
        if class == "ArcanistConfigurationDrivenUnitTestEngine" {
            return load_config_selection_classified(cwd);
        }
        return Ok(Selection::Single(class));
    }
    if find_arcunit_from(cwd).is_some() {
        return load_config_selection_classified(cwd);
    }
    // UNIT-U7: nothing configured is `ArcanistNoEngineException` (a usage
    // error), not a cargo default.
    Err(ClassifiedError::usage(no_engine_message()))
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

/// Whether the workflow echoes an engine's results after the run
/// (`ArcanistUnitTestEngine::shouldEchoTestResults`, UNIT-U1): the base
/// returns true and only the configuration-driven engine overrides it to
/// false (`ArcanistConfigurationDrivenUnitTestEngine.php:172-174`), echoing
/// non-echoing children's results itself while they run (`:151-157`).
#[must_use]
pub fn should_echo_test_results(engine_type: &str) -> bool {
    engine_type != "ArcanistConfigurationDrivenUnitTestEngine"
}

/// Real command runner: spawn `argv` with `cwd`, capturing output.
struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, argv: &[String], cwd: &str) -> arc_rs::unit_engine::CapturedOutput {
        if argv.is_empty() {
            return arc_rs::unit_engine::CapturedOutput::new(1, "", "empty command");
        }
        match crate::proc_env::command(&argv[0])
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

/// Fetch one `.arcconfig` string key (legacy project-root-only read, kept
/// for compat; engine code reads through [`engine_param_from`] instead).
#[must_use]
pub fn arcconfig_string(project_root: &str, key: &str) -> Option<String> {
    read_arcconfig(project_root)?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Default-options merged load for `project_root` (local, project, user,
/// system, default sources), or `None` when any source is unreadable
/// (UNIT-U13 follow-through: `unit.*` engine params read through the same
/// merge as `unit.engine`). Runtime `--config` overrides do not reach
/// engine params: the runners below have no CLI handle (documented gap).
fn lenient_engine_merged(project_root: &str) -> Option<get_config::MergedConfig> {
    let user_file = config::arcrc_path();
    get_config::load_merged(
        Path::new(project_root),
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
    .ok()
}

/// One `unit.*` engine param: the merged winner when a merged load is
/// available, else the legacy project-root `.arcconfig` read (preserves
/// the old leniency when sources are unreadable).
fn engine_param_from(
    merged: Option<&get_config::MergedConfig>,
    project_root: &str,
    key: &str,
) -> Option<serde_json::Value> {
    if let Some(merged) = merged {
        return get_config::merged_value(key, merged).map(|(_, value)| value);
    }
    read_arcconfig(project_root)?.get(key).cloned()
}

/// String form of [`engine_param_from`]: missing and non-string values
/// read empty, like the old project-root read.
fn engine_param_string_from(
    merged: Option<&get_config::MergedConfig>,
    project_root: &str,
    key: &str,
) -> Option<String> {
    engine_param_from(merged, project_root, key)?
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
/// Engine params read through the config merge (UNIT-U13 follow-through).
pub fn prepare_xunit_run(ctx: &EngineContext) -> Result<XunitPlan, EngineError> {
    let merged = lenient_engine_merged(&ctx.project_root);
    prepare_xunit_run_with_merged(ctx, merged.as_ref())
}

/// [`prepare_xunit_run`] with an explicit merged config (`None` falls back
/// to the project-root `.arcconfig`, like the pre-merge read).
pub fn prepare_xunit_run_with_merged(
    ctx: &EngineContext,
    merged: Option<&get_config::MergedConfig>,
) -> Result<XunitPlan, EngineError> {
    let is_windows = cfg!(windows);
    let discovery = engine_param_from(merged, &ctx.project_root, unit_xunit::DISCOVERY_KEY)
        .and_then(|v| parse_discovery_rules(&v));
    let hint = engine_param_string_from(merged, &ctx.project_root, unit_xunit::XUNIT_BINARY_KEY);
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

/// Load the C# cover configuration from the config merge: the cscover hint
/// plus the match/excluded filters (UNIT-U13 follow-through). `None` when
/// coverage is disabled (then every assembly runs the plain xUnit future).
/// A `None` merged load falls back to the project-root `.arcconfig`.
fn load_csharp_cover(
    ctx: &EngineContext,
    merged: Option<&get_config::MergedConfig>,
) -> Result<Option<CsharpCover>, EngineError> {
    let hint = engine_param_string_from(merged, &ctx.project_root, unit_csharp::CSCOVER_BINARY_KEY);
    let cover_engine = unit_csharp::load_cover_engine(
        &ctx.project_root,
        hint.as_deref(),
        ctx.enable_coverage,
        &|p| Path::new(p).exists(),
    )?;
    let Some(cover_engine) = cover_engine else {
        return Ok(None);
    };
    let match_regex =
        engine_param_string_from(merged, &ctx.project_root, unit_csharp::COVERAGE_MATCH_KEY);
    let excluded = engine_param_from(
        merged,
        &ctx.project_root,
        unit_csharp::COVERAGE_EXCLUDED_KEY,
    )
    .and_then(|v| parse_excluded_files(&v));
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
/// lines 216-285) in first-seen file order (UNIT-U20). Line counts come
/// from the real filesystem, matching PHP `count(file(...))`.
fn read_run_coverage(
    cover_output: Option<&str>,
    project_root: &str,
    cache: &mut unit_csharp::CoverageCache,
) -> Result<Option<Vec<(String, String)>>, EngineError> {
    let Some(cover_file) = cover_output else {
        return Ok(None);
    };
    let xml = std::fs::read_to_string(cover_file)
        .map_err(|e| EngineError::Failed(format!("cannot read '{cover_file}': {e}")))?;
    cache
        .read_cached_ordered(cover_file, &xml, project_root, &|p| {
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
    let results = unit_csharp::parse_assembly_results_ordered(&xml, coverage.as_deref())?;
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
    let merged = lenient_engine_merged(&ctx.project_root);
    let cover = load_csharp_cover(ctx, merged.as_ref())?;
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

/// Build a phpunit engine from the config merge (`prepareConfigFile`,
/// `PhpunitTestEngine.php:254-278`): `phpunit_config` plus
/// `unit.phpunit.binary` (UNIT-U13 follow-through).
#[must_use]
pub fn phpunit_engine_from_config(project_root: &str) -> unit_phpunit::PhpunitEngine {
    let merged = lenient_engine_merged(project_root);
    phpunit_engine_from_merged(project_root, merged.as_ref())
}

/// [`phpunit_engine_from_config`] with an explicit merged config (`None`
/// falls back to the project-root `.arcconfig`, like the pre-merge read).
#[must_use]
pub fn phpunit_engine_from_merged(
    project_root: &str,
    merged: Option<&get_config::MergedConfig>,
) -> unit_phpunit::PhpunitEngine {
    let config_value = engine_param_string_from(merged, project_root, unit_phpunit::CONFIG_KEY);
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
    let configured = engine_param_string_from(merged, project_root, unit_phpunit::BINARY_KEY);
    let binary = match configured {
        None => unit_phpunit::PHPUNIT_BINARY.to_string(),
        Some(bin) if binary_exists(&bin) => bin,
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
    let merged = lenient_engine_merged(&ctx.project_root);
    run_phutil_with_runner_merged(ctx, runner, engine, php, merged.as_ref())
}

/// [`run_phutil_with_runner`] with an explicit merged config for the
/// init-script param (`None` falls back to the project-root `.arcconfig`).
pub fn run_phutil_with_runner_merged(
    ctx: &EngineContext,
    runner: &dyn CommandRunner,
    engine: &str,
    php: &dyn Fn() -> Option<Vec<String>>,
    merged: Option<&get_config::MergedConfig>,
) -> Result<Vec<EngineUnitResult>, EngineError> {
    let Some(prefix) = php() else {
        return Err(EngineError::Usage(unit_phutil::missing_php_message(engine)));
    };
    let init_script =
        engine_param_string_from(merged, &ctx.project_root, unit_phutil::INIT_SCRIPT_KEY);
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
    if !SINGLE_AVAILABLE.contains(&engine_type) {
        return Err(EngineError::Usage(not_a_subclass_message(engine_type)));
    }
    EngineContext::check_set_run_all_tests(
        supports_run_all_tests(engine_type),
        engine_type,
        ctx.run_all_tests,
    )
    .map_err(EngineError::Usage)?;
    // UNIT-U13 follow-through: one merged load feeds every engine-param
    // read below (missing/unreadable sources fall back per-param).
    let merged = lenient_engine_merged(&ctx.project_root);
    let merged_ref = merged.as_ref();
    match engine_type {
        "NoseTestEngine" => {
            unit_nose::NoseEngine::new().run_with(ctx, runner, &unit_nose::RealNoseFs)
        }
        "PytestTestEngine" => unit_pytest::PytestEngine::new().run_with(ctx, runner, &RealPytestFs),
        "PhpunitTestEngine" => {
            let engine = phpunit_engine_from_merged(&ctx.project_root, merged_ref);
            // Surface a configured-but-missing phpunit config verbatim
            // (`PhpunitTestEngine.php:263-266`).
            if let Some(config_value) =
                engine_param_string_from(merged_ref, &ctx.project_root, unit_phpunit::CONFIG_KEY)
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
        "PhutilUnitTestEngine" | "phutil" => {
            run_phutil_with_runner_merged(ctx, runner, engine_type, php, merged_ref)
        }
        "XUnitTestEngine" => run_xunit_with_runner(ctx, runner),
        "CSharpToolsTestEngine" => run_csharp_with_runner(ctx, runner),
        _ => Err(EngineError::Usage(not_a_subclass_message(engine_type))),
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
    run_single_report_classified(
        engine_type,
        project_root,
        paths,
        run_all_tests,
        enable_coverage,
        runner,
    )
    .map_err(|error| error.message)
}

/// [`run_single_report`] with errors classified for the R4 envelope,
/// preserving `NoEffect` (stdout, exit 0).
pub fn run_single_report_classified(
    engine_type: &str,
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<ConfigReport, ClassifiedError> {
    let ctx = EngineContext {
        project_root: project_root.to_string(),
        paths: paths.to_vec(),
        enable_coverage,
        run_all_tests,
    };
    let started = std::time::Instant::now();
    let results =
        run_single_with_runner(engine_type, &ctx, runner).map_err(classify_engine_error)?;
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
    run_config_with_runner_classified(
        planned,
        project_root,
        paths,
        run_all_tests,
        enable_coverage,
        runner,
    )
    .map_err(|error| error.message)
}

/// [`run_config_with_runner`] with errors classified for the R4
/// envelope, preserving `NoEffect` (stdout, exit 0).
pub fn run_config_with_runner_classified(
    planned: &[PlannedEngine],
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<ConfigReport, ClassifiedError> {
    run_config_with_php_classified(
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
    run_config_with_php_classified(
        planned,
        project_root,
        paths,
        run_all_tests,
        enable_coverage,
        runner,
        php,
    )
    .map_err(|error| error.message)
}

/// [`run_config_with_php`] with errors classified for the R4 envelope,
/// preserving `NoEffect` (stdout, exit 0).
pub fn run_config_with_php_classified(
    planned: &[PlannedEngine],
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
    php: &dyn Fn() -> Option<Vec<String>>,
) -> Result<ConfigReport, ClassifiedError> {
    run_config_with_echo_classified(
        planned,
        project_root,
        paths,
        run_all_tests,
        enable_coverage,
        runner,
        php,
    )
    .map(|(report, _)| report)
}

/// [`run_config_with_php_classified`], plus one echo flag per merged result
/// (UNIT-U1): the proxy echoes only non-echoing children's results
/// (`ArcanistConfigurationDrivenUnitTestEngine.php:151-157`), and the
/// workflow echoes nothing itself (`:172-174`), so full runs print zero
/// per-result lines when every child echoes (all ported engines do).
pub fn run_config_with_echo_classified(
    planned: &[PlannedEngine],
    project_root: &str,
    paths: &[String],
    run_all_tests: bool,
    enable_coverage: Option<bool>,
    runner: &dyn CommandRunner,
    php: &dyn Fn() -> Option<Vec<String>>,
) -> Result<(ConfigReport, Vec<bool>), ClassifiedError> {
    let child_supports: Vec<bool> = planned
        .iter()
        .map(|engine| supports_run_all_tests(&engine.engine_type))
        .collect();
    if run_all_tests && !unit_config::supports_run_all_tests(&child_supports) {
        return Err(ClassifiedError::usage(
            "Engine 'ArcanistConfigurationDrivenUnitTestEngine' does not support --everything."
                .to_string(),
        ));
    }
    let top = EngineContext {
        project_root: project_root.to_string(),
        paths: paths.to_vec(),
        enable_coverage,
        run_all_tests,
    };
    let started = std::time::Instant::now();
    let mut runs = Vec::new();
    let mut echo_children = Vec::new();
    for engine in planned {
        let child =
            unit_config::child_context(&top, engine, supports_run_all_tests(&engine.engine_type));
        echo_children.push(!should_echo_test_results(&engine.engine_type));
        runs.push(run_single_with_php(
            &engine.engine_type,
            &child,
            runner,
            php,
        ));
    }
    let echo = config_echo_mask(&echo_children, &runs);
    let results = unit_config::merge_engine_runs(runs).map_err(classify_engine_error)?;
    let duration_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let (passed, failed) = count_results(&results);
    let success =
        arc_rs::unit_engine::overall_result(&results) == arc_rs::unit_engine::WORKFLOW_RESULT_OKAY;
    let report = ConfigReport {
        engine: "ArcanistConfigurationDrivenUnitTestEngine".to_string(),
        passed,
        failed,
        duration_ms,
        success,
        results,
    };
    Ok((report, echo))
}

/// One echo flag per merged result for a config run (UNIT-U1): each
/// successful child's results take that child's proxy-echo flag, and
/// no-effect children contribute nothing (mirroring `merge_engine_runs`).
fn config_echo_mask(
    echo_children: &[bool],
    runs: &[Result<Vec<EngineUnitResult>, EngineError>],
) -> Vec<bool> {
    let mut echo = Vec::new();
    for (child_echo, run) in echo_children.iter().zip(runs.iter()) {
        if let Ok(results) = run {
            echo.extend(std::iter::repeat_n(*child_echo, results.len()));
        }
    }
    echo
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
/// (`ArcanistUnitConsoleRenderer.php:5-30`): `  <LABEL>[ time] name`,
/// plus `userData` for non-passing results. Both modes compose the same
/// `getFormattedResult` badge (lines 32-54) and `formatTestDuration`
/// bucket (lines 56-81) through `console_format`; non-TTYs just strip the
/// markup, keeping the star, spaces, and padding like PHP (UNIT-U10).
#[must_use]
pub fn render_unit_result(result: &EngineUnitResult) -> String {
    render_unit_result_with_ansi(result, terminal::ansi_enabled())
}

/// Render one result with an explicit ANSI flag (see
/// [`render_unit_result`]).
#[must_use]
pub fn render_unit_result_with_ansi(result: &EngineUnitResult, ansi: bool) -> String {
    if ansi {
        render_unit_result_tty(result)
    } else {
        render_unit_result_plain(result)
    }
}

/// One `getFormattedResult` badge (lines 32-54) with an explicit ANSI
/// flag: `<bg:green/red/yellow>** LABEL **</bg>` like PHP. The stripped
/// form keeps the badge padding (` PASS `), like PHP (UNIT-U10).
fn format_result_label(code: &str, ansi: bool) -> String {
    let (color, label) = match code {
        arc_rs::unit_engine::RESULT_PASS => ("green", "PASS"),
        arc_rs::unit_engine::RESULT_FAIL => ("red", "FAIL"),
        arc_rs::unit_engine::RESULT_SKIP => ("yellow", "SKIP"),
        arc_rs::unit_engine::RESULT_BROKEN => ("red", "BROKEN"),
        arc_rs::unit_engine::RESULT_UNSOUND => ("yellow", "UNSOUND"),
        _ => return String::new(),
    };
    terminal::console_format(&format!("<bg:{color}>** %s **</bg>"), &[label], ansi)
}

/// The fast-run star: U+2605, or `*` on Windows consoles (lines 60-64).
fn duration_star() -> &'static str {
    if cfg!(windows) { "*" } else { "\u{2605}" }
}

/// One `formatTestDuration` value (lines 56-81) with an explicit ANSI
/// flag: at most 50ms green with a star, 200ms green, 500ms yellow,
/// else red, each with two trailing spaces (one after the star).
fn format_test_duration(seconds: f64, ansi: bool) -> String {
    let duration = format_time(seconds);
    let milliseconds = seconds * 1000.0;
    if milliseconds <= 50.0 {
        terminal::console_format(
            "<fg:green>%s</fg><fg:yellow>%s</fg> ",
            &[&duration, duration_star()],
            ansi,
        )
    } else if milliseconds <= 200.0 {
        terminal::console_format("<fg:green>%s</fg>  ", &[&duration], ansi)
    } else if milliseconds <= 500.0 {
        terminal::console_format("<fg:yellow>%s</fg>  ", &[&duration], ansi)
    } else {
        terminal::console_format("<fg:red>%s</fg>  ", &[&duration], ansi)
    }
}

/// `namespace::name` (or bare `name`) for one result (lines 12-16).
fn result_display_name(result: &EngineUnitResult) -> String {
    match result.get_namespace() {
        Some(namespace) if !namespace.is_empty() => {
            format!("{}::{}", namespace, result.get_name())
        }
        _ => result.get_name().to_string(),
    }
}

/// TTY rendering: the badge, duration, and name composed with
/// `sprintf("  %s %s\n", ...)` like PHP, plus raw `userData`.
fn render_unit_result_tty(result: &EngineUnitResult) -> String {
    render_unit_result_composed(result, true)
}

/// Plain rendering: the same composition with markup stripped, like PHP
/// on a non-TTY (UNIT-U10).
fn render_unit_result_plain(result: &EngineUnitResult) -> String {
    render_unit_result_composed(result, false)
}

/// Badge, duration, and name composed with `sprintf("  %s %s\n", ...)`
/// like PHP (lines 18-23), plus raw `userData` for non-passing results
/// (lines 25-28). The ANSI flag only switches markup on/off.
fn render_unit_result_composed(result: &EngineUnitResult, ansi: bool) -> String {
    let code = result.get_result();
    let label = format_result_label(code, ansi);
    let duration = if code == arc_rs::unit_engine::RESULT_PASS {
        format!(
            " {}",
            format_test_duration(result.get_duration().unwrap_or(0.0), ansi)
        )
    } else {
        String::new()
    };
    let mut out = format!("  {label}{duration} {}\n", result_display_name(result));
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
    collect_coverage_ordered(results).into_iter().collect()
}

/// [`collect_coverage`] in first-seen file order (UNIT-U12): PHP's
/// `$coverage` map preserves insertion order, which survives `asort`
/// ties, so the render path keeps it instead of hashing.
#[must_use]
pub fn collect_coverage_ordered(results: &[EngineUnitResult]) -> Vec<(String, Vec<String>)> {
    let mut coverage: Vec<(String, Vec<String>)> = Vec::new();
    for result in results {
        // Per-result files keep document order (UNIT-U20; legacy maps
        // fall back to sorted pairs).
        let Some(files) = result.coverage_ordered() else {
            continue;
        };
        for (file, report) in &files {
            match coverage.iter_mut().find(|(name, _)| name == file) {
                Some((_, reports)) => reports.push(report.clone()),
                None => coverage.push((file.clone(), vec![report.clone()])),
            }
        }
    }
    coverage
}

/// Merge per-file reports and compute fractions
/// (`ArcanistUnitWorkflow.php:201-217`): `C / (C + U)` else `0`.
/// Returns `(sorted_file_coverage, file_reports)`; `paths` seeds every
/// requested file at `0` like PHP `array_fill_keys($paths, 0)`. Frozen
/// unordered form (filename tiebreak); the render path uses
/// [`merge_file_coverage_ordered`] (UNIT-U12).
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

/// [`merge_file_coverage`] over first-seen coverage order (UNIT-U12): seeds
/// `paths` in order like PHP `array_fill_keys($paths, 0)`, updates covered
/// files in place, appends new ones, then sorts by fraction with a STABLE
/// sort so ties keep insertion order like PHP `asort`.
#[must_use]
pub fn merge_file_coverage_ordered(
    paths: &[String],
    coverage: &[(String, Vec<String>)],
) -> (Vec<(String, f64)>, HashMap<String, String>) {
    let mut ordered: Vec<(String, f64)> = Vec::new();
    for path in paths {
        if !ordered.iter().any(|(name, _)| name == path) {
            ordered.push((path.clone(), 0.0));
        }
    }
    let mut file_reports: HashMap<String, String> = HashMap::new();
    for (file, reports) in coverage {
        let merged = EngineUnitResult::merge_coverage(reports).unwrap_or_default();
        let fraction = arc_rs::unit_engine::coverage_fraction(&merged);
        match ordered.iter_mut().find(|(name, _)| name == file) {
            Some(slot) => slot.1 = fraction,
            None => ordered.push((file.clone(), fraction)),
        }
        file_reports.insert(file.clone(), merged);
    }
    ordered.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    (ordered, file_reports)
}

/// Render the coverage report (`ArcanistUnitWorkflow.php:218-239`):
/// `\nCOVERAGE REPORT\n` plus `    %3d%%     file` lines and optional
/// detailed reports. TTYs render the `__`/`**` console markup like
/// PHP; non-TTYs keep the historical plain rendering byte-identical.
#[must_use]
pub fn render_coverage_report(
    sorted: &[(String, f64)],
    file_reports: &HashMap<String, String>,
    detailed: bool,
    project_root: &Path,
) -> String {
    render_coverage_report_with_ansi(
        sorted,
        file_reports,
        detailed,
        project_root,
        terminal::ansi_enabled(),
    )
}

/// Render the coverage report with an explicit ANSI flag (see
/// [`render_coverage_report`]).
#[must_use]
pub fn render_coverage_report_with_ansi(
    sorted: &[(String, f64)],
    file_reports: &HashMap<String, String>,
    detailed: bool,
    project_root: &Path,
    ansi: bool,
) -> String {
    let mut out = if ansi {
        terminal::console_format("\n__%s__\n", &["COVERAGE REPORT"], true)
    } else {
        String::from("\nCOVERAGE REPORT\n")
    };
    for (file, fraction) in sorted {
        let pct = (100.0 * fraction) as i32;
        out.push_str(&coverage_file_line(pct, file, ansi));
        if detailed {
            let full = project_root.join(file);
            if let Some(report) = file_reports.get(file)
                && full.is_file()
                && let Ok(data) = std::fs::read_to_string(&full)
            {
                out.push_str(&render_detailed_coverage_report_with_ansi(
                    &data, report, ansi,
                ));
            }
        }
    }
    out
}

/// One `    **%3d%%**     file` coverage line (workflow line 222-224)
/// with an explicit ANSI flag.
fn coverage_file_line(pct: i32, file: &str, ansi: bool) -> String {
    if ansi {
        let pct_text = format!("{pct:>3}%");
        terminal::console_format("    **%s**     %s\n", &[&pct_text, file], true)
    } else {
        format!("    {pct:>3}%     {file}\n")
    }
}

/// Render one file's detailed coverage (`ArcanistUnitWorkflow.php:292-331`).
/// Both modes compose the same `%5d  ` plus background-wrapped line;
/// non-TTYs just strip the markup, so no mark letter appears (UNIT-U11).
#[must_use]
pub fn render_detailed_coverage_report(data: &str, report: &str) -> String {
    render_detailed_coverage_report_with_ansi(data, report, terminal::ansi_enabled())
}

/// Render one file's detailed coverage with an explicit ANSI flag
/// (see [`render_detailed_coverage_report`]).
#[must_use]
pub fn render_detailed_coverage_report_with_ansi(data: &str, report: &str, ansi: bool) -> String {
    render_detailed_composed(data, report, ansi)
}

/// `%5d  ` plus the padded line wrapped in the mark's background (`C`
/// green, `U` red, `X` magenta, else plain). The ANSI flag only switches
/// markup on/off, like PHP on/off a TTY.
fn render_detailed_composed(data: &str, report: &str, ansi: bool) -> String {
    let report_bytes = report.as_bytes();
    let mut out = String::new();
    for (n, line) in data.split('\n').enumerate() {
        out.push_str(&detailed_line(
            n + 1,
            line,
            detail_mark(report_bytes.get(n).copied()),
            ansi,
        ));
    }
    out
}

/// Pad a source line to 80 columns like PHP `str_pad($line, 80, ' ')`,
/// which counts BYTES and never truncates (UNIT-U24).
fn pad_detailed_line(line: &str) -> String {
    if line.len() >= 80 {
        return line.to_string();
    }
    let mut padded = String::with_capacity(80);
    padded.push_str(line);
    for _ in line.len()..80 {
        padded.push(' ');
    }
    padded
}

/// The effective mark for one line: PHP's `empty($report[$n])`
/// maps a missing report byte (and `"0"`) to `N`.
fn detail_mark(byte: Option<u8>) -> char {
    match byte {
        None | Some(b'0') => 'N',
        Some(other) => other as char,
    }
}

/// One detailed-coverage line for `mark` with an explicit ANSI flag.
fn detailed_line(number: usize, line: &str, mark: char, ansi: bool) -> String {
    let padded = pad_detailed_line(line);
    let painted = match mark {
        'C' => terminal::console_format("<bg:green> %s </bg>", &[&padded], ansi),
        'U' => terminal::console_format("<bg:red> %s </bg>", &[&padded], ansi),
        'X' => terminal::console_format("<bg:magenta> %s </bg>", &[&padded], ansi),
        _ => format!(" {padded} "),
    };
    format!("{number:>5}  {painted}\n")
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

/// Render an optional duration like `json_encode`: `null` when unset,
/// bare when set as an integer, float form otherwise (UNIT-U18).
fn php_json_duration_value(duration: Option<f64>, is_int: bool) -> String {
    match duration {
        None => "null".to_string(),
        Some(value) if is_int => format!("{}", value as i64),
        Some(value) => php_json_number(value),
    }
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
/// Unset `name`/`result` render null like PHP (UNIT-U19).
fn append_result_ugly(out: &mut String, result: &EngineUnitResult) {
    let dict = result.to_dictionary();
    out.push_str("{\"namespace\":");
    append_json_string_or_null(out, dict.namespace.as_deref(), true);
    out.push_str(",\"name\":");
    append_json_string_or_null(out, dict.name.as_deref(), true);
    out.push_str(",\"link\":");
    append_json_string_or_null(out, dict.link.as_deref(), true);
    out.push_str(",\"result\":");
    append_json_string_or_null(out, dict.result.as_deref(), true);
    out.push_str(",\"duration\":");
    out.push_str(&php_json_duration_value(
        dict.duration,
        dict.duration_is_int,
    ));
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
        ("name", php_json_string_or_null(dict.name.as_deref(), false)),
        ("link", php_json_string_or_null(dict.link.as_deref(), false)),
        (
            "result",
            php_json_string_or_null(dict.result.as_deref(), false),
        ),
        (
            "duration",
            php_json_duration_value(dict.duration, dict.duration_is_int),
        ),
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
/// Nested `extra`/`coverage` pairs keep insertion order (UNIT-U20).
/// Remaining known divergence: Harbormaster upload encoding (the shared
/// conduit client serializes with serde).
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
/// in `toDictionary` key order with `null` for missing values (including
/// unset `name`/`result`, T1153 UNIT-U19; integer durations stay bare,
/// T1153 UNIT-U18; insertion order on the wire, T1153 UNIT-U21).
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
            map.insert(
                "name".to_string(),
                d.name
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
            map.insert(
                "link".to_string(),
                d.link
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
            map.insert(
                "result".to_string(),
                d.result
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
            map.insert(
                "duration".to_string(),
                match d.duration {
                    None => serde_json::Value::Null,
                    Some(v) if d.duration_is_int => serde_json::Value::from(v as i64),
                    Some(v) => serde_json::Number::from_f64(v)
                        .map_or(serde_json::Value::Null, serde_json::Value::Number),
                },
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

/// Modern unit dictionary (`ArcanistWorkflow.php:2243-2253` at ceb):
/// drop `null` values (order kept), rename non-empty `userData` to a
/// trailing `details` (T1153 UNIT-U21: insertion order end to end, like
/// PHP's `json_encode`).
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user_file = config::arcrc_path();
    upload_results_with(
        target_phid,
        overall,
        results,
        conduit_uri,
        &cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
}

/// [`upload_results`] with an explicit working copy, user file, and
/// [`get_config::LoadOptions`], so `--config`/`--config-file`/
/// `--arcrc-file` reach Harbormaster uploads like stock PHP.
pub fn upload_results_with(
    target_phid: &str,
    overall: i32,
    results: &[EngineUnitResult],
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<(), String> {
    let client = config::client_for_with(conduit_uri, dir, user_file, opts)?;
    let params = sendmessage_params(target_phid, overall, results);
    client
        .call("harbormaster.sendmessage", &params)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// `harbormaster.sendmessage` params for `results`
/// (`ArcanistUnitWorkflow.php:421-427` at ceb): `buildTargetPHID`,
/// modern `unit` dicts, `type`, in that order (T1153 UNIT-U21: PHP
/// sends insertion order via `json_encode`; `preserve_order` keeps
/// this builder's order on the wire too).
#[must_use]
pub fn sendmessage_params(
    target_phid: &str,
    overall: i32,
    results: &[EngineUnitResult],
) -> serde_json::Value {
    let raw = results_to_json(results);
    let modern: Vec<serde_json::Value> = raw
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(modern_unit_value)
        .collect();
    serde_json::json!({
        "buildTargetPHID": target_phid,
        "unit": modern,
        "type": harbormaster_type(overall),
    })
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
    render_workflow_output_with_ansi(
        results,
        paths,
        output_format,
        detailed,
        project_root,
        terminal::ansi_enabled(),
    )
}

/// Render the workflow output with an explicit ANSI flag: the `full`
/// rendering threads it into the per-result and coverage renderers;
/// `json`/`ugly`/`none` carry no markup in any mode. Every result echoes
/// (the single-engine case, where `shouldEchoTestResults` is true).
#[must_use]
pub fn render_workflow_output_with_ansi(
    results: &[EngineUnitResult],
    paths: &[String],
    output_format: &str,
    detailed: bool,
    project_root: &Path,
    ansi: bool,
) -> String {
    render_workflow_output_with_echo(
        results,
        paths,
        output_format,
        detailed,
        project_root,
        ansi,
        None,
    )
}

/// [`render_workflow_output_with_ansi`] with one echo flag per result
/// (UNIT-U1): config runs echo only non-echoing children's results, so a
/// `false` flag skips that result's line. Coverage and machine formats
/// still see every result.
#[must_use]
pub fn render_workflow_output_with_echo(
    results: &[EngineUnitResult],
    paths: &[String],
    output_format: &str,
    detailed: bool,
    project_root: &Path,
    ansi: bool,
    echo: Option<&[bool]>,
) -> String {
    match output_format {
        "json" => results_to_json_text(results, true),
        "ugly" => results_to_json_text(results, false),
        "none" => String::new(),
        _ => render_full_output(results, paths, detailed, project_root, ansi, echo),
    }
}

/// The `full` rendering: per-result lines (gated by `echo`) plus the
/// coverage report when coverage is present.
fn render_full_output(
    results: &[EngineUnitResult],
    paths: &[String],
    detailed: bool,
    project_root: &Path,
    ansi: bool,
    echo: Option<&[bool]>,
) -> String {
    let mut out = String::new();
    for (index, result) in results.iter().enumerate() {
        let echoed = echo.as_ref().map(|mask| mask.get(index)).unwrap_or(None);
        if echoed == Some(&false) {
            continue;
        }
        out.push_str(&render_unit_result_with_ansi(result, ansi));
    }
    let coverage = collect_coverage_ordered(results);
    if !coverage.is_empty() {
        let (sorted, reports) = merge_file_coverage_ordered(paths, &coverage);
        out.push_str(&render_coverage_report_with_ansi(
            &sorted,
            &reports,
            detailed,
            project_root,
            ansi,
        ));
    }
    out
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
    match run_cargo_collecting(opts, cwd) {
        Ok((code, _)) => code,
        Err(error) => error.report(),
    }
}

/// [`run_cargo_selection`] that also returns the aggregate results as
/// engine dicts, so `diff` can collect them (T1153 DIFF-D22).
fn run_cargo_collecting(
    opts: &Options,
    cwd: &Path,
) -> Result<(ExitCode, Vec<EngineUnitResult>), ClassifiedError> {
    // The `--everything` plus paths gate already ran in `plan_run`.
    if let Some(warning) = cargo_coverage_warning(opts) {
        eprintln!("{warning}");
    }
    // T1153 R4 (UNIT-U5): cargo and upload failures are generic.
    let report = execute(opts, cwd, None).map_err(ClassifiedError::exception)?;
    let format = output_format(opts);
    print_cargo_output(&report, &format);
    upload_cargo_target(opts, &report).map_err(ClassifiedError::exception)?;
    let code = if report.success {
        ExitCode::SUCCESS
    } else {
        exit_code_for_overall(arc_rs::unit_engine::WORKFLOW_RESULT_FAIL)
    };
    let results = cargo_aggregate_results(&report).map_err(ClassifiedError::exception)?;
    Ok((code, results))
}

/// Cargo aggregate results as engine results (T1153 DIFF-D22): one
/// pass/fail result named after the engine, like PHP's
/// `getTestResults()` entries.
fn cargo_aggregate_results(report: &EngineReport) -> Result<Vec<EngineUnitResult>, String> {
    let mut out = Vec::with_capacity(report.results.len());
    for result in &report.results {
        let mut converted = EngineUnitResult::new();
        converted.set_name(&result.name)?;
        converted.set_result(result.outcome.as_str());
        converted.set_duration(result.duration_secs);
        out.push(converted);
    }
    Ok(out)
}

/// Run one engine named by `--engine` or `unit.engine`.
fn run_single_selection(
    opts: &Options,
    cwd: &Path,
    class: &str,
    paths: &[String],
    ansi: bool,
) -> ExitCode {
    match run_single_collecting(opts, cwd, class, paths, ansi) {
        Ok((code, _)) => code,
        Err(error) => error.report(),
    }
}

/// [`run_single_selection`] that also returns the per-test results, so
/// `diff` can collect them (T1153 DIFF-D22).
fn run_single_collecting(
    opts: &Options,
    cwd: &Path,
    class: &str,
    paths: &[String],
    ansi: bool,
) -> Result<(ExitCode, Vec<EngineUnitResult>), ClassifiedError> {
    if !opts.cargo_args.is_empty() {
        // T1153 R4 (UNIT-U4): an invocation mistake (Rust-only; PHP
        // has no cargo engine), reported as usage.
        return Err(ClassifiedError::usage(
            "cargo test args after '--' require the cargo engine".to_string(),
        ));
    }
    let root = project_root_path(cwd);
    let root_string = root.to_string_lossy().replace('\\', "/");
    let runner = RealRunner;
    let coverage = enable_coverage(opts);
    // T1153 R4 (UNIT-U3, UNIT-U4, UNIT-U5): the classified report
    // preserves `NoEffect` (stdout, exit 0), usage, and generic.
    match run_single_report_classified(
        class,
        &root_string,
        paths,
        opts.everything,
        coverage,
        &runner,
    ) {
        Ok(report) => finish_php_report_collecting(opts, &root, paths, &report, ansi, None),
        Err(error) => Err(error),
    }
}

/// Run a configuration-driven (`.arcunit`) selection.
fn run_config_selection(
    opts: &Options,
    engines: &[PlannedEngine],
    root: &Path,
    paths: &[String],
    ansi: bool,
) -> ExitCode {
    match run_config_collecting(opts, engines, root, paths, ansi) {
        Ok((code, _)) => code,
        Err(error) => error.report(),
    }
}

/// [`run_config_selection`] that also returns the per-test results, so
/// `diff` can collect them (T1153 DIFF-D22).
fn run_config_collecting(
    opts: &Options,
    engines: &[PlannedEngine],
    root: &Path,
    paths: &[String],
    ansi: bool,
) -> Result<(ExitCode, Vec<EngineUnitResult>), ClassifiedError> {
    if !opts.cargo_args.is_empty() {
        // T1153 R4 (UNIT-U4): an invocation mistake (see above).
        return Err(ClassifiedError::usage(
            "cargo test args after '--' require the cargo engine".to_string(),
        ));
    }
    let root_string = root.to_string_lossy().replace('\\', "/");
    let runner = RealRunner;
    let coverage = enable_coverage(opts);
    // T1153 R4 (UNIT-U3, UNIT-U4, UNIT-U5): classified (see above).
    // UNIT-U1: the proxy echoes only non-echoing children's results.
    match run_config_with_echo_classified(
        engines,
        &root_string,
        paths,
        opts.everything,
        coverage,
        &runner,
        &resolve_php_prefix,
    ) {
        Ok((report, echo)) => {
            finish_php_report_collecting(opts, root, paths, &report, ansi, Some(&echo))
        }
        Err(error) => Err(error),
    }
}

/// Print a PHP-engine report per the output format, upload to `target` when
/// set, and map the overall result to the process exit code
/// (`ArcanistUnitWorkflow.php:184-282`). `echo` gates per-result lines
/// (UNIT-U1): `None` echoes all (single runs), `Some` masks (config runs).
/// Also returns the per-test results, so `diff` can collect them (T1153
/// DIFF-D22).
fn finish_php_report_collecting(
    opts: &Options,
    project_root: &Path,
    paths: &[String],
    report: &ConfigReport,
    ansi: bool,
    echo: Option<&[bool]>,
) -> Result<(ExitCode, Vec<EngineUnitResult>), ClassifiedError> {
    let format = output_format(opts);
    let text = render_workflow_output_with_echo(
        &report.results,
        paths,
        &format,
        opts.detailed_coverage,
        project_root,
        ansi,
        echo,
    );
    print!("{text}");
    let overall = arc_rs::unit_engine::overall_result(&report.results);
    if let Some(target) = opts.target.as_deref()
        && let Err(msg) =
            upload_results_for_options(opts, project_root, target, overall, &report.results)
    {
        // T1153 R4 (UNIT-U5): upload failures are generic.
        return Err(ClassifiedError::exception(msg));
    }
    Ok((exit_code_for_overall(overall), report.results.clone()))
}

/// Upload through [`upload_results_with`] with the run's config overrides.
fn upload_results_for_options(
    opts: &Options,
    dir: &Path,
    target: &str,
    overall: i32,
    results: &[EngineUnitResult],
) -> Result<(), String> {
    let cli = get_config::load_options_from_cli(
        &opts.config,
        &opts.config_file,
        opts.arcrc_file.as_deref(),
    )?;
    let user_file = config::arcrc_path();
    upload_results_with(
        target,
        overall,
        results,
        opts.conduit_uri.as_deref(),
        dir,
        user_file.as_deref(),
        &cli,
    )
}

fn parse_unit_options(argv: &[String]) -> Result<Options, ExitCode> {
    match parse_options(argv) {
        Ok(Some(opts)) => Ok(opts),
        Ok(None) => {
            print!("{}", usage());
            Err(ExitCode::SUCCESS)
        }
        // T1153 R4 (UNIT-U4): `unit` is a subprocess workflow:
        // usage errors render `Usage Exception:` with no usage dump
        // (`scripts/arcanist.php:432-444`).
        Err(msg) => Err(conduit::report(EnvelopeKind::Usage, &msg)),
    }
}

fn unit_working_dir() -> Result<PathBuf, ExitCode> {
    match std::env::current_dir() {
        Ok(dir) => Ok(dir),
        Err(e) => Err(conduit::report(
            EnvelopeKind::Exception,
            &format!("cannot read current directory: {e}"),
        )),
    }
}

/// Classify a path-selection error: `--everything` with paths and
/// missing explicit paths are usage errors
/// (`ArcanistUnitWorkflow.php:136-143`,
/// `ArcanistWorkflow.php:1631-1637`); VCS command failures are
/// generic.
fn classify_select_paths_error(message: String) -> ClassifiedError {
    if message.starts_with("You can not specify paths with --everything.")
        || (message.starts_with("Path '") && message.ends_with("' does not exist!"))
    {
        ClassifiedError::usage(message)
    } else {
        ClassifiedError::exception(message)
    }
}

/// UNIT-U23: `arc unit` requires a working copy and a repository API
/// (`ArcanistUnitWorkflow.php:109-115`); outside one, PHP throws a usage
/// error (`scripts/arcanist.php:260-266`).
fn check_working_copy_gate(cwd: &Path) -> Result<(), ClassifiedError> {
    if detect_vcs(cwd).is_none() {
        return Err(ClassifiedError::usage(
            "This command must be run in a Git, Mercurial or Subversion working copy.".to_string(),
        ));
    }
    Ok(())
}

/// A planned run: engine selection plus (for PHP engines) selected paths.
#[derive(Debug)]
enum PlannedRun {
    Cargo,
    Single {
        class: String,
        paths: Vec<String>,
    },
    Config {
        engines: Vec<PlannedEngine>,
        root: PathBuf,
        paths: Vec<String>,
    },
}

/// Plan the run in PHP bootstrap order: VCS supports, the working-copy
/// gate (UNIT-U23), the `--everything` plus paths conflict (UNIT-U9, first
/// line of PHP `run()`), engine discovery, then path selection. Discovery
/// stays ahead of selection (unlike PHP's select-then-engine): cargo paths
/// are test-name filters that must not validate as files.
fn plan_run(opts: &Options, cwd: &Path) -> Result<PlannedRun, ClassifiedError> {
    if let Some(vcs) = detect_vcs(cwd) {
        // T1153 R4 (UNIT-U4): VCS support refusals are usage errors.
        check_supports(vcs, &opts.provided).map_err(ClassifiedError::usage)?;
    }
    check_working_copy_gate(cwd)?;
    // T1153 R4 (UNIT-U4): `--everything` with paths is a usage error
    // (`ArcanistUnitWorkflow.php:136-143`), checked pre-discovery.
    check_everything_paths(opts.everything, &opts.paths).map_err(ClassifiedError::usage)?;
    // Cargo ignores paths/rev (it runs `cargo test` in the crate root).
    match discover_selection_classified(opts, cwd)? {
        Selection::Cargo => Ok(PlannedRun::Cargo),
        Selection::Single(class) => {
            let root = project_root_path(cwd);
            let paths = select_paths(opts, cwd, &root).map_err(classify_select_paths_error)?;
            Ok(PlannedRun::Single { class, paths })
        }
        Selection::Config { engines, root } => {
            let paths = select_paths(opts, cwd, &root).map_err(classify_select_paths_error)?;
            Ok(PlannedRun::Config {
                engines,
                root,
                paths,
            })
        }
    }
}

/// Report of a collecting child unit run (T1153 DIFF-D22).
#[derive(Debug, Clone)]
pub struct ChildUnitReport {
    /// Workflow exit code.
    pub code: ExitCode,
    /// Per-test results (`testResults`).
    pub results: Vec<EngineUnitResult>,
}

/// Run unit as `diff`'s child in `cwd`, collecting test-result dicts
/// (T1153 DIFF-D22: `runUnit()`,
/// `src/workflow/ArcanistDiffWorkflow.php:1213-1278`). Renders and
/// uploads like the standalone run; errors return unreported for the
/// caller to map (`NoEffect` prints with exit 0, everything else
/// aborts the diff, like PHP's child catches).
pub fn run_in_collecting(cwd: &Path, argv: &[String]) -> Result<ChildUnitReport, ClassifiedError> {
    let opts = match parse_options(argv) {
        Ok(Some(opts)) => opts,
        Ok(None) => {
            print!("{}", usage());
            return Err(ClassifiedError::usage(
                "unexpected --help in child unit argv".to_string(),
            ));
        }
        // T1153 R4 (UNIT-U4): usage errors abort the diff unreported
        // here (the caller renders them).
        Err(message) => return Err(ClassifiedError::usage(message)),
    };
    let ansi = terminal::ansi_enabled();
    let (code, results) = match plan_run(&opts, cwd)? {
        PlannedRun::Cargo => run_cargo_collecting(&opts, cwd)?,
        PlannedRun::Single { class, paths } => {
            run_single_collecting(&opts, cwd, &class, &paths, ansi)?
        }
        PlannedRun::Config {
            engines,
            root,
            paths,
        } => run_config_collecting(&opts, &engines, &root, &paths, ansi)?,
    };
    Ok(ChildUnitReport { code, results })
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
    let ansi = terminal::ansi_enabled();
    match plan_run(&opts, &cwd) {
        Ok(PlannedRun::Cargo) => run_cargo_selection(&opts, &cwd),
        Ok(PlannedRun::Single { class, paths }) => {
            run_single_selection(&opts, &cwd, &class, &paths, ansi)
        }
        Ok(PlannedRun::Config {
            engines,
            root,
            paths,
        }) => run_config_selection(&opts, &engines, &root, &paths, ansi),
        Err(error) => error.report(),
    }
}

#[cfg(test)]
#[path = "unit_tests.rs"]
mod unit_tests;
