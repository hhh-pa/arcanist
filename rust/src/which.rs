//! `arc which` — explain the resolved config/repo context.
//!
//! PHP reference: `src/workflow/ArcanistWhichWorkflow.php` (`run()` at
//! `:57-261`), the base commit rules
//! (`src/parser/ArcanistBaseCommitParser.php`), and the git pieces of
//! `src/repository/api/ArcanistGitAPI.php`.
//!
//! Layout (T1153 RB-D16): the three ceb prose sections — `REPOSITORY`
//! with its association reasons, `COMMIT RANGE` with its prose, and
//! `MATCHING REVISIONS` — each `phutil_console_wrap()`-wrapped like
//! PHP, printed in stages so failures land after the sections PHP has
//! already printed. Association runs `repository.query` (T1153 RB-D17);
//! the range shows the 16-char base with its summary (T1153 RB-D18).
//!
//! Deviations (see T1128): only git working copies are detected — the
//! hg/svn repository APIs are not ported, so outside git the command
//! reports PHP's working-copy usage text (T1153 RB-D16, Rule 3: no Rust
//! hg/svn backend exists to drive). `--json` stays an arc-rs extension
//! (PHP rejects it); it shapes no default byte.
//!
//! Console colors follow the PHP markup sites on TTYs: every section
//! header renders bold (PHP wraps its `REPOSITORY`, `COMMIT RANGE`, and
//! `MATCHING REVISIONS` titles in `**`), as do the `created`/`updated`
//! verdicts in the guidance sentences. The multi-revision guidance stays
//! plain like PHP (it never passes through `phutil_console_format`).

extern crate self as arc_rs;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{
    conduit::{self, EnvelopeKind},
    config, get_config, git, terminal,
};

/// Subcommand name, matching `arc which`.
pub const NAME: &str = "which";

/// The empty-tree hash git uses as the "no base" marker
/// (`ArcanistGitAPI::GIT_MAGIC_ROOT_COMMIT`).
pub const GIT_MAGIC_ROOT_COMMIT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `arc_rs::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistWhichWorkflow.php:12-65`).
const HELP_SYNOPSIS: &str =
    "      **which** [options] (svn)\n      **which** [options] [__commit__] (hg, git)";
const HELP_TEXT: &str = "          Supports: svn, git, hg\n          Shows which repository the current working copy corresponds to,\n          which commits 'arc diff' will select, and which revision is in\n          the working copy (or which revisions, if more than one matches).";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[
    (
        "any-status",
        None,
        None,
        "Show committed and abandoned revisions.",
    ),
    (
        "base",
        None,
        Some("rules"),
        "Additional rules for determining base revision.",
    ),
    ("show-base", None, None, "Print base commit only and exit."),
    (
        "head",
        None,
        Some("commit"),
        "Specify the end of the commit range to select.",
    ),
];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<arc_rs::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    arc_rs::help::usage_text(
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
    usage_text(arc_rs::terminal::ansi_enabled())
}

/// One commit in the reported range (T1153 RB-D16: cloned into the
/// staged range render).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

/// One local commit with its tree hash, mirroring
/// `ArcanistGitAPI::getLocalCommitInformation()` rows.
pub struct LocalCommit {
    pub commit: String,
    pub tree: String,
    pub summary: String,
    pub message: String,
}

/// One Differential revision matching the working copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedRevision {
    pub monogram: String,
    pub title: String,
    pub author: Option<String>,
    pub why: String,
}

/// Everything `COMMIT RANGE` prints: the resolved base, its 16-char
/// display with summary, the `git diff` command line, and the commits
/// (T1153 RB-D18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeData {
    pub base: String,
    pub explanation: String,
    pub base16: String,
    pub summary: String,
    pub command: String,
    pub commits: Vec<Commit>,
    /// Resolved head SHA, like PHP's `getHeadCommit()`.
    pub head: String,
}

/// Resolved context for one directory (T1153 RB-D16: the staged
/// sections' shared model, rendered by [`render_ansi`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Directory the report was built for.
    pub dir: String,
    /// `git`, or `none` outside a working copy.
    pub vcs: &'static str,
    /// Repository top level, when in a repo.
    pub vcs_root: Option<String>,
    /// Nearest directory holding `.arcconfig`, if any.
    pub project_root: Option<String>,
    /// The `.arcconfig` file used, if any.
    pub project_file: Option<String>,
    /// Current branch short name, or `None` when detached.
    pub branch: Option<String>,
    /// HEAD full SHA.
    pub head: Option<String>,
    /// HEAD subject line.
    pub head_subject: Option<String>,
    /// `remote.origin.url`, if configured.
    pub remote_url: Option<String>,
    /// `repository.callsign` from any config source, if set.
    pub callsign: Option<String>,
    /// Resolved conduit URI, if any.
    pub conduit_uri: Option<String>,
    /// Where the conduit URI came from.
    pub conduit_source: &'static str,
    /// Whether `~/.arcrc` holds a token for the URI.
    pub token_installed: Option<bool>,
    /// Associated repository name, if any.
    pub repo_name: Option<String>,
    /// Association reason trail.
    pub repo_reasons: Vec<String>,
    /// Base commit of the range.
    pub base: Option<String>,
    /// Why that base was chosen.
    pub base_explanation: Option<String>,
    /// 16-char base display.
    pub base16: Option<String>,
    /// Base commit summary.
    pub summary: Option<String>,
    /// `git diff` command line for the range.
    pub command: Option<String>,
    /// End of the range (`--head` or `HEAD`).
    pub head_ref: String,
    /// Whether `--head` was given explicitly.
    pub head_explicit: bool,
    /// Commits in the range, newest first.
    pub commits: Vec<Commit>,
    /// Argument suffix for `arc diff` guidance (`' X'`, `' --head Y'`).
    pub diff_arg: String,
    /// Differential revisions matching the working copy.
    pub matching: Vec<MatchedRevision>,
}

/// Resolve the conduit URI with its source, mirroring
/// [`config::client_for`] precedence: flag, environment, then the merged
/// `phabricator.uri` (else `default`, like [`config::discover_from`]).
/// T1153 Wave-2 carry-over: the merged source label comes from
/// [`get_config::merged_value`], so user/system/local URIs are no longer
/// misreported as `project (.arcconfig)`.
#[must_use]
pub fn resolve_conduit_in(
    dir: &Path,
    flag: Option<&str>,
    env: Option<&str>,
) -> (Option<String>, &'static str) {
    if let Some(uri) = flag.filter(|v| !v.trim().is_empty()) {
        return (Some(uri.to_string()), "command line (--conduit-uri)");
    }
    if let Some(uri) = env.filter(|v| !v.trim().is_empty()) {
        return (Some(uri.to_string()), "environment (ARC_CONDUIT_URI)");
    }
    let user_file = config::arcrc_path();
    let opts = get_config::LoadOptions::default();
    let Ok(merged) = get_config::load_merged(dir, user_file.as_deref(), &opts) else {
        return (None, "none");
    };
    let hit = get_config::merged_value("phabricator.uri", &merged)
        .or_else(|| get_config::merged_value("default", &merged));
    match hit {
        Some((source, value)) => match value.as_str() {
            Some(uri) => (Some(uri.to_string()), merged_source_label(source, dir)),
            None => (None, "none"),
        },
        None => (None, "none"),
    }
}

/// Display label for a [`get_config::merged_value`] source, parallel to
/// the `read_callsign` labels. The local label names the VCS metadata
/// directory holding `arc/config`.
fn merged_source_label(source: &str, dir: &Path) -> &'static str {
    match source {
        get_config::SOURCE_RUNTIME => "command line (--config)",
        get_config::SOURCE_LOCAL => match get_config::find_vcs_root(dir).map(|(kind, _)| kind) {
            Some(get_config::VcsKind::Hg) => "local (.hg/arc/config)",
            Some(get_config::VcsKind::Svn) => "local (.svn/arc/config)",
            Some(get_config::VcsKind::Git) => "local (.git/arc/config)",
            None => "local",
        },
        get_config::SOURCE_PROJECT => "project (.arcconfig)",
        get_config::SOURCE_USER => "user (~/.arcrc)",
        get_config::SOURCE_SYSTEM => {
            if cfg!(windows) {
                "system (ProgramData\\Phabricator\\Arcanist\\config)"
            } else {
                "system (/etc/arcconfig)"
            }
        }
        _ => "default",
    }
}

/// Whether `user_file` (a `~/.arcrc`) holds a token for `uri`. Missing
/// or unreadable files count as missing tokens, never as errors.
#[must_use]
pub fn token_installed(user_file: Option<&Path>, uri: &str) -> bool {
    match user_file {
        Some(path) => config::token_for_file(path, uri).is_some(),
        None => false,
    }
}

/// Find the nearest `.arcconfig` above `dir`: `(root, file)`.
#[must_use]
/// Locate the project root and `.arcconfig` for `dir`, mirroring
/// `ArcanistWorkingCopyIdentity::newFromPathWithConfig()` (T1153
/// CORE-33): git and Mercurial read `.arcconfig` ONLY at the
/// working-copy root, Subversion takes the nearest `.arcconfig` from
/// `dir` up to the root, and outside any working copy there is no
/// project at all. Infallible like before: unreadable files simply miss.
pub fn find_project(dir: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    let Some((kind, root)) = get_config::find_vcs_root(dir) else {
        return (None, None);
    };
    if kind == get_config::VcsKind::Svn {
        for ancestor in dir.ancestors() {
            let file = ancestor.join(".arcconfig");
            if file.is_file() {
                return (Some(ancestor.to_path_buf()), Some(file));
            }
            if ancestor == root.as_path() {
                break;
            }
        }
        return (None, None);
    }
    let file = root.join(".arcconfig");
    if file.is_file() {
        return (Some(root), Some(file));
    }
    (None, None)
}

/// The working-copy gate (`scripts/arcanist.php:260-266` at ceb): usage
/// error, raised before conduit setup.
pub const WORKING_COPY_REQUIRED: &str =
    "This command must be run in a Git, Mercurial or Subversion working copy.";

/// The stale-server association reason (`ArcanistWorkflow.php:1958-1963`
/// at ceb).
const OUTDATED_PHABRICATOR_REASON: &str = "This version of Arcanist is more recent than the version of \
    Phabricator you are connecting to: the Phabricator install is out of date and does not have support \
    for identifying repositories by callsign or URI. Update Phabricator to enable these features.";

/// The no-match association reason (`ArcanistWorkflow.php:1970-1973` at
/// ceb).
const NO_REPOS_MATCHED_REASON: &str = "No repositories matched the query. Check that your configuration \
    is correct, or use \"repository.callsign\" to select a repository explicitly.";

