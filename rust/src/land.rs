//! `arc land`: publish reviewed changes.
//!
//! Full-state port of `src/workflow/ArcanistLandWorkflow.php` (arguments,
//! prompts, usage strings) and the land engines
//! (`src/land/engine/ArcanistLandEngine.php`,
//! `ArcanistGitLandEngine.php`, `ArcanistMercurialLandEngine.php`).
//!
//! Flag names, defaults, remote/branch selection order, merge strategies
//! (`squash` and `merge`), status lines, banners, prompts, and error
//! strings are verbatim from PHP; each item cites its PHP source lines.
//! Argument parsing mirrors the D1 pattern (`diff.rs`, itself a port of
//! `ArcanistWorkflow::parseArguments()`): `--` ends flags, positionals
//! collect into the `ref` wildcard, `--bool=value` splices the value back
//! as the next token, and `--help` short-circuits.
//!
//! Console rendering goes through the shared terminal styling
//! helpers with a tty-gated ANSI flag (`Io::ansi`, set once by the
//! entry points): on TTYs, `tsprintf` markup (`**bold**`, `<bg:...>`,
//! `%!`/`%W`/`%?`/`%>` blocks) and log-engine colors render like PHP;
//! otherwise `tsprintf` markup renders as plain text and log statuses
//! render as `** LABEL ** message`. Statuses go to stderr (like
//! `ArcanistLogEngine::writeBytes()`); banners, commit lists, and prompts
//! go to stdout (like PHP `echo`).
//!
//! Divergences from PHP (all deliberate, all documented):
//! - `--conduit-uri` is threaded through every command (like the diff
//!   port); PHP reads endpoint configuration instead.
//! - `--push` does not exist: PHP always pushes unless `--hold` or
//!   `--preview` is given, and so does this port.
//! - Without Conduit credentials the port lands offline as one unassociated
//!   commit set with a synthesized message (plus a note); PHP requires
//!   Conduit and would fail. Revision discovery, gates needing revision
//!   data, and Differential commit messages only run when Conduit is
//!   reachable. The large-working-set gate is pure and always runs.
//! - `--revision D123` resolves to revision 123 (the diff port does the
//!   same for `--update`). PHP casts through `(int)$matches[1]`
//!   (`ArcanistSimpleSymbolRef.php:34`), which yields 0 for `D123`; matching
//!   that cast bug would break the flag, so the port resolves the digits.
//! - Land messages come from `differential.getcommitmessage`
//!   (`'edit' => false`,
//!   `src/ref/revision/ArcanistRevisionCommitMessageHardpointQuery.php:24-29`)
//!   and commit via stdin (`git commit -F -`,
//!   `ArcanistGitLandEngine.php:1792-1796`; `hg rebase --logfile -`,
//!   `ArcanistMercurialLandEngine.php:837-842`). Verified: PHP opens no
//!   `$EDITOR` anywhere in the land workflow or engines (the only editor
//!   mention is skipping `git p4 submit`'s editor with
//!   `-c git-p4.skipSubmitEdit=true`, `:553-561`), there is no amend
//!   step, and commit messages arrive verbatim from Conduit — so the
//!   port has no editor seam.
//! - Prompt answers ending in `*` save to user configuration (`~/.arcrc`)
//!   and `!` to the working-copy local config (`.git/arc/config`,
//!   `.hg/arc/config`), like `ArcanistPrompt::saveResponse()`.
//! - Banner bodies wrap at 78 columns (`%W`), `%?` hints wrap with the
//!   `?` marker and 6-space indent, and confirm prompts wrap with a
//!   4-space indent, all via a faithful `phutil_console_wrap()` port.
//! - Display summaries truncate to 64 glyphs via a faithful
//!   `PhutilUTF8StringTruncator` port (word boundaries, sentence-stop
//!   cuts, the `…` terminator).
//! - Revision/marker/buildable/build ref views render the
//!   `ArcanistRefView` `*`/`://` marker lines with glyph-truncated
//!   titles. Titles measure against a fixed 80-column terminal (PHP
//!   probes `tput cols`, which yields 80 in practice); the 16-glyph
//!   floor for missing widths is kept.
//! - Git/Perforce mode is supported: a `p4` remote selects it with the
//!   `P4 MODE` warning, requires squash, syncs (`P4 SYNC`), submits via
//!   `git p4 submit` (stdio inherited, like PHP passthru), and
//!   reconciles per `reconcileLocalState()`. PHP's automatic `p4`
//!   remote fallback (`ArcanistGitLandEngine.php:1241-1253`) checks a
//!   leftover variable that is always null there, so it never fires;
//!   the port follows the effective behavior (fall through to `origin`).
//! - Saved prompt responses concatenate the system, user, project, and
//!   local `prompts` lists in order with last-match-wins, like the PHP
//!   multi-source option. PHP requires list form and throws on anything
//!   else; the port also accepts a `{prompt: response}` map and skips
//!   malformed entries.
//! - The `MULTIPLE "ONTO" BRANCHES` error renders branch views from the
//!   local branch tips; PHP renders remote marker data.
//! - Mercurial remote-ref existence probes run `hg identify --id --rev REF
//!   REMOTE`; if that probe errors for any reason the ref is treated as
//!   new (PHP queries remote markers instead).
//! - Mercurial `selectCommits()` with a null into-commit queries
//!   `reverse(ancestors(symbol))`; PHP passes the null into-commit instead
//!   (`ArcanistMercurialLandEngine.php:711-716`), which looks like an
//!   upstream bug.
//! - Local-state capture/restore wraps the merge loop
//!   (`ArcanistRepositoryLocalState`): merge conflicts fail, dirty
//!   externals confirm, uncommitted changes stash after prompting (git
//!   stash, Mercurial shelve), failures restore the saved state, and
//!   the hold output lists the manual recovery commands. The `SAVE
//!   STATE` trace line is not printed (PHP only shows it with tracing
//!   enabled).
//! - Verified: PHP never updates the onto ref after pushing (it is set
//!   once in `execute()`, `ArcanistLandEngine.php:1120-1125`); neither
//!   does this port.
//! - `--preview` matches PHP (`ArcanistLandWorkflow.php:185-189`,
//!   `ArcanistLandEngine.php:1207-1212`): the run stops after the
//!   landing list with `PREVIEW`. Only `--conduit-uri` is a port
//!   extension (PHP has no such land flag).

extern crate self as arc_rs;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{
    commitgraph, conduit, config, diff_local, diff_submit, get_config, git, hg, land_checks,
    revision,
};

use crate::conduit::ClassifiedError;
use crate::terminal::{self, TVal};

/// Subcommand name, matching `arc land`.
pub const NAME: &str = "land";

const USAGE: &str = "usage: arc land [--hold] [--keep-branches] [--onto-remote REMOTE] [--onto BRANCH] [--strategy NAME] [--revision REV] [--preview] [--into REF] [--into-remote REMOTE] [--into-local] [--into-empty] [--incremental] [--pick] [--conduit-uri URI] [--config K=V] [--config-file FILE] [--arcrc-file FILE] [-- ref ...]\n";

/// `user.whoami` Conduit method; the result's `phid` is the viewer PHID
/// (`src/workflow/ArcanistWorkflow.php:489-494`).
pub const METHOD_WHOAMI: &str = "user.whoami";

/// `user.search` Conduit method for author monograms
/// (`ArcanistUserSymbolHardpointQuery.php:66-77`).
pub const METHOD_USER_SEARCH: &str = "user.search";

/// `edge.search` Conduit method for parent-revision edges
/// (`ArcanistRevisionParentRevisionsHardpointQuery.php:26-28`).
pub const METHOD_EDGE_SEARCH: &str = "edge.search";

/// `harbormaster.buildable.search` Conduit method
/// (`ArcanistRevisionBuildableHardpointQuery.php:29-34`).
pub const METHOD_BUILDABLE_SEARCH: &str = "harbormaster.buildable.search";

/// `harbormaster.build.search` Conduit method
/// (`ArcanistBuildableBuildsHardpointQuery.php:19-23`).
pub const METHOD_BUILD_SEARCH: &str = "harbormaster.build.search";

/// `harbormaster.buildplan.search` Conduit method
/// (`ArcanistBuildBuildplanHardpointQuery.php:21-25`).
pub const METHOD_BUILDPLAN_SEARCH: &str = "harbormaster.buildplan.search";

/// Abort message, from `ArcanistUserAbortException`
/// (`src/exception/usage/ArcanistUserAbortException.php:9-11`).
pub const ABORT: &str = "User aborted the workflow.";

/// Error when the working copy is neither git nor hg
/// (`ArcanistLandWorkflow.php:300-304`).
pub const NOT_GIT_OR_HG: &str = "\"arc land\" must be run in a Git or Mercurial working copy.";

/// Parsed `land` options.
///
/// Flag names are verbatim from
/// `ArcanistLandWorkflow::getWorkflowArguments()` (`:131-235`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Prepare but do not push (`hold`, `:133-137`).
    pub hold: bool,
    /// Keep branches after landing (`keep-branches`, `:138-143`).
    pub keep_branches: bool,
    /// Remote to push onto (`onto-remote`, `:144-147`).
    pub onto_remote: Option<String>,
    /// Branches to push onto, repeatable (`onto`, `:148-159`).
    pub onto: Vec<String>,
    /// Merge strategy (`strategy`, `:160-178`).
    pub strategy: Option<String>,
    /// Land a specific revision (`revision`, `:179-184`).
    pub revision: Option<String>,
    /// Preview only (`preview`, `:185-189`).
    pub preview: bool,
    /// State to merge into (`into`, `:190-195`).
    pub into: Option<String>,
    /// Remote to fetch the into ref from (`into-remote`, `:196-201`).
    pub into_remote: Option<String>,
    /// Use the local into state (`into-local`, `:202-206`).
    pub into_local: bool,
    /// Merge into the empty state (`into-empty`, `:207-213`).
    pub into_empty: bool,
    /// Merge and push one set at a time (`incremental`, `:214-226`).
    pub incremental: bool,
    /// Land only directly-named changes (`pick`, `:227-231`).
    pub pick: bool,
    /// Positional refs (the `ref` wildcard, `:232-233`).
    pub refs: Vec<String>,
    /// Conduit URI override (global `--conduit-uri`,
    /// `src/workflow/ArcanistWorkflow.php:304-310`).
    pub conduit_uri: Option<String>,
    /// Runtime `--config key=value` pairs (highest-precedence source).
    pub config: Vec<String>,
    /// `--config-file` paths (replace the system/user sources).
    pub config_file: Vec<String>,
    /// `--arcrc-file` path (replaces `~/.arcrc`).
    pub arcrc_file: Option<String>,
    /// Canonical long flag names in first-provision order (PHP `$dict`
    /// insertion order, `ArcanistWorkflow.php:791-884`). Empty for
    /// hand-built options, which skip the order-dependent checks.
    pub provided: Vec<&'static str>,
}

/// A value-taking `land` flag (`--onto BRANCH`, `--onto=BRANCH`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueFlag {
    OntoRemote,
    Onto,
    Strategy,
    Revision,
    Into,
    IntoRemote,
    ConduitUri,
    Config,
    ConfigFile,
    ArcrcFile,
}

/// A boolean `land` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolFlag {
    Hold,
    KeepBranches,
    Preview,
    IntoLocal,
    IntoEmpty,
    Incremental,
    Pick,
}

/// Any `land` flag.
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

/// One `getWorkflowArguments()` entry: canonical long name and kind.
/// Land declares no short flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlagSpec {
    long: &'static str,
    flag: Flag,
}

/// `land` flags in `getWorkflowArguments()` order
/// (`src/workflow/ArcanistLandWorkflow.php:131-235`), plus the global
/// `--conduit-uri` this port threads through every command.
const FLAG_SPECS: &[FlagSpec] = &[
    FlagSpec {
        long: "hold",
        flag: Flag::Bool(BoolFlag::Hold),
    },
    FlagSpec {
        long: "keep-branches",
        flag: Flag::Bool(BoolFlag::KeepBranches),
    },
    FlagSpec {
        long: "onto-remote",
        flag: Flag::Value(ValueFlag::OntoRemote),
    },
    FlagSpec {
        long: "onto",
        flag: Flag::Value(ValueFlag::Onto),
    },
    FlagSpec {
        long: "strategy",
        flag: Flag::Value(ValueFlag::Strategy),
    },
    FlagSpec {
        long: "revision",
        flag: Flag::Value(ValueFlag::Revision),
    },
    FlagSpec {
        long: "preview",
        flag: Flag::Bool(BoolFlag::Preview),
    },
    FlagSpec {
        long: "into",
        flag: Flag::Value(ValueFlag::Into),
    },
    FlagSpec {
        long: "into-remote",
        flag: Flag::Value(ValueFlag::IntoRemote),
    },
    FlagSpec {
        long: "into-local",
        flag: Flag::Bool(BoolFlag::IntoLocal),
    },
    FlagSpec {
        long: "into-empty",
        flag: Flag::Bool(BoolFlag::IntoEmpty),
    },
    FlagSpec {
        long: "incremental",
        flag: Flag::Bool(BoolFlag::Incremental),
    },
    FlagSpec {
        long: "pick",
        flag: Flag::Bool(BoolFlag::Pick),
    },
    FlagSpec {
        long: "conduit-uri",
        flag: Flag::Value(ValueFlag::ConduitUri),
    },
    FlagSpec {
        long: "config",
        flag: Flag::Value(ValueFlag::Config),
    },
    FlagSpec {
        long: "config-file",
        flag: Flag::Value(ValueFlag::ConfigFile),
    },
    FlagSpec {
        long: "arcrc-file",
        flag: Flag::Value(ValueFlag::ArcrcFile),
    },
];

/// Find a spec by long name (`--onto`).
fn lookup_long(name: &str) -> Option<FlagSpec> {
    FLAG_SPECS.iter().find(|spec| spec.long == name).copied()
}

/// Store a parsed value-flag value in `options`.
fn store_value_flag(options: &mut Options, flag: ValueFlag, value: String) {
    match flag {
        ValueFlag::OntoRemote => options.onto_remote = Some(value),
        // Repeatable (`setRepeatable(true)`, `:150`): every use appends.
        ValueFlag::Onto => options.onto.push(value),
        ValueFlag::Strategy => options.strategy = Some(value),
        ValueFlag::Revision => options.revision = Some(value),
        ValueFlag::Into => options.into = Some(value),
        ValueFlag::IntoRemote => options.into_remote = Some(value),
        ValueFlag::ConduitUri => options.conduit_uri = Some(value),
        ValueFlag::Config => options.config.push(value),
        ValueFlag::ConfigFile => options.config_file.push(value),
        ValueFlag::ArcrcFile => options.arcrc_file = Some(value),
    }
}

/// Store a parsed boolean flag in `options`.
fn store_bool_flag(options: &mut Options, flag: BoolFlag) {
    match flag {
        BoolFlag::Hold => options.hold = true,
        BoolFlag::KeepBranches => options.keep_branches = true,
        BoolFlag::Preview => options.preview = true,
        BoolFlag::IntoLocal => options.into_local = true,
        BoolFlag::IntoEmpty => options.into_empty = true,
        BoolFlag::Incremental => options.incremental = true,
        BoolFlag::Pick => options.pick = true,
    }
}

/// Record one parsed flag in `options` (`$dict[$arg_key] = ...`,
/// `ArcanistWorkflow.php:867-883`), tracking first-provision order.
///
/// Re-providing a flag overwrites, except repeatable `--onto`, which
/// appends (PHP has no "provided twice" error).
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
/// Values may be empty and may look like flags; only a trailing `param`
/// flag errors (`ArcanistWorkflow.php:867-883`).
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

/// Parse `argv` (excluding the subcommand name).
///
/// Ports `ArcanistWorkflow::parseArguments()`
/// (`src/workflow/ArcanistWorkflow.php:788-926`): `--` ends flags,
/// positionals collect into `refs` (the `ref` wildcard), and `--help`
/// short-circuits. Land declares no short flags and no conflicts, so any
/// `-x` token errors like an unknown long name. Returns `Ok(None)` when
/// `--help` was requested.
pub fn parse_options(argv: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut queue: VecDeque<String> = argv.iter().cloned().collect();
    let mut end_of_flags = false;
    while let Some(arg) = queue.pop_front() {
        if end_of_flags {
            options.refs.push(arg);
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
            // No short flags: PHP matches the whole remainder against an
            // empty short map (`ArcanistWorkflow.php:852-862`).
            return Err(format!("Unknown argument '{spelling}'. Try 'arc help'."));
        }
        options.refs.push(arg);
    }
    Ok(Some(options))
}

/// Collected process output: stdout and stderr stay separate, like PHP.
///
/// Statuses render into `stderr` (`ArcanistLogEngine::writeBytes()` writes
/// to stderr, `src/log/ArcanistLogEngine.php:21-24`); banners, commit
/// lists, and prompts render into `stdout` (PHP `echo`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Out {
    /// Bytes for stdout.
    pub stdout: String,
    /// Bytes for stderr.
    pub stderr: String,
}

impl Out {
    /// Append stdout text.
    pub fn push_out(&mut self, text: &str) {
        self.stdout.push_str(text);
    }

    /// Append stderr text.
    pub fn push_err(&mut self, text: &str) {
        self.stderr.push_str(text);
    }

    /// One log line (`** LABEL ** message`), to stderr
    /// (`ArcanistLogEngine::writeMessage()`, `:30-40`, colors stripped).
    pub fn push_status(&mut self, label: &str, message: &str) {
        self.push_status_with_ansi(label, message, "blue", terminal::ansi_enabled());
    }

    /// One log line with an explicit writer color and ANSI flag, to
    /// stderr. TTYs render `tsprintf("**<bg:color> %s </bg>** %s\n",
    /// ...)` like PHP `writeMessage()`; non-TTYs keep the stripped
    /// `** LABEL ** message` rendering byte-identical.
    pub fn push_status_with_ansi(&mut self, label: &str, message: &str, color: &str, ansi: bool) {
        self.push_err(&status_line(label, message, color, ansi));
    }

    /// True when both streams are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }
}

/// One `** LABEL ** message` status line (see
/// [`Out::push_status_with_ansi`]).
fn status_line(label: &str, message: &str, color: &str, ansi: bool) -> String {
    if ansi {
        terminal::format_log(color, label, message, true)
    } else {
        format!("** {label} ** {message}\n")
    }
}

/// Interactive IO for prompts.
///
/// `canned` holds scripted prompt answers, consumed front to back; when it
/// is empty, answers come from real stdin. `saved` holds saved prompt
/// responses keyed by prompt key. `live` echoes every output byte to the
/// real stdout/stderr as it is written, so interactive users see prompts
/// before answering; when `live`, `run_in` prints nothing more, since
/// everything already echoed. `config_writer` persists `*`/`!` answers
/// back to configuration files.
#[derive(Debug, Default)]
pub struct Io {
    /// Scripted answers for tests; empty means read real stdin.
    pub canned: VecDeque<String>,
    /// Saved prompt responses by prompt key.
    pub saved: HashMap<String, SavedResponse>,
    /// Collected output.
    pub out: Out,
    /// Echo output to the real console as it is written.
    pub live: bool,
    /// Scripted `--patch -` text for tests; `None` reads real stdin.
    /// Used only by `patch`; ignored by `land`.
    pub patch_stdin: Option<String>,
    /// Persists `*`/`!` prompt answers to configuration files, when set.
    pub config_writer: Option<ConfigWriter>,
    /// True when console output may carry ANSI styling. Set once from
    /// `terminal::ansi_enabled()` by the `run_in`/`execute` entry
    /// points and threaded down to every render site; tests set it
    /// explicitly (`false` keeps the historical plain rendering
    /// byte-identical).
    pub ansi: bool,
    /// True when stdin is an interactive terminal. Scripted
    /// (`canned`) IO sets this, modelling a TTY session; `buffered`
    /// and `live` IO read the real stdin state, so piped input fails
    /// prompts like `phutil_console_require_tty()`.
    pub stdin_tty: bool,
}

/// Configuration source scope holding saved prompt answers
/// (`ArcanistConfigurationSource::SCOPE_*`,
/// `src/config/source/ArcanistConfigurationSource.php:6-7`, plus the
/// read-only system and project sources).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptScope {
    /// `/etc/arcconfig` (or `%ProgramData%` on Windows): read-only.
    System,
    /// `~/.arcrc`: the `*` suffix target.
    User,
    /// `.arcconfig`: read-only.
    Project,
    /// The working-copy local config (`.git/arc/config` or
    /// `.hg/arc/config`): the `!` suffix target.
    WorkingCopy,
}

impl PromptScope {
    /// The PHP scope name (`user` / `working-copy`) for writable scopes.
    fn name(self) -> &'static str {
        match self {
            PromptScope::User => "user",
            _ => "working-copy",
        }
    }
}

/// A saved prompt answer from configuration
/// (`ArcanistPromptResponse`, `src/toolset/ArcanistPromptResponse.php`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedResponse {
    /// The raw saved answer string (e.g. `y`).
    pub response: String,
    /// The source scope the answer was read from.
    pub scope: PromptScope,
}

/// Writes saved prompt answers back to configuration files
/// (`ArcanistPrompt::saveResponse()`, `src/toolset/ArcanistPrompt.php:253-285`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigWriter {
    /// User config file (`~/.arcrc`) for `*` answers, when known.
    pub user_path: Option<PathBuf>,
    /// Working-copy local config (`.git/arc/config`, `.hg/arc/config`)
    /// for `!` answers, when known.
    pub local_path: Option<PathBuf>,
}

impl Io {
    /// Buffered IO reading real stdin (for `execute`).
    #[must_use]
    pub fn buffered() -> Self {
        Self {
            stdin_tty: crate::prompt::stdin_is_terminal(),
            ..Self::default()
        }
    }

    /// Live IO reading real stdin (for `run_in`).
    #[must_use]
    pub fn live() -> Self {
        Self {
            live: true,
            stdin_tty: crate::prompt::stdin_is_terminal(),
            ..Self::default()
        }
    }

    /// Scripted answers, for tests (models a TTY session).
    #[must_use]
    pub fn canned(answers: &[&str]) -> Self {
        Self {
            canned: answers.iter().map(|answer| (*answer).to_string()).collect(),
            stdin_tty: true,
            ..Self::default()
        }
    }

    /// Append stdout text, echoing live when enabled.
    pub fn push_out(&mut self, text: &str) {
        self.out.push_out(text);
        if self.live {
            use std::io::Write;
            print!("{text}");
            std::io::stdout().flush().ok();
        }
    }

    /// Append stderr text, echoing live when enabled.
    pub fn push_err(&mut self, text: &str) {
        self.out.push_err(text);
        if self.live {
            use std::io::Write;
            eprint!("{text}");
            std::io::stderr().flush().ok();
        }
    }

    /// One log line, to stderr (the `writeStatus` blue writer).
    pub fn push_status(&mut self, label: &str, message: &str) {
        self.push_status_with_ansi(label, message, "blue", self.ansi);
    }

    /// One log line with an explicit writer color and ANSI flag, to
    /// stderr (see [`Out::push_status_with_ansi`]), echoing live when
    /// enabled.
    pub fn push_status_with_ansi(&mut self, label: &str, message: &str, color: &str, ansi: bool) {
        self.push_err(&status_line(label, message, color, ansi));
    }

    /// Read one answer line, without the trailing newline.
    ///
    /// Returns `None` on end of input: real-stdin EOF, or exhausted
    /// scripted answers.
    fn read_answer(&mut self) -> Option<String> {
        if !self.canned.is_empty() {
            return self.canned.pop_front();
        }
        // `live` output already echoed above, so the prompt is visible.
        // Flush once more in case output bypassed `push_out`.
        use std::io::{BufRead, Write};
        std::io::stdout().flush().ok();
        std::io::stderr().flush().ok();
        let mut line = String::new();
        let stdin = std::io::stdin();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
            Err(_) => None,
        }
    }
}

/// Soft-wrap text for console display
/// (`phutil_console_wrap()`, `src/console/format.php:102-191`).
///
/// Wraps at spaces past column `78 - indent`, preserving embedded
/// newlines and ANSI color escapes (which do not count toward the width).
/// Every emitted line but optionally the first gains `indent` spaces.
/// Lines without a usable break (long words) overflow, like PHP.
/// Delegates to the shared [`terminal::console_wrap`] port.
pub fn console_wrap(text: &str, indent: usize, with_prefix: bool) -> String {
    terminal::console_wrap(text, indent, with_prefix)
}

/// Default truncation terminator: U+2026 HORIZONTAL ELLIPSIS
/// (`PhutilUTF8StringTruncator.php:39`).
pub const TRUNCATION_TERMINATOR: &str = "…";

/// True for Unicode combining codepoints
/// (`phutil_utf8_is_combining_character()`, `src/utils/utf8.php:856-874`).
fn is_combining_mark(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// Split text into glyphs: base characters with combining marks merged
/// (`phutil_utf8v_combine_characters()`, `src/utils/utf8.php:899-935`).
///
/// A leading combining mark attaches to a synthetic space, and
/// single-byte characters pass through (none of them combine).
fn split_glyphs(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut buf: Option<String> = if is_combining_mark(chars[0]) {
        Some(" ".to_string())
    } else {
        None
    };
    for ch in chars {
        if ch.len_utf8() > 1 && is_combining_mark(ch) {
            match &mut buf {
                Some(pending) => pending.push(ch),
                None => buf = Some(ch.to_string()),
            }
        } else {
            if let Some(pending) = buf.take() {
                parts.push(pending);
            }
            buf = Some(ch.to_string());
        }
    }
    if let Some(pending) = buf {
        parts.push(pending);
    }
    parts
}

/// Truncate text to `max_glyphs` glyphs
/// (`PhutilUTF8StringTruncator::truncateString()` in glyph mode,
/// `src/utils/PhutilUTF8StringTruncator.php:86-293`).
///
/// Short strings return unchanged; longer ones keep `max_glyphs - 1`
/// glyphs plus the `…` terminator, preferring word breaks (`space`,
/// newline, `;`, `:`, `[`, `(`, `,`, `-`) and cutting without a
/// terminator at sentence stops (`.`, `!`, `?`).
pub fn truncate_glyphs(text: &str, max_glyphs: usize) -> String {
    if text.len() <= max_glyphs {
        return text.to_string();
    }
    let glyphs = split_glyphs(text);
    if glyphs.len() <= max_glyphs {
        return text.to_string();
    }
    let cutoff = max_glyphs.saturating_sub(1);
    if cutoff == 0 {
        return TRUNCATION_TERMINATOR.to_string();
    }
    truncate_at_boundary(&glyphs, cutoff)
}

/// Cut `glyphs` at a word boundary near `cutoff`
/// (`src/utils/PhutilUTF8StringTruncator.php:239-293`).
fn truncate_at_boundary(glyphs: &[String], cutoff: usize) -> String {
    const BREAK: &[&str] = &[" ", "\n", ";", ":", "[", "(", ",", "-"];
    const STOP: &[&str] = &[".", "!", "?"];
    // PHP divides by 2 in floats (`$cutoff / 2`); `div_ceil` is the
    // same integer bound.
    let search = cutoff.saturating_sub(256).max(cutoff.div_ceil(2));
    let start = cutoff.min(glyphs.len().saturating_sub(1));
    let mut word_boundary: Option<usize> = None;
    let mut stop_boundary: Option<usize> = None;
    let mut any_nonboundary = false;
    for index in (search..=start).rev() {
        let glyph = glyphs[index].as_str();
        if BREAK.contains(&glyph) {
            word_boundary = Some(index);
        } else if STOP.contains(&glyph) {
            stop_boundary = Some(index + 1);
            break;
        } else {
            any_nonboundary = true;
            if word_boundary.is_some() {
                break;
            }
        }
    }
    if let Some(stop) = stop_boundary {
        return glyphs[..stop].concat();
    }
    let end = match word_boundary {
        Some(found) if any_nonboundary => found,
        _ => cutoff,
    };
    format!("{}{TRUNCATION_TERMINATOR}", glyphs[..end].concat())
}

/// Console display width of text
/// (`phutil_utf8_console_strlen()`, `src/utils/utf8.php:316-361`).
///
/// ANSI color escapes contribute nothing; ASCII counts one column per
/// byte; other glyphs count their first codepoint, double for wide
/// (CJK) characters.
pub fn console_strlen(text: &str) -> usize {
    let stripped = strip_ansi_colors(text);
    if stripped.bytes().all(|byte| (1..=0x7F).contains(&byte)) {
        return stripped.len();
    }
    let mut width = 0;
    for glyph in split_glyphs(&stripped) {
        let Some(first) = glyph.chars().next() else {
            continue;
        };
        let code = first as u32;
        if code == 0 {
            continue;
        }
        width += 1 + usize::from(is_wide_codepoint(code));
    }
    width
}

/// Strip ANSI `ESC [ digits m` color escapes, like the console-length
/// fast path (`src/utils/utf8.php:320`).
fn strip_ansi_colors(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\x1b'
            && let Some(end) = ansi_color_end(&chars, index)
        {
            index = end;
            continue;
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// End index past an ANSI color escape at `start`, or `None`
/// (`/\x1B\[\d*m/`, digits optional).
fn ansi_color_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if chars.get(index) != Some(&'[') {
        return None;
    }
    index += 1;
    while chars.get(index).is_some_and(|ch| ch.is_ascii_digit()) {
        index += 1;
    }
    if chars.get(index) != Some(&'m') {
        return None;
    }
    Some(index + 1)
}

/// Wide (double-column) codepoints, after `wcwidth.c`
/// (`src/utils/utf8.php:341-354`).
fn is_wide_codepoint(code: u32) -> bool {
    code >= 0x1100
        && (code <= 0x115F
            || code == 0x2329
            || code == 0x232A
            || (0x2E80..=0xA4CF).contains(&code) && code != 0x303F
            || (0xAC00..=0xD7A3).contains(&code)
            || (0xF900..=0xFAFF).contains(&code)
            || (0xFE10..=0xFE19).contains(&code)
            || (0xFE30..=0xFE6F).contains(&code)
            || (0xFF00..=0xFF60).contains(&code)
            || (0xFFE0..=0xFFE6).contains(&code)
            || (0x20000..=0x2FFFD).contains(&code)
            || (0x30000..=0x3FFFD).contains(&code))
}

/// Render one banner block to stdout: a blank line, a `** TITLE **` line,
/// the wrapped body, and a trailing blank line.
///
/// Ports `echo tsprintf("\n%!\n%W\n\n", $title, $body)`; TTYs render
/// the `%!`/`%W` markup, non-TTYs keep the stripped rendering.
pub fn push_banner(io: &mut Io, title: &str, body: &str) {
    push_banner_mode(io, title, body, true);
}

/// Banner core: `tsprintf("\n%!\n%BODY\n\n", ...)` on TTYs, else the
/// plain `** TITLE **` rendering. `wrap_body` selects `%W` (most
/// banners) vs `%s` (`HOLD CHANGES`, which PHP prints unwrapped,
/// `ArcanistGitLandEngine.php:907-910`).
fn push_banner_mode(io: &mut Io, title: &str, body: &str, wrap_body: bool) {
    if io.ansi {
        let pattern = if wrap_body {
            "\n%!\n%W\n\n"
        } else {
            "\n%!\n%s\n\n"
        };
        let text = terminal::tsprintf_ansi(pattern, &[TVal::Text(title), TVal::Text(body)], true)
            .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!(
            "\n** {title} **\n{}\n\n",
            console_wrap(body, 0, true)
        ));
    }
}

/// Render a banner with a single trailing newline
/// (`echo tsprintf("\n%!\n%W\n", ...)`), for banners PHP follows with
/// more output on the next line (`IMPLICIT COMMITS`,
/// `ArcanistLandEngine.php:735-736`; `DISCARDING ANCESTORS`, `:1553-1558`).
/// TTYs render the markup, non-TTYs keep the stripped rendering.
pub fn push_banner_continued(io: &mut Io, title: &str, body: &str) {
    if io.ansi {
        let text =
            terminal::tsprintf_ansi("\n%!\n%W\n", &[TVal::Text(title), TVal::Text(body)], true)
                .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!(
            "\n** {title} **\n{}\n",
            console_wrap(body, 0, true)
        ));
    }
}

/// Render one `%?` hint line: the `?` marker plus wrapped text
/// (`src/xsprintf/tsprintf.php:51-56`). Callers add the trailing newline.
pub fn render_hint(text: &str) -> String {
    render_hint_with_ansi(text, terminal::ansi_enabled())
}

/// Render one `%?` hint line with an explicit ANSI flag: TTYs render
/// the green `?` badge, non-TTYs keep the stripped rendering
/// byte-identical. Callers add the trailing newline.
pub fn render_hint_with_ansi(text: &str, ansi: bool) -> String {
    if ansi {
        terminal::tsprintf_ansi("%?", &[TVal::Text(text)], true)
            .expect("fixed tsprintf pattern")
            .as_str()
            .to_string()
    } else {
        console_wrap(&format!("  ?   {text}"), 6, false)
    }
}

/// One `ArcanistRefView` node (`src/ref/ArcanistRefView.php`): an object
/// name, a title, an optional URI, plain lines, and child views.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefView {
    /// Bold object name (`D123`, `Branch "x"`), if any.
    pub object_name: Option<String>,
    /// Title, truncated to the usable width when rendered.
    pub title: String,
    /// Optional underlined URI line.
    pub uri: Option<String>,
    /// Plain indented lines.
    pub lines: Vec<String>,
    /// Child views, rendered one indent deeper.
    pub children: Vec<RefView>,
}

/// Terminal width for ref-view titles. PHP probes `tput cols`
/// (`PhutilConsoleMetrics::getTerminalWidth()`), which yields 80 in
/// practice; the port fixes 80 so output is deterministic (see the
/// module docs).
pub const REF_VIEW_TERMINAL_WIDTH: usize = 80;

impl RefView {
    /// Render the view: the `*` title line, the `://` URI line, the
    /// plain lines, then children (`ArcanistRefView::newLines()`,
    /// `src/ref/ArcanistRefView.php:69-144`), with `tsprintf` markup
    /// stripped like the `>>>` prompt marker.
    #[must_use]
    pub fn render(&self) -> String {
        self.render_with_ansi(terminal::ansi_enabled())
    }

