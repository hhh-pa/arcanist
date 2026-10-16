//! `arc which` — explain the resolved config/repo context.
//!
//! PHP reference: `src/workflow/ArcanistWhichWorkflow.php`, the base
//! commit rules (`src/parser/ArcanistBaseCommitParser.php`), and the git
//! pieces of `src/repository/api/ArcanistGitAPI.php`.
//!
//! Deviations (see T1128): the report keeps arc-rs's own section layout
//! instead of PHP's console-formatted sections; long guidance paragraphs
//! are not word-wrapped; only git working copies are detected; revision
//! matching is opportunistic — when no conduit client is available the
//! matching section says why instead of failing like PHP.
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

const USAGE: &str = "usage: arc which [--conduit-uri URI] [--head COMMIT] [--show-base] [--json] [--base RULES] [--any-status] [COMMIT]\n\
|\n\
|Explain how arc resolves the working copy, project, conduit, and\n\
|commit-range context for the current directory. Optional COMMIT pins the\n\
|base of the reported range; --head pins its end (default HEAD).\n\
|\n\
|Options:\n\
|  --conduit-uri URI   Override Conduit URI discovery for this report.\n\
|  --head COMMIT       End of the commit range (default HEAD).\n\
|  --show-base         Print the base commit only and exit.\n\
|  --base RULES        Additional rules for determining the base commit.\n\
|  --any-status        Show committed and abandoned revisions.\n\
|  --json              Emit the report as JSON.\n\
|  -h, --help          Show this help.\n";

/// One commit in the reported range.
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

/// Revision-matching outcome for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matching {
    OutsideRepo,
    Unavailable(String),
    Found(Vec<MatchedRevision>),
}

/// Resolved context for one directory.
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
    /// `repository.callsign` with where it came from.
    pub callsign: Option<String>,
    pub callsign_source: Option<&'static str>,
    /// Resolved conduit URI, if any.
    pub conduit_uri: Option<String>,
    /// Where the conduit URI came from.
    pub conduit_source: &'static str,
    /// Whether `~/.arcrc` holds a token for the URI.
    pub token_installed: Option<bool>,
    /// Base commit of the range.
    pub base: Option<String>,
    /// Why that base was chosen.
    pub base_explanation: Option<String>,
    /// End of the range (`--head` or `HEAD`).
    pub head_ref: String,
    /// Commits in the range, oldest first.
    pub commits: Vec<Commit>,
    /// Argument suffix for `arc diff` guidance (`' X'` or `''`).
    pub diff_arg: String,
    /// Differential revisions matching the working copy.
    pub matching: Matching,
}

/// How `load_context` should match revisions.
pub enum MatchConfig<'a> {
    /// Skip matching (e.g. `--show-base`); the section is still rendered.
    Unavailable(String),
    /// Match through this client; `any_status` selects `status-any`.
    Live {
        client: &'a conduit::Client,
        any_status: bool,
    },
}

/// Interactive inputs for [`load_context`]: `--base` rules, a prompt hook
/// (`None` on EOF), and revision matching. The production hook reports
/// a non-TTY stdin out-of-band (see [`take_prompt_tty_failed`]); a
/// scripted `None` still takes the caller's default.
pub struct Resolution<'a> {
    pub runtime_rules: &'a str,
    pub prompt: &'a dyn Fn(&str) -> Option<String>,
    pub match_config: MatchConfig<'a>,
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

