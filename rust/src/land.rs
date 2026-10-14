//! `land` workflow: squash-merge the current branch onto a target branch.
//!
//! Tracks T1128. Mirrors the core `arc land` flow from
//! `src/workflow/ArcanistLandWorkflow.php` under the default "squash"
//! strategy: check out the target branch, squash-merge the feature branch,
//! and commit the result as a single commit naming the branch.
//!
//! Out of scope (not yet ported, see T1128): Differential accepted-status
//! checks, and landing directly onto remote-tracking branches.

use std::path::Path;
use std::process::{Command, ExitCode};

pub const NAME: &str = "land";

const DEFAULT_ONTO: &str = "master";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    onto: String,
    preview: bool,
    push: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub branch: String,
    pub onto: String,
    pub base: String,
    pub head: String,
    pub commits: Vec<Commit>,
}

fn usage() -> String {
    format!(
        "usage: arc-rs {NAME} [--onto BRANCH] [--preview] [--push]\n\
         \n\
         Squash-merge the current branch onto BRANCH (default: {DEFAULT_ONTO})\n\
         and commit the result as a single commit naming the branch.\n\
         \n\
         Options:\n\
         \x20 --onto BRANCH  land onto BRANCH instead of {DEFAULT_ONTO} (must be a local branch)\n\
         \x20 --preview      print the commits that would land; change nothing\n\
         \x20 --push         push the target branch after landing\n\
         \n\
         Not yet ported (see T1128): Differential accepted-status checks,\n\
         landing directly onto remote-tracking branches.\n"
    )
}

fn wants_help(argv: &[String]) -> bool {
    argv.iter()
        .any(|arg| arg == "-h" || arg == "--help" || arg == "help")
}

fn parse_args(argv: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        onto: DEFAULT_ONTO.to_string(),
        preview: false,
        push: false,
    };
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--onto" {
            match iter.next() {
                Some(value) if !value.is_empty() && !value.starts_with('-') => {
                    opts.onto = value.clone();
                }
                _ => return Err("--onto requires a non-empty branch name".to_string()),
            }
        } else if let Some(value) = arg.strip_prefix("--onto=") {
            if value.is_empty() {
                return Err("--onto requires a non-empty branch name".to_string());
            }
            opts.onto = value.to_string();
        } else if arg == "--preview" {
            opts.preview = true;
        } else if arg == "--push" {
            opts.push = true;
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            return Err(format!("unexpected argument '{arg}'"));
        }
    }
    Ok(opts)
}

