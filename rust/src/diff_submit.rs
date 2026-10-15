//! Differential submit/update payload builders and submit-flow helpers.
//!
//! Port of the submit/update paths in `src/workflow/ArcanistDiffWorkflow.php`
//! (`run`, lines 345-577): build the `differential.creatediff` spec, create
//! or update the revision, fetch the revised commit message, and attach diff
//! properties. Every helper takes plain values and returns plain values; all
//! network I/O goes through a caller-supplied conduit seam (method name +
//! params -> result), so these helpers run fully offline against canned
//! responses.
//!
//! Conduit method names and payload field names are verbatim from PHP; each
//! builder cites the PHP lines it ports.
//!
//! Divergences (behavior limits with PHP refs):
//! - `set_diff_properties` calls `differential.setdiffproperty` sequentially
//!   over the seam; PHP fans the calls out as futures and resolves them
//!   together (`ArcanistDiffWorkflow.php:2403-2424`).
//! - Interactive editing is not ported: `--edit`, the `edit-messages.json`
//!   / `update-messages.json` scratch files, and the `--message` prompt
//!   (`ArcanistDiffWorkflow.php:654-716`). Callers pass final `fields` and
//!   `message` values in.
//! - `getUpdateMessage` templates, staging-area push (`:2598-2622`),
//!   autotargets (`:401-404`), binary file uploads (`:2473-2507`), lint/unit
//!   execution (`:1146-1280`), commit amend (`:539-549`), browser opening,
//!   and event dispatch are caller concerns, not ported.
//! - `parse_commit_message_parse_response` only treats array-shaped `errors`
//!   as failures; PHP `empty()` also rejects other truthy shapes
//!   (`ArcanistDifferentialCommitMessage.php:64-67`).

/// `differential.creatediff` (`ArcanistDiffWorkflow.php:384`).
pub const METHOD_CREATE_DIFF: &str = "differential.creatediff";
/// `differential.updaterevision` (`ArcanistDiffWorkflow.php:464`).
pub const METHOD_UPDATE_REVISION: &str = "differential.updaterevision";
/// `differential.revision.edit` (`ArcanistDiffWorkflow.php:497`).
pub const METHOD_REVISION_EDIT: &str = "differential.revision.edit";
/// `differential.createrevision` (`ArcanistDiffWorkflow.php:526`).
pub const METHOD_CREATE_REVISION: &str = "differential.createrevision";
/// `differential.getcommitmessage` (`ArcanistDiffWorkflow.php:534`).
pub const METHOD_GET_COMMIT_MESSAGE: &str = "differential.getcommitmessage";
/// `differential.setdiffproperty` (`ArcanistDiffWorkflow.php:2405`).
pub const METHOD_SET_DIFF_PROPERTY: &str = "differential.setdiffproperty";
/// `differential.query` (`ArcanistDiffWorkflow.php:1581`).
pub const METHOD_QUERY: &str = "differential.query";
/// `user.query` (`ArcanistDiffWorkflow.php:1636`).
pub const METHOD_USER_QUERY: &str = "user.query";
/// `differential.revision.search` (`ArcanistDiffWorkflow.php:2124`).
pub const METHOD_REVISION_SEARCH: &str = "differential.revision.search";
/// `differential.diff.search` (`ArcanistDiffWorkflow.php:2139`).
pub const METHOD_DIFF_SEARCH: &str = "differential.diff.search";
/// `differential.querydiffs` (`ArcanistDiffWorkflow.php:2168`).
pub const METHOD_QUERY_DIFFS: &str = "differential.querydiffs";
/// `differential.parsecommitmessage`
/// (`ArcanistDifferentialCommitMessage.php:53`).
pub const METHOD_PARSE_COMMIT_MESSAGE: &str = "differential.parsecommitmessage";

/// Diff property holding unresolved lint messages
/// (`ArcanistDiffWorkflow.php:2300`).
pub const PROP_LINT: &str = "arc:lint";
/// Diff property holding unit test results
/// (`ArcanistDiffWorkflow.php:2317`).
pub const PROP_UNIT: &str = "arc:unit";
/// Diff property holding local commit info
/// (`ArcanistDiffWorkflow.php:2338`).
pub const PROP_LOCAL_COMMITS: &str = "local:commits";
/// Diff property holding land-onto targets
/// (`ArcanistDiffWorkflow.php:2348`).
pub const PROP_ONTO: &str = "arc:onto";

/// `creationMethod` value for diffs built by arc
/// (`ArcanistDiffWorkflow.php:2272`).
pub const CREATION_METHOD_ARC: &str = "arc";

/// `ArcanistLintWorkflow::RESULT_OKAY` (`ArcanistLintWorkflow.php:8`).
pub const LINT_RESULT_OKAY: i64 = 0;
/// `ArcanistLintWorkflow::RESULT_WARNINGS` (`ArcanistLintWorkflow.php:9`).
pub const LINT_RESULT_WARNINGS: i64 = 1;
/// `ArcanistLintWorkflow::RESULT_ERRORS` (`ArcanistLintWorkflow.php:10`).
pub const LINT_RESULT_ERRORS: i64 = 2;
/// `ArcanistLintWorkflow::RESULT_SKIP` (`ArcanistLintWorkflow.php:11`).
pub const LINT_RESULT_SKIP: i64 = 3;

/// `ArcanistUnitWorkflow::RESULT_OKAY` (`ArcanistUnitWorkflow.php:8`).
pub const UNIT_RESULT_OKAY: i64 = 0;
/// `ArcanistUnitWorkflow::RESULT_UNSOUND` (`ArcanistUnitWorkflow.php:9`).
pub const UNIT_RESULT_UNSOUND: i64 = 1;
/// `ArcanistUnitWorkflow::RESULT_FAIL` (`ArcanistUnitWorkflow.php:10`).
pub const UNIT_RESULT_FAIL: i64 = 2;
/// `ArcanistUnitWorkflow::RESULT_SKIP` (`ArcanistUnitWorkflow.php:11`).
pub const UNIT_RESULT_SKIP: i64 = 3;

/// Diff spec status "okay" (`ArcanistDiffWorkflow.php:2190`).
pub const STATUS_OKAY: &str = "okay";
/// Diff spec status "fail" (`ArcanistDiffWorkflow.php:2191`).
pub const STATUS_FAIL: &str = "fail";
/// Diff spec status "warn" (`ArcanistDiffWorkflow.php:2192`).
pub const STATUS_WARN: &str = "warn";
/// Diff spec status "skip" (`ArcanistDiffWorkflow.php:2194`).
pub const STATUS_SKIP: &str = "skip";
/// Diff spec status "none" (`ArcanistDiffWorkflow.php:2195`).
pub const STATUS_NONE: &str = "none";

