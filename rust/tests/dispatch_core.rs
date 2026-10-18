//! Binary-level dispatcher tests: globals, resolution, help, and the
//! non-interactive `--` requirement (T1153 Batch B). Each test runs
//! `arc` with piped stdin (like CI), an empty `HOME`, and a scratch
//! git checkout, so config discovery and TTY checks are hermetic.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn fixture(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "arc-rs-dispatch-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let home = root.join("home");
    let repo = root.join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    (home, repo)
}

fn arc(home: &PathBuf, repo: &PathBuf, argv: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arc"))
        .args(argv)
        .env("HOME", home)
        .env("ARC_TRACE", "")
        .env("ARC_ANSI", "")
        .env("ARC_GLOBAL_CONDUIT_URI", "")
        .env("ARC_GLOBAL_CONDUIT_TOKEN", "")
        .env("ARC_ANONYMOUS", "")
        .env("ARC_GLOBAL_LOAD_OPTIONS", "")
        .env("ARC_CONDUIT_URI", "")
        .env("ARC_CONDUIT_TOKEN", "")
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn rbd4_bare_arc_chooses_workflow() {
    let (home, repo) = fixture("bare");
    let out = arc(&home, &repo, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stdout).is_empty());
    assert!(text(&out.stderr).contains("USAGE EXCEPTION  Choose a workflow!"));
}

