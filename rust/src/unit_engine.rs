//! Shared unit-test engine base: result model, engine trait, parsers.
//!
//! PHP references: `src/unit/ArcanistUnitTestResult.php` (result accessors,
//! coverage merge, result-code specs), `src/unit/engine/ArcanistUnitTestEngine.php`
//! (engine base), `src/unit/parser/ArcanistXUnitTestResultParser.php` (JUnit
//! `<testcase>` parsing shared by the nose/pytest engines), and
//! `src/workflow/ArcanistUnitWorkflow.php` (result-shape contract: engine
//! results must be a list of result objects, overall exit mapping, coverage
//! aggregation).
//!
//! Divergences from PHP (each cites the PHP line it relaxes):
//! - XML is parsed with a dependency-free mini parser instead of
//!   `DOMDocument` (`ArcanistXUnitTestResultParser.php:26`
//!   `$xunit_dom->loadXML(...)`). Only the tags/attributes the engines read
//!   are supported; malformed input that does not start with `<` is reported
//!   as a load failure.
//! - JSON is parsed with a dependency-free mini parser instead of
//!   `phutil_json_decode`. Namespaces, DOCTYPEs, entities beyond the five
//!   predefined XML entities, and duplicate attributes are not supported.
//! - `setDuration` takes `f64` only; PHP (`ArcanistUnitTestResult.php:86-94`)
//!   accepts int|float and throws otherwise. The Rust type makes the throw
//!   unreachable.
//! - `extraData` values are strings; PHP (`ArcanistUnitTestResult.php:114-121`)
//!   allows wild values.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `ArcanistUnitTestResult::RESULT_PASS` (`src/unit/ArcanistUnitTestResult.php:8`).
pub const RESULT_PASS: &str = "pass";
/// `ArcanistUnitTestResult::RESULT_FAIL` (`src/unit/ArcanistUnitTestResult.php:9`).
pub const RESULT_FAIL: &str = "fail";
/// `ArcanistUnitTestResult::RESULT_SKIP` (`src/unit/ArcanistUnitTestResult.php:10`).
pub const RESULT_SKIP: &str = "skip";
/// `ArcanistUnitTestResult::RESULT_BROKEN` (`src/unit/ArcanistUnitTestResult.php:11`).
pub const RESULT_BROKEN: &str = "broken";
/// `ArcanistUnitTestResult::RESULT_UNSOUND` (`src/unit/ArcanistUnitTestResult.php:12`).
pub const RESULT_UNSOUND: &str = "unsound";

/// `ArcanistUnitWorkflow::RESULT_OKAY` (`src/workflow/ArcanistUnitWorkflow.php:8`).
pub const WORKFLOW_RESULT_OKAY: i32 = 0;
/// `ArcanistUnitWorkflow::RESULT_UNSOUND` (`src/workflow/ArcanistUnitWorkflow.php:9`).
pub const WORKFLOW_RESULT_UNSOUND: i32 = 1;
/// `ArcanistUnitWorkflow::RESULT_FAIL` (`src/workflow/ArcanistUnitWorkflow.php:10`).
pub const WORKFLOW_RESULT_FAIL: i32 = 2;
/// `ArcanistUnitWorkflow::RESULT_SKIP` (`src/workflow/ArcanistUnitWorkflow.php:11`).
pub const WORKFLOW_RESULT_SKIP: i32 = 3;

/// Maximum `name` length in bytes (`src/unit/ArcanistUnitTestResult.php:33`).
pub const MAX_NAME_BYTES: usize = 255;

/// Outcome of running one unit test.
///
/// Mirrors `ArcanistUnitTestResult` (`src/unit/ArcanistUnitTestResult.php:6-22`):
/// `namespace`, `name`, `link`, `result`, `duration`, `userData`, `extraData`,
/// `coverage`, with the same accessors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnitResult {
    namespace: Option<String>,
    name: String,
    link: Option<String>,
    result: String,
    duration: Option<f64>,
    user_data: Option<String>,
    extra_data: Option<HashMap<String, String>>,
    coverage: Option<HashMap<String, String>>,
}

impl UnitResult {
    /// New empty result, mirroring `new ArcanistUnitTestResult()`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `setNamespace` (`src/unit/ArcanistUnitTestResult.php:23-26`).
    pub fn set_namespace(&mut self, namespace: &str) -> &mut Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// `getNamespace` (`src/unit/ArcanistUnitTestResult.php:28-30`).
    #[must_use]
    pub fn get_namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// `setName` (`src/unit/ArcanistUnitTestResult.php:32-49`): names longer
    /// than 255 bytes are an error, mirroring the PHP exception.
    pub fn set_name(&mut self, name: &str) -> Result<&mut Self, String> {
        let actual = name.len();
        if actual > MAX_NAME_BYTES {
            return Err(format!(
                "Parameter (\"{name}\") passed to \"setName()\" when constructing a unit test \
                 message must be a string with a maximum length of {MAX_NAME_BYTES} bytes, but \
                 is {actual} bytes in length."
            ));
        }
        self.name = name.to_string();
        Ok(self)
    }

