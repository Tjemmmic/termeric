use super::*;

fn populated(limit: usize) -> Terminal {
    let mut terminal = Terminal::new(8, 4);
    terminal.grid_mut().set_scrollback_limit(limit);
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
    terminal.process(b"\x1b[1;3r");
    terminal
}

#[test]
fn archival_graphics_follow_retained_text_above_a_fixed_footer() {
    for limit in [0, 6, 20] {
        for count in [1, 2, 3] {
            for command in [
                format!("\x1b[{count}S"),
                format!("\x1b[{count}M"),
                format!("\x1b[3;1H{}", "\n".repeat(count)),
            ] {
                let mut terminal = populated(limit);
                let before: Vec<_> = terminal.graphics_placements().cloned().collect();
                terminal.process(command.as_bytes());
                let oldest = -(terminal.grid().scrollback_len() as i64);
                let mut expected_count = 0;
                for original in before {
                    let expected_row = if original.row < 3 {
                        original.row - count as i64
                    } else {
                        original.row
                    };
                    let actual = terminal
                        .graphics_placements()
                        .find(|p| p.image_id == original.image_id);
                    if expected_row < oldest {
                        assert!(actual.is_none(), "evicted image survived: {original:?}");
                        continue;
                    }
                    expected_count += 1;
                    let actual = actual.expect("retained row must keep its image");
                    let mut expected = original.clone();
                    expected.row = expected_row;
                    assert_eq!(
                        format!("{actual:?}"),
                        format!("{expected:?}"),
                        "{command:?}"
                    );
                    let letter = char::from(b'A' + original.image_id.0 as u8 - 1);
                    assert_eq!(
                        terminal
                            .grid()
                            .row_at_offset(expected_row as i32)
                            .unwrap()
                            .to_string(),
                        letter.to_string()
                    );
                    assert_eq!(
                        terminal.get_image(original.image_id).unwrap().data.as_ref(),
                        [1, 2, 3, 4]
                    );
                }
                assert_eq!(terminal.graphics_placements().count(), expected_count);
            }
        }
    }
}

#[test]
fn archival_graphics_preserve_source_geometry_and_boundary_crossing_policy() {
    let mut terminal = populated(20);
    terminal.process(b"\x1b[1;1H");
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=20,c=1,r=2,C=1,q=2;AQIDBA==");
    terminal.process(b"\x1b[3;1H");
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=21,c=1,r=2,C=1,q=2;AQIDBA==");
    terminal.process(b"\x1b[1;1H");
    let before: Vec<_> = terminal
        .graphics_placements()
        .filter(|p| p.image_id.0 >= 20)
        .cloned()
        .collect();
    for step in 1..=4 {
        terminal.process(b"\x1b[S");
        for original in &before {
            let mut expected = original.clone();
            if original.image_id == ImageId(20) {
                expected.row -= step;
            }
            let actual = terminal
                .graphics_placements()
                .find(|p| p.image_id == original.image_id)
                .unwrap();
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
    }
}

#[test]
fn relative_children_follow_archived_or_stationary_parents_and_cascade_on_eviction() {
    for limit in [0, 6, 20] {
        for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
            let mut terminal = populated(limit);
            for (id, parent, offset) in [(20, 7, 1), (21, 10, -1)] {
                terminal.handle_graphics_command(
                    format!("a=t,f=32,s=1,v=1,i={id},q=2;AQIDBA==").as_bytes(),
                );
                terminal.handle_graphics_command(
                    format!("a=p,i={id},p=1,P={parent},Q=1,H=1,V={offset},c=1,r=1,q=2").as_bytes(),
                );
            }
            terminal.process(command);
            let presentations = terminal.presentation_images();
            let child = presentations
                .iter()
                .find(|p| p.placement.image_id == ImageId(20));
            if limit == 0 {
                assert!(child.is_none());
                assert!(terminal.get_image(ImageId(20)).is_none());
            } else {
                assert_eq!(child.unwrap().placement.row, 0);
                assert_eq!(child.unwrap().placement.col, 2);
            }
            // A child inside the moving region does not move independently
            // when its parent is in the unchanged footer.
            let footer_child = presentations
                .iter()
                .find(|p| p.placement.image_id == ImageId(21))
                .unwrap();
            assert_eq!(footer_child.placement.row, 2);
            assert_eq!(footer_child.placement.col, 2);
        }
    }
}

