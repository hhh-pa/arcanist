//! `arc-rs linters`: list available linters.
//!
//! PHP reference: `src/workflow/ArcanistLintersWorkflow.php` (flags
//! `--verbose` and repeatable `--search`, positional exact names,
//! case-insensitive matching, exact and search mutually exclusive).
//!
//! Scope: the ported linters from [`arc_rs::lint::all_linters`], one line
//! each. Engine configuration, versions, and per-linter options from the PHP
//! workflow have no counterpart yet: every ported linter is reported as
//! available.

use std::process::ExitCode;

// Alias the current crate so `arc_rs::...` paths resolve both once wired
// into lib.rs and when included standalone by an integration test.
extern crate self as arc_rs;

/// Subcommand name, matching `arc linters`.
pub const NAME: &str = "linters";

fn usage() -> String {
    "usage: arc-rs linters [--verbose] [--search TERM]... [NAME...]\n\
     \n\
     List the available linters, with one line describing each.\n\
     \n\
     If NAMEs are given, only the linters with those exact (case-insensitive)\n\
     names are shown. Otherwise --search filters linters by case-insensitive\n\
     substring over names and descriptions.\n\
     \n\
     Options:\n\
     \x20 --verbose      Show detailed information for each linter.\n\
     \x20 --search TERM  Only show linters matching TERM (repeatable).\n\
     \x20 -h, --help     Show this help.\n"
        .to_string()
}

/// One available linter: its registry name plus a one-line description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinterInfo {
    pub name: String,
    pub description: String,
}

/// One-line description for a registry linter name.
fn describe(name: &str) -> &'static str {
    match name {
        "TXT" => "Trailing whitespace, tab literals, and missing EOF newlines (TXT2/TXT4/TXT6).",
        "NAME" => "File names must be boring (NAME1).",
        "JSON" => "JSON syntax errors, native or via jsonlint.",
        "XML" => "XML well-formedness errors.",
        "MERGECONFLICT" => "Unresolved merge-conflict markers.",
        "GEN" => "Generated-file marker (stops all linters).",
        "NOLINT" => "Nolint marker (stops all linters).",
        "CHMOD" => "Executable bit and script permissions.",
        "SPELL" => "Common spelling mistakes.",
        "S&RX" => "Custom script-and-regex rules.",
        "COMPOSER" => "Stale composer.lock (COMPOSER1).",
        "PHL" => "Phutil library symbol map consistency.",
        "JSHint" => "JavaScript errors via jshint.",
        "JSCS" => "JavaScript code style via jscs.",
        "CSSLint" => "CSS errors via csslint.",
        "LESSC" => "LESS compile errors via lessc.",
        "GJSLINT" => "JavaScript style via gjslint.",
        "COFFEE" => "CoffeeScript errors via coffeelint.",
        "flake8" => "Python errors via flake8.",
        "PYFLAKES" => "Python errors via pyflakes.",
        "PyLint" => "Python errors via pylint.",
        "PEP8" => "Python style via pep8.",
        "RuboCop" => "Ruby style via rubocop.",
        "RUBY" => "Ruby syntax via ruby -c.",
        "cppcheck" => "C/C++ errors via cppcheck.",
        "C++ Google's Styleguide" => "C++ style via cpplint.",
        "GOLINT" => "Go style via golint.",
        "HLINT" => "Haskell suggestions via hlint.",
        "C#" => "C# style via cslint.",
        "PHPCS" => "PHP style via phpcs.",
        "PHP" => "PHP syntax via php -l.",
        "PUPPETLINT" => "Puppet style via puppet-lint.",
        "XHP" => "XHP AST lint (parse wrapper).",
        _ => "No description available.",
    }
}

/// Extra detail shown for `--verbose`, when the linter is known.
fn details(name: &str) -> Option<&'static str> {
    match name {
        "TXT" => Some(
            "Rules: TXT2 Tab Literal (error), TXT4 File Does Not End in Newline (error), \
             TXT6 Trailing Whitespace (autofix).",
        ),
        _ => None,
    }
}

