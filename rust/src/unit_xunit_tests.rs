use super::*;
use arc_rs::unit_engine::{
    CapturedOutput, CommandRunner, EngineContext, RESULT_FAIL, RESULT_PASS, RESULT_SKIP,
    RESULT_UNSOUND,
};

fn test_env() -> XunitEnvironment {
    XunitEnvironment {
        build_engine: "xbuild".to_string(),
        runtime_engine: "mono".to_string(),
        test_engine: "xunit".to_string(),
        project_root: "/r".to_string(),
        discovery_rules: vec![DiscoveryRule {
            regex: "(.*)".to_string(),
            targets: vec![DiscoveryTarget {
                project: "$1.Tests.csproj".to_string(),
                assembly: "$1.Tests.dll".to_string(),
            }],
        }],
    }
}

#[test]
fn environment_selection_and_errors() {
    let exists = |b: &str| matches!(b, "xbuild" | "mono" | "xunit.console.clr4.exe");
    let resolve = |b: &str| format!("/bin/{b}");
    let file = |_: &str| false;
    let rules = vec![];
    let env = load_environment("/r", false, &exists, &resolve, Some(rules), None, &file).unwrap();
    assert_eq!(env.build_engine, "xbuild");
    assert_eq!(env.runtime_engine, "/bin/mono");
    assert_eq!(env.test_engine, "xunit.console.clr4.exe");

    let win = load_environment("/r", true, &exists, &resolve, Some(vec![]), None, &file).unwrap();
    assert_eq!(win.runtime_engine, "");

    let no_build = |_: &str| false;
    let err =
        load_environment("/r", true, &no_build, &resolve, Some(vec![]), None, &file).unwrap_err();
    assert!(
        err.message()
            .contains("Unable to find msbuild or xbuild in PATH!"),
        "{}",
        err.message()
    );

    let no_mono = |b: &str| b == "msbuild";
    let err =
        load_environment("/r", false, &no_mono, &resolve, Some(vec![]), None, &file).unwrap_err();
    assert!(
        err.message().contains("Unable to find Mono"),
        "{}",
        err.message()
    );

    let err = load_environment("/r", true, &exists, &resolve, None, None, &file).unwrap_err();
    assert!(
        err.message().contains("unit.csharp.discovery"),
        "{}",
        err.message()
    );

    let hinted = load_environment(
        "/r",
        true,
        &exists,
        &resolve,
        Some(vec![]),
        Some("tools/xunit.exe"),
        &|p: &str| p == "/r/tools/xunit.exe",
    )
    .unwrap();
    assert_eq!(hinted.test_engine, "/r/tools/xunit.exe");

    let no_runner = |b: &str| b == "msbuild";
    let err = load_environment(
        "/r",
        true,
        &no_runner,
        &resolve,
        Some(vec![]),
        None,
        &|_: &str| false,
    )
    .unwrap_err();
    assert!(
        err.message().contains("unit.csharp.xunit.binary"),
        "{}",
        err.message()
    );
}

#[test]
fn discovery_match_and_replace() {
    let captures = discovery_match("src/(.*)/file", "src/Foo/file").unwrap();
    assert_eq!(captures, vec!["Foo".to_string()]);
    assert_eq!(
        discovery_replace("$1.Tests/$1.dll", &captures),
        "Foo.Tests/Foo.dll"
    );
    assert_eq!(discovery_replace("${1}.dll", &captures), "Foo.dll");
    assert!(discovery_match("src/", "lib/x").is_none());
    assert!(discovery_match("lib/", "lib/x").is_some());
}

#[test]
fn map_paths_expands_and_dedupes() {
    let env = test_env();
    let exists = |p: &str| p == "/r/App.Tests.csproj";
    let projects = map_paths_to_results(&["App".to_string(), "App".to_string()], &env, &exists);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].project, "/r/App.Tests.csproj");
    assert_eq!(projects[0].assembly, "/r/App.Tests.dll");
    let missing = map_paths_to_results(&["App".to_string()], &env, &|_: &str| false);
    assert!(missing.is_empty());
}

#[test]
fn run_all_tests_short_circuits_failing_stages() {
    let projects = vec![TestProject {
        project: "p".to_string(),
        assembly: "a".to_string(),
    }];
    let mut pass = UnitResult::new();
    pass.set_name("g").unwrap();
    pass.set_result(RESULT_PASS);
    let mut fail = UnitResult::new();
    fail.set_name("b").unwrap();
    fail.set_result(RESULT_FAIL);
    assert!(results_contain_failures(&[pass.clone(), fail.clone()]));
    assert!(!results_contain_failures(&[pass.clone()]));

    let mut calls = Vec::new();
    let out = run_all_tests(
        &projects,
        &|| vec![pass.clone()],
        &|_| vec![fail.clone()],
        &|_| {
            panic!("test stage must not run after a failed build");
        },
    );
    assert!(results_contain_failures(&out));
    assert_eq!(out.len(), 2);
    calls.push(out.len());

    let out = run_all_tests(
        &[],
        &|| vec![pass.clone()],
        &|_| vec![pass.clone()],
        &|_| vec![pass.clone()],
    );
    assert!(out.is_empty());
    assert_eq!(calls, vec![2]);
}