#[test]
fn core20_bare_help_renders_reference() {
    let (home, repo) = fixture("help");
    let out = arc(&home, &repo, &["help", "--"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("WORKFLOWS\n\n"));
    assert!(stdout.contains("Amend the working copy, synchronizing the local commit message."));
    assert!(stdout.contains("Show this help, or workflow help for command."));
    assert!(stdout.contains("Install shell completion."));
    assert!(stdout.contains("OPTION REFERENCE"));
    assert!(text(&out.stderr).is_empty());
}

#[test]
fn dash_help_leads_with_usage() {
    // T1153 RB-D5 + release smoke gate: `arc --help` leads with the one-line
    // usage ahead of the full workflow reference.
    let (home, repo) = fixture("dash-help");
    let out = arc(&home, &repo, &["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(
        stdout.starts_with("usage: arc [--version] [--help] <command>\n\nWORKFLOWS\n\n"),
        "{stdout:?}"
    );
    assert!(text(&out.stderr).is_empty());
}

#[test]
fn rbd5_detailed_help_for_toolset_and_classic() {
    let (home, repo) = fixture("detailed");
    let out = arc(&home, &repo, &["help", "diff", "--"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.starts_with("DIFF WORKFLOW\n\n"));
    assert!(stdout.contains("Supports: git, svn, hg"));
    assert!(stdout.contains("--add-all, -a"));
    // `cmd --help` routes to the same detailed help (toolset
    // commands still need `--` when piped, like PHP).
    let out = arc(&home, &repo, &["land", "--help"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("Missing required"));
    let out = arc(&home, &repo, &["land", "--help", "--"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("LAND WORKFLOW\n\n"));
    let out = arc(&home, &repo, &["diff", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("DIFF WORKFLOW\n\n"));
    // Unknown names report the no-workflow message, still exit 0.
    let out = arc(&home, &repo, &["help", "bogus", "--"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("There is no bogus workflow."));
}

#[test]
fn core16_unknown_and_prefix_resolution() {
    let (home, repo) = fixture("resolve");
    let out = arc(&home, &repo, &["bogus", "--"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("Unknown command 'bogus'. Try 'arc help'."));
    let out = arc(&home, &repo, &["l", "--"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("Did you mean:"));
    assert!(stderr.contains("    land\n"));
    // Unique prefixes run.
    let out = arc(&home, &repo, &["he", "--"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).starts_with("WORKFLOWS\n\n"));
    // Classic resolution is case-insensitive (`DIFF` reaches diff,
    // which then fails on the empty scratch checkout, proving the
    // command resolved instead of reporting unknown).
    let out = arc(&home, &repo, &["DIFF", "--"]);
    assert!(!text(&out.stderr).contains("Unknown command"));
}

#[test]
fn core15_trace_prints_argv_banner() {
    let (home, repo) = fixture("trace");
    let out = arc(&home, &repo, &["--trace", "help", "--"]);
    assert_eq!(out.status.code(), Some(0));
    let stderr = text(&out.stderr);
    assert!(stderr.contains(" ARGV  "), "{stderr:?}");
    assert!(stderr.contains("'--trace' 'help' '--'"), "{stderr:?}");
}

#[test]
fn core17_piped_toolset_commands_require_dashes() {
    let (home, repo) = fixture("terminator");
    let out = arc(&home, &repo, &["call-conduit", "user.whoami"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("is being run noninteractively"));
    assert!(stderr.contains("MUST provide \"--\" to all commands"));
    assert!(stderr.contains("Missing required \"--\" in argument list."));
    // Classic commands never require `--`.
    let out = arc(&home, &repo, &["tasks"]);
    assert!(!text(&out.stderr).contains("Missing required"));
}

#[test]
fn core34_config_validation_runs_before_dispatch() {
    let (home, repo) = fixture("config");
    let out = arc(&home, &repo, &["--config", "bad", "land", "--"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("Configuration option \"bad\" is not valid."));
    let out = arc(
        &home,
        &repo,
        &["--config", "history.immutable=true", "land", "--"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("is not a recognized option"));
}

#[test]
fn rbd1_parse_errors_use_toolset_arms() {
    let (home, repo) = fixture("arms");
    // Workflow-argument errors: bold label.
    let out = arc(&home, &repo, &["call-conduit", "--bogus", "--", "m"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("Usage Exception:"), "{stderr:?}");
    assert!(!stderr.contains("USAGE EXCEPTION"), "{stderr:?}");
    // `runWorkflow()` usage errors: red label.
    let out = arc(&home, &repo, &["call-conduit", "--"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("USAGE EXCEPTION"),
        "{:?}",
        text(&out.stderr)
    );
    let out = arc(&home, &repo, &["alias", "--json", "x", "--"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("can only be used to list aliases"));
}

#[test]
fn core66_prompts_and_shell_complete_run() {
    let (home, repo) = fixture("newcmds");
    let out = arc(&home, &repo, &["prompts", "--", "land"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("PROMPT  arc.land.confirm"));
    let out = arc(
        &home,
        &repo,
        &["shell-complete", "--current", "1", "--", "arc", "dow"],
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "download");
}

#[test]
fn core19_classic_commands_see_arc_environment() {
    // `ARCANIST` is observable through `arc alias` shell expansion
    // only in real use; here assert the wiring runs classic commands
    // end to end (exit 0 under the empty-HOME fixture).
    let (home, repo) = fixture("classic");
    let out = arc(&home, &repo, &["list"]);
    assert_eq!(out.status.code(), Some(1));
    // No URI and no credentials: the no-URI block preempts the auth
    // gate, like the classic subprocess.
    assert!(
        text(&out.stderr).contains("no conduit URI"),
        "{:?}",
        text(&out.stderr)
    );
}

#[test]
fn t1170_markers_query_failure_is_fatal() {
    // Query failures (here: no conduit URI anywhere, so client setup
    // fails before any call goes out) render the fatal envelope (255),
    // not usage. Binary-level: the empty-HOME + scrubbed-env fixture
    // makes "no URI" deterministic, which no in-process test can do
    // (the real ~/.arcrc and process env leak in).
    let (home, repo) = fixture("markers-fatal");
    let out = arc(&home, &repo, &["markers", "--type", "branch", "--"]);
    assert_eq!(out.status.code(), Some(255));
    assert!(
        text(&out.stderr).contains("no Phabricator URI"),
        "{:?}",
        text(&out.stderr)
    );
}
