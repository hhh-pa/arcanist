//! Tests for the shared help renderer (T1153 RB-D5).

use super::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

#[test]
fn rbd5_usage_text_matches_single_help_body() {
    // `usage_text` renders the same bytes `arc help <name>` prints for
    // one name: header, examples, synopsis, blank, help, blank, specs,
    // blank, blank.
    let text = usage_text(
        "paste",
        Some("**paste** [__options__] --\n**paste** [__options__] -- __object__"),
        None,
        Some("Share text."),
        true,
        &[("title", None, Some("title"), Some("Title for the paste."))],
        false,
    );
    assert!(text.starts_with("PASTE WORKFLOW\n\n"), "{text:?}");
    assert!(text.contains("paste [options] --\n"), "{text:?}");
    assert!(text.contains("Share text."), "{text:?}");
    assert!(text.contains("      --title title\n"), "{text:?}");
    assert!(text.ends_with("\n\n"), "{text:?}");
}

#[test]
fn rbd5_examples_default_and_unwrap() {
    // `None` examples fall back to `**name**`; toolset help unwraps
    // single newlines, classic keeps them.
    let toolset = usage_text("weld", None, Some("Fuse."), Some("a\nb"), true, &[], false);
    assert!(toolset.contains("weld\n"), "{toolset:?}");
    assert!(toolset.contains("a b"), "{toolset:?}");
    let classic = usage_text("diff", None, Some("  syn"), Some("a\nb"), false, &[], false);
    assert!(classic.contains("diff\n"), "{classic:?}");
    assert!(classic.contains("a\nb"), "{classic:?}");
}

#[test]
fn rbd5_missing_help_body_skips_help_section() {
    // `None` help renders no help block: header, examples, synopsis,
    // then the trailing blank.
    let text = usage_text("paste", None, Some("syn"), None, false, &[], false);
    assert!(text.starts_with("PASTE WORKFLOW\n\n"), "{text:?}");
    assert!(text.contains("syn\n"), "{text:?}");
    assert!(text.ends_with("\n\n"), "{text:?}");
}

#[test]
fn rbd5_specs_sort_by_name_with_shorts() {
    let text = render_argument_specs(
        &[
            ("message", Some("m"), Some("message"), Some("Use it.")),
            ("add-all", Some("a"), None, Some("Add all.")),
        ],
        false,
    );
    let add_at = text.find("--add-all").unwrap();
    let msg_at = text.find("--message").unwrap();
    assert!(add_at < msg_at, "{text:?}");
    assert!(text.contains("--add-all, -a"), "{text:?}");
    assert!(text.contains("--message message, -m message"), "{text:?}");
}

#[test]
fn b21_terminator_missing_needs_dashes_when_piped() {
    assert!(terminator_missing(&argv(&["P123"]), false));
    assert!(terminator_missing(&argv(&[]), false));
    assert!(!terminator_missing(&argv(&["--", "P123"]), false));
    assert!(!terminator_missing(&argv(&["P123"]), true));
    assert_eq!(
        MISSING_TERMINATOR,
        "Missing required \"--\" in argument list."
    );
}

#[test]
fn rbd5_bare_lead_kept_for_smoke_gate() {
    assert_eq!(
        BARE_HELP_LEAD,
        "usage: arc [--version] [--help] <command>\n\n"
    );
}