/// Run `git` in `dir`, returning trimmed stdout or trimmed combined output.
fn git_output(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}{stderr}");
        let trimmed = combined.trim();
        if trimmed.is_empty() {
            Err(format!("git {} failed", args.join(" ")))
        } else {
            Err(trimmed.to_string())
        }
    }
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn is_remote_tracking(dir: &Path, onto: &str) -> bool {
    let Ok(remotes) = git_output(dir, &["remote"]) else {
        return false;
    };
    remotes.lines().map(str::trim).any(|remote| {
        !remote.is_empty()
            && onto
                .strip_prefix(remote)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Fail unless the working tree has no staged, unstaged, or untracked changes.
pub fn ensure_clean(dir: &Path) -> Result<(), String> {
    let status = git_output(dir, &["status", "--porcelain"])?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(
            "working tree is not clean; commit or stash your changes first (or use --preview)"
                .to_string(),
        )
    }
}

/// Validate preconditions and collect the commits `merge-base..HEAD` would land.
///
/// Checks: inside a git repo, on a named branch, `onto` names a local
/// branch, and the commit list is non-empty. Dirt is checked separately by
/// [`ensure_clean`] so `--preview` can skip it.
pub fn plan(dir: &Path, onto: &str) -> Result<Plan, String> {
    if !git_ok(dir, &["rev-parse", "--git-dir"]) {
        return Err("not inside a git repository".to_string());
    }
    let branch = git_output(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    if branch.is_empty() {
        return Err(
            "not on a named branch (detached HEAD); check out a branch to land".to_string(),
        );
    }
    if !git_ok(
        dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{onto}"),
        ],
    ) {
        if is_remote_tracking(dir, onto) {
            return Err(format!(
                "not yet ported: landing directly onto remote-tracking branch '{onto}' is not supported (see T1128)"
            ));
        }
        if git_ok(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{onto}^{{commit}}"),
            ],
        ) {
            return Err(format!("onto target '{onto}' is not a local branch"));
        }
        return Err(format!("onto target '{onto}' does not exist"));
    }
    let base = git_output(dir, &["merge-base", "HEAD", onto]).map_err(|_| {
        format!("cannot find a merge base between '{branch}' and '{onto}' (unrelated histories?)")
    })?;
    let head = git_output(dir, &["rev-parse", "HEAD"])?;
    let range = format!("{base}..{head}");
    let log = git_output(dir, &["log", "--format=%H%x1f%s", &range])?;
    let commits: Vec<Commit> = log
        .lines()
        .filter_map(|line| {
            line.split_once('\x1f').map(|(sha, subject)| Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect();
    if commits.is_empty() {
        return Err(format!(
            "nothing to land: '{branch}' has no commits beyond '{onto}'"
        ));
    }
    Ok(Plan {
        branch,
        onto: onto.to_string(),
        base,
        head,
        commits,
    })
}

pub fn format_preview(plan: &Plan) -> String {
    let mut out = format!(
        "Preview: land '{}' onto '{}' ({} commit(s), nothing changed).\nbase: {}\nhead: {}\ncommits:\n",
        plan.branch,
        plan.onto,
        plan.commits.len(),
        plan.base,
        plan.head
    );
    for commit in &plan.commits {
        out.push_str(&format!("{} {}\n", commit.sha, commit.subject));
    }
    out
}

pub fn build_message(plan: &Plan) -> String {
    let mut message = format!("Land {}\n", plan.branch);
    message.push_str(&format!(
        "\nSquashed {} commit(s) from '{}' onto '{}':\n",
        plan.commits.len(),
        plan.branch,
        plan.onto
    ));
    for commit in &plan.commits {
        let short: String = commit.sha.chars().take(7).collect();
        message.push_str(&format!("{short} {}\n", commit.subject));
    }
    message
}

fn push_remote(dir: &Path) -> Result<String, String> {
    let remotes = git_output(dir, &["remote"])?;
    let names: Vec<&str> = remotes
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.contains(&"origin") {
        return Ok("origin".to_string());
    }
    if names.len() == 1 {
        return Ok(names[0].to_string());
    }
    if names.is_empty() {
        return Err("cannot push: this repository has no git remotes".to_string());
    }
    Err("cannot push: there is no 'origin' remote and several remotes exist".to_string())
}

fn land_plan(dir: &Path, plan: &Plan, push: bool) -> Result<String, String> {
    git_output(dir, &["checkout", "-q", &plan.onto])
        .map_err(|detail| format!("failed to check out '{}': {detail}", plan.onto))?;
    git_output(dir, &["merge", "--squash", &plan.branch]).map_err(|detail| {
        format!(
            "squash-merge of '{}' into '{}' failed: {detail}\nResolve the conflicts and commit, or run 'git reset --hard HEAD' to abort.",
            plan.branch, plan.onto
        )
    })?;
    if git_ok(dir, &["diff", "--cached", "--quiet"]) {
        return Err(format!(
            "squash-merge of '{}' produced no changes relative to '{}'; nothing to commit",
            plan.branch, plan.onto
        ));
    }
    let message = build_message(plan);
    git_output(dir, &["commit", "-q", "-m", &message])
        .map_err(|detail| format!("failed to commit the landed change: {detail}"))?;
    let landed = git_output(dir, &["rev-parse", "HEAD"])?;
    let mut out = format!(
        "Landed '{}' onto '{}' as {}.\n",
        plan.branch, plan.onto, landed
    );
    if push {
        let remote = push_remote(dir)?;
        git_output(dir, &["push", "-q", &remote, &plan.onto]).map_err(|detail| {
            format!(
                "landed '{}' onto '{}' but failed to push to '{remote}': {detail}",
                plan.branch, plan.onto
            )
        })?;
        out.push_str(&format!("Pushed '{}' to '{remote}'.\n", plan.onto));
    }
    Ok(out)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    if wants_help(argv) {
        return Ok(usage());
    }
    let opts = parse_args(argv).map_err(|message| format!("{message}\n\n{}", usage()))?;
    let land = plan(dir, &opts.onto)?;
    if opts.preview {
        let mut out = format_preview(&land);
        if opts.push {
            out.push_str("--push has no effect with --preview; nothing was pushed.\n");
        }
        return Ok(out);
    }
    ensure_clean(dir)?;
    land_plan(dir, &land, opts.push)
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs land: {message}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_string()).collect()
    }

    fn sample_plan() -> Plan {
        Plan {
            branch: "feature".to_string(),
            onto: "master".to_string(),
            base: "base-sha".to_string(),
            head: "head-sha".to_string(),
            commits: vec![
                Commit {
                    sha: "aaa1111aaa1111aaa1111aaa1111aaa1111aaa1".to_string(),
                    subject: "add a".to_string(),
                },
                Commit {
                    sha: "bbb2222bbb2222bbb2222bbb2222bbb2222bbb2".to_string(),
                    subject: "add b".to_string(),
                },
            ],
        }
    }

    #[test]
    fn parses_defaults() {
        assert_eq!(
            parse_args(&args(&[])),
            Ok(Options {
                onto: "master".to_string(),
                preview: false,
                push: false,
            })
        );
    }

    #[test]
    fn parses_onto_forms_and_flags() {
        assert_eq!(
            parse_args(&args(&["--onto", "stable"])).unwrap().onto,
            "stable"
        );
        assert_eq!(
            parse_args(&args(&["--onto=stable"])).unwrap().onto,
            "stable"
        );
        let opts = parse_args(&args(&["--preview", "--push", "--onto", "x"])).unwrap();
        assert!(opts.preview && opts.push && opts.onto == "x");
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&args(&["--onto"])).is_err());
        assert!(parse_args(&args(&["--onto="])).is_err());
        assert!(parse_args(&args(&["--onto", "--preview"])).is_err());
        let err = parse_args(&args(&["--bogus"])).unwrap_err();
        assert!(err.contains("unknown flag"), "unexpected: {err}");
        let err = parse_args(&args(&["feature"])).unwrap_err();
        assert!(err.contains("unexpected argument"), "unexpected: {err}");
    }

    #[test]
    fn detects_help() {
        assert!(wants_help(&args(&["--help"])));
        assert!(wants_help(&args(&["-h"])));
        assert!(wants_help(&args(&["help"])));
        assert!(!wants_help(&args(&[])));
        assert!(!wants_help(&args(&["--preview"])));
    }

    #[test]
    fn preview_names_target_base_head_and_commits() {
        let plan = sample_plan();
        let text = format_preview(&plan);
        assert!(text.contains("'feature'"), "missing branch: {text}");
        assert!(text.contains("'master'"), "missing onto: {text}");
        assert!(text.contains("base-sha"), "missing base: {text}");
        assert!(text.contains("head-sha"), "missing head: {text}");
        assert!(
            text.contains("add a") && text.contains("add b"),
            "missing commits: {text}"
        );
    }

    #[test]
    fn message_names_branch_and_lists_commits() {
        let plan = sample_plan();
        let message = build_message(&plan);
        let first: String = message.lines().take(1).collect();
        assert!(
            first.contains("feature"),
            "subject must name branch: {message}"
        );
        assert!(
            message.contains("add a") && message.contains("add b"),
            "missing commits: {message}"
        );
    }

    #[test]
    fn usage_covers_options_and_limits() {
        let text = usage();
        assert!(text.contains("--onto") && text.contains("--preview") && text.contains("--push"));
        assert!(text.contains("remote-tracking"));
    }
}
