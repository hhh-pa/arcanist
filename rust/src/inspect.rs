//! `arc inspect`: show internal object information.
//!
//! PHP reference: `src/workflow/ArcanistInspectWorkflow.php` with the
//! `browse` and `working-copy` inspectors
//! (`src/inspector/Arcanist{Browse,WorkingCopyState}RefInspector.php`), the
//! `working-copy` ref (`src/ref/ArcanistWorkingCopyStateRef.php`), and the
//! ref-tree rendering (`+` refs, `*` attached versus `o` unloaded
//! hardpoints, `>` values).
//!
//! Scope: `working-copy(<commit>)` resolves the commit with git and renders
//! the `ArcanistWorkingCopyStateRef` tree; `--explore` additionally loads
//! the commit message. `browse(<token>)` resolves through the shared
//! browse engine (see `browse`); revision annotations stay unloaded
//! (`revisionRefs`).
//!
//! The ref trees carry no console markup: every PHP `tsprintf` pattern
//! here is markup-free, so both ANSI modes share the exact bytes.

use std::path::Path;
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::{
    browse,
    conduit::{self, EnvelopeKind},
    config, diff_submit, git,
};

/// Subcommand name, matching `arc inspect`.
pub const NAME: &str = "inspect";

/// Object types this command can name, in listed order.
pub const SUPPORTED_TYPES: &[&str] = &["browse", "working-copy"];

fn usage() -> String {
    "usage: arc inspect [--explore] [-- OBJECT...]\n\
     \n\
     Show internal object information.\n\
     \n\
     With no OBJECTs, list the inspectable object types. Otherwise each\n\
     OBJECT has the form NAME or NAME(ARG), for example \"working-copy(HEAD)\".\n\
     \n\
     Options:\n\
     \x20 --explore     Load all object hardpoints.\n\
     \x20 -h, --help    Show this help.\n\
     \n\
     Not yet ported (see T1128): revision annotations (revisionRefs).\n"
        .to_string()
}

/// Parsed `inspect` options.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub explore: bool,
    pub objects: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// Parse `argv` (excluding the subcommand name).
pub fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut explore = false;
    let mut objects = Vec::new();
    let mut end_of_flags = false;
    for arg in argv {
        if end_of_flags {
            objects.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--explore" {
            explore = true;
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            objects.push(arg.clone());
        }
    }
    ParseOutcome::Args(Options { explore, objects })
}

/// One parsed object specification: `NAME` or `NAME(ARG)`.
#[derive(Debug, PartialEq, Eq)]
pub struct ObjectSpec {
    pub function: String,
    pub arguments: Option<String>,
}

fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// Parse an object specification, mirroring `/^([\w-]+)(?:\((.*)\))?\z/`.
pub fn parse_spec(text: &str) -> Result<ObjectSpec, String> {
    let invalid = || {
        format!(
            "Object specification \"{text}\" is unknown, expected a specification like \"commit(HEAD)\"."
        )
    };
    match text.find('(') {
        None => {
            if text.is_empty() || !text.bytes().all(is_name_char) {
                return Err(invalid());
            }
            Ok(ObjectSpec {
                function: text.to_string(),
                arguments: None,
            })
        }
        Some(open) => {
            let name = &text[..open];
            if name.is_empty() || !name.bytes().all(is_name_char) {
                return Err(invalid());
            }
            let rest = &text[open + 1..];
            let Some(arguments) = rest.strip_suffix(')') else {
                return Err(invalid());
            };
            Ok(ObjectSpec {
                function: name.to_string(),
                arguments: Some(arguments.to_string()),
            })
        }
    }
}

/// Render a string hardpoint value, mirroring `describeValue`: truncate to
/// 64 bytes (like `substr`) and escape newlines, tabs, backslashes, and
/// quotes (like `addcslashes`). A multibyte character split by the cut
/// renders as U+FFFD instead of PHP's raw partial bytes.
pub fn render_value(value: &str) -> String {
    let bytes = value.as_bytes();
    let cut = &bytes[..bytes.len().min(64)];
    let mut escaped: Vec<u8> = Vec::with_capacity(cut.len() + 2);
    escaped.push(b'"');
    for &byte in cut {
        match byte {
            b'\n' => escaped.extend_from_slice(b"\\n"),
            b'\r' => escaped.extend_from_slice(b"\\r"),
            b'\t' => escaped.extend_from_slice(b"\\t"),
            b'\\' => escaped.extend_from_slice(b"\\\\"),
            b'"' => escaped.extend_from_slice(b"\\\""),
            _ => escaped.push(byte),
        }
    }
    escaped.push(b'"');
    String::from_utf8_lossy(&escaped).into_owned()
}

/// Commit data backing a `working-copy(...)` ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub hash: String,
    pub message: String,
}

