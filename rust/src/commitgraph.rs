//! Commit-graph engine behind `arc branches` and `arc bookmarks`.
//!
//! PHP reference: `src/repository/graph/ArcanistCommitGraph.php`,
//! `src/repository/graph/ArcanistCommitNode.php`,
//! `src/repository/graph/query/ArcanistGitCommitGraphQuery.php`,
//! `src/repository/graph/query/ArcanistMercurialCommitGraphQuery.php`,
//! `src/repository/graph/query/ArcanistSimpleCommitGraphQuery.php`,
//! `src/repository/graph/ArcanistCommitGraphPartitionQuery.php`,
//! `src/repository/graph/ArcanistCommitGraphSetQuery.php`,
//! `src/repository/graph/ArcanistCommitGraphPartition.php`,
//! `src/repository/graph/ArcanistCommitGraphSet.php`, and
//! `src/utils/PhutilSortVector.php` (plus `msortv()` / `msortv_natural()`
//! in `src/utils/utils.php`).
//!
//! Divergences:
//! - PHP runs one `git log` future per exact-hash head with up to 8
//!   concurrent line iterators; queries here run sequentially. The markers
//!   workflows only ever issue a single future for head queries and pass no
//!   limit for exact-hash queries, so observable order is identical.
//! - `hg` revset quoting (`hgsprintf()`) is the identity: graph queries
//!   only interpolate hex hashes. The degenerate call with empty head
//!   hashes plus exact hashes (which PHP renders as `(Array or ...)`)
//!   is normalized to the exact-hash revset instead.
//! - Epoch filters (`--after`/`--before`, `hg --date`) are formatted in
//!   UTC; PHP uses the configured PHP timezone. The markers workflows
//!   never set an epoch range.

use std::collections::{BTreeSet, HashMap};

/// Insertion-ordered string map, mirroring PHP array order semantics.
#[derive(Debug, Clone)]
pub struct OrderedMap<V> {
    keys: Vec<String>,
    values: HashMap<String, V>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            values: HashMap::new(),
        }
    }
}

impl<V> OrderedMap<V> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert, keeping first-insertion order like PHP arrays.
    pub fn insert(&mut self, key: String, value: V) {
        if !self.values.contains_key(&key) {
            self.keys.push(key.clone());
        }
        self.values.insert(key, value);
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&V> {
        self.values.get(key)
    }

    #[must_use]
    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.values.get_mut(key)
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[must_use]
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.keys
            .iter()
            .filter_map(|key| self.values.get(key).map(|value| (key, value)))
    }
}

/// One commit node (`ArcanistCommitNode`): parent/child links keep
/// insertion order like the PHP node maps.
#[derive(Debug, Clone)]
pub struct CommitNode {
    hash: String,
    parents: Vec<String>,
    children: Vec<String>,
    message: Option<String>,
    epoch: Option<i64>,
}

impl CommitNode {
    fn new(hash: &str) -> Self {
        Self {
            hash: hash.to_string(),
            parents: Vec::new(),
            children: Vec::new(),
            message: None,
            epoch: None,
        }
    }

    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    #[must_use]
    pub fn parents(&self) -> &[String] {
        &self.parents
    }

    #[must_use]
    pub fn children(&self) -> &[String] {
        &self.children
    }

    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    #[must_use]
    pub fn epoch(&self) -> Option<i64> {
        self.epoch
    }

    fn add_child(&mut self, hash: &str) {
        if !self.children.iter().any(|item| item == hash) {
            self.children.push(hash.to_string());
        }
    }

    fn add_parent(&mut self, hash: &str) {
        if !self.parents.iter().any(|item| item == hash) {
            self.parents.push(hash.to_string());
        }
    }
}

/// A commit graph (`ArcanistCommitGraph`).
#[derive(Debug, Clone, Default)]
pub struct CommitGraph {
    nodes: OrderedMap<CommitNode>,
}

impl CommitGraph {
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: OrderedMap::new(),
        }
    }

    #[must_use]
    pub fn node(&self, hash: &str) -> Option<&CommitNode> {
        self.nodes.get(hash)
    }

    fn node_mut(&mut self, hash: &str) -> Option<&mut CommitNode> {
        self.nodes.get_mut(hash)
    }

    /// Add a node; duplicate hashes fail like `ArcanistCommitGraph::newNode`.
    pub fn new_node(&mut self, hash: &str) -> Result<(), String> {
        if self.nodes.contains(hash) {
            return Err(format!("Graph already has a node \"{hash}\"!"));
        }
        self.nodes.insert(hash.to_string(), CommitNode::new(hash));
        Ok(())
    }

    fn get_or_new_node(&mut self, hash: &str) {
        if !self.nodes.contains(hash) {
            self.nodes.insert(hash.to_string(), CommitNode::new(hash));
        }
    }

    #[must_use]
    pub fn nodes(&self) -> &OrderedMap<CommitNode> {
        &self.nodes
    }
}

