//! Blocking Conduit API client, mirroring `src/conduit/ConduitClient.php`.
//!
//! Requests are form posts: `params` carries the method parameters plus the
//! `__conduit__` auth metadata as one JSON blob, with `output=json`.
//! Responses may carry the `for(;;);` shield and an
//! `X-Conduit-Capabilities` header (mirroring `ConduitFuture.php`).

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

// PHP parity: `ConduitClient::$timeout` defaults to 300 seconds.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
// PHP parity: `ConduitClient::SIGNATURE_CONSIGN_1`.
const SIGNATURE_CONSIGN_1: &str = "Consign1.0/";
// PHP parity: `ConduitFuture` strips this shield before parsing JSON.
const RESPONSE_SHIELD: &str = "for(;;);";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    Protocol(String),
    Api { code: String, info: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // T1153 CORE-6: PHP prints conduit failures with no `transport
        // error:` / `protocol error:` / `conduit error` infix — the raw
        // exception message goes under the envelope banner (subprocess)
        // or into the fatal trace (toolset runtime). API failures render
        // exactly like `ConduitClientException` (`"{code}: {info}"`,
        // `src/conduit/ConduitClientException.php:7-9` @ ceb — no
        // `<method>` infix; T1153 CORE-1 dropped the newer-baseline
        // infix from `parse_response` so `info` is the raw `error_info`).
        match self {
            Error::Transport(msg) => write!(f, "{msg}"),
            Error::Protocol(msg) => write!(f, "{msg}"),
            Error::Api { code, info } => write!(f, "{code}: {info}"),
        }
    }
}

impl std::error::Error for Error {}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("base", &self.base).finish()
    }
}

#[derive(Debug, Default)]
struct SessionState {
    session_key: Option<String>,
    connection_id: Option<String>,
    capabilities: BTreeSet<String>,
    auth_user: Option<(String, String)>,
}

#[derive(Debug)]
struct SigningKeys {
    public_key: String,
    private_pem: String,
}

/// Base HTTP client builder: the 300s PHP timeout with default proxy
/// handling. T1153 CORE-7: PHP's conduit stack is cURL-based, which
/// honors `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` on its own (no arc
/// extension provides a proxy; `HTTPSFuture.php:230` only consults
/// extensions and applies the result). reqwest's default proxy matcher
/// implements the same curl-compatible env rules, so no `.no_proxy()`.
fn http_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder().timeout(REQUEST_TIMEOUT)
}

/// TLS options for [`Client::new_with_tls`], mirroring the
/// `scripts/arcanist.php` startup reads of `https.cabundle` (a global
/// CA bundle path, resolved against the project root) and
/// `https.blindly-trust-domains` (peer verification off for listed
/// domains via `ArcanistBlindlyTrustHTTPEngineExtension`).
#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    /// PEM CA bundle path (`https.cabundle`), already resolved.
    pub cabundle: Option<std::path::PathBuf>,
    /// Lowercased blind-trust domains (`https.blindly-trust-domains`).
    pub blind_trust: Vec<String>,
}

impl TlsConfig {
    /// True when `uri`'s domain is blind-trusted. Both sides compare
    /// lowercased, mirroring `phutil_utf8_strtolower()` in the
    /// extension's setter and matcher.
    #[must_use]
    pub fn trusts_domain(&self, uri: &str) -> bool {
        let domain = uri
            .split("://")
            .nth(1)
            .unwrap_or(uri)
            .split('/')
            .next()
            .unwrap_or("")
            .rsplit('@')
            .next()
            .unwrap_or("");
        let domain = domain
            .rsplit_once(':')
            .map_or(domain, |(host, _)| host)
            .to_ascii_lowercase();
        self.blind_trust.iter().any(|d| d == &domain)
    }
}

/// Apply [`TlsConfig`] to a client builder. A missing/unreadable bundle
/// fails here (PHP fails at connect time instead, but construction
/// always immediately precedes the first call, so the observable is a
/// transport-class failure either way). Blind trust maps to
/// `danger_accept_invalid_certs`: reqwest cannot express PHP's exact
/// "peer off, hostname on" split (`CURLOPT_SSL_VERIFYHOST` stays `2`
/// in `HTTPSFuture`), so listed domains skip both — the closest
/// equivalent for an explicitly trust-anything list.
fn tls_client_builder(
    tls: &TlsConfig,
    uri: &str,
) -> Result<reqwest::blocking::ClientBuilder, Error> {
    let mut builder = http_client_builder();
    if let Some(path) = &tls.cabundle {
        let pem = std::fs::read(path)
            .map_err(|e| Error::Transport(format!("failed to read CA bundle: {e}")))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| Error::Transport(format!("invalid CA bundle: {e}")))?;
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }
    if tls.trusts_domain(uri) {
        builder = builder.danger_accept_invalid_certs(true);
    }
    Ok(builder)
}