/// `ArcanistDiffChangeType::TYPE_ADD` (`ArcanistDiffChangeType.php:8`).
pub const TYPE_ADD: i64 = 1;
/// `ArcanistDiffChangeType::TYPE_CHANGE` (`ArcanistDiffChangeType.php:9`).
pub const TYPE_CHANGE: i64 = 2;
/// `ArcanistDiffChangeType::TYPE_DELETE` (`ArcanistDiffChangeType.php:10`).
pub const TYPE_DELETE: i64 = 3;
/// `ArcanistDiffChangeType::TYPE_MOVE_AWAY` (`ArcanistDiffChangeType.php:11`).
pub const TYPE_MOVE_AWAY: i64 = 4;
/// `ArcanistDiffChangeType::TYPE_COPY_AWAY` (`ArcanistDiffChangeType.php:12`).
pub const TYPE_COPY_AWAY: i64 = 5;
/// `ArcanistDiffChangeType::TYPE_MOVE_HERE` (`ArcanistDiffChangeType.php:13`).
pub const TYPE_MOVE_HERE: i64 = 6;
/// `ArcanistDiffChangeType::TYPE_COPY_HERE` (`ArcanistDiffChangeType.php:14`).
pub const TYPE_COPY_HERE: i64 = 7;
/// `ArcanistDiffChangeType::TYPE_MULTICOPY` (`ArcanistDiffChangeType.php:15`).
pub const TYPE_MULTICOPY: i64 = 8;
/// `ArcanistDiffChangeType::TYPE_MESSAGE` (`ArcanistDiffChangeType.php:16`).
pub const TYPE_MESSAGE: i64 = 9;
/// `ArcanistDiffChangeType::TYPE_CHILD` (`ArcanistDiffChangeType.php:17`).
pub const TYPE_CHILD: i64 = 10;

/// `ArcanistDiffChangeType::FILE_TEXT` (`ArcanistDiffChangeType.php:19`).
pub const FILE_TEXT: i64 = 1;
/// `ArcanistDiffChangeType::FILE_IMAGE` (`ArcanistDiffChangeType.php:20`).
pub const FILE_IMAGE: i64 = 2;
/// `ArcanistDiffChangeType::FILE_BINARY` (`ArcanistDiffChangeType.php:21`).
pub const FILE_BINARY: i64 = 3;
/// `ArcanistDiffChangeType::FILE_DIRECTORY` (`ArcanistDiffChangeType.php:22`).
pub const FILE_DIRECTORY: i64 = 4;
/// `ArcanistDiffChangeType::FILE_SYMLINK` (`ArcanistDiffChangeType.php:23`).
pub const FILE_SYMLINK: i64 = 5;
/// `ArcanistDiffChangeType::FILE_DELETED` (`ArcanistDiffChangeType.php:24`).
pub const FILE_DELETED: i64 = 6;
/// `ArcanistDiffChangeType::FILE_NORMAL` (`ArcanistDiffChangeType.php:25`).
pub const FILE_NORMAL: i64 = 7;

/// Abort text when updating a revision owned by someone else
/// (`ArcanistDiffWorkflow.php:2464-2466`).
pub const OWNERSHIP_ABORT: &str = "Aborted update of revision: You are not the owner.";

/// Caller-supplied conduit seam: method name + params -> parsed `result`.
pub type ConduitCall<'a> =
    dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, String> + 'a;

/// Map a lint result code to a diff spec status.
///
/// Ports `getLintStatus` (`ArcanistDiffWorkflow.php:2188-2196`).
#[must_use]
pub fn lint_status(result: i64) -> &'static str {
    match result {
        LINT_RESULT_OKAY => STATUS_OKAY,
        LINT_RESULT_ERRORS => STATUS_FAIL,
        LINT_RESULT_WARNINGS => STATUS_WARN,
        LINT_RESULT_SKIP => STATUS_SKIP,
        _ => STATUS_NONE,
    }
}

/// Map a unit result code to a diff spec status.
///
/// Ports `getUnitStatus` (`ArcanistDiffWorkflow.php:2202-2210`).
#[must_use]
pub fn unit_status(result: i64) -> &'static str {
    match result {
        UNIT_RESULT_OKAY => STATUS_OKAY,
        UNIT_RESULT_FAIL => STATUS_FAIL,
        UNIT_RESULT_UNSOUND => STATUS_WARN,
        UNIT_RESULT_SKIP => STATUS_SKIP,
        _ => STATUS_NONE,
    }
}

/// One hunk of a file change; keys mirror
/// `ArcanistDiffHunk::toDictionary` (`ArcanistDiffHunk.php:18-30`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffHunk {
    /// `oldOffset`.
    pub old_offset: i64,
    /// `newOffset`.
    pub new_offset: i64,
    /// `oldLength`.
    pub old_length: i64,
    /// `newLength`.
    pub new_length: i64,
    /// `addLines`.
    pub add_lines: i64,
    /// `delLines`.
    pub del_lines: i64,
    /// `isMissingOldNewline`.
    pub is_missing_old_newline: bool,
    /// `isMissingNewNewline`.
    pub is_missing_new_newline: bool,
    /// `corpus`.
    pub corpus: String,
}

/// One file change; keys mirror `ArcanistDiffChange::toDictionary`
/// (`ArcanistDiffChange.php:46-64`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffChange {
    /// `metadata`.
    pub metadata: serde_json::Value,
    /// `oldPath`.
    pub old_path: String,
    /// `currentPath`.
    pub current_path: String,
    /// `awayPaths`.
    pub away_paths: Vec<String>,
    /// `oldProperties`.
    pub old_properties: serde_json::Value,
    /// `newProperties`.
    pub new_properties: serde_json::Value,
    /// `type` (`TYPE_*`).
    pub change_type: i64,
    /// `fileType` (`FILE_*`).
    pub file_type: i64,
    /// `commitHash`, null when unknown.
    pub commit_hash: Option<String>,
    /// `hunks`.
    pub hunks: Vec<DiffHunk>,
}

impl Default for DiffChange {
    /// PHP property initializers: `TYPE_CHANGE` / `FILE_TEXT`
    /// (`ArcanistDiffChange.php:18-19`), everything else empty.
    fn default() -> Self {
        Self {
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            old_path: String::new(),
            current_path: String::new(),
            away_paths: Vec::new(),
            old_properties: serde_json::Value::Array(Vec::new()),
            new_properties: serde_json::Value::Array(Vec::new()),
            change_type: TYPE_CHANGE,
            file_type: FILE_TEXT,
            commit_hash: None,
            hunks: Vec::new(),
        }
    }
}