/// First line of a trimmed commit message (`ArcanistCommitRef::getSummary`).
#[must_use]
pub fn commit_summary(message: &str) -> String {
    let trimmed = message.trim();
    crate::grid::split_lines(trimmed)
        .first()
        .unwrap_or(&"")
        .to_string()
}

/// Build a graph from a `SimpleCommitGraph` corpus (`A>B` edges).
pub fn graph_from_corpus(graph: &mut CommitGraph, corpus: &str) -> Result<(), String> {
    for edge in corpus.split_whitespace() {
        let Some((parent, child)) = edge.split_once('>') else {
            return Err(format!(
                "Failed to match SimpleCommitGraph directive \"{edge}\"."
            ));
        };
        if parent.is_empty() || child.is_empty() || child.contains('>') {
            return Err(format!(
                "Failed to match SimpleCommitGraph directive \"{edge}\"."
            ));
        }
        graph.get_or_new_node(parent);
        graph.get_or_new_node(child);
        if let Some(node) = graph.node_mut(child) {
            node.add_parent(parent);
        }
        if let Some(node) = graph.node_mut(parent) {
            node.add_child(child);
        }
    }
    Ok(())
}

/// One parsed `git log` / `hg log` record: hash, parents, epoch, message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub hash: String,
    pub parents: Vec<String>,
    pub epoch: i64,
    pub message: String,
}

/// Split one `\x02`-separated log record into 5 fields.
///
/// `tool` names the log command in the failure message; PHP's Mercurial
/// parser (a copy-paste in the reference) also says `"git log"`.
pub fn parse_log_record(
    line: &str,
    tool: &str,
) -> Result<(String, String, String, String), String> {
    let fields: Vec<&str> = line.split('\x02').collect();
    if fields.len() != 5 {
        return Err(format!(
            "Failed to split line \"{line}\" from \"{tool} log\"."
        ));
    }
    Ok((
        fields[1].to_string(),
        fields[2].to_string(),
        fields[3].to_string(),
        fields[4].to_string(),
    ))
}

/// Feed one record into the graph, returning false once `limit` is reached.
fn ingest_record(
    graph: &mut CommitGraph,
    seen: &mut OrderedMap<()>,
    record: &LogRecord,
    limit: Option<usize>,
) -> bool {
    graph.get_or_new_node(&record.hash);
    seen.insert(record.hash.clone(), ());
    if let Some(node) = graph.node_mut(&record.hash) {
        node.message = Some(record.message.clone());
        node.epoch = Some(record.epoch);
        node.parents.clear();
        for parent in &record.parents {
            node.add_parent(parent);
        }
    }
    for parent in &record.parents {
        graph.get_or_new_node(parent);
        if let Some(node) = graph.node_mut(parent) {
            node.add_child(&record.hash);
        }
    }
    limit.is_none_or(|max| seen.len() < max)
}

/// Ingest `\x01`-separated log output, skipping bare `"\n"` records.
fn ingest_log_output(
    graph: &mut CommitGraph,
    seen: &mut OrderedMap<()>,
    stdout: &str,
    limit: Option<usize>,
    mut record: impl FnMut(&str) -> Result<LogRecord, String>,
) -> Result<(), String> {
    for chunk in stdout.split('\x01') {
        if chunk == "\n" {
            continue;
        }
        let parsed = record(chunk)?;
        if !ingest_record(graph, seen, &parsed, limit) {
            return Ok(());
        }
    }
    Ok(())
}

fn git_log_record(line: &str) -> Result<LogRecord, String> {
    let (hash, parents, epoch, message) = parse_log_record(line, "git")?;
    let parents = if parents.is_empty() {
        Vec::new()
    } else {
        parents.split(' ').map(str::to_string).collect()
    };
    Ok(LogRecord {
        hash,
        parents,
        epoch: epoch.parse::<i64>().unwrap_or(0),
        message,
    })
}

/// Format an epoch as an ISO-8601 timestamp in UTC for `--after`/`--before`.
fn iso8601_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}+00:00",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = (shifted.rem_euclid(146_097)) as u32;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Ref lists for the git graph query: one combined heads/tails list, then
/// one list per exact hash.
fn git_ref_lists(
    heads: Option<&[String]>,
    tails: Option<&[String]>,
    exact: Option<&[String]>,
) -> Result<Vec<Vec<String>>, String> {
    let has_heads = heads.is_some_and(|list| !list.is_empty());
    let has_exact = exact.is_some_and(|list| !list.is_empty());
    if !has_heads && !has_exact {
        return Err("Need head hashes or exact hashes!".to_string());
    }
    let mut lists = Vec::new();
    if let Some(heads) = heads
        && !heads.is_empty()
    {
        let mut refs = heads.to_vec();
        if let Some(tails) = tails {
            for tail in tails {
                refs.push(format!("^{tail}^@"));
            }
        }
        lists.push(refs);
    }
    if let Some(exact) = exact {
        for hash in exact {
            lists.push(vec![hash.clone(), format!("^{hash}^@"), "--".to_string()]);
        }
    }
    Ok(lists)
}

