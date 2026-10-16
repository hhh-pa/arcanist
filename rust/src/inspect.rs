//! `arc inspect`: show internal object information.
//!
//! PHP reference: `src/workflow/ArcanistInspectWorkflow.php` with all
//! twelve inspectors (browse, working-copy, commit, user, remote, and
//! the seven simple-symbol types), and the ref-tree rendering (`+`
//! refs, `*` attached versus `o` unloaded hardpoints, `>` values).
//!
//! Scope: every type constructs (with PHP's arity and symbol-shape
//! validation) and renders; `--explore` loads what git can answer
//! (commit resolution and messages). Conduit-backed loads — upstream
//! and revision refs, symbol/user objects, remote repository refs —
//! stay unloaded (T1153 RB-D15 remainder).
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
    config, diff_submit, get_config, git, hg, markers, terminal,
};

/// Subcommand name, matching `arc inspect`.
pub const NAME: &str = "inspect";

/// Object types this command can name, in listed order.
pub const SUPPORTED_TYPES: &[&str] = &[
    "browse",
    "working-copy",
    "commit",
    "user",
    "remote",
    "build",
    "buildable",
    "buildplan",
    "file",
    "paste",
    "revision",
    "task",
];

/// One simple-symbol type: inspect name, ref class, display kind,
/// optional monogram prefix, and PHID type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolType {
    pub name: &'static str,
    pub class: &'static str,
    pub kind: &'static str,
    pub prefix: Option<char>,
    pub phid_type: &'static str,
}

/// The seven `ArcanistSimpleSymbolRef` types.
pub const SYMBOL_TYPES: &[SymbolType] = &[
    SymbolType {
        name: "build",
        class: "ArcanistBuildSymbolRef",
        kind: "Build",
        prefix: None,
        phid_type: "HMBD",
    },
    SymbolType {
        name: "buildable",
        class: "ArcanistBuildableSymbolRef",
        kind: "Buildable",
        prefix: None,
        phid_type: "HMBB",
    },
    SymbolType {
        name: "buildplan",
        class: "ArcanistBuildPlanSymbolRef",
        kind: "Build Plan",
        prefix: None,
        phid_type: "HMCP",
    },
    SymbolType {
        name: "file",
        class: "ArcanistFileSymbolRef",
        kind: "File",
        prefix: Some('F'),
        phid_type: "FILE",
    },
    SymbolType {
        name: "paste",
        class: "ArcanistPasteSymbolRef",
        kind: "Paste",
        prefix: Some('P'),
        phid_type: "PSTE",
    },
    SymbolType {
        name: "revision",
        class: "ArcanistRevisionSymbolRef",
        kind: "Revision",
        prefix: Some('D'),
        phid_type: "DREV",
    },
    SymbolType {
        name: "task",
        class: "ArcanistTaskSymbolRef",
        kind: "Task",
        prefix: Some('T'),
        phid_type: "TASK",
    },
];

/// Look up a simple-symbol type by inspect name.
#[must_use]
pub fn symbol_type(name: &str) -> Option<&'static SymbolType> {
    SYMBOL_TYPES.iter().find(|kind| kind.name == name)
}

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `arc_rs::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistInspectWorkflow.php:10-31`).
const HELP_EXAMPLES: &str = "**inspect** [__options__] -- __object__";
const HELP_SYNOPSIS: &str = "Show internal object information.";
const HELP_TEXT: &str = "Inspect internal object properties.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] =
    &[("explore", None, None, "Load all object hardpoints.")];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<arc_rs::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    arc_rs::help::usage_text(
        NAME,
        Some(HELP_EXAMPLES),
        Some(HELP_SYNOPSIS),
        Some(HELP_TEXT),
        true,
        &specs,
        ansi,
    )
}

fn usage() -> String {
    usage_text(arc_rs::terminal::ansi_enabled())
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

/// A resolved symbol: the display spelling plus its kind (`id` carries
/// the integer; every other kind carries its matched text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSymbol {
    pub kind: &'static str,
    pub value: String,
}

fn symbol_format_error(symbol: &str) -> String {
    format!(
        "The format of symbol \"{symbol}\" is unrecognized. Expected a monogram like \"X123\", \
         or an ID like \"123\", or a PHID."
    )
}

/// True for `[1-9][0-9]*` (no leading zeroes, ASCII only).
fn is_decimal_id(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_digit() && c != '0')
        && chars.all(|c| c.is_ascii_digit())
}

/// True for PCRE `\S+` (no whitespace, ASCII or otherwise).
fn is_nonspace(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_whitespace)
}

