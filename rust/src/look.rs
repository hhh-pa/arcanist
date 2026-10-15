//! `arc-rs look`: a small text adventure around the working copy.
//!
//! PHP reference: `src/workflow/ArcanistLookWorkflow.php` ("Arcventure").
//! With no arguments it describes a clearing (with time-of-day flavor),
//! `look remotes` lists git remotes through ref views, and `look published`
//! lists published commits. Paragraphs render through the `%W` / `%!` /
//! `%s` `tsprintf` conversions; remotes resolve repository display names
//! via `diffusion.repository.search`, and published commits keep only
//! permanent refs (`ArcanistRepositoryRef::isPermanentRef`).
//!
//! Known limits: the hour is UTC (the PHP uses local time via `date('G')`,
//! which needs platform timezone data); `regexp(...)` permanent-ref rules
//! never match (no regex engine); without a usable Conduit client the
//! command degrades to local-only data (repository names render as `-` and
//! published lists every remote-tracking tip) instead of failing.
//!
//! Two renderings exist: the plain-text builders (`render`,
//! `render_clearing`, `render_unknown`, `render_remote`, `render_remotes`,
//! `render_published`) and the console builders used by `run`
//! (`render_title`, `render_paragraph`, `render_markup`, `render_line`,
//! `render_quiet_river`, `render_bubbles_tail`, `render_remote_view`),
//! which use the `%W` / `%!` / `%s` conversions plus ref views.

use std::path::Path;
use std::process::ExitCode;

use crate::{conduit, config, diff_submit, git, terminal};

/// Subcommand name, matching `arc look`.
pub const NAME: &str = "look";

const USAGE: &str = "usage: arc-rs look [-- THING...]\n";

/// `pht('Arcventure')`, the `%!` banner title.
pub const TITLE: &str = "Arcventure";

/// Parse `argv` (excluding the subcommand name) into the thing words.
///
/// Returns `Ok(None)` when `--help` was requested.
pub fn parse_args(argv: &[String]) -> Result<Option<Vec<String>>, String> {
    let mut thing = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            thing.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--help" || arg == "-h" {
            return Ok(None);
        } else if arg.starts_with('-') {
            return Err(format!("unknown flag '{arg}'"));
        } else {
            thing.push(arg.clone());
        }
    }
    Ok(Some(thing))
}

/// Hour of day (0-23) for a unix timestamp, in UTC.
#[must_use]
pub fn hour_of(now_unix: u64) -> u32 {
    ((now_unix / 3600) % 24) as u32
}

/// Approximate lunar phase for flavor text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LunarPhase {
    New,
    Full,
    Waxing,
    Waning,
}

/// `PhutilLunarPhase`: a new moon (Aug 11, 1999).
const LUNAR_NEW_MOON_EPOCH: f64 = 934_354_800.0;
/// `PhutilLunarPhase`: seconds in one lunar phase.
const LUNAR_PHASE_LENGTH: f64 = 2_551_442.8768992;

/// Lunar cycle fraction for `epoch`, mirroring `PhutilLunarPhase`.
#[must_use]
pub fn lunar_fraction(epoch: i64) -> f64 {
    (epoch as f64 - LUNAR_NEW_MOON_EPOCH) % LUNAR_PHASE_LENGTH / LUNAR_PHASE_LENGTH
}

/// Approximate the lunar phase, checking new/full/waxing/waning in the
/// same order as the PHP workflow.
#[must_use]
pub fn lunar_phase(epoch: i64) -> LunarPhase {
    let phase = lunar_fraction(epoch);
    if phase < 0.02 || phase > 0.98 {
        LunarPhase::New
    } else if (phase - 0.5).abs() < 0.02 {
        LunarPhase::Full
    } else if phase < 0.5 {
        LunarPhase::Waxing
    } else {
        LunarPhase::Waning
    }
}

