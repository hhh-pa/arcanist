//! Pre-land revision and build gates as pure functions.
//!
//! Port of the land-gate checks in `src/land/engine/ArcanistLandEngine.php`
//! (`confirmRevisions`, lines 278-534, and `confirmBuilds`, lines 536-718)
//! plus the large-working-set limit (`confirmCommits`, lines 1015-1069, and
//! `ArcanistLandWorkflow::getLargeWorkingSetLimit`, lines 291-293). Each
//! gate reads caller-supplied revision/build JSON shaped like modern
//! `differential.revision.search` results (see `ArcanistRevisionRef.php`)
//! and reports a finding with the verbatim prompt key and query PHP would
//! ask. Callers decide how to prompt; this module never prompts, fetches,
//! or touches the working copy.
//!
//! Status values, build statuses, plan behaviors, prompt keys, and query
//! strings are verbatim from PHP; each item cites its PHP lines.
//!
//! Divergences (behavior limits with PHP refs):
//! - The open-parents title drops a stray trailing space in PHP's
//!   `'%s OPEN PARENT REVISION(S) '` (`ArcanistLandEngine.php:486`).
//! - `classify_builds` preserves input order in `problems`; PHP sorts the
//!   printed list passed-builds-first (`ArcanistLandEngine.php:576`), which
//!   is display order only and does not change the verdict.
//! - The caller joins builds to revisions (`revision_phid`): PHP walks
//!   buildable `objectPHID` -> revision `diffPHID`
//!   (`ArcanistLandEngine.php:623-644`), which needs conduit data this
//!   module does not fetch.
//! - Legacy `differential.query` rows with an unknown numeric status map to
//!   an empty status value (PHP `idx()` yields null,
//!   `ArcanistRevisionRef.php:70`); such revisions land in `not_accepted`.
//! - The `arc.land.implicit` gate (commits without revisions,
//!   `ArcanistLandEngine.php:720-756`) needs VCS history and is not ported.
//! - Reviewer approval has no separate gate: the land engine reads reviewer
//!   acceptance only through the revision status (`isStatusAccepted`,
//!   `ArcanistRevisionRef.php:132-135`).

/// Modern status `needs-review` (`ArcanistRevisionRef.php:53`).
pub const STATUS_NEEDS_REVIEW: &str = "needs-review";
/// Modern status `needs-revision` (`ArcanistRevisionRef.php:54`).
pub const STATUS_NEEDS_REVISION: &str = "needs-revision";
/// Modern status `accepted` (`ArcanistRevisionRef.php:55`).
pub const STATUS_ACCEPTED: &str = "accepted";
/// Modern status `published` (`ArcanistRevisionRef.php:56`).
pub const STATUS_PUBLISHED: &str = "published";
/// Modern status `abandoned` (`ArcanistRevisionRef.php:57`).
pub const STATUS_ABANDONED: &str = "abandoned";
/// Modern status `changes-planned` (`ArcanistRevisionRef.php:58`).
pub const STATUS_CHANGES_PLANNED: &str = "changes-planned";

/// Prompt for landing revisions owned by someone else
/// (`ArcanistLandEngine.php:330`).
pub const PROMPT_UNAUTHORED: &str = "arc.land.unauthored";
/// Prompt for landing revisions with changes planned
/// (`ArcanistLandEngine.php:384`).
pub const PROMPT_CHANGES_PLANNED: &str = "arc.land.changes-planned";
/// Prompt for landing already-published revisions
/// (`ArcanistLandEngine.php:413`).
pub const PROMPT_PUBLISHED: &str = "arc.land.published";
/// Prompt for landing revisions that are not accepted
/// (`ArcanistLandEngine.php:445`).
pub const PROMPT_NOT_ACCEPTED: &str = "arc.land.not-accepted";
/// Prompt for landing with open parent revisions
/// (`ArcanistLandEngine.php:519`).
pub const PROMPT_OPEN_PARENTS: &str = "arc.land.open-parents";
/// Prompt for landing despite failed builds
/// (`ArcanistLandEngine.php:673`).
pub const PROMPT_FAILED_BUILDS: &str = "arc.land.failed-builds";
/// Prompt for landing despite ongoing builds
/// (`ArcanistLandEngine.php:686`).
pub const PROMPT_ONGOING_BUILDS: &str = "arc.land.ongoing-builds";
/// Prompt for landing a large working set
/// (`ArcanistLandEngine.php:1067`).
pub const PROMPT_LARGE_WORKING_SET: &str = "arc.land.large-working-set";

