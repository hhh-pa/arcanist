//! Local-depth helpers for `arc-rs diff` (bundle D1, task T1128).
//!
//! Pure-local half of `src/workflow/ArcanistDiffWorkflow.php`: commit-message
//! field parsing, base/revision-range spec parsing, preview/stat/patch-set
//! building, and the lint/unit hook decision layer. Nothing here calls
//! Conduit; submit-time behavior belongs to bundle D2.
//!
//! PHP identifiers (field keys, tag names, status words, property names) are
//! quoted VERBATIM from the PHP sources cited on each item. Every deliberate
//! behavior difference is marked `Divergence:` with the PHP line it departs
//! from.

extern crate self as arc_rs;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use arc_rs::{diff, diff_submit, git, hg, lint, svn, unit};

/* -(  Commit Messages  )---------------------------------------------------- */

/// Failure to pull commit-message data from Conduit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullDataError {
    /// Parser errors from `differential.parsecommitmessage`
    /// (`ArcanistDifferentialCommitMessageParserException`, carrying the
    /// error list; the exception message is the first error).
    Parser(Vec<String>),
    /// Transport or malformed-response failure.
    Other(String),
}

impl PullDataError {
    /// Render for string-error contexts: the first parser error, or the
    /// transport message.
    #[must_use]
    pub fn into_message(self) -> String {
        match self {
            PullDataError::Parser(errors) => errors.into_iter().next().unwrap_or_default(),
            PullDataError::Other(message) => message,
        }
    }
}

/// A parsed commit message: `ArcanistDifferentialCommitMessage` with the
/// Conduit round-trip.
///
/// PHP: `src/differential/ArcanistDifferentialCommitMessage.php:6-15`
/// (properties) and `newFromRawCorpus()` at `:17-31`.
///
/// Divergence: [`parse_field_tags`] stays as the offline field parser
/// for the merge flows; Conduit-resolved fields (names to PHIDs) only
/// exist after [`CommitMessage::pull_data_from_conduit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessage {
    /// Verbatim corpus, as passed to `newFromRawCorpus()`.
    pub raw_corpus: String,
    /// Revision ID from the `Differential Revision:` field, if present.
    pub revision_id: Option<u64>,
    /// `git-svn-id` base revision (`url@rev`), if present.
    pub git_svn_base_revision: Option<String>,
    /// `git-svn-id` base path (url), if present.
    pub git_svn_base_path: Option<String>,
    /// `git-svn-id` UUID, if present.
    pub git_svn_uuid: Option<String>,
    /// Parsed fields, empty until [`CommitMessage::pull_data_from_conduit`].
    pub fields: CommitFields,
    /// Server transactions, when the response carries them.
    pub xactions: Option<serde_json::Value>,
}

/// Coerce one conduit field value to a [`FieldValue`]: strings stay text,
/// arrays become lists (non-string items render as JSON), anything else
/// renders as JSON text.
fn conduit_field_value(value: &serde_json::Value) -> FieldValue {
    match value {
        serde_json::Value::String(text) => FieldValue::Text(text.clone()),
        serde_json::Value::Array(items) => FieldValue::List(
            items
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_string).unwrap_or_else(|| item.to_string())
                })
                .collect(),
        ),
        other => FieldValue::Text(other.to_string()),
    }
}

/// Coerce the `fields` member of a `parsecommitmessage` result.
///
/// PHP assigns `$result['fields']` verbatim (possibly null); a missing or
/// non-object member yields no fields here.
fn conduit_fields(result: &serde_json::Value) -> CommitFields {
    result
        .get("fields")
        .and_then(serde_json::Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), conduit_field_value(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// Parser errors from a `parsecommitmessage` result: a non-empty `errors`
/// array, else none. PHP throws on any non-empty `errors`; non-array
/// members never occur from real servers and are ignored.
fn conduit_parser_errors(result: &serde_json::Value) -> Option<Vec<String>> {
    let errors = result.get("errors")?.as_array()?;
    if errors.is_empty() {
        return None;
    }
    Some(
        errors
            .iter()
            .map(|error| {
                error
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| error.to_string())
            })
            .collect(),
    )
}

impl CommitMessage {
    /// Parse `corpus` into revision and git-svn metadata.
    ///
    /// PHP: `ArcanistDifferentialCommitMessage::newFromRawCorpus()`
    /// (`src/differential/ArcanistDifferentialCommitMessage.php:17-31`).
    /// Fails only when the `Differential Revision:` field is present but
    /// invalid (same as PHP, which throws from
    /// `parseRevisionIDFromRawCorpus()`).
    pub fn new_from_raw_corpus(corpus: &str) -> Result<Self, String> {
        let (git_svn_base_revision, git_svn_base_path, git_svn_uuid) =
            match parse_git_svn_id(corpus) {
                Some((revision, path, uuid)) => (Some(revision), Some(path), Some(uuid)),
                None => (None, None, None),
            };
        Ok(Self {
            raw_corpus: corpus.to_string(),
            revision_id: parse_revision_id_from_raw_corpus(corpus)?,
            git_svn_base_revision,
            git_svn_base_path,
            git_svn_uuid,
            fields: CommitFields::new(),
            xactions: None,
        })
    }

    /// Pull parsed fields and transactions from Conduit.
    ///
    /// PHP: `pullDataFromConduit()`
    /// (`src/differential/ArcanistDifferentialCommitMessage.php:48-70`):
    /// `differential.parsecommitmessage` with `corpus` and `partial`;
    /// assigns `fields` plus `transactions` (absent before late October
    /// 2017), then throws the parser errors when `errors` is non-empty.
    pub fn pull_data_from_conduit(
        &mut self,
        call: &diff_submit::ConduitCall<'_>,
        partial: bool,
    ) -> Result<(), PullDataError> {
        let params = diff_submit::parse_commit_message_params(&self.raw_corpus, partial);
        let result = call(diff_submit::METHOD_PARSE_COMMIT_MESSAGE, &params)
            .map_err(PullDataError::Other)?;
        self.fields = conduit_fields(&result);
        self.xactions = result.get("transactions").cloned();
        if let Some(errors) = conduit_parser_errors(&result) {
            return Err(PullDataError::Parser(errors));
        }
        Ok(())
    }

    /// Read one parsed field (`getFieldValue()`, `:72-77`).
    #[must_use]
    pub fn get_field_value(&self, key: &str) -> Option<&FieldValue> {
        self.fields.get(key)
    }

    /// Write one parsed field (`setFieldValue()`, `:79-82`).
    pub fn set_field_value(&mut self, key: &str, value: FieldValue) {
        self.fields.insert(key.to_string(), value);
    }

    /// All parsed fields (`getFields()`, `:84-86`).
    #[must_use]
    pub fn get_fields(&self) -> &CommitFields {
        &self.fields
    }

    /// Server transactions (`getTransactions()`, `:105-107`).
    #[must_use]
    pub fn get_transactions(&self) -> Option<&serde_json::Value> {
        self.xactions.as_ref()
    }

    /// `D123` for the revision ID, if present (`getRevisionMonogram()`,
    /// `:40-45`).
    #[must_use]
    pub fn get_revision_monogram(&self) -> Option<String> {
        self.revision_id.map(|id| format!("D{id}"))
    }
}

/// Extract the revision ID from a `Differential Revision:` field.
///
/// PHP: `ArcanistDifferentialCommitMessage::parseRevisionIDFromRawCorpus()`
/// (`src/differential/ArcanistDifferentialCommitMessage.php:117-147`):
/// first `/^Differential Revision:\s*(.+)/im` match wins; the value is
/// accepted as a bare `D123` (`/^[dD]([1-9]\d*)\z/`) or as a URI whose path
/// is exactly `D123`; anything else is a usage error.
///
/// Divergence: PHP trims with C-locale whitespace while Rust trims Unicode
/// whitespace; only non-ASCII padding differs.
pub fn parse_revision_id_from_raw_corpus(corpus: &str) -> Result<Option<u64>, String> {
    const TAG: &str = "differential revision:";
    let mut value: Option<&str> = None;
    for line in corpus.lines() {
        if let Some(prefix) = line.get(..TAG.len())
            && prefix.eq_ignore_ascii_case(TAG)
        {
            value = Some(line[TAG.len()..].trim_start());
            break;
        }
    }
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if let Some(id) = parse_revision_monogram(value) {
        return Ok(Some(id));
    }
    // Otherwise, try to find a full URI (PHP :131-137).
    if let Some(id) = parse_revision_monogram(strip_uri_path(value)) {
        return Ok(Some(id));
    }
    Err(format!(
        "Invalid \"Differential Revision\" field in commit message. This field \
         should have a revision identifier like \"D123\" or a server URI \
         like \"https://devtools.example.com/D123\", but has \"{value}\"."
    ))
}

/// Strip scheme/host (when present) plus query/fragment, then trim `/`,
/// mirroring `PhutilURI::getPath()` + `trim($path, '/')` at
/// `src/differential/ArcanistDifferentialCommitMessage.php:132-134`.
fn strip_uri_path(value: &str) -> &str {
    let path = if let Some((_, after)) = value.split_once("://") {
        after.split_once('/').map_or("", |(_, rest)| rest)
    } else {
        value
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    path.trim_matches('/')
}

/// Match `/^[dD]([1-9]\d*)\z/` (PHP `:124`).
fn parse_revision_monogram(value: &str) -> Option<u64> {
    let mut chars = value.chars();
    match chars.next() {
        Some('d' | 'D') => {}
        _ => return None,
    }
    let digits: String = chars.collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if digits.starts_with('0') {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Parse the first `git-svn-id:` line into
/// `(base_revision, base_path, uuid)`.
///
/// PHP: `newFromRawCorpus()`
/// (`src/differential/ArcanistDifferentialCommitMessage.php:22-28`) with
/// `/^git-svn-id:\s*([^@]+)@(\d+)\s+(.*)$/m`: base revision is `url@rev`.
/// The pattern is case-sensitive (no `i` flag) and `preg_match` finds the
/// first match; both are mirrored here.
fn parse_git_svn_id(corpus: &str) -> Option<(String, String, String)> {
    const PREFIX: &str = "git-svn-id:";
    for line in corpus.lines() {
        let Some(rest) = line.strip_prefix(PREFIX) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some((url, after)) = rest.split_once('@') else {
            continue;
        };
        if url.is_empty() {
            continue;
        }
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        // `(\d+)\s+(.*)`: at least one whitespace byte must separate the
        // revision from the UUID part.
        let after_digits = &after[digits.len()..];
        let tail = after_digits.trim_start();
        if tail.len() == after_digits.len() {
            continue;
        }
        return Some((
            format!("{url}@{digits}"),
            url.to_string(),
            tail.trim_end().to_string(),
        ));
    }
    None
}

/// Render a revision monogram (`D123`).
///
/// PHP: `ArcanistDifferentialCommitMessage::getRevisionMonogram()`
/// (`src/differential/ArcanistDifferentialCommitMessage.php:41-46`).
/// PHP returns `null` without an ID; the caller holds the `Option` here.
#[must_use]
pub fn revision_monogram(revision_id: u64) -> String {
    format!("D{revision_id}")
}

/// Remove trailing `#` comment lines from a commit message template.
///
/// PHP: `ArcanistCommentRemover::removeComments()`
/// (`src/parser/ArcanistCommentRemover.php:10-30`): rtrim, drop trailing
/// lines starting with `#` (only `#` at column zero counts), rtrim again,
/// append `"\n"`. Line splitting mirrors `phutil_split_lines()`
/// (`src/utils/utils.php:910-927`).
#[must_use]
pub fn remove_comments(body: &str) -> String {
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        return "\n".to_string();
    }
    let mut lines: Vec<&str> = trimmed.split_inclusive('\n').collect();
    while lines.last().is_some_and(|line| line.starts_with('#')) {
        lines.pop();
    }
    format!("{}\n", lines.concat().trim_end())
}

/// Field key for the message title.
///
/// PHP: `parseCommitMessagesIntoFields()` uses `$dict['title']`
/// (`src/workflow/ArcanistDiffWorkflow.php:1907,1935`).
pub const FIELD_TITLE: &str = "title";
/// Field key for the message summary (`src/workflow/ArcanistDiffWorkflow.php:1921`).
pub const FIELD_SUMMARY: &str = "summary";
/// Field key for the Test Plan.
///
/// Divergence: the key spelling is server-defined
/// (`differential.parsecommitmessage`) and unattested in this repository;
/// `testPlan` follows the camelCase convention of `reviewerPHIDs`
/// (`src/workflow/ArcanistDiffWorkflow.php:1633`) and `authorEmail`
/// (`src/repository/api/ArcanistGitAPI.php:201`).
pub const FIELD_TEST_PLAN: &str = "testPlan";
/// Field key for reviewers (`src/workflow/ArcanistDiffWorkflow.php:1633`).
///
/// Divergence: PHP holds resolved PHIDs after the Conduit round-trip; the
/// local parser holds raw names.
pub const FIELD_REVIEWERS: &str = "reviewerPHIDs";
/// Field key for CCs (`src/workflow/ArcanistTodoWorkflow.php:85`).
///
/// Divergence: raw names locally, PHIDs after Conduit (same as reviewers).
pub const FIELD_CCS: &str = "ccPHIDs";
/// Field key for attached tasks.
///
/// Divergence: like [`FIELD_TEST_PLAN`], the spelling is server-defined
/// and unattested in this repository.
pub const FIELD_TASKS: &str = "tasks";
/// Field key for the revision ID (`src/workflow/ArcanistDiffWorkflow.php:1752,1782`).
pub const FIELD_REVISION_ID: &str = "revisionID";

/// One parsed field value: PHP fields are strings or lists of strings
/// (`src/workflow/ArcanistDiffWorkflow.php:1941-1965`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// A string field (`Summary`, `Test Plan`, `title`).
    Text(String),
    /// A list field (`reviewerPHIDs`, `ccPHIDs`, `tasks`).
    List(Vec<String>),
}

/// Field dictionary for one commit message, keyed by the `FIELD_*` keys.
pub type CommitFields = BTreeMap<String, FieldValue>;

/// Split `Name: value` tag lines; returns `(name, trimmed value)`.
///
/// Tag names are ASCII letters and spaces without surrounding padding.
fn split_tag_line(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.split_once(':')?;
    if name.is_empty() || name.trim() != name {
        return None;
    }
    if !name.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        return None;
    }
    Some((name, value.trim()))
}

/// Map a canonical tag name to its field key. `Differential Revision` is
/// recognized but owned by [`parse_revision_id_from_raw_corpus`], so it maps
/// to no key and its line is dropped.
fn tag_field_key(name: &str) -> Option<Option<&'static str>> {
    match name {
        // Tag spellings: `Summary:` / `Test Plan:` in parser fixtures
        // (`src/parser/__tests__/ArcanistDiffParserTestCase.php:434,446`),
        // `Reviewers:` / `CC:` in the faux message
        // (`src/workflow/ArcanistDiffWorkflow.php:1819,1822`),
        // `Differential Revision:` in the revision parser
        // (`src/differential/ArcanistDifferentialCommitMessage.php:119`).
        // `Tasks:` is server-defined and unattested; see `FIELD_TASKS`.
        "Summary" => Some(Some(FIELD_SUMMARY)),
        "Test Plan" => Some(Some(FIELD_TEST_PLAN)),
        "Reviewers" => Some(Some(FIELD_REVIEWERS)),
        "CC" => Some(Some(FIELD_CCS)),
        "Tasks" => Some(Some(FIELD_TASKS)),
        "Differential Revision" => Some(None),
        _ => None,
    }
}

/// Parse well-known field tags out of a commit message corpus.
///
/// The first line is always the title (PHP relies on this at
/// `src/workflow/ArcanistDiffWorkflow.php:1834-1835`). Later `Tag: value`
/// lines fill known fields; non-tag text accumulates into `summary`.
///
/// Divergence: PHP parses fields server-side via
/// `differential.parsecommitmessage`
/// (`src/differential/ArcanistDifferentialCommitMessage.php:48-70`), so the
/// tag table, continuation rules, and `,` list separator below are local
/// approximations: values continue over following non-blank, non-tag lines
/// until a blank line or the next tag; `Reviewers`/`CC`/`Tasks` split on
/// commas; unknown tags are kept verbatim as body text; body text precedes
/// an explicit `Summary` tag joined with a blank line; empty values and
/// empty lists are dropped.
#[must_use]
pub fn parse_field_tags(corpus: &str) -> CommitFields {
    let mut fields = CommitFields::new();
    let mut lines = corpus.lines();
    let Some(first) = lines.next() else {
        return fields;
    };
    let title = first.trim();
    if !title.is_empty() {
        fields.insert(FIELD_TITLE.to_string(), FieldValue::Text(title.to_string()));
    }
    let (body, tags) = split_field_lines(lines);
    insert_tag_fields(&mut fields, tags);
    merge_body_summary(&mut fields, &body);
    fields
}

/// Split message lines after the title into body lines and `(key, text)`
/// tag values; values continue over following non-blank, non-tag lines
/// until a blank line or the next tag.
fn split_field_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
) -> (Vec<&'a str>, Vec<(String, String)>) {
    let mut body: Vec<&'a str> = Vec::new();
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in lines {
        if line.trim().is_empty() {
            flush_current_tag(&mut current, &mut tags);
            body.push("");
            continue;
        }
        if let Some((name, value)) = split_tag_line(line) {
            step_field_tag_line(name, value, line, &mut current, &mut tags, &mut body);
            continue;
        }
        if let Some((_, parts)) = current.as_mut() {
            parts.push(line.to_string());
        } else {
            body.push(line);
        }
    }
    flush_current_tag(&mut current, &mut tags);
    (body, tags)
}

/// Join the open tag value (if any) onto the collected tags.
fn flush_current_tag(
    current: &mut Option<(String, Vec<String>)>,
    tags: &mut Vec<(String, String)>,
) {
    if let Some((key, parts)) = current.take() {
        tags.push((key, parts.join("\n")));
    }
}

/// Route one `Tag: value` line: known tags open a value, `Differential
/// Revision` lines are dropped, unknown tags stay verbatim body text.
fn step_field_tag_line<'a>(
    name: &str,
    value: &str,
    line: &'a str,
    current: &mut Option<(String, Vec<String>)>,
    tags: &mut Vec<(String, String)>,
    body: &mut Vec<&'a str>,
) {
    match tag_field_key(name) {
        Some(Some(key)) => {
            flush_current_tag(current, tags);
            *current = Some((key.to_string(), vec![value.to_string()]));
        }
        Some(None) => {
            flush_current_tag(current, tags);
        }
        None => {
            flush_current_tag(current, tags);
            body.push(line);
        }
    }
}

/// Insert collected tag values: `Reviewers`/`CC`/`Tasks` split on commas
/// into lists, other tags become text; repeats append, empties drop.
fn insert_tag_fields(fields: &mut CommitFields, tags: Vec<(String, String)>) {
    for (key, text) in tags {
        if key == FIELD_REVIEWERS || key == FIELD_CCS || key == FIELD_TASKS {
            insert_list_field(fields, &key, &text);
        } else {
            insert_text_field(fields, &key, &text);
        }
    }
}

/// Append comma-separated names to a list field; empty lists are dropped.
fn insert_list_field(fields: &mut CommitFields, key: &str, text: &str) {
    let names: Vec<String> = text
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        return;
    }
    match fields.get_mut(key) {
        Some(FieldValue::List(existing)) => existing.extend_from_slice(&names),
        _ => {
            fields.insert(key.to_string(), FieldValue::List(names));
        }
    }
}

/// Append a tag value to a text field; blank values are dropped.
fn insert_text_field(fields: &mut CommitFields, key: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    match fields.get_mut(key) {
        Some(FieldValue::Text(existing)) => {
            existing.push_str("\n\n");
            existing.push_str(text);
        }
        _ => {
            fields.insert(key.to_string(), FieldValue::Text(text.to_string()));
        }
    }
}

/// Fold body text into `summary`: ahead of an explicit `Summary` tag when
/// one exists, alone otherwise.
fn merge_body_summary(fields: &mut CommitFields, body: &[&str]) {
    let body_text = body.join("\n");
    let body_text = body_text.trim();
    if body_text.is_empty() {
        return;
    }
    match fields.get(FIELD_SUMMARY) {
        Some(FieldValue::Text(explicit)) if !explicit.trim().is_empty() => {
            let combined = format!("{body_text}\n\n{explicit}");
            fields.insert(FIELD_SUMMARY.to_string(), FieldValue::Text(combined));
        }
        _ => {
            fields.insert(
                FIELD_SUMMARY.to_string(),
                FieldValue::Text(body_text.to_string()),
            );
        }
    }
}

/// Placeholder title prepended to the `--reviewers`/`--cc` faux message so
/// those tags are never the first line (which always parses as a title).
///
/// PHP: `$placeholder_title` at
/// `src/workflow/ArcanistDiffWorkflow.php:1837`.
pub const PLACEHOLDER_TITLE: &str = "<placeholder>";
/// Faux commit hash keying the command-line-flags message in the merge
/// input (`(Flags)` plus five spaces: 12 columns, like a short hash).
///
/// PHP: `src/workflow/ArcanistDiffWorkflow.php:1843`.
pub const FLAGS_COMMIT_HASH: &str = "(Flags)     ";
/// Human summary for the faux flags message in the included-commits list.
///
/// PHP: `src/workflow/ArcanistDiffWorkflow.php:1845`.
pub const FLAGS_SUMMARY: &str = "Command-Line Flags";

/// Build the faux message implied by `--reviewers` / `--cc`.
///
/// PHP: `parseCommitMessagesIntoFields()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1814-1848`): `Reviewers: ...`
/// (`:1819`) and `CC: ...` (`:1822`) lines under the placeholder title,
/// joined with blank lines. `getDependsOnRevisionRef()` always returns
/// `null` (`:2857-2860`), so no `Depends on` line is ever added.
///
/// Divergence: PHP tests argument truthiness, so `--reviewers "0"` would be
/// dropped; only empty values are dropped here.
#[must_use]
pub fn faux_flag_message(reviewers: Option<&str>, cc: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(names) = reviewers
        && !names.is_empty()
    {
        parts.push(format!("Reviewers: {names}"));
    }
    if let Some(names) = cc
        && !names.is_empty()
    {
        parts.push(format!("CC: {names}"));
    }
    if parts.is_empty() {
        return None;
    }
    parts.insert(0, PLACEHOLDER_TITLE.to_string());
    Some(parts.join("\n\n"))
}

/// Render one `included commits` line: 12-char hash plus summary.
///
/// PHP: `src/workflow/ArcanistDiffWorkflow.php:1853-1855`.
#[must_use]
pub fn included_commit_line(hash: &str, summary: &str) -> String {
    let short: String = hash.chars().take(12).collect();
    format!("{short} {summary}")
}

/// Merge per-commit field dictionaries into one revision field dictionary.
///
/// PHP: the merge in `parseCommitMessagesIntoFields()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1890-1969`): input runs
/// oldest-first (`:1904`); the first title wins and later titles demote
/// into their own summary (`:1906-1929`); list values concatenate
/// (`:1941-1948`); string values join with a blank line, skipping blanks
/// (`:1949-1965`); the placeholder title is skipped (`:1912-1914`).
///
/// Callers pass `get_local_commit_information()` output oldest-first (git
/// log order is newest-first, so reverse it). The Conduit field fetch
/// (`pullDataFromConduit($partial = true)`, `:1867-1888`) and its parse
/// notes are not ported; see [`parse_field_tags`].
///
/// Divergence: PHP fatals when one key holds a string in one message and a
/// list in another; here the newer value wins. PHP also treats the strings
/// `"0"`/`"0.0"` as empty (truthiness); only whitespace-blank strings are
/// skipped here.
#[must_use]
pub fn merge_fields_oldest_first(messages: &[CommitFields]) -> CommitFields {
    let mut result = CommitFields::new();
    let mut summary_override: Vec<Option<String>> = vec![None; messages.len()];
    for (index, dict) in messages.iter().enumerate() {
        demote_later_title(&mut result, &mut summary_override, index, dict);
    }

    for (index, dict) in messages.iter().enumerate() {
        let mut effective = dict.clone();
        if let Some(summary) = summary_override[index].take() {
            effective.insert(FIELD_SUMMARY.to_string(), FieldValue::Text(summary));
        }
        merge_message_fields(&mut result, &effective);
    }
    result
}

/// Title pass of the oldest-first merge: the first real title wins; later
/// titles demote into their own summary override. Placeholder titles and
/// missing titles are skipped.
fn demote_later_title(
    result: &mut CommitFields,
    overrides: &mut [Option<String>],
    index: usize,
    dict: &CommitFields,
) {
    let Some(FieldValue::Text(title)) = dict.get(FIELD_TITLE) else {
        return;
    };
    if title.is_empty() || title == PLACEHOLDER_TITLE {
        return;
    }
    if result.contains_key(FIELD_TITLE) {
        let base = match dict.get(FIELD_SUMMARY) {
            Some(FieldValue::Text(summary)) if !summary.is_empty() => {
                format!("{title}\n\n{summary}")
            }
            _ => title.clone(),
        };
        overrides[index] = Some(base);
    } else {
        result.insert(FIELD_TITLE.to_string(), FieldValue::Text(title.clone()));
    }
}

/// Merge one message's non-title fields: lists concatenate, strings join
/// with a blank line, blanks skip, and a newer mismatched type wins.
fn merge_message_fields(result: &mut CommitFields, effective: &CommitFields) {
    use std::collections::btree_map::Entry;

    for (key, value) in effective {
        if key == FIELD_TITLE {
            continue;
        }
        if let FieldValue::Text(text) = value
            && text.trim().is_empty()
        {
            continue;
        }
        match result.entry(key.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(value.clone());
            }
            Entry::Occupied(mut slot) => merge_occupied_field(slot.get_mut(), value),
        }
    }
}

/// Merge one value into an occupied field entry.
fn merge_occupied_field(slot: &mut FieldValue, value: &FieldValue) {
    match (slot, value) {
        (FieldValue::List(existing), FieldValue::List(items)) => {
            existing.extend_from_slice(items);
        }
        (FieldValue::Text(existing), FieldValue::Text(text)) => {
            if existing.is_empty() {
                *existing = text.clone();
            } else {
                existing.push_str("\n\n");
                existing.push_str(text);
            }
        }
        (slot, newer) => {
            *slot = newer.clone();
        }
    }
}

/// Read a revision message from `--message-file` and parse its local parts.
///
/// PHP: `getCommitMessageFromFile()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1561-1571`): `readFile`,
/// `newFromRawCorpus`, Conduit pull, validate. Only the read and the local
/// parse happen here; [`get_commit_message_from_file_with`] adds the
/// Conduit pull and [`validate_commit_message`].
pub fn commit_message_from_file(path: &Path) -> Result<CommitMessage, String> {
    let data = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read message file '{}': {e}", path.display()))?;
    CommitMessage::new_from_raw_corpus(&data)
}

/* -(  Update Messages  )---------------------------------------------------- */

/// One commit's hash plus full message text, feeding update-message
/// selection and parent-log parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogMessage {
    /// Full commit hash.
    pub commit_hash: String,
    /// Full commit message text.
    pub message: String,
}

