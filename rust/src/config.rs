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
    host_field_for_str(text, conduit_uri, "token")
}

/// One `hosts[uri]` credential field from arcrc text. PHP reads `token`,
/// legacy `user`, and legacy `cert` (`scripts/arcanist.php` startup).
pub fn host_field_for_str(text: &str, conduit_uri: &str, field: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    doc.get("hosts")?
        .get(host_key(conduit_uri))?
        .get(field)?
        .as_str()
        .map(str::to_string)
}

/// Credentials for one conduit URI: the new-style `token` plus the
/// legacy `user` + `cert` pair, mirroring the `scripts/arcanist.php`
/// startup reads (T1153 CORE-4/T1153 CORE-9).
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    /// `hosts[uri].token` (or the `--conduit-token` override).
    pub token: Option<String>,
    /// Legacy `hosts[uri].user`.
    pub user: Option<String>,
    /// Legacy `hosts[uri].cert`.
    pub cert: Option<String>,
}

impl Credentials {
    /// Whether the classic startup gate (`scripts/arcanist.php`
    /// `$need_auth`) lets the command proceed: a token or a complete
    /// legacy pair, all three under `empty()` semantics (so `"0"`
    /// counts as missing here, even though `authenticateConduit()`
    /// would accept a `"0"` token under `strlen()`).
    #[must_use]
    pub fn has_any(&self) -> bool {
        if let Some(token) = &self.token
            && !php_empty_str(token)
        {
            return true;
        }
        match (&self.user, &self.cert) {
            (Some(user), Some(cert)) => !php_empty_str(user) && !php_empty_str(cert),
            _ => false,
        }
    }
}

/// PHP `empty()` for credential strings: `""` and `"0"` (plus, here,
/// only those two shapes can occur) count as missing.
fn php_empty_str(value: &str) -> bool {
    value.is_empty() || value == "0"
}

/// Resolve the effective token: `--conduit-token` wins when given and
/// nonempty (T1153 CORE-10; PHP's classic startup applies it under
/// truthiness, the toolset under `!== null` — the `""`-override split
/// between the two runtimes is not replicated), else the stored file
/// token.
#[must_use]
pub fn resolve_token(force: Option<&str>, file: Option<String>) -> Option<String> {
    match force {
        Some(token) if !token.is_empty() => Some(token.to_string()),
        _ => file,
    }
}

/// Load [`Credentials`] for `uri` from `user_file`, applying the
/// `--conduit-token` override to the token slot. Precedence: explicit
/// override, global `--conduit-token` (T1153 CORE-10, delivered via
/// `ARC_GLOBAL_CONDUIT_TOKEN`), `ARC_CONDUIT_TOKEN`, stored file token.
/// Global `--anonymous` (`ARC_ANONYMOUS=1`) nulls the token only —
/// legacy `user`/`cert` still authenticate, like PHP's startup.
#[must_use]
pub fn credentials_for(
    user_file: Option<&Path>,
    conduit_uri: &str,
    force_token: Option<&str>,
) -> Credentials {
    let global_token = crate::globals::conduit_token();
    let env_token = std::env::var("ARC_CONDUIT_TOKEN").ok();
    let force_token = force_token
        .or(global_token.as_deref())
        .or(env_token.as_deref());
    let anonymous = crate::globals::anonymous();
    let text = user_file.and_then(|path| std::fs::read_to_string(path).ok());
    let file_token = text
        .as_deref()
        .and_then(|text| token_for_str(text, conduit_uri));
    Credentials {
        token: if anonymous {
            None
        } else {
            resolve_token(force_token, file_token)
        },
        user: text
            .as_deref()
            .and_then(|text| host_field_for_str(text, conduit_uri, "user")),
        cert: text
            .as_deref()
            .and_then(|text| host_field_for_str(text, conduit_uri, "cert")),
    }
}

