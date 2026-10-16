//! `liberate` workflow: create or update a libphutil library.
//!
//! PHP reference: `src/workflow/ArcanistLiberateWorkflow.php`.
//!
//! With no path, one level of subdirectories is scanned for
//! `__phutil_library_init__.php` files. A path without a library gets a
//! new init file (after an interactive prompt for the directory and the
//! library name); a library at format version 2 gets its map rebuilt by
//! shelling out to `support/lib/rebuild-map.php` with `php`, exactly as
//! the PHP workflow does.
//!
//! Status lines go to stderr through the log engine (`SCAN`/`WORK` blue,
//! `DONE` green); the missing-directory banner renders via
//! `tsprintf("%!\n%W\n", ...)` and the `DONE` count through the `pht`
//! plural variants (`Updated library.` / `Updated %s libraries.`).
//!
//! Like PHP at ceb, the rebuild status is ignored entirely: `DONE`
//! always prints and the run exits 0, even when `rebuild-map.php`
//! fails or `php` is missing (T1153 CORE-50). Parse errors are bold
//! usage errors (the toolset `parse()` catch, exit 1); no-libraries,
//! ancient, and unknown-version failures throw `ArcanistUsageException`,
//! which escapes the toolset runtime into a PHP fatal (255). A
//! declined directory prompt aborts with `User aborted the workflow.`
//! through the `---` arm (exit 1).
//!
//! The rebuild script ships in-tree under `support/` (restored from the
//! PHP master). It is found by an upward `support/lib/rebuild-map.php`
//! search above the library first, then above the running binary: an
//! installed `arc` (e.g. `<prefix>/bin/arc` with the tree at
//! `<prefix>/support/...`) resolves just like PHP's install-relative
//! bundled path, and so do out-of-tree libraries.
//!
//! Divergences:
//! - End-of-input while naming a library fails instead of re-prompting
//!   forever (T1153 CORE-54, kept).
//! - `?`, `*`/`!` saves, and saved responses work through the shared
//!   prompt engine (T1153 CORE-52); the contract-test seam models a
//!   TTY, production gates on the real stdin state.
//! - Prompts print plain text (no `phutil_console_prompt` wrapping or
//!   readline history).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::conduit::ClassifiedError;
use crate::terminal::{self, PhtArg, TVal};

pub const NAME: &str = "liberate";

const INIT_FILE: &str = "__phutil_library_init__.php";
const MAP_FILE: &str = "__phutil_library_map__.php";
const VERSION_MARKER: &str = "@phutil-library-version";

/// T1153 R10: `--help` renders the generated workflow help
/// (`PhutilHelpArgumentWorkflow::execute()` + `renderWorkflowHelp()`),
/// not a short custom usage string. The renderer is the shared
/// `crate::help` module; this file keeps only the data (ceb
/// `src/workflow/ArcanistLiberateWorkflow.php:11-34`).
const HELP_EXAMPLES: &str = "**liberate**\n**liberate** [__path__]";
const HELP_SYNOPSIS: &str = "Create or update an Arcanist library.";
const HELP_TEXT: &str = "Create or update an Arcanist library.";
/// Non-wildcard argument specs: (name, short, param, help).
const HELP_SPECS: &[(&str, Option<&str>, Option<&str>, &str)] = &[(
    "clean",
    None,
    None,
    "Perform a clean rebuild, ignoring caches. Thorough, but slow.",
)];

/// The full `--help` text via the shared renderer.
fn usage_text(ansi: bool) -> String {
    let specs: Vec<crate::help::SpecView<'_>> = HELP_SPECS
        .iter()
        .map(|(name, short, param, help)| (*name, *short, *param, Some(*help)))
        .collect();
    crate::help::usage_text(
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
    usage_text(terminal::ansi_enabled())
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    clean: bool,
    path: Option<String>,
}

enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
    /// T1153 B21/B23: stdin is piped and argv carries no `--`.
    MissingTerminator,
}