/// Select the commit messages usable to prefill a revision update.
///
/// PHP: the selection loop in `getGitUpdateMessage()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2001-2040`): a single message
/// means an amend workflow, so nothing is usable; otherwise messages run
/// newest-first and selection stops before the first message carrying a
/// revision ID or already attached to the diff. An invalid `Differential
/// Revision:` field propagates (PHP has no catch here, unlike
/// `getGitParentLogInfo()`).
///
/// `active_hashes` is what `loadActiveDiffLocalCommitHashes()` (`:2110-2179`,
/// Conduit, D2) returns. An empty result means "no prefill" (PHP returns
/// `null`, `:2037-2040`); otherwise feed it to [`format_usable_logs`].
pub fn select_usable_messages(
    messages: &[LogMessage],
    active_hashes: &HashSet<String>,
) -> Result<Vec<String>, String> {
    if messages.len() == 1 {
        return Ok(Vec::new());
    }
    let mut usable = Vec::new();
    for message in messages {
        if parse_revision_id_from_raw_corpus(&message.message)?.is_some() {
            break;
        }
        if active_hashes.contains(&message.commit_hash) {
            break;
        }
        usable.push(message.message.clone());
    }
    Ok(usable)
}

/// Format usable log messages as a `  - first line` prefill template.
///
/// PHP: `formatUsableLogs()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2090-2108`): input runs
/// newest-first and is flipped to read chronologically; each message
/// contributes its trimmed first line prefixed with two spaces, `-`, one
/// space, plus a newline.
#[must_use]
pub fn format_usable_logs(usable_newest_first: &[String]) -> String {
    let mut out = String::new();
    for message in usable_newest_first.iter().rev() {
        let text = message.trim();
        let first = text.split('\n').next().unwrap_or("");
        out.push_str("  - ");
        out.push_str(first);
        out.push('\n');
    }
    out
}

/// Build the interactive update-message template around prefilled comments.
///
/// PHP: `getUpdateMessage()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1741-1759`): comments are
/// right-trimmed (`:1744-1745`) and wrapped in the `#`-comment template
/// (`:1747-1758`). The interactive editor itself is not ported.
#[must_use]
pub fn default_update_message_template(comments: &str, revision_id: u64, title: &str) -> String {
    format!(
        "{}\n\n# Updating D{revision_id}: {title}\n\
         #\n\
         # Enter a brief description of the changes included in this update.\n\
         # The first line is used as subject, next lines as comment.\n\
         #\n\
         # If you intended to create a new revision, use:\n\
         #  $ arc diff --create\n\n",
        comments.trim_end(),
    )
}

/// Error when `--raw` updates a revision without `--message`.
///
/// PHP: `getUpdateMessage()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1721-1729`).
pub const RAW_UPDATE_MESSAGE_ERROR: &str = "When using '--raw' to update a revision, \
    specify an update message with '--message'. (Normally, we'd launch an editor to ask you \
    for a message, but can not do that because stdin is the diff source.)";

/// Reject update-message prompting for raw diff sources.
///
/// PHP: the `--raw` guard in `getUpdateMessage()` (`:1721-1729`).
pub fn require_non_raw_update_message(is_raw: bool) -> Result<(), String> {
    if is_raw {
        return Err(RAW_UPDATE_MESSAGE_ERROR.to_string());
    }
    Ok(())
}

/* -(  Base and Range Specs  )------------------------------------------------ */

/// Parse the optional positional base commit (`arc diff [commit]`).
///
/// PHP: `ArcanistWorkflow::parseBaseCommitArgument()`
/// (`src/workflow/ArcanistWorkflow.php:1825-1846`): no argument is a no-op;
/// a VCS without commit ranges (svn) rejects any argument; more than one
/// argument is a usage error. The selected commit becomes the symbolic base
/// whose merge-base with the head is diffed.
pub fn parse_base_commit_argument(
    supports_commit_ranges: bool,
    argv: &[String],
) -> Result<Option<String>, String> {
    if argv.is_empty() {
        return Ok(None);
    }
    if !supports_commit_ranges {
        return Err("This version control system does not support commit ranges.".to_string());
    }
    if argv.len() > 1 {
        return Err(
            "Specify exactly one base commit. The end of the commit range is \
             always the working copy state."
                .to_string(),
        );
    }
    Ok(Some(argv[0].clone()))
}

/// The empty-tree commit used as the base for zero/one-commit repositories.
///
/// PHP: `ArcanistGitAPI::GIT_MAGIC_ROOT_COMMIT`
/// (`src/repository/api/ArcanistGitAPI.php:15`).
pub const GIT_MAGIC_ROOT_COMMIT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// How many history entries to scan for parent revision info.
///
/// PHP: `ArcanistGitAPI::SEARCH_LENGTH_FOR_PARENT_REVISIONS`
/// (`src/repository/api/ArcanistGitAPI.php:9`).
pub const SEARCH_LENGTH_FOR_PARENT_REVISIONS: u32 = 16;

/// Strip one leading `D`/`d` from a revision parameter (`D123` -> `123`).
///
/// PHP: `ArcanistWorkflow::normalizeRevisionID()`
/// (`src/workflow/ArcanistWorkflow.php:1519-1521`):
/// `preg_replace('/^D/i', '', $revision_id)`.
#[must_use]
pub fn normalize_revision_id(revision_id: &str) -> &str {
    revision_id
        .strip_prefix('D')
        .or_else(|| revision_id.strip_prefix('d'))
        .unwrap_or(revision_id)
}

/// Validate a `--update` revision parameter into a numeric revision ID.
///
/// PHP: `buildCommitMessage()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1340-1346`): normalize, then
/// require a Differential revision number.
///
/// Divergence: PHP uses `is_numeric()`, which also accepts inputs like
/// `1e3`, `12.3`, or `+123`; only ASCII digits (around optional padding)
/// are accepted here, and overlong digit strings fail `u64` parsing.
pub fn parse_update_revision_id(target: &str) -> Result<u64, String> {
    let digits = normalize_revision_id(target).trim();
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && let Ok(id) = digits.parse::<u64>()
    {
        return Ok(id);
    }
    Err("Parameter to --update must be a Differential Revision number.".to_string())
}

/// Resolve an explicit symbolic base to the merge-base with the head.
///
/// PHP: `ArcanistGitAPI::buildBaseCommit()`
/// (`src/repository/api/ArcanistGitAPI.php:208-243`): `merge-base` of the
/// symbolic commit and the head commit, else a usage error naming the
/// unknown commit (`:216-225`). The base-commit explanations (`:227-240`)
/// are presentation for `arc which` and are not ported.
pub fn resolve_explicit_base(
    dir: &Path,
    symbolic_base: &str,
    head: &str,
) -> Result<String, String> {
    let out = git::output(dir, &["merge-base", "--", symbolic_base, head])?;
    if out.status.success() {
        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !sha.is_empty() {
            return Ok(sha);
        }
    }
    Err(format!(
        "Unable to find any git commit named '{symbolic_base}' in this repository."
    ))
}

/// Require the head to descend from the base (non-empty, forward range).
///
/// PHP: the `--head` range check in `getLocalCommitInformation()`
/// (`src/repository/api/ArcanistGitAPI.php:132-157`) using `isDescendant()`
/// (`:89-97`: the merge-base of child and parent equals the parent but not
/// the child). The message names the resolved base and the symbolic head,
/// exactly as PHP does (`:133-156`); an unresolvable head reports the
/// `resolveCommit()` error (`:410-423`).
pub fn require_head_descends_base(
    dir: &Path,
    base_commit: &str,
    symbolic_head: &str,
) -> Result<(), String> {
    let resolved_head = match git::output(dir, &["rev-parse", symbolic_head]) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => {
            return Err(format!(
                "Unable to find any git commit named '{symbolic_head}' in this repository."
            ));
        }
    };
    let common = git::output(dir, &["merge-base", "--", &resolved_head, base_commit])?;
    let common = String::from_utf8_lossy(&common.stdout).trim().to_string();
    if common == base_commit && common != resolved_head {
        return Ok(());
    }
    Err(format!(
        "The specified commit range is empty, backward or invalid: the base ({base_commit}) \
         is not an ancestor of the head ({symbolic_head}). You can not diff an empty or \
         reversed commit range."
    ))
}

/* -(  Local Commit Information  )-------------------------------------------- */

/// One commit from `git log`, keyed exactly like PHP.
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation()`
/// (`src/repository/api/ArcanistGitAPI.php:193-202`): `commit`, `tree`,
/// `parents`, `time`, `author`, `summary`, `message`, `authorEmail`.
/// [`LocalCommitInfo::to_json`] emits those keys verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCommitInfo {
    /// Full commit hash (`commit`).
    pub commit: String,
    /// Tree hash (`tree`).
    pub tree: String,
    /// Parent hashes (`parents`).
    pub parents: Vec<String>,
    /// Author timestamp (`time`, `%at` text).
    pub time: String,
    /// Author name (`author`).
    pub author: String,
    /// Subject line (`summary`).
    pub summary: String,
    /// Full message, right-trimmed (`message`).
    pub message: String,
    /// Author email (`authorEmail`).
    pub author_email: String,
}

impl LocalCommitInfo {
    /// Render with the verbatim PHP keys (for `local:commits`, D2).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "commit": self.commit,
            "tree": self.tree,
            "parents": self.parents,
            "time": self.time,
            "author": self.author,
            "summary": self.summary,
            "message": self.message,
            "authorEmail": self.author_email,
        })
    }
}

/// `git log --format` string for local commit information, verbatim.
///
/// PHP: `src/repository/api/ArcanistGitAPI.php:178`.
pub const LOCAL_COMMIT_LOG_FORMAT: &str =
    "%H%x01%T%x01%P%x01%at%x01%an%x01%aE%x01%s%x01%s%n%n%b%x02";

/// Parse `git log` output in [`LOCAL_COMMIT_LOG_FORMAT`].
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation()`
/// (`src/repository/api/ArcanistGitAPI.php:182-205`): trim framing, split
/// records on `\x02`, fields on `\x01` (8 fields; the message keeps
/// embedded separators via the split limit), right-trim the message, split
/// parents on spaces dropping empties.
///
/// Divergence: PHP assigns `null`s with warnings on short records; short
/// records are an error here (git output is machine-generated, so this
/// only fires on corruption).
pub fn parse_local_commit_information(output: &str) -> Result<Vec<LocalCommitInfo>, String> {
    let info = output.trim_matches([' ', '\n', '\x02']);
    if info.is_empty() {
        return Ok(Vec::new());
    }
    let mut commits = Vec::new();
    for (index, record) in info.split('\x02').enumerate() {
        let parts: Vec<&str> = record.trim().splitn(8, '\x01').collect();
        if parts.len() != 8 {
            return Err(format!(
                "malformed local commit information record {}",
                index + 1
            ));
        }
        commits.push(LocalCommitInfo {
            commit: parts[0].to_string(),
            tree: parts[1].to_string(),
            parents: parts[2]
                .split(' ')
                .filter(|parent| !parent.is_empty())
                .map(str::to_string)
                .collect(),
            time: parts[3].to_string(),
            author: parts[4].to_string(),
            summary: parts[6].to_string(),
            message: parts[7].trim_end().to_string(),
            author_email: parts[5].to_string(),
        });
    }
    Ok(commits)
}

/// List the commits in `head --not base` (or `HEAD` for a lone commit).
///
/// PHP: `ArcanistGitAPI::getLocalCommitInformation()`
/// (`src/repository/api/ArcanistGitAPI.php:99-206`): repositories without
/// commits fail (`:101-106`); a magic-root base lists `HEAD` (`:106-108`);
/// otherwise commits reachable from the head but not the base are listed
/// newest-first (`:109-163`), which matches user expectations on branched
/// history better than the literal diff range. Output order is newest-first
/// (git log order).
///
/// The `--head` descendant check lives in [`require_head_descends_base`]
/// (call it first when the head is explicit). The Windows `%`-escaping
/// workaround (`:165-178`) has no Rust equivalent (no shell) and needs
/// none.
pub fn get_local_commit_information(
    dir: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<LocalCommitInfo>, String> {
    if !git::ok(dir, &["rev-parse", "--verify", "HEAD"]) {
        return Err(
            "You can't get local commit information for a repository with no commits.".to_string(),
        );
    }
    let format_arg = format!("--format={LOCAL_COMMIT_LOG_FORMAT}");
    let args: Vec<&str> = if base == GIT_MAGIC_ROOT_COMMIT {
        vec!["log", "HEAD", &format_arg, "--"]
    } else {
        vec!["log", head, "--not", base, &format_arg, "--"]
    };
    let out = git::output(dir, &args)?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_local_commit_information(&String::from_utf8_lossy(&out.stdout))
}

/// `git log --format` string for (hash, message) pair logs.
///
/// Divergence: PHP fetches `--format=medium` logs and re-parses them with
/// the diff parser (`getGitCommitLog()` at
/// `src/repository/api/ArcanistGitAPI.php:564-583` and `getGitHistoryLog()`
/// at `:585-591`); the unported `ArcanistDiffParser` is replaced by an
/// explicit unambiguous format here.
pub const MESSAGE_LOG_FORMAT: &str = "%H%x01%B%x02";

/// Parse `git log` output in [`MESSAGE_LOG_FORMAT`] into hash/message pairs.
pub fn parse_hash_message_log(output: &str) -> Result<Vec<LogMessage>, String> {
    let info = output.trim_matches([' ', '\n', '\x02']);
    if info.is_empty() {
        return Ok(Vec::new());
    }
    let mut messages = Vec::new();
    for (index, record) in info.split('\x02').enumerate() {
        let parts: Vec<&str> = record.trim().splitn(2, '\x01').collect();
        if parts.len() != 2 {
            return Err(format!("malformed commit message record {}", index + 1));
        }
        messages.push(LogMessage {
            commit_hash: parts[0].to_string(),
            message: parts[1].to_string(),
        });
    }
    Ok(messages)
}

/// Fetch the messages between the base and the head for update prefills.
///
/// PHP command shape: `ArcanistGitAPI::getGitCommitLog()`
/// (`src/repository/api/ArcanistGitAPI.php:564-583`): `log --format=medium
/// HEAD --` for a lone commit, else `log --first-parent --format=medium
/// base..head --`. Output order is newest-first. See [`MESSAGE_LOG_FORMAT`]
/// for the format divergence.
pub fn get_git_update_messages(
    dir: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<LogMessage>, String> {
    let format_arg = format!("--format={MESSAGE_LOG_FORMAT}");
    let range = format!("{base}..{head}");
    let args: Vec<&str> = if base == GIT_MAGIC_ROOT_COMMIT {
        vec!["log", &format_arg, "HEAD", "--"]
    } else {
        vec!["log", "--first-parent", &format_arg, &range, "--"]
    };
    let out = git::output(dir, &args)?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_hash_message_log(&String::from_utf8_lossy(&out.stdout))
}

/// Fetch recent history messages ending at the base for parent discovery.
///
/// PHP command shape: `ArcanistGitAPI::getGitHistoryLog()`
/// (`src/repository/api/ArcanistGitAPI.php:585-591`): `log --format=medium
/// -n16 base --`. See [`MESSAGE_LOG_FORMAT`] for the format divergence.
pub fn get_git_parent_history(dir: &Path, base: &str) -> Result<Vec<LogMessage>, String> {
    let format_arg = format!("--format={MESSAGE_LOG_FORMAT}");
    let count = format!("-n{}", SEARCH_LENGTH_FOR_PARENT_REVISIONS);
    let out = git::output(dir, &["log", &format_arg, &count, base, "--"])?;
    if !out.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_hash_message_log(&String::from_utf8_lossy(&out.stdout))
}

/// Parent revision and git-svn metadata discovered from history.
///
/// PHP keys: `parent`, `base_revision`, `base_path`, `uuid`
/// (`src/workflow/ArcanistDiffWorkflow.php:1029-1034`).
/// [`ParentLogInfo::to_json`] emits those keys verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentLogInfo {
    /// Nearest revision ID found in history (`parent`).
    pub parent: Option<u64>,
    /// First git-svn base revision (`base_revision`).
    pub base_revision: Option<String>,
    /// First git-svn base path (`base_path`).
    pub base_path: Option<String>,
    /// Most recent git-svn UUID (`uuid`).
    pub uuid: Option<String>,
}

impl ParentLogInfo {
    /// Render with the verbatim PHP keys.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "parent": self.parent,
            "base_revision": self.base_revision,
            "base_path": self.base_path,
            "uuid": self.uuid,
        })
    }
}

/// Discover parent revision info from history messages, newest-first.
///
/// PHP: `getGitParentLogInfo()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1028-1072`): the first message
/// with a revision ID sets `parent`; the first git-svn data sets
/// `base_revision`/`base_path`; any UUID overwrites `uuid`; scanning stops
/// once both `parent` and `base_revision` are known. Messages with an
/// invalid `Differential Revision:` field are skipped entirely (PHP catches
/// both parser and usage exceptions, `:1064-1068`).
#[must_use]
pub fn parent_log_info(messages: &[String]) -> ParentLogInfo {
    let mut info = ParentLogInfo {
        parent: None,
        base_revision: None,
        base_path: None,
        uuid: None,
    };
    for text in messages {
        let Ok(parsed) = CommitMessage::new_from_raw_corpus(text) else {
            continue;
        };
        if parsed.revision_id.is_some() && info.parent.is_none() {
            info.parent = parsed.revision_id;
        }
        if parsed.git_svn_base_revision.is_some() && info.base_revision.is_none() {
            info.base_revision = parsed.git_svn_base_revision.clone();
            info.base_path = parsed.git_svn_base_path.clone();
        }
        if let Some(uuid) = &parsed.git_svn_uuid
            && !uuid.is_empty()
        {
            info.uuid = Some(uuid.clone());
        }
        if info.parent.is_some() && info.base_revision.is_some() {
            break;
        }
    }
    info
}

/// Strip one `refs/heads/` prefix from a symbolic ref into a branch name.
///
/// PHP: `ArcanistGitAPI::getBranchNameFromRef()`
/// (`src/repository/api/ArcanistGitAPI.php:508-520`): exactly one leading
/// prefix must match, and the remainder must be non-empty.
#[must_use]
pub fn branch_name_from_ref(symbolic_ref: &str) -> Option<String> {
    let branch = symbolic_ref.strip_prefix("refs/heads/")?;
    if branch.is_empty() {
        return None;
    }
    Some(branch.to_string())
}

/// Read the current branch name (`None` when HEAD is detached).
///
/// PHP: `ArcanistGitAPI::getBranchName()`
/// (`src/repository/api/ArcanistGitAPI.php:522-543`): `symbolic-ref --quiet
/// HEAD`; exit 0 parses `refs/heads/...` (`Failed to parse ...` on
/// mismatch, `:531-534`); exit 1 means detached; anything else is a
/// command failure (`:540-542`).
pub fn get_branch_name(dir: &Path) -> Result<Option<String>, String> {
    let out = git::output(dir, &["symbolic-ref", "--quiet", "HEAD"])?;
    match out.status.code() {
        Some(0) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let reference = stdout.trim_end_matches([' ', '\t', '\n', '\r', '\0', '\x0B']);
            branch_name_from_ref(reference)
                .map(Some)
                .ok_or_else(|| "Failed to parse git symbolic-ref output!".to_string())
        }
        Some(1) => Ok(None),
        _ => Err(format!(
            "Command git symbolic-ref failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
    }
}

/* -(  Change Sets and Preview  )--------------------------------------------- */

/// Change-type constants, verbatim values.
///
/// PHP: `ArcanistDiffChangeType::TYPE_*`
/// (`src/parser/diff/ArcanistDiffChangeType.php:8-17`).
pub const TYPE_ADD: u32 = 1;
/// Modified file (`TYPE_CHANGE`).
pub const TYPE_CHANGE: u32 = 2;
/// Deleted file (`TYPE_DELETE`).
pub const TYPE_DELETE: u32 = 3;
/// Old side of a move (`TYPE_MOVE_AWAY`).
pub const TYPE_MOVE_AWAY: u32 = 4;
/// Old side of a copy (`TYPE_COPY_AWAY`).
pub const TYPE_COPY_AWAY: u32 = 5;
/// New side of a move (`TYPE_MOVE_HERE`).
pub const TYPE_MOVE_HERE: u32 = 6;
/// New side of a copy (`TYPE_COPY_HERE`).
pub const TYPE_COPY_HERE: u32 = 7;
/// Deleted after multiple copies (`TYPE_MULTICOPY`).
pub const TYPE_MULTICOPY: u32 = 8;
/// Commit message pseudo-change (`TYPE_MESSAGE`).
pub const TYPE_MESSAGE: u32 = 9;
/// Child change (`TYPE_CHILD`).
pub const TYPE_CHILD: u32 = 10;

/// File-type constants, verbatim values.
///
/// PHP: `ArcanistDiffChangeType::FILE_*`
/// (`src/parser/diff/ArcanistDiffChangeType.php:19-25`).
pub const FILE_TEXT: u32 = 1;
/// Image file (`FILE_IMAGE`).
pub const FILE_IMAGE: u32 = 2;
/// Binary file (`FILE_BINARY`).
pub const FILE_BINARY: u32 = 3;
/// Directory (`FILE_DIRECTORY`).
pub const FILE_DIRECTORY: u32 = 4;
/// Symlink (`FILE_SYMLINK`).
pub const FILE_SYMLINK: u32 = 5;
/// Deleted file (`FILE_DELETED`).
pub const FILE_DELETED: u32 = 6;
/// Normal file (`FILE_NORMAL`).
pub const FILE_NORMAL: u32 = 7;

/// One-letter summary character for a change type.
///
/// PHP: `ArcanistDiffChangeType::getSummaryCharacterForChangeType()`
/// (`src/parser/diff/ArcanistDiffChangeType.php:27-41`): unknown and absent
/// types render `~`.
#[must_use]
pub fn summary_character_for_change_type(change_type: Option<u32>) -> char {
    match change_type {
        Some(TYPE_ADD) => 'A',
        Some(TYPE_CHANGE) => 'M',
        Some(TYPE_DELETE) => 'D',
        Some(TYPE_MOVE_AWAY) | Some(TYPE_MOVE_HERE) => 'V',
        Some(TYPE_COPY_AWAY) | Some(TYPE_COPY_HERE) | Some(TYPE_MULTICOPY) => 'P',
        Some(TYPE_MESSAGE) => 'Q',
        Some(TYPE_CHILD) => '@',
        _ => '~',
    }
}

/// Short attribute name for a file type (`None` for plain text).
///
/// PHP: `ArcanistDiffChangeType::getShortNameForFileType()`
/// (`src/parser/diff/ArcanistDiffChangeType.php:43-52`): text renders no
/// attribute; unknown and absent types render `???` (this includes
/// `FILE_DELETED` and `FILE_NORMAL`, which have no short names).
#[must_use]
pub fn short_name_for_file_type(file_type: Option<u32>) -> Option<&'static str> {
    match file_type {
        Some(FILE_TEXT) => None,
        Some(FILE_DIRECTORY) => Some("dir"),
        Some(FILE_IMAGE) => Some("img"),
        Some(FILE_BINARY) => Some("bin"),
        Some(FILE_SYMLINK) => Some("sym"),
        _ => Some("???"),
    }
}

/// True for change types rooted at the old location.
///
/// PHP: `ArcanistDiffChangeType::isOldLocationChangeType()`
/// (`src/parser/diff/ArcanistDiffChangeType.php:54-61`).
#[must_use]
pub fn is_old_location_change_type(change_type: u32) -> bool {
    matches!(
        change_type,
        TYPE_MOVE_AWAY | TYPE_COPY_AWAY | TYPE_MULTICOPY
    )
}

/// True for change types rooted at the new location.
///
/// PHP: `ArcanistDiffChangeType::isNewLocationChangeType()`
/// (`src/parser/diff/ArcanistDiffChangeType.php:63-69`).
#[must_use]
pub fn is_new_location_change_type(change_type: u32) -> bool {
    matches!(change_type, TYPE_MOVE_HERE | TYPE_COPY_HERE)
}

/// One file change: its types and locations plus numstat counts.
///
/// Counts reuse [`diff::FileDiff`]; types follow `ArcanistDiffChange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    /// `TYPE_*` change type.
    pub change_type: u32,
    /// `FILE_*` file type.
    pub file_type: u32,
    /// Old path for new-location changes (`getOldPath()`).
    pub old_path: Option<String>,
    /// Destination paths for old-location changes (`getAwayPaths()`).
    pub away_paths: Vec<String>,
    /// Current path plus added/deleted counts.
    pub current: diff::FileDiff,
}

/// Indent before `to:`/`from:` location lines: 13 spaces.
///
/// PHP: `ArcanistDiffChange::renderTextSummary()`
/// (`src/parser/diff/ArcanistDiffChange.php:283,287`).
const LOCATION_INDENT: &str = "             ";

/// Render one change as a one-line (plus location lines) text summary.
///
/// PHP: `ArcanistDiffChange::renderTextSummary()`
/// (`src/parser/diff/ArcanistDiffChange.php:264-291`):
/// `sprintf('%s %5.5s %s', $char, $attr, $path)` (`:276-280`) where `$attr`
/// is `(short)` or empty (`:270-273`); old-location changes append
/// `to: ...` lines (`:281-285`), new-location changes a `from: ...` line
/// (`:286-288`).
///
/// Divergence: `%5.5s` truncates by bytes in PHP and by chars here; the
/// attributes are fixed ASCII so this only differs for hypothetical
/// non-ASCII input.
#[must_use]
pub fn render_text_summary(entry: &ChangeEntry) -> String {
    let symbol = summary_character_for_change_type(Some(entry.change_type));
    let attribute = match short_name_for_file_type(Some(entry.file_type)) {
        Some(name) => format!("({name})"),
        None => String::new(),
    };
    let attribute: String = attribute.chars().take(5).collect();
    let mut out = format!("{symbol} {attribute:>5} {}", entry.current.path);
    if is_old_location_change_type(entry.change_type) {
        for path in &entry.away_paths {
            out.push_str(&format!("\n{LOCATION_INDENT}to: {path}"));
        }
    }
    if is_new_location_change_type(entry.change_type) {
        let old = entry.old_path.as_deref().unwrap_or("");
        out.push_str(&format!("\n{LOCATION_INDENT}from: {old}"));
    }
    out
}

/// Render the `Included changes:` preview block.
///
/// PHP: `ArcanistDiffWorkflow::run()`
/// (`src/workflow/ArcanistDiffWorkflow.php:565-568`).
#[must_use]
pub fn render_included_changes(entries: &[ChangeEntry]) -> String {
    let mut out = "Included changes:\n".to_string();
    for entry in entries {
        out.push_str("  ");
        out.push_str(&render_text_summary(entry));
        out.push('\n');
    }
    out
}

/// True when the data contains a NUL byte (git's binary heuristic).
///
/// PHP: `ArcanistDiffUtils::isHeuristicBinaryFile()`
/// (`src/difference/ArcanistDiffUtils.php:14-19`).
#[must_use]
pub fn is_heuristic_binary_file(data: &[u8]) -> bool {
    data.contains(&0)
}

/// Per-change hunk corpus size limit (4 MiB).
///
/// PHP: `$limit = 1024 * 1024 * 4` in `generateChanges()`
/// (`src/workflow/ArcanistDiffWorkflow.php:903`).
pub const GIGANTIC_DIFF_LIMIT_BYTES: usize = 1024 * 1024 * 4;