pub struct Client {
    // NOTE: no `Debug` derive on purpose (tokens and keys must never print).
    base: String,
    scheme: String,
    host: String,
    host_custom: bool,
    port: Option<u16>,
    token: String,
    oauth_token: Option<String>,
    basic_auth: Option<(String, String)>,
    timeout: Duration,
    http: reqwest::blocking::Client,
    state: Mutex<SessionState>,
    signing: Option<SigningKeys>,
}

impl Client {
    pub fn new(conduit_uri: &str, token: &str) -> Result<Self, Error> {
        let (scheme, host, port) = split_uri(conduit_uri)?;
        let http = http_client_builder()
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::from_parts(
            conduit_uri,
            token,
            scheme,
            host,
            port,
            http,
        ))
    }

    /// [`Client::new`] with [`TlsConfig`] (`https.cabundle` /
    /// `https.blindly-trust-domains`, T1153 CORE-8).
    pub fn new_with_tls(conduit_uri: &str, token: &str, tls: &TlsConfig) -> Result<Self, Error> {
        let (scheme, host, port) = split_uri(conduit_uri)?;
        let http = tls_client_builder(tls, conduit_uri)?
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self::from_parts(
            conduit_uri,
            token,
            scheme,
            host,
            port,
            http,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        conduit_uri: &str,
        token: &str,
        scheme: String,
        host: String,
        port: Option<u16>,
        http: reqwest::blocking::Client,
    ) -> Self {
        Self {
            base: normalize_base(conduit_uri),
            scheme,
            host,
            host_custom: false,
            port,
            token: token.to_string(),
            oauth_token: None,
            basic_auth: None,
            timeout: REQUEST_TIMEOUT,
            http,
            state: Mutex::new(SessionState::default()),
            signing: None,
        }
    }

    /// The normalized API base URI (for absolutizing relative URIs).
    #[must_use]
    pub fn base_uri(&self) -> &str {
        &self.base
    }

    /// Whether the client carries a token (`getConduitToken()`
    /// non-null on the engine).
    #[must_use]
    pub fn has_token(&self) -> bool {
        !self.token.is_empty()
    }

    /// Record the authenticated identity, mirroring the workflow's
    /// cached `userName`/`userPHID` after `authenticateConduit()`
    /// (T1153 RB-D8: PHP resolves whoami once first, then reuses it).
    pub fn set_authenticated_user(&self, user: &str, phid: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.auth_user = Some((user.to_string(), phid.to_string()));
    }

    /// The cached authenticated `(user_name, user_phid)`, if any.
    #[must_use]
    pub fn authenticated_user(&self) -> Option<(String, String)> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .auth_user
            .clone()
    }

    /// Translate a toolset API failure into login guidance, if any
    /// (T1153 CORE-5): the `ArcanistConduitCallFuture` mapping turns
    /// `ERR-INVALID-SESSION` (tokenless engine) and `ERR-INVALID-AUTH`
    /// into guidance rendered with [`report_auth`]. Returns `None`
    /// when `error` is not a mapped auth failure (classic workflows
    /// never call this: their raw client raises unmapped errors).
    #[must_use]
    pub fn auth_guidance_for(&self, error: &Error) -> Option<AuthGuidance> {
        match error {
            Error::Api { code, .. } => auth_guidance(code, self.has_token(), &self.base),
            _ => None,
        }
    }

    /// Override the host identity, mirroring `ConduitClient::setHost()`.
    pub fn set_host(&mut self, host: &str) -> &mut Self {
        self.host = host.to_string();
        self.host_custom = true;
        self
    }

    pub fn set_conduit_token(&mut self, token: &str) -> &mut Self {
        self.token = token.to_string();
        self
    }

    pub fn set_oauth_token(&mut self, token: &str) -> &mut Self {
        self.oauth_token = Some(token.to_string());
        self
    }

    pub fn set_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = timeout;
        self
    }

    pub fn set_basic_auth_credentials(&mut self, username: &str, password: &str) -> &mut Self {
        self.basic_auth = Some((username.to_string(), password.to_string()));
        self
    }

    /// Enable asymmetric request signing, mirroring
    /// `ConduitClient::setSigningKeys()`. Like PHP, the key is stored blindly;
    /// a bad key fails at signing time.
    pub fn set_signing_keys(&mut self, public_key: &str, private_pem: &str) -> &mut Self {
        self.signing = Some(SigningKeys {
            public_key: public_key.to_string(),
            private_pem: private_pem.to_string(),
        });
        self
    }

    /// Union server-advertised capabilities, mirroring
    /// `ConduitClient::enableCapabilities()`.
    pub fn enable_capabilities(&self, capabilities: &[String]) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.capabilities.extend(capabilities.iter().cloned());
    }

    pub fn call(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        let mut map = call_params_map(params)?;
        if method == "conduit.connect" {
            apply_connect_auth(&mut map);
        }
        let (body, gzipped) = self.encode_call_body(method, &mut map)?;
        let text = self.post_call_body(method, body, gzipped)?;
        let result = parse_response(&text, method)?;
        if method == "conduit.connect" {
            self.store_session(&result);
        }
        Ok(result)
    }

    fn encode_call_body(
        &self,
        method: &str,
        map: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(Vec<u8>, bool), Error> {
        let meta = self.build_meta(method, map)?;
        map.insert("__conduit__".to_string(), serde_json::Value::Object(meta));
        let json = serde_json::to_string(&map).map_err(json_protocol_error)?;
        // PHP `json_encode()` escapes `/` as `\/`; serde does not.
        let json = json.replace('/', "\\/");
        let query = build_query(&json);
        let gzipped = self.uses_gzip();
        let body = if gzipped {
            gzip_bytes(query.as_bytes())
        } else {
            query.into_bytes()
        };
        Ok((body, gzipped))
    }

    fn post_call_body(&self, method: &str, body: Vec<u8>, gzipped: bool) -> Result<String, Error> {
        let url = format!("{}{}", self.base, method);
        // PHP sends `Content-Type: application/x-www-form-urlencoded` (cURL
        // default for POST fields) with the raw querystring body.
        let mut request = self
            .http
            .post(&url)
            .timeout(self.timeout)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body);
        if gzipped {
            request = request.header(reqwest::header::CONTENT_ENCODING, "gzip");
        }
        if self.host_custom {
            request = request.header(reqwest::header::HOST, self.host_for_header());
        }
        if let Some((username, password)) = &self.basic_auth {
            request = request.basic_auth(username, Some(password));
        }
        let response = request.send().map_err(transport_error)?;
        check_call_status(response.status())?;
        self.absorb_capabilities(&response);
        response.text().map_err(transport_error)
    }

    fn absorb_capabilities(&self, response: &reqwest::blocking::Response) {
        if let Some(raw) = response.headers().get("x-conduit-capabilities")
            && let Ok(text) = raw.to_str()
        {
            let caps: Vec<String> = text.split(' ').map(str::to_string).collect();
            self.enable_capabilities(&caps);
        }
    }

    fn build_meta(
        &self,
        method: &str,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, Error> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut meta = serde_json::Map::new();
        if let Some(key) = &state.session_key {
            meta.insert("sessionKey".to_string(), json_string(key));
        }
        if let Some(id) = &state.connection_id {
            meta.insert("connectionID".to_string(), json_string(id));
        }
        drop(state);
        if let Some(signing) = &self.signing {
            meta.insert("auth.type".to_string(), json_string("asymmetric"));
            meta.insert("auth.key".to_string(), json_string(&signing.public_key));
            meta.insert(
                "auth.host".to_string(),
                json_string(&self.host_for_signature()),
            );
            let signature = sign_request(method, params, &meta, &signing.private_pem)?;
            meta.insert("auth.signature".to_string(), json_string(&signature));
        }
        if !self.token.is_empty() {
            meta.insert("token".to_string(), json_string(&self.token));
        }
        if let Some(oauth) = &self.oauth_token {
            meta.insert("access_token".to_string(), json_string(oauth));
        }
        Ok(meta)
    }

    fn uses_gzip(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capabilities
            .contains("gzip")
    }

    fn store_session(&self, result: &serde_json::Value) {
        let Some(obj) = result.as_object() else {
            return;
        };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.session_key = obj
            .get("sessionKey")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        state.connection_id = obj
            .get("connectionID")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }

    /// Header form: the default port for the scheme is implicit, mirroring
    /// `ConduitClient::newHostString(false)`.
    fn host_for_header(&self) -> String {
        match self.port {
            Some(port) if port != implicit_port(&self.scheme) => {
                format!("{}:{port}", self.host)
            }
            _ => self.host.clone(),
        }
    }

    /// Signature form: the port is always explicit, mirroring
    /// `ConduitClient::newHostString(true)`.
    fn host_for_signature(&self) -> String {
        let port = self.port.unwrap_or_else(|| implicit_port(&self.scheme));
        format!("{}:{port}", self.host)
    }
}