/// Resolve `rev` and read its full hash and message with git.
pub fn read_commit(dir: &Path, rev: &str) -> Result<CommitInfo, String> {
    if !git::is_inside_work_tree(dir) {
        return Err(
            "\"arc inspect\" must be run in a Git working copy to inspect \"working-copy(...)\"."
                .to_string(),
        );
    }
    let arg = format!("{rev}^{{commit}}");
    let hash = git::stdout_text(dir, &["rev-parse", "--verify", &arg])
        .map_err(|_| format!("unknown commit \"{rev}\"."))?;
    if hash.is_empty() {
        return Err(format!("unknown commit \"{rev}\"."));
    }
    let message = git::stdout_text(dir, &["show", "-s", "--format=%B", &hash]).unwrap_or_default();
    Ok(CommitInfo { hash, message })
}

/// Render one `working-copy(...)` ref tree, mirroring `describeRef`.
pub fn render_working_copy(info: &CommitInfo, explore: bool) -> String {
    let mut text = String::new();
    text.push_str("+ [ArcanistWorkingCopyStateRef] Working Copy State\n");
    text.push_str(" * [ArcanistScalarHardpoint] commitRef\n");
    text.push_str(&format!(
        "  + [ArcanistCommitRef] Commit \"{}\"\n",
        info.hash
    ));
    if explore {
        text.push_str("   * [ArcanistScalarHardpoint] message\n");
        text.push_str(&format!("    > {}\n", render_value(&info.message)));
        text.push_str("   o [ArcanistScalarHardpoint] upstream\n");
    } else {
        text.push_str("   o [ArcanistScalarHardpoint] message\n");
        text.push_str("   o [ArcanistScalarHardpoint] upstream\n");
    }
    text.push_str(" o [ArcanistObjectListHardpoint] revisionRefs\n");
    text
}

const BROWSE: &str = "[ArcanistBrowseRef]";
const BROWSE_URI: &str = "[ArcanistBrowseURIRef]";
const VECTOR: &str = "[ArcanistVectorHardpoint]";

/// Render a `browse(<token>)` tree; with `ctx`, `--explore` loads the
/// commits (plus messages) and URIs through the browse engine.
pub fn render_browse(
    dir: &Path,
    token: &str,
    explore: bool,
    ctx: Option<&mut browse::BrowseCtx>,
) -> Result<String, String> {
    let mut out = format!("+ {BROWSE} Browse Query \"{token}\"\n");
    let Some(ctx) = ctx.filter(|_| explore) else {
        out.push_str(&format!(" o {VECTOR} commitRefs\n"));
        out.push_str(&format!(" o {VECTOR} uris\n"));
        return Ok(out);
    };
    let refs = vec![browse::BrowseRef {
        token: Some(token.to_string()),
        types: Vec::new(),
        branch: None,
    }];
    let resolved = browse::resolve_refs(ctx, &refs)?;
    let hit = &resolved[0];
    out.push_str(&format!(" * {VECTOR} commitRefs\n"));
    for hash in &hit.commits {
        out.push_str(&format!("  + [ArcanistCommitRef] Commit \"{hash}\"\n"));
        let message = read_commit(dir, hash).unwrap_or_else(|_| CommitInfo {
            hash: hash.clone(),
            message: String::new(),
        });
        out.push_str("   * [ArcanistScalarHardpoint] message\n");
        out.push_str(&format!("    > {}\n", render_value(&message.message)));
        out.push_str("   o [ArcanistScalarHardpoint] upstream\n");
    }
    out.push_str(&format!(" * {VECTOR} uris\n"));
    for uri in &hit.uris {
        out.push_str(&format!(
            "  + {BROWSE_URI} Browse URI \"{u}\"\n",
            u = uri.uri
        ));
    }
    Ok(out)
}

/// Render one object specification, or the failure message.
pub fn inspect_one(dir: &Path, spec: &str, explore: bool) -> Result<String, String> {
    inspect_one_with(dir, spec, explore, None)
}

/// Render one object specification with an optional browse context for
/// `browse(<token>)` exploration.
pub fn inspect_one_with(
    dir: &Path,
    spec: &str,
    explore: bool,
    ctx: Option<&mut browse::BrowseCtx>,
) -> Result<String, String> {
    let parsed = parse_spec(spec)?;
    if !SUPPORTED_TYPES.contains(&parsed.function.as_str()) {
        let mut names = SUPPORTED_TYPES.to_vec();
        names.sort_unstable();
        return Err(format!(
            "Unknown object type \"{}\", supported types are: {}.",
            parsed.function,
            names.join(", ")
        ));
    }
    match parsed.function.as_str() {
        "working-copy" => {
            let Some(argument) = parsed.arguments else {
                return Err(
                    "Expected exactly one argument to \"working-copy(...)\" with a commit hash."
                        .to_string(),
                );
            };
            Ok(render_working_copy(&read_commit(dir, &argument)?, explore))
        }
        "browse" => {
            let Some(token) = parsed.arguments else {
                return Err(
                    "Expected exactly one argument to \"browse(...)\" with a token.".to_string(),
                );
            };
            render_browse(dir, &token, explore, ctx)
        }
        _ => unreachable!("supported types are dispatched above"),
    }
}