    /// `getName` (`src/unit/ArcanistUnitTestResult.php:51-53`).
    #[must_use]
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// `setLink` (`src/unit/ArcanistUnitTestResult.php:55-58`).
    pub fn set_link(&mut self, link: &str) -> &mut Self {
        self.link = Some(link.to_string());
        self
    }

    /// `getLink` (`src/unit/ArcanistUnitTestResult.php:60-62`).
    #[must_use]
    pub fn get_link(&self) -> Option<&str> {
        self.link.as_deref()
    }

    /// `setResult` (`src/unit/ArcanistUnitTestResult.php:64-67`).
    pub fn set_result(&mut self, result: &str) -> &mut Self {
        self.result = result.to_string();
        self
    }

    /// `getResult` (`src/unit/ArcanistUnitTestResult.php:69-71`).
    #[must_use]
    pub fn get_result(&self) -> &str {
        &self.result
    }

    /// `setDuration` (`src/unit/ArcanistUnitTestResult.php:86-94`): seconds as
    /// a float. See the module docs for the int|float Divergence.
    pub fn set_duration(&mut self, duration: f64) -> &mut Self {
        self.duration = Some(duration);
        self
    }

    /// `getDuration` (`src/unit/ArcanistUnitTestResult.php:97-99`).
    #[must_use]
    pub fn get_duration(&self) -> Option<f64> {
        self.duration
    }

    /// `setUserData` (`src/unit/ArcanistUnitTestResult.php:101-104`).
    pub fn set_user_data(&mut self, user_data: &str) -> &mut Self {
        self.user_data = Some(user_data.to_string());
        self
    }

    /// `getUserData` (`src/unit/ArcanistUnitTestResult.php:106-108`).
    #[must_use]
    pub fn get_user_data(&self) -> Option<&str> {
        self.user_data.as_deref()
    }

    /// `setExtraData` (`src/unit/ArcanistUnitTestResult.php:114-117`).
    pub fn set_extra_data(&mut self, extra_data: HashMap<String, String>) -> &mut Self {
        self.extra_data = Some(extra_data);
        self
    }

    /// `getExtraData` (`src/unit/ArcanistUnitTestResult.php:119-121`).
    #[must_use]
    pub fn get_extra_data(&self) -> Option<&HashMap<String, String>> {
        self.extra_data.as_ref()
    }

    /// `setCoverage` (`src/unit/ArcanistUnitTestResult.php:123-126`).
    pub fn set_coverage(&mut self, coverage: HashMap<String, String>) -> &mut Self {
        self.coverage = Some(coverage);
        self
    }

    /// `getCoverage` (`src/unit/ArcanistUnitTestResult.php:128-130`).
    #[must_use]
    pub fn get_coverage(&self) -> Option<&HashMap<String, String>> {
        self.coverage.as_ref()
    }

    /// `mergeCoverage` (`src/unit/ArcanistUnitTestResult.php:138-162`): merge
    /// several per-file coverage strings; any `C` wins, longer tails are kept.
    #[must_use]
    pub fn merge_coverage(coverage: &[String]) -> Option<String> {
        if coverage.is_empty() {
            return None;
        }
        let mut bytes = coverage[0].clone().into_bytes();
        for more in &coverage[1..] {
            let more_bytes = more.as_bytes();
            for (slot, &byte) in bytes.iter_mut().zip(more_bytes.iter()) {
                if byte == b'C' {
                    *slot = b'C';
                }
            }
            if more_bytes.len() > bytes.len() {
                bytes.extend_from_slice(&more_bytes[bytes.len()..]);
            }
        }
        // Coverage maps are ASCII byte runs; merging only writes ASCII `C`,
        // so valid input stays valid UTF-8.
        Some(String::from_utf8(bytes).expect("coverage maps must be ASCII"))
    }

    /// `toDictionary` (`src/unit/ArcanistUnitTestResult.php:164-175`).
    #[must_use]
    pub fn to_dictionary(&self) -> UnitDictionary {
        UnitDictionary {
            namespace: self.namespace.clone(),
            name: self.name.clone(),
            link: self.link.clone(),
            result: self.result.clone(),
            duration: self.duration,
            extra: self.extra_data.as_ref().map(sorted_pairs),
            user_data: self.user_data.clone(),
            coverage: self.coverage.as_ref().map(sorted_pairs),
        }
    }

