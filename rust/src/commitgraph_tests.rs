use super::*;
use std::collections::HashMap;

fn corpus_graph(corpus: &str) -> CommitGraph {
    let mut graph = CommitGraph::new();
    graph_from_corpus(&mut graph, corpus).unwrap();
    graph
}

fn partition_count(heads: &[&str], corpus: &str) -> usize {
    let graph = corpus_graph(corpus);
    let heads: Vec<String> = heads.iter().map(|item| (*item).to_string()).collect();
    partition_query(&graph, &heads, None).unwrap().len()
}

#[test]
fn rbd33_empty_log_output_ingests_zero_records() {
    // T1153 RB-D33: empty `git log` output yields nothing, like PHP's
    // line iterator — only malformed non-empty chunks fail.
    let mut graph = CommitGraph::new();
    let mut seen = OrderedMap::new();
    ingest_log_output(&mut graph, &mut seen, "", None, git_log_record).unwrap();
    assert!(seen.is_empty());
    let mut graph = CommitGraph::new();
    let mut seen = OrderedMap::new();
    assert!(ingest_log_output(&mut graph, &mut seen, "garbage", None, git_log_record).is_err());
}

#[test]
fn rejects_duplicate_nodes() {
    let mut graph = CommitGraph::new();
    graph.new_node("a").unwrap();
    assert_eq!(
        graph.new_node("a").unwrap_err(),
        "Graph already has a node \"a\"!"
    );
}

#[test]
fn rejects_bad_corpus_edges() {
    let mut graph = CommitGraph::new();
    assert!(graph_from_corpus(&mut graph, "A>B C").is_err());
    assert!(graph_from_corpus(&mut graph, "A>B>").is_err());
    assert!(graph_from_corpus(&mut graph, ">B").is_err());
}

#[test]
fn corpus_links_parents_and_children() {
    let graph = corpus_graph("A>B B>C");
    assert_eq!(graph.node("B").unwrap().parents(), &["A".to_string()]);
    assert_eq!(graph.node("B").unwrap().children(), &["C".to_string()]);
}

#[test]
fn matches_php_partition_counts() {
    assert_eq!(partition_count(&["D"], "A>B B>C C>D"), 1);
    assert_eq!(partition_count(&["D", "E"], "A>B B>C C>D C>E"), 1);
    assert_eq!(partition_count(&["B"], "A>B C>D"), 1);
    assert_eq!(partition_count(&["B", "D"], "A>B C>D"), 2);
    assert_eq!(
        partition_count(
            &["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K"],
            "A>B B>C B>D B>E E>F E>G E>H C>H A>I C>I B>J J>K I>K"
        ),
        1
    );
}

#[test]
fn partitions_require_heads() {
    let graph = corpus_graph("A>B");
    assert_eq!(
        partition_query(&graph, &[], None).unwrap_err(),
        "Partition query requires heads."
    );
}

#[test]
fn partitions_report_heads_and_tails() {
    let graph = corpus_graph("A>B B>C C>D");
    let heads = vec!["D".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].heads(), &["D".to_string()]);
    assert_eq!(partitions[0].tails(), &["A".to_string()]);
    assert_eq!(
        partitions[0].hashes(),
        &[
            "D".to_string(),
            "C".to_string(),
            "B".to_string(),
            "A".to_string()
        ]
    );
}

#[test]
fn disjoint_heads_stay_in_separate_partitions() {
    let graph = corpus_graph("A>B C>D");
    let heads = vec!["B".to_string(), "D".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    assert_eq!(partitions.len(), 2);
    assert_eq!(partitions[0].heads(), &["B".to_string()]);
    assert_eq!(partitions[1].heads(), &["D".to_string()]);
}

#[test]
fn scope_limits_parent_walks() {
    let graph = corpus_graph("A>B B>C C>D");
    let heads = vec!["D".to_string()];
    let scope = vec!["D".to_string(), "C".to_string()];
    let partitions = partition_query(&graph, &heads, Some(&scope)).unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].hashes(), &["D".to_string(), "C".to_string()]);
    assert_eq!(partitions[0].tails(), &["C".to_string()]);
}