/// Commit count above which landing warns
/// (`ArcanistLandWorkflow.php:291-293`).
pub const LARGE_WORKING_SET_LIMIT: usize = 50;

/// Complete build statuses: `passed`, `failed`, `aborted`, `error`,
/// `deadlocked` (`ArcanistBuildRef.php:70-81`).
pub const COMPLETE_BUILD_STATUSES: &[&str] =
    &["passed", "failed", "aborted", "error", "deadlocked"];
/// Passing build status (`ArcanistBuildRef.php:83-85`).
pub const BUILD_PASSED: &str = "passed";

/// Build plan behavior key read at land time
/// (`ArcanistLandEngine.php:583`).
pub const PLAN_BEHAVIOR_KEY: &str = "arc-land";
/// Default plan behavior (`ArcanistLandEngine.php:583`).
pub const PLAN_ALWAYS: &str = "always";
/// Plan behavior warning only while building
/// (`ArcanistLandEngine.php:584`).
pub const PLAN_BUILDING: &str = "building";
/// Plan behavior warning only once complete
/// (`ArcanistLandEngine.php:585`).
pub const PLAN_COMPLETE: &str = "complete";
/// Plan behavior never warning (`ArcanistLandEngine.php:586`).
pub const PLAN_NEVER: &str = "never";

/// Revision fields the land gates need, after
/// `ArcanistRevisionRef::newFromConduit` / `newFromConduitQuery`
/// (`ArcanistRevisionRef.php:30-92`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionView {
    /// `id`.
    pub id: i64,
    /// `D{id}` monogram (`ArcanistRevisionRef.php:94-96`).
    pub monogram: String,
    /// `fields.title`.
    pub name: String,
    /// `fields.status.value` (modern) or the legacy mapped value.
    pub status: String,
    /// `fields.status.name` (modern) or `statusName` (legacy).
    pub status_name: String,
    /// `fields.status.closed` (modern), or legacy `Abandoned`/`Closed`.
    pub closed: bool,
    /// `fields.authorPHID`.
    pub author_phid: String,
    /// `phid`.
    pub phid: String,
    /// `fields.diffPHID`, absent on legacy rows.
    pub diff_phid: Option<String>,
}

impl RevisionView {
    /// `isStatusChangesPlanned` (`ArcanistRevisionRef.php:117-120`).
    #[must_use]
    pub fn is_status_changes_planned(&self) -> bool {
        self.status == STATUS_CHANGES_PLANNED
    }

    /// `isStatusPublished` (`ArcanistRevisionRef.php:127-130`).
    #[must_use]
    pub fn is_status_published(&self) -> bool {
        self.status == STATUS_PUBLISHED
    }

    /// `isStatusAccepted` (`ArcanistRevisionRef.php:132-135`).
    #[must_use]
    pub fn is_status_accepted(&self) -> bool {
        self.status == STATUS_ACCEPTED
    }

    /// `isClosed` (`ArcanistRevisionRef.php:146-148`).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Status buckets from `confirmRevisions`
/// (`ArcanistLandEngine.php:335-346`): changes-planned, else published,
/// else anything not accepted. Accepted revisions fall through silently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusPartition {
    /// Indexes of changes-planned revisions.
    pub planned: Vec<usize>,
    /// Indexes of published revisions.
    pub published: Vec<usize>,
    /// Indexes of revisions that are neither planned, published, nor
    /// accepted.
    pub not_accepted: Vec<usize>,
}

/// One child -> parent dependency edge for the open-parents gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentLink {
    /// Index of the landing child revision.
    pub child_index: usize,
    /// The parent revision.
    pub parent: RevisionView,
}

/// One open parent with the landing children that depend on it
/// (`ArcanistLandEngine.php:472-480`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenParentGroup {
    /// The open parent revision.
    pub parent: RevisionView,
    /// Indexes of landing children depending on it.
    pub children: Vec<usize>,
}