/// Time-of-day description for `hour`, mirroring the PHP ranges.
#[must_use]
pub fn hour_text(hour: u32, phase: LunarPhase) -> &'static str {
    if (5..=7).contains(&hour) {
        "It is early morning. Glimses of sunlight peek through the trees and you hear the faint sound of birds overhead."
    } else if (8..=10).contains(&hour) {
        "It is morning. The sun is high in the sky to the east and you hear birds all around you. A gentle breeze rustles the leaves overhead."
    } else if (11..=13).contains(&hour) {
        "It is midday. The sun is high overhead and the air is still. It is very warm. You hear the cry of a hawk high overhead and far in the distance."
    } else if (14..=16).contains(&hour) {
        "It is afternoon. The air has changed and it feels as though it may rain. You hear a squirrel chittering high overhead."
    } else if (17..=19).contains(&hour) {
        "It is nearly dusk. The wind has picked up and the trees around you sway and rustle."
    } else if (21..=23).contains(&hour) {
        "It is late in the evening. The air is cool and still, and filled with the sound of crickets."
    } else {
        match phase {
            LunarPhase::New => {
                "Night has fallen, and the thin sliver of moon overhead offers no comfort. It is almost pitch black. The night is bitter cold. It will be difficult to look around in these conditions."
            }
            LunarPhase::Full => {
                "Night has fallen, but your surroundings are illuminated by the silvery glow of a full moon overhead. The night is cool and the air is crisp. The trees are calm."
            }
            LunarPhase::Waxing => {
                "Night has fallen. The moon overhead is waxing, and provides just enough light that you can make out your surroundings. It is quite cold."
            }
            LunarPhase::Waning => {
                "Night has fallen. The moon overhead is waning. You can barely make out your surroundings. It is very cold."
            }
        }
    }
}

/// `pht('You stand in the middle of a small clearing in the woods.')`.
#[must_use]
pub fn clearing_text() -> &'static str {
    "You stand in the middle of a small clearing in the woods."
}

/// `pht('Several small trails and footpaths cross here, ...')`.
#[must_use]
pub fn trails_text() -> &'static str {
    "Several small trails and footpaths cross here, twisting away from you among the trees."
}

/// `pht("Just ahead to the north, you can see **remotes**.\n")`.
#[must_use]
pub fn north_markup() -> &'static str {
    "Just ahead to the north, you can see **remotes**.\n"
}

/// `pht('You do not see "%s" anywhere.', ...)` with the thing joined.
#[must_use]
pub fn unknown_message(thing: &[String]) -> String {
    format!("You do not see \"{}\" anywhere.", thing.join(" "))
}

/// `pht('You follow a wide, straight path to the north, ...')`.
#[must_use]
pub fn grove_text() -> &'static str {
    "You follow a wide, straight path to the north and arrive in a grove of fruit trees after a few minutes of walking. The grass underfoot is thick and small insects flit through the air."
}

/// `pht('At the far edge of the grove, you see remotes:')`.
#[must_use]
pub fn grove_remotes_text() -> &'static str {
    "At the far edge of the grove, you see remotes:"
}

/// `pht("Across the grove, a stream flows north toward **published** commits.\n")`.
#[must_use]
pub fn stream_markup() -> &'static str {
    "Across the grove, a stream flows north toward **published** commits.\n"
}

/// `pht('You walk along the narrow bank of the stream, ...')`.
#[must_use]
pub fn river_text() -> &'static str {
    "You walk along the narrow bank of the stream as it winds lazily downhill and turns east, gradually widening into a river."
}

/// `pht('Floating on the water, you see published commits:')`.
#[must_use]
pub fn floating_text() -> &'static str {
    "Floating on the water, you see published commits:"
}

/// `pht('The river bubbles quietly, ...')`.
#[must_use]
pub fn bubbles_quiet_text() -> &'static str {
    "The river bubbles quietly, but you do not see any published commits anywhere."
}

/// `pht('They river bubbles peacefully.')`.
#[must_use]
pub fn bubbles_text() -> &'static str {
    "They river bubbles peacefully."
}

/// One git remote with its fetch/push URIs and resolved repository names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub push_uri: Option<String>,
    pub fetch_uri: Option<String>,
    pub push_repository: Option<String>,
    pub fetch_repository: Option<String>,
}

