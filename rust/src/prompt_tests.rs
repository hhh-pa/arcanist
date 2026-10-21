//! Tests for [`crate::prompt`].

use std::io::BufRead;

use crate::prompt;

fn cursor(text: &str) -> impl BufRead + use<> {
    std::io::Cursor::new(text.to_string())
}

#[test]
fn prompt_text_wraps_at_indent_four() {
    assert_eq!(prompt::prompt_text("Continue?"), "\n\n    Continue? ");
}

#[test]
fn prompt_text_wraps_long_queries() {
    let text = prompt::prompt_text(
        "Ignore the changes to these submodules and continue? This suffix pushes past the wrap column.",
    );
    assert!(text.starts_with("\n\n    Ignore"), "{text:?}");
    assert!(text.contains("\n    "), "{text:?}");
}

#[test]
fn prompt_line_strips_badge() {
    assert_eq!(
        prompt::prompt_line("Stash these changes and continue?"),
        " >>>  Stash these changes and continue? [y/N/?] "
    );
}

#[test]
fn confirm_accepts_short_answers_with_default_no() {
    let mut input = cursor("y\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Proceed?", "[y/N]", true, true),
        Ok(true)
    );
    let mut input = cursor("N\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Proceed?", "[y/N]", true, true),
        Ok(false)
    );
    let mut input = cursor("\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Proceed?", "[y/N]", true, true),
        Ok(false)
    );
}

#[test]
fn confirm_defaults_to_yes_and_eof() {
    let mut input = cursor("\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Proceed?", "[Y/n]", false, true),
        Ok(true)
    );
    let mut input = cursor("");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Proceed?", "[Y/n]", false, true),
        Ok(true)
    );
}

#[test]
fn confirm_rejects_non_tty_after_echo() {
    let mut input = cursor("y\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Proceed?", "[y/N]", true, false),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
}

#[test]
fn select_reads_bounded_integers() {
    let mut input = cursor("x\n99\n2\n");
    assert_eq!(
        prompt::console_select_with_tty(&mut input, "Pick", 1, 3, true),
        Ok(2)
    );
}

#[test]
fn prompt_returns_raw_answer() {
    let mut input = cursor("  name  \n");
    assert_eq!(
        prompt::console_prompt_with_tty(&mut input, "Name?", true),
        Ok("  name  ".to_string())
    );
}

#[test]
fn execute_prompt_confirms_and_declines() {
    let mut input = cursor("y\n");
    assert!(prompt::execute_prompt(&mut input, "k", "d", "Go?", true, None).is_ok());
    let mut input = cursor("n\n");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "d", "Go?", true, None),
        Err(prompt::ABORT.to_string())
    );
    let mut input = cursor("");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "d", "Go?", true, None),
        Err(prompt::ABORT.to_string())
    );
}

#[test]
fn execute_prompt_strips_save_suffixes() {
    let mut input = cursor("y*\n");
    assert!(prompt::execute_prompt(&mut input, "k", "d", "Go?", true, None).is_ok());
    let mut input = cursor("n!\n");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "d", "Go?", true, None),
        Err(prompt::ABORT.to_string())
    );
}

#[test]
fn execute_prompt_rejects_empty_query_and_non_tty() {
    let mut input = cursor("y\n");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "d", "", true, None),
        Err("Prompt (\"k\") has no query text!".to_string())
    );
    let mut input = cursor("y\n");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "d", "Go?", false, None),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
}

#[test]
fn execute_prompt_uses_saved_responses() {
    let mut input = cursor("");
    assert!(prompt::execute_prompt(&mut input, "k", "d", "Go?", true, Some("y")).is_ok());
    let mut input = cursor("");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "d", "Go?", true, Some("n")),
        Err(prompt::ABORT.to_string())
    );
}

#[test]
fn help_block_shape() {
    let help = prompt::prompt_help("arc.state.stash", "Does things.");
    assert!(
        help.starts_with("\n PROMPT  arc.state.stash\n\n"),
        "{help:?}"
    );
    assert!(help.contains("Does things.\n\n"), "{help:?}");
    assert!(
        help.contains("The default response to this prompt is \"n\".\n\n"),
        "{help:?}"
    );
    assert!(
        help.contains("  ?   Use \"*\" after a response"),
        "{help:?}"
    );
    assert!(help.ends_with("\n\n"), "{help:?}");
}

