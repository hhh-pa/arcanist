//! Shared `arc help` renderer (T1153 RB-D5).
//!
//! Unifies the `arc --help` lead (`BARE_HELP_LEAD`, kept for the release
//! smoke gate) with the `arc help` bodies (`render_help_bare`,
//! `render_workflow_help`) and the per-command `--help` texts
//! (`usage_text`). Previously `main.rs` and each of `browse`, `download`,
//! `export`, `paste`, `upload` duplicated `indent`/`renderArgumentSpecs`
//! and the terminator hints; all call sites now share this module so a
//! byte change in one place changes every help path identically.
//!
//! PHP references at `ceb082ef`:
//! - `src/parser/argument/PhutilArgumentParser.php:680-790`
//!   (`renderHelp`, `renderWorkflowHelp`, `renderArgumentSpecs`).
//! - `src/parser/argument/workflow/PhutilHelpArgumentWorkflow.php:36-58`
//!   (`execute`: header plus `renderWorkflowHelp`).
//! - `src/runtime/ArcanistRuntime.php:80-86,172-188` (noninteractive `--`
//!   gate and its three cyan hints).
//!
//! Classic workflows (`newLegacyPhutilWorkflow`) keep their source
//! indentation verbatim and never unwrap help; toolset workflows
//! (`newPhutilWorkflow`) unwrap single newlines between non-space
//! characters (`ArcanistWorkflow.php:118-166`).

use crate::terminal;

/// `arc --help` leads with the one-line usage (the release smoke test
/// greps it) ahead of the full workflow reference (T1153 RB-D5).
pub const BARE_HELP_LEAD: &str = "usage: arc [--version] [--help] <command>\n\n";

/// `indent()`: interpret console markers, then wrap at 78 columns with
/// a `level`-space prefix (`PhutilArgumentParser::indent`).
#[must_use]
pub fn indent_text(level: usize, text: &str, ansi: bool) -> String {
    terminal::console_wrap(&terminal::interpret(text, ansi), level, true)
}

/// Unwrap single newlines in toolset help bodies
/// (`preg_replace("/(?<=\S)\n(?=\S)/", ' ', $help)` in
/// `ArcanistWorkflow::newPhutilWorkflow`).
#[must_use]
pub fn unwrap_help(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, chr) in chars.iter().enumerate() {
        if *chr == '\n' {
            let before = index
                .checked_sub(1)
                .and_then(|prev| chars.get(prev))
                .is_some_and(|prev| !prev.is_whitespace());
            let after = chars
                .get(index + 1)
                .is_some_and(|next| !next.is_whitespace());
            if before && after {
                out.push(' ');
                continue;
            }
        }
        out.push(*chr);
    }
    out
}

/// One displayed argument spec: name, short flag, parameter, help.
pub type SpecView<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);

