//! Differential revision refs and repository URI matching for the
//! markers workflows.
//!
//! PHP reference: `src/ref/revision/ArcanistRevisionRef.php` (ref fields,
//! `differential.query` mangling, status display),
//! `src/query/ArcanistGitWorkingCopyRevisionHardpointQuery.php` (loading by
//! commit hash), `src/query/ArcanistMessageRevisionHardpointQuery.php`
//! (loading by commit-message revision id),
//! `src/differential/ArcanistDifferentialCommitMessage.php`
//! (`parseRevisionIDFromRawCorpus()`),
//! `src/conduit/ConduitSearchFuture.php` (paginated search),
//! `src/ref/ArcanistRepositoryRef.php` (repository refs, permanent-ref
//! rules), and
//! `src/repository/remote/ArcanistRemoteRepositoryRefsHardpointQuery.php`.
//!
//! Divergences:
//! - `regexp(...)` permanent-ref rules run through a small backtracking
//!   matcher (literals, `.`, `*`, `+`, `?`, `{n,m}`, `|`, groups,
//!   character classes, `^`, `$`, `\d\w\s` escapes) instead of PCRE.
//!   Patterns using other constructs never match; exact-name rules (the
//!   common case) behave identically. The crate set has no regex engine,
//!   like `lint_script_and_regex`.

use std::collections::HashMap;

/// A Differential revision (`ArcanistRevisionRef`), backed by the raw
/// conduit parameters dict.
#[derive(Debug, Clone)]
pub struct RevisionRef {
    params: serde_json::Value,
}

impl RevisionRef {
    /// Build from a modern `differential.revision.search` hit.
    #[must_use]
    pub fn from_search_hit(hit: &serde_json::Value) -> Self {
        Self {
            params: hit.clone(),
        }
    }

    /// Build from a legacy `differential.query` row, mangling it into the
    /// modern shape (`ArcanistRevisionRef::newFromConduitQuery`).
    #[must_use]
    pub fn from_query_row(row: &serde_json::Value) -> Self {
        let status_name = row
            .get("statusName")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let is_closed = matches!(status_name.as_str(), Some("Abandoned" | "Closed"));
        let status_value = match row.get("status").and_then(|value| {
            if let Some(number) = value.as_u64() {
                Some(number.to_string())
            } else {
                value.as_str().map(str::to_string)
            }
        }) {
            Some(code) => match code.as_str() {
                "0" => Some("needs-review"),
                "1" => Some("needs-revision"),
                "2" => Some("accepted"),
                "3" => Some("published"),
                "4" => Some("abandoned"),
                "5" => Some("changes-planned"),
                _ => None,
            },
            None => None,
        };
        let ansi_color = match status_value {
            Some("needs-review") => Some("magenta"),
            Some("needs-revision" | "changes-planned") => Some("red"),
            Some("accepted") => Some("green"),
            Some("published") => Some("cyan"),
            _ => None,
        };
        let mut merged = row.clone();
        if let Some(object) = merged.as_object_mut() {
            object.insert(
                "fields".to_string(),
                serde_json::json!({
                    "uri": row.get("uri").cloned().unwrap_or(serde_json::Value::Null),
                    "title": row.get("title").cloned().unwrap_or(serde_json::Value::Null),
                    "authorPHID": row.get("authorPHID").cloned().unwrap_or(serde_json::Value::Null),
                    "status": {
                        "name": status_name,
                        "closed": is_closed,
                        "value": status_value,
                        "color.ansi": ansi_color,
                    },
                    "dateCreated": row.get("dateCreated").and_then(serde_json::Value::as_i64),
                }),
            );
        }
        Self { params: merged }
    }

    fn field(&self, key: &str) -> Option<&serde_json::Value> {
        self.params.get("fields")?.get(key)
    }

    fn status_field(&self, key: &str) -> Option<&serde_json::Value> {
        self.field("status")?.get(key)
    }

    /// Revision id (`(int)$parameters['id']`).
    #[must_use]
    pub fn id(&self) -> u32 {
        match self.params.get("id") {
            Some(serde_json::Value::Number(number)) => number.as_u64().unwrap_or(0) as u32,
            Some(serde_json::Value::String(text)) => php_int_prefix(text) as u32,
            _ => 0,
        }
    }

