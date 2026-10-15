//! `arc-rs install-certificate` — install Conduit credentials.
//!
//! PHP reference: `src/workflow/ArcanistInstallCertificateWorkflow.php`
//! (prompts: `phutil_console_prompt` / `phutil_console_confirm` in
//! `src/console/format.php`; TTY gating: `phutil_console_require_tty`).
//!
//! Flow: resolve the server URI (positional URI, `--conduit-uri`,
//! `ARC_CONDUIT_URI`, or the enclosing `.arcconfig`), print the login page,
//! read one API token, then branch on server capabilities. Modern servers
//! verify the token with `user.whoami` and store `{"token": ...}`; legacy
//! servers exchange it via `conduit.getcertificate` and store
//! `{"user": ..., "cert": ...}`. Everything lands in `~/.arcrc` under
//! `hosts`, written only after every check passes.
//!
//! arc-rs extensions: `--token` skips the interactive prompt and `--force`
//! auto-answers the "standard `api-` token" confirmation. Prompts go
//! through the shared `prompt` module; on EOF the token prompt fails fast
//! instead of re-prompting forever like the PHP `do/while` loop.

extern crate self as arc_rs;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arc_rs::{conduit, config, diff_submit, prompt, terminal};

/// Subcommand name, matching `arc install-certificate`.
pub const NAME: &str = "install-certificate";

const USAGE: &str = "usage: arc-rs install-certificate [URI] [--conduit-uri URI] [--token TOKEN] [--force]\n\
\n\
Install Conduit credentials into ~/.arcrc for URI. Without URI, the URI\n\
comes from --conduit-uri, $ARC_CONDUIT_URI, or the enclosing .arcconfig.\n\
Without --token, the token is read from stdin after printing the login page.\n\
\n\
Options:\n\
  --conduit-uri URI   Conduit server URI (under the positional URI).\n\
  --token TOKEN       API token (else prompt via stdin).\n\
  --force             Install standard api- tokens without prompting.\n\
  -h, --help          Show this help.\n";

const EXAMPLE_URI: &str = "https://devtools.example.com/";

/// `pht('Paste API Token from that page:')`.
const TOKEN_PROMPT: &str = "Paste API Token from that page:";
/// `pht('Install this token anyway?')`.
const CONFIRM_INSTALL_ANYWAY: &str = "Install this token anyway?";
/// `pht('Not installing API token.')`.
const NOT_INSTALLING: &str = "Not installing API token.";

/// Kinds of installable tokens.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// A `cli-` token, the expected kind.
    Cli,
    /// A standard `api-` token, installed only after confirmation.
    Standard,
}

/// A legacy certificate exchanged for a token.
#[derive(Debug, PartialEq, Eq)]
pub struct CertificateInfo {
    pub username: String,
    pub certificate: String,
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

/// Validate a pasted token's shape (`pht` token-auth branch).
pub fn validate_token(token: &str) -> Result<TokenKind, String> {
    if token.len() != 32 {
        return Err(format!(
            "The token \"{token}\" is not formatted correctly. API tokens should be 32 characters long. Make sure you visited the correct URI and copy/pasted the token correctly."
        ));
    }
    if token.starts_with("api-") {
        return Ok(TokenKind::Standard);
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

/// The login page for a normalized URI: `/conduit/login/` on modern
/// servers, `/conduit/token/` on legacy ones.
#[must_use]
pub fn token_login_url(normalized_uri: &str, is_token_auth: bool) -> String {
    let base = normalized_uri
        .strip_suffix("api/")
        .unwrap_or(normalized_uri);
    let path = if is_token_auth {
        "conduit/login/"
    } else {
        "conduit/token/"
    };
    format!("{base}{path}")
}

/// `pht('CONNECT')`, the `writeInfo` title.
#[must_use]
pub fn connect_title() -> &'static str {
    "CONNECT"
}

/// `pht('Connecting to "%s"...', $uri)`.
#[must_use]
pub fn connect_message(uri: &str) -> String {
    format!("Connecting to \"{uri}\"...")
}

/// The `**LOG IN**` header pattern (`phutil_console_format("**%s**\n")`).
#[must_use]
pub fn login_header_markup() -> String {
    "**LOG IN**\n".to_string()
}

/// The login-page block (`"%s\n\n%s\n\n%s"`, no trailing newline).
#[must_use]
pub fn login_page_block(token_url: &str) -> String {
    format!(
        "Open this page in your browser and log in if necessary:\n\n{token_url}\n\nThen paste the API Token on that page below."
    )
}

/// The standard-token notice, echoed verbatim (no trailing newline).
#[must_use]
pub fn standard_token_notice() -> &'static str {
    "You are installing a standard API token, but a CLI API token was expected. If you're writing a script, consider passing the token at runtime with --conduit-token instead of installing it."
}

/// `pht('Downloading authentication certificate...')`.
#[must_use]
pub fn downloading_message() -> &'static str {
    "Downloading authentication certificate..."
}