fn git_log_format() -> String {
    ["%e", "%H", "%P", "%ct", "%B"].join("\x02") + "\x01"
}

/// Bounds for a commit-graph query: record limit plus an optional
/// committer-epoch range.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryBounds {
    pub limit: Option<usize>,
    pub min_epoch: Option<i64>,
    pub max_epoch: Option<i64>,
}

impl QueryBounds {
    /// No limit and no epoch range.
    #[must_use]
    pub fn unbounded() -> Self {
        Self::default()
    }

    /// Only a record limit.
    #[must_use]
    pub fn limit(limit: usize) -> Self {
        Self {
            limit: Some(limit),
            min_epoch: None,
            max_epoch: None,
        }
    }
}

/// Run the git commit-graph query into `graph`, returning the seen
/// hashes in `git log` order.
pub fn run_git_query(
    graph: &mut CommitGraph,
    dir: &std::path::Path,
    heads: Option<&[String]>,
    tails: Option<&[String]>,
    exact: Option<&[String]>,
    bounds: &QueryBounds,
) -> Result<Vec<String>, String> {
    let lists = git_ref_lists(heads, tails, exact)?;
    let mut seen = OrderedMap::new();
    for refs in &lists {
        let blob = refs.join("\n") + "\n";
        let mut args = vec!["log".to_string(), format!("--format={}", git_log_format())];
        if let Some(min) = bounds.min_epoch {
            args.push("--after".to_string());
            args.push(iso8601_utc(min));
        }
        if let Some(max) = bounds.max_epoch {
            args.push("--before".to_string());
            args.push(iso8601_utc(max));
        }
        args.push("--stdin".to_string());
        args.push("--".to_string());
        // `setResolveOnError(true)`: parse whatever stdout arrives.
        let output = git_command_with_input(dir, &args, blob.as_bytes());
        let stdout = output.unwrap_or_default();
        ingest_log_output(graph, &mut seen, &stdout, bounds.limit, git_log_record)?;
        if bounds.limit.is_some_and(|max| seen.len() >= max) {
            break;
        }
    }
    Ok(seen.keys().to_vec())
}

fn git_command_with_input(
    dir: &std::path::Path,
    args: &[String],
    input: &[u8],
) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        stdin.write_all(input).ok();
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Join revsets with `or`, parenthesizing two or more.
fn join_or_revsets(revsets: &[String]) -> String {
    join_revsets(revsets, false)
}

/// Join revsets with `and`, parenthesizing two or more.
fn join_and_revsets(revsets: &[String]) -> String {
    join_revsets(revsets, true)
}

fn join_revsets(revsets: &[String], is_and: bool) -> String {
    if revsets.is_empty() {
        return String::new();
    }
    if revsets.len() == 1 {
        return revsets[0].clone();
    }
    let glue = if is_and { " and " } else { " or " };
    format!("({})", revsets.join(glue))
}

/// Build the Mercurial graph-query revset.
pub fn hg_query_revset(
    heads: Option<&[String]>,
    tails: Option<&[String]>,
    exact: Option<&[String]>,
) -> Result<String, String> {
    let has_heads = heads.is_some_and(|list| !list.is_empty());
    let has_exact = exact.is_some_and(|list| !list.is_empty());
    if !has_heads && !has_exact {
        return Err("Need head hashes or exact hashes!".to_string());
    }
    let mut revsets = Vec::new();
    if let Some(heads) = heads
        && !heads.is_empty()
    {
        let revs: Vec<String> = heads
            .iter()
            .map(|hash| format!("ancestors({hash})"))
            .collect();
        revsets.push(join_or_revsets(&revs));
    }
    if let Some(tails) = tails
        && !tails.is_empty()
    {
        let revs: Vec<String> = tails
            .iter()
            .map(|hash| format!("descendants({hash})"))
            .collect();
        revsets.push(join_or_revsets(&revs));
    }
    if !revsets.is_empty() {
        revsets = vec![join_and_revsets(&revsets)];
    }
    if let Some(exact) = exact
        && !exact.is_empty()
    {
        revsets.push(join_or_revsets(exact));
    }
    Ok(join_or_revsets(&revsets))
}