#[test]
fn uncolored_partitions_form_one_set() {
    let graph = corpus_graph("A>B B>C");
    let heads = vec!["C".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    let mut counter = 1;
    let sets = set_query(&graph, &partitions[0], &HashMap::new(), &mut counter).unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].set_id(), 1);
    assert_eq!(sets[0].color(), &SetColor::Open);
    assert_eq!(
        sets[0].hashes(),
        &["A".to_string(), "B".to_string(), "C".to_string()]
    );
    assert!(sets[0].parent_sets().is_empty());
    assert_eq!(counter, 2);
}

#[test]
fn waypoint_colors_split_sets() {
    let graph = corpus_graph("A>B B>C");
    let heads = vec!["C".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    let mut waypoints = HashMap::new();
    waypoints.insert("C".to_string(), vec![7]);
    let mut counter = 1;
    let sets = set_query(&graph, &partitions[0], &waypoints, &mut counter).unwrap();
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0].color(), &SetColor::Open);
    assert_eq!(sets[0].hashes(), &["A".to_string(), "B".to_string()]);
    assert_eq!(sets[1].color(), &SetColor::Fused(vec![7]));
    assert_eq!(sets[1].hashes(), &["C".to_string()]);
    assert_eq!(sets[0].child_sets(), &[sets[1].set_id()]);
    assert_eq!(sets[1].parent_sets(), &[sets[0].set_id()]);
    assert_eq!(sets[0].display_child_sets(), &[sets[1].set_id()]);
}

#[test]
fn merge_commits_wait_for_every_parent() {
    let graph = corpus_graph("A>C B>C");
    let heads = vec!["C".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    assert_eq!(partitions[0].tails(), &["A".to_string(), "B".to_string()]);
    let mut counter = 1;
    let sets = set_query(&graph, &partitions[0], &HashMap::new(), &mut counter).unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].hashes().len(), 3);
}

#[test]
fn set_counter_is_shared_across_partitions() {
    let graph = corpus_graph("A>B C>D");
    let heads = vec!["B".to_string(), "D".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    let mut counter = 1;
    let first = set_query(&graph, &partitions[0], &HashMap::new(), &mut counter).unwrap();
    let second = set_query(&graph, &partitions[1], &HashMap::new(), &mut counter).unwrap();
    assert_eq!(first[0].set_id(), 1);
    assert_eq!(second[0].set_id(), 2);
    assert_eq!(counter, 3);
}

#[test]
fn ingest_skips_eof_empty_chunk() {
    // T1153 RB-D33: empty output yields nothing (like `LinesOfALarge`),
    // and a trailing delimiter leaves no empty chunk behind.
    let mut graph = CommitGraph::new();
    let mut seen = OrderedMap::new();
    let no_chunks = |chunk: &str| -> Result<LogRecord, String> { panic!("chunk {chunk:?}") };
    ingest_log_output(&mut graph, &mut seen, "", None, no_chunks).unwrap();
    assert!(seen.is_empty());
    let mut graph = CommitGraph::new();
    let mut seen = OrderedMap::new();
    ingest_log_output(
        &mut graph,
        &mut seen,
        "e\x02h\x02\x020\x02m\x01",
        None,
        git_log_record,
    )
    .unwrap();
    assert_eq!(seen.keys(), &["h".to_string()]);
    assert_eq!(graph.node("h").unwrap().message(), Some("m"));
}

#[test]
fn parses_log_records() {
    let (hash, parents, epoch, message) =
        parse_log_record("e\x02abc\x02p1 p2\x021729000000\x02hello\n", "git").unwrap();
    assert_eq!(hash, "abc");
    assert_eq!(parents, "p1 p2");
    assert_eq!(epoch, "1729000000");
    assert_eq!(message, "hello\n");
    let err = parse_log_record("too\x02few", "git").unwrap_err();
    assert_eq!(err, "Failed to split line \"too\x02few\" from \"git log\".");
}

#[test]
fn git_records_default_bad_epochs_to_zero() {
    let record = git_log_record("e\x02h\x02\x02soon\x02m").unwrap();
    assert_eq!(record.epoch, 0);
    assert!(record.parents.is_empty());
}

#[test]
fn hg_records_drop_zero_parents_and_read_rfc822_dates() {
    let record = hg_log_record(
        "\x02h\x02p1 0000000000000000000000000000000000000000\x02Thu, 01 Jan 1970 00:00:00 +0000\x02m",
    )
    .unwrap();
    assert_eq!(record.parents, vec!["p1".to_string()]);
    assert_eq!(record.epoch, 0);
}

#[test]
fn builds_git_ref_lists() {
    let heads = vec!["h".to_string()];
    let tails = vec!["t".to_string()];
    assert!(git_ref_lists(None, None, None).is_err());
    assert!(git_ref_lists(Some(&[]), None, None).is_err());
    let lists = git_ref_lists(Some(&heads), Some(&tails), None).unwrap();
    assert_eq!(lists, vec![vec!["h".to_string(), "^t^@".to_string()]]);
    let exact = vec!["x".to_string()];
    let lists = git_ref_lists(None, None, Some(&exact)).unwrap();
    assert_eq!(
        lists,
        vec![vec!["x".to_string(), "^x^@".to_string(), "--".to_string()]]
    );
}

#[test]
fn builds_hg_revsets() {
    assert!(hg_query_revset(None, None, None).is_err());
    let heads = vec!["a".to_string()];
    assert_eq!(
        hg_query_revset(Some(&heads), None, None).unwrap(),
        "ancestors(a)"
    );
    let heads = vec!["a".to_string(), "b".to_string()];
    let tails = vec!["t".to_string()];
    assert_eq!(
        hg_query_revset(Some(&heads), Some(&tails), None).unwrap(),
        "((ancestors(a) or ancestors(b)) and descendants(t))"
    );
    let exact = vec!["x".to_string(), "y".to_string()];
    assert_eq!(
        hg_query_revset(None, None, Some(&exact)).unwrap(),
        "(x or y)"
    );
}

#[test]
fn formats_iso8601_utc() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00+00:00");
    assert_eq!(iso8601_utc(1_729_000_000), "2024-10-15T13:46:40+00:00");
}