/// Names of changes whose corpus exceeds [`GIGANTIC_DIFF_LIMIT_BYTES`].
///
/// PHP: the size scan in `generateChanges()` (`:903-909`); the comparison
/// is strict (`$size > $limit`).
#[must_use]
pub fn gigantic_diff_paths(sizes: &[(&str, usize)]) -> Vec<String> {
    sizes
        .iter()
        .filter(|(_, size)| *size > GIGANTIC_DIFF_LIMIT_BYTES)
        .map(|(path, _)| (*path).to_string())
        .collect()
}

/// Warning for an over-limit change.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:910-914`).
///
/// Divergence: PHP interpolates a `PhutilNumber`, which has no string form
/// in this repository (`src/internationalization/PhutilNumber.php`); the
/// size renders as plain digits here. The SVN-only binary hint (`:915-922`)
/// and the interactive mark-as-binary prompt (`:923-933`) are not ported.
#[must_use]
pub fn gigantic_diff_warning(path: &str, size_bytes: usize) -> String {
    format!(
        "Diff for '{path}' with context is {size_bytes} bytes in length. \
         Generally, source changes should not be this large."
    )
}

/// Abort message when gigantic-diff generation is refused.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:929-930`).
pub const ABORTED_GIGANTIC_DIFF: &str = "Aborted generation of gigantic diff.";

/// Error when the git range yields no diff text.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:887-890`,
/// also `:893-897` for Mercurial).
pub const NO_CHANGES_FOUND: &str = "No changes found. (Did you specify the wrong commit range?)";

/// Base `git diff` options: no external drivers, no textconv, short
/// submodule view.
///
/// PHP: `ArcanistGitAPI::getDiffBaseOptions()`
/// (`src/repository/api/ArcanistGitAPI.php:442-457`).
pub const DIFF_BASE_OPTIONS: &[&str] = &["--no-ext-diff", "--no-textconv", "--submodule=short"];

/// Default diff context lines (effectively whole-file context).
///
/// PHP: `ArcanistRepositoryAPI::$diffLinesOfContext = 0x7FFF`
/// (`src/repository/api/ArcanistRepositoryAPI.php:31`).
pub const DEFAULT_DIFF_CONTEXT_LINES: u32 = 0x7FFF;

/// `git -c` overrides for diff generation.
///
/// PHP: `ArcanistGitAPI::getFullGitDiff()`
/// (`src/repository/api/ArcanistGitAPI.php:468-475`, see T13432).
pub const DIFF_CONFIG_OPTIONS: &[&str] = &["-c", "diff.suppressBlankEmpty=false"];

/// Full `git diff` option list for `context_lines` lines of context.
///
/// PHP: `ArcanistGitAPI::getDiffFullOptions()`
/// (`src/repository/api/ArcanistGitAPI.php:425-440`): base options, then
/// `--no-color --src-prefix=a/ --dst-prefix=b/ -U<n>`, then `-M -C` move
/// and rename detection.
#[must_use]
pub fn full_diff_options(context_lines: u32) -> Vec<String> {
    let mut options: Vec<String> = DIFF_BASE_OPTIONS
        .iter()
        .map(|option| (*option).to_string())
        .collect();
    options.push("--no-color".to_string());
    options.push("--src-prefix=a/".to_string());
    options.push("--dst-prefix=b/".to_string());
    options.push(format!("-U{context_lines}"));
    options.push("-M".to_string());
    options.push("-C".to_string());
    options
}

/// Generate the full unified diff of `base` against the head/worktree.
///
/// PHP: `ArcanistGitAPI::getFullGitDiff()`
/// (`src/repository/api/ArcanistGitAPI.php:464-491`): with a head, `diff
/// {options} base head --`; without, `diff {options} base --`; both
/// prefixed with the `-c` overrides. Output bytes are preserved exactly.
///
/// Divergence: git failures surface as `git diff failed: ...` (matching
/// `diff.rs`); PHP throws the raw exec exception.
pub fn get_full_git_diff(
    dir: &Path,
    base: &str,
    head: Option<&str>,
    context_lines: u32,
) -> Result<String, String> {
    let options = full_diff_options(context_lines);
    let mut args: Vec<&str> = Vec::with_capacity(options.len() + 7);
    args.extend_from_slice(DIFF_CONFIG_OPTIONS);
    args.push("diff");
    args.extend(options.iter().map(String::as_str));
    args.push(base);
    if let Some(revision) = head {
        args.push(revision);
    }
    args.push("--");
    let out = git::output(dir, &args)?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Generate the full diff text, failing when the range is empty.
///
/// PHP: the git branch of `generateChanges()`
/// (`src/workflow/ArcanistDiffWorkflow.php:883-891`): an empty diff is a
/// usage error. Diff-text parsing (`ArcanistDiffParser`) is not ported;
/// use [`collect_change_entries`] for the change list.
pub fn full_diff_text_or_fail(
    dir: &Path,
    base: &str,
    head: Option<&str>,
    context_lines: u32,
) -> Result<String, String> {
    let text = get_full_git_diff(dir, base, head, context_lines)?;
    if text.is_empty() {
        return Err(NO_CHANGES_FOUND.to_string());
    }
    Ok(text)
}

/// One `git diff --name-status -z` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameStatusEntry {
    /// Status letter (`A`, `M`, `D`, `R`, `C`, `T`, `U`).
    pub status: char,
    /// Old path for renames and copies.
    pub old_path: Option<String>,
    /// Current path.
    pub path: String,
}

/// Parse `git diff --name-status -z` output.
///
/// Rename/copy records carry a similarity score after the letter (`R100`)
/// plus old and new paths; all other records carry one path. Record shapes
/// mirror [`diff::parse_numstat_z`].
pub fn parse_name_status_z(raw: &[u8]) -> Result<Vec<NameStatusEntry>, String> {
    let mut entries = Vec::new();
    let mut fields: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    if fields.last() == Some(&b"".as_slice()) {
        fields.pop();
    }
    let mut index = 0;
    while index < fields.len() {
        let status_field = String::from_utf8_lossy(fields[index]);
        let status = status_field
            .chars()
            .next()
            .ok_or_else(|| format!("malformed name-status record {}", entries.len() + 1))?;
        index += 1;
        if status == 'R' || status == 'C' {
            let (Some(old), Some(new)) = (fields.get(index), fields.get(index + 1)) else {
                return Err(format!("truncated rename record {}", entries.len() + 1));
            };
            index += 2;
            entries.push(NameStatusEntry {
                status,
                old_path: Some(String::from_utf8_lossy(old).into_owned()),
                path: String::from_utf8_lossy(new).into_owned(),
            });
        } else {
            let Some(path) = fields.get(index) else {
                return Err(format!(
                    "malformed name-status record {}",
                    entries.len() + 1
                ));
            };
            index += 1;
            entries.push(NameStatusEntry {
                status,
                old_path: None,
                path: String::from_utf8_lossy(path).into_owned(),
            });
        }
    }
    Ok(entries)
}

/// Classify a worktree path into a `FILE_*` type.
///
/// Symlinks report `FILE_SYMLINK`; missing paths, deleted changes, and
/// non-files report `FILE_TEXT`; otherwise content with a NUL byte reports
/// `FILE_BINARY` ([`is_heuristic_binary_file`]). Unreadable files fall back
/// to `FILE_TEXT`.
fn classify_file_type(dir: &Path, path: &str, change_type: u32) -> u32 {
    if change_type == TYPE_DELETE {
        return FILE_TEXT;
    }
    let Ok(meta) = std::fs::symlink_metadata(dir.join(path)) else {
        return FILE_TEXT;
    };
    if meta.file_type().is_symlink() {
        return FILE_SYMLINK;
    }
    if !meta.is_file() {
        return FILE_TEXT;
    }
    match std::fs::read(dir.join(path)) {
        Ok(bytes) if is_heuristic_binary_file(&bytes) => FILE_BINARY,
        _ => FILE_TEXT,
    }
}

/// Build the change list for `base` against the head/worktree.
///
/// Combines `git diff --name-status -z -M -C` (change types) with
/// `git diff --numstat -z -M -C` (counts via [`diff::parse_numstat_z`]) in
/// git's own order.
///
/// Divergence: PHP parses unified diff text with `ArcanistDiffParser`
/// (`generateChanges()`, `src/workflow/ArcanistDiffWorkflow.php:802-1026`),
/// which is not ported. Status letters map as `A`->`TYPE_ADD`,
/// `M`->`TYPE_CHANGE`, `D`->`TYPE_DELETE`, `R`->`TYPE_MOVE_HERE`,
/// `C`->`TYPE_COPY_HERE`; `T` (typechange) and `U` (unmerged) fold to
/// `TYPE_CHANGE`; anything else is an error. Renames yield a single
/// `TYPE_MOVE_HERE` entry (PHP may yield paired away/here entries), and
/// deleted paths are never sniffed for binary content.
pub fn collect_change_entries(
    dir: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<ChangeEntry>, String> {
    let mut range: Vec<&str> = vec![base];
    if let Some(revision) = head {
        range.push(revision);
    }
    range.push("--");
    let status_raw = git_diff_output(dir, "--name-status", &range)?;
    let numstat_raw = git_diff_output(dir, "--numstat", &range)?;
    let counts: BTreeMap<String, (u64, u64)> = diff::parse_numstat_z(&numstat_raw)?
        .into_iter()
        .map(|file| (file.path, (file.added, file.deleted)))
        .collect();

    let mut entries = Vec::new();
    for record in parse_name_status_z(&status_raw)? {
        let (change_type, old_path) = status_change_type(&record)?;
        let (added, deleted) = counts.get(&record.path).copied().unwrap_or((0, 0));
        let file_type = classify_file_type(dir, &record.path, change_type);
        entries.push(ChangeEntry {
            change_type,
            file_type,
            old_path,
            away_paths: Vec::new(),
            current: diff::FileDiff {
                path: record.path,
                added,
                deleted,
            },
        });
    }
    Ok(entries)
}

/// Run `git diff <kind> -z -M -C <range>`; failures report trimmed stderr.
fn git_diff_output(dir: &Path, kind: &str, range: &[&str]) -> Result<Vec<u8>, String> {
    let mut args = vec!["diff", kind, "-z", "-M", "-C"];
    args.extend_from_slice(range);
    let out = git::output(dir, &args)?;
    if !out.status.success() {
        return Err(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Map a name-status letter to its `TYPE_*` change type plus the old path
/// for renames and copies; anything else is an error.
fn status_change_type(record: &NameStatusEntry) -> Result<(u32, Option<String>), String> {
    match record.status {
        'A' => Ok((TYPE_ADD, None)),
        'M' | 'T' | 'U' => Ok((TYPE_CHANGE, None)),
        'D' => Ok((TYPE_DELETE, None)),
        'R' => Ok((TYPE_MOVE_HERE, record.old_path.clone())),
        'C' => Ok((TYPE_COPY_HERE, record.old_path.clone())),
        other => Err(format!(
            "unsupported change status '{other}' for '{}'",
            record.path
        )),
    }
}

/* -(  Lint and Unit Hooks  )-------------------------------------------------- */

/// Lint workflow result codes, verbatim values.
///
/// PHP: `ArcanistLintWorkflow::RESULT_*`
/// (`src/workflow/ArcanistLintWorkflow.php:8-11`).
pub const LINT_RESULT_OKAY: i32 = 0;
/// Lint issued unresolved warnings (`RESULT_WARNINGS`).
pub const LINT_RESULT_WARNINGS: i32 = 1;
/// Lint raised errors (`RESULT_ERRORS`).
pub const LINT_RESULT_ERRORS: i32 = 2;
/// Lint was skipped (`RESULT_SKIP`).
pub const LINT_RESULT_SKIP: i32 = 3;

/// Unit workflow result codes, verbatim values.
///
/// PHP: `ArcanistUnitWorkflow::RESULT_*`
/// (`src/workflow/ArcanistUnitWorkflow.php:8-11`).
pub const UNIT_RESULT_OKAY: i32 = 0;
/// Failures, all in unsound tests (`RESULT_UNSOUND`).
pub const UNIT_RESULT_UNSOUND: i32 = 1;
/// Unit testing raised errors (`RESULT_FAIL`).
pub const UNIT_RESULT_FAIL: i32 = 2;
/// Unit testing was skipped (`RESULT_SKIP`).
pub const UNIT_RESULT_SKIP: i32 = 3;

/// Map a lint result to its diff-spec status word.
///
/// PHP: `getLintStatus()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2188-2196`): `okay`, `fail`,
/// `warn`, `skip`, defaulting to `none` (which covers `null`, i.e. no
/// engine configured).
#[must_use]
pub fn lint_status(lint_result: Option<i32>) -> &'static str {
    match lint_result {
        Some(LINT_RESULT_OKAY) => "okay",
        Some(LINT_RESULT_ERRORS) => "fail",
        Some(LINT_RESULT_WARNINGS) => "warn",
        Some(LINT_RESULT_SKIP) => "skip",
        _ => "none",
    }
}

/// Map a unit result to its diff-spec status word.
///
/// PHP: `getUnitStatus()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2202-2210`): `okay`, `fail`,
/// `warn`, `skip`, defaulting to `none`.
#[must_use]
pub fn unit_status(unit_result: Option<i32>) -> &'static str {
    match unit_result {
        Some(UNIT_RESULT_OKAY) => "okay",
        Some(UNIT_RESULT_FAIL) => "fail",
        Some(UNIT_RESULT_UNSOUND) => "warn",
        Some(UNIT_RESULT_SKIP) => "skip",
        _ => "none",
    }
}

/// Map a unit result to a Harbormaster message type.
///
/// PHP: `ArcanistUnitWorkflow::getHarbormasterTypeFromResult()`
/// (`src/workflow/ArcanistUnitWorkflow.php:388-400`): `okay` and `skip`
/// report `pass`, everything else `fail`.
#[must_use]
pub fn harbormaster_type_from_result(unit_result: i32) -> &'static str {
    match unit_result {
        UNIT_RESULT_OKAY | UNIT_RESULT_SKIP => "pass",
        _ => "fail",
    }
}

/// True when lint must be skipped (reporting `RESULT_SKIP`).
///
/// PHP: `runLint()` (`src/workflow/ArcanistDiffWorkflow.php:1146-1151`):
/// `--nolint`, a raw diff source, or an explicit `--head` all skip. PHP
/// tests argument truthiness, so an empty `--head=` does not skip.
#[must_use]
pub fn should_skip_lint(nolint: bool, is_raw: bool, head: Option<&str>) -> bool {
    nolint || is_raw || head.is_some_and(|h| !h.is_empty())
}

/// True when unit tests must be skipped (reporting `RESULT_SKIP`).
///
/// PHP: `runUnit()` (`src/workflow/ArcanistDiffWorkflow.php:1214-1219`):
/// `--nounit`, a raw diff source, or an explicit `--head` all skip. Same
/// head truthiness as [`should_skip_lint`]: an empty `--head=` does not skip.
#[must_use]
pub fn should_skip_unit(nounit: bool, is_raw: bool, head: Option<&str>) -> bool {
    nounit || is_raw || head.is_some_and(|h| !h.is_empty())
}

/// `diff` flags passed through to the `lint` child workflow, in spec order.
///
/// PHP: `getArguments()`
/// (`src/workflow/ArcanistDiffWorkflow.php:202-237`): `apply-patches`,
/// `never-apply-patches`, `amend-all`, `amend-autofixes`, all valueless.
pub const LINT_PASSTHRU_FLAGS: &[&str] = &[
    "apply-patches",
    "never-apply-patches",
    "amend-all",
    "amend-autofixes",
];

/// `diff` flags passed through to the `unit` child workflow, in spec order.
///
/// PHP: `getArguments()`
/// (`src/workflow/ArcanistDiffWorkflow.php:305-319`): `coverage`,
/// `no-coverage`, both valueless.
pub const UNIT_PASSTHRU_FLAGS: &[&str] = &["coverage", "no-coverage"];

/// Build the passthrough argv for a child workflow from the flags the user
/// set.
///
/// PHP: `ArcanistWorkflow::getPassthruArgumentsAsArgv()`
/// (`src/workflow/ArcanistWorkflow.php:1547-1558`): each set flag becomes
/// `--flag` in argument-spec order (plus its value when the spec declares
/// a `param`; no lint/unit passthrough flag declares one). Unknown commands
/// yield no flags, as in PHP.
#[must_use]
pub fn passthru_argv(command: &str, set_flags: &[String]) -> Vec<String> {
    let spec = match command {
        "lint" => LINT_PASSTHRU_FLAGS,
        "unit" => UNIT_PASSTHRU_FLAGS,
        _ => return Vec::new(),
    };
    spec.iter()
        .filter(|flag| set_flags.iter().any(|set| set == *flag))
        .map(|flag| format!("--{flag}"))
        .collect()
}

/// Extend child-workflow argv with `--rev <base>` for commit-range linting.
///
/// PHP: `runLint()` (`src/workflow/ArcanistDiffWorkflow.php:1157-1161`) and
/// `runUnit()` (`:1225-1229`) append `--rev` plus the base commit when the
/// repository supports commit ranges (git/hg; svn never). Callers pass
/// `None` for repositories without commit ranges.
#[must_use]
pub fn lint_unit_argv(passthru: &[String], base_commit: Option<&str>) -> Vec<String> {
    let mut argv = passthru.to_vec();
    if let Some(base) = base_commit {
        argv.push("--rev".to_string());
        argv.push(base.to_string());
    }
    argv
}

/// Combined lint/unit outcome, keyed exactly like PHP.
///
/// PHP: `runLintUnit()`
/// (`src/workflow/ArcanistDiffWorkflow.php:1131-1140`): `lintResult`,
/// `unresolvedLint`, `unitResult`, `testResults`.
/// [`LintUnitReport::to_json`] emits those keys verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintUnitReport {
    /// Lint result code (`lintResult`; `None` when no engine ran).
    pub lint_result: Option<i32>,
    /// Unresolved lint message dictionaries (`unresolvedLint`).
    pub unresolved_lint: Vec<serde_json::Value>,
    /// Unit result code (`unitResult`; `None` when no engine ran).
    pub unit_result: Option<i32>,
    /// Unit test result dictionaries (`testResults`).
    pub test_results: Vec<serde_json::Value>,
}

impl LintUnitReport {
    /// Render with the verbatim PHP keys.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "lintResult": self.lint_result,
            "unresolvedLint": self.unresolved_lint,
            "unitResult": self.unit_result,
            "testResults": self.test_results,
        })
    }

    /// Empty report: no engines ran (`None` codes, empty dictionaries).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            lint_result: None,
            unresolved_lint: Vec::new(),
            unit_result: None,
            test_results: Vec::new(),
        }
    }
}

/// Outcome of a lint/unit child run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildOutcome {
    /// Workflow result code (`RESULT_OKAY`/`WARNINGS`/`ERRORS`/`SKIP` for
    /// lint; `RESULT_OKAY`/`UNSOUND`/`FAIL`/`SKIP` for unit).
    Code(i32),
    /// `ArcanistNoEngineException`: nothing is configured.
    NoEngine,
    /// `ArcanistNoEffectException`, with its message.
    NoEffect(String),
    /// The child failed outside its result codes (configuration or
    /// runtime error); the diff aborts with the message, like PHP's
    /// uncaught child exceptions.
    Failed(String),
}

/// Abort message, from `ArcanistUserAbortException`
/// (`src/exception/usage/ArcanistUserAbortException.php:9-11`).
pub const USER_ABORT: &str = "User aborted the workflow.";

/// Numeric value of a lint/unit child exit code (0-3).
///
/// Both workflows return their result code verbatim (`ExitCode::from`)
/// or `FAILURE` for internal errors; anything unrecognized maps to
/// `1` (warnings/unsound), matching the `FAILURE` shape.
fn exit_code_value(code: std::process::ExitCode) -> i32 {
    use std::process::ExitCode;
    if code == ExitCode::SUCCESS {
        0
    } else if code == ExitCode::from(2) {
        2
    } else if code == ExitCode::from(3) {
        3
    } else {
        1
    }
}

/// Prefix of lint's no-engine error
/// (`lint::engine_for_dir` → "No lint engine is configured ...").
const LINT_NO_ENGINE_PREFIX: &str = "No lint engine is configured";

/// Run `lint` in-process, mapping to a [`ChildOutcome`].
///
/// `NoEngine` is probed via [`lint::engine_for_dir`]; anything else that
/// fails before the workflow runs aborts. Result codes come from the
/// workflow exit code verbatim.
///
/// Divergence: a workflow error inside `lint::run_in` (bad configuration
/// surfacing late, patch-application failures) exits `FAILURE` and maps
/// to `Code(1)` (`RESULT_WARNINGS`) instead of aborting: `run_in`
/// cannot distinguish its own errors from result codes. Surfacing real
/// errors needs a fallible lint entry point (lint owner).
pub fn run_lint_in_process(cwd: &Path, argv: &[String]) -> ChildOutcome {
    match lint::engine_for_dir(cwd) {
        Ok(_) => {}
        Err(message) if message.starts_with(LINT_NO_ENGINE_PREFIX) => {
            return ChildOutcome::NoEngine;
        }
        Err(message) => return ChildOutcome::Failed(message),
    }
    ChildOutcome::Code(exit_code_value(lint::run_in(cwd, argv)))
}

/// Run `unit` in-process, mapping to a [`ChildOutcome`].
///
/// `NoEngine` mirrors PHP's engine lookup (`--engine` never passes
/// through from `diff`, so only `unit.engine` and `.arcunit` count).
/// Same late-workflow-error caveat as [`run_lint_in_process`].
///
/// The caller must pass the process working directory: `unit::run` has
/// no `cwd` parameter (a `unit::run_in` is unit-owner work).
pub fn run_unit_in_process(cwd: &Path, argv: &[String]) -> ChildOutcome {
    if unit::find_unit_engine_from(cwd).is_none() && unit::find_arcunit_from(cwd).is_none() {
        return ChildOutcome::NoEngine;
    }
    ChildOutcome::Code(exit_code_value(unit::run(argv)))
}

/// Print a lint/unit result banner (`** TITLE ** detail`), with console
/// markup stripped to plain text like the submit reports.
fn write_banner(out: &mut dyn Write, title: &str, detail: &str) -> Result<(), String> {
    writeln!(out, "** {title} ** {detail}").map_err(|e| format!("failed to write output: {e}"))
}

/// Run lint (`runLint()`, `:1146-1207`).
///
/// `base` is `Some` only when the repository supports commit ranges
/// (git/hg); `set_flags` holds the user's set passthrough flags;
/// `skip` is [`should_skip_lint`]; `run_child` executes the child (see
/// [`run_lint_in_process`]). Notices and banners go to `out`. Returns
/// the lint result, or `None` when no engine ran.
///
/// Divergence: `setShouldAmendChanges()` (bare, from `diff`'s
/// `shouldAmend()`) has no argv form, so plain `arc diff` runs lint
/// without amending; explicit `--amend-all` / `--amend-autofixes`
/// pass through exactly. Unresolved message dictionaries are always
/// empty: in-process runs return only the result code (a
/// message-returning lint API is lint-owner work, and would also feed
/// the `arc:lint` diff property).
pub fn run_lint<W: Write>(
    cwd: &Path,
    base: Option<&str>,
    set_flags: &[String],
    skip: bool,
    run_child: &dyn Fn(&Path, &[String]) -> ChildOutcome,
    out: &mut W,
) -> Result<Option<i32>, String> {
    if skip {
        return Ok(Some(LINT_RESULT_SKIP));
    }
    let argv = lint_unit_argv(&passthru_argv("lint", set_flags), base);
    writeln!(out, "{LINTING}").map_err(|e| format!("failed to write output: {e}"))?;
    match run_child(cwd, &argv) {
        ChildOutcome::NoEngine => {
            writeln!(out, "{NO_LINT_ENGINE}")
                .map_err(|e| format!("failed to write output: {e}"))?;
            Ok(None)
        }
        ChildOutcome::NoEffect(message) => {
            writeln!(out, "{message}").map_err(|e| format!("failed to write output: {e}"))?;
            Ok(None)
        }
        ChildOutcome::Failed(message) => Err(message),
        ChildOutcome::Code(result) => {
            if let Some((title, detail)) = lint_result_notice(result) {
                write_banner(out, title, detail)?;
            }
            Ok(Some(result))
        }
    }
}

/// Run unit tests (`runUnit()`, `:1213-1278`).
///
/// Shape mirrors [`run_lint`]; the `RESULT_UNSOUND` prompt reads from
/// `input` (default yes, like `phutil_console_confirm()`), and declining
/// aborts with [`USER_ABORT`]. Test-result dictionaries are always
/// empty (same in-process caveat as [`run_lint`]).
pub fn run_unit<R: BufRead, W: Write>(
    cwd: &Path,
    base: Option<&str>,
    set_flags: &[String],
    skip: bool,
    run_child: &dyn Fn(&Path, &[String]) -> ChildOutcome,
    input: &mut R,
    out: &mut W,
) -> Result<Option<i32>, String> {
    if skip {
        return Ok(Some(UNIT_RESULT_SKIP));
    }
    let argv = lint_unit_argv(&passthru_argv("unit", set_flags), base);
    writeln!(out, "{RUNNING_UNIT_TESTS}").map_err(|e| format!("failed to write output: {e}"))?;
    match run_child(cwd, &argv) {
        ChildOutcome::NoEngine => {
            writeln!(out, "{NO_UNIT_ENGINE}")
                .map_err(|e| format!("failed to write output: {e}"))?;
            Ok(None)
        }
        ChildOutcome::NoEffect(message) => {
            writeln!(out, "{message}").map_err(|e| format!("failed to write output: {e}"))?;
            Ok(None)
        }
        ChildOutcome::Failed(message) => Err(message),
        ChildOutcome::Code(result) => {
            if result == UNIT_RESULT_UNSOUND
                && !diff::prompt_confirm_from(input, out, UNSOUND_PROMPT, false)?
            {
                return Err(USER_ABORT.to_string());
            }
            if let Some((title, detail)) = unit_result_notice(result) {
                write_banner(out, title, detail)?;
            }
            Ok(Some(result))
        }
    }
}

/// Run lint and unit together (`runLintUnit()`, `:1131-1140`).
///
/// Returns the combined [`LintUnitReport`] (dictionaries always empty;
/// see [`run_lint`]).
#[allow(clippy::too_many_arguments)]
pub fn run_lint_unit<R: BufRead, W: Write>(
    cwd: &Path,
    base: Option<&str>,
    lint_flags: &[String],
    unit_flags: &[String],
    skip_lint: bool,
    skip_unit: bool,
    run_lint_child: &dyn Fn(&Path, &[String]) -> ChildOutcome,
    run_unit_child: &dyn Fn(&Path, &[String]) -> ChildOutcome,
    input: &mut R,
    out: &mut W,
) -> Result<LintUnitReport, String> {
    let lint_result = run_lint(cwd, base, lint_flags, skip_lint, run_lint_child, out)?;
    let unit_result = run_unit(cwd, base, unit_flags, skip_unit, run_unit_child, input, out)?;
    Ok(LintUnitReport {
        lint_result,
        unresolved_lint: Vec::new(),
        unit_result,
        test_results: Vec::new(),
    })
}

/// Diff property holding unresolved lint (`updateLintDiffProperty()`, `:2296-2304`).
pub const DIFF_PROPERTY_LINT: &str = "arc:lint";
/// Diff property holding unit results (`updateUnitDiffProperty()`, `:2314-2320`).
pub const DIFF_PROPERTY_UNIT: &str = "arc:unit";
/// Diff property holding local commits (`updateLocalDiffProperty()`, `:2328-2339`).
pub const DIFF_PROPERTY_LOCAL_COMMITS: &str = "local:commits";
/// Diff property holding onto targets (`updateOntoDiffProperty()`, `:2341-2349`).
pub const DIFF_PROPERTY_ONTO: &str = "arc:onto";
/// Diff property holding staging status (`submitChangesToStagingArea()`, `:2617-2619`).
pub const DIFF_PROPERTY_STAGING: &str = "arc.staging";

