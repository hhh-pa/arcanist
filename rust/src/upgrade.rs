//! `arc upgrade` — report the running version, or self-update the binary.
//!
//! PHP reference: `src/workflow/ArcanistUpgradeWorkflow.php`.
//!
//! Without flags this reports the running version plus the enclosing
//! checkout's branch/HEAD, cleanliness, and ahead/behind counts versus
//! its upstream. `--apply` instead replaces the running executable with
//! the latest stable release asset for the current platform (see
//! [`execute_apply`]).
//!
//! Release repo: the `fork` remote's repo, else `origin`'s, else
//! [`DEFAULT_RELEASE_REPO`]. `fork` is preferred because it carries
//! this port's releases. [`resolve_apply_target`], [`run_apply`],
//! [`uncommitted_paths`], and [`render_uncommitted_block`] remain as the
//! checkout-based (`git pull`) port for library checkouts; the live
//! `--apply` path replaces the binary and does not use them.
//!
//! Redesign note (T1153 CORE-56, kept): PHP pulls the install
//! checkout (`git pull --rebase`), which is unportable — a static
//! binary has no checkout to pull. Bare `arc upgrade` reports,
//! `--apply` replaces the binary from release assets instead.

extern crate self as arc_rs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use arc_rs::download;
use arc_rs::git;
use arc_rs::prompt;
use arc_rs::terminal::{self, PhtArg, TVal};

/// Subcommand name, matching `arc upgrade`.
pub const NAME: &str = "upgrade";

const USAGE: &str = "usage: arc upgrade [--check] [--json] [--apply] [--yes]\n\
\n\
Report the running arc version and whether the enclosing git checkout\n\
is behind its upstream. With --apply, replace the running executable\n\
with the latest stable release instead of reporting (--check is\n\
ignored; --json prints the result as JSON).\n\
\n\
Options:\n\
  --check         Exit non-zero when the checkout is behind its upstream.\n\
  --json          Emit the report (or --apply result) as JSON.\n\
  --apply         Download the latest stable release and replace the\n\
                  running executable (prompts first; --yes skips).\n\
  --yes           Skip the upgrade prompt (scripting).\n\
  -h, --help      Show this help.\n";

/// The running version, from `Cargo.toml`.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Update status of one directory.
pub struct Status {
    /// Running `arc` version.
    pub version: String,
    /// Whether `dir` is inside a git working tree.
    pub in_repo: bool,
    /// Repository top level, when in a repo.
    pub root: Option<String>,
    /// Current branch short name, or `None` when detached.
    pub branch: Option<String>,
    /// HEAD short SHA, or `None` when there are no commits.
    pub head: Option<String>,
    /// Whether the working copy has no changes.
    pub clean: Option<bool>,
    /// Number of `git status --porcelain` lines.
    pub changed_files: Option<usize>,
    /// Upstream short name (e.g. `origin/main`), if configured.
    pub upstream: Option<String>,
    /// Commits ahead of the upstream.
    pub ahead: Option<u64>,
    /// Commits behind the upstream.
    pub behind: Option<u64>,
}

/// Inspect `dir` using only local git state (no fetch, no network).
pub fn inspect(dir: &Path) -> Result<Status, String> {
    let version = version().to_string();
    if !git::is_inside_work_tree(dir) {
        return Ok(Status {
            version,
            in_repo: false,
            root: None,
            branch: None,
            head: None,
            clean: None,
            changed_files: None,
            upstream: None,
            ahead: None,
            behind: None,
        });
    }
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])?;
    let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
    let head = git::stdout_text(dir, &["rev-parse", "--short", "HEAD"]).ok();
    let porcelain = git::stdout_text(dir, &["status", "--porcelain"])?;
    let changed_files = if porcelain.is_empty() {
        0
    } else {
        porcelain.lines().count()
    };
    let clean = changed_files == 0;
    let (upstream, ahead, behind) = if git::ok(dir, &["rev-parse", "--verify", "@{upstream}"]) {
        let name = git::stdout_text(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok();
        let (ahead, behind) = git::stdout_text(
            dir,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .ok()
        .and_then(|text| parse_ahead_behind(&text))
        .unzip();
        (name, ahead, behind)
    } else {
        (None, None, None)
    };
    Ok(Status {
        version,
        in_repo: true,
        root: Some(root),
        branch,
        head,
        clean: Some(clean),
        changed_files: Some(changed_files),
        upstream,
        ahead,
        behind,
    })
}

fn parse_ahead_behind(text: &str) -> Option<(u64, u64)> {
    let mut parts = text.split_whitespace();
    let ahead = parts.next()?.parse().ok()?;
    let behind = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

/// Whether an update is available: behind > 0. `None` when unknown (not
/// in a repo, or no upstream).
#[must_use]
pub fn update_available(status: &Status) -> Option<bool> {
    status.behind.map(|behind| behind > 0)
}

/// How to update arc manually.
#[must_use]
pub fn how_to_update() -> &'static str {
    "To update the checkout: run `git pull --rebase` in the arc checkout, then `cargo build` (or `cargo install --path rust`). To update the arc binary itself: run `arc upgrade --apply`. Without --apply, arc never updates itself."
}

/// Library name `--apply` upgrades, mirroring PHP's `$library`.
pub const APPLY_LIBRARY: &str = "arcanist";

/// Branches eligible for automatic upgrades.
pub const APPLY_BRANCHES: &[&str] = &["master", "stable"];

/// Target of an `--apply` upgrade: the enclosing checkout and its branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyTarget {
    pub root: PathBuf,
    pub branch: String,
}

/// One log line through the log engine (`writeStatus` blue,
/// `writeSuccess` green), with an explicit ANSI setting so tests stay
/// deterministic.
fn log_line(color: &str, label: &str, message: &str, ansi: bool) -> String {
    terminal::format_log(color, label, message, ansi)
}

/// One log line on stderr, through the log engine (`writeStatus`
/// blue, `writeSuccess` green).
fn push_status(color: &str, label: &str, message: &str) {
    eprint!(
        "{}",
        log_line(color, label, message, terminal::ansi_enabled())
    );
}