    /// `getAllResultCodes` (`src/unit/ArcanistUnitTestResult.php:177-185`).
    #[must_use]
    pub fn get_all_result_codes() -> Vec<&'static str> {
        vec![
            RESULT_PASS,
            RESULT_FAIL,
            RESULT_SKIP,
            RESULT_BROKEN,
            RESULT_UNSOUND,
        ]
    }

    /// `getResultCodeName` (`src/unit/ArcanistUnitTestResult.php:187-193`).
    #[must_use]
    pub fn get_result_code_name(result_code: &str) -> Option<&'static str> {
        get_result_code_spec(result_code).map(|(name, _)| name)
    }

    /// `getResultCodeDescription` (`src/unit/ArcanistUnitTestResult.php:195-201`).
    #[must_use]
    pub fn get_result_code_description(result_code: &str) -> Option<&'static str> {
        get_result_code_spec(result_code).map(|(_, description)| description)
    }
}

/// `getResultCodeSpecs` (`src/unit/ArcanistUnitTestResult.php:208-238`).
fn get_result_code_spec(result_code: &str) -> Option<(&'static str, &'static str)> {
    match result_code {
        RESULT_PASS => Some(("Pass", "The test passed.")),
        RESULT_FAIL => Some(("Fail", "The test failed.")),
        RESULT_SKIP => Some(("Skip", "The test was not executed.")),
        RESULT_BROKEN => Some((
            "Broken",
            "The test failed in an abnormal or severe way. For example, the harness crashed \
             instead of reporting a failure.",
        )),
        RESULT_UNSOUND => Some((
            "Unsound",
            "The test failed, but this change is probably not what broke it. For example, it \
             might have already been failing.",
        )),
        _ => None,
    }
}

fn sorted_pairs(map: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> =
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    pairs.sort();
    pairs
}

/// Dictionary form of a result, mirroring `toDictionary` keys
/// (`src/unit/ArcanistUnitTestResult.php:164-175`): `namespace`, `name`,
/// `link`, `result`, `duration`, `extra`, `userData`, `coverage`.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitDictionary {
    /// `namespace` key.
    pub namespace: Option<String>,
    /// `name` key.
    pub name: String,
    /// `link` key.
    pub link: Option<String>,
    /// `result` key.
    pub result: String,
    /// `duration` key.
    pub duration: Option<f64>,
    /// `extra` key.
    pub extra: Option<Vec<(String, String)>>,
    /// `userData` key.
    pub user_data: Option<String>,
    /// `coverage` key.
    pub coverage: Option<Vec<(String, String)>>,
}

impl UnitDictionary {
    /// Encode as a JSON object with the PHP `toDictionary` keys, in PHP order.
    #[must_use]
    pub fn to_json_string(&self) -> String {
        let mut out = String::from("{");
        out.push_str(&format!(
            "\"namespace\":{},",
            json_string_or_null(self.namespace.as_deref())
        ));
        out.push_str(&format!("\"name\":{},", json_escape(&self.name)));
        out.push_str(&format!(
            "\"link\":{},",
            json_string_or_null(self.link.as_deref())
        ));
        out.push_str(&format!("\"result\":{},", json_escape(&self.result)));
        out.push_str(&format!(
            "\"duration\":{},",
            self.duration
                .map_or_else(|| "null".to_string(), |d| format!("{d:?}"))
        ));
        out.push_str(&format!(
            "\"extra\":{},",
            json_map_or_null(self.extra.as_deref())
        ));
        out.push_str(&format!(
            "\"userData\":{},",
            json_string_or_null(self.user_data.as_deref())
        ));
        out.push_str(&format!(
            "\"coverage\":{}",
            json_map_or_null(self.coverage.as_deref())
        ));
        out.push('}');
        out
    }
}

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_string_or_null(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_string(), json_escape)
}

fn json_map_or_null(pairs: Option<&[(String, String)]>) -> String {
    pairs.map_or_else(
        || "null".to_string(),
        |entries| {
            let body: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}:{}", json_escape(k), json_escape(v)))
                .collect();
            format!("{{{}}}", body.join(","))
        },
    )
}

/// Engine failure modes: `ArcanistNoEffectException` ("no tests"), usage
/// errors, and hard failures (command errors, parse errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// No tests to run (`ArcanistNoEffectException`, e.g.
    /// `src/unit/engine/PhpunitTestEngine.php:45`).
    NoEffect(String),
    /// Bad configuration or invocation (`ArcanistUsageException`).
    Usage(String),
    /// A command or parse failure (`Exception` / `CommandException`).
    Failed(String),
}

impl EngineError {
    /// `new ArcanistNoEffectException(pht('No tests to run.'))`.
    #[must_use]
    pub fn no_effect() -> Self {
        Self::NoEffect("No tests to run.".to_string())
    }