/// One argument spec line plus its help (`renderArgumentSpecs()`,
/// sorted by name): `      __--name__ __param__[, __-s__ __param__]`
/// then the 10-space help, then a blank line.
#[must_use]
pub fn render_argument_specs(specs: &[SpecView<'_>], ansi: bool) -> String {
    let mut sorted: Vec<_> = specs.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = Vec::new();
    for (name, short, param, help) in sorted {
        let mut line = indent_text(6, &format!("__--{name}__"), ansi);
        let mut short_part =
            short.map(|alias| terminal::interpret(&format!(", __-{alias}__"), ansi));
        if let Some(param) = param {
            let rendered = terminal::interpret(&format!(" __{param}__"), ansi);
            line.push_str(&rendered);
            if let Some(short) = short_part.as_mut() {
                short.push_str(&rendered);
            }
        }
        if let Some(short) = short_part {
            line.push_str(&short);
        }
        out.push(line);
        out.push(indent_text(10, help.unwrap_or(""), ansi));
        out.push(String::new());
    }
    out.join("\n")
}

/// `**NAME WORKFLOW**` header from `PhutilHelpArgumentWorkflow`
/// (`'%s WORKFLOW'`, uppercased name), console-formatted.
#[must_use]
pub fn workflow_header(name: &str, ansi: bool) -> String {
    terminal::console_format(
        "**%s**",
        &[&format!("{} WORKFLOW", name.to_uppercase())],
        ansi,
    )
}

/// `There is no ... workflow.` body from `renderWorkflowHelp` (shown
/// when `help <name>` names nothing known), console-formatted.
#[must_use]
pub fn no_workflow_line(name: &str, ansi: bool) -> String {
    terminal::console_format("There is no **%s** workflow.", &[name], ansi)
}

/// The full per-command `--help` text (T1153 BROWSE-B23): the
/// `**NAME WORKFLOW**` header plus detailed workflow help and a
/// trailing blank line, byte-identical to `arc help <name>` for one
/// name (`PhutilHelpArgumentWorkflow::execute`).
///
/// `examples` falls back to `**name**` when `None` (the
/// `PhutilArgumentWorkflow` default); `synopsis` renders empty when
/// `None` (PHP sets no synopsis for e.g. `paste`); `help` is unwrapped
/// when `unwrap` is set (toolset) and kept verbatim otherwise
/// (classic). `specs` are the non-wildcard argument specs, sorted by
/// name like `renderArgumentSpecs()`.
#[must_use]
pub fn usage_text(
    name: &str,
    examples: Option<&str>,
    synopsis: Option<&str>,
    help: Option<&str>,
    unwrap: bool,
    specs: &[SpecView<'_>],
    ansi: bool,
) -> String {
    let header = terminal::console_format(
        "**%s**\n\n",
        &[&format!("{} WORKFLOW", name.to_uppercase())],
        ansi,
    );
    let examples = examples
        .map(str::to_string)
        .unwrap_or_else(|| format!("**{name}**"));
    let mut parts = vec![
        indent_text(0, &examples, ansi),
        indent_text(0, synopsis.unwrap_or(""), ansi),
    ];
    if let Some(help) = help {
        let body = if unwrap {
            unwrap_help(help)
        } else {
            help.to_string()
        };
        parts.push(String::new());
        parts.push(indent_text(0, &body, ansi));
    }
    if !specs.is_empty() {
        parts.push(String::new());
        parts.push(render_argument_specs(specs, ansi));
    }
    parts.push(String::new());
    let body = parts.join("\n");
    format!("{header}{body}\n")
}

/// True when a toolset invocation must fail with [`MISSING_TERMINATOR`]:
/// stdin is piped and argv carries no standalone `--` token (T1153
/// BROWSE-B21; `ArcanistRuntime.php:80-86`, enforced by
/// `PhutilArgumentParser::parseFull()`). Any standalone `--` satisfies
/// PHP: the standard-arguments scan that records it never consumes
/// workflow flag values.
#[must_use]
pub fn terminator_missing(argv: &[String], stdin_tty: bool) -> bool {
    !stdin_tty && !argv.iter().any(|arg| arg == "--")
}

/// `Missing required "--" in argument list.` (`ArcanistRuntime.php:188`).
pub const MISSING_TERMINATOR: &str = "Missing required \"--\" in argument list.";

/// The three cyan `USAGE` hints PHP prints before the terminator error
/// (`ArcanistRuntime.php:172-188`), on stderr like PHP.
pub fn print_terminator_hints(ansi: bool) {
    use crate::terminal::{PhtArg, TVal};
    let usage = terminal::pht("USAGE", &[]);
    for hint in [
        terminal::pht(
            "\"%s\" is being run noninteractively, but the argument list is missing \"--\" to indicate end of flags.",
            &[PhtArg::Text("arc")],
        ),
        terminal::pht(
            "When running noninteractively, you MUST provide \"--\" to all commands (even if they take no arguments).",
            &[],
        ),
    ] {
        eprint!("{}", terminal::format_log("cyan", &usage, &hint, ansi));
    }
    // `tsprintf('%s <__%s__>', ...)` nested like PHP: the inner line
    // passes through the outer `%s` unescaped (`TVal::Terminal`).
    let inner = terminal::tsprintf_ansi(
        "%s <__%s__>",
        &[
            TVal::Text(&terminal::pht("Learn More:", &[])),
            TVal::Text("https://phurl.io/u/noninteractive"),
        ],
        ansi,
    )
    .expect("fixed tsprintf pattern");
    let line = terminal::tsprintf_ansi(
        "**<bg:cyan> %s </bg>** %s\n",
        &[TVal::Text(&usage), TVal::Terminal(&inner)],
        ansi,
    )
    .expect("fixed tsprintf pattern");
    eprint!("{}", line.as_str());
}

#[cfg(test)]
#[path = "help_tests.rs"]
mod help_tests;