/// [`get_config::LoadOptions`] from the global `--config`,
/// `--config-file`, and `--arcrc-file` flags (T1153 CORE-10/T1153 CORE-11
/// family), delivered by the dispatcher as JSON in
/// `ARC_GLOBAL_LOAD_OPTIONS`. Unparseable or absent input reads
/// default (no overrides); only the dispatcher writes this channel.
#[must_use]
pub fn global_load_options() -> get_config::LoadOptions {
    let Some(raw) = crate::globals::load_options() else {
        return get_config::LoadOptions::default();
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return get_config::LoadOptions::default();
    };
    let runtime = doc
        .get("runtime")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let config_files = doc
        .get("config_files")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str().map(PathBuf::from))
        .collect();
    let arcrc_file = doc
        .get("arcrc_file")
        .and_then(|value| value.as_str().map(PathBuf::from));
    get_config::LoadOptions {
        runtime,
        config_files,
        arcrc_file,
    }
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
/// [`get_config::LoadOptions`]. Tokens come from the resolved user file
/// (overridden by `--conduit-token`/`ARC_CONDUIT_TOKEN`, T1153
/// T1153 CORE-10), so `--arcrc-file` redirects credential lookup like stock
/// PHP. Missing credentials stay an error here; commands PHP runs
/// tokenless use [`client_for_tokenless_with`].
pub fn client_for_with(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<crate::conduit::Client, String> {
    build_client(conduit_uri, dir, user_file, opts, true)
}

/// Tokenless-capable [`client_for_with`] (T1153 CORE-4): PHP's engine
/// construction never fails for missing credentials — the engine runs
/// tokenless and auth failures surface per call. Missing credentials
/// here yield an empty-token client instead of an error. Classic
/// commands gate with [`Credentials::has_any`] + [`require_auth_text`]
/// and verify with [`authenticate`]; toolset commands call through
/// [`Client::call_mapped`](crate::conduit::Client::call_mapped).
pub fn client_for_tokenless_with(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> Result<crate::conduit::Client, String> {
    build_client(conduit_uri, dir, user_file, opts, false)
}

fn build_client(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
    strict_token: bool,
) -> Result<crate::conduit::Client, String> {
    let user = get_config::resolve_user_file(
        user_file.map(Path::to_path_buf),
        opts.arcrc_file.as_deref(),
    )?;
    // Precedence: explicit flag, global `--conduit-uri` (T1153 CORE-11,
    // delivered via `ARC_GLOBAL_CONDUIT_URI`), `ARC_CONDUIT_URI`, merged
    // config. Empty values fall through like PHP's falsy checks.
    let global_uri = crate::globals::conduit_uri();
    let uri = conduit_uri
        .map(str::to_string)
        .filter(|uri| !uri.trim().is_empty())
        .or(global_uri.filter(|uri| !uri.trim().is_empty()))
        .or_else(|| {
            std::env::var("ARC_CONDUIT_URI")
                .ok()
                .filter(|uri| !uri.trim().is_empty())
        })
        .or_else(|| discover_uri_with(dir, user.as_deref(), opts))
        .ok_or_else(|| {
            "no conduit URI: pass --conduit-uri, set ARC_CONDUIT_URI, or run inside a project with .arcconfig"
                .to_string()
        })?;
    let creds = credentials_for(user.as_deref(), &uri, None);
    let token = match (&creds.token, strict_token) {
        (Some(token), _) => token.clone(),
        (None, false) => String::new(),
        (None, true) => return Err(format!("no token for {uri} in ~/.arcrc")),
    };
    let tls = tls_config_for(dir, user.as_deref(), opts);
    crate::conduit::Client::new_with_tls(&uri, &token, &tls).map_err(|e| e.to_string())
}

/// Read [`TlsConfig`](crate::conduit::TlsConfig) from merged config
/// (`https.cabundle`, `https.blindly-trust-domains`, T1153 CORE-8).
/// Lenient like discovery: unreadable config reads default TLS.
fn tls_config_for(
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
) -> crate::conduit::TlsConfig {
    let Ok(merged) = get_config::load_merged(dir, user_file, opts) else {
        return crate::conduit::TlsConfig::default();
    };
    let cabundle = get_config::merged_value("https.cabundle", &merged)
        .and_then(|(_, value)| value.as_str().map(str::to_string))
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            let path = PathBuf::from(&raw);
            if path.is_absolute() {
                path
            } else {
                // Resolved against the project root, like PHP's
                // `Filesystem::resolvePath($ca_bundle, $project_root)`.
                let root = merged
                    .sources
                    .project_file
                    .as_ref()
                    .and_then(|file| file.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| dir.to_path_buf());
                root.join(path)
            }
        });
    let blind_trust = get_config::merged_value("https.blindly-trust-domains", &merged)
        .and_then(|(_, value)| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str().map(|domain| domain.to_ascii_lowercase()))
        .collect();
    crate::conduit::TlsConfig {
        cabundle,
        blind_trust,
    }
}