/// Resolve a simple symbol like `ArcanistSimpleSymbolRef::resolveSymbol`:
/// an optional monogram prefix plus an ID (cast to int PHP-style), or a
/// `PHID-<TYPE>-...`. Anything else is a usage error.
///
/// Note the `(int)` quirk PHP carries: the cast applies to the whole
/// match, so a prefixed monogram like `D123` resolves to `0`.
pub fn resolve_simple_symbol(kind: &SymbolType, symbol: &str) -> Result<ResolvedSymbol, String> {
    let rest = match kind.prefix {
        Some(prefix) => symbol
            .strip_prefix(prefix)
            .or_else(|| symbol.strip_prefix(prefix.to_ascii_lowercase()))
            .unwrap_or(symbol),
        None => symbol,
    };
    if is_decimal_id(rest) {
        return Ok(ResolvedSymbol {
            kind: "id",
            value: terminal::php_int_cast(symbol).to_string(),
        });
    }
    let head = format!("PHID-{}-", kind.phid_type);
    if let Some(tail) = symbol.strip_prefix(&head)
        && is_nonspace(tail)
    {
        return Ok(ResolvedSymbol {
            kind: "phid",
            value: symbol.to_string(),
        });
    }
    Err(symbol_format_error(symbol))
}

fn user_symbol_format_error(symbol: &str) -> String {
    format!(
        "The format of user symbol \"{symbol}\" is unrecognized. Expected a username like \
         \"alice\" or \"@alice\", or a user PHID, or a user ID, or a special function like \
         \"viewer()\"."
    )
}

/// True for PCRE `\s` (space, tab, newline, CR, form feed, vertical tab).
fn is_pcre_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}' | '\u{b}')
}

/// True for `^\S+\(\s*\)\s*\z`: a bare function call.
fn is_function_symbol(symbol: &str) -> bool {
    let Some(open) = symbol.find('(') else {
        return false;
    };
    let (head, tail) = symbol.split_at(open);
    if !is_nonspace(head) {
        return false;
    }
    let Some(close) = tail[1..].find(')') else {
        return false;
    };
    let (between, after) = tail[1..].split_at(close);
    between.chars().all(is_pcre_space) && after[1..].chars().all(is_pcre_space)
}

