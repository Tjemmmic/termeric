use base64::engine::general_purpose::STANDARD;

use crate::term::*;

/// Six rows of ten 10x20-pixel cells, every row written, the cursor on the
/// last one.
fn full_screen() -> Terminal {
    let mut terminal = Terminal::new(10, 6);
    terminal.set_pixel_geometry(100, 120);
    terminal.process(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r");
    terminal
}

fn cursor(terminal: &Terminal) -> (usize, usize) {
    let cursor = terminal.grid().cursor();
    (cursor.row, cursor.col)
}

fn row_text(terminal: &Terminal, row: usize) -> String {
    terminal
        .grid()
        .row(row)
        .unwrap()
        .to_string()
        .trim_end()
        .to_owned()
}

#[test]
fn a_kitty_image_leaves_the_cursor_on_its_last_row_after_its_last_column() {
    let mut terminal = Terminal::new(10, 6);
    terminal.set_pixel_geometry(100, 120);
    terminal.grid_mut().set_cursor(1, 2);
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=3,r=2,q=2;AQIDBA==");
    assert_eq!(cursor(&terminal), (2, 5));

    terminal.grid_mut().set_cursor(3, 8);
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=2,c=2,r=1,q=2;AQIDBA==");
    assert_eq!(cursor(&terminal), (4, 0), "past the right edge wraps");
}

#[test]
fn a_kitty_image_at_the_bottom_scrolls_into_view_above_the_next_output() {
    let mut terminal = full_screen();
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=2,r=3,q=2;AQIDBA==");

    let placement = terminal.graphics_placements().next().unwrap();
    assert_eq!(placement.row, 3, "it scrolled up with the row it began on");
    assert_eq!(row_text(&terminal, 3), "6");
    assert_eq!(terminal.grid().scrollback_len(), 2);
    assert_eq!(cursor(&terminal), (5, 2));

    terminal.process(b"\r\n$ ");
    let placement = terminal.graphics_placements().next().unwrap();
    assert_eq!(
        placement.row + 3,
        5,
        "the prompt follows the image's last row"
    );
    assert_eq!(row_text(&terminal, 5), "$");
}

#[test]
fn an_animation_redrawn_by_moving_up_its_height_stays_in_place() {
    // timg draws each frame, starts a new line, and moves back up the
    // frame's height, relying on the cursor ending on the image's last row.
    let mut terminal = full_screen();
    for frame in 0..6 {
        let id = 1 + frame % 2;
        terminal.handle_graphics_command(
            format!("a=T,f=32,s=1,v=1,i={id},c=2,r=3,q=2;AQIDBA==").as_bytes(),
        );
        terminal.process(b"\r\n\x1b[3A");
    }

    let rows: Vec<_> = terminal.graphics_placements().map(|p| p.row).collect();
    assert_eq!(rows, [2, 2]);
    assert_eq!(cursor(&terminal), (2, 0));
}

#[test]
fn a_sixel_image_at_the_bottom_scrolls_and_leaves_the_cursor_below_it() {
    let mut terminal = full_screen();
    terminal.process(b"\x1b[6;4H\x1bP0;1q\"1;1;4;24#1;2;100;0;0!4~-!4~-!4~-!4~\x1b\\");

    let placement = terminal.graphics_placements().next().unwrap();
    assert_eq!(placement.height, Some(2));
    assert_eq!((placement.row, placement.col), (3, 3));
    assert_eq!(row_text(&terminal, 3), "6");
    assert_eq!(cursor(&terminal), (5, 3));
}

#[test]
fn an_iterm2_image_at_the_bottom_scrolls_and_leaves_the_cursor_below_it() {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0, 255]).unwrap();
    }
    let mut terminal = full_screen();
    terminal.process(
        format!(
            "\x1b]1337;File=inline=1;width=2;height=2;preserveAspectRatio=0:{}\x07",
            STANDARD.encode(png_bytes)
        )
        .as_bytes(),
    );

    let placement = terminal.graphics_placements().next().unwrap();
    assert_eq!((placement.row, placement.height), (3, Some(2)));
    assert_eq!(row_text(&terminal, 3), "6");
    assert_eq!(cursor(&terminal), (5, 0));
}