/// Domain of a conduit URI for auth prose (`PhutilURI::getDomain()`:
/// host without port or userinfo).
#[must_use]
pub fn uri_domain(uri: &str) -> String {
    let after_scheme = uri.split("://").nth(1).unwrap_or(uri);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let bare = hostport.strip_prefix('[').and_then(|rest| {
        rest.find(']').map(|end| {
            let (host, _) = hostport.split_at(end + 2);
            host.to_string()
        })
    });
    match bare {
        Some(host) => host,
        None => hostport
            .rsplit_once(':')
            .map_or(hostport, |(host, _)| host)
            .to_string(),
    }
}

/// Classic `$need_auth` gate text (`scripts/arcanist.php`, T1153
/// T1153 CORE-4), thrown as `ArcanistUsageException` when no credential
/// exists at all. `forced_uri` is `Some` when `--conduit-uri` selected
/// the URI, in which case the hint reproduces the flag so the
/// install command targets the same server.
#[must_use]
pub fn require_auth_text(domain: &str, forced_uri: Option<&str>) -> String {
    let arc = match forced_uri {
        Some(uri) => format!("arc --conduit-uri={uri}"),
        None => "arc".to_string(),
    };
    let pattern = format!("%s\n\n%s\n\n%s **%s:**\n\n      $ **{arc} install-certificate**\n");
    crate::terminal::console_format(
        &pattern,
        &[
            "YOU NEED TO AUTHENTICATE TO CONTINUE",
            &format!(
                "You are trying to connect to a server ({domain}) that you do not have any \
                 credentials stored for."
            ),
            "To retrieve and store credentials for this server,",
            "run this command",
        ],
        crate::terminal::ansi_enabled(),
    )
}

/// The invoking command line for legacy `conduit.connect`
/// `clientDescription` (`implode(' ', $original_argv)`).
#[must_use]
pub fn command_description() -> String {
    std::env::args().collect::<Vec<_>>().join(" ")
}

/// Classic `requiresAuthentication` startup (T1153 CORE-4/T1153 CORE-9):
/// tokenless engine, credential gate, then `authenticateConduit()`.
/// Returns the authenticated client, or a classified error ready to
/// report. A missing URI is a usage error (PHP's no-URI
/// `ArcanistUsageException`; the message bytes stay stable for other
/// scopes' pins); setup failures otherwise stay generic. The gate
/// hint reproduces `--conduit-uri` when argv (flag or global) forced
/// the URI.
pub fn authed_client_for(
    conduit_uri: Option<&str>,
    dir: &Path,
    user_file: Option<&Path>,
    opts: &get_config::LoadOptions,
    description: &str,
) -> Result<crate::conduit::Client, crate::conduit::ClassifiedError> {
    use crate::conduit::{ClassifiedError, EnvelopeKind};
    let mut client = match client_for_tokenless_with(conduit_uri, dir, user_file, opts) {
        Ok(client) => client,
        Err(message) => {
            let kind = if message.starts_with("no conduit URI") {
                EnvelopeKind::Usage
            } else {
                EnvelopeKind::Exception
            };
            return Err(ClassifiedError::new(kind, message));
        }
    };
    // Re-resolve (build_client just did): `--arcrc-file` redirects
    // credential lookup, and only the resolved file has the credentials.
    let user =
        get_config::resolve_user_file(user_file.map(Path::to_path_buf), opts.arcrc_file.as_deref())
            .map_err(ClassifiedError::exception)?;
    let creds = credentials_for(user.as_deref(), client.base_uri(), None);
    if !creds.has_any() {
        let global_uri = crate::globals::conduit_uri();
        let forced = conduit_uri
            .filter(|uri| !uri.trim().is_empty())
            .or(global_uri.as_deref());
        let text = require_auth_text(&uri_domain(client.base_uri()), forced);
        return Err(ClassifiedError::usage(text));
    }
    authenticate(&mut client, &creds, description).map(|_| client)
}