    /// The error message.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::NoEffect(message) | Self::Usage(message) | Self::Failed(message) => message,
        }
    }

    /// True for `NoEffect`, which the configuration-driven engine swallows
    /// per engine (`src/unit/engine/ArcanistConfigurationDrivenUnitTestEngine.php:158-160`).
    #[must_use]
    pub fn is_no_effect(&self) -> bool {
        matches!(self, Self::NoEffect(_))
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for EngineError {}

/// Engine inputs: working copy root, target paths, coverage flag, run-all flag.
///
/// Mirrors the `ArcanistUnitTestEngine` accessors
/// (`src/unit/engine/ArcanistUnitTestEngine.php:21-79`): `setRunAllTests` /
/// `getRunAllTests`, `setWorkingCopy` / `getWorkingCopy` (project root only),
/// `setPaths` / `getPaths`, `setEnableCoverage` / `getEnableCoverage`.
/// `enable_coverage: None` means "default", like PHP's `null`
/// (`src/workflow/ArcanistUnitWorkflow.php:161`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineContext {
    /// Project root (`getWorkingCopy()->getProjectRoot()`).
    pub project_root: String,
    /// Target paths (`getPaths()`).
    pub paths: Vec<String>,
    /// Coverage flag (`getEnableCoverage()`); `None` is the default.
    pub enable_coverage: Option<bool>,
    /// Run-all flag (`getRunAllTests()`).
    pub run_all_tests: bool,
}

impl EngineContext {
    /// Context with a project root and no paths.
    #[must_use]
    pub fn new(project_root: &str) -> Self {
        Self {
            project_root: project_root.to_string(),
            paths: Vec::new(),
            enable_coverage: None,
            run_all_tests: false,
        }
    }

    /// `setRunAllTests` (`src/unit/engine/ArcanistUnitTestEngine.php:21-32`):
    /// enabling run-all on an engine without support is an error mirroring
    /// `pht("Engine '%s' does not support %s.", ..., '--everything')`.
    pub fn check_set_run_all_tests(
        supports_run_all_tests: bool,
        engine: &str,
        run_all_tests: bool,
    ) -> Result<bool, String> {
        if !supports_run_all_tests && run_all_tests {
            return Err(format!("Engine '{engine}' does not support --everything."));
        }
        Ok(run_all_tests)
    }
}

/// Unit test engine, mirroring `ArcanistUnitTestEngine`
/// (`src/unit/engine/ArcanistUnitTestEngine.php:6-98`).
pub trait UnitEngine {
    /// `getEngineConfigurationName`
    /// (`src/unit/engine/ArcanistUnitTestEngine.php:17-19`); default `None`.
    fn engine_configuration_name(&self) -> Option<&'static str> {
        None
    }

    /// `supportsRunAllTests`
    /// (`src/unit/engine/ArcanistUnitTestEngine.php:38-40`); default false.
    fn supports_run_all_tests(&self) -> bool {
        false
    }

    /// `shouldEchoTestResults`
    /// (`src/unit/engine/ArcanistUnitTestEngine.php:94-96`); default true.
    fn should_echo_test_results(&self) -> bool {
        true
    }

    /// `run()` (`src/unit/engine/ArcanistUnitTestEngine.php:86`): run the
    /// engine. The workflow contract
    /// (`src/workflow/ArcanistUnitWorkflow.php:358-386`) requires a list of
    /// result objects; every command goes through `runner` so tests can use
    /// canned outputs instead of real tools.
    fn run(
        &self,
        ctx: &EngineContext,
        runner: &dyn CommandRunner,
    ) -> Result<Vec<UnitResult>, EngineError>;
}