/// True when unresolved lint must be stored on the diff.
///
/// PHP: `updateLintDiffProperty()` (`:2296-2304`): only when autotargets
/// did not already receive the results and lint messages are unresolved.
/// The Conduit update itself (`differential.setdiffproperty`, `:2403-2411`)
/// belongs to D2.
#[must_use]
pub fn should_update_lint_property(hit_autotargets: bool, has_unresolved_lint: bool) -> bool {
    !hit_autotargets && has_unresolved_lint
}

/// True when unit results must be stored on the diff.
///
/// PHP: `updateUnitDiffProperty()` (`:2314-2320`).
#[must_use]
pub fn should_update_unit_property(hit_autotargets: bool, has_test_results: bool) -> bool {
    !hit_autotargets && has_test_results
}

/// True when local commit info must be stored on the diff.
///
/// PHP: `updateLocalDiffProperty()` (`:2328-2339`): raw sources and empty
/// commit info store nothing.
#[must_use]
pub fn should_update_local_property(is_raw: bool, has_local_info: bool) -> bool {
    !is_raw && has_local_info
}

/// True when onto targets must be stored on the diff.
///
/// PHP: `updateOntoDiffProperty()` (`:2341-2349`): empty targets store
/// nothing.
#[must_use]
pub fn should_update_onto_property(has_targets: bool) -> bool {
    has_targets
}

/* -(  Diff Specification  )--------------------------------------------------- */

/// Creation method marker for arc-built diffs.
///
/// PHP: `buildDiffSpecification()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2272`).
pub const CREATION_METHOD: &str = "arc";

/// Source control name reported for git checkouts.
///
/// PHP: `ArcanistGitAPI::getSourceControlSystemName()`
/// (`src/repository/api/ArcanistGitAPI.php:49-51`).
pub const SOURCE_CONTROL_SYSTEM_GIT: &str = "git";

/// Local half of the `differential.creatediff` specification.
///
/// PHP: `buildDiffSpecification()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2216-2283`): `sourceMachine`,
/// `sourcePath`, `branch`, `bookmark`, `sourceControlSystem`,
/// `sourceControlPath`, `sourceControlBaseRevision`, `creationMethod`, plus
/// `repositoryPHID` when known. [`DiffSpec::to_json`] emits those keys
/// verbatim, keeping `null`s like PHP does (except `repositoryPHID`, which
/// PHP omits when unknown, `:2275-2280`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSpec {
    /// Creating machine name (`sourceMachine`).
    pub source_machine: String,
    /// Working copy path (`sourcePath`).
    pub source_path: Option<String>,
    /// Current branch (`branch`; `None` when detached).
    pub branch: Option<String>,
    /// Active bookmark (`bookmark`; always `None` for git).
    pub bookmark: Option<String>,
    /// VCS name (`sourceControlSystem`).
    pub source_control_system: Option<String>,
    /// VCS path (`sourceControlPath`; always `None` for git).
    pub source_control_path: Option<String>,
    /// Resolved base revision (`sourceControlBaseRevision`).
    pub source_control_base_revision: Option<String>,
    /// Creation method (`creationMethod`, always `arc`).
    pub creation_method: String,
    /// Repository PHID (`repositoryPHID`; Conduit-resolved by D2).
    pub repository_phid: Option<String>,
}

impl DiffSpec {
    /// Render with the verbatim PHP keys.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut spec = serde_json::json!({
            "sourceMachine": self.source_machine,
            "sourcePath": self.source_path,
            "branch": self.branch,
            "bookmark": self.bookmark,
            "sourceControlSystem": self.source_control_system,
            "sourceControlPath": self.source_control_path,
            "sourceControlBaseRevision": self.source_control_base_revision,
            "creationMethod": self.creation_method,
        });
        if let Some(phid) = &self.repository_phid
            && let Some(object) = spec.as_object_mut()
        {
            object.insert(
                "repositoryPHID".to_string(),
                serde_json::Value::String(phid.clone()),
            );
        }
        spec
    }
}

/// Best-effort local machine name for `sourceMachine`.
///
/// Divergence: PHP uses `php_uname('n')`
/// (`src/workflow/ArcanistDiffWorkflow.php:2265`), which has no
/// dependency-free Rust equivalent; the `HOSTNAME`/`COMPUTERNAME`
/// environment is consulted instead, falling back to `unknown`.
#[must_use]
pub fn default_source_machine_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Load the local diff specification for `dir` at `base`.
///
/// Wires [`get_branch_name`], worktree root, and base `rev-parse` (PHP's
/// `getSourceControlBaseRevision()`,
/// `src/repository/api/ArcanistGitAPI.php:593-598`) into [`DiffSpec`]. Raw
/// sources leave every repository field unset (PHP `:2227`). The
/// `repositoryPHID` lookup needs Conduit and stays caller-provided (D2).
pub fn load_diff_specification(
    dir: &Path,
    base: &str,
    source_machine: &str,
    repository_phid: Option<&str>,
    is_raw: bool,
) -> Result<DiffSpec, String> {
    if is_raw {
        return Ok(DiffSpec {
            source_machine: source_machine.to_string(),
            source_path: None,
            branch: None,
            bookmark: None,
            source_control_system: None,
            source_control_path: None,
            source_control_base_revision: None,
            creation_method: CREATION_METHOD.to_string(),
            repository_phid: None,
        });
    }
    let branch = get_branch_name(dir)?;
    let source_path = git::stdout_text(dir, &["rev-parse", "--show-toplevel"]).ok();
    let base_revision = git::stdout_text(dir, &["rev-parse", base])?;
    Ok(DiffSpec {
        source_machine: source_machine.to_string(),
        source_path,
        branch,
        bookmark: None,
        source_control_system: Some(SOURCE_CONTROL_SYSTEM_GIT.to_string()),
        source_control_path: None,
        source_control_base_revision: Some(base_revision),
        creation_method: CREATION_METHOD.to_string(),
        repository_phid: repository_phid.map(str::to_string),
    })
}

/// One land-onto target, keyed exactly like PHP.
///
/// PHP: `getDiffOntoTargets()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2368-2388`): `type`, `name`,
/// `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OntoTarget {
    /// Target type (`type`, always `branch`).
    pub target_type: String,
    /// Branch name (`name`).
    pub name: String,
    /// Target kind (`kind`).
    pub kind: String,
}

impl OntoTarget {
    /// Render with the verbatim PHP keys.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": self.target_type,
            "name": self.name,
            "kind": self.kind,
        })
    }
}

/// Onto-target kind for upstream tracking branches (`:2372`).
pub const ONTO_KIND_UPSTREAM: &str = "upstream";

/// Config key for the default onto target, doubling as its kind.
///
/// PHP: `getDiffOntoTargets()`
/// (`src/workflow/ArcanistDiffWorkflow.php:2378-2389`).
pub const DIFF_ONTO_CONFIG_KEY: &str = "arc.land.onto.default";

/// Resolve land-onto targets from the upstream branch or config.
///
/// PHP: `getDiffOntoTargets()` (`:2351-2392`): raw sources and non-git
/// repositories yield nothing (handled by only calling this for git); a
/// tracked upstream branch wins (`kind: upstream`, `:2362-2376`);
/// otherwise `arc.land.onto.default` is used (`:2378-2389`).
#[must_use]
pub fn diff_onto_targets(
    remote_branch: Option<&str>,
    onto_default: Option<&str>,
) -> Vec<OntoTarget> {
    if let Some(name) = remote_branch
        && !name.is_empty()
    {
        return vec![OntoTarget {
            target_type: "branch".to_string(),
            name: name.to_string(),
            kind: ONTO_KIND_UPSTREAM.to_string(),
        }];
    }
    if let Some(name) = onto_default
        && !name.is_empty()
    {
        return vec![OntoTarget {
            target_type: "branch".to_string(),
            name: name.to_string(),
            kind: DIFF_ONTO_CONFIG_KEY.to_string(),
        }];
    }
    Vec::new()
}

/// Resolve the upstream tracking branch short name (`origin/main`).
///
/// Divergence: PHP resolves through push-path machinery
/// (`getPathToUpstream()->getRemoteBranchName()`,
/// `src/workflow/ArcanistDiffWorkflow.php:2364-2367`); this uses
/// `git rev-parse --abbrev-ref @{upstream}` and yields `None` without an
/// upstream.
#[must_use]
pub fn resolve_upstream_branch(dir: &Path) -> Option<String> {
    let out = git::output(dir, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/* -(  Workflow Predicates  )-------------------------------------------------- */

/// True when the diff comes from stdin/a command rather than the worktree.
///
/// PHP: `isRawDiffSource()`
/// (`src/workflow/ArcanistDiffWorkflow.php:341-343`).
#[must_use]
pub fn is_raw_diff_source(raw: bool, raw_command: Option<&str>) -> bool {
    raw || raw_command.is_some()
}

/// True when only a diff (no revision) should be created.
///
/// PHP: `shouldOnlyCreateDiff()`
/// (`src/workflow/ArcanistDiffWorkflow.php:724-738`): `--create` and
/// `--update` force revision mode; raw sources force diff-only mode;
/// otherwise `--only` decides.
///
/// Divergence: PHP tests argument truthiness; only `None` updates count as
/// unset here (empty updates are unreachable via CLI parsing).
#[must_use]
pub fn should_only_create_diff(
    create: bool,
    update: Option<&str>,
    is_raw: bool,
    only: bool,
) -> bool {
    if create {
        return false;
    }
    if update.is_some() {
        return false;
    }
    if is_raw {
        return true;
    }
    only
}

/// True when the worktree commit message should be amended after submit.
///
/// PHP: `shouldAmend()` (`src/workflow/ArcanistDiffWorkflow.php:1102-1122`):
/// raw sources, `--no-amend`, explicit `--head`, and immutable history
/// (`isHistoryImmutable()`,
/// `src/workflow/ArcanistWorkflow.php:1595-1604`, backed by the
/// `history.immutable` config) all disable amending.
#[must_use]
pub fn should_amend(
    is_raw: bool,
    no_amend: bool,
    head: Option<&str>,
    history_immutable: bool,
) -> bool {
    if is_raw {
        return false;
    }
    if no_amend {
        return false;
    }
    if head.is_some() {
        return false;
    }
    if history_immutable {
        return false;
    }
    true
}

/* -(  Unified Diff Parser (G1 full-state)  )------------------------------------ */

/// One parsed hunk: header offsets plus the verbatim corpus.
///
/// PHP: `ArcanistDiffHunk` (`src/parser/diff/ArcanistDiffHunk.php`); the
/// corpus is the `@@` header line plus every body line through the next
/// header, exactly as `ArcanistDiffParser` accumulates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHunk {
    /// Old-file start line (`-old_start`).
    pub old_start: u64,
    /// Old-file line count (`-old_start,old_len`; bare start means 1).
    pub old_len: u64,
    /// New-file start line (`+new_start`).
    pub new_start: u64,
    /// New-file line count.
    pub new_len: u64,
    /// Verbatim hunk text: header line plus body lines, `\n`-joined with a
    /// trailing newline when non-empty.
    pub corpus: String,
}

/// One parsed file change (or commit message) from unified diff text.
///
/// PHP: `ArcanistDiffChange` (`src/parser/diff/ArcanistDiffChange.php`) as
/// built by `ArcanistDiffParser::parseDiff()`. Only the fields the diff
/// workflow reads are ported: paths, types, hunks, and (for `TYPE_MESSAGE`
/// changes from `git show`/`git log`) the commit hash and message body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedChange {
    /// Old path; equals `current_path` except for renames, copies, and
    /// adds (empty here; `/dev/null` upstream).
    pub old_path: String,
    /// Current path (`getCurrentPath()`).
    pub current_path: String,
    /// `TYPE_*` change type.
    pub change_type: u32,
    /// `FILE_*` file type.
    pub file_type: u32,
    /// Parsed hunks (empty for binary and message changes).
    pub hunks: Vec<ParsedHunk>,
    /// Destination paths for old-location changes (`getAwayPaths()`),
    /// filled by the Subversion copy/move pass, else empty.
    pub away_paths: Vec<String>,
    /// Commit hash for `TYPE_MESSAGE` changes, else `None`.
    pub commit_hash: Option<String>,
    /// Message body for `TYPE_MESSAGE` changes, else `None`.
    pub message: Option<String>,
}

impl ParsedChange {
    /// True for commit-message pseudo-changes (`TYPE_MESSAGE`), which raw
    /// diffs drop (`ArcanistDiffWorkflow.php:821-826`).
    #[must_use]
    pub fn is_message(&self) -> bool {
        self.change_type == TYPE_MESSAGE
    }

    /// Total hunk corpus size in bytes (the gigantic-diff scan input).
    #[must_use]
    pub fn corpus_size(&self) -> usize {
        self.hunks.iter().map(|hunk| hunk.corpus.len()).sum()
    }
}

/// Parse unified diff text into per-file changes.
///
/// Handles `diff --git` (git), `diff -r` (hg), and `Index:` (svn) headers,
/// `---`/`+++` paths (with `a/`/`b/` prefixes and `/dev/null` for
/// add/delete), rename/copy headers, `Binary files ... differ` and
/// `GIT binary patch` markers, and `commit <hash>` message blocks from
/// `git show`/`git log`. Unknown text between blocks is skipped, mirroring
/// the PHP parser's tolerance for extended headers.
///
/// Combined diffs fail with PHP's `didFailParse()` errors: `diff --cc` /
/// `diff --combined` headers match no PHP header pattern (`:276-283`),
/// and `@@@` hunks match no PHP hunk pattern (`:897-920`). A leading
/// `diff --cc` line recovers past itself only when a later `diff -r` /
/// `diff --git` line starts a real block (`:260-272`), like PHP.
pub fn parse_unified_diff(text: &str) -> Result<Vec<ParsedChange>, String> {
    let mut changes: Vec<ParsedChange> = Vec::new();
    let mut pending_message: Option<(String, Vec<String>)> = None;
    let lines: Vec<&str> = text.lines().collect();
    let raw: Vec<&str> = split_keep_endings(text);
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(hash) = line.strip_prefix("commit ") {
            if let Some((done_hash, body)) = pending_message.take() {
                changes.push(message_change(&done_hash, &body));
            }
            let hash = hash.split_whitespace().next().unwrap_or("").to_string();
            pending_message = Some((hash, Vec::new()));
            index += 1;
            continue;
        }
        if is_diff_header(line) {
            if let Some((done_hash, body)) = pending_message.take()
                && (!body_is_empty(&body) || changes.is_empty())
            {
                changes.push(message_change(&done_hash, &body));
            }
            let (change, next) = parse_file_block(&lines, &raw, index)?;
            changes.push(change);
            index = next;
            continue;
        }
        if is_combined_diff_header(line) {
            // PHP recovers past a leading `diff --cc` line only when a
            // later `diff -r` / `diff --git` line starts a real block
            // (`:260-272`); otherwise (and always mid-diff) it fails at
            // this line (`:273-283`).
            if is_first_content_line(&lines, index)
                && let Some(found) = next_diff_start(&lines, index)
                && is_diff_header(lines[found])
            {
                index = found;
                continue;
            }
            return Err(diff_parse_error(&raw, index, EXPECTED_DIFF_START));
        }
        if let Some((_, body)) = pending_message.as_mut() {
            body.push(line.to_string());
        }
        index += 1;
    }
    if let Some((hash, body)) = pending_message {
        changes.push(message_change(&hash, &body));
    }
    Ok(changes)
}

/// Split text into lines retaining endings, like `phutil_split_lines()`.
///
/// Indices correspond 1:1 with `str::lines()`; used for parse-error
/// context, where PHP echoes raw lines verbatim.
fn split_keep_endings(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split_inclusive('\n').collect()
    }
}

/// `didFailParse()` message for a bad hunk header (`:917-920`).
pub const EXPECTED_HUNK_HEADER: &str = "Expected hunk header '@@ -NN,NN +NN,NN @@'.";

/// `didFailParse()` message when no diff header matches (`:276-283`).
///
/// Used here only for combined-diff (`diff --cc`) headers; other
/// unrecognized text between blocks stays skipped (see the
/// [`parse_unified_diff`] docs).
pub const EXPECTED_DIFF_START: &str = "Expected a hunk header, like 'Index: /path/to/file.ext' (svn), 'Property changes on: /path/to/file.ext' (svn properties), 'commit 59bcc3ad6775562f845953cf01624225' (git show), 'diff --git' (git diff), '--- filename' (unified diff), or 'diff -r' (hg diff or patch).";

/// Combined-diff header (`git diff --cc` / `git diff --combined`).
fn is_combined_diff_header(line: &str) -> bool {
    line.starts_with("diff --cc") || line.starts_with("diff --combined")
}

/// Whether `lines[index]` is the first content line, like PHP's
/// `isFirstNonEmptyLine()` (`:1162-1181`): blank and `#`-comment lines
/// do not count.
fn is_first_content_line(lines: &[&str], index: usize) -> bool {
    for (position, line) in lines.iter().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        return position == index;
    }
    false
}

/// Whether a line looks like a diff start, like PHP's
/// `nextLineThatLooksLikeDiffStart()` (`:1151-1158`):
/// `/^\s*diff\s+-(?:r|-git)/`.
fn looks_like_diff_start(line: &str) -> bool {
    const WS: [char; 5] = [' ', '\t', '\r', '\x0C', '\x0B'];
    let rest = line.trim_start_matches(WS);
    let Some(after_diff) = rest.strip_prefix("diff") else {
        return false;
    };
    let trimmed = after_diff.trim_start_matches(WS);
    if trimmed.len() == after_diff.len() {
        return false;
    }
    let Some(after_dash) = trimmed.strip_prefix('-') else {
        return false;
    };
    after_dash.starts_with('r') || after_dash.starts_with("-git")
}

/// Find the next diff-start-looking line after `index`, like PHP's
/// `nextLineThatLooksLikeDiffStart()`.
fn next_diff_start(lines: &[&str], index: usize) -> Option<usize> {
    (index + 1..lines.len()).find(|&position| looks_like_diff_start(lines[position]))
}

/// Right-align `text` in `width` columns, truncating like `%8.8s`.
///
/// Markers and line numbers are ASCII, so char truncation matches PHP's
/// byte truncation exactly.
fn fit_right(text: &str, width: usize) -> String {
    let clipped: String = text.chars().take(width).collect();
    format!("{clipped:>width$}")
}

/// Render PHP's `didFailParse()` exception text (`:1198-1226`).
///
/// `raw` holds the input lines with endings; `line` is the 0-based
/// failing index. `writeDiffOnFailure` defaults off, so only the message
/// plus the ±5-line context render, joined by a blank line.
fn diff_parse_error(raw: &[&str], line: usize, message: &str) -> String {
    let min = line.saturating_sub(5);
    let max = (line + 5).min(raw.len().saturating_sub(1));
    let mut context = String::new();
    for (number, raw_line) in raw.iter().enumerate().take(max + 1).skip(min) {
        let marker = if number == line { ">>>  " } else { "" };
        context.push_str(&format!(
            "{} {}   {}",
            fit_right(marker, 8),
            fit_right(&(number + 1).to_string(), 6),
            raw_line
        ));
    }
    format!("Diff Parse Exception: {message}\n\n{context}")
}

/// Drop `TYPE_MESSAGE` changes from a raw parse (`:821-826`).
#[must_use]
pub fn without_message_changes(changes: &[ParsedChange]) -> Vec<ParsedChange> {
    changes
        .iter()
        .filter(|change| !change.is_message())
        .cloned()
        .collect()
}

fn body_is_empty(body: &[String]) -> bool {
    body.iter().all(|line| line.trim().is_empty())
}

fn message_change(hash: &str, body: &[String]) -> ParsedChange {
    ParsedChange {
        old_path: String::new(),
        current_path: String::new(),
        change_type: TYPE_MESSAGE,
        file_type: FILE_TEXT,
        hunks: Vec::new(),
        away_paths: Vec::new(),
        commit_hash: Some(hash.to_string()),
        message: Some(body.join("\n")),
    }
}

/// Subversion binary-file marker (`parseIndexHunk()`,
/// `src/parser/ArcanistDiffParser.php:734-738`).
pub const SVN_BINARY_MARKER: &str = "Cannot display: file marked as a binary type.";

/// True for the Subversion binary-file marker line. PHP matches the
/// right-trimmed line exactly (`:735-738`), so leading whitespace
/// disqualifies (hunk content never matches).
#[must_use]
pub fn is_svn_binary_marker(line: &str) -> bool {
    line.trim_end() == SVN_BINARY_MARKER
}

fn is_diff_header(line: &str) -> bool {
    line.starts_with("diff --git ")
        || line.starts_with("diff -r ")
        || line.starts_with("diff -u ")
        || line.starts_with("Index: ")
}

/// Parse one `diff`/`Index:` block starting at `lines[start]`.
///
/// `raw` parallels `lines` with endings retained, for parse errors.
fn parse_file_block(
    lines: &[&str],
    raw: &[&str],
    start: usize,
) -> Result<(ParsedChange, usize), String> {
    let mut old_path = String::new();
    let mut new_path = String::new();
    let mut change_type = TYPE_CHANGE;
    let mut file_type = FILE_TEXT;
    let mut hunks: Vec<ParsedHunk> = Vec::new();
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;
    let mut is_copy = false;
    let mut is_rename = false;
    let mut new_file = false;
    let mut deleted_file = false;
    let mut binary = false;

    // Seed paths from the header line when present (`diff --git a/o b/n`,
    // `diff -r <rev> <file>`, `Index: <file>`).
    let header = lines[start];
    if let Some(rest) = header.strip_prefix("diff --git ") {
        let mut parts = rest.split_whitespace();
        if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
            old_path = strip_ab_prefix(old).to_string();
            new_path = strip_ab_prefix(new).to_string();
        }
    } else if let Some(rest) = header.strip_prefix("Index: ") {
        old_path = rest.trim().to_string();
        new_path = old_path.clone();
    } else if (header.starts_with("diff -r ") || header.starts_with("diff -u "))
        && let Some(last) = header.split_whitespace().last()
    {
        old_path = strip_ab_prefix(last).to_string();
        new_path = old_path.clone();
    }

    let mut index = start + 1;
    // Extended headers up to `---`.
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("--- ")
            || line.starts_with("Binary files ")
            || line.starts_with("GIT binary patch")
            || is_svn_binary_marker(line)
            || line.starts_with("@@ ")
            || is_diff_header(line)
            || line.starts_with("commit ")
        {
            break;
        }
        if let Some(path) = line.strip_prefix("rename from ") {
            rename_from = Some(path.trim().to_string());
            is_rename = true;
        } else if let Some(path) = line.strip_prefix("rename to ") {
            rename_to = Some(path.trim().to_string());
            is_rename = true;
        } else if let Some(path) = line.strip_prefix("copy from ") {
            rename_from = Some(path.trim().to_string());
            is_copy = true;
        } else if let Some(path) = line.strip_prefix("copy to ") {
            rename_to = Some(path.trim().to_string());
            is_copy = true;
        } else if line.starts_with("new file mode") {
            new_file = true;
        } else if line.starts_with("deleted file mode") {
            deleted_file = true;
        }
        index += 1;
    }

    // `Index:` blocks key on the header path; `---`/`+++` only supply
    // the old path with `(...)` suffixes stripped (`parseHunkTarget()`,
    // `src/parser/ArcanistDiffParser.php:840-878`).
    let index_block = header.starts_with("Index: ");
    // `---` / `+++` path lines.
    if index < lines.len() && lines[index].starts_with("--- ") {
        let old = diff_path_value(lines[index], index_block);
        if old == "/dev/null" {
            new_file = true;
        } else if !old.is_empty() {
            old_path = old;
        }
        index += 1;
    }
    if index < lines.len() && lines[index].starts_with("+++ ") {
        let new = diff_path_value(lines[index], index_block);
        if new == "/dev/null" {
            deleted_file = true;
            new_path.clone_from(&old_path);
        } else if !new.is_empty() {
            new_path = new;
        }
        index += 1;
    }
    if index_block {
        new_path = header["Index: ".len()..].trim().to_string();
    }
    if new_path.is_empty() {
        new_path.clone_from(&old_path);
    }
    if old_path.is_empty() {
        old_path.clone_from(&new_path);
    }

    // Body: binary markers and hunks until the next block.
    while index < lines.len() {
        let line = lines[index];
        if is_diff_header(line) || is_combined_diff_header(line) || line.starts_with("commit ") {
            break;
        }
        if is_svn_binary_marker(line) {
            // `Cannot display: ...` plus its `svn:mime-type` line
            // (`:739-743`); the mime line is the next non-empty line.
            binary = true;
            index += 1;
            while index < lines.len() && lines[index].trim().is_empty() {
                index += 1;
            }
            if index < lines.len() {
                index += 1;
            }
            continue;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            binary = true;
            index += 1;
            // Skip binary payload lines (`literal ...`, base85, ...).
            while index < lines.len()
                && !lines[index].starts_with("@@ ")
                && !lines[index].starts_with("@@@")
                && !is_diff_header(lines[index])
                && !is_combined_diff_header(lines[index])
                && !lines[index].starts_with("commit ")
            {
                index += 1;
            }
            continue;
        }
        if let Some((old_start, old_len, new_start, new_len)) = parse_hunk_header(line) {
            let (hunk, next) = collect_hunk(lines, index, old_start, old_len, new_start, new_len);
            hunks.push(hunk);
            index = next;
            continue;
        }
        if line.starts_with("@@@") {
            return Err(diff_parse_error(raw, index, EXPECTED_HUNK_HEADER));
        }
        index += 1;
    }

    if binary {
        file_type = FILE_BINARY;
        hunks.clear();
    }
    if is_rename {
        change_type = TYPE_MOVE_HERE;
        if let (Some(from), Some(to)) = (rename_from, rename_to) {
            old_path = from;
            new_path = to;
        }
    } else if is_copy {
        change_type = TYPE_COPY_HERE;
        if let (Some(from), Some(to)) = (rename_from, rename_to) {
            old_path = from;
            new_path = to;
        }
    } else if new_file {
        change_type = TYPE_ADD;
    } else if deleted_file {
        change_type = TYPE_DELETE;
    }
    if old_path == "/dev/null" {
        old_path.clear();
    }

    Ok((
        ParsedChange {
            old_path,
            current_path: new_path,
            change_type,
            file_type,
            hunks,
            away_paths: Vec::new(),
            commit_hash: None,
            message: None,
        },
        index,
    ))
}

fn strip_ab_prefix(path: &str) -> &str {
    if path.len() > 2 && (path.starts_with("a/") || path.starts_with("b/")) {
        &path[2..]
    } else {
        path
    }
}

/// Strip a trailing ` (...)` hunk-target suffix (`parseHunkTarget()`,
/// `src/parser/ArcanistDiffParser.php:840-862`).
///
/// PHP's non-git remainder is `(?:\s*\(.*\))?$` with a lazy path, so a
/// value ending in `)` truncates at the first `(`.
fn strip_svn_paren_suffix(path: &str) -> &str {
    if !path.ends_with(')') {
        return path;
    }
    match path.find('(') {
        Some(open) => path[..open].trim_end(),
        None => path,
    }
}