impl RemoteInfo {
    fn new(name: String, push_uri: Option<String>, fetch_uri: Option<String>) -> Self {
        Self {
            name,
            push_uri,
            fetch_uri,
            push_repository: None,
            fetch_repository: None,
        }
    }
}

/// List git remotes under `cwd` (repository names unresolved).
pub fn query_remotes(cwd: &Path) -> Result<Vec<RemoteInfo>, String> {
    if !git::is_inside_work_tree(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let names = git::stdout_text(cwd, &["remote"])?;
    let mut remotes = Vec::new();
    for name in names.lines().filter(|l| !l.trim().is_empty()) {
        let push_uri = git::stdout_text(cwd, &["remote", "get-url", "--push", name]).ok();
        let fetch_uri = git::stdout_text(cwd, &["remote", "get-url", name]).ok();
        remotes.push(RemoteInfo::new(name.to_string(), push_uri, fetch_uri));
    }
    Ok(remotes)
}

/// The four ref-view lines for one remote (`pht('Push URI: %s')`, ...).
#[must_use]
pub fn remote_view_lines(remote: &RemoteInfo) -> [String; 4] {
    let push = remote.push_uri.as_deref().unwrap_or("-");
    let push_repo = remote.push_repository.as_deref().unwrap_or("-");
    let fetch = remote.fetch_uri.as_deref().unwrap_or("-");
    let fetch_repo = remote.fetch_repository.as_deref().unwrap_or("-");
    [
        format!("Push URI: {push}"),
        format!("Push Repository: {push_repo}"),
        format!("Fetch URI: {fetch}"),
        format!("Fetch Repository: {fetch_repo}"),
    ]
}

/// Build the `ArcanistRefView` for one remote (object name + lines).
#[must_use]
pub fn remote_ref_view(remote: &RemoteInfo) -> terminal::RefView {
    terminal::RefView {
        object_name: Some(remote.name.clone()),
        title: None,
        uri: None,
        lines: remote_view_lines(remote).to_vec(),
        children: Vec::new(),
    }
}

/// A repository from `diffusion.repository.search`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRef {
    pub name: String,
    pub uris: Vec<String>,
    /// `fields.refRules.permanentRefRules` (`None` when absent).
    pub permanent_rules: Option<Vec<String>>,
}

/// Search params for repository URIs (house `constraints` shape).
#[must_use]
pub fn repository_search_params(uris: &[String]) -> serde_json::Value {
    serde_json::json!({"constraints": {"uris": uris}, "attachments": {"uris": true}})
}

