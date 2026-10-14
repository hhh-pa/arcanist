//! `arc-rs which` — explain the resolved config/repo context.
//!
//! PHP reference: `src/workflow/ArcanistWhichWorkflow.php` (repository,
//! commit-range, and matching-revision sections).
//!
//! This port reports how arc-rs resolves the working copy, project,
//! conduit endpoint, and commit range for a directory, using only local
//! state plus `~/.arcrc`. Querying Differential for matching revisions
//! needs conduit search calls that are not yet ported, so `--any-status`
//! fails and the report says so (see T1128); likewise `--base` rules.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{config, git};

/// Subcommand name, matching `arc which`.
pub const NAME: &str = "which";

const USAGE: &str = "usage: arc-rs which [--conduit-uri URI] [--head COMMIT] [--show-base] [--json] [COMMIT]\n\
\n\
Explain how arc-rs resolves the working copy, project, conduit, and\n\
commit-range context for the current directory. Optional COMMIT pins the\n\
base of the reported range; --head pins its end (default HEAD).\n\
\n\
Options:\n\
  --conduit-uri URI   Override Conduit URI discovery for this report.\n\
  --head COMMIT       End of the commit range (default HEAD).\n\
  --show-base         Print the base commit only and exit.\n\
  --json              Emit the report as JSON.\n\
  -h, --help          Show this help.\n\
\n\
Not yet ported: Differential revision matching (--any-status), base\n\
rules (--base); see T1128.\n";

/// One commit in the reported range.
pub struct Commit {
    pub sha: String,
    pub subject: String,
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
    /// Commits in `base..head`, oldest first.
    pub commits: Vec<Commit>,
}

/// Resolve the conduit URI with its source, mirroring
/// [`config::client_for`] precedence: flag, environment, `.arcconfig`.
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
    match config::discover_from(dir) {
        Some(uri) => (Some(uri), "project (.arcconfig)"),
        None => (None, "none"),
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
pub fn find_project(dir: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    for ancestor in dir.ancestors() {
        let file = ancestor.join(".arcconfig");
        if file.is_file() {
            return (Some(ancestor.to_path_buf()), Some(file));
        }
    }
    (None, None)
}

/// Read `repository.callsign` from the project file, else the user file's
/// `config` section.
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
        && let Ok(text) = std::fs::read_to_string(path)
        && let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(callsign) = doc
            .get("config")
            .and_then(|c| c.get("repository.callsign"))
            .and_then(|v| v.as_str())
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

/// Pick the range base: explicit commit, merge-base with the upstream, or
/// the head itself when no upstream exists.
pub fn resolve_base(
    dir: &Path,
    explicit: Option<&str>,
    head_ref: &str,
) -> Result<(String, String), String> {
    if let Some(commit) = explicit {
        return rev_to_commit(dir, commit)
            .map(|sha| (sha, "explicit commit argument".to_string()))
            .ok_or_else(|| format!("invalid commit '{commit}'"));
    }
    if git::ok(dir, &["rev-parse", "--verify", "@{upstream}"])
        && let Ok(out) = git::output(dir, &["merge-base", head_ref, "@{upstream}"])
        && out.status.success()
    {
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            return Ok((sha, format!("merge-base of {head_ref} and its upstream")));
        }
    }
    rev_to_commit(dir, head_ref)
        .map(|sha| {
            let why = if git::ok(dir, &["rev-parse", "--verify", "@{upstream}"]) {
                format!("merge-base failed; using {head_ref}")
            } else {
                format!("no upstream configured; using {head_ref}")
            };
            (sha, why)
        })
        .ok_or_else(|| format!("cannot resolve '{head_ref}' (empty repository?)"))
}

/// Commits in `base..head`, oldest first.
pub fn commits_between(dir: &Path, base: &str, head: &str) -> Result<Vec<Commit>, String> {
    let range = format!("{base}..{head}");
    let out = git::output(dir, &["log", "--format=%H %s", "--reverse", &range])
        .map_err(|e| format!("failed to list commits: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let mut commits = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((sha, subject)) = line.split_once(' ') else {
            continue;
        };
        commits.push(Commit {
            sha: sha.to_string(),
            subject: subject.to_string(),
        });
    }
    Ok(commits)
}

