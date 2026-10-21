//! Interactive console prompts, mirroring `src/toolset/ArcanistPrompt.php`
//! (`execute()`), `phutil_console_prompt()`, `phutil_console_confirm()`,
//! and `phutil_console_select()` (`src/console/format.php`).
//!
//! Deviations: prompt answers are never persisted (`*`/`!` suffixes are
//! stripped and no `SAVE PROMPT` line is printed); configured (saved)
//! responses are not read, so the `Using saved response` echo never
//! prints; multi-line pastes are consumed one line per prompt instead of
//! being discarded as a group; EOF on a TTY answers with the default
//! instead of PHP's nonblocking spin. TTY detection uses the injected
//! `stdin_tty` flag so tests stay deterministic.

use std::io::{BufRead, Write};

use crate::terminal;

/// Abort text for declined prompts (same as `work::ABORT`).
pub const ABORT: &str = "User aborted the workflow.";

pub const STASH_PROMPT_KEY: &str = "arc.state.stash";

pub const STASH_DESCRIPTION: &str = "Prompts the user to stash changes and continue when the \
     working copy has untracked, uncommitted, or unstaged changes.";

/// `PhutilConsoleStdinNotInteractiveException` message.
pub const NOT_INTERACTIVE: &str = "The program is attempting to read user input, but stdin is \
     being piped from some other source (not a TTY).";

fn ansi() -> bool {
    terminal::ansi_enabled()
}

/// True when standard input is an interactive terminal.
#[must_use]
pub fn stdin_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// `CSI ? 9001 l`: leave win32-input mode (key-event reporting).
/// Written before live prompt reads on Windows; terminals that do not
/// implement the mode ignore it.
#[cfg(windows)]
pub const WIN32_INPUT_DISABLE: &str = "\x1b[?9001l";

/// Lock live stdin for a prompt read.
///
/// On Windows, first restores cooked input reporting: arc never
/// enables win32-input mode (T1191 audit), but a driving harness may
/// have left it on — keys would then arrive as escape sequences with
/// no newline and the prompt read below would block forever. The
/// flush applies the mode change before the read. Unix emits nothing
/// (zero behavior change).
pub fn live_stdin_lock() -> std::io::StdinLock<'static> {
    #[cfg(windows)]
    {
        print!("{WIN32_INPUT_DISABLE}");
        std::io::stdout().flush().ok();
    }
    std::io::stdin().lock()
}

/// The echoed prompt: two blank lines plus the query wrapped at indent 4.
#[must_use]
pub fn prompt_text(prompt: &str) -> String {
    format!(
        "\n\n{}",
        terminal::console_wrap(&format!("{prompt} "), 4, true)
    )
}

/// Read one answer line (`None` on EOF/error), like `rtrim($response)`.
pub fn read_answer(input: &mut dyn BufRead) -> Option<String> {
    std::io::stdout().flush().ok();
    read_answer_result(input).ok().flatten()
}

/// Read one answer line, without the trailing newline, surfacing read
/// failures: `Ok(None)` is EOF, `Err` is a read failure. The shared core
/// behind [`read_answer`] (which collapses errors to `None`) and the
/// land/browse/diff prompt readers. Lines split on `\n` with a trailing
/// `\r` trimmed, so LF-only, CRLF, and lone-CR answers all complete.
pub fn read_answer_result(input: &mut dyn BufRead) -> Result<Option<String>, std::io::Error> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => Ok(Some(line.trim_end_matches(['\r', '\n']).to_string())),
        Err(error) => Err(error),
    }
}

/// PHP `trim()` charlist (`" \t\n\r\0\x0B"`, no form feed).
fn trim_php(text: &str) -> &str {
    text.trim_matches([' ', '\t', '\n', '\r', '\0', '\u{b}'])
}

/// Ask `query` until `y`/`n`/empty, mirroring
/// `phutil_console_confirm()`. EOF answers with the default; a
/// non-interactive stdin echoes the prompt once, then fails.
pub fn console_confirm(
    input: &mut dyn BufRead,
    query: &str,
    default_no: bool,
) -> Result<bool, String> {
    let options = if default_no { "[y/N]" } else { "[Y/n]" };
    let tty = stdin_is_terminal();
    console_confirm_with_tty(input, query, options, default_no, tty)
}

/// [`console_confirm`] with an injected TTY flag (deterministic tests).
pub fn console_confirm_with_tty(
    input: &mut dyn BufRead,
    query: &str,
    options: &str,
    default_no: bool,
    stdin_tty: bool,
) -> Result<bool, String> {
    loop {
        print!("{}", prompt_text(&format!("{query} {options}")));
        if !stdin_tty {
            return Err(NOT_INTERACTIVE.to_string());
        }
        let raw = read_answer(input).unwrap_or_default();
        match trim_php(&raw).to_ascii_lowercase().as_str() {
            "y" => {
                println!();
                return Ok(true);
            }
            "n" => {
                println!();
                return Ok(false);
            }
            "" => {
                println!();
                return Ok(!default_no);
            }
            _ => {}
        }
    }
}

