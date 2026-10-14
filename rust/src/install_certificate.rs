//! `arc-rs install-certificate` — install a Conduit API token.
//!
//! PHP reference: `src/workflow/ArcanistInstallCertificateWorkflow.php`.
//!
//! Flow: resolve the server URI (positional URI, `--conduit-uri`,
//! `ARC_CONDUIT_URI`, or the enclosing `.arcconfig`), validate the pasted
//! token's format (`cli-` + 32 chars), verify it with `user.whoami`, then
//! store it under `hosts` in `~/.arcrc`.
//!
//! The legacy certificate exchange (`conduit.getcertificate`) is not
//! ported: servers without token auth fail with a clear message (see
//! T1128). Unlike PHP, the token is validated before any network call so
//! typos fail fast, and `--force` replaces the interactive "install this
//! standard token anyway?" prompt.

extern crate self as arc_rs;

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{conduit, config};

/// Subcommand name, matching `arc install-certificate`.
pub const NAME: &str = "install-certificate";

const USAGE: &str = "usage: arc-rs install-certificate [URI] [--conduit-uri URI] [--token TOKEN] [--force]\n\
\n\
Install a Conduit API token into ~/.arcrc for URI. Without URI, the URI\n\
comes from --conduit-uri, $ARC_CONDUIT_URI, or the enclosing .arcconfig.\n\
Without --token, the token is read from stdin after printing the login page.\n\
\n\
Options:\n\
  --conduit-uri URI   Conduit server URI (under the positional URI).\n\
  --token TOKEN       32-character cli- API token (else prompt via stdin).\n\
  --force             Also accept standard api- tokens (CLI tokens expected).\n\
  -h, --help          Show this help.\n";

const EXAMPLE_URI: &str = "https://devtools.example.com/";

/// Kinds of installable tokens.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// A `cli-` token, the expected kind.
    Cli,
    /// A standard `api-` token, accepted only with `--force`.
    Standard,
}

/// Normalize a server URI to the `scheme://authority/api/` form used as
/// the `~/.arcrc` host key. Only http/https URIs with a domain qualify.
pub fn normalize_uri(raw: &str) -> Result<String, String> {
    let uri = raw.trim();
    let Some((scheme, rest)) = uri.split_once("://") else {
        return Err(format!(
            "Server URI \"{raw}\" must include a protocol and domain. It should be in the form \"{EXAMPLE_URI}\"."
        ));
    };
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Server URI \"{raw}\" must include the \"http\" or \"https\" protocol. It should be in the form \"{EXAMPLE_URI}\"."
        ));
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        return Err(format!(
            "Server URI \"{raw}\" must include a protocol and domain. It should be in the form \"{EXAMPLE_URI}\"."
        ));
    }
    Ok(format!("{scheme}://{authority}/api/"))
}

/// Validate a pasted token's shape. `api-` tokens need `force`.
pub fn validate_token(token: &str, force: bool) -> Result<TokenKind, String> {
    if token.len() != 32 {
        return Err(format!(
            "The token \"{token}\" is not formatted correctly. API tokens should be 32 characters long. Make sure you visited the correct URI and copy/pasted the token correctly."
        ));
    }
    if token.starts_with("api-") {
        if force {
            return Ok(TokenKind::Standard);
        }
        return Err(format!(
            "The token \"{token}\" is a standard API token, but a CLI API token was expected. Pass --force to install it anyway."
        ));
    }
    if token.starts_with("cli-") {
        return Ok(TokenKind::Cli);
    }
    Err(format!(
        "The token \"{token}\" is not formatted correctly. Valid API tokens should begin \"cli-\" and be 32 characters long. Make sure you visited the correct URI and copy/pasted the token correctly."
    ))
}

/// Resolve the server URI: positional, `--conduit-uri`, environment, then
/// the `.arcconfig` enclosing `dir`.
pub fn resolve_uri(
    positional: Option<&str>,
    flag: Option<&str>,
    env: Option<&str>,
    dir: &Path,
) -> Result<String, String> {
    let picked = [positional, flag, env]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| config::discover_from(dir));
    match picked {
        Some(uri) => normalize_uri(&uri),
        None => Err(
            "Specify an explicit URI or run this command from within a project which is configured with a .arcconfig."
                .to_string(),
        ),
    }
}

/// The login page a user visits to mint a token for a normalized URI.
#[must_use]
pub fn token_login_url(normalized_uri: &str) -> String {
    format!(
        "{}conduit/login/",
        normalized_uri
            .strip_suffix("api/")
            .unwrap_or(normalized_uri)
    )
}