/// `pht("Installing certificate for '%s'...", $user)`.
#[must_use]
pub fn installing_message(user: &str) -> String {
    format!("Installing certificate for '{user}'...")
}

/// `pht('Writing %s...', '~/.arcrc')`.
#[must_use]
pub fn writing_message() -> &'static str {
    "Writing ~/.arcrc..."
}

/// `pht('SUCCESS!')`, the success banner title.
#[must_use]
pub fn success_title() -> &'static str {
    "SUCCESS!"
}

/// `pht('API Token installed.')` or `pht('Certificate installed.')`.
#[must_use]
pub fn success_message(is_token_auth: bool) -> &'static str {
    if is_token_auth {
        "API Token installed."
    } else {
        "Certificate installed."
    }
}

/// Accept one token-prompt line: trim it, skip blanks (`None` reprompts).
#[must_use]
pub fn accept_token_line(line: &str) -> Option<String> {
    let token = line.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
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

/// The mutable `hosts` object of an `~/.arcrc` document.
fn hosts_section<'a>(
    doc: &'a mut serde_json::Value,
    path: &Path,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, String> {
    let hosts = doc
        .as_object_mut()
        .expect("arcrc document object")
        .entry("hosts".to_string())
        .or_insert_with(|| serde_json::json!({}));
    hosts
        .as_object_mut()
        .ok_or_else(|| format!("'{}' has a non-object \"hosts\" section", path.display()))
}

/// Stage a token entry for `uri` in a live `~/.arcrc` document.
pub fn apply_token_entry(
    doc: &mut serde_json::Value,
    path: &Path,
    uri: &str,
    token: &str,
) -> Result<(), String> {
    hosts_section(doc, path)?.insert(uri.to_string(), serde_json::json!({"token": token}));
    Ok(())
}

/// Stage a legacy certificate entry for `uri` in a live document.
pub fn apply_certificate_entry(
    doc: &mut serde_json::Value,
    path: &Path,
    uri: &str,
    info: &CertificateInfo,
) -> Result<(), String> {
    hosts_section(doc, path)?.insert(
        uri.to_string(),
        serde_json::json!({"user": info.username, "cert": info.certificate}),
    );
    Ok(())
}

/// Write an `~/.arcrc` document (pretty JSON, mode 600 outside Windows),
/// mirroring `writeUserConfigurationFile`.
pub fn write_token_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    let mut text =
        serde_json::to_string_pretty(doc).map_err(|e| format!("cannot encode JSON: {e}"))?;
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

/// Store `token` for `uri` in the `~/.arcrc` at `path`, preserving the
/// rest of the file.
pub fn install_token(path: &Path, uri: &str, token: &str) -> Result<(), String> {
    let mut doc = read_token_doc(path)?;
    apply_token_entry(&mut doc, path, uri, token)?;
    write_token_doc(path, &doc)
}

/// Store a legacy certificate for `uri` in the `~/.arcrc` at `path`.
pub fn install_certificate_entry(
    path: &Path,
    uri: &str,
    info: &CertificateInfo,
) -> Result<(), String> {
    let mut doc = read_token_doc(path)?;
    apply_certificate_entry(&mut doc, path, uri, info)?;
    write_token_doc(path, &doc)
}

/// True when a `conduit.getcapabilities` response advertises token auth.
#[must_use]
pub fn token_auth_supported(capabilities: &serde_json::Value) -> bool {
    capabilities
        .get("authentication")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|methods| methods.iter().any(|m| m.as_str() == Some("token")))
}

/// Ping the server; failures become "failed to connect" errors.
pub fn ping_with(call: &diff_submit::ConduitCall, uri: &str) -> Result<(), String> {
    call("conduit.ping", &serde_json::json!({}))
        .map(|_| ())
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))
}

/// Detect token auth; unreadable capabilities mean a legacy server (the
/// PHP `catch` ignores the failure and keeps `/conduit/token/`).
pub fn detect_token_auth(call: &diff_submit::ConduitCall) -> bool {
    call("conduit.getcapabilities", &serde_json::json!({}))
        .map(|capabilities| token_auth_supported(&capabilities))
        .unwrap_or(false)
}

