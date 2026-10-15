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
        match self {
            Error::Transport(msg) => write!(f, "transport error: {msg}"),
            Error::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Error::Api { code, info } => write!(f, "conduit error {code}: {info}"),
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
}

#[derive(Debug)]
struct SigningKeys {
    public_key: String,
    private_pem: String,
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
        let http = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // Intranet hosts are reached directly; proxy env vars must not
            // reroute API calls (and this keeps tests hermetic).
            .no_proxy()
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self {
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
        })
    }

    /// The normalized API base URI (for absolutizing relative URIs).
    #[must_use]
    pub fn base_uri(&self) -> &str {
        &self.base
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
    let trimmed = uri.trim_end_matches('/');
    if let Some(root) = trimmed.strip_suffix("/api") {
        format!("{root}/api/")
    } else {
        format!("{trimmed}/api/")
    }
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
            for (key, item) in map {
                out.push_str(&encode_signature_string(key));
                out.push_str(&encode_raw_data_for_signature(item)?);
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
        // PHP parity: `ConduitClientException` messages are `<method> info`.
        return Err(Error::Api {
            code,
            info: format!("<{method}> {info}"),
        });
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

#[cfg(test)]
#[path = "conduit_tests.rs"]
mod conduit_tests;