/// Read a whole `~/.arcrc` document; a missing file yields `{}`.
pub fn read_token_doc(path: &Path) -> Result<serde_json::Value, String> {
    if !path.is_file() {
        return Ok(serde_json::json!({}));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("'{}' is not a valid JSON file: {e}", path.display()))?;
    if doc.is_object() {
        Ok(doc)
    } else {
        Err(format!("'{}' is not a JSON object", path.display()))
    }
}

/// Store `token` for `uri` in the `~/.arcrc` at `path`, preserving the
/// rest of the file.
pub fn install_token(path: &Path, uri: &str, token: &str) -> Result<(), String> {
    let mut doc = read_token_doc(path)?;
    let hosts = doc
        .as_object_mut()
        .expect("read_token_doc object")
        .entry("hosts".to_string())
        .or_insert_with(|| serde_json::json!({}));
    let hosts = hosts
        .as_object_mut()
        .ok_or_else(|| format!("'{}' has a non-object \"hosts\" section", path.display()))?;
    hosts.insert(uri.to_string(), serde_json::json!({"token": token}));
    let mut text =
        serde_json::to_string_pretty(&doc).map_err(|e| format!("cannot encode JSON: {e}"))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("cannot write '{}': {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(path, permissions).ok();
        }
    }
    Ok(())
}

/// Ping the server; failures become "failed to connect" errors.
pub fn ping(uri: &str) -> Result<(), String> {
    let client = conduit::Client::new(uri, "")
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))?;
    client
        .call("conduit.ping", &serde_json::json!({}))
        .map(|_| ())
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))
}

/// Reject servers that affirmatively lack token auth. Servers whose
/// capabilities cannot be read are assumed modern and proceed to the
/// `user.whoami` check, which is authoritative.
pub fn check_token_auth(uri: &str) -> Result<(), String> {
    let client = conduit::Client::new(uri, "")
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))?;
    let capabilities = match client.call("conduit.getcapabilities", &serde_json::json!({})) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let auth = capabilities
        .get("authentication")
        .and_then(serde_json::Value::as_array);
    match auth {
        Some(methods) if methods.iter().any(|m| m.as_str() == Some("token")) => Ok(()),
        Some(_) => Err(format!(
            "Server \"{uri}\" does not support token auth; the legacy certificate exchange is not yet ported (see T1128)."
        )),
        None => Ok(()),
    }
}

/// Verify `token` against the server.
pub fn whoami(uri: &str, token: &str) -> Result<(), String> {
    let client = conduit::Client::new(uri, token)
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))?;
    client
        .call("user.whoami", &serde_json::json!({}))
        .map(|_| ())
        .map_err(|e| {
            format!(
                "The token \"{token}\" is not a valid API Token. The server returned this response when trying to use it as a token: {e}"
            )
        })
}

/// Validate, verify, and store `token` for `uri`. Returns the message to
/// print. Nothing is written unless every check passes.
pub fn install(uri: &str, token: &str, arcrc: &Path, force: bool) -> Result<String, String> {
    validate_token(token, force)?;
    ping(uri)?;
    check_token_auth(uri)?;
    whoami(uri, token)?;
    install_token(arcrc, uri, token)?;
    Ok("API Token installed.".to_string())
}