    /// Render with an explicit ANSI flag: TTYs render the `tsprintf`
    /// markup (`**bold**`, `<bg:cyan>`, `__underline__`) like
    /// `ArcanistRefView::newLines()`; non-TTYs keep the stripped
    /// rendering byte-identical.
    #[must_use]
    pub fn render_with_ansi(&self, ansi: bool) -> String {
        if ansi {
            to_terminal_view(self).render(0, REF_VIEW_TERMINAL_WIDTH, true)
        } else {
            let mut out = String::new();
            self.render_at(0, &mut out);
            out
        }
    }

    /// Render at `indent` levels (`newLines($indent)`, `:69-144`).
    fn render_at(&self, indent: usize, out: &mut String) {
        let reserve = self
            .object_name
            .as_ref()
            .map(|name| console_strlen(name) + 1)
            .unwrap_or(0);
        let pad: String = "  ".repeat(indent);
        let usable = REF_VIEW_TERMINAL_WIDTH
            .saturating_sub(6 + reserve + pad.len())
            .max(16);
        let title = truncate_glyphs(&self.title, usable);
        let display = match (&self.object_name, title.is_empty()) {
            (Some(name), false) => format!("{name} {title}"),
            (Some(name), true) => name.clone(),
            (None, _) => title,
        };
        out.push_str(&format!("  *   {pad}{display}\n"));
        if let Some(uri) = &self.uri {
            out.push_str(&format!(" ://    {pad}{uri}\n"));
        }
        for line in &self.lines {
            out.push_str(&format!("        {pad}{line}\n"));
        }
        for child in &self.children {
            child.render_at(indent + 1, out);
        }
    }
}

/// Convert to the shared terminal ref view for TTY rendering.
fn to_terminal_view(view: &RefView) -> terminal::RefView {
    terminal::RefView {
        object_name: view.object_name.clone(),
        title: Some(view.title.clone()),
        uri: view.uri.clone(),
        lines: view.lines.clone(),
        children: view.children.iter().map(to_terminal_view).collect(),
    }
}

/// Revision ref view (`ArcanistRevisionRef::buildRefView()`,
/// `src/ref/revision/ArcanistRevisionRef.php:213-217`).
#[must_use]
pub fn revision_ref_view(revision: &land_checks::RevisionView) -> RefView {
    RefView {
        object_name: Some(revision.monogram.clone()),
        title: revision.name.clone(),
        ..Default::default()
    }
}

/// Marker ref view (`ArcanistMarkerRef::buildRefView()`,
/// `src/repository/marker/ArcanistMarkerRef.php:185-194`).
///
/// `display_name` is `Branch "x"` / `Bookmark "x"` / `Marker "x"`
/// (`getRefDisplayName()`, `:27-36`); `hash` is the 12-character
/// display hash. Name-only markers (empty hash and summary) render
/// the `" "` title PHP builds from two nulls, trailing space included.
#[must_use]
pub fn marker_ref_view(display_name: &str, hash: &str, summary: &str) -> RefView {
    RefView {
        object_name: Some(display_name.to_string()),
        title: format!("{hash} {summary}"),
        ..Default::default()
    }
}

/// Buildable ref view (`ArcanistBuildableRef::buildRefView()`,
/// `src/ref/buildable/ArcanistBuildableRef.php:45-49`).
#[must_use]
pub fn buildable_ref_view(monogram: &str, uri: &str) -> RefView {
    RefView {
        object_name: Some(monogram.to_string()),
        title: format!("Buildable \"{monogram}\""),
        uri: Some(uri.to_string()),
        ..Default::default()
    }
}

/// Build ref view (`ArcanistBuildRef::buildRefView()`,
/// `src/ref/build/ArcanistBuildRef.php:38-42`).
#[must_use]
pub fn build_ref_view(id: i64, name: &str) -> RefView {
    RefView {
        object_name: Some(format!("Build {id}")),
        title: name.to_string(),
        ..Default::default()
    }
}

/// Execute an `arc.land.*` prompt: print `>>>`, loop until `y`/`n`.
///
/// Ports `ArcanistPrompt::execute()` (`src/toolset/ArcanistPrompt.php:47-254`)
/// in plain text: a leading blank line, then `>>> {query} [y/N/?] ` per
/// attempt. Empty answers mean `n`; `?` prints the prompt help; answers
/// ending in `*` save to user configuration and `!` to working-copy
/// configuration (`saveResponse()`, `:253-285`). A saved response runs
/// the same loop without prompting, and the `Using saved response`
/// notice prints after the loop (`:238-249`). Declining — or end of
/// input — aborts with [`ABORT`]. Non-interactive stdin echoes the
/// query and fails first, even with a saved response
/// (`phutil_console_require_tty()`, `:77-88`).
pub fn prompt_for_key(io: &mut Io, key: &str, query: &str) -> Result<(), String> {
    if !io.stdin_tty {
        io.push_out(&format!("{query}\n"));
        return Err(crate::prompt::NOT_INTERACTIVE.to_string());
    }
    io.push_out("\n");
    let mut saved_response = io.saved.get(key).map(|saved| saved.response.clone());
    let mut save_scope: Option<PromptScope> = None;
    loop {
        let (mut candidate, is_saved) = next_prompt_candidate(io, query, &mut saved_response)?;
        if !is_saved {
            let (stripped, scope) = strip_prompt_save_suffix(candidate);
            candidate = stripped;
            save_scope = scope.or(save_scope);
            if candidate.is_empty() {
                candidate = "n".to_string();
            }
        }
        match classify_prompt_answer(&candidate) {
            PromptVerdict::Yes => {
                return finish_prompt(io, key, query, save_scope, is_saved, &candidate, true);
            }
            PromptVerdict::No => {
                return finish_prompt(io, key, query, save_scope, is_saved, &candidate, false);
            }
            PromptVerdict::Help => push_prompt_help(io, key),
            PromptVerdict::Invalid => {}
        }
    }
}

/// One prompt attempt: the saved response (once), else a printed `>>>`
/// line plus a stdin answer. Returns the trimmed candidate and whether it
/// came from a saved response.
fn next_prompt_candidate(
    io: &mut Io,
    query: &str,
    saved_response: &mut Option<String>,
) -> Result<(String, bool), String> {
    match saved_response.take() {
        Some(saved) => Ok((saved, true)),
        None => {
            push_prompt_query(io, query);
            let Some(raw) = io.read_answer() else {
                return Err(ABORT.to_string());
            };
            Ok((raw.trim().to_string(), false))
        }
    }
}

/// One `>>>` prompt attempt line (`ArcanistPrompt.php:111-115`):
/// `tsprintf('**<bg:cyan> %s </bg>** %s %s ', '>>>', $query, $options)`
/// on TTYs, else the plain `>>>` rendering.
fn push_prompt_query(io: &mut Io, query: &str) {
    if io.ansi {
        let text = terminal::tsprintf_ansi(
            "**<bg:cyan> %s </bg>** %s %s ",
            &[TVal::Text(">>>"), TVal::Text(query), TVal::Text("[y/N/?]")],
            true,
        )
        .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!(">>> {query} [y/N/?] "));
    }
}

/// Split a `*`/`!` save suffix off a live answer, returning the remainder
/// and the target scope (`saveResponse()`, `:253-285`).
fn strip_prompt_save_suffix(candidate: String) -> (String, Option<PromptScope>) {
    if let Some(stripped) = candidate.strip_suffix('*') {
        return (stripped.to_string(), Some(PromptScope::User));
    }
    if let Some(stripped) = candidate.strip_suffix('!') {
        return (stripped.to_string(), Some(PromptScope::WorkingCopy));
    }
    (candidate, None)
}

/// Classification of one prompt candidate.
enum PromptVerdict {
    /// `y` (any case): accept.
    Yes,
    /// `n` (any case): decline.
    No,
    /// `?`: print help and reprompt.
    Help,
    /// Anything else: reprompt silently.
    Invalid,
}

/// Classify one prompt candidate (`ArcanistPrompt.php:191-237`).
fn classify_prompt_answer(candidate: &str) -> PromptVerdict {
    let lowered = candidate.to_lowercase();
    if lowered == "y" {
        PromptVerdict::Yes
    } else if lowered == "n" {
        PromptVerdict::No
    } else if candidate == "?" {
        PromptVerdict::Help
    } else {
        PromptVerdict::Invalid
    }
}

/// Finish a prompt: persist a `*`/`!` answer, print the saved-response
/// notice when the answer was saved, and map the verdict to `Ok`/abort.
fn finish_prompt(
    io: &mut Io,
    key: &str,
    query: &str,
    save_scope: Option<PromptScope>,
    is_saved: bool,
    response: &str,
    result: bool,
) -> Result<(), String> {
    if let Some(scope) = save_scope {
        save_prompt_response(io, key, response, scope)?;
    }
    if is_saved {
        push_saved_notice(io, key, query, response);
    }
    if result {
        Ok(())
    } else {
        Err(ABORT.to_string())
    }
}

/// The saved-response notice (`ArcanistPrompt.php:238-249`):
/// `tsprintf("<bg:cyan>** %s **</bg> %s **<%s>**\n<bg:cyan>** %s
/// **</bg> (%s)\n\n", ...)` on TTYs, else the plain `>>>` rendering
/// (which omits the closing parenthesis, unlike PHP).
fn push_saved_notice(io: &mut Io, key: &str, query: &str, response: &str) {
    if io.ansi {
        let saved = format!("Using saved response to prompt \"{key}\".");
        let text = terminal::tsprintf_ansi(
            "<bg:cyan>** %s **</bg> %s **<%s>**\n<bg:cyan>** %s **</bg> (%s)\n\n",
            &[
                TVal::Text(">>>"),
                TVal::Text(query),
                TVal::Text(response),
                TVal::Text(">>>"),
                TVal::Text(&saved),
            ],
            true,
        )
        .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!(
            ">>> {query} <{response}>\n>>> (Using saved response to prompt \"{key}\".\n\n"
        ));
    }
}

/// The `SAVE PROMPT` notice (`ArcanistPrompt.php:282-288`):
/// `tsprintf("<bg:green>** %s **</bg> %s\n", ...)` on TTYs, else the
/// plain `** SAVE PROMPT **` rendering.
fn push_save_notice(io: &mut Io, key: &str, response: &str) {
    if io.ansi {
        let message = format!("Saving response \"{response}\" to prompt \"{key}\".");
        let text = terminal::tsprintf_ansi(
            "<bg:green>** %s **</bg> %s\n",
            &[TVal::Text("SAVE PROMPT"), TVal::Text(&message)],
            true,
        )
        .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!(
            "** SAVE PROMPT ** Saving response \"{response}\" to prompt \"{key}\".\n"
        ));
    }
}

/// Persist an answer to configuration
/// (`ArcanistPrompt::saveResponse()`, `src/toolset/ArcanistPrompt.php:253-285`).
///
/// Prints the `SAVE PROMPT` notice, then replaces any saved answer for
/// `key` in the scope file and appends `{prompt, response}`.
fn save_prompt_response(
    io: &mut Io,
    key: &str,
    response: &str,
    scope: PromptScope,
) -> Result<(), String> {
    push_save_notice(io, key, response);
    let path = io.config_writer.as_ref().and_then(|writer| match scope {
        PromptScope::User => writer.user_path.clone(),
        _ => writer.local_path.clone(),
    });
    let Some(path) = path else {
        return Err(format!(
            "Unable to save response to config: no writable configuration source for scope \
             (\"{}\").",
            scope.name()
        ));
    };
    write_saved_prompt(&path, key, response)
}

/// Replace-or-append one `{prompt, response}` entry in the `prompts` list
/// of the JSON config file at `path`, creating parent directories.
///
/// PHP writes pretty-printed JSON (`FilesystemConfigurationSource.php:38-56`
/// with `PhutilJSON::encodeFormatted()`); the port stages the new
/// content in a sibling temp file and renames it over the destination.
pub fn write_saved_prompt(path: &Path, key: &str, response: &str) -> Result<(), String> {
    let mut values = read_config_map(path)?;
    let mut entries = prompts_list(&values)?;
    entries.retain(|entry| entry_key(entry) != Some(key));
    entries.push(prompt_entry(key, response));
    values.insert("prompts".to_string(), serde_json::Value::Array(entries));
    let content = format!(
        "{}\n",
        serde_json::to_string_pretty(&values).unwrap_or_default()
    );
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Unable to write configuration file: {err}"))?;
    }
    write_sibling_temp(path, content.as_bytes())
}

/// Read a JSON config file as a map; missing or blank files read empty.
/// Invalid JSON fails, like PHP's `phutil_json_decode()`.
fn read_config_map(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        _ => Err(format!(
            "Unable to read configuration file \"{}\": invalid JSON.",
            path.display()
        )),
    }
}

/// Read the `prompts` list for writing: a list of entries, a `{prompt:
/// response}` map (accepted on read), or empty when absent. Anything
/// else fails like PHP (`ArcanistPromptsConfigOption.php:10-17`).
fn prompts_list(
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    let Some(prompts) = values.get("prompts") else {
        return Ok(Vec::new());
    };
    if let Some(list) = prompts.as_array() {
        return Ok(list.clone());
    }
    if let Some(map) = prompts.as_object() {
        let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
        entries.sort_by(|left, right| left.0.cmp(right.0));
        return Ok(entries
            .into_iter()
            .filter_map(|(key, value)| value.as_str().map(|response| prompt_entry(key, response)))
            .collect());
    }
    Err("Expected a natural list!".to_string())
}

/// One `{prompt, response}` storage entry
/// (`ArcanistPromptResponse::getStorageDictionary()`).
fn prompt_entry(key: &str, response: &str) -> serde_json::Value {
    serde_json::json!({"prompt": key, "response": response})
}

/// The `prompt` key of a storage entry, when it is well-formed.
fn entry_key(entry: &serde_json::Value) -> Option<&str> {
    match (entry.get("prompt"), entry.get("response")) {
        (Some(key), Some(_)) => key.as_str(),
        _ => None,
    }
}

/// Write `content` to `path` via a sibling temp file plus rename.
fn write_sibling_temp(path: &Path, content: &[u8]) -> Result<(), String> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staging = path.with_file_name(format!("{file_name}.tmp-{}", std::process::id()));
    std::fs::write(&staging, content)
        .map_err(|err| format!("Unable to write configuration file: {err}"))?;
    std::fs::rename(&staging, path)
        .map_err(|err| format!("Unable to write configuration file: {err}"))?;
    Ok(())
}

/// Print the `?` help for a prompt (`ArcanistPrompt.php:191-230`).
///
/// The header, description, and default lines print verbatim; the three
/// save/config hints render as `%?` lines (`%?`, `tsprintf.php:51-56`).
fn push_prompt_help(io: &mut Io, key: &str) {
    let description = land_checks::prompt_description(key).unwrap_or("");
    let ansi = io.ansi;
    if ansi {
        let text = terminal::tsprintf_ansi(
            "\n<bg:green>** %s **</bg> **%s**\n\n%s\n\n%s\n\n",
            &[
                TVal::Text("PROMPT"),
                TVal::Text(key),
                TVal::Text(description),
                TVal::Text("The default response to this prompt is \"n\"."),
            ],
            true,
        )
        .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!(
            "\n** PROMPT ** **{key}**\n\n{description}\n\nThe default response to this prompt is \
             \"n\".\n\n"
        ));
    }
    for hint in [
        "Use \"*\" after a response to save it in user configuration.",
        "Use \"!\" after a response to save it in working copy configuration.",
        "Run \"arc help prompts\" for detailed help on configuring responses.",
    ] {
        io.push_out(&render_hint_with_ansi(hint, ansi));
        io.push_out("\n");
    }
    io.push_out("\n");
}

/// Ask a yes/no question, returning the answer.
///
/// Ports `phutil_console_confirm()` (`src/console/format.php:11-25`):
/// prints two blank lines plus the query and `[y/N]` (or `[Y/n]` when
/// `default_no` is false) wrapped with a 4-space indent
/// (`phutil_console_prompt()`, `:42-45`), loops until `y`/`n`/empty,
/// then prints a blank line. End of input aborts with [`ABORT`].
/// Non-interactive stdin echoes the prompt once, then fails with the
/// `phutil_console_require_tty()` error (`:46-52`).
pub fn console_confirm(io: &mut Io, query: &str, default_no: bool) -> Result<bool, String> {
    let options = if default_no { "[y/N]" } else { "[Y/n]" };
    let prompt = console_wrap(&format!("{query} {options} "), 4, true);
    loop {
        io.push_out(&format!("\n\n{prompt}"));
        if !io.stdin_tty {
            return Err(crate::prompt::NOT_INTERACTIVE.to_string());
        }
        let Some(raw) = io.read_answer() else {
            return Err(ABORT.to_string());
        };
        match raw.trim().to_lowercase().as_str() {
            "y" => {
                io.push_out("\n");
                return Ok(true);
            }
            "n" => {
                io.push_out("\n");
                return Ok(false);
            }
            "" => {
                io.push_out("\n");
                return Ok(!default_no);
            }
            _ => {}
        }
    }
}

/// Working-copy VCS, named like `getSourceControlSystemName()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vcs {
    /// Git (`ArcanistGitAPI.php:49-51`).
    Git,
    /// Mercurial (`ArcanistMercurialAPI.php:45-47`).
    Hg,
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

/// Walk toward the filesystem root for a config file.
fn find_dot_file(start: &Path, name: &str) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Detect a landable working-copy VCS for `dir`.
///
/// Git wins over Mercurial; anything else (including Subversion) is not
/// landable, and the caller reports [`NOT_GIT_OR_HG`].
fn detect_vcs(dir: &Path) -> Option<Vcs> {
    if git::is_inside_work_tree(dir) {
        return Some(Vcs::Git);
    }
    if find_dot_dir(dir, ".hg").is_some() {
        return Some(Vcs::Hg);
    }
    None
}

/// Read one `arc.land.*` value through the full source merge (runtime
/// `--config` > local > project > user > system > default), mirroring
/// `ArcanistLandEngine::get*FromConfiguration()` → `getConfig()`.
fn land_config_value(
    options: &Options,
    dir: &Path,
    key: &str,
) -> Result<Option<serde_json::Value>, String> {
    let cli = get_config::load_options_from_cli(
        &options.config,
        &options.config_file,
        options.arcrc_file.as_deref(),
    )?;
    let user_file = config::arcrc_path();
    let merged = get_config::load_merged(dir, user_file.as_deref(), &cli)?;
    Ok(get_config::merged_value(key, &merged).map(|(_, value)| value))
}

/// Parse an `arc.land.onto` configuration value
/// (`ArcanistStringListConfigOption` via
/// `ArcanistListConfigOption::getValueFromStorageValue()`,
/// `src/config/option/ArcanistListConfigOption.php:48-69`).
///
/// Non-arrays fail with `Expected a list!`, non-natural lists with
/// `Expected a natural list!` (plain exceptions, i.e. fatal), and
/// non-string items with `Expected a string (at index ...)` (a usage
/// error, `ArcanistStringListConfigOption::validateListItem()`).
fn parse_onto_config(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let items = match value {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Object(map) => {
            natural_list_items(map).ok_or_else(|| "Expected a natural list!".to_string())?
        }
        _ => return Err("Expected a list!".to_string()),
    };
    let mut onto = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match item.as_str() {
            Some(name) => onto.push(name.to_string()),
            None => {
                return Err(format!(
                    "Expected a string (at index \"{index}\"), found \"{}\".",
                    describe_json_type(item)
                ));
            }
        }
    }
    Ok(onto)
}

/// Collect a JSON object's values in `0..len` key order when they form a
/// natural list (PHP decodes JSON objects to possibly-natural arrays).
fn natural_list_items(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<Vec<serde_json::Value>> {
    let mut items = vec![serde_json::Value::Null; map.len()];
    for (key, value) in map {
        let index: usize = key.parse().ok()?;
        // Canonical decimal keys covering `0..len`: unique map keys
        // reaching every index exactly once.
        if key != &index.to_string() || index >= map.len() {
            return None;
        }
        items[index] = value.clone();
    }
    Some(items)
}

/// `phutil_describe_type()` for JSON values in config-item errors.
/// Scalar names follow libphutil (`integer`, `double`, `boolean`,
/// `NULL`); nested arrays/objects report `array` (the exact libphutil
/// rendering for those is unrecoverable from here).
fn describe_json_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "NULL",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(number) => {
            if number.is_f64() {
                "double"
            } else {
                "integer"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => "array",
    }
}

/// Parse one string configuration value (`arc.land.onto-remote`,
/// `arc.land.strategy`).
fn parse_string_config(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

/// System-wide config file (`ArcanistConfigurationEngine.php:39-50`).
fn system_config_path() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var("ProgramData").unwrap_or_default();
        PathBuf::from(root)
            .join("Phabricator")
            .join("Arcanist")
            .join("config")
    } else {
        PathBuf::from("/etc/arcconfig")
    }
}

/// Load saved prompt responses from every configuration source.
///
/// Ports the multi-source read: system, user (`~/.arcrc`), project
/// (`.arcconfig`), and local (`.git/arc/config`, `.hg/arc/config`)
/// sources concatenate their `prompts` lists in order
/// (`ArcanistConfigurationEngine.php:32-63`,
/// `ArcanistMultiSourceConfigOption.php:6-30`), and the last answer
/// for a prompt key wins (`ArcanistPrompt::getSavedResponse()`,
/// `ArcanistPrompt.php:289-303`).
///
/// PHP requires each `prompts` value to be a list of `{prompt,
/// response}` maps (`ArcanistPromptsConfigOption.php:10-25`); the port
/// also accepts a `{prompt: response}` map and skips malformed entries.
fn load_saved_prompts(dir: &Path) -> HashMap<String, SavedResponse> {
    let mut files: Vec<(PathBuf, PromptScope)> = vec![(system_config_path(), PromptScope::System)];
    if let Some(user) = config::arcrc_path() {
        files.push((user, PromptScope::User));
    }
    if let Some(project) = find_dot_file(dir, ".arcconfig") {
        files.push((project, PromptScope::Project));
    }
    // The local file lives under the repository metadata directory at
    // the repo root (`getLocalConfigurationFilePath()`,
    // `ArcanistWorkingCopy.php:85-89`), not under the CWD. Git wins
    // over Mercurial, like [`detect_vcs`].
    let metadata = find_dot_dir(dir, ".git").or_else(|| find_dot_dir(dir, ".hg"));
    if let Some(root) = metadata {
        let local = root.join("arc").join("config");
        if local.is_file() {
            files.push((local, PromptScope::WorkingCopy));
        }
    }
    load_saved_prompts_from(&files)
}

/// Load saved prompt responses from explicit `(file, scope)` sources,
/// last match wins. Missing, blank, or invalid files read empty.
pub fn load_saved_prompts_from(files: &[(PathBuf, PromptScope)]) -> HashMap<String, SavedResponse> {
    let mut saved = HashMap::new();
    for (path, scope) in files {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        collect_saved_prompts(json.get("prompts"), *scope, &mut saved);
    }
    saved
}

/// Collect one `prompts` value (list or map form) into `saved`.
fn collect_saved_prompts(
    value: Option<&serde_json::Value>,
    scope: PromptScope,
    saved: &mut HashMap<String, SavedResponse>,
) {
    match value {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                let prompt = item.get("prompt").and_then(|v| v.as_str());
                let response = item.get("response").and_then(|v| v.as_str());
                if let (Some(prompt), Some(response)) = (prompt, response) {
                    saved.insert(
                        prompt.to_string(),
                        SavedResponse {
                            response: response.to_string(),
                            scope,
                        },
                    );
                }
            }
        }
        Some(serde_json::Value::Object(map)) => {
            for (key, response) in map {
                if let Some(text) = response.as_str() {
                    saved.insert(
                        key.clone(),
                        SavedResponse {
                            response: text.to_string(),
                            scope,
                        },
                    );
                }
            }
        }
        _ => {}
    }
}

/// A `--revision` symbol: a numeric ID or a revision PHID
/// (`ArcanistRevisionSymbolRef`, `src/ref/revision/ArcanistRevisionSymbolRef.php`,
/// resolved by `ArcanistSimpleSymbolRef::resolveSymbol()`,
/// `src/ref/simple/ArcanistSimpleSymbolRef.php:21-51`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionSymbol {
    /// `123` or `D123`.
    Id(u64),
    /// `PHID-DREV-...`.
    Phid(String),
}

impl RevisionSymbol {
    /// The normalized symbol text. (PHP's `getSymbol()` echoes the raw
    /// input instead; failure messages use the raw text via
    /// [`resolve_forced_revision_with_raw`].)
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            RevisionSymbol::Id(id) => id.to_string(),
            RevisionSymbol::Phid(phid) => phid.clone(),
        }
    }
}

/// Parse a `--revision` symbol (`D123`, `123`, or a `PHID-DREV-...`).
///
/// Ports the format half of `resolveSymbol()`: the ID pattern is
/// `[Dd]?[1-9][0-9]*` (no zero, no leading zeros) and the PHID pattern is
/// `PHID-DREV-` plus non-space, with the verbatim format error. See the
/// module docs for the intentional `(int)` cast deviation.
pub fn parse_revision_symbol(symbol: &str) -> Result<RevisionSymbol, String> {
    let digits = symbol
        .strip_prefix('D')
        .or_else(|| symbol.strip_prefix('d'))
        .unwrap_or(symbol);
    if is_revision_id(digits)
        && let Ok(id) = digits.parse::<u64>()
    {
        return Ok(RevisionSymbol::Id(id));
    }
    if is_revision_phid(symbol) {
        return Ok(RevisionSymbol::Phid(symbol.to_string()));
    }
    Err(format!(
        "The format of symbol \"{symbol}\" is unrecognized. Expected a monogram like \"X123\", \
         or an ID like \"123\", or a PHID."
    ))
}

/// True for `[1-9][0-9]*` (the numeric half of the symbol ID pattern).
fn is_revision_id(text: &str) -> bool {
    let mut bytes = text.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_digit() && first != b'0')
        && bytes.all(|byte| byte.is_ascii_digit())
}

/// True for `PHID-DREV-` plus non-space (`ArcanistSimpleSymbolRef.php:37-44`).
fn is_revision_phid(symbol: &str) -> bool {
    symbol
        .strip_prefix("PHID-DREV-")
        .is_some_and(|rest| !rest.is_empty() && !rest.chars().any(char::is_whitespace))
}

/// Validated merge strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Collapse each set into one commit (the default).
    Squash,
    /// Merge with a merge commit, keeping checkpoint commits.
    Merge,
}

/// Select the merge strategy: `--strategy`, then `arc.land.strategy`
/// configuration, then the `squash` default
/// (`ArcanistLandEngine::selectMergeStrategy()`,
/// `ArcanistLandEngine.php:1464-1525`).
///
/// Returns the strategy plus the `STRATEGY` status line.
pub fn select_strategy(
    argument: Option<&str>,
    config: Option<&str>,
) -> Result<(Strategy, String), String> {
    select_strategy_with_ansi(argument, config, terminal::ansi_enabled())
}

/// Select the merge strategy with an explicit ANSI flag for the
/// `STRATEGY` status line (the `writeStatus` blue writer).
pub fn select_strategy_with_ansi(
    argument: Option<&str>,
    config: Option<&str>,
    ansi: bool,
) -> Result<(Strategy, String), String> {
    const SUPPORTED: &str = "merge, squash";
    if let Some(strategy) = argument {
        let parsed = parse_strategy_name(strategy).ok_or_else(|| {
            format!(
                "Merge strategy \"{strategy}\" specified with \"--strategy\" is unknown. \
                 Supported merge strategies are: {SUPPORTED}."
            )
        })?;
        let line = status_line(
            "STRATEGY",
            &format!("Merging with \"{strategy}\" strategy, selected with \"--strategy\"."),
            "blue",
            ansi,
        );
        return Ok((parsed, line));
    }
    if let Some(strategy) = config {
        let parsed = parse_strategy_name(strategy).ok_or_else(|| {
            format!(
                "Merge strategy \"{strategy}\" specified in \"arc.land.strategy\" configuration \
                 is unknown. Supported merge strategies are: {SUPPORTED}."
            )
        })?;
        let line = status_line(
            "STRATEGY",
            &format!(
                "Merging with \"{strategy}\" strategy, configured with \"arc.land.strategy\"."
            ),
            "blue",
            ansi,
        );
        return Ok((parsed, line));
    }
    Ok((
        Strategy::Squash,
        status_line(
            "STRATEGY",
            "Merging with \"squash\" strategy, the default strategy.",
            "blue",
            ansi,
        ),
    ))
}

/// Parse one strategy name, or `None` when unknown.
fn parse_strategy_name(name: &str) -> Option<Strategy> {
    match name {
        "squash" => Some(Strategy::Squash),
        "merge" => Some(Strategy::Merge),
        _ => None,
    }
}

/// Validate argument combinations
/// (`ArcanistLandEngine::validateArguments()`,
/// `ArcanistLandEngine.php:1335-1395`).
///
/// Checks the into-local/into-remote/into-empty exclusivity, the
/// into/into-empty exclusivity, and the pick-under-merge ban, with the
/// verbatim errors. Strategy selection and revision-symbol validation run
/// here too, since they only need the options.
fn validate_options(options: &Options, dir: &Path, io: &mut Io) -> Result<Validated, String> {
    let mut into_count = 0;
    if options.into_remote.is_some() {
        into_count += 1;
    }
    if options.into_local {
        into_count += 1;
    }
    if options.into_empty {
        into_count += 1;
    }
    if into_count > 1 {
        return Err(
            "Arguments \"--into-local\", \"--into-remote\", and \"--into-empty\" are \
             mutually exclusive."
                .to_string(),
        );
    }
    if options.into.is_some() && options.into_empty {
        return Err(
            "Arguments \"--into\" and \"--into-empty\" are mutually exclusive.".to_string(),
        );
    }
    let strategy_config = land_config_value(options, dir, "arc.land.strategy")?
        .as_ref()
        .and_then(parse_string_config);
    let (strategy, line) = select_strategy_with_ansi(
        options.strategy.as_deref(),
        strategy_config.as_deref(),
        io.ansi,
    )?;
    io.push_err(&line);
    if options.pick && strategy != Strategy::Squash {
        return Err("You can not \"--pick\" changes under the \"merge\" strategy.".to_string());
    }
    let revision = options
        .revision
        .as_deref()
        .map(parse_revision_symbol)
        .transpose()?;
    Ok(Validated {
        strategy,
        revision,
        revision_raw: options.revision.clone(),
    })
}

/// Options plus validated selections.
#[derive(Debug)]
struct Validated {
    strategy: Strategy,
    revision: Option<RevisionSymbol>,
    /// Raw `--revision` text, echoed verbatim on resolution failure
    /// (PHP echoes `getSymbol()`, `ArcanistLandEngine.php:821-827`).
    revision_raw: Option<String>,
}

/// Resolve the conduit URI: the flag, `ARC_CONDUIT_URI`, or the
/// merged-config discovery from `dir` (like the diff port's
/// `discover_from(&dir)`, not the process working directory).
pub fn resolve_conduit_uri(conduit_uri: Option<&str>, dir: &Path) -> Option<String> {
    conduit_uri
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(|| config::discover_from(dir))
}