/// Parse a `diffusion.repository.search` result into repository refs.
pub fn parse_repository_search(value: &serde_json::Value) -> Vec<RepositoryRef> {
    let Some(rows) = value.get("data").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .map(|row| {
            let name = row
                .get("fields")
                .and_then(|f| f.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let uris = row
                .get("attachments")
                .and_then(|a| a.get("uris"))
                .and_then(|u| u.get("uris"))
                .and_then(serde_json::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|entry| {
                            entry
                                .get("fields")
                                .and_then(|f| f.get("uri"))
                                .and_then(|u| u.get("effective"))
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let permanent_rules = row
                .get("fields")
                .and_then(|f| f.get("refRules"))
                .and_then(|r| r.get("permanentRefRules"))
                .and_then(serde_json::Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                });
            RepositoryRef {
                name,
                uris,
                permanent_rules,
            }
        })
        .collect()
}

/// Collect the distinct fetch/push URIs of `remotes` for the search.
#[must_use]
pub fn collect_remote_uris(remotes: &[RemoteInfo]) -> Vec<String> {
    let mut uris = Vec::new();
    for remote in remotes {
        for uri in remote.fetch_uri.iter().chain(remote.push_uri.iter()) {
            if !uris.contains(uri) {
                uris.push(uri.clone());
            }
        }
    }
    uris
}

/// Resolve repositories for `uris` via `diffusion.repository.search`.
pub fn resolve_repositories(
    call: &diff_submit::ConduitCall,
    uris: &[String],
) -> Result<Vec<RepositoryRef>, String> {
    if uris.is_empty() {
        return Ok(Vec::new());
    }
    let result = call(
        "diffusion.repository.search",
        &repository_search_params(uris),
    )?;
    Ok(parse_repository_search(&result))
}

/// Split a git URI into `(domain, path)`, covering `scheme://` and
/// scp-like (`[user@]host:path`) spellings; plain paths have no domain.
fn split_git_uri(uri: &str) -> (Option<String>, String) {
    if let Some(rest) = uri.split_once("://").map(|(_, rest)| rest) {
        let after_host = rest.find('/').map_or("", |i| &rest[i..]);
        let authority = rest.split('/').next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        let host = host.split(':').next().unwrap_or("");
        if host.is_empty() {
            return (None, after_host.to_string());
        }
        return (Some(host.to_string()), after_host.to_string());
    }
    // Windows drive paths (`C:\...`, `C:/...`) are local, not scp-like.
    if uri.len() >= 2
        && uri.as_bytes()[0].is_ascii_alphabetic()
        && uri.as_bytes()[1] == b':'
        && uri[2..].starts_with(['/', '\\'])
    {
        return (None, uri.to_string());
    }
    if let Some((host_part, path)) = uri.split_once(':')
        && !host_part.contains('/')
        && !host_part.contains('\\')
    {
        let host = host_part.rsplit('@').next().unwrap_or("");
        if !host.is_empty() {
            return (Some(host.to_string()), path.to_string());
        }
    }
    (None, uri.to_string())
}

/// Normalize a git URI for comparison, porting
/// `ArcanistRepositoryURINormalizer` (`TYPE_GIT`): lowercased domain plus
/// the trimmed path without `.git`, with Phabricator `/diffusion/X/`
/// prefixes collapsed to the callsign.
#[must_use]
pub fn normalize_git_uri(uri: &str) -> String {
    let (domain, path) = split_git_uri(uri);
    let domain = domain.map_or("<void>".to_string(), |d| d.to_lowercase());
    let mut path = path.trim_matches('/').to_string();
    if let Some(stripped) = path.strip_suffix(".git") {
        path = stripped.to_string();
    }
    if let Some(rest) = path.strip_prefix("diffusion/") {
        let head = rest.split('/').next().unwrap_or("");
        if !head.is_empty()
            && (head.chars().all(|c| c.is_ascii_uppercase())
                || head.chars().all(|c| c.is_ascii_digit()))
        {
            path = format!("diffusion/{head}");
        }
    }
    format!("{domain}/{path}")
}

/// Find the repository whose URIs contain `uri` after normalization.
#[must_use]
pub fn match_repository<'a>(repos: &'a [RepositoryRef], uri: &str) -> Option<&'a RepositoryRef> {
    let wanted = normalize_git_uri(uri);
    repos
        .iter()
        .find(|repo| repo.uris.iter().any(|u| normalize_git_uri(u) == wanted))
}

/// Fill push/fetch repository display names from resolved repositories.
pub fn fill_remote_repositories(remotes: &mut [RemoteInfo], repos: &[RepositoryRef]) {
    for remote in remotes {
        if let Some(uri) = remote.push_uri.clone() {
            remote.push_repository = match_repository(repos, &uri).map(|r| r.name.clone());
        }
        if let Some(uri) = remote.fetch_uri.clone() {
            remote.fetch_repository = match_repository(repos, &uri).map(|r| r.name.clone());
        }
    }
}

/// A remote-tracking branch marker (`refs/remotes/<remote>/<name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub remote: String,
    pub name: String,
    pub hash: String,
}

/// Parse `for-each-ref` `refname + objectname` output into markers.
#[must_use]
pub fn parse_marker_refs(stdout: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        let Some((refname, hash)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Some(label) = refname.strip_prefix("refs/remotes/") else {
            continue;
        };
        let Some((remote, name)) = label.split_once('/') else {
            continue;
        };
        if remote.is_empty() || name.is_empty() || hash.trim().is_empty() {
            continue;
        }
        markers.push(Marker {
            remote: remote.to_string(),
            name: name.to_string(),
            hash: hash.trim().to_string(),
        });
    }
    markers
}