impl DiffChange {
    /// A plain text change of one path, PHP defaults otherwise.
    #[must_use]
    pub fn new_text(old_path: &str, current_path: &str) -> Self {
        Self {
            old_path: old_path.to_string(),
            current_path: current_path.to_string(),
            ..Self::default()
        }
    }
}

/// Render a hunk with verbatim `ArcanistDiffHunk::toDictionary` keys.
#[must_use]
pub fn hunk_json(hunk: &DiffHunk) -> serde_json::Value {
    serde_json::json!({
        "oldOffset": hunk.old_offset,
        "newOffset": hunk.new_offset,
        "oldLength": hunk.old_length,
        "newLength": hunk.new_length,
        "addLines": hunk.add_lines,
        "delLines": hunk.del_lines,
        "isMissingOldNewline": hunk.is_missing_old_newline,
        "isMissingNewNewline": hunk.is_missing_new_newline,
        "corpus": hunk.corpus,
    })
}

/// Render a change with verbatim `ArcanistDiffChange::toDictionary` keys.
#[must_use]
pub fn change_json(change: &DiffChange) -> serde_json::Value {
    let hunks: Vec<serde_json::Value> = change.hunks.iter().map(hunk_json).collect();
    serde_json::json!({
        "metadata": change.metadata,
        "oldPath": change.old_path,
        "currentPath": change.current_path,
        "awayPaths": change.away_paths,
        "oldProperties": change.old_properties,
        "newProperties": change.new_properties,
        "type": change.change_type,
        "fileType": change.file_type,
        "commitHash": change.commit_hash.clone().map_or(serde_json::Value::Null, serde_json::Value::String),
        "hunks": hunks,
    })
}

/// Repository identity half of the diff spec; fields mirror
/// `buildDiffSpecification` (`ArcanistDiffWorkflow.php:2216-2283`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffSpecInput {
    /// `sourceMachine` (`php_uname('n')` in PHP).
    pub source_machine: String,
    /// `sourcePath`.
    pub source_path: Option<String>,
    /// `branch`.
    pub branch: Option<String>,
    /// `bookmark`.
    pub bookmark: Option<String>,
    /// `sourceControlSystem`.
    pub source_control_system: Option<String>,
    /// `sourceControlPath`.
    pub source_control_path: Option<String>,
    /// `sourceControlBaseRevision`.
    pub source_control_base_revision: Option<String>,
    /// `repositoryPHID`, sent only when known (`:2275-2280`).
    pub repository_phid: Option<String>,
}

fn opt_json(value: &Option<String>) -> serde_json::Value {
    value
        .clone()
        .map_or(serde_json::Value::Null, serde_json::Value::String)
}

/// Build the `differential.creatediff` params.
///
/// `changes` render via `toDictionary` (`ArcanistDiffWorkflow.php:377`),
/// `lint`/`unit` are `getLintStatus`/`getUnitStatus` strings (`:378-379`),
/// and the remaining keys come from `buildDiffSpecification` (`:2264-2280`).
/// Missing identity fields are sent as null, like PHP; `repositoryPHID` is
/// omitted when unknown.
#[must_use]
pub fn diff_spec_json(
    changes: &[DiffChange],
    lint: &str,
    unit: &str,
    input: &DiffSpecInput,
) -> serde_json::Value {
    let changes: Vec<serde_json::Value> = changes.iter().map(change_json).collect();
    let mut spec = serde_json::json!({
        "changes": changes,
        "lintStatus": lint,
        "unitStatus": unit,
        "sourceMachine": input.source_machine,
        "sourcePath": opt_json(&input.source_path),
        "branch": opt_json(&input.branch),
        "bookmark": opt_json(&input.bookmark),
        "sourceControlSystem": opt_json(&input.source_control_system),
        "sourceControlPath": opt_json(&input.source_control_path),
        "sourceControlBaseRevision": opt_json(&input.source_control_base_revision),
        "creationMethod": CREATION_METHOD_ARC,
    });
    if let Some(phid) = input.repository_phid.as_deref() {
        spec["repositoryPHID"] = serde_json::Value::String(phid.to_string());
    }
    spec
}

/// Build the `differential.updaterevision` params: `fields`, `id`, `message`,
/// `diffid` (`ArcanistDiffWorkflow.php:436,463-465,698-711`).
#[must_use]
pub fn update_revision_params(
    id: i64,
    fields: serde_json::Value,
    diff_id: i64,
    message: &str,
) -> serde_json::Value {
    serde_json::json!({
        "fields": fields,
        "id": id,
        "message": message,
        "diffid": diff_id,
    })
}

/// Build the legacy `differential.createrevision` params: `fields`, `diffid`
/// (`ArcanistDiffWorkflow.php:436,525-527`).
#[must_use]
pub fn create_revision_params(fields: serde_json::Value, diff_id: i64) -> serde_json::Value {
    serde_json::json!({
        "fields": fields,
        "diffid": diff_id,
    })
}

/// Build the `differential.revision.edit` params: the commit message
/// transactions plus an `update` transaction for the new diff PHID and,
/// for drafts, a `draft` transaction
/// (`ArcanistDiffWorkflow.php:482-500`). A missing diff PHID is sent as
/// null, matching PHP's `idx()` tolerance.
#[must_use]
pub fn revision_edit_params(
    base: &[serde_json::Value],
    diff_phid: Option<&str>,
    is_draft: bool,
) -> serde_json::Value {
    let mut transactions: Vec<serde_json::Value> = base.to_vec();
    transactions.push(serde_json::json!({
        "type": "update",
        "value": diff_phid.map_or(serde_json::Value::Null, |phid| {
            serde_json::Value::String(phid.to_string())
        }),
    }));
    if is_draft {
        transactions.push(serde_json::json!({"type": "draft", "value": true}));
    }
    serde_json::json!({"transactions": transactions})
}

/// Build the post-create `differential.getcommitmessage` params:
/// `revision_id` only (`ArcanistDiffWorkflow.php:533-537`).
#[must_use]
pub fn commit_message_after_create_params(revision_id: i64) -> serde_json::Value {
    serde_json::json!({"revision_id": revision_id})
}