/// [`resolve_conduit_uri`] with an explicit user file and
/// [`get_config::LoadOptions`].
pub fn resolve_conduit_uri_with(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Option<String> {
    conduit_uri
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(|| config::discover_uri_with(dir, user_file, opts))
}

/// Resolve the conduit URI (flag, `ARC_CONDUIT_URI`, or merged-config
/// discovery from `dir`) and build an authenticated client from `~/.arcrc`.
///
/// Mirrors [`config::client_for`], but discovers from the target working
/// copy — like the diff port (`discover_from(&dir)`) — instead of the
/// process working directory.
pub fn client_for_dir(
    conduit_uri: Option<&str>,
    dir: &Path,
) -> Result<arc_rs::conduit::Client, String> {
    let uri = resolve_conduit_uri(conduit_uri, dir).ok_or_else(|| {
        "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project \
         with .arcconfig"
            .to_string()
    })?;
    let token = config::arcrc_path()
        .and_then(|path| config::token_for_file(&path, &uri))
        .ok_or_else(|| format!("no token for {uri} in ~/.arcrc"))?;
    arc_rs::conduit::Client::new(&uri, &token).map_err(|e| e.to_string())
}

/// [`client_for_dir`] with an explicit user file and
/// [`get_config::LoadOptions`], so `--config`/`--config-file`/
/// `--arcrc-file` reach conduit setup like stock PHP.
pub fn client_for_dir_with(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<arc_rs::conduit::Client, String> {
    config::client_for_with(conduit_uri, dir, user_file, opts)
}

/// Config overrides for a land run: parsed `--config`/`--config-file`/
/// `--arcrc-file` plus the default user file.
fn load_options_for_run(
    options: &Options,
) -> Result<(get_config::LoadOptions, Option<PathBuf>), String> {
    let cli = get_config::load_options_from_cli(
        &options.config,
        &options.config_file,
        options.arcrc_file.as_deref(),
    )?;
    Ok((cli, config::arcrc_path()))
}

/// Note printed when Conduit is unavailable and revision discovery is
/// skipped (a port extension; PHP requires Conduit).
pub fn format_skip_note(reason: &str) -> String {
    format_skip_note_with_ansi(reason, terminal::ansi_enabled())
}

/// Format the Conduit-unavailable note with an explicit ANSI flag.
/// The note carries no markup, so both modes render byte-identical
/// text through the console formatter.
pub fn format_skip_note_with_ansi(reason: &str, ansi: bool) -> String {
    terminal::console_format(
        "Note: revision info unavailable (%s); landing without revision association.\n",
        &[reason],
        ansi,
    )
}

/// Load revision rows for `symbol` via `differential.revision.search`,
/// following `cursor.after` pagination like `ConduitSearchFuture`.
///
/// Numeric symbols constrain `ids`; PHID symbols constrain `phids`
/// (`ArcanistRevisionSymbolRef`, `src/ref/revision/ArcanistRevisionSymbolRef.php:18-24`).
pub fn fetch_revision_views(
    call: &diff_submit::ConduitCall<'_>,
    symbol: &RevisionSymbol,
) -> Result<Vec<land_checks::RevisionView>, String> {
    let constraints = match symbol {
        RevisionSymbol::Id(id) => {
            let numeric =
                i64::try_from(*id).map_err(|_| format!("Revision ID {id} is out of range."))?;
            serde_json::json!({"ids": [numeric]})
        }
        RevisionSymbol::Phid(phid) => serde_json::json!({"phids": [phid]}),
    };
    search_revision_views(call, &constraints)
}

/// Run one `differential.revision.search` with `constraints`, following
/// `cursor.after` pagination like `ConduitSearchFuture`.
///
/// A missing or malformed `data` list degrades to no rows
/// (`ConduitSearchFuture::readResults()`,
/// `src/conduit/ConduitSearchFuture.php:112-114`).
fn search_revision_views(
    call: &diff_submit::ConduitCall<'_>,
    constraints: &serde_json::Value,
) -> Result<Vec<land_checks::RevisionView>, String> {
    let params = revision::conduit_search_params(constraints, None);
    let mut views = Vec::new();
    for page in conduit::search_pages(call, diff_submit::METHOD_REVISION_SEARCH, &params)? {
        let rows = page.get("data").and_then(|data| data.as_array());
        views.extend(
            rows.into_iter()
                .flatten()
                .map(land_checks::parse_revision)
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    Ok(views)
}

/// Resolve a `--revision` symbol to its revision, or fail with the
/// verbatim unresolved-symbol error
/// (`ArcanistLandEngine.php:823-829`).
pub fn resolve_forced_revision(
    call: &diff_submit::ConduitCall<'_>,
    symbol: &RevisionSymbol,
) -> Result<land_checks::RevisionView, String> {
    resolve_forced_revision_with_raw(call, symbol, &symbol.display())
}

/// [`resolve_forced_revision`] echoing the raw `--revision` text on
/// failure, as PHP echoes `getSymbol()`
/// (`ArcanistLandEngine.php:821-827`).
pub fn resolve_forced_revision_with_raw(
    call: &diff_submit::ConduitCall<'_>,
    symbol: &RevisionSymbol,
    raw: &str,
) -> Result<land_checks::RevisionView, String> {
    let views = fetch_revision_views(call, symbol)?;
    views
        .into_iter()
        .next()
        .ok_or_else(|| format!("Symbol \"{raw}\" does not identify a valid revision."))
}

/// Load the viewer PHID via `user.whoami` (`result['phid']`,
/// `src/workflow/ArcanistWorkflow.php:489-494`).
pub fn fetch_viewer_phid(call: &diff_submit::ConduitCall<'_>) -> Result<String, String> {
    let result = call(METHOD_WHOAMI, &serde_json::json!({}))?;
    result
        .get("phid")
        .and_then(|phid| phid.as_str())
        .filter(|phid| !phid.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "user.whoami response has no phid".to_string())
}

/// Load open-parent edges for `revision_phids` via `edge.search`,
/// following `cursor.after` pagination
/// (`ArcanistRevisionParentRevisionsHardpointQuery.php:16-43`).
///
/// Returns `(source PHID, destination PHID)` pairs.
pub fn fetch_parent_edges(
    call: &diff_submit::ConduitCall<'_>,
    revision_phids: &[String],
) -> Result<Vec<(String, String)>, String> {
    let mut edges = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut params = serde_json::json!({
            "sourcePHIDs": revision_phids,
            "types": ["revision.parent"],
        });
        if let Some(cursor) = &after {
            params["after"] = serde_json::Value::String(cursor.clone());
        }
        let result = call(METHOD_EDGE_SEARCH, &params)?;
        // A missing or malformed `data` list degrades to no edges
        // (PHP iterates `$results['data']` directly, which yields no
        // rows on malformed responses,
        // `ArcanistRevisionParentRevisionsHardpointQuery.php:34-36`).
        let items = result
            .get("data")
            .and_then(|data| data.as_array())
            .cloned()
            .unwrap_or_default();
        for item in &items {
            let src = item.get("sourcePHID").and_then(|v| v.as_str());
            let dst = item.get("destinationPHID").and_then(|v| v.as_str());
            if let (Some(src), Some(dst)) = (src, dst) {
                edges.push((src.to_string(), dst.to_string()));
            }
        }
        after = result
            .get("cursor")
            .and_then(|cursor| cursor.get("after"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if after.is_none() {
            break;
        }
    }
    Ok(edges)
}

/// Load parent revision views for the landing `revisions`.
///
/// Follows `revision.parent` edges, then resolves the distinct parent
/// PHIDs with one batched `differential.revision.search`, like
/// `ArcanistRevisionParentRevisionsHardpointQuery` (`:45-80`) feeding the
/// batched symbol hardpoint query
/// (`ArcanistSimpleSymbolHardpointQuery.php:47-62`).
pub fn fetch_parent_links(
    call: &diff_submit::ConduitCall<'_>,
    revisions: &[land_checks::RevisionView],
) -> Result<Vec<land_checks::ParentLink>, String> {
    let phids: Vec<String> = revisions.iter().map(|rev| rev.phid.clone()).collect();
    let edges = fetch_parent_edges(call, &phids)?;
    let mut distinct: Vec<String> = Vec::new();
    for (_, dst) in &edges {
        if !distinct.contains(dst) {
            distinct.push(dst.clone());
        }
    }
    let mut parents: HashMap<String, land_checks::RevisionView> = HashMap::new();
    if !distinct.is_empty() {
        let views = search_revision_views(call, &serde_json::json!({"phids": distinct}))?;
        for view in views {
            parents.insert(view.phid.clone(), view);
        }
    }
    let mut links = Vec::new();
    for (child_index, revision) in revisions.iter().enumerate() {
        for (src, dst) in &edges {
            if *src == revision.phid
                && let Some(parent) = parents.get(dst)
            {
                links.push(land_checks::ParentLink {
                    child_index,
                    parent: parent.clone(),
                });
            }
        }
    }
    Ok(links)
}

/// One buildable row as `harbormaster.buildable.search` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildableRow {
    phid: String,
    object_phid: String,
    monogram: String,
    uri: Option<String>,
}

/// Parse `harbormaster.buildable.search` data rows
/// (`ArcanistBuildableRef::newFromConduit()`, `getObjectPHID()`,
/// `getMonogram()`, `getURI()`,
/// `src/ref/buildable/ArcanistBuildableRef.php:23-63`).
fn parse_buildable_rows(result: &serde_json::Value) -> Vec<BuildableRow> {
    // A missing or malformed `data` list degrades to no rows
    // (`ConduitSearchFuture::readResults()`,
    // `src/conduit/ConduitSearchFuture.php:112-114`).
    let empty = Vec::new();
    let items = result
        .get("data")
        .and_then(|data| data.as_array())
        .unwrap_or(&empty);
    let mut rows = Vec::new();
    for item in items {
        let phid = item.get("phid").and_then(|v| v.as_str()).unwrap_or("");
        let object = item
            .get("fields")
            .and_then(|fields| fields.get("objectPHID"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if phid.is_empty() || object.is_empty() {
            continue;
        }
        let monogram = item
            .get("id")
            .and_then(json_id)
            .map(|id| format!("B{id}"))
            .unwrap_or_else(|| "B".to_string());
        let uri = item
            .get("fields")
            .and_then(|fields| fields.get("uri"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        rows.push(BuildableRow {
            phid: phid.to_string(),
            object_phid: object.to_string(),
            monogram,
            uri,
        });
    }
    rows
}

/// Read a numeric `id` field (number or numeric string), like the
/// revision parser.
fn json_id(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_u64()
            .and_then(|n| i64::try_from(n).ok())
            .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

/// One build row as `harbormaster.build.search` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildRow {
    status: String,
    buildable_phid: String,
    plan_phid: String,
    id: i64,
    name: String,
}

/// Parse `harbormaster.build.search` data rows
/// (`ArcanistBuildRef`, `src/ref/build/ArcanistBuildRef.php:26-62`).
fn parse_build_rows(result: &serde_json::Value) -> Vec<BuildRow> {
    // A missing or malformed `data` list degrades to no rows
    // (`ConduitSearchFuture::readResults()`,
    // `src/conduit/ConduitSearchFuture.php:112-114`).
    let empty = Vec::new();
    let items = result
        .get("data")
        .and_then(|data| data.as_array())
        .unwrap_or(&empty);
    let mut rows = Vec::new();
    for item in items {
        let fields = item.get("fields");
        let status = fields
            .and_then(|fields| fields.get("buildStatus"))
            .and_then(|status| status.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let buildable = fields
            .and_then(|fields| fields.get("buildablePHID"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let plan = fields
            .and_then(|fields| fields.get("buildPlanPHID"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = fields
            .and_then(|fields| fields.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        rows.push(BuildRow {
            status: status.to_string(),
            buildable_phid: buildable.to_string(),
            plan_phid: plan.to_string(),
            id: item.get("id").and_then(json_id).unwrap_or(0),
            name: name.to_string(),
        });
    }
    rows
}

/// Guess the absolute URI for a buildable, like
/// `ArcanistWorkflow::getAbsoluteURI()` (`src/workflow/ArcanistWorkflow.php:2423-2440`):
/// URIs with a domain stay as-is, otherwise the conduit URI's
/// scheme-plus-authority replaces the missing domain and the raw path
/// is kept (query strings dropped). Without a conduit URI the raw URI
/// (or the `/{monogram}` fallback, `ArcanistBuildableRef.php:55-63`)
/// is returned unchanged.
pub fn absolute_buildable_uri(
    conduit_uri: Option<&str>,
    monogram: &str,
    raw_uri: Option<&str>,
) -> String {
    let raw = raw_uri.unwrap_or_default();
    if raw.contains("://") {
        return raw.to_string();
    }
    let fallback = format!("/{monogram}");
    let path = if raw.is_empty() { &fallback } else { raw };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let Some(base) = conduit_uri.map(conduit_base_uri) else {
        return path.to_string();
    };
    format!("{base}{path}")
}

/// Scheme plus authority of a conduit URI
/// (`https://phab.example.com/api/` -> `https://phab.example.com`).
fn conduit_base_uri(conduit_uri: &str) -> String {
    let (scheme, rest) = match conduit_uri.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("https", conduit_uri),
    };
    let authority = rest.split('/').next().unwrap_or("");
    format!("{scheme}://{authority}")
}

/// Parse `harbormaster.buildplan.search` data rows into
/// `plan PHID -> arc-land behavior`
/// (`ArcanistBuildPlanRef::getBehavior()`,
/// `src/ref/buildplan/ArcanistBuildPlanRef.php:30-35`).
fn parse_plan_behaviors(result: &serde_json::Value) -> HashMap<String, String> {
    // A missing or malformed `data` list degrades to no rows
    // (`ConduitSearchFuture::readResults()`,
    // `src/conduit/ConduitSearchFuture.php:112-114`).
    let empty = Vec::new();
    let items = result
        .get("data")
        .and_then(|data| data.as_array())
        .unwrap_or(&empty);
    let mut behaviors = HashMap::new();
    for item in items {
        let phid = item.get("phid").and_then(|v| v.as_str()).unwrap_or("");
        if phid.is_empty() {
            continue;
        }
        let behavior = item
            .get("fields")
            .and_then(|fields| fields.get("behaviors"))
            .and_then(|all| all.get(land_checks::PLAN_BEHAVIOR_KEY))
            .and_then(|one| one.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or(land_checks::PLAN_ALWAYS);
        behaviors.insert(phid.to_string(), behavior.to_string());
    }
    behaviors
}

/// Load build views for the landing `revisions`, joined to revision PHIDs.
///
/// Ports the buildable/builds/plan hardpoint chain:
/// `harbormaster.buildable.search` with `objectPHIDs` (the revisions'
/// diff PHIDs) and `manual: false`
/// (`ArcanistRevisionBuildableHardpointQuery.php:29-34`),
/// `harbormaster.build.search` with `buildables`
/// (`ArcanistBuildableBuildsHardpointQuery.php:19-23`), and
/// `harbormaster.buildplan.search` with `phids`
/// (`ArcanistBuildBuildplanHardpointQuery.php:21-25`). Each search
/// follows `cursor.after` pagination like `ConduitSearchFuture`.
/// Revisions without a diff PHID (legacy rows) contribute no builds.
/// `conduit_uri` absolutizes buildable URIs (see
/// [`absolute_buildable_uri`]).
pub fn fetch_build_views(
    call: &diff_submit::ConduitCall<'_>,
    revisions: &[land_checks::RevisionView],
    conduit_uri: Option<&str>,
) -> Result<Vec<land_checks::BuildView>, String> {
    let diff_phids = collect_diff_phids(revisions);
    if diff_phids.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for page in conduit::search_pages(
        call,
        METHOD_BUILDABLE_SEARCH,
        &revision::conduit_search_params(
            &serde_json::json!({"objectPHIDs": diff_phids, "manual": false}),
            None,
        ),
    )? {
        rows.extend(parse_buildable_rows(&page));
    }
    // One buildable per diff PHID, last row wins
    // (`$buildable_refs[$object_phid] = $buildable_ref`,
    // `ArcanistRevisionBuildableHardpointQuery.php:36-41`).
    let rows = last_buildable_per_object(rows);
    // Join buildable objectPHID -> revision diffPHID, like
    // `confirmBuilds` (`ArcanistLandEngine.php:623-644`).
    let buildable_to_revision = join_buildable_revisions(&rows, revisions);
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let phids: Vec<String> = rows.iter().map(|row| row.phid.clone()).collect();
    let mut build_rows = Vec::new();
    for page in conduit::search_pages(
        call,
        METHOD_BUILD_SEARCH,
        &revision::conduit_search_params(&serde_json::json!({"buildables": phids}), None),
    )? {
        build_rows.extend(parse_build_rows(&page));
    }
    let behaviors = fetch_plan_behaviors(call, &build_rows)?;
    let mut views = Vec::new();
    for row in &build_rows {
        if let Some(view) =
            build_view_for_row(row, &rows, &buildable_to_revision, &behaviors, conduit_uri)
        {
            views.push(view);
        }
    }
    Ok(views)
}

/// Distinct non-empty diff PHIDs of `revisions` (legacy rows without one
/// contribute no builds).
fn collect_diff_phids(revisions: &[land_checks::RevisionView]) -> Vec<String> {
    let mut diff_phids: Vec<String> = Vec::new();
    for revision in revisions {
        if let Some(diff) = &revision.diff_phid
            && !diff.is_empty()
            && !diff_phids.contains(diff)
        {
            diff_phids.push(diff.clone());
        }
    }
    diff_phids
}

/// Join buildable PHID -> landing revision PHID over the revisions'
/// diff PHIDs (`ArcanistLandEngine.php:623-644`).
fn join_buildable_revisions(
    rows: &[BuildableRow],
    revisions: &[land_checks::RevisionView],
) -> HashMap<String, String> {
    let mut buildable_to_revision: HashMap<String, String> = HashMap::new();
    for row in rows {
        for revision in revisions {
            if revision.diff_phid.as_deref() == Some(row.object_phid.as_str()) {
                buildable_to_revision.insert(row.phid.clone(), revision.phid.clone());
            }
        }
    }
    buildable_to_revision
}

/// Load `plan PHID -> arc-land behavior` for the plans referenced by
/// `build_rows` (`ArcanistBuildBuildplanHardpointQuery.php:21-25`).
fn fetch_plan_behaviors(
    call: &diff_submit::ConduitCall<'_>,
    build_rows: &[BuildRow],
) -> Result<HashMap<String, String>, String> {
    let mut plan_phids: Vec<String> = build_rows
        .iter()
        .map(|row| row.plan_phid.clone())
        .filter(|phid| !phid.is_empty())
        .collect();
    plan_phids.sort();
    plan_phids.dedup();
    if plan_phids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut behaviors = HashMap::new();
    for page in conduit::search_pages(
        call,
        METHOD_BUILDPLAN_SEARCH,
        &revision::conduit_search_params(&serde_json::json!({"phids": plan_phids}), None),
    )? {
        behaviors.extend(parse_plan_behaviors(&page));
    }
    Ok(behaviors)
}

/// Keep the last buildable row per `objectPHID`, in first-seen object
/// order (PHP's `$buildable_refs` map keeps insertion order while
/// overwriting values).
fn last_buildable_per_object(rows: Vec<BuildableRow>) -> Vec<BuildableRow> {
    let mut order: Vec<String> = Vec::new();
    let mut by_object: HashMap<String, BuildableRow> = HashMap::new();
    for row in rows {
        if !by_object.contains_key(&row.object_phid) {
            order.push(row.object_phid.clone());
        }
        by_object.insert(row.object_phid.clone(), row);
    }
    order
        .into_iter()
        .filter_map(|object| by_object.remove(&object))
        .collect()
}

/// One build view for `row`, or `None` when its buildable belongs to no
/// landing revision.
fn build_view_for_row(
    row: &BuildRow,
    rows: &[BuildableRow],
    buildable_to_revision: &HashMap<String, String>,
    behaviors: &HashMap<String, String>,
    conduit_uri: Option<&str>,
) -> Option<land_checks::BuildView> {
    let revision_phid = buildable_to_revision.get(&row.buildable_phid)?;
    let behavior = plan_behavior_for(row, behaviors);
    let (monogram, uri) = buildable_display_for(rows, &row.buildable_phid, conduit_uri);
    Some(land_checks::BuildView {
        status: row.status.clone(),
        plan_behavior: behavior,
        revision_phid: revision_phid.clone(),
        build_id: row.id,
        build_name: row.name.clone(),
        buildable_monogram: monogram,
        buildable_uri: uri,
    })
}

/// The arc-land behavior for one build row: `None` (skipped) without a
/// plan ref (`ArcanistLandEngine.php:578-581`), else the plan's behavior.
fn plan_behavior_for(row: &BuildRow, behaviors: &HashMap<String, String>) -> Option<String> {
    if row.plan_phid.is_empty() {
        return None;
    }
    Some(
        behaviors
            .get(&row.plan_phid)
            .cloned()
            .unwrap_or_else(|| land_checks::PLAN_ALWAYS.to_string()),
    )
}

/// The `(monogram, uri)` display pair for one buildable PHID.
fn buildable_display_for(
    rows: &[BuildableRow],
    buildable_phid: &str,
    conduit_uri: Option<&str>,
) -> (String, String) {
    let buildable = rows
        .iter()
        .find(|candidate| candidate.phid == buildable_phid);
    match buildable {
        Some(found) => (
            found.monogram.clone(),
            absolute_buildable_uri(conduit_uri, &found.monogram, found.uri.as_deref()),
        ),
        None => (String::new(), String::new()),
    }
}

/// Load one revision's landed commit message via
/// `differential.getcommitmessage` with `{revision_id, edit: false}`
/// (`ArcanistRevisionCommitMessageHardpointQuery.php:22-33`).
pub fn fetch_commit_message(
    call: &diff_submit::ConduitCall<'_>,
    revision_id: i64,
) -> Result<String, String> {
    let result = call(
        diff_submit::METHOD_GET_COMMIT_MESSAGE,
        &serde_json::json!({"revision_id": revision_id, "edit": false}),
    )?;
    result
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "differential.getcommitmessage response is not a string".to_string())
}

/// Evaluate the pre-land gates for `revisions` over `call`.
///
/// Loads the viewer PHID, parent links, and builds, then runs
/// [`land_checks::evaluate_gates`]. A viewer failure propagates: PHP
/// throws (`ArcanistRuntime::getViewer()`,
/// `src/runtime/ArcanistRuntime.php:825-832`) instead of falling back to
/// a revision author.
pub fn load_gate_findings(
    call: &diff_submit::ConduitCall<'_>,
    revisions: &[land_checks::RevisionView],
    commit_count: usize,
) -> Result<Vec<land_checks::GateFinding>, String> {
    let viewer = fetch_viewer_phid(call)?;
    let links = fetch_parent_links(call, revisions)?;
    let landing: Vec<&str> = revisions.iter().map(|rev| rev.phid.as_str()).collect();
    // Gate verdicts never render URIs, so no conduit URI is needed.
    let builds = fetch_build_views(call, revisions, None)?;
    Ok(land_checks::evaluate_gates(
        revisions,
        &viewer,
        &links,
        &landing,
        &builds,
        Some(commit_count),
    ))
}

/// One gate finding as an indented line.
fn format_finding_line(finding: &land_checks::GateFinding, ansi: bool) -> String {
    terminal::console_format(
        "  [%s] %s: %s\n",
        &[
            finding.prompt,
            finding.title.as_str(),
            finding.query.as_str(),
        ],
        ansi,
    )
}

/// Advisory gate report for `--preview`: a pass line or listed findings.
pub fn format_gate_preview(findings: &[land_checks::GateFinding]) -> String {
    format_gate_preview_with_ansi(findings, terminal::ansi_enabled())
}

/// Advisory gate report with an explicit ANSI flag. The report
/// carries no markup, so both modes render byte-identical text
/// through the console formatter.
pub fn format_gate_preview_with_ansi(findings: &[land_checks::GateFinding], ansi: bool) -> String {
    if findings.is_empty() {
        return "Pre-land checks passed.\n".to_string();
    }
    let mut out = "Pre-land checks:\n".to_string();
    for finding in findings {
        out.push_str(&format_finding_line(finding, ansi));
    }
    out
}

/// One landing commit (`ArcanistLandCommit`, `src/land/ArcanistLandCommit.php`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandCommit {
    /// Full commit hash.
    pub hash: String,
    /// Parent hashes, newest first as listed by the VCS.
    pub parents: Vec<String>,
    /// Raw subject line.
    pub summary: String,
    /// Symbols directly naming this commit (first commit per symbol).
    pub direct_symbols: Vec<String>,
    /// All symbols this commit is reachable from.
    pub indirect_symbols: Vec<String>,
    /// Revision selected by discovery, if any.
    pub explicit: Option<land_checks::RevisionView>,
    /// Revisions related to this commit, before selection.
    pub related: Vec<land_checks::RevisionView>,
    /// True when no in-range child exists.
    pub is_head: bool,
    /// True when the commit lands under a set whose revision it was not
    /// explicitly associated with.
    pub is_implicit: bool,
}

/// Display summary: the subject truncated to 64 glyphs
/// (`ArcanistLandCommit::getDisplaySummary()`,
/// `src/land/ArcanistLandCommit.php:38-45`, via
/// `PhutilUTF8StringTruncator`).
pub fn display_summary(summary: &str) -> String {
    truncate_glyphs(summary, 64)
}

/// One `git log --format=%H%x00%P%x00%s%x00` line, split into fields.
fn split_git_log_line(line: &str) -> Result<(&str, &str, &str), String> {
    let mut parts = line.split('\0');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(hash), Some(parents), Some(summary)) if !hash.is_empty() => {
            Ok((hash, parents, summary))
        }
        _ => Err(format!("Unexpected output from \"git log ...\": {line}")),
    }
}

/// Parse `git log` NUL-separated lines into `(hash, parents, summary)`
/// triples (`ArcanistGitLandEngine::selectCommits()`, `:1563-1594`).
pub fn parse_git_log_lines(output: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut commits = Vec::new();
    for line in output.split('\n') {
        if line.is_empty() {
            continue;
        }
        let (hash, parents, summary) = split_git_log_line(line)?;
        commits.push((hash.to_string(), parents.to_string(), summary.to_string()));
    }
    Ok(commits)
}

/// Parse one `hg log` template line, `{node}-{parents}-{firstline}`
/// (`ArcanistMercurialLandEngine::selectCommits()`, `:726-757`).
pub fn parse_hg_log_line(line: &str) -> Result<(String, String, String), String> {
    let mut parts = line.splitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(hash), Some(parents), Some(summary)) if !hash.is_empty() => {
            Ok((hash.to_string(), parents.to_string(), summary.to_string()))
        }
        _ => Err(format!("Unexpected output from \"hg log ...\": {line}")),
    }
}

/// Split a space-separated parent list.
fn split_parents(parents: &str) -> Vec<String> {
    let trimmed = parents.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split(' ').map(str::to_string).collect()
    }
}

/// Insert `git log` output for `symbol` into `commits` (newest first),
/// tagging direct/indirect symbols
/// (`ArcanistGitLandEngine.php:1564-1603`).
pub fn collect_git_commits(
    commits: &mut Vec<LandCommit>,
    output: &str,
    symbol: &str,
) -> Result<(), String> {
    let mut first = true;
    for (hash, parents, summary) in parse_git_log_lines(output)? {
        let position = commits.iter().position(|commit| commit.hash == hash);
        let index = match position {
            Some(index) => index,
            None => {
                commits.push(LandCommit {
                    hash: hash.clone(),
                    parents: split_parents(&parents),
                    summary,
                    direct_symbols: Vec::new(),
                    indirect_symbols: Vec::new(),
                    explicit: None,
                    related: Vec::new(),
                    is_head: false,
                    is_implicit: false,
                });
                commits.len() - 1
            }
        };
        if first {
            commits[index].direct_symbols.push(symbol.to_string());
            first = false;
        }
        if !commits[index]
            .indirect_symbols
            .contains(&symbol.to_string())
        {
            commits[index].indirect_symbols.push(symbol.to_string());
        }
    }
    Ok(())
}

/// Insert `hg log` output for `symbol` into `commits` (max first via
/// `reverse()`, `:696-767`).
pub fn collect_hg_commits(
    commits: &mut Vec<LandCommit>,
    output: &str,
    symbol: &str,
) -> Result<(), String> {
    let mut first = true;
    for line in output.split('\n') {
        if line.is_empty() {
            continue;
        }
        let (hash, parents, summary) = parse_hg_log_line(line)?;
        let position = commits.iter().position(|commit| commit.hash == hash);
        let index = match position {
            Some(index) => index,
            None => {
                commits.push(LandCommit {
                    hash: hash.clone(),
                    parents: split_parents(&parents),
                    summary,
                    direct_symbols: Vec::new(),
                    indirect_symbols: Vec::new(),
                    explicit: None,
                    related: Vec::new(),
                    is_head: false,
                    is_implicit: false,
                });
                commits.len() - 1
            }
        };
        if first {
            commits[index].direct_symbols.push(symbol.to_string());
            first = false;
        }
        if !commits[index]
            .indirect_symbols
            .contains(&symbol.to_string())
        {
            commits[index].indirect_symbols.push(symbol.to_string());
        }
    }
    Ok(())
}

/// Reverse a newest-first commit list to oldest-first, link in-range
/// parents, and mark head commits without in-range children
/// (`ArcanistLandEngine::confirmCommits()`, `:1011-1099`, minus the
/// banners, which [`confirm_commit_count`] prints).
pub fn link_commits(commits: &mut [LandCommit]) {
    commits.reverse();
    let hashes: Vec<String> = commits.iter().map(|commit| commit.hash.clone()).collect();
    let mut has_child = vec![false; commits.len()];
    for (index, commit) in commits.iter().enumerate() {
        for parent in &commit.parents {
            if let Some(parent_index) = hashes.iter().position(|hash| hash == parent) {
                has_child[parent_index] = true;
            }
            let _ = index;
        }
    }
    for (commit, headed) in commits.iter_mut().zip(has_child) {
        commit.is_head = !headed;
    }
}

/// Print the commit-count banners and resolve the large-working-set gate.
///
/// Ports the banner half of `confirmCommits()`
/// (`ArcanistLandEngine.php:986-1070`): no commits prints the
/// `NOTHING TO LAND` banner and fails with `There are no commits to
/// land.`; more than the workflow limit prints the `LARGE WORKING SET`
/// banner (oldest 5, an ellipsis line, newest 5) and prompts. Expects
/// oldest-first commits with display hashes resolved by the caller.
pub fn confirm_commit_count(
    io: &mut Io,
    commits: &[LandCommit],
    symbols_display: &str,
    into_display: Option<&str>,
    display_hash: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    if commits.is_empty() {
        let into = into_display.unwrap_or_default();
        let message = format!(
            "There are no commits reachable from the specified sources ({symbols_display}) which \
             are not already present in the state you are merging into (\"{into}\"), so nothing \
             can land."
        );
        push_banner(io, "NOTHING TO LAND", &message);
        return Err("There are no commits to land.".to_string());
    }
    let count = commits.len();
    let limit = land_checks::LARGE_WORKING_SET_LIMIT;
    if count <= limit {
        return Ok(());
    }
    let message = match into_display {
        None => format!(
            "There are {count} commit(s) reachable from the specified sources \
             ({symbols_display}). You are landing into the empty state, so all of these commits \
             will land:"
        ),
        Some(into) => format!(
            "There are {count} commit(s) reachable from the specified sources ({symbols_display}) \
             that are not present in the repository state you are merging into (\"{into}\"). All \
             of these commits will land:"
        ),
    };
    push_banner(io, "LARGE WORKING SET", &message);
    const SHOW: usize = 5;
    for commit in commits.iter().take(SHOW) {
        let hash = display_hash(&commit.hash);
        let summary = display_summary(&commit.summary);
        io.push_out(&terminal::console_format(
            "  %s %s\n",
            &[&hash, &summary],
            io.ansi,
        ));
    }
    let more = (count - SHOW * 2).to_string();
    io.push_out(&terminal::console_format(
        "  < ... %s more commits ... >\n",
        &[&more],
        io.ansi,
    ));
    for commit in commits.iter().skip(count - SHOW) {
        let hash = display_hash(&commit.hash);
        let summary = display_summary(&commit.summary);
        io.push_out(&terminal::console_format(
            "  %s %s\n",
            &[&hash, &summary],
            io.ansi,
        ));
    }
    let query = format!("Land {count} commit(s)?");
    prompt_for_key(io, land_checks::PROMPT_LARGE_WORKING_SET, &query)
}

/// Quote and join source symbols for display
/// (`ArcanistLandEngine::getDisplaySymbols()`, `:758-766`).
pub fn display_symbols(symbols: &[String]) -> String {
    symbols
        .iter()
        .map(|symbol| {
            let escaped = symbol.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Extract the `Differential Revision` ID from a commit message corpus.
///
/// Ports `ArcanistDifferentialCommitMessage::parseRevisionIDFromRawCorpus()`
/// (`src/differential/ArcanistDifferentialCommitMessage.php:114-148`): the
/// first line starting with `Differential Revision:` (case-insensitive)
/// supplies the value (the rest of the line, trimmed), which must be a
/// `D123` monogram or a URI whose path is one. Anything else is invalid:
/// PHP throws an `ArcanistUsageException` there, which the message
/// hardpoint query catches per commit
/// (`ArcanistMessageRevisionHardpointQuery.php:33-41`), so callers treat
/// an invalid or absent field as "no ID" and the result holds at most
/// one ID. Bare digits, `D0`, and trailing garbage are invalid.
pub fn differential_revision_ids(message: &str) -> Vec<u64> {
    const TAG: &[u8] = b"differential revision:";
    for line in message.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.len() < TAG.len() || !line.as_bytes()[..TAG.len()].eq_ignore_ascii_case(TAG) {
            continue;
        }
        let Some(value) = line.get(TAG.len()..) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            return Vec::new();
        }
        return parse_differential_revision_value(value)
            .into_iter()
            .collect();
    }
    Vec::new()
}

/// Parse one `Differential Revision` field value: `D123`, or a URI whose
/// path (leading/trailing slashes trimmed) is `D123`. Returns `None`
/// for anything else, where PHP throws.
fn parse_differential_revision_value(value: &str) -> Option<u64> {
    if let Some(id) = parse_revision_monogram(value) {
        return Some(id);
    }
    // Otherwise, try to find a full URI: strip any query/fragment,
    // drop the scheme and authority, and match the path.
    let path = value.split(['?', '#']).next().unwrap_or(value);
    let path = match path.find("://") {
        Some(scheme_end) => path[scheme_end + 3..]
            .split_once('/')
            .map_or("", |(_, rest)| rest),
        None => path,
    };
    parse_revision_monogram(path.trim_matches('/'))
}

/// Parse a `D123` monogram (`[dD][1-9][0-9]*`, anchored).
fn parse_revision_monogram(text: &str) -> Option<u64> {
    let digits = text.strip_prefix('D').or_else(|| text.strip_prefix('d'))?;
    if !is_revision_id(digits) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Query legacy `differential.query` rows by revision ID.
pub fn query_revisions_by_ids(
    call: &diff_submit::ConduitCall<'_>,
    ids: &[u64],
) -> Result<Vec<land_checks::RevisionView>, String> {
    let ids: Vec<i64> = ids
        .iter()
        .map(|id| i64::try_from(*id).unwrap_or(i64::MAX))
        .collect();
    let result = call(diff_submit::METHOD_QUERY, &serde_json::json!({"ids": ids}))?;
    let rows = result.as_array().cloned().unwrap_or_default();
    rows.iter().map(land_checks::parse_revision).collect()
}

/// Query legacy `differential.query` rows by commit hash.
///
/// Each hash is a `(type, value)` pair: `gtcm`/`gttr` under git,
/// `hgcm` under Mercurial
/// (`ArcanistDifferentialRevisionHash.php:7-9`,
/// `ArcanistGitAPI.php:1221-1232`).
pub fn query_revisions_by_hashes(
    call: &diff_submit::ConduitCall<'_>,
    hashes: &[(String, String)],
) -> Result<Vec<land_checks::RevisionView>, String> {
    let pairs: Vec<serde_json::Value> = hashes
        .iter()
        .map(|(kind, hash)| serde_json::json!([kind, hash]))
        .collect();
    let result = call(
        diff_submit::METHOD_QUERY,
        &serde_json::json!({"commitHashes": pairs}),
    )?;
    let rows = result.as_array().cloned().unwrap_or_default();
    rows.iter().map(land_checks::parse_revision).collect()
}

/// Attach related revisions to every commit and select explicit ones
/// (`ArcanistLandEngine::loadRevisionRefs()`, `:798-984`).
///
/// `message_of` reads a commit's full message; `hashes_of` lists the
/// commit-hash pairs to look up. Message-derived IDs resolve through one
/// batched `differential.revision.search` (rows keep `diffPHID`,
/// `ArcanistMessageRevisionHardpointQuery.php:56-60`) and commit hashes
/// through one batched `differential.query`
/// (`ArcanistGitWorkingCopyRevisionHardpointQuery.php:47-78`); every
/// commit's related revisions are the PHID-deduped union of both
/// (`ArcanistObjectListHardpoint.php:10-24`). Commits with exactly one
/// related revision select it; a forced `--revision` selects itself for
/// commits with no related revisions (or related sets including it),
/// fails with the TODO error when other commits belong elsewhere, and
/// ambiguity without a forced revision prints the `AMBIGUOUS REVISION`
/// banner and fails.
/// Reads `(hash-type, hash)` pairs for one commit.
type HashesOf<'a> = &'a dyn Fn(&str) -> Result<Vec<(String, String)>, String>;

pub fn load_revision_refs(
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
    commits: &mut [LandCommit],
    message_of: &dyn Fn(&str) -> Result<String, String>,
    hashes_of: HashesOf<'_>,
    forced: Option<&land_checks::RevisionView>,
) -> Result<(), String> {
    let mut related: Vec<Vec<land_checks::RevisionView>> = vec![Vec::new(); commits.len()];
    attach_message_revisions(call, commits, message_of, &mut related)?;
    attach_hash_revisions(call, commits, hashes_of, &mut related)?;
    for (commit, views) in commits.iter_mut().zip(related) {
        // Union the two hardpoint results, PHID-deduped
        // (`ArcanistObjectListHardpoint::mergeHardpointValues()`).
        let mut union: Vec<land_checks::RevisionView> = Vec::new();
        for view in views {
            if !union.iter().any(|known| known.phid == view.phid) {
                union.push(view);
            }
        }
        commit.related = union;
    }
    for commit in commits.iter_mut() {
        if commit.related.len() == 1 {
            commit.explicit = Some(commit.related[0].clone());
        }
    }
    if let Some(force) = forced {
        apply_forced_revision(commits, force)?;
    } else {
        reject_ambiguous_commits(io, commits)?;
    }
    Ok(())
}

/// Resolve message-derived revision IDs through one batched
/// `differential.revision.search`, attaching each returned revision to
/// the commits whose messages named it
/// (`ArcanistMessageRevisionHardpointQuery.php:29-70`).
fn attach_message_revisions(
    call: &diff_submit::ConduitCall<'_>,
    commits: &[LandCommit],
    message_of: &dyn Fn(&str) -> Result<String, String>,
    related: &mut [Vec<land_checks::RevisionView>],
) -> Result<(), String> {
    let mut id_to_commits: HashMap<i64, Vec<usize>> = HashMap::new();
    for (index, commit) in commits.iter().enumerate() {
        let message = message_of(&commit.hash)?;
        for id in differential_revision_ids(&message) {
            let id = i64::try_from(id).unwrap_or(i64::MAX);
            id_to_commits.entry(id).or_default().push(index);
        }
    }
    if id_to_commits.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = id_to_commits.keys().copied().collect();
    let views = search_revision_views(call, &serde_json::json!({"ids": ids}))?;
    for view in views {
        if let Some(indexes) = id_to_commits.get(&view.id) {
            for index in indexes {
                related[*index].push(view.clone());
            }
        }
    }
    Ok(())
}

/// Resolve commit hashes through one batched `differential.query`
/// `commitHashes` call, attaching each returned revision to the commits
/// whose hash pairs its `hashes` list echoes back
/// (`ArcanistGitWorkingCopyRevisionHardpointQuery.php:20-78`).
fn attach_hash_revisions(
    call: &diff_submit::ConduitCall<'_>,
    commits: &[LandCommit],
    hashes_of: HashesOf<'_>,
    related: &mut [Vec<land_checks::RevisionView>],
) -> Result<(), String> {
    let mut hash_to_commits: HashMap<String, Vec<usize>> = HashMap::new();
    let mut hashes: Vec<(String, String)> = Vec::new();
    for (index, commit) in commits.iter().enumerate() {
        for (kind, hash) in hashes_of(&commit.hash)? {
            hash_to_commits
                .entry(format!("{kind}:{hash}"))
                .or_default()
                .push(index);
            hashes.push((kind, hash));
        }
    }
    if hashes.is_empty() {
        return Ok(());
    }
    let pairs: Vec<serde_json::Value> = hashes
        .iter()
        .map(|(kind, hash)| serde_json::json!([kind, hash]))
        .collect();
    let result = call(
        diff_submit::METHOD_QUERY,
        &serde_json::json!({"commitHashes": pairs}),
    )?;
    let rows = result.as_array().cloned().unwrap_or_default();
    for row in rows {
        let view = land_checks::parse_revision(&row)?;
        let empty = Vec::new();
        let echoed = row
            .get("hashes")
            .and_then(|hashes| hashes.as_array())
            .unwrap_or(&empty);
        for pair in echoed {
            let key = pair
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .unwrap_or_default();
            if let Some(indexes) = hash_to_commits.get(&key) {
                for index in indexes {
                    related[*index].push(view.clone());
                }
            }
        }
    }
    Ok(())
}

/// Select a forced `--revision` for eligible commits, or fail when commits
/// belong to other revisions (`ArcanistLandEngine.php:856-920`).
fn apply_forced_revision(
    commits: &mut [LandCommit],
    force: &land_checks::RevisionView,
) -> Result<(), String> {
    for commit in commits.iter_mut() {
        if commit.explicit.is_some() {
            continue;
        }
        if commit.related.is_empty() {
            commit.explicit = Some(force.clone());
            continue;
        }
        if commit
            .related
            .iter()
            .any(|related| related.phid == force.phid)
        {
            commit.explicit = Some(force.clone());
        }
    }
    let mut foreign = false;
    for commit in commits.iter() {
        let Some(explicit) = &commit.explicit else {
            continue;
        };
        if explicit.phid != force.phid {
            foreign = true;
        }
    }
    if foreign {
        return Err(
            "TODO: You are forcing a revision, but commits are associated with some other \
             revision. Are you REALLY sure you want to land ALL these commits with a different \
             unrelated revision???"
                .to_string(),
        );
    }
    Ok(())
}

/// Fail on commits related to several revisions without a forced selection
/// (`ArcanistLandEngine.php:922-980`).
///
/// Related revisions render as ref views (`:966-970`). PHP interpolates
/// the leftover `$hash` loop variable instead of the ambiguous commit
/// (`:944`) and joins raw (unquoted) symbols; the port reports the
/// actual commit and matches the unquoted join.
fn reject_ambiguous_commits(io: &mut Io, commits: &[LandCommit]) -> Result<(), String> {
    for commit in commits {
        if commit.explicit.is_some() || commit.related.is_empty() {
            continue;
        }
        let symbols = commit.indirect_symbols.join(", ");
        let short = commit.hash.chars().take(12).collect::<String>();
        let message = format!(
            "The revision associated with commit \"{short}\" (an ancestor of: {symbols}) is \
             ambiguous. These {} revision(s) are associated with the commit:",
            commit.related.len()
        );
        push_banner(io, "AMBIGUOUS REVISION", &message);
        for related in &commit.related {
            io.push_out(&revision_ref_view(related).render_with_ansi(io.ansi));
        }
        io.push_out("\n");
        return Err(format!(
            "Revision for commit \"{short}\" is ambiguous. Use \"--revision\" to force selection \
             of a particular revision."
        ));
    }
    Ok(())
}

/// Resolve a commit's revision: explicit, else the single distinct
/// in-range parent revision (`ArcanistLandCommit::getRevisionRef()` /
/// `newRevisionRef()`, `src/land/ArcanistLandCommit.php:125-158`).
///
/// Several distinct parent revisions fail with `Too many distinct parent
/// refs!`; no revision anywhere resolves to `None` (the caller reports
/// `UNKNOWN REVISION`).
pub fn resolve_commit_revision(
    commits: &[LandCommit],
    index: usize,
) -> Result<Option<land_checks::RevisionView>, String> {
    resolve_commit_revision_inner(commits, index, &mut Vec::new())
}

fn resolve_commit_revision_inner(
    commits: &[LandCommit],
    index: usize,
    stack: &mut Vec<usize>,
) -> Result<Option<land_checks::RevisionView>, String> {
    if let Some(explicit) = &commits[index].explicit {
        return Ok(Some(explicit.clone()));
    }
    if stack.contains(&index) {
        return Ok(None);
    }
    stack.push(index);
    let mut found: Option<land_checks::RevisionView> = None;
    for parent in &commits[index].parents {
        let Some(parent_index) = commits.iter().position(|commit| &commit.hash == parent) else {
            continue;
        };
        let parent_ref = resolve_commit_revision_inner(commits, parent_index, stack)?;
        if let Some(parent_ref) = parent_ref {
            if let Some(existing) = &found {
                if existing.phid != parent_ref.phid {
                    return Err("Too many distinct parent refs!".to_string());
                }
            } else {
                found = Some(parent_ref);
            }
        }
    }
    stack.pop();
    Ok(found)
}

/// Ancestor revision PHIDs, parents first then self
/// (`ArcanistLandCommit::getAncestorRevisionPHIDs()`,
/// `src/land/ArcanistLandCommit.php:110-123`).
pub fn ancestor_revision_phids(commits: &[LandCommit], index: usize) -> Vec<String> {
    let mut phids = Vec::new();
    ancestor_revision_phids_inner(commits, index, &mut Vec::new(), &mut phids);
    phids
}

fn ancestor_revision_phids_inner(
    commits: &[LandCommit],
    index: usize,
    stack: &mut Vec<usize>,
    phids: &mut Vec<String>,
) {
    if stack.contains(&index) {
        return;
    }
    stack.push(index);
    for parent in &commits[index].parents {
        if let Some(parent_index) = commits.iter().position(|commit| &commit.hash == parent) {
            ancestor_revision_phids_inner(commits, parent_index, stack, phids);
        }
    }
    if let Ok(Some(revision)) = resolve_commit_revision(commits, index) {
        let phid = revision.phid.clone();
        if !phids.contains(&phid) {
            phids.push(phid);
        }
    }
    stack.pop();
}

/// One revision's commits in landing order (`ArcanistLandCommitSet`,
/// `src/land/ArcanistLandCommitSet.php`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSet {
    /// The set's revision.
    pub revision: Option<land_checks::RevisionView>,
    /// Indexes into the oldest-first commit list.
    pub commits: Vec<usize>,
    /// True when `--pick` selected this set directly.
    pub is_pick: bool,
}

impl CommitSet {
    /// True when any commit was implicitly associated
    /// (`hasImplicitCommits()`, `:43-51`).
    fn has_implicit_commits(&self, commits: &[LandCommit]) -> bool {
        self.commits.iter().any(|index| commits[*index].is_implicit)
    }

    /// True when any commit carries a direct symbol
    /// (`hasDirectSymbols()`, `:53-61`).
    fn has_direct_symbols(&self, commits: &[LandCommit]) -> bool {
        self.commits
            .iter()
            .any(|index| !commits[*index].direct_symbols.is_empty())
    }
}

/// Group commits into revision sets, oldest ancestors first
/// (`ArcanistLandEngine::execute()`, `:1140-1191`).
///
/// Every commit must resolve to a revision; unresolvable commits print the
/// `UNKNOWN REVISION` banner and fail. Implicit flags are assigned like
/// `setCommits()` (`ArcanistLandCommitSet.php:19-37`): commits whose
/// explicit revision is not the set revision become implicit.
pub fn group_commit_sets(
    io: &mut Io,
    commits: &mut [LandCommit],
    display_hash: &dyn Fn(&str) -> String,
) -> Result<Vec<CommitSet>, String> {
    let groups = resolve_group_keys(io, commits, display_hash)?;
    // Order by ancestor walk over head commits (`:1164-1180`).
    let order = order_group_phids(commits);
    let mut sets = ordered_commit_sets(commits, &groups, &order)?;
    push_unordered_groups(&mut sets, commits, &groups)?;
    mark_implicit_commits(commits, &sets);
    Ok(sets)
}

/// Resolve every commit to its revision PHID and bucket indexes by PHID
/// (first-seen order). Unresolvable commits print the `UNKNOWN REVISION`
/// banner and fail (`ArcanistLandEngine.php:1140-1159`).
fn resolve_group_keys(
    io: &mut Io,
    commits: &[LandCommit],
    display_hash: &dyn Fn(&str) -> String,
) -> Result<Vec<(String, Vec<usize>)>, String> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, commit) in commits.iter().enumerate() {
        let revision = resolve_commit_revision(commits, index)?
            .ok_or_else(|| unknown_revision(io, &commit.hash, display_hash))?;
        push_group_member(&mut groups, revision.phid.clone(), index);
    }
    Ok(groups)
}

/// Append `index` to the `phid` bucket, creating it first-seen.
fn push_group_member(groups: &mut Vec<(String, Vec<usize>)>, phid: String, index: usize) {
    match groups.iter_mut().find(|(key, _)| *key == phid) {
        Some((_, members)) => members.push(index),
        None => groups.push((phid, vec![index])),
    }
}

/// Revision PHIDs in landing order: the ancestor walk over head commits
/// (`:1164-1180`).
fn order_group_phids(commits: &[LandCommit]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for (index, commit) in commits.iter().enumerate() {
        if !commit.is_head {
            continue;
        }
        for phid in ancestor_revision_phids(commits, index) {
            if !order.contains(&phid) {
                order.push(phid);
            }
        }
    }
    order
}

/// Commit sets for the ordered PHIDs, skipping groups with no members.
fn ordered_commit_sets(
    commits: &[LandCommit],
    groups: &[(String, Vec<usize>)],
    order: &[String],
) -> Result<Vec<CommitSet>, String> {
    let mut sets = Vec::new();
    for phid in order {
        let Some((_, members)) = groups.iter().find(|(key, _)| key == phid) else {
            continue;
        };
        sets.push(commit_set_for(commits, members)?);
    }
    Ok(sets)
}

/// Append groups unreachable from a head (no heads marked), keeping map
/// order.
fn push_unordered_groups(
    sets: &mut Vec<CommitSet>,
    commits: &[LandCommit],
    groups: &[(String, Vec<usize>)],
) -> Result<(), String> {
    for (phid, members) in groups {
        if sets_assembled(sets, phid) {
            continue;
        }
        sets.push(commit_set_for(commits, members)?);
    }
    Ok(())
}

/// True when `sets` already holds a set for `phid`.
fn sets_assembled(sets: &[CommitSet], phid: &str) -> bool {
    sets.iter()
        .any(|set: &CommitSet| set.revision.as_ref().is_some_and(|rev| rev.phid == phid))
}

/// One commit set for `members` (the first member's revision).
fn commit_set_for(commits: &[LandCommit], members: &[usize]) -> Result<CommitSet, String> {
    let revision = resolve_commit_revision(commits, members[0])?.expect("grouped revision");
    Ok(CommitSet {
        revision: Some(revision),
        commits: members.to_vec(),
        is_pick: false,
    })
}

/// Flag implicitly-associated commits, like `setCommits()`
/// (`ArcanistLandCommitSet.php:19-37`): commits whose explicit revision is
/// not the set revision become implicit.
fn mark_implicit_commits(commits: &mut [LandCommit], sets: &[CommitSet]) {
    for set in sets {
        let phid = set.revision.as_ref().map(|rev| rev.phid.clone());
        for index in &set.commits {
            let commit = &mut commits[*index];
            let explicit_is_set = commit
                .explicit
                .as_ref()
                .is_some_and(|explicit| Some(&explicit.phid) == phid.as_ref());
            if !explicit_is_set {
                commit.is_implicit = true;
            }
        }
    }
}

/// Print the `UNKNOWN REVISION` banner and build its error
/// (`ArcanistLandEngine.php:1143-1159`).
fn unknown_revision(io: &mut Io, hash: &str, display_hash: &dyn Fn(&str) -> String) -> String {
    let short = display_hash(hash);
    let message = format!(
        "Unable to determine which revision is associated with commit \"{short}\". Use \"arc \
         diff\" to create or update a revision with this commit, or \"--revision\" to force \
         selection of a particular revision."
    );
    push_banner(io, "UNKNOWN REVISION", &message);
    format!("Unable to determine revision for commit \"{short}\".")
}

/// Drop closed-ancestor sets under squash and apply `--pick`
/// (`ArcanistLandEngine::filterCommitSets()`, `:1527-1586`).
///
/// Closed revisions without direct symbols print the
/// `DISCARDING ANCESTORS` banner and are skipped (squash only); `--pick`
/// keeps only sets with direct symbols and marks them picked.
pub fn filter_commit_sets(
    io: &mut Io,
    sets: Vec<CommitSet>,
    commits: &[LandCommit],
    squash: bool,
    pick: bool,
    render_set: &dyn Fn(&CommitSet) -> String,
) -> Vec<CommitSet> {
    let mut kept = sets;
    if squash {
        let mut discarded = Vec::new();
        kept.retain(|set| {
            let discardable = set
                .revision
                .as_ref()
                .is_some_and(|revision| revision.is_closed())
                && !set.has_direct_symbols(commits);
            if discardable {
                discarded.push(set.clone());
                false
            } else {
                true
            }
        });
        if !discarded.is_empty() {
            push_banner_continued(
                io,
                "DISCARDING ANCESTORS",
                "Some ancestor commits are associated with revisions that have already been \
                 closed. These changes will be skipped:",
            );
            for set in &discarded {
                io.push_out(&render_set(set));
            }
            io.push_out("\n");
        }
    }
    if pick {
        for set in kept.iter_mut() {
            if set.has_direct_symbols(commits) {
                set.is_pick = true;
            }
        }
        kept.retain(|set| set.is_pick);
    }
    kept
}

/// Default git source symbols: the current branch, else the current
/// commit hash (`ArcanistGitLandEngine::getDefaultSymbols()`, `:1609-1633`).
pub fn git_default_symbols(dir: &Path, io: &mut Io) -> Result<Vec<String>, String> {
    let branch = git::branch_name(dir).map_err(|_| "not inside a git repository".to_string())?;
    if let Some(branch) = branch {
        io.push_status(
            "SOURCE",
            &format!("Landing the current branch, \"{branch}\"."),
        );
        return Ok(vec![branch]);
    }
    let commit = git::working_copy_revision(dir)?;
    io.push_status(
        "SOURCE",
        &format!("Landing the current HEAD, \"{commit}\"."),
    );
    Ok(vec![commit])
}

/// Resolve git source symbols to commits
/// (`ArcanistGitLandEngine::resolveSymbols()`, `:942-963`).
pub fn git_resolve_symbols(dir: &Path, symbols: &[String]) -> Result<Vec<String>, String> {
    let mut commits = Vec::new();
    for symbol in symbols {
        let output = git::output(dir, &["rev-parse", "--verify", symbol])?;
        if !output.status.success() {
            return Err(format!(
                "Branch \"{symbol}\" does not exist in the local working copy."
            ));
        }
        commits.push(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Ok(commits)
}

/// True when `remote` is a Perforce remote: named `p4` with
/// `refs/remotes/p4` present (`ArcanistGitAPI::isPerforceRemote()`,
/// `src/repository/api/ArcanistGitAPI.php:1522-1540`).
pub fn git_is_perforce_remote(dir: &Path, remote: &str) -> bool {
    if remote != "p4" {
        return false;
    }
    let git_dir = git::output(dir, &["rev-parse", "--git-dir"])
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|_| ".git".to_string());
    let root = if Path::new(&git_dir).is_absolute() {
        PathBuf::from(git_dir)
    } else {
        dir.join(git_dir)
    };
    root.join("refs").join("remotes").join("p4").exists()
}

/// Select the git onto remote: `--onto-remote`, then `arc.land.onto-remote`
/// configuration, then symbol upstreams, then `origin`
/// (`ArcanistGitLandEngine::selectOntoRemote()`/`newOntoRemote()`,
/// `:1140-1264`).
///
/// Returns the remote; the `ONTO REMOTE` status line is pushed. A
/// Perforce remote selects Git/Perforce mode with the `P4 MODE` warning
/// and requires the squash strategy (`:1157-1173`).
pub fn git_select_onto_remote(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    symbols: &[String],
) -> Result<String, String> {
    let remote = git_new_onto_remote(dir, io, options, symbols)?;
    let pushable = git::remote_push_uri(dir, &remote).is_some();
    let perforce = git_is_perforce_remote(dir, &remote);
    if !pushable && !perforce {
        return Err(format!(
            "No pushable remote \"{remote}\" exists. Use the \"--onto-remote\" flag to choose a \
             valid, pushable remote to land changes onto."
        ));
    }
    if perforce {
        io.push_status_with_ansi(
            "P4 MODE",
            "Operating in Git/Perforce mode after selecting a Perforce remote.",
            "yellow",
            io.ansi,
        );
        let squash = options
            .strategy
            .as_deref()
            .is_none_or(|name| name == "squash");
        if !squash {
            return Err(
                "Perforce mode does not support the \"merge\" land strategy. Use the \
                        \"squash\" land strategy when landing to a Perforce remote (you can use \
                        \"--squash\" to select this strategy)."
                    .to_string(),
            );
        }
    }
    Ok(remote)
}

/// Resolve the git onto remote name without validating it (the
/// `newOntoRemote()` half of selection).
fn git_new_onto_remote(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    symbols: &[String],
) -> Result<String, String> {
    if let Some(remote) = options.onto_remote.as_deref() {
        io.push_status(
            "ONTO REMOTE",
            &format!("Remote \"{remote}\" was selected with the \"--onto-remote\" flag."),
        );
        return Ok(remote.to_string());
    }
    if let Some(remote) = land_config_value(options, dir, "arc.land.onto-remote")?
        .as_ref()
        .and_then(parse_string_config)
    {
        io.push_status(
            "ONTO REMOTE",
            &format!(
                "Remote \"{remote}\" was selected by reading \"arc.land.onto-remote\" \
                 configuration."
            ),
        );
        return Ok(remote.to_string());
    }
    let mut upstream_remotes: Vec<String> = Vec::new();
    for symbol in symbols {
        let path = git::get_path_to_upstream(dir, symbol)?;
        if let Some(remote) = path.get_remote_remote_name()
            && !upstream_remotes.contains(&remote.to_string())
        {
            upstream_remotes.push(remote.to_string());
        }
    }
    if upstream_remotes.len() > 1 {
        return Err(
            "The \"onto\" refs you have selected are connected to multiple different \
             remotes via Git branch upstreams. Use \"--onto-remote\" to select a single remote."
                .to_string(),
        );
    }
    if let Some(remote) = upstream_remotes.into_iter().next() {
        io.push_status(
            "ONTO REMOTE",
            &format!(
                "Remote \"{remote}\" was selected by following tracking branches upstream to the \
                 closest remote."
            ),
        );
        return Ok(remote);
    }
    // NOTE: PHP checks `isPerforceRemote($remote)` here (`:1241-1253`),
    // but `$remote` is the leftover loop variable (always null at this
    // point: any non-null remote would have returned above), so the
    // check never fires and selection falls through to `origin`. The
    // port follows the effective behavior; explicit `--onto-remote p4`
    // still selects Perforce mode in `git_select_onto_remote()`.
    io.push_status(
        "ONTO REMOTE",
        "Landing onto remote \"origin\", the default remote under Git.",
    );
    Ok("origin".to_string())
}

/// Select the git onto refs: `--onto`, then `arc.land.onto`
/// configuration, then symbol upstreams, then `master`
/// (`ArcanistGitLandEngine::selectOntoRefs()`, `:1035-1138`).
///
/// Returns the refs; the `ONTO TARGET` status line (or cycle warnings) is
/// pushed.
pub fn git_select_onto_refs(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    symbols: &[String],
) -> Result<Vec<String>, String> {
    if !options.onto.is_empty() {
        io.push_status(
            "ONTO TARGET",
            &format!(
                "Refs were selected with the \"--onto\" flag: {}.",
                options.onto.join(", ")
            ),
        );
        return Ok(options.onto.clone());
    }
    if let Some(value) = land_config_value(options, dir, "arc.land.onto")? {
        let onto = parse_onto_config(&value)?;
        if !onto.is_empty() {
            io.push_status(
                "ONTO TARGET",
                &format!(
                    "Refs were selected by reading \"arc.land.onto\" configuration: {}.",
                    onto.join(", ")
                ),
            );
            return Ok(onto);
        }
    }
    let mut remote_onto: Vec<String> = Vec::new();
    for symbol in symbols {
        let path = git::get_path_to_upstream(dir, symbol)?;
        if path.get_length() == 0 {
            continue;
        }
        if let Some(cycle) = path.get_cycle() {
            io.push_status_with_ansi(
                "LOCAL CYCLE",
                &format!(
                    "Local branch \"{symbol}\" tracks an upstream, but following it leads to a \
                     local cycle; ignoring branch upstream."
                ),
                "yellow",
                io.ansi,
            );
            io.push_status_with_ansi("LOCAL CYCLE", &cycle.join(" -> "), "yellow", io.ansi);
            continue;
        }
        if !path.is_connected_to_remote() {
            io.push_status_with_ansi(
                "NO PATH TO REMOTE",
                &format!(
                    "Local branch \"{symbol}\" tracks an upstream, but there is no path to a \
                     remote; ignoring branch upstream."
                ),
                "yellow",
                io.ansi,
            );
            continue;
        }
        if let Some(onto) = path.get_remote_branch_name()
            && !remote_onto.contains(&onto.to_string())
        {
            remote_onto.push(onto.to_string());
        }
    }
    if remote_onto.len() > 1 {
        return Err(
            "The branches you are landing are connected to multiple different remote \
             branches via Git branch upstreams. Use \"--onto\" to select the refs you want to \
             push to."
                .to_string(),
        );
    }
    if let Some(onto) = remote_onto.into_iter().next() {
        io.push_status(
            "ONTO TARGET",
            &format!(
                "Landing onto target \"{onto}\", selected by following tracking branches \
                 upstream to the closest remote branch."
            ),
        );
        return Ok(vec![onto]);
    }
    io.push_status(
        "ONTO TARGET",
        "Landing onto target \"master\", the default target under Git.",
    );
    Ok(vec!["master".to_string()])
}

/// Parse `git ls-remote --heads REMOTE` output into branch names.
pub fn parse_ls_remote_heads(output: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in output.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((_, name)) = line.split_once('\t')
            && let Some(branch) = name.strip_prefix("refs/heads/")
        {
            names.push(branch.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Confirm the git onto refs, prompting to create missing remote branches
/// (`ArcanistGitLandEngine::confirmOntoRefs()`, `:965-1033`).
///
/// Empty refs fail; refs missing from the remote print the
/// `CREATE N BRANCHE(S)` banner (spelling verbatim) and prompt
/// `arc.land.create`. With `--hold`, the hold note prints first.
pub fn git_confirm_onto_refs(
    dir: &Path,
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    hold: bool,
) -> Result<(), String> {
    for onto_ref in onto_refs {
        if onto_ref.is_empty() {
            return Err(format!(
                "Selected \"onto\" ref \"{onto_ref}\" is invalid: the empty string is not a valid \
                 ref."
            ));
        }
    }
    let output = git::output(dir, &["ls-remote", "--heads", "--", onto_remote])
        .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
        .unwrap_or_default();
    let remote_branches = parse_ls_remote_heads(&output);
    let new_markers: Vec<&String> = onto_refs
        .iter()
        .filter(|name| !remote_branches.contains(name))
        .collect();
    if new_markers.is_empty() {
        return Ok(());
    }
    let count = new_markers.len();
    push_banner(
        io,
        &format!("CREATE {count} BRANCHE(S)"),
        &format!(
            "These {count} symbol(s) do not exist in the remote. They will be created as new \
             branches:"
        ),
    );
    for name in &new_markers {
        io.push_out(
            &marker_ref_view(&format!("Branch \"{name}\""), "", "").render_with_ansi(io.ansi),
        );
    }
    io.push_out("\n");
    if hold {
        io.push_out(&render_hint_with_ansi(
            &format!(
                "You are using \"--hold\", so execution will stop before the {count} branche(s) \
                 are actually created. You will be given instructions to create the branches."
            ),
            io.ansi,
        ));
        io.push_out("\n");
    }
    let query = format!("Create {count} new branche(s) in the remote?");
    prompt_for_key(io, land_checks::PROMPT_CREATE, &query)
}

/// Selected into state: empty, local, or a remote ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntoState {
    /// Merge into the empty state (`--into-empty` or implied).
    Empty,
    /// Merge into a local ref without fetching (`--into-local`).
    Local(String),
    /// Merge into a remote ref after fetching.
    Remote {
        /// Remote name.
        remote: String,
        /// Ref name.
        name: String,
    },
}

/// Select the into remote and ref
/// (`ArcanistGitLandEngine::selectIntoRemote()`/`selectIntoRef()`,
/// `:1266-1375`; the Mercurial twins print identical lines,
/// `ArcanistMercurialLandEngine.php:385-495`).
///
/// Pushes the `INTO REMOTE` and `INTO TARGET` status lines. `fetchable`
/// validates `--into-remote` values per VCS.
pub fn select_into_state(
    io: &mut Io,
    options: &Options,
    onto_remote: &str,
    onto_refs: &[String],
    fetchable: &dyn Fn(&str) -> bool,
) -> Result<IntoState, String> {
    if options.into_empty {
        io.push_status(
            "INTO REMOTE",
            "Will merge into empty state, selected with the \"--into-empty\" flag.",
        );
        io.push_status(
            "INTO TARGET",
            "Will merge into empty state, selected with the \"--into-empty\" flag.",
        );
        return Ok(IntoState::Empty);
    }
    if options.into_local {
        io.push_status(
            "INTO REMOTE",
            "Will merge into local state, selected with the \"--into-local\" flag.",
        );
    } else if let Some(into) = options.into_remote.as_deref() {
        if !fetchable(into) {
            return Err(format!(
                "Remote \"{into}\", specified with \"--into\", is not a valid fetchable remote."
            ));
        }
        io.push_status(
            "INTO REMOTE",
            &format!("Will merge into remote \"{into}\", selected with the \"--into\" flag."),
        );
    } else {
        io.push_status(
            "INTO REMOTE",
            &format!(
                "Will merge into remote \"{onto_remote}\" by default, because this is the remote \
                 the change is landing onto."
            ),
        );
    }
    if let Some(into) = options.into.as_deref() {
        io.push_status(
            "INTO TARGET",
            &format!("Will merge into target \"{into}\", selected with the \"--into\" flag."),
        );
        if options.into_local {
            return Ok(IntoState::Local(into.to_string()));
        }
        let remote = options
            .into_remote
            .clone()
            .unwrap_or_else(|| onto_remote.to_string());
        return Ok(IntoState::Remote {
            remote,
            name: into.to_string(),
        });
    }
    let onto = onto_refs
        .first()
        .cloned()
        .unwrap_or_else(|| "master".to_string());
    if onto_refs.len() > 1 {
        io.push_status(
            "INTO TARGET",
            &format!(
                "Will merge into target \"{onto}\" by default, because this is the first \
                 \"onto\" target."
            ),
        );
    } else {
        io.push_status(
            "INTO TARGET",
            &format!(
                "Will merge into target \"{onto}\" by default, because this is the \"onto\" \
                 target."
            ),
        );
    }
    if options.into_local {
        return Ok(IntoState::Local(onto));
    }
    let remote = options
        .into_remote
        .clone()
        .unwrap_or_else(|| onto_remote.to_string());
    Ok(IntoState::Remote { remote, name: onto })
}

/// Resolve the git into state to a commit, fetching when remote
/// (`ArcanistGitLandEngine::selectIntoCommit()`, `:1377-1462`).
///
/// Returns `Ok(None)` for the empty state (explicit or implied when the
/// remote ref does not exist and `--into` was not given). Pushes the
/// `INTO COMMIT` status line.
pub fn git_select_into_commit(
    dir: &Path,
    io: &mut Io,
    state: &IntoState,
    into_argument: Option<&str>,
) -> Result<Option<String>, String> {
    match state {
        IntoState::Empty => {
            io.push_status("INTO COMMIT", "Preparing merge into the empty state.");
            Ok(None)
        }
        IntoState::Local(local_ref) => {
            let output = git::output(dir, &["rev-parse", "--verify", local_ref])?;
            if !output.status.success() {
                return Err(format!("Local ref \"{local_ref}\" does not exist."));
            }
            let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
            io.push_status(
                "INTO COMMIT",
                &format!(
                    "Preparing merge into local target \"{local_ref}\", at commit \"{}\".",
                    git::display_hash(&commit)
                ),
            );
            Ok(Some(commit))
        }
        IntoState::Remote { remote, name } => {
            if let Some(commit) = git_fetch_target(dir, io, remote, name)? {
                io.push_status(
                    "INTO COMMIT",
                    &format!(
                        "Preparing merge into \"{name}\" from remote \"{remote}\", at commit \
                         \"{}\".",
                        git::display_hash(&commit)
                    ),
                );
                return Ok(Some(commit));
            }
            if into_argument.is_some() {
                return Err(format!(
                    "Ref \"{name}\" does not exist in remote \"{remote}\"."
                ));
            }
            io.push_status(
                "INTO COMMIT",
                &format!(
                    "Preparing merge into the empty state to create target \"{name}\" in remote \
                     \"{remote}\"."
                ),
            );
            Ok(None)
        }
    }
}

/// Fetch a git land target, returning its remote commit
/// (`ArcanistGitLandEngine::fetchTarget()`, `:183-248`).
///
/// In Perforce mode the target always syncs from Perforce (`:192-209`).
/// Otherwise, when no local copy exists, the first fetch ignores
/// failures; a still missing ref resolves to `None` (the caller implies
/// `--into-empty`). Returns the `refs/remotes/REMOTE/REF` commit
/// otherwise.
fn git_fetch_target(
    dir: &Path,
    io: &mut Io,
    remote: &str,
    name: &str,
) -> Result<Option<String>, String> {
    if git_is_perforce_remote(dir, remote) {
        io.push_status(
            "P4 SYNC",
            &format!("Synchronizing \"{name}\" from Perforce..."),
        );
        git_p4_sync(dir, remote, name)?;
        return Ok(Some(git_remote_commit(
            dir,
            &format!("refs/remotes/{remote}/{name}"),
        )));
    }
    let full_ref = format!("refs/remotes/{remote}/{name}");
    if git::output(dir, &["rev-parse", "--verify", &full_ref])
        .is_ok_and(|out| !out.status.success())
    {
        io.push_status_with_ansi(
            "TARGET",
            &format!(
                "No local copy of ref \"{name}\" in remote \"{remote}\" exists, attempting \
                 fetch..."
            ),
            "yellow",
            io.ansi,
        );
        git_fetch_quiet(dir, remote, name);
        if git::output(dir, &["rev-parse", "--verify", &full_ref])
            .is_ok_and(|out| !out.status.success())
        {
            return Ok(None);
        }
        io.push_status(
            "FETCHED",
            &format!("Fetched ref \"{name}\" from remote \"{remote}\"."),
        );
        return Ok(Some(git_remote_commit(dir, &full_ref)));
    }
    io.push_status(
        "FETCH",
        &format!("Fetching \"{name}\" from remote \"{remote}\"..."),
    );
    if !git_fetch_quiet(dir, remote, name) {
        return Err(format!(
            "Fetch of \"{name}\" from remote \"{remote}\" failed! Fix the error and run \"arc \
             land\" again."
        ));
    }
    Ok(Some(git_remote_commit(dir, &full_ref)))
}

/// Run `git fetch --no-tags --quiet -- REMOTE REF`, returning success
/// (`ArcanistGitLandEngine::fetchLandTarget()`, `:1510-1539`).
fn git_fetch_quiet(dir: &Path, remote: &str, name: &str) -> bool {
    git_passthru_with(
        dir,
        "git",
        &[],
        &["fetch", "--no-tags", "--quiet", "--", remote, name],
    )
}

/// Read a fetched remote-tracking commit (which must exist by now).
fn git_remote_commit(dir: &Path, full_ref: &str) -> String {
    git::output(dir, &["rev-parse", "--verify", full_ref])
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Collect the commits landing from `symbols` against `into_commit`
/// (`ArcanistGitLandEngine::selectCommits()`, `:1541-1607`).
///
/// Runs `git log --format=%H%x00%P%x00%s%x00 SYMBOL [--not INTO] --` per
/// symbol and merges the newest-first lists.
pub fn git_select_commits(
    dir: &Path,
    symbols: &[GitSymbol],
    into_commit: Option<&str>,
) -> Result<Vec<LandCommit>, String> {
    let mut commits = Vec::new();
    for symbol in symbols {
        let format = "--format=%H%x00%P%x00%s%x00";
        let output = match into_commit {
            None => git::stdout_text(dir, &["log", format, &symbol.commit, "--"])?,
            Some(into) => {
                git::stdout_text(dir, &["log", format, &symbol.commit, "--not", into, "--"])?
            }
        };
        collect_git_commits(&mut commits, &output, &symbol.symbol)?;
    }
    Ok(commits)
}

/// One resolved source symbol: the raw text plus its commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSymbol {
    /// Raw symbol text (`arc land <symbol>` or the default).
    pub symbol: String,
    /// Resolved commit hash.
    pub commit: String,
}

/// Read a commit's author (`Name <email>`) and date
/// (`ArcanistGitLandEngine::getAuthorAndDate()`, `:871-886`).
pub fn git_author_and_date(dir: &Path, commit: &str) -> Result<(String, String), String> {
    let info = git::stdout_text(dir, &["log", "-n1", "--format=%aD%n%an%n%ae", commit, "--"])?;
    let mut lines = info.split('\n');
    match (lines.next(), lines.next(), lines.next()) {
        (Some(date), Some(author), Some(email)) => {
            Ok((format!("{author} <{email}>"), date.to_string()))
        }
        _ => Err(format!("could not read author and date of \"{commit}\"")),
    }
}

/// Rebase `min..max` onto `dst`, returning the new HEAD
/// (`ArcanistGitLandEngine::executeMerge()`, `:326-337`).
///
/// Checks `max` out first, then `rebase --onto`. Failures abort the
/// rebase, reset hard, and propagate.
fn git_apply_rebase(dir: &Path, min: &str, max: &str, dst: &str) -> Result<String, String> {
    git::stdout_text(dir, &["checkout", max, "--"])?;
    let base = format!("{min}^");
    let args = ["rebase", "--onto", dst, "--", base.as_str()];
    if git::output(dir, &args).is_ok_and(|out| !out.status.success()) {
        git::output(dir, &["rebase", "--abort"]).ok();
        git::output(dir, &["reset", "--hard", "HEAD", "--"]).ok();
        return Err("rebase failed".to_string());
    }
    git::stdout_text(dir, &["rev-parse", "--verify", "HEAD"])
}

/// Merge `src` into `dst`, leaving the result uncommitted
/// (`ArcanistGitLandEngine::executeMerge()`, `:299-345`).
///
/// Squash merges pass `--ff --squash` (the explicit `--ff` overrides user
/// `merge.ff` configuration); non-squash merges pass `--no-ff`. Empty
/// merges add `--allow-unrelated-histories`. Failures abort, reset, and
/// propagate.
fn git_apply_merge(
    dir: &Path,
    src: &str,
    dst: &str,
    squash: bool,
    is_empty: bool,
) -> Result<(), String> {
    git::stdout_text(dir, &["checkout", dst, "--"])?;
    let mut args = vec!["merge", "--no-stat", "--no-commit"];
    if is_empty {
        args.push("--allow-unrelated-histories");
    }
    if squash {
        args.extend(["--ff", "--squash"]);
    } else {
        args.push("--no-ff");
    }
    args.extend(["--", src]);
    if git::output(dir, &args).is_ok_and(|out| !out.status.success()) {
        git::output(dir, &["merge", "--abort"]).ok();
        git::output(dir, &["reset", "--hard", "HEAD", "--"]).ok();
        return Err("merge failed".to_string());
    }
    Ok(())
}

/// Commit the index with `message`, returning the new HEAD
/// (`ArcanistGitLandEngine::executeMerge() commit step`, `:403-415`).
///
/// The message arrives via stdin (`-F -`), like PHP's future write.
fn git_apply_commit(
    dir: &Path,
    message: &str,
    author: Option<&str>,
    date: Option<&str>,
    allow_empty: bool,
) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut args = vec!["commit"];
    if let Some(author) = author {
        args.extend(["--author", author]);
    }
    if let Some(date) = date {
        args.extend(["--date", date]);
    }
    if allow_empty {
        args.push("--allow-empty");
    }
    args.extend(["-F", "-", "--"]);
    let mut child = std::process::Command::new("git")
        .current_dir(dir)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(detail.trim().to_string());
    }
    git::stdout_text(dir, &["rev-parse", "--verify", "HEAD"])
}

/// Merge one commit set into `into_commit`
/// (`ArcanistGitLandEngine::executeMerge()`, `:250-440`).
///
/// Returns the new cursor. Squash merges rebase the range onto the into
/// state first (nothing else: the post-T13576 reduce fallback does not
/// exist at ceb); anything else merges the max commit directly. Empty
/// diffs fail (usually: already landed); a failed merge prints the `MERGE
/// CONFLICT` banner and fails with `Encountered a merge conflict.`.
/// `message_of` provides the landed commit message for the set.
/// `has_unpushed` (previously merged but unpushed sets exist) selects the
/// `--incremental` one-by-one hint (`:379-385`).
#[allow(clippy::too_many_arguments)]
pub fn git_execute_merge(
    dir: &Path,
    io: &mut Io,
    set: &CommitSet,
    commits: &[LandCommit],
    into_commit: Option<&str>,
    squash: bool,
    has_unpushed: bool,
    message_of: &dyn Fn(&CommitSet) -> Result<String, String>,
) -> Result<String, String> {
    let min_hash = commits[set.commits[0]].hash.clone();
    let max_hash = commits[set.commits[set.commits.len() - 1]].hash.clone();
    let max_summary = display_summary(&commits[set.commits[set.commits.len() - 1]].summary);
    let is_empty = into_commit.is_none();
    // Merge onto a scratch empty commit when landing into the empty
    // state (`:257-261`): an empty tree with no author, committer, or
    // message, written with `hash-object` (no git identity needed).
    let into_commit = match into_commit {
        Some(commit) => commit.to_string(),
        None => git_write_raw_commit(dir, &RawCommit::empty())?,
    };
    let changes = git::stdout_text(
        dir,
        &[
            "diff",
            "--no-ext-diff",
            &format!("{into_commit}..{max_hash}"),
            "--",
        ],
    )?;
    if changes.trim().is_empty() {
        return Err(format!(
            "Merging local \"{}\" into \"{}\" produces an empty diff. This usually means these \
             changes have already landed.",
            git::display_hash(&max_hash),
            git::display_hash(&into_commit)
        ));
    }
    io.push_status(
        "MERGING",
        &format!("{} {max_summary}", git::display_hash(&max_hash)),
    );
    // Squash (non-empty) rebases the range onto the into state, then
    // merges; anything else merges the max commit directly (`:322-345`).
    let merged = if squash && !is_empty {
        git_apply_rebase(dir, &min_hash, &max_hash, &into_commit).and_then(|merged| {
            git_apply_merge(dir, &merged, &into_commit, squash, is_empty).map(|()| merged)
        })
    } else {
        git_apply_merge(dir, &max_hash, &into_commit, squash, is_empty).map(|()| max_hash.clone())
    };
    let merged_hash = match merged {
        Ok(merged) => merged,
        Err(_) => {
            return Err(git_merge_conflict(
                io,
                commits,
                set,
                &max_hash,
                &into_commit,
                has_unpushed,
            ));
        }
    };
    let (author, date) = git_author_and_date(dir, &max_hash)?;
    let message = message_of(set)?;
    let mut cursor = git_apply_commit(dir, &message, Some(&author), Some(&date), false)?;
    if is_empty {
        // Drop the scratch empty parent (T12876, `:417-437`): re-read
        // the raw commit, replace its parents (none under squash, the
        // merged hash under merge), and write it back, preserving the
        // tree, author, committer, and message bytes.
        let mut raw = git_read_raw_commit(dir, &cursor)?;
        raw.parents = if squash {
            Vec::new()
        } else {
            vec![merged_hash]
        };
        cursor = git_write_raw_commit(dir, &raw)?;
        git::stdout_text(dir, &["checkout", &cursor, "--"])?;
    }
    Ok(cursor)
}

/// A parsed git commit object (`ArcanistGitRawCommit`,
/// `src/repository/raw/ArcanistGitRawCommit.php`), with the raw author,
/// committer, and message preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RawCommit {
    /// `tree` line value, if present.
    tree: Option<String>,
    /// `parent` line values, in order.
    parents: Vec<String>,
    /// `author` line value, if present.
    author: Option<String>,
    /// `committer` line value, if present.
    committer: Option<String>,
    /// Message after the blank divider line, if any.
    message: Option<String>,
}

impl RawCommit {
    /// The scratch into-empty commit: the empty tree with no parents,
    /// author, committer, or message (`newEmptyCommit()`).
    fn empty() -> Self {
        Self {
            tree: Some(git::GIT_MAGIC_ROOT_COMMIT.to_string()),
            ..Self::default()
        }
    }
}

/// Parse a raw commit blob (`newFromRawBlob()`, `:21-94`): `tree`,
/// `parent`, `author`, and `committer` header lines, a blank divider,
/// then the verbatim message. Repeated non-parent labels, unknown
/// labels, and malformed lines throw with the verbatim errors.
fn parse_raw_commit_blob(blob: &str) -> Result<RawCommit, String> {
    const PATTERN: &str = "(^(\\w+) ([^\\n]+)\\n?\\z)";
    let mut raw = RawCommit::default();
    let mut seen: HashMap<String, String> = HashMap::new();
    let items: Vec<&str> = blob.split_inclusive('\n').collect();
    let mut index = 0;
    while index < items.len() {
        let line = items[index];
        index += 1;
        if line == "\n" {
            break;
        }
        let stripped = line.strip_suffix('\n').unwrap_or(line);
        let Some((label, value)) = split_header_line(stripped) else {
            return Err(format!(
                "Expected to match pattern \"{PATTERN}\" against line \"{stripped}\" in raw \
                 commit blob: {blob}"
            ));
        };
        if let Some(first) = seen.get(label) {
            if label != "parent" {
                return Err(format!(
                    "Encountered two \"{label}\" lines (\"{first}\", \"{stripped}\") while \
                     parsing raw commit blob, expected at most one: {blob}"
                ));
            }
        } else {
            seen.insert(label.to_string(), stripped.to_string());
        }
        match label {
            "tree" => raw.tree = Some(value.to_string()),
            "parent" => raw.parents.push(value.to_string()),
            "author" => raw.author = Some(value.to_string()),
            "committer" => raw.committer = Some(value.to_string()),
            _ => {
                return Err(format!(
                    "Unknown attribute label \"{label}\" in line \"{stripped}\" while parsing \
                     raw commit blob: {blob}"
                ));
            }
        }
    }
    let message: String = items[index..].concat();
    if !message.is_empty() {
        raw.message = Some(message);
    }
    Ok(raw)
}

/// Split one `label value` header line (`(\w+) ([^\n]+)`, anchored).
fn split_header_line(line: &str) -> Option<(&str, &str)> {
    let (label, value) = line.split_once(' ')?;
    if label.is_empty()
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || value.is_empty()
    {
        return None;
    }
    Some((label, value))
}

/// Render a raw commit blob (`getRawBlob()`, `:102-130`): the tree line,
/// parent lines, author and committer lines, a blank divider, then the
/// message. Absent fields emit nothing.
fn render_raw_commit_blob(raw: &RawCommit) -> String {
    let mut out = String::new();
    if let Some(tree) = &raw.tree {
        out.push_str(&format!("tree {tree}\n"));
    }
    for parent in &raw.parents {
        out.push_str(&format!("parent {parent}\n"));
    }
    if let Some(author) = &raw.author {
        out.push_str(&format!("author {author}\n"));
    }
    if let Some(committer) = &raw.committer {
        out.push_str(&format!("committer {committer}\n"));
    }
    out.push('\n');
    if let Some(message) = &raw.message {
        out.push_str(message);
    }
    out
}

/// Read one raw commit (`ArcanistGitAPI::readRawCommit()`,
/// `src/repository/api/ArcanistGitAPI.php:1738-1744`).
fn git_read_raw_commit(dir: &Path, hash: &str) -> Result<RawCommit, String> {
    let output = git::output(dir, &["cat-file", "commit", "--", hash])
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let trimmed = detail.trim();
        if trimmed.is_empty() {
            return Err(format!("git cat-file commit {hash} failed"));
        }
        return Err(trimmed.to_string());
    }
    // The blob is used verbatim (no trimming): the message keeps its
    // exact bytes.
    parse_raw_commit_blob(&String::from_utf8_lossy(&output.stdout))
}

/// Write one raw commit, returning its hash
/// (`ArcanistGitAPI::writeRawCommit()`,
/// `src/repository/api/ArcanistGitAPI.php:1746-1754`).
///
/// `--literally` bypasses modern git's write-time fsck, which refuses
/// PHP's authorless scratch commit; the stored object bytes are
/// identical to PHP's write.
fn git_write_raw_commit(dir: &Path, raw: &RawCommit) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let blob = render_raw_commit_blob(raw);
    let mut child = std::process::Command::new("git")
        .current_dir(dir)
        .args([
            "hash-object",
            "-t",
            "commit",
            "--literally",
            "--stdin",
            "-w",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| "failed to write raw commit".to_string())?
        .write_all(blob.as_bytes())
        .map_err(|e| format!("failed to write raw commit: {e}"))?;
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        return Err("git hash-object failed".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Print the `MERGE CONFLICT` banner and return its error
/// (`ArcanistGitLandEngine.php:347-401`).
///
/// The `--incremental` hint shows only when previously merged sets are
/// still unpushed (`getHasUnpushedChanges()`, `:379-385`).
fn git_merge_conflict(
    io: &mut Io,
    commits: &[LandCommit],
    set: &CommitSet,
    max_hash: &str,
    into_commit: &str,
    has_unpushed: bool,
) -> String {
    let max = &commits[set.commits[set.commits.len() - 1]];
    let short_max = git::display_hash(max_hash);
    let short_into = git::display_hash(into_commit);
    let message = if max.direct_symbols.is_empty() && max.indirect_symbols.is_empty() {
        format!(
            "Local commit \"{short_max}\" does not merge cleanly into \"{short_into}\". Rebase or \
             merge local changes so they can merge cleanly."
        )
    } else if !max.direct_symbols.is_empty() {
        format!(
            "Local commit \"{short_max}\" ({}) does not merge cleanly into \"{short_into}\". \
             Rebase or merge local changes so they can merge cleanly.",
            display_symbols(&max.direct_symbols)
        )
    } else {
        format!(
            "Local commit \"{short_max}\" (reachable from: {}) does not merge cleanly into \
             \"{short_into}\". Rebase or merge local changes so they can merge cleanly.",
            display_symbols(&max.indirect_symbols)
        )
    };
    push_banner(io, "MERGE CONFLICT", &message);
    if has_unpushed {
        io.push_out(&render_hint_with_ansi(
            "Use \"--incremental\" to merge and push changes one by one.",
            io.ansi,
        ));
        io.push_out("\n\n");
    }
    "Encountered a merge conflict.".to_string()
}

/// Build push refspecs (`ArcanistGitLandEngine::newOntoRefArguments()`,
/// `:1635-1647`): `SHORT:refs/heads/ONTO` per onto ref.
pub fn git_onto_refspecs(onto_refs: &[String], into_commit: &str) -> Vec<String> {
    onto_refs
        .iter()
        .map(|onto| format!("{}:refs/heads/{onto}", git::display_hash(into_commit)))
        .collect()
}

/// Push the landed cursor onto every onto ref
/// (`ArcanistGitLandEngine::pushChange()`, `:595-609`).
pub fn git_push_change(
    dir: &Path,
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: &str,
) -> Result<(), String> {
    git_push_change_with(dir, "git", &[], io, onto_remote, onto_refs, into_commit)
}

/// [`git_push_change`] with a program override (test seam).
///
/// The push inherits stdio (`newPassthru()`, `:494-501`): it may
/// prompt for credentials, and progress belongs on the terminal.
pub fn git_push_change_with(
    dir: &Path,
    program: &str,
    prefix: &[String],
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: &str,
) -> Result<(), String> {
    io.push_status("PUSHING", &format!("Pushing changes to \"{onto_remote}\"."));
    let refspecs = git_onto_refspecs(onto_refs, into_commit);
    let mut args = vec!["push", "--", onto_remote];
    args.extend(refspecs.iter().map(String::as_str));
    if !git_passthru_with(dir, program, prefix, &args) {
        return Err("Push failed! Fix the error and run \"arc land\" again.".to_string());
    }
    Ok(())
}

/// Run `git` inheriting stdio, returning success (`newPassthru()`).
///
/// Fetch and push must not capture output: they may prompt for
/// credentials, and progress belongs on the terminal
/// (`ArcanistGitLandEngine.php:186-190`).
fn git_passthru_with(dir: &Path, program: &str, prefix: &[String], args: &[&str]) -> bool {
    let mut command = std::process::Command::new(program);
    command.current_dir(dir).args(prefix).args(args);
    command.status().is_ok_and(|status| status.success())
}

/// `git p4 submit` argv for Perforce mode (`pushChange()`, `:551-585`):
/// skip the submit editor and its confirmation, disable the implicit
/// rebase, detect moves, and quit on conflict.
pub fn git_p4_submit_argv(into_commit: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        "git-p4.skipSubmitEdit=true".to_string(),
        "-c".to_string(),
        "git-p4.skipSubmitEditCheck=true".to_string(),
        "p4".to_string(),
        "submit".to_string(),
        "--disable-rebase".to_string(),
        "-M".to_string(),
        "--conflict=quit".to_string(),
        "--commit".to_string(),
        into_commit.to_string(),
        "--".to_string(),
    ]
}

/// Submit one landed commit in Perforce mode (`pushChange()`, `:540-593`).
pub fn git_p4_submit(
    dir: &Path,
    io: &mut Io,
    onto_remote: &str,
    into_commit: &str,
) -> Result<(), String> {
    git_p4_submit_with(dir, "git", &[], io, onto_remote, into_commit)
}

/// [`git_p4_submit`] with the `git` invocation overridden by `program`
/// plus `prefix` (test seam). Like PHP's passthru, `git` inherits
/// stdio; any failure maps to the verbatim submit error.
pub fn git_p4_submit_with(
    dir: &Path,
    program: &str,
    prefix: &[String],
    io: &mut Io,
    onto_remote: &str,
    into_commit: &str,
) -> Result<(), String> {
    io.push_status(
        "SUBMITTING",
        &format!("Submitting changes to \"{onto_remote}\"."),
    );
    let argv = git_p4_submit_argv(into_commit);
    let mut command = std::process::Command::new(program);
    command.current_dir(dir).args(prefix).args(&argv);
    let ok = command.status().is_ok_and(|status| status.success());
    if ok {
        Ok(())
    } else {
        Err("Submit failed! Fix the error and run \"arc land\" again.".to_string())
    }
}

/// Synchronize one target from Perforce (`fetchTarget()`, `:192-209`),
/// with the `git` invocation overridden (test seam; see
/// [`git_p4_submit_with`]).
pub fn git_p4_sync_with(
    dir: &Path,
    program: &str,
    prefix: &[String],
    remote: &str,
    name: &str,
) -> Result<(), String> {
    let mut command = std::process::Command::new(program);
    command.current_dir(dir).args(prefix).args([
        "p4",
        "sync",
        "--silent",
        "--branch",
        &format!("{remote}/{name}"),
        "--",
    ]);
    let ok = command.status().is_ok_and(|status| status.success());
    if ok {
        Ok(())
    } else {
        Err("Perforce sync failed! Fix the error and run \"arc land\" again.".to_string())
    }
}

/// Synchronize one target from Perforce (`fetchTarget()`, `:192-209`).
pub fn git_p4_sync(dir: &Path, remote: &str, name: &str) -> Result<(), String> {
    git_p4_sync_with(dir, "git", &[], remote, name)
}

/// Parse `git for-each-ref --format %(refname) %(objectname)` lines into
/// local branch names and hashes
/// (`ArcanistGitLandEngine::getBranchesForCommits()`, `:52-114`).
pub fn parse_for_each_ref(output: &str) -> Result<Vec<(String, String)>, String> {
    let mut branches = Vec::new();
    for line in output.split('\n') {
        if line.is_empty() {
            continue;
        }
        let Some((name, hash)) = line.split_once(' ') else {
            return Err(format!("Failed to explode line \"{line}\"."));
        };
        if hash.contains(' ') {
            return Err(format!("Failed to explode line \"{line}\"."));
        }
        let Some(branch) = name.strip_prefix("refs/heads/") else {
            continue;
        };
        branches.push((branch.to_string(), hash.to_string()));
    }
    // `natcasesort()` over the branch names (`:119-120`).
    branches.sort_by(|a, b| commitgraph::natural_cmp(a.0.as_bytes(), b.0.as_bytes(), true));
    Ok(branches)
}

/// List local branches pointing at (`contains` false) or containing
/// (`contains` true) `hash`.
fn git_branches_for_commit(dir: &Path, hash: &str, contains: bool) -> Vec<(String, String)> {
    let flag = if contains {
        "--contains"
    } else {
        "--points-at"
    };
    let output = git::output(
        dir,
        &[
            "for-each-ref",
            flag,
            hash,
            "--format",
            "%(refname) %(objectname)",
            "--",
        ],
    )
    .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
    .unwrap_or_default();
    parse_for_each_ref(&output).unwrap_or_default()
}

/// Delete local branches pointing exactly at landed commits
/// (`ArcanistGitLandEngine::pruneBranches()`, `:19-50`).
///
/// Returns the deleted branch names. Each deletion prints the `CLEANUP`
/// status plus the recovery command.
pub fn git_prune_branches(
    dir: &Path,
    io: &mut Io,
    sets: &[CommitSet],
    commits: &[LandCommit],
) -> Vec<String> {
    // Collect branch names for every set hash first, like PHP's one map
    // for all sets (`:31-40`), then sort once globally (`:42-49`).
    let mut collected: Vec<(String, String)> = Vec::new();
    for set in sets {
        let old = &commits[set.commits[set.commits.len() - 1]].hash;
        collected.extend(git_branches_for_commit(dir, old, false));
    }
    collected.sort_by(|a, b| commitgraph::natural_cmp(a.0.as_bytes(), b.0.as_bytes(), true));
    collected.dedup_by(|a, b| a.0 == b.0);
    let mut deleted = Vec::new();
    for (branch, hash) in collected {
        io.push_status(
            "CLEANUP",
            &format!("Cleaning up branch \"{branch}\". To recover, run:"),
        );
        let recover = format!("git checkout -b {branch} {}", git::display_hash(&hash));
        io.push_out(&format!("\n{}\n", shell_line(&recover, io.ansi)));
        git::output(dir, &["branch", "-D", "--", &branch]).ok();
        deleted.push(branch);
    }
    deleted
}

/// Rebase descendant branches onto the landed state (squash only)
/// (`ArcanistGitLandEngine::cascadeState()`, `:116-181`).
///
/// Branches pointing exactly at the old commit are skipped (pruned
/// later); `--pick` sets rebase onto the range start's parent instead of
/// the landed commit. Rebase failures abort, reset, and warn with
/// `REBASE CONFLICT`.
pub fn git_cascade_state(
    dir: &Path,
    io: &mut Io,
    set: &CommitSet,
    commits: &[LandCommit],
    new_commit: &str,
    squash: bool,
) {
    if !squash {
        return;
    }
    let min = &commits[set.commits[0]].hash;
    let old = &commits[set.commits[set.commits.len() - 1]].hash;
    for (branch, head) in git_branches_for_commit(dir, old, true) {
        if head == *old {
            continue;
        }
        io.push_status(
            "CASCADE",
            &format!("Rebasing \"{branch}\" onto landed state..."),
        );
        let target;
        let rebase_target = if set.is_pick {
            target = format!("{min}^");
            target.as_str()
        } else {
            new_commit
        };
        if git::output(
            dir,
            &["rebase", "--onto", rebase_target, "--", old, &branch],
        )
        .is_ok_and(|out| !out.status.success())
        {
            git::output(dir, &["rebase", "--abort"]).ok();
            git::output(dir, &["reset", "--hard", "HEAD", "--"]).ok();
            io.push_status_with_ansi(
                "REBASE CONFLICT",
                &format!(
                    "Branch \"{branch}\" does not rebase cleanly from \"{}\" onto \"{}\", \
                     skipping.",
                    git::display_hash(old),
                    git::display_hash(rebase_target)
                ),
                "yellow",
                io.ansi,
            );
        }
    }
}

/// Candidate branches for [`git_reconcile`] (`:638-677`): the original
/// ref, its saved upstream path, the into ref, and the onto refs, minus
/// deleted and missing branches, deduplicated like `array_fuse()`.
///
/// `into_ref` is `None` for the empty and local into states, which PHP
/// skips (`:652-654`); the option already encodes that.
fn git_reconcile_candidates(
    dir: &Path,
    state: &GitLocalState,
    into_ref: Option<&str>,
    onto_refs: &[String],
    deleted: &[String],
) -> Vec<String> {
    let mut update = Vec::new();
    if let Some(local_ref) = &state.local_ref {
        update.push(local_ref.clone());
        if let Some(path) = &state.local_path {
            update.extend(path.get_local_branches());
        }
    }
    update.extend(into_ref.map(str::to_string));
    update.extend(onto_refs.iter().cloned());
    let mut branches: Vec<String> = Vec::new();
    for branch in update {
        if !branches.contains(&branch) {
            branches.push(branch);
        }
    }
    branches.retain(|branch| !deleted.contains(branch));
    branches.retain(|branch| {
        git::output(dir, &["rev-parse", "--verify", branch]).is_ok_and(|out| out.status.success())
    });
    branches
}

/// Pull list for [`git_reconcile`] (`:732-776`): local branches of the
/// first candidate whose upstream path reaches the pushed remote and ref,
/// closest to the upstream first. Cyclic upstreams warn and are skipped.
fn git_reconcile_pull_branches(
    dir: &Path,
    io: &mut Io,
    update_branches: &[String],
    onto_remote: &str,
    onto_refs: &[String],
) -> Result<Vec<String>, String> {
    let mut pull_branches: Vec<String> = Vec::new();
    for branch in update_branches {
        let path = git::get_path_to_upstream(dir, branch)?;
        if path.get_cycle().is_some() {
            io.push_status_with_ansi(
                "LOCAL CYCLE",
                &format!(
                    "Local branch \"{branch}\" tracks an upstream but following it leads to a \
                     local cycle, ignoring branch."
                ),
                "yellow",
                io.ansi,
            );
            continue;
        }
        if !path.is_connected_to_remote() {
            continue;
        }
        if path.get_remote_remote_name() != Some(onto_remote) {
            continue;
        }
        if !onto_refs
            .iter()
            .any(|onto| Some(onto.as_str()) == path.get_remote_branch_name())
        {
            continue;
        }
        pull_branches = path.get_local_branches();
        break;
    }
    pull_branches.reverse();
    let mut fused: Vec<String> = Vec::new();
    for branch in pull_branches {
        if !fused.contains(&branch) {
            fused.push(branch);
        }
    }
    Ok(fused)
}

/// Reconcile after a land (`reconcileLocalState()`, `:611-677,731-852`,
/// non-Perforce half): update the local branches connected to the pushed
/// remote ref, check the last one out, then discard the saved state.
///
/// Each pull branch at-or-behind the landed commit is force-updated to
/// it. A surviving original ref outside the pull list is restored
/// instead (`RETURN`); with no pull list the checkout stays detached.
#[allow(clippy::too_many_arguments)]
pub fn git_reconcile(
    dir: &Path,
    io: &mut Io,
    state: &GitLocalState,
    into_commit: &str,
    into_ref: Option<&str>,
    onto_remote: &str,
    onto_refs: &[String],
    deleted: &[String],
) -> Result<(), String> {
    let update_branches = git_reconcile_candidates(dir, state, into_ref, onto_refs, deleted);
    let pull_branches =
        git_reconcile_pull_branches(dir, io, &update_branches, onto_remote, onto_refs)?;

    if let Some(local_ref) = &state.local_ref
        && update_branches.contains(local_ref)
        && !pull_branches.contains(local_ref)
    {
        io.push_status(
            "RETURN",
            &format!("Returning to original branch \"{local_ref}\" in original state."),
        );
        return restore_local_state_git(dir, io, state);
    }

    let mut dst_branch = None;
    for branch in &pull_branches {
        if !git_is_ancestor_of(dir, branch, into_commit)? {
            io.push_status(
                "LOCAL CHANGES",
                &format!("Local branch \"{branch}\" has unpublished changes, ending updates."),
            );
            break;
        }
        io.push_status("UPDATE", &format!("Updating local branch \"{branch}\"..."));
        git::stdout_text(dir, &["branch", "-f", branch, into_commit, "--"])?;
        dst_branch = Some(branch.clone());
    }

    if let Some(dst) = dst_branch {
        io.push_status("CHECKOUT", &format!("Checking out \"{dst}\"."));
        git::stdout_text(dir, &["checkout", &dst, "--"])?;
    } else {
        io.push_status(
            "DETACHED HEAD",
            "Unable to find any local branches to update, staying on detached head.",
        );
    }
    discard_local_state_git(dir, io, state)
}

/// True when `branch` is an ancestor of (or equal to) `commit`
/// (`ArcanistGitLandEngine::isAncestorOf()`, `:854-869`).
fn git_is_ancestor_of(dir: &Path, branch: &str, commit: &str) -> Result<bool, String> {
    let base = git::stdout_text(dir, &["merge-base", "--", branch, commit])?;
    let hash = git::stdout_text(dir, &["rev-parse", "--verify", branch])?;
    Ok(base == hash)
}

/// Reconcile after a Perforce-mode land (`reconcileLocalState()`,
/// `:679-730`): switch to the most desirable surviving branch (resetting
/// it when it holds no unpublished changes), then discard the saved
/// state.
#[allow(clippy::too_many_arguments)]
pub fn git_reconcile_perforce(
    dir: &Path,
    io: &mut Io,
    state: &GitLocalState,
    into_ref: Option<&str>,
    onto_refs: &[String],
    deleted: &[String],
    into_commit: &str,
) -> Result<(), String> {
    let mut update: Vec<String> = Vec::new();
    if let Some(local_ref) = &state.local_ref {
        update.push(local_ref.clone());
        if let Some(path) = &state.local_path {
            update.extend(path.get_local_branches());
        }
    }
    update.extend(into_ref.map(str::to_string));
    update.extend(onto_refs.iter().cloned());
    update.retain(|branch| !deleted.contains(branch));
    update.retain(|branch| {
        git::output(dir, &["rev-parse", "--verify", branch]).is_ok_and(|out| out.status.success())
    });
    // Deduplicate like `array_fuse()`, keeping first occurrences.
    let mut unique: Vec<String> = Vec::new();
    for branch in update {
        if !unique.contains(&branch) {
            unique.push(branch);
        }
    }
    let Some(dst) = unique.into_iter().next() else {
        io.push_status(
            "DETACHED HEAD",
            "Unable to find any local branches to update, staying on detached head.",
        );
        return discard_local_state_git(dir, io, state);
    };
    if !git_is_ancestor_of(dir, &dst, into_commit)? {
        io.push_status(
            "CHECKOUT",
            &format!(
                "Local branch \"{dst}\" has unpublished changes, checking it out but leaving them \
                 in place."
            ),
        );
        git::stdout_text(dir, &["checkout", &dst, "--"])?;
    } else {
        io.push_status("UPDATE", &format!("Switching to local branch \"{dst}\"."));
        git::stdout_text(dir, &["checkout", &dst, "--"])?;
        git::stdout_text(dir, &["reset", "--hard", into_commit, "--"])?;
    }
    discard_local_state_git(dir, io, state)
}

/// Print the hold instructions
/// (`ArcanistGitLandEngine::didHoldChanges()`, `:888-940`).
///
/// Shows the `HOLD CHANGES` banner, the manual push command, the restore
/// commands, and the branches-unchanged note. In Perforce mode (`:892-898`)
/// the changes "have not been submitted" and the manual command is
/// `git p4 submit`.
pub fn git_hold_text(
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: &str,
    restore: &[String],
    perforce: bool,
) {
    let message = if perforce {
        "Holding changes locally, they have not been submitted."
    } else {
        "Holding changes locally, they have not been pushed."
    };
    push_banner_mode(io, "HOLD CHANGES", message, false);
    let command = if perforce {
        format!("git p4 submit -M --commit {into_commit} --")
    } else {
        let refspecs = git_onto_refspecs(onto_refs, into_commit);
        format!("git push -- {onto_remote} {}", refspecs.join(" "))
    };
    io.push_out("To push changes manually, run this command:\n\n");
    io.push_out(&format!("{}\n", shell_line(&command, io.ansi)));
    if !restore.is_empty() {
        let count = restore.len().to_string();
        io.push_out(&terminal::console_format(
            "To go back to how things were before you ran \"arc land\", run these %s \
             command(s):\n\n",
            &[&count],
            io.ansi,
        ));
        for command in restore {
            io.push_out(&shell_line(command, io.ansi));
        }
        io.push_out("\n");
    }
    io.push_out(
        "Local branches have not been changed, and are still in the same state as before.\n",
    );
}

/// Parse `hg bookmarks` output into `(name, active)` pairs.
///
/// The active bookmark's line starts with ` * `; inactive lines start with
/// three spaces. Each line continues with the `REV:HASH` position.
pub fn parse_hg_bookmarks(output: &str) -> Vec<(String, bool)> {
    let mut bookmarks = Vec::new();
    for line in output.split('\n') {
        let (active, rest) = match line.strip_prefix(" * ") {
            Some(rest) => (true, rest),
            None => match line.strip_prefix("   ") {
                Some(rest) => (false, rest),
                None => continue,
            },
        };
        let name = rest.split_whitespace().next().unwrap_or("");
        if name.is_empty() {
            continue;
        }
        bookmarks.push((name.to_string(), active));
    }
    bookmarks
}

/// Parse `hg branches` output into branch names.
///
/// Plain `hg branches` lists `NAME REV:HASH` lines; the active branch is
/// read separately via `hg branch`.
pub fn parse_hg_branches(output: &str) -> Vec<String> {
    let mut branches = Vec::new();
    for line in output.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let name = line.split_whitespace().next().unwrap_or("");
        if !name.is_empty() {
            branches.push(name.to_string());
        }
    }
    branches
}

/// Default Mercurial source symbols: the active bookmark, else the active
/// branch, else the active commit
/// (`ArcanistMercurialLandEngine::getDefaultSymbols()`, `:11-75`).
pub fn hg_default_symbols(dir: &Path, io: &mut Io) -> Result<Vec<String>, String> {
    hg_default_symbols_with(dir, &hg::HgCommand::system(), io)
}

/// [`hg_default_symbols`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_default_symbols_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
) -> Result<Vec<String>, String> {
    let bookmarks = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    if let Some((bookmark, _)) = parse_hg_bookmarks(&bookmarks)
        .into_iter()
        .find(|(_, active)| *active)
    {
        io.push_status(
            "SOURCE",
            &format!("Landing the active bookmark, \"{bookmark}\"."),
        );
        return Ok(vec![bookmark]);
    }
    if let Ok(branch) = hg::stdout_text_with(dir, hg_cmd, &["branch"]) {
        let branch = branch.trim().to_string();
        if !branch.is_empty() {
            io.push_status(
                "SOURCE",
                &format!("Landing the active branch, \"{branch}\"."),
            );
            return Ok(vec![branch]);
        }
    }
    let commit = hg::stdout_text_with(
        dir,
        hg_cmd,
        &["log", "-l", "1", "--template", "{node}", "-r", "."],
    )?;
    let short = commit.chars().take(12).collect::<String>();
    io.push_status(
        "SOURCE",
        &format!("Landing the active commit, \"{short}\"."),
    );
    Ok(vec![commit.trim().to_string()])
}

/// Resolve Mercurial source symbols to commits
/// (`ArcanistMercurialLandEngine::resolveSymbols()`, `:77-152`).
///
/// Bookmarks win over branches; anything else resolves as a commit (or
/// fails with the verbatim error). Ambiguous markers print the
/// `AMBIGUOUS SYMBOL` banner and fail like PHP.
pub fn hg_resolve_symbols(
    dir: &Path,
    io: &mut Io,
    symbols: &[String],
) -> Result<Vec<String>, String> {
    hg_resolve_symbols_with(dir, &hg::HgCommand::system(), io, symbols)
}

/// [`hg_resolve_symbols`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_resolve_symbols_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    symbols: &[String],
) -> Result<Vec<String>, String> {
    let bookmarks = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    let bookmark_names: Vec<String> = parse_hg_bookmarks(&bookmarks)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let branches = hg::stdout_text_with(dir, hg_cmd, &["branches"]).unwrap_or_default();
    let branch_names = parse_hg_branches(&branches);
    let mut commits = Vec::new();
    for symbol in symbols {
        let bookmark_hits = bookmark_names.iter().filter(|name| *name == symbol).count();
        let branch_hits = branch_names.iter().filter(|name| *name == symbol).count();
        if bookmark_hits > 1 {
            return Err(hg_ambiguous_symbol(io, symbol, "bookmark", bookmark_hits));
        }
        if branch_hits > 1 {
            return Err(hg_ambiguous_symbol(io, symbol, "branch", branch_hits));
        }
        if bookmark_hits == 1 || branch_hits == 1 {
            commits.push(hg_canonical_with(dir, hg_cmd, symbol)?);
            continue;
        }
        match hg_canonical_with(dir, hg_cmd, symbol) {
            Ok(commit) => commits.push(commit),
            Err(_) => {
                return Err(format!(
                    "Symbol \"{symbol}\" does not identify a bookmark, branch, or commit."
                ));
            }
        }
    }
    Ok(commits)
}

/// Print the `AMBIGUOUS SYMBOL` banner and return its error
/// (`ArcanistMercurialLandEngine.php:102-122`).
///
/// Each matching marker renders as a ref view (`:112-114`). Markers
/// that share one name are indistinguishable to the port, so each
/// view shows the name alone.
fn hg_ambiguous_symbol(io: &mut Io, symbol: &str, marker_type: &str, hits: usize) -> String {
    push_banner(
        io,
        "AMBIGUOUS SYMBOL",
        &format!(
            "Symbol \"{symbol}\" is ambiguous: it matches multiple markers (of type \
             \"{marker_type}\"). Use an unambiguous identifier."
        ),
    );
    let display = match marker_type {
        "bookmark" => format!("Bookmark \"{symbol}\""),
        "branch" => format!("Branch \"{symbol}\""),
        _ => format!("Marker \"{symbol}\""),
    };
    for _ in 0..hits {
        io.push_out(&marker_ref_view(&display, "", "").render_with_ansi(io.ansi));
    }
    io.push_out("\n");
    format!("Symbol \"{symbol}\" is ambiguous.")
}

/// Resolve one Mercurial revision to its node hash
/// (`ArcanistMercurialAPI::getCanonicalRevisionName()`, `:57-64`),
/// driving `hg` through `hg_cmd` (test seam).
fn hg_canonical_with(dir: &Path, hg_cmd: &hg::HgCommand, symbol: &str) -> Result<String, String> {
    hg::stdout_text_with(
        dir,
        hg_cmd,
        &["log", "-l", "1", "--template", "{node}", "-r", symbol, "--"],
    )
}

/// Select the Mercurial onto remote: `--onto-remote`, then
/// `arc.land.onto-remote` configuration, then `default`
/// (`ArcanistMercurialLandEngine::selectOntoRemote()`, `:154-217`).
///
/// The remote must exist (`hg paths NAME`); pushes the `ONTO REMOTE`
/// status line.
pub fn hg_select_onto_remote(dir: &Path, io: &mut Io, options: &Options) -> Result<String, String> {
    hg_select_onto_remote_with(dir, &hg::HgCommand::system(), io, options)
}

/// [`hg_select_onto_remote`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_select_onto_remote_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    options: &Options,
) -> Result<String, String> {
    let remote = if let Some(remote) = options.onto_remote.as_deref() {
        io.push_status(
            "ONTO REMOTE",
            &format!("Remote \"{remote}\" was selected with the \"--onto-remote\" flag."),
        );
        remote.to_string()
    } else if let Some(remote) = land_config_value(options, dir, "arc.land.onto-remote")?
        .as_ref()
        .and_then(parse_string_config)
    {
        io.push_status(
            "ONTO REMOTE",
            &format!(
                "Remote \"{remote}\" was selected by reading \"arc.land.onto-remote\" \
                 configuration."
            ),
        );
        remote
    } else {
        io.push_status(
            "ONTO REMOTE",
            "Landing onto remote \"default\", the default remote under Mercurial.",
        );
        "default".to_string()
    };
    if !hg::ok_with(dir, hg_cmd, &["paths", &remote]) {
        return Err(format!("No remote \"{remote}\" exists in this repository."));
    }
    Ok(remote)
}

/// Select the Mercurial onto refs: `--onto`, then `arc.land.onto`
/// configuration, then `default`
/// (`ArcanistMercurialLandEngine::selectOntoRefs()`, `:219-260`).
pub fn hg_select_onto_refs(
    dir: &Path,
    io: &mut Io,
    options: &Options,
) -> Result<Vec<String>, String> {
    if !options.onto.is_empty() {
        io.push_status(
            "ONTO TARGET",
            &format!(
                "Refs were selected with the \"--onto\" flag: {}.",
                options.onto.join(", ")
            ),
        );
        return Ok(options.onto.clone());
    }
    if let Some(value) = land_config_value(options, dir, "arc.land.onto")? {
        let onto = parse_onto_config(&value)?;
        if !onto.is_empty() {
            io.push_status(
                "ONTO TARGET",
                &format!(
                    "Refs were selected by reading \"arc.land.onto\" configuration: {}.",
                    onto.join(", ")
                ),
            );
            return Ok(onto);
        }
    }
    io.push_status(
        "ONTO TARGET",
        "Landing onto target \"default\", the default target under Mercurial.",
    );
    Ok(vec!["default".to_string()])
}

/// True when `name` names a local branch (vs a bookmark), driving
/// `hg` through `hg_cmd` (test seam).
fn hg_is_branch_with(dir: &Path, hg_cmd: &hg::HgCommand, name: &str) -> bool {
    let branches = hg::stdout_text_with(dir, hg_cmd, &["branches"]).unwrap_or_default();
    parse_hg_branches(&branches).contains(&name.to_string())
}

/// Probe whether `name` exists in `remote` via
/// `hg identify --id --rev NAME REMOTE`; probe failures count as missing
/// (see the module docs). Drives `hg` through `hg_cmd` (test seam).
fn hg_remote_ref_exists_with(dir: &Path, hg_cmd: &hg::HgCommand, remote: &str, name: &str) -> bool {
    hg::output_with(dir, hg_cmd, &["identify", "--id", "--rev", name, remote])
        .is_ok_and(|out| out.status.success())
}

/// Confirm the Mercurial onto refs
/// (`ArcanistMercurialLandEngine::confirmOntoRefs()`, `:262-383`).
///
/// Empty refs fail; several branches fail with the multiple-branches
/// banner and error; missing refs print the `CREATE N BOOKMARK(S)` banner
/// and prompt `arc.land.create` (with the hold note under `--hold`).
/// Returns the onto branch marker, if any.
pub fn hg_confirm_onto_refs(
    dir: &Path,
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    hold: bool,
) -> Result<Option<String>, String> {
    hg_confirm_onto_refs_with(
        dir,
        &hg::HgCommand::system(),
        io,
        onto_remote,
        onto_refs,
        hold,
    )
}

/// [`hg_confirm_onto_refs`] driving `hg` through `hg` (test seam).
pub fn hg_confirm_onto_refs_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    hold: bool,
) -> Result<Option<String>, String> {
    for onto_ref in onto_refs {
        if onto_ref.is_empty() {
            return Err(format!(
                "Selected \"onto\" ref \"{onto_ref}\" is invalid: the empty string is not a valid \
                 ref."
            ));
        }
    }
    let branches: Vec<&String> = onto_refs
        .iter()
        .filter(|name| hg_is_branch_with(dir, hg_cmd, name))
        .collect();
    if branches.len() > 1 {
        push_banner(
            io,
            "MULTIPLE \"ONTO\" BRANCHES",
            "You have selected multiple branches to push changes onto. Pushing to multiple \
             branches is not supported by \"arc land\" in Mercurial: Mercurial commits may only \
             belong to one branch, so this operation can not be executed atomically.\n\nYou may \
             land one branches and any number of bookmarks in a single operation.\n\nThese \
             branches were selected:",
        );
        for branch in &branches {
            io.push_out(&hg_branch_marker_view_with(dir, hg_cmd, branch).render_with_ansi(io.ansi));
        }
        io.push_out("\n");
        return Err(
            "Landing onto multiple branches at once is not supported in Mercurial.".to_string(),
        );
    }
    let new_markers: Vec<&String> = onto_refs
        .iter()
        .filter(|name| !hg_remote_ref_exists_with(dir, hg_cmd, onto_remote, name))
        .collect();
    if !new_markers.is_empty() {
        let count = new_markers.len();
        push_banner(
            io,
            &format!("CREATE {count} BOOKMARK(S)"),
            &format!(
                "These {count} symbol(s) do not exist in the remote. They will be created as new \
                 bookmarks:"
            ),
        );
        for name in &new_markers {
            io.push_out(
                &marker_ref_view(&format!("Bookmark \"{name}\""), "", "").render_with_ansi(io.ansi),
            );
        }
        io.push_out("\n");
        if hold {
            io.push_out(&render_hint_with_ansi(
                &format!(
                    "You are using \"--hold\", so execution will stop before the {count} \
                     bookmark(s) are actually created. You will be given instructions to create \
                     the bookmarks."
                ),
                io.ansi,
            ));
            io.push_out("\n");
        }
        let query = format!("Create {count} new remote bookmark(s)?");
        prompt_for_key(io, land_checks::PROMPT_CREATE, &query)?;
    }
    Ok(branches.into_iter().next().cloned())
}

/// Branch marker view for the multiple-branches error: the local tip
/// hash and summary. PHP renders remote marker data; the port reads
/// the local branch tip (see the module docs).
fn hg_branch_marker_view_with(dir: &Path, hg_cmd: &hg::HgCommand, branch: &str) -> RefView {
    let hash = hg_canonical_with(dir, hg_cmd, branch)
        .map(|node| display_hash_12(&node))
        .unwrap_or_default();
    let summary = hg::stdout_text_with(dir, hg_cmd, &["log", "-r", branch, "--template", "{desc}"])
        .map(|desc| first_summary_line(&desc))
        .unwrap_or_default();
    marker_ref_view(&format!("Branch \"{branch}\""), &hash, &summary)
}

/// First line of a commit message, like `{desc|firstline}`.
fn first_summary_line(desc: &str) -> String {
    desc.split('\n').next().unwrap_or("").trim().to_string()
}

/// Resolve the Mercurial into state to a commit, pulling when remote
/// (`ArcanistMercurialLandEngine::selectIntoCommit()`, `:497-565`,
/// plus `fetchTarget()`).
///
/// Returns `Ok(None)` for the empty state (explicit or implied).
pub fn hg_select_into_commit(
    dir: &Path,
    io: &mut Io,
    state: &IntoState,
    into_argument: Option<&str>,
) -> Result<Option<String>, String> {
    hg_select_into_commit_with(dir, &hg::HgCommand::system(), io, state, into_argument)
}

/// [`hg_select_into_commit`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_select_into_commit_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    state: &IntoState,
    into_argument: Option<&str>,
) -> Result<Option<String>, String> {
    match state {
        IntoState::Empty => {
            io.push_status("INTO COMMIT", "Preparing merge into the empty state.");
            Ok(None)
        }
        IntoState::Local(local_ref) => {
            let into_commit = hg_canonical_with(dir, hg_cmd, local_ref)
                .map_err(|_| format!("Local ref \"{local_ref}\" does not exist."))?;
            io.push_status(
                "INTO COMMIT",
                &format!(
                    "Preparing merge into local target \"{local_ref}\", at commit \"{}\".",
                    display_hash_12(&into_commit)
                ),
            );
            Ok(Some(into_commit))
        }
        IntoState::Remote { remote, name } => {
            // Pull the ref, then resolve it locally; failures leave the
            // previous state alone, like the git ignore-failure fetch.
            // The pull inherits stdio (`newPassthru()`).
            hg_passthru_with(dir, hg_cmd, &["pull", "--rev", name, remote]);
            if let Ok(commit) = hg_canonical_with(dir, hg_cmd, name) {
                io.push_status(
                    "INTO COMMIT",
                    &format!(
                        "Preparing merge into \"{name}\" from remote \"{remote}\", at commit \
                         \"{}\".",
                        display_hash_12(&commit)
                    ),
                );
                return Ok(Some(commit));
            }
            if into_argument.is_some() {
                return Err(format!(
                    "Ref \"{name}\" does not exist in remote \"{remote}\"."
                ));
            }
            io.push_status(
                "INTO COMMIT",
                &format!(
                    "Preparing merge into the empty state to create target \"{name}\" in remote \
                     \"{remote}\"."
                ),
            );
            Ok(None)
        }
    }
}