/// List remote-tracking branch markers under `cwd`.
pub fn query_markers(cwd: &Path) -> Result<Vec<Marker>, String> {
    if !git::is_inside_work_tree(cwd) {
        return Err("not inside a git repository".to_string());
    }
    let out = git::stdout_text(
        cwd,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/remotes",
        ],
    )?;
    Ok(parse_marker_refs(&out))
}

/// Mirror `ArcanistRepositoryRef::isPermanentRef`: absent rules match
/// nothing, empty rules match everything, otherwise exact names match.
/// `regexp(...)` rules never match (no regex engine).
#[must_use]
pub fn is_permanent_ref(rules: Option<&[String]>, name: &str) -> bool {
    let Some(rules) = rules else {
        return false;
    };
    if rules.is_empty() {
        return true;
    }
    rules.iter().any(|rule| rule == name)
}

/// Published hashes: markers whose remote resolves to a push repository
/// whose permanent-ref rules match the branch name.
#[must_use]
pub fn published_hashes(
    markers: &[Marker],
    remotes: &[RemoteInfo],
    repos: &[RepositoryRef],
) -> Vec<String> {
    let mut hashes = Vec::new();
    for marker in markers {
        let Some(remote) = remotes.iter().find(|r| r.name == marker.remote) else {
            continue;
        };
        let Some(push_uri) = remote.push_uri.as_deref() else {
            continue;
        };
        let Some(repo) = match_repository(repos, push_uri) else {
            continue;
        };
        if is_permanent_ref(repo.permanent_rules.as_deref(), &marker.name) {
            hashes.push(marker.hash.clone());
        }
    }
    hashes
}

/// Remotes under `cwd`, with repositories resolved when `call` is given.
pub fn look_remotes_data(
    cwd: &Path,
    call: Option<&diff_submit::ConduitCall>,
) -> Result<Vec<RemoteInfo>, String> {
    let mut remotes = query_remotes(cwd)?;
    if let Some(call) = call {
        let uris = collect_remote_uris(&remotes);
        let repos = resolve_repositories(call, &uris)?;
        fill_remote_repositories(&mut remotes, &repos);
    }
    Ok(remotes)
}

/// Published hashes under `cwd`: permanent refs when `call` resolves
/// repositories, else every remote-tracking tip (offline fallback).
pub fn look_published_data(
    cwd: &Path,
    call: Option<&diff_submit::ConduitCall>,
) -> Result<Vec<String>, String> {
    let markers = query_markers(cwd)?;
    let Some(call) = call else {
        return Ok(markers.into_iter().map(|m| m.hash).collect());
    };
    let remotes = query_remotes(cwd)?;
    let uris = collect_remote_uris(&remotes);
    let repos = resolve_repositories(call, &uris)?;
    Ok(published_hashes(&markers, &remotes, &repos))
}

/// Tips of remote-tracking branches under `cwd` (unfiltered).
pub fn query_published(cwd: &Path) -> Result<Vec<String>, String> {
    Ok(query_markers(cwd)?
        .into_iter()
        .map(|marker| marker.hash)
        .collect())
}

/// Plain-text clearing description for `now_unix` (no console markup).
#[must_use]
pub fn render_clearing(now_unix: u64) -> String {
    let mut out = format!("{}\n\n", clearing_text());
    out += hour_text(hour_of(now_unix), lunar_phase(now_unix as i64));
    out += "\n\n";
    out += &format!("{}\n\n", trails_text());
    out += "Just ahead to the north, you can see remotes.\n";
    out
}

/// Plain-text message for looking at something which is not there.
#[must_use]
pub fn render_unknown(thing: &[String]) -> String {
    format!("{}\n", unknown_message(thing))
}

/// Plain-text rendering of one remote block.
#[must_use]
pub fn render_remote(remote: &RemoteInfo) -> String {
    let push = remote.push_uri.as_deref().unwrap_or("-");
    let push_repo = remote.push_repository.as_deref().unwrap_or("-");
    let fetch = remote.fetch_uri.as_deref().unwrap_or("-");
    let fetch_repo = remote.fetch_repository.as_deref().unwrap_or("-");
    format!(
        "Remote \"{}\":\n  Push URI: {push}\n  Push Repository: {push_repo}\n  Fetch URI: {fetch}\n  Fetch Repository: {fetch_repo}\n",
        remote.name
    )
}

