use crate::term::*;

fn populated() -> Terminal {
    let mut terminal = Terminal::new(8, 4);
    for (index, letter) in (b'A'..=b'J').enumerate() {
        if index > 0 {
            terminal.process(b"\r\n");
        }
        terminal.process(&[letter]);
        terminal.handle_graphics_command(
            format!(
                "a=T,f=32,s=1,v=1,i={},p=1,c=1,r=1,C=1,q=2;AQIDBA==",
                index + 1
            )
            .as_bytes(),
        );
    }
    terminal
}

#[test]
fn history_budget_reduction_retires_only_unretained_primary_image_placements() {
    for alternate in [false, true] {
        for lines in [0, 1, 3, 6, 20] {
            for budget_divisor in [1, 2, usize::MAX] {
                let mut terminal = populated();
                let budget = terminal.grid().memory_usage().scrollback_bytes / budget_divisor;
                let before: Vec<_> = terminal.graphics_placements().cloned().collect();
                if alternate {
                    terminal.enter_alt_screen();
                    terminal
                        .handle_graphics_command(b"a=T,f=32,s=1,v=1,i=50,c=1,r=1,C=1,q=2;AQIDBA==");
                }
                terminal.set_scrollback_limits(lines, budget);
                let oldest = -(terminal.grid.scrollback_len() as i64);
                for original in before {
                    let actual = terminal
                        .graphics
                        .placements(false)
                        .find(|p| p.image_id == original.image_id);
                    if original.row < oldest {
                        assert!(actual.is_none(), "evicted image remained: {original:?}");
                    } else {
                        assert_eq!(format!("{:?}", actual.unwrap()), format!("{original:?}"));
                        let letter = char::from(b'A' + original.image_id.0 as u8 - 1);
                        assert_eq!(
                            terminal
                                .grid
                                .row_at_offset(original.row as i32)
                                .unwrap()
                                .to_string(),
                            letter.to_string()
                        );
                    }
                    // Kitty image data is reusable independently of placements.
                    assert!(terminal.get_image(original.image_id).is_some());
                }
                if alternate {
                    assert_eq!(terminal.graphics_placements().count(), 1);
                    assert_eq!(
                        terminal.graphics_placements().next().unwrap().image_id,
                        ImageId(50)
                    );
                }
            }
        }
    }
}

#[test]
fn clear_history_removes_placement_groups_but_keeps_live_and_reusable_images() {
    for csi in [false, true] {
        let mut terminal = populated();
        terminal.handle_graphics_command(b"a=t,f=32,s=1,v=1,i=20,q=2;AQIDBA==");
        terminal.handle_graphics_command(b"a=p,i=20,p=1,P=1,Q=1,V=8,c=1,r=1,q=2");
        terminal.handle_graphics_command(b"a=T,U=1,f=32,s=1,v=1,i=30,c=1,r=1,q=2;AQIDBA==");
        terminal.process("\x1b[2;1H\x1b[38;5;30m\u{10eeee}\x1b[0m".as_bytes());
        let visible_text = terminal.rows_text(0, 3);
        assert!(
            terminal
                .presentation_images()
                .iter()
                .any(|p| p.placement.image_id == ImageId(20) && p.placement.row == 2)
        );
        if csi {
            terminal.process(b"\x1b[3J");
        } else {
            terminal.clear_scrollback();
        }
        assert_eq!(terminal.grid().scrollback_len(), 0);
        assert_eq!(terminal.rows_text(0, 3), visible_text);
        assert_eq!(
            terminal
                .graphics_placements()
                .map(|p| p.image_id.0)
                .collect::<Vec<_>>(),
            vec![7, 8, 9, 10]
        );
        assert!(terminal.get_image(ImageId(20)).is_none());
        assert!(
            terminal
                .presentation_images()
                .iter()
                .any(|p| p.placement.image_id == ImageId(30))
        );
        for id in 1..=10 {
            assert!(terminal.get_image(ImageId(id)).is_some());
        }
        terminal.set_scrollback_limits(100, 1024 * 1024);
        terminal.process(b"\x1b[4;1H\n\n\n");
        terminal.grid_mut().set_scroll_offset(3);
        assert!(
            terminal
                .presentation_images()
                .iter()
                .all(|p| p.placement.image_id.0 > 6)
        );
        // A later explicit display can still use the transmitted Kitty data.
        terminal.handle_graphics_command(b"a=p,i=1,p=2,c=1,r=1,C=1,q=2");
        assert!(
            terminal
                .graphics_placements()
                .any(|p| p.image_id == ImageId(1) && p.placement_id == Some(2))
        );
    }
}

#[test]
fn retired_anonymous_history_pixels_live_only_until_frozen_presentation_releases() {
    for operation in 0..3 {
        let mut terminal = Terminal::new(8, 4);
        terminal.set_pixel_geometry(16, 12);
        terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[3S");
        let id = terminal.graphics_placements().next().unwrap().image_id;
        let pixels = std::sync::Arc::downgrade(&terminal.get_image(id).unwrap().data);
        assert_eq!(terminal.graphics_memory_usage(), 4 * 6 * 4);
        terminal.grid_mut().set_scroll_offset(3);
        terminal.process(b"\x1b[?2026h");
        let frozen_placement = terminal.presentation_images()[0].placement.clone();
        match operation {
            0 => terminal.clear_scrollback(),
            1 => terminal.process(b"\x1b[3J"),
            _ => terminal.set_scrollback_limits(0, 0),
        }
        assert_eq!(terminal.graphics_image_count(), 0);
        assert_eq!(terminal.graphics_memory_usage(), 0);
        assert!(terminal.graphics_placements().next().is_none());
        assert_eq!(terminal.grid().scrollback_len(), 0);
        // The frozen grid owns only the captured viewport, not a history copy.
        assert_eq!(terminal.presentation_grid().scrollback_len(), 0);
        assert_eq!(
            format!("{:?}", terminal.presentation_images()[0].placement),
            format!("{frozen_placement:?}")
        );
        assert!(pixels.upgrade().is_some());
        terminal.process(b"\x1b[?2026l");
        assert!(terminal.presentation_images().is_empty());
        assert!(pixels.upgrade().is_none());
    }
}

#[test]
fn history_cleanup_preserves_partially_retained_images_and_other_screen_ownership() {
    let mut terminal = Terminal::new(8, 4);
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=3,C=1,q=2;AQIDBA==");
    terminal.process(b"\x1b[2S");
    let original = terminal.graphics_placements().next().unwrap().clone();
    assert_eq!(original.row, -2);
    terminal.clear_scrollback();
    assert_eq!(
        format!("{:?}", terminal.graphics_placements().next().unwrap()),
        format!("{original:?}")
    );
    terminal.set_scrollback_limits(0, 0);
    assert_eq!(
        format!("{:?}", terminal.graphics_placements().next().unwrap()),
        format!("{original:?}")
    );

    let mut terminal = populated();
    let primary: Vec<_> = terminal.graphics_placements().cloned().collect();
    terminal.enter_alt_screen();
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=50,c=1,r=1,C=1,q=2;AQIDBA==");
    terminal.process(b"\x1b[3J");
    terminal.clear_scrollback();
    assert_eq!(terminal.graphics_placements().count(), 1);
    terminal.exit_alt_screen();
    assert_eq!(
        format!("{:?}", terminal.graphics_placements().collect::<Vec<_>>()),
        format!("{primary:?}")
    );
    assert_eq!(terminal.grid().scrollback_len(), 6);
}
