//! Conduit endpoint and credential discovery, mirroring `ArcanistRuntime`:
//! the URI comes from `--conduit-uri`, `ARC_CONDUIT_URI`, or the merged
//! config (`phabricator.uri`, falling back to `default`); tokens come
//! from the user file's host entries.

use std::path::{Path, PathBuf};

use crate::get_config;

pub fn arcrc_path() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }?;
    Some(PathBuf::from(home).join(".arcrc"))
}

pub fn token_for_file(path: &Path, conduit_uri: &str) -> Option<String> {
    token_for_str(&std::fs::read_to_string(path).ok()?, conduit_uri)
}

pub fn token_for_str(text: &str, conduit_uri: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    doc.get("hosts")?
        .get(host_key(conduit_uri))?
        .get("token")?
        .as_str()
        .map(str::to_string)
}

fn host_key(conduit_uri: &str) -> String {
    let after_scheme = conduit_uri.split("://").nth(1).unwrap_or(conduit_uri);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let scheme = conduit_uri.split("://").next().unwrap_or("https");
    format!("{scheme}://{authority}/api/")
}

pub fn discover_conduit_uri() -> Option<String> {
    discover_from(&std::env::current_dir().ok()?)
}

/// Discover the conduit URI from `dir`: merged `phabricator.uri` (any
/// source, including the project legacy `conduit_uri`), else merged
/// `default`, mirroring the ceb resolution (`scripts/arcanist.php` for
/// classic workflows, `ArcanistRuntime::newConduitEngine()` for toolset
/// ones). Lenient: unreadable sources and type mismatches read empty;
/// commands that can surface errors read through
/// [`get_config::checked_value`] instead.
pub fn discover_from(dir: &Path) -> Option<String> {
    let user_file = arcrc_path();
    discover_uri_with(
        dir,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
}

/// [`discover_from`] with an explicit user file and [`get_config::LoadOptions`].
pub fn discover_uri_with(
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Option<String> {
    let merged = get_config::load_merged(dir, user_file, opts).ok()?;
    get_config::merged_value("phabricator.uri", &merged)
        .or_else(|| get_config::merged_value("default", &merged))
        .and_then(|(_, value)| value.as_str().map(str::to_string))
}

/// Resolve the conduit URI (explicit override, `ARC_CONDUIT_URI`, or
/// merged-config discovery) and build an authenticated client from
/// `~/.arcrc`.
pub fn client_for(conduit_uri: Option<&str>) -> Result<crate::conduit::Client, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user_file = arcrc_path();
    client_for_with(
        conduit_uri,
        &cwd,
        user_file.as_deref(),
        &get_config::LoadOptions::default(),
    )
}

/// [`client_for`] with an explicit working copy, user file, and
/// [`get_config::LoadOptions`]. Tokens come from the resolved user file,
/// so `--arcrc-file` redirects credential lookup like stock PHP.
pub fn client_for_with(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<crate::conduit::Client, String> {
    let user = get_config::resolve_user_file(
        user_file.map(Path::to_path_buf),
        opts.arcrc_file.as_deref(),
    )?;
    let uri = conduit_uri
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(|| discover_uri_with(dir, user.as_deref(), opts))
        .ok_or_else(|| {
            "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with .arcconfig"
                .to_string()
        })?;
    let token = user
        .as_deref()
        .and_then(|path| token_for_file(path, &uri))
        .ok_or_else(|| format!("no token for {uri} in ~/.arcrc"))?;
    crate::conduit::Client::new(&uri, &token).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