/// Build fields the land gates need: `fields.buildStatus.value`
/// (`ArcanistBuildRef.php:56-58`) plus the owning plan's `arc-land`
/// behavior (`ArcanistBuildPlanRef.php:30-35`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildView {
    /// `fields.buildStatus.value`.
    pub status: String,
    /// The plan's `arc-land` behavior (`always` by default); `None` when
    /// there is no plan ref, in which case the build is skipped
    /// (`ArcanistLandEngine.php:578-581`).
    pub plan_behavior: Option<String>,
    /// Caller-joined key of the revision under land (PHP joins
    /// buildable `objectPHID` -> revision `diffPHID`,
    /// `ArcanistLandEngine.php:623-644`).
    pub revision_phid: String,
    /// Build `id`, for the `Build {id}` ref-view object name
    /// (`ArcanistBuildRef.php:16-18`); 0 when the row has no id (PHP
    /// formats null with `%d`).
    pub build_id: i64,
    /// `fields.name`, the build ref-view title
    /// (`ArcanistBuildRef.php:38-42`).
    pub build_name: String,
    /// Owning buildable `B{id}` monogram
    /// (`ArcanistBuildableRef.php:41-43`); `B` when the row has no id.
    pub buildable_monogram: String,
    /// Owning buildable URI, absolutized like
    /// `ArcanistWorkflow::getAbsoluteURI()` (`ArcanistWorkflow.php:2423`).
    pub buildable_uri: String,
}

/// Outcome of `confirmBuilds` filtering
/// (`ArcanistLandEngine.php:572-617`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildVerdict {
    /// Count of complete, non-passing builds.
    pub failures: usize,
    /// Count of incomplete, non-passing builds.
    pub ongoing: usize,
    /// Indexes of problem builds, in input order.
    pub problems: Vec<usize>,
    /// Distinct `revision_phid` values among problem builds (PHP counts
    /// distinct revisions in `$build_map`).
    pub affected_revisions: usize,
}

/// One land gate that fired: the verbatim prompt key, banner title, and
/// query PHP would ask, plus the counted items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateFinding {
    /// Verbatim `arc.land.*` prompt key.
    pub prompt: &'static str,
    /// Banner title (e.g. `NOT REVISION AUTHOR`).
    pub title: String,
    /// Prompt query with counts filled in.
    pub query: String,
    /// What the query counted (revisions, parents, or commits).
    pub count: usize,
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_u64()
            .and_then(|n| i64::try_from(n).ok())
            .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

fn get_str<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

/// Legacy numeric status -> modern value
/// (`ArcanistRevisionRef.php:52-59`).
fn legacy_status_value(code: i64) -> Option<&'static str> {
    match code {
        0 => Some(STATUS_NEEDS_REVIEW),
        1 => Some(STATUS_NEEDS_REVISION),
        2 => Some(STATUS_ACCEPTED),
        3 => Some(STATUS_PUBLISHED),
        4 => Some(STATUS_ABANDONED),
        5 => Some(STATUS_CHANGES_PLANNED),
        _ => None,
    }
}

/// Parse a revision row in modern `differential.revision.search` shape
/// (`fields.status.value` / `name` / `closed`, `fields.title`,
/// `fields.authorPHID`, `fields.diffPHID`, `id`, `phid`), falling back to
/// the legacy `differential.query` row mapping in `newFromConduitQuery`
/// (`ArcanistRevisionRef.php:36-92`).
pub fn parse_revision(value: &serde_json::Value) -> Result<RevisionView, String> {
    let id = value
        .get("id")
        .and_then(json_i64)
        .ok_or_else(|| "revision entry has no id".to_string())?;
    let monogram = format!("D{id}");
    let status = value
        .get("fields")
        .and_then(|fields| fields.get("status"))
        .and_then(|status| status.get("value"))
        .and_then(|v| v.as_str());
    if let Some(status) = status {
        Ok(parse_modern_revision(value, id, monogram, status))
    } else {
        Ok(parse_legacy_revision(value, id, monogram))
    }
}