    /// Revision PHID.
    #[must_use]
    pub fn phid(&self) -> Option<&str> {
        self.params.get("phid").and_then(serde_json::Value::as_str)
    }

    /// Revision monogram (`D123`).
    #[must_use]
    pub fn monogram(&self) -> String {
        format!("D{}", self.id())
    }

    /// Revision title.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.field("title").and_then(serde_json::Value::as_str)
    }

    /// Raw status value (`needs-review`, ...).
    #[must_use]
    pub fn status(&self) -> Option<&str> {
        self.status_field("value")
            .and_then(serde_json::Value::as_str)
    }

    #[must_use]
    pub fn is_status_published(&self) -> bool {
        self.status() == Some("published")
    }

    #[must_use]
    pub fn is_status_needs_review(&self) -> bool {
        self.status() == Some("needs-review")
    }

    /// Short status display name (`Review` for needs-review, else the
    /// status name).
    #[must_use]
    pub fn status_short_name(&self) -> Option<&str> {
        if self.is_status_needs_review() {
            return Some("Review");
        }
        self.status_display_name()
    }

    /// Full status display name.
    #[must_use]
    pub fn status_display_name(&self) -> Option<&str> {
        self.status_field("name")
            .and_then(serde_json::Value::as_str)
    }

    /// ANSI status color, if the server sent one.
    #[must_use]
    pub fn status_ansi_color(&self) -> Option<&str> {
        self.status_field("color.ansi")
            .and_then(serde_json::Value::as_str)
    }
}

/// Leading-integer parse like PHP's `(int)` cast: optional whitespace and
/// sign, then digits; anything else yields zero.
fn php_int_prefix(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let (sign, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let mut value: i64 = 0;
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value
            .saturating_mul(10)
            .saturating_add((byte - b'0') as i64);
    }
    value.saturating_mul(sign)
}

/// Parse the `Differential Revision:` field of a commit message, returning
/// the revision id.
///
/// Mirrors `parseRevisionIDFromRawCorpus()`: accepts `D123` or a URI whose
/// path is `D123`; a present-but-invalid field is a usage error (which the
/// message hardpoint query catches and skips).
pub fn revision_id_from_message(corpus: &str) -> Result<Option<u32>, String> {
    // `/^Differential Revision:\s*(.+)/im`: the field match is
    // case-insensitive, and `\s*` may span line breaks.
    let prefix = "differential revision:";
    let mut search = 0;
    while search <= corpus.len() {
        let line_end = corpus[search..]
            .find('\n')
            .map(|index| search + index)
            .unwrap_or(corpus.len());
        let line = &corpus[search..line_end];
        let mut value = None;
        if line.len() >= prefix.len() && line[..prefix.len()].eq_ignore_ascii_case(prefix) {
            let rest = &corpus[search + prefix.len()..];
            // `\s*(.+)` backtracks, so the field matches whenever any
            // non-newline character follows the prefix — even when the
            // capture trims to nothing (which then fails validation).
            if rest.contains(|char| char != '\n') {
                let skipped = rest.trim_start_matches([' ', '\t', '\r', '\n', '\x0B', '\x0C']);
                let captured = skipped.split('\n').next().unwrap_or("").trim();
                value = Some(captured);
            }
        }
        if let Some(raw) = value {
            if let Some(id) = bare_revision_id(raw) {
                return Ok(Some(id));
            }
            if let Some(id) = uri_revision_id(raw) {
                return Ok(Some(id));
            }
            return Err(format!(
                "Invalid \"Differential Revision\" field in commit message. This field should have a revision identifier like \"D123\" or a server URI like \"https://devtools.example.com/D123\", but has \"{raw}\"."
            ));
        }
        if line_end == corpus.len() {
            return Ok(None);
        }
        search = line_end + 1;
    }
    Ok(None)
}