/// Plain-text rendering of the `remotes` grove.
#[must_use]
pub fn render_remotes(remotes: &[RemoteInfo]) -> String {
    let mut out = format!("{}\n\n", grove_text());
    out += &format!("{}\n\n", grove_remotes_text());
    for remote in remotes {
        out += &render_remote(remote);
    }
    out += "\nAcross the grove, a stream flows north toward published commits.\n";
    out
}

/// Plain-text rendering of the `published` river.
#[must_use]
pub fn render_published(hashes: &[String]) -> String {
    let mut out = format!("{}\n\n", river_text());
    if hashes.is_empty() {
        out += &format!("{}\n", bubbles_quiet_text());
    } else {
        out += &format!("{}\n\n", floating_text());
        for hash in hashes {
            out += hash;
            out.push('\n');
        }
        out += &format!("\n{}\n", bubbles_text());
    }
    out
}

/// Render `thing` under `cwd` as plain text (no console markup),
/// returning the full stdout text and exit code. Local-only: repository
/// names stay `-` and published lists every remote-tracking tip.
pub fn render(cwd: &Path, thing: &[String], now_unix: u64) -> Result<(String, ExitCode), String> {
    let mut out = "Arcventure\n\n".to_string();
    if thing.is_empty() {
        out += &render_clearing(now_unix);
        return Ok((out, ExitCode::SUCCESS));
    }
    if thing == ["remotes"] {
        out += &render_remotes(&query_remotes(cwd)?);
        return Ok((out, ExitCode::SUCCESS));
    }
    if thing == ["published"] {
        out += &render_published(&query_published(cwd)?);
        return Ok((out, ExitCode::SUCCESS));
    }
    out += &render_unknown(thing);
    Ok((out, ExitCode::FAILURE))
}

/// The `%!` title banner plus blank line.
pub fn render_title(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%!\n\n", &[terminal::TVal::Text(TITLE)], ansi)
        .map(|line| line.as_str().to_string())
}

/// One `%W` paragraph plus blank line.
pub fn render_paragraph(text: &str, ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%W\n\n", &[terminal::TVal::Text(text)], ansi)
        .map(|line| line.as_str().to_string())
}

/// A markup pattern with no substitutions (the `**remotes**` /
/// `**published**` lines and bare newlines).
pub fn render_markup(markup: &str, ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi(markup, &[], ansi).map(|line| line.as_str().to_string())
}

/// One `%s` line (unknown things and commit hashes).
pub fn render_line(text: &str, ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%s\n", &[terminal::TVal::Text(text)], ansi)
        .map(|line| line.as_str().to_string())
}

/// The empty-river paragraph (`"%W\n"`, no blank line).
pub fn render_quiet_river(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("%W\n", &[terminal::TVal::Text(bubbles_quiet_text())], ansi)
        .map(|line| line.as_str().to_string())
}

/// The peaceful-river tail (`"\n%W\n"`).
pub fn render_bubbles_tail(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("\n%W\n", &[terminal::TVal::Text(bubbles_text())], ansi)
        .map(|line| line.as_str().to_string())
}