/// Ask for an integer in `min..=max`, mirroring
/// `phutil_console_select()`.
pub fn console_select_with_tty(
    input: &mut dyn BufRead,
    prompt: &str,
    min: i64,
    max: i64,
    stdin_tty: bool,
) -> Result<i64, String> {
    let options = format!("[{min} - {max}]");
    loop {
        print!("{}", prompt_text(&format!("{prompt} {options}")));
        if !stdin_tty {
            return Err(NOT_INTERACTIVE.to_string());
        }
        let raw = read_answer(input).unwrap_or_default();
        let selection = trim_php(&raw);
        if !selection.is_empty()
            && selection.bytes().all(|b| b.is_ascii_digit())
            && let Ok(number) = selection.parse::<i64>()
            && number >= min
            && number <= max
        {
            return Ok(number);
        }
    }
}

/// Ask for free-form input, mirroring `phutil_console_prompt()` without
/// history support.
pub fn console_prompt_with_tty(
    input: &mut dyn BufRead,
    prompt: &str,
    stdin_tty: bool,
) -> Result<String, String> {
    print!("{}", prompt_text(prompt));
    if !stdin_tty {
        return Err(NOT_INTERACTIVE.to_string());
    }
    Ok(read_answer(input).unwrap_or_default())
}

/// The `?` help for a prompt key, on stdout like PHP.
#[must_use]
pub fn prompt_help(key: &str, description: &str) -> String {
    let ansi = ansi();
    let mut out = terminal::tsprintf_ansi(
        "\n<bg:green>** %s **</bg> **%s**\n\n",
        &[terminal::TVal::Text("PROMPT"), terminal::TVal::Text(key)],
        ansi,
    )
    .map(|t| t.to_string())
    .unwrap_or_default();
    out.push_str(
        &terminal::tsprintf_ansi("%s\n", &[terminal::TVal::Text(description)], ansi)
            .map(|t| t.to_string())
            .unwrap_or_default(),
    );
    out.push('\n');
    out.push_str(
        &terminal::tsprintf_ansi(
            "%s\n",
            &[terminal::TVal::Text(
                "The default response to this prompt is \"n\".",
            )],
            ansi,
        )
        .map(|t| t.to_string())
        .unwrap_or_default(),
    );
    out.push('\n');
    for hint in [
        "Use \"*\" after a response to save it in user configuration.",
        "Use \"!\" after a response to save it in working copy configuration.",
        "Run \"arc help prompts\" for detailed help on configuring responses.",
    ] {
        out.push_str(
            &terminal::tsprintf_ansi("%?\n", &[terminal::TVal::Text(hint)], ansi)
                .map(|t| t.to_string())
                .unwrap_or_default(),
        );
    }
    out.push('\n');
    out
}

/// The saved-response echo, on stdout like PHP.
#[must_use]
pub fn saved_response_echo(query: &str, response: &str, key: &str) -> String {
    terminal::tsprintf_ansi(
        "<bg:cyan>** %s **</bg> %s **<%s>**\n<bg:cyan>** %s **</bg> (%s)\n\n",
        &[
            terminal::TVal::Text(">>>"),
            terminal::TVal::Text(query),
            terminal::TVal::Text(response),
            terminal::TVal::Text(">>>"),
            terminal::TVal::Text(&format!("Using saved response to prompt \"{key}\".")),
        ],
        ansi(),
    )
    .map(|t| t.to_string())
    .unwrap_or_default()
}

/// The `>>> query [y/N/?]` prompt line, on stdout like PHP.
#[must_use]
pub fn prompt_line(query: &str) -> String {
    terminal::tsprintf_ansi(
        "**<bg:cyan> %s </bg>** %s %s ",
        &[
            terminal::TVal::Text(">>>"),
            terminal::TVal::Text(query),
            terminal::TVal::Text("[y/N/?]"),
        ],
        ansi(),
    )
    .map(|t| t.to_string())
    .unwrap_or_default()
}

fn strip_save_suffix(response: &str) -> &str {
    response
        .strip_suffix('*')
        .or_else(|| response.strip_suffix('!'))
        .unwrap_or(response)
}

/// Run a `y`/`n` prompt to completion, mirroring
/// `ArcanistPrompt::execute()`. `saved` is a configured response, if any;
/// declining or EOF aborts with [`ABORT`].
pub fn execute_prompt(
    input: &mut dyn BufRead,
    key: &str,
    description: &str,
    query: &str,
    stdin_tty: bool,
    saved: Option<&str>,
) -> Result<(), String> {
    if query.is_empty() {
        return Err(format!("Prompt (\"{key}\") has no query text!"));
    }
    let mut saved_response = saved;
    if !stdin_tty {
        println!("{query}");
        return Err(NOT_INTERACTIVE.to_string());
    }
    println!();
    loop {
        let (response, is_saved) = match saved_response.take() {
            Some(response) => (response.to_string(), true),
            None => {
                print!("{}", prompt_line(query));
                let raw = read_answer(input).unwrap_or_default();
                let trimmed = trim_php(&raw).to_string();
                let response = if trimmed.is_empty() {
                    "n".to_string()
                } else {
                    trimmed
                };
                (response, false)
            }
        };
        let response = if is_saved {
            response
        } else {
            strip_save_suffix(&response).to_string()
        };
        match response.to_lowercase().as_str() {
            "y" => {
                if is_saved {
                    print!("{}", saved_response_echo(query, &response, key));
                }
                return Ok(());
            }
            "n" => {
                if is_saved {
                    print!("{}", saved_response_echo(query, &response, key));
                }
                return Err(ABORT.to_string());
            }
            "?" => {
                print!("{}", prompt_help(key, description));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod prompt_tests;