/// First 12 characters of a hash for display (like
/// `ArcanistRepositoryAPI::getDisplayHash()`).
fn display_hash_12(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// Collect the commits landing from `symbols` against `into_commit`
/// (`ArcanistMercurialLandEngine::selectCommits()`, `:696-770`).
///
/// Runs `hg log --rev reverse(ancestors(SYMBOL) - ancestors(INTO))
/// --template {node}-{parents}-{desc|firstline}` per symbol (or
/// `reverse(ancestors(SYMBOL))` into the empty state; see the module docs
/// for the intentional upstream deviation).
pub fn hg_select_commits(
    dir: &Path,
    symbols: &[GitSymbol],
    into_commit: Option<&str>,
) -> Result<Vec<LandCommit>, String> {
    hg_select_commits_with(dir, &hg::HgCommand::system(), symbols, into_commit)
}

/// [`hg_select_commits`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_select_commits_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    symbols: &[GitSymbol],
    into_commit: Option<&str>,
) -> Result<Vec<LandCommit>, String> {
    let template = "{node}-{parents % '{node} '}-{desc|firstline}\\n";
    let mut commits = Vec::new();
    for symbol in symbols {
        let revset = match into_commit {
            None => format!("reverse(ancestors({}))", symbol.commit),
            Some(into) => format!(
                "reverse(ancestors({}) - ancestors({}))",
                symbol.commit, into
            ),
        };
        let output = hg::stdout_text_with(
            dir,
            hg_cmd,
            &["log", "--rev", &revset, "--template", template, "--"],
        )?;
        collect_hg_commits(&mut commits, &output, &symbol.symbol)?;
    }
    Ok(commits)
}