/// Build the update-flow `differential.getcommitmessage` params:
/// `revision_id`, `edit` = `edit`, `fields`
/// (`ArcanistDiffWorkflow.php:662-668`).
#[must_use]
pub fn commit_message_update_params(
    revision_id: i64,
    fields: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "revision_id": revision_id,
        "edit": "edit",
        "fields": fields,
    })
}

/// Build the `--update` message `differential.getcommitmessage` params:
/// `revision_id`, `edit` = `edit` (`ArcanistDiffWorkflow.php:1600-1603`).
#[must_use]
pub fn get_commit_message_edit_params(revision_id: i64) -> serde_json::Value {
    serde_json::json!({
        "revision_id": revision_id,
        "edit": "edit",
    })
}

/// Build the create-template `differential.getcommitmessage` params:
/// `revision_id` = null, `edit` = `create`, `fields`
/// (`ArcanistDiffWorkflow.php:1416-1422`).
#[must_use]
pub fn commit_message_template_params(fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "revision_id": serde_json::Value::Null,
        "edit": "create",
        "fields": fields,
    })
}

/// Build the from-revision `differential.getcommitmessage` params:
/// `revision_id`, `edit` = false (`ArcanistDiffWorkflow.php:1602-1607`).
#[must_use]
pub fn commit_message_from_revision_params(revision_id: i64) -> serde_json::Value {
    serde_json::json!({
        "revision_id": revision_id,
        "edit": false,
    })
}

/// Build the `differential.setdiffproperty` params: `diff_id`, `name`, `data`
/// (`ArcanistDiffWorkflow.php:2403-2411`).
#[must_use]
pub fn set_diff_property_params(diff_id: i64, name: &str, data: &str) -> serde_json::Value {
    serde_json::json!({
        "diff_id": diff_id,
        "name": name,
        "data": data,
    })
}

/// Build the `differential.query` params: `ids`
/// (`ArcanistDiffWorkflow.php:1580-1584`).
#[must_use]
pub fn differential_query_params(ids: &[i64]) -> serde_json::Value {
    serde_json::json!({"ids": ids})
}

/// Build the `user.query` params: `phids`
/// (`ArcanistDiffWorkflow.php:1635-1640`).
#[must_use]
pub fn user_query_params(phids: &[&str]) -> serde_json::Value {
    serde_json::json!({"phids": phids})
}

/// Build the `differential.revision.search` params: `constraints` + `ids`
/// (`ArcanistDiffWorkflow.php:2123-2131`).
#[must_use]
pub fn revision_search_params(ids: &[i64]) -> serde_json::Value {
    serde_json::json!({"constraints": {"ids": ids}})
}

/// Build the `differential.diff.search` params: `constraints` +
/// `revisionPHIDs`, `attachments` + `commits`, `limit` = 1
/// (`ArcanistDiffWorkflow.php:2138-2150`).
#[must_use]
pub fn diff_search_params(revision_phids: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "constraints": {"revisionPHIDs": revision_phids},
        "attachments": {"commits": true},
        "limit": 1,
    })
}

/// Build the `differential.querydiffs` params: `revisionIDs`
/// (`ArcanistDiffWorkflow.php:2167-2171`).
#[must_use]
pub fn query_diffs_params(revision_ids: &[i64]) -> serde_json::Value {
    serde_json::json!({"revisionIDs": revision_ids})
}

/// Build the `differential.parsecommitmessage` params: `corpus`, `partial`
/// (`ArcanistDifferentialCommitMessage.php:52-57`).
#[must_use]
pub fn parse_commit_message_params(corpus: &str, partial: bool) -> serde_json::Value {
    serde_json::json!({
        "corpus": corpus,
        "partial": partial,
    })
}

/// Parsed `differential.creatediff` result: `diffid`, `phid`, `uri`
/// (`ArcanistDiffWorkflow.php:387,399,420`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedDiff {
    /// `diffid`.
    pub diff_id: i64,
    /// `phid`, absent on old servers.
    pub phid: Option<String>,
    /// `uri`.
    pub uri: String,
}

/// Parsed `differential.updaterevision` / `differential.createrevision`
/// result: `revisionid`, `uri` (`ArcanistDiffWorkflow.php:473-474,529-530`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionResult {
    /// `revisionid`.
    pub revision_id: i64,
    /// `uri`.
    pub uri: String,
}

/// Parsed `differential.parsecommitmessage` result: `fields` plus optional
/// `transactions` (`ArcanistDifferentialCommitMessage.php:59-62`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    /// `fields`.
    pub fields: serde_json::Value,
    /// `transactions`, absent before late October 2017.
    pub transactions: Option<serde_json::Value>,
}

/// A reviewer whose `currentStatus` is `away`
/// (`ArcanistDiffWorkflow.php:1654-1674`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwayReviewer {
    /// `userName`, or `userName (realName)` when a real name is set.
    pub name: String,
    /// Raw `currentStatusUntil`.
    pub until: serde_json::Value,
}

/// One `differential.setdiffproperty` write: `name`, `data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffProperty {
    /// `name` (one of `PROP_*`).
    pub name: String,
    /// `data` (a JSON-encoded string in PHP).
    pub data: String,
}

/// Outcome of the `--draft` check when updating a revision
/// (`ArcanistDiffWorkflow.php:438-461`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftUpdate {
    /// Proceed silently (no `--draft`, or updating without it).
    Proceed,
    /// Proceed, showing the warning: the revision is already a draft.
    WarnAlreadyDraft(String),
    /// Abort with the error: a published revision cannot become a draft.
    AbortPublished(String),
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_u64()
            .and_then(|n| i64::try_from(n).ok())
            .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

fn first_row(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => items.first(),
        serde_json::Value::Object(map) => map.values().next(),
        _ => None,
    }
}

/// Parse a `differential.creatediff` result.
pub fn parse_creatediff_response(value: &serde_json::Value) -> Result<CreatedDiff, String> {
    let diff_id = value
        .get("diffid")
        .and_then(json_i64)
        .ok_or_else(|| "differential.creatediff response has no diffid".to_string())?;
    let uri = value
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "differential.creatediff response has no uri".to_string())?;
    Ok(CreatedDiff {
        diff_id,
        phid: value
            .get("phid")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        uri: uri.to_string(),
    })
}

/// Parse a `differential.updaterevision` / `differential.createrevision`
/// result.
pub fn parse_revision_response(value: &serde_json::Value) -> Result<RevisionResult, String> {
    let revision_id = value
        .get("revisionid")
        .and_then(json_i64)
        .ok_or_else(|| "differential revision response has no revisionid".to_string())?;
    let uri = value
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "differential revision response has no uri".to_string())?;
    Ok(RevisionResult {
        revision_id,
        uri: uri.to_string(),
    })
}