/// Captured child-process output: exit code plus stdout/stderr text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOutput {
    /// Process exit code.
    pub exit_code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl CapturedOutput {
    /// Build a captured output triple.
    #[must_use]
    pub fn new(exit_code: i32, stdout: &str, stderr: &str) -> Self {
        Self {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }
}

/// Output-capture seam for engine runs: every external command goes through
/// here, so tests inject canned outputs and never need real tools.
pub trait CommandRunner {
    /// Run `argv` with working directory `cwd`, capturing output.
    fn run(&self, argv: &[String], cwd: &str) -> CapturedOutput;
}

impl<F> CommandRunner for F
where
    F: Fn(&[String], &str) -> CapturedOutput,
{
    fn run(&self, argv: &[String], cwd: &str) -> CapturedOutput {
        self(argv, cwd)
    }
}

/// Overall workflow result for a result list
/// (`src/workflow/ArcanistUnitWorkflow.php:243-253`): any fail/broken wins
/// with `RESULT_FAIL`; otherwise any unsound gives `RESULT_UNSOUND`.
#[must_use]
pub fn overall_result(results: &[UnitResult]) -> i32 {
    let mut overall = WORKFLOW_RESULT_OKAY;
    for result in results {
        let code = result.get_result();
        if code == RESULT_FAIL || code == RESULT_BROKEN {
            overall = WORKFLOW_RESULT_FAIL;
            break;
        } else if code == RESULT_UNSOUND {
            overall = WORKFLOW_RESULT_UNSOUND;
        }
    }
    overall
}

/// Coverage fraction for one merged report string
/// (`src/workflow/ArcanistUnitWorkflow.php:208-214`): `C / (C + U)`, else 0.
#[must_use]
pub fn coverage_fraction(report: &str) -> f64 {
    let covered = report.bytes().filter(|b| *b == b'C').count();
    let uncovered = report.bytes().filter(|b| *b == b'U').count();
    if covered + uncovered > 0 {
        covered as f64 / (covered + uncovered) as f64
    } else {
        0.0
    }
}

/// Build a per-file coverage string from `(line_number, hits)` pairs given in
/// document order, mirroring the nose loop
/// (`src/unit/engine/NoseTestEngine.php:150-174`) shared verbatim by pytest
/// (`src/unit/engine/PytestTestEngine.php:113-137`): `N` before the first
/// reported line, `U` for zero hits, `C` for positive hits, `N` padding after
/// the last reported line (note the PHP `<` quirk on the trailing fill).
#[must_use]
pub fn coverage_string_from_lines(line_count: usize, lines: &[(u32, i64)]) -> String {
    let mut coverage = String::new();
    let mut start_line: u32 = 1;
    for (number, hits) in lines {
        while start_line < *number {
            coverage.push('N');
            start_line += 1;
        }
        if *hits == 0 {
            coverage.push('U');
        } else if *hits > 0 {
            coverage.push('C');
        }
        start_line += 1;
    }
    if (start_line as usize) < line_count {
        for _ in start_line as usize..=line_count {
            coverage.push('N');
        }
    }
    coverage
}

// --- Mini XML ---------------------------------------------------------------

/// One parsed XML element: tag name, attributes, and raw inner markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlElement {
    /// Tag name.
    pub name: String,
    /// `(name, value)` attribute pairs in document order.
    pub attrs: Vec<(String, String)>,
    /// Raw inner markup (empty for self-closing tags).
    pub inner: String,
}

impl XmlElement {
    /// Fetch an attribute value by name.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Strip `<!-- ... -->` comments from XML text.
#[must_use]
pub fn strip_xml_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if xml[i..].starts_with("<!--") {
            if let Some(end) = xml[i..].find("-->") {
                i += end + 3;
            } else {
                break;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Unescape the five predefined XML entities plus numeric character references.
#[must_use]
pub fn unescape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some(semi) = rest.find(';') {
            let entity = &rest[..=semi];
            rest = &rest[semi + 1..];
            match entity {
                "&amp;" => out.push('&'),
                "&lt;" => out.push('<'),
                "&gt;" => out.push('>'),
                "&quot;" => out.push('"'),
                "&apos;" => out.push('\''),
                _ if entity.starts_with("&#") => {
                    let body = &entity[2..entity.len() - 1];
                    let code = if let Some(hex) =
                        body.strip_prefix('x').or_else(|| body.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        body.parse::<u32>().ok()
                    };
                    match code.and_then(char::from_u32) {
                        Some(ch) => out.push(ch),
                        None => out.push_str(entity),
                    }
                }
                _ => out.push_str(entity),
            }
        } else {
            out.push_str(rest);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Text content of inner markup: CDATA sections are unwrapped, nested tags are
/// stripped, entities are unescaped. Mirrors `DOMNode::$nodeValue`.
#[must_use]
pub fn text_content(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            if let Some(end) = after.find("]]>") {
                out.push_str(&after[..end]);
                rest = &after[end + 3..];
            } else {
                out.push_str(after);
                break;
            }
        } else if rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                rest = &rest[end + 1..];
            } else {
                break;
            }
        } else if let Some(lt) = rest.find('<') {
            out.push_str(&rest[..lt]);
            rest = &rest[lt..];
        } else {
            out.push_str(rest);
            break;
        }
    }
    unescape_xml(&out)
}

/// Find top-level elements named `tag`, in document order. Nested elements
/// with the same name are not supported (no engine format nests like tags).
#[must_use]
pub fn find_elements(xml: &str, tag: &str) -> Vec<XmlElement> {
    let cleaned = strip_xml_comments(xml);
    let xml = cleaned.as_str();
    let mut out = Vec::new();
    let mut pos = 0;
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    while let Some(start) = xml[pos..].find(&open) {
        let abs = pos + start;
        let after = abs + open.len();
        let boundary = xml[after..].chars().next();
        match boundary {
            Some(ch) if ch == '>' || ch == '/' || ch.is_whitespace() => {}
            _ => {
                pos = after;
                continue;
            }
        }
        let Some(tag_end) = find_tag_end(&xml[after..]).map(|e| after + e) else {
            break;
        };
        let head = &xml[after..tag_end];
        let self_closing = head.trim_end().ends_with('/');
        let attrs = parse_attrs(head);
        if self_closing {
            out.push(XmlElement {
                name: tag.to_string(),
                attrs,
                inner: String::new(),
            });
            pos = tag_end + 1;
        } else if let Some(end) = xml[tag_end..].find(&close) {
            let inner_start = tag_end + 1;
            let inner_end = tag_end + end;
            out.push(XmlElement {
                name: tag.to_string(),
                attrs,
                inner: xml[inner_start..inner_end].to_string(),
            });
            pos = inner_end + close.len();
        } else {
            break;
        }
    }
    out
}