/// Extract the path from a `---`/`+++` line, dropping hg/svn suffixes.
///
/// Handles `a/f`, `b/f`, bare `f`, quoted `"f"`, svn `f (revision N)` /
/// `f (working copy)` / `f (synthetic)` suffixes, and hg tab-separated
/// date suffixes. `Index:` blocks strip every trailing ` (...)`
/// (`parseHunkTarget()`); other blocks keep the legacy suffix checks.
fn diff_path_value(line: &str, strip_paren_suffix: bool) -> String {
    let value = line[4..].trim_end();
    // Tab separates svn/hg metadata suffixes.
    let path = value.split('\t').next().unwrap_or("").trim();
    // Svn appends ` (revision N)` / ` (working copy)` after a space.
    let path = if strip_paren_suffix {
        strip_svn_paren_suffix(path)
    } else if path.contains(" (revision ") || path.ends_with(" (working copy)") {
        path.rsplit_once(" (").map_or(path, |(head, _)| head)
    } else {
        path
    };
    let path = path.trim();
    let unquoted = if path.len() >= 2 && path.starts_with('"') && path.ends_with('"') {
        &path[1..path.len() - 1]
    } else {
        path
    };
    strip_ab_prefix(unquoted).to_string()
}

/// Parse an `@@ -a[,b] +c[,d] @@` header into its four offsets.
fn parse_hunk_header(line: &str) -> Option<(u64, u64, u64, u64)> {
    let inner = line.strip_prefix("@@ ")?.split_once("@@")?.0;
    let mut parts = inner.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_len) = parse_hunk_range(old)?;
    let (new_start, new_len) = parse_hunk_range(new)?;
    Some((old_start, old_len, new_start, new_len))
}

fn parse_hunk_range(text: &str) -> Option<(u64, u64)> {
    if let Some((start, len)) = text.split_once(',') {
        Some((start.parse().ok()?, len.parse().ok()?))
    } else {
        Some((text.parse().ok()?, 1))
    }
}

fn collect_hunk(
    lines: &[&str],
    start: usize,
    old_start: u64,
    old_len: u64,
    new_start: u64,
    new_len: u64,
) -> (ParsedHunk, usize) {
    let mut corpus = String::new();
    corpus.push_str(lines[start]);
    corpus.push('\n');
    let mut index = start + 1;
    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("@@ ")
            || line.starts_with("@@@")
            || is_diff_header(line)
            || is_combined_diff_header(line)
            || line.starts_with("commit ")
        {
            break;
        }
        corpus.push_str(line);
        corpus.push('\n');
        index += 1;
    }
    (
        ParsedHunk {
            old_start,
            old_len,
            new_start,
            new_len,
            corpus,
        },
        index,
    )
}

/* -(  Raw Sources, Hg, Svn (G1 full-state)  )----------------------------------- */

/// Notice written to stderr while reading a `--raw` diff from stdin.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:808-813`).
pub const RAW_STDIN_NOTICE: &str = "Reading diff from stdin...\n";

/// Error for an unreachable raw-source branch (`:817`).
pub const UNKNOWN_RAW_DIFF_SOURCE: &str = "Unknown raw diff source.";

/// Read a `--raw` diff fully from `reader` (stdin in production).
///
/// PHP: `file_get_contents('php://stdin')` (`:813`). The caller prints
/// [`RAW_STDIN_NOTICE`] to stderr first.
pub fn read_raw_stdin_from<R: std::io::Read>(mut reader: R) -> Result<String, String> {
    let mut text = String::new();
    reader
        .read_to_string(&mut text)
        .map_err(|e| format!("failed to read diff from stdin: {e}"))?;
    Ok(text)
}

/// Run `command` through the platform shell and capture its output.
///
/// PHP: `execx('%C', ...)` / `passthru('%C', ...)`.
///
/// On Windows the command passes verbatim (`raw_arg`): re-quoting it for
/// `CreateProcess` would corrupt inner quotes for `cmd`.
pub fn shell_command_output(command: &str) -> Result<std::process::Output, String> {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C"]);
        <std::process::Command as std::os::windows::process::CommandExt>::raw_arg(
            &mut cmd, command,
        );
        cmd.output()
            .map_err(|e| format!("failed to run shell command: {e}"))
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .args(["-c", command])
            .output()
            .map_err(|e| format!("failed to run shell command: {e}"))
    }
}

/// Run `command` through the platform shell with stdio inherited.
///
/// Same verbatim-argument rule as [`shell_command_output`].
pub fn shell_command_status(command: &str) -> Result<std::process::ExitStatus, String> {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C"]);
        <std::process::Command as std::os::windows::process::CommandExt>::raw_arg(
            &mut cmd, command,
        );
        cmd.status()
            .map_err(|e| format!("failed to run shell command: {e}"))
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh")
            .args(["-c", command])
            .status()
            .map_err(|e| format!("failed to run shell command: {e}"))
    }
}

/// Run a `--raw-command` string and capture its stdout (`:815`).
pub fn run_raw_command_text(command: &str) -> Result<String, String> {
    let output = shell_command_output(command)?;
    if !output.status.success() {
        return Err(format!(
            "raw command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `hg diff` option list for `context_lines` lines of context.
///
/// PHP: `ArcanistMercurialAPI::getDiffOptions()`
/// (`src/repository/api/ArcanistMercurialAPI.php:353-359`).
#[must_use]
pub fn hg_diff_options(context_lines: u32) -> Vec<String> {
    vec!["--git".to_string(), format!("-U{context_lines}")]
}

/// Full `hg diff` argv against `base`.
///
/// PHP: `getRawDiffText('')` (`ArcanistMercurialAPI.php:361-384`):
/// `diff {options} --rev {base} -- ''`.
#[must_use]
pub fn hg_full_diff_argv(base: &str, context_lines: u32) -> Vec<String> {
    let mut argv = vec!["diff".to_string()];
    argv.extend(hg_diff_options(context_lines));
    argv.extend(["--rev".to_string(), base.to_string(), "--".to_string()]);
    argv.push(String::new());
    argv
}

/// Generate the full Mercurial diff of `base` against the worktree.
///
/// PHP: `getFullMercurialDiff()` (`ArcanistMercurialAPI.php:382-384`).
/// Like [`get_full_git_diff`], output decodes lossy.
pub fn get_full_hg_diff(dir: &Path, base: &str, context_lines: u32) -> Result<String, String> {
    get_full_hg_diff_with(dir, &hg::HgCommand::default(), base, context_lines)
}

/// [`get_full_hg_diff`] over an injected `hg` backend (fake binaries in
/// tests).
pub fn get_full_hg_diff_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    base: &str,
    context_lines: u32,
) -> Result<String, String> {
    let argv = hg_full_diff_argv(base, context_lines);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = hg::output_with(dir, hg_cmd, &refs)?;
    if !out.status.success() {
        return Err(format!(
            "hg diff failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `hg branch` argv (`ArcanistMercurialAPI.php:70-76`).
#[must_use]
pub fn hg_branch_argv() -> Vec<String> {
    vec!["branch".to_string()]
}

/// Read the Mercurial branch name (trimmed stdout).
pub fn get_hg_branch_name(dir: &Path) -> Result<String, String> {
    get_hg_branch_name_with(dir, &hg::HgCommand::default())
}

/// [`get_hg_branch_name`] over an injected `hg` backend.
pub fn get_hg_branch_name_with(dir: &Path, hg_cmd: &hg::HgCommand) -> Result<String, String> {
    let out = hg::stdout_text_with(dir, hg_cmd, &["branch"])?;
    Ok(out.trim().to_string())
}

/// `hg status --rev <since>` argv (`ArcanistMercurialAPI.php:275-278`).
#[must_use]
pub fn hg_status_argv(since_commit: &str) -> Vec<String> {
    vec![
        "status".to_string(),
        "--rev".to_string(),
        since_commit.to_string(),
    ]
}

/// One `hg status` line: the status code plus path (and copy source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgStatusEntry {
    /// Status code (`A`, `R`, `M`, `C`, `!`, `?`, `I`).
    pub status: char,
    /// Repository-relative path.
    pub path: String,
    /// Copy/move source from a following `  <path>` line, if any.
    pub from: Option<String>,
}

/// Parse `hg status` output into entries with copy sources.
///
/// PHP: `ArcanistMercurialParser::parseMercurialStatusDetails()`
/// (`src/repository/parser/ArcanistMercurialParser.php:24-91`), including
/// both error strings. Clean (`C`) and ignored (`I`) entries are kept with
/// a zero mask so callers can filter them like PHP does.
pub fn parse_hg_status_details(output: &str) -> Result<Vec<(HgStatusEntry, u32)>, String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(HgStatusEntry, u32)> = Vec::new();
    for line in trimmed.split('\n') {
        let bytes = line.as_bytes();
        if bytes.len() < 2 || bytes[1] != b' ' {
            return Err(format!("Unparsable Mercurial status line '{line}'."));
        }
        let code = bytes[0] as char;
        let path = line[2..].to_string();
        if code == ' ' {
            let Some((last, _)) = entries.last_mut() else {
                return Err(format!("Unexpected copy source in hg status, '{line}'."));
            };
            last.from = Some(path);
            continue;
        }
        let flags = match code {
            'A' => FLAG_ADDED,
            'R' => FLAG_DELETED,
            'M' => FLAG_MODIFIED,
            'C' | 'I' => 0,
            '!' => FLAG_MISSING,
            '?' => FLAG_UNTRACKED,
            _ => return Err(format!("Unknown Mercurial status '{code}'.")),
        };
        entries.push((
            HgStatusEntry {
                status: code,
                path,
                from: None,
            },
            flags,
        ));
    }
    Ok(entries)
}

/// Parse `hg status` output into a path-to-mask map (`:103-106`).
pub fn parse_hg_status(output: &str) -> Result<Vec<(String, u32)>, String> {
    Ok(parse_hg_status_details(output)?
        .into_iter()
        .map(|(entry, flags)| (entry.path, flags))
        .collect())
}

/// `hg log` argv for the commit-message log between `base` and `.`.
///
/// PHP: `getCommitMessageLog()` (`ArcanistMercurialAPI.php:532-538`).
#[must_use]
pub fn hg_commit_log_argv(base: &str, branch: &str) -> Vec<String> {
    vec![
        "log".to_string(),
        "--template".to_string(),
        "{node}\u{1}{desc}\u{2}".to_string(),
        "--rev".to_string(),
        format!("({base}::. - {base})"),
        "--branch".to_string(),
        branch.to_string(),
        "--".to_string(),
    ]
}

/// Parse the `{node}\1{desc}\2` commit-message log into `(node, desc)` pairs
/// (`ArcanistMercurialAPI.php:542-546`).
#[must_use]
pub fn parse_hg_commit_log(output: &str) -> Vec<(String, String)> {
    output
        .trim()
        .split('\u{2}')
        .filter(|log| !log.is_empty())
        .filter_map(|log| {
            let (node, desc) = log.split_once('\u{1}')?;
            Some((node.to_string(), desc.to_string()))
        })
        .collect()
}

/// True when `hg help commit` output advertises `--amend`.
///
/// PHP: `supportsAmend()` (`ArcanistMercurialAPI.php:469-476`).
#[must_use]
pub fn hg_supports_amend_from_help(help_text: &str) -> bool {
    help_text.contains("amend")
}

/// `hg commit --amend --logfile <file> --` argv for head commits
/// (`ArcanistMercurialAPI.php:715`).
#[must_use]
pub fn hg_amend_argv(logfile: &str) -> Vec<String> {
    vec![
        "commit".to_string(),
        "--amend".to_string(),
        "--logfile".to_string(),
        logfile.to_string(),
        "--".to_string(),
    ]
}

/// Working-copy status flag: modified (`ArcanistRepositoryAPI.php:10`).
pub const FLAG_MODIFIED: u32 = 1;
/// Working-copy status flag: added (`:11`).
pub const FLAG_ADDED: u32 = 2;
/// Working-copy status flag: deleted (`:12`).
pub const FLAG_DELETED: u32 = 4;
/// Working-copy status flag: untracked (`:13`).
pub const FLAG_UNTRACKED: u32 = 8;
/// Working-copy status flag: conflicted (`:14`).
pub const FLAG_CONFLICT: u32 = 16;
/// Working-copy status flag: missing (`:15`).
pub const FLAG_MISSING: u32 = 32;
/// Working-copy status flag: unstaged (`:16`).
pub const FLAG_UNSTAGED: u32 = 64;
/// Working-copy status flag: uncommitted (`:17`).
pub const FLAG_UNCOMMITTED: u32 = 128;
/// Working-copy status flag: inside `svn:externals` (`:21`).
pub const FLAG_EXTERNALS: u32 = 256;
/// Working-copy status flag: obstructed (`:25`).
pub const FLAG_OBSTRUCTED: u32 = 512;
/// Working-copy status flag: incomplete (`:28`).
pub const FLAG_INCOMPLETE: u32 = 1024;

/// `svn --xml status` argv, limited to `paths` when non-empty.
///
/// PHP: `getSVNStatus()` (`src/repository/api/ArcanistSubversionAPI.php:69-77`).
#[must_use]
pub fn svn_status_argv(paths: &[String]) -> Vec<String> {
    let mut argv = vec!["--xml".to_string(), "status".to_string()];
    argv.extend(paths.iter().cloned());
    argv
}

/// Map an svn `wc-status` `props` value to its flag mask.
///
/// PHP: `getSVNStatus()` (`ArcanistSubversionAPI.php:100-111`).
pub fn svn_props_mask(props: &str) -> Result<u32, String> {
    match props {
        "none" | "normal" => Ok(0),
        "modified" => Ok(FLAG_MODIFIED),
        _ => Err(format!("Unrecognized property status '{props}'.")),
    }
}

/// Map an svn `wc-status` `item` value to its flag mask.
///
/// PHP: `parseSVNStatus()` (`ArcanistSubversionAPI.php:151-183`).
pub fn parse_svn_item_status(item: &str) -> Result<u32, String> {
    match item {
        "none" | "normal" => Ok(0),
        "external" => Ok(FLAG_EXTERNALS),
        "unversioned" => Ok(FLAG_UNTRACKED),
        "obstructed" => Ok(FLAG_OBSTRUCTED),
        "missing" => Ok(FLAG_MISSING),
        "added" => Ok(FLAG_ADDED),
        "replaced" | "modified" => Ok(FLAG_MODIFIED),
        "deleted" => Ok(FLAG_DELETED),
        "conflicted" => Ok(FLAG_CONFLICT),
        "incomplete" => Ok(FLAG_INCOMPLETE),
        _ => Err(format!("Unrecognized item status '{item}'.")),
    }
}

/// `svn info` argv for `path` (`buildInfoFuture()`, `:255-279`).
///
/// `/` maps to `info .` (the symlink bug workaround, `:256-271`);
/// other paths gain a trailing `@` so literal `@` parses correctly
/// (`:273-277`).
#[must_use]
pub fn svn_info_argv(path: &str) -> Vec<String> {
    if path == "/" {
        vec!["info".to_string(), ".".to_string()]
    } else {
        vec!["info".to_string(), format!("{path}@")]
    }
}

/// `svn diff` argv for `path` (`buildDiffFuture()`, `:281-314`).
///
/// `--depth empty` keeps directory diffs non-recursive. On Windows the plain
/// diff applies (`:295-302`); elsewhere `binary_safe_diff.sh` is the
/// `--diff-cmd` with `-U<context>` (`:303-313`).
#[must_use]
pub fn svn_diff_argv(path: &str, diff_cmd: Option<&str>, context_lines: u32) -> Vec<String> {
    let mut argv = vec![
        "diff".to_string(),
        "--depth".to_string(),
        "empty".to_string(),
    ];
    if let Some(cmd) = diff_cmd {
        argv.extend([
            "--diff-cmd".to_string(),
            cmd.to_string(),
            "-x".to_string(),
            format!("-U{context_lines}"),
        ]);
    }
    argv.push(path.to_string());
    argv
}

/// Render one `svn status` base-revision line (`:854`).
#[must_use]
pub fn svn_base_rev_line(baserev: i64, path: &str) -> String {
    format!("    Revision {baserev}, {path}")
}

/// Render one `svn status` base-revision line from a raw revision string (`:854`).
#[must_use]
pub fn svn_base_rev_line_str(baserev: &str, path: &str) -> String {
    format!("    Revision {baserev}, {path}")
}

/// True for base revisions PHP drops (`$bases[$path] <= 0`, `:843-847`):
/// empty or numerically non-positive. Non-numeric strings compare
/// lexicographically under PHP 8 (`"abc" <= "0"` is false), so they stay.
fn svn_baserev_is_new(baserev: &str) -> bool {
    if baserev.is_empty() {
        return true;
    }
    match baserev.parse::<i64>() {
        Ok(number) => number <= 0,
        Err(_) => false,
    }
}

/// Require consistent svn base revisions over raw revision strings.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:840-878`).
/// Comparison is strict (`!==`) on the raw strings, so `"09"` and `"9"`
/// mismatch; the first surviving value wins. Returns the effective base
/// revision, or `None` when every path is new (`:852` skips the override).
pub fn check_svn_base_revision_strings(bases: &[(&str, &str)]) -> Result<Option<String>, String> {
    let live: Vec<(&str, &str)> = bases
        .iter()
        .copied()
        .filter(|(_, rev)| !svn_baserev_is_new(rev))
        .collect();
    let Some((_, first)) = live.first() else {
        return Ok(None);
    };
    if live.iter().all(|(_, other)| *other == *first) {
        return Ok(Some((*first).to_string()));
    }
    let revlist = live
        .iter()
        .map(|(path, baserev)| svn_base_rev_line_str(baserev, path))
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Base revisions of changed paths are mismatched. Update all paths to the same base \
         revision before creating a diff: \n\n{revlist}"
    ))
}

/// Require consistent svn base revisions; new files (`<= 0`) are ignored.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:840-878`).
/// Returns the effective base revision (the common value), or 0 when
/// every path is new.
pub fn check_svn_base_consistency(bases: &[(&str, i64)]) -> Result<i64, String> {
    let rendered: Vec<String> = bases.iter().map(|(_, rev)| rev.to_string()).collect();
    let refs: Vec<(&str, &str)> = bases
        .iter()
        .zip(rendered.iter())
        .map(|((path, _), rev)| (*path, rev.as_str()))
        .collect();
    match check_svn_base_revision_strings(&refs)? {
        // The revision strings above were formatted from `i64`, so parsing
        // back cannot fail.
        Some(rev) => Ok(rev.parse::<i64>().unwrap_or(0)),
        None => Ok(0),
    }
}

/// Warning printed when `svn:externals` paths carry local modifications.
///
/// PHP: `generateAffectedPaths()`
/// (`src/workflow/ArcanistDiffWorkflow.php:768-779`). The echo format has
/// two verbs but three arguments, so `sprintf` silently drops the wrapped
/// path list: PHP prints these two paragraphs plus `"\n\n"` and never the
/// file list. Callers must not append the list.
#[must_use]
pub fn svn_externals_warning() -> String {
    "The working copy includes changes to 'svn:externals' paths. These changes will not be \
     included in the diff because SVN can not commit 'svn:externals' changes alongside normal \
     changes.\n\nModified 'svn:externals' files:"
        .to_string()
}

/// Confirm prompt after the externals warning (`:780`).
pub const PROMPT_SVN_EXTERNALS_ANYWAY: &str = "Generate a diff (with just local changes) anyway?";

/// Derive the svn branch name from the working-copy URL and repository root.
///
/// PHP: `getBranchName()` (`ArcanistSubversionAPI.php:240-249`).
#[must_use]
pub fn svn_branch_name(url: &str, repo_root: &str) -> String {
    if let Some(rest) = url.strip_prefix(repo_root) {
        rest.to_string()
    } else {
        "svn".to_string()
    }
}

/* -(  Change Review Texts (G1 full-state)  )------------------------------------- */

/// Svn-only gigantic-diff error: warn plus the binary-marking hint.
///
/// PHP: `generateChanges()` (`src/workflow/ArcanistDiffWorkflow.php:915-922`).
#[must_use]
pub fn gigantic_svn_error(path: &str, size_bytes: usize) -> String {
    format!(
        "{} If the file is not a text file, mark it as binary with:\n\n  $ svn propset \
         svn:mime-type application/octet-stream <filename>",
        gigantic_diff_warning(path, size_bytes)
    )
}

/// Confirm question appended to the gigantic-diff warning for git/hg (`:923-925`).
pub const GIGANTIC_MARK_BINARY_QUESTION: &str = "If the file is not a text file, you can mark it \
    'binary'. Mark this file as 'binary' and continue?";

/// Full gigantic-diff confirm prompt for git/hg (`:910-925`).
#[must_use]
pub fn gigantic_confirm(path: &str, size_bytes: usize) -> String {
    format!(
        "{} {GIGANTIC_MARK_BINARY_QUESTION}",
        gigantic_diff_warning(path, size_bytes)
    )
}

/// Header for the non-UTF8 warning block (`:1005-1007`).
pub const UTF8_HEADER: &str = "Invalid Content Encoding (Non-UTF8)";

/// Body of the non-UTF8 warning block for `count` affected files (`:984-1000`).
#[must_use]
pub fn utf8_warning_body(count: usize) -> String {
    format!(
        "This diff includes {count} file(s) which are not valid UTF-8 (they contain invalid byte \
         sequences). You can either stop this workflow and fix these files, or continue. If you \
         continue, these files will be marked as binary.\n\nYou can learn more about how this \
         software handles character encodings (and how to configure encoding settings and detect \
         and correct encoding problems) by reading 'User Guide: UTF-8 and Character Encoding' in \
         the documentation.\n\n    {count} AFFECTED FILE(S)\n"
    )
}

/// Confirm prompt for the non-UTF8 warning (`:1001-1003`).
#[must_use]
pub fn utf8_continue_prompt(count: usize) -> String {
    format!("Do you want to mark these {count} file(s) as binary and continue?")
}

/// Abort message when the user stops to fix encodings (`:1015`).
pub const ABORTED_UTF8: &str = "Aborted workflow to fix UTF-8.";

/// Notice when a hunk corpus is converted to UTF-8 (`:964-970`).
#[must_use]
pub fn converted_hunk_notice(name: &str, encoding: &str) -> String {
    format!("Converted a '{name}' hunk from '{encoding}' to UTF-8.\n")
}

/// Notice when the project encoding lookup fails (`:952-955`).
#[must_use]
pub fn encoding_lookup_failed(message: &str) -> String {
    format!("Lookup of encoding in project failed: {message}")
}

/// Lint start notice (`src/workflow/ArcanistDiffWorkflow.php:1155`).
pub const LINTING: &str = "Linting...";
/// Unit start notice (`:1223`).
pub const RUNNING_UNIT_TESTS: &str = "Running unit tests...";

/// Banner for a lint result, or `None` when nothing prints.
///
/// PHP: `runLint()` (`:1172-1191`). `RESULT_SKIP` and the no-engine
/// fallthrough print no banner.
#[must_use]
pub fn lint_result_notice(result: i32) -> Option<(&'static str, &'static str)> {
    match result {
        r if r == LINT_RESULT_OKAY => Some(("LINT OKAY", "No lint problems.")),
        r if r == LINT_RESULT_WARNINGS => {
            Some(("LINT MESSAGES", "Lint issued unresolved warnings."))
        }
        r if r == LINT_RESULT_ERRORS => Some(("LINT ERRORS", "Lint raised errors!")),
        _ => None,
    }
}

/// Banner for a unit result, or `None` when nothing prints.
///
/// PHP: `runUnit()` (`:1233-1262`). `RESULT_SKIP` and the no-engine
/// fallthrough print no banner.
#[must_use]
pub fn unit_result_notice(result: i32) -> Option<(&'static str, &'static str)> {
    match result {
        r if r == UNIT_RESULT_OKAY => Some(("UNIT OKAY", "No unit test failures.")),
        r if r == UNIT_RESULT_UNSOUND => Some((
            "UNIT UNSOUND",
            "Unit testing raised errors, but all failing tests are unsound.",
        )),
        r if r == UNIT_RESULT_FAIL => Some(("UNIT ERRORS", "Unit testing raised errors!")),
        _ => None,
    }
}

/// Confirm prompt for unsound-only unit failures (`:1241-1244`).
pub const UNSOUND_PROMPT: &str = "Unit test results included failures, but all failing tests are \
    known to be unsound. Ignore unsound test failures?";

/// Notice when no lint engine is configured (`:1200-1202`).
pub const NO_LINT_ENGINE: &str = "No lint engine configured for this project.";
/// Notice when no unit engine is configured (`:1270-1273`).
pub const NO_UNIT_ENGINE: &str = "No unit test engine is configured for this project.";

/// `git commit --amend` argv, reusing `HEAD` or writing `message_file`.
///
/// PHP: `ArcanistGitAPI::amendCommit()`
/// (`src/repository/api/ArcanistGitAPI.php:829-838`).
#[must_use]
pub fn git_amend_argv(message_file: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "commit".to_string(),
        "--amend".to_string(),
        "--allow-empty".to_string(),
    ];
    match message_file {
        None => argv.extend(["-C".to_string(), "HEAD".to_string()]),
        Some(path) => argv.extend(["-F".to_string(), path.to_string()]),
    }
    argv
}

/* -(  Message, Staging, Upload Texts (G1 full-state)  )-------------------------- */

/// Prompt to reuse a saved `create-message` scratch file (`:1388-1390`).
pub const PROMPT_USE_SAVED_MESSAGE: &str = "Do you want to use this message?";
/// Prompt to re-edit a message with parser errors (`:1533-1535`).
pub const PROMPT_EDIT_MESSAGE_AGAIN: &str = "Do you want to edit the message?";
/// Interactive-editor task for a new revision (`:1482-1483`).
pub const TASK_NEW_REVISION: &str = "Provide the details for a new revision, then save and exit.";
/// Interactive-editor task for update comments (`:1763-1764`).
pub const TASK_UPDATE_COMMENTS: &str = "Update the revision comments, then save and exit.";
/// Interactive-editor task for editing revision info (`:675-676`).
pub const TASK_EDIT_REVISION_INFO: &str = "Update the details for a revision, then save and exit.";
/// Error when the default create template is saved unedited (`:1489`).
pub const TEMPLATE_NOT_EDITED: &str = "Template not edited.";
/// Lead line before commit-message parser errors (`:1525`).
pub const COMMIT_MESSAGE_HAS_ERRORS: &str = "Commit message has errors:";
/// First re-edit issue line (`:1526`).
pub const RESOLVE_THESE_ERRORS: &str = "Resolve these errors:";
/// Notice after listing parser errors (`:1532`).
pub const MUST_RESOLVE_ERRORS: &str = "You must resolve these errors to continue.";
/// Error when parser errors stay unresolved (`:1543-1544`).
pub const MESSAGE_HAS_UNRESOLVED_ERRORS: &str = "Message has unresolved errors.";

/// Lead line for the saved-message preview (`:1377`).
pub const MESSAGE_BEGINS: &str = "Message begins:";

/// Truncate a saved-message first line to 64 glyphs.
///
/// PHP: `PhutilUTF8StringTruncator::setMaximumGlyphs(64)` (`:1369-1374`).
/// Glyphs are approximated with `char`s.
#[must_use]
pub fn truncate_preview_64(saved: &str) -> String {
    let first = saved.split('\n').next().unwrap_or("").trim();
    first.chars().take(64).collect()
}

/// Notice that a saved revision message exists (`:1382-1386`).
///
/// `preview` is the [`truncate_preview_64`] result; an empty preview omits
/// the `Message begins:` block (`:1376-1380`).
#[must_use]
pub fn saved_message_notice(where_path: &str, preview: &str) -> String {
    let block = if preview.is_empty() {
        String::new()
    } else {
        format!("{MESSAGE_BEGINS}\n\n       {preview}\n\n")
    };
    format!(
        "You have a saved revision message in '{where_path}'.\n{block}You can use this message, or discard it."
    )
}

/// `Included commits` block for the create-message editor (`:1428-1448`).
///
/// Returns the blank line, the lead line, another blank line, then one
/// 8-space-indented commit per entry. `branch` is `None` for raw sources.
#[must_use]
pub fn included_commits_block(branch: Option<&str>, commits: &[String]) -> Vec<String> {
    let mut block = vec![String::new()];
    match branch {
        Some(name) => block.push(format!("Included commits in branch {name}:")),
        None => block.push("Included commits:".to_string()),
    }
    block.push(String::new());
    block.extend(commits.iter().map(|commit| format!("        {commit}")));
    block
}

/// First create-message editor issue line (`:1452`).
pub const NEW_DIFFERENTIAL_REVISION: &str = "NEW DIFFERENTIAL REVISION";
/// Second create-message editor issue line (`:1453`).
pub const DESCRIBE_NEW_REVISION: &str = "Describe the changes in this new revision.";
/// Hint that no revision matched the working copy (`:1458-1459`).
pub const ARC_COULD_NOT_IDENTIFY: &str =
    "arc could not identify any existing revision in your working copy.";
/// Hint lead for updating instead (`:1460`).
pub const IF_INTENDED_UPDATE: &str = "If you intended to update an existing revision, use:";
/// Hint command for updating instead (`:1462`).
pub const UPDATE_HINT_CMD: &str = "  $ arc diff --update <revision>";

/// Full create-message editor issue list (`:1450-1466`).
#[must_use]
pub fn new_revision_issues(included: &[String], notes: &[String]) -> Vec<String> {
    let mut issues = vec![
        NEW_DIFFERENTIAL_REVISION.to_string(),
        DESCRIBE_NEW_REVISION.to_string(),
    ];
    issues.extend(included.iter().cloned());
    issues.extend([
        String::new(),
        ARC_COULD_NOT_IDENTIFY.to_string(),
        IF_INTENDED_UPDATE.to_string(),
        String::new(),
        UPDATE_HINT_CMD.to_string(),
    ]);
    if !notes.is_empty() {
        issues.push(String::new());
        issues.extend(notes.iter().cloned());
    }
    issues
}

/// Note when a commit message fails partial parsing (`:1881-1883`).
#[must_use]
pub fn commit_parse_note(short_hash: &str) -> String {
    format!("NOTE: commit {short_hash} could not be completely parsed:")
}

/// Scratch location name when the commit message itself was amended (`:1511`).
pub const WHERE_COMMIT_MESSAGE: &str = "commit message";

/// Suffix for the unresolved-errors abort (`:1541`).
#[must_use]
pub fn copy_saved_to(where_path: &str) -> String {
    format!("A copy was saved to {where_path}.")
}

/// Notice that a failed message was still saved (`:1548`).
#[must_use]
pub fn message_saved_to(where_path: &str) -> String {
    format!("(Message saved to {where_path}.)")
}

/// Staging status: refs were pushed (`:25`).
pub const STAGING_PUSHED: &str = "pushed";
/// Staging status: `--skip-staging` was given (`:26`).
pub const STAGING_USER_SKIP: &str = "user.skip";
/// Staging status: raw changes can not be pushed (`:27`).
pub const STAGING_DIFF_RAW: &str = "diff.raw";
/// Staging status: no repository PHID (`:28`).
pub const STAGING_REPOSITORY_UNKNOWN: &str = "repository.unknown";
/// Staging status: server predates staging areas (`:29`).
pub const STAGING_REPOSITORY_UNAVAILABLE: &str = "repository.unavailable";
/// Staging status: server disabled staging for the repo (`:30`).
pub const STAGING_REPOSITORY_UNSUPPORTED: &str = "repository.unsupported";
/// Staging status: no staging URI configured (`:31`).
pub const STAGING_REPOSITORY_UNCONFIGURED: &str = "repository.unconfigured";
/// Staging status: non-git working copy (`:32`).
pub const STAGING_CLIENT_UNSUPPORTED: &str = "client.unsupported";

/// Header for every staging notice (`:2624-2684`).
pub const SKIP_STAGING_HEADER: &str = "SKIP STAGING";

/// Body for a staging skip `reason` (`:2622-2674`).
#[must_use]
pub fn staging_skip_body(reason: &str) -> &'static str {
    if reason == STAGING_USER_SKIP {
        "Flag --skip-staging was specified."
    } else if reason == STAGING_DIFF_RAW {
        "Raw changes can not be pushed to a staging area."
    } else if reason == STAGING_REPOSITORY_UNKNOWN {
        "Unable to determine repository for this change."
    } else if reason == STAGING_REPOSITORY_UNAVAILABLE {
        "The server does not support staging areas."
    } else if reason == STAGING_REPOSITORY_UNSUPPORTED {
        "The server does not support staging areas for this repository."
    } else if reason == STAGING_REPOSITORY_UNCONFIGURED {
        "No staging area is configured for this repository."
    } else {
        "This client version does not support staging this repository."
    }
}

/// Staging repository configuration subset read by the push decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingConfig {
    /// `supported` flag for this repository.
    pub repo_supported: bool,
    /// Staging-area push URI, if configured.
    pub uri: Option<String>,
    /// Staging tag prefix (`prefix`, default `phabricator`).
    pub prefix: String,
    /// True for git working copies (only those can push).
    pub is_git: bool,
}