fn parse_args(argv: &[String]) -> ParseOutcome {
    parse_args_with_tty(argv, crate::prompt::stdin_is_terminal())
}

/// [`parse_args`] with an injected TTY flag (deterministic tests).
/// T1153 B21/B23: piped runs without `--` fail with
/// `MissingTerminator` (checked after unknown-flag scanning, like
/// PHP's `parseFull()`, but before path-count validation). `--help`
/// without `--` when piped reports the terminator. Vs ceb
/// `ArcanistRuntime.php:80-86,172-188` and
/// `PhutilArgumentParser.php:331-336`.
fn parse_args_with_tty(argv: &[String], stdin_tty: bool) -> ParseOutcome {
    let missing = crate::help::terminator_missing(argv, stdin_tty);
    // `--help` never reaches the workflow parser in PHP (the global
    // parser consumes it); the port keeps the shortcut for direct
    // calls, like the other commands.
    if argv
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "-h" || arg == "--help")
    {
        if missing {
            return ParseOutcome::MissingTerminator;
        }
        return ParseOutcome::Help;
    }
    let parsed = match crate::argparse::parse_toolset(
        argv,
        &[crate::argparse::FlagSpec::boolean("clean")],
        true,
        stdin_tty,
    ) {
        Ok(parsed) => parsed,
        Err(message) => return ParseOutcome::Error(message),
    };
    let ansi = terminal::ansi_enabled();
    for notice in &parsed.notices {
        eprint!(
            "{}",
            terminal::tsprintf_ansi("%s\n", &[TVal::Text(notice)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default()
        );
    }
    if missing {
        return ParseOutcome::MissingTerminator;
    }
    let clean = parsed.flag("clean");
    let mut positionals = parsed.positionals.into_iter();
    let path = positionals.next();
    if positionals.next().is_some() {
        return ParseOutcome::Error(one_path_error());
    }
    ParseOutcome::Args(Options { clean, path })
}

fn one_path_error() -> String {
    "Provide only one path to \"arc liberate\". The path should identify a directory where you want to create or update a library."
        .to_string()
}

/// Library roots one level below `dir` (dirs holding an init file), sorted.
pub fn scan_libraries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot list '{}': {e}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot list '{}': {e}", dir.display()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let candidate = entry.path().join(INIT_FILE);
        if candidate.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

/// Parse the `@phutil-library-version N` marker from map text.
///
/// Mirrors `/@phutil-library-version (\d+)/` plus PHP's `(int)` cast:
/// exactly one space, then digits saturating at `i64::MAX`. Later
/// markers still match when an early one has no digits.
pub fn parse_map_version(map_text: &str) -> Option<i64> {
    let mut search = 0;
    while let Some(at) = map_text[search..].find(VERSION_MARKER) {
        let rest = &map_text[search + at + VERSION_MARKER.len()..];
        let digits: String = rest
            .strip_prefix(' ')
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            let mut value: i64 = 0;
            for byte in digits.bytes() {
                value = value
                    .saturating_mul(10)
                    .saturating_add(i64::from(byte - b'0'));
            }
            return Some(value);
        }
        search += at + VERSION_MARKER.len();
    }
    None
}

/// Library format version: marker value, or 1 when the map is absent.
pub fn library_version(path: &Path) -> Result<i64, String> {
    let map_file = path.join(MAP_FILE);
    if !map_file.exists() {
        return Ok(1);
    }
    let map = std::fs::read_to_string(&map_file)
        .map_err(|e| format!("cannot read '{}': {e}", map_file.display()))?;
    Ok(parse_map_version(&map).unwrap_or(1))
}

/// True for valid new library names (`^[a-z-]+$`).
#[must_use]
pub fn is_valid_library_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

/// One log-engine line on stderr (`writeStatus` blue, `writeError`
/// red, `writeSuccess` green).
fn push_log(color: &str, label: &str, message: &str) {
    eprint!(
        "{}",
        terminal::format_log(color, label, message, terminal::ansi_enabled())
    );
}

