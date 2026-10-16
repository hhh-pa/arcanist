use super::*;

#[test]
fn splits_lines_without_endings() {
    assert_eq!(split_lines(""), vec![""]);
    assert_eq!(split_lines("a"), vec!["a"]);
    assert_eq!(split_lines("a\n"), vec!["a"]);
    assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
    assert_eq!(split_lines("a\nb\n"), vec!["a", "b"]);
    assert_eq!(split_lines("a\r\nb\r\n"), vec!["a", "b"]);
    assert_eq!(split_lines("a\n\n"), vec!["a", ""]);
    assert_eq!(split_lines("\n"), vec![""]);
}

#[test]
fn measures_ascii_width_by_bytes() {
    assert_eq!(console_width(""), 0);
    assert_eq!(console_width("hello"), 5);
    assert_eq!(console_width("\x01\x7f"), 2);
}

#[test]
fn measures_box_drawing_as_narrow() {
    // Tree marks used by the markers grid are single-column glyphs.
    assert_eq!(console_width("• "), 2);
    assert_eq!(console_width("└ "), 2);
    assert_eq!(console_width("│ "), 2);
    assert_eq!(console_width("┗━┓ "), 4);
}

#[test]
fn measures_wide_characters_as_two_columns() {
    assert_eq!(console_width("日"), 2);
    assert_eq!(console_width("a日b"), 4);
    assert_eq!(console_width("한"), 2);
}

#[test]
fn combining_characters_add_no_width() {
    assert_eq!(console_width("é"), 1);
    assert_eq!(console_width("́"), 1);
}

#[test]
fn strips_ansi_sequences_before_measuring() {
    assert_eq!(console_width("\x1b[1mok\x1b[m"), 2);
    assert_eq!(console_width("\x1b[31mred"), 3);
    // A bare ESC byte is not a sequence.
    assert_eq!(console_width("\x1b"), 1);
}

#[test]
fn lowercases_unicode() {
    assert_eq!(utf8_lower("AbC"), "abc");
    assert_eq!(utf8_lower("ÄÖ"), "äö");
}

#[test]
fn groups_thousands() {
    assert_eq!(group_number(0), "0");
    assert_eq!(group_number(999), "999");
    assert_eq!(group_number(1000), "1,000");
    assert_eq!(group_number(1_234_567), "1,234,567");
}

#[test]
fn truncates_short_strings_unchanged() {
    assert_eq!(truncate_glyphs("hello", 5), "hello");
    assert_eq!(truncate_glyphs("hello", 80), "hello");
    assert_eq!(truncate_glyphs("", 0), "");
}

#[test]
fn truncates_long_words_with_terminator() {
    assert_eq!(truncate_glyphs("abcdefghij", 8), "abcdefg…");
    assert_eq!(truncate_glyphs("abcdefghij", 1), "…");
}

#[test]
fn truncates_at_word_boundaries() {
    assert_eq!(truncate_glyphs("hello world", 8), "hello…");
}

#[test]
fn rbd35_odd_cutoff_searches_from_the_ceiling() {
    // T1153 RB-D35: PHP's `$cutoff / 2` is float division, so with
    // maximum 8 (cutoff 7) the break search starts at glyph 4, not 3:
    // the space at 3 is out of reach and the cut lands at 7.
    assert_eq!(truncate_glyphs("abc defghijklmnop", 8), "abc def…");
}

#[test]
fn truncates_at_stop_characters_without_terminator() {
    assert_eq!(truncate_glyphs("hello. bye friend", 8), "hello.");
}

#[test]
fn counts_glyphs_not_bytes() {
    assert_eq!(truncate_glyphs("aé日b", 4), "aé日b");
    assert_eq!(truncate_glyphs("aé日b", 3), "aé…");
}

fn markers_grid() -> GridView {
    let mut grid = GridView::new();
    grid.new_column("marker");
    grid.new_column("commits");
    grid.new_column("status");
    grid.new_column("revisions");
    grid.new_column("messages").set_minimum_width(12);
    grid
}

#[test]
fn draws_padded_cells_with_two_space_gutters() {
    let mut grid = markers_grid();
    grid.new_row(vec![
        GridCell::new("marker", vec!["• * main\n".to_string()]),
        GridCell::new("commits", vec!["abc1234\n".to_string()]),
        GridCell::new("status", vec!["\n".to_string()]),
        GridCell::new("revisions", vec!["\n".to_string()]),
        GridCell::new("messages", vec!["first\n".to_string()]),
    ]);
    let text = grid.draw(Some(120));
    // Gutter run between commits and messages: 2 + 1 + 2 + 1 + 2 spaces.
    assert_eq!(text, format!("• * main  abc1234{}first\n", " ".repeat(8)));
}

#[test]
fn shrinks_flexible_columns_to_minimum_without_a_terminal() {
    let mut grid = markers_grid();
    grid.new_row(vec![
        GridCell::new("marker", vec!["• * main\n".to_string()]),
        GridCell::new("commits", vec!["abc1234\n".to_string()]),
        GridCell::new("status", vec!["\n".to_string()]),
        GridCell::new("revisions", vec!["\n".to_string()]),
        GridCell::new(
            "messages",
            vec!["a much longer commit message here\n".to_string()],
        ),
    ]);
    // No terminal width: available space is negative, so the messages
    // column shrinks to its minimum width of 12. Truncated lines are not
    // re-padded, matching the single PHP pad/truncate pass.
    let text = grid.draw(None);
    assert_eq!(text, format!("• * main  abc1234{}a much…\n", " ".repeat(8)));
}

#[test]
fn stacks_multiline_cells_and_pads_missing_lines() {
    let mut grid = GridView::new();
    grid.new_column("left");
    grid.new_column("right");
    grid.new_row(vec![
        GridCell::new("left", vec!["one\ntwo\n".to_string()]),
        GridCell::new("right", vec!["1\n".to_string()]),
    ]);
    assert_eq!(grid.draw(Some(80)), "one  1\ntwo   \n");
}

#[test]
fn pads_and_truncates_center_and_right_columns() {
    let mut grid = GridView::new();
    grid.new_column("a").set_alignment(Alignment::Center);
    grid.new_column("b").set_alignment(Alignment::Right);
    grid.new_row(vec![
        GridCell::new("a", vec!["x\n".to_string()]),
        GridCell::new("b", vec!["yy\n".to_string()]),
    ]);
    grid.new_row(vec![
        GridCell::new("a", vec!["longer\n".to_string()]),
        GridCell::new("b", vec!["z\n".to_string()]),
    ]);
    assert_eq!(grid.draw(Some(80)), "  x     yy\nlonger   z\n");
}

#[test]
fn empty_grid_renders_like_php() {
    // T1153 RB-D36: the empty row list string-casts to `Array`.
    let grid = markers_grid();
    assert_eq!(grid.draw(Some(80)), "Array\n");
}

#[test]
fn cell_width_ignores_ansi() {
    let cell = GridCell::new("status", vec!["\x1b[32mAccepted\x1b[m\n".to_string()]);
    assert_eq!(cell.content_width(), 8);
}