fn hg_log_record(line: &str) -> Result<LogRecord, String> {
    let (hash, parents, epoch, message) = parse_log_record(line, "git")?;
    let mut kept = Vec::new();
    if !parents.is_empty() {
        for parent in parents.split(' ') {
            if parent == "0000000000000000000000000000000000000000" {
                continue;
            }
            kept.push(parent.to_string());
        }
    }
    Ok(LogRecord {
        hash,
        parents: kept,
        epoch: crate::hg::parse_rfc822_date(&epoch).unwrap_or(0),
        message,
    })
}

/// Run the Mercurial commit-graph query into `graph`, returning the
/// seen hashes in `hg log` order.
pub fn run_hg_query(
    graph: &mut CommitGraph,
    dir: &std::path::Path,
    hg: &crate::hg::HgCommand,
    heads: Option<&[String]>,
    tails: Option<&[String]>,
    exact: Option<&[String]>,
    bounds: &QueryBounds,
) -> Result<Vec<String>, String> {
    let revset = hg_query_revset(heads, tails, exact)?;
    let template = [
        "",
        "{node}",
        "{p1node} {p2node}",
        "{date|rfc822date}",
        "{desc|utf8}",
    ]
    .join("\x02")
        + "\x01";
    let mut args = vec!["log".to_string(), "--rev".to_string(), revset];
    args.push("--template".to_string());
    args.push(template);
    match (bounds.min_epoch, bounds.max_epoch) {
        (Some(min), Some(max)) => {
            args.push("--date".to_string());
            args.push(format!("{} to {}", iso8601_utc(min), iso8601_utc(max)));
        }
        (Some(min), None) => {
            args.push("--date".to_string());
            args.push(format!(">{}", iso8601_utc(min)));
        }
        (None, Some(max)) => {
            args.push("--date".to_string());
            args.push(format!("<{}", iso8601_utc(max)));
        }
        (None, None) => {}
    }
    args.push("--".to_string());
    // `setResolveOnError(true)`: parse whatever stdout arrives.
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout = crate::hg::output_with(dir, hg, &argv)
        .map(|output| output.stdout)
        .unwrap_or_default();
    let stdout = String::from_utf8_lossy(&stdout).to_string();
    let mut seen = OrderedMap::new();
    ingest_log_output(graph, &mut seen, &stdout, bounds.limit, hg_log_record)?;
    Ok(seen.keys().to_vec())
}

/// One commit-graph partition (`ArcanistCommitGraphPartition`).
#[derive(Debug, Clone)]
pub struct Partition {
    hashes: Vec<String>,
    heads: Vec<String>,
    tails: Vec<String>,
    waypoints: Vec<String>,
}

impl Partition {
    #[must_use]
    pub fn hashes(&self) -> &[String] {
        &self.hashes
    }

    #[must_use]
    pub fn heads(&self) -> &[String] {
        &self.heads
    }

    #[must_use]
    pub fn tails(&self) -> &[String] {
        &self.tails
    }

    #[must_use]
    pub fn waypoints(&self) -> &[String] {
        &self.waypoints
    }
}

/// Partition heads by walking parents, merging on shared ancestors.
///
/// Mirrors `ArcanistCommitGraphPartitionQuery::execute()`, including the
/// `TODO: WARNING` echo for heads with no graph node.
pub fn partition_query(
    graph: &CommitGraph,
    heads: &[String],
    hashes: Option<&[String]>,
) -> Result<Vec<Partition>, String> {
    let mut fused: Vec<String> = Vec::new();
    for head in heads {
        if !fused.contains(head) {
            fused.push(head.clone());
        }
    }
    if fused.is_empty() {
        return Err("Partition query requires heads.".to_string());
    }
    let waypoints = fused.clone();
    let mut live_heads = fused.clone();
    let mut partitions: OrderedMap<usize> = OrderedMap::new();
    let mut identities: Vec<BTreeSet<usize>> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut next_id = 0;
    for hash in fused.iter() {
        if graph.node(hash).is_none() {
            println!("TODO: WARNING: Bad hash {hash}");
            continue;
        }
        partitions.insert(hash.clone(), next_id);
        let mut identity = BTreeSet::new();
        identity.insert(next_id);
        identities.push(identity);
        next_id += 1;
        stack.push(hash.clone());
    }
    let scope: Option<HashMap<&str, ()>> =
        hashes.map(|list| list.iter().map(|hash| (hash.as_str(), ())).collect());
    let mut leaves: HashMap<String, ()> = HashMap::new();
    while let Some(hash) = stack.pop() {
        let node = graph.node(&hash).expect("stack holds known nodes");
        let node_id = *partitions.get(&hash).expect("stack holds assigned nodes");
        let node_partition = identities[node_id].clone();
        let mut saw_parent = false;
        for parent in node.parents() {
            if let Some(scope) = &scope
                && !scope.contains_key(parent.as_str())
            {
                continue;
            }
            saw_parent = true;
            if let Some(&parent_id) = partitions.get(parent) {
                live_heads.retain(|head| head != parent);
                let parent_partition = identities[parent_id].clone();
                let merged: BTreeSet<usize> =
                    node_partition.union(&parent_partition).copied().collect();
                if node_partition != merged {
                    for id in &node_partition {
                        identities[*id] = merged.clone();
                    }
                }
                if parent_partition != merged {
                    for id in &parent_partition {
                        identities[*id] = merged.clone();
                    }
                }
                continue;
            }
            partitions.insert(parent.clone(), node_id);
            stack.push(parent.clone());
        }
        if !saw_parent {
            leaves.insert(hash, ());
        }
    }
    let mut lists: OrderedMap<Vec<String>> = OrderedMap::new();
    let mut list_heads: HashMap<usize, Vec<String>> = HashMap::new();
    let mut list_waypoints: HashMap<usize, Vec<String>> = HashMap::new();
    let mut list_leaves: HashMap<usize, Vec<String>> = HashMap::new();
    for (hash, id) in partitions.iter() {
        let resolved = *identities[*id].iter().next().expect("identity is nonempty");
        lists
            .get_mut(&resolved.to_string())
            .map(|list| list.push(hash.clone()))
            .unwrap_or_else(|| {
                lists.insert(resolved.to_string(), vec![hash.clone()]);
            });
        if live_heads.contains(hash) {
            list_heads.entry(resolved).or_default().push(hash.clone());
        }
        if waypoints.contains(hash) {
            list_waypoints
                .entry(resolved)
                .or_default()
                .push(hash.clone());
        }
        if leaves.contains_key(hash) {
            list_leaves.entry(resolved).or_default().push(hash.clone());
        }
    }
    let mut results = Vec::new();
    for (key, list) in lists.iter() {
        let id: usize = key.parse().expect("partition keys are ids");
        results.push(Partition {
            hashes: list.clone(),
            heads: list_heads.remove(&id).unwrap_or_default(),
            tails: list_leaves.remove(&id).unwrap_or_default(),
            waypoints: list_waypoints.remove(&id).unwrap_or_default(),
        });
    }
    Ok(results)
}

