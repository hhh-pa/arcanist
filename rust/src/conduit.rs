//! Blocking Conduit API client, mirroring `src/conduit/ConduitClient.php`.
//!
//! Requests are form posts: `params` carries the method parameters plus the
//! `__conduit__` auth metadata as one JSON blob, with `output=json`.

use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

pub struct Client {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn new(conduit_uri: &str, token: &str) -> Result<Self, Error> {
        let http = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // Intranet hosts are reached directly; proxy env vars must not
            // reroute API calls (and this keeps tests hermetic).
            .no_proxy()
            .build()
            .map_err(|e| Error::Transport(e.to_string()))?;
        Ok(Self {
            base: normalize_base(conduit_uri),
            token: token.to_string(),
            http,
        })
    }

    pub fn call(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        let body = build_params(params, &self.token)?;
        let url = format!("{}{}", self.base, method);
        let text = self
            .http
            .post(&url)
            .form(&[
                ("params", body.as_str()),
                ("output", "json"),
                ("__conduit__", "true"),
            ])
            .send()
            .map_err(|e| Error::Transport(e.to_string()))?
            .text()
            .map_err(|e| Error::Transport(e.to_string()))?;
        parse_response(&text)
    }
}

fn normalize_base(uri: &str) -> String {
    let trimmed = uri.trim_end_matches('/');
    if let Some(root) = trimmed.strip_suffix("/api") {
        format!("{root}/api/")
    } else {
        format!("{trimmed}/api/")
    }
}

fn build_params(params: &serde_json::Value, token: &str) -> Result<String, Error> {
    let mut map = match params {
        serde_json::Value::Object(map) => map.clone(),
        _ => return Err(Error::Protocol("params must be a JSON object".to_string())),
    };
    map.insert(
        "__conduit__".to_string(),
        serde_json::json!({"token": token}),
    );
    serde_json::to_string(&map).map_err(|e| Error::Protocol(e.to_string()))
}

fn parse_response(text: &str) -> Result<serde_json::Value, Error> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Error::Protocol(format!("invalid JSON response: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| Error::Protocol("response is not an object".to_string()))?;
    match obj.get("error_code") {
        None | Some(serde_json::Value::Null) => {}
        Some(code) => {
            let info = obj
                .get("error_info")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let code = code.as_str().unwrap_or("ERR-UNKNOWN").to_string();
            return Err(Error::Api { code, info });
        }
    }
    Ok(obj
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

#[cfg(test)]
#[path = "conduit_tests.rs"]
mod conduit_tests;