/// Collect paths from one `status --porcelain=2 -z` entry. Rename entries
/// (`2`) contribute the new path here; the old path arrives as the next
/// NUL-separated field and is collected by [`uncommitted_paths`].
fn porcelain_entry_paths(entry: &str) -> Option<&str> {
    let kind = entry.split(' ').next()?;
    match kind {
        // Limited splits keep paths with spaces intact, like PHP's
        // `explode(' ', $entry, N)`.
        "1" => entry.splitn(9, ' ').nth(8),
        "2" => entry.splitn(10, ' ').nth(9),
        "u" => entry.splitn(11, ' ').nth(10),
        "?" => entry.split_once(' ').map(|(_, rest)| rest),
        _ => None,
    }
}

/// Sorted uncommitted paths, mirroring `getUncommittedStatus()` keys
/// (`status --porcelain=2 -z`, renames contribute both sides).
pub fn uncommitted_paths(dir: &Path) -> Result<Vec<String>, String> {
    let out = git::output(dir, &["status", "--porcelain=2", "-z"])
        .map_err(|e| format!("failed to read working copy status: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.split('\0');
    let mut paths: Vec<String> = Vec::new();
    while let Some(entry) = fields.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(path) = porcelain_entry_paths(entry) {
            paths.push(path.to_string());
        }
        if entry.starts_with("2 ")
            && let Some(orig) = fields.next()
            && !orig.is_empty()
        {
            paths.push(orig.to_string());
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// One block paragraph: `tsprintf('%s', $item)->applyWrap()`
/// (escape, then wrap at indent 0).
fn wrap_paragraph(message: &str, ansi: bool) -> String {
    let text = terminal::tsprintf_ansi("%s", &[TVal::Text(message)], ansi)
        .expect("fixed tsprintf pattern");
    terminal::console_wrap(text.as_str(), 0, true)
}

/// Render the uncommitted-changes console block (stdout text, mirroring
/// `PhutilConsoleBlock` with an unwrapped `PhutilConsoleList`): wrapped
/// `pht` paragraphs around `      - ` items, escaped at draw time.
#[must_use]
pub fn render_uncommitted_block(root: &Path, paths: &[String], ansi: bool) -> String {
    let root_text = root.display().to_string();
    let first = terminal::pht(
        "You have uncommitted changes in the working copy (\"%s\") for this library (\"%s\"):",
        &[PhtArg::Text(&root_text), PhtArg::Text(APPLY_LIBRARY)],
    );
    let mut out = format!("{}\n\n", wrap_paragraph(&first, ansi));
    for path in paths {
        out.push_str(&format!("      - {}\n", terminal::escape_value(path, true)));
    }
    let last = terminal::pht("Discard these changes before running \"arc upgrade\".", &[]);
    out.push_str(&format!("\n{}\n\n", wrap_paragraph(&last, ansi)));
    out
}

/// PHP-shaped "unsupported branch" error, shared by
/// [`resolve_apply_target`] and [`resolve_release_target`].
fn unsupported_branch_message(root: &str, branch: Option<&str>) -> String {
    format!(
        "Library \"{APPLY_LIBRARY}\" (in \"{root}\") is on branch \"{}\", but this branch is \
         not supported for automatic upgrades. Supported branches are: {}.",
        branch.unwrap_or("HEAD (detached)"),
        APPLY_BRANCHES.join(", ")
    )
}

/// Resolve the `--apply` target: `dir` must be a git working copy on
/// `master` or `stable`.
pub fn resolve_apply_target(dir: &Path) -> Result<ApplyTarget, String> {
    if !git::is_inside_work_tree(dir) {
        return Err(format!(
            "The \"upgrade\" workflow uses \"git pull\" to upgrade, but the software directory \
             (in \"{}\") is not a Git working copy. You must leave this directory as a Git \
             working copy to use \"arc upgrade\".",
            dir.display()
        ));
    }
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("failed to find the checkout root: {e}"))?;
    let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
    match branch {
        Some(name) if APPLY_BRANCHES.contains(&name.as_str()) => Ok(ApplyTarget {
            root: PathBuf::from(root),
            branch: name,
        }),
        other => Err(unsupported_branch_message(&root, other.as_deref())),
    }
}

/// Reminder printed after a successful `--apply`: the running binary
/// predates the upgraded checkout, so it must be rebuilt.
#[must_use]
pub fn rebuild_note() -> &'static str {
    "Rebuild arc from the upgraded checkout before using it (e.g. `cargo build` in the checkout root)."
}

/// Run `git pull --rebase origin -- <branch>` in `root`, returning stdout
/// on success and `(stdout, stderr)` on failure.
fn pull_rebase(root: &Path, branch: &str) -> Result<String, (String, String)> {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["pull", "--rebase", "origin", "--", branch])
        .output()
        .map_err(|e| (String::new(), format!("failed to run git pull: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        Err((stdout, stderr))
    }
}

/// Best-effort `git rebase --abort` after a failed pull, mirroring PHP.
fn abort_rebase(root: &Path) {
    std::process::Command::new("git")
        .current_dir(root)
        .args(["rebase", "--abort"])
        .output()
        .ok();
}

/// Perform the `--apply` upgrade: cleanliness gate, pull, abort on
/// failure. Status lines go to stderr; pull output passes through stdout.
pub fn run_apply(target: &ApplyTarget) -> Result<(), String> {
    let dirty = uncommitted_paths(&target.root)?;
    if !dirty.is_empty() {
        print!(
            "{}",
            render_uncommitted_block(&target.root, &dirty, terminal::ansi_enabled())
        );
        return Err("\"arc upgrade\" can only upgrade clean working copies.".to_string());
    }
    push_status(
        "blue",
        &terminal::pht("UPGRADING", &[]),
        &terminal::pht(
            "Upgrading \"%s\" (on branch \"%s\").",
            &[PhtArg::Text(APPLY_LIBRARY), PhtArg::Text(&target.branch)],
        ),
    );
    match pull_rebase(&target.root, &target.branch) {
        Ok(stdout) => {
            print!("{stdout}");
            push_status(
                "green",
                &terminal::pht("UPGRADED", &[]),
                &terminal::pht("This software is now up to date.", &[]),
            );
            eprintln!("{}", rebuild_note());
            Ok(())
        }
        Err((stdout, stderr)) => {
            print!("{stdout}");
            eprint!("{stderr}");
            abort_rebase(&target.root);
            let detail = stderr.trim();
            if detail.is_empty() {
                Err("git pull --rebase failed.".to_string())
            } else {
                Err(detail.to_string())
            }
        }
    }
}

/// Render the human-readable report.
#[must_use]
pub fn render(status: &Status) -> String {
    let mut out = format!("arc {}\n\n", status.version);
    if !status.in_repo {
        out.push_str("Not inside a git checkout; newest available is unknown.\n\n");
        out.push_str(how_to_update());
        out.push('\n');
        return out;
    }
    if let Some(root) = &status.root {
        out.push_str(&format!("Repository: {root}\n"));
    }
    out.push_str(&format!(
        "Branch: {}\n",
        status.branch.as_deref().unwrap_or("HEAD (detached)")
    ));
    out.push_str(&format!(
        "HEAD: {}\n",
        status.head.as_deref().unwrap_or("none (no commits)")
    ));
    match (status.clean, status.changed_files) {
        (Some(true), _) => out.push_str("Working copy: clean\n"),
        (Some(false), Some(n)) => {
            out.push_str(&format!("Working copy: dirty ({n} changed files)\n"))
        }
        _ => out.push_str("Working copy: unknown\n"),
    }
    match (&status.upstream, status.ahead, status.behind) {
        (Some(name), Some(ahead), Some(behind)) => {
            out.push_str(&format!(
                "Upstream: {name} (ahead {ahead}, behind {behind})\n"
            ));
        }
        (Some(name), _, _) => out.push_str(&format!("Upstream: {name} (counts unknown)\n")),
        (None, _, _) => out.push_str("Upstream: none configured\n"),
    }
    match update_available(status) {
        Some(true) => out.push_str("Status: update available\n"),
        Some(false) => out.push_str("Status: up to date\n"),
        None => out.push_str("Status: newest available unknown\n"),
    }
    out.push('\n');
    out.push_str(how_to_update());
    out.push('\n');
    out
}

/// Render the machine-readable report.
#[must_use]
pub fn report_json(status: &Status) -> serde_json::Value {
    serde_json::json!({
        "version": status.version,
        "in_repo": status.in_repo,
        "root": status.root,
        "branch": status.branch,
        "head": status.head,
        "clean": status.clean,
        "changed_files": status.changed_files,
        "upstream": status.upstream,
        "ahead": status.ahead,
        "behind": status.behind,
        "update_available": update_available(status),
    })
}

/// Default release repository: the fork whose GitHub releases carry
/// the `arc` binaries. Checkouts whose `fork` (else `origin`)
/// remote points at another `github.com` repo use that repo instead.
pub const DEFAULT_RELEASE_REPO: &str = "m-hu/arcanist";

/// GitHub REST API base for release lookups. Every network seam takes
/// this as a parameter so tests can point it at a local mock server.
pub const RELEASE_API_BASE: &str = "https://api.github.com";

/// Timeout for the release-metadata request. Asset downloads scale with
/// size instead (see [`download::download_timeout`]).
const RELEASE_TIMEOUT: Duration = Duration::from_secs(30);

/// One installable release binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// Asset file name (e.g. `arc-windows-x86_64.exe`).
    pub name: String,
    /// Expected byte size, verified after download.
    pub size: u64,
    /// Public download URL (`browser_download_url`).
    pub url: String,
    /// Release checksum (`digest`, e.g. `sha256:<hex>`); `None` when the
    /// release carries none (installation is then refused).
    pub digest: Option<String>,
}

/// Where `--apply` upgrades from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTarget {
    /// `owner/repo` serving the releases.
    pub repo: String,
    /// Release channel: the checkout branch, or `"stable"` outside checkouts.
    pub channel: String,
}

/// `owner/repo` from a git remote URL when it points at `github.com`.
/// Accepts `https://`/`http://` (with optional `www.`), `ssh://` (with
/// optional `user@`), and scp-like `git@github.com:owner/repo` shapes,
/// with or without a `.git` suffix.
#[must_use]
pub fn parse_github_repo(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let bare = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    scp_github_repo(bare).or_else(|| scheme_github_repo(bare))
}

/// Strip a case-insensitive `<host>/` prefix: the path after it.
fn strip_host_path<'a>(text: &'a str, host: &str) -> Option<&'a str> {
    let head = text.get(..host.len())?;
    if !head.eq_ignore_ascii_case(host) {
        return None;
    }
    if text.as_bytes().get(host.len()) != Some(&b'/') {
        return None;
    }
    text.get(host.len() + 1..)
}