/// Parse a modern `differential.revision.search` row
/// (`fields.status.value` / `name` / `closed`, `fields.title`,
/// `fields.authorPHID`, `fields.diffPHID`, `id`, `phid`).
fn parse_modern_revision(
    value: &serde_json::Value,
    id: i64,
    monogram: String,
    status: &str,
) -> RevisionView {
    let fields = value.get("fields");
    let status_obj = fields.and_then(|fields| fields.get("status"));
    let status_name = status_obj
        .and_then(|status| status.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let closed = status_obj
        .and_then(|status| status.get("closed"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let name = fields
        .and_then(|fields| fields.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let author_phid = fields
        .and_then(|fields| fields.get("authorPHID"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    RevisionView {
        id,
        monogram,
        name: name.to_string(),
        status: status.to_string(),
        status_name: status_name.to_string(),
        closed,
        author_phid: author_phid.to_string(),
        phid: get_str(value, "phid").to_string(),
        diff_phid: fields
            .and_then(|fields| fields.get("diffPHID"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    }
}

/// Parse a legacy `differential.query` row via the `newFromConduitQuery`
/// mapping (`ArcanistRevisionRef.php:36-92`).
fn parse_legacy_revision(value: &serde_json::Value, id: i64, monogram: String) -> RevisionView {
    let status = value
        .get("status")
        .and_then(json_i64)
        .and_then(legacy_status_value)
        .unwrap_or("");
    let status_name = get_str(value, "statusName");
    let closed = matches!(status_name, "Abandoned" | "Closed");
    RevisionView {
        id,
        monogram,
        name: get_str(value, "title").to_string(),
        status: status.to_string(),
        status_name: status_name.to_string(),
        closed,
        author_phid: get_str(value, "authorPHID").to_string(),
        phid: get_str(value, "phid").to_string(),
        diff_phid: None,
    }
}

/// Bucket revisions into planned / published / not-accepted, in PHP order
/// (`ArcanistLandEngine.php:338-346`).
#[must_use]
pub fn partition_by_status(revisions: &[RevisionView]) -> StatusPartition {
    let mut partition = StatusPartition::default();
    for (index, revision) in revisions.iter().enumerate() {
        if revision.is_status_changes_planned() {
            partition.planned.push(index);
        } else if revision.is_status_published() {
            partition.published.push(index);
        } else if !revision.is_status_accepted() {
            partition.not_accepted.push(index);
        }
    }
    partition
}

/// Indexes of revisions whose `authorPHID` differs from the viewer PHID
/// (`ArcanistLandEngine.php:285-291`).
#[must_use]
pub fn unauthored_indexes(revisions: &[RevisionView], viewer_phid: &str) -> Vec<usize> {
    revisions
        .iter()
        .enumerate()
        .filter_map(|(index, revision)| {
            if revision.author_phid == viewer_phid {
                None
            } else {
                Some(index)
            }
        })
        .collect()
}

/// Parse one parent revision row into a child -> parent edge.
pub fn parent_link(
    child_index: usize,
    parent_json: &serde_json::Value,
) -> Result<ParentLink, String> {
    Ok(ParentLink {
        child_index,
        parent: parse_revision(parent_json)?,
    })
}

/// Group open parents: skip parents that also land in this operation and
/// parents that are closed, then group children by parent PHID in
/// first-seen order (`ArcanistLandEngine.php:456-481`).
#[must_use]
pub fn open_parent_groups(links: &[ParentLink], landing_phids: &[&str]) -> Vec<OpenParentGroup> {
    use std::collections::hash_map::Entry;
    use std::collections::{HashMap, HashSet};

    let landing: HashSet<&str> = landing_phids.iter().copied().collect();
    let mut groups: HashMap<String, OpenParentGroup> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for link in links {
        if landing.contains(link.parent.phid.as_str()) || link.parent.is_closed() {
            continue;
        }
        match groups.entry(link.parent.phid.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().children.push(link.child_index);
            }
            Entry::Vacant(entry) => {
                order.push(link.parent.phid.clone());
                entry.insert(OpenParentGroup {
                    parent: link.parent.clone(),
                    children: vec![link.child_index],
                });
            }
        }
    }
    order.iter().filter_map(|key| groups.remove(key)).collect()
}

/// Parse one build row with its plan row (`None` when the build has no plan
/// ref). The behavior defaults to `always`, like `getBehavior('arc-land',
/// 'always')` (`ArcanistBuildPlanRef.php:30-35`,
/// `ArcanistLandEngine.php:583`).
#[must_use]
pub fn parse_build(
    build_json: &serde_json::Value,
    plan_json: Option<&serde_json::Value>,
    revision_phid: &str,
) -> BuildView {
    let status = build_json
        .get("fields")
        .and_then(|fields| fields.get("buildStatus"))
        .and_then(|status| status.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let plan_behavior = plan_json.map(|plan| {
        plan.get("fields")
            .and_then(|fields| fields.get("behaviors"))
            .and_then(|behaviors| behaviors.get(PLAN_BEHAVIOR_KEY))
            .and_then(|behavior| behavior.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or(PLAN_ALWAYS)
            .to_string()
    });
    // `parse_build` sees only the build row: the buildable display
    // fields stay empty and the caller fills them from the buildable
    // row (like `fetch_build_views` does).
    BuildView {
        status: status.to_string(),
        plan_behavior,
        revision_phid: revision_phid.to_string(),
        build_id: build_json.get("id").and_then(json_i64).unwrap_or(0),
        build_name: build_json
            .get("fields")
            .and_then(|fields| fields.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        buildable_monogram: String::new(),
        buildable_uri: String::new(),
    }
}

/// `ArcanistBuildRef::isComplete` (`ArcanistBuildRef.php:70-81`).
#[must_use]
pub fn is_build_complete(status: &str) -> bool {
    COMPLETE_BUILD_STATUSES.contains(&status)
}

/// `ArcanistBuildRef::isPassed` (`ArcanistBuildRef.php:83-85`).
#[must_use]
pub fn is_build_passed(status: &str) -> bool {
    status == BUILD_PASSED
}

/// Filter builds to the problem set: skip builds without a plan ref,
/// `never` plans, `complete` plans on incomplete builds, `building` plans
/// on complete builds, and passing builds; the rest are failures when
/// complete, ongoing otherwise (`ArcanistLandEngine.php:576-617`).
#[must_use]
pub fn classify_builds(builds: &[BuildView]) -> BuildVerdict {
    use std::collections::HashSet;

    let mut verdict = BuildVerdict::default();
    let mut seen: HashSet<&str> = HashSet::new();
    for (index, build) in builds.iter().enumerate() {
        let Some(behavior) = build.plan_behavior.as_deref() else {
            continue;
        };
        if behavior == PLAN_NEVER {
            continue;
        }
        let complete = is_build_complete(&build.status);
        if behavior == PLAN_COMPLETE && !complete {
            continue;
        }
        if behavior == PLAN_BUILDING && complete {
            continue;
        }
        if is_build_passed(&build.status) {
            continue;
        }
        if complete {
            verdict.failures += 1;
        } else {
            verdict.ongoing += 1;
        }
        verdict.problems.push(index);
        if seen.insert(build.revision_phid.as_str()) {
            verdict.affected_revisions += 1;
        }
    }
    verdict
}

/// Prompt key for a build verdict: failures win over ongoing; `None` when
/// there are no problem builds and PHP returns early
/// (`ArcanistLandEngine.php:619-621,673,686`).
#[must_use]
pub fn build_prompt_key(has_failures: bool, has_ongoing: bool) -> Option<&'static str> {
    if has_failures {
        Some(PROMPT_FAILED_BUILDS)
    } else if has_ongoing {
        Some(PROMPT_ONGOING_BUILDS)
    } else {
        None
    }
}

/// `NOT REVISION AUTHOR` gate (`ArcanistLandEngine.php:293-333`).
#[must_use]
pub fn unauthored_gate(revisions: &[RevisionView], viewer_phid: &str) -> Option<GateFinding> {
    let count = unauthored_indexes(revisions, viewer_phid).len();
    if count == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_UNAUTHORED,
        title: "NOT REVISION AUTHOR".to_string(),
        query: "Land revisions you are not the author of?".to_string(),
        count,
    })
}

/// Changes-planned gate (`ArcanistLandEngine.php:352-387`).
#[must_use]
pub fn planned_gate(partition: &StatusPartition) -> Option<GateFinding> {
    let count = partition.planned.len();
    if count == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_CHANGES_PLANNED,
        title: format!("{count} REVISION(S) HAVE CHANGES PLANNED"),
        query: format!("Land {count} revision(s) with changes planned?"),
        count,
    })
}

/// Already-published gate (`ArcanistLandEngine.php:392-416`).
#[must_use]
pub fn published_gate(partition: &StatusPartition) -> Option<GateFinding> {
    let count = partition.published.len();
    if count == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_PUBLISHED,
        title: format!("{count} REVISION(S) ARE ALREADY PUBLISHED"),
        query: format!("Land {count} revision(s) that are already published?"),
        count,
    })
}

/// Not-accepted gate (`ArcanistLandEngine.php:418-448`).
#[must_use]
pub fn not_accepted_gate(partition: &StatusPartition) -> Option<GateFinding> {
    let count = partition.not_accepted.len();
    if count == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_NOT_ACCEPTED,
        title: format!("{count} REVISION(S) ARE NOT ACCEPTED"),
        query: format!("Land {count} revision(s) in the wrong state?"),
        count,
    })
}

/// Open-parents gate (`ArcanistLandEngine.php:483-522`).
#[must_use]
pub fn open_parents_gate(groups: &[OpenParentGroup]) -> Option<GateFinding> {
    let count = groups.len();
    if count == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_OPEN_PARENTS,
        title: format!("{count} OPEN PARENT REVISION(S)"),
        query: format!("Land changes that depend on {count} open revision(s)?"),
        count,
    })
}