/// One string-typed config value, mirroring `getConfigFromAllSources()`
/// plus `willReadValue()` for `type: string` keys (T1153 RB-D17/T1153
/// RB-D38, Rule 0: `ArcanistConfigurationManager.php:80-131` and
/// `ArcanistSettings.php:247-257` at ceb). Every present non-null
/// value must be a string (else the usage error below); the winner is
/// the first non-null value in runtime > local > project > user >
/// system > default order. JSON nulls read as missing, like PHP's
/// `idx()`.
fn string_config_winner(
    key: &str,
    merged: &get_config::MergedConfig,
) -> Result<Option<String>, String> {
    let maps = [
        &merged.runtime,
        &merged.sources.local,
        &merged.sources.project,
        &merged.sources.user,
        &merged.sources.system,
        &merged.sources.default,
    ];
    let mut winner: Option<String> = None;
    for map in maps {
        let Some(value) = map.get(key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let text = value
            .as_str()
            .ok_or_else(|| format!("Type of setting '{key}' must be string."))?;
        if winner.is_none() {
            winner = Some(text.to_string());
        }
    }
    Ok(winner)
}

/// `repository.callsign` from any config source, type-checked like PHP.
pub fn repository_callsign(merged: &get_config::MergedConfig) -> Result<Option<String>, String> {
    string_config_winner("repository.callsign", merged)
}

/// PHP `if ($spec)` truthiness for a rule spec: only `""` and `"0"` are
/// falsy (T1153 RB-D38, Rule 0: `buildBaseCommit()` gates on the raw
/// values, so whitespace-only specs still take the rules path).
fn is_truthy_rule_spec(spec: &str) -> bool {
    !spec.is_empty() && spec != "0"
}

/// How the working copy associated (or not) with a Phabricator
/// repository: the reason trail plus the unique match's name, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Association {
    pub reasons: Vec<String>,
    pub repo_name: Option<String>,
}

/// Identify the working-copy repository via `repository.query`,
/// mirroring `getRepositoryQuery()` + `loadRepositoryInformation()`
/// (T1153 RB-D17, Rule 0: `ArcanistWorkflow.php:1940-2044` at ceb):
/// configured callsign first, then the VCS UUID (git has none), then
/// the remote URI; no identifier means no query at all. Only a unique
/// match associates; an outdated server (`ERR-CONDUIT-CALL`) means
/// unassociated, while other conduit failures propagate.
pub fn load_association(
    client: &conduit::Client,
    dir: &Path,
    merged: &get_config::MergedConfig,
) -> Result<Association, String> {
    let mut reasons = Vec::new();
    let callsign = repository_callsign(merged)?;
    let query = match callsign {
        Some(name) if is_truthy_rule_spec(&name) => {
            reasons.push(format!(
                "Configuration value \"repository.callsign\" is set to \"{name}\"."
            ));
            serde_json::json!({"callsigns": [name]})
        }
        _ => {
            reasons.push("Configuration value \"repository.callsign\" is empty.".to_string());
            // Git never overrides `getRepositoryUUID()` (the base
            // `ArcanistRepositoryAPI.php:660` returns null).
            reasons
                .push("This repository has no VCS UUID (this is normal for git/hg).".to_string());
            match git::remote_uri(dir)? {
                Some(uri) => {
                    reasons.push(format!(
                        "The remote URI for this working copy is \"{uri}\"."
                    ));
                    serde_json::json!({"remoteURIs": [uri]})
                }
                None => {
                    reasons.push(
                        "Unable to determine the remote URI for this repository.".to_string(),
                    );
                    return Ok(Association {
                        reasons,
                        repo_name: None,
                    });
                }
            }
        }
    };
    let results = match client.call("repository.query", &query) {
        Ok(results) => results,
        Err(conduit::Error::Api { code, .. }) if code == "ERR-CONDUIT-CALL" => {
            reasons.push(OUTDATED_PHABRICATOR_REASON.to_string());
            return Ok(Association {
                reasons,
                repo_name: None,
            });
        }
        Err(error) => return Err(error.to_string()),
    };
    let rows = query_rows(&results);
    let repo_name = if rows.is_empty() {
        reasons.push(NO_REPOS_MATCHED_REASON.to_string());
        None
    } else if rows.len() > 1 {
        // Sic `repostories` (`ArcanistWorkflow.php:1975` at ceb).
        let callsigns = rows
            .iter()
            .map(|row| row_string(row, "callsign"))
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!(
            "Multiple repostories ({callsigns}) matched the query. You can use the \
             \"repository.callsign\" configuration to select the one you want."
        ));
        None
    } else {
        reasons.push("Found a unique matching repository.".to_string());
        let name = row_string(rows[0], "name");
        (!name.is_empty()).then_some(name)
    };
    Ok(Association { reasons, repo_name })
}

/// Resolve `rev` to a full commit SHA.
fn rev_to_commit(dir: &Path, rev: &str) -> Option<String> {
    let arg = format!("{rev}^{{commit}}");
    let out = git::output(dir, &["rev-parse", "--verify", &arg]).ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// Missing-commit usage text (`ArcanistGitAPI.php:223,418`).
fn unknown_commit_message(symbolic: &str) -> String {
    format!("Unable to find any git commit named '{symbolic}' in this repository.")
}

/// True for the usage-class errors, which render `Usage Exception:`
/// while most resolution failures are generic exceptions (T1153 RB-D4,
/// Rule 0: the classes come from the ceb throw sites —
/// `ArcanistGitAPI.php:221,269`, `ArcanistBaseCommitParser.php:23,111`,
/// `ArcanistDifferentialCommitMessage.php:139`,
/// `ArcanistSettings.php:252`, `scripts/arcanist.php:262`).
fn is_usage_message(message: &str) -> bool {
    message.starts_with("Unable to find any git commit named '")
        || message.starts_with("The specified commit range is empty, backward or invalid")
        || message.starts_with("None of the rules in your 'base' configuration")
        || (message.starts_with("Rule '")
            && message.contains("is invalid, it must have a type and name"))
        || message.ends_with("is not a recognized rule.")
        || message.starts_with("Invalid \"Differential Revision\" field in commit message.")
        || message.starts_with("Specify exactly one base commit.")
        || message.starts_with(WORKING_COPY_REQUIRED)
        || message.starts_with("Type of setting '")
}

/// Report a resolution failure through its envelope arm (T1153 RB-D4).
fn report_which_error(message: &str) -> ExitCode {
    let kind = if is_usage_message(message) {
        EnvelopeKind::Usage
    } else {
        EnvelopeKind::Exception
    };
    conduit::report(kind, message)
}

/// Resolve the range head like `getHeadCommit()` (`rev-parse` plus the
/// missing-commit usage error, T1153 RB-D21).
fn resolve_head(dir: &Path, head_ref: &str) -> Result<String, String> {
    let out = git::output(dir, &["rev-parse", head_ref])?;
    if !out.status.success() {
        return Err(unknown_commit_message(head_ref));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `base` rules from every config source, mirroring
/// `getConfigFromAllSources('base')`.
pub struct BaseSources {
    pub local: String,
    pub project: String,
    pub user: String,
    pub system: String,
}

impl BaseSources {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            local: String::new(),
            project: String::new(),
            user: String::new(),
            system: String::new(),
        }
    }
}

/// `base` rules from loaded sources (`local`, `project`, `user`,
/// `system`): plain per-source reads (T1153 RB-D38, Rule 0:
/// `getConfigFromAllSources()` reads each source by exact key).
/// Callers type-check first via [`string_config_winner`], so present
/// values are strings here and missing ones read empty.
#[must_use]
pub fn base_sources_from(loaded: &get_config::Sources) -> BaseSources {
    let at = |map: &serde_json::Map<String, serde_json::Value>| {
        map.get("base")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    };
    BaseSources {
        local: at(&loaded.local),
        project: at(&loaded.project),
        user: at(&loaded.user),
        system: at(&loaded.system),
    }
}

/// Split a `base` specification into rules, mirroring
/// `tokenizeBaseCommitSpecification()`
/// (`ArcanistBaseCommitParser.php:13-30` at ceb, T1153 RB-D38e:
/// `array_filter` drops falsy rules, so `0` vanishes silently).
pub fn tokenize_base_rules(raw: &str) -> Result<Vec<String>, String> {
    let mut rules = Vec::new();
    for rule in raw
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty() && *rule != "0")
    {
        if !rule.contains(':') {
            return Err(format!(
                "Rule '{rule}' is invalid, it must have a type and name like 'arc:upstream'."
            ));
        }
        rules.push(rule.to_string());
    }
    Ok(rules)
}

fn unknown_rule(rule: &str, source: &str) -> String {
    format!("Base commit rule '{rule}' (from source '{source}') is not a recognized rule.")
}

/// One rule's outcome: move to the next ruleset, try the next rule, or a
/// resolved commit.
enum RuleOutcome {
    NextRuleset,
    NoMatch,
    Commit(String),
}

struct SourceRules {
    name: &'static str,
    rules: VecDeque<String>,
}

/// The `arc:base` rules engine, mirroring `ArcanistBaseCommitParser` with
/// git rule resolution (`ArcanistGitAPI::resolveBaseCommitRule()`).
struct Engine<'a> {
    dir: &'a Path,
    project_root: Option<PathBuf>,
    prompt: &'a dyn Fn(&str) -> Option<String>,
    verbose: bool,
    retry: Vec<&'static str>,
    explanation: Option<String>,
}

impl<'a> Engine<'a> {
    fn new(
        dir: &'a Path,
        project_root: Option<PathBuf>,
        prompt: &'a dyn Fn(&str) -> Option<String>,
    ) -> Self {
        Self {
            dir,
            project_root,
            prompt,
            verbose: false,
            retry: Vec::new(),
            explanation: None,
        }
    }

    fn log(&self, message: &str) {
        if self.verbose {
            eprintln!("{message}");
        }
    }

    fn explain(&mut self, explanation: String) {
        self.explanation = Some(explanation);
    }