fn json_string(text: &str) -> serde_json::Value {
    serde_json::Value::String(text.to_string())
}

fn implicit_port(scheme: &str) -> u16 {
    if scheme == "https" { 443 } else { 80 }
}

fn normalize_base(uri: &str) -> String {
    // T1153 CORE-3: PHP replaces the whole path (`setPath('/api/'.$method)`
    // per call; `setPath('/api/')` at startup), so a configured path like
    // `http://host:port/foo` posts to `/api/<method>`, never
    // `/foo/api/<method>`. Keep only scheme + authority here.
    let after_scheme = uri.split("://").nth(1).unwrap_or(uri);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let scheme = uri.split("://").next().unwrap_or("https");
    format!("{scheme}://{authority}/api/")
}

/// Split a Conduit URI into (scheme, host, port), mirroring the
/// `PhutilURI` validation in `ConduitClient::__construct()`.
fn split_uri(uri: &str) -> Result<(String, String, Option<u16>), Error> {
    let invalid = || Error::Transport(format!("Conduit URI '{uri}' must include a valid host."));
    let mut parts = uri.splitn(2, "://");
    let scheme = parts.next().unwrap_or("").to_ascii_lowercase();
    let after_scheme = parts.next().ok_or_else(invalid)?;
    let authority = after_scheme.split('/').next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or("");
    if authority.is_empty() {
        return Err(invalid());
    }
    let (host, port) = split_authority(authority);
    if host.is_empty() {
        return Err(invalid());
    }
    Ok((scheme, host, port.filter(|port| *port != 0)))
}