/// `echo tsprintf("%!\n%W\n", ...)` for a missing directory: a banner
/// plus a wrapped paragraph.
#[must_use]
pub fn new_library_block(path: &str, ansi: bool) -> String {
    let title = terminal::pht("NEW LIBRARY", &[]);
    let body = terminal::pht(
        "The directory \"%s\" does not exist. Do you want to create it?",
        &[PhtArg::Text(path)],
    );
    terminal::tsprintf_ansi("%!\n%W\n", &[TVal::Text(&title), TVal::Text(&body)], ansi)
        .expect("fixed tsprintf pattern")
        .as_str()
        .to_string()
}

/// `echo tsprintf("%s\n", ...)` when no library exists at the path.
#[must_use]
pub fn no_library_line(path: &str, ansi: bool) -> String {
    let message = terminal::pht(
        "No library currently exists at the path \"%s\"...",
        &[PhtArg::Text(path)],
    );
    terminal::tsprintf_ansi("%s\n", &[TVal::Text(&message)], ansi)
        .expect("fixed tsprintf pattern")
        .as_str()
        .to_string()
}

/// `pht('Updated %s librarie(s).', ...)` with the US English variants
/// (`Updated library.` / `Updated %s libraries.`).
#[must_use]
pub fn done_message(count: usize) -> String {
    terminal::pht("Updated %s librarie(s).", &[PhtArg::Int(count as i64)])
}

fn prompt_line(prompt: &str, input: &mut dyn BufRead) -> Option<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    let read = input.read_line(&mut line).ok()?;
    if read == 0 {
        return None;
    }
    Some(line.trim().to_string())
}

/// Ask to create a missing directory through the shared prompt
/// engine (`arc.liberate.create`, T1153 CORE-52): `?` help, `*`/`!`
/// saves, and saved responses all work, and a non-TTY stdin throws
/// after echoing the query, like `ArcanistPrompt.php:77-86` at ceb.
/// Answers come from `input`; `stdin_tty` carries the real TTY
/// state (the contract-test seam passes `true`).
fn confirm_create_directory(
    path: &Path,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    saved: Option<&str>,
    save: &mut dyn FnMut(crate::prompts::PromptScope, &str) -> Result<(), String>,
) -> Result<bool, String> {
    print!(
        "{}",
        new_library_block(&path.display().to_string(), terminal::ansi_enabled())
    );
    match crate::prompts::execute_prompt_saving(
        input,
        crate::prompts::LIBERATE_PROMPT_KEY,
        crate::prompts::LIBERATE_PROMPT_DESCRIPTION,
        "Create new library?",
        stdin_tty,
        saved,
        save,
    ) {
        Ok(()) => Ok(true),
        Err(message) if message == crate::prompt::ABORT => Ok(false),
        Err(message) => Err(message),
    }
}

/// Name prompt (`liberateCreateLibrary`, T1153 CORE-52): PHP prints
/// the `Choose a name...` line on every retry, and loops forever
/// (T1153 CORE-54, kept: EOF fails fast instead of re-prompting).
fn ask_library_name(input: &mut dyn BufRead) -> Result<String, String> {
    loop {
        println!(
            "{}",
            terminal::pht("Choose a name for the new library.", &[])
        );
        let name = prompt_line("What do you want to name this library? ", input)
            .ok_or_else(|| "cancelled: no library name given".to_string())?;
        if is_valid_library_name(&name) {
            return Ok(name);
        }
        // `echo phutil_console_format("%s\n", ...)`: substituted raw.
        println!(
            "{}",
            terminal::pht(
                "Library name should contain only lowercase letters and hyphens.",
                &[]
            )
        );
    }
}