/// Scp-like `user@github.com:owner/repo` shape.
fn scp_github_repo(text: &str) -> Option<String> {
    let rest = text.split_once('@')?.1;
    let head = rest.get(.."github.com".len())?;
    if !head.eq_ignore_ascii_case("github.com") {
        return None;
    }
    if rest.as_bytes().get("github.com".len()) != Some(&b':') {
        return None;
    }
    owner_slash_repo(rest.get("github.com".len() + 1..)?)
}

/// `https://`/`http://`/`ssh://` shapes, with optional `user@`/`www.`.
fn scheme_github_repo(text: &str) -> Option<String> {
    let rest = scheme_rest(text)?;
    let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    owner_slash_repo(strip_host_path(rest, "github.com")?)
}

/// The path after a supported URL scheme (matched case-insensitively).
fn scheme_rest(text: &str) -> Option<&str> {
    for scheme in ["https://", "http://", "ssh://"] {
        let Some(head) = text.get(..scheme.len()) else {
            continue;
        };
        if head.eq_ignore_ascii_case(scheme) {
            return text.get(scheme.len()..);
        }
    }
    None
}

/// Exactly `owner/repo`, with neither side empty.
fn owner_slash_repo(path: &str) -> Option<String> {
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Release repo for `dir`: the `fork` remote's repo, else `origin`'s,
/// else [`DEFAULT_RELEASE_REPO`]. Non-`github.com` remotes are skipped:
/// only GitHub hosts this port's releases.
#[must_use]
pub fn release_repo_for(dir: &Path) -> String {
    for remote in ["fork", "origin"] {
        if let Ok(url) = git::stdout_text(dir, &["remote", "get-url", remote])
            && let Some(repo) = parse_github_repo(&url)
        {
            return repo;
        }
    }
    DEFAULT_RELEASE_REPO.to_string()
}

/// Resolve the `--apply` release target: outside checkouts the default
/// repo on the `"stable"` channel; inside checkouts the `fork`/`origin`
/// repo on the checkout branch, which must be supported (PHP's branch
/// rule: a dev binary built from a feature branch is never clobbered
/// by a stable release).
pub fn resolve_release_target(dir: &Path) -> Result<ReleaseTarget, String> {
    if !git::is_inside_work_tree(dir) {
        return Ok(ReleaseTarget {
            repo: DEFAULT_RELEASE_REPO.to_string(),
            channel: "stable".to_string(),
        });
    }
    let root = git::stdout_text(dir, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("failed to find the checkout root: {e}"))?;
    let branch = git::stdout_text(dir, &["symbolic-ref", "--short", "HEAD"]).ok();
    match branch {
        Some(name) if APPLY_BRANCHES.contains(&name.as_str()) => Ok(ReleaseTarget {
            repo: release_repo_for(dir),
            channel: name,
        }),
        other => Err(unsupported_branch_message(&root, other.as_deref())),
    }
}

/// Current platform tag (`os-arch`, e.g. `windows-x86_64`), from
/// `std::env::consts`. Release binaries are published as
/// `arc-<tag>` (plus `.exe` on Windows).
#[must_use]
pub fn platform_tag() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Exact asset names for `platform`, in preference order.
fn platform_asset_names(platform: &str) -> [String; 2] {
    [format!("arc-{platform}.exe"), format!("arc-{platform}")]
}

/// `releases/latest` API URL for `repo` under `api_base` (the stable
/// channel: GitHub's latest non-prerelease, non-draft release).
#[must_use]
pub fn latest_release_url(api_base: &str, repo: &str) -> String {
    format!(
        "{}/repos/{repo}/releases/latest",
        api_base.trim_end_matches('/')
    )
}

/// Fetch the latest stable release document over plain HTTPS (or from a
/// mock `api_base` under test). A UA header is sent (the API rejects
/// anonymous clients without one); no auth token.
pub fn fetch_latest_release(api_base: &str, repo: &str) -> Result<serde_json::Value, String> {
    let http = reqwest::blocking::Client::builder()
        .timeout(RELEASE_TIMEOUT)
        .no_proxy()
        .user_agent(format!("arc-rs-upgrade/{}", version()))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let response = http
        .get(latest_release_url(api_base, repo))
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("failed to check for updates: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(release_http_error(repo, status.as_u16()));
    }
    let body = response
        .text()
        .map_err(|e| format!("failed to check for updates: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("failed to parse release metadata: {e}"))
}

fn release_http_error(repo: &str, status: u16) -> String {
    if status == 404 {
        format!("failed to check for updates: HTTP 404 (no published releases for \"{repo}\"?).")
    } else {
        format!("failed to check for updates: HTTP {status}.")
    }
}

/// The release tag (`tag_name`, e.g. `v0.1.1`).
pub fn release_tag(release: &serde_json::Value) -> Result<String, String> {
    release
        .get("tag_name")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| "release metadata has no \"tag_name\".".to_string())
}

/// Pick the current-platform binary from a release document: an exact
/// `arc-<tag>[.exe]` name wins, else a lone `arc-*` asset;
/// anything else is a clean error (never a guess between candidates).
pub fn select_asset(release: &serde_json::Value, platform: &str) -> Result<ReleaseAsset, String> {
    let assets = release
        .get("assets")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "release metadata has no \"assets\" list.".to_string())?;
    asset_for_platform(&parse_assets(assets), platform)
}

/// Installable candidates: assets with a name, a URL, and a nonzero
/// size (sidecar checksum files and malformed entries never qualify).
/// The checksum rides along for the install-time gate; selection itself
/// is by name only.
fn parse_assets(raw: &[serde_json::Value]) -> Vec<ReleaseAsset> {
    let mut out = Vec::new();
    for asset in raw {
        let name = asset
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let url = asset
            .get("browser_download_url")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let size = asset
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if name.is_empty() || url.is_empty() || size == 0 {
            continue;
        }
        let digest = asset
            .get("digest")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        out.push(ReleaseAsset {
            name: name.to_string(),
            size,
            url: url.to_string(),
            digest,
        });
    }
    out
}

fn asset_for_platform(assets: &[ReleaseAsset], platform: &str) -> Result<ReleaseAsset, String> {
    let names = platform_asset_names(platform);
    if let Some(exact) = assets.iter().find(|asset| names.contains(&asset.name)) {
        return Ok(exact.clone());
    }
    let candidates: Vec<&ReleaseAsset> = assets
        .iter()
        .filter(|asset| asset.name.starts_with("arc-"))
        .collect();
    match candidates.as_slice() {
        [only] => Ok((*only).clone()),
        [] => Err(format!(
            "the latest release has no installable asset for \"{platform}\"."
        )),
        _ => Err(ambiguous_asset_error(platform, &candidates)),
    }
}

fn ambiguous_asset_error(platform: &str, candidates: &[&ReleaseAsset]) -> String {
    let names: Vec<&str> = candidates.iter().map(|asset| asset.name.as_str()).collect();
    format!(
        "the latest release has no exact asset for \"{platform}\" (candidates: {}).",
        names.join(", ")
    )
}

/// Numeric core plus prerelease suffix of a version tag (one leading
/// `v` stripped): `v1.2.3-rc1` becomes `([1, 2, 3], Some("rc1"))`.
fn split_version(tag: &str) -> (Vec<u64>, Option<&str>) {
    let bare = strip_v_prefix(tag);
    let (core, suffix) = match bare.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (bare, None),
    };
    (version_core(core), suffix)
}