#[test]
fn platform_argv_and_naming() {
    assert_eq!(detect_platform(true, &[]), "Windows");
    assert_eq!(detect_platform(false, &[]), "Linux");
    assert_eq!(detect_platform(false, &["App.Mac.sln"]), "Mac");
    assert!(should_generate(true, true));
    assert!(!should_generate(false, true));
    assert_eq!(
        build_regenerate_argv("mono", "Linux"),
        vec!["mono", "Protobuild.exe", "--resync", "Linux"]
    );
    assert_eq!(
        build_regenerate_argv("", "Windows"),
        vec!["Protobuild.exe", "--resync", "Windows"]
    );
    assert_eq!(
        regenerate_result_name("Linux"),
        "(regenerate projects for Linux)"
    );
    assert_eq!(
        build_build_argv("xbuild"),
        vec!["xbuild", "/p:SkipTestsOnBuild=True"]
    );
    assert_eq!(build_result_name("P"), "(build) P");
    assert_eq!(
        build_test_argv("", "xunit", "A.dll", "r.results.xml"),
        vec!["xunit", "A.dll", "/xml", "r.results.xml"]
    );
    assert_eq!(xml_report_name("abcdefghij"), "abcdefghij.results.xml");
    assert_eq!(xunit_output_path("/r", "x.xml", false), "/r/x.xml");
    assert_eq!(xunit_output_path("C:\\r", "x.xml", true), "C:\\r\\x.xml");
    assert_eq!(parse_coverage_result(), None);
    assert_eq!(stage_outcome(0), StageOutcome::Pass);
    assert_eq!(stage_outcome(1), StageOutcome::Fail);
    assert_eq!(stage_outcome(2), StageOutcome::HardError);
    let ok = stage_result("(build) P", 0, "", 1.0).unwrap();
    assert_eq!(ok.get_result(), RESULT_PASS);
    let failed = stage_result("(build) P", 1, "log", 1.0).unwrap();
    assert_eq!(failed.get_result(), RESULT_FAIL);
    assert_eq!(failed.get_user_data(), Some("log"));
    assert!(stage_result("(build) P", 9, "", 1.0).is_err());
}

#[test]
fn assembly_results_map_statuses_and_details() {
    let xml = r#"<assembly>
      <test name="A.t1" time="0.1" result="Pass"/>
      <test name="A.t2" time="0.2" result="Fail"><failure><message>bad</message><stack-trace>at A</stack-trace></failure></test>
      <test name="A.t3" time="0" result="Skip"><reason><message>why</message></reason></test>
      <test name="A.t4" time="0" result="Weird"/>
    </assembly>"#;
    let results = parse_assembly_results(xml, None).unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].get_result(), RESULT_PASS);
    assert_eq!(results[0].get_duration(), Some(0.1));
    assert_eq!(results[1].get_result(), RESULT_FAIL);
    assert_eq!(results[1].get_user_data(), Some("bad\nat A"));
    assert_eq!(results[2].get_result(), RESULT_SKIP);
    assert_eq!(results[2].get_user_data(), Some("why"));
    assert_eq!(results[3].get_result(), RESULT_UNSOUND);
}

#[test]
fn run_selection_and_missing_reports() {
    let ctx_paths = vec!["a".to_string()];
    let all = vec!["a".to_string(), "b".to_string()];
    assert_eq!(select_run_paths(true, &ctx_paths, &all), all);
    assert_eq!(select_run_paths(false, &ctx_paths, &all), ctx_paths);
    assert!(should_parse_output(true));
    assert!(!should_parse_output(false));
}

#[test]
fn test_assemblies_parse_and_clean_reports() {
    let engine = XunitEngine::new(test_env(), false);
    assert!(engine.supports_run_all_tests());
    assert_eq!(engine.engine_configuration_name(), None);
    let projects = vec![TestProject {
        project: "/r/App.Tests.csproj".to_string(),
        assembly: "/r/App.Tests.dll".to_string(),
    }];
    struct Runner;
    impl CommandRunner for Runner {
        fn run(&self, argv: &[String], _cwd: &str) -> CapturedOutput {
            assert!(argv.contains(&"/xml".to_string()));
            CapturedOutput::new(0, "", "")
        }
    }
    let xml = "<assembly><test name=\"A.t\" time=\"1\" result=\"Pass\"/></assembly>";
    let results = engine
        .test_assemblies_with(
            &projects,
            &Runner,
            &|| " fixedname".trim().to_string(),
            &|_| Some(xml.to_string()),
            &|_| true,
            &|_| {},
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get_name(), "A.t");
    // Missing reports contribute nothing.
    let results = engine
        .test_assemblies_with(
            &projects,
            &Runner,
            &|| "x".to_string(),
            &|_| None,
            &|_| false,
            &|_| {},
        )
        .unwrap();
    assert!(results.is_empty());
    let ctx = EngineContext::new("/r");
    assert!(describe_run(&ctx, &projects).contains("projects 1"));
}