    /// Resolve across sources, returning the commit (explanation lands in
    /// `self.explanation`).
    fn resolve(&mut self, all: &[SourceRules]) -> Result<Option<String>, String> {
        let mut queues: Vec<(&'static str, VecDeque<String>)> =
            all.iter().map(|s| (s.name, s.rules.clone())).collect();
        self.retry = vec!["runtime", "local", "project", "user", "system"];
        while let Some(source) = self.retry.first().copied() {
            let queue = queues
                .iter_mut()
                .find(|(name, _)| *name == source)
                .map(|(_, q)| q);
            let Some(queue) = queue else {
                self.retry.remove(0);
                continue;
            };
            if queue.is_empty() {
                self.log(&format!("No rules left from source '{source}'."));
                self.retry.remove(0);
                continue;
            }
            self.log(&format!("Trying rules from source '{source}'."));
            while let Some(rule) = queue.pop_front() {
                self.log(&format!("Trying rule '{rule}'."));
                match self.resolve_rule(&rule, source)? {
                    RuleOutcome::NextRuleset => break,
                    RuleOutcome::NoMatch => {}
                    RuleOutcome::Commit(commit) => {
                        self.log(&format!("Resolved commit '{commit}' from rule '{rule}'."));
                        return Ok(Some(commit));
                    }
                }
            }
        }
        Ok(None)
    }

    fn resolve_rule(&mut self, rule: &str, source: &'static str) -> Result<RuleOutcome, String> {
        let Some((kind, name)) = rule.split_once(':') else {
            return Err(format!(
                "Rule '{rule}' is invalid, it must have a type and name like 'arc:upstream'."
            ));
        };
        match kind {
            "literal" => Ok(RuleOutcome::Commit(name.to_string())),
            "git" => Ok(self
                .resolve_git_rule(rule, name, source)?
                .map_or(RuleOutcome::NoMatch, RuleOutcome::Commit)),
            "hg" => Ok(RuleOutcome::NoMatch),
            "arc" => self.resolve_arc_rule(rule, name, source),
            _ => Err(unknown_rule(rule, source)),
        }
    }

    fn translate_legacy_arc_name<'b>(&mut self, name: &'b str) -> &'b str {
        match name {
            "global" => {
                self.log("Translating legacy name 'global' to 'user'");
                "user"
            }
            "args" => {
                self.log("Translating legacy name 'args' to 'runtime'");
                "runtime"
            }
            other => other,
        }
    }

    fn resolve_arc_verbose(&mut self) -> Result<RuleOutcome, String> {
        self.verbose = true;
        self.log("Enabled verbose mode.");
        Ok(RuleOutcome::NoMatch)
    }

    fn resolve_arc_prompt(&mut self) -> Result<RuleOutcome, String> {
        self.explain("it is what you typed when prompted.".to_string());
        let answer = (self.prompt)("Against which commit?").unwrap_or_default();
        if take_prompt_tty_failed() {
            return Err(crate::prompt::NOT_INTERACTIVE.to_string());
        }
        if answer.is_empty() {
            Ok(RuleOutcome::NoMatch)
        } else {
            Ok(RuleOutcome::Commit(answer))
        }
    }

    fn resolve_arc_source(&mut self, name: &str) -> Result<RuleOutcome, String> {
        let target: &'static str = match name {
            "local" => "local",
            "user" => "user",
            "project" => "project",
            "runtime" => "runtime",
            _ => "system",
        };
        self.retry.insert(0, target);
        self.log(&format!("Switching to source '{name}'."));
        Ok(RuleOutcome::NextRuleset)
    }

    fn resolve_arc_flow(&mut self, name: &str, source: &str) -> Result<RuleOutcome, String> {
        match name {
            "yield" => {
                let head = self.retry.remove(0);
                self.retry.push(head);
                self.log(&format!("Yielding processing of rules from '{source}'."));
                Ok(RuleOutcome::NextRuleset)
            }
            "halt" => {
                self.retry.clear();
                self.log("Halting all rule processing.");
                Ok(RuleOutcome::NextRuleset)
            }
            _ => Ok(RuleOutcome::NoMatch),
        }
    }

    fn resolve_arc_git(
        &mut self,
        rule: &str,
        name: &str,
        source: &'static str,
    ) -> Result<RuleOutcome, String> {
        Ok(self
            .resolve_git_rule(rule, name, source)?
            .map_or(RuleOutcome::NoMatch, RuleOutcome::Commit))
    }

    fn resolve_arc_fallback(
        &mut self,
        rule: &str,
        name: &str,
        source: &'static str,
    ) -> Result<RuleOutcome, String> {
        if let Some(inner) = strip_wrapped(name, "exec(", ")") {
            return Ok(self
                .run_exec(inner)
                .map_or(RuleOutcome::NoMatch, RuleOutcome::Commit));
        }
        if strip_wrapped(name, "nodiff(", ")").is_some() {
            return self.resolve_arc_git(rule, name, source);
        }
        Err(unknown_rule(rule, source))
    }

    fn resolve_arc_rule(
        &mut self,
        rule: &str,
        name: &str,
        source: &'static str,
    ) -> Result<RuleOutcome, String> {
        let name = self.translate_legacy_arc_name(name);
        match name {
            "verbose" => self.resolve_arc_verbose(),
            "prompt" => self.resolve_arc_prompt(),
            "local" | "user" | "project" | "runtime" | "system" => self.resolve_arc_source(name),
            "yield" | "halt" | "skip" => self.resolve_arc_flow(name, source),
            "empty" | "upstream" | "outgoing" | "bookmark" | "amended" | "this" => {
                self.resolve_arc_git(rule, name, source)
            }
            _ => self.resolve_arc_fallback(rule, name, source),
        }
    }

    fn run_exec(&self, command: &str) -> Option<String> {
        let cwd = self
            .project_root
            .clone()
            .unwrap_or_else(|| self.dir.to_path_buf());
        let mut cmd = if cfg!(windows) {
            let mut cmd = crate::proc_env::command("cmd");
            cmd.arg("/C").arg(command);
            cmd
        } else {
            let mut cmd = crate::proc_env::command("sh");
            cmd.arg("-c").arg(command);
            cmd
        };
        let out = cmd.current_dir(&cwd).output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `ArcanistGitAPI::resolveBaseCommitRule()`.
    fn resolve_git_rule(
        &mut self,
        rule: &str,
        name: &str,
        source: &str,
    ) -> Result<Option<String>, String> {
        let (kind, _) = rule.split_once(':').unwrap_or((rule, ""));
        if kind == "git" {
            return self.resolve_git_typed_rule(rule, name, source);
        }
        match name {
            "empty" => {
                self.explain(format!(
                    "you specified '{rule}' in your {source} 'base' configuration."
                ));
                Ok(Some(GIT_MAGIC_ROOT_COMMIT.to_string()))
            }
            "amended" => {
                let text = git::stdout_text(self.dir, &["log", "-n1", "--format=%B", "HEAD"])
                    .unwrap_or_default();
                if revision_id_from_corpus(&text)?.is_some() {
                    self.explain(format!(
                        "HEAD has been amended with 'Differential Revision:', as specified by \
                         '{rule}' in your {source} 'base' configuration."
                    ));
                    Ok(Some("HEAD^".to_string()))
                } else {
                    Ok(None)
                }
            }
            "upstream" => {
                let upstream = git::stdout_text(
                    self.dir,
                    &[
                        "rev-parse",
                        "--abbrev-ref",
                        "--symbolic-full-name",
                        "@{upstream}",
                    ],
                );
                let Ok(upstream) = upstream else {
                    return Ok(None);
                };
                // `merge-base %s HEAD`, like PHP (no `--`): failures
                // raise `CommandException`, not a wrapped message.
                let command = format!("git merge-base {} HEAD", shell_arg(&upstream));
                let out = git::output(self.dir, &["merge-base", &upstream, "HEAD"])?;
                if !out.status.success() {
                    return Err(execx_failed(&command, out.status, &out));
                }
                let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
                self.explain(format!(
                    "it is the merge-base of the upstream of the current branch and HEAD, and \
                     matched the rule '{rule}' in your {source} 'base' configuration."
                ));
                Ok(Some(base))
            }
            "this" => {
                self.explain(format!(
                    "you specified '{rule}' in your {source} 'base' configuration."
                ));
                Ok(Some("HEAD^".to_string()))
            }
            _ => Ok(None),
        }
    }

    fn resolve_git_typed_rule(
        &mut self,
        rule: &str,
        name: &str,
        source: &str,
    ) -> Result<Option<String>, String> {
        if let Some(inner) = strip_wrapped_nonempty(name, "merge-base(", ")") {
            let out = git::output(self.dir, &["merge-base", "--", inner, "HEAD"])
                .map_err(|e| format!("git merge-base failed: {e}"))?;
            if !out.status.success() {
                return Ok(None);
            }
            let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
            self.explain(format!(
                "it is the merge-base of '{inner}' and HEAD, as specified by '{rule}' in your \
                 {source} 'base' configuration."
            ));
            return Ok(Some(base));
        }
        if let Some(inner) = strip_wrapped_nonempty(name, "branch-unique(", ")") {
            return self.resolve_branch_unique(rule, inner, source);
        }
        if git::ok(self.dir, &["cat-file", "-t", name]) {
            self.explain(format!(
                "it is specified by '{rule}' in your {source} 'base' configuration."
            ));
            return Ok(Some(name.to_string()));
        }
        Ok(None)
    }

    /// `git:branch-unique(X)`: the first commit between `merge-base(X, HEAD)`
    /// and HEAD which is contained by more branches than HEAD is.
    fn resolve_branch_unique(
        &mut self,
        _rule: &str,
        inner: &str,
        _source: &str,
    ) -> Result<Option<String>, String> {
        let out = git::output(self.dir, &["merge-base", "--", inner, "HEAD"])
            .map_err(|e| format!("git merge-base failed: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let merge_base = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let range = format!("{merge_base}..HEAD");
        let out = git::output(self.dir, &["log", "--format=%H", &range, "--"])
            .map_err(|e| format!("git log failed: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let mut commits: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if commits.is_empty() {
            return Ok(None);
        }
        commits.push(merge_base.clone());
        let branches = local_branch_names(self.dir);
        let mut head_count: Option<usize> = None;
        for commit in &commits {
            let contained = branches_containing(self.dir, commit, &branches);
            if let Some(expected) = head_count {
                if contained.len() > expected {
                    self.explain(format!(
                        "it is the first commit between '{merge_base}' (the merge-base of \
                         '{inner}' and HEAD) which is also contained by another branch ({}).",
                        contained.join(", ")
                    ));
                    return Ok(Some(commit.clone()));
                }
            } else {
                head_count = Some(contained.len());
            }
        }
        Ok(None)
    }
}

/// Strip one `prefix...suffix` wrapper (greedy inner, like `(.*)`).
/// Unwrap `prefix...suffix`, mirroring PHP's `/^...\(.*\)$/`
/// wrappings: `.` never spans `\n`, so multi-line inners never match
/// (T1153 RB-D38e). (A trailing `\n` would still match PHP's `$`, but
/// rule tokenizing trims every rule, so that shape is unreachable.)
fn strip_wrapped<'b>(name: &'b str, prefix: &str, suffix: &str) -> Option<&'b str> {
    let inner = name
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))?;
    if inner.contains('\n') {
        return None;
    }
    Some(inner)
}