#[test]
fn saved_response_echo_shape() {
    assert_eq!(
        prompt::saved_response_echo("Go?", "y", "k"),
        " >>>  Go? <y>\n >>>  (Using saved response to prompt \"k\".)\n\n"
    );
}

// -( R5 TTY gating + prompt core )-----------------------------------------

#[test]
fn r5_land20_close_default_no_enter_aborts() {
    // `ArcanistCloseRevisionWorkflow.php:121` calls
    // `phutil_console_confirm($prompt)` with the default `$default_no`,
    // so Enter declines (`[y/N]`) and aborts the close.
    let mut input = cursor("\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Sure?", "[y/N]", true, true),
        Ok(false)
    );
}

#[test]
fn r5_lint8_confirm_rejects_yes_no_words() {
    // `phutil_console_confirm()` answers `y`/`n`/empty only; `yes` and
    // `no` re-prompt (`src/console/format.php:14-24`).
    let mut input = cursor("yes\nn\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Sure?", "[Y/n]", false, true),
        Ok(false)
    );
    let mut input = cursor("no\ny\n");
    assert_eq!(
        prompt::console_confirm_with_tty(&mut input, "Sure?", "[y/N]", true, true),
        Ok(true)
    );
}

#[test]
fn r5_core39_prompt_requires_tty() {
    // CLEAN: `phutil_console_prompt()` requires a TTY
    // (`src/console/format.php:46-52`), so the token prompt's gate
    // matches PHP; piped input throws after the echo.
    let mut input = cursor("cli-token\n");
    assert_eq!(
        prompt::console_prompt_with_tty(&mut input, "Paste API Token from that page:", false),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
}

#[test]
fn r5_core40_install_anyway_defaults_no() {
    // CLEAN: `ArcanistInstallCertificateWorkflow.php:126` calls
    // `phutil_console_confirm()` with the default `$default_no`, so
    // Enter declines (`[y/N]`) and the token is not installed. (The
    // default-YES confirm at `ArcanistConfigurationManager.php:198` is
    // the chmod prompt, which Rust does not implement — T1153 CORE-41.)
    let mut input = cursor("\n");
    assert_eq!(
        prompt::console_confirm_with_tty(
            &mut input,
            "Install this token anyway?",
            "[y/N]",
            true,
            true
        ),
        Ok(false)
    );
}

#[test]
fn confirm_routes_through_the_live_tty_flag() {
    // `console_confirm` reads the real stdin TTY flag, then behaves
    // exactly like the injected-flag form: EOF answers the default on
    // a TTY, and a piped stdin fails after echoing the prompt once.
    // Cargo-test stdin is never a TTY; fail loud (rather than branch
    // on a live terminal) if that ever changes.
    assert!(
        !prompt::stdin_is_terminal(),
        "tests must run with non-TTY stdin"
    );
    for (default_no, options) in [(true, "[y/N]"), (false, "[Y/n]")] {
        let mut live = cursor("");
        let mut injected = cursor("");
        let result = prompt::console_confirm(&mut live, "Proceed?", default_no);
        assert_eq!(result, Err(prompt::NOT_INTERACTIVE.to_string()));
        assert_eq!(
            result,
            prompt::console_confirm_with_tty(&mut injected, "Proceed?", options, default_no, false)
        );
    }
}

#[test]
fn select_fails_fast_without_a_tty() {
    let mut input = cursor("1\n");
    assert_eq!(
        prompt::console_select_with_tty(&mut input, "Pick", 1, 3, false),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
}

#[test]
fn prompt_loop_ignores_unknown_answers() {
    let mut input = cursor("bogus\n?\ny\n");
    assert_eq!(
        prompt::execute_prompt(&mut input, "k", "desc", "Stash?", true, None),
        Ok(())
    );
}

// -( T1191 shared prompt reader )--------------------------------------------

/// Always-failing `BufRead` (T1176 `FailingReader` idiom): proves error
/// mapping where it IS read, and gate-before-read order where it must
/// NOT be read (a read would resolve to a default instead of erroring).
struct FailingReader;
impl std::io::Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("boom"))
    }
}
impl std::io::BufRead for FailingReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("boom"))
    }
    fn consume(&mut self, _amount: usize) {}
}