/// Strip one leading `v`/`V` when a digit follows it.
fn strip_v_prefix(tag: &str) -> &str {
    tag.strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .filter(|rest| {
            rest.as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_digit())
        })
        .unwrap_or(tag)
}

/// Numeric components of a dotted core (`1.2` becomes `[1, 2]`;
/// non-numeric parts become 0).
fn version_core(core: &str) -> Vec<u64> {
    core.split('.').map(component_value).collect()
}

/// Leading digits of one component, saturating on overflow.
fn component_value(part: &str) -> u64 {
    let digits: String = part
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .map(|byte| byte as char)
        .collect();
    if digits.is_empty() {
        0
    } else {
        digits.parse().unwrap_or(u64::MAX)
    }
}

/// Component-wise numeric comparison (missing components are 0).
fn compare_cores(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let len = left.len().max(right.len());
    for index in 0..len {
        let pair = (
            left.get(index).copied().unwrap_or(0),
            right.get(index).copied().unwrap_or(0),
        );
        if pair.0 != pair.1 {
            return pair.0.cmp(&pair.1);
        }
    }
    std::cmp::Ordering::Equal
}

/// True when release `latest` is newer than `current`: numeric cores
/// compared component-wise; on equal cores a bare release beats its
/// prereleases, and prereleases compare lexically.
#[must_use]
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (latest_core, latest_suffix) = split_version(latest);
    let (current_core, current_suffix) = split_version(current);
    match compare_cores(&latest_core, &current_core) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => newer_suffix(latest_suffix, current_suffix),
    }
}