/// Parse a `differential.revision.edit` result: `object.id` is required
/// (`ArcanistDiffWorkflow.php:502-508`).
pub fn parse_revision_edit_response(value: &serde_json::Value) -> Result<i64, String> {
    value
        .get("object")
        .and_then(|object| object.get("id"))
        .and_then(json_i64)
        .ok_or_else(|| {
            "Expected a revision ID to be returned by \"differential.revision.edit\".".to_string()
        })
}

/// URI path for a revision created via `revision.edit`: `/D{id}`
/// (`ArcanistDiffWorkflow.php:512-513`). The caller prefixes the conduit URI.
#[must_use]
pub fn revision_edit_uri_path(revision_id: i64) -> String {
    format!("/D{revision_id}")
}

/// Commit hashes from a `differential.querydiffs` result: the head row's
/// `properties` + `local:commits` + `commit`
/// (`ArcanistDiffWorkflow.php:2167-2178`).
#[must_use]
pub fn local_commit_hashes_from_querydiffs(value: &serde_json::Value) -> Vec<String> {
    first_row(value)
        .and_then(|row| row.get("properties"))
        .and_then(|properties| properties.get("local:commits"))
        .and_then(|local| local.as_array())
        .map(|commits| {
            commits
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("commit")
                        .and_then(|commit| commit.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Commit hashes from a `differential.diff.search` result:
/// `data` head + `attachments` + `commits` + `commits` + `identifier`.
/// Returns `None` when the attachment is missing or null so the caller
/// falls back to `differential.querydiffs`
/// (`ArcanistDiffWorkflow.php:2152-2161`).
#[must_use]
pub fn local_commit_hashes_from_diff_search(value: &serde_json::Value) -> Option<Vec<String>> {
    let diff = value.get("data").and_then(first_row)?;
    let commits = diff.get("attachments")?.get("commits")?.get("commits")?;
    if commits.is_null() {
        return None;
    }
    let commits = commits.as_array()?;
    Some(
        commits
            .iter()
            .filter_map(|entry| {
                entry
                    .get("identifier")
                    .and_then(|id| id.as_str())
                    .map(str::to_string)
            })
            .collect(),
    )
}

/// Parse a `differential.parsecommitmessage` result, failing on parser
/// `errors` (`ArcanistDifferentialCommitMessage.php:59-67`).
pub fn parse_commit_message_parse_response(
    value: &serde_json::Value,
) -> Result<ParsedMessage, String> {
    if let Some(errors) = value.get("errors").and_then(|errors| errors.as_array())
        && !errors.is_empty()
    {
        let joined = errors
            .iter()
            .map(|error| {
                error
                    .as_str()
                    .map_or_else(|| error.to_string(), str::to_string)
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(joined);
    }
    Ok(ParsedMessage {
        fields: value
            .get("fields")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        transactions: value.get("transactions").cloned(),
    })
}

/// Reviewers from a `user.query` result whose `currentStatus` is `away`,
/// named `userName (realName)` when a real name is set
/// (`ArcanistDiffWorkflow.php:1654-1674`).
#[must_use]
pub fn away_reviewers(users: &serde_json::Value) -> Vec<AwayReviewer> {
    let list: Vec<&serde_json::Value> = match users {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => map.values().collect(),
        _ => Vec::new(),
    };
    let mut away = Vec::new();
    for user in list {
        if user.get("currentStatus").and_then(|s| s.as_str()) != Some("away") {
            continue;
        }
        let username = user.get("userName").and_then(|v| v.as_str()).unwrap_or("");
        let real_name = user.get("realName").and_then(|v| v.as_str()).unwrap_or("");
        let name = if real_name.is_empty() {
            username.to_string()
        } else {
            format!("{username} ({real_name})")
        };
        away.push(AwayReviewer {
            name,
            until: user
                .get("currentStatusUntil")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        });
    }
    away
}

/// Fail when there are no changes to submit
/// (`ArcanistDiffWorkflow.php:370-374`).
pub fn require_changes(change_count: usize) -> Result<(), String> {
    if change_count == 0 {
        return Err("There are no changes to generate a diff from!".to_string());
    }
    Ok(())
}

/// Check `--draft` when updating `revision_monogram` (e.g. `D123`).
/// A draft revision proceeds with the `ALREADY A DRAFT` warning; a
/// published revision aborts (`ArcanistDiffWorkflow.php:438-461`).
#[must_use]
pub fn check_draft_update(
    is_draft: bool,
    revision_monogram: &str,
    revision_is_draft: bool,
) -> DraftUpdate {
    if !is_draft {
        return DraftUpdate::Proceed;
    }
    if revision_is_draft {
        DraftUpdate::WarnAlreadyDraft(format!(
            "You are updating a revision (\"{revision_monogram}\") with the \"--draft\" flag, but this revision is already a draft. You only need to provide the \"--draft\" flag when creating a revision. Draft revisions are not published until you explicitly request review from the web UI."
        ))
    } else {
        DraftUpdate::AbortPublished(format!(
            "You are updating a revision (\"{revision_monogram}\") with the \"--draft\" flag, but this revision has already been published for review. You can not turn a revision back into a draft once it has been published."
        ))
    }
}

/// Check `--draft` when creating a revision: without server transactions
/// the server is too old for drafts
/// (`ArcanistDiffWorkflow.php:515-521`).
pub fn check_draft_create(is_draft: bool, has_transactions: bool) -> Result<(), String> {
    if is_draft && !has_transactions {
        return Err("You have specified \"--draft\", but the software version on the server is too old to support draft revisions. Omit the flag or upgrade the server software.".to_string());
    }
    Ok(())
}

/// True when updating needs the ownership prompt: `authorPHID` differs from
/// the viewer PHID (`ArcanistDiffWorkflow.php:2446-2449`).
#[must_use]
pub fn needs_ownership_confirm(author_phid: &str, viewer_phid: &str) -> bool {
    author_phid != viewer_phid
}

/// Ownership prompt for updating someone else's revision
/// (`ArcanistDiffWorkflow.php:2454-2460`).
#[must_use]
pub fn ownership_prompt(revision_id: i64, title: &str) -> String {
    format!(
        "You don't own revision D{revision_id}: \"{title}\". Normally, you should only update revisions you own. You can \"Commandeer\" this revision from the web interface if you want to become the owner.\n\nUpdate this revision anyway?"
    )
}

/// `There is no revision D{id}.` for an unknown update target
/// (`ArcanistDiffWorkflow.php:1647-1651`).
#[must_use]
pub fn missing_revision_message(revision_id: i64) -> String {
    format!("There is no revision D{revision_id}.")
}

/// `Revision '<id>' does not exist!` for `--update` of an unknown revision
/// (`ArcanistDiffWorkflow.php:1588-1592`).
#[must_use]
pub fn unknown_revision_message(revision_id: &str) -> String {
    format!("Revision '{revision_id}' does not exist!")
}

/// Call `differential.creatediff` over the seam and parse the result.
pub fn create_diff(
    call: &ConduitCall<'_>,
    spec: &serde_json::Value,
) -> Result<CreatedDiff, String> {
    let result = call(METHOD_CREATE_DIFF, spec)?;
    parse_creatediff_response(&result)
}

/// Call `differential.updaterevision` over the seam and parse the result.
pub fn update_revision(
    call: &ConduitCall<'_>,
    params: &serde_json::Value,
) -> Result<RevisionResult, String> {
    let result = call(METHOD_UPDATE_REVISION, params)?;
    parse_revision_response(&result)
}

/// Call legacy `differential.createrevision` over the seam and parse the
/// result.
pub fn create_revision_legacy(
    call: &ConduitCall<'_>,
    params: &serde_json::Value,
) -> Result<RevisionResult, String> {
    let result = call(METHOD_CREATE_REVISION, params)?;
    parse_revision_response(&result)
}

/// Call `differential.revision.edit` over the seam and return `object.id`.
pub fn create_revision_via_edit(
    call: &ConduitCall<'_>,
    params: &serde_json::Value,
) -> Result<i64, String> {
    let result = call(METHOD_REVISION_EDIT, params)?;
    parse_revision_edit_response(&result)
}

/// Call `differential.getcommitmessage` over the seam; the result is the raw
/// message corpus string.
pub fn fetch_commit_message(
    call: &ConduitCall<'_>,
    params: &serde_json::Value,
) -> Result<String, String> {
    let result = call(METHOD_GET_COMMIT_MESSAGE, params)?;
    result
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "differential.getcommitmessage response is not a string".to_string())
}

/// Write diff properties over the seam, one `differential.setdiffproperty`
/// call each. Divergence: PHP issues these as parallel futures
/// (`ArcanistDiffWorkflow.php:2403-2424`); the seam runs them in order.
pub fn set_diff_properties(
    call: &ConduitCall<'_>,
    diff_id: i64,
    properties: &[DiffProperty],
) -> Result<(), String> {
    for property in properties {
        let params = set_diff_property_params(diff_id, &property.name, &property.data);
        call(METHOD_SET_DIFF_PROPERTY, &params)?;
    }
    Ok(())
}

/// Look one revision up with `differential.query`; `None` when the result
/// has no head row (`ArcanistDiffWorkflow.php:1580-1585`).
pub fn query_revision_row(
    call: &ConduitCall<'_>,
    id: i64,
) -> Result<Option<serde_json::Value>, String> {
    let result = call(METHOD_QUERY, &differential_query_params(&[id]))?;
    Ok(first_row(&result).cloned())
}

/// Active-diff local commit hashes via search with a `querydiffs`
/// fallback: prefer `differential.revision.search` plus
/// `differential.diff.search`, falling back to `differential.querydiffs` on
/// any failure or missing attachment, like
/// `loadActiveDiffLocalCommitHashes`
/// (`ArcanistDiffWorkflow.php:2110-2179`).
pub fn active_diff_hashes(call: &ConduitCall<'_>, revision_id: i64) -> Result<Vec<String>, String> {
    if let Ok(Some(hashes)) = active_diff_hashes_via_search(call, revision_id) {
        return Ok(hashes);
    }
    let current = call(METHOD_QUERY_DIFFS, &query_diffs_params(&[revision_id]))?;
    Ok(local_commit_hashes_from_querydiffs(&current))
}

fn active_diff_hashes_via_search(
    call: &ConduitCall<'_>,
    revision_id: i64,
) -> Result<Option<Vec<String>>, String> {
    let revisions_raw = call(
        METHOD_REVISION_SEARCH,
        &revision_search_params(&[revision_id]),
    )?;
    let data = revisions_raw.get("data");
    let revision = data.and_then(first_row);
    let Some(revision) = revision else {
        return Ok(None);
    };
    let Some(phid) = revision.get("phid").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let diffs_raw = call(METHOD_DIFF_SEARCH, &diff_search_params(&[phid]))?;
    Ok(local_commit_hashes_from_diff_search(&diffs_raw))
}

/// Create a diff, then update `revision_id` with `fields` and `message`
/// (`ArcanistDiffWorkflow.php:382-385,463-465`).
pub fn submit_update_revision(
    call: &ConduitCall<'_>,
    spec: &serde_json::Value,
    revision_id: i64,
    fields: serde_json::Value,
    message: &str,
) -> Result<(CreatedDiff, RevisionResult), String> {
    let created = create_diff(call, spec)?;
    let params = update_revision_params(revision_id, fields, created.diff_id, message);
    let updated = update_revision(call, &params)?;
    Ok((created, updated))
}

/// Create a diff, then create a revision via legacy
/// `differential.createrevision` (`ArcanistDiffWorkflow.php:382-385,525-527`).
pub fn submit_create_revision_legacy(
    call: &ConduitCall<'_>,
    spec: &serde_json::Value,
    fields: serde_json::Value,
) -> Result<(CreatedDiff, RevisionResult), String> {
    let created = create_diff(call, spec)?;
    let params = create_revision_params(fields, created.diff_id);
    let revision = create_revision_legacy(call, &params)?;
    Ok((created, revision))
}

/// Create a diff, then create a revision via `differential.revision.edit`,
/// appending the `update` (and optional `draft`) transactions
/// (`ArcanistDiffWorkflow.php:382-385,482-500`). Returns the created diff
/// and the new revision id; see `revision_edit_uri_path` for its URI.
pub fn submit_create_revision_via_edit(
    call: &ConduitCall<'_>,
    spec: &serde_json::Value,
    base_transactions: &[serde_json::Value],
    is_draft: bool,
) -> Result<(CreatedDiff, i64), String> {
    let created = create_diff(call, spec)?;
    let params = revision_edit_params(base_transactions, created.phid.as_deref(), is_draft);
    let revision_id = create_revision_via_edit(call, &params)?;
    Ok((created, revision_id))
}

/* -(  Revision Flow Texts (G1 full-state)  )------------------------------------- */

/// Lead line when a diff-only submit succeeds (`:418`).
pub const CREATED_DIFF_LEAD: &str = "Created a new Differential diff:";
/// Label for the diff URI (`:419`).
pub const DIFF_URI_LABEL: &str = "Diff URI:";
/// Lead line when an update succeeds (`:476`).
pub const UPDATED_REVISION_LEAD: &str = "Updated an existing Differential revision:";
/// Lead line when a create succeeds (`:551`).
pub const CREATED_REVISION_LEAD: &str = "Created a new Differential revision:";
/// Label for the revision URI (`:557`).
pub const REVISION_URI_LABEL: &str = "Revision URI:";
/// Lead line for the change summary (`:565`).
pub const INCLUDED_CHANGES_LABEL: &str = "Included changes:";

/// Render matching revisions for the several-revisions error.
///
/// PHP: `ArcanistWorkflow::renderRevisionList()`
/// (`src/workflow/ArcanistWorkflow.php:1665-1671`): five spaces, `- D{id}:`,
/// title, newline.
#[must_use]
pub fn render_revision_list(revisions: &[(i64, &str)]) -> String {
    revisions
        .iter()
        .map(|(id, title)| format!("     - D{id}: {title}\n"))
        .collect()
}

/// Error when several open revisions match the working copy (`:1321-1328`).
#[must_use]
pub fn several_revisions_error(revision_list: &str) -> String {
    format!(
        "There are several revisions which match the working copy:\n\n{revision_list}\nUse \
         '--update' to choose one, or '--create' to create a new revision."
    )
}

/// Error when `--update` is not a revision number (`:1342-1346`).
pub const UPDATE_PARAM_MUST_BE_NUMBER: &str =
    "Parameter to --update must be a Differential Revision number.";

/// Ownership decision for updating `revision_id`: the confirm prompt when
/// the viewer is not the author, else `None` (`:2446-2466`).
#[must_use]
pub fn check_revision_ownership(
    author_phid: &str,
    viewer_phid: &str,
    revision_id: i64,
    title: &str,
) -> Option<String> {
    if needs_ownership_confirm(author_phid, viewer_phid) {
        Some(ownership_prompt(revision_id, title))
    } else {
        None
    }
}

/// Banner header for the already-a-draft warning (`:444`).
pub const ALREADY_A_DRAFT: &str = "ALREADY A DRAFT";

/// Header when every reviewer is away (`:1680-1682`).
#[must_use]
pub fn away_reviewers_all_message(formatted_date: &str) -> String {
    format!("All reviewers are away until {formatted_date}:")
}

/// Header when some reviewers are away (`:1684`).
pub const AWAY_SOME_MESSAGE: &str = "Some reviewers are currently away:";

/// One away-reviewer list item (`:1692-1698`).
#[must_use]
pub fn away_reviewer_list_item(name: &str, formatted_date: &str) -> String {
    format!("{name} (until {formatted_date})")
}

/// Prompt to continue despite away reviewers (`:1704`).
pub const CONTINUE_DESPITE_AWAY: &str = "Continue even though reviewers are unavailable?";
/// Abort message when the user stops over away reviewers (`:1706-1707`).
pub const SPECIFY_AVAILABLE_REVIEWERS: &str = "Specify available reviewers and retry.";

/// Format a unix timestamp like `date('l, M j Y', $ts)` (`:1682,1696`),
/// shifted by `offset_seconds` east of UTC.
///
/// Renders `Thursday, Jan 1 1970` for `(0, 0)`.
///
/// Divergence: PHP renders in the server's default timezone, which this
/// port cannot observe: Rust's standard library has no local-time API,
/// reading `TZ`/system zone files is not portable, and new timezone
/// crates are unavailable (`Cargo.toml`/`Cargo.lock` are shared and
/// read-only). Production therefore passes `0` (UTC) via
/// [`format_phab_date`]; a future timezone source plugs in here. The
/// civil-date math itself is exact for every offset.
#[must_use]
pub fn format_phab_date_with_offset(timestamp: i64, offset_seconds: i64) -> String {
    const WEEKDAYS: [&str; 7] = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = timestamp.saturating_add(offset_seconds).div_euclid(86_400);
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!("{weekday}, {} {day} {year}", MONTHS[(month - 1) as usize])
}

/// Format a unix timestamp like `date('l, M j Y', $ts)` in UTC.
///
/// Production entry point (see [`format_phab_date_with_offset`] for the
/// server-timezone divergence): renders `Thursday, Jan 1 1970` for `0`.
#[must_use]
pub fn format_phab_date(timestamp: i64) -> String {
    format_phab_date_with_offset(timestamp, 0)
}

/// Numeric `currentStatusUntil`, defaulting to `0` when absent.
#[must_use]
pub fn away_until_timestamp(until: &serde_json::Value) -> i64 {
    json_i64(until).unwrap_or(0)
}

/// Earliest `currentStatusUntil` across away reviewers (`:1678`).
#[must_use]
pub fn away_earliest_return(aways: &[AwayReviewer]) -> Option<i64> {
    aways
        .iter()
        .map(|away| away_until_timestamp(&away.until))
        .min()
}

/// Full away-reviewers notice block, or `None` when nobody is away
/// (`:1676-1702`), with dates shifted by `offset_seconds` east of UTC.
///
/// The header is followed by a blank line, then one 6-space-indented item
/// per reviewer (`PhutilConsoleList`, `src/console/view/PhutilConsoleList.php:39-61`).
#[must_use]
pub fn away_reviewers_block_with_offset(
    aways: &[AwayReviewer],
    total_reviewers: usize,
    offset_seconds: i64,
) -> Option<String> {
    if aways.is_empty() {
        return None;
    }
    let header = if aways.len() == total_reviewers {
        let earliest = away_earliest_return(aways).unwrap_or(0);
        away_reviewers_all_message(&format_phab_date_with_offset(earliest, offset_seconds))
    } else {
        AWAY_SOME_MESSAGE.to_string()
    };
    let mut block = format!("{header}\n\n");
    for away in aways {
        let item = away_reviewer_list_item(
            &away.name,
            &format_phab_date_with_offset(away_until_timestamp(&away.until), offset_seconds),
        );
        block.push_str(&format!("      {item}\n"));
    }
    Some(block)
}

/// Full away-reviewers notice block in UTC (see
/// [`format_phab_date_with_offset`] for the server-timezone divergence).
#[must_use]
pub fn away_reviewers_block(aways: &[AwayReviewer], total_reviewers: usize) -> Option<String> {
    away_reviewers_block_with_offset(aways, total_reviewers, 0)
}

/* -(  Autotargets and Staging Property (G1 full-state)  )---------------------- */

/// `harbormaster.queryautotargets` (`:2782`).
pub const METHOD_QUERY_AUTOTARGETS: &str = "harbormaster.queryautotargets";
/// `harbormaster.sendmessage` (`:2818,2836`).
pub const METHOD_SENDMESSAGE: &str = "harbormaster.sendmessage";
/// Autotarget key for lint (`:2778`).
pub const AUTOTARGET_LINT_KEY: &str = "arcanist.lint";
/// Autotarget key for unit (`:2779`).
pub const AUTOTARGET_UNIT_KEY: &str = "arcanist.unit";

/// Params for `harbormaster.queryautotargets` (`:2782-2790`).
#[must_use]
pub fn query_autotargets_params(diff_phid: &str) -> serde_json::Value {
    serde_json::json!({
        "objectPHID": diff_phid,
        "targetKeys": [AUTOTARGET_LINT_KEY, AUTOTARGET_UNIT_KEY],
    })
}

/// Target PHID for `key` from a `queryautotargets` response
/// (`:2791,2798,2826`).
#[must_use]
pub fn autotarget_phid(response: &serde_json::Value, key: &str) -> Option<String> {
    response
        .get("targetMap")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Lint autotarget type: `fail` when any severity is warning/error, else
/// `pass` (`:2807-2815`).
#[must_use]
pub fn lint_autotarget_type(severities: &[&str]) -> &'static str {
    if severities
        .iter()
        .any(|severity| *severity == "warning" || *severity == "error")
    {
        "fail"
    } else {
        "pass"
    }
}

/// Params for a lint `harbormaster.sendmessage` (`:2817-2823`).
#[must_use]
pub fn sendmessage_lint_params(
    target_phid: &str,
    lint: &[serde_json::Value],
    autotarget_type: &str,
) -> serde_json::Value {
    serde_json::json!({
        "buildTargetPHID": target_phid,
        "lint": lint,
        "type": autotarget_type,
    })
}

/// Params for a unit `harbormaster.sendmessage` (`:2835-2841`).
#[must_use]
pub fn sendmessage_unit_params(
    target_phid: &str,
    unit: &[serde_json::Value],
    autotarget_type: &str,
) -> serde_json::Value {
    serde_json::json!({
        "buildTargetPHID": target_phid,
        "unit": unit,
        "type": autotarget_type,
    })
}

/// Push lint/unit results to Harbormaster autotargets over the seam.
///
/// PHP: `updateAutotargets()` (`:2777-2855`). Lint and unit messages are
/// caller-normalized (`getModernLintDictionary()` / `getModernUnitDictionary()`
/// are identity here: this port stores dictionaries already). Returns false
/// when the query fails or any send fails, like PHP's exception swallowing
/// (`:2792-2794`, `:2844-2853`).
///
/// Divergence: PHP resolves the sends as parallel futures; the seam runs
/// them in order.
pub fn update_autotargets(
    call: &ConduitCall<'_>,
    diff_phid: &str,
    lint_messages: &[serde_json::Value],
    unit_results: &[serde_json::Value],
    unit_result: i64,
) -> bool {
    let response = match call(
        METHOD_QUERY_AUTOTARGETS,
        &query_autotargets_params(diff_phid),
    ) {
        Ok(response) => response,
        Err(_) => return false,
    };
    let mut sends = Vec::new();
    if let Some(target) = autotarget_phid(&response, AUTOTARGET_LINT_KEY) {
        let severities: Vec<&str> = lint_messages
            .iter()
            .filter_map(|message| message.get("severity").and_then(|value| value.as_str()))
            .collect();
        sends.push(sendmessage_lint_params(
            &target,
            lint_messages,
            lint_autotarget_type(&severities),
        ));
    }
    if let Some(target) = autotarget_phid(&response, AUTOTARGET_UNIT_KEY) {
        let unit_type = crate::diff_local::harbormaster_type_from_result(
            i32::try_from(unit_result).unwrap_or(crate::diff_local::UNIT_RESULT_FAIL),
        );
        sends.push(sendmessage_unit_params(&target, unit_results, unit_type));
    }
    for params in &sends {
        if call(METHOD_SENDMESSAGE, params).is_err() {
            return false;
        }
    }
    true
}

/// Escape `/` as `\/` in encoded JSON, like PHP `json_encode()`.
///
/// `serde_json` never escapes forward slashes; PHP escapes them by
/// default. Every diff JSON emit site (submit `--json`, diff property
/// payloads, scratch files) must pass through this so bytes match PHP.
#[must_use]
pub fn php_escape_json_slashes(text: &str) -> String {
    text.replace('/', "\\/")
}

/// Encode a value exactly like PHP `json_encode()` (compact form).
///
/// This is `serde_json::to_string` plus [`php_escape_json_slashes`].
/// Remaining `serde_json`-vs-`json_encode` divergences, audited 2026-10:
/// non-ASCII bytes stay raw UTF-8 here but PHP emits `\uXXXX` (with
/// surrogate pairs); object key order is serde's sorted order here
/// while PHP preserves insertion order (callers with ordered objects,
/// like [`staging_property_data`], hand-render keys); float formatting
/// edge cases may differ. Empty PHP arrays encode as `[]`: callers must
/// build `Value::Array` (not `Value::Object`) where PHP has a list.
pub fn php_json_encode(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    serde_json::to_string(value).map(|text| php_escape_json_slashes(&text))
}

/// Encode a value like PHP `json_encode()` with pretty printing.
///
/// Same slash escaping as [`php_json_encode`]; used for human-facing
/// `--json` inspection output (an arc-rs extension: PHP has no
/// inspect `--json`, but its bytes stay slash-consistent regardless).
pub fn php_json_encode_pretty(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(value).map(|text| php_escape_json_slashes(&text))
}

/// `arc.staging` diff property payload (`:2598-2620`).
///
/// Keys render `status` first like `phutil_json_encode()`; `refs` is the
/// pushed-ref list (empty on skip).
#[must_use]
pub fn staging_property_data(status: &str, refs: &[serde_json::Value]) -> String {
    let status = php_escape_json_slashes(
        &serde_json::to_string(status).unwrap_or_else(|_| "\"\"".to_string()),
    );
    let refs =
        php_escape_json_slashes(&serde_json::to_string(refs).unwrap_or_else(|_| "[]".to_string()));
    format!("{{\"status\":{status},\"refs\":{refs}}}")
}

#[cfg(test)]
#[path = "diff_submit_tests.rs"]
mod diff_submit_tests;
