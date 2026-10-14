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

#[cfg(test)]
mod tests {
    use super::*;

    const ARCRC: &str = r#"{"hosts": {"https://orsay.hhh.pa/api/": {"token": "api-secret"}}}"#;

    #[test]
    fn finds_token_regardless_of_uri_shape() {
        for uri in [
            "https://orsay.hhh.pa",
            "https://orsay.hhh.pa/",
            "https://orsay.hhh.pa/api",
            "https://orsay.hhh.pa/api/",
        ] {
            assert_eq!(
                token_for_str(ARCRC, uri),
                Some("api-secret".to_string()),
                "for {uri}"
            );
        }
    }

    #[test]
    fn misses_cleanly() {
        assert_eq!(token_for_str(ARCRC, "https://other.example/"), None);
        assert_eq!(token_for_str("{}", "https://orsay.hhh.pa/"), None);
        assert_eq!(token_for_str("not json", "https://orsay.hhh.pa/"), None);
        assert_eq!(
            token_for_str(
                r#"{"hosts": {"https://orsay.hhh.pa/api/": {}}}"#,
                "https://orsay.hhh.pa/"
            ),
            None
        );
    }

    #[test]
    fn discovers_arcconfig_upwards() {
        let root = std::env::temp_dir().join(format!("arc-rs-disc-{}", std::process::id()));
        let child = root.join("a").join("b");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(
            root.join("a").join(".arcconfig"),
            r#"{"phabricator.uri": "https://orsay.hhh.pa/"}"#,
        )
        .unwrap();
        assert_eq!(
            discover_from(&child),
            Some("https://orsay.hhh.pa/".to_string())
        );
        assert_eq!(discover_from(&root), None);
        std::fs::remove_dir_all(&root).ok();
    }
}