/// Merge one commit set with Mercurial rebase-collapse
/// (`ArcanistMercurialLandEngine::executeMerge()`, `:772-886`).
///
/// Only the squash strategy is supported (anything else fails with the
/// verbatim `TODO: Support merge strategies`). Updates to the into commit,
/// sets the onto branch marker when landing onto a branch, then
/// `rebase --dest INTO --rev MIN..MAX --logfile - --keep --collapse` with
/// the commit message on stdin; restores moved bookmarks and returns the
/// new tip. Rebase failures abort and propagate.
#[allow(clippy::too_many_arguments)]
pub fn hg_execute_merge(
    dir: &Path,
    io: &mut Io,
    set: &CommitSet,
    commits: &[LandCommit],
    into_commit: Option<&str>,
    squash: bool,
    branch_marker: Option<&str>,
    message_of: &dyn Fn(&CommitSet) -> Result<String, String>,
) -> Result<String, String> {
    hg_execute_merge_with(
        dir,
        &hg::HgCommand::system(),
        io,
        set,
        commits,
        into_commit,
        squash,
        branch_marker,
        message_of,
    )
}

/// [`hg_execute_merge`] driving `hg` through `hg_cmd` (test seam).
#[allow(clippy::too_many_arguments)]
pub fn hg_execute_merge_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    _io: &mut Io,
    set: &CommitSet,
    commits: &[LandCommit],
    into_commit: Option<&str>,
    squash: bool,
    branch_marker: Option<&str>,
    message_of: &dyn Fn(&CommitSet) -> Result<String, String>,
) -> Result<String, String> {
    if !squash {
        return Err("TODO: Support merge strategies".to_string());
    }
    let Some(into_commit) = into_commit else {
        return Err("landing into the empty state is not supported under Mercurial".to_string());
    };
    let before = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    let mut bookmark_positions: Vec<(String, String)> = Vec::new();
    for (name, _) in parse_hg_bookmarks(&before) {
        if let Some(position) = hg_bookmark_position_with(dir, hg_cmd, &name) {
            bookmark_positions.push((name, position));
        }
    }
    hg::stdout_text_with(dir, hg_cmd, &["update", "--rev", into_commit])?;
    if let Some(branch) = branch_marker {
        hg::stdout_text_with(dir, hg_cmd, &["branch", "--", branch])?;
    }
    let min = &commits[set.commits[0]].hash;
    let max = &commits[set.commits[set.commits.len() - 1]].hash;
    let message = message_of(set)?;
    let range = format!("{min}..{max}");
    let wrote = hg_write_stdin_with(
        dir,
        hg_cmd,
        &[
            "rebase",
            "--dest",
            into_commit,
            "--rev",
            &range,
            "--logfile",
            "-",
            "--keep",
            "--collapse",
        ],
        &message,
    );
    if !wrote {
        hg::output_with(dir, hg_cmd, &["rebase", "--abort"]).ok();
        return Err("rebase failed".to_string());
    }
    // Put back bookmarks the rebase moved (`:853-873`).
    let obsolete: Vec<&str> = set
        .commits
        .iter()
        .map(|index| commits[*index].hash.as_str())
        .collect();
    for (name, position) in &bookmark_positions {
        if obsolete.contains(&position.as_str()) {
            hg::output_with(
                dir,
                hg_cmd,
                &["bookmark", "--force", "--rev", position, "--", name],
            )
            .ok();
        }
    }
    let tip = hg::stdout_text_with(
        dir,
        hg_cmd,
        &["log", "--rev", "tip", "--template", "{node}"],
    )?;
    Ok(tip.trim().to_string())
}

