//! `arc-rs land`: publish reviewed changes.
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
//! Console rendering follows the diff port: `tsprintf` markup (`**bold**`,
//! `<bg:...>`, `%!`/`%W`/`%?`/`%>` blocks) renders as plain text, and log
//! statuses render as `** LABEL ** message`. Statuses go to stderr (like
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
//! - Interactive editors are not ported: flows that open `$EDITOR` in PHP
//!   (commit-message fallback) fail with a guidance error instead.
//! - Prompt answers ending in `*` or `!` are accepted with the suffix
//!   stripped; persisting the answer to configuration is not ported.
//! - `phutil_console_wrap()` line wrapping is not ported; prompt and banner
//!   text renders unwrapped.
//! - Display summaries truncate to 64 characters plus `...`; PHP truncates
//!   to 64 glyphs via `PhutilUTF8StringTruncator` (exact ellipsis behavior
//!   unverified).
//! - Revision/marker "ref views" render in a stable plain-text shape; the
//!   `ArcanistRefView` box drawing is not ported.
//! - `git-p4`/Perforce remotes are rejected with a not-ported error (PHP
//!   submits via `git p4`; no `p4` binary is assumed).
//! - Saved prompt responses read `.arcconfig` `prompts` (a map or a list of
//!   `{prompt, response}` objects); the PHP configuration-source plumbing
//!   around `ArcanistPromptResponse` is not ported.
//! - `arc.land.onto` configuration accepts one string or a list of strings.
//! - `history.immutable` legacy-strategy ambiguity check
//!   (`ArcanistGitLandEngine.php:1649-1693`) is not ported: without
//!   `--strategy` or `arc.land.strategy` configuration the port always
//!   selects `squash`.
//! - Mercurial remote-ref existence probes run `hg identify --id --rev REF
//!   REMOTE`; if that probe errors for any reason the ref is treated as
//!   new (PHP queries remote markers instead).
//! - Mercurial `selectCommits()` with a null into-commit queries
//!   `reverse(ancestors(symbol))`; PHP passes the null into-commit instead
//!   (`ArcanistMercurialLandEngine.php:711-716`), which looks like an
//!   upstream bug.
//! - Local-state capture/restore around the merge loop
//!   (`ArcanistRepositoryLocalState`) is not ported: on failure the port
//!   checks the original ref back out when it still exists, and the hold
//!   output lists the manual recovery commands.

extern crate self as arc_rs;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{config, diff_submit, git, hg, land_checks};

/// Subcommand name, matching `arc land`.
pub const NAME: &str = "land";

const USAGE: &str = "usage: arc-rs land [--hold] [--keep-branches] [--onto-remote REMOTE] [--onto BRANCH] [--strategy NAME] [--revision REV] [--preview] [--into REF] [--into-remote REMOTE] [--into-local] [--into-empty] [--incremental] [--pick] [--conduit-uri URI] [-- ref ...]\n";

/// `user.whoami` Conduit method; the result's `phid` is the viewer PHID
/// (`src/workflow/ArcanistWorkflow.php:489-494`).
pub const METHOD_WHOAMI: &str = "user.whoami";

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
        self.push_err(&format!("** {label} ** {message}\n"));
    }

    /// True when both streams are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }
}

/// Interactive IO for prompts.
///
/// `canned` holds scripted prompt answers, consumed front to back; when it
/// is empty, answers come from real stdin. `saved` holds saved prompt
/// responses keyed by prompt key (from `.arcconfig` `prompts`). `live`
/// echoes every output byte to the real stdout/stderr as it is written, so
/// interactive users see prompts before answering; when `live`, `run_in`
/// prints nothing more, since everything already echoed.
#[derive(Debug, Default)]
pub struct Io {
    /// Scripted answers for tests; empty means read real stdin.
    pub canned: VecDeque<String>,
    /// Saved prompt responses by prompt key.
    pub saved: HashMap<String, bool>,
    /// Collected output.
    pub out: Out,
    /// Echo output to the real console as it is written.
    pub live: bool,
    /// Scripted `--patch -` text for tests; `None` reads real stdin.
    /// Used only by `patch`; ignored by `land`.
    pub patch_stdin: Option<String>,
}

impl Io {
    /// Buffered IO reading real stdin (for `execute`).
    #[must_use]
    pub fn buffered() -> Self {
        Self::default()
    }

    /// Live IO reading real stdin (for `run_in`).
    #[must_use]
    pub fn live() -> Self {
        Self {
            live: true,
            ..Self::default()
        }
    }