impl StagingConfig {
    /// Build a config with the default `phabricator` prefix.
    #[must_use]
    pub fn new(repo_supported: bool, uri: Option<&str>, is_git: bool) -> Self {
        Self {
            repo_supported,
            uri: uri.map(str::to_string),
            prefix: "phabricator".to_string(),
            is_git,
        }
    }
}

/// Decide whether to skip the staging push, returning the skip reason.
///
/// PHP: `pushChangesToStagingArea()` (`:2622-2674`). `None` means push;
/// `Some(reason)` is one of the `STAGING_*` constants. A `None` config
/// means the server predates staging areas (`:2644-2650`).
#[must_use]
pub fn staging_skip_reason(
    skip_staging: bool,
    is_raw: bool,
    repository_phid: Option<&str>,
    config: Option<&StagingConfig>,
) -> Option<&'static str> {
    if skip_staging {
        return Some(STAGING_USER_SKIP);
    }
    if is_raw {
        return Some(STAGING_DIFF_RAW);
    }
    if repository_phid.is_none() {
        return Some(STAGING_REPOSITORY_UNKNOWN);
    }
    let Some(config) = config else {
        return Some(STAGING_REPOSITORY_UNAVAILABLE);
    };
    if !config.repo_supported {
        return Some(STAGING_REPOSITORY_UNSUPPORTED);
    }
    if config.uri.as_deref().is_none_or(|uri| uri.is_empty()) {
        return Some(STAGING_REPOSITORY_UNCONFIGURED);
    }
    if !config.is_git {
        return Some(STAGING_CLIENT_UNSUPPORTED);
    }
    None
}

/// Header/body for the staging push start (`:2682-2684`).
pub const PUSH_STAGING_HEADER: &str = "PUSH STAGING";
/// Body for the staging push start (`:2684`).
pub const PUSH_STAGING_BODY: &str = "Pushing changes to staging area...";

/// Build a staging tag ref (`:2679-2680`). `kind` is `base` or `diff`.
#[must_use]
pub fn staging_tag(prefix: &str, kind: &str, id: i64) -> String {
    format!("refs/tags/{prefix}/{kind}/{id}")
}

/// Refspecs for the staging push: the base ref (unless the magic root
/// commit) plus the diff ref (`:2708-2729`).
///
/// LFS working copies push the base commit to the diff ref here; the real
/// head follows via [`staging_lfs_refspec`] (`:2722`).
#[must_use]
pub fn staging_refspecs(
    base_commit: &str,
    head_commit: &str,
    prefix: &str,
    id: i64,
    is_lfs: bool,
) -> Vec<String> {
    let mut refspecs = Vec::new();
    if base_commit != GIT_MAGIC_ROOT_COMMIT {
        refspecs.push(format!("{base_commit}:{}", staging_tag(prefix, "base", id)));
    }
    let diff_commit = if is_lfs { base_commit } else { head_commit };
    refspecs.push(format!("{diff_commit}:{}", staging_tag(prefix, "diff", id)));
    refspecs
}

/// Forced refspec for the LFS follow-up push (`:2749`).
#[must_use]
pub fn staging_lfs_refspec(head_commit: &str, prefix: &str, id: i64) -> String {
    format!("+{head_commit}:{}", staging_tag(prefix, "diff", id))
}

/// `git push` argv for the staging area (`:2731-2735`).
#[must_use]
pub fn staging_push_argv(
    uri: &str,
    refspecs: &[String],
    git_supports_no_verify: bool,
) -> Vec<String> {
    let mut argv = vec!["push".to_string()];
    if git_supports_no_verify {
        argv.push("--no-verify".to_string());
    }
    argv.extend(["--".to_string(), uri.to_string()]);
    argv.extend(refspecs.iter().cloned());
    argv
}

/// True when `git_version` supports `push --no-verify` (`>= 1.8.2`, `:2687`).
#[must_use]
pub fn git_supports_push_no_verify(git_version: &str) -> bool {
    compare_version(git_version, "1.8.2") != std::cmp::Ordering::Less
}

/// Compare dotted numeric version prefixes (`version_compare()` subset).
fn compare_version(left: &str, right: &str) -> std::cmp::Ordering {
    let parse = |text: &str| {
        text.split('.')
            .map(|part| {
                part.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u64>()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>()
    };
    parse(left).cmp(&parse(right))
}

/// Header for staging failures (`:2738-2740`, `:2756-2758`).
pub const STAGING_FAILED_HEADER: &str = "STAGING FAILED";
/// Body for staging push failures (`:2740`).
pub const STAGING_FAILED_BODY: &str = "Unable to push changes to the staging area.";
/// Body for LFS staging push failures (`:2758`).
pub const STAGING_FAILED_LFS_BODY: &str = "Unable to push lfs changes to the staging area.";
/// Usage error after a staging push failure (`:2742-2745`).
pub const STAGING_PUSH_FAILED: &str = "Failed to push changes to staging area. Correct the issue, or use --skip-staging to skip \
     this step.";
/// Usage error after an LFS staging push failure (`:2760-2763`).
pub const STAGING_LFS_PUSH_FAILED: &str = "Failed to push lfs changes to staging area. Correct the issue, or use --skip-staging to \
     skip this step.";

/// Notice after all binary uploads finish (`:2585`).
pub const UPLOAD_COMPLETE: &str = "Upload complete.";

/// Per-file binary upload notice (`:2582`).
#[must_use]
pub fn uploaded_binary_notice(name: &str) -> String {
    format!("Uploaded binary data for \"{name}\".")
}

/// Per-file binary upload failure (`:2556-2558`).
#[must_use]
pub fn failed_binary_upload(name: &str) -> String {
    format!("Failed to upload binary \"{name}\".")
}

/* -(  SVN affected paths + per-path diff fan-out (D5/D11)  )------------------------- */

/// Split `svn:externals` entries out of a status map.
///
/// PHP: `generateAffectedPaths()`
/// (`src/workflow/ArcanistDiffWorkflow.php:756-766`). Externals are always
/// dropped; the ones carrying add/modify/delete flags are returned second
/// for the warning prompt, in status order.
#[must_use]
pub fn split_svn_externals(status: &svn::StatusMap) -> (svn::StatusMap, Vec<String>) {
    let mut kept = svn::StatusMap::new();
    let mut warn = Vec::new();
    for (path, mask) in status {
        if mask & svn::FLAG_EXTERNALS == 0 {
            kept.insert(path.clone(), *mask);
            continue;
        }
        let modified = mask & (svn::FLAG_ADDED | svn::FLAG_MODIFIED | svn::FLAG_DELETED) != 0;
        if modified {
            warn.push(path.clone());
        }
    }
    (kept, warn)
}

/// Lexically absolutize `path` against `base` (`Filesystem::resolvePath()`
/// subset for `FileList`).
///
/// Separators normalize to `/`; `.` and `..` collapse lexically. Like PHP,
/// a trailing `/` marks directories (via a filesystem check).
fn resolve_list_path(base: &Path, path: &str) -> String {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base.join(path)
    };
    let mut parts: Vec<String> = Vec::new();
    let mut rooted = false;
    for component in joined.components() {
        match component {
            std::path::Component::Prefix(prefix) => {
                parts.push(prefix.as_os_str().to_string_lossy().into_owned());
            }
            std::path::Component::RootDir => rooted = true,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = parts.join("/");
    if rooted {
        out.insert(0, '/');
    }
    if out != "/" && joined.is_dir() {
        out.push('/');
    }
    out
}

/// CLI path filter (`FileList`, `src/filesystem/FileList.php:33-81`).
///
/// An empty list contains every path; otherwise a candidate matches by
/// exact resolved path or when a listed directory is its parent
/// (`contains($path, true)`, `:56-81`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SvnFileList {
    files: Vec<String>,
    dirs: BTreeSet<String>,
}

impl SvnFileList {
    /// Resolve CLI `paths` against the process working directory (`:33-45`).
    #[must_use]
    pub fn new(cwd: &Path, paths: &[String]) -> Self {
        let mut files = Vec::new();
        let mut dirs = BTreeSet::new();
        for path in paths {
            let resolved = resolve_list_path(cwd, path);
            if resolved.ends_with('/') {
                dirs.insert(resolved.clone());
            }
            files.push(resolved);
        }
        Self { files, dirs }
    }

    /// True when the repo-relative `path` (joined to the working-copy
    /// `root`, like `$repository_api->getPath($path)`) is listed (`:56-81`).
    #[must_use]
    pub fn contains(&self, root: &Path, repo_path: &str) -> bool {
        if self.files.is_empty() {
            return true;
        }
        let candidate = resolve_list_path(root, repo_path);
        for file in &self.files {
            if *file == candidate {
                return true;
            }
            if self.dirs.contains(file) && candidate.starts_with(file.as_str()) {
                return true;
            }
        }
        false
    }
}

/// Drop untracked paths from affected paths (`:793-797`).
#[must_use]
pub fn drop_untracked_svn_paths(status: &svn::StatusMap) -> svn::StatusMap {
    status
        .iter()
        .filter(|(_, mask)| *mask & svn::FLAG_UNTRACKED == 0)
        .map(|(path, mask)| (path.clone(), *mask))
        .collect()
}

/// Injected prompt stdio plus the externals warning flag for the svn
/// fan-out: `warned_externals` is the workflow's `hasWarnedExternals`.
pub struct SvnPromptIo<'a, R: BufRead, W: Write> {
    /// Prompt input.
    pub reader: &'a mut R,
    /// Warning and prompt output.
    pub writer: &'a mut W,
    /// Whether the externals warning already printed.
    pub warned_externals: bool,
}

/// Warn once about modified externals and confirm generation (`:768-787`).
///
/// Writes [`svn_externals_warning`] plus `"\n\n"` (the dropped-`sprintf`-arg
/// shape), then confirms with [`PROMPT_SVN_EXTERNALS_ANYWAY`] (default no).
/// A refusal fails with [`USER_ABORT`]; `warned` is the workflow's
/// `hasWarnedExternals` flag.
pub fn confirm_svn_externals<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    warned: &mut bool,
    externals: &[String],
) -> Result<(), String> {
    if externals.is_empty() || *warned {
        return Ok(());
    }
    let text = format!("{}\n\n", svn_externals_warning());
    writer
        .write_all(text.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    if !diff::prompt_confirm_from(reader, writer, PROMPT_SVN_EXTERNALS_ANYWAY, true)? {
        return Err(USER_ABORT.to_string());
    }
    *warned = true;
    Ok(())
}

/// Skeleton change for one svn path (`parseSubversionDiff()`,
/// `src/parser/ArcanistDiffParser.php:63-90`).
///
/// The type comes from the status flags (added, deleted, else changed);
/// directories and symlinks set the file type (the link check runs after
/// the directory check, so link-to-dir reports symlink). Parsed hunks
/// merge into the skeleton later; the old path stays empty (PHP `null`)
/// until the `---` target or the copy pass sets it.
#[must_use]
pub fn svn_skeleton_change(root: &Path, path: &str, flags: u32) -> ParsedChange {
    let change_type = if flags & svn::FLAG_ADDED != 0 {
        TYPE_ADD
    } else if flags & svn::FLAG_DELETED != 0 {
        TYPE_DELETE
    } else {
        TYPE_CHANGE
    };
    let absolute = root.join(path);
    let mut file_type = FILE_TEXT;
    if absolute.is_dir() {
        file_type = FILE_DIRECTORY;
    }
    if absolute.is_symlink() {
        file_type = FILE_SYMLINK;
    }
    ParsedChange {
        old_path: String::new(),
        current_path: path.to_string(),
        change_type,
        file_type,
        hunks: Vec::new(),
        away_paths: Vec::new(),
        commit_hash: None,
        message: None,
    }
}

/// Fanned-out per-path working-copy data: status flags, `svn info` fields,
/// and the raw `--depth empty` diff text (`None` when svn reports none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SvnPathData {
    /// Repo-relative path.
    pub path: String,
    /// Status `FLAG_*` mask.
    pub flags: u32,
    /// Parsed `svn info` fields.
    pub info: BTreeMap<String, String>,
    /// Raw per-path diff bytes, if any.
    pub diff: Option<Vec<u8>>,
}

/// Fan out over status paths (D5): `svn info` plus the per-path
/// `--depth empty` diff for each usable path.
///
/// PHP: `primeSubversionWorkingCopyData()` plus the `getRawDiffText()`
/// loop (`ArcanistDiffWorkflow.php:1068-1094`,
/// `ArcanistDiffParser.php:92-94`). Untracked, conflicted, and missing
/// paths are dropped (`:54-60`). Iteration follows the (sorted) status
/// map; PHP follows `svn --xml status` order.
pub fn collect_svn_path_data_with(
    dir: &Path,
    svn_cmd: &svn::SvnCommand,
    status: &svn::StatusMap,
    config: &svn::SvnDiffConfig<'_>,
) -> Result<Vec<SvnPathData>, String> {
    let mut data = Vec::new();
    for (path, flags) in status {
        if flags & (svn::FLAG_UNTRACKED | svn::FLAG_CONFLICT | svn::FLAG_MISSING) != 0 {
            continue;
        }
        let info = svn::get_svn_info_with(dir, svn_cmd, path)?;
        let diff = svn::get_raw_diff_text_with(dir, svn_cmd, path, status, config)?;
        data.push(SvnPathData {
            path: path.clone(),
            flags: *flags,
            info,
            diff,
        });
    }
    Ok(data)
}

/// Empty-diff parse failure (`parseDiff()`,
/// `src/parser/ArcanistDiffParser.php:191-193`).
pub const EMPTY_DIFF: &str = "Can't parse an empty diff!";

/// Merge one parsed block into the keyed change set.
///
/// Matching skeletons gain the parsed hunks and old path (PHP's
/// `setOldPath()` from `---`, plus `markBinary()`); new paths append
/// as-is. A repeat path drops the earlier hunks so the later block wins
/// (T5555, `parseChangeset()`,
/// `src/parser/ArcanistDiffParser.php:880-887`).
fn merge_one_svn_block(
    changes: &mut Vec<ParsedChange>,
    index: &mut BTreeMap<String, usize>,
    parsed: ParsedChange,
) {
    match index.get(&parsed.current_path).copied() {
        Some(at) => {
            let target = &mut changes[at];
            target.hunks = parsed.hunks;
            target.old_path = parsed.old_path;
            if parsed.file_type == FILE_BINARY {
                target.file_type = FILE_BINARY;
            }
        }
        None => {
            index.insert(parsed.current_path.clone(), changes.len());
            changes.push(parsed);
        }
    }
}

/// Deleted-or-moved-away directories report `FILE_DIRECTORY` (`:128-136`).
fn apply_svn_delete_dir_types(changes: &mut [ParsedChange], data: &[SvnPathData]) {
    for datum in data {
        let is_dir = datum
            .info
            .get("Node Kind")
            .is_some_and(|kind| kind == "directory");
        if !is_dir {
            continue;
        }
        if let Some(change) = changes
            .iter_mut()
            .find(|change| change.current_path == datum.path)
            && (change.change_type == TYPE_DELETE || change.change_type == TYPE_MOVE_AWAY)
        {
            change.file_type = FILE_DIRECTORY;
        }
    }
}

/// PHP loose `!=` over revision strings: two numeric strings compare
/// numerically, anything else compares as strings.
fn php_loose_not_equal(left: &str, right: &str) -> bool {
    match (left.parse::<i64>(), right.parse::<i64>()) {
        (Ok(a), Ok(b)) => a != b,
        _ => left != right,
    }
}

/// Root-relative copy source for a `Copied From URL` (`:96-124`).
///
/// Returns `None` for empty sources and sources outside the working-copy
/// root. Otherwise the root prefix (plus `/`) is stripped, and `@rev` is
/// appended when `Copied From Rev` differs loosely from `Revision`.
#[must_use]
pub fn svn_copy_old_path(
    copied_from_url: &str,
    copied_from_rev: &str,
    revision: &str,
    root_url: &str,
) -> Option<String> {
    if copied_from_url.is_empty() {
        return None;
    }
    let root = format!("{root_url}/");
    let cpath = copied_from_url.strip_prefix(root.as_str())?;
    let mut out = cpath.to_string();
    if !copied_from_rev.is_empty() && php_loose_not_equal(copied_from_rev, revision) {
        out.push('@');
        out.push_str(copied_from_rev);
    }
    Some(out)
}

/// Origin-type transition for a copy/move source (`:148-166`).
fn origin_transition(change_type: u32) -> Result<u32, String> {
    match change_type {
        TYPE_MULTICOPY | TYPE_COPY_AWAY | TYPE_ADD => Ok(change_type),
        TYPE_DELETE => Ok(TYPE_MOVE_AWAY),
        TYPE_MOVE_AWAY => Ok(TYPE_MULTICOPY),
        TYPE_CHANGE => Ok(TYPE_COPY_AWAY),
        other => Err(format!("Bad origin state {other}.")),
    }
}

/// New-location type from the transitioned origin type (`:168-183`).
fn here_type(origin_type: u32) -> Result<u32, String> {
    match origin_type {
        TYPE_MULTICOPY | TYPE_MOVE_AWAY => Ok(TYPE_MOVE_HERE),
        TYPE_ADD | TYPE_COPY_AWAY => Ok(TYPE_COPY_HERE),
        other => Err(format!("Bad origin state {other}.")),
    }
}

/// Fresh origin change (`buildChange()` default: `TYPE_CHANGE`, text).
fn fresh_origin_change(origin: &str) -> ParsedChange {
    ParsedChange {
        old_path: String::new(),
        current_path: origin.to_string(),
        change_type: TYPE_CHANGE,
        file_type: FILE_TEXT,
        hunks: Vec::new(),
        away_paths: Vec::new(),
        commit_hash: None,
        message: None,
    }
}

/// Apply `Copied From` old paths plus the origin copy/move pass (`:96-184`).
///
/// `root_url` is the working-copy root URL (`getSVNInfo('.')['URL']`),
/// fetched lazily by the caller only when a copied path exists.
pub fn apply_svn_copy_info(
    changes: &mut Vec<ParsedChange>,
    data: &[SvnPathData],
    root_url: Option<&str>,
) -> Result<(), String> {
    let mut from: BTreeMap<String, String> = BTreeMap::new();
    if let Some(root) = root_url {
        for datum in data {
            let copied = datum
                .info
                .get("Copied From URL")
                .map(String::as_str)
                .unwrap_or("");
            let from_rev = datum
                .info
                .get("Copied From Rev")
                .map(String::as_str)
                .unwrap_or("");
            let revision = datum.info.get("Revision").map(String::as_str).unwrap_or("");
            if let Some(cpath) = svn_copy_old_path(copied, from_rev, revision, root) {
                if let Some(change) = changes.iter_mut().find(|c| c.current_path == datum.path) {
                    change.old_path = cpath.clone();
                }
                from.insert(datum.path.clone(), cpath);
            }
        }
    }
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (at, change) in changes.iter().enumerate() {
        index.insert(change.current_path.clone(), at);
    }
    for (path, origin) in &from {
        let Some(at) = index.get(path).copied() else {
            continue;
        };
        if !index.contains_key(origin) && changes[at].change_type == TYPE_COPY_HERE {
            continue;
        }
        let origin_at = match index.get(origin).copied() {
            Some(at) => at,
            None => {
                changes.push(fresh_origin_change(origin));
                let at = changes.len() - 1;
                index.insert(origin.clone(), at);
                at
            }
        };
        changes[origin_at].away_paths.push(path.clone());
        let transitioned = origin_transition(changes[origin_at].change_type)?;
        changes[origin_at].change_type = transitioned;
        changes[at].change_type = here_type(transitioned)?;
    }
    Ok(())
}

/// Parse fanned-out per-path texts into changes (`parseSubversionDiff()`).
///
/// Every path gets a skeleton; non-empty texts parse and merge (blank-only
/// texts fail like PHP's `parseDiff()` guard). Delete-dir file types and
/// the copy/move pass apply after the merge.
pub fn merge_svn_path_diffs(
    root: &Path,
    data: &[SvnPathData],
    root_url: Option<&str>,
) -> Result<Vec<ParsedChange>, String> {
    let mut changes: Vec<ParsedChange> = data
        .iter()
        .map(|datum| svn_skeleton_change(root, &datum.path, datum.flags))
        .collect();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (at, change) in changes.iter().enumerate() {
        index.insert(change.current_path.clone(), at);
    }
    for datum in data {
        let Some(bytes) = datum.diff.as_ref() else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        if text.trim().is_empty() {
            return Err(EMPTY_DIFF.to_string());
        }
        for parsed in parse_unified_diff(&text)? {
            merge_one_svn_block(&mut changes, &mut index, parsed);
        }
    }
    apply_svn_delete_dir_types(&mut changes, data);
    apply_svn_copy_info(&mut changes, data, root_url)?;
    Ok(changes)
}