fn split_authority(authority: &str) -> (String, Option<u16>) {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return (authority.to_string(), None);
        };
        let port = rest[end + 1..].strip_prefix(':').and_then(parse_port);
        return (rest[..end].to_string(), port);
    }
    match authority.rsplit_once(':') {
        Some((host, raw_port)) => match parse_port(raw_port) {
            Some(port) => (host.to_string(), Some(port)),
            None => (authority.to_string(), None),
        },
        None => (authority.to_string(), None),
    }
}

fn parse_port(raw: &str) -> Option<u16> {
    if raw.is_empty() || raw.len() > 5 {
        return None;
    }
    raw.parse::<u16>().ok().filter(|_| {
        raw.bytes().all(|b| b.is_ascii_digit()) && !(raw.len() > 1 && raw.starts_with('0'))
    })
}

/// `conduit.connect` certificate handshake: swap the certificate for an
/// `authToken`/`authSignature` pair, mirroring `ConduitClient::callMethod()`.
fn apply_connect_auth(map: &mut serde_json::Map<String, serde_json::Value>) {
    let certificate = map.get("certificate").cloned().unwrap_or_default();
    if php_truthy(&certificate) {
        let token = unix_time();
        map.insert(
            "authToken".to_string(),
            serde_json::Value::Number(token.into()),
        );
        let signature = connect_signature(&token.to_string(), &php_stringify(&certificate));
        map.insert("authSignature".to_string(), json_string(&signature));
    }
    map.remove("certificate");
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `sha1($token.$certificate)` in hex, mirroring PHP's default hex digest.
fn connect_signature(token: &str, certificate: &str) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(token.as_bytes());
    hasher.update(certificate.as_bytes());
    hex_bytes(&hasher.finalize())
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

fn php_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => {
            number.as_i64().is_none_or(|n| n != 0)
                && number.as_u64().is_none_or(|n| n != 0)
                && number.as_f64().is_none_or(|n| n != 0.0)
        }
        serde_json::Value::String(text) => !text.is_empty() && text != "0",
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}

fn php_stringify(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(true) => "1".to_string(),
        serde_json::Value::Bool(false) => String::new(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => "Array".to_string(),
    }
}

/// `rawurlencode()`: unreserved bytes pass through, everything else becomes
/// uppercase `%XX`, mirroring `phutil_build_http_querystring()`.
fn rawurlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn call_params_map(
    params: &serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>, Error> {
    match params {
        serde_json::Value::Object(map) => Ok(map.clone()),
        _ => Err(Error::Protocol("params must be a JSON object".to_string())),
    }
}

fn transport_error(error: reqwest::Error) -> Error {
    Error::Transport(error.to_string())
}

fn json_protocol_error(error: serde_json::Error) -> Error {
    Error::Protocol(error.to_string())
}

fn check_call_status(status: reqwest::StatusCode) -> Result<(), Error> {
    if (200..300).contains(&status.as_u16()) {
        Ok(())
    } else {
        Err(Error::Transport(format!(
            "Host returned HTTP/{} in response to a Conduit method call.",
            status.as_u16()
        )))
    }
}

fn build_query(params_json: &str) -> String {
    format!(
        "params={}&output=json&__conduit__=1",
        rawurlencode(params_json)
    )
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).expect("gzip to memory cannot fail");
    encoder.finish().expect("gzip to memory cannot fail")
}

/// Canonical request encoding for asymmetric signatures, mirroring
/// `ConduitClient::encodeRawDataForSignature()` (objects sort by key).
pub fn encode_raw_data_for_signature(value: &serde_json::Value) -> Result<String, Error> {
    match value {
        serde_json::Value::Array(items) => {
            let mut out = format!("A{}:", items.len());
            for item in items {
                out.push_str(&encode_raw_data_for_signature(item)?);
            }
            Ok(out)
        }
        serde_json::Value::Object(map) => {
            let mut out = format!("O{}:", map.len());
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                out.push_str(&encode_signature_string(key));
                out.push_str(&encode_raw_data_for_signature(&map[key])?);
            }
            Ok(out)
        }
        serde_json::Value::String(text) => Ok(encode_signature_string(text)),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Ok(encode_signature_int(&int.to_string()))
            } else if let Some(uint) = number.as_u64() {
                Ok(encode_signature_int(&uint.to_string()))
            } else {
                unexpected_signature_type("double")
            }
        }
        serde_json::Value::Null => Ok("N:".to_string()),
        serde_json::Value::Bool(true) => Ok("B1:".to_string()),
        serde_json::Value::Bool(false) => Ok("B0:".to_string()),
    }
}

fn encode_signature_string(text: &str) -> String {
    format!("S{}:{text}", text.len())
}