    /// Scripted answers, for tests.
    #[must_use]
    pub fn canned(answers: &[&str]) -> Self {
        Self {
            canned: answers.iter().map(|answer| (*answer).to_string()).collect(),
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

    /// One log line, to stderr.
    pub fn push_status(&mut self, label: &str, message: &str) {
        self.push_err(&format!("** {label} ** {message}\n"));
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

/// Render one banner block to stdout: a blank line, a `** TITLE **` line,
/// the body lines, and a trailing blank line.
///
/// Ports `echo tsprintf("\n%!\n%W\n\n", $title, $body)` with markup
/// stripped (see the module docs).
pub fn push_banner(io: &mut Io, title: &str, body: &str) {
    io.push_out(&format!("\n** {title} **\n{body}\n\n"));
}

/// Execute an `arc.land.*` prompt: print `>>>`, loop until `y`/`n`.
///
/// Ports `ArcanistPrompt::execute()` (`src/toolset/ArcanistPrompt.php:47-254`)
/// in plain text: a leading blank line, then `>>> {query} [y/N/?] ` per
/// attempt. Empty answers mean `n`; `?` prints the prompt help; answers
/// ending in `*` or `!` strip the suffix (persisting the answer is not
/// ported). A saved response answers immediately with the
/// `Using saved response` notice. Declining — or end of input — aborts
/// with [`ABORT`].
pub fn prompt_for_key(io: &mut Io, key: &str, query: &str) -> Result<(), String> {
    if let Some(saved) = io.saved.get(key).copied() {
        io.push_out(&format!(
            ">>> {query} <{}>\n>>> (Using saved response to prompt \"{key}\".\n\n",
            if saved { "y" } else { "n" }
        ));
        return if saved {
            Ok(())
        } else {
            Err(ABORT.to_string())
        };
    }
    io.push_out("\n");
    loop {
        io.push_out(&format!(">>> {query} [y/N/?] "));
        let Some(raw) = io.read_answer() else {
            return Err(ABORT.to_string());
        };
        let mut response = raw.trim().to_string();
        if response.is_empty() {
            response = "n".to_string();
        }
        // Strip the save-to-config suffixes (`ArcanistPrompt.php:167-179`);
        // persisting is not ported.
        if let Some(stripped) = response
            .strip_suffix('*')
            .or_else(|| response.strip_suffix('!'))
        {
            response = stripped.to_string();
        }
        let lowered = response.to_lowercase();
        if lowered == "y" {
            return Ok(());
        }
        if lowered == "n" {
            return Err(ABORT.to_string());
        }
        if lowered == "?" {
            push_prompt_help(io, key);
            continue;
        }
    }
}

/// Print the `?` help for a prompt (`ArcanistPrompt.php:191-230`).
fn push_prompt_help(io: &mut Io, key: &str) {
    let description = land_checks::prompt_description(key).unwrap_or("");
    io.push_out(&format!(
        "\n** PROMPT ** **{key}**\n\n{description}\n\nThe default response to this prompt is \"n\".\n\nUse \"*\" after a response to save it in user configuration.\nUse \"!\" after a response to save it in working copy configuration.\nRun \"arc help prompts\" for detailed help on configuring responses.\n\n"
    ));
}

/// Ask a yes/no question, returning the answer.
///
/// Ports `phutil_console_confirm()` (`src/console/format.php:11-25`):
/// prints two blank lines plus `{query} [y/N]` (or `[Y/n]` when
/// `default_no` is false), loops until `y`/`n`/empty, then prints a blank
/// line. End of input aborts with [`ABORT`].
pub fn console_confirm(io: &mut Io, query: &str, default_no: bool) -> Result<bool, String> {
    let options = if default_no { "[y/N]" } else { "[Y/n]" };
    loop {
        io.push_out(&format!("\n\n{query} {options} "));
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

/// Read one `.arcconfig` value, walking toward the filesystem root.
///
/// Later (nearer) files win; the first file defining `key` provides it.
/// Mirrors the project half of `config::discover_from()`, generalized to
/// any key.
fn arcconfig_value(dir: &Path, key: &str) -> Option<serde_json::Value> {
    dir.ancestors().find_map(|ancestor| {
        let text = std::fs::read_to_string(ancestor.join(".arcconfig")).ok()?;
        let doc: serde_json::Value = serde_json::from_str(&text).ok()?;
        doc.get(key).cloned()
    })
}

/// Parse an `arc.land.onto` configuration value: one string or a list of
/// strings (`ArcanistLandEngine::getOntoFromConfiguration()`,
/// `ArcanistLandEngine.php:255-258`).
fn parse_onto_config(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(name) => vec![name.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse one string configuration value (`arc.land.onto-remote`,
/// `arc.land.strategy`).
fn parse_string_config(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

/// Load saved prompt responses from `.arcconfig` `prompts`.
///
/// Accepts a map (`{"arc.land.confirm": "y"}`) or a list of
/// `{prompt, response}` objects. Responses compare case-insensitively;
/// anything but `y`/`yes` counts as `n`.
fn load_saved_prompts(dir: &Path) -> HashMap<String, bool> {
    let mut saved = HashMap::new();
    let Some(value) = arcconfig_value(dir, "prompts") else {
        return saved;
    };
    match &value {
        serde_json::Value::Object(map) => {
            for (key, response) in map {
                if let Some(text) = response.as_str() {
                    saved.insert(key.clone(), is_yes(text));
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                let prompt = item.get("prompt").and_then(|v| v.as_str());
                let response = item.get("response").and_then(|v| v.as_str());
                if let (Some(prompt), Some(response)) = (prompt, response) {
                    saved.insert(prompt.to_string(), is_yes(response));
                }
            }
        }
        _ => {}
    }
    saved
}

/// True for `y`/`yes` answers, case-insensitively.
fn is_yes(text: &str) -> bool {
    matches!(text.trim().to_lowercase().as_str(), "y" | "yes")
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
    /// The resolved symbol text, as `getSymbol()` would return it.
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
    const SUPPORTED: &str = "merge, squash";
    if let Some(strategy) = argument {
        let parsed = parse_strategy_name(strategy).ok_or_else(|| {
            format!(
                "Merge strategy \"{strategy}\" specified with \"--strategy\" is unknown. \
                 Supported merge strategies are: {SUPPORTED}."
            )
        })?;
        let line = format!(
            "** STRATEGY ** Merging with \"{strategy}\" strategy, selected with \"--strategy\".\n"
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
        let line = format!(
            "** STRATEGY ** Merging with \"{strategy}\" strategy, configured with \
             \"arc.land.strategy\".\n"
        );
        return Ok((parsed, line));
    }
    Ok((
        Strategy::Squash,
        "** STRATEGY ** Merging with \"squash\" strategy, the default strategy.\n".to_string(),
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
    let strategy_config = arcconfig_value(dir, "arc.land.strategy")
        .as_ref()
        .and_then(parse_string_config);
    let (strategy, line) =
        select_strategy(options.strategy.as_deref(), strategy_config.as_deref())?;
    io.push_err(&line);
    if options.pick && strategy != Strategy::Squash {
        return Err("You can not \"--pick\" changes under the \"merge\" strategy.".to_string());
    }
    let revision = options
        .revision
        .as_deref()
        .map(parse_revision_symbol)
        .transpose()?;
    Ok(Validated { strategy, revision })
}

/// Options plus validated selections.
#[derive(Debug)]
struct Validated {
    strategy: Strategy,
    revision: Option<RevisionSymbol>,
}

/// Resolve the conduit URI (flag, `ARC_CONDUIT_URI`, or `.arcconfig`
/// walk-up from `dir`) and build an authenticated client from `~/.arcrc`.
///
/// Mirrors [`config::client_for`], but discovers from the target working
/// copy — like the diff port (`discover_from(&dir)`) — instead of the
/// process working directory.
pub fn client_for_dir(
    conduit_uri: Option<&str>,
    dir: &Path,
) -> Result<arc_rs::conduit::Client, String> {
    let uri = conduit_uri
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(|| config::discover_from(dir))
        .ok_or_else(|| {
            "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project \
             with .arcconfig"
                .to_string()
        })?;
    let token = config::arcrc_path()
        .and_then(|path| config::token_for_file(&path, &uri))
        .ok_or_else(|| format!("no token for {uri} in ~/.arcrc"))?;
    arc_rs::conduit::Client::new(&uri, &token).map_err(|e| e.to_string())
}

/// Note printed when Conduit is unavailable and revision discovery is
/// skipped (a port extension; PHP requires Conduit).
pub fn format_skip_note(reason: &str) -> String {
    format!("Note: revision info unavailable ({reason}); landing without revision association.\n")
}

/// Load revision rows for `symbol` via `differential.revision.search`.
///
/// Numeric symbols constrain `ids`; PHID symbols constrain `phids`
/// (`ArcanistRevisionSymbolRef`, `src/ref/revision/ArcanistRevisionSymbolRef.php:18-24`).
pub fn fetch_revision_views(
    call: &diff_submit::ConduitCall<'_>,
    symbol: &RevisionSymbol,
) -> Result<Vec<land_checks::RevisionView>, String> {
    let params = match symbol {
        RevisionSymbol::Id(id) => {
            let numeric =
                i64::try_from(*id).map_err(|_| format!("Revision ID {id} is out of range."))?;
            diff_submit::revision_search_params(&[numeric])
        }
        RevisionSymbol::Phid(phid) => serde_json::json!({"phids": [phid]}),
    };
    let result = call(diff_submit::METHOD_REVISION_SEARCH, &params)?;
    let rows = result
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "differential.revision.search response has no data list".to_string())?;
    rows.iter().map(land_checks::parse_revision).collect()
}

/// Resolve a `--revision` symbol to its revision, or fail with the
/// verbatim unresolved-symbol error
/// (`ArcanistLandEngine.php:823-829`).
pub fn resolve_forced_revision(
    call: &diff_submit::ConduitCall<'_>,
    symbol: &RevisionSymbol,
) -> Result<land_checks::RevisionView, String> {
    let views = fetch_revision_views(call, symbol)?;
    views.into_iter().next().ok_or_else(|| {
        format!(
            "Symbol \"{}\" does not identify a valid revision.",
            symbol.display()
        )
    })
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
        let items = result
            .get("data")
            .and_then(|data| data.as_array())
            .ok_or_else(|| "edge.search response has no data list".to_string())?;
        for item in items {
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
/// Follows `revision.parent` edges, then resolves each distinct parent
/// PHID through `differential.revision.search`, like
/// `ArcanistRevisionParentRevisionsHardpointQuery` (`:45-80`).
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
    for phid in &distinct {
        let views = fetch_revision_views(call, &RevisionSymbol::Phid(phid.clone()))?;
        if let Some(view) = views.into_iter().next() {
            parents.insert(phid.clone(), view);
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
}

/// Parse `harbormaster.buildable.search` data rows
/// (`ArcanistBuildableRef::newFromConduit()`, `getObjectPHID()`,
/// `src/ref/buildable/ArcanistBuildableRef.php:23-39`).
fn parse_buildable_rows(result: &serde_json::Value) -> Result<Vec<BuildableRow>, String> {
    let items = result
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "harbormaster.buildable.search response has no data list".to_string())?;
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
        rows.push(BuildableRow {
            phid: phid.to_string(),
            object_phid: object.to_string(),
        });
    }
    Ok(rows)
}

/// One build row as `harbormaster.build.search` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildRow {
    status: String,
    buildable_phid: String,
    plan_phid: String,
}

/// Parse `harbormaster.build.search` data rows
/// (`ArcanistBuildRef`, `src/ref/build/ArcanistBuildRef.php:48-58`).
fn parse_build_rows(result: &serde_json::Value) -> Result<Vec<BuildRow>, String> {
    let items = result
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "harbormaster.build.search response has no data list".to_string())?;
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
        rows.push(BuildRow {
            status: status.to_string(),
            buildable_phid: buildable.to_string(),
            plan_phid: plan.to_string(),
        });
    }
    Ok(rows)
}

/// Parse `harbormaster.buildplan.search` data rows into
/// `plan PHID -> arc-land behavior`
/// (`ArcanistBuildPlanRef::getBehavior()`,
/// `src/ref/buildplan/ArcanistBuildPlanRef.php:30-35`).
fn parse_plan_behaviors(result: &serde_json::Value) -> Result<HashMap<String, String>, String> {
    let items = result
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| "harbormaster.buildplan.search response has no data list".to_string())?;
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
    Ok(behaviors)
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
/// (`ArcanistBuildBuildplanHardpointQuery.php:21-25`). Revisions without a
/// diff PHID (legacy rows) contribute no builds.
pub fn fetch_build_views(
    call: &diff_submit::ConduitCall<'_>,
    revisions: &[land_checks::RevisionView],
) -> Result<Vec<land_checks::BuildView>, String> {
    let mut diff_phids: Vec<String> = Vec::new();
    for revision in revisions {
        if let Some(diff) = &revision.diff_phid
            && !diff.is_empty()
            && !diff_phids.contains(diff)
        {
            diff_phids.push(diff.clone());
        }
    }
    if diff_phids.is_empty() {
        return Ok(Vec::new());
    }
    let buildables = call(
        METHOD_BUILDABLE_SEARCH,
        &serde_json::json!({"objectPHIDs": diff_phids, "manual": false}),
    )?;
    let rows = parse_buildable_rows(&buildables)?;
    // Join buildable objectPHID -> revision diffPHID, like
    // `confirmBuilds` (`ArcanistLandEngine.php:623-644`).
    let mut buildable_to_revision: HashMap<String, String> = HashMap::new();
    for row in &rows {
        for revision in revisions {
            if revision.diff_phid.as_deref() == Some(row.object_phid.as_str()) {
                buildable_to_revision.insert(row.phid.clone(), revision.phid.clone());
            }
        }
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let phids: Vec<String> = rows.iter().map(|row| row.phid.clone()).collect();
    let builds = call(
        METHOD_BUILD_SEARCH,
        &serde_json::json!({"buildables": phids}),
    )?;
    let build_rows = parse_build_rows(&builds)?;
    let mut plan_phids: Vec<String> = build_rows
        .iter()
        .map(|row| row.plan_phid.clone())
        .filter(|phid| !phid.is_empty())
        .collect();
    plan_phids.sort();
    plan_phids.dedup();
    let behaviors = if plan_phids.is_empty() {
        HashMap::new()
    } else {
        let plans = call(
            METHOD_BUILDPLAN_SEARCH,
            &serde_json::json!({"phids": plan_phids}),
        )?;
        parse_plan_behaviors(&plans)?
    };
    let mut views = Vec::new();
    for row in &build_rows {
        let Some(revision_phid) = buildable_to_revision.get(&row.buildable_phid) else {
            continue;
        };
        let behavior = if row.plan_phid.is_empty() {
            // No plan ref: the build is skipped
            // (`ArcanistLandEngine.php:578-581`).
            None
        } else {
            Some(
                behaviors
                    .get(&row.plan_phid)
                    .cloned()
                    .unwrap_or_else(|| land_checks::PLAN_ALWAYS.to_string()),
            )
        };
        views.push(land_checks::BuildView {
            status: row.status.clone(),
            plan_behavior: behavior,
            revision_phid: revision_phid.clone(),
        });
    }
    Ok(views)
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
/// [`land_checks::evaluate_gates`]. Without a viewer identity the author
/// check can not run; viewing as the first revision's author reduces it to
/// a no-op for single-revision lands.
pub fn load_gate_findings(
    call: &diff_submit::ConduitCall<'_>,
    revisions: &[land_checks::RevisionView],
    commit_count: usize,
) -> Result<Vec<land_checks::GateFinding>, String> {
    let fallback = revisions
        .first()
        .map(|revision| revision.author_phid.clone())
        .unwrap_or_default();
    let viewer = fetch_viewer_phid(call).unwrap_or(fallback);
    let links = fetch_parent_links(call, revisions)?;
    let landing: Vec<&str> = revisions.iter().map(|rev| rev.phid.as_str()).collect();
    let builds = fetch_build_views(call, revisions)?;
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
fn format_finding_line(finding: &land_checks::GateFinding) -> String {
    format!(
        "  [{}] {}: {}\n",
        finding.prompt, finding.title, finding.query
    )
}

/// Advisory gate report for `--preview`: a pass line or listed findings.
pub fn format_gate_preview(findings: &[land_checks::GateFinding]) -> String {
    if findings.is_empty() {
        return "Pre-land checks passed.\n".to_string();
    }
    let mut out = "Pre-land checks:\n".to_string();
    for finding in findings {
        out.push_str(&format_finding_line(finding));
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

/// Display summary: the subject truncated to 64 characters plus `...`
/// (`ArcanistLandCommit::getDisplaySummary()`,
/// `src/land/ArcanistLandCommit.php:38-45`; ellipsis behavior approximate,
/// see the module docs).
pub fn display_summary(summary: &str) -> String {
    const LIMIT: usize = 64;
    if summary.chars().count() <= LIMIT {
        return summary.to_string();
    }
    let truncated: String = summary.chars().take(LIMIT).collect();
    format!("{truncated}...")
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
        io.push_out(&format!(
            "  {} {}\n",
            display_hash(&commit.hash),
            display_summary(&commit.summary)
        ));
    }
    io.push_out(&format!(
        "  < ... {} more commits ... >\n",
        count - SHOW * 2
    ));
    for commit in commits.iter().skip(count - SHOW) {
        io.push_out(&format!(
            "  {} {}\n",
            display_hash(&commit.hash),
            display_summary(&commit.summary)
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

/// Extract `Differential Revision` IDs from a commit message corpus.
///
/// Matches `differential revision:` (case-insensitive) followed by an
/// optional URI and a `D123`/`123` monogram, collecting every ID. Ports the
/// explicit-ID half of `loadWorkingCopyDifferentialRevisions()`
/// (`ArcanistGitAPI.php:1192-1202`).
pub fn differential_revision_ids(message: &str) -> Vec<u64> {
    const TAG: &[u8] = b"differential revision:";
    let bytes = message.as_bytes();
    let mut ids = Vec::new();
    let mut index = 0;
    while index + TAG.len() <= bytes.len() {
        if bytes[index..index + TAG.len()].eq_ignore_ascii_case(TAG)
            && let Some(after) = message.get(index + TAG.len()..)
            && let Some(id) = first_revision_id_after(after)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
        index += 1;
    }
    ids
}

/// Find the first `D123`/`123` monogram in `text`.
fn first_revision_id_after(text: &str) -> Option<u64> {
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let start = if ch == 'D' || ch == 'd' {
            match chars.peek() {
                Some((_, next)) if next.is_ascii_digit() => index + 1,
                _ => continue,
            }
        } else if ch.is_ascii_digit() {
            // Skip digits that continue a longer token (part of a URI path
            // segment or word); monograms start at a boundary.
            if index > 0 {
                let prev = text.as_bytes()[index - 1];
                if prev.is_ascii_alphanumeric() {
                    continue;
                }
            }
            index
        } else {
            continue;
        };
        let digits: String = text[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if let Ok(id) = digits.parse::<u64>() {
            return Some(id);
        }
    }
    None
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
/// commit-hash pairs to look up. Commits with exactly one related revision
/// select it; a forced `--revision` selects itself for commits with no
/// related revisions (or related sets including it), fails with the TODO
/// error when other commits belong elsewhere, and ambiguity without a
/// forced revision prints the `AMBIGUOUS REVISION` banner and fails.
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
    for commit in commits.iter_mut() {
        let message = message_of(&commit.hash)?;
        let ids = differential_revision_ids(&message);
        if ids.is_empty() {
            let hashes = hashes_of(&commit.hash)?;
            commit.related = query_revisions_by_hashes(call, &hashes)?;
        } else {
            commit.related = query_revisions_by_ids(call, &ids)?;
        }
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
fn reject_ambiguous_commits(io: &mut Io, commits: &[LandCommit]) -> Result<(), String> {
    for commit in commits {
        if commit.explicit.is_some() || commit.related.is_empty() {
            continue;
        }
        let symbols = commit
            .indirect_symbols
            .iter()
            .map(|symbol| format!("\"{symbol}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let short = commit.hash.chars().take(12).collect::<String>();
        let message = format!(
            "The revision associated with commit \"{short}\" (an ancestor of: {symbols}) is \
             ambiguous. These {} revision(s) are associated with the commit:",
            commit.related.len()
        );
        push_banner(io, "AMBIGUOUS REVISION", &message);
        for related in &commit.related {
            io.push_out(&format!("  {} {}\n", related.monogram, related.name));
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
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, commit) in commits.iter().enumerate() {
        let revision = resolve_commit_revision(commits, index)?
            .ok_or_else(|| unknown_revision(io, &commit.hash, display_hash))?;
        let phid = revision.phid.clone();
        match groups.iter_mut().find(|(key, _)| *key == phid) {
            Some((_, members)) => members.push(index),
            None => groups.push((phid, vec![index])),
        }
    }
    // Order by ancestor walk over head commits (`:1164-1180`).
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
    let mut sets = Vec::new();
    for phid in order {
        let Some((_, members)) = groups.iter().find(|(key, _)| *key == phid) else {
            continue;
        };
        let revision = resolve_commit_revision(commits, members[0])?.expect("grouped revision");
        sets.push(CommitSet {
            revision: Some(revision),
            commits: members.clone(),
            is_pick: false,
        });
    }
    // Any group unreachable from a head (no heads marked) keeps map order.
    for (phid, members) in &groups {
        if sets
            .iter()
            .any(|set: &CommitSet| set.revision.as_ref().is_some_and(|rev| &rev.phid == phid))
        {
            continue;
        }
        let revision = resolve_commit_revision(commits, members[0])?.expect("grouped revision");
        sets.push(CommitSet {
            revision: Some(revision),
            commits: members.clone(),
            is_pick: false,
        });
    }
    for set in &mut sets {
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
    Ok(sets)
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
            push_banner(
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

/// Reject Perforce remotes, which this port does not drive (PHP submits
/// via `git p4`; see the module docs).
fn reject_perforce_remote(dir: &Path, remote: &str) -> Result<(), String> {
    if remote != "p4" {
        return Ok(());
    }
    let git_dir = git::output(dir, &["rev-parse", "--git-dir"])
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|_| ".git".to_string());
    let root = if Path::new(&git_dir).is_absolute() {
        PathBuf::from(git_dir)
    } else {
        dir.join(git_dir)
    };
    if root.join("refs").join("remotes").join("p4").exists() {
        return Err("Perforce remotes are not yet ported by arc-rs land (see T1128)".to_string());
    }
    Ok(())
}

/// Select the git onto remote: `--onto-remote`, then `arc.land.onto-remote`
/// configuration, then symbol upstreams, then the `p4` remote, then
/// `origin` (`ArcanistGitLandEngine::selectOntoRemote()`/`newOntoRemote()`,
/// `:1140-1264`).
///
/// Returns the remote; the `ONTO REMOTE` status line is pushed.
pub fn git_select_onto_remote(
    dir: &Path,
    io: &mut Io,
    options: &Options,
    symbols: &[String],
) -> Result<String, String> {
    let remote = git_new_onto_remote(dir, io, options, symbols)?;
    reject_perforce_remote(dir, &remote)?;
    if git::remote_push_uri(dir, &remote).is_none() {
        return Err(format!(
            "No pushable remote \"{remote}\" exists. Use the \"--onto-remote\" flag to choose a \
             valid, pushable remote to land changes onto."
        ));
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
    if let Some(remote) = arcconfig_value(dir, "arc.land.onto-remote")
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
    // The `p4` remote implies a Perforce-synchronized working copy
    // (`:1241-1253`); reject it here with the not-ported error when it
    // would apply, so selection never silently lands past it.
    if is_perforce_working_copy(dir) {
        return Err("Perforce remotes are not yet ported by arc-rs land (see T1128)".to_string());
    }
    io.push_status(
        "ONTO REMOTE",
        "Landing onto remote \"origin\", the default remote under Git.",
    );
    Ok("origin".to_string())
}

/// True when `.git/refs/remotes/p4` exists
/// (`ArcanistGitAPI::isPerforceRemote()`, `:1522-1540`).
fn is_perforce_working_copy(dir: &Path) -> bool {
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
    if let Some(value) = arcconfig_value(dir, "arc.land.onto") {
        let onto = parse_onto_config(&value);
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
            io.push_status(
                "LOCAL CYCLE",
                &format!(
                    "Local branch \"{symbol}\" tracks an upstream, but following it leads to a \
                     local cycle; ignoring branch upstream."
                ),
            );
            io.push_status("LOCAL CYCLE", &cycle.join(" -> "));
            continue;
        }
        if !path.is_connected_to_remote() {
            io.push_status(
                "NO PATH TO REMOTE",
                &format!(
                    "Local branch \"{symbol}\" tracks an upstream, but there is no path to a \
                     remote; ignoring branch upstream."
                ),
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
    let list = new_markers
        .iter()
        .map(|name| format!("  {name}\n"))
        .collect::<String>();
    push_banner(
        io,
        &format!("CREATE {count} BRANCHE(S)"),
        &format!(
            "These {count} symbol(s) do not exist in the remote. They will be created as new \
             branches:\n\n{list}"
        ),
    );
    if hold {
        io.push_out(&format!(
            "You are using \"--hold\", so execution will stop before the {count} branche(s) are \
             actually created. You will be given instructions to create the branches.\n"
        ));
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
/// When no local copy exists, the first fetch ignores failures; a still
/// missing ref resolves to `None` (the caller implies `--into-empty`).
/// Returns the `refs/remotes/REMOTE/REF` commit otherwise.
fn git_fetch_target(
    dir: &Path,
    io: &mut Io,
    remote: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let full_ref = format!("refs/remotes/{remote}/{name}");
    if git::output(dir, &["rev-parse", "--verify", &full_ref])
        .is_ok_and(|out| !out.status.success())
    {
        io.push_status(
            "TARGET",
            &format!(
                "No local copy of ref \"{name}\" in remote \"{remote}\" exists, attempting \
                 fetch..."
            ),
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
    git::output(dir, &["fetch", "--no-tags", "--quiet", "--", remote, name])
        .is_ok_and(|out| out.status.success())
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
/// (`ArcanistGitLandEngine::applyRebaseOperation()`, `:1695-1730`).
///
/// Checks `max` out first; `force_resolve` adds the recursive-theirs
/// strategy. Failures abort the rebase, reset hard, and propagate.
fn git_apply_rebase(
    dir: &Path,
    min: &str,
    max: &str,
    dst: &str,
    force_resolve: bool,
) -> Result<String, String> {
    git::stdout_text(dir, &["checkout", max, "--"])?;
    let base = format!("{min}^");
    let mut args = vec!["rebase", "--onto", dst];
    if force_resolve {
        args.extend(["--strategy", "recursive", "--strategy-option", "theirs"]);
    }
    args.extend(["--", &base]);
    if git::output(dir, &args).is_ok_and(|out| !out.status.success()) {
        git::output(dir, &["rebase", "--abort"]).ok();
        git::output(dir, &["reset", "--hard", "HEAD", "--"]).ok();
        return Err("rebase failed".to_string());
    }
    git::stdout_text(dir, &["rev-parse", "--verify", "HEAD"])
}

/// Merge `src` into `dst`, leaving the result uncommitted
/// (`ArcanistGitLandEngine::applyMergeOperation()`, `:1732-1767`).
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
        git::output(dir, &["reset", "--hard", "HEAD"]).ok();
        return Err("merge failed".to_string());
    }
    Ok(())
}

/// Commit the index with `message`, returning the new HEAD
/// (`ArcanistGitLandEngine::applyCommitOperation()`, `:1769-1802`).
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

/// Merge approaches for one set, in attempt order
/// (`ArcanistGitLandEngine::executeMerge()`, `:329-336`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeApproach {
    /// Squash a complex range down to one commit, then rebase-merge it.
    ReduceRebaseMerge,
    /// Rebase the range onto the into state, then merge.
    RebaseMerge,
    /// Merge the max commit directly.
    Merge,
}

/// Approaches for a strategy: squash (non-empty) tries rebase-merge then
/// reduce-rebase-merge; anything else merges directly.
fn merge_approaches(squash: bool, is_empty: bool) -> Vec<MergeApproach> {
    if squash && !is_empty {
        vec![MergeApproach::RebaseMerge, MergeApproach::ReduceRebaseMerge]
    } else {
        vec![MergeApproach::Merge]
    }
}

/// Execute one git merge approach, returning the merged max hash.
#[allow(clippy::too_many_arguments)]
fn git_merge_attempt(
    dir: &Path,
    io: &mut Io,
    approach: MergeApproach,
    min_hash: &str,
    max_hash: &str,
    into_commit: &str,
    squash: bool,
    is_empty: bool,
) -> Result<String, String> {
    match approach {
        MergeApproach::RebaseMerge => {
            io.push_status("MERGE", "Attempting to rebase changes.");
            let merged = git_apply_rebase(dir, min_hash, max_hash, into_commit, false)?;
            git_apply_merge(dir, &merged, into_commit, squash, is_empty)?;
            Ok(merged)
        }
        MergeApproach::ReduceRebaseMerge => {
            io.push_status("MERGE", "Attempting to reduce and rebase changes.");
            let reduce_dst = format!("{min_hash}^");
            // Squash the into state on top of the range, forcing later
            // conflicts to resolve in our favor (`:389-402`).
            git_apply_merge(dir, into_commit, max_hash, true, is_empty)?;
            let join_hash = git_apply_commit(
                dir,
                &format!(
                    "arc land: join ({} -> {})",
                    git::display_hash(into_commit),
                    git::display_hash(max_hash)
                ),
                None,
                None,
                true,
            )?;
            // Squash the range plus the join into one commit (`:409-420`).
            git_apply_merge(dir, &join_hash, &reduce_dst, true, is_empty)?;
            let reduce_hash = git_apply_commit(
                dir,
                &format!(
                    "arc land: reduce ({}..{} -> {})",
                    git::display_hash(min_hash),
                    git::display_hash(max_hash),
                    reduce_dst
                ),
                None,
                None,
                false,
            )?;
            let merged = git_apply_rebase(dir, &reduce_hash, &reduce_hash, into_commit, true)?;
            git_apply_merge(dir, &merged, into_commit, squash, is_empty)?;
            Ok(merged)
        }
        MergeApproach::Merge => {
            io.push_status("MERGE", "Attempting to merge changes.");
            git_apply_merge(dir, max_hash, into_commit, squash, is_empty)?;
            Ok(max_hash.to_string())
        }
    }
}

/// Merge one commit set into `into_commit`
/// (`ArcanistGitLandEngine::executeMerge()`, `:250-534`).
///
/// Returns the new cursor. Empty diffs fail (usually: already landed);
/// every approach failing prints the `MERGE CONFLICT` banner and fails
/// with `Encountered a merge conflict.`. `message_of` provides the landed
/// commit message for the set.
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
    // Merge onto a scratch empty commit when landing into the empty state
    // (`:258-261`); `commit-tree` builds it without touching branches.
    let into_commit = match into_commit {
        Some(commit) => commit.to_string(),
        None => git::stdout_text(
            dir,
            &[
                "commit-tree",
                git::GIT_MAGIC_ROOT_COMMIT,
                "-m",
                "arc land: empty",
            ],
        )?,
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
    let mut merged_hash = None;
    for approach in merge_approaches(squash, is_empty) {
        if let Ok(merged) = git_merge_attempt(
            dir,
            io,
            approach,
            &min_hash,
            &max_hash,
            &into_commit,
            squash,
            is_empty,
        ) {
            merged_hash = Some(merged);
            break;
        }
    }
    let Some(merged_hash) = merged_hash else {
        return Err(git_merge_conflict(
            io,
            commits,
            set,
            &max_hash,
            &into_commit,
            has_unpushed,
        ));
    };
    io.push_status("DONE", "Merge succeeded.");
    let (author, date) = git_author_and_date(dir, &max_hash)?;
    let message = message_of(set)?;
    let mut cursor = git_apply_commit(dir, &message, Some(&author), Some(&date), false)?;
    if is_empty {
        // Drop the scratch empty parent (T12876, `:514-531`): rebuild the
        // commit with squash (no parents) or merge (the merged hash) via
        // `commit-tree`, then check it out.
        cursor = git_rebuild_without_empty_parent(
            dir,
            &cursor,
            &message,
            &author,
            &date,
            squash,
            &merged_hash,
        )?;
        git::stdout_text(dir, &["checkout", &cursor, "--"])?;
    }
    Ok(cursor)
}

/// Rebuild `cursor` without its scratch empty parent, preserving the tree,
/// message, author, and date (`ArcanistGitLandEngine.php:522-530`).
///
/// Squash rebuilds parentless; merge re-parents onto the merged hash.
/// Author and date travel via `GIT_AUTHOR_*`/`GIT_COMMITTER_*`, since
/// `commit-tree` takes no `--author` flag.
fn git_rebuild_without_empty_parent(
    dir: &Path,
    cursor: &str,
    message: &str,
    author: &str,
    date: &str,
    squash: bool,
    merged_hash: &str,
) -> Result<String, String> {
    let tree = git::stdout_text(dir, &["rev-parse", &format!("{cursor}^{{tree}}")])?;
    let (name, email) = split_author(author);
    let mut command = std::process::Command::new("git");
    command.current_dir(dir).arg("commit-tree").arg(&tree);
    if !squash {
        command.arg("-p").arg(merged_hash);
    }
    command
        .arg("-m")
        .arg(message)
        .env("GIT_AUTHOR_NAME", &name)
        .env("GIT_AUTHOR_EMAIL", &email)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_NAME", &name)
        .env("GIT_COMMITTER_EMAIL", &email)
        .env("GIT_COMMITTER_DATE", date);
    let output = command
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        return Err("git commit-tree failed".to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Split `Name <email>` into parts; malformed authors become empty parts.
fn split_author(author: &str) -> (String, String) {
    if let Some(prefix) = author.strip_suffix('>')
        && let Some((name, email)) = prefix.split_once(" <")
    {
        return (name.to_string(), email.to_string());
    }
    (String::new(), String::new())
}

/// Print the `MERGE CONFLICT` banner and return its error
/// (`ArcanistGitLandEngine.php:461-501`).
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
        io.push_out("Use \"--incremental\" to merge and push changes one by one.\n\n");
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
    io.push_status("PUSHING", &format!("Pushing changes to \"{onto_remote}\"."));
    let refspecs = git_onto_refspecs(onto_refs, into_commit);
    let mut args = vec!["push", "--", onto_remote];
    args.extend(refspecs.iter().map(String::as_str));
    if git::output(dir, &args).is_ok_and(|out| !out.status.success()) {
        return Err("Push failed! Fix the error and run \"arc land\" again.".to_string());
    }
    Ok(())
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
    branches.sort_by_key(|branch| branch.0.to_lowercase());
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
    let mut deleted = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for set in sets {
        let old = &commits[set.commits[set.commits.len() - 1]].hash;
        for (branch, hash) in git_branches_for_commit(dir, old, false) {
            if seen.contains(&branch) {
                continue;
            }
            seen.push(branch.clone());
            io.push_status(
                "CLEANUP",
                &format!("Cleaning up branch \"{branch}\". To recover, run:"),
            );
            io.push_out(&format!(
                "\n    $ git checkout -b {branch} {}\n\n",
                git::display_hash(&hash)
            ));
            git::output(dir, &["branch", "-D", "--", &branch]).ok();
            deleted.push(branch);
        }
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
            io.push_status(
                "REBASE CONFLICT",
                &format!(
                    "Branch \"{branch}\" does not rebase cleanly from \"{}\" onto \"{}\", \
                     skipping.",
                    git::display_hash(old),
                    git::display_hash(rebase_target)
                ),
            );
        }
    }
}

/// Check out the best surviving local branch after landing.
///
/// Ports the core of `reconcileLocalState()`
/// (`ArcanistGitLandEngine.php:611-677`): prefer the original ref, then the
/// into ref (unless empty/local), then the onto refs, skipping deleted and
/// missing branches. Prints nothing; the caller prints `DONE`.
pub fn git_reconcile(
    dir: &Path,
    original: Option<&str>,
    into_ref: Option<&str>,
    onto_refs: &[String],
    deleted: &[String],
) {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(local_ref) = original {
        candidates.push(local_ref.to_string());
    }
    if let Some(into) = into_ref {
        candidates.push(into.to_string());
    }
    candidates.extend(onto_refs.iter().cloned());
    candidates.retain(|branch| !deleted.contains(branch));
    for branch in candidates {
        if git::output(dir, &["rev-parse", "--verify", &branch])
            .is_ok_and(|out| out.status.success())
        {
            git::output(dir, &["checkout", &branch, "--"]).ok();
            return;
        }
    }
}

/// Print the hold instructions
/// (`ArcanistGitLandEngine::didHoldChanges()`, `:888-940`).
///
/// Shows the `HOLD CHANGES` banner, the manual push command, the restore
/// commands, and the branches-unchanged note.
pub fn git_hold_text(
    io: &mut Io,
    onto_remote: &str,
    onto_refs: &[String],
    into_commit: &str,
    restore: &[String],
) {
    push_banner(
        io,
        "HOLD CHANGES",
        "Holding changes locally, they have not been pushed.",
    );
    let refspecs = git_onto_refspecs(onto_refs, into_commit);
    io.push_out("To push changes manually, run this command:\n\n");
    io.push_out(&format!(
        "    $ git push -- {onto_remote} {}\n\n",
        refspecs.join(" ")
    ));
    if !restore.is_empty() {
        io.push_out(&format!(
            "To go back to how things were before you ran \"arc land\", run these {} \
             command(s):\n\n",
            restore.len()
        ));
        for command in restore {
            io.push_out(&format!("    $ {command}\n"));
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
    let bookmarks = hg::stdout_text(dir, &["bookmarks"]).unwrap_or_default();
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
    if let Ok(branch) = hg::stdout_text(dir, &["branch"]) {
        let branch = branch.trim().to_string();
        if !branch.is_empty() {
            io.push_status(
                "SOURCE",
                &format!("Landing the active branch, \"{branch}\"."),
            );
            return Ok(vec![branch]);
        }
    }
    let commit = hg::stdout_text(dir, &["log", "-l", "1", "--template", "{node}", "-r", "."])?;
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
    let bookmarks = hg::stdout_text(dir, &["bookmarks"]).unwrap_or_default();
    let bookmark_names: Vec<String> = parse_hg_bookmarks(&bookmarks)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let branches = hg::stdout_text(dir, &["branches"]).unwrap_or_default();
    let branch_names = parse_hg_branches(&branches);
    let mut commits = Vec::new();
    for symbol in symbols {
        let bookmark_hits = bookmark_names.iter().filter(|name| *name == symbol).count();
        let branch_hits = branch_names.iter().filter(|name| *name == symbol).count();
        if bookmark_hits > 1 {
            return Err(hg_ambiguous_symbol(io, symbol, "bookmark"));
        }
        if branch_hits > 1 {
            return Err(hg_ambiguous_symbol(io, symbol, "branch"));
        }
        if bookmark_hits == 1 || branch_hits == 1 {
            commits.push(hg_canonical(dir, symbol)?);
            continue;
        }
        match hg_canonical(dir, symbol) {
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
fn hg_ambiguous_symbol(io: &mut Io, symbol: &str, marker_type: &str) -> String {
    push_banner(
        io,
        "AMBIGUOUS SYMBOL",
        &format!(
            "Symbol \"{symbol}\" is ambiguous: it matches multiple markers (of type \
             \"{marker_type}\"). Use an unambiguous identifier."
        ),
    );
    format!("Symbol \"{symbol}\" is ambiguous.")
}

/// Resolve one Mercurial revision to its node hash
/// (`ArcanistMercurialAPI::getCanonicalRevisionName()`, `:57-64`).
fn hg_canonical(dir: &Path, symbol: &str) -> Result<String, String> {
    hg::stdout_text(
        dir,
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
    let remote = if let Some(remote) = options.onto_remote.as_deref() {
        io.push_status(
            "ONTO REMOTE",
            &format!("Remote \"{remote}\" was selected with the \"--onto-remote\" flag."),
        );
        remote.to_string()
    } else if let Some(remote) = arcconfig_value(dir, "arc.land.onto-remote")
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
    if !hg::ok(dir, &["paths", &remote]) {
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
    if let Some(value) = arcconfig_value(dir, "arc.land.onto") {
        let onto = parse_onto_config(&value);
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

/// True when `name` names a local branch (vs a bookmark).
fn hg_is_branch(dir: &Path, name: &str) -> bool {
    let branches = hg::stdout_text(dir, &["branches"]).unwrap_or_default();
    parse_hg_branches(&branches).contains(&name.to_string())
}

/// Probe whether `name` exists in `remote` via
/// `hg identify --id --rev NAME REMOTE`; probe failures count as missing
/// (see the module docs).
fn hg_remote_ref_exists(dir: &Path, remote: &str, name: &str) -> bool {
    hg::output(dir, &["identify", "--id", "--rev", name, remote])
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
        .filter(|name| hg_is_branch(dir, name))
        .collect();
    if branches.len() > 1 {
        let list = branches
            .iter()
            .map(|name| format!("  {name}\n"))
            .collect::<String>();
        push_banner(
            io,
            "MULTIPLE \"ONTO\" BRANCHES",
            &format!(
                "You have selected multiple branches to push changes onto. Pushing to multiple \
                 branches is not supported by \"arc land\" in Mercurial: Mercurial commits may \
                 only belong to one branch, so this operation can not be executed atomically.\n\nYou \
                 may land one branches and any number of bookmarks in a single operation.\n\nThese \
                 branches were selected:\n\n{list}"
            ),
        );
        return Err(
            "Landing onto multiple branches at once is not supported in Mercurial.".to_string(),
        );
    }
    let new_markers: Vec<&String> = onto_refs
        .iter()
        .filter(|name| !hg_remote_ref_exists(dir, onto_remote, name))
        .collect();
    if !new_markers.is_empty() {
        let count = new_markers.len();
        let list = new_markers
            .iter()
            .map(|name| format!("  {name}\n"))
            .collect::<String>();
        push_banner(
            io,
            &format!("CREATE {count} BOOKMARK(S)"),
            &format!(
                "These {count} symbol(s) do not exist in the remote. They will be created as new \
                 bookmarks:\n\n{list}"
            ),
        );
        if hold {
            io.push_out(&format!(
                "You are using \"--hold\", so execution will stop before the {count} bookmark(s) \
                 are actually created. You will be given instructions to create the bookmarks.\n"
            ));
        }
        let query = format!("Create {count} new remote bookmark(s)?");
        prompt_for_key(io, land_checks::PROMPT_CREATE, &query)?;
    }
    Ok(branches.into_iter().next().cloned())
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
    match state {
        IntoState::Empty => {
            io.push_status("INTO COMMIT", "Preparing merge into the empty state.");
            Ok(None)
        }
        IntoState::Local(local_ref) => {
            let into_commit = hg_canonical(dir, local_ref)
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
            hg::output(dir, &["pull", "--rev", name, remote]).ok();
            if let Ok(commit) = hg_canonical(dir, name) {
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
        let output = hg::stdout_text(
            dir,
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
    let before = hg::stdout_text(dir, &["bookmarks"]).unwrap_or_default();
    let mut bookmark_positions: Vec<(String, String)> = Vec::new();
    for (name, _) in parse_hg_bookmarks(&before) {
        if let Some(position) = hg_bookmark_position(dir, &name) {
            bookmark_positions.push((name, position));
        }
    }
    hg::stdout_text(dir, &["update", "--rev", into_commit])?;
    if let Some(branch) = branch_marker {
        hg::stdout_text(dir, &["branch", "--", branch])?;
    }
    let min = &commits[set.commits[0]].hash;
    let max = &commits[set.commits[set.commits.len() - 1]].hash;
    let message = message_of(set)?;
    let range = format!("{min}..{max}");
    let wrote = hg_write_stdin(
        dir,
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
        hg::output(dir, &["rebase", "--abort"]).ok();
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
            hg::output(dir, &["bookmark", "--force", "--rev", position, "--", name]).ok();
        }
    }
    let tip = hg::stdout_text(dir, &["log", "--rev", "tip", "--template", "{node}"])?;
    Ok(tip.trim().to_string())
}

/// Current position of bookmark `name`, if any.
fn hg_bookmark_position(dir: &Path, name: &str) -> Option<String> {
    let output = hg::stdout_text(dir, &["log", "--rev", name, "--template", "{node}"]).ok()?;
    let trimmed = output.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Run `hg` with `args` in `dir`, writing `stdin_text` to its stdin.
/// Returns success.
fn hg_write_stdin(dir: &Path, args: &[&str], stdin_text: &str) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = std::process::Command::new("hg");
    command
        .current_dir(dir)
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
    _io: &mut Io,
    onto_remote: &str,
    bookmarks: &[String],
    into_commit: &str,
) -> Result<(), String> {
    let mut restore: Vec<(String, String)> = Vec::new();
    for bookmark in bookmarks {
        let old = hg_bookmark_position(dir, bookmark);
        if old.as_deref() == Some(into_commit) {
            continue;
        }
        hg::stdout_text(
            dir,
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
    let pushed = hg::output(dir, &argv).is_ok_and(|out| out.status.success());
    if !pushed {
        for (bookmark, old) in &restore {
            hg::output(dir, &["bookmark", "--force", "--rev", old]).ok();
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
        hg::output(dir, &pull).ok();
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
    if !squash {
        return Ok(());
    }
    let old = &commits[set.commits[set.commits.len() - 1]].hash;
    let output = hg::stdout_text(
        dir,
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
        if hg::output(
            dir,
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
            hg::output(dir, &["rebase", "--abort"]).ok();
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
    let before = hg::stdout_text(dir, &["bookmarks"]).unwrap_or_default();
    for (name, _) in parse_hg_bookmarks(&before) {
        let position = hg_bookmark_position(dir, &name);
        if position.is_some_and(|hash| obsolete.contains(&hash.as_str())) {
            io.push_status("CLEANUP", &format!("Deleting bookmark \"{name}\"."));
            hg::stdout_text(dir, &["bookmark", "--delete", "--", &name])?;
        }
    }
    let revset = format!("({})", revs.join(") or ("));
    hg::stdout_text(dir, &["strip", "--rev", &revset])?;
    Ok(())
}

/// Update back to the original commit or bookmark after landing.
///
/// Ports the not-rebased half of `reconcileLocalState()`
/// (`ArcanistMercurialLandEngine.php:1178-1198`); the rebased-active-commit
/// half is not ported (see the module docs on local state).
pub fn hg_reconcile(dir: &Path, local_commit: &str, local_bookmark: Option<&str>) {
    let marker = local_bookmark.unwrap_or(local_commit);
    hg::output(dir, &["update", "--", marker]).ok();
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
    push_banner(
        io,
        "HOLD CHANGES",
        "Holding changes locally, they have not been pushed.",
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
    io.push_out(&format!(
        "To push changes manually, run these {} command(s):\n\n",
        commands.len()
    ));
    for command in &commands {
        io.push_out(&format!("    $ {command}\n"));
    }
    io.push_out("\n");
    if !restore.is_empty() {
        io.push_out(&format!(
            "To go back to how things were before you ran \"arc land\", run these {} \
             command(s):\n\n",
            restore.len()
        ));
        for command in restore {
            io.push_out(&format!("    $ {command}\n"));
        }
        io.push_out("\n");
    }
    io.push_out(
        "Local branches and bookmarks have not been changed, and are still in the same state as \
         before.\n",
    );
}

/// Render one commit set: the revision line plus its commits
/// (`ArcanistLandEngine::printCommitSet()`, `:768-796`).
///
/// Implicit commits are marked with `!` (plain-text rendering of the
/// yellow highlight, which PHP notes survives copy/paste).
pub fn render_commit_set(
    set: &CommitSet,
    commits: &[LandCommit],
    display_hash: &dyn Fn(&str) -> String,
) -> String {
    let mut text = String::new();
    match &set.revision {
        Some(revision) => {
            text.push_str(&format!("\n  {} {}\n", revision.monogram, revision.name));
        }
        None => text.push_str("\n  (no associated revision)\n"),
    }
    for index in &set.commits {
        let commit = &commits[*index];
        let short = display_hash(&commit.hash);
        let summary = display_summary(&commit.summary);
        if commit.is_implicit {
            text.push_str(&format!("       ! {short} {summary}\n"));
        } else {
            text.push_str(&format!("          {short}  {summary}\n"));
        }
    }
    text
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
    push_banner(
        io,
        "IMPLICIT COMMITS",
        &format!(
            "Some commits reachable from the specified sources ({symbols_display}) are not \
             associated with revisions, and may not have been reviewed. These commits will be \
             landed as though they belong to the nearest ancestor revision:"
        ),
    );
    for set in &implicit {
        io.push_out(&render_commit_set(set, commits, display_hash));
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
    /// Viewer name from `user.whoami` (`userName`, else the PHID).
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
    let name = result
        .get("userName")
        .and_then(|name| name.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| phid.clone());
    Ok(ViewerIdentity { phid, name })
}

pub fn confirm_revision_gates(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    viewer: &ViewerIdentity,
    links: &[land_checks::ParentLink],
    builds: &[land_checks::BuildView],
    findings: &[land_checks::GateFinding],
) -> Result<(), String> {
    for finding in findings {
        match finding.prompt {
            land_checks::PROMPT_UNAUTHORED => {
                confirm_unauthored(io, revisions, viewer)?;
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
            land_checks::PROMPT_LARGE_WORKING_SET => {
                prompt_for_key(io, finding.prompt, &finding.query)?;
            }
            _ => {
                prompt_for_key(io, finding.prompt, &finding.query)?;
            }
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
fn confirm_unauthored(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    viewer: &ViewerIdentity,
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
        io.push_out(&format!("  {} {}\n", revision.monogram, revision.name));
        io.push_out(&format!("  Author: {}\n", revision.author_phid));
    }
    io.push_out("Use \"Commandeer\" in the web interface to become the author of a revision.\n\n");
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
    let (title, intro) = match group {
        StatusGroup::Planned => (
            format!("{count} REVISION(S) HAVE CHANGES PLANNED"),
            format!(
                "You are landing {count} revision(s) which are currently in a state indicating \
                 that you expect to revise them before moving forward.\n\nNormally, you should \
                 update these {count} revision(s), submit them for review, and wait for reviewers \
                 to accept them before you continue. To resubmit a revision for review, either: \
                 update the revision with revised changes; or use \"Request Review\" from the web \
                 interface.\n\nThese {count} revision(s) have changes planned:"
            ),
        ),
        StatusGroup::Published => (
            format!("{count} REVISION(S) ARE ALREADY PUBLISHED"),
            format!(
                "You are landing {count} revision(s) which are already in a published state, \
                 indicating that they have previously landed:"
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
        io.push_out(&format!("  {} {}\n", revision.monogram, revision.name));
        if group == StatusGroup::NotAccepted {
            io.push_out(&format!("  Status: {}\n", revision.status_name));
        }
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
        &format!("{count} OPEN PARENT REVISION(S)"),
        &format!(
            "The changes you are landing depend on {count} open parent revision(s). Usually, you \
             should land parent revisions before landing the changes which depend on them. These \
             parent revisions are open:"
        ),
    );
    for group in &groups {
        io.push_out(&format!(
            "  {} {}\n",
            group.parent.monogram, group.parent.name
        ));
        io.push_out(&format!("  Status: {}\n", group.parent.status_name));
        for child in &group.children {
            if let Some(revision) = revisions.get(*child) {
                io.push_out(&format!(
                    "  Parent of: {} {}\n",
                    revision.monogram, revision.name
                ));
            }
        }
    }
    prompt_for_key(io, finding.prompt, &finding.query)
}

/// Print the builds banner and prompt (`:646-717`).
fn confirm_builds(
    io: &mut Io,
    revisions: &[land_checks::RevisionView],
    builds: &[land_checks::BuildView],
    finding: &land_checks::GateFinding,
) -> Result<(), String> {
    let verdict = land_checks::classify_builds(builds);
    let count = verdict.affected_revisions;
    if verdict.failures > 0 {
        let message = if verdict.ongoing > 0 {
            format!("{count} revision(s) have build failures or ongoing builds:")
        } else {
            format!("{count} revision(s) have build failures:")
        };
        push_banner(io, "BUILD FAILURES", &message);
    } else {
        push_banner(
            io,
            "ONGOING BUILDS",
            &format!("{count} revision(s) have ongoing builds:"),
        );
    }
    for revision in revisions {
        io.push_out(&format!("  {} {}\n", revision.monogram, revision.name));
    }
    for build in builds {
        io.push_out(&format!(
            "  Build {} for {}\n",
            build.status, build.revision_phid
        ));
    }
    prompt_for_key(io, finding.prompt, &finding.query)
}

/// Fail unless the working tree has no changes.
///
/// Git reads `git status --porcelain`; Mercurial reads `hg status`. A
/// dirty tree fails with the port safeguard error (PHP stashes via local
/// state, which is not ported; see the module docs).
pub fn ensure_clean_tree(dir: &Path, vcs: Vcs) -> Result<(), String> {
    let dirty = match vcs {
        Vcs::Git => !git::stdout_text(dir, &["status", "--porcelain"])?.is_empty(),
        Vcs::Hg => {
            let output = hg::stdout_text(dir, &["status"]).unwrap_or_default();
            !hg::parse_mercurial_status(&output)
                .map(|map| map.is_empty())
                .unwrap_or(true)
        }
    };
    if dirty {
        return Err(
            "working tree is not clean; commit or stash your changes first (or use --preview)"
                .to_string(),
        );
    }
    Ok(())
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
        io.push_out(&render_commit_set(set, commits, display_hash));
    }
}

/// Run the land engine (`ArcanistLandEngine::execute()`,
/// `ArcanistLandEngine.php:1101-1333`).
///
/// `options.refs` selects the sources (else VCS defaults); then onto
/// remote/refs, into state/commit, commit selection, revision discovery
/// (online) or the offline single set, set filtering, the landing list,
/// preview-or-confirm, gate confirms, and the merge/push loop.
fn execute_engine(
    dir: &Path,
    options: &Options,
    vcs: Vcs,
    validated: &Validated,
    io: &mut Io,
) -> Result<(), String> {
    let squash = validated.strategy == Strategy::Squash;
    // Sources and remotes.
    let (raw_symbols, onto_remote, onto_refs, into_commit, into_ref_for_reconcile, hg_branch) =
        select_targets(dir, options, vcs, io)?;
    let symbols_display = display_symbols(&raw_symbols);
    // Commits.
    let resolved = resolve_sources(dir, io, vcs, &raw_symbols)?;
    let mut commits = select_source_commits(dir, vcs, &resolved, into_commit.as_deref())?;
    link_commits(&mut commits);
    let into_display = into_commit.as_deref().map(|hash| match vcs {
        Vcs::Git => git::display_hash(hash).to_string(),
        Vcs::Hg => display_hash_12(hash),
    });
    confirm_commit_count(
        io,
        &commits,
        &symbols_display,
        into_display.as_deref(),
        &hash_display_for(vcs),
    )?;
    // Conduit: online discovery, or the offline single set with a note.
    let client = client_for_dir(options.conduit_uri.as_deref(), dir);
    let online = client.as_ref().ok();
    if online.is_none()
        && let Err(reason) = &client
    {
        io.push_err(&format_skip_note(reason));
    }
    let mut sets = build_sets(
        dir,
        io,
        vcs,
        validated,
        &mut commits,
        online,
        &hash_display_for(vcs),
    )?;
    sets = filter_commit_sets(
        io,
        sets,
        &commits,
        squash,
        options.pick,
        &render_for(vcs, &commits),
    );
    if !options.preview {
        confirm_implicit_sets(
            io,
            &sets,
            &commits,
            &symbols_display,
            &hash_display_for(vcs),
        )?;
    }
    print_landing(io, &sets, &commits, &hash_display_for(vcs));
    if options.preview {
        io.push_status("PREVIEW", "Completed preview of land operation.");
        return Ok(());
    }
    ensure_clean_tree(dir, vcs)?;
    prompt_for_key(io, land_checks::PROMPT_CONFIRM, "Land these changes?")?;
    if let Some(client) = online {
        let call = |method: &str, params: &serde_json::Value| {
            client.call(method, params).map_err(|e| e.to_string())
        };
        confirm_online_gates(io, &call, &sets, &commits)?;
    }
    match vcs {
        Vcs::Git => land_git(
            dir,
            io,
            options,
            squash,
            &raw_symbols,
            &onto_remote,
            &onto_refs,
            into_commit.as_deref(),
            into_ref_for_reconcile.as_deref(),
            &sets,
            &commits,
            online,
        ),
        Vcs::Hg => land_hg(
            dir,
            io,
            options,
            squash,
            &raw_symbols,
            &onto_remote,
            &onto_refs,
            into_commit.as_deref(),
            hg_branch.as_deref(),
            &sets,
            &commits,
            online,
        ),
    }
}

/// Display-hash resolver per VCS.
fn hash_display_for(vcs: Vcs) -> Box<dyn Fn(&str) -> String> {
    match vcs {
        Vcs::Git => Box::new(|hash| git::display_hash(hash).to_string()),
        Vcs::Hg => Box::new(display_hash_12),
    }
}

/// Set renderer per VCS (borrows the commit list).
fn render_for<'a>(vcs: Vcs, commits: &'a [LandCommit]) -> Box<dyn Fn(&CommitSet) -> String + 'a> {
    match vcs {
        Vcs::Git => Box::new(|set| {
            render_commit_set(set, commits, &|hash| git::display_hash(hash).to_string())
        }),
        Vcs::Hg => Box::new(|set| render_commit_set(set, commits, &display_hash_12)),
    }
}

/// Source symbols, onto remote/refs, and the into commit, per VCS.
#[allow(clippy::type_complexity)]
fn select_targets(
    dir: &Path,
    options: &Options,
    vcs: Vcs,
    io: &mut Io,
) -> Result<
    (
        Vec<String>,
        String,
        Vec<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    String,
> {
    match vcs {
        Vcs::Git => {
            let raw = if options.refs.is_empty() {
                git_default_symbols(dir, io)?
            } else {
                options.refs.clone()
            };
            let onto_remote = git_select_onto_remote(dir, io, options, &raw)?;
            let onto_refs = git_select_onto_refs(dir, io, options, &raw)?;
            git_confirm_onto_refs(dir, io, &onto_remote, &onto_refs, options.hold)?;
            let into_state = select_into_state(io, options, &onto_remote, &onto_refs, &|remote| {
                git::remote_fetch_uri(dir, remote).is_some()
            })?;
            let into_ref = match &into_state {
                IntoState::Remote { name, .. } => Some(name.clone()),
                _ => None,
            };
            let into_commit =
                git_select_into_commit(dir, io, &into_state, options.into.as_deref())?;
            Ok((raw, onto_remote, onto_refs, into_commit, into_ref, None))
        }
        Vcs::Hg => {
            let raw = if options.refs.is_empty() {
                hg_default_symbols(dir, io)?
            } else {
                options.refs.clone()
            };
            let onto_remote = hg_select_onto_remote(dir, io, options)?;
            let onto_refs = hg_select_onto_refs(dir, io, options)?;
            let branch = hg_confirm_onto_refs(dir, io, &onto_remote, &onto_refs, options.hold)?;
            let into_state = select_into_state(io, options, &onto_remote, &onto_refs, &|remote| {
                hg::ok(dir, &["paths", remote])
            })?;
            let into_commit = hg_select_into_commit(dir, io, &into_state, options.into.as_deref())?;
            Ok((raw, onto_remote, onto_refs, into_commit, None, branch))
        }
    }
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
    let forced = validated
        .revision
        .as_ref()
        .map(|symbol| resolve_forced_revision(&call, symbol))
        .transpose()?;
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
/// interactively.
fn confirm_online_gates(
    io: &mut Io,
    call: &diff_submit::ConduitCall<'_>,
    sets: &[CommitSet],
    commits: &[LandCommit],
) -> Result<(), String> {
    let revisions: Vec<land_checks::RevisionView> =
        sets.iter().filter_map(|set| set.revision.clone()).collect();
    let viewer = fetch_viewer_identity(call).unwrap_or(ViewerIdentity {
        phid: revisions
            .first()
            .map(|revision| revision.author_phid.clone())
            .unwrap_or_default(),
        name: String::new(),
    });
    let links = fetch_parent_links(call, &revisions)?;
    let landing: Vec<&str> = revisions.iter().map(|rev| rev.phid.as_str()).collect();
    let builds = fetch_build_views(call, &revisions)?;
    let findings = land_checks::evaluate_gates(
        &revisions,
        &viewer.phid,
        &links,
        &landing,
        &builds,
        Some(commits.len()),
    );
    confirm_revision_gates(io, &revisions, &viewer, &links, &builds, &findings)
}

/// Run the git merge/push loop
/// (`ArcanistLandEngine.php:1234-1332`, git halves).
///
/// Merges every set, pushing per `--hold`/`--incremental`, cascading and
/// pruning after each push, then reconciles (or prints hold instructions)
/// and `DONE`. Failures check the original ref back out best-effort.
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
    let original = git::branch_name(dir)
        .ok()
        .flatten()
        .or_else(|| git::working_copy_revision(dir).ok());
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
        original.as_deref(),
    );
    if outcome.is_err()
        && let Some(restore) = original.as_deref()
    {
        git::output(dir, &["checkout", restore, "--"]).ok();
    }
    outcome
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
    original: Option<&str>,
) -> Result<(), String> {
    let mut cursor = into_commit.map(str::to_string);
    let mut has_unpushed = false;
    let mut need_cascade: Vec<CommitSet> = Vec::new();
    let mut need_prune: Vec<CommitSet> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    let last = sets.len().saturating_sub(1);
    for (index, set) in sets.iter().enumerate() {
        need_prune.push(set.clone());
        need_cascade.push(set.clone());
        let message_of = git_message_closure(raw_symbols, onto_refs, commits, online);
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
        let should_push = if options.hold {
            false
        } else if options.incremental {
            true
        } else {
            index == last
        };
        if should_push {
            git_push_change(
                dir,
                io,
                onto_remote,
                onto_refs,
                cursor_or(cursor.as_deref()),
            )?;
            has_unpushed = false;
            need_cascade.reverse();
            for cascade_set in &need_cascade {
                git_cascade_state(
                    dir,
                    io,
                    cascade_set,
                    commits,
                    cursor_or(cursor.as_deref()),
                    squash,
                );
            }
            need_cascade.clear();
            if !options.keep_branches {
                deleted.extend(git_prune_branches(dir, io, &need_prune, commits));
                need_prune.clear();
            }
        }
    }
    if options.hold {
        let restore: Vec<String> = original
            .map(|name| format!("git checkout {name} --"))
            .into_iter()
            .collect();
        git_hold_text(
            io,
            onto_remote,
            onto_refs,
            cursor_or(cursor.as_deref()),
            &restore,
        );
        return Ok(());
    }
    git_reconcile(dir, original, into_ref, onto_refs, &deleted);
    io.push_status("DONE", "Landed changes.");
    Ok(())
}

/// Landed-message closure: Differential messages online, the offline
/// message otherwise.
type SetMessage<'a> = Box<dyn Fn(&CommitSet) -> Result<String, String> + 'a>;

fn git_message_closure<'a>(
    raw_symbols: &'a [String],
    onto_refs: &'a [String],
    commits: &'a [LandCommit],
    online: Option<&'a arc_rs::conduit::Client>,
) -> SetMessage<'a> {
    Box::new(move |set| match (online, &set.revision) {
        (Some(client), Some(revision)) => {
            let call = |method: &str, params: &serde_json::Value| {
                client.call(method, params).map_err(|e| e.to_string())
            };
            fetch_commit_message(&call, revision.id)
        }
        _ => {
            let members: Vec<LandCommit> = set
                .commits
                .iter()
                .map(|index| commits[*index].clone())
                .collect();
            Ok(offline_message(raw_symbols, onto_refs, &members))
        }
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
    let local_commit = hg::stdout_text(dir, &["log", "-l", "1", "--template", "{node}", "-r", "."])
        .unwrap_or_default();
    let bookmarks = hg::stdout_text(dir, &["bookmarks"]).unwrap_or_default();
    let local_bookmark = parse_hg_bookmarks(&bookmarks)
        .into_iter()
        .find(|(_, active)| *active)
        .map(|(name, _)| name);
    let outcome = land_hg_inner(
        dir,
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
    );
    if outcome.is_err() {
        hg_reconcile(dir, &local_commit, local_bookmark.as_deref());
    }
    outcome
}

/// The Mercurial merge/push loop proper (see [`land_hg`]).
#[allow(clippy::too_many_arguments)]
fn land_hg_inner(
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
    let push_bookmarks: Vec<String> = onto_refs
        .iter()
        .filter(|name| Some(name.as_str()) != branch_marker)
        .cloned()
        .collect();
    let mut cursor = into_commit.map(str::to_string);
    let mut need_cascade: Vec<CommitSet> = Vec::new();
    let mut need_prune: Vec<CommitSet> = Vec::new();
    let last = sets.len().saturating_sub(1);
    for (index, set) in sets.iter().enumerate() {
        need_prune.push(set.clone());
        need_cascade.push(set.clone());
        let message_of = git_message_closure(raw_symbols, onto_refs, commits, online);
        let merged = hg_execute_merge(
            dir,
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
            hg_push_change(
                dir,
                io,
                onto_remote,
                &push_bookmarks,
                cursor_or(cursor.as_deref()),
            )?;
            need_cascade.reverse();
            for cascade_set in &need_cascade {
                hg_cascade_state(
                    dir,
                    cascade_set,
                    commits,
                    cursor_or(cursor.as_deref()),
                    squash,
                )?;
            }
            need_cascade.clear();
            if !options.keep_branches {
                hg_prune_branches(dir, io, &need_prune, commits)?;
                need_prune.clear();
            }
        }
    }
    if options.hold {
        let local_commit =
            hg::stdout_text(dir, &["log", "-l", "1", "--template", "{node}", "-r", "."])
                .unwrap_or_default();
        hg_hold_text(
            io,
            &push_bookmarks,
            onto_remote,
            cursor_or(cursor.as_deref()),
            &[format!("hg update -- {local_commit}")],
        );
        return Ok(());
    }
    let local_commit = hg::stdout_text(dir, &["log", "-l", "1", "--template", "{node}", "-r", "."])
        .unwrap_or_default();
    let bookmarks = hg::stdout_text(dir, &["bookmarks"]).unwrap_or_default();
    let local_bookmark = parse_hg_bookmarks(&bookmarks)
        .into_iter()
        .find(|(_, active)| *active)
        .map(|(name, _)| name);
    hg_reconcile(dir, &local_commit, local_bookmark.as_deref());
    io.push_status("DONE", "Landed changes.");
    Ok(())
}

/// Run the workflow with scripted or live IO, returning collected output.
///
/// Parse errors append the usage text (like the old port); `--help`
/// returns the usage text.
pub fn execute_with_io(dir: &Path, argv: &[String], io: &mut Io) -> Result<Out, String> {
    let options = match parse_options(argv) {
        Ok(Some(options)) => options,
        Ok(None) => {
            io.push_out(USAGE);
            return Ok(std::mem::take(&mut io.out));
        }
        Err(message) => return Err(format!("{message}\n\n{USAGE}")),
    };
    let Some(vcs) = detect_vcs(dir) else {
        return Err(NOT_GIT_OR_HG.to_string());
    };
    for (key, answer) in load_saved_prompts(dir) {
        io.saved.entry(key).or_insert(answer);
    }
    let validated = validate_options(&options, dir, io)?;
    execute_engine(dir, &options, vcs, &validated, io)?;
    Ok(std::mem::take(&mut io.out))
}

/// Run the workflow rooted at `dir` with buffered IO.
pub fn execute(dir: &Path, argv: &[String]) -> Result<Out, String> {
    execute_with_io(dir, argv, &mut Io::buffered())
}

/// Run the workflow rooted at `dir` with live console IO.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute_with_io(dir, argv, &mut Io::live()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("arc-rs land: {message}");
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
#[path = "land_tests.rs"]
mod land_tests;