/// Offset of the `>` closing a tag head, skipping quoted attribute values.
fn find_tag_end(head: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, ch) in head.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(i);
        }
    }
    None
}

/// Parse `name="value"` pairs from a tag head.
fn parse_attrs(head: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let chars: Vec<char> = head.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match scan_unit_attr(&chars, i) {
            UnitAttrScan::Parsed(name, value, next) => {
                attrs.push((name, unescape_xml(&value)));
                i = next;
            }
            UnitAttrScan::Skipped(next) => i = next,
        }
    }
    attrs
}

/// Outcome of scanning one attribute: parsed or skipped (resume at the
/// offset). Valueless names are silently dropped, like the PHP parser.
enum UnitAttrScan {
    Parsed(String, String, usize),
    Skipped(usize),
}

/// Scan one attribute starting at `start` (past leading padding).
fn scan_unit_attr(chars: &[char], start: usize) -> UnitAttrScan {
    let mut i = skip_unit_gap(chars, start);
    let name_start = i;
    while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' && chars[i] != '/' {
        i += 1;
    }
    if name_start == i {
        if i < chars.len() {
            i += 1;
        }
        return UnitAttrScan::Skipped(i);
    }
    let name: String = chars[name_start..i].iter().collect();
    scan_unit_attr_value(chars, name, i)
}

/// Scan the `= "value"` half of an attribute whose name ends at `start`.
fn scan_unit_attr_value(chars: &[char], name: String, start: usize) -> UnitAttrScan {
    let mut i = skip_unit_ws(chars, start);
    if i >= chars.len() || chars[i] != '=' {
        return UnitAttrScan::Skipped(i);
    }
    i += 1;
    i = skip_unit_ws(chars, i);
    if i >= chars.len() || (chars[i] != '"' && chars[i] != '\'') {
        return UnitAttrScan::Skipped(i);
    }
    let quote = chars[i];
    i += 1;
    let value_start = i;
    while i < chars.len() && chars[i] != quote {
        i += 1;
    }
    let value: String = chars[value_start..i].iter().collect();
    if i < chars.len() {
        i += 1;
    }
    UnitAttrScan::Parsed(name, value, i)
}

/// Skip inter-attribute padding (whitespace and `/`).
fn skip_unit_gap(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && (chars[i].is_whitespace() || chars[i] == '/') {
        i += 1;
    }
    i
}