/// Match a bare `D123` value (`/^[dD]([1-9]\d*)\z/`).
fn bare_revision_id(raw: &str) -> Option<u32> {
    let digits = raw.strip_prefix('d').or_else(|| raw.strip_prefix('D'))?;
    let mut bytes = digits.bytes();
    match bytes.next() {
        Some(b'1'..=b'9') => {}
        _ => return None,
    }
    if !bytes.all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// Match a URI whose trimmed path is a bare revision id.
fn uri_revision_id(raw: &str) -> Option<u32> {
    let (_, after_scheme) = raw.split_once("://")?;
    let path_start = after_scheme.find('/').map(|index| index + 1)?;
    let path = after_scheme[path_start..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let trimmed = path.trim_matches('/');
    if trimmed.contains('/') {
        return None;
    }
    bare_revision_id(trimmed)
}

/// Run a paginated conduit search (`ConduitSearchFuture`): POST
/// `{constraints, attachments?, after?}`, accumulate `data`, follow
/// `cursor.after` until null.
pub fn conduit_search(
    client: &crate::conduit::Client,
    method: &str,
    constraints: &serde_json::Value,
    attachments: Option<&serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut objects = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut params = serde_json::json!({ "constraints": constraints });
        if let Some(extra) = attachments {
            params["attachments"] = extra.clone();
        }
        if let Some(cursor) = &after {
            params["after"] = serde_json::Value::String(cursor.clone());
        }
        let result = client
            .call(method, &params)
            .map_err(|error| error.to_string())?;
        if let Some(items) = result.get("data").and_then(serde_json::Value::as_array) {
            objects.extend(items.iter().cloned());
        }
        let cursor = result
            .get("cursor")
            .and_then(|value| value.get("after"))
            .and_then(|value| value.as_str().map(str::to_string));
        match cursor {
            Some(next) => after = Some(next),
            None => return Ok(objects),
        }
    }
}

/// Load revision refs by commit hash (`gtcm`) and tree hash (`gttr`).
///
/// Mirrors `ArcanistGitWorkingCopyRevisionHardpointQuery`: one
/// `differential.query` call with `commitHashes`, returning
/// `(hash key, ref)` pairs in conduit order for the caller to map back.
pub fn load_by_commit_hashes(
    client: &crate::conduit::Client,
    hashes: &[(String, String)],
) -> Result<Vec<(String, RevisionRef)>, String> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let pairs: Vec<serde_json::Value> = hashes
        .iter()
        .map(|(kind, hash)| serde_json::json!([kind, hash]))
        .collect();
    let result = client
        .call(
            "differential.query",
            &serde_json::json!({ "commitHashes": pairs }),
        )
        .map_err(|error| error.to_string())?;
    let rows = result.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for row in &rows {
        let Some(hashes) = row.get("hashes").and_then(serde_json::Value::as_array) else {
            continue;
        };
        if hashes.is_empty() {
            continue;
        }
        let reference = RevisionRef::from_query_row(row);
        for hash in hashes {
            let (Some(kind), Some(digest)) = (
                hash.get(0).and_then(serde_json::Value::as_str),
                hash.get(1).and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            out.push((format!("{kind}:{digest}"), reference.clone()));
        }
    }
    Ok(out)
}

/// Load revision refs by id through `differential.revision.search`.
pub fn load_by_ids(
    client: &crate::conduit::Client,
    ids: &[u32],
) -> Result<Vec<RevisionRef>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let hits = conduit_search(
        client,
        "differential.revision.search",
        &serde_json::json!({ "ids": ids }),
        None,
    )?;
    Ok(hits.iter().map(RevisionRef::from_search_hit).collect())
}

/// Merge hash-query refs with message-query refs, deduplicating by PHID
/// with hash-query refs first.
///
/// Mirrors the hardpoint engine merge (`ArcanistObjectListHardpoint`,
/// queries in class-map order: the git query sorts before the message
/// query). Refs without a PHID are kept as-is.
pub fn merge_revision_refs(
    by_hash: Vec<RevisionRef>,
    by_message: Vec<RevisionRef>,
) -> Vec<RevisionRef> {
    let mut merged: Vec<RevisionRef> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for reference in by_hash.into_iter().chain(by_message) {
        match reference.phid() {
            Some(phid) => {
                if seen.contains_key(phid) {
                    continue;
                }
                seen.insert(phid.to_string(), ());
                merged.push(reference);
            }
            None => merged.push(reference),
        }
    }
    merged
}