/// Read `repository.callsign` from the project file, else the user file
/// through the classic `readUserArcConfig()` transform (T1153 CORE-30:
/// the `config` section when present, else the whole file, so modern
/// top-level keys resolve).
#[must_use]
pub fn read_callsign(
    project_file: Option<&Path>,
    user_file: Option<&Path>,
) -> Option<(String, &'static str)> {
    if let Some(path) = project_file
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(callsign) = doc.get("repository.callsign").and_then(|v| v.as_str())
    {
        return Some((callsign.to_string(), "project (.arcconfig)"));
    }
    if let Some(path) = user_file
        && let Ok(user) = get_config::read_user_file(path)
        && let Some(callsign) = user.get("repository.callsign").and_then(|v| v.as_str())
    {
        return Some((callsign.to_string(), "user (~/.arcrc)"));
    }
    None
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

/// Resolve the range head like `getHeadCommit()`, with git's own error.
fn resolve_head(dir: &Path, head_ref: &str) -> Result<String, String> {
    let out = git::output(dir, &["show", "-s", "--format=%H", head_ref, "--"])
        .map_err(|e| format!("cannot resolve '{head_ref}': {e}"))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if detail.is_empty() {
            return Err(format!("cannot resolve '{head_ref}' (empty repository?)"));
        }
        return Err(detail);
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

    fn is_empty(&self) -> bool {
        self.local.trim().is_empty()
            && self.project.trim().is_empty()
            && self.user.trim().is_empty()
            && self.system.trim().is_empty()
    }
}

/// Load every config source for `dir`.
pub fn load_config_sources(
    dir: &Path,
    user_file: Option<&Path>,
) -> Result<get_config::Sources, String> {
    get_config::load_sources(dir, user_file)
}

/// `base` rules from loaded sources (`local`, `project`, `user`, `system`).
#[must_use]
pub fn base_sources_from(loaded: &get_config::Sources) -> BaseSources {
    let at = |source: &str| {
        get_config::value_at("base", source, loaded)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default()
    };
    BaseSources {
        local: at("local"),
        project: at("project"),
        user: at("user"),
        system: at("system"),
    }
}

/// Split a `base` specification into rules, mirroring
/// `tokenizeBaseCommitSpecification()`.
pub fn tokenize_base_rules(raw: &str) -> Result<Vec<String>, String> {
    let mut rules = Vec::new();
    for rule in raw.split(',').map(str::trim).filter(|r| !r.is_empty()) {
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
            let mut cmd = std::process::Command::new("cmd");
            cmd.arg("/C").arg(command);
            cmd
        } else {
            let mut cmd = std::process::Command::new("sh");
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
                let out = git::output(self.dir, &["merge-base", "--", &upstream, "HEAD"])
                    .map_err(|e| format!("git merge-base failed: {e}"))?;
                if !out.status.success() {
                    return Err(format!(
                        "git merge-base failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
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
        if let Some(inner) = strip_wrapped(name, "merge-base(", ")") {
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
        if let Some(inner) = strip_wrapped(name, "branch-unique(", ")") {
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
fn strip_wrapped<'b>(name: &'b str, prefix: &str, suffix: &str) -> Option<&'b str> {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
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
    loaded: &get_config::Sources,
    inputs: &BaseInputs<'_>,
) -> Result<(String, String), String> {
    if let Some(commit) = explicit {
        return resolve_explicit_base(dir, commit, head_ref, head_explicit);
    }
    if is_shallow_history(dir) {
        return Ok(shallow_base(dir));
    }
    let sources = base_sources_from(loaded);
    if !inputs.runtime_rules.trim().is_empty() || !sources.is_empty() {
        let all = [
            source_rules("runtime", inputs.runtime_rules, &sources)?,
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
        return Err(
            "None of the rules in your 'base' configuration matched a valid commit. \
            Adjust rules or specify which commit you want to use explicitly."
                .to_string(),
        );
    }
    resolve_default_base(dir, loaded, inputs.prompt)
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
        return Err(format!(
            "Unable to find any git commit named '{commit}' in this repository."
        ));
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

/// Commits for the range, oldest first, mirroring
/// `getLocalCommitInformation()` (`HEAD --not base`, or all of HEAD past
/// the empty tree).
pub fn local_commit_information(
    dir: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<LocalCommit>, String> {
    let format = "%H%x01%T%x01%s%x01%B%x02";
    let flag = std::format!("--format={format}");
    let out = if base == GIT_MAGIC_ROOT_COMMIT {
        git::output(dir, &["log", &flag, "HEAD", "--"])
    } else {
        git::output(dir, &["log", head, "--not", base, &flag, "--"])
    }
    .map_err(|e| format!("failed to list commits: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(parse_commit_records(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_commit_records(text: &str) -> Vec<LocalCommit> {
    let mut commits = Vec::new();
    for record in text.split('\u{2}') {
        let record = record.trim_matches([' ', '\n', '\u{2}']);
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(4, '\u{1}');
        let (Some(commit), Some(tree), Some(summary), Some(message)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        commits.push(LocalCommit {
            commit: commit.to_string(),
            tree: tree.to_string(),
            summary: summary.to_string(),
            message: message.trim_end().to_string(),
        });
    }
    commits
}

/// Commits in the range, oldest first.
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
/// How `/^Differential Revision:\s*(.+)/im` matches at one line start:
/// no match (try the next), a whitespace-only value (throw), or a raw
/// value (monogram, URI, or throw).
enum FooterValue<'a> {
    None,
    Empty,
    Value(&'a str),
}

/// PCRE `\s`: space, tab, newline, CR, form feed, vertical tab.
const FOOTER_SPACE: [char; 6] = [' ', '\t', '\n', '\r', '\u{c}', '\u{b}'];

/// PHP `trim()` charlist (no form feed).
const TRIM_CHARS: [char; 6] = [' ', '\t', '\n', '\r', '\0', '\u{b}'];

fn footer_value(after: &str) -> FooterValue<'_> {
    let stripped = after.trim_start_matches(FOOTER_SPACE);
    if !stripped.is_empty() {
        let raw = stripped.split('\n').next().unwrap_or("");
        return FooterValue::Value(raw);
    }
    let run = &after[..after.len() - stripped.len()];
    if run.bytes().any(|b| b != b'\n') {
        FooterValue::Empty
    } else {
        FooterValue::None
    }
}

fn invalid_footer(value: &str) -> String {
    format!(
        "Invalid \"Differential Revision\" field in commit message. This field should have a \
         revision identifier like \"D123\" or a server URI like \
         \"https://devtools.example.com/D123\", but has \"{value}\"."
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
        match footer_value(after) {
            FooterValue::None => continue,
            FooterValue::Empty => return Err(invalid_footer("")),
            FooterValue::Value(raw) => {
                let value = raw.trim_matches(TRIM_CHARS);
                if let Some(id) = bare_revision_id(value) {
                    return Ok(Some(id));
                }
                if let Some(id) = uri_revision_id(value) {
                    return Ok(Some(id));
                }
                return Err(invalid_footer(value));
            }
        }
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

/// A server-URI footer value: the whole URI path must be a monogram.
fn uri_revision_id(value: &str) -> Option<String> {
    let without_query = value.split(['?', '#']).next().unwrap_or("");
    let path = match without_query.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, path)| path),
        None => without_query,
    };
    bare_revision_id(path.trim_matches('/'))
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

/// Resolve other authors' names via `user.query`, like the PHP workflow.
fn attach_authors(
    client: &conduit::Client,
    rows: Vec<(String, String, String, String)>,
) -> Result<Vec<MatchedRevision>, String> {
    // T1153 RB-D8: PHP resolves whoami once at startup auth, then
    // reuses the cached viewer PHID (`getUserPHID()`); only call when
    // uncached (e.g. direct library use without `config::authenticate`).
    let cached = client.authenticated_user();
    let who;
    let own = match &cached {
        Some((_, phid)) => phid.as_str(),
        None => {
            who = client
                .call("user.whoami", &serde_json::json!({}))
                .map_err(|e| e.to_string())?;
            who.get("phid").and_then(|v| v.as_str()).unwrap_or("")
        }
    };
    let mut others: Vec<String> = Vec::new();
    for (_, _, author, _) in &rows {
        if author != own && !others.iter().any(|p| p == author) {
            others.push(author.clone());
        }
    }
    let mut names: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    if !others.is_empty() {
        let users = client
            .call("user.query", &serde_json::json!({"phids": others}))
            .map_err(|e| e.to_string())?;
        for user in query_rows(&users) {
            names.insert(row_string(user, "phid"), row_string(user, "userName"));
        }
    }
    Ok(rows
        .into_iter()
        .map(|(id, title, author, why)| {
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
        })
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

struct RangeResolution {
    base: String,
    explanation: String,
    commits: Vec<Commit>,
    matching: Matching,
}

fn resolve_report_range(
    dir: &Path,
    user_file: Option<&Path>,
    explicit_base: Option<&str>,
    head_ref: &str,
    head_explicit: bool,
    resolution: &Resolution<'_>,
) -> Result<RangeResolution, String> {
    let loaded = load_config_sources(dir, user_file)?;
    let inputs = BaseInputs {
        runtime_rules: resolution.runtime_rules,
        prompt: resolution.prompt,
    };
    let (base, why) = resolve_base(
        dir,
        explicit_base,
        head_ref,
        head_explicit,
        &loaded,
        &inputs,
    )?;
    if !has_any_commit(dir) {
        return Err(
            "You can't get local commit information for a repository with no commits.".to_string(),
        );
    }
    let resolved_head = resolve_head(dir, head_ref)?;
    if head_explicit && base != GIT_MAGIC_ROOT_COMMIT {
        check_range_direction(dir, &base, &resolved_head, head_ref)?;
    }
    let commits = commits_between(dir, &base, head_ref)?;
    let matching =
        match &resolution.match_config {
            MatchConfig::Unavailable(reason) => Matching::Unavailable(reason.clone()),
            MatchConfig::Live { client, any_status } => Matching::Found(
                match_working_copy_revisions(client, dir, &base, head_ref, *any_status)?,
            ),
        };
    Ok(RangeResolution {
        base,
        explanation: why,
        commits,
        matching,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn load_context(
    dir: &Path,
    user_file: Option<&Path>,
    flag_uri: Option<&str>,
    env: Option<&str>,
    explicit_base: Option<&str>,
    head_ref: &str,
    head_explicit: bool,
    resolution: &Resolution<'_>,
) -> Result<Context, String> {
    let in_repo = git::is_inside_work_tree(dir);
    let identity = load_repo_identity(dir, head_ref, in_repo);
    let (project_root, project_file) = find_project(dir);
    let callsign = read_callsign(project_file.as_deref(), user_file);
    let (conduit_uri, conduit_source) = resolve_conduit_in(dir, flag_uri, env);
    let token = conduit_uri
        .as_ref()
        .map(|uri| token_installed(user_file, uri));
    let (base, explanation, commits, matching) = if in_repo {
        let range = resolve_report_range(
            dir,
            user_file,
            explicit_base,
            head_ref,
            head_explicit,
            resolution,
        )?;
        (
            Some(range.base),
            Some(range.explanation),
            range.commits,
            range.matching,
        )
    } else {
        (None, None, Vec::new(), Matching::OutsideRepo)
    };
    let (callsign_name, callsign_source) = match callsign {
        Some((name, source)) => (Some(name), Some(source)),
        None => (None, None),
    };
    Ok(Context {
        dir: dir.to_string_lossy().to_string(),
        vcs: if in_repo { "git" } else { "none" },
        vcs_root: identity.vcs_root,
        project_root: project_root.map(path_display),
        project_file: project_file.map(path_display),
        branch: identity.branch,
        head: identity.head,
        head_subject: identity.head_subject,
        remote_url: identity.remote_url,
        callsign: callsign_name,
        callsign_source,
        conduit_uri,
        conduit_source,
        token_installed: token,
        base,
        base_explanation: explanation,
        head_ref: head_ref.to_string(),
        commits,
        diff_arg: match explicit_base {
            Some(base) => format!(" {base}"),
            None => String::new(),
        },
        matching,
    })
}

/// Bold a section header or verdict word on TTYs (`**%s**`), plain
/// otherwise — the same markup PHP applies to these words.
fn bold(text: &str, ansi: bool) -> String {
    terminal::console_format("**%s**", &[text], ansi)
}

fn render_working_copy(ctx: &Context, out: &mut String, ansi: bool) {
    out.push_str(&bold("WORKING COPY", ansi));
    out.push('\n');
    out.push_str(&format!("    Directory: {}\n", ctx.dir));
    match (&ctx.vcs_root, ctx.vcs) {
        (Some(root), vcs) => out.push_str(&format!("    VCS: {vcs} (root {root})\n")),
        (None, _) => out.push_str("    VCS: none (not inside a working copy)\n"),
    }
    match (&ctx.project_root, &ctx.project_file) {
        (Some(root), Some(file)) => {
            out.push_str(&format!("    Project: {root} (from {file})\n"));
        }
        _ => out.push_str("    Project: no .arcconfig found above this directory\n"),
    }
}

fn render_repository(ctx: &Context, out: &mut String, ansi: bool) {
    out.push('\n');
    out.push_str(&bold("REPOSITORY", ansi));
    out.push('\n');
    out.push_str(&format!(
        "    Branch: {}\n",
        ctx.branch.as_deref().unwrap_or("HEAD (detached)")
    ));
    match (&ctx.head, &ctx.head_subject) {
        (Some(head), Some(subject)) => out.push_str(&format!("    HEAD: {head} {subject}\n")),
        (Some(head), None) => out.push_str(&format!("    HEAD: {head}\n")),
        (None, _) => out.push_str("    HEAD: none\n"),
    }
    out.push_str(&format!(
        "    Remote (origin): {}\n",
        ctx.remote_url.as_deref().unwrap_or("none")
    ));
    match (&ctx.callsign, &ctx.callsign_source) {
        (Some(name), Some(source)) => {
            out.push_str(&format!("    Callsign: {name} (from {source})\n"))
        }
        (Some(name), None) => out.push_str(&format!("    Callsign: {name}\n")),
        (None, _) => out.push_str("    Callsign: not configured\n"),
    }
}

fn render_conduit(ctx: &Context, out: &mut String, ansi: bool) {
    out.push('\n');
    out.push_str(&bold("CONDUIT", ansi));
    out.push('\n');
    match &ctx.conduit_uri {
        Some(uri) => out.push_str(&format!("    URI: {uri} (from {})\n", ctx.conduit_source)),
        None => out.push_str("    URI: none configured\n"),
    }
    match ctx.token_installed {
        Some(true) => out.push_str("    Token: installed\n"),
        Some(false) => out.push_str("    Token: missing (run `arc install-certificate`)\n"),
        None => out.push_str("    Token: unknown (no conduit URI)\n"),
    }
}

fn render_range(ctx: &Context, out: &mut String, ansi: bool) {
    out.push('\n');
    out.push_str(&bold("COMMIT RANGE", ansi));
    out.push('\n');
    match (&ctx.base, &ctx.base_explanation) {
        (Some(base), Some(why)) => {
            out.push_str(&format!("    Base: {base} ({why})\n"));
            out.push_str(&format!("    Head: {}\n", ctx.head_ref));
            out.push_str(&format!("    Command: git diff {base}..{}\n", ctx.head_ref));
            if ctx.commits.is_empty() {
                out.push_str("    (No commits.)\n");
            } else {
                for commit in &ctx.commits {
                    let short = commit.sha.chars().take(16).collect::<String>();
                    out.push_str(&format!("    {short}  {}\n", commit.subject));
                }
            }
        }
        _ => out.push_str("    (No commit range outside a working copy.)\n"),
    }
}

/// `created` guidance: PHP bolds the verdict (`**created**`) in the
/// same sentence, so it substitutes as a marked value here.
fn created_guidance(diff_arg: &str, ansi: bool) -> String {
    let command = format!("arc diff{diff_arg}");
    terminal::console_format(
        "Since there are no revisions in Differential which match this working copy, a \
         new revision will be **%s** if you run '%s'.\n\n",
        &["created", &command],
        ansi,
    )
}

/// `updated` guidance: same shape, PHP bolds `**updated**`.
fn updated_guidance(diff_arg: &str, ansi: bool) -> String {
    let command = format!("arc diff{diff_arg}");
    terminal::console_format(
        "Since exactly one revision in Differential matches this working copy, it \
         will be **%s** if you run '%s'.\n\n",
        &["updated", &command],
        ansi,
    )
}

fn render_matching(ctx: &Context, out: &mut String, ansi: bool) {
    out.push('\n');
    out.push_str(&bold("MATCHING REVISIONS", ansi));
    out.push('\n');
    match &ctx.matching {
        Matching::OutsideRepo => {
            out.push_str("    (No revisions match outside a working copy.)\n");
        }
        Matching::Unavailable(reason) => {
            out.push_str(&format!("    (Revision matching unavailable: {reason})\n"));
        }
        Matching::Found(revisions) if revisions.is_empty() => {
            out.push_str("    (No revisions match.)\n\n");
            out.push_str(&created_guidance(&ctx.diff_arg, ansi));
        }
        Matching::Found(revisions) => {
            for revision in revisions {
                match &revision.author {
                    Some(author) => out.push_str(&format!(
                        "    {} ({author}) {}\n",
                        revision.monogram, revision.title
                    )),
                    None => {
                        out.push_str(&format!("    {} {}\n", revision.monogram, revision.title))
                    }
                }
                out.push_str(&format!("        Reason: {}\n\n", revision.why));
            }
            if revisions.len() == 1 {
                out.push_str(&updated_guidance(&ctx.diff_arg, ansi));
            } else {
                out.push_str(&format!(
                    "Since more than one revision in Differential matches this working copy, you \
                     will be asked which revision you want to update if you run 'arc diff {}'.\n\n",
                    ctx.diff_arg
                ));
            }
        }
    }
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
    let mut out = String::new();
    render_working_copy(ctx, &mut out, ansi);
    render_repository(ctx, &mut out, ansi);
    render_conduit(ctx, &mut out, ansi);
    render_range(ctx, &mut out, ansi);
    render_matching(ctx, &mut out, ansi);
    out
}

/// Render the machine-readable report.
#[must_use]
pub fn report_json(ctx: &Context) -> serde_json::Value {
    let (matching, matching_note): (Vec<serde_json::Value>, Option<String>) = match &ctx.matching {
        Matching::OutsideRepo => (Vec::new(), Some("outside a working copy".to_string())),
        Matching::Unavailable(reason) => (Vec::new(), Some(reason.clone())),
        Matching::Found(revisions) => (
            revisions
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "revision": r.monogram,
                        "title": r.title,
                        "author": r.author,
                        "why": r.why,
                    })
                })
                .collect(),
            None,
        ),
    };
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
        "base": ctx.base,
        "base_explanation": ctx.base_explanation,
        "head_ref": ctx.head_ref,
        "commits": ctx.commits.iter().map(|c| serde_json::json!({"commit": c.sha, "summary": c.subject})).collect::<Vec<_>>(),
        "matching": matching,
        "matching_note": matching_note,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    conduit_uri: Option<String>,
    head: String,
    head_explicit: bool,
    show_base: bool,
    json: bool,
    base_rules: Option<String>,
    any_status: bool,
    commit: Option<String>,
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
    commit: Option<String>,
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
            commit: None,
        }
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Option<Step> {
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
        if self.commit.is_some() {
            Step::Done(Parsed::Error(
                "Specify exactly one base commit. The end of the commit range is always the \
                 working copy state."
                    .to_string(),
            ))
        } else {
            self.commit = Some(arg.to_string());
            Step::Handled
        }
    }

    fn consume_rest(&mut self, iter: &mut ArgIter<'_>) -> Option<Step> {
        for rest in iter.by_ref() {
            if self.commit.is_some() {
                return Some(Step::Done(Parsed::Error(
                    "Specify exactly one base commit. The end of the commit range is always the \
                     working copy state."
                        .to_string(),
                )));
            }
            self.commit = Some(rest.clone());
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
            commit: self.commit,
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
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::Error(message) => {
            // T1153 R4 (RB-D4): `which` is a subprocess workflow, so
            // usage errors render `Usage Exception:` with no usage dump
            // (`scripts/arcanist.php:432-444`).
            return conduit::report(EnvelopeKind::Usage, &message);
        }
        Parsed::Options(options) => options,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let env_uri = std::env::var("ARC_CONDUIT_URI").ok();
    let user_file = config::arcrc_path();
    if options.show_base {
        return run_show_base(
            &cwd,
            user_file.as_deref(),
            options.commit.as_deref(),
            &options.head,
            options.head_explicit,
            options.base_rules.as_deref().unwrap_or(""),
        );
    }
    // T1153 CORE-4/CORE-9: tokenless engine, then the classic
    // credential check. Without credentials the report stays lenient
    // (matching unavailable, exit 0) as before; with credentials the
    // client verifies like `authenticateConduit()` (`user.whoami` /
    // `conduit.connect`), failing the command on rejection, and the
    // verified identity feeds revision matching (T1153 RB-D8).
    let mut owned_client = config::client_for_tokenless_with(
        options.conduit_uri.as_deref(),
        &cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    );
    let match_config = match &mut owned_client {
        Ok(client) => {
            let creds = config::credentials_for(user_file.as_deref(), client.base_uri(), None);
            if !creds.has_any() {
                MatchConfig::Unavailable(format!("no token for {} in ~/.arcrc", client.base_uri()))
            } else {
                match config::authenticate(client, &creds, &config::command_description()) {
                    Ok(_) => MatchConfig::Live {
                        client,
                        any_status: options.any_status,
                    },
                    Err(error) => return error.report(),
                }
            }
        }
        Err(message) => MatchConfig::Unavailable(message.clone()),
    };
    let resolution = Resolution {
        runtime_rules: options.base_rules.as_deref().unwrap_or(""),
        prompt: &prompt_line,
        match_config,
    };
    let ctx = match load_context(
        &cwd,
        user_file.as_deref(),
        options.conduit_uri.as_deref(),
        env_uri.as_deref(),
        options.commit.as_deref(),
        &options.head,
        options.head_explicit,
        &resolution,
    ) {
        Ok(ctx) => ctx,
        // T1153 R4 (RB-D4): context/resolution failures are generic
        // exceptions under the red banner (`:449-468`).
        Err(message) => return conduit::report(EnvelopeKind::Exception, &message),
    };
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&ctx)).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        print!("{}", render(&ctx));
    }
    ExitCode::SUCCESS
}

/// `--show-base` prints the base commit only, without conduit.
fn run_show_base(
    cwd: &Path,
    user_file: Option<&Path>,
    explicit: Option<&str>,
    head_ref: &str,
    head_explicit: bool,
    runtime_rules: &str,
) -> ExitCode {
    if !git::is_inside_work_tree(cwd) {
        // T1153 R4 (RB-D4): the working-copy gate is a usage error.
        return conduit::report(
            EnvelopeKind::Usage,
            "no commit range outside a working copy",
        );
    }
    let loaded = match load_config_sources(cwd, user_file) {
        Ok(loaded) => loaded,
        Err(message) => return conduit::report(EnvelopeKind::Exception, &message),
    };
    let inputs = BaseInputs {
        runtime_rules,
        prompt: &prompt_line,
    };
    match resolve_base(cwd, explicit, head_ref, head_explicit, &loaded, &inputs) {
        Ok((base, _)) => {
            println!("{base}");
            ExitCode::SUCCESS
        }
        Err(message) => conduit::report(EnvelopeKind::Exception, &message),
    }
}

#[cfg(test)]
#[path = "which_tests.rs"]
mod which_tests;