/// Skip whitespace between an attribute name, `=`, and its value.
fn skip_unit_ws(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Parse JUnit/XUnit `<testcase>` results, mirroring
/// `ArcanistXUnitTestResultParser::parseTestResults`
/// (`src/unit/parser/ArcanistXUnitTestResultParser.php:16-99`): empty input is
/// an error, `skipped` maps to skip, `failure` to fail, `error` to broken
/// (later blocks override earlier ones), names are `classname.name`, and
/// failure/error bodies gain a trailing newline.
pub fn parse_xunit_testcases(test_results: &str) -> Result<Vec<UnitResult>, String> {
    if test_results.is_empty() {
        return Err("test_results argument to parseTestResults() must not be empty".to_string());
    }
    if !test_results.trim_start().starts_with('<') {
        let start: String = test_results.chars().take(150).collect();
        return Err(format!(
            "Failed to load XUnit report; Input starts with:\n\n{start}"
        ));
    }
    let mut results = Vec::new();
    for testcase in find_elements(test_results, "testcase") {
        results.push(build_xunit_case_result(&testcase)?);
    }
    Ok(results)
}

/// Build one testcase result: `classname.name`, parsed duration, and the
/// status/user-data resolved from its `skipped`/`failure`/`error` blocks.
fn build_xunit_case_result(testcase: &XmlElement) -> Result<UnitResult, String> {
    let classname = testcase.attr("classname").unwrap_or("");
    let name = testcase.attr("name").unwrap_or("");
    let time = testcase.attr("time").unwrap_or("");
    let (status, user_data) = xunit_case_status(&testcase.inner);
    let mut result = UnitResult::new();
    result.set_name(&format!("{classname}.{name}"))?;
    result.set_result(status);
    result.set_duration(time.parse::<f64>().unwrap_or(0.0));
    result.set_user_data(&user_data);
    Ok(result)
}

/// Resolve a testcase's status from its outcome blocks: `skipped` maps to
/// skip, `failure` to fail, `error` to broken; later blocks override earlier
/// ones, and failure/error bodies gain a trailing newline.
fn xunit_case_status(inner: &str) -> (&'static str, String) {
    let mut status = RESULT_PASS;
    let mut user_data = String::new();
    if let Some(messages) = xunit_block_text(inner, "skipped") {
        status = RESULT_SKIP;
        user_data.push_str(&messages);
    }
    if let Some(messages) = xunit_block_text(inner, "failure") {
        status = RESULT_FAIL;
        user_data.push_str(&messages);
        user_data.push('\n');
    }
    if let Some(messages) = xunit_block_text(inner, "error") {
        status = RESULT_BROKEN;
        user_data.push_str(&messages);
        user_data.push('\n');
    }
    (status, user_data)
}

/// Joined text of a testcase's outcome blocks, or `None` when the testcase
/// has no such block.
fn xunit_block_text(inner: &str, tag: &str) -> Option<String> {
    let blocks = find_elements(inner, tag);
    if blocks.is_empty() {
        return None;
    }
    let messages: Vec<String> = blocks
        .iter()
        .map(|e| {
            text_content(&e.inner)
                .trim_matches(|c| c == ' ' || c == '\n')
                .to_string()
        })
        .collect();
    Some(messages.join("\n"))
}

// --- Mini JSON --------------------------------------------------------------

/// A JSON value, preserving object key order.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// String.
    Str(String),
    /// Number.
    Num(f64),
    /// Array.
    Arr(Vec<JsonValue>),
    /// Object with ordered keys.
    Obj(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// Fetch an object member by key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        match self {
            Self::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Borrow as an object entry list.
    #[must_use]
    pub fn as_obj(&self) -> Option<&[(String, JsonValue)]> {
        match self {
            Self::Obj(entries) => Some(entries),
            _ => None,
        }
    }

    /// Borrow as an array.
    #[must_use]
    pub fn as_arr(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// Borrow as a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// Borrow as a number.
    #[must_use]
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Self::Num(value) => Some(*value),
            _ => None,
        }
    }
}