/// Unwrap a git-side `prefix...+...suffix` wrapping (`merge-base(`,
/// `branch-unique(`): like [`strip_wrapped`], but the inner `.+`
/// also rejects empties.
fn strip_wrapped_nonempty<'b>(name: &'b str, prefix: &str, suffix: &str) -> Option<&'b str> {
    let inner = strip_wrapped(name, prefix, suffix)?;
    if inner.is_empty() {
        return None;
    }
    Some(inner)
}

/// `csprintf` `%s` escaping (`PhutilCommandString::escapeLinux()`):
/// safe shell words pass through, anything else is single-quoted.
fn shell_arg(value: &str) -> String {
    let safe = !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b':' | b'/' | b'@' | b'.' | b'_' | b'+' | b'-')
        });
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Render a failed `git` call like PHP's `CommandException`
/// (`src/future/exec/CommandException.php`), which is what
/// `execxLocal` raises (T1153 RB-D23).
fn execx_failed(
    command: &str,
    status: std::process::ExitStatus,
    out: &std::process::Output,
) -> String {
    crate::markers::command_exception_text(
        status.code().unwrap_or(1),
        command,
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// Local branch short names (`refs/heads/*`).
fn local_branch_names(dir: &Path) -> Vec<String> {
    let out = git::output(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--",
            "refs/heads",
        ],
    );
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Known local branches containing `commit`, mirroring the
/// `branch --contains` filtering (current marker and detached-HEAD lines
/// dropped).
fn branches_containing(dir: &Path, commit: &str, known: &[String]) -> Vec<String> {
    let out = git::output(
        dir,
        &["branch", "--no-color", "--no-column", "--contains", commit],
    );
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().trim_matches([' ', '*']).trim().to_string())
        .filter(|b| known.iter().any(|k| k == b))
        .collect()
}

/// Resolve one source's rules, for tests and [`resolve_base`].
fn source_rules(
    name: &'static str,
    runtime: &str,
    sources: &BaseSources,
) -> Result<SourceRules, String> {
    let raw = match name {
        "runtime" => runtime,
        "local" => &sources.local,
        "project" => &sources.project,
        "user" => &sources.user,
        _ => &sources.system,
    };
    Ok(SourceRules {
        name,
        rules: tokenize_base_rules(raw)?.into_iter().collect(),
    })
}

/// Interactive base resolution inputs shared by [`resolve_base`].
pub struct BaseInputs<'a> {
    pub runtime_rules: &'a str,
    pub prompt: &'a dyn Fn(&str) -> Option<String>,
}

/// Pick the range base, mirroring `ArcanistGitAPI::buildBaseCommit()`:
/// explicit commit, zero/one-commit detection, `base` rules, then the
/// default chain (`git.default-relative-commit`, upstream, scratch file,
/// prompt).
pub fn resolve_base(
    dir: &Path,
    explicit: Option<&str>,
    head_ref: &str,
    head_explicit: bool,
    loaded: &get_config::MergedConfig,
    inputs: &BaseInputs<'_>,
) -> Result<(String, String), String> {
    if let Some(commit) = explicit {
        return resolve_explicit_base(dir, commit, head_ref, head_explicit);
    }
    if is_shallow_history(dir) {
        return Ok(shallow_base(dir));
    }
    // T1153 RB-D38, Rule 0: the gate reads the winning `base` value
    // (`ArcanistGitAPI.php:265-266` at ceb), so a falsy winner (`""`,
    // `"0"`) takes the default chain even when lower sources hold
    // rules; every source is type-checked first.
    let winner = string_config_winner("base", loaded)?;
    let gate = is_truthy_rule_spec(inputs.runtime_rules)
        || winner.as_deref().is_some_and(is_truthy_rule_spec);
    if !gate {
        return resolve_default_base(dir, &loaded.sources, inputs.prompt);
    }
    // A `--config base=` value overrides the `--base` flag (`$all_sources
    // + $base_commit_rules` in `ArcanistRepositoryAPI.php:652` at ceb).
    let runtime_spec = loaded
        .runtime
        .get("base")
        .and_then(|v| v.as_str())
        .unwrap_or(inputs.runtime_rules);
    let sources = base_sources_from(&loaded.sources);
    let all = [
        source_rules("runtime", runtime_spec, &sources)?,
        source_rules("local", "", &sources)?,
        source_rules("project", "", &sources)?,
        source_rules("user", "", &sources)?,
        source_rules("system", "", &sources)?,
    ];
    let (project_root, _) = find_project(dir);
    let mut engine = Engine::new(dir, project_root, inputs.prompt);
    if let Some(commit) = engine.resolve(&all)? {
        let why = engine.explanation.unwrap_or_default();
        return Ok((commit, why));
    }
    Err(
        "None of the rules in your 'base' configuration matched a valid commit. \
        Adjust rules or specify which commit you want to use explicitly."
            .to_string(),
    )
}

/// An explicit base commit resolves to its merge-base with the head.
fn resolve_explicit_base(
    dir: &Path,
    commit: &str,
    head_ref: &str,
    head_explicit: bool,
) -> Result<(String, String), String> {
    if commit == GIT_MAGIC_ROOT_COMMIT {
        return Ok((
            commit.to_string(),
            "you explicitly specified the empty tree.".to_string(),
        ));
    }
    let head = resolve_head(dir, head_ref)?;
    let out = git::output(dir, &["merge-base", "--", commit, &head])
        .map_err(|e| format!("git merge-base failed: {e}"))?;
    if !out.status.success() {
        return Err(unknown_commit_message(commit));
    }
    let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let why = if head_explicit {
        format!(
            "it is the merge-base of the explicitly specified base commit '{commit}' and the \
             explicitly specified head commit '{head_ref}'."
        )
    } else {
        format!("it is the merge-base of the explicitly specified base commit '{commit}' and HEAD.")
    };
    Ok((base, why))
}

/// Zero- or one-commit repositories have no `HEAD^`.
fn is_shallow_history(dir: &Path) -> bool {
    !git::ok(dir, &["rev-parse", "--verify", "HEAD^"])
}

fn shallow_base(dir: &Path) -> (String, String) {
    if git::ok(dir, &["rev-parse", "--verify", "HEAD"]) {
        (
            GIT_MAGIC_ROOT_COMMIT.to_string(),
            "the repository has only one commit.".to_string(),
        )
    } else {
        (
            GIT_MAGIC_ROOT_COMMIT.to_string(),
            "the repository has no commits.".to_string(),
        )
    }
}

/// Whether the repository has any commits at all.
fn has_any_commit(dir: &Path) -> bool {
    git::ok(dir, &["rev-parse", "--verify", "HEAD"])
}

/// Default base chain without any configured rules.
fn resolve_default_base(
    dir: &Path,
    loaded: &get_config::Sources,
    prompt: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, String), String> {
    let from_project = get_config::value_at("git.default-relative-commit", "project", loaded)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    if !from_project.is_empty() {
        return finish_default_base(
            dir,
            &from_project,
            format!(
                "it is the merge-base of '{from_project}' and HEAD, as specified in \
                 'git.default-relative-commit' in '.arcconfig'. This setting overrides other \
                 settings."
            ),
            false,
        );
    }
    if let Ok(upstream) = git::stdout_text(
        dir,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    ) {
        return finish_default_base(
            dir,
            &upstream,
            format!(
                "it is the merge-base of '{upstream}' (the Git upstream of the current branch) \
                 HEAD."
            ),
            false,
        );
    }
    let scratch = read_scratch_file(dir).trim().to_string();
    if !scratch.is_empty() {
        return finish_default_base(
            dir,
            &scratch,
            format!(
                "it is the merge-base of '{scratch}' and HEAD, as specified in \
                 '.git/arc/default-relative-commit'."
            ),
            false,
        );
    }
    print!(
        "{}{}",
        default_range_banner(terminal::ansi_enabled()),
        default_range_essay()
    );
    let answer = prompt("What default do you want to use? [origin/master]").unwrap_or_default();
    if take_prompt_tty_failed() {
        return Err(crate::prompt::NOT_INTERACTIVE.to_string());
    }
    let default = default_range_answer(&answer);
    finish_default_base(
        dir,
        &default,
        format!("it is the merge-base of '{default}' and HEAD, as you just specified."),
        true,
    )
}

/// Raw default-range essay paragraphs (`ArcanistGitAPI.php:328-352`),
/// wrapped at render time by [`default_range_essay`] like PHP's
/// `phutil_console_wrap()` call.
const DEFAULT_RANGE_ESSAY: &str = "You're running a command which operates on a range of revisions \
(usually, from some revision to HEAD) but have not specified the revision that should determine \
the start of the range.\n\n\
Previously, arc assumed you meant 'HEAD^' when you did not specify a start revision, but this \
behavior does not make much sense in most workflows outside of Facebook's historic git-svn \
workflow.\n\n\
arc no longer assumes 'HEAD^'. You must specify a relative commit explicitly when you invoke a \
command (e.g., `arc diff HEAD^`, not just `arc diff`) or select a default for this working \
copy.\n\n\
In most cases, the best default is 'origin/master'. You can also select 'HEAD^' to preserve the \
old behavior, or some other remote or branch. But you almost certainly want to \
select 'origin/master'.\n\n\
(Technically: the merge-base of the selected revision and HEAD is used to determine the start \
of the commit range.)";

/// The `Select a Default Commit Range` banner
/// (`ArcanistGitAPI.php:323-326`): `phutil_console_format()` of
/// `<bg:green>** %s **</bg>`, ANSI on TTYs, markup-stripped otherwise.
fn default_range_banner(ansi: bool) -> String {
    terminal::tsprintf_ansi(
        "<bg:green>** %s **</bg>\n\n",
        &[terminal::TVal::Text("Select a Default Commit Range")],
        ansi,
    )
    .map(|line| line.as_str().to_string())
    .unwrap_or_default()
}

/// The default-range essay wrapped at column 78, like PHP's
/// `phutil_console_wrap()` with no indent.
fn default_range_essay() -> String {
    terminal::console_wrap(DEFAULT_RANGE_ESSAY, 0, true)
}

/// Map one default-range answer: blank (whitespace-only) takes
/// `origin/master`, anything else passes through raw (`:356-358`).
fn default_range_answer(answer: &str) -> String {
    if answer.trim().is_empty() {
        "origin/master".to_string()
    } else {
        answer.to_string()
    }
}