/// Build the full context for `dir`. `user_file` is the `~/.arcrc` path
/// (`None` skips token/callsign user lookups), `env` is the
/// `ARC_CONDUIT_URI` value, `explicit_base` pins the base, and `head_ref`
/// pins the range end.
pub fn load_context(
    dir: &Path,
    user_file: Option<&Path>,
    flag_uri: Option<&str>,
    env: Option<&str>,
    explicit_base: Option<&str>,
    head_ref: &str,
) -> Result<Context, String> {
    let in_repo = git::is_inside_work_tree(dir);
    let (vcs_root, branch, head, head_subject, remote_url) = if in_repo {
        let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"]).ok();
        let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
        let head = rev_to_commit(dir, head_ref);
        let head_subject = head
            .as_ref()
            .and_then(|sha| git::stdout_text(dir, &["log", "-1", "--format=%s", sha]).ok());
        let remote = git::stdout_text(dir, &["config", "--get", "remote.origin.url"]).ok();
        (root, branch, head, head_subject, remote)
    } else {
        (None, None, None, None, None)
    };
    let (project_root, project_file) = find_project(dir);
    let callsign = read_callsign(project_file.as_deref(), user_file);
    let (conduit_uri, conduit_source) = resolve_conduit_in(dir, flag_uri, env);
    let token = conduit_uri
        .as_ref()
        .map(|uri| token_installed(user_file, uri));
    let (base, explanation, commits) = if in_repo {
        let (base, why) = resolve_base(dir, explicit_base, head_ref)?;
        let commits = commits_between(dir, &base, head_ref)?;
        (Some(base), Some(why), commits)
    } else {
        (None, None, Vec::new())
    };
    Ok(Context {
        dir: dir.to_string_lossy().to_string(),
        vcs: if in_repo { "git" } else { "none" },
        vcs_root,
        project_root: project_root.map(|p| p.to_string_lossy().to_string()),
        project_file: project_file.map(|p| p.to_string_lossy().to_string()),
        branch,
        head,
        head_subject,
        remote_url,
        callsign: callsign.as_ref().map(|(name, _)| name.clone()),
        callsign_source: callsign.map(|(_, source)| source),
        conduit_uri,
        conduit_source,
        token_installed: token,
        base,
        base_explanation: explanation,
        head_ref: head_ref.to_string(),
        commits,
    })
}

/// Render the human-readable report.
#[must_use]
pub fn render(ctx: &Context) -> String {
    let mut out = String::new();
    out.push_str("WORKING COPY\n");
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
    out.push_str("\nREPOSITORY\n");
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
    out.push_str("\nCONDUIT\n");
    match &ctx.conduit_uri {
        Some(uri) => out.push_str(&format!("    URI: {uri} (from {})\n", ctx.conduit_source)),
        None => out.push_str("    URI: none configured\n"),
    }
    match ctx.token_installed {
        Some(true) => out.push_str("    Token: installed\n"),
        Some(false) => out.push_str("    Token: missing (run `arc-rs install-certificate`)\n"),
        None => out.push_str("    Token: unknown (no conduit URI)\n"),
    }
    out.push_str("\nCOMMIT RANGE\n");
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
    out.push_str("\nMATCHING REVISIONS\n");
    out.push_str("    (Not yet ported: Differential revision matching; see T1128.)\n");
    out
}

/// Render the machine-readable report.
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
        "base": ctx.base,
        "base_explanation": ctx.base_explanation,
        "head_ref": ctx.head_ref,
        "commits": ctx.commits.iter().map(|c| serde_json::json!({"commit": c.sha, "summary": c.subject})).collect::<Vec<_>>(),
    })
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    conduit_uri: Option<String>,
    head: String,
    show_base: bool,
    json: bool,
    commit: Option<String>,
}

enum Parsed {
    Help,
    Options(Options),
    NotYetPorted(String),
    Error(String),
}

fn parse_args(argv: &[String]) -> Parsed {
    let mut conduit_uri = None;
    let mut head = "HEAD".to_string();
    let mut show_base = false;
    let mut json = false;
    let mut commit = None;
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--conduit-uri" {
            let Some(value) = iter.next() else {
                return Parsed::Error("--conduit-uri requires a value".to_string());
            };
            conduit_uri = Some(value.clone());
        } else if arg == "--head" {
            let Some(value) = iter.next() else {
                return Parsed::Error("--head requires a value".to_string());
            };
            head = value.clone();
        } else if arg == "--show-base" {
            show_base = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--base" {
            return Parsed::NotYetPorted(format!(
                "arc-rs {NAME}: --base is not yet ported (base rules; see T1128)"
            ));
        } else if arg == "--any-status" {
            return Parsed::NotYetPorted(format!(
                "arc-rs {NAME}: --any-status is not yet ported (Differential revision matching; see T1128)"
            ));
        } else if arg == "-h" || arg == "--help" {
            return Parsed::Help;
        } else if arg == "--" {
            for rest in iter.by_ref() {
                if commit.is_some() {
                    return Parsed::Error("Specify at most one commit.".to_string());
                }
                commit = Some(rest.clone());
            }
        } else if arg.starts_with('-') {
            return Parsed::Error(format!("unknown flag '{arg}'"));
        } else {
            if commit.is_some() {
                return Parsed::Error("Specify at most one commit.".to_string());
            }
            commit = Some(arg.clone());
        }
    }
    Parsed::Options(Options {
        conduit_uri,
        head,
        show_base,
        json,
        commit,
    })
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Parsed::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::NotYetPorted(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
        Parsed::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Parsed::Options(options) => options,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let env_uri = std::env::var("ARC_CONDUIT_URI").ok();
    let user_file = config::arcrc_path();
    let ctx = match load_context(
        &cwd,
        user_file.as_deref(),
        options.conduit_uri.as_deref(),
        env_uri.as_deref(),
        options.commit.as_deref(),
        &options.head,
    ) {
        Ok(ctx) => ctx,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    if options.show_base {
        match ctx.base {
            Some(base) => {
                println!("{base}");
                return ExitCode::SUCCESS;
            }
            None => {
                eprintln!("arc-rs {NAME}: no commit range outside a working copy");
                return ExitCode::FAILURE;
            }
        }
    }
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

#[cfg(test)]
#[path = "which_tests.rs"]
mod which_tests;
