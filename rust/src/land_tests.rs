use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_string()).collect()
}

fn sample_plan() -> Plan {
    Plan {
        branch: "feature".to_string(),
        onto: "master".to_string(),
        base: "base-sha".to_string(),
        head: "head-sha".to_string(),
        commits: vec![
            Commit {
                sha: "aaa1111aaa1111aaa1111aaa1111aaa1111aaa1".to_string(),
                subject: "add a".to_string(),
            },
            Commit {
                sha: "bbb2222bbb2222bbb2222bbb2222bbb2222bbb2".to_string(),
                subject: "add b".to_string(),
            },
        ],
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(
        parse_args(&args(&[])),
        Ok(Options {
            onto: "master".to_string(),
            preview: false,
            push: false,
        })
    );
}

#[test]
fn parses_onto_forms_and_flags() {
    assert_eq!(
        parse_args(&args(&["--onto", "stable"])).unwrap().onto,
        "stable"
    );
    assert_eq!(
        parse_args(&args(&["--onto=stable"])).unwrap().onto,
        "stable"
    );
    let opts = parse_args(&args(&["--preview", "--push", "--onto", "x"])).unwrap();
    assert!(opts.preview && opts.push && opts.onto == "x");
}

#[test]
fn rejects_bad_arguments() {
    assert!(parse_args(&args(&["--onto"])).is_err());
    assert!(parse_args(&args(&["--onto="])).is_err());
    assert!(parse_args(&args(&["--onto", "--preview"])).is_err());
    let err = parse_args(&args(&["--bogus"])).unwrap_err();
    assert!(err.contains("unknown flag"), "unexpected: {err}");
    let err = parse_args(&args(&["feature"])).unwrap_err();
    assert!(err.contains("unexpected argument"), "unexpected: {err}");
}

#[test]
fn detects_help() {
    assert!(wants_help(&args(&["--help"])));
    assert!(wants_help(&args(&["-h"])));
    assert!(wants_help(&args(&["help"])));
    assert!(!wants_help(&args(&[])));
    assert!(!wants_help(&args(&["--preview"])));
}

#[test]
fn preview_names_target_base_head_and_commits() {
    let plan = sample_plan();
    let text = format_preview(&plan);
    assert!(text.contains("'feature'"), "missing branch: {text}");
    assert!(text.contains("'master'"), "missing onto: {text}");
    assert!(text.contains("base-sha"), "missing base: {text}");
    assert!(text.contains("head-sha"), "missing head: {text}");
    assert!(
        text.contains("add a") && text.contains("add b"),
        "missing commits: {text}"
    );
}

#[test]
fn message_names_branch_and_lists_commits() {
    let plan = sample_plan();
    let message = build_message(&plan);
    let first: String = message.lines().take(1).collect();
    assert!(
        first.contains("feature"),
        "subject must name branch: {message}"
    );
    assert!(
        message.contains("add a") && message.contains("add b"),
        "missing commits: {message}"
    );
}

#[test]
fn usage_covers_options_and_limits() {
    let text = usage();
    assert!(text.contains("--onto") && text.contains("--preview") && text.contains("--push"));
    assert!(text.contains("remote-tracking"));
}
