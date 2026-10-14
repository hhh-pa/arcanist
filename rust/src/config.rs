//! Conduit endpoint and credential discovery, mirroring `ArcanistRuntime`:
//! the URI comes from `--conduit-uri`, `ARC_CONDUIT_URI`, or a `.arcconfig`
//! walk-up; tokens come from `~/.arcrc` host entries.

use std::path::{Path, PathBuf};

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

pub fn discover_from(dir: &Path) -> Option<String> {
    dir.ancestors().find_map(|dir| {
        let text = std::fs::read_to_string(dir.join(".arcconfig")).ok()?;
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()?
            .get("phabricator.uri")?
            .as_str()
            .map(str::to_string)
    })
}

/// Resolve the conduit URI (explicit override, `ARC_CONDUIT_URI`, or
/// `.arcconfig` walk-up) and build an authenticated client from `~/.arcrc`.
pub fn client_for(conduit_uri: Option<&str>) -> Result<crate::conduit::Client, String> {
    let uri = conduit_uri
        .map(str::to_string)
        .or_else(|| std::env::var("ARC_CONDUIT_URI").ok())
        .or_else(discover_conduit_uri)
        .ok_or_else(|| {
            "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with .arcconfig"
                .to_string()
        })?;
    let token = arcrc_path()
        .and_then(|path| token_for_file(&path, &uri))
        .ok_or_else(|| format!("no token for {uri} in ~/.arcrc"))?;
    crate::conduit::Client::new(&uri, &token).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