/// Create the directory (prompting) and init file when missing.
///
/// Returns `true` when a library was created. Prints the interactive
/// prompts to stdout and reads answers from `input`. A decline fails
/// with `cancelled: User aborted the workflow.`: the `cancelled: `
/// prefix keeps the R4 contract shape, and `classify_error` strips it
/// so the CLI renders PHP's abort text.
pub fn ensure_library(path: &Path, input: &mut dyn BufRead) -> Result<bool, String> {
    let mut noop = |_: crate::prompts::PromptScope, _: &str| Ok(());
    ensure_library_with(path, input, true, None, &mut noop)
}

/// [`ensure_library`] with an explicit prompt channel (T1153
/// CORE-52): `stdin_tty` gates like PHP, `saved` is the saved
/// `arc.liberate.create` response, and `save` persists `*`/`!`
/// suffixes. The contract-test seam passes `true`; production
/// passes the real TTY state.
pub fn ensure_library_with(
    path: &Path,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    saved: Option<&str>,
    save: &mut dyn FnMut(crate::prompts::PromptScope, &str) -> Result<(), String>,
) -> Result<bool, String> {
    if path.join(INIT_FILE).is_file() {
        return Ok(false);
    }
    print!(
        "{}",
        no_library_line(&path.display().to_string(), terminal::ansi_enabled())
    );
    if path.exists() {
        if !path.is_dir() {
            return Err(
                "Provide a directory to create or update a libphutil library in.".to_string(),
            );
        }
    } else if confirm_create_directory(path, input, stdin_tty, saved, save)? {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("cannot create '{}': {e}", path.display()))?;
    } else {
        return Err(format!("cancelled: {}", crate::prompt::ABORT));
    }
    if path.join(INIT_FILE).is_file() {
        return Ok(false);
    }
    // `echo pht(...)."\n"`: substituted raw, newline outside.
    let dir = path.display().to_string();
    println!(
        "{}",
        terminal::pht(
            "Creating new libphutil library in '%s'.",
            &[PhtArg::Text(&dir)]
        )
    );
    let name = ask_library_name(input)?;
    // The trailing newline is inside the `pht` pattern here.
    print!(
        "{}",
        terminal::pht(
            "Writing '%s' to '%s'...\n",
            &[PhtArg::Text(INIT_FILE), PhtArg::Text(&dir)]
        )
    );
    std::fs::write(
        path.join(INIT_FILE),
        format!("<?php\n\nphutil_register_library('{name}', __FILE__);\n"),
    )
    .map_err(|e| format!("cannot write init file: {e}"))?;
    Ok(true)
}

/// Find `support/lib/rebuild-map.php` above `path`, then above `fallback`,
/// then above the running binary.
///
/// The binary search is the install story: `arc` ships with a `support/`
/// tree next to its install root (like PHP's install-relative bundled
/// script), so out-of-tree libraries still resolve the script.
/// Search `start` and its ancestors for `support/lib/rebuild-map.php`.
/// Pure helper behind [`find_rebuild_script`]; split out so tests do not
/// depend on where the test binary lives (e.g. `CARGO_TARGET_DIR` outside
/// the repo, as in CI's gates image).
fn find_in_ancestors(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|ancestor| {
        let candidate = ancestor.join("support/lib/rebuild-map.php");
        candidate.is_file().then_some(candidate)
    })
}

pub fn find_rebuild_script(path: &Path, fallback: &Path) -> Option<PathBuf> {
    for root in [path, fallback] {
        if let Some(found) = find_in_ancestors(root) {
            return Some(found);
        }
    }
    // Installed binaries (`cargo install` lays out no `support/` tree)
    // resolve the script only when run from a checkout carrying it,
    // same as `anoid`. A dev binary under `rust/target/` additionally
    // resolves via its own ancestors.
    std::env::current_exe()
        .ok()
        .and_then(|exe| find_in_ancestors(&exe))
}