fn newer_suffix(latest: Option<&str>, current: Option<&str>) -> bool {
    match (latest, current) {
        (None, Some(_)) => true,
        (Some(left), Some(right)) => left > right,
        _ => false,
    }
}

/// Download the release binary over plain HTTPS (size-scaled timeout)
/// and verify its size and SHA-256 checksum; any failure leaves the
/// executable untouched.
pub fn download_release_bytes(
    url: &str,
    expected_size: u64,
    digest: Option<&str>,
) -> Result<Vec<u8>, String> {
    let want_hex = pre_download_checks(expected_size, digest)?;
    let bytes =
        download::download_bytes_with_timeout(url, download::download_timeout(expected_size))?;
    verify_download_size(bytes.len() as u64, expected_size)?;
    verify_sha256_hex(&bytes, &want_hex)?;
    Ok(bytes)
}

/// Pre-download gate: refuse empty assets and releases without a usable
/// SHA-256 checksum, before any byte is fetched.
fn pre_download_checks(expected_size: u64, digest: Option<&str>) -> Result<String, String> {
    if expected_size == 0 {
        return Err("refusing to install an empty release asset (expected size 0).".to_string());
    }
    require_sha256_hex(digest)
}

/// The expected SHA-256 hex from a release `digest` value
/// (`sha256:<hex>`); fail-closed on missing, foreign-algorithm, or
/// malformed checksums.
fn require_sha256_hex(digest: Option<&str>) -> Result<String, String> {
    let Some(value) = digest.map(str::trim).filter(|text| !text.is_empty()) else {
        return Err("refusing to install a release asset without a SHA-256 checksum.".to_string());
    };
    let Some(hex) = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("SHA256:"))
    else {
        return Err(format!(
            "refusing to install a release asset with an unsupported checksum ({value:?}); want \"sha256:<hex>\"."
        ));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(
            "refusing to install a release asset with a malformed SHA-256 checksum.".to_string(),
        );
    }
    Ok(hex.to_ascii_lowercase())
}

/// Fail when the downloaded bytes do not match the release checksum.
fn verify_sha256_hex(bytes: &[u8], want_hex: &str) -> Result<(), String> {
    let got_hex = sha256_hex(bytes);
    if got_hex != want_hex {
        return Err(format!(
            "Downloaded file checksum ({got_hex}) does not match the release checksum ({want_hex}). \
             This download may be incomplete or corrupt."
        ));
    }
    Ok(())
}

/// Lowercase SHA-256 hex of `data` (FIPS 180-4, dependency-free).
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in sha256_digest(data) {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 15)] as char);
    }
    text
}

fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6a09_e667u32,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut input = data.to_vec();
    input.push(0x80);
    while input.len() % 64 != 56 {
        input.push(0);
    }
    input.extend_from_slice(&bit_len.to_be_bytes());
    for block in input.as_chunks::<64>().0 {
        sha256_compress(&mut state, block);
    }
    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn sha256_compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let schedule = sha256_schedule(block);
    let mut work = *state;
    for (index, round) in SHA256_K.iter().enumerate() {
        sha256_round(&mut work, schedule[index], *round);
    }
    for (slot, word) in state.iter_mut().zip(work) {
        *slot = slot.wrapping_add(word);
    }
}

