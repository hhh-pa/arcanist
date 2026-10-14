//! `arc-rs inspect`: show internal object information.
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
//! the commit message. `browse(<token>)` needs Conduit and is not yet
//! ported, as are revision annotations (`revisionRefs` always renders
//! unloaded).

use std::path::Path;
use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

use arc_rs::git;

/// Subcommand name, matching `arc inspect`.
pub const NAME: &str = "inspect";

/// Object types this command can name, in listed order.
pub const SUPPORTED_TYPES: &[&str] = &["browse", "working-copy"];

fn usage() -> String {
    "usage: arc-rs inspect [--explore] [-- OBJECT...]\n\
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
     Not yet ported (see T1128): browse(<token>) via Conduit and revision\n\
     annotations (revisionRefs).\n"
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
/// 64 characters and escape newlines, tabs, backslashes, and quotes.
pub fn render_value(value: &str) -> String {
    let mut escaped = String::new();
    escaped.push('"');
    for ch in value.chars().take(64) {
        match ch {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
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
            "\"arc-rs inspect\" must be run in a Git working copy to inspect \"working-copy(...)\"."
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

/// Render one object specification, or the failure message.
pub fn inspect_one(dir: &Path, spec: &str, explore: bool) -> Result<String, String> {
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
            if parsed.arguments.is_none() {
                return Err(
                    "Expected exactly one argument to \"browse(...)\" with a token.".to_string(),
                );
            }
            Err("\"browse(...)\" is not yet ported (requires Conduit; see T1128)".to_string())
        }
        _ => unreachable!("supported types are dispatched above"),
    }
}

/// Inspect in `dir` per `argv`.
pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    let options = match parse_args(argv) {
        ParseOutcome::Help => {
            print!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ParseOutcome::Error(message) => {
            eprintln!("arc-rs {NAME}: {message}\n\n{}", usage());
            return ExitCode::FAILURE;
        }
        ParseOutcome::Args(options) => options,
    };
    if options.objects.is_empty() {
        let mut text = "Choose an object to inspect:\n\n".to_string();
        for name in SUPPORTED_TYPES {
            text.push_str(&format!("    - {name}\n"));
        }
        text.push('\n');
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    let mut blocks = Vec::new();
    for spec in &options.objects {
        match inspect_one(dir, spec, options.explore) {
            Ok(block) => blocks.push(block),
            Err(message) => {
                eprintln!("arc-rs {NAME}: {message}");
                return ExitCode::FAILURE;
            }
        }
    }
    print!("{}", blocks.join("\n"));
    ExitCode::SUCCESS
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "inspect_tests.rs"]
mod inspect_tests;
