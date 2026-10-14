use super::*;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn info(name: &str, description: &str) -> LinterInfo {
    LinterInfo {
        name: name.to_string(),
        description: description.to_string(),
    }
}

fn options() -> Options {
    Options {
        verbose: false,
        search: Vec::new(),
        exact: Vec::new(),
    }
}

#[test]
fn parses_defaults() {
    assert_eq!(
        parse_args(&args(&[])),
        ParseOutcome::Args(Options {
            verbose: false,
            search: Vec::new(),
            exact: Vec::new(),
        })
    );
}

#[test]
fn parses_verbose_search_and_exact() {
    assert_eq!(
        parse_args(&args(&["--verbose", "--search", "a", "--search=b", "text"])),
        ParseOutcome::Args(Options {
            verbose: true,
            search: vec!["a".to_string(), "b".to_string()],
            exact: vec!["text".to_string()],
        })
    );
}

#[test]
fn parses_double_dash_and_help() {
    assert_eq!(
        parse_args(&args(&["--", "--verbose"])),
        ParseOutcome::Args(Options {
            verbose: false,
            search: Vec::new(),
            exact: vec!["--verbose".to_string()],
        })
    );
    assert_eq!(parse_args(&args(&["--help"])), ParseOutcome::Help);
    assert_eq!(parse_args(&args(&["-h"])), ParseOutcome::Help);
}

#[test]
fn rejects_missing_search_value_and_unknown_flags() {
    assert_eq!(
        parse_args(&args(&["--search"])),
        ParseOutcome::Error("--search requires a value".to_string())
    );
    assert_eq!(
        parse_args(&args(&["--bogus"])),
        ParseOutcome::Error("unknown flag '--bogus'".to_string())
    );
}

#[test]
fn registry_lists_txt_linter_with_one_line() {
    let all = available_linters();
    assert!(!all.is_empty());
    let text = all.iter().find(|info| info.name == "TXT").unwrap();
    assert!(!text.description.is_empty());
    assert!(!text.description.contains('\n'));
    let mut names: Vec<&str> = all.iter().map(|info| info.name.as_str()).collect();
    names.sort_unstable();
    let listed: Vec<&str> = all.iter().map(|info| info.name.as_str()).collect();
    assert_eq!(listed, names);
}

#[test]
fn selects_exact_names_case_insensitively() {
    let all = vec![
        info("text", "plain text rules"),
        info("other", "more rules"),
    ];
    let mut opts = options();
    opts.exact = vec!["TEXT".to_string()];
    let selected = select_linters(&all, &opts).unwrap();
    assert_eq!(selected, vec![info("text", "plain text rules")]);
}

#[test]
fn selects_no_exact_match_as_empty() {
    let all = vec![info("text", "plain text rules")];
    let mut opts = options();
    opts.exact = vec!["missing".to_string()];
    assert!(select_linters(&all, &opts).unwrap().is_empty());
}

#[test]
fn searches_names_and_descriptions_case_insensitively() {
    let all = vec![
        info("text", "plain text rules"),
        info("other", "more rules"),
    ];
    let mut opts = options();
    opts.search = vec!["PLAIN".to_string()];
    assert_eq!(
        select_linters(&all, &opts).unwrap(),
        vec![info("text", "plain text rules")]
    );
    opts.search = vec!["nomatch".to_string()];
    assert!(select_linters(&all, &opts).unwrap().is_empty());
}

#[test]
fn rejects_exact_and_search_together() {
    let all = vec![info("text", "plain text rules")];
    let mut opts = options();
    opts.exact = vec!["text".to_string()];
    opts.search = vec!["text".to_string()];
    assert_eq!(
        select_linters(&all, &opts),
        Err("Specify either search expression or exact name".to_string())
    );
}

#[test]
fn renders_plain_verbose_and_no_match() {
    let all = vec![info("text", "plain text rules")];
    let plain = render(&all, &options());
    assert!(plain.contains("text - plain text rules"));
    assert!(plain.contains("arc-rs linters --verbose"));

    let mut verbose = options();
    verbose.verbose = true;
    let details = render(&all, &verbose);
    assert!(details.contains("text\n"));
    assert!(details.contains("plain text rules"));
    assert!(!details.contains("for more details"));

    let mut exact = options();
    exact.exact = vec!["missing".to_string()];
    let missing = render(&[], &exact);
    assert!(missing.contains("No match found"));
    assert!(missing.contains("--search missing"));
}

#[test]
fn run_reports_help_usage_conflicts_and_success() {
    assert_eq!(run(&args(&["--help"])), std::process::ExitCode::SUCCESS);
    assert_eq!(run(&args(&["--bogus"])), std::process::ExitCode::FAILURE);
    assert_eq!(
        run(&args(&["text", "--search", "text"])),
        std::process::ExitCode::FAILURE
    );
    assert_eq!(run(&args(&[])), std::process::ExitCode::SUCCESS);
    assert_eq!(
        run(&args(&["no-such-linter"])),
        std::process::ExitCode::SUCCESS
    );
}