fn sha256_schedule(block: &[u8; 64]) -> [u32; 64] {
    let mut schedule = [0u32; 64];
    for (index, word) in schedule.iter_mut().take(16).enumerate() {
        let start = index * 4;
        *word = u32::from_be_bytes([
            block[start],
            block[start + 1],
            block[start + 2],
            block[start + 3],
        ]);
    }
    for index in 16..64 {
        let low = schedule[index - 15];
        let high = schedule[index - 2];
        let small0 = low.rotate_right(7) ^ low.rotate_right(18) ^ (low >> 3);
        let small1 = high.rotate_right(17) ^ high.rotate_right(19) ^ (high >> 10);
        schedule[index] = schedule[index - 16]
            .wrapping_add(small0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(small1);
    }
    schedule
}

fn sha256_round(work: &mut [u32; 8], schedule_word: u32, round: u32) {
    let sum1 = work[4].rotate_right(6) ^ work[4].rotate_right(11) ^ work[4].rotate_right(25);
    let choice = (work[4] & work[5]) ^ ((!work[4]) & work[6]);
    let temp1 = work[7]
        .wrapping_add(sum1)
        .wrapping_add(choice)
        .wrapping_add(round)
        .wrapping_add(schedule_word);
    let sum0 = work[0].rotate_right(2) ^ work[0].rotate_right(13) ^ work[0].rotate_right(22);
    let majority = (work[0] & work[1]) ^ (work[0] & work[2]) ^ (work[1] & work[2]);
    let temp2 = sum0.wrapping_add(majority);
    work[7] = work[6];
    work[6] = work[5];
    work[5] = work[4];
    work[4] = work[3].wrapping_add(temp1);
    work[3] = work[2];
    work[2] = work[1];
    work[1] = work[0];
    work[0] = temp1.wrapping_add(temp2);
}

#[rustfmt::skip]
const SHA256_K: [u32; 64] = [
    0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5,
    0x3956_c25b, 0x59f1_11f1, 0x923f_82a4, 0xab1c_5ed5,
    0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3,
    0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174,
    0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc,
    0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
    0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7,
    0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967,
    0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13,
    0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85,
    0xa2bf_e8a1, 0xa81a_664b, 0xc24b_8b70, 0xc76c_51a3,
    0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
    0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5,
    0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
    0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208,
    0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
];

fn verify_download_size(got: u64, expected: u64) -> Result<(), String> {
    if expected == 0 {
        return Err("refusing to install an empty release asset (expected size 0).".to_string());
    }
    if got != expected {
        return Err(format!(
            "Downloaded file size ({} bytes) does not match expected file size ({} bytes). \
             This download may be incomplete or corrupt.",
            download::format_count(got),
            download::format_count(expected)
        ));
    }
    Ok(())
}

/// Refuse unless `exe` can be replaced: an existing executable must not
/// be read-only, and its directory must accept new files (probed with a
/// unique temp file, removed again). No changes persist.
pub fn ensure_executable_writable(exe: &Path) -> Result<(), String> {
    if exe.exists()
        && let Ok(meta) = std::fs::metadata(exe)
        && meta.permissions().readonly()
    {
        return Err(not_writable_error(exe));
    }
    probe_dir_writable(exe)
}

fn not_writable_error(exe: &Path) -> String {
    format!(
        "The executable at \"{}\" is not writable; refusing to upgrade in place.",
        exe.display()
    )
}

/// The executable's directory (`.` when it has none).
fn parent_or_dot(exe: &Path) -> &Path {
    exe.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Create and remove a unique probe file in the executable's directory.
fn probe_dir_writable(exe: &Path) -> Result<(), String> {
    let probe = parent_or_dot(exe).join(format!(
        ".arc-rs-upgrade-writable-{}-{}",
        std::process::id(),
        probe_counter()
    ));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            std::fs::remove_file(&probe).ok();
            Ok(())
        }
        Err(_) => Err(not_writable_error(exe)),
    }
}

fn probe_counter() -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// Atomically replace `exe` with `bytes`: stage a temp file in the same
/// directory, then rename over (unix) or rename-replace via `<exe>.old`
/// (Windows, where the running image is locked). Returns the kept
/// previous executable, if any. Any failure removes the staged file and
/// leaves `exe` untouched.
pub fn install_bytes_at(exe: &Path, bytes: &[u8]) -> Result<Option<PathBuf>, String> {
    ensure_executable_writable(exe)?;
    let tmp = sibling_temp(exe);
    std::fs::remove_file(&tmp).ok();
    if let Err(error) = std::fs::write(&tmp, bytes) {
        std::fs::remove_file(&tmp).ok();
        return Err(format!("failed to stage the new executable: {error}"));
    }
    if let Err(message) = make_executable(&tmp) {
        std::fs::remove_file(&tmp).ok();
        return Err(message);
    }
    replace_with_tmp(exe, &tmp)
}

/// Staging path next to the executable (same filesystem, so the final
/// rename is atomic): `<exe>.arc-new`.
fn sibling_temp(exe: &Path) -> PathBuf {
    let file = exe
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "arc".to_string());
    parent_or_dot(exe).join(format!("{file}.arc-new"))
}

/// `chmod 755` the staged binary on unix; a no-op on Windows.
fn make_executable(tmp: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed to make the staged executable runnable: {e}"))
    }
    #[cfg(not(unix))]
    {
        let _ = tmp;
        Ok(())
    }
}

fn replace_with_tmp(exe: &Path, tmp: &Path) -> Result<Option<PathBuf>, String> {
    #[cfg(windows)]
    {
        replace_windows(exe, tmp)
    }
    #[cfg(not(windows))]
    {
        replace_unix(exe, tmp)
    }
}

/// Unix replacement: rename over the running image (allowed on unix).
#[cfg(not(windows))]
fn replace_unix(exe: &Path, tmp: &Path) -> Result<Option<PathBuf>, String> {
    if let Err(error) = std::fs::rename(tmp, exe) {
        std::fs::remove_file(tmp).ok();
        return Err(format!("failed to replace \"{}\": {error}", exe.display()));
    }
    Ok(None)
}

/// Windows replacement: move the live image aside (it stays mapped for
/// the running process), then rename the staged file into place. A
/// failed second rename restores the old image best-effort.
#[cfg(windows)]
fn replace_windows(exe: &Path, tmp: &Path) -> Result<Option<PathBuf>, String> {
    let old = windows_backup(exe);
    std::fs::remove_file(&old).ok();
    let backup = move_aside(exe, &old, tmp)?;
    if let Err(error) = std::fs::rename(tmp, exe) {
        if backup.is_some() {
            std::fs::rename(&old, exe).ok();
        }
        std::fs::remove_file(tmp).ok();
        return Err(format!("failed to replace \"{}\": {error}", exe.display()));
    }
    Ok(backup)
}

/// Previous-executable backup: `<exe>.old`.
#[cfg(windows)]
fn windows_backup(exe: &Path) -> PathBuf {
    PathBuf::from(format!("{}.old", exe.display()))
}