/// A waypoint color: fused revision ids, or `Open` for uncolored paint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetColor {
    Open,
    Fused(Vec<u32>),
}

fn fuse_ids(ids: &[u32]) -> Vec<u32> {
    let mut fused = Vec::new();
    for &id in ids {
        if !fused.contains(&id) {
            fused.push(id);
        }
    }
    fused
}

fn merge_colors(first: &SetColor, second: &SetColor) -> SetColor {
    match (first, second) {
        (SetColor::Open, other) | (other, SetColor::Open) => other.clone(),
        (SetColor::Fused(left), SetColor::Fused(right)) => {
            let mut merged = left.clone();
            for &id in right {
                if !merged.contains(&id) {
                    merged.push(id);
                }
            }
            SetColor::Fused(merged)
        }
    }
}

/// One commit-graph set (`ArcanistCommitGraphSet`).
#[derive(Debug, Clone)]
pub struct CommitSet {
    set_id: u64,
    color: SetColor,
    hashes: Vec<String>,
    parent_hashes: Vec<String>,
    child_hashes: Vec<String>,
    parent_sets: Vec<u64>,
    child_sets: Vec<u64>,
    display_child_sets: Vec<u64>,
}

impl CommitSet {
    #[must_use]
    pub fn set_id(&self) -> u64 {
        self.set_id
    }

    #[must_use]
    pub fn color(&self) -> &SetColor {
        &self.color
    }

    #[must_use]
    pub fn hashes(&self) -> &[String] {
        &self.hashes
    }

    #[must_use]
    pub fn parent_hashes(&self) -> &[String] {
        &self.parent_hashes
    }

    #[must_use]
    pub fn child_hashes(&self) -> &[String] {
        &self.child_hashes
    }

    #[must_use]
    pub fn parent_sets(&self) -> &[u64] {
        &self.parent_sets
    }

    #[must_use]
    pub fn child_sets(&self) -> &[u64] {
        &self.child_sets
    }

    #[must_use]
    pub fn display_child_sets(&self) -> &[u64] {
        &self.display_child_sets
    }

    /// Restrict display children to `order`, in that order
    /// (`array_select_keys()` on the PHP side).
    pub fn reorder_display_children(&mut self, order: &[u64]) {
        let kept: Vec<u64> = order
            .iter()
            .copied()
            .filter(|id| self.display_child_sets.contains(id))
            .collect();
        self.display_child_sets = kept;
    }
}

