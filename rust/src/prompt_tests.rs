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
    // the chmod prompt, which Rust does not implement — CORE-41.)
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