/// Failed/ongoing-builds gate over distinct affected revisions
/// (`ArcanistLandEngine.php:646-687`).
#[must_use]
pub fn builds_gate(verdict: &BuildVerdict) -> Option<GateFinding> {
    let prompt = build_prompt_key(verdict.failures > 0, verdict.ongoing > 0)?;
    let count = verdict.affected_revisions;
    let (title, query) = if verdict.failures > 0 && verdict.ongoing > 0 {
        (
            "BUILD FAILURES",
            format!("Land {count} revision(s) anyway, despite ongoing and failed builds?"),
        )
    } else if verdict.failures > 0 {
        (
            "BUILD FAILURES",
            format!("Land {count} revision(s) anyway, despite failed builds?"),
        )
    } else {
        (
            "ONGOING BUILDS",
            format!("Land {count} revision(s) anyway, despite ongoing builds?"),
        )
    };
    Some(GateFinding {
        prompt,
        title: title.to_string(),
        query,
        count,
    })
}

/// True when the commit count exceeds the warning limit
/// (`ArcanistLandEngine.php:1015-1017`).
#[must_use]
pub fn large_working_set_exceeded(commit_count: usize, limit: usize) -> bool {
    commit_count > limit
}

/// `LARGE WORKING SET` gate (`ArcanistLandEngine.php:1035-1069`).
#[must_use]
pub fn large_working_set_gate(commit_count: usize, limit: usize) -> Option<GateFinding> {
    if !large_working_set_exceeded(commit_count, limit) {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_LARGE_WORKING_SET,
        title: "LARGE WORKING SET".to_string(),
        query: format!("Land {commit_count} commit(s)?"),
        count: commit_count,
    })
}