#[test]
fn encodes_sort_vector_integers() {
    let mut vector = SortVector::new();
    vector.add_int(0);
    vector.add_int(-1);
    vector.add_int(i64::MIN);
    vector.add_int(7);
    let surrogate = vector.sort_string();
    let parts: Vec<&str> = surrogate.split('\0').collect();
    assert_eq!(parts[0], "C00000000000000000000");
    assert_eq!(parts[1], "B09223372036854775806");
    assert_eq!(parts[2], "A09223372036854775808");
    assert_eq!(parts[3], "C00000000000000000007");
    // T1153 RB-D38d (kept): NUL fails here (unreachable in practice).
    assert!(SortVector::new().add_string("a\0b").is_err());
}

#[test]
fn compares_natural_order() {
    use std::cmp::Ordering;
    assert_eq!(natural_cmp(b"a1", b"a2", true), Ordering::Less);
    assert_eq!(natural_cmp(b"a10", b"a2", true), Ordering::Greater);
    assert_eq!(natural_cmp(b"a01", b"a1", true), Ordering::Less);
    assert_eq!(natural_cmp(b"01", b"1", true), Ordering::Equal);
    assert_eq!(natural_cmp(b"img12", b"img10", true), Ordering::Greater);
    assert_eq!(natural_cmp(b"b", b"A", true), Ordering::Greater);
    assert_eq!(natural_cmp(b"b", b"A", false), Ordering::Greater);
    assert_eq!(natural_cmp(b"a", b"B", true), Ordering::Less);
    assert_eq!(natural_cmp(b"a b", b"ab", true), Ordering::Equal);
    assert_eq!(natural_cmp(b"a ", b"a", true), Ordering::Greater);
    assert_eq!(natural_cmp(b"", b"a", true), Ordering::Less);
    assert_eq!(natural_cmp(b"", b"", true), Ordering::Equal);
    // Case differences alone compare equal, like `strnatcasecmp()`.
    assert_eq!(natural_cmp(b"Foo", b"foo", true), Ordering::Equal);
    assert_eq!(natural_cmp(b"Foo", b"foo", false), Ordering::Less);
}