/// Move the live `exe` aside to `old`; `None` when there is no live
/// executable yet (fresh install over a missing path).
#[cfg(windows)]
fn move_aside(exe: &Path, old: &Path, tmp: &Path) -> Result<Option<PathBuf>, String> {
    if !exe.exists() {
        return Ok(None);
    }
    if let Err(error) = std::fs::rename(exe, old) {
        std::fs::remove_file(tmp).ok();
        return Err(format!("failed to replace \"{}\": {error}", exe.display()));
    }
    Ok(Some(old.to_path_buf()))
}

/// The pre-upgrade confirmation query (PHP's `phutil_console_confirm`
/// shape, asked via [`prompt::console_confirm`]).
#[must_use]
pub fn upgrade_prompt(current: &str, latest: &str) -> String {
    terminal::pht(
        "Upgrade \"%s\" from \"%s\" to \"%s\"?",
        &[
            PhtArg::Text(APPLY_LIBRARY),
            PhtArg::Text(current),
            PhtArg::Text(latest),
        ],
    )
}

/// Inputs for [`execute_apply`]; every side effect is injected so tests
/// never touch the real executable or the network.
#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// API base (production: [`RELEASE_API_BASE`]).
    pub api_base: String,
    /// Running version (production: [`version`]).
    pub current_version: String,
    /// Executable to replace (`None` resolves the running one).
    pub exe: Option<PathBuf>,
    /// Skip the confirmation prompt.
    pub yes: bool,
    /// Machine-readable result (implies no prompting without `--yes`).
    pub json: bool,
    /// ANSI colors in the transcript (production: TTY-gated).
    pub ansi: bool,
}

/// Outcome of [`execute_apply`]: the release, the replaced path, and
/// the human transcript (log lines on `stderr_text`, details on
/// `stdout_text`; `--json` replaces the latter).
#[derive(Debug, Clone)]
pub struct ApplyReport {
    /// `owner/repo` the release came from.
    pub repo: String,
    /// Release channel (checkout branch, or `"stable"`).
    pub channel: String,
    /// Running version at upgrade time.
    pub current: String,
    /// Latest release tag.
    pub latest: String,
    /// Whether the release was newer than the running version.
    pub update_available: bool,
    /// Whether the executable was replaced.
    pub updated: bool,
    /// Installed asset (`None` when already up to date).
    pub asset: Option<ReleaseAsset>,
    /// Replaced (or already current) executable path.
    pub path: PathBuf,
    /// Kept previous executable (Windows only).
    pub backup: Option<PathBuf>,
    /// Human detail lines (stdout).
    pub stdout_text: String,
    /// Log-engine lines (stderr).
    pub stderr_text: String,
}

/// Machine-readable `--apply` result.
#[must_use]
pub fn apply_result_json(report: &ApplyReport) -> serde_json::Value {
    serde_json::json!({
        "version": report.current,
        "repo": report.repo,
        "channel": report.channel,
        "latest": report.latest,
        "update_available": report.update_available,
        "updated": report.updated,
        "asset": report.asset.as_ref().map(|asset| serde_json::json!({
            "name": asset.name,
            "size": asset.size,
            "url": asset.url,
            "digest": asset.digest,
        })),
        "path": report.path.display().to_string(),
        "backup": report.backup.as_ref().map(|path| path.display().to_string()),
    })
}

/// One stdout detail line through `tsprintf` (escaped, ANSI-gated).
fn detail_line(pattern: &str, args: &[TVal], ansi: bool) -> String {
    terminal::tsprintf_ansi(pattern, args, ansi)
        .map(|line| line.to_string())
        .unwrap_or_default()
}

fn downloading_line(name: &str, size: u64, ansi: bool) -> String {
    detail_line(
        "Downloading \"%s\" (%s byte(s))...\n",
        &[TVal::Text(name), TVal::Owned(download::format_count(size))],
        ansi,
    )
}

fn replaced_line(exe: &Path, latest: &str, backup: Option<&Path>, ansi: bool) -> String {
    let exe_text = exe.display().to_string();
    let mut out = detail_line(
        "Replaced \"%s\" with arc \"%s\".\n",
        &[TVal::Text(&exe_text), TVal::Text(latest)],
        ansi,
    );
    if let Some(old) = backup {
        let old_text = old.display().to_string();
        out.push_str(&detail_line(
            "Previous executable kept as \"%s\" (removed on the next upgrade).\n",
            &[TVal::Text(&old_text)],
            ansi,
        ));
    }
    out
}

fn already_line(current: &str, latest: &str, ansi: bool) -> String {
    detail_line(
        "arc \"%s\" is already up to date (latest release: \"%s\").\n",
        &[TVal::Text(current), TVal::Text(latest)],
        ansi,
    )
}

/// `UPGRADING` log line (PHP's message; the channel plays the branch role).
fn upgrading_line(channel: &str, ansi: bool) -> String {
    log_line(
        "blue",
        &terminal::pht("UPGRADING", &[]),
        &terminal::pht(
            "Upgrading \"%s\" (on branch \"%s\").",
            &[PhtArg::Text(APPLY_LIBRARY), PhtArg::Text(channel)],
        ),
        ansi,
    )
}

/// `UPGRADED` log line (PHP's message, verbatim).
fn upgraded_line(ansi: bool) -> String {
    log_line(
        "green",
        &terminal::pht("UPGRADED", &[]),
        &terminal::pht("This software is now up to date.", &[]),
        ansi,
    )
}

/// Run the `--apply` self-update against `dir` (which only selects the
/// release channel; the checkout itself is never modified): resolve the
/// channel, fetch the latest release, confirm, download, verify, and
/// atomically replace the executable. `confirm` asks the pre-upgrade
/// question. Returns the transcript for `run` to emit; prints nothing.
pub fn execute_apply(
    dir: &Path,
    opts: &ApplyOptions,
    confirm: &dyn Fn(&str) -> Result<bool, String>,
) -> Result<ApplyReport, String> {
    let target = resolve_release_target(dir)?;
    let release = fetch_latest_release(&opts.api_base, &target.repo)?;
    let latest = release_tag(&release)?;
    let exe = resolve_exe_path(opts)?;
    if !is_newer(&latest, &opts.current_version) {
        return Ok(already_report(&target, &latest, opts, &exe));
    }
    ensure_executable_writable(&exe)?;
    let asset = select_asset(&release, &platform_tag())?;
    confirm_upgrade(opts, confirm, &latest)?;
    download_install_report(&target, &latest, opts, &exe, &asset)
}