fn encode_signature_int(digits: &str) -> String {
    format!("I{}:{digits}", digits.len())
}

fn unexpected_signature_type(got: &str) -> Result<String, Error> {
    Err(Error::Protocol(format!(
        "Unexpected data type in request data: {got}."
    )))
}

/// RSA/SHA1 request signing, mirroring `ConduitClient::signRequest()`
/// (`openssl_sign()` defaults to SHA1 with PKCS#1 v1.5 padding).
fn sign_request(
    method: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    meta: &serde_json::Map<String, serde_json::Value>,
    private_pem: &str,
) -> Result<String, Error> {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs8::DecodePrivateKey;
    use sha1::Digest;
    let sign_failed =
        || Error::Protocol("Unable to sign Conduit request with signing key.".to_string());
    let structure = serde_json::json!({
        "method": method,
        "protocol": meta,
        "parameters": params,
    });
    let input = encode_raw_data_for_signature(&structure)?;
    let key = rsa::RsaPrivateKey::from_pkcs1_pem(private_pem)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs8_pem(private_pem))
        .map_err(|_| sign_failed())?;
    let padding = rsa::Pkcs1v15Sign::new::<sha1::Sha1>();
    let digest = sha1::Sha1::digest(input.as_bytes());
    let signature = key.sign(padding, &digest).map_err(|_| sign_failed())?;
    use base64::Engine;
    let bytes: Box<[u8]> = signature.into();
    Ok(format!(
        "{SIGNATURE_CONSIGN_1}{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.as_ref())
    ))
}

fn parse_response(text: &str, method: &str) -> Result<serde_json::Value, Error> {
    let raw = text.strip_prefix(RESPONSE_SHIELD).unwrap_or(text);
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| {
        Error::Protocol(
            "Host returned HTTP/200, but invalid JSON data in response to a Conduit method call."
                .to_string(),
        )
    })?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Protocol("response is not an object".to_string()))?;
    if has_error_code(obj.get("error_code")) {
        let info = obj
            .get("error_info")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let code = obj
            .get("error_code")
            .and_then(|v| v.as_str())
            .unwrap_or("ERR-UNKNOWN")
            .to_string();
        // T1153 CORE-1: at ceb `ConduitClientException` is exactly
        // `"{code}: {info}"` (`ConduitClientException.php:7-9`) — the
        // `<method>` infix is a newer-baseline artifact. `Display`
        // adds the code prefix; `info` stays the raw `error_info`.
        let _ = method;
        return Err(Error::Api { code, info });
    }
    Ok(obj
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

/// PHP `if ($data['error_code'])`: only truthy codes are failures.
fn has_error_code(code: Option<&serde_json::Value>) -> bool {
    code.is_some_and(php_truthy)
}

/// Login guidance for a failed toolset call, mirroring
/// `ArcanistConduitAuthenticationException` (T1153 CORE-5): the
/// `ArcanistConduitCallFuture::didReceiveException()` mapping
/// (`src/conduit/ArcanistConduitCallFuture.php:74-88`) turns
/// `ERR-INVALID-SESSION` (tokenless engine) and `ERR-INVALID-AUTH`
/// into this title + multi-part body, rendered by the toolset
/// runtime's fourth catch arm (`ArcanistRuntime.php:42-44`) via
/// `writeError($title, $body)` and exiting 1. The original server
/// error is discarded, not chained, like PHP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthGuidance {
    /// `LOGIN REQUIRED` / `INVALID CREDENTIALS` label.
    pub title: String,
    /// One-line summary (`ConduitClientException`-style message).
    pub summary: String,
    /// Wrapped paragraphs + install command block.
    pub body: String,
}

/// Map an API error code to login guidance, if any. `has_token` is
/// whether the engine carries a token (`getConduitToken()`); `uri` is
/// the engine URI (displayed with path `/`, domain extracted for the
/// prose). Classic workflows never call this: their raw
/// `ConduitClient` raises unmapped `ConduitClientException`s, and
/// their startup gate lives in `config::require_auth_*`.
#[must_use]
pub fn auth_guidance(code: &str, has_token: bool, uri: &str) -> Option<AuthGuidance> {
    // `didReceiveException()`: a tokenless `ERR-INVALID-SESSION` needs
    // login; with a token the original error is rethrown (`break`).
    // `ERR-INVALID-AUTH` always maps.
    let login = match code {
        "ERR-INVALID-SESSION" if !has_token => true,
        "ERR-INVALID-AUTH" => false,
        _ => return None,
    };
    let (domain, install_uri) = guidance_uri_parts(uri);
    let summary = if login {
        "Conduit API login required."
    } else {
        "Invalid Conduit API credentials."
    };
    let problem = if login {
        format!(
            "You are trying to connect to a server (\"{domain}\") that you do not have any \
             stored credentials for, but the command you are running requires authentication."
        )
    } else {
        format!(
            "Your stored credentials for the server you are trying to connect to (\"{domain}\") \
             are not valid."
        )
    };
    let remedy = if login {
        "To log in and save credentials for this server, run this command:"
    } else {
        "To log in and save valid credentials for this server, run this command:"
    };
    let install = format!("arc install-certificate {}", shell_arg(&install_uri));
    let body = format!(
        "{}{}{}",
        tsprintf_wrapped("\n\n%W\n\n", &problem),
        tsprintf_wrapped("%W\n\n", remedy),
        tsprintf_wrapped("%>", &install),
    );
    Some(AuthGuidance {
        title: if login {
            "LOGIN REQUIRED".to_string()
        } else {
            "INVALID CREDENTIALS".to_string()
        },
        summary: summary.to_string(),
        body,
    })
}