/// Run `hg` inheriting stdio, returning success (`newPassthru()`).
///
/// Push and pull must not capture output: they may prompt for
/// credentials, and progress belongs on the terminal. Honors the
/// `HgCommand` program/prefix plus the encoding args and `HGPLAIN`.
fn hg_passthru_with(dir: &Path, hg_cmd: &hg::HgCommand, args: &[&str]) -> bool {
    let mut command = std::process::Command::new(&hg_cmd.program);
    command
        .current_dir(dir)
        .args(&hg_cmd.prefix)
        .args(hg::HG_ENCODING_ARGS)
        .args(args)
        .env(hg::HGPLAIN.0, hg::HGPLAIN.1);
    command.status().is_ok_and(|status| status.success())
}

/// Current position of bookmark `name`, if any, driving `hg`
/// through `hg_cmd` (test seam).
fn hg_bookmark_position_with(dir: &Path, hg_cmd: &hg::HgCommand, name: &str) -> Option<String> {
    let output =
        hg::stdout_text_with(dir, hg_cmd, &["log", "--rev", name, "--template", "{node}"]).ok()?;
    let trimmed = output.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Run `hg` with `args` in `dir`, writing `stdin_text` to its stdin.
/// Returns success. Drives `hg` through `hg_cmd` (test seam).
fn hg_write_stdin_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    args: &[&str],
    stdin_text: &str,
) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = std::process::Command::new(&hg_cmd.program);
    command
        .current_dir(dir)
        .args(&hg_cmd.prefix)
        .args(hg::HG_ENCODING_ARGS)
        .args(args)
        .env(hg::HGPLAIN.0, hg::HGPLAIN.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    if child
        .stdin
        .as_mut()
        .is_none_or(|stdin| stdin.write_all(stdin_text.as_bytes()).is_err())
    {
        return false;
    }
    child
        .wait_with_output()
        .is_ok_and(|out| out.status.success())
}

/// Push one landed Mercurial commit
/// (`ArcanistMercurialLandEngine::pushChange()`, `:888-922`, plus
/// `newPushCommands()`, `:924-1043`).
///
/// Moves local bookmarks onto the commit, pushes `--bookmark ...` (or
/// `--rev` without bookmarks) to the remote, then pulls the bookmarks
/// back; on failure restores the moved bookmarks and fails with
/// `Push failed! ...`.
pub fn hg_push_change(
    dir: &Path,
    io: &mut Io,
    onto_remote: &str,
    bookmarks: &[String],
    into_commit: &str,
) -> Result<(), String> {
    hg_push_change_with(
        dir,
        &hg::HgCommand::system(),
        io,
        onto_remote,
        bookmarks,
        into_commit,
    )
}

/// [`hg_push_change`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_push_change_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    _io: &mut Io,
    onto_remote: &str,
    bookmarks: &[String],
    into_commit: &str,
) -> Result<(), String> {
    let mut restore: Vec<(String, String)> = Vec::new();
    for bookmark in bookmarks {
        let old = hg_bookmark_position_with(dir, hg_cmd, bookmark);
        if old.as_deref() == Some(into_commit) {
            continue;
        }
        hg::stdout_text_with(
            dir,
            hg_cmd,
            &["bookmark", "--force", "--rev", into_commit, "--", bookmark],
        )?;
        if let Some(old) = old {
            restore.push((bookmark.clone(), old));
        }
    }
    let mut args = vec!["push".to_string()];
    if bookmarks.is_empty() {
        args.push("--rev".to_string());
        args.push(into_commit.to_string());
    } else {
        for bookmark in bookmarks {
            args.push("--bookmark".to_string());
            args.push(bookmark.clone());
        }
    }
    args.push("--".to_string());
    args.push(onto_remote.to_string());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    // The push inherits stdio (`newPassthru()`,
    // `ArcanistMercurialLandEngine.php:871-877`).
    let pushed = hg_passthru_with(dir, hg_cmd, &argv);
    if !pushed {
        for (bookmark, old) in &restore {
            hg::output_with(dir, hg_cmd, &["bookmark", "--force", "--rev", old]).ok();
            let _ = bookmark;
        }
        return Err("Push failed! Fix the error and run \"arc land\" again.".to_string());
    }
    if !restore.is_empty() {
        let mut pull = vec!["pull"];
        let mut owned: Vec<String> = Vec::new();
        for (bookmark, _) in &restore {
            owned.push("--bookmark".to_string());
            owned.push(bookmark.clone());
        }
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        pull.extend(refs);
        hg::output_with(dir, hg_cmd, &pull).ok();
    }
    Ok(())
}

/// Rebase children of the landed commit onto the new state (squash only)
/// (`ArcanistMercurialLandEngine::cascadeState()`, `:1045-1086`).
pub fn hg_cascade_state(
    dir: &Path,
    set: &CommitSet,
    commits: &[LandCommit],
    new_commit: &str,
    squash: bool,
) -> Result<(), String> {
    hg_cascade_state_with(
        dir,
        &hg::HgCommand::system(),
        set,
        commits,
        new_commit,
        squash,
    )
}

/// [`hg_cascade_state`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_cascade_state_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    set: &CommitSet,
    commits: &[LandCommit],
    new_commit: &str,
    squash: bool,
) -> Result<(), String> {
    if !squash {
        return Ok(());
    }
    let old = &commits[set.commits[set.commits.len() - 1]].hash;
    let output = hg::stdout_text_with(
        dir,
        hg_cmd,
        &[
            "log",
            "--rev",
            &format!("children({old})"),
            "--template",
            "{node}\n",
        ],
    )
    .unwrap_or_default();
    for child in output.split('\n') {
        let child = child.trim();
        if child.is_empty() {
            continue;
        }
        if hg::output_with(
            dir,
            hg_cmd,
            &[
                "rebase",
                "--source",
                child,
                "--dest",
                new_commit,
                "--keep",
                "--keepbranches",
            ],
        )
        .is_ok_and(|out| !out.status.success())
        {
            hg::output_with(dir, hg_cmd, &["rebase", "--abort"]).ok();
            return Err(format!("rebase of \"{child}\" onto landed state failed"));
        }
    }
    Ok(())
}

/// Delete obsolete bookmarks and strip the landed range (squash only)
/// (`ArcanistMercurialLandEngine::pruneBranches()`, `:1089-1176`).
///
/// Uses `strip --rev MIN::MAX` (the evolve `prune` path is not ported).
pub fn hg_prune_branches(
    dir: &Path,
    io: &mut Io,
    sets: &[CommitSet],
    commits: &[LandCommit],
) -> Result<(), String> {
    hg_prune_branches_with(dir, &hg::HgCommand::system(), io, sets, commits)
}

/// [`hg_prune_branches`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_prune_branches_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    sets: &[CommitSet],
    commits: &[LandCommit],
) -> Result<(), String> {
    let mut revs: Vec<String> = Vec::new();
    let mut obsolete: Vec<&str> = Vec::new();
    for set in sets.iter().rev() {
        let min = &commits[set.commits[0]].hash;
        let max = &commits[set.commits[set.commits.len() - 1]].hash;
        revs.push(format!("{min}::{max}"));
        for index in &set.commits {
            obsolete.push(commits[*index].hash.as_str());
        }
    }
    let before = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    for (name, _) in parse_hg_bookmarks(&before) {
        let position = hg_bookmark_position_with(dir, hg_cmd, &name);
        if position.is_some_and(|hash| obsolete.contains(&hash.as_str())) {
            io.push_status("CLEANUP", &format!("Deleting bookmark \"{name}\"."));
            hg::stdout_text_with(dir, hg_cmd, &["bookmark", "--delete", "--", &name])?;
        }
    }
    let revset = format!("({})", revs.join(") or ("));
    hg::stdout_text_with(dir, hg_cmd, &["strip", "--rev", &revset])?;
    Ok(())
}

/// Update back to the original commit or bookmark after landing.
///
/// Ports the not-rebased half of `reconcileLocalState()`
/// (`ArcanistMercurialLandEngine.php:1178-1198`); the rebased-active-commit
/// half is not ported (see the module docs on local state).
pub fn hg_reconcile(dir: &Path, local_commit: &str, local_bookmark: Option<&str>) {
    hg_reconcile_with(dir, &hg::HgCommand::system(), local_commit, local_bookmark);
}

/// [`hg_reconcile`] driving `hg` through `hg_cmd` (test seam).
pub fn hg_reconcile_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    local_commit: &str,
    local_bookmark: Option<&str>,
) {
    let marker = local_bookmark.unwrap_or(local_commit);
    hg::output_with(dir, hg_cmd, &["update", "--", marker]).ok();
}

/// Print the Mercurial hold instructions
/// (`ArcanistMercurialLandEngine::didHoldChanges()`, `:1234-1282`).
pub fn hg_hold_text(
    io: &mut Io,
    bookmarks: &[String],
    onto_remote: &str,
    into_commit: &str,
    restore: &[String],
) {
    push_banner_mode(
        io,
        "HOLD CHANGES",
        "Holding changes locally, they have not been pushed.",
        false,
    );
    let mut commands: Vec<String> = Vec::new();
    for bookmark in bookmarks {
        commands.push(format!(
            "hg bookmark --force --rev {into_commit} -- {bookmark}"
        ));
    }
    if bookmarks.is_empty() {
        commands.push(format!("hg push --rev {into_commit} -- {onto_remote}"));
    } else {
        let mut push = "hg push".to_string();
        for bookmark in bookmarks {
            push.push_str(&format!(" --bookmark {bookmark}"));
        }
        push.push_str(&format!(" -- {onto_remote}"));
        commands.push(push);
    }
    let count = commands.len().to_string();
    io.push_out(&terminal::console_format(
        "To push changes manually, run these %s command(s):\n\n",
        &[&count],
        io.ansi,
    ));
    for command in &commands {
        io.push_out(&shell_line(command, io.ansi));
    }
    io.push_out("\n");
    if !restore.is_empty() {
        let count = restore.len().to_string();
        io.push_out(&terminal::console_format(
            "To go back to how things were before you ran \"arc land\", run these %s \
             command(s):\n\n",
            &[&count],
            io.ansi,
        ));
        for command in restore {
            io.push_out(&shell_line(command, io.ansi));
        }
        io.push_out("\n");
    }
    io.push_out(
        "Local branches and bookmarks have not been changed, and are still in the same state as \
         before.\n",
    );
}

/// Render one commit set: the revision ref view plus its commits
/// (`ArcanistLandEngine::printCommitSet()`, `:768-796`).
///
/// Implicit commits are marked with `!` (plain-text rendering of the
/// yellow highlight, which PHP notes survives copy/paste).
pub fn render_commit_set(
    set: &CommitSet,
    commits: &[LandCommit],
    display_hash: &dyn Fn(&str) -> String,
) -> String {
    render_commit_set_with_ansi(set, commits, display_hash, terminal::ansi_enabled())
}

/// Render one commit set with an explicit ANSI flag: TTYs render the
/// ref-view markup and the yellow `<bg:yellow>` implicit-commit
/// highlight; non-TTYs keep the stripped rendering byte-identical.
pub fn render_commit_set_with_ansi(
    set: &CommitSet,
    commits: &[LandCommit],
    display_hash: &dyn Fn(&str) -> String,
    ansi: bool,
) -> String {
    let mut text = String::new();
    match &set.revision {
        Some(revision) => {
            text.push('\n');
            text.push_str(&revision_ref_view(revision).render_with_ansi(ansi));
        }
        None => text.push_str("\n  (no associated revision)\n"),
    }
    for index in &set.commits {
        let commit = &commits[*index];
        let short = display_hash(&commit.hash);
        let summary = display_summary(&commit.summary);
        text.push_str(&commit_set_line(&short, &summary, commit.is_implicit, ansi));
    }
    text
}

/// One `printCommitSet()` commit line (`:784-795`): implicit commits
/// carry the yellow `<bg:yellow>` highlight on TTYs, else the plain
/// rendering.
fn commit_set_line(short: &str, summary: &str, implicit: bool, ansi: bool) -> String {
    if !ansi {
        return if implicit {
            format!("       ! {short} {summary}\n")
        } else {
            format!("          {short}  {summary}\n")
        };
    }
    let pattern = if implicit {
        "       ! <bg:yellow> %s </bg> %s\n"
    } else {
        "          %s  %s\n"
    };
    terminal::tsprintf_ansi(pattern, &[TVal::Text(short), TVal::Text(summary)], true)
        .expect("fixed tsprintf pattern")
        .as_str()
        .to_string()
}