/// Working-copy root for `cwd`: the parent of the nearest `.svn`
/// directory (`ArcanistSubversionAPI::$path`), else `cwd` itself.
#[must_use]
pub fn svn_working_copy_root(cwd: &Path) -> PathBuf {
    svn::find_metadata_path(cwd)
        .and_then(|marker| marker.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Default `--diff-cmd` script (`buildDiffFuture()`,
/// `ArcanistSubversionAPI.php:281-313`).
///
/// Windows uses the plain `diff --depth empty` shape (`None`); Unix
/// resolves `<exe>/../scripts/repository/binary_safe_diff.sh`, mirroring
/// PHP's `$root.'/../scripts/repository/binary_safe_diff.sh'`.
#[must_use]
pub fn default_svn_diff_cmd() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("../scripts/repository/binary_safe_diff.sh"))
}

/// Collect submit-ready parsed changes from an svn working copy.
///
/// PHP: the Subversion branch of `generateChanges()` plus
/// `generateAffectedPaths()`
/// (`ArcanistDiffWorkflow.php:746-799,832-884`): argv-limited status with
/// externals, `FileList` limiting, the externals warn/confirm, the
/// base-revision consistency check, the per-path fan-out, and the
/// svn-only gigantic-diff throw (`:915-922`).
///
/// Divergence: `svn::get_raw_diff_text_with` re-reads `svn info` for
/// copied adds (PHP's `svnInfo` cache has no port), so copied adds cost
/// one redundant `info` call with identical behavior.
pub fn collect_svn_parsed_changes_with_io<R: BufRead, W: Write>(
    cwd: &Path,
    svn_cmd: &svn::SvnCommand,
    cli_paths: &[String],
    config: &svn::SvnDiffConfig<'_>,
    io: &mut SvnPromptIo<'_, R, W>,
) -> Result<Vec<ParsedChange>, String> {
    let root = svn_working_copy_root(cwd);
    let path_refs: Vec<&str> = cli_paths.iter().map(String::as_str).collect();
    let status = svn::get_svn_status_with(&root, svn_cmd, &path_refs, true)?;
    let list = SvnFileList::new(cwd, cli_paths);
    let limited: svn::StatusMap = status
        .files
        .iter()
        .filter(|(path, _)| list.contains(&root, path))
        .map(|(path, mask)| (path.clone(), *mask))
        .collect();
    let (paths, warn) = split_svn_externals(&limited);
    confirm_svn_externals(io.reader, io.writer, &mut io.warned_externals, &warn)?;
    let paths = drop_untracked_svn_paths(&paths);
    let base_pairs: Vec<(&str, &str)> = status
        .base_revisions
        .iter()
        .map(|(path, rev)| (path.as_str(), rev.as_str()))
        .collect();
    check_svn_base_revision_strings(&base_pairs)?;
    let data = collect_svn_path_data_with(&root, svn_cmd, &paths, config)?;
    let root_url = if data.iter().any(|datum| {
        datum
            .info
            .get("Copied From URL")
            .is_some_and(|url| !url.is_empty())
    }) {
        svn::get_svn_info_with(&root, svn_cmd, ".")?
            .get("URL")
            .cloned()
    } else {
        None
    };
    let changes = merge_svn_path_diffs(&root, &data, root_url.as_deref())?;
    for change in &changes {
        if change.corpus_size() > GIGANTIC_DIFF_LIMIT_BYTES {
            return Err(gigantic_svn_error(
                &change.current_path,
                change.corpus_size(),
            ));
        }
    }
    Ok(changes)
}

/// Confirm prompt after upload failures (`:2563`).
pub const PROMPT_CONTINUE: &str = "Continue?";

/// Abort message when uploads fail and the user stops (`:2566-2571`).
pub const ABORTED_UPLOAD_FAILURE: &str =
    "Aborted due to file upload failure. You can use --skip-binaries to skip binary uploads.";

/// True when a change needs its file data uploaded (`:2479-2486`).
#[must_use]
pub fn needs_binary_upload(file_type: u32, skip_binaries: bool) -> bool {
    file_type == FILE_BINARY && !skip_binaries
}

/// True when a mime type marks a change as an image (`:2530-2533`).
#[must_use]
pub fn is_image_mime_type(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// Notice while amending the worktree commit message (`:542`).
pub const UPDATING_COMMIT_MESSAGE: &str = "Updating commit message...";
/// Notice when amend is unsupported (`:545-548`).
pub const AMEND_UNSUPPORTED: &str = "Commit message was not amended. Amending commit message is \
    only supported in git and hg (version 2.2 or newer)";

/// True when created objects should open in a browser (`:2594-2596`).
#[must_use]
pub fn should_open_in_browser(browse: bool) -> bool {
    browse
}

/* -(  Hg Local Commits and Svn Info (G1 full-state)  )--------------------------- */

/// Split an hg author string into display name and email.
///
/// PHP: `ArcanistMercurialAPI::parseFullAuthor()`
/// (`src/repository/api/ArcanistMercurialAPI.php:645-656`) plus
/// `PhutilEmailAddress` (`src/parser/PhutilEmailAddress.php:20-32`): no
/// `@` keeps the whole string as the name; `Display <addr>` trims quotes
/// and spaces from the display part; a bare address has no display name.
#[must_use]
pub fn parse_hg_author(full_author: &str) -> (Option<String>, Option<String>) {
    if !full_author.contains('@') {
        return (Some(full_author.to_string()), None);
    }
    let trimmed = full_author.trim();
    if let Some((display, rest)) = trimmed.rsplit_once('<')
        && let Some(inner) = rest.strip_suffix('>')
    {
        let display = display.trim().trim_matches(['\'', '"', ' ']).to_string();
        return (Some(display), Some(inner.trim().to_string()));
    }
    (None, Some(trimmed.to_string()))
}

/// `hg log` template for local commit information.
///
/// PHP: `getLocalCommitInformation()`
/// (`ArcanistMercurialAPI.php:193-198`).
///
/// Divergence: `{date|hgdate}` replaces `{date|rfc822date}`; both name the
/// same instant, and `hgdate` parses without a date library.
pub const HG_LOCAL_COMMIT_TEMPLATE: &str = "{node}\u{1}{rev}\u{1}{author}\u{1}{date|hgdate}\u{1}{branch}\u{1}{tag}\u{1}{parents}\u{1}{desc}\u{2}";

/// `hg log` argv for local commit information between `base` and `.` (`:193-198`).
#[must_use]
pub fn hg_local_commit_argv(base: &str, branch: &str) -> Vec<String> {
    vec![
        "log".to_string(),
        "--template".to_string(),
        HG_LOCAL_COMMIT_TEMPLATE.to_string(),
        "--rev".to_string(),
        format!("({base}::. - {base})"),
        "--branch".to_string(),
        branch.to_string(),
        "--".to_string(),
    ]
}

/// One hg local commit, verbatim PHP keys via [`to_json`](HgLocalCommit::to_json).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgLocalCommit {
    /// Node hash (`commit`; also `rev`, which PHP aliases to the node).
    pub node: String,
    /// Local revision number (`local`).
    pub rev: String,
    /// Display name (`author`), `None` for bare addresses.
    pub author: Option<String>,
    /// Author email (`authorEmail`).
    pub author_email: Option<String>,
    /// Author timestamp (`time`, unix).
    pub time: i64,
    /// Branch (`branch`).
    pub branch: String,
    /// Tag (`tag`).
    pub tag: String,
    /// Parent nodes (`parents`); `None` needs an `hg parents` lookup.
    pub parents: Option<Vec<String>>,
    /// First message line (`summary`).
    pub summary: String,
    /// Full message (`message`).
    pub message: String,
}

impl HgLocalCommit {
    /// Render with the verbatim PHP keys (`:232-244`).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "author": self.author.clone().map_or(serde_json::Value::Null, serde_json::Value::String),
            "time": self.time,
            "branch": self.branch,
            "tag": self.tag,
            "commit": self.node,
            "rev": self.node,
            "local": self.rev,
            "parents": self.parents.clone().unwrap_or_default(),
            "summary": self.summary,
            "message": self.message,
            "authorEmail": self.author_email.clone().map_or(serde_json::Value::Null, serde_json::Value::String),
        })
    }
}

/// Parse `hg log` local-commit output, newest-first.
///
/// PHP: `getLocalCommitInformation()` (`:199-260`): records split on
/// `\x02`, fields on `\x01`; empty `{parents}` chains to the previous
/// (older) node, otherwise the parents need a lookup; the oldest-first
/// input reverses to newest-first (`:260`).
///
/// Divergence: records with fewer than 8 fields are skipped; PHP assigns
/// nulls with warnings.
#[must_use]
pub fn parse_hg_local_commits(output: &str) -> Vec<HgLocalCommit> {
    let mut commits = Vec::new();
    let mut last_node: Option<String> = None;
    for log in output.split('\u{2}').filter(|log| !log.is_empty()) {
        let parts: Vec<&str> = log.splitn(9, '\u{1}').collect();
        if parts.len() < 8 {
            continue;
        }
        let (author, author_email) = parse_hg_author(parts[2]);
        let time = parts[3]
            .split_whitespace()
            .next()
            .and_then(|stamp| stamp.parse::<i64>().ok())
            .unwrap_or(0);
        let parents = if parts[6].is_empty() {
            last_node.clone().map(|node| vec![node])
        } else {
            None
        };
        let message = parts[7].to_string();
        commits.push(HgLocalCommit {
            node: parts[0].to_string(),
            rev: parts[1].to_string(),
            author,
            author_email,
            time,
            branch: parts[4].to_string(),
            tag: parts[5].to_string(),
            parents,
            summary: message.lines().next().unwrap_or("").to_string(),
            message,
        });
        last_node = Some(parts[0].to_string());
    }
    commits.reverse();
    commits
}

/// `hg parents` argv for one node's parent nodes (`:226-230`).
#[must_use]
pub fn hg_parents_argv(node: &str) -> Vec<String> {
    vec![
        "parents".to_string(),
        "--template".to_string(),
        "{node}\n".to_string(),
        "--rev".to_string(),
        node.to_string(),
    ]
}

/// List hg local commits between `base` and `.`, newest-first.
///
/// PHP: `getLocalCommitInformation()` (`:190-266`).
///
/// Divergence: parent lookups run sequentially; PHP fans out with a
/// limit of 4 (`:249-255`).
pub fn get_hg_local_commits(
    dir: &Path,
    base: &str,
    branch: &str,
) -> Result<Vec<HgLocalCommit>, String> {
    get_hg_local_commits_with(dir, &hg::HgCommand::default(), base, branch)
}

/// [`get_hg_local_commits`] over an injected `hg` backend.
pub fn get_hg_local_commits_with(
    dir: &Path,
    hg_cmd: &hg::HgCommand,
    base: &str,
    branch: &str,
) -> Result<Vec<HgLocalCommit>, String> {
    let argv = hg_local_commit_argv(base, branch);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let text = hg::stdout_text_with(dir, hg_cmd, &refs)?;
    let mut commits = parse_hg_local_commits(&text);
    for commit in &mut commits {
        if commit.parents.is_none() {
            let argv = hg_parents_argv(&commit.node);
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let parents = hg::stdout_text_with(dir, hg_cmd, &refs)?;
            commit.parents = Some(
                parents
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        }
    }
    Ok(commits)
}

/// Parse `svn info` output into key/value pairs.
///
/// PHP: `getSVNInfo()` (`src/repository/api/ArcanistSubversionAPI.php:326-348`)
/// extracts URL, Revision, and last-changed keys with regexes; every
/// `Key: value` line is kept here (a superset) and callers read what they
/// need. CRLF line endings are tolerated (`:340`).
#[must_use]
pub fn parse_svn_info(output: &str) -> BTreeMap<String, String> {
    let mut info = BTreeMap::new();
    for line in output.lines() {
        if let Some((key, value)) = line.split_once(": ") {
            info.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    info
}

/// Read and parse `svn info` for `path`.
///
/// PHP: `getSVNInfo()` (`:326-337`), including the nonzero-exit error.
pub fn get_svn_info(dir: &Path, path: &str) -> Result<BTreeMap<String, String>, String> {
    get_svn_info_with(dir, &svn::SvnCommand::default(), path)
}

/// [`get_svn_info`] over an injected `svn` backend.
pub fn get_svn_info_with(
    dir: &Path,
    svn_cmd: &svn::SvnCommand,
    path: &str,
) -> Result<BTreeMap<String, String>, String> {
    let argv = svn_info_argv(path);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = svn::output_with(dir, svn_cmd, &refs)?;
    if !out.status.success() {
        return Err(format!(
            "Error #{} executing svn info against '{path}'.",
            out.status.code().unwrap_or(-1)
        ));
    }
    Ok(parse_svn_info(&String::from_utf8_lossy(&out.stdout)))
}

/* -(  Create-message edit loop (D1)  )-------------------------------------- */

/// Scratch file holding the in-progress create message (`:1382`).
pub const SCRATCH_CREATE_MESSAGE: &str = "create-message";

/// PHP `rtrim()` default charlist (also `trim()` and `ltrim()`).
const PHP_TRIM_CHARS: [char; 6] = [' ', '\t', '\n', '\r', '\0', '\x0B'];

/// Build the editor template: message, blank line, `#`-comment issues,
/// trailing blank line (`getCommitMessageFromUser()`, `:1467-1474`).
#[must_use]
pub fn build_editor_template(template: &str, issues: &[String]) -> String {
    let mut out = format!("{}\n\n", template.trim_end_matches(['\r', '\n']));
    for issue in issues {
        out.push_str(format!("# {issue}").trim_end_matches(PHP_TRIM_CHARS));
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Saved-message resume block (`getCommitMessageFromUser()`, `:1362-1400`).
///
/// Skipped for `--verbatim`. A saved `create-message` scratch template
/// prints the saved notice and confirms reuse (`[Y/n]`); a refusal
/// deletes the scratch file. Returns the reused template, if any.
pub fn resolve_saved_template<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    metadata_dir: &Path,
    work_root: &Path,
    verbatim: bool,
) -> Result<Option<String>, String> {
    if verbatim {
        return Ok(None);
    }
    let Some(saved) = diff::read_scratch_file(metadata_dir, SCRATCH_CREATE_MESSAGE) else {
        return Ok(None);
    };
    if saved.is_empty() {
        return Ok(None);
    }
    let preview = truncate_preview_64(&saved);
    let where_path =
        diff::readable_scratch_file_path(metadata_dir, work_root, SCRATCH_CREATE_MESSAGE);
    let notice = saved_message_notice(&where_path, &preview);
    writer
        .write_all(notice.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    if diff::prompt_confirm_from(reader, writer, PROMPT_USE_SAVED_MESSAGE, false)? {
        return Ok(Some(saved));
    }
    diff::remove_scratch_file(metadata_dir, SCRATCH_CREATE_MESSAGE);
    Ok(None)
}

/// Refuse (after confirm) to update someone else's revision (`:2446-2467`).
pub fn check_revision_ownership<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    revision: &serde_json::Value,
    user_phid: &str,
) -> Result<(), String> {
    let author = revision
        .get("authorPHID")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !diff_submit::needs_ownership_confirm(author, user_phid) {
        return Ok(());
    }
    let id = revision
        .get("id")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let title = revision
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let prompt = diff_submit::ownership_prompt(id, title);
    if !diff::prompt_confirm_from(reader, writer, &prompt, true)? {
        return Err(diff_submit::OWNERSHIP_ABORT.to_string());
    }
    Ok(())
}

/// `reviewerPHIDs` as a string list, else empty (`getFieldValue()`, `:1634`).
fn message_reviewer_phids(message: &CommitMessage) -> Vec<&str> {
    match message.get_field_value("reviewerPHIDs") {
        Some(FieldValue::List(phids)) => phids.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Filter `user.query` rows to the requested reviewers, in reviewer order
/// (`array_select_keys()`, `:1641`).
fn select_reviewer_users<'a>(
    users: &'a serde_json::Value,
    reviewers: &[&str],
) -> Vec<&'a serde_json::Value> {
    let mut out = Vec::new();
    if let serde_json::Value::Object(map) = users {
        for phid in reviewers {
            if let Some(user) = map.get(*phid) {
                out.push(user);
            }
        }
    }
    out
}

/// Away-reviewer block of `validateCommitMessage()` (`:1633-1712`).
fn validate_reviewer_availability<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    call: &diff_submit::ConduitCall<'_>,
    reviewers: &[&str],
    away_offset: i64,
) -> Result<(), String> {
    if reviewers.is_empty() {
        return Ok(());
    }
    let params = diff_submit::user_query_params(reviewers);
    let users = call(diff_submit::METHOD_USER_QUERY, &params)?;
    let selected: Vec<serde_json::Value> = select_reviewer_users(&users, reviewers)
        .into_iter()
        .cloned()
        .collect();
    let away = diff_submit::away_reviewers(&serde_json::Value::Array(selected));
    let Some(block) =
        diff_submit::away_reviewers_block_with_offset(&away, reviewers.len(), away_offset)
    else {
        return Ok(());
    };
    writer
        .write_all(block.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    if !diff::prompt_confirm_from(reader, writer, diff_submit::CONTINUE_DESPITE_AWAY, true)? {
        return Err(diff_submit::SPECIFY_AVAILABLE_REVIEWERS.to_string());
    }
    Ok(())
}

/// Validate a message against Conduit (`validateCommitMessage()`, `:1620-1649`).
///
/// With a `Differential Revision:` field, `differential.query` must find
/// the row (else `There is no revision D{id}.`), then the ownership
/// check runs. `reviewerPHIDs` fields trigger the away-reviewer block.
pub fn validate_commit_message<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    call: &diff_submit::ConduitCall<'_>,
    message: &CommitMessage,
    user_phid: &str,
    away_offset: i64,
) -> Result<(), String> {
    if let Some(id) = message.revision_id {
        let id_arg = i64::try_from(id).unwrap_or(i64::MAX);
        let params = diff_submit::differential_query_params(&[id_arg]);
        let result = call(diff_submit::METHOD_QUERY, &params)?;
        let row = result.as_array().and_then(|rows| rows.first());
        match row {
            Some(row) => check_revision_ownership(reader, writer, row, user_phid)?,
            None => return Err(diff_submit::missing_revision_message(id_arg)),
        }
    }
    let reviewers = message_reviewer_phids(message);
    validate_reviewer_availability(reader, writer, call, &reviewers, away_offset)
}

/// Seams plus context for the create-message edit loop.
pub struct CreateMessageEditor<'a, R: BufRead, W: Write> {
    /// Prompt input.
    pub reader: &'a mut R,
    /// Echo plus prompt output.
    pub writer: &'a mut W,
    /// Conduit caller.
    pub call: &'a diff_submit::ConduitCall<'a>,
    /// VCS metadata dir (scratch files).
    pub metadata_dir: &'a Path,
    /// Work root (readable scratch paths).
    pub work_root: &'a Path,
    /// Interactive editor: template text to edited text.
    pub edit: &'a dyn Fn(&str) -> Result<String, String>,
    /// Amend hook when a single worktree commit absorbs the message
    /// (`shouldAmend()`); `None` saves to scratch instead.
    pub amend_commit: Option<&'a dyn Fn(&str) -> Result<(), String>>,
    /// Viewer PHID (ownership checks).
    pub user_phid: &'a str,
    /// Server-tz offset seconds (away-reviewer dates).
    pub away_offset: i64,
}

/// One parse/validate attempt inside the loop.
enum AttemptError {
    /// Conduit parser errors: re-editable.
    Parser(Vec<String>),
    /// Anything else: fatal after the saved-copy note.
    Other(String),
}

/// Parse, pull, and validate one draft (`:1516-1522`).
fn attempt_message<R: BufRead, W: Write>(
    editor: &mut CreateMessageEditor<'_, R, W>,
    template: &str,
) -> Result<CommitMessage, AttemptError> {
    let mut message =
        CommitMessage::new_from_raw_corpus(template).map_err(AttemptError::Other)?;
    message
        .pull_data_from_conduit(editor.call, false)
        .map_err(|error| match error {
            PullDataError::Parser(errors) => AttemptError::Parser(errors),
            PullDataError::Other(text) => AttemptError::Other(text),
        })?;
    validate_commit_message(
        &mut *editor.reader,
        &mut *editor.writer,
        editor.call,
        &message,
        editor.user_phid,
        editor.away_offset,
    )
    .map_err(AttemptError::Other)?;
    Ok(message)
}

/// Unresolved-errors abort (`:1541-1544`): with a saved copy the suffix
/// names it, else PHP's `" ...errors." . " "` trailing space survives.
#[must_use]
pub fn unresolved_errors_error(saved_where: Option<&str>) -> String {
    match saved_where {
        Some(where_path) => format!(
            "{} {}",
            MESSAGE_HAS_UNRESOLVED_ERRORS,
            copy_saved_to(where_path)
        ),
        None => format!("{MESSAGE_HAS_UNRESOLVED_ERRORS} "),
    }
}

/// Echo parser errors and confirm re-editing (`:1524-1540`).
///
/// Returns the next-round issues on re-edit, or the abort error on refusal.
fn confirm_message_reedit<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    errors: &[String],
    saved_where: Option<&str>,
) -> Result<Vec<String>, String> {
    writer
        .write_all(format!("{COMMIT_MESSAGE_HAS_ERRORS}\n\n").as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    let mut issues = vec![RESOLVE_THESE_ERRORS.to_string()];
    for error in errors {
        let line = diff::console_wrap(&format!("- {error}\n"), 6, true);
        writer
            .write_all(line.as_bytes())
            .map_err(|error| format!("failed to write prompt: {error}"))?;
        issues.push(format!("  - {error}"));
    }
    writer
        .write_all(b"\n")
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    writer
        .write_all(MUST_RESOLVE_ERRORS.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    if !diff::prompt_confirm_from(reader, writer, PROMPT_EDIT_MESSAGE_AGAIN, false)? {
        return Err(unresolved_errors_error(saved_where));
    }
    Ok(issues)
}

/// Amend-or-scratch the draft (`:1502-1514`).
///
/// Returns `(wrote, where)`: `where` is `None` only when amending left
/// the message unchanged.
fn save_message_draft<R: BufRead, W: Write>(
    editor: &CreateMessageEditor<'_, R, W>,
    old_message: &str,
    template: &str,
) -> Result<(bool, Option<String>), String> {
    if let Some(amend) = editor.amend_commit {
        let wrote =
            old_message.trim_end_matches(PHP_TRIM_CHARS) != template.trim_end_matches(PHP_TRIM_CHARS);
        if wrote {
            amend(template)?;
            return Ok((true, Some(WHERE_COMMIT_MESSAGE.to_string())));
        }
        return Ok((false, None));
    }
    let wrote = diff::write_scratch_file(editor.metadata_dir, SCRATCH_CREATE_MESSAGE, template);
    let where_path = format!(
        "'{}'",
        diff::readable_scratch_file_path(editor.metadata_dir, editor.work_root, SCRATCH_CREATE_MESSAGE)
    );
    Ok((wrote, Some(where_path)))
}

/// Echo the saved-copy note for a fatal loop error (`:1546-1550`).
fn echo_saved_copy_note<W: Write>(
    writer: &mut W,
    where_path: &str,
) -> Result<(), String> {
    let note = diff::console_wrap(&format!("{}\n", message_saved_to(where_path)), 0, true);
    writer
        .write_all(note.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))
}

/// Interactively build a create message (`getCommitMessageFromUser()`,
/// `:1359-1559`).
///
/// `template`/`old_message` are the starting and pre-edit templates,
/// `issues` the first-round `#` comments, `template_is_default` whether
/// the template came from the server default, and `verbatim` the flag
/// (skips the editor on the first pass for non-default templates).
/// Returns the parsed, pulled, validated message.
pub fn edit_create_message<R: BufRead, W: Write>(
    editor: &mut CreateMessageEditor<'_, R, W>,
    template: &str,
    old_message: &str,
    template_is_default: bool,
    verbatim: bool,
    issues: &[String],
) -> Result<CommitMessage, String> {
    let mut template = template.to_string();
    let mut issues = issues.to_vec();
    let mut first = true;
    loop {
        let edit_text = build_editor_template(&template, &issues);
        let new_template = if first && verbatim && !template_is_default {
            template.clone()
        } else {
            let edited = (editor.edit)(&edit_text)?;
            let stripped = remove_comments(&edited);
            if template_is_default && stripped == template {
                return Err(TEMPLATE_NOT_EDITED.to_string());
            }
            stripped
        };
        first = false;
        template = new_template;
        let (wrote, where_path) = save_message_draft(editor, old_message, &template)?;
        match attempt_message(editor, &template) {
            Ok(message) => return Ok(message),
            Err(AttemptError::Parser(errors)) => {
                issues = confirm_message_reedit(
                    &mut *editor.reader,
                    &mut *editor.writer,
                    &errors,
                    where_path.as_deref(),
                )?;
            }
            Err(AttemptError::Other(error)) => {
                if wrote {
                    if let Some(where_path) = where_path {
                        echo_saved_copy_note(&mut *editor.writer, &where_path)?;
                    }
                }
                return Err(error);
            }
        }
    }
}

/// Twelve-char hash prefix (`substr($commit, 0, 12)`, `:1853-1855`).
fn short_commit_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// One default-field message outcome.
enum CreateFieldOutcome {
    /// Pulled fields.
    Fields(CommitFields),
    /// Parse-failure note lines.
    Notes(Vec<String>),
}

/// Parse one message for default fields (`:1874-1886`).
///
/// Usage errors from the corpus parser and transport failures propagate;
/// conduit parser errors become note lines (verbatim rethrows the first).
fn parse_create_message_fields(
    call: &diff_submit::ConduitCall<'_>,
    hash: &str,
    message: &str,
    verbatim: bool,
) -> Result<CreateFieldOutcome, String> {
    let mut parsed = CommitMessage::new_from_raw_corpus(message)?;
    match parsed.pull_data_from_conduit(call, true) {
        Ok(()) => Ok(CreateFieldOutcome::Fields(parsed.fields)),
        Err(PullDataError::Parser(errors)) => {
            if verbatim {
                return Err(errors.into_iter().next().unwrap_or_default());
            }
            let mut notes = vec![commit_parse_note(&short_commit_hash(hash))];
            notes.extend(errors.iter().map(|error| format!("  - {error}")));
            Ok(CreateFieldOutcome::Notes(notes))
        }
        Err(PullDataError::Other(error)) => Err(error),
    }
}

/// Default create fields from local commits (`getDefaultCreateFields()`,
/// `:1770-1848`).
///
/// `local` is newest-first (as returned); `reviewers`/`cc` build the faux
/// flag message. Returns merged fields, `Included commits` lines
/// (newest-first, faux first), and parse notes.
/// `getDependsOnRevisionRef()` always returns null in PHP (`:1993-1996`
/// TODO), so no dependency line is emitted. The
/// `TYPE_DIFF_WILLBUILDMESSAGE` event (`:2431-2444`) has no port: there
/// are no extensions to dispatch to.
pub fn get_default_create_fields_with(
    call: &diff_submit::ConduitCall<'_>,
    local: &[LocalCommitInfo],
    create: bool,
    reviewers: Option<&str>,
    cc: Option<&str>,
    verbatim: bool,
) -> Result<(CommitFields, Vec<String>, Vec<String>), String> {
    let mut messages: Vec<(String, String, String)> = Vec::new();
    if let Some(faux) = faux_flag_message(reviewers, cc) {
        messages.push((
            FLAGS_COMMIT_HASH.to_string(),
            faux,
            "Command-Line Flags".to_string(),
        ));
    }
    for info in local {
        messages.push((info.commit.clone(), info.message.clone(), info.summary.clone()));
    }
    let mut included = Vec::new();
    for (hash, _, summary) in &messages {
        included.push(included_commit_line(hash, summary));
    }
    messages.reverse();
    let mut dicts = Vec::new();
    let mut notes = Vec::new();
    for (hash, message, _) in &messages {
        match parse_create_message_fields(call, hash, message, verbatim)? {
            CreateFieldOutcome::Fields(dict) => dicts.push(dict),
            CreateFieldOutcome::Notes(mut lines) => notes.append(&mut lines),
        }
    }
    let mut fields = merge_fields_oldest_first(&dicts);
    if create {
        fields.remove(FIELD_REVISION_ID);
    }
    Ok((fields, included, notes))
}

/// Failure to fetch the `--update` template (`:1606-1610`).
pub const FAILED_TO_RETRIEVE_TEMPLATE: &str =
    "Failed to retrieve commit message template from server.";

/// Message from `--message-file` (`getCommitMessageFromFile()`, `:1561-1575`).
pub fn get_commit_message_from_file_with<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    call: &diff_submit::ConduitCall<'_>,
    path: &Path,
    user_phid: &str,
    away_offset: i64,
) -> Result<CommitMessage, String> {
    let mut message = commit_message_from_file(path)?;
    message
        .pull_data_from_conduit(call, false)
        .map_err(PullDataError::into_message)?;
    validate_commit_message(reader, writer, call, &message, user_phid, away_offset)?;
    Ok(message)
}

/// Message from `--update` (`getCommitMessageFromRevision()`, `:1577-1618`).
///
/// Returns the message plus whether the revision is a draft. Unlike the
/// create paths, PHP never validates here (no away-reviewer block).
pub fn get_commit_message_from_revision_with<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    call: &diff_submit::ConduitCall<'_>,
    revision_id: &str,
    user_phid: &str,
) -> Result<(CommitMessage, bool), String> {
    let id_arg = i64::try_from(parse_update_revision_id(revision_id)?).unwrap_or(i64::MAX);
    let params = diff_submit::differential_query_params(&[id_arg]);
    let result = call(diff_submit::METHOD_QUERY, &params)?;
    let row = result
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if row.is_null() {
        return Err(diff_submit::unknown_revision_message(revision_id));
    }
    check_revision_ownership(reader, writer, &row, user_phid)?;
    let is_draft =
        row.get("statusName").and_then(serde_json::Value::as_str) == Some("Draft");
    let params = diff_submit::get_commit_message_edit_params(id_arg);
    let template = call(diff_submit::METHOD_GET_COMMIT_MESSAGE, &params)?;
    let Some(corpus) = template.as_str() else {
        return Err(FAILED_TO_RETRIEVE_TEMPLATE.to_string());
    };
    let mut message = CommitMessage::new_from_raw_corpus(corpus)?;
    message
        .pull_data_from_conduit(call, false)
        .map_err(PullDataError::into_message)?;
    Ok((message, is_draft))
}