/// Render one guidance part through `tsprintf` with the process ANSI
/// setting (`%W` wraps, `%>` indents as a shell command).
fn tsprintf_wrapped(pattern: &str, text: &str) -> String {
    crate::terminal::tsprintf(pattern, &[crate::terminal::TVal::Text(text)])
        .map(|t| t.to_string())
        .unwrap_or_default()
}

/// Split an engine URI into the prose domain and the install-command
/// URI (`getConduitURI()` clones the engine URI and sets path `/`).
/// The domain excludes any port (`PhutilURI::getDomain()`); the
/// install URI keeps the full authority.
fn guidance_uri_parts(uri: &str) -> (String, String) {
    let after_scheme = uri.split("://").nth(1).unwrap_or(uri);
    let authority = after_scheme.split('/').next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let (domain, _) = split_authority(hostport);
    let scheme = uri.split("://").next().unwrap_or("https");
    (domain, format!("{scheme}://{authority}/"))
}

/// `csprintf` `%s` escaping (`PhutilCommandString::escapeLinux()`):
/// safe shell words pass through, anything else is single-quoted like
/// `escapeshellarg()`.
fn shell_arg(value: &str) -> String {
    let safe = !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b':' | b'/' | b'@' | b'.' | b'_' | b'+' | b'-')
        });
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Report login guidance like the toolset runtime's auth arm: the red
/// title label plus the body on stderr, exit 1. The body passes as
/// terminal text (PHP's body is an array of `PhutilTerminalString`,
/// which `%s` casts raw), so its newlines render instead of escaping
/// to `<LF>`.
pub fn report_auth(title: &str, body: &str) -> std::process::ExitCode {
    let body = crate::terminal::TerminalString::new(body.to_string());
    let text = crate::terminal::tsprintf(
        "**<bg:red> %s </bg>** %s\n",
        &[
            crate::terminal::TVal::Text(title),
            crate::terminal::TVal::Terminal(&body),
        ],
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    eprint!("{text}");
    std::process::ExitCode::FAILURE
}

/// The auth error code when `message` is an API failure Display
/// (`"{code}: {info}"`), for toolset fail arms whose conduit errors
/// arrive stringified (T1153 CORE-5). Only prefix-matching: PHP maps
/// by code in every toolset context, so a leading auth code is always
/// a mapped failure; wrapped messages fall through to the fatal arm
/// as before.
#[must_use]
pub fn auth_error_code(message: &str) -> Option<&str> {
    ["ERR-INVALID-SESSION:", "ERR-INVALID-AUTH:"]
        .into_iter()
        .find(|prefix| message.starts_with(prefix))
        .map(|prefix| prefix.trim_end_matches(':'))
}

/// Report a stringified toolset conduit failure (T1153 CORE-5): when
/// `message` carries a code [`auth_guidance`] maps for `client`,
/// render login guidance (exit 1); else `None` and the caller keeps
/// its fatal arm. `client` is the engine the calls went out on (its
/// token presence selects the mapping, its URI feeds the hint).
#[must_use]
pub fn auth_report_for(client: &Client, message: &str) -> Option<std::process::ExitCode> {
    let code = auth_error_code(message)?;
    let guidance = auth_guidance(code, client.has_token(), client.base_uri())?;
    Some(report_auth(&guidance.title, &guidance.body))
}

/// The toolset no-URI error text: `ArcanistConduitEngine::raiseURIException()`
/// throws the `PhutilConsoleBlock` string (paragraph + 3-item list) as
/// `ArcanistUsageException`, which escapes the toolset runtime into a
/// fatal (255). Byte layout follows `drawView`/`drawLines`: wrapped
/// paragraph, blank line, `      - ` items, trailing blank line.
#[must_use]
pub fn no_uri_block() -> String {
    let para =
        "This command needs to communicate with Phabricator, but no Phabricator URI is configured.";
    let mut out = crate::terminal::console_wrap(para, 0, true);
    out.push_str("\n\n");
    for item in [
        "Run in a working copy with \"phabricator.uri\" set in \".arcconfig\".",
        "Set a default URI with `arc set-config phabricator.uri <uri>`.",
        "Specify a URI explicitly with `--config phabricator.uri=<uri>`.",
    ] {
        out.push_str("      - ");
        out.push_str(&crate::terminal::console_wrap(item, 8, false));
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Map a tokenless-engine setup failure to its toolset report text:
/// the no-URI block for missing URIs, anything else unchanged (it
/// already reads as a fatal-class message).
#[must_use]
pub fn toolset_setup_message(message: &str) -> String {
    if message.starts_with("no conduit URI") {
        no_uri_block()
    } else {
        message.to_string()
    }
}

/// Fetch every page of a new-style `.search` call through the
/// caller-supplied `call` seam, following `cursor.after` like
/// `ConduitSearchFuture` (`src/conduit/ConduitSearchFuture.php:50-87`):
/// POST the base `params` (`{constraints, attachments?}`), then re-POST
/// with a top-level `after` cursor until `cursor.after` is null. Returns
/// the raw page results so each caller keeps its own `data` handling.
/// This is the [`ConduitCall`](crate::diff_submit::ConduitCall) counterpart
/// of [`conduit_search`](crate::revision::conduit_search).
pub fn search_pages(
    call: &crate::diff_submit::ConduitCall<'_>,
    method: &str,
    params: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, String> {
    let mut pages = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut page_params = params.clone();
        if let Some(cursor) = &after {
            page_params["after"] = serde_json::Value::String(cursor.clone());
        }
        let result = call(method, &page_params)?;
        let cursor = result
            .get("cursor")
            .and_then(|value| value.get("after"))
            .and_then(|value| value.as_str().map(str::to_string));
        pages.push(result);
        match cursor {
            Some(next) => after = Some(next),
            None => return Ok(pages),
        }
    }
}

/// Exit code for an uncaught toolset error. `ArcanistRuntime::execute()`
/// catches four exception types; anything else (including
/// `ArcanistUsageException`) escapes into a PHP fatal, which exits 255
/// (`src/runtime/ArcanistRuntime.php:18-50`,
/// `support/init/init-arcanist.php`).
pub const EXIT_FATAL: u8 = 255;

/// Error class for the T1153 R4 exit envelope, the one shared port of
/// the two ceb dispatch envelopes. Toolset workflows (those extending
/// `ArcanistArcWorkflow`: land, liberate, browse, download, upload,
/// weld, look, inspect, branches, markers, ...) run in-process under
/// `ArcanistRuntime::execute()`; classic workflows (lint, unit,
/// linters, export, set-config, which, list, tasks, todo, ...) run in a
/// `scripts/arcanist.php` subprocess via `phutil_passthru`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeKind {
    /// `ArcanistNoEffectException`: message on stdout, exit 0
    /// (`scripts/arcanist.php:428-430`).
    NoEffect,
    /// Subprocess `ArcanistUsageException`: `Usage Exception: {msg}`
    /// on stderr, exit 1, no usage dump (`:432-444`).
    Usage,
    /// Subprocess generic `Exception`: red banner, message chain,
    /// `--trace` hint on stderr, exit 1 (`:449-468`).
    Exception,
    /// Toolset `PhutilArgumentUsageException`: red `USAGE EXCEPTION`
    /// label on stderr, exit 1 (`ArcanistRuntime.php:38-39`).
    ToolsetUsage,
    /// Toolset `ArcanistConduitException`: red `CONDUIT` label on
    /// stderr, exit 1 (`ArcanistRuntime.php:36-37`). No ceb throw
    /// sites exist (only the never-thrown
    /// `ArcanistNoURIConduitException` subclass); the arm exists for
    /// completeness and nothing routes to it.
    ToolsetConduit,
    /// Toolset `ArcanistUserAbortException`: red `---` label on
    /// stderr, exit 1 (`ArcanistRuntime.php:40-41`).
    UserAbort,
    /// Toolset workflow-argument parse error (`PhutilArgumentParser::parse()`
    /// catch): bold `**Usage Exception:**` like [`Self::Usage`], exit 1.
    /// Unlike [`Self::Usage`] this arm never rethrows under `--trace`:
    /// the toolset parser catches with or without trace mode, while only
    /// the classic subprocess rethrows (`scripts/arcanist.php:443-446`).
    /// (T1153 CORE-16/RB-D1.)
    ToolsetParseError,
    /// Anything uncaught in the toolset runtime: the raw message on
    /// stderr, exit 255. PHP prints a fatal trace with file paths and
    /// line numbers, which is unportable; the message plus the 255
    /// exit are the stable observables.
    Fatal,
}

impl EnvelopeKind {
    /// Process exit code for this error class.
    #[must_use]
    pub fn exit_code(self) -> std::process::ExitCode {
        match self {
            Self::NoEffect => std::process::ExitCode::SUCCESS,
            Self::Fatal => std::process::ExitCode::from(EXIT_FATAL),
            Self::Usage
            | Self::Exception
            | Self::ToolsetUsage
            | Self::ToolsetConduit
            | Self::ToolsetParseError
            | Self::UserAbort => std::process::ExitCode::FAILURE,
        }
    }

    /// True when the rendering goes to stdout (only [`Self::NoEffect`]).
    #[must_use]
    pub fn uses_stdout(self) -> bool {
        matches!(self, Self::NoEffect)
    }

    /// Render the envelope body (with trailing newline) for `message`.
    #[must_use]
    pub fn render(self, message: &str, ansi: bool) -> String {
        match self {
            Self::NoEffect => format!("{message}\n"),
            Self::Usage => crate::terminal::console_format(
                "**%s** %s\n",
                &["Usage Exception:", rtrim_php(message)],
                ansi,
            ),
            Self::Exception => format!(
                "{}{message}\n{}",
                crate::terminal::console_format("<bg:red>** %s **</bg>\n", &["Exception"], ansi),
                crate::terminal::console_format(
                    "(%s)\n",
                    &["Run with `--trace` for a full exception trace."],
                    ansi,
                ),
            ),
            Self::ToolsetUsage => {
                crate::terminal::format_log("red", "USAGE EXCEPTION", message, ansi)
            }
            Self::ToolsetConduit => crate::terminal::format_log("red", "CONDUIT", message, ansi),
            Self::ToolsetParseError => crate::terminal::console_format(
                "**%s** %s\n",
                &["Usage Exception:", rtrim_php(message)],
                ansi,
            ),
            Self::UserAbort => crate::terminal::format_log("red", "---", message, ansi),
            Self::Fatal => format!("{message}\n"),
        }
    }
}

/// PHP `rtrim()` with the default charlist (`" \t\n\r\0\x0B"`), as
/// applied to usage messages (`scripts/arcanist.php:436`).
fn rtrim_php(message: &str) -> &str {
    message.trim_end_matches([' ', '\t', '\n', '\r', '\0', '\x0B'])
}

/// A workflow error classified for the R4 envelope: the kind selects
/// the banner, stream, and exit code; the message renders verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedError {
    /// Envelope arm to report through.
    pub kind: EnvelopeKind,
    /// Raw message (no `arc <cmd>:` prefix, no usage suffix).
    pub message: String,
}

impl ClassifiedError {
    /// Classify `message` under `kind`.
    #[must_use]
    pub fn new(kind: EnvelopeKind, message: String) -> Self {
        Self { kind, message }
    }

    /// [`EnvelopeKind::NoEffect`] constructor (for `map_err`).
    #[must_use]
    pub fn no_effect(message: String) -> Self {
        Self::new(EnvelopeKind::NoEffect, message)
    }

    /// [`EnvelopeKind::Usage`] constructor (for `map_err`).
    #[must_use]
    pub fn usage(message: String) -> Self {
        Self::new(EnvelopeKind::Usage, message)
    }

    /// [`EnvelopeKind::Exception`] constructor (for `map_err`).
    #[must_use]
    pub fn exception(message: String) -> Self {
        Self::new(EnvelopeKind::Exception, message)
    }

    /// [`EnvelopeKind::ToolsetUsage`] constructor (for `map_err`).
    #[must_use]
    pub fn toolset_usage(message: String) -> Self {
        Self::new(EnvelopeKind::ToolsetUsage, message)
    }

    /// [`EnvelopeKind::ToolsetConduit`] constructor (for `map_err`).
    #[must_use]
    pub fn toolset_conduit(message: String) -> Self {
        Self::new(EnvelopeKind::ToolsetConduit, message)
    }

    /// [`EnvelopeKind::UserAbort`] constructor (for `map_err`).
    #[must_use]
    pub fn user_abort(message: String) -> Self {
        Self::new(EnvelopeKind::UserAbort, message)
    }

    /// [`EnvelopeKind::ToolsetParseError`] constructor (for `map_err`).
    #[must_use]
    pub fn toolset_parse_error(message: String) -> Self {
        Self::new(EnvelopeKind::ToolsetParseError, message)
    }

    /// [`EnvelopeKind::Fatal`] constructor (for `map_err`).
    #[must_use]
    pub fn fatal(message: String) -> Self {
        Self::new(EnvelopeKind::Fatal, message)
    }

    /// Report through the envelope: print and return the exit code.
    pub fn report(&self) -> std::process::ExitCode {
        report(self.kind, &self.message)
    }
}

/// Report `message` through the envelope arm `kind`: print to the
/// arm's stream and return its exit code.
pub fn report(kind: EnvelopeKind, message: &str) -> std::process::ExitCode {
    // T1153 CORE-15: under `--trace` the classic handler rethrows, so
    // usage and generic errors become fatal traces (newline, raw
    // message, backtrace, exit 255) instead of their envelopes. The
    // toolset arms keep their rendering: the toolset runtime catches
    // them with or without `--trace`.
    if crate::terminal::trace_enabled()
        && matches!(kind, EnvelopeKind::Usage | EnvelopeKind::Exception)
    {
        let trace = std::backtrace::Backtrace::capture();
        eprint!("\n{message}\n{trace}");
        return std::process::ExitCode::from(EXIT_FATAL);
    }
    let text = kind.render(message, crate::terminal::ansi_enabled());
    if kind.uses_stdout() {
        print!("{text}");
    } else {
        eprint!("{text}");
    }
    kind.exit_code()
}

#[cfg(test)]
#[path = "conduit_tests.rs"]
mod conduit_tests;