/// Confirm implicitly-associated commits
/// (`ArcanistLandEngine::confirmImplicitCommits()`, `:720-756`).
///
/// Prints the `IMPLICIT COMMITS` banner with every implicit set and
/// prompts `arc.land.implicit`. Skipped when no set has implicit commits.
pub fn confirm_implicit_sets(
    io: &mut Io,
    sets: &[CommitSet],
    commits: &[LandCommit],
    symbols_display: &str,
    display_hash: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    let implicit: Vec<&CommitSet> = sets
        .iter()
        .filter(|set| set.has_implicit_commits(commits))
        .collect();
    if implicit.is_empty() {
        return Ok(());
    }
    push_banner_continued(
        io,
        "IMPLICIT COMMITS",
        &format!(
            "Some commits reachable from the specified sources ({symbols_display}) are not \
             associated with revisions, and may not have been reviewed. These commits will be \
             landed as though they belong to the nearest ancestor revision:"
        ),
    );
    for set in &implicit {
        io.push_out(&render_commit_set_with_ansi(
            set,
            commits,
            display_hash,
            io.ansi,
        ));
    }
    prompt_for_key(
        io,
        land_checks::PROMPT_IMPLICIT,
        "Continue with this mapping between commits and revisions?",
    )
}

/// Confirm every revision gate interactively, in PHP order
/// (`ArcanistLandEngine::confirmRevisions()`, `:278-534`).
///
/// Each fired finding prints its banner (title, explanation, affected
/// revisions) and prompts its `arc.land.*` key. Declining aborts.
/// Viewer identity for gate banners: the PHID plus a display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerIdentity {
    /// Viewer PHID from `user.whoami`.
    pub phid: String,
    /// Viewer monogram from `user.whoami` (`@userName`,
    /// `ArcanistUserRef::newFromConduitWhoami()` + `getMonogram()`).
    pub name: String,
}

/// Load the viewer identity via `user.whoami`.
pub fn fetch_viewer_identity(
    call: &diff_submit::ConduitCall<'_>,
) -> Result<ViewerIdentity, String> {
    let result = call(METHOD_WHOAMI, &serde_json::json!({}))?;
    let phid = result
        .get("phid")
        .and_then(|phid| phid.as_str())
        .filter(|phid| !phid.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "user.whoami response has no phid".to_string())?;
    let username = result
        .get("userName")
        .and_then(|name| name.as_str())
        .unwrap_or("");
    Ok(ViewerIdentity {
        phid,
        name: format!("@{username}"),
    })
}

/// Load author monograms (`@username`) for `phids` with one batched
/// `user.search`, keyed by user PHID
/// (`ArcanistRevisionAuthorHardpointQuery.php:17-34`,
/// `ArcanistUserSymbolHardpointQuery.php:66-77`). Unknown PHIDs are
/// absent from the map.
pub fn fetch_user_monograms(
    call: &diff_submit::ConduitCall<'_>,
    phids: &[String],
) -> Result<HashMap<String, String>, String> {
    let mut distinct: Vec<String> = Vec::new();
    for phid in phids {
        if !phid.is_empty() && !distinct.contains(phid) {
            distinct.push(phid.clone());
        }
    }
    let mut monograms = HashMap::new();
    if distinct.is_empty() {
        return Ok(monograms);
    }
    let params = revision::conduit_search_params(&serde_json::json!({"phids": distinct}), None);
    for page in conduit::search_pages(call, METHOD_USER_SEARCH, &params)? {
        let empty = Vec::new();
        let rows = page
            .get("data")
            .and_then(|data| data.as_array())
            .unwrap_or(&empty);
        for row in rows {
            let phid = row.get("phid").and_then(|v| v.as_str()).unwrap_or("");
            if phid.is_empty() {
                continue;
            }
            let username = row
                .get("fields")
                .and_then(|fields| fields.get("username"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            monograms.insert(phid.to_string(), format!("@{username}"));
        }
    }
    Ok(monograms)
}

pub fn confirm_revision_gates(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    viewer: &ViewerIdentity,
    links: &[land_checks::ParentLink],
    builds: &[land_checks::BuildView],
    findings: &[land_checks::GateFinding],
) -> Result<(), String> {
    confirm_revision_gates_with_authors(
        io,
        revisions,
        viewer,
        links,
        builds,
        findings,
        &HashMap::new(),
    )
}

/// [`confirm_revision_gates`] with author monograms for the
/// `NOT REVISION AUTHOR` banner (`author PHID -> @username`, see
/// [`fetch_user_monograms`]). Revisions without a known monogram show
/// no `Author:` line, like PHP's missing author ref
/// (`ArcanistLandEngine.php:310-316`).
pub fn confirm_revision_gates_with_authors(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    viewer: &ViewerIdentity,
    links: &[land_checks::ParentLink],
    builds: &[land_checks::BuildView],
    findings: &[land_checks::GateFinding],
    authors: &HashMap<String, String>,
) -> Result<(), String> {
    for finding in findings {
        confirm_single_gate(io, revisions, viewer, links, builds, finding, authors)?;
    }
    Ok(())
}

/// Confirm one fired revision gate: print its banner and prompt its
/// `arc.land.*` key. Declining aborts.
#[allow(clippy::too_many_arguments)]
fn confirm_single_gate(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    viewer: &ViewerIdentity,
    links: &[land_checks::ParentLink],
    builds: &[land_checks::BuildView],
    finding: &land_checks::GateFinding,
    authors: &HashMap<String, String>,
) -> Result<(), String> {
    match finding.prompt {
        land_checks::PROMPT_UNAUTHORED => {
            confirm_unauthored(io, revisions, viewer, authors)?;
        }
        land_checks::PROMPT_CHANGES_PLANNED => {
            confirm_status_group(io, revisions, finding, StatusGroup::Planned)?;
        }
        land_checks::PROMPT_PUBLISHED => {
            confirm_status_group(io, revisions, finding, StatusGroup::Published)?;
        }
        land_checks::PROMPT_NOT_ACCEPTED => {
            confirm_status_group(io, revisions, finding, StatusGroup::NotAccepted)?;
        }
        land_checks::PROMPT_OPEN_PARENTS => {
            confirm_open_parents(io, revisions, links, finding)?;
        }
        land_checks::PROMPT_FAILED_BUILDS | land_checks::PROMPT_ONGOING_BUILDS => {
            confirm_builds(io, revisions, builds, finding)?;
        }
        _ => {
            prompt_for_key(io, finding.prompt, &finding.query)?;
        }
    }
    Ok(())
}

/// Which status bucket a finding covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusGroup {
    Planned,
    Published,
    NotAccepted,
}

/// Print the `NOT REVISION AUTHOR` banner and prompt
/// (`ArcanistLandEngine.php:293-333`).
///
/// `authors` maps author PHIDs to `@username` monograms (see
/// [`fetch_user_monograms`]); revisions without a known monogram show
/// no `Author:` line.
fn confirm_unauthored(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    viewer: &ViewerIdentity,
    authors: &HashMap<String, String>,
) -> Result<(), String> {
    push_banner(
        io,
        "NOT REVISION AUTHOR",
        &format!(
            "You are landing revisions which you (\"{}\") are not the author of:",
            viewer.name
        ),
    );
    for index in land_checks::unauthored_indexes(revisions, &viewer.phid) {
        let revision = &revisions[index];
        let mut view = revision_ref_view(revision);
        if let Some(monogram) = authors.get(&revision.author_phid) {
            view.lines.push(format!("Author: {monogram}"));
        }
        io.push_out(&view.render_with_ansi(io.ansi));
    }
    io.push_out("\n");
    io.push_out(&render_hint_with_ansi(
        "Use \"Commandeer\" in the web interface to become the author of a revision.",
        io.ansi,
    ));
    io.push_out("\n");
    prompt_for_key(
        io,
        land_checks::PROMPT_UNAUTHORED,
        "Land revisions you are not the author of?",
    )
}

/// Print a status-group banner and prompt (`:352-448`).
fn confirm_status_group(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    finding: &land_checks::GateFinding,
    group: StatusGroup,
) -> Result<(), String> {
    let partition = land_checks::partition_by_status(revisions);
    let indexes: &[usize] = match group {
        StatusGroup::Planned => &partition.planned,
        StatusGroup::Published => &partition.published,
        StatusGroup::NotAccepted => &partition.not_accepted,
    };
    let count = indexes.len();
    let example_status = indexes
        .first()
        .map(|index| revisions[*index].status_name.as_str())
        .unwrap_or("");
    let (title, intro) = match group {
        StatusGroup::Planned => (
            format!("{count} REVISION(S) HAVE CHANGES PLANNED"),
            format!(
                "You are landing {count} revision(s) which are currently in the state \
                 \"{example_status}\", indicating that you expect to revise them before moving \
                 forward.\n\nNormally, you should update these {count} revision(s), submit them \
                 for review, and wait for reviewers to accept them before you continue. To \
                 resubmit a revision for review, either: update the revision with revised \
                 changes; or use \"Request Review\" from the web interface.\n\nThese {count} \
                 revision(s) have changes planned:"
            ),
        ),
        StatusGroup::Published => (
            format!("{count} REVISION(S) ARE ALREADY PUBLISHED"),
            format!(
                "You are landing {count} revision(s) which are already in the state \
                 \"{example_status}\", indicating that they have previously landed:"
            ),
        ),
        StatusGroup::NotAccepted => (
            format!("{count} REVISION(S) ARE NOT ACCEPTED"),
            format!(
                "You are landing {count} revision(s) which are not in state \"Accepted\", \
                 indicating that they have not been accepted by reviewers. Normally, you should \
                 land changes only once they have been accepted. These revisions are in the wrong \
                 state:"
            ),
        ),
    };
    push_banner(io, &title, &intro);
    for index in indexes {
        let revision = &revisions[*index];
        let mut view = revision_ref_view(revision);
        if group == StatusGroup::NotAccepted {
            view.lines.push(format!("Status: {}", revision.status_name));
        }
        io.push_out(&view.render_with_ansi(io.ansi));
    }
    prompt_for_key(io, finding.prompt, &finding.query)
}

/// Print the open-parents banner and prompt (`:483-522`).
fn confirm_open_parents(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    links: &[land_checks::ParentLink],
    finding: &land_checks::GateFinding,
) -> Result<(), String> {
    let landing: Vec<&str> = revisions.iter().map(|rev| rev.phid.as_str()).collect();
    let groups = land_checks::open_parent_groups(links, &landing);
    let count = groups.len();
    push_banner(
        io,
        &finding.title,
        &format!(
            "The changes you are landing depend on {count} open parent revision(s). Usually, you \
             should land parent revisions before landing the changes which depend on them. These \
             parent revisions are open:"
        ),
    );
    for group in &groups {
        let mut view = revision_ref_view(&group.parent);
        view.lines
            .push(format!("Status: {}", group.parent.status_name));
        for child in &group.children {
            if let Some(revision) = revisions.get(*child) {
                view.lines.push(format!(
                    "Parent of: {} {}",
                    revision.monogram, revision.name
                ));
            }
        }
        io.push_out(&view.render_with_ansi(io.ansi));
    }
    prompt_for_key(io, finding.prompt, &finding.query)
}

/// Problem builds grouped by landing revision, first-seen order
/// (PHP `$build_map`, `ArcanistLandEngine.php:623-644`).
struct BuildGroup<'a> {
    revision_phid: String,
    buildable_monogram: String,
    buildable_uri: String,
    builds: Vec<&'a land_checks::BuildView>,
}

/// Group problem builds by revision, first-seen over `problems`.
fn group_problem_builds<'a>(
    builds: &'a [land_checks::BuildView],
    problems: &[usize],
) -> Vec<BuildGroup<'a>> {
    let mut groups: Vec<BuildGroup<'a>> = Vec::new();
    for index in problems {
        let Some(build) = builds.get(*index) else {
            continue;
        };
        match groups
            .iter_mut()
            .find(|group| group.revision_phid == build.revision_phid)
        {
            Some(group) => group.builds.push(build),
            None => groups.push(BuildGroup {
                revision_phid: build.revision_phid.clone(),
                buildable_monogram: build.buildable_monogram.clone(),
                buildable_uri: build.buildable_uri.clone(),
                builds: vec![build],
            }),
        }
    }
    groups
}

/// Print the builds banner and prompt (`:646-717`).
///
/// The banner has no leading blank line (`"%!\n%s\n"`, `:668-671` /
/// `:675-680`); each affected revision renders as a nested
/// revision > buildable > builds tree (`:691-712`). Groups for
/// revisions that are not landing are skipped.
/// The builds-gate banner (`ArcanistLandEngine.php:668-683`):
/// `tsprintf("%!\n%s\n", ...)` plus the blank line PHP echoes next,
/// on TTYs; else the plain `** TITLE **` rendering.
fn push_builds_banner(io: &mut Io, title: &str, message: &str) {
    if io.ansi {
        let text = terminal::tsprintf_ansi(
            "%!\n%s\n\n",
            &[TVal::Text(title), TVal::Text(message)],
            true,
        )
        .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
    } else {
        io.push_out(&format!("** {title} **\n{message}\n\n"));
    }
}

fn confirm_builds(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    builds: &[land_checks::BuildView],
    finding: &land_checks::GateFinding,
) -> Result<(), String> {
    let verdict = land_checks::classify_builds(builds);
    let count = verdict.affected_revisions;
    let (title, message) = if verdict.failures > 0 {
        let message = if verdict.ongoing > 0 {
            format!("{count} revision(s) have build failures or ongoing builds:")
        } else {
            format!("{count} revision(s) have build failures:")
        };
        ("BUILD FAILURES", message)
    } else {
        (
            "ONGOING BUILDS",
            format!("{count} revision(s) have ongoing builds:"),
        )
    };
    push_builds_banner(io, title, &message);
    for group in group_problem_builds(builds, &verdict.problems) {
        let Some(revision) = revisions
            .iter()
            .find(|revision| revision.phid == group.revision_phid)
        else {
            continue;
        };
        let mut view = revision_ref_view(revision);
        let mut buildable = buildable_ref_view(&group.buildable_monogram, &group.buildable_uri);
        for build in group.builds {
            buildable
                .children
                .push(build_ref_view(build.build_id, &build.build_name));
        }
        view.children.push(buildable);
        io.push_out(&view.render_with_ansi(io.ansi));
        io.push_out("\n");
    }
    prompt_for_key(io, finding.prompt, &finding.query)
}

/// Build the offline landed message for one unassociated set.
///
/// Ports the shape of the old `build_message()`: a `Land X` subject plus
/// the squashed commit list. Online lands use Differential messages.
pub fn offline_message(symbols: &[String], onto: &[String], commits: &[LandCommit]) -> String {
    let source = symbols.first().cloned().unwrap_or_default();
    let target = onto.first().cloned().unwrap_or_default();
    let mut message = format!("Land {source}\n");
    message.push_str(&format!(
        "\nSquashed {} commit(s) from '{source}' onto '{target}':\n",
        commits.len()
    ));
    for commit in commits {
        let short: String = commit.hash.chars().take(7).collect();
        message.push_str(&format!("{short} {}\n", commit.summary));
    }
    message
}

/// Print the `LANDING` status and every commit set
/// (`ArcanistLandEngine.php:1199-1205`).
fn print_landing(
    io: &mut Io,
    sets: &[CommitSet],
    commits: &[LandCommit],
    display_hash: &dyn Fn(&str) -> String,
) {
    io.push_status("LANDING", "These changes will land:");
    for set in sets {
        io.push_out(&render_commit_set_with_ansi(
            set,
            commits,
            display_hash,
            io.ansi,
        ));
    }
}

/// Run the land engine (`ArcanistLandEngine::execute()`,
/// `ArcanistLandEngine.php:1101-1333`).
///
/// `options.refs` selects the sources (else VCS defaults); then onto
/// remote/refs, into state/commit, commit selection, revision discovery,
/// set filtering, the landing list, preview-or-confirm, gate confirms,
/// and the merge/push loop. Conduit is required: PHP resolves the viewer
/// before the engine runs (`ArcanistLandWorkflow.php:329`,
/// `ArcanistRuntime.php:825-832`), so land fails without it instead of
/// landing an unassociated offline set.
fn execute_engine(
    dir: &Path,
    options: &Options,
    vcs: Vcs,
    validated: &Validated,
    io: &mut Io,
) -> Result<(), String> {
    let squash = validated.strategy == Strategy::Squash;
    let mut prep = prepare_engine_commits(dir, options, vcs, io)?;
    let (cli, user_file) = load_options_for_run(options)?;
    let client = client_for_dir_with(
        options.conduit_uri.as_deref(),
        dir,
        user_file.as_deref(),
        &cli,
    )?;
    let mut sets = build_sets(
        dir,
        io,
        vcs,
        validated,
        &mut prep.commits,
        Some(&client),
        &hash_display_for(vcs),
    )?;
    let ansi = io.ansi;
    sets = filter_commit_sets(
        io,
        sets,
        &prep.commits,
        squash,
        options.pick,
        &render_for(vcs, &prep.commits, ansi),
    );
    if !options.preview {
        confirm_implicit_sets(
            io,
            &sets,
            &prep.commits,
            &prep.symbols_display,
            &hash_display_for(vcs),
        )?;
    }
    print_landing(io, &sets, &prep.commits, &hash_display_for(vcs));
    if options.preview {
        io.push_status("PREVIEW", "Completed preview of land operation.");
        return Ok(());
    }
    prompt_for_key(io, land_checks::PROMPT_CONFIRM, "Land these changes?")?;
    confirm_gates_if_online(dir, options, io, Some(&client), &sets, &prep.commits)?;
    match vcs {
        Vcs::Git => land_git(
            dir,
            io,
            options,
            squash,
            &prep.raw_symbols,
            &prep.onto_remote,
            &prep.onto_refs,
            prep.into_commit.as_deref(),
            prep.into_ref.as_deref(),
            &sets,
            &prep.commits,
            Some(&client),
        ),
        Vcs::Hg => land_hg(
            dir,
            io,
            options,
            squash,
            &prep.raw_symbols,
            &prep.onto_remote,
            &prep.onto_refs,
            prep.into_commit.as_deref(),
            prep.hg_branch.as_deref(),
            &sets,
            &prep.commits,
            Some(&client),
        ),
    }
}

/// Selected targets plus the confirmed landing commits.
struct EnginePrep {
    raw_symbols: Vec<String>,
    onto_remote: String,
    onto_refs: Vec<String>,
    into_commit: Option<String>,
    into_ref: Option<String>,
    hg_branch: Option<String>,
    symbols_display: String,
    commits: Vec<LandCommit>,
}

/// Select targets and landing commits: sources and remotes, commit
/// selection and linking, then the commit-count confirm.
///
/// Source symbols resolve before onto/into selection
/// (`ArcanistLandEngine.php:1106-1128`), so an invalid symbol fails
/// before any onto/into output.
fn prepare_engine_commits(
    dir: &Path,
    options: &Options,
    vcs: Vcs,
    io: &mut Io,
) -> Result<EnginePrep, String> {
    // Sources, resolved first.
    let raw_symbols = default_source_symbols(dir, options, vcs, io)?;
    let symbols_display = display_symbols(&raw_symbols);
    let resolved = resolve_sources(dir, io, vcs, &raw_symbols)?;
    // Remotes, then commits.
    let (onto_remote, onto_refs, into_commit, into_ref, hg_branch) =
        select_targets(dir, options, vcs, io, &raw_symbols)?;
    let mut commits = select_source_commits(dir, vcs, &resolved, into_commit.as_deref())?;
    link_commits(&mut commits);
    let into_display = into_display_string(vcs, into_commit.as_deref());
    confirm_commit_count(
        io,
        &commits,
        &symbols_display,
        into_display.as_deref(),
        &hash_display_for(vcs),
    )?;
    Ok(EnginePrep {
        raw_symbols,
        onto_remote,
        onto_refs,
        into_commit,
        into_ref,
        hg_branch,
        symbols_display,
        commits,
    })
}

/// The into commit for display, per VCS.
fn into_display_string(vcs: Vcs, into_commit: Option<&str>) -> Option<String> {
    into_commit.map(|hash| match vcs {
        Vcs::Git => git::display_hash(hash).to_string(),
        Vcs::Hg => display_hash_12(hash),
    })
}

/// Confirm the online revision gates when Conduit is reachable.
fn confirm_gates_if_online(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    online: Option<&arc_rs::conduit::Client>,
    sets: &[CommitSet],
    commits: &[LandCommit],
) -> Result<(), String> {
    let Some(client) = online else {
        return Ok(());
    };
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    let (cli, user_file) = load_options_for_run(options)?;
    let uri = resolve_conduit_uri_with(
        options.conduit_uri.as_deref(),
        dir,
        user_file.as_deref(),
        &cli,
    );
    confirm_online_gates(io, &call, sets, commits, uri.as_deref())
}

/// Display-hash resolver per VCS.
fn hash_display_for(vcs: Vcs) -> Box<dyn Fn(&str) -> String> {
    match vcs {
        Vcs::Git => Box::new(|hash| git::display_hash(hash).to_string()),
        Vcs::Hg => Box::new(display_hash_12),
    }
}

/// Set renderer per VCS (borrows the commit list).
fn render_for<'a>(
    vcs: Vcs,
    commits: &'a [LandCommit],
    ansi: bool,
) -> Box<dyn Fn(&CommitSet) -> String + 'a> {
    match vcs {
        Vcs::Git => Box::new(move |set| {
            render_commit_set_with_ansi(
                set,
                commits,
                &|hash| git::display_hash(hash).to_string(),
                ansi,
            )
        }),
        Vcs::Hg => {
            Box::new(move |set| render_commit_set_with_ansi(set, commits, &display_hash_12, ansi))
        }
    }
}

/// One `%> ` shell-command line (`tsprintf("%>", ...)`, e.g. the
/// recovery commands, `ArcanistGitLandEngine.php:914-916`): bold
/// `$ ...` on TTYs, else the plain rendering.
fn shell_line(command: &str, ansi: bool) -> String {
    if ansi {
        terminal::tsprintf_ansi("%>", &[TVal::Text(command)], true)
            .expect("fixed tsprintf pattern")
            .as_str()
            .to_string()
    } else {
        format!("    $ {command}\n")
    }
}

/// Onto remote/refs, the into commit, the into ref for reconcile
/// (git), and the branch marker (hg).
type TargetSelection = (
    String,
    Vec<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Raw source symbols: explicit refs, else the VCS defaults
/// (`ArcanistLandEngine.php:1106-1109`).
fn default_source_symbols(
    dir: &Path,
    options: &Options,
    vcs: Vcs,
    io: &mut Io,
) -> Result<Vec<String>, String> {
    if !options.refs.is_empty() {
        return Ok(options.refs.clone());
    }
    match vcs {
        Vcs::Git => git_default_symbols(dir, io),
        Vcs::Hg => hg_default_symbols(dir, io),
    }
}

/// Onto remote/refs and the into commit for resolved `raw` symbols, per VCS.
fn select_targets(
    dir: &Path,
    options: &Options,
    vcs: Vcs,
    io: &mut Io,
    raw: &[String],
) -> Result<TargetSelection, String> {
    match vcs {
        Vcs::Git => select_git_targets(dir, options, io, raw),
        Vcs::Hg => select_hg_targets(dir, options, io, raw),
    }
}

/// Git onto remote/refs and the into commit for resolved `raw` symbols.
fn select_git_targets(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    raw: &[String],
) -> Result<TargetSelection, String> {
    let onto_remote = git_select_onto_remote(dir, io, options, raw)?;
    let onto_refs = git_select_onto_refs(dir, io, options, raw)?;
    git_confirm_onto_refs(dir, io, &onto_remote, &onto_refs, options.hold)?;
    let into_state = select_into_state(io, options, &onto_remote, &onto_refs, &|remote| {
        git::remote_fetch_uri(dir, remote).is_some()
    })?;
    let into_ref = match &into_state {
        IntoState::Remote { name, .. } => Some(name.clone()),
        _ => None,
    };
    let into_commit = git_select_into_commit(dir, io, &into_state, options.into.as_deref())?;
    Ok((onto_remote, onto_refs, into_commit, into_ref, None))
}

/// Mercurial onto remote/refs and the into commit for resolved `raw` symbols.
fn select_hg_targets(
    dir: &Path,
    options: &Options,
    io: &mut Io,
    _raw: &[String],
) -> Result<TargetSelection, String> {
    let onto_remote = hg_select_onto_remote(dir, io, options)?;
    let onto_refs = hg_select_onto_refs(dir, io, options)?;
    let branch = hg_confirm_onto_refs(dir, io, &onto_remote, &onto_refs, options.hold)?;
    let into_state = select_into_state(io, options, &onto_remote, &onto_refs, &|remote| {
        hg::ok(dir, &["paths", remote])
    })?;
    let into_commit = hg_select_into_commit(dir, io, &into_state, options.into.as_deref())?;
    Ok((onto_remote, onto_refs, into_commit, None, branch))
}

/// Resolve source symbols to commits, per VCS.
fn resolve_sources(
    dir: &Path,
    io: &mut Io,
    vcs: Vcs,
    raw: &[String],
) -> Result<Vec<GitSymbol>, String> {
    let resolved = match vcs {
        Vcs::Git => git_resolve_symbols(dir, raw)?,
        Vcs::Hg => hg_resolve_symbols(dir, io, raw)?,
    };
    Ok(raw
        .iter()
        .cloned()
        .zip(resolved)
        .map(|(symbol, commit)| GitSymbol { symbol, commit })
        .collect())
}

/// Select landing commits, per VCS.
fn select_source_commits(
    dir: &Path,
    vcs: Vcs,
    symbols: &[GitSymbol],
    into_commit: Option<&str>,
) -> Result<Vec<LandCommit>, String> {
    match vcs {
        Vcs::Git => git_select_commits(dir, symbols, into_commit),
        Vcs::Hg => hg_select_commits(dir, symbols, into_commit),
    }
}

/// Build landing sets: online discovery and grouping, or the offline
/// single unassociated set.
fn build_sets(
    dir: &Path,
    io: &mut Io,
    vcs: Vcs,
    validated: &Validated,
    commits: &mut [LandCommit],
    online: Option<&arc_rs::conduit::Client>,
    display_hash: &dyn Fn(&str) -> String,
) -> Result<Vec<CommitSet>, String> {
    let Some(client) = online else {
        let indexes: Vec<usize> = (0..commits.len()).collect();
        return Ok(vec![CommitSet {
            revision: None,
            commits: indexes,
            is_pick: false,
        }]);
    };
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    let forced = match (&validated.revision, &validated.revision_raw) {
        (Some(symbol), Some(raw)) => Some(resolve_forced_revision_with_raw(&call, symbol, raw)?),
        (Some(symbol), None) => Some(resolve_forced_revision(&call, symbol)?),
        (None, _) => None,
    };
    let message_of = commit_message_reader(dir, vcs);
    let hashes_of = commit_hash_reader(dir, vcs);
    load_revision_refs(io, &call, commits, &message_of, &hashes_of, forced.as_ref())?;
    group_commit_sets(io, commits, display_hash)
}

/// Reads one commit message.
type MessageReader<'a> = Box<dyn Fn(&str) -> Result<String, String> + 'a>;

/// Full-message reader per VCS: `git log --format=%B` or
/// `hg log --template {desc}`.
fn commit_message_reader(dir: &Path, vcs: Vcs) -> MessageReader<'_> {
    match vcs {
        Vcs::Git => {
            Box::new(|hash| git::stdout_text(dir, &["log", "-n1", "--format=%B", hash, "--"]))
        }
        Vcs::Hg => {
            Box::new(|hash| hg::stdout_text(dir, &["log", "--rev", hash, "--template", "{desc}"]))
        }
    }
}

/// Reads `(hash-type, hash)` pairs for one commit (owned closure).
type HashReader<'a> = Box<dyn Fn(&str) -> Result<Vec<(String, String)>, String> + 'a>;

/// Commit-hash-pair reader per VCS: `gtcm`/`gttr` under git, `hgcm`
/// under Mercurial.
fn commit_hash_reader(dir: &Path, vcs: Vcs) -> HashReader<'_> {
    match vcs {
        Vcs::Git => Box::new(|hash| {
            let tree = git::stdout_text(dir, &["rev-parse", &format!("{hash}^{{tree}}")])?;
            Ok(vec![
                ("gtcm".to_string(), hash.to_string()),
                ("gttr".to_string(), tree),
            ])
        }),
        Vcs::Hg => Box::new(|hash| Ok(vec![("hgcm".to_string(), hash.to_string())])),
    }
}

/// Confirm the online revision gates: load parents, builds, and the
/// viewer identity, evaluate every gate, then confirm each finding
/// interactively. `conduit_uri` absolutizes buildable URIs for display.
fn confirm_online_gates(
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
    sets: &[CommitSet],
    commits: &[LandCommit],
    conduit_uri: Option<&str>,
) -> Result<(), String> {
    let revisions: Vec<land_checks::RevisionView> =
        sets.iter().filter_map(|set| set.revision.clone()).collect();
    // A viewer failure throws (PHP `ArcanistRuntime::getViewer()`); it
    // must not fall back to a revision author and no-op the unauthored
    // gate.
    let viewer = fetch_viewer_identity(call)?;
    let links = fetch_parent_links(call, &revisions)?;
    let landing: Vec<&str> = revisions.iter().map(|rev| rev.phid.as_str()).collect();
    let builds = fetch_build_views(call, &revisions, conduit_uri)?;
    let findings = land_checks::evaluate_gates(
        &revisions,
        &viewer.phid,
        &links,
        &landing,
        &builds,
        Some(commits.len()),
    );
    // Author monograms for the unauthored banner, loaded only when that
    // gate fires (`ArcanistLandEngine.php:295-299`).
    let mut author_phids: Vec<String> = Vec::new();
    if findings
        .iter()
        .any(|finding| finding.prompt == land_checks::PROMPT_UNAUTHORED)
    {
        for index in land_checks::unauthored_indexes(&revisions, &viewer.phid) {
            author_phids.push(revisions[index].author_phid.clone());
        }
    }
    let authors = fetch_user_monograms(call, &author_phids)?;
    confirm_revision_gates_with_authors(
        io, &revisions, &viewer, &links, &builds, &findings, &authors,
    )
}

/// Saved git state (`ArcanistRepositoryLocalState` plus
/// `ArcanistGitLocalState.php:6-16`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLocalState {
    /// Branch name at save time, or `None` on a detached HEAD.
    pub local_ref: Option<String>,
    /// `HEAD` at save time.
    pub local_commit: String,
    /// Upstream path of the saved branch, or `None` on a detached HEAD.
    pub local_path: Option<git::UpstreamPath>,
    /// A stash entry was saved and must be applied back.
    pub stashed: bool,
}

/// Saved Mercurial state (`ArcanistMercurialLocalState.php:6-12`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgLocalState {
    /// Working-copy parent at save time.
    pub local_commit: String,
    /// Active branch at save time.
    pub local_branch: String,
    /// Active bookmark at save time, if any.
    pub local_bookmark: Option<String>,
    /// Shelve name, when changes were shelved.
    pub stash: Option<String>,
}

/// Uncommitted-status flags per VCS (same values both sides, from
/// `ArcanistRepositoryAPI.php`; kept separate so each side reads its
/// own module's constants).
#[derive(Debug, Clone, Copy)]
struct ChangeFlags {
    conflict: u32,
    externals: u32,
    untracked: u32,
    unstaged: u32,
    uncommitted: u32,
}

/// Git status flags (`git::FLAG_*`).
const GIT_CHANGE_FLAGS: ChangeFlags = ChangeFlags {
    conflict: git::FLAG_CONFLICT,
    externals: git::FLAG_EXTERNALS,
    untracked: git::FLAG_UNTRACKED,
    unstaged: git::FLAG_UNSTAGED,
    uncommitted: git::FLAG_UNCOMMITTED,
};

/// Mercurial status flags (`hg::FLAG_*`).
const HG_CHANGE_FLAGS: ChangeFlags = ChangeFlags {
    conflict: hg::FLAG_CONFLICT,
    externals: hg::FLAG_EXTERNALS,
    untracked: hg::FLAG_UNTRACKED,
    unstaged: hg::FLAG_UNSTAGED,
    uncommitted: hg::FLAG_UNCOMMITTED,
};

/// Sorted paths carrying `mask` in an uncommitted-status map
/// (`getUntrackedChanges()` and siblings,
/// `ArcanistRepositoryAPI.php:158-184`).
fn status_paths(status: &BTreeMap<String, u32>, mask: u32) -> Vec<String> {
    status
        .iter()
        .filter(|(_, flags)| **flags & mask != 0)
        .map(|(path, _)| path.clone())
        .collect()
}

/// Uncommitted changes split into the display lists
/// (`saveLocalState()`, `ArcanistRepositoryLocalState.php:59-134`).
#[derive(Debug, Default, PartialEq, Eq)]
struct ChangeLists {
    conflicts: Vec<String>,
    externals: Vec<String>,
    untracked: Vec<String>,
    unstaged: Vec<String>,
    uncommitted: Vec<String>,
}

/// Split a status map: unstaged changes exclude externals, and
/// uncommitted changes exclude both (`:107-134`).
fn change_lists(status: &BTreeMap<String, u32>, flags: &ChangeFlags) -> ChangeLists {
    let conflicts = status_paths(status, flags.conflict);
    let externals = status_paths(status, flags.externals);
    let untracked = status_paths(status, flags.untracked);
    let unstaged: Vec<String> = status_paths(status, flags.unstaged)
        .into_iter()
        .filter(|path| !externals.contains(path))
        .collect();
    let uncommitted: Vec<String> = status_paths(status, flags.uncommitted)
        .into_iter()
        .filter(|path| !unstaged.contains(path) && !externals.contains(path))
        .collect();
    ChangeLists {
        conflicts,
        externals,
        untracked,
        unstaged,
        uncommitted,
    }
}

/// Print a local-state banner: the title, the wrapped message, the
/// working-copy path, and the changed-file lists (`saveLocalState()`,
/// `:78-96`).
fn push_state_banner(
    io: &mut Io,
    title: &str,
    message: &str,
    dir: &Path,
    lists: &[(&str, &[String])],
) {
    if io.ansi {
        let dir_text = dir.display().to_string();
        let working_copy = terminal::tsprintf_ansi(
            "    %s: %s\n",
            &[TVal::Text("Working Copy"), TVal::Text(&dir_text)],
            true,
        )
        .expect("fixed tsprintf pattern");
        let text = terminal::tsprintf_ansi(
            "\n%!\n%W\n\n%s\n",
            &[
                TVal::Text(title),
                TVal::Text(message),
                TVal::Terminal(&working_copy),
            ],
            true,
        )
        .expect("fixed tsprintf pattern");
        io.push_out(text.as_str());
        io.push_out(&render_file_lists(lists, true));
    } else {
        io.push_out(&format!(
            "\n** {title} **\n{}\n\n    Working Copy: {}\n\n{}",
            console_wrap(message, 0, true),
            dir.display(),
            render_file_lists(lists, false),
        ));
    }
}