/// Verify `token` against the server.
pub fn whoami_with(call: &diff_submit::ConduitCall, token: &str) -> Result<(), String> {
    call("user.whoami", &serde_json::json!({})).map(|_| ()).map_err(|e| {
        format!(
            "The token \"{token}\" is not a valid API Token. The server returned this response when trying to use it as a token: {e}"
        )
    })
}

/// Exchange `token` for a legacy certificate (`conduit.getcertificate`).
pub fn exchange_certificate(
    call: &diff_submit::ConduitCall,
    uri: &str,
    token: &str,
) -> Result<CertificateInfo, String> {
    let info = call(
        "conduit.getcertificate",
        &serde_json::json!({"token": token, "host": uri}),
    )?;
    let username = info
        .get("username")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let certificate = info
        .get("certificate")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if username.is_empty() || certificate.is_empty() {
        return Err(format!(
            "Server \"{uri}\" returned an invalid certificate response."
        ));
    }
    Ok(CertificateInfo {
        username,
        certificate,
    })
}

/// Require token auth, failing on legacy servers. Callers that can do
/// the legacy `conduit.getcertificate` exchange use [`detect_token_auth`]
/// instead; this strict gate stays for callers that can not.
pub fn check_token_auth(uri: &str) -> Result<(), String> {
    let client = conduit::Client::new(uri, "")
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    if detect_token_auth(&call) {
        Ok(())
    } else {
        Err(format!("Server \"{uri}\" does not support token auth."))
    }
}

/// Ping the server; failures become "failed to connect" errors.
pub fn ping(uri: &str) -> Result<(), String> {
    let client = conduit::Client::new(uri, "")
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    ping_with(&call, uri)
}

/// Verify `token` against the server.
pub fn whoami(uri: &str, token: &str) -> Result<(), String> {
    let client = conduit::Client::new(uri, token)
        .map_err(|e| format!("Failed to connect to server ({uri}): {e}"))?;
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    whoami_with(&call, token)
}

/// Validate, verify, and store a token-auth `token` for `uri`.
pub fn install_token_flow(
    call: &diff_submit::ConduitCall,
    uri: &str,
    token: &str,
    doc: &mut serde_json::Value,
    path: &Path,
) -> Result<TokenKind, String> {
    let kind = validate_token(token)?;
    whoami_with(call, token)?;
    apply_token_entry(doc, path, uri, token)?;
    Ok(kind)
}

/// Exchange and stage a legacy certificate for `uri`.
pub fn install_certificate_flow(
    call: &diff_submit::ConduitCall,
    uri: &str,
    token: &str,
    doc: &mut serde_json::Value,
    path: &Path,
) -> Result<CertificateInfo, String> {
    let info = exchange_certificate(call, uri, token)?;
    apply_certificate_entry(doc, path, uri, &info)?;
    Ok(info)
}

/// The `writeInfo` CONNECT line (stderr; status blue).
#[must_use]
pub fn render_connect(uri: &str, ansi: bool) -> String {
    terminal::format_log("blue", connect_title(), &connect_message(uri), ansi)
}

/// The `**LOG IN**` header line (stdout).
pub fn render_login_header(ansi: bool) -> Result<String, String> {
    terminal::tsprintf_ansi("**%s**\n", &[terminal::TVal::Text("LOG IN")], ansi)
        .map(|line| line.as_str().to_string())
}

/// The green `** SUCCESS! **` banner (stdout; success green).
#[must_use]
pub fn render_success(is_token_auth: bool, ansi: bool) -> String {
    terminal::format_log(
        "green",
        success_title(),
        success_message(is_token_auth),
        ansi,
    )
}