/// Rebuild one v2 library map via `php rebuild-map.php`.
///
/// The script streams to the console directly (`phutil_passthru`); only
/// the exit status comes back. Returns the failure message when the
/// rebuild fails, or `None` on success.
pub fn rebuild_map(path: &Path, clean: bool, script: Option<&Path>) -> Option<String> {
    let script = script.map_or_else(
        || find_rebuild_script(path, path),
        |s| Some(s.to_path_buf()),
    );
    let Some(script) = script else {
        return Some(format!(
            "cannot rebuild '{}': support/lib/rebuild-map.php was not found above the library or the arc binary",
            path.display()
        ));
    };
    let mut command = std::process::Command::new("php");
    command.arg("-f").arg(&script).arg("--");
    if clean {
        command.arg("--drop-cache");
    }
    command.arg(path);
    match command.status() {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!(
            "rebuild-map.php failed for '{}' (exit {status})",
            path.display()
        )),
        Err(e) => Some(format!(
            "cannot run php '{}': {e} (is PHP installed?)",
            script.display()
        )),
    }
}

/// Update or create the library at `path`.
///
/// Returns `Ok(None)` on success and `Ok(Some(message))` when the rebuild
/// step fails (a per-library failure: reported, but the workflow still
/// exits 0). Usage errors (ancient or unknown versions, unreadable
/// maps) return `Err`, abort the workflow, and exit 255 at the
/// `run_in` layer (T1153 CORE-53); declined prompts exit 1 through
/// the `---` arm (T1153 CORE-51).
pub fn liberate_path(
    path: &Path,
    clean: bool,
    input: &mut dyn BufRead,
) -> Result<Option<String>, String> {
    let mut noop = |_: crate::prompts::PromptScope, _: &str| Ok(());
    liberate_path_with(path, clean, input, true, None, &mut noop)
}

/// [`liberate_path`] with an explicit prompt channel (see
/// [`ensure_library_with`]).
pub fn liberate_path_with(
    path: &Path,
    clean: bool,
    input: &mut dyn BufRead,
    stdin_tty: bool,
    saved: Option<&str>,
    save: &mut dyn FnMut(crate::prompts::PromptScope, &str) -> Result<(), String>,
) -> Result<Option<String>, String> {
    if ensure_library_with(path, input, stdin_tty, saved, save)? {
        // Fresh libraries skip the version check and go straight to a v2
        // rebuild, mirroring `liberateCreateLibrary`.
        return Ok(rebuild_map(path, clean, None));
    }
    match library_version(path)? {
        1 => Err("This very old library is no longer supported.".to_string()),
        2 => Ok(rebuild_map(path, clean, None)),
        other => Err(format!("Unknown library version '{other}'!")),
    }
}