/// Parsed repository URI: enough of `PhutilURI` for normalization
/// (protocol, domain, path), including `user@host:path` scp syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedUri {
    protocol: String,
    domain: String,
    path: String,
}

fn is_scheme_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')
}

/// Test for `user@host:path` scp syntax (`PhutilURI::isGitURIPattern`).
fn is_scp_uri(uri: &str) -> bool {
    let Some(colon) = uri.find(':') else {
        return false;
    };
    let (head, last) = (&uri[..colon], &uri[colon + 1..]);
    if head.contains('/') || last.starts_with("//") {
        return false;
    }
    if head.chars().chain(last.chars()).any(char::is_whitespace) {
        return false;
    }
    if !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    head.contains(['.', '@'])
}

fn valid_host(host: &str) -> bool {
    let mut bytes = host.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn parse_standard_uri(uri: &str) -> ParsedUri {
    let empty = ParsedUri {
        protocol: String::new(),
        domain: String::new(),
        path: String::new(),
    };
    if uri.starts_with(char::is_whitespace) {
        return empty;
    }
    // Reject ambiguous `scheme://host?query:with-colon` URIs outright.
    if let Some(scheme_end) = uri.find("://") {
        let (scheme, after) = (&uri[..scheme_end], &uri[scheme_end + 3..]);
        if !scheme.contains(['/', ':'])
            && let Some(host_end) = after.find('/')
        {
            let authority = &after[..host_end];
            if authority.contains(['?', '#']) && after.contains(':') {
                return empty;
            }
        }
    }
    let mut rest = uri;
    let mut protocol = String::new();
    if let Some(colon) = rest.find(':') {
        let candidate = &rest[..colon];
        if !candidate.is_empty()
            && candidate
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic())
            && candidate.bytes().all(is_scheme_char)
        {
            protocol = candidate.to_string();
            rest = &rest[colon + 1..];
        }
    }
    let mut domain = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let authority_end = after.find('/').unwrap_or(after.len());
        let authority = &after[..authority_end];
        rest = &after[authority_end..];
        let host_port = match authority.rsplit_once('@') {
            Some((_, host)) => host,
            None => authority,
        };
        let host = match host_port.rsplit_once(':') {
            Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => host,
            _ => host_port,
        };
        if !host.is_empty() && !valid_host(host) {
            return empty;
        }
        domain = host.to_string();
    }
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    ParsedUri {
        protocol,
        domain,
        path: rest[..path_end].to_string(),
    }
}

fn parse_uri(uri: &str) -> ParsedUri {
    if is_scp_uri(uri) {
        let colon = uri.find(':').unwrap_or(0);
        let (head, path) = (&uri[..colon], &uri[colon + 1..]);
        let host = head.rsplit_once('@').map(|(_, host)| host).unwrap_or(head);
        let host = host.split('/').next().unwrap_or(host);
        return ParsedUri {
            protocol: "ssh".to_string(),
            domain: host.to_string(),
            path: path.to_string(),
        };
    }
    parse_standard_uri(uri)
}

/// Normalized git repository path (`getNormalizedPath()` with `TYPE_GIT`).
#[must_use]
pub fn normalize_git_path(uri: &str) -> String {
    let parsed = parse_uri(uri);
    let mut path = parsed.path.trim_matches('/').to_string();
    if let Some(stripped) = path.strip_suffix(".git") {
        path = stripped.to_string();
    }
    // `diffusion/X/anything` clones collapse to the callsign.
    if path.starts_with("diffusion/") {
        let mut parts = path.split('/');
        if let (Some(_), Some(second)) = (parts.next(), parts.next())
            && !second.is_empty()
            && (second.bytes().all(|byte| byte.is_ascii_uppercase())
                || second.bytes().all(|byte| byte.is_ascii_digit()))
        {
            path = format!("diffusion/{second}");
        }
    }
    path
}

/// Normalized git repository domain (`getNormalizedDomain()`).
#[must_use]
pub fn normalize_git_domain(uri: &str, domain_map: &HashMap<String, String>) -> String {
    let parsed = parse_uri(uri);
    if parsed.domain.is_empty() {
        return "<void>".to_string();
    }
    let mut domain = crate::grid::utf8_lower(&parsed.domain);
    for (key, value) in domain_map {
        if domain == *value {
            domain = key.clone();
            break;
        }
    }
    domain
}