/// Every ported linter, sorted by name.
#[must_use]
pub fn available_linters() -> Vec<LinterInfo> {
    let mut out: Vec<LinterInfo> = arc_rs::lint::all_linters()
        .iter()
        .map(|linter| {
            let name = linter.name();
            LinterInfo {
                name: name.to_string(),
                description: describe(name).to_string(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parsed `linters` options.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub verbose: bool,
    pub search: Vec<String>,
    pub exact: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    Args(Options),
    Help,
    Error(String),
}

/// Parse `argv` (excluding the subcommand name).
pub fn parse_args(argv: &[String]) -> ParseOutcome {
    let mut verbose = false;
    let mut search = Vec::new();
    let mut exact = Vec::new();
    let mut end_of_flags = false;
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if end_of_flags {
            exact.push(arg.clone());
        } else if arg == "--" {
            end_of_flags = true;
        } else if arg == "--verbose" {
            verbose = true;
        } else if arg == "--search" {
            let Some(term) = iter.next() else {
                return ParseOutcome::Error("--search requires a value".to_string());
            };
            search.push(term.clone());
        } else if let Some(term) = arg.strip_prefix("--search=") {
            search.push(term.to_string());
        } else if arg == "-h" || arg == "--help" {
            return ParseOutcome::Help;
        } else if arg.starts_with('-') {
            return ParseOutcome::Error(format!("unknown flag '{arg}'"));
        } else {
            exact.push(arg.clone());
        }
    }
    ParseOutcome::Args(Options {
        verbose,
        search,
        exact,
    })
}

/// Apply exact/search selection, mirroring the PHP workflow: exact names
/// match case-insensitively, search terms match case-insensitively against
/// names and descriptions, and the two modes are mutually exclusive.
pub fn select_linters(all: &[LinterInfo], options: &Options) -> Result<Vec<LinterInfo>, String> {
    if !options.exact.is_empty() && !options.search.is_empty() {
        return Err("Specify either search expression or exact name".to_string());
    }
    if !options.exact.is_empty() {
        let mut out = Vec::new();
        for info in all {
            if options
                .exact
                .iter()
                .any(|name| info.name.eq_ignore_ascii_case(name))
            {
                out.push(info.clone());
            }
        }
        return Ok(out);
    }
    if options.search.is_empty() {
        return Ok(all.to_vec());
    }
    let mut out = Vec::new();
    for info in all {
        let name = info.name.to_lowercase();
        let description = info.description.to_lowercase();
        let hit = options.search.iter().any(|term| {
            let term = term.to_lowercase();
            name.contains(&term) || description.contains(&term)
        });
        if hit {
            out.push(info.clone());
        }
    }
    Ok(out)
}

/// Render the selection as the command's stdout text.
pub fn render(selected: &[LinterInfo], options: &Options) -> String {
    if !options.exact.is_empty() && selected.is_empty() {
        let first = options.exact.first().cloned().unwrap_or_default();
        return format!(
            "No match found. Try `arc-rs linters --search {first}` to search for a linter.\n"
        );
    }
    let mut text = String::new();
    for info in selected {
        if options.verbose {
            text.push_str(&format!("{}\n    {}\n", info.name, info.description));
            if let Some(detail) = details(&info.name) {
                text.push_str(&format!("    {detail}\n"));
            }
            text.push('\n');
        } else {
            text.push_str(&format!("{} - {}\n", info.name, info.description));
        }
    }
    if !options.verbose {
        text.push_str("(Run `arc-rs linters --verbose` for more details.)\n");
    }
    text
}

/// Entry point: `argv` excludes the subcommand name.
pub fn run(argv: &[String]) -> ExitCode {
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
    let all = available_linters();
    let selected = match select_linters(&all, &options) {
        Ok(selected) => selected,
        Err(message) => {
            eprintln!("arc-rs {NAME}: {message}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", render(&selected, &options));
    ExitCode::SUCCESS
}

#[cfg(test)]
#[path = "linters_tests.rs"]
mod linters_tests;