/// Render changed-file lists (`printFileLists()`, `:372-411`): each
/// non-empty list prints its title, a blank line, and its indented
/// paths; lists are `"\n\n"` separated with a trailing blank line.
fn render_file_lists(lists: &[(&str, &[String])], ansi: bool) -> String {
    let mut out = String::new();
    let mut first = true;
    for (title, paths) in lists {
        if paths.is_empty() {
            continue;
        }
        if !first {
            out.push_str("\n\n");
        }
        first = false;
        out.push_str(&terminal::console_format("%s\n\n", &[title], ansi));
        for path in *paths {
            out.push_str(&terminal::console_format("    %s\n", &[path], ansi));
        }
    }
    out.push('\n');
    out
}

/// Fail on unresolved merge conflicts (`:100-107`). The banner lists
/// conflicts plus the untracked/unstaged/uncommitted files, like PHP's
/// `printFileLists($conflicts, $untracked, $unstaged, $uncommitted)`.
fn gate_merge_conflicts(io: &mut Io, dir: &Path, lists: &ChangeLists) -> Result<(), String> {
    if lists.conflicts.is_empty() {
        return Ok(());
    }
    push_state_banner(
        io,
        "MERGE CONFLICTS",
        "You have merge conflicts in this working copy.",
        dir,
        &[
            ("Merge conflicts in working copy:", &lists.conflicts),
            ("Untracked changes in working copy:", &lists.untracked),
            ("Unstaged changes in working copy:", &lists.unstaged),
            ("Uncommitted changes in working copy:", &lists.uncommitted),
        ],
    );
    Err("Resolve merge conflicts before proceeding.".to_string())
}

/// Confirm past dirty externals (`:109-134`): the message paragraph, a
/// blank line, and the unwrapped 6-space-indented list, then a
/// default-yes confirm (decline aborts).
fn confirm_dirty_externals(io: &mut Io, externals: &[String]) -> Result<(), String> {
    let count = diff_local::format_count(externals.len());
    let message = format!("{count} submodule(s) have uncommitted or untracked changes:");
    io.push_out(&terminal::console_format("%s\n\n", &[&message], io.ansi));
    for external in externals {
        io.push_out(&terminal::console_format(
            "      %s\n",
            &[external],
            io.ansi,
        ));
    }
    let prompt = format!("Ignore the changes to these {count} submodule(s) and continue?");
    if console_confirm(io, &prompt, false)? {
        Ok(())
    } else {
        Err(ABORT.to_string())
    }
}

/// Print the uncommitted-changes banner with file lists and ignore hints
/// (`:137-155`). Returns true when changes need stashing. Ignore hints
/// print only with untracked files (`:142-148`).
fn push_uncommitted_banner(
    io: &mut Io,
    dir: &Path,
    lists: &ChangeLists,
    ignore_hint: &str,
) -> bool {
    let total = lists.untracked.len() + lists.unstaged.len() + lists.uncommitted.len();
    if total == 0 {
        return false;
    }
    push_state_banner(
        io,
        "UNCOMMITTED CHANGES",
        "You have uncommitted changes in this working copy.",
        dir,
        &[
            ("Untracked changes in working copy:", &lists.untracked),
            ("Unstaged changes in working copy:", &lists.unstaged),
            ("Uncommitted changes in working copy:", &lists.uncommitted),
        ],
    );
    if lists.untracked.is_empty() {
        return true;
    }
    io.push_out(&render_hint_with_ansi(ignore_hint, io.ansi));
    io.push_out("\n");
    true
}

/// Save git working-copy state, stashing uncommitted changes after
/// prompting (`saveLocalState()`, `:59-231`, with the git halves).
///
/// The `SAVE STATE` trace line is not printed: PHP only shows trace
/// output with tracing enabled. Failures before capture (conflicts,
/// stashing) propagate without restoring, like PHP.
pub fn save_local_state_git(dir: &Path, io: &mut Io) -> Result<GitLocalState, String> {
    let status = git::uncommitted_status(dir)?;
    let lists = change_lists(&status, &GIT_CHANGE_FLAGS);
    gate_merge_conflicts(io, dir, &lists)?;
    if !lists.externals.is_empty() {
        confirm_dirty_externals(io, &lists.externals)?;
    }
    let mut stashed = false;
    if push_uncommitted_banner(
        io,
        dir,
        &lists,
        "To configure Git to ignore certain files in this working copy, add the file paths to \
         \".git/info/exclude\".",
    ) {
        prompt_for_key(
            io,
            land_checks::PROMPT_STASH,
            "Stash these changes and continue?",
        )?;
        git::stdout_text(dir, &["stash", "push", "--include-untracked", "--"])?;
        io.push_status("SAVE STASH", "Saved uncommitted changes from working copy.");
        stashed = true;
    }
    let local_commit = git::working_copy_revision(dir)?;
    let branch = git::branch_name(dir)?;
    let local_path = match &branch {
        Some(name) => Some(git::get_path_to_upstream(dir, name)?),
        None => None,
    };
    Ok(GitLocalState {
        local_ref: branch,
        local_commit,
        local_path,
        stashed,
    })
}

/// Save Mercurial working-copy state, driving `hg` through the system
/// binary (see [`save_local_state_hg_with`]).
pub fn save_local_state_hg(dir: &Path, io: &mut Io) -> Result<HgLocalState, String> {
    save_local_state_hg_with(dir, &hg::HgCommand::system(), io)
}

/// Save Mercurial working-copy state, stashing uncommitted changes
/// after prompting (`saveLocalState()`, `:59-231`, with the Mercurial
/// halves), driving `hg` through `hg_cmd` (test seam).
pub fn save_local_state_hg_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
) -> Result<HgLocalState, String> {
    let status = hg::get_uncommitted_status_with(dir, hg_cmd)?;
    let lists = change_lists(&status, &HG_CHANGE_FLAGS);
    gate_merge_conflicts(io, dir, &lists)?;
    if !lists.externals.is_empty() {
        confirm_dirty_externals(io, &lists.externals)?;
    }
    let mut stash = None;
    if push_uncommitted_banner(
        io,
        dir,
        &lists,
        "To configure Mercurial to ignore certain files in the working copy, add them to \
         \".hgignore\".",
    ) {
        if !hg::mercurial_feature_supported_with(dir, hg_cmd, "shelve").unwrap_or(false) {
            return Err(
                "You can not continue with uncommitted changes. Commit or discard them before \
                 proceeding."
                    .to_string(),
            );
        }
        prompt_for_key(
            io,
            land_checks::PROMPT_STASH,
            "Stash these changes and continue?",
        )?;
        let name = new_shelve_name();
        hg::exec_with_extension_with(
            dir,
            hg_cmd,
            "shelve",
            &["shelve", "--unknown", "--name", &name, "--"],
            None,
        )?;
        io.push_status("SHELVE", "Shelving uncommitted changes from working copy.");
        stash = Some(name);
    }
    let local_commit = hg::stdout_text_with(
        dir,
        hg_cmd,
        &["log", "-l", "1", "--template", "{node}", "-r", "."],
    )?;
    let branch = hg::stdout_text_with(dir, hg_cmd, &["branch"])?;
    let bookmarks = hg::stdout_text_with(dir, hg_cmd, &["bookmarks"]).unwrap_or_default();
    let local_bookmark = parse_hg_bookmarks(&bookmarks)
        .into_iter()
        .find(|(_, active)| *active)
        .map(|(name, _)| name);
    Ok(HgLocalState {
        local_commit: local_commit.trim().to_string(),
        local_branch: branch.trim().to_string(),
        local_bookmark,
        stash,
    })
}

/// `arc-` plus 12 random `[a-z2-7]` characters, like
/// `Filesystem::readRandomCharacters(12)`
/// (`src/filesystem/Filesystem.php:538-558`).
fn new_shelve_name() -> String {
    const MAP: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ (std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut state = seed | 1;
    let mut name = String::from("arc-");
    for _ in 0..12 {
        // xorshift64*; the high 5 bits index the map, like PHP.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let byte = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        name.push(MAP[(byte >> 59) as usize] as char);
    }
    name
}

/// Restore saved git state (`restoreLocalState()`, `:248-282`): check
/// the saved ref or commit back out, update submodules, then apply the
/// stash back.
pub fn restore_local_state_git(
    dir: &Path,
    io: &mut Io,
    state: &GitLocalState,
) -> Result<(), String> {
    match &state.local_ref {
        Some(reference) => {
            io.push_status(
                "LOAD STATE",
                &format!(
                    "Restoring local state (to ref \"{reference}\" at commit \"{}\").",
                    git::display_hash(&state.local_commit)
                ),
            );
            git::stdout_text(
                dir,
                &["checkout", "-B", reference, &state.local_commit, "--"],
            )?;
        }
        None => {
            io.push_status(
                "LOAD STATE",
                &format!(
                    "Restoring local state (to detached commit \"{}\").",
                    git::display_hash(&state.local_commit)
                ),
            );
            git::stdout_text(dir, &["checkout", &state.local_commit, "--"])?;
        }
    }
    git::stdout_text(dir, &["submodule", "update", "--init", "--recursive"])?;
    apply_stash_git(dir, io, state)
}

/// Apply the saved stash back and drop it (`applyStash()` plus
/// `restoreStash()`/`discardStash()`, `:294-327`).
fn apply_stash_git(dir: &Path, io: &mut Io, state: &GitLocalState) -> Result<(), String> {
    if !state.stashed {
        return Ok(());
    }
    io.push_status(
        "LOAD STASH",
        "Restoring uncommitted changes to working copy.",
    );
    git::stdout_text(dir, &["stash", "apply"])?;
    git::stdout_text(dir, &["stash", "drop"])?;
    Ok(())
}

/// Discard saved git state (`discardLocalState()`, `:334-358`): apply
/// the stash back; on failure restore first, then report the original
/// error.
pub fn discard_local_state_git(
    dir: &Path,
    io: &mut Io,
    state: &GitLocalState,
) -> Result<(), String> {
    match apply_stash_git(dir, io, state) {
        Ok(()) => Ok(()),
        Err(err) => {
            restore_local_state_git(dir, io, state)?;
            Err(err)
        }
    }
}

/// Restore commands for display (`newRestoreCommandsForDisplay()`,
/// `ArcanistGitLocalState.php:86-108`).
pub fn git_restore_commands(state: &GitLocalState) -> Vec<String> {
    match &state.local_ref {
        Some(reference) => vec![format!(
            "git checkout -B {reference} {} --",
            git::display_hash(&state.local_commit)
        )],
        None => vec![format!(
            "git checkout {} --",
            git::display_hash(&state.local_commit)
        )],
    }
}

/// Restore saved Mercurial state, driving `hg` through the system
/// binary (see [`restore_local_state_hg_with`]).
pub fn restore_local_state_hg(dir: &Path, io: &mut Io, state: &HgLocalState) -> Result<(), String> {
    restore_local_state_hg_with(dir, &hg::HgCommand::system(), io, state)
}

/// Restore saved Mercurial state (`restoreLocalState()`, `:248-282`,
/// with `ArcanistMercurialLocalState.php:90-115`), driving `hg` through
/// `hg_cmd` (test seam).
pub fn restore_local_state_hg_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    state: &HgLocalState,
) -> Result<(), String> {
    match &state.local_bookmark {
        Some(bookmark) => io.push_status(
            "LOAD STATE",
            &format!(
                "Restoring local state (at \"{}\" on branch \"{}\", bookmarked as \
                 \"{bookmark}\").",
                display_hash_12(&state.local_commit),
                state.local_branch
            ),
        ),
        None => io.push_status(
            "LOAD STATE",
            &format!(
                "Restoring local state (at \"{}\" on branch \"{}\").",
                display_hash_12(&state.local_commit),
                state.local_branch
            ),
        ),
    }
    hg::stdout_text_with(dir, hg_cmd, &["update", "--", &state.local_commit])?;
    hg::stdout_text_with(
        dir,
        hg_cmd,
        &["branch", "--force", "--", &state.local_branch],
    )?;
    if let Some(bookmark) = &state.local_bookmark {
        hg::stdout_text_with(dir, hg_cmd, &["bookmark", "--force", "--", bookmark])?;
    }
    apply_shelve_with(dir, hg_cmd, io, state)
}

/// Unshelve saved changes and delete the shelf (`applyStash()` plus
/// `restoreStash()`/`discardStash()`, `:294-327`).
fn apply_shelve_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    state: &HgLocalState,
) -> Result<(), String> {
    let Some(name) = &state.stash else {
        return Ok(());
    };
    io.push_status("UNSHELVE", "Restoring uncommitted changes to working copy.");
    hg::exec_with_extension_with(
        dir,
        hg_cmd,
        "shelve",
        &["unshelve", "--keep", "--name", name, "--"],
        None,
    )?;
    hg::exec_with_extension_with(
        dir,
        hg_cmd,
        "shelve",
        &["shelve", "--delete", name, "--"],
        None,
    )?;
    Ok(())
}

/// Discard saved Mercurial state, driving `hg` through the system
/// binary (see [`discard_local_state_hg_with`]).
pub fn discard_local_state_hg(dir: &Path, io: &mut Io, state: &HgLocalState) -> Result<(), String> {
    discard_local_state_hg_with(dir, &hg::HgCommand::system(), io, state)
}

/// Discard saved Mercurial state (`discardLocalState()`, `:334-358`),
/// driving `hg` through `hg_cmd` (test seam).
pub fn discard_local_state_hg_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    state: &HgLocalState,
) -> Result<(), String> {
    match apply_shelve_with(dir, hg_cmd, io, state) {
        Ok(()) => Ok(()),
        Err(err) => {
            restore_local_state_hg_with(dir, hg_cmd, io, state)?;
            Err(err)
        }
    }
}

/// Restore commands for display (`newRestoreCommandsForDisplay()`,
/// `ArcanistMercurialLocalState.php:134-153`).
pub fn hg_restore_commands(state: &HgLocalState) -> Vec<String> {
    let mut commands = vec![
        format!("hg update -- {}", display_hash_12(&state.local_commit)),
        format!("hg branch --force -- {}", state.local_branch),
    ];
    if let Some(bookmark) = &state.local_bookmark {
        commands.push(format!("hg bookmark --force -- {bookmark}"));
    }
    commands
}

/// Run the git merge/push loop
/// (`ArcanistLandEngine.php:1234-1332`, git halves).
///
/// Saves local state first, merges every set (pushing per
/// `--hold`/`--incremental`), cascades and prunes after each push,
/// then reconciles (or prints hold instructions) and `DONE`. Failures
/// restore the saved state before propagating.
#[allow(clippy::too_many_arguments)]
fn land_git(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    squash: bool,
    raw_symbols: &[String],
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: Option<&str>,
    into_ref: Option<&str>,
    sets: &[CommitSet],
    commits: &[LandCommit],
    online: Option<&arc_rs::conduit::Client>,
) -> Result<(), String> {
    let state = save_local_state_git(dir, io)?;
    let outcome = land_git_inner(
        dir,
        io,
        options,
        squash,
        raw_symbols,
        onto_remote,
        onto_refs,
        into_commit,
        into_ref,
        sets,
        commits,
        online,
        &state,
    );
    match outcome {
        Ok(()) => Ok(()),
        Err(err) => {
            restore_local_state_git(dir, io, &state)?;
            Err(err)
        }
    }
}

/// The git merge/push loop proper (see [`land_git`]).
#[allow(clippy::too_many_arguments)]
fn land_git_inner(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    squash: bool,
    raw_symbols: &[String],
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: Option<&str>,
    into_ref: Option<&str>,
    sets: &[CommitSet],
    commits: &[LandCommit],
    online: Option<&arc_rs::conduit::Client>,
    state: &GitLocalState,
) -> Result<(), String> {
    let mut cursor = into_commit.map(str::to_string);
    let mut need_cascade: Vec<CommitSet> = Vec::new();
    let mut need_prune: Vec<CommitSet> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    // Merged-but-unpushed sets exist once a merge succeeds
    // (`:1247`); pushing clears the flag (`:1261`). The conflict hint
    // reads it.
    let mut has_unpushed = false;
    let last = sets.len().saturating_sub(1);
    let client_call = online.map(client_call_boxed);
    let messages = prefetch_set_messages(
        raw_symbols,
        onto_refs,
        commits,
        client_call.as_deref(),
        sets,
    )?;
    // No `--strategy`, no `arc.land.strategy`, but legacy
    // `history.immutable` set: the default is ambiguous (git only,
    // checked at merge time like `executeMerge()`).
    confirm_legacy_strategy_configuration(dir, options, io)?;
    for (index, set) in sets.iter().enumerate() {
        need_prune.push(set.clone());
        need_cascade.push(set.clone());
        let message = messages[index].clone();
        let message_of = move |_: &CommitSet| Ok(message.clone());
        cursor = Some(git_execute_merge(
            dir,
            io,
            set,
            commits,
            cursor.as_deref(),
            squash,
            has_unpushed,
            &message_of,
        )?);
        has_unpushed = true;
        if git_should_push(options, index, last) {
            git_push_and_update(
                dir,
                io,
                onto_remote,
                onto_refs,
                cursor_or(cursor.as_deref()),
                squash,
                options.keep_branches,
                &mut need_cascade,
                &mut need_prune,
                &mut deleted,
                commits,
            )?;
            has_unpushed = false;
        }
    }
    git_finish_landing(
        dir,
        io,
        options.hold,
        state,
        onto_remote,
        onto_refs,
        cursor_or(cursor.as_deref()),
        into_ref,
        &deleted,
    )
}

/// True when the loop pushes after set `index`: never under `--hold`,
/// after every set under `--incremental`, else after the last set only.
fn git_should_push(options: &Options, index: usize, last: usize) -> bool {
    if options.hold {
        false
    } else if options.incremental {
        true
    } else {
        index == last
    }
}

/// Push the merged cursor (Perforce submit under `p4`), then cascade
/// state and prune branches.
#[allow(clippy::too_many_arguments)]
fn git_push_and_update(
    dir: &Path,
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    cursor: &str,
    squash: bool,
    keep_branches: bool,
    need_cascade: &mut Vec<CommitSet>,
    need_prune: &mut Vec<CommitSet>,
    deleted: &mut Vec<String>,
    commits: &[LandCommit],
) -> Result<(), String> {
    if git_is_perforce_remote(dir, onto_remote) {
        git_p4_submit(dir, io, onto_remote, cursor)?;
    } else {
        git_push_change(dir, io, onto_remote, onto_refs, cursor)?;
    }
    need_cascade.reverse();
    for cascade_set in need_cascade.iter() {
        git_cascade_state(dir, io, cascade_set, commits, cursor, squash);
    }
    need_cascade.clear();
    if !keep_branches {
        deleted.extend(git_prune_branches(dir, io, need_prune, commits));
        need_prune.clear();
    }
    Ok(())
}

/// Finish a git land: hold instructions under `--hold`, else reconcile
/// (Perforce or plain) and `DONE`.
#[allow(clippy::too_many_arguments)]
fn git_finish_landing(
    dir: &Path,
    io: &mut Io,
    hold: bool,
    state: &GitLocalState,
    onto_remote: &str,
    onto_refs: &[String],
    cursor: &str,
    into_ref: Option<&str>,
    deleted: &[String],
) -> Result<(), String> {
    if hold {
        let restore = git_restore_commands(state);
        let perforce = git_is_perforce_remote(dir, onto_remote);
        git_hold_text(io, onto_remote, onto_refs, cursor, &restore, perforce);
        discard_local_state_git(dir, io, state)?;
        return Ok(());
    }
    if git_is_perforce_remote(dir, onto_remote) {
        git_reconcile_perforce(dir, io, state, into_ref, onto_refs, deleted, cursor)?;
    } else {
        git_reconcile(
            dir,
            io,
            state,
            cursor,
            into_ref,
            onto_remote,
            onto_refs,
            deleted,
        )?;
    }
    io.push_status_with_ansi("DONE", "Landed changes.", "green", io.ansi);
    Ok(())
}

/// Fail when the merge strategy is ambiguous under legacy
/// `history.immutable` configuration
/// (`ArcanistGitLandEngine::confirmLegacyStrategyConfiguration()`,
/// `:1556-1600`).
///
/// With no `--strategy` and no `arc.land.strategy`, a truthy merged
/// `history.immutable` value prints the `MERGE STRATEGY IS AMBIGUOUS`
/// banner and fails (a usage error in PHP).
fn confirm_legacy_strategy_configuration(
    dir: &Path,
    options: &Options,
    io: &mut Io,
) -> Result<(), String> {
    if options.strategy.is_some() {
        return Ok(());
    }
    let strategy_config = land_config_value(options, dir, "arc.land.strategy")?
        .as_ref()
        .and_then(parse_string_config);
    if strategy_config
        .as_deref()
        .is_some_and(|name| !name.is_empty())
    {
        return Ok(());
    }
    let immutable = land_config_value(options, dir, "history.immutable")?;
    if !immutable.as_ref().is_some_and(is_truthy_config) {
        return Ok(());
    }
    push_banner(
        io,
        "MERGE STRATEGY IS AMBIGUOUS",
        "See <https://secure.phabricator.com/T13547>. The default merge strategy under Git with \
         \"history.immutable\" has changed from \"merge\" to \"squash\". Your configuration is \
         ambiguous under this behavioral change. (Use \"--strategy\" or configure \
         \"arc.land.strategy\" to bypass this check.)",
    );
    Err("Desired merge strategy is ambiguous, choose an explicit strategy.".to_string())
}

/// PHP `(bool)` cast for merged config values.
fn is_truthy_config(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        serde_json::Value::String(text) => !text.is_empty() && text != "0",
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}

/// Fetch every set's landed message up front, in set order
/// (`ArcanistLandEngine.php:526-534`): sets with a revision read
/// `differential.getcommitmessage` over `call`; sets without one (or
/// with no `call`) synthesize the local message. A fetch failure aborts
/// before any merge runs. Returns one message per set, aligned with
/// `sets`.
fn prefetch_set_messages(
    raw_symbols: &[String],
    onto_refs: &[String],
    commits: &[LandCommit],
    call: Option<&diff_submit::ConduitCall<'_>>,
    sets: &[CommitSet],
) -> Result<Vec<String>, String> {
    let mut messages = Vec::with_capacity(sets.len());
    for set in sets {
        let members: Vec<LandCommit> = set
            .commits
            .iter()
            .map(|index| commits[*index].clone())
            .collect();
        messages.push(match (call, &set.revision) {
            (Some(call), Some(revision)) => fetch_commit_message(call, revision.id)?,
            _ => offline_message(raw_symbols, onto_refs, &members),
        });
    }
    Ok(messages)
}

/// Adapt a client to a boxed [`diff_submit::ConduitCall`].
fn client_call_boxed(client: &arc_rs::conduit::Client) -> Box<diff_submit::ConduitCall<'_>> {
    Box::new(move |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    })
}

/// Unwrap a merge cursor that must exist by now.
fn cursor_or(cursor: Option<&str>) -> &str {
    cursor.unwrap_or_default()
}

/// Run the Mercurial merge/push loop (base loop plus hg halves).
#[allow(clippy::too_many_arguments)]
fn land_hg(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    squash: bool,
    raw_symbols: &[String],
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: Option<&str>,
    branch_marker: Option<&str>,
    sets: &[CommitSet],
    commits: &[LandCommit],
    online: Option<&arc_rs::conduit::Client>,
) -> Result<(), String> {
    land_hg_with(
        dir,
        &hg::HgCommand::system(),
        io,
        options,
        squash,
        raw_symbols,
        onto_remote,
        onto_refs,
        into_commit,
        branch_marker,
        sets,
        commits,
        online,
    )
}

/// Run the Mercurial merge/push loop, driving `hg` through `hg_cmd`
/// (test seam).
///
/// Saves local state first; failures restore the saved state before
/// propagating (`ArcanistLandEngine.php:1227-1332`, hg halves).
#[allow(clippy::too_many_arguments)]
pub fn land_hg_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    options: &Options,
    squash: bool,
    raw_symbols: &[String],
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: Option<&str>,
    branch_marker: Option<&str>,
    sets: &[CommitSet],
    commits: &[LandCommit],
    online: Option<&arc_rs::conduit::Client>,
) -> Result<(), String> {
    let state = save_local_state_hg_with(dir, hg_cmd, io)?;
    let outcome = land_hg_inner_with(
        dir,
        hg_cmd,
        io,
        options,
        squash,
        raw_symbols,
        onto_remote,
        onto_refs,
        into_commit,
        branch_marker,
        sets,
        commits,
        online,
        &state,
    );
    match outcome {
        Ok(()) => Ok(()),
        Err(err) => {
            restore_local_state_hg_with(dir, hg_cmd, io, &state)?;
            Err(err)
        }
    }
}

/// The Mercurial merge/push loop proper (see [`land_hg_with`]).
#[allow(clippy::too_many_arguments)]
fn land_hg_inner_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    io: &mut Io,
    options: &Options,
    squash: bool,
    raw_symbols: &[String],
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: Option<&str>,
    branch_marker: Option<&str>,
    sets: &[CommitSet],
    commits: &[LandCommit],
    online: Option<&arc_rs::conduit::Client>,
    state: &HgLocalState,
) -> Result<(), String> {
    let push_bookmarks: Vec<String> = onto_refs
        .iter()
        .filter(|name| Some(name.as_str()) != branch_marker)
        .cloned()
        .collect();
    let mut cursor = into_commit.map(str::to_string);
    let mut need_cascade: Vec<CommitSet> = Vec::new();
    let mut need_prune: Vec<CommitSet> = Vec::new();
    let last = sets.len().saturating_sub(1);
    let client_call = online.map(client_call_boxed);
    let messages = prefetch_set_messages(
        raw_symbols,
        onto_refs,
        commits,
        client_call.as_deref(),
        sets,
    )?;
    for (index, set) in sets.iter().enumerate() {
        need_prune.push(set.clone());
        need_cascade.push(set.clone());
        let message = messages[index].clone();
        let message_of = move |_: &CommitSet| Ok(message.clone());
        let merged = hg_execute_merge_with(
            dir,
            hg_cmd,
            io,
            set,
            commits,
            cursor.as_deref(),
            squash,
            branch_marker,
            &message_of,
        )?;
        cursor = Some(merged);
        let should_push = if options.hold {
            false
        } else if options.incremental {
            true
        } else {
            index == last
        };
        if should_push {
            hg_push_change_with(
                dir,
                hg_cmd,
                io,
                onto_remote,
                &push_bookmarks,
                cursor_or(cursor.as_deref()),
            )?;
            need_cascade.reverse();
            for cascade_set in &need_cascade {
                hg_cascade_state_with(
                    dir,
                    hg_cmd,
                    cascade_set,
                    commits,
                    cursor_or(cursor.as_deref()),
                    squash,
                )?;
            }
            need_cascade.clear();
            if !options.keep_branches {
                hg_prune_branches_with(dir, hg_cmd, io, &need_prune, commits)?;
                need_prune.clear();
            }
        }
    }
    if options.hold {
        let restore = hg_restore_commands(state);
        hg_hold_text(
            io,
            &push_bookmarks,
            onto_remote,
            cursor_or(cursor.as_deref()),
            &restore,
        );
        discard_local_state_hg_with(dir, hg_cmd, io, state)?;
        return Ok(());
    }
    hg_reconcile_with(
        dir,
        hg_cmd,
        &state.local_commit,
        state.local_bookmark.as_deref(),
    );
    discard_local_state_hg_with(dir, hg_cmd, io, state)?;
    io.push_status_with_ansi("DONE", "Landed changes.", "green", io.ansi);
    Ok(())
}

/// True for the engine errors PHP raises as
/// `PhutilArgumentUsageException` (exit 1 under the `USAGE EXCEPTION`
/// label): discovery failures (`ArcanistLandEngine.php:823-1153`),
/// onto/into selection errors, merge conflicts, and empty merges
/// (`ArcanistGitLandEngine.php:286-1348`), hg resolution errors
/// (`ArcanistMercurialLandEngine.php:116-334`), push failures rewrapped
/// by the cascade retry (`ArcanistLandEngine.php:1278-1290`), stash and
/// strategy errors, and config-item errors. Anything else —
/// fetch/merge operation failures (plain `ArcanistUsageException` or
/// `CommandException`) and conduit errors — escapes into a fatal.
fn is_toolset_usage_message(message: &str) -> bool {
    const EXACT: &[&str] = &[
        "There are no commits to land.",
        "Encountered a merge conflict.",
        "Landing onto multiple branches at once is not supported in Mercurial.",
        // Push failures rewrap into usage after the cascade retry
        // (`ArcanistLandEngine.php:1278-1290`); these texts arise only
        // from the push sites.
        "Push failed! Fix the error and run \"arc land\" again.",
        "Submit failed! Fix the error and run \"arc land\" again.",
        // Stash errors (`ArcanistRepositoryLocalState.php:158-186`).
        "Resolve merge conflicts before proceeding.",
        "You can not continue with uncommitted changes. Commit or discard them before proceeding.",
        // Legacy strategy ambiguity
        // (`ArcanistGitLandEngine.php:1584-1599`).
        "Desired merge strategy is ambiguous, choose an explicit strategy.",
    ];
    if EXACT.contains(&message) {
        return true;
    }
    const PREFIXES: &[&str] = &[
        "TODO: You are forcing a revision,",
        "Revision for commit \"",
        "Unable to determine revision for commit \"",
        "Merging local \"",
        "No pushable remote \"",
        "Perforce mode does not support",
        "The \"onto\" refs you have selected are connected to multiple different",
        "The branches you are landing are connected to multiple different",
        "Selected \"onto\" ref \"",
        "No remote \"",
        // `arc.land.onto` item errors
        // (`ArcanistStringListConfigOption::validateListItem()`).
        "Expected a string (at index \"",
    ];
    if PREFIXES.iter().any(|prefix| message.starts_with(prefix)) {
        return true;
    }
    const SHAPES: &[(&str, &str)] = &[
        ("Symbol \"", "\" does not identify a valid revision."),
        (
            "Symbol \"",
            "\" does not identify a bookmark, branch, or commit.",
        ),
        ("Symbol \"", "\" is ambiguous."),
        ("Branch \"", "\" does not exist in the local working copy."),
        ("Remote \"", "is not a valid fetchable remote."),
        ("Local ref \"", "\" does not exist."),
        ("Ref \"", "\" does not exist in remote \""),
    ];
    SHAPES
        .iter()
        .any(|(prefix, infix)| message.starts_with(prefix) && message.contains(infix))
}

/// Classify an engine-stage error: declines ([`ABORT`]) are
/// `ArcanistUserAbortException` (the `---` arm), toolset-usage texts
/// render `USAGE EXCEPTION`, and everything else escapes into a
/// fatal.
fn classify_engine_error(message: String) -> ClassifiedError {
    if message == ABORT {
        ClassifiedError::user_abort(message)
    } else if is_toolset_usage_message(&message) {
        ClassifiedError::toolset_usage(message)
    } else {
        ClassifiedError::fatal(message)
    }
}

/// Run the workflow with scripted or live IO, returning collected output.
///
/// Parse errors carry no usage text (PHP renders usage errors with no
/// dump); `--help` returns the usage text.
pub fn execute_with_io(dir: &Path, argv: &[String], io: &mut Io) -> Result<Out, String> {
    execute_with_io_classified(dir, argv, io).map_err(|error| error.message)
}

/// [`execute_with_io`] with errors classified for the R4 envelope:
/// argument, working-copy, and validation errors are toolset usage;
/// engine errors split into declines (`---`), toolset usage, and
/// fatal (see [`is_toolset_usage_message`]).
pub fn execute_with_io_classified(
    dir: &Path,
    argv: &[String],
    io: &mut Io,
) -> Result<Out, ClassifiedError> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            io.push_out(USAGE);
            return Ok(std::mem::take(&mut io.out));
        }
        Err(message) => return Err(ClassifiedError::toolset_usage(message)),
    };
    let Some(vcs) = detect_vcs(dir) else {
        return Err(ClassifiedError::toolset_usage(NOT_GIT_OR_HG.to_string()));
    };
    for (key, answer) in load_saved_prompts(dir) {
        io.saved.entry(key).or_insert(answer);
    }
    if io.config_writer.is_none() {
        let metadata = match vcs {
            Vcs::Git => ".git",
            Vcs::Hg => ".hg",
        };
        let user_path = get_config::resolve_user_file(
            config::arcrc_path(),
            options.arcrc_file.as_deref().map(Path::new),
        )
        .map_err(ClassifiedError::toolset_usage)?;
        // Saved answers persist under the repo-root metadata directory,
        // like the loader above (falling back to the CWD join when no
        // metadata directory is found, e.g. `GIT_DIR` setups).
        let local_path = find_dot_dir(dir, metadata)
            .map(|root| root.join("arc").join("config"))
            .unwrap_or_else(|| dir.join(metadata).join("arc").join("config"));
        io.config_writer = Some(ConfigWriter {
            user_path,
            local_path: Some(local_path),
        });
    }
    let validated = validate_options(&options, dir, io).map_err(ClassifiedError::toolset_usage)?;
    execute_engine(dir, &options, vcs, &validated, io).map_err(classify_engine_error)?;
    Ok(std::mem::take(&mut io.out))
}

/// Run the workflow rooted at `dir` with buffered IO.
pub fn execute(dir: &Path, argv: &[String]) -> Result<Out, String> {
    let mut io = Io::buffered();
    io.ansi = terminal::ansi_enabled();
    execute_with_io(dir, argv, &mut io)
}

/// Run the workflow rooted at `dir` with live console IO.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    // T1153 R4 (LAND-37): `land` is a toolset workflow
    // (`ArcanistLandWorkflow extends ArcanistArcWorkflow`). Argument
    // and validation errors render the red `USAGE EXCEPTION` label
    // (exit 1), declined prompts the red `---` label (exit 1), and
    // operation failures escape into a PHP fatal (255).
    let mut io = Io::live();
    io.ansi = terminal::ansi_enabled();
    match execute_with_io_classified(dir, argv, &mut io) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => error.report(),
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "land_tests.rs"]
mod land_tests;