/// Normalized git repository URI: domain plus path.
#[must_use]
pub fn normalize_git_uri(uri: &str) -> String {
    format!(
        "{}/{}",
        normalize_git_domain(uri, &HashMap::new()),
        normalize_git_path(uri)
    )
}

/// A diffusion repository (`ArcanistRepositoryRef`), backed by the raw
/// `diffusion.repository.search` hit.
#[derive(Debug, Clone)]
pub struct RepositoryRef {
    params: serde_json::Value,
}

impl RepositoryRef {
    #[must_use]
    pub fn from_search_hit(hit: &serde_json::Value) -> Self {
        Self {
            params: hit.clone(),
        }
    }

    /// Effective repository URIs from the `uris` attachment.
    #[must_use]
    pub fn uris(&self) -> Vec<String> {
        let mut out = Vec::new();
        let uris = self
            .params
            .get("attachments")
            .and_then(|value| value.get("uris"))
            .and_then(|value| value.get("uris"))
            .and_then(serde_json::Value::as_array);
        for uri in uris.into_iter().flatten() {
            if let Some(effective) = uri
                .get("fields")
                .and_then(|value| value.get("uri"))
                .and_then(|value| value.get("effective"))
                .and_then(serde_json::Value::as_str)
            {
                out.push(effective.to_string());
            }
        }
        out
    }

    /// Permanent-ref rules, or `None` when the server sent no rule set.
    fn permanent_rules(&self) -> Option<Vec<String>> {
        let rules = self
            .params
            .get("fields")?
            .get("refRules")?
            .get("permanentRefRules")?;
        if rules.is_null() {
            return None;
        }
        let rules = rules.as_array()?;
        Some(
            rules
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect(),
        )
    }

    /// True when `ref_name` is a permanent ref (`isPermanentRef()`):
    /// missing rules match nothing, an empty rule set matches everything,
    /// otherwise exact names or `regexp(...)` patterns match.
    #[must_use]
    pub fn is_permanent_ref(&self, ref_name: &str) -> bool {
        let Some(rules) = self.permanent_rules() else {
            return false;
        };
        if rules.is_empty() {
            return true;
        }
        for rule in &rules {
            if let Some(pattern) = rule
                .strip_prefix("regexp(")
                .and_then(|rest| rest.strip_suffix(')'))
            {
                if regexp_match(pattern, ref_name) {
                    return true;
                }
            } else if rule == ref_name {
                return true;
            }
        }
        false
    }
}

/// Load repository refs for `uris` (`diffusion.repository.search` with the
/// `uris` attachment and `uris: true`).
pub fn load_repositories_by_uris(
    client: &crate::conduit::Client,
    uris: &[String],
) -> Result<Vec<RepositoryRef>, String> {
    if uris.is_empty() {
        return Ok(Vec::new());
    }
    let hits = conduit_search(
        client,
        "diffusion.repository.search",
        &serde_json::json!({ "uris": uris }),
        Some(&serde_json::json!({ "uris": true })),
    )?;
    Ok(hits.iter().map(RepositoryRef::from_search_hit).collect())
}

// --- Small backtracking matcher for `regexp(...)` rules. ---

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    Empty,
    Literal(char),
    Any,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Concat(Vec<Pattern>),
    Alternate(Vec<Pattern>),
    Repeat(Box<Pattern>, usize, Option<usize>),
    Start,
    End,
}

struct PatternParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    supported: bool,
}