#[test]
fn finds_natural_maxima() {
    assert_eq!(natural_max(&[]), None);
    assert_eq!(natural_max(&["b2", "b10", "a"]), Some("b10"));
    // Ties resolve to the last occurrence.
    assert_eq!(natural_max(&["a", "A"]), Some("A"));
}

#[test]
fn sorts_vectors_with_byte_and_natural_order() {
    let mut items = vec!["b10".to_string(), "b2".to_string(), "a".to_string()];
    sort_by_vector(&mut items, |name| {
        let mut vector = SortVector::new();
        vector.add_int(0);
        vector.add_string(name).unwrap();
        vector
    });
    assert_eq!(items, vec!["a", "b10", "b2"]);
    sort_by_vector_natural(&mut items, |name| {
        let mut vector = SortVector::new();
        vector.add_int(0);
        vector.add_string(name).unwrap();
        vector
    });
    assert_eq!(items, vec!["a", "b2", "b10"]);
}

#[test]
fn vector_sorts_are_stable() {
    let mut items = vec![(1, "x".to_string()), (2, "x".to_string())];
    sort_by_vector_natural(&mut items, |(_, name)| {
        let mut vector = SortVector::new();
        vector.add_string(name).unwrap();
        vector
    });
    assert_eq!(items[0].0, 1);
    assert_eq!(items[1].0, 2);
}

#[test]
fn summarizes_commit_messages() {
    assert_eq!(commit_summary("subject\n\nbody\n"), "subject");
    assert_eq!(commit_summary("  subject  \n"), "subject");
    assert_eq!(commit_summary(""), "");
}

// -( Coverage: queries, sets, display, natural order )--------------------------