/// Verify a default relative commit, persist it when freshly prompted,
/// and return its merge-base with HEAD.
fn finish_default_base(
    dir: &Path,
    default_relative: &str,
    explanation: String,
    persist: bool,
) -> Result<(String, String), String> {
    let object_type =
        git::stdout_text(dir, &["cat-file", "-t", default_relative]).unwrap_or_default();
    if object_type.trim() != "commit" {
        return Err(format!(
            "Relative commit '{default_relative}' is not the name of a commit!"
        ));
    }
    if persist {
        write_scratch_file(dir, default_relative);
    }
    let out = git::output(dir, &["merge-base", "--", default_relative, "HEAD"])
        .map_err(|e| format!("git merge-base failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git merge-base failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let base = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((base, explanation))
}

fn scratch_file(dir: &Path) -> Option<PathBuf> {
    let git_dir = git::stdout_text(dir, &["rev-parse", "--git-dir"]).ok()?;
    let git_dir = PathBuf::from(git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        dir.join(git_dir)
    };
    Some(git_dir.join("arc").join("default-relative-commit"))
}

fn read_scratch_file(dir: &Path) -> String {
    scratch_file(dir)
        .and_then(|file| std::fs::read_to_string(file).ok())
        .unwrap_or_default()
}

fn write_scratch_file(dir: &Path, value: &str) {
    if let Some(file) = scratch_file(dir) {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(file, value).ok();
    }
}

/// The `getLocalCommitInformation()` log format (T1153 RB-D18, Rule 0:
/// `ArcanistGitAPI.php:175-179` at ceb).
const LOCAL_INFO_FORMAT: &str = "%H%x01%T%x01%P%x01%at%x01%an%x01%aE%x01%s%x01%s%n%n%b%x02";

/// Commits for the range, newest first, mirroring
/// `getLocalCommitInformation()`: `log <head> --not <base>` (or all of
/// `HEAD` past the empty tree), with the `CommandException` text on
/// failure like `execxLocal` (T1153 RB-D18, Rule 0:
/// `ArcanistGitAPI.php:99-207` at ceb). `head` is the resolved head
/// SHA, like PHP's `getHeadCommit()`.
pub fn local_commit_information(
    dir: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<LocalCommit>, String> {
    let flag = format!("--format={LOCAL_INFO_FORMAT}");
    let command = if base == GIT_MAGIC_ROOT_COMMIT {
        format!("git log HEAD --format={} --", shell_arg(LOCAL_INFO_FORMAT))
    } else {
        format!(
            "git log {} --not {} --format={} --",
            shell_arg(head),
            shell_arg(base),
            shell_arg(LOCAL_INFO_FORMAT)
        )
    };
    let argv: Vec<&str> = if base == GIT_MAGIC_ROOT_COMMIT {
        vec!["log", "HEAD", &flag, "--"]
    } else {
        vec!["log", head, "--not", base, &flag, "--"]
    };
    let out = git::output(dir, &argv)?;
    if !out.status.success() {
        return Err(execx_failed(&command, out.status, &out));
    }
    Ok(parse_commit_records(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse `%H/%T/%P/%at/%an/%aE/%s/(%s%n%n%b)` records: the title feeds
/// `summary`, the trailing message is PHP-`rtrim()`ed (T1153 RB-D38a,
/// kept: the `\x01`/`\x02` separators make a `commit <40hex>` line
/// inside a message harmless, where PHP's medium-format `DiffParser`
/// split misparses).
fn parse_commit_records(text: &str) -> Vec<LocalCommit> {
    let mut commits = Vec::new();
    for record in text.split('\u{2}') {
        let record = record.trim_matches([' ', '\t', '\n', '\r', '\0', '\u{B}', '\u{2}']);
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(8, '\u{1}');
        let (
            Some(commit),
            Some(tree),
            _parents,
            _time,
            _author,
            _author_email,
            Some(summary),
            Some(message),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            continue;
        };
        commits.push(LocalCommit {
            commit: commit.to_string(),
            tree: tree.to_string(),
            summary: summary.to_string(),
            message: message
                .trim_end_matches([' ', '\t', '\n', '\r', '\0', '\u{B}'])
                .to_string(),
        });
    }
    commits
}

/// Commits in the range, newest first (T1153 RB-D18: `git log` order).
pub fn commits_between(dir: &Path, base: &str, head: &str) -> Result<Vec<Commit>, String> {
    Ok(local_commit_information(dir, base, head)?
        .into_iter()
        .map(|c| Commit {
            sha: c.commit,
            subject: c.summary,
        })
        .collect())
}

/// Reject an empty or reversed explicit range, mirroring
/// `getLocalCommitInformation()`.
fn check_range_direction(
    dir: &Path,
    base: &str,
    resolved_head: &str,
    head_ref: &str,
) -> Result<(), String> {
    let resolved_base = rev_to_commit(dir, base).unwrap_or_else(|| base.to_string());
    let same = resolved_base == resolved_head;
    let ancestor = !same
        && git::ok(
            dir,
            &["merge-base", "--is-ancestor", &resolved_base, resolved_head],
        );
    if !ancestor {
        return Err(format!(
            "The specified commit range is empty, backward or invalid: the base ({base}) is not \
             an ancestor of the head ({head_ref}). You can not diff an empty or reversed commit \
             range."
        ));
    }
    Ok(())
}

/// `(hash, message)` pairs for the range, mirroring `getGitCommitLog()`.
pub fn git_commit_messages(
    dir: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<(String, String)>, String> {
    let format = "%H%x01%B%x02";
    let flag = std::format!("--format={format}");
    let out = if base == GIT_MAGIC_ROOT_COMMIT {
        git::output(dir, &["log", &flag, "HEAD", "--"])
    } else {
        let range = format!("{base}..{head}");
        git::output(dir, &["log", "--first-parent", &flag, &range, "--"])
    }
    .map_err(|e| format!("failed to list commits: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut messages = Vec::new();
    for record in String::from_utf8_lossy(&out.stdout).split('\u{2}') {
        let record = record.trim_matches([' ', '\n', '\u{2}']);
        if record.is_empty() {
            continue;
        }
        if let Some((hash, corpus)) = record.split_once('\u{1}') {
            messages.push((hash.to_string(), corpus.to_string()));
        }
    }
    Ok(messages)
}

/// Revision ID from a `Differential Revision:` footer, mirroring
/// `ArcanistDifferentialCommitMessage::parseRevisionIDFromRawCorpus()`.
/// Returns `Ok(None)` when the corpus has no footer; errors on a
/// malformed one.
/// PCRE `\s`: space, tab, newline, CR, form feed, vertical tab.
const FOOTER_SPACE: [char; 6] = [' ', '\t', '\n', '\r', '\u{c}', '\u{b}'];

/// PHP `trim()` charlist (no form feed).
const TRIM_CHARS: [char; 6] = [' ', '\t', '\n', '\r', '\0', '\u{b}'];

/// How `/^Differential Revision:\s*(.+)/im` matches at one line start:
/// the raw value, or no match (try the next start). T1153 RB-D20: a
/// whitespace-only tail never matches — even with spaces in it — so
/// an empty footer is skipped instead of throwing.
fn footer_value(after: &str) -> Option<&str> {
    let stripped = after.trim_start_matches(FOOTER_SPACE);
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.split('\n').next().unwrap_or(""))
}

fn invalid_footer(value: &str) -> String {
    // T1153 BROWSE-B25: the ceb-era Phabricator example, not the 2023
    // devtools text.
    format!(
        "Invalid \"Differential Revision\" field in commit message. This field should have a \
         revision identifier like \"D123\" or a Phabricator URI like \
         \"https://phabricator.example.com/D123\", but has \"{value}\"."
    )
}

pub fn revision_id_from_corpus(corpus: &str) -> Result<Option<String>, String> {
    const PREFIX: &str = "differential revision:";
    let mut starts = vec![0usize];
    for (index, byte) in corpus.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    for start in starts {
        let Some(rest) = corpus.get(start..) else {
            continue;
        };
        if rest.len() < PREFIX.len() {
            continue;
        }
        let (head, after) = rest.split_at(PREFIX.len());
        if !head.eq_ignore_ascii_case(PREFIX) {
            continue;
        }
        let Some(raw) = footer_value(after) else {
            continue;
        };
        let value = raw.trim_matches(TRIM_CHARS);
        if let Some(id) = bare_revision_id(value) {
            return Ok(Some(id));
        }
        if let Some(id) = uri_revision_id(value) {
            return Ok(Some(id));
        }
        return Err(invalid_footer(value));
    }
    Ok(None)
}

/// A bare `D123` footer value (`[dD][1-9][0-9]*`, no leading zeroes).
fn bare_revision_id(value: &str) -> Option<String> {
    let digits = value
        .strip_prefix('D')
        .or_else(|| value.strip_prefix('d'))?;
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && !digits.starts_with('0')
    {
        Some(digits.to_string())
    } else {
        None
    }
}

/// True for `PhutilURI` scheme heads (alpha start, scheme chars).
fn is_uri_scheme(head: &str) -> bool {
    let mut chars = head.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// A server-URI footer value: the whole URI path must be a monogram.
/// T1153 RB-D38b: without `://`, PHP's `PhutilURI` still splits a
/// scheme, so `host:D123` carries path `D123` and resolves.
fn uri_revision_id(value: &str) -> Option<String> {
    let without_query = value.split(['?', '#']).next().unwrap_or("");
    if let Some((_, rest)) = without_query.split_once("://") {
        let path = rest.split_once('/').map_or("", |(_, path)| path);
        return bare_revision_id(path.trim_matches('/'));
    }
    if let Some((head, rest)) = without_query.split_once(':')
        && is_uri_scheme(head)
    {
        return bare_revision_id(rest.trim_matches('/'));
    }
    bare_revision_id(without_query.trim_matches('/'))
}

fn id_number(id: &str) -> serde_json::Value {
    id.parse::<u64>().map_or_else(
        |_| serde_json::Value::String(id.to_string()),
        serde_json::Value::from,
    )
}

fn query_rows(result: &serde_json::Value) -> Vec<&serde_json::Value> {
    match result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            keys.into_iter().filter_map(|k| map.get(k)).collect()
        }
        _ => Vec::new(),
    }
}

fn row_string(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| {
            v.as_str().map(str::to_string).or_else(|| {
                v.as_i64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            })
        })
        .unwrap_or_default()
}

/// Differential revisions matching the working copy, mirroring
/// `ArcanistGitAPI::loadWorkingCopyDifferentialRevisions()`: explicit
/// `Differential Revision` footers first, commit/tree hashes second.
fn conduit_error_text(error: conduit::Error) -> String {
    error.to_string()
}

fn hash_prefix(hash: &str) -> String {
    hash.chars().take(16).collect()
}

fn collect_explicit_revision_ids(
    messages: &[(String, String)],
) -> Result<
    (
        Vec<serde_json::Value>,
        std::collections::BTreeMap<String, String>,
    ),
    String,
> {
    let mut ids: Vec<serde_json::Value> = Vec::new();
    let mut reasons: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (hash, corpus) in messages {
        if let Some(id) = revision_id_from_corpus(corpus)? {
            ids.push(id_number(&id));
            reasons.insert(id, hash.clone());
        }
    }
    Ok((ids, reasons))
}

fn query_revisions_by_hashes(
    client: &conduit::Client,
    dir: &Path,
    base: &str,
    head: &str,
    status: &str,
) -> Result<Vec<(String, String, String, String)>, String> {
    let info = local_commit_information(dir, base, head)?;
    let mut hashes: Vec<serde_json::Value> = Vec::new();
    for commit in &info {
        hashes.push(serde_json::json!(["gtcm", commit.commit]));
        hashes.push(serde_json::json!(["gttr", commit.tree]));
    }
    let result = client
        .call(
            "differential.query",
            &serde_json::json!({"status": status, "commitHashes": hashes}),
        )
        .map_err(conduit_error_text)?;
    let mut rows = Vec::new();
    for row in query_rows(&result) {
        rows.push((
            row_string(row, "id"),
            row_string(row, "title"),
            row_string(row, "authorPHID"),
            "A git commit or tree hash in the commit range is already attached to the Differential revision."
                .to_string(),
        ));
    }
    Ok(rows)
}

fn query_revisions_by_ids(
    client: &conduit::Client,
    ids: Vec<serde_json::Value>,
    reasons: &std::collections::BTreeMap<String, String>,
    status: &str,
) -> Result<Vec<(String, String, String, String)>, String> {
    let result = client
        .call(
            "differential.query",
            &serde_json::json!({"status": status, "ids": ids}),
        )
        .map_err(conduit_error_text)?;
    let mut rows = Vec::new();
    for row in query_rows(&result) {
        let id = row_string(row, "id");
        let hash16: String = reasons
            .get(&id)
            .map(|hash| hash_prefix(hash))
            .unwrap_or_default();
        rows.push((
            id,
            row_string(row, "title"),
            row_string(row, "authorPHID"),
            format!("Commit message for '{hash16}' has explicit 'Differential Revision'."),
        ));
    }
    Ok(rows)
}

pub fn match_working_copy_revisions(
    client: &conduit::Client,
    dir: &Path,
    base: &str,
    head: &str,
    any_status: bool,
) -> Result<Vec<MatchedRevision>, String> {
    let messages = git_commit_messages(dir, base, head)?;
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let status = if any_status {
        "status-any"
    } else {
        "status-open"
    };
    let (ids, reasons) = collect_explicit_revision_ids(&messages)?;
    let rows = if ids.is_empty() {
        query_revisions_by_hashes(client, dir, base, head, status)?
    } else {
        query_revisions_by_ids(client, ids, &reasons, status)?
    };
    attach_authors(client, rows)
}

/// The viewer's PHID: the cached auth PHID, else one `user.whoami`
/// call.
///
/// T1153 RB-D8: PHP resolves whoami once at startup auth, then reuses
/// the cached viewer PHID (`getUserPHID()`); only call when uncached
/// (e.g. direct library use without `config::authenticate`).
fn viewer_phid(client: &conduit::Client) -> Result<String, String> {
    if let Some((_, phid)) = client.authenticated_user() {
        return Ok(phid);
    }
    let who = client
        .call("user.whoami", &serde_json::json!({}))
        .map_err(|e| e.to_string())?;
    Ok(who
        .get("phid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Distinct author PHIDs in `rows` excluding the viewer.
fn other_author_phids(rows: &[(String, String, String, String)], own: &str) -> Vec<String> {
    let mut others: Vec<String> = Vec::new();
    for (_, _, author, _) in rows {
        if author != own && !others.iter().any(|p| p == author) {
            others.push(author.clone());
        }
    }
    others
}

/// Map other-author PHIDs to user names via one `user.query` call.
fn query_author_names(
    client: &conduit::Client,
    others: &[String],
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut names: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    if !others.is_empty() {
        let users = client
            .call("user.query", &serde_json::json!({"phids": others}))
            .map_err(|e| e.to_string())?;
        for user in query_rows(&users) {
            names.insert(row_string(user, "phid"), row_string(user, "userName"));
        }
    }
    Ok(names)
}

/// Build one matched revision, resolving other authors' names.
fn build_matched_revision(
    id: String,
    title: String,
    author: String,
    why: String,
    own: &str,
    names: &std::collections::BTreeMap<String, String>,
) -> MatchedRevision {
    let name = if author == own {
        None
    } else {
        Some(names.get(&author).cloned().unwrap_or_default())
    };
    MatchedRevision {
        monogram: format!("D{id}"),
        title,
        author: name,
        why,
    }
}

/// Resolve other authors' names via `user.query`, like the PHP workflow.
fn attach_authors(
    client: &conduit::Client,
    rows: Vec<(String, String, String, String)>,
) -> Result<Vec<MatchedRevision>, String> {
    let own_phid = viewer_phid(client)?;
    let own = own_phid.as_str();
    let others = other_author_phids(&rows, own);
    let names = query_author_names(client, &others)?;
    Ok(rows
        .into_iter()
        .map(|(id, title, author, why)| build_matched_revision(id, title, author, why, own, &names))
        .collect())
}

/// Build the full context for `dir`. `user_file` is the `~/.arcrc` path
/// (`None` skips token/callsign user lookups), `env` is the
/// `ARC_CONDUIT_URI` value, `explicit_base` pins the base, `head_ref`
/// pins the range end, and `resolution` carries `--base` rules, the
/// prompt hook, and revision matching.
fn path_display(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}

struct RepoIdentity {
    vcs_root: Option<String>,
    branch: Option<String>,
    head: Option<String>,
    head_subject: Option<String>,
    remote_url: Option<String>,
}

fn load_repo_identity(dir: &Path, head_ref: &str, in_repo: bool) -> RepoIdentity {
    if !in_repo {
        return RepoIdentity {
            vcs_root: None,
            branch: None,
            head: None,
            head_subject: None,
            remote_url: None,
        };
    }
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"]).ok();
    let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
    let head = rev_to_commit(dir, head_ref);
    let head_subject = match &head {
        Some(sha) => git::stdout_text(dir, &["log", "-1", "--format=%s", sha]).ok(),
        None => None,
    };
    let remote = git::stdout_text(dir, &["config", "--get", "remote.origin.url"]).ok();
    RepoIdentity {
        vcs_root: root,
        branch,
        head,
        head_subject,
        remote_url: remote,
    }
}

/// `ArcanistGitAPI::getLocalCommitInformation()` on a repository with
/// no commits (`ArcanistGitAPI.php:99-113`): PHP prints the repository
/// section first, then fails with this text.
const NO_LOCAL_COMMITS: &str =
    "You can't get local commit information for a repository with no commits.";

/// One commit's summary line, mirroring `getCommitSummary()` (T1153
/// RB-D18, Rule 0: `ArcanistGitAPI.php:1246-1257` at ceb): the empty
/// tree reads `(The Empty Tree)`, anything else resolves through
/// `git log -n 1 --format=%s` (PHP-trimmed), and an unresolvable base
/// raises the `CommandException` text like `execxLocal` does.
fn commit_summary(dir: &Path, relative: &str) -> Result<String, String> {
    if relative == GIT_MAGIC_ROOT_COMMIT {
        return Ok("(The Empty Tree)".to_string());
    }
    let out = git::output(dir, &["log", "-n", "1", "--format=%s", relative])?;
    if !out.status.success() {
        let command = format!("git log -n 1 --format=%s {}", shell_arg(relative));
        return Err(execx_failed(&command, out.status, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_matches(TRIM_CHARS)
        .to_string())
}

/// The `arc diff` argument suffix: ` <commit>` plus ` --head %R` when
/// `--head` was given (T1153 RB-D18, Rule 0:
/// `ArcanistWhichWorkflow.php:89-99` at ceb).
#[must_use]
pub fn diff_arg(explicit: Option<&str>, head: Option<&str>) -> String {
    let mut arg = explicit.map_or_else(String::new, |base| format!(" {base}"));
    if let Some(head) = head {
        arg.push_str(&format!(" --head {}", shell_arg(head)));
    }
    arg
}

/// Resolve everything `COMMIT RANGE` prints: base, direction check,
/// summary, command line, and commits (T1153 RB-D18, Rule 0:
/// `ArcanistWhichWorkflow.php:103-175` at ceb). The base stays raw —
/// a rule like `git:HEAD~2` displays (and diffs) unexpanded.
pub fn load_range_data(
    dir: &Path,
    merged: &get_config::MergedConfig,
    explicit: Option<&str>,
    head_ref: &str,
    head_explicit: bool,
    runtime_rules: &str,
    prompt: &dyn Fn(&str) -> Option<String>,
) -> Result<RangeData, String> {
    let inputs = BaseInputs {
        runtime_rules,
        prompt,
    };
    let (base, explanation) =
        resolve_base(dir, explicit, head_ref, head_explicit, merged, &inputs)?;
    if !has_any_commit(dir) {
        return Err(NO_LOCAL_COMMITS.to_string());
    }
    let resolved_head = resolve_head(dir, head_ref)?;
    if head_explicit && base != GIT_MAGIC_ROOT_COMMIT {
        check_range_direction(dir, &base, &resolved_head, head_ref)?;
    }
    // T1153 RB-D18: PHP lists commits (`getLocalCommitInformation()`)
    // before resolving the summary (`getCommitSummary()`), both
    // through the resolved head.
    let commits = commits_between(dir, &base, &resolved_head)?;
    let summary = commit_summary(dir, &base)?;
    let base16: String = base.chars().take(16).collect();
    let command = format!("git diff {}", shell_arg(&format!("{base16}..{head_ref}")));
    Ok(RangeData {
        base,
        explanation,
        base16,
        summary,
        command,
        commits,
        head: resolved_head,
    })
}

/// Build the full context for `dir`, for `--json` and tests. The text
/// path resolves in stages instead (see [`run`]), so failures land
/// after the sections PHP has already printed. `user_file` is the
/// `~/.arcrc` path (`None` skips token lookups), `env` is the
/// `ARC_CONDUIT_URI` value, `explicit_base` pins the base, `head_ref`
/// pins the range end, `inputs` carries `--base` rules plus the prompt
/// hook, and `client` (already authenticated) drives association plus
/// revision matching (`any_status` selects `status-any`).
#[allow(clippy::too_many_arguments)]
pub fn load_context(
    dir: &Path,
    user_file: Option<&Path>,
    flag_uri: Option<&str>,
    env: Option<&str>,
    explicit_base: Option<&str>,
    head_ref: &str,
    head_explicit: bool,
    merged: &get_config::MergedConfig,
    inputs: &BaseInputs<'_>,
    client: &conduit::Client,
    any_status: bool,
) -> Result<Context, String> {
    if !git::is_inside_work_tree(dir) {
        return Err(WORKING_COPY_REQUIRED.to_string());
    }
    let identity = load_repo_identity(dir, head_ref, true);
    let (project_root, project_file) = find_project(dir);
    let callsign = repository_callsign(merged)?;
    let (conduit_uri, conduit_source) = resolve_conduit_in(dir, flag_uri, env);
    let token = conduit_uri
        .as_ref()
        .map(|uri| token_installed(user_file, uri));
    let assoc = load_association(client, dir, merged)?;
    let range = load_range_data(
        dir,
        merged,
        explicit_base,
        head_ref,
        head_explicit,
        inputs.runtime_rules,
        inputs.prompt,
    )?;
    let matching = match_working_copy_revisions(client, dir, &range.base, &range.head, any_status)?;
    let head = head_explicit.then_some(head_ref);
    Ok(Context {
        dir: dir.to_string_lossy().to_string(),
        vcs: "git",
        vcs_root: identity.vcs_root,
        project_root: project_root.map(path_display),
        project_file: project_file.map(path_display),
        branch: identity.branch,
        head: identity.head,
        head_subject: identity.head_subject,
        remote_url: identity.remote_url,
        callsign,
        conduit_uri,
        conduit_source,
        token_installed: token,
        repo_name: assoc.repo_name,
        repo_reasons: assoc.reasons,
        base: Some(range.base),
        base_explanation: Some(range.explanation),
        base16: Some(range.base16),
        summary: Some(range.summary),
        command: Some(range.command),
        head_ref: head_ref.to_string(),
        head_explicit,
        commits: range.commits,
        diff_arg: diff_arg(explicit_base, head),
        matching,
    })
}

/// Bold a section header or verdict word on TTYs (`**%s**`), plain
/// otherwise — the same markup PHP applies to these words.
fn bold(text: &str, ansi: bool) -> String {
    terminal::console_format("**%s**", &[text], ansi)
}

/// The bold `REPOSITORY` header line, printed before association
/// resolves (T1153 RB-D17: `printRepositorySection()` writes the header
/// first at `ArcanistWhichWorkflow.php:265`, so even callsign-type
/// errors still show it).
fn repository_header(ansi: bool) -> String {
    terminal::console_format("**%s**\n", &["REPOSITORY"], ansi)
}

/// The `REPOSITORY` body: process prose, wrapped reasons, and the
/// association verdict (T1153 RB-D16, Rule 0:
/// `ArcanistWhichWorkflow.php:264-285` at ceb).
fn render_repository_body(assoc: &Association, ansi: bool) -> String {
    let mut out = String::from(
        "To identify the repository associated with this working copy, arc followed this process:\n\n",
    );
    for reason in &assoc.reasons {
        out.push_str(&terminal::console_wrap(reason, 4, true));
        out.push_str("\n\n");
    }
    match &assoc.repo_name {
        Some(name) => out.push_str(&format!(
            "This working copy is associated with the {} repository.\n",
            bold(name, ansi)
        )),
        None => out.push_str("This working copy is not associated with any repository.\n"),
    }
    out
}

/// The full `REPOSITORY` section (T1153 RB-D16): header plus body. The
/// caller appends the blank line `run()` adds after
/// `printRepositorySection()`.
#[must_use]
pub fn render_repository(assoc: &Association, ansi: bool) -> String {
    format!(
        "{}{}",
        repository_header(ansi),
        render_repository_body(assoc, ansi)
    )
}

/// The `COMMIT RANGE` section: wrapped prose around the 16-char base
/// plus summary, the `will be sent` sentence, the diff command, and
/// the commit list (T1153 RB-D16/T1153 RB-D18, Rule 0:
/// `ArcanistWhichWorkflow.php:130-175` at ceb). `head` is the raw
/// `--head` value when given.
#[must_use]
pub fn render_range(range: &RangeData, arg: &str, head: Option<&str>, ansi: bool) -> String {
    let mut out = terminal::console_wrap(
        &terminal::console_format(
            "**%s**\n%s\n\n    %s  %s\n\n",
            &[
                "COMMIT RANGE",
                &format!("If you run 'arc diff{arg}', changes between the commit:"),
                &range.base16,
                &range.summary,
            ],
            ansi,
        ),
        0,
        true,
    );
    let will_be_sent = match head {
        None => format!(
            "...and the current working copy state will be sent to Differential, because {}",
            range.explanation
        ),
        Some(head) => format!(
            "...and \"{head}\" will be sent to Differential, because {}",
            range.explanation
        ),
    };
    out.push_str(&terminal::console_wrap(
        &terminal::console_format(
            "%s\n\n%s\n\n    $ %s\n\n%s\n\n",
            &[
                &will_be_sent,
                "You can see the exact changes that will be sent by running this command:",
                &range.command,
                "These commits will be included in the diff:",
            ],
            ansi,
        ),
        0,
        true,
    ));
    if range.commits.is_empty() {
        out.push_str("    (No commits.)");
    } else {
        let lines: Vec<String> = range
            .commits
            .iter()
            .map(|commit| {
                format!(
                    "    {}  {}",
                    commit.sha.chars().take(16).collect::<String>(),
                    commit.subject
                )
            })
            .collect();
        out.push_str(&lines.join("\n"));
    }
    out.push_str("\n\n\n");
    out
}

/// The `MATCHING REVISIONS` section (T1153 RB-D16, Rule 0:
/// `ArcanistWhichWorkflow.php:186-261` at ceb). The subheader prints on
/// every run (T1153 RB-D19); the multi-revision sentence keeps PHP's
/// literal `"arc diff {$arg}"` double space and skips
/// `phutil_console_format` like PHP.
#[must_use]
pub fn render_matching(revisions: &[MatchedRevision], diff_arg: &str, ansi: bool) -> String {
    let mut out = terminal::console_wrap(
        &terminal::console_format(
            "**%s**\n%s\n\n",
            &[
                "MATCHING REVISIONS",
                "These Differential revisions match the changes in this working copy:",
            ],
            ansi,
        ),
        0,
        true,
    );
    if revisions.is_empty() {
        out.push_str("    (No revisions match.)\n\n");
        out.push_str(&terminal::console_wrap(
            &terminal::console_format(
                "Since there are no revisions in Differential which match this working copy, a \
                 new revision will be **created** if you run '%s'.\n\n",
                &[&format!("arc diff{diff_arg}")],
                ansi,
            ),
            0,
            true,
        ));
        return out;
    }
    for revision in revisions {
        match &revision.author {
            Some(author) => out.push_str(&format!(
                "    {} ({author}) {}\n",
                revision.monogram, revision.title
            )),
            None => out.push_str(&format!("    {} {}\n", revision.monogram, revision.title)),
        }
        out.push_str(&format!("        Reason: {}\n\n", revision.why));
    }
    if revisions.len() == 1 {
        out.push_str(&terminal::console_wrap(
            &terminal::console_format(
                "Since exactly one revision in Differential matches this working copy, it \
                 will be **updated** if you run '%s'.",
                &[&format!("arc diff{diff_arg}")],
                ansi,
            ),
            0,
            true,
        ));
    } else {
        let sentence = format!(
            "Since more than one revision in Differential matches this working copy, you will be \
             asked which revision you want to update if you run 'arc diff {diff_arg}'."
        );
        out.push_str(&terminal::console_wrap(&sentence, 0, true));
    }
    out.push_str("\n\n");
    out
}

/// Render the human-readable report with the process ANSI setting.
#[must_use]
pub fn render(ctx: &Context) -> String {
    render_ansi(ctx, terminal::ansi_enabled())
}

/// Render the human-readable report with an explicit ANSI setting
/// (deterministic under test).
#[must_use]
pub fn render_ansi(ctx: &Context, ansi: bool) -> String {
    let assoc = Association {
        reasons: ctx.repo_reasons.clone(),
        repo_name: ctx.repo_name.clone(),
    };
    let range = RangeData {
        base: ctx.base.clone().unwrap_or_default(),
        explanation: ctx.base_explanation.clone().unwrap_or_default(),
        base16: ctx.base16.clone().unwrap_or_default(),
        summary: ctx.summary.clone().unwrap_or_default(),
        command: ctx.command.clone().unwrap_or_default(),
        commits: ctx.commits.clone(),
        head: ctx.head.clone().unwrap_or_default(),
    };
    let head = ctx.head_explicit.then_some(ctx.head_ref.as_str());
    format!(
        "{}\n{}{}",
        render_repository(&assoc, ansi),
        render_range(&range, &ctx.diff_arg, head, ansi),
        render_matching(&ctx.matching, &ctx.diff_arg, ansi)
    )
}

/// Render the machine-readable report (an arc-rs extension; PHP has
/// no `--json`). Carries the association rows (T1153 RB-D17) next to
/// the pre-existing keys.
#[must_use]
pub fn report_json(ctx: &Context) -> serde_json::Value {
    serde_json::json!({
        "dir": ctx.dir,
        "vcs": ctx.vcs,
        "vcs_root": ctx.vcs_root,
        "project_root": ctx.project_root,
        "project_file": ctx.project_file,
        "branch": ctx.branch,
        "head": ctx.head,
        "head_subject": ctx.head_subject,
        "remote_url": ctx.remote_url,
        "callsign": ctx.callsign,
        "conduit_uri": ctx.conduit_uri,
        "conduit_source": ctx.conduit_source,
        "token_installed": ctx.token_installed,
        "repository_name": ctx.repo_name,
        "repository_reasons": ctx.repo_reasons,
        "base": ctx.base,
        "base_explanation": ctx.base_explanation,
        "head_ref": ctx.head_ref,
        "commits": ctx.commits.iter().map(|c| serde_json::json!({"commit": c.sha, "summary": c.subject})).collect::<Vec<_>>(),
        "matching": ctx.matching.iter().map(|r| {
            serde_json::json!({
                "revision": r.monogram,
                "title": r.title,
                "author": r.author,
                "why": r.why,
            })
        }).collect::<Vec<_>>(),
    })
}

/// The multi-commit error, checked after the repository section
/// prints (T1153 RB-D16: `parseBaseCommitArgument()`).
const TOO_MANY_COMMITS: &str = "Specify exactly one base commit. The end of the commit range is always the \
    working copy state.";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    conduit_uri: Option<String>,
    head: String,
    head_explicit: bool,
    show_base: bool,
    json: bool,
    base_rules: Option<String>,
    any_status: bool,
    commits: Vec<String>,
}

enum Parsed {
    Help,
    Options(Options),
    Error(String),
}

type ArgIter<'a> = std::iter::Peekable<std::slice::Iter<'a, String>>;

/// One argument's parse step: consumed, or a terminal outcome.
enum Step {
    Handled,
    Done(Parsed),
}

struct Parser {
    conduit_uri: Option<String>,
    head: String,
    head_explicit: bool,
    show_base: bool,
    json: bool,
    base_rules: Option<String>,
    any_status: bool,
    commits: Vec<String>,
}

impl Parser {
    fn new() -> Self {
        Self {
            conduit_uri: None,
            head: "HEAD".to_string(),
            head_explicit: false,
            show_base: false,
            json: false,
            base_rules: None,
            any_status: false,
            commits: Vec::new(),
        }
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Option<Step> {
        // T1153 RB-D25: `--flag=value` is accepted like PHP
        // (`PhutilArgumentParser.php:143-150` at ceb).
        if let Some(value) = arg.strip_prefix("--conduit-uri=") {
            self.conduit_uri = Some(value.to_string());
            return Some(Step::Handled);
        }
        if arg != "--conduit-uri" {
            return None;
        }
        let Some(value) = iter.next() else {
            return Some(Step::Done(Parsed::Error(
                "--conduit-uri requires a value".to_string(),
            )));
        };
        self.conduit_uri = Some(value.clone());
        Some(Step::Handled)
    }

    fn flag_head(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Option<Step> {
        // T1153 RB-D25: `--head=X` is accepted like `--base=X`.
        if let Some(value) = arg.strip_prefix("--head=") {
            self.head = value.to_string();
            self.head_explicit = true;
            return Some(Step::Handled);
        }
        if arg != "--head" {
            return None;
        }
        let Some(value) = iter.next() else {
            return Some(Step::Done(Parsed::Error(
                "--head requires a value".to_string(),
            )));
        };
        self.head = value.clone();
        self.head_explicit = true;
        Some(Step::Handled)
    }

    fn flag_base(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Option<Step> {
        if arg != "--base" {
            if let Some(rules) = arg.strip_prefix("--base=") {
                self.base_rules = Some(rules.to_string());
                return Some(Step::Handled);
            }
            return None;
        }
        let Some(value) = iter.next() else {
            return Some(Step::Done(Parsed::Error(
                "--base requires a value".to_string(),
            )));
        };
        self.base_rules = Some(value.clone());
        Some(Step::Handled)
    }

    fn push_commit(&mut self, arg: &str) -> Step {
        // T1153 RB-D16: the count check runs after the repository
        // section prints (`parseBaseCommitArgument()` at
        // `ArcanistWhichWorkflow.php:84-87`), so parsing collects.
        self.commits.push(arg.to_string());
        Step::Handled
    }

    fn consume_rest(&mut self, iter: &mut ArgIter<'_>) -> Option<Step> {
        for rest in iter.by_ref() {
            self.commits.push(rest.clone());
        }
        None
    }

    fn tail(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Step {
        if arg == "--show-base" {
            self.show_base = true;
            return Step::Handled;
        }
        if arg == "--json" {
            self.json = true;
            return Step::Handled;
        }
        if arg == "--any-status" {
            self.any_status = true;
            return Step::Handled;
        }
        if arg == "-h" || arg == "--help" {
            return Step::Done(Parsed::Help);
        }
        if arg == "--" {
            if let Some(step) = self.consume_rest(iter) {
                return step;
            }
            return Step::Handled;
        }
        if arg.starts_with('-') {
            return Step::Done(Parsed::Error(format!("unknown flag '{arg}'")));
        }
        self.push_commit(arg)
    }

    fn consume(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Step {
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_head(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_base(arg, iter) {
            return step;
        }
        self.tail(arg, iter)
    }

    fn finish(self) -> Options {
        Options {
            conduit_uri: self.conduit_uri,
            head: self.head,
            head_explicit: self.head_explicit,
            show_base: self.show_base,
            json: self.json,
            base_rules: self.base_rules,
            any_status: self.any_status,
            commits: self.commits,
        }
    }
}

fn parse_args(argv: &[String]) -> Parsed {
    let mut parser = Parser::new();
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        match parser.consume(arg, &mut iter) {
            Step::Handled => {}
            Step::Done(parsed) => return parsed,
        }
    }
    Parsed::Options(parser.finish())
}

thread_local! {
    /// Set by [`prompt_line_with_tty`] when stdin is not a TTY. The
    /// prompt hook type (`Fn(&str) -> Option<String>`) has no error
    /// channel, so the `phutil_console_require_tty()` failure rides
    /// out-of-band: each prompt call site takes it right after the
    /// hook returns (see [`take_prompt_tty_failed`]). Scripted hooks
    /// never set it, so their `None` keeps meaning EOF/default.
    static PROMPT_TTY_FAILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Take the recorded prompt TTY failure, if any (resetting it).
fn take_prompt_tty_failed() -> bool {
    PROMPT_TTY_FAILED.take()
}

/// `phutil_console_prompt()`: two blank lines plus the query wrapped
/// at indent 4, then one raw answer line. Piped stdin echoes the
/// prompt, records the TTY failure (see [`take_prompt_tty_failed`]),
/// and yields `None`; EOF yields `None` (the caller takes its default).
fn prompt_line(prompt: &str) -> Option<String> {
    prompt_line_with_tty(prompt, crate::prompt::stdin_is_terminal())
}

/// [`prompt_line`] with an injected TTY flag (deterministic tests).
fn prompt_line_with_tty(prompt: &str, stdin_tty: bool) -> Option<String> {
    print!("{}", crate::prompt::prompt_text(prompt));
    if !stdin_tty {
        PROMPT_TTY_FAILED.set(true);
        return None;
    }
    crate::prompt::read_answer(&mut std::io::stdin().lock())
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Parsed::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Parsed::Error(message) => {
            // T1153 RB-D4 (R4): `which` is a subprocess workflow, so
            // usage errors render `Usage Exception:` with no usage dump
            // (`scripts/arcanist.php:432-444`).
            return conduit::report(EnvelopeKind::Usage, &message);
        }
        Parsed::Options(options) => options,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // T1153 RB-D16: the working-copy gate runs before conduit setup
    // (`scripts/arcanist.php:260-266` at ceb).
    if !git::is_inside_work_tree(&cwd) {
        return conduit::report(EnvelopeKind::Usage, WORKING_COPY_REQUIRED);
    }
    let user_file = config::arcrc_path();
    let opts = get_config::LoadOptions::default();
    // T1153 RB-D17: conduit plus authentication come next
    // (`requiresConduit()`/`requiresAuthentication()`), even for
    // `--show-base`, which never reaches the repository section.
    let client = match config::authed_client_for(
        options.conduit_uri.as_deref(),
        &cwd,
        user_file.as_deref(),
        &opts,
        &config::command_description(),
    ) {
        Ok(client) => client,
        Err(error) => return error.report(),
    };
    let merged = match get_config::load_merged(&cwd, user_file.as_deref(), &opts) {
        Ok(merged) => merged,
        Err(message) => return conduit::report(EnvelopeKind::Exception, &message),
    };
    if options.json {
        return run_json(&cwd, user_file.as_deref(), &options, &merged, &client);
    }
    if options.show_base {
        return run_show_base(&cwd, &options, &merged);
    }
    run_text(&cwd, &options, &merged, &client)
}

/// The text report, printed in stages like `run()` at
/// `ArcanistWhichWorkflow.php:79-261`: the repository section first,
/// then the base-argument check, then the range, then matching, so
/// each failure lands after the sections PHP has already printed.
fn run_text(
    cwd: &Path,
    options: &Options,
    merged: &get_config::MergedConfig,
    client: &conduit::Client,
) -> ExitCode {
    let ansi = terminal::ansi_enabled();
    print!("{}", repository_header(ansi));
    let assoc = match load_association(client, cwd, merged) {
        Ok(assoc) => assoc,
        Err(message) => return report_which_error(&message),
    };
    println!("{}", render_repository_body(&assoc, ansi));
    if options.commits.len() > 1 {
        return conduit::report(EnvelopeKind::Usage, TOO_MANY_COMMITS);
    }
    let explicit = options.commits.first().map(String::as_str);
    let runtime_rules = options.base_rules.as_deref().unwrap_or("");
    let range = match load_range_data(
        cwd,
        merged,
        explicit,
        &options.head,
        options.head_explicit,
        runtime_rules,
        &prompt_line,
    ) {
        Ok(range) => range,
        Err(message) => return report_which_error(&message),
    };
    let head = options.head_explicit.then_some(options.head.as_str());
    let arg = diff_arg(explicit, head);
    print!("{}", render_range(&range, &arg, head, ansi));
    let revisions = match match_working_copy_revisions(
        client,
        cwd,
        &range.base,
        &range.head,
        options.any_status,
    ) {
        Ok(revisions) => revisions,
        Err(message) => return report_which_error(&message),
    };
    print!("{}", render_matching(&revisions, &arg, ansi));
    ExitCode::SUCCESS
}

/// `--show-base` prints the base commit only: authenticated like the
/// main report, but with no repository section and no commit listing
/// (`ArcanistWhichWorkflow.php:103-110` at ceb).
fn run_show_base(cwd: &Path, options: &Options, merged: &get_config::MergedConfig) -> ExitCode {
    if options.commits.len() > 1 {
        return conduit::report(EnvelopeKind::Usage, TOO_MANY_COMMITS);
    }
    let inputs = BaseInputs {
        runtime_rules: options.base_rules.as_deref().unwrap_or(""),
        prompt: &prompt_line,
    };
    match resolve_base(
        cwd,
        options.commits.first().map(String::as_str),
        &options.head,
        options.head_explicit,
        merged,
        &inputs,
    ) {
        Ok((base, _)) => {
            println!("{base}");
            ExitCode::SUCCESS
        }
        Err(message) => report_which_error(&message),
    }
}

/// `--json` builds the whole context before printing anything.
fn run_json(
    cwd: &Path,
    user_file: Option<&Path>,
    options: &Options,
    merged: &get_config::MergedConfig,
    client: &conduit::Client,
) -> ExitCode {
    if options.commits.len() > 1 {
        return conduit::report(EnvelopeKind::Usage, TOO_MANY_COMMITS);
    }
    let env_uri = std::env::var("ARC_CONDUIT_URI").ok();
    let inputs = BaseInputs {
        runtime_rules: options.base_rules.as_deref().unwrap_or(""),
        prompt: &prompt_line,
    };
    let ctx = match load_context(
        cwd,
        user_file,
        options.conduit_uri.as_deref(),
        env_uri.as_deref(),
        options.commits.first().map(String::as_str),
        &options.head,
        options.head_explicit,
        merged,
        &inputs,
        client,
        options.any_status,
    ) {
        Ok(ctx) => ctx,
        Err(message) => return report_which_error(&message),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&report_json(&ctx)).unwrap_or_else(|_| "{}".to_string())
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "which_tests.rs"]
mod which_tests;