impl<'a> PatternParser<'a> {
    fn new(pattern: &'a str) -> Self {
        Self {
            bytes: pattern.as_bytes(),
            pos: 0,
            supported: true,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn parse(&mut self) -> Pattern {
        let mut alternatives = vec![self.parse_concat()];
        while self.peek() == Some(b'|') {
            self.next();
            alternatives.push(self.parse_concat());
        }
        if alternatives.len() == 1 {
            alternatives.pop().unwrap_or(Pattern::Empty)
        } else {
            Pattern::Alternate(alternatives)
        }
    }

    fn parse_concat(&mut self) -> Pattern {
        let mut parts = Vec::new();
        while let Some(byte) = self.peek() {
            if matches!(byte, b'|' | b')') {
                break;
            }
            parts.push(self.parse_repeat());
        }
        if parts.len() == 1 {
            parts.pop().unwrap_or(Pattern::Empty)
        } else {
            Pattern::Concat(parts)
        }
    }

    fn parse_repeat(&mut self) -> Pattern {
        let mut atom = self.parse_atom();
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.next();
                    self.skip_lazy();
                    atom = Pattern::Repeat(Box::new(atom), 0, None);
                }
                Some(b'+') => {
                    self.next();
                    self.skip_lazy();
                    atom = Pattern::Repeat(Box::new(atom), 1, None);
                }
                Some(b'?') => {
                    self.next();
                    self.skip_lazy();
                    atom = Pattern::Repeat(Box::new(atom), 0, Some(1));
                }
                Some(b'{') => {
                    if let Some((min, max)) = self.parse_count() {
                        atom = Pattern::Repeat(Box::new(atom), min, max);
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        atom
    }

    /// Skip a lazy `?` modifier (irrelevant for match/no-match).
    fn skip_lazy(&mut self) {
        if self.peek() == Some(b'?') {
            self.next();
        }
    }

    fn parse_count(&mut self) -> Option<(usize, Option<usize>)> {
        let start = self.pos;
        self.next();
        let mut min_text = String::new();
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            min_text.push(self.next().unwrap_or(b'0') as char);
        }
        let mut max = None;
        if self.peek() == Some(b',') {
            self.next();
            let mut max_text = String::new();
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                max_text.push(self.next().unwrap_or(b'0') as char);
            }
            if max_text.is_empty() {
                max = None;
            } else {
                max = max_text.parse::<usize>().ok();
            }
        }
        if self.peek() != Some(b'}') || min_text.is_empty() {
            self.pos = start;
            return None;
        }
        self.next();
        self.skip_lazy();
        let min = min_text.parse::<usize>().ok()?;
        if max.is_none()
            && !min_text.is_empty()
            && self.bytes.get(start + 1 + min_text.len()) != Some(&b',')
        {
            max = Some(min);
        }
        if let Some(bound) = max
            && bound < min
        {
            return None;
        }
        Some((min, max))
    }

    fn parse_atom(&mut self) -> Pattern {
        match self.next() {
            Some(b'(') => {
                if self.peek() == Some(b'?') {
                    self.supported = false;
                    self.next();
                    // Skip the group kind marker; contents still parse so
                    // positions stay sane, but the pattern never matches.
                    if matches!(self.peek(), Some(b'=' | b'!' | b'<')) {
                        self.next();
                    }
                    let _ = self.parse();
                    if self.peek() == Some(b')') {
                        self.next();
                    }
                    return Pattern::Empty;
                }
                let group = self.parse();
                if self.peek() == Some(b')') {
                    self.next();
                } else {
                    self.supported = false;
                }
                group
            }
            Some(b'[') => self.parse_class(),
            Some(b'.') => Pattern::Any,
            Some(b'^') => Pattern::Start,
            Some(b'$') => Pattern::End,
            Some(b'\\') => self.parse_escape(),
            Some(byte) => Pattern::Literal(byte as char),
            None => Pattern::Empty,
        }
    }

    fn parse_class(&mut self) -> Pattern {
        let mut negated = false;
        if self.peek() == Some(b'^') {
            negated = true;
            self.next();
        }
        let mut ranges = Vec::new();
        let mut first = true;
        while let Some(byte) = self.peek() {
            if byte == b']' && !first {
                self.next();
                break;
            }
            first = false;
            let low = self.class_char();
            if self.peek() == Some(b'-') && self.bytes.get(self.pos + 1) != Some(&b']') {
                self.next();
                let high = self.class_char();
                ranges.push((low.min(high), low.max(high)));
            } else {
                ranges.push((low, low));
            }
        }
        Pattern::Class { negated, ranges }
    }

    fn class_char(&mut self) -> char {
        match self.next() {
            Some(b'\\') => match self.next() {
                Some(byte) => byte as char,
                None => '\\',
            },
            Some(byte) => byte as char,
            None => '\0',
        }
    }

    fn parse_escape(&mut self) -> Pattern {
        match self.next() {
            Some(b'd') => Pattern::Class {
                negated: false,
                ranges: vec![('0', '9')],
            },
            Some(b'w') => Pattern::Class {
                negated: false,
                ranges: vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
            },
            Some(b's') => Pattern::Class {
                negated: false,
                ranges: vec![
                    (' ', ' '),
                    ('\t', '\t'),
                    ('\n', '\n'),
                    ('\x0B', '\x0B'),
                    ('\x0C', '\x0C'),
                    ('\r', '\r'),
                ],
            },
            Some(b'D') => Pattern::Class {
                negated: true,
                ranges: vec![('0', '9')],
            },
            Some(b'W') => Pattern::Class {
                negated: true,
                ranges: vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
            },
            Some(b'S') => Pattern::Class {
                negated: true,
                ranges: vec![
                    (' ', ' '),
                    ('\t', '\t'),
                    ('\n', '\n'),
                    ('\x0B', '\x0B'),
                    ('\x0C', '\x0C'),
                    ('\r', '\r'),
                ],
            },
            Some(byte) if byte.is_ascii_digit() => {
                // Backreferences are unsupported.
                self.supported = false;
                Pattern::Empty
            }
            Some(byte) => Pattern::Literal(byte as char),
            None => Pattern::Literal('\\'),
        }
    }
}

fn match_here(pattern: &Pattern, chars: &[char], pos: usize) -> Vec<usize> {
    match pattern {
        Pattern::Empty => vec![pos],
        Pattern::Literal(expected) => {
            if chars.get(pos) == Some(expected) {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Pattern::Any => {
            if pos < chars.len() && chars[pos] != '\n' {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Pattern::Class { negated, ranges } => match chars.get(pos) {
            Some(char) => {
                let inside = ranges
                    .iter()
                    .any(|&(low, high)| *char >= low && *char <= high);
                if inside != *negated {
                    vec![pos + 1]
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        },
        Pattern::Start => {
            if pos == 0 {
                vec![pos]
            } else {
                Vec::new()
            }
        }
        Pattern::End => {
            if pos == chars.len() {
                vec![pos]
            } else {
                Vec::new()
            }
        }
        Pattern::Concat(parts) => {
            let mut positions = vec![pos];
            for part in parts {
                let mut next = Vec::new();
                for at in positions {
                    next.extend(match_here(part, chars, at));
                }
                positions = next;
                if positions.is_empty() {
                    break;
                }
            }
            positions
        }
        Pattern::Alternate(options) => {
            let mut positions = Vec::new();
            for option in options {
                positions.extend(match_here(option, chars, pos));
            }
            positions
        }
        Pattern::Repeat(inner, min, max) => {
            let mut counts = vec![pos];
            let mut longest = Vec::new();
            if *min == 0 {
                longest.push(pos);
            }
            let mut depth = 0;
            loop {
                if let Some(bound) = max
                    && depth >= *bound
                {
                    break;
                }
                let mut advanced = Vec::new();
                for at in &counts {
                    for next in match_here(inner, chars, *at) {
                        if next != *at {
                            advanced.push(next);
                        }
                    }
                }
                if advanced.is_empty() {
                    break;
                }
                depth += 1;
                if depth >= *min {
                    longest.extend(advanced.iter().copied());
                }
                counts = advanced;
                if depth > chars.len() + 1 {
                    break;
                }
            }
            longest.sort_unstable();
            longest.dedup();
            longest
        }
    }
}

/// Match `text` against a `regexp(...)` rule body, unanchored like
/// `preg_match()`. Patterns using unsupported constructs never match.
#[must_use]
pub fn regexp_match(pattern: &str, text: &str) -> bool {
    let mut parser = PatternParser::new(pattern);
    let compiled = parser.parse();
    if !parser.supported || parser.pos != parser.bytes.len() {
        return false;
    }
    let chars: Vec<char> = text.chars().collect();
    for start in 0..=chars.len() {
        if !match_here(&compiled, &chars, start).is_empty() {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "revision_tests.rs"]
mod revision_tests;