/// Parse a JSON document (stand-in for `phutil_json_decode`). Returns the
/// value or a human-readable error; trailing data after the document is an
/// error.
pub fn parse_json(text: &str) -> Result<JsonValue, String> {
    let mut parser = JsonParser {
        chars: text.chars().collect(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos != parser.chars.len() {
        return Err("unexpected trailing data after JSON document".to_string());
    }
    Ok(value)
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        if self.pos >= self.chars.len() {
            return Err("unexpected end of JSON input".to_string());
        }
        match self.chars[self.pos] {
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            '"' => Ok(JsonValue::Str(self.parse_string()?)),
            't' => self.parse_literal("true", JsonValue::Bool(true)),
            'f' => self.parse_literal("false", JsonValue::Bool(false)),
            'n' => self.parse_literal("null", JsonValue::Null),
            ch if ch == '-' || ch.is_ascii_digit() => self.parse_number(),
            ch => Err(format!("unexpected character '{ch}' in JSON input")),
        }
    }

    fn parse_literal(&mut self, word: &str, value: JsonValue) -> Result<JsonValue, String> {
        let end = self.pos + word.len();
        if end <= self.chars.len() && self.chars[self.pos..end].iter().collect::<String>() == word {
            self.pos = end;
            Ok(value)
        } else {
            Err(format!("invalid JSON literal near '{}'", &word[..1]))
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.pos += 1;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.pos < self.chars.len() && self.chars[self.pos] == '}' {
            self.pos += 1;
            return Ok(JsonValue::Obj(entries));
        }
        loop {
            self.skip_ws();
            if self.pos >= self.chars.len() || self.chars[self.pos] != '"' {
                return Err("expected string key in JSON object".to_string());
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.pos >= self.chars.len() || self.chars[self.pos] != ':' {
                return Err("expected ':' in JSON object".to_string());
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            if self.pos >= self.chars.len() {
                return Err("unexpected end of JSON object".to_string());
            }
            if self.chars[self.pos] == '}' {
                self.pos += 1;
                return Ok(JsonValue::Obj(entries));
            }
            if self.chars[self.pos] != ',' {
                return Err("expected ',' or '}' in JSON object".to_string());
            }
            self.pos += 1;
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.pos < self.chars.len() && self.chars[self.pos] == ']' {
            self.pos += 1;
            return Ok(JsonValue::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            if self.pos >= self.chars.len() {
                return Err("unexpected end of JSON array".to_string());
            }
            if self.chars[self.pos] == ']' {
                self.pos += 1;
                return Ok(JsonValue::Arr(items));
            }
            if self.chars[self.pos] != ',' {
                return Err("expected ',' or ']' in JSON array".to_string());
            }
            self.pos += 1;
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            match ch {
                '"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                '\\' => {
                    self.pos += 1;
                    self.parse_string_escape(&mut out)?;
                }
                c if (c as u32) < 0x20 => {
                    return Err("unescaped control character in JSON string".to_string());
                }
                _ => {
                    out.push(ch);
                    self.pos += 1;
                }
            }
        }
        Err("unterminated JSON string".to_string())
    }

    /// One `\` escape. Precondition: the position is just past the
    /// backslash; on success it is past the escape.
    fn parse_string_escape(&mut self, out: &mut String) -> Result<(), String> {
        if self.pos >= self.chars.len() {
            return Err("unterminated escape in JSON string".to_string());
        }
        let esc = self.chars[self.pos];
        match esc {
            '"' | '\\' | '/' => {
                out.push(esc);
                self.pos += 1;
            }
            'b' => {
                out.push('\u{0008}');
                self.pos += 1;
            }
            'f' => {
                out.push('\u{000C}');
                self.pos += 1;
            }
            'n' => {
                out.push('\n');
                self.pos += 1;
            }
            'r' => {
                out.push('\r');
                self.pos += 1;
            }
            't' => {
                out.push('\t');
                self.pos += 1;
            }
            'u' => {
                let ch = self.parse_unicode_escape()?;
                out.push(ch);
            }
            _ => return Err(format!("invalid escape '\\{esc}' in JSON string")),
        }
        Ok(())
    }

    /// One `\uXXXX` escape with surrogate-pair handling. Precondition: the
    /// position is at the `u`; on success it is past the escape.
    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        self.pos += 1;
        let mut code = self.parse_hex4()?;
        if (0xD800..0xDC00).contains(&code) {
            code = self.parse_low_surrogate(code)?;
        }
        char::from_u32(code).ok_or_else(|| "invalid code point in JSON string".to_string())
    }

    /// The `\uDC00`-`\uDFFF` second half of a surrogate pair, combining it
    /// with the high surrogate into the astral code point.
    fn parse_low_surrogate(&mut self, high: u32) -> Result<u32, String> {
        if self.pos + 1 < self.chars.len()
            && self.chars[self.pos] == '\\'
            && self.chars[self.pos + 1] == 'u'
        {
            self.pos += 2;
            let low = self.parse_hex4()?;
            if (0xDC00..0xE000).contains(&low) {
                return Ok(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
            }
            return Err("invalid low surrogate in JSON string".to_string());
        }
        Err("unpaired high surrogate in JSON string".to_string())
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.chars.len() {
            return Err("truncated \\u escape in JSON string".to_string());
        }
        let hex: String = self.chars[self.pos..self.pos + 4].iter().collect();
        let value = u32::from_str_radix(&hex, 16)
            .map_err(|_| "invalid \\u escape in JSON string".to_string())?;
        self.pos += 4;
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        self.skip_number_sign();
        self.skip_number_digits();
        if self.pos < self.chars.len() && self.chars[self.pos] == '.' {
            self.pos += 1;
            self.skip_number_digits();
        }
        self.skip_number_exponent();
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<f64>()
            .map(JsonValue::Num)
            .map_err(|_| format!("invalid JSON number '{text}'"))
    }

    /// The optional leading `-` of a JSON number.
    fn skip_number_sign(&mut self) {
        if self.pos < self.chars.len() && self.chars[self.pos] == '-' {
            self.pos += 1;
        }
    }

    /// A run of ASCII digits (possibly empty; validity is settled by the
    /// final `f64` parse).
    fn skip_number_digits(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
    }

    /// The optional `e[+-]digits` exponent of a JSON number.
    fn skip_number_exponent(&mut self) {
        if self.pos < self.chars.len() && matches!(self.chars[self.pos], 'e' | 'E') {
            self.pos += 1;
            if self.pos < self.chars.len() && matches!(self.chars[self.pos], '+' | '-') {
                self.pos += 1;
            }
            self.skip_number_digits();
        }
    }
}

// --- Temp files -------------------------------------------------------------

/// Scratch output file, mirroring `new TempFile()` (the file is removed when
/// dropped unless [`TempFile::keep`] is called).
pub struct TempFile {
    path: PathBuf,
    keep: bool,
}

impl TempFile {
    /// Create a uniquely named empty file in the system temp dir.
    pub fn create(prefix: &str, suffix: &str) -> Result<Self, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        for _ in 0..1000 {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "arc-rs-{prefix}-{}-{n}{suffix}",
                std::process::id()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path, keep: false }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot create temp file: {e}")),
            }
        }
        Err("cannot create temp file: too many collisions".to_string())
    }

    /// The scratch path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Keep the file on drop, mirroring `TempFile::setPreserveFile(true)`.
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.keep {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

#[cfg(test)]
#[path = "unit_engine_tests.rs"]
mod unit_engine_tests;