/// Render `path` relative to the working copy root when it sits under
/// `dir`, else unchanged (`Filesystem::readablePath()`).
pub fn readable_path(dir: &Path, path: &Path) -> String {
    let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
    let text = path.display().to_string();
    match text.strip_prefix(&prefix) {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// Run the workflow; reads interactive answers from `input`.
///
/// `stdin_tty` gates the create-directory prompt like PHP
/// (`ArcanistPrompt.php:77-86` at ceb, T1153 CORE-52): contract
/// tests pass `true` with piped answers; [`execute`] passes the
/// real TTY state.
pub fn execute_with_input(
    dir: &Path,
    argv: &[String],
    input: &mut dyn BufRead,
    stdin_tty: bool,
) -> Result<String, String> {
    // T1153 B21/B23: the parse threads the prompt TTY flag so
    // contract tests (TTY) need no `--` while piped runs do.
    let opts = match parse_args_with_tty(argv, stdin_tty) {
        ParseOutcome::Args(opts) => opts,
        ParseOutcome::Help => return Ok(usage()),
        // T1153 B21/B23: the terminator failure carries no usage dump.
        ParseOutcome::MissingTerminator => {
            return Err(crate::help::MISSING_TERMINATOR.to_string());
        }
        // T1153 CORE-21 (R4): PHP renders usage errors with no dump.
        ParseOutcome::Error(message) => return Err(message),
    };
    let paths = match opts.path {
        Some(path) => vec![dir.join(&path)],
        None => {
            push_log(
                "blue",
                &terminal::pht("SCAN", &[]),
                &terminal::pht(
                    "Searching for libraries in the current working directory...",
                    &[],
                ),
            );
            let found = scan_libraries(dir)?;
            if found.is_empty() {
                return Err(
                    "Unable to find any libraries under the current working directory. To create a library, provide a path."
                        .to_string(),
                );
            }
            found
        }
    };
    // The `arc.liberate.create` prompt channel: answers come from
    // `input` through the TTY gate, with saved responses and `*`/`!`
    // saves wired to the real config scopes.
    let user_file = crate::config::arcrc_path();
    let local_file = crate::prompts::working_copy_config_file(dir);
    let saved_list =
        crate::prompts::load_saved_responses(user_file.as_deref(), local_file.as_deref());
    let saved = saved_list
        .iter()
        .filter(|(prompt, _)| prompt == crate::prompts::LIBERATE_PROMPT_KEY)
        .map(|(_, response)| response.clone())
        .next_back();
    let mut saver =
        crate::prompts::response_saver(crate::prompts::LIBERATE_PROMPT_KEY, user_file, local_file);
    let out = String::new();
    for path in &paths {
        let entry = format!("{}{}", readable_path(dir, path), std::path::MAIN_SEPARATOR);
        push_log(
            "blue",
            &terminal::pht("WORK", &[]),
            &terminal::pht("Updating library: %s", &[PhtArg::Text(&entry)]),
        );
        // T1153 CORE-50: the rebuild status is ignored entirely (the
        // script already streamed to the console); only `Err` aborts.
        match liberate_path_with(
            path,
            opts.clean,
            input,
            stdin_tty,
            saved.as_deref(),
            &mut saver,
        ) {
            Ok(_) => {}
            Err(message) => return Err(message),
        }
    }
    push_log(
        "green",
        &terminal::pht("DONE", &[]),
        &done_message(paths.len()),
    );
    Ok(out)
}

/// Run the workflow rooted at `dir`, returning stdout text or an error message.
pub fn execute(dir: &Path, argv: &[String]) -> Result<String, String> {
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    execute_with_input(dir, argv, &mut locked, crate::prompt::stdin_is_terminal())
}

/// Classify a workflow error for the R4 envelope: parser errors
/// are toolset usage errors (red label, exit 1 -- the R4 mapping,
/// kept; PHP's `parse()` catch renders these bold, like
/// `ToolsetParseError` elsewhere); declined prompts (`cancelled:
/// ...`) are `ArcanistUserAbortException` (the `---` arm, exit 1,
/// T1153 CORE-51), rendered without the `cancelled: ` prefix so
/// declines show PHP's abort text; no-libraries and ancient/unknown
/// versions throw `ArcanistUsageException`, which escapes the
/// toolset runtime into a PHP fatal (255, T1153 CORE-53).
/// Per-library rebuild failures never reach here (ignored above,
/// T1153 CORE-50).
fn classify_error(argv: &[String], message: String) -> ClassifiedError {
    // T1153 B21/B23: the terminator failure is toolset usage (exit 1).
    if matches!(
        parse_args(argv),
        ParseOutcome::Error(_) | ParseOutcome::MissingTerminator
    ) {
        ClassifiedError::toolset_usage(message)
    } else if let Some(body) = message.strip_prefix("cancelled: ") {
        ClassifiedError::user_abort(body.to_string())
    } else {
        ClassifiedError::fatal(message)
    }
}

pub fn run_in(dir: &Path, argv: &[String]) -> ExitCode {
    match execute(dir, argv) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // T1153 B21/B23: the terminator failure prints the three
            // cyan hints first (like `ArcanistRuntime.php:172-188`).
            if message == crate::help::MISSING_TERMINATOR {
                crate::help::print_terminator_hints(crate::terminal::ansi_enabled());
            }
            classify_error(argv, message).report()
        }
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    run_in(&cwd, argv)
}

#[cfg(test)]
#[path = "liberate_tests.rs"]
mod liberate_tests;