/// Run every gate in PHP order: unauthored, planned, published,
/// not-accepted, open-parents, builds, then the large-working-set gate at
/// `LARGE_WORKING_SET_LIMIT` when a commit count is given.
#[must_use]
pub fn evaluate_gates(
    revisions: &[RevisionView],
    viewer_phid: &str,
    parent_links: &[ParentLink],
    landing_phids: &[&str],
    builds: &[BuildView],
    commit_count: Option<usize>,
) -> Vec<GateFinding> {
    let mut findings = Vec::new();
    let partition = partition_by_status(revisions);
    findings.extend(unauthored_gate(revisions, viewer_phid));
    findings.extend(planned_gate(&partition));
    findings.extend(published_gate(&partition));
    findings.extend(not_accepted_gate(&partition));
    findings.extend(open_parents_gate(&open_parent_groups(
        parent_links,
        landing_phids,
    )));
    findings.extend(builds_gate(&classify_builds(builds)));
    if let Some(count) = commit_count
        && let Some(finding) = large_working_set_gate(count, LARGE_WORKING_SET_LIMIT)
    {
        findings.push(finding);
    }
    findings
}

#[cfg(test)]
#[path = "land_checks_tests.rs"]
mod land_checks_tests;

/// Prompt for the final land confirmation (`ArcanistLandEngine.php:1216`).
pub const PROMPT_CONFIRM: &str = "arc.land.confirm";
/// Prompt for landing with implicitly-associated commits
/// (`ArcanistLandEngine.php:753`).
pub const PROMPT_IMPLICIT: &str = "arc.land.implicit";
/// Prompt for creating new branches or bookmarks in the remote
/// (`ArcanistGitLandEngine.php:1029`,
/// `ArcanistMercurialLandEngine.php:377`).
pub const PROMPT_CREATE: &str = "arc.land.create";
/// Prompt for stashing uncommitted changes before landing
/// (`ArcanistWorkflow.php:2350-2351`, asked by
/// `ArcanistRepositoryLocalState::saveLocalState()`, `:209-230`).
pub const PROMPT_STASH: &str = "arc.state.stash";