/// Resolved `buildCommitMessage()` source (`:1292-1357`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageSource {
    /// `--only` or raw: no message.
    None,
    /// `--create --message-file`: from file.
    CreateFromFile(String),
    /// Create (or `--verbatim`): interactive editor.
    CreateFromUser,
    /// `--update`: from revision (numeric id).
    UpdateFromRevision(u64),
}

/// Dispatch `buildCommitMessage()` (`:1292-1357`).
///
/// `loaded` carries the working-copy revision lookup (id, title) when the
/// caller ran it; it is required unless `only`, raw, `create`, or
/// `update` short-circuits. Zero revisions create; one updates; several
/// fail. The lookup itself (`loadWorkingCopyDifferentialRevisions()`)
/// is caller-provided: per-VCS discovery plus Conduit.
pub fn resolve_message_source(
    only: bool,
    verbatim: bool,
    create: bool,
    update: Option<&str>,
    message_file: Option<&str>,
    is_raw: bool,
    loaded: Option<&[(i64, String)]>,
) -> Result<MessageSource, String> {
    if only || is_raw {
        return Ok(MessageSource::None);
    }
    if verbatim {
        return Ok(MessageSource::CreateFromUser);
    }
    if !create && update.is_none() {
        let revisions = loaded.unwrap_or(&[]);
        if revisions.is_empty() {
            return Ok(MessageSource::CreateFromUser);
        }
        if revisions.len() > 1 {
            let list: Vec<(i64, &str)> = revisions
                .iter()
                .map(|(id, title)| (*id, title.as_str()))
                .collect();
            return Err(diff_submit::several_revisions_error(
                &diff_submit::render_revision_list(&list),
            ));
        }
        return Ok(MessageSource::UpdateFromRevision(
            u64::try_from(revisions[0].0).unwrap_or(0),
        ));
    }
    if create {
        if let Some(file) = message_file {
            return Ok(MessageSource::CreateFromFile(file.to_string()));
        }
        return Ok(MessageSource::CreateFromUser);
    }
    match update {
        Some(id) => Ok(MessageSource::UpdateFromRevision(parse_update_revision_id(id)?)),
        // Unreachable: `!create && update.is_none()` returned above.
        None => Ok(MessageSource::CreateFromUser),
    }
}

/* -(  Clean working-copy check (D3)  )--------------------------------------- */

/// Commit-mode state machine (`COMMIT_DISABLE/ENABLE/ALLOW`,
/// `src/workflow/ArcanistWorkflow.php`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMode {
    /// `--uncommitted`: never offer to commit.
    Disable,
    /// `--add-all`: always commit without asking.
    Enable,
    /// Neither flag: prompt.
    Allow,
}

/// Commit-mode selection (`runDiffSetupBasics()`,
/// `src/workflow/ArcanistDiffWorkflow.php:608-614`).
#[must_use]
pub fn resolve_commit_mode(add_all: bool, uncommitted: bool) -> CommitMode {
    if add_all {
        CommitMode::Enable
    } else if uncommitted {
        CommitMode::Disable
    } else {
        CommitMode::Allow
    }
}

/// Working-copy change lists: mask-filtered status
/// (`getUncommittedStatus()` plus the `get*Changes()` getters,
/// `src/repository/api/ArcanistRepositoryAPI.php:380-440`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingCopyChanges {
    /// `FLAG_INCOMPLETE` paths (svn only).
    pub incomplete: Vec<String>,
    /// `FLAG_CONFLICT` paths.
    pub conflicts: Vec<String>,
    /// `FLAG_MISSING` paths.
    pub missing: Vec<String>,
    /// `FLAG_EXTERNALS` paths.
    pub externals: Vec<String>,
    /// `FLAG_UNCOMMITTED` paths.
    pub uncommitted: Vec<String>,
    /// `FLAG_UNSTAGED` paths.
    pub unstaged: Vec<String>,
    /// `FLAG_UNTRACKED` paths.
    pub untracked: Vec<String>,
}

/// Mask-filter one status map into change lists.
///
/// PHP `ksort()`s the status first; `BTreeMap` iteration is already
/// sorted. Masks arrive as
/// `[incomplete, conflict, missing, externals, uncommitted, unstaged, untracked]`.
fn changes_from_status(status: &BTreeMap<String, u32>, masks: [u32; 7]) -> WorkingCopyChanges {
    let mut out = WorkingCopyChanges::default();
    for (path, mask) in status {
        if mask & masks[0] != 0 {
            out.incomplete.push(path.clone());
        }
        if mask & masks[1] != 0 {
            out.conflicts.push(path.clone());
        }
        if mask & masks[2] != 0 {
            out.missing.push(path.clone());
        }
        if mask & masks[3] != 0 {
            out.externals.push(path.clone());
        }
        if mask & masks[4] != 0 {
            out.uncommitted.push(path.clone());
        }
        if mask & masks[5] != 0 {
            out.unstaged.push(path.clone());
        }
        if mask & masks[6] != 0 {
            out.untracked.push(path.clone());
        }
    }
    out
}

/// Git change lists from `git::uncommitted_status()`.
#[must_use]
pub fn git_working_copy_changes(status: &BTreeMap<String, u32>) -> WorkingCopyChanges {
    changes_from_status(
        status,
        [
            git::FLAG_INCOMPLETE,
            git::FLAG_CONFLICT,
            git::FLAG_MISSING,
            git::FLAG_EXTERNALS,
            git::FLAG_UNCOMMITTED,
            git::FLAG_UNSTAGED,
            git::FLAG_UNTRACKED,
        ],
    )
}

/// Mercurial change lists from `hg::get_uncommitted_status_with()`.
#[must_use]
pub fn hg_working_copy_changes(status: &BTreeMap<String, u32>) -> WorkingCopyChanges {
    changes_from_status(
        status,
        [
            hg::FLAG_INCOMPLETE,
            hg::FLAG_CONFLICT,
            hg::FLAG_MISSING,
            hg::FLAG_EXTERNALS,
            hg::FLAG_UNCOMMITTED,
            hg::FLAG_UNSTAGED,
            hg::FLAG_UNTRACKED,
        ],
    )
}

/// Subversion change lists from `svn::get_svn_status_with()`.
#[must_use]
pub fn svn_working_copy_changes(status: &BTreeMap<String, u32>) -> WorkingCopyChanges {
    changes_from_status(
        status,
        [
            svn::FLAG_INCOMPLETE,
            svn::FLAG_CONFLICT,
            svn::FLAG_MISSING,
            svn::FLAG_EXTERNALS,
            svn::FLAG_UNCOMMITTED,
            svn::FLAG_UNSTAGED,
            svn::FLAG_UNTRACKED,
        ],
    )
}

/// `Working copy:` description line (`:1025-1029`).
///
/// PHP wraps the path in `__...__` console markup (underline when
/// colorized, stripped otherwise); the port never colorizes, so the
/// plain shape renders.
#[must_use]
pub fn working_copy_desc(work_path: &str) -> String {
    format!("  Working copy: {work_path}\n\n")
}

/// Incomplete-checkout error (`:1032-1048`).
///
/// The stray apostrophe in `proceeding.'` is upstream verbatim.
#[must_use]
pub fn incomplete_checkout_error(desc: &str, incomplete: &[String]) -> String {
    format!(
        "{}\n\n{}  {}\n    {}\n\n{}",
        "You have incompletely checked out directories in this working copy. Fix them before \
         proceeding.'",
        desc,
        "Incomplete directories in working copy:",
        incomplete.join("\n    "),
        "You can fix these paths by running 'svn update' on them."
    )
}

/// Merge-conflict error (`:1050-1062`).
#[must_use]
pub fn merge_conflicts_error(desc: &str, conflicts: &[String]) -> String {
    format!(
        "{}\n\n{}  {}\n    {}",
        "You have merge conflicts in this working copy. Resolve merge conflicts before proceeding.",
        desc,
        "Conflicts in working copy:",
        conflicts.join("\n    ")
    )
}

/// Missing-files error (`:1064-1078`): note the trailing newline.
#[must_use]
pub fn missing_files_error(desc: &str, missing: &[String]) -> String {
    format!(
        "{}\n\n{}  {}\n    {}\n",
        "You have missing files in this working copy. Revert or formally remove them (with `svn \
         rm`) before proceeding.",
        desc,
        "Missing files in working copy:",
        missing.join("\n    ")
    )
}

/// Thousands-grouped count (`PhutilNumber` via `formatNumber()`,
/// `src/internationalization/PhutilTranslator.php:249-256`).
#[must_use]
pub fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(byte as char);
    }
    out
}

/// Dirty-externals notice block (`:1088-1103`): paragraph plus one
/// 6-space-indented item per path (`PhutilConsoleBlock`/`PhutilConsoleList`).
#[must_use]
pub fn dirty_externals_block(externals: &[String]) -> String {
    let mut out = format!(
        "{} submodule(s) have uncommitted or untracked changes:\n\n",
        format_count(externals.len())
    );
    for external in externals {
        out.push_str(&format!("      {external}\n"));
    }
    out
}

/// Prompt to ignore dirty externals (`:1090-1094`, default yes).
#[must_use]
pub fn ignore_externals_prompt(count: usize) -> String {
    format!(
        "Ignore the changes to these {} submodule(s) and continue?",
        format_count(count)
    )
}

/// Untracked-files lead (`:1123-1126`).
pub const UNTRACKED_FILES_HERE: &str = "You have untracked files in this working copy.";
/// Untracked-files list header (`:1149`).
pub const UNTRACKED_CHANGES_IN: &str = "Untracked changes in working copy:";
/// Unstaged-files list header (`:1177`).
pub const UNSTAGED_CHANGES_IN: &str = "Unstaged changes in working copy:";
/// Uncommitted-files list header (`:1185`).
pub const UNCOMMITTED_CHANGES_IN: &str = "Uncommitted changes in working copy:";
/// Uncommitted-changes lead (`:1168-1170`).
pub const UNCOMMITTED_CHANGES_HERE: &str = "You have uncommitted changes in this working copy.";

/// Ignore hint naming the per-VCS ignore file (`:1129-1143`).
#[must_use]
pub fn untracked_hint(kind: diff::VcsKind, count: usize) -> String {
    let file = match kind {
        diff::VcsKind::Git => ".git/info/exclude",
        diff::VcsKind::Svn => "svn:ignore",
        diff::VcsKind::Hg => ".hgignore",
    };
    format!(
        "(To ignore these {} change(s), add them to \"{file}\".)",
        format_count(count)
    )
}

/// Untracked-files list echo (`:1147-1151`).
#[must_use]
pub fn untracked_list_echo(hint: &str, untracked: &[String]) -> String {
    format!(
        "  {}\n  {}\n    {}",
        UNTRACKED_CHANGES_IN,
        hint,
        untracked.join("\n    ")
    )
}

/// Prompt to ignore untracked files (`:1153-1157`, default no).
#[must_use]
pub fn ignore_untracked_prompt(count: usize) -> String {
    format!(
        "Ignore these {} untracked file(s) and continue?",
        format_count(count)
    )
}

/// Unstaged list echo: the header carries two leading spaces (`:1175-1180`).
#[must_use]
pub fn unstaged_list_echo(unstaged: &[String]) -> String {
    format!(
        "  {}\n    {}",
        UNSTAGED_CHANGES_IN,
        unstaged.join("\n    ")
    )
}

/// Uncommitted list echo: no header indent (`:1183-1188`).
#[must_use]
pub fn uncommitted_list_echo(uncommitted: &[String]) -> String {
    format!(
        "{}\n    {}",
        UNCOMMITTED_CHANGES_IN,
        uncommitted.join("\n    ")
    )
}

/// Prompt to amend the current commit (`:1350-1354`, default no).
#[must_use]
pub fn amend_changes_prompt(count: usize) -> String {
    format!(
        "Do you want to amend these {} change(s) to the current commit?",
        format_count(count)
    )
}

/// Prompt to create a new commit (`:1355-1359`, default no).
#[must_use]
pub fn create_commit_prompt(count: usize) -> String {
    format!(
        "Do you want to create a new commit with these {} change(s)?",
        format_count(count)
    )
}

/// Refusal to continue with uncommitted changes (`:1209-1213`).
pub const CANNOT_CONTINUE_UNCOMMITTED: &str =
    "You can not continue with uncommitted changes. Commit or discard them before proceeding.";
/// Autostash notice (`:1202-1205`).
pub const STASHING_UNCOMMITTED: &str =
    "Stashing uncommitted changes. (You can restore them with `git stash pop`).";
/// Empty commit-template error (`:1246-1249`).
pub const MUST_PROVIDE_MESSAGE: &str = "You must provide a commit message.";
/// Empty commit-message error (`:1255-1258`, dead in PHP: comment removal
/// always leaves the trailing newline, so the length is never zero).
pub const MUST_PROVIDE_NONEMPTY_MESSAGE: &str = "You must provide a nonempty commit message.";
/// Commit-template editor name (`:1236`).
pub const NAME_COMMIT_MESSAGE: &str = "commit-message";
/// Commit-template editor task (`:1237-1240`).
pub const TASK_SUPPLY_COMMIT_MESSAGE: &str =
    "Supply commit message for uncommitted changes, then save and exit.";

/// Commit-message editor template (`:1224-1233`): sorted unique paths.
#[must_use]
pub fn commit_editor_template(paths: &[String]) -> String {
    let mut sorted: Vec<&str> = paths.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out = "\n\n# Enter a commit message.\n#\n# Changes:\n#\n".to_string();
    for path in sorted {
        out.push_str(&format!("#     {path}\n"));
    }
    out
}

/// Commit-mode prompt decision (`askForAdd()`, `:1330-1344`).
pub fn ask_for_add<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    mode: CommitMode,
    should_amend: bool,
    files: &[String],
) -> Result<bool, String> {
    match mode {
        CommitMode::Disable => Ok(false),
        CommitMode::Enable => Ok(true),
        CommitMode::Allow => {
            let prompt = if should_amend {
                amend_changes_prompt(files.len())
            } else {
                create_commit_prompt(files.len())
            };
            Ok(diff::prompt_confirm_from(reader, writer, &prompt, true)?)
        }
    }
}

/// VCS hooks for the clean check.
pub struct CleanCheckVcs<'a> {
    /// Working-copy kind (svn zeroes the externals list, `:1083-1086`).
    pub kind: diff::VcsKind,
    /// `getPath()` for the `Working copy:` description.
    pub work_path: String,
    /// `supportsLocalCommits()`.
    pub supports_local_commits: bool,
    /// `canStashChanges()`.
    pub can_stash: bool,
    /// `arc.autostash` configuration.
    pub autostash: bool,
    /// `shouldRequireCleanUntrackedFiles()` (false under
    /// `--allow-untracked`).
    pub require_clean_untracked: bool,
    /// Precomputed `getShouldAmend()`.
    pub should_amend: bool,
    /// Head local-commit message, when amending may apply.
    pub head_commit_message: Option<String>,
    /// `addToCommit()`.
    pub add_to_commit: Option<&'a dyn Fn(&[String]) -> Result<(), String>>,
    /// `stashChanges()`.
    pub stash_changes: Option<&'a dyn Fn() -> Result<(), String>>,
    /// `doCommit()`.
    pub do_commit: Option<&'a dyn Fn(&str) -> Result<(), String>>,
    /// `amendCommit()`.
    pub amend_commit: Option<&'a dyn Fn(&str) -> Result<(), String>>,
    /// Commit-message editor: template text to edited text.
    pub edit: &'a dyn Fn(&str) -> Result<String, String>,
}

/// Clean-check outcome: whether the flow stashed (`$this->stashed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanCheckOutcome {
    /// True after autostashing.
    pub stashed: bool,
}

/// Order-preserving difference (`array_diff()`, `:1113-1115`).
fn without_paths(paths: &[String], remove: &[Vec<String>]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !remove.iter().any(|list| list.contains(path)))
        .cloned()
        .collect()
}

/// Dirty-externals gate (`:1080-1107`).
fn check_dirty_externals<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    kind: diff::VcsKind,
    externals: &[String],
) -> Result<(), String> {
    let mut externals = externals.to_vec();
    if kind == diff::VcsKind::Svn {
        externals.clear();
    }
    if externals.is_empty() {
        return Ok(());
    }
    let block = dirty_externals_block(&externals);
    writer
        .write_all(block.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    if !diff::prompt_confirm_from(reader, writer, &ignore_externals_prompt(externals.len()), false)?
    {
        return Err(USER_ABORT.to_string());
    }
    Ok(())
}

/// Untracked-files gate (`:1117-1159`).
fn check_untracked_files<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    vcs: &CleanCheckVcs<'_>,
    desc: &str,
    untracked: &[String],
) -> Result<(), String> {
    if untracked.is_empty() || !vcs.require_clean_untracked {
        return Ok(());
    }
    let echo = format!("{UNTRACKED_FILES_HERE}\n\n{desc}");
    writer
        .write_all(echo.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    let hint = untracked_hint(vcs.kind, untracked.len());
    let list = untracked_list_echo(&hint, untracked);
    writer
        .write_all(list.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    if !diff::prompt_confirm_from(
        reader,
        writer,
        &ignore_untracked_prompt(untracked.len()),
        true,
    )? {
        return Err(USER_ABORT.to_string());
    }
    Ok(())
}

/// Uncommitted-changes gate (`:1162-1215`).
///
/// Returns whether the flow should commit.
fn check_uncommitted_changes<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    vcs: &CleanCheckVcs<'_>,
    desc: &str,
    unstaged: &[String],
    uncommitted: &[String],
    mode: CommitMode,
) -> Result<bool, String> {
    if unstaged.is_empty() && uncommitted.is_empty() {
        return Ok(false);
    }
    let echo = format!("{UNCOMMITTED_CHANGES_HERE}\n\n{desc}");
    writer
        .write_all(echo.as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    let mut lists = Vec::new();
    if !unstaged.is_empty() {
        lists.push(unstaged_list_echo(unstaged));
    }
    if !uncommitted.is_empty() {
        lists.push(uncommitted_list_echo(uncommitted));
    }
    writer
        .write_all(format!("{}\n", lists.join("\n\n")).as_bytes())
        .map_err(|error| format!("failed to write prompt: {error}"))?;
    let mut all = unstaged.to_vec();
    all.extend(uncommitted.iter().cloned());
    if ask_for_add(reader, writer, mode, vcs.should_amend, &all)? {
        if !unstaged.is_empty()
            && let Some(add) = vcs.add_to_commit
        {
            add(unstaged)?;
        }
        return Ok(true);
    }
    if vcs.autostash && vcs.can_stash
        && let Some(stash) = vcs.stash_changes
    {
        writer
            .write_all(format!("{STASHING_UNCOMMITTED}\n").as_bytes())
            .map_err(|error| format!("failed to write prompt: {error}"))?;
        stash()?;
        return Ok(false);
    }
    if vcs.autostash && vcs.can_stash {
        return Ok(false);
    }
    Err(CANNOT_CONTINUE_UNCOMMITTED.to_string())
}

/// Commit-or-amend UX (`:1217-1261`).
fn commit_clean_check_changes(
    vcs: &CleanCheckVcs<'_>,
    unstaged: &[String],
    uncommitted: &[String],
) -> Result<(), String> {
    if vcs.should_amend {
        let Some(head) = vcs.head_commit_message.clone() else {
            return Err("cannot amend without a local commit".to_string());
        };
        if let Some(amend) = vcs.amend_commit {
            amend(&head)?;
        }
        return Ok(());
    }
    if !vcs.supports_local_commits {
        return Ok(());
    }
    let mut paths = uncommitted.to_vec();
    paths.extend(unstaged.iter().cloned());
    let template = commit_editor_template(&paths);
    let edited = (vcs.edit)(&template)?;
    if edited == template {
        return Err(MUST_PROVIDE_MESSAGE.to_string());
    }
    let message = remove_comments(&edited);
    if message.is_empty() {
        return Err(MUST_PROVIDE_NONEMPTY_MESSAGE.to_string());
    }
    if let Some(commit) = vcs.do_commit {
        commit(&message)?;
    }
    Ok(())
}

/// Require a clean working copy (`requireCleanWorkingCopy()`,
/// `src/workflow/ArcanistWorkflow.php:1014-1262`).
///
/// Runs the fatal gates (incomplete, conflicts, missing), the
/// externals/untracked confirms, the commit-or-stash decision, and the
/// commit/amend UX. VCS state arrives precomputed in `changes`; effects
/// run through `vcs` hooks.
pub fn require_clean_working_copy<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    vcs: &CleanCheckVcs<'_>,
    changes: &WorkingCopyChanges,
    mode: CommitMode,
) -> Result<CleanCheckOutcome, String> {
    let desc = working_copy_desc(&vcs.work_path);
    if !changes.incomplete.is_empty() {
        return Err(incomplete_checkout_error(&desc, &changes.incomplete));
    }
    if !changes.conflicts.is_empty() {
        return Err(merge_conflicts_error(&desc, &changes.conflicts));
    }
    if !changes.missing.is_empty() {
        return Err(missing_files_error(&desc, &changes.missing));
    }
    check_dirty_externals(reader, writer, vcs.kind, &changes.externals)?;
    let unstaged = without_paths(&changes.unstaged, &[changes.externals.clone()]);
    let uncommitted = without_paths(&changes.uncommitted, &[unstaged.clone(), changes.externals.clone()]);
    check_untracked_files(reader, writer, vcs, &desc, &changes.untracked)?;
    let should_commit =
        check_uncommitted_changes(reader, writer, vcs, &desc, &unstaged, &uncommitted, mode)?;
    // The gate stashes exactly when the lists are non-empty, no commit
    // was requested, and autostashing applies (`$this->stashed`).
    let stashed = !should_commit
        && (!unstaged.is_empty() || !uncommitted.is_empty())
        && vcs.autostash
        && vcs.can_stash;
    if should_commit {
        commit_clean_check_changes(vcs, &unstaged, &uncommitted)?;
    }
    Ok(CleanCheckOutcome { stashed })
}

/// Project repository reference for the amend check (`id`, `callsign`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmendRepository {
    /// Diffusion repository id.
    pub id: i64,
    /// Repository callsign, when set.
    pub callsign: Option<String>,
}

/// Inputs for the amend decision (`calculateShouldAmend()`,
/// `src/workflow/ArcanistWorkflow.php:1268-1330`).
pub struct AmendCheck<'a> {
    /// `history.immutable` configuration.
    pub history_immutable: bool,
    /// `supportsAmend()`.
    pub supports_amend: bool,
    /// Head local commit, newest first.
    pub head: Option<LocalCommitInfo>,
    /// `getAuthor()`.
    pub api_author: &'a str,
    /// `--create` was given.
    pub create: bool,
    /// Project repository (`loadProjectRepository()`), when known.
    pub repository: Option<AmendRepository>,
    /// Open working-copy revisions for the viewer
    /// (`loadWorkingCopyDifferentialRevisions()`, `status-open`).
    pub load_open_revisions: &'a dyn Fn() -> Result<Vec<(i64, String)>, String>,
    /// Conduit caller (`diffusion.querycommits`).
    pub call: &'a diff_submit::ConduitCall<'a>,
}

/// PHP truthiness for conduit values: null, false, 0, `""`, and empty
/// arrays/objects are falsy.
fn conduit_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => {
            number.as_i64().is_some_and(|n| n != 0)
                || number.as_f64().is_some_and(|n| n != 0.0)
        }
        serde_json::Value::String(text) => !text.is_empty() && text != "0",
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}

/// Commit name for the published check (`:1304-1309`): old-style
/// `r{callsign}{hash}` when a callsign exists, else `R{id}:{hash}`.
#[must_use]
pub fn amend_commit_name(repository: &AmendRepository, commit_hash: &str) -> String {
    match &repository.callsign {
        Some(callsign) => format!("r{callsign}{commit_hash}"),
        None => format!("R{}:{commit_hash}", repository.id),
    }
}

/// Decide whether uncommitted changes amend the head commit
/// (`calculateShouldAmend()`, `:1268-1330`).
pub fn calculate_should_amend(check: &AmendCheck) -> Result<bool, String> {
    if check.history_immutable || !check.supports_amend {
        return Ok(false);
    }
    let Some(head) = &check.head else {
        return Ok(false);
    };
    let message = CommitMessage::new_from_raw_corpus(&head.message)?;
    if message.git_svn_base_revision.is_some() {
        return Ok(false);
    }
    if check.api_author != head.author {
        return Ok(false);
    }
    if message.revision_id.is_some() && check.create {
        return Ok(false);
    }
    if let Some(repository) = &check.repository {
        let name = amend_commit_name(repository, &head.commit);
        let params = diff_submit::diffusion_query_commits_params(&[name.as_str()]);
        let result = (check.call)(diff_submit::METHOD_QUERY_COMMITS, &params)?;
        let known = result
            .get("identifierMap")
            .and_then(|map| map.get(name.as_str()))
            .unwrap_or(&serde_json::Value::Null);
        if conduit_truthy(known) {
            return Ok(false);
        }
    }
    if message.revision_id.is_none() {
        return Ok(true);
    }
    Ok(!(check.load_open_revisions)()?.is_empty())
}

#[cfg(test)]
#[path = "diff_local_tests.rs"]
mod diff_local_tests;