fn conduit_error_text(error: arc_rs::conduit::Error) -> String {
    error.to_string()
}

fn parse_inspect_options(argv: &[String]) -> Result<Options, ExitCode> {
    match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            Err(ExitCode::SUCCESS)
        }
        ParseOutcome::Error(message) => {
            // T1153 R4 (RB-D4): `inspect` is a toolset workflow:
            // parser errors render the red `USAGE EXCEPTION` label
            // with no usage dump.
            Err(conduit::report(EnvelopeKind::ToolsetUsage, &message))
        }
        ParseOutcome::Args(options) => Ok(options),
    }
}

fn print_object_choices() {
    let mut text = "Choose an object to inspect:\n\n".to_string();
    for name in SUPPORTED_TYPES {
        text.push_str(&format!("    - {name}\n"));
    }
    text.push('\n');
    print!("{text}");
}

fn is_browse_spec(spec: &str) -> bool {
    parse_spec(spec)
        .map(|parsed| parsed.function == "browse")
        .unwrap_or(false)
}

fn wants_browse_client(options: &Options) -> bool {
    options.explore && options.objects.iter().any(|spec| is_browse_spec(spec))
}

/// True for the spec-shape errors PHP raises as
/// `PhutilArgumentUsageException` (exit 1 under the `USAGE EXCEPTION`
/// label): malformed specs and unknown object types
/// (`ArcanistInspectWorkflow.php:62-78`), the working-copy gate (a
/// "must be run in ..." refusal like land's), and the arity guards.
/// Anything else (bad commits, render failures) escapes into a fatal.
fn is_toolset_usage_message(message: &str) -> bool {
    (message.starts_with("Object specification \"")
        && message.contains("\" is unknown, expected a specification like \"commit(HEAD)\"."))
        || message.starts_with("Unknown object type \"")
        || message.starts_with("Expected exactly one argument to \"")
        || message
            == "\"arc inspect\" must be run in a Git working copy to inspect \"working-copy(...)\"."
}

fn finish_inspect_blocks(
    dir: &Path,
    options: &Options,
    mut ctx: Option<&mut browse::BrowseCtx>,
) -> ExitCode {
    let mut blocks = Vec::new();
    for spec in &options.objects {
        match inspect_one_with(dir, spec, options.explore, ctx.as_deref_mut()) {
            Ok(block) => blocks.push(block),
            Err(message) => {
                // T1153 R4 (RB-D4): spec-shape errors are toolset
                // usage (exit 1); the rest escape into a fatal (255).
                let kind = if is_toolset_usage_message(&message) {
                    EnvelopeKind::ToolsetUsage
                } else {
                    EnvelopeKind::Fatal
                };
                return conduit::report(kind, &message);
            }
        }
    }
    print!("{}", blocks.join("\n"));
    ExitCode::SUCCESS
}

fn run_inspect_objects(
    dir: &Path,
    options: &Options,
    call: Option<&diff_submit::ConduitCall>,
) -> ExitCode {
    if !wants_browse_client(options) {
        return finish_inspect_blocks(dir, options, None);
    }
    match call {
        Some(call) => {
            let mut ctx = browse::BrowseCtx::new(dir, call, "");
            finish_inspect_blocks(dir, options, Some(&mut ctx))
        }
        None => match config::client_for(None) {
            Ok(client) => {
                let live = |method: &str, params: &serde_json::Value| {
                    client.call(method, params).map_err(conduit_error_text)
                };
                let mut ctx = browse::BrowseCtx::new(dir, &live, "");
                finish_inspect_blocks(dir, options, Some(&mut ctx))
            }
            Err(message) => conduit::report(EnvelopeKind::Fatal, &message),
        },
    }
}

/// Inspect in `dir` per `argv`, with an injected Conduit caller (`None`
/// builds one from config, only when `--explore` meets `browse(...)`).
pub fn run_in_with(
    dir: &Path,
    argv: &[String],
    call: Option<&diff_submit::ConduitCall>,
) -> ExitCode {
    let options = match parse_inspect_options(argv) {
        Ok(options) => options,
        Err(code) => return code,
    };
    if options.objects.is_empty() {
        print_object_choices();
        return ExitCode::SUCCESS;
    }
    run_inspect_objects(dir, &options, call)
}

/// Inspect in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    run_in_with(dir, argv, None)
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod inspect_tests;