/// Description of every `arc land` prompt, verbatim from `newPrompts()`
/// (`ArcanistLandWorkflow.php:237-289`; `arc.state.stash` from
/// `ArcanistWorkflow.php:2350-2351`).
///
/// Shown by the `?` prompt help (`ArcanistPrompt.php:191-231`). Returns
/// `None` for unknown keys.
#[must_use]
pub fn prompt_description(key: &str) -> Option<&'static str> {
    match key {
        PROMPT_LARGE_WORKING_SET => {
            Some("Confirms landing more than 50 commit(s) in a single operation.")
        }
        PROMPT_CONFIRM => Some("Confirms that the correct changes have been selected to land."),
        PROMPT_IMPLICIT => Some(
            "Confirms that local commits which are not associated with a revision have been \
             associated correctly and should land.",
        ),
        PROMPT_UNAUTHORED => Some("Confirms that revisions you did not author should land."),
        PROMPT_CHANGES_PLANNED => Some("Confirms that revisions with changes planned should land."),
        PROMPT_PUBLISHED => Some("Confirms that revisions that are already published should land."),
        PROMPT_NOT_ACCEPTED => Some("Confirms that revisions that are not accepted should land."),
        PROMPT_OPEN_PARENTS => {
            Some("Confirms that revisions with open parent revisions should land.")
        }
        PROMPT_FAILED_BUILDS => Some("Confirms that revisions with failed builds should land."),
        PROMPT_ONGOING_BUILDS => Some("Confirms that revisions with ongoing builds should land."),
        PROMPT_CREATE => {
            Some("Confirms that new branches or bookmarks should be created in the remote.")
        }
        PROMPT_STASH => Some(
            "Prompts the user to stash changes and continue when the working copy has untracked, \
             uncommitted, or unstaged changes.",
        ),
        _ => None,
    }
}

/// `IMPLICIT COMMITS` gate (`ArcanistLandEngine.php:720-756`).
///
/// Fires when `implicit_commits` commits land without an explicit
/// revision association. The PHP query takes no count; `count` carries
/// the implicit commit count for list rendering.
#[must_use]
pub fn implicit_gate(implicit_commits: usize) -> Option<GateFinding> {
    if implicit_commits == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_IMPLICIT,
        title: "IMPLICIT COMMITS".to_string(),
        query: "Continue with this mapping between commits and revisions?".to_string(),
        count: implicit_commits,
    })
}

/// `CREATE N BRANCHE(S)` gate, git flavour
/// (`ArcanistGitLandEngine.php:998-1032`).
///
/// The `branche(s)` spelling is verbatim from PHP.
#[must_use]
pub fn create_gate_git(new_refs: usize) -> Option<GateFinding> {
    if new_refs == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_CREATE,
        title: format!("CREATE {new_refs} BRANCHE(S)"),
        query: format!("Create {new_refs} new branche(s) in the remote?"),
        count: new_refs,
    })
}

/// `CREATE N BOOKMARK(S)` gate, Mercurial flavour
/// (`ArcanistMercurialLandEngine.php:345-380`).
#[must_use]
pub fn create_gate_hg(new_refs: usize) -> Option<GateFinding> {
    if new_refs == 0 {
        return None;
    }
    Some(GateFinding {
        prompt: PROMPT_CREATE,
        title: format!("CREATE {new_refs} BOOKMARK(S)"),
        query: format!("Create {new_refs} new remote bookmark(s)?"),
        count: new_refs,
    })
}