/// Authenticate a classic-workflow client, mirroring
/// `ArcanistWorkflow::authenticateConduit()` (T1153 CORE-9): a
/// `strlen()`-nonempty token verifies with `user.whoami` (any failure
/// unsets the token before propagating); otherwise the legacy `user` +
/// `cert` pair runs the `conduit.connect` handshake. Returns the
/// authenticated `(user_name, user_phid)`. The three credential codes
/// (`ERR-NO-CERTIFICATE`, `ERR-INVALID-USER`, `ERR-INVALID-AUTH`,
/// including the locally raised empty-credential ones) render the
/// install-certificate usage guidance; anything else propagates raw
/// under the generic `Exception` arm. `NEW-ARC-VERSION` is not
/// special-cased: orsay answers our `clientVersion` 6 exactly as it
/// answers PHP's, so the upgrade prompt is unreachable in practice.
pub fn authenticate(
    client: &mut crate::conduit::Client,
    creds: &Credentials,
    description: &str,
) -> Result<(String, String), crate::conduit::ClassifiedError> {
    if let Some(token) = &creds.token
        && !token.is_empty()
    {
        client.set_conduit_token(token);
        match client.call("user.whoami", &serde_json::json!({})) {
            Ok(result) => {
                let user = result
                    .get("userName")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                let phid = result
                    .get("phid")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string();
                client.set_authenticated_user(&user, &phid);
                return Ok((user, phid));
            }
            Err(error) => {
                client.set_conduit_token("");
                return Err(map_auth_failure(client.base_uri(), &error));
            }
        }
    }
    // Legacy path (also with an empty-string token: `strlen()` fails
    // and the pair is required, like PHP).
    let user = creds.user.clone().unwrap_or_default();
    let cert = creds.cert.clone().unwrap_or_default();
    if php_empty_str(&user) {
        return Err(install_guidance(
            client.base_uri(),
            "ERR-INVALID-USER: Empty user in credentials.",
        ));
    }
    if php_empty_str(&cert) {
        return Err(install_guidance(
            client.base_uri(),
            "ERR-NO-CERTIFICATE: Empty certificate in credentials.",
        ));
    }
    let params = serde_json::json!({
        "client": "arc",
        "clientVersion": 6,
        "clientDescription": format!(
            "{}:{description}",
            crate::diff_local::default_source_machine_name()
        ),
        "user": user,
        "certificate": cert,
        "host": client.base_uri(),
    });
    match client.call("conduit.connect", &params) {
        Ok(connection) => {
            let phid = connection
                .get("userPHID")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            client.set_authenticated_user(&user, &phid);
            Ok((user, phid))
        }
        Err(error) => Err(map_auth_failure(client.base_uri(), &error)),
    }
}

/// Map an `authenticateConduit()` failure: the three credential codes
/// become the install-certificate usage guidance, everything else
/// propagates raw under the generic arm.
fn map_auth_failure(
    conduit_uri: &str,
    error: &crate::conduit::Error,
) -> crate::conduit::ClassifiedError {
    if let crate::conduit::Error::Api { code, .. } = error
        && matches!(
            code.as_str(),
            "ERR-NO-CERTIFICATE" | "ERR-INVALID-USER" | "ERR-INVALID-AUTH"
        )
    {
        install_guidance(conduit_uri, &error.to_string())
    } else {
        crate::conduit::ClassifiedError::exception(error.to_string())
    }
}

/// The `authenticateConduit()` install-certificate guidance
/// (`ArcanistUsageException`, rendered under `Usage Exception:`).
/// Markup markers in the values print literally:
/// `phutil_console_format` interprets the pattern only, then
/// `sprintf()`-substitutes raw.
fn install_guidance(conduit_uri: &str, server_message: &str) -> crate::conduit::ClassifiedError {
    let message = crate::terminal::console_format(
        "\n%s\n\n    %s\n\n%s\n%s",
        &[
            "YOU NEED TO __INSTALL A CERTIFICATE__ TO LOGIN TO PHABRICATOR",
            "To do this, run: **arc install-certificate**",
            &format!("The server '{conduit_uri}' rejected your request:"),
            server_message,
        ],
        crate::terminal::ansi_enabled(),
    );
    crate::conduit::ClassifiedError::usage(message)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