/// Paint a partition from its tails and collect same-color sets.
///
/// Mirrors `ArcanistCommitGraphSetQuery::execute()`; `waypoint_map` maps a
/// commit hash to its revision ids, and `set_counter` is the shared
/// process-wide set id counter (PHP's `static $set_n`, starting at 1).
pub fn set_query(
    graph: &CommitGraph,
    partition: &Partition,
    waypoint_map: &HashMap<String, Vec<u32>>,
    set_counter: &mut u64,
) -> Result<Vec<CommitSet>, String> {
    let mut waypoint_color: HashMap<String, SetColor> = HashMap::new();
    for (hash, ids) in waypoint_map {
        waypoint_color.insert(hash.clone(), SetColor::Fused(fuse_ids(ids)));
    }
    let mut colors: OrderedMap<SetColor> = OrderedMap::new();
    let mut stack: Vec<String> = Vec::new();
    for hash in partition.tails() {
        stack.push(hash.clone());
        let color = waypoint_color.get(hash).cloned().unwrap_or(SetColor::Open);
        colors.insert(hash.clone(), color);
    }
    let in_partition: HashMap<&str, ()> = partition
        .hashes()
        .iter()
        .map(|hash| (hash.as_str(), ()))
        .collect();
    let mut wait: HashMap<String, usize> = HashMap::new();
    for hash in partition.hashes() {
        let Some(node) = graph.node(hash) else {
            continue;
        };
        if node.parents().len() < 2 {
            continue;
        }
        let need = node
            .parents()
            .iter()
            .filter(|parent| in_partition.contains_key(parent.as_str()))
            .count();
        if need >= 2 {
            wait.insert(hash.clone(), need);
        }
    }
    while let Some(hash) = stack.pop() {
        let node_color = colors.get(&hash).cloned().unwrap_or(SetColor::Open);
        let node = graph.node(&hash).expect("painted nodes exist");
        for child in node.children() {
            let color = if let Some(waypoint) = waypoint_color.get(child) {
                waypoint.clone()
            } else if let Some(existing) = colors.get(child) {
                merge_colors(existing, &node_color)
            } else {
                node_color.clone()
            };
            colors.insert(child.clone(), color);
            if let Some(need) = wait.get_mut(child) {
                *need -= 1;
                if *need > 0 {
                    continue;
                }
                wait.remove(child);
            }
            stack.push(child.clone());
        }
    }
    if !wait.is_empty() {
        return Err("Did not reach every wait node??".to_string());
    }
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut sets = Vec::new();
    for (hash, color) in colors.iter() {
        if seen.contains_key(hash) {
            continue;
        }
        seen.insert(hash.clone(), ());
        let mut in_set = vec![hash.clone()];
        let mut flood = vec![hash.clone()];
        while let Some(current) = flood.pop() {
            let node = graph.node(&current).expect("flooded nodes exist");
            let nearby = node.parents().iter().chain(node.children().iter());
            for hash in nearby {
                if seen.contains_key(hash) {
                    continue;
                }
                if colors.get(hash) != Some(color) {
                    continue;
                }
                seen.insert(hash.clone(), ());
                in_set.push(hash.clone());
                flood.push(hash.clone());
            }
        }
        let set_id = *set_counter;
        *set_counter += 1;
        sets.push(CommitSet {
            set_id,
            color: color.clone(),
            hashes: in_set,
            parent_hashes: Vec::new(),
            child_hashes: Vec::new(),
            parent_sets: Vec::new(),
            child_sets: Vec::new(),
            display_child_sets: Vec::new(),
        });
    }
    link_sets(graph, &mut sets);
    build_display_layout(&mut sets);
    Ok(sets)
}

/// Edges crossing out of one set: parent hashes, child hashes, and the
/// adjacent parent/child set ids.
type SetLinks = (Vec<String>, Vec<String>, Vec<u64>, Vec<u64>);

fn link_sets(graph: &CommitGraph, sets: &mut [CommitSet]) {
    let mut set_map: HashMap<String, usize> = HashMap::new();
    for (index, set) in sets.iter().enumerate() {
        for hash in set.hashes() {
            set_map.insert(hash.clone(), index);
        }
    }
    let set_ids: Vec<u64> = sets.iter().map(CommitSet::set_id).collect();
    let links: Vec<SetLinks> = sets
        .iter()
        .enumerate()
        .map(|(index, set)| link_one_set(graph, &set_map, &set_ids, index, set))
        .collect();
    for (set, (parents, children, parent_sets, child_sets)) in sets.iter_mut().zip(links) {
        set.parent_hashes = parents;
        set.child_hashes = children;
        set.parent_sets = parent_sets;
        set.child_sets = child_sets;
    }
}