#[test]
fn node_and_graph_getters() {
    let graph = corpus_graph("A>B B>C");
    assert_eq!(graph.node("B").unwrap().hash(), "B");
    assert_eq!(graph.nodes().len(), 3);
    assert!(graph.nodes().contains("A"));
    let heads = vec!["C".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    assert_eq!(partitions[0].waypoints(), &["C".to_string()]);
    let mut counter = 1;
    let sets = set_query(&graph, &partitions[0], &HashMap::new(), &mut counter).unwrap();
    assert_eq!(sets.len(), 1);
    assert!(sets[0].parent_hashes().is_empty());
    assert!(sets[0].child_hashes().is_empty());
}

#[test]
fn ingest_stops_at_record_limit() {
    let mut graph = CommitGraph::new();
    let mut seen = OrderedMap::new();
    ingest_log_output(
        &mut graph,
        &mut seen,
        "e\x02h1\x02\x020\x02m1\x01e\x02h2\x02\x020\x02m2",
        Some(1),
        git_log_record,
    )
    .unwrap();
    assert_eq!(seen.keys(), &["h1".to_string()]);
    assert!(graph.node("h2").is_none());
}

#[test]
fn git_query_passes_epoch_bounds() {
    // A bogus head never resolves (and a missing `git` resolves to
    // nothing), so the query stays empty either way after the epoch
    // flags are composed.
    let dir = std::env::temp_dir().join(format!("arc-rs-commitgraph-git-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut graph = CommitGraph::new();
    let heads = vec!["arc-rs-definitely-not-a-branch-xyz".to_string()];
    let bounds = QueryBounds {
        limit: None,
        min_epoch: Some(0),
        max_epoch: Some(86_400),
    };
    let seen = run_git_query(&mut graph, &dir, Some(&heads), None, None, &bounds).unwrap();
    assert!(seen.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_query_zero_limit_stops_before_lists() {
    let dir = std::env::temp_dir().join(format!("arc-rs-commitgraph-git0-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut graph = CommitGraph::new();
    let exact = vec!["0".repeat(40), "1".repeat(40)];
    let bounds = QueryBounds {
        limit: Some(0),
        min_epoch: None,
        max_epoch: None,
    };
    let seen = run_git_query(&mut graph, &dir, None, None, Some(&exact), &bounds).unwrap();
    assert!(seen.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_revsets_join_to_nothing() {
    let empty: Vec<String> = Vec::new();
    assert_eq!(join_or_revsets(&empty), "");
    assert_eq!(join_and_revsets(&empty), "");
}

#[test]
fn hg_records_accept_empty_parents() {
    let record = hg_log_record("\x02h\x02\x02Thu, 01 Jan 1970 00:00:00 +0000\x02m").unwrap();
    assert_eq!(record.hash, "h");
    assert!(record.parents.is_empty());
}

#[test]
#[cfg(unix)]
fn hg_query_passes_date_ranges() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("arc-rs-commitgraph-hg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let seen_args = dir.join("seen-args.txt");
    let tool = dir.join("fake-hg");
    std::fs::write(
        &tool,
        format!(
            "#!/bin/sh\necho \"$@\" > '{}'\n",
            seen_args.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hg = crate::hg::HgCommand::custom(&tool.to_string_lossy(), &[]);
    let heads = vec!["abc123".to_string()];
    let run = |bounds: &QueryBounds| {
        let mut graph = CommitGraph::new();
        let seen = run_hg_query(&mut graph, &dir, &hg, Some(&heads), None, None, bounds).unwrap();
        assert!(seen.is_empty());
        std::fs::read_to_string(&seen_args).unwrap()
    };
    let both = run(&QueryBounds {
        limit: None,
        min_epoch: Some(0),
        max_epoch: Some(86_400),
    });
    assert!(
        both.contains("--date 1970-01-01T00:00:00+00:00 to 1970-01-02T00:00:00+00:00"),
        "{both}"
    );
    let after = run(&QueryBounds {
        limit: None,
        min_epoch: Some(0),
        max_epoch: None,
    });
    assert!(
        after.contains("--date >1970-01-01T00:00:00+00:00"),
        "{after}"
    );
    let before = run(&QueryBounds {
        limit: None,
        min_epoch: None,
        max_epoch: Some(86_400),
    });
    assert!(
        before.contains("--date <1970-01-02T00:00:00+00:00"),
        "{before}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn partitions_skip_unknown_heads() {
    let graph = corpus_graph("A>B");
    let heads = vec!["nope".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    assert!(partitions.is_empty());
}

#[test]
fn shared_ancestors_fuse_identities_without_updates() {
    // One head over a diamond: the second visit to the shared
    // ancestor merges identical identity sets, taking neither
    // update arm.
    assert_eq!(partition_count(&["D"], "A>B A>C B>D C>D"), 1);
    let graph = corpus_graph("A>B A>C B>D C>D");
    let heads = vec!["D".to_string()];
    let partitions = partition_query(&graph, &heads, None).unwrap();
    assert_eq!(partitions[0].tails(), &["A".to_string()]);
}

#[test]
fn merge_colors_fuses_revision_ids() {
    assert_eq!(
        merge_colors(&SetColor::Fused(vec![1]), &SetColor::Fused(vec![2, 1])),
        SetColor::Fused(vec![1, 2])
    );
    assert_eq!(
        merge_colors(&SetColor::Open, &SetColor::Fused(vec![3])),
        SetColor::Fused(vec![3])
    );
    assert_eq!(
        merge_colors(&SetColor::Fused(vec![3]), &SetColor::Open),
        SetColor::Fused(vec![3])
    );
}

#[test]
fn wait_counts_skip_unknown_hashes() {
    let graph = corpus_graph("A>B");
    let partition = Partition {
        hashes: vec!["A".to_string(), "BOGUS".to_string()],
        heads: Vec::new(),
        tails: Vec::new(),
        waypoints: Vec::new(),
    };
    let membership = partition_membership(&partition);
    assert!(set_wait_counts(&graph, &partition, &membership).is_empty());
}

#[test]
fn set_query_reports_unreached_wait_nodes() {
    // The merge's second parent is cut off from the tails, so its
    // wait count never drains.
    let graph = corpus_graph("T>A U>B A>M B>M");
    let partition = Partition {
        hashes: ["T", "A", "U", "B", "M"]
            .iter()
            .map(|hash| (*hash).to_string())
            .collect(),
        heads: vec!["M".to_string()],
        tails: vec!["T".to_string()],
        waypoints: Vec::new(),
    };
    let mut counter = 1;
    assert_eq!(
        set_query(&graph, &partition, &HashMap::new(), &mut counter).unwrap_err(),
        "Did not reach every wait node??"
    );
}

fn blank_set(set_id: u64, hashes: &[&str]) -> CommitSet {
    CommitSet {
        set_id,
        color: SetColor::Open,
        hashes: hashes.iter().map(|hash| (*hash).to_string()).collect(),
        parent_hashes: Vec::new(),
        child_hashes: Vec::new(),
        parent_sets: Vec::new(),
        child_sets: Vec::new(),
        display_child_sets: Vec::new(),
    }
}

#[test]
fn link_sets_dedups_and_skips_unknown() {
    let graph = corpus_graph("A>B A>C A2>C");
    let mut sets = vec![blank_set(1, &["B", "C"]), blank_set(2, &["BOGUS"])];
    link_sets(&graph, &mut sets);
    // `A` crosses twice but lists once; neither `A` nor `A2` lives
    // in a set, so no set ids link.
    assert_eq!(
        sets[0].parent_hashes(),
        &["A".to_string(), "A2".to_string()]
    );
    assert!(sets[0].parent_sets().is_empty());
    assert!(sets[0].child_hashes().is_empty());
    // Hashes with no graph node contribute no edges.
    assert!(sets[1].parent_hashes().is_empty());
    assert!(sets[1].child_hashes().is_empty());
}

#[test]
fn display_layout_defers_unready_children() {
    fn linked(set_id: u64, parents: &[u64], children: &[u64]) -> CommitSet {
        let mut set = blank_set(set_id, &[]);
        set.parent_sets = parents.to_vec();
        set.child_sets = children.to_vec();
        set
    }
    // A diamond: `D` visits (and defers) under `B` before `C` is
    // ready, then lands under `C`.
    let mut sets = vec![
        linked(1, &[], &[2, 3]),
        linked(2, &[1], &[4]),
        linked(3, &[1], &[4]),
        linked(4, &[2, 3], &[]),
    ];
    build_display_layout(&mut sets);
    assert_eq!(sets[0].display_child_sets(), &[2u64, 3]);
    assert!(sets[1].display_child_sets().is_empty());
    assert_eq!(sets[2].display_child_sets(), &[4u64]);
    assert!(sets[3].display_child_sets().is_empty());
    // Revisiting a laid-out set reports it done.
    let mut visited = HashMap::from([(1u64, ())]);
    assert!(visit_display_set(&mut sets, &mut visited, 0));
}

#[test]
fn natural_cmp_compares_fractional_runs() {
    use std::cmp::Ordering;
    // Leading zeros compare left-aligned: the shorter run, then the
    // first differing digit, decides.
    assert_eq!(natural_cmp(b"a0", b"a00", true), Ordering::Less);
    assert_eq!(natural_cmp(b"a00", b"a0", true), Ordering::Greater);
    assert_eq!(natural_cmp(b"a02", b"a01", true), Ordering::Greater);
    assert_eq!(natural_cmp(b"a01", b"a02", true), Ordering::Less);
}

#[test]
fn natural_cmp_orders_exhausted_sides() {
    use std::cmp::Ordering;
    assert_eq!(natural_cmp(b"a", b"ab", true), Ordering::Less);
    assert_eq!(natural_cmp(b"ab", b"a", true), Ordering::Greater);
}

#[test]
fn ingest_links_record_parents() {
    let mut graph = CommitGraph::new();
    let mut seen = OrderedMap::new();
    ingest_log_output(
        &mut graph,
        &mut seen,
        "e\x02child\x02mum dad\x0242\x02m",
        None,
        git_log_record,
    )
    .unwrap();
    assert_eq!(seen.keys(), &["child".to_string()]);
    let node = graph.node("child").unwrap();
    assert_eq!(node.parents(), &["mum".to_string(), "dad".to_string()]);
    assert_eq!(node.epoch(), Some(42));
    assert_eq!(
        graph.node("mum").unwrap().children(),
        &["child".to_string()]
    );
    assert_eq!(
        graph.node("dad").unwrap().children(),
        &["child".to_string()]
    );
}