/// Resolve a user symbol (`id`, `phid`, `function`, or `username`), in
/// PHP's order.
pub fn resolve_user_symbol(symbol: &str) -> Result<ResolvedSymbol, String> {
    if is_decimal_id(symbol) {
        return Ok(ResolvedSymbol {
            kind: "id",
            value: terminal::php_int_cast(symbol).to_string(),
        });
    }
    if let Some(tail) = symbol.strip_prefix("PHID-USER-")
        && is_nonspace(tail)
    {
        return Ok(ResolvedSymbol {
            kind: "phid",
            value: symbol.to_string(),
        });
    }
    if is_function_symbol(symbol) {
        return Ok(ResolvedSymbol {
            kind: "function",
            value: symbol.to_string(),
        });
    }
    let name = symbol.strip_prefix('@').unwrap_or(symbol);
    if is_nonspace(name) {
        return Ok(ResolvedSymbol {
            kind: "username",
            value: name.to_string(),
        });
    }
    Err(user_symbol_format_error(symbol))
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

/// Render a `name(...)` symbol tree: the symbol ref plus its unloaded
/// `ref.symbol.object` (T1153 RB-D13; object loads need conduit).
pub fn render_symbol(class: &str, kind: &str, resolved: &ResolvedSymbol) -> String {
    format!(
        "+ [{class}] {kind} Symbol \"{}\"\n o [ArcanistScalarHardpoint] ref.symbol.object\n",
        resolved.value
    )
}

/// A `commit(...)` object: unloaded, null-attached (the symbol names no
/// commit), or a resolved commit.
pub enum CommitSymbolObject {
    Unloaded,
    Null,
    Commit(CommitInfo),
}

/// Render a `commit(...)` tree (T1153 RB-D13).
pub fn render_commit_symbol(symbol: &str, object: CommitSymbolObject, explore: bool) -> String {
    let mut text = format!("+ [ArcanistCommitSymbolRef] Commit Symbol \"{symbol}\"\n");
    match &object {
        CommitSymbolObject::Commit(info) => {
            text.push_str(" * [ArcanistScalarHardpoint] ref.symbol.object\n");
            text.push_str(&format!(
                "  + [ArcanistCommitRef] Commit \"{}\"\n",
                info.hash
            ));
            if explore {
                text.push_str("   * [ArcanistScalarHardpoint] message\n");
                text.push_str(&format!("    > {}\n", render_value(&info.message)));
            } else {
                text.push_str("   o [ArcanistScalarHardpoint] message\n");
            }
            text.push_str("   o [ArcanistScalarHardpoint] upstream\n");
        }
        CommitSymbolObject::Null => {
            text.push_str(" * [ArcanistScalarHardpoint] ref.symbol.object\n");
            text.push_str(&format!("  > {}\n", render_value("")));
        }
        CommitSymbolObject::Unloaded => {
            text.push_str(" o [ArcanistScalarHardpoint] ref.symbol.object\n");
        }
    }
    text
}

/// Render a `remote(...)` tree (T1153 RB-D13; the repository-refs load
/// needs conduit).
pub fn render_remote(name: &str) -> String {
    format!(
        "+ [ArcanistRemoteRef] Remote \"{name}\"\n o [ArcanistObjectListHardpoint] \
         arc.remote.repositoryRefs\n"
    )
}

/// Parse `hg paths` output into `(name, uri)` pairs, strictly: every
/// line must look like `name = uri`, like PHP.
pub fn parse_hg_paths(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for line in arc_rs::grid::split_lines(text) {
        let Some((name, uri)) = line.split_once(" = ") else {
            return Err(format!(
                "Failed to match remote pattern against line \"{line}\"."
            ));
        };
        out.push((name.to_string(), uri.to_string()));
    }
    Ok(out)
}

/// Resolve a commit symbol through `git cat-file --batch-check`, like
/// `ArcanistGitCommitSymbolCommitHardpointQuery`: the commit hash, or
/// `None` when the symbol names no commit. Newlines are fatal.
pub fn resolve_commit_symbol(dir: &Path, symbol: &str) -> Result<Option<String>, String> {
    if symbol.contains('\n') {
        let escaped = symbol.replace('\\', "\\\\").replace('\n', "\\n");
        return Err(format!(
            "Commit symbol \"{escaped}\" contains a newline. This is not a valid character in a \
             Git commit symbol."
        ));
    }
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["cat-file", "--batch-check", "--"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = writeln!(stdin, "{symbol}");
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.lines().next().unwrap_or("");
    let mut parts = line.splitn(3, ' ');
    let (Some(hash), Some(kind)) = (parts.next(), parts.next()) else {
        return Err(format!(
            "Execution of \"git cat-file --batch-check\" emitted an unexpected line (\"{line}\")."
        ));
    };
    if kind != "commit" {
        return Ok(None);
    }
    Ok(Some(hash.to_string()))
}

/// Look up remote `name` like `ArcanistRemoteRefInspector`: git and hg
/// search their remotes (usage error when absent); anything else throws
/// the not-implemented fatal PHP's base query template throws.
pub fn lookup_remote(dir: &Path, name: &str) -> Result<(), String> {
    if git::is_inside_work_tree(dir) {
        let remotes = markers::query_git_remotes(dir)?;
        if remotes.iter().any(|remote| remote.name == name) {
            return Ok(());
        }
    } else if markers::find_hg_root(dir).is_some() {
        let output = hg::output_with(dir, &hg::HgCommand::system(), &["paths"])?;
        if !output.status.success() {
            return Err(markers::command_exception_text(
                output.status.code().unwrap_or(1),
                "hg paths",
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            ));
        }
        let paths = parse_hg_paths(&String::from_utf8_lossy(&output.stdout))?;
        if paths.iter().any(|(remote, _)| remote == name) {
            return Ok(());
        }
    } else if markers::find_svn_root(dir).is_some() {
        return Err(
            "Method newRemoteRefQueryTemplate in class ArcanistSubversionAPI is not implemented!"
                .to_string(),
        );
    } else {
        return Err(
            "Method newRemoteRefQueryTemplate in class ArcanistFilesystemAPI is not implemented!"
                .to_string(),
        );
    }
    Err(format!("This working copy has no remote named \"{name}\"."))
}

/// Comma-separated argument check shared by every inspector: PHP
/// `explode(',', $arguments)` must yield exactly one argument, else
/// the caller's usage text.
fn single_argument(arguments: Option<&str>, usage: &str) -> Result<String, String> {
    match arguments {
        Some(value) => {
            let mut parts = value.split(',');
            match (parts.next(), parts.next()) {
                (Some(only), None) => Ok(only.to_string()),
                _ => Err(usage.to_string()),
            }
        }
        None => Err(usage.to_string()),
    }
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
fn check_supported_type(function: &str) -> Result<(), String> {
    if SUPPORTED_TYPES.contains(&function) {
        return Ok(());
    }
    let mut names = SUPPORTED_TYPES.to_vec();
    names.sort_unstable();
    Err(format!(
        "Unknown object type \"{function}\", supported types are: {}.",
        names.join(", ")
    ))
}

/// T1153 RB-D14: without `--explore` the raw argument renders
/// unresolved, with no repository needed.
fn inspect_working_copy(
    dir: &Path,
    arguments: Option<&str>,
    explore: bool,
) -> Result<String, String> {
    let argument = single_argument(
        arguments,
        "Expected exactly one argument to \"working-copy(...)\" with a commit hash.",
    )?;
    if explore {
        Ok(render_working_copy(&read_commit(dir, &argument)?, true))
    } else {
        Ok(render_working_copy(
            &CommitInfo {
                hash: argument,
                message: String::new(),
            },
            false,
        ))
    }
}

fn inspect_commit(dir: &Path, arguments: Option<&str>, explore: bool) -> Result<String, String> {
    let symbol = single_argument(
        arguments,
        "Expected exactly one argument to \"commit(...)\" with a commit symbol.",
    )?;
    if !explore {
        return Ok(render_commit_symbol(
            &symbol,
            CommitSymbolObject::Unloaded,
            false,
        ));
    }
    match resolve_commit_symbol(dir, &symbol)? {
        Some(hash) => Ok(render_commit_symbol(
            &symbol,
            CommitSymbolObject::Commit(read_commit(dir, &hash)?),
            true,
        )),
        None => Ok(render_commit_symbol(
            &symbol,
            CommitSymbolObject::Null,
            true,
        )),
    }
}

fn inspect_simple_symbol(name: &str, arguments: Option<&str>) -> Result<String, String> {
    let kind = symbol_type(name).expect("supported types are dispatched above");
    let usage = format!("Expected exactly one argument to \"{name}(...)\" with a symbol.");
    let symbol = single_argument(arguments, &usage)?;
    let resolved = resolve_simple_symbol(kind, &symbol)?;
    Ok(render_symbol(kind.class, kind.kind, &resolved))
}

pub fn inspect_one_with(
    dir: &Path,
    spec: &str,
    explore: bool,
    ctx: Option<&mut browse::BrowseCtx>,
) -> Result<String, String> {
    let parsed = parse_spec(spec)?;
    check_supported_type(&parsed.function)?;
    let arguments = parsed.arguments.as_deref();
    match parsed.function.as_str() {
        "working-copy" => inspect_working_copy(dir, arguments, explore),
        "browse" => {
            let token = single_argument(
                arguments,
                "Expected exactly one argument to \"browse(...)\" with a token.",
            )?;
            render_browse(dir, &token, explore, ctx)
        }
        "commit" => inspect_commit(dir, arguments, explore),
        "user" => {
            let symbol = single_argument(
                arguments,
                "Expected exactly one argument to \"user(...)\" with a user symbol.",
            )?;
            let resolved = resolve_user_symbol(&symbol)?;
            Ok(render_symbol("ArcanistUserSymbolRef", "User", &resolved))
        }
        "remote" => {
            let name = single_argument(
                arguments,
                "Expected exactly one argument to \"remote(...)\" with a remote name.",
            )?;
            lookup_remote(dir, &name)?;
            Ok(render_remote(&name))
        }
        name => inspect_simple_symbol(name, arguments),
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
            // T1153 RB-D4 (R4): `inspect` is a toolset workflow:
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
        || message.starts_with("This working copy has no remote named \"")
        || message.starts_with("The format of symbol \"")
        || message.starts_with("The format of user symbol \"")
        || message
            == "\"arc inspect\" must be run in a Git working copy to inspect \"working-copy(...)\"."
}

fn finish_inspect_blocks(
    dir: &Path,
    options: &Options,
    mut ctx: Option<&mut browse::BrowseCtx>,
    auth_client: Option<&conduit::Client>,
) -> ExitCode {
    let mut blocks = Vec::new();
    for spec in &options.objects {
        match inspect_one_with(dir, spec, options.explore, ctx.as_deref_mut()) {
            Ok(block) => blocks.push(block),
            Err(message) => {
                // T1153 CORE-5: auth-coded browse failures render login
                // guidance (the toolset runtime's auth arm).
                if let Some(client) = auth_client
                    && let Some(code) = conduit::auth_report_for(client, &message)
                {
                    return code;
                }
                // T1153 RB-D4 (R4): spec-shape errors are toolset
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
        return finish_inspect_blocks(dir, options, None, None);
    }
    match call {
        Some(call) => {
            let mut ctx = browse::BrowseCtx::new(dir, call, "");
            finish_inspect_blocks(dir, options, Some(&mut ctx), None)
        }
        None => {
            // T1153 CORE-4: the browse engine runs tokenless; a missing
            // URI fails with the toolset no-URI block (fatal, like the
            // `raiseURIException()` escape).
            let user_file = config::arcrc_path();
            match config::client_for_tokenless_with(
                None,
                dir,
                user_file.as_deref(),
                &get_config::LoadOptions::default(),
            ) {
                Ok(client) => {
                    let live = |method: &str, params: &serde_json::Value| {
                        client.call(method, params).map_err(conduit_error_text)
                    };
                    let mut ctx = browse::BrowseCtx::new(dir, &live, "");
                    finish_inspect_blocks(dir, options, Some(&mut ctx), Some(&client))
                }
                Err(message) => conduit::report(
                    EnvelopeKind::Fatal,
                    &conduit::toolset_setup_message(&message),
                ),
            }
        }
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