/// Edges crossing out of one set: parent/child hashes plus the adjacent
/// set ids in first-appearance order.
fn link_one_set(
    graph: &CommitGraph,
    set_map: &HashMap<String, usize>,
    set_ids: &[u64],
    index: usize,
    set: &CommitSet,
) -> SetLinks {
    let mut parents: Vec<String> = Vec::new();
    let mut children: Vec<String> = Vec::new();
    for hash in set.hashes() {
        let Some(node) = graph.node(hash) else {
            continue;
        };
        for edge in node.parents() {
            if set_map.get(edge) != Some(&index) && !parents.contains(edge) {
                parents.push(edge.clone());
            }
        }
        for edge in node.children() {
            if set_map.get(edge) != Some(&index) && !children.contains(edge) {
                children.push(edge.clone());
            }
        }
    }
    let mut parent_sets = Vec::new();
    for edge in &parents {
        if let Some(&adjacent) = set_map.get(edge) {
            let id = set_ids[adjacent];
            if !parent_sets.contains(&id) {
                parent_sets.push(id);
            }
        }
    }
    let mut child_sets = Vec::new();
    for edge in &children {
        if let Some(&adjacent) = set_map.get(edge) {
            let id = set_ids[adjacent];
            if !child_sets.contains(&id) {
                child_sets.push(id);
            }
        }
    }
    (parents, children, parent_sets, child_sets)
}

fn build_display_layout(sets: &mut [CommitSet]) {
    let mut visited: HashMap<u64, ()> = HashMap::new();
    let roots: Vec<usize> = sets
        .iter()
        .enumerate()
        .filter(|(_, set)| set.parent_sets().is_empty())
        .map(|(index, _)| index)
        .collect();
    for root in roots {
        visit_display_set(sets, &mut visited, root);
    }
}

fn visit_display_set(sets: &mut [CommitSet], visited: &mut HashMap<u64, ()>, index: usize) -> bool {
    let id = sets[index].set_id();
    if visited.contains_key(&id) {
        return true;
    }
    for parent in sets[index].parent_sets().to_vec() {
        if !visited.contains_key(&parent) {
            return false;
        }
    }
    visited.insert(id, ());
    let mut display = Vec::new();
    for child in sets[index].child_sets().to_vec() {
        let position = sets
            .iter()
            .position(|set| set.set_id() == child)
            .expect("child sets exist");
        if visit_display_set(sets, visited, position) {
            display.push(child);
        }
    }
    sets[index].display_child_sets = display;
    true
}

/// A sortable vector (`PhutilSortVector`): integers sort as zero-padded
/// prefixed strings, joined with NUL separators.
#[derive(Debug, Clone, Default)]
pub struct SortVector {
    parts: Vec<String>,
}

impl SortVector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an integer part, encoded so plain string comparison sorts
    /// numerically (`A` for `i64::MIN`, `B` below zero, `C` otherwise).
    pub fn add_int(&mut self, value: i64) {
        let (prefix, magnitude) = if value == i64::MIN {
            ("A", value as u64)
        } else if value < 0 {
            ("B", (i64::MAX as u64).wrapping_add(value as u64))
        } else {
            ("C", value as u64)
        };
        self.parts.push(format!("{prefix}{magnitude:020}"));
    }

    /// Add a string part; NUL bytes fail like the PHP vector.
    pub fn add_string(&mut self, value: &str) -> Result<(), String> {
        if !value.is_empty() && value.contains('\0') {
            return Err(
                "String components of a sort vector must not contain NULL bytes.".to_string(),
            );
        }
        self.parts.push(value.to_string());
        Ok(())
    }

    /// The comparison surrogate: parts joined with NUL separators.
    #[must_use]
    pub fn sort_string(&self) -> String {
        self.parts.join("\0")
    }
}

/// Order `items` by their vectors with byte comparison (`msortv()` with
/// `SORT_STRING`), breaking ties by original position like the PHP
/// stability suffix.
pub fn sort_by_vector<T>(items: &mut Vec<T>, mut vector: impl FnMut(&T) -> SortVector) {
    let mut order: Vec<(String, usize)> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (vector(item).sort_string(), index))
        .collect();
    order.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then(left.1.cmp(&right.1))
    });
    apply_order(items, &order);
}

/// Order `items` by their vectors with case-insensitive natural comparison
/// (`msortv_natural()`), breaking ties by original position.
pub fn sort_by_vector_natural<T>(items: &mut Vec<T>, mut vector: impl FnMut(&T) -> SortVector) {
    let mut order: Vec<(String, usize)> = items
        .iter()
        .enumerate()
        .map(|(index, item)| (vector(item).sort_string(), index))
        .collect();
    order.sort_by(|left, right| {
        natural_cmp(left.0.as_bytes(), right.0.as_bytes(), true).then(left.1.cmp(&right.1))
    });
    apply_order(items, &order);
}

fn apply_order<T>(items: &mut Vec<T>, order: &[(String, usize)]) {
    let mut owned: Vec<Option<T>> = items.drain(..).map(Some).collect();
    for (_, index) in order {
        items.push(owned[*index].take().expect("each index used once"));
    }
}