/// The executable to replace: the injected path, else the running one.
fn resolve_exe_path(opts: &ApplyOptions) -> Result<PathBuf, String> {
    match &opts.exe {
        Some(path) => Ok(path.clone()),
        None => std::env::current_exe()
            .map_err(|e| format!("failed to locate the running executable: {e}")),
    }
}

/// Up-to-date outcome: no changes, report only.
fn already_report(
    target: &ReleaseTarget,
    latest: &str,
    opts: &ApplyOptions,
    exe: &Path,
) -> ApplyReport {
    ApplyReport {
        repo: target.repo.clone(),
        channel: target.channel.clone(),
        current: opts.current_version.clone(),
        latest: latest.to_string(),
        update_available: false,
        updated: false,
        asset: None,
        path: exe.to_path_buf(),
        backup: None,
        stdout_text: already_line(&opts.current_version, latest, opts.ansi),
        stderr_text: String::new(),
    }
}

/// Confirm the upgrade: `--yes` skips; `--json` without `--yes` refuses
/// (prompting would corrupt the machine-readable output); declining
/// aborts with no changes.
fn confirm_upgrade(
    opts: &ApplyOptions,
    confirm: &dyn Fn(&str) -> Result<bool, String>,
    latest: &str,
) -> Result<(), String> {
    if opts.yes {
        return Ok(());
    }
    if opts.json {
        return Err(
            "refusing to upgrade without confirmation in --json mode; pass --yes to upgrade non-interactively."
                .to_string(),
        );
    }
    if confirm(&upgrade_prompt(&opts.current_version, latest))? {
        Ok(())
    } else {
        Err("upgrade declined; leaving the current executable in place.".to_string())
    }
}

/// Download, verify, install, and report the upgrade.
fn download_install_report(
    target: &ReleaseTarget,
    latest: &str,
    opts: &ApplyOptions,
    exe: &Path,
    asset: &ReleaseAsset,
) -> Result<ApplyReport, String> {
    let upgrading = upgrading_line(&target.channel, opts.ansi);
    let bytes = download_release_bytes(&asset.url, asset.size, asset.digest.as_deref())?;
    let backup = install_bytes_at(exe, &bytes)?;
    let mut stdout_text = downloading_line(&asset.name, asset.size, opts.ansi);
    stdout_text.push_str(&replaced_line(exe, latest, backup.as_deref(), opts.ansi));
    let mut stderr_text = upgrading;
    stderr_text.push_str(&upgraded_line(opts.ansi));
    Ok(ApplyReport {
        repo: target.repo.clone(),
        channel: target.channel.clone(),
        current: opts.current_version.clone(),
        latest: latest.to_string(),
        update_available: true,
        updated: true,
        asset: Some(asset.clone()),
        path: exe.to_path_buf(),
        backup,
        stdout_text,
        stderr_text,
    })
}

/// Ask the pre-upgrade question on stdin (`[y/N]`: the default keeps
/// the current executable, since replacement is irreversible).
fn confirm_interactive(query: &str) -> Result<bool, String> {
    prompt::console_confirm(&mut std::io::stdin().lock(), query, true)
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    check: bool,
    json: bool,
    apply: bool,
    yes: bool,
}

enum Parsed {
    Help,
    Options(Options),
    Error(String),
}

fn parse_args(argv: &[String]) -> Parsed {
    let mut check = false;
    let mut json = false;
    let mut apply = false;
    let mut yes = false;
    for arg in argv {
        if arg == "--check" {
            check = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--apply" {
            apply = true;
        } else if arg == "--yes" {
            yes = true;
        } else if arg == "-h" || arg == "--help" {
            return Parsed::Help;
        } else if arg.starts_with('-') {
            return Parsed::Error(format!("unknown flag '{arg}'"));
        } else {
            return Parsed::Error(format!("unexpected argument '{arg}'"));
        }
    }
    Parsed::Options(Options {
        check,
        json,
        apply,
        yes,
    })
}

/// `PREPARING` log line (PHP's message, verbatim).
fn preparing_line(ansi: bool) -> String {
    log_line(
        "blue",
        &terminal::pht("PREPARING", &[]),
        &terminal::pht(
            "Preparing to upgrade \"%s\"...",
            &[PhtArg::Text(APPLY_LIBRARY)],
        ),
        ansi,
    )
}

/// Run the `--apply` self-update (see [`execute_apply`]): human
/// transcript, or the JSON result with `--json`.
fn run_apply_command(cwd: &Path, options: &Options) -> ExitCode {
    eprint!("{}", preparing_line(terminal::ansi_enabled()));
    let apply = ApplyOptions {
        api_base: RELEASE_API_BASE.to_string(),
        current_version: version().to_string(),
        exe: None,
        yes: options.yes,
        json: options.json,
        ansi: terminal::ansi_enabled(),
    };
    match execute_apply(cwd, &apply, &confirm_interactive) {
        Ok(report) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&apply_result_json(&report))
                        .unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                print!("{}", report.stdout_text);
            }
            eprint!("{}", report.stderr_text);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Entry point: `argv` excludes the subcommand name. `--apply` is live
/// (network fetch, then replaces the running executable): keep it out
/// of integration tests; `upgrade_tests` covers it hermetically.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        Parsed::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Parsed::Error(message) => {
            eprintln!("arc {NAME}: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
        Parsed::Options(options) => options,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if options.apply {
        return run_apply_command(&cwd, &options);
    }
    let status = match inspect(&cwd) {
        Ok(status) => status,
        Err(message) => {
            eprintln!("arc {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&status))
                .unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        print!("{}", render(&status));
    }
    if options.check && update_available(&status) == Some(true) {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod upgrade_tests;