/// One remote's ref view (`tsprintf('%s', $view)`).
#[must_use]
pub fn render_remote_view(remote: &RemoteInfo, width: usize, ansi: bool) -> String {
    remote_ref_view(remote).render(0, width, ansi)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Best-effort Conduit client for repository mapping (`None` degrades to
/// local-only data).
fn look_conduit_client() -> Option<conduit::Client> {
    // NOTE: `conduit::Client` is not `Debug`; keep the error private.
    config::client_for(None).ok()
}

fn fail(message: String) -> ExitCode {
    eprintln!("arc-rs {NAME}: {message}");
    ExitCode::FAILURE
}

fn conduit_error_text(error: conduit::Error) -> String {
    error.to_string()
}

fn call_or_unavailable(
    client: Option<&conduit::Client>,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    match client {
        Some(client) => client.call(method, params).map_err(conduit_error_text),
        None => Err("conduit unavailable".to_string()),
    }
}

fn optional_call<'a>(
    client: &'a Option<conduit::Client>,
    call: &'a dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String>,
) -> Option<&'a diff_submit::ConduitCall<'a>> {
    if client.is_some() { Some(call) } else { None }
}

fn render_remotes_output(remotes: &[RemoteInfo], ansi: bool) -> Result<String, String> {
    let mut out = String::new();
    for part in [
        render_title(ansi),
        render_paragraph(grove_text(), ansi),
        render_paragraph(grove_remotes_text(), ansi),
    ] {
        out.push_str(&part?);
    }
    let width = terminal::terminal_width();
    for remote in remotes {
        out.push_str(&render_remote_view(remote, width, ansi));
    }
    let blank = render_markup("\n", ansi)?;
    let tail = render_markup(stream_markup(), ansi)?;
    out.push_str(&blank);
    out.push_str(&tail);
    Ok(out)
}

fn render_published_output(hashes: &[String], ansi: bool) -> Result<String, String> {
    let mut out = String::new();
    for part in [render_title(ansi), render_paragraph(river_text(), ansi)] {
        out.push_str(&part?);
    }
    if hashes.is_empty() {
        out.push_str(&render_quiet_river(ansi)?);
        return Ok(out);
    }
    out.push_str(&render_paragraph(floating_text(), ansi)?);
    for hash in hashes {
        out.push_str(&render_line(hash, ansi)?);
    }
    out.push_str(&render_bubbles_tail(ansi)?);
    Ok(out)
}

fn run_clearing(ansi: bool) -> ExitCode {
    let now = now_unix();
    let time = hour_text(hour_of(now), lunar_phase(now as i64));
    let parts = [
        render_title(ansi),
        render_paragraph(clearing_text(), ansi),
        render_paragraph(time, ansi),
        render_paragraph(trails_text(), ansi),
        render_markup(north_markup(), ansi),
    ];
    let mut out = String::new();
    for part in parts {
        match part {
            Ok(text) => out.push_str(&text),
            Err(message) => return fail(message),
        }
    }
    print!("{out}");
    ExitCode::SUCCESS
}

fn run_remotes(cwd: &Path, ansi: bool) -> ExitCode {
    let client = look_conduit_client();
    let call = |method: &str, params: &serde_json::Value| {
        call_or_unavailable(client.as_ref(), method, params)
    };
    let remotes = match look_remotes_data(cwd, optional_call(&client, &call)) {
        Ok(remotes) => remotes,
        Err(message) => return fail(message),
    };
    match render_remotes_output(&remotes, ansi) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(message) => fail(message),
    }
}

fn run_published(cwd: &Path, ansi: bool) -> ExitCode {
    let client = look_conduit_client();
    let call = |method: &str, params: &serde_json::Value| {
        call_or_unavailable(client.as_ref(), method, params)
    };
    let hashes = match look_published_data(cwd, optional_call(&client, &call)) {
        Ok(hashes) => hashes,
        Err(message) => return fail(message),
    };
    match render_published_output(&hashes, ansi) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(message) => fail(message),
    }
}

fn run_unknown(thing: &[String], ansi: bool) -> ExitCode {
    let mut out = String::new();
    for part in [
        render_title(ansi),
        render_line(&unknown_message(thing), ansi),
    ] {
        match part {
            Ok(text) => out.push_str(&text),
            Err(message) => return fail(message),
        }
    }
    print!("{out}");
    ExitCode::FAILURE
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let thing = match parse_args(argv) {
        Ok(Some(thing)) => thing,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("arc-rs {NAME}: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let ansi = terminal::ansi_enabled();
    if thing.is_empty() {
        return run_clearing(ansi);
    }
    if thing == ["remotes"] {
        return run_remotes(&cwd, ansi);
    }
    if thing == ["published"] {
        return run_published(&cwd, ansi);
    }
    run_unknown(&thing, ansi)
}

#[cfg(test)]
#[path = "look_tests.rs"]
mod look_tests;