fn natural_is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// Right-aligned digit-run comparison from `strnatcmp.c`: the longest run
/// wins, otherwise the greatest value wins.
fn compare_right(a: &[u8], ai: &mut usize, b: &[u8], bi: &mut usize) -> i32 {
    let mut bias = 0;
    loop {
        let a_digit = a.get(*ai).is_some_and(u8::is_ascii_digit);
        let b_digit = b.get(*bi).is_some_and(u8::is_ascii_digit);
        match (a_digit, b_digit) {
            (false, false) => return bias,
            (false, true) => return -1,
            (true, false) => return 1,
            (true, true) => {}
        }
        match a[*ai].cmp(&b[*bi]) {
            std::cmp::Ordering::Less => {
                if bias == 0 {
                    bias = -1;
                }
            }
            std::cmp::Ordering::Greater => {
                if bias == 0 {
                    bias = 1;
                }
            }
            std::cmp::Ordering::Equal => {}
        }
        *ai += 1;
        *bi += 1;
    }
}

/// Left-aligned digit-run comparison from `strnatcmp.c`: the first
/// different value wins.
fn compare_left(a: &[u8], ai: &mut usize, b: &[u8], bi: &mut usize) -> i32 {
    loop {
        let a_digit = a.get(*ai).is_some_and(u8::is_ascii_digit);
        let b_digit = b.get(*bi).is_some_and(u8::is_ascii_digit);
        match (a_digit, b_digit) {
            (false, false) => return 0,
            (false, true) => return -1,
            (true, false) => return 1,
            (true, true) => {}
        }
        match a[*ai].cmp(&b[*bi]) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => {}
        }
        *ai += 1;
        *bi += 1;
    }
}

/// Natural-order byte comparison, ported from php-src
/// `ext/standard/strnatcmp.c` (`strnatcasecmp()` when
/// `case_insensitive`).
///
/// Past-the-end reads see a NUL byte, matching the NUL-terminated
/// buffers the C version over-reads when skipping whitespace.
pub fn natural_cmp(a: &[u8], b: &[u8], case_insensitive: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.is_empty() || b.is_empty() {
        return a.len().cmp(&b.len());
    }
    // Skip leading zeros once, before the main loop.
    let mut ai = 0;
    while ai + 1 < a.len() && a[ai] == b'0' && a[ai + 1].is_ascii_digit() {
        ai += 1;
    }
    let mut bi = 0;
    while bi + 1 < b.len() && b[bi] == b'0' && b[bi + 1].is_ascii_digit() {
        bi += 1;
    }
    let mut ca = a[ai];
    let mut cb = b[bi];
    loop {
        while natural_is_space(ca) {
            ai += 1;
            ca = *a.get(ai).unwrap_or(&0);
        }
        while natural_is_space(cb) {
            bi += 1;
            cb = *b.get(bi).unwrap_or(&0);
        }
        // Digit runs on both sides compare numerically; afterwards PHP
        // falls through to compare the bytes after the runs, it does not
        // restart the loop.
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let fractional = ca == b'0' || cb == b'0';
            let result = if fractional {
                compare_left(a, &mut ai, b, &mut bi)
            } else {
                compare_right(a, &mut ai, b, &mut bi)
            };
            match result.cmp(&0) {
                Ordering::Less => return Ordering::Less,
                Ordering::Greater => return Ordering::Greater,
                Ordering::Equal => {}
            }
            let a_end = ai >= a.len();
            let b_end = bi >= b.len();
            if a_end && b_end {
                return Ordering::Equal;
            } else if a_end {
                return Ordering::Less;
            } else if b_end {
                return Ordering::Greater;
            }
            ca = a[ai];
            cb = b[bi];
        }
        if case_insensitive {
            ca = ca.to_ascii_uppercase();
            cb = cb.to_ascii_uppercase();
        }
        match ca.cmp(&cb) {
            Ordering::Less => return Ordering::Less,
            Ordering::Greater => return Ordering::Greater,
            Ordering::Equal => {}
        }
        ai += 1;
        bi += 1;
        let a_end = ai >= a.len();
        let b_end = bi >= b.len();
        if a_end && b_end {
            return Ordering::Equal;
        } else if a_end {
            return Ordering::Less;
        } else if b_end {
            return Ordering::Greater;
        }
        ca = a[ai];
        cb = b[bi];
    }
}

/// Maximum of `names` under case-insensitive natural order, like
/// `natcasesort()` followed by `last()`. Ties resolve to the last
/// occurrence (PHP 8 stable-sort behavior).
pub fn natural_max<'a>(names: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<&'a str> = None;
    for &name in names {
        match best {
            None => best = Some(name),
            Some(current) => {
                if natural_cmp(current.as_bytes(), name.as_bytes(), true)
                    != std::cmp::Ordering::Greater
                {
                    best = Some(name);
                }
            }
        }
    }
    best
}

#[cfg(test)]
#[path = "commitgraph_tests.rs"]
mod commitgraph_tests;