/// Prompt for a token on stdin, reprompting on blank lines. Echo and
/// TTY gating mirror `phutil_console_prompt`; EOF fails fast instead of
/// re-prompting forever like the PHP `do/while` loop.
pub fn prompt_token() -> Result<String, String> {
    print!("{}", prompt::prompt_text(TOKEN_PROMPT));
    if !prompt::stdin_is_terminal() {
        return Err(prompt::NOT_INTERACTIVE.to_string());
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    loop {
        match prompt::read_answer(&mut input) {
            Some(line) => {
                if let Some(token) = accept_token_line(&line) {
                    return Ok(token);
                }
                print!("{}", prompt::prompt_text(TOKEN_PROMPT));
            }
            None => return Err("no token provided (EOF on stdin)".to_string()),
        }
    }
}

/// Confirm `question` via the shared `phutil_console_confirm` port.
pub fn prompt_confirm(question: &str, default_no: bool) -> Result<bool, String> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    prompt::console_confirm(&mut input, question, default_no)
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

fn fail(message: String) -> ExitCode {
    eprintln!("arc-rs {NAME}: {message}");
    ExitCode::FAILURE
}

fn install_arcrc() -> Result<PathBuf, ExitCode> {
    match config::arcrc_path() {
        Some(path) => Ok(path),
        None => Err(fail(
            "cannot locate ~/.arcrc (HOME/USERPROFILE is not set)".to_string(),
        )),
    }
}

/// Read the token: `--token` (trimmed like prompted input) or the
/// interactive prompt loop.
fn install_token_value(options: &Options) -> Result<String, ExitCode> {
    if let Some(token) = options.token.as_deref().map(str::trim)
        && !token.is_empty()
    {
        return Ok(token.to_string());
    }
    match prompt_token() {
        Ok(token) => Ok(token),
        Err(message) => Err(fail(message)),
    }
}

/// Confirm installing a standard `api-` token (`--force` skips the ask).
fn confirm_standard_token(force: bool) -> Result<(), ExitCode> {
    if force {
        return Ok(());
    }
    print!("{}", standard_token_notice());
    let _ = std::io::stdout().flush();
    match prompt_confirm(CONFIRM_INSTALL_ANYWAY, true) {
        Ok(true) => Ok(()),
        Ok(false) => Err(fail(NOT_INSTALLING.to_string())),
        Err(message) => Err(fail(message)),
    }
}

fn run_token_flow(
    uri: &str,
    token: &str,
    arcrc: &Path,
    doc: &mut serde_json::Value,
    force: bool,
    client: &mut conduit::Client,
) -> Result<(), ExitCode> {
    let kind = match validate_token(token) {
        Ok(kind) => kind,
        Err(message) => return Err(fail(message)),
    };
    if kind == TokenKind::Standard {
        confirm_standard_token(force)?;
    }
    client.set_conduit_token(token);
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    match install_token_flow(&call, uri, token, doc, arcrc) {
        Ok(_) => Ok(()),
        Err(message) => Err(fail(message)),
    }
}

fn run_certificate_flow(
    uri: &str,
    token: &str,
    arcrc: &Path,
    doc: &mut serde_json::Value,
    client: &conduit::Client,
) -> Result<(), ExitCode> {
    println!();
    println!("{}", downloading_message());
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    match install_certificate_flow(&call, uri, token, doc, arcrc) {
        Ok(info) => {
            println!("{}", installing_message(&info.username));
            Ok(())
        }
        Err(message) => Err(fail(message)),
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
    let arcrc = match install_arcrc() {
        Ok(path) => path,
        Err(code) => return code,
    };
    let mut doc = match read_token_doc(&arcrc) {
        Ok(doc) => doc,
        Err(message) => return fail(message),
    };
    let ansi = terminal::ansi_enabled();
    eprint!("{}", render_connect(&uri, ansi));
    let mut client = match conduit::Client::new(&uri, "") {
        Ok(client) => client,
        Err(e) => return fail(format!("Failed to connect to server ({uri}): {e}")),
    };
    let call = |method: &str, params: &serde_json::Value| {
        client.call(method, params).map_err(|e| e.to_string())
    };
    if let Err(message) = ping_with(&call, &uri) {
        return fail(message);
    }
    let is_token_auth = detect_token_auth(&call);
    match render_login_header(ansi) {
        Ok(header) => print!("{header}"),
        Err(message) => return fail(message),
    }
    let token_url = token_login_url(&uri, is_token_auth);
    println!("{}", login_page_block(&token_url));
    let token = match install_token_value(&options) {
        Ok(token) => token,
        Err(code) => return code,
    };
    let flow = if is_token_auth {
        run_token_flow(&uri, &token, &arcrc, &mut doc, options.force, &mut client)
    } else {
        run_certificate_flow(&uri, &token, &arcrc, &mut doc, &client)
    };
    if let Err(code) = flow {
        return code;
    }
    println!("{}", writing_message());
    if let Err(message) = write_token_doc(&arcrc, &doc) {
        return fail(message);
    }
    print!("{}", render_success(is_token_auth, ansi));
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "install_certificate_tests.rs"]
mod install_certificate_tests;