/// Prompt for a token on stdin, reprompting until a non-empty line or EOF.
pub fn prompt_token() -> Result<String, String> {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        match lines.next() {
            Some(Ok(line)) if !line.trim().is_empty() => return Ok(line.trim().to_string()),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(format!("cannot read token from stdin: {e}")),
            None => return Err("no token provided (EOF on stdin)".to_string()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    uri: Option<String>,
    conduit_uri: Option<String>,
    token: Option<String>,
    force: bool,
}

type ArgIter<'a> = std::iter::Peekable<std::slice::Iter<'a, String>>;

/// One argument's parse step: consumed, `--help`, or a fatal error.
enum Step {
    Handled,
    Help,
    Error(String),
}

struct Parser {
    uri: Option<String>,
    conduit_uri: Option<String>,
    token: Option<String>,
    force: bool,
    positional: u32,
}

impl Parser {
    fn new() -> Self {
        Self {
            uri: None,
            conduit_uri: None,
            token: None,
            force: false,
            positional: 0,
        }
    }

    fn flag_conduit(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Option<Step> {
        if arg != "--conduit-uri" {
            return None;
        }
        let Some(value) = iter.next() else {
            return Some(Step::Error("--conduit-uri requires a value".to_string()));
        };
        self.conduit_uri = Some(value.clone());
        Some(Step::Handled)
    }

    fn flag_token(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Option<Step> {
        if arg != "--token" {
            return None;
        }
        let Some(value) = iter.next() else {
            return Some(Step::Error("--token requires a value".to_string()));
        };
        self.token = Some(value.clone());
        Some(Step::Handled)
    }

    fn push_positional(&mut self, arg: &str) -> Step {
        if self.positional > 0 {
            Step::Error("Specify at most one URI.".to_string())
        } else {
            self.uri = Some(arg.to_string());
            self.positional += 1;
            Step::Handled
        }
    }

    fn consume_rest(&mut self, iter: &mut ArgIter<'_>) -> Option<String> {
        for rest in iter.by_ref() {
            if self.positional > 0 {
                return Some("Specify at most one URI.".to_string());
            }
            self.uri = Some(rest.clone());
            self.positional += 1;
        }
        None
    }

    fn tail(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Step {
        if arg == "--force" {
            self.force = true;
            return Step::Handled;
        }
        if arg == "-h" || arg == "--help" {
            return Step::Help;
        }
        if arg == "--" {
            if let Some(message) = self.consume_rest(iter) {
                return Step::Error(message);
            }
            return Step::Handled;
        }
        if arg.starts_with('-') {
            return Step::Error(format!("unknown flag '{arg}'"));
        }
        self.push_positional(arg)
    }

    fn consume(&mut self, arg: &str, iter: &mut ArgIter<'_>) -> Step {
        if let Some(step) = self.flag_conduit(arg, iter) {
            return step;
        }
        if let Some(step) = self.flag_token(arg, iter) {
            return step;
        }
        self.tail(arg, iter)
    }

    fn finish(self) -> Options {
        Options {
            uri: self.uri,
            conduit_uri: self.conduit_uri,
            token: self.token,
            force: self.force,
        }
    }
}

fn parse_args(argv: &[String]) -> Result<Option<Options>, String> {
    let mut parser = Parser::new();
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        match parser.consume(arg, &mut iter) {
            Step::Handled => {}
            Step::Help => return Ok(None),
            Step::Error(message) => return Err(message),
        }
    }
    Ok(Some(parser.finish()))
}

fn install_options(argv: &[String]) -> Result<Options, ExitCode> {
    match parse_args(argv) {
        Ok(Some(options)) => Ok(options),
        Ok(None) => {
            print!("{USAGE}");
            Err(ExitCode::SUCCESS)
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{USAGE}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn resolve_install_uri(options: &Options, cwd: &Path) -> Result<String, ExitCode> {
    let env_uri = std::env::var("ARC_CONDUIT_URI").ok();
    match resolve_uri(
        options.uri.as_deref(),
        options.conduit_uri.as_deref(),
        env_uri.as_deref(),
        cwd,
    ) {
        Ok(uri) => Ok(uri),
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn install_token_value(options: &Options, uri: &str) -> Result<String, ExitCode> {
    if let Some(token) = &options.token {
        return Ok(token.clone());
    }
    println!("Open this page in your browser and log in if necessary:\n");
    println!("{}\n", token_login_url(uri));
    println!("Then paste the API Token on that page below.");
    print!("Paste API Token from that page: ");
    use std::io::Write;
    if std::io::stdout().flush().is_err() {
        eprintln!("arc-rs {NAME}: failed to write output");
        return Err(ExitCode::FAILURE);
    }
    match prompt_token() {
        Ok(token) => Ok(token),
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            Err(ExitCode::FAILURE)
        }
    }
}

fn install_arcrc() -> Result<PathBuf, ExitCode> {
    match config::arcrc_path() {
        Some(path) => Ok(path),
        None => {
            eprintln!("arc-rs {NAME}: cannot locate ~/.arcrc (HOME/USERPROFILE is not set)");
            Err(ExitCode::FAILURE)
        }
    }
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let options = match install_options(argv) {
        Ok(options) => options,
        Err(code) => return code,
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let uri = match resolve_install_uri(&options, &cwd) {
        Ok(uri) => uri,
        Err(code) => return code,
    };
    let token = match install_token_value(&options, &uri) {
        Ok(token) => token,
        Err(code) => return code,
    };
    println!("Connecting to \"{uri}\"...");
    let arcrc = match install_arcrc() {
        Ok(path) => path,
        Err(code) => return code,
    };
    match install(&uri, &token, &arcrc, options.force) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "install_certificate_tests.rs"]
mod install_certificate_tests;