#[test]
fn archival_graphics_honor_byte_eviction_and_release_anonymous_pixels_after_snapshot() {
    for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
        let mut terminal = populated(20);
        let budget = terminal.grid().memory_usage().scrollback_bytes;
        terminal.set_scrollback_limits(20, budget);
        terminal
            .grid_mut()
            .cell_mut(0, 0)
            .unwrap()
            .set_grapheme(&format!("G{}", "\u{301}".repeat(budget / 4)));
        terminal.process(command);
        let oldest = -(terminal.grid().scrollback_len() as i64);
        assert!(oldest > -6);
        for id in 1..=10 {
            let expected_row = if id == 10 { 3 } else { i64::from(id) - 8 };
            let placement = terminal
                .graphics_placements()
                .find(|p| p.image_id == ImageId(id));
            assert_eq!(
                placement.map(|p| p.row),
                (expected_row >= oldest).then_some(expected_row)
            );
        }
    }

    let mut terminal = Terminal::new(8, 4);
    terminal.set_pixel_geometry(16, 12);
    terminal.set_scrollback_limits(1, 1024);
    terminal.process(b"\x1b[1;3r\x1bPq\"1;1!4~\x1b\\");
    let original = terminal.graphics_placements().next().unwrap().clone();
    assert_eq!(
        (
            original.height,
            original.pixel_height,
            original.source_height
        ),
        (Some(2), Some(6), Some(6))
    );
    let pixels = terminal.get_image(original.image_id).unwrap().data.clone();
    terminal.process(b"\x1b[?2026h");
    for step in 1..=3 {
        terminal.process(b"\x1b[S");
        if step < 3 {
            let actual = terminal.graphics_placements().next().unwrap();
            let mut expected = original.clone();
            expected.row -= step;
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
            assert!(std::sync::Arc::ptr_eq(
                &pixels,
                &terminal.get_image(original.image_id).unwrap().data
            ));
        } else {
            assert!(terminal.graphics_placements().next().is_none());
            assert!(terminal.get_image(original.image_id).is_none());
            assert_eq!(terminal.graphics.memory_usage(), 0);
        }
        let frozen = terminal.presentation_images();
        assert_eq!(frozen.len(), 1);
        assert_eq!(frozen[0].placement.row, original.row);
        assert!(std::sync::Arc::ptr_eq(&pixels, &frozen[0].image.data));
    }
    terminal.process(b"\x1b[?2026l");
    assert!(terminal.presentation_images().is_empty());
}

#[test]
fn nonarchival_graphics_stay_isolated_and_full_height_delete_archives_tall_images() {
    for command in [
        b"\x1b[2;3r\x1b[S".as_slice(),
        b"\x1b[2;1H\x1b[M",
        b"\x1b[?69h\x1b[2;6s\x1b[S",
        b"\x1b[?69h\x1b[2;6s\x1b[1;2H\x1b[M",
        b"\x1b[?1049h\x1b[S\x1b[1;1H\x1b[M\x1b[?1049l",
    ] {
        let mut terminal = populated(20);
        let historical: Vec<_> = terminal
            .graphics_placements()
            .filter(|p| p.row < 0)
            .cloned()
            .collect();
        terminal.process(command);
        let after: Vec<_> = terminal
            .graphics_placements()
            .filter(|p| p.row < 0)
            .cloned()
            .collect();
        assert_eq!(format!("{after:?}"), format!("{historical:?}"));
    }
    for command in [b"\x1b[S".as_slice(), b"\x1b[M"] {
        let mut terminal = Terminal::new(8, 4);
        terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=8,C=1,q=2;AQIDBA==");
        terminal.process(command);
        let placement = terminal.graphics_placements().next().unwrap();
        assert_eq!(placement.row, -1);
        assert_eq!(placement.height, Some(8));
        assert_eq!(placement.source_height, Some(1));
    }
    // Alternate-screen row-zero scrolling discards content, never archives it.
    let mut terminal = Terminal::new(8, 4);
    terminal.process(b"\x1b[?1049h\x1b[1;3r");
    terminal.handle_graphics_command(b"a=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1,q=2;AQIDBA==");
    terminal.process(b"\x1b[S");
    assert!(terminal.graphics_placements().next().is_none());
    assert_eq!(terminal.grid().scrollback_len(), 0);
}