#[test]
fn t1191_read_answer_splits_lf_and_trims_cr() {
    // LF-only (no CR): the Windows ConPTY-injection shape — completes
    // like any newline, never blocks.
    let mut lf = cursor("y\n");
    assert_eq!(prompt::read_answer(&mut lf), Some("y".to_string()));
    // CRLF (the Enter key on every console).
    let mut crlf = cursor("n\r\n");
    assert_eq!(prompt::read_answer(&mut crlf), Some("n".to_string()));
    // Lone CR at EOF and unterminated final lines still answer.
    let mut lone_cr = cursor("y\r");
    assert_eq!(prompt::read_answer(&mut lone_cr), Some("y".to_string()));
    let mut bare = cursor("y");
    assert_eq!(prompt::read_answer(&mut bare), Some("y".to_string()));
    // An empty line answers empty (callers map it to the default);
    // EOF answers None.
    let mut empty_line = cursor("\n");
    assert_eq!(prompt::read_answer(&mut empty_line), Some(String::new()));
    let mut eof = cursor("");
    assert_eq!(prompt::read_answer(&mut eof), None);
    // Multi-line input reads one line per call.
    let mut two = cursor("a\nb\n");
    assert_eq!(prompt::read_answer(&mut two), Some("a".to_string()));
    assert_eq!(prompt::read_answer(&mut two), Some("b".to_string()));
    assert_eq!(prompt::read_answer(&mut two), None);
    // Invalid UTF-8 fails the line read, like any read error.
    let mut bad = std::io::Cursor::new([0xffu8, b'\n']);
    assert_eq!(prompt::read_answer(&mut bad), None);
}

#[test]
fn t1191_read_answer_maps_errors_to_none() {
    assert_eq!(prompt::read_answer(&mut FailingReader), None);
    // Supertrait stubs never run via `read_line` (it only needs
    // `fill_buf`), so drive them directly (T1176 idiom).
    let mut reader = FailingReader;
    assert!(std::io::Read::read(&mut reader, &mut [0u8; 4]).is_err());
    std::io::BufRead::consume(&mut reader, 1);
}

#[test]
fn t1191_read_answer_result_reports_eof_and_errors() {
    // The shared core surfaces the read outcome: lines, EOF, errors.
    let mut lf = cursor("y\n");
    assert_eq!(
        prompt::read_answer_result(&mut lf).unwrap(),
        Some("y".to_string())
    );
    let mut eof = cursor("");
    assert_eq!(prompt::read_answer_result(&mut eof).unwrap(), None);
    assert!(prompt::read_answer_result(&mut FailingReader).is_err());
}

#[test]
fn t1191_live_stdin_lock_returns_a_lock() {
    // Construction only: stdin is never a TTY under test, so this just
    // exercises the helper (and, on Windows, the guard write).
    let mut lock = prompt::live_stdin_lock();
    let _ = &mut lock;
}

#[test]
#[cfg(windows)]
fn t1191_win32_input_disable_sequence_shape() {
    // `CSI ? 9001 l`: leave win32-input mode (key-event reporting).
    assert_eq!(prompt::WIN32_INPUT_DISABLE, "\x1b[?9001l");
}

#[test]
fn t1191_prompts_refuse_piped_stdin_without_reading() {
    // The not-interactive gate precedes every read: had any prompt read
    // the failing input, the error would have resolved to a default
    // (Ok) instead of refusing.
    assert_eq!(
        prompt::console_confirm_with_tty(&mut FailingReader, "Proceed?", "[y/N]", true, false),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
    assert_eq!(
        prompt::console_select_with_tty(&mut FailingReader, "Pick", 1, 3, false),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
    assert_eq!(
        prompt::console_prompt_with_tty(&mut FailingReader, "Name?", false),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
    assert_eq!(
        prompt::execute_prompt(&mut FailingReader, "k", "d", "Go?", false, None),
        Err(prompt::NOT_INTERACTIVE.to_string())
    );
}
