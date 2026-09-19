use super::*;

#[test]
fn atomic_host_resize_updates_live_and_frozen_pixel_coverage_before_eviction() {
    for frozen in [false, true] {
        let mut terminal = Terminal::new(8, 5);
        terminal.set_scrollback_limits(0, 0);
        terminal.set_pixel_geometry(16, 15);
        terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[5;1HX");
        let id = terminal.graphics_placements().next().unwrap().image_id;
        let pixels = terminal.get_image(id).unwrap().data.clone();
        if frozen {
            terminal.process(b"\x1b[?2026h");
            terminal.presentation_grid_mut().set_scrollback_limit(0);
        }
        terminal.resize_with_pixel_geometry(8, 2, 8, 2);
        let live = terminal
            .graphics_placements()
            .next()
            .expect("new pixel footprint remains partly visible");
        assert_eq!((live.row, live.width, live.height), (-3, Some(4), Some(6)));
        assert_eq!(terminal.grid().scrollback_len(), 0);
        let presented = terminal.presentation_images();
        assert_eq!(presented.len(), 1);
        assert_eq!(
            (presented[0].placement.row, presented[0].placement.height),
            (-3, Some(6))
        );
        assert!(std::sync::Arc::ptr_eq(&pixels, &presented[0].image.data));
    }
}

#[test]
fn pixel_only_host_geometry_preserves_margins_and_cursor() {
    let mut terminal = Terminal::new(8, 5);
    terminal.process(b"\x1b[2;4r\x1b[?69h\x1b[2;7s\x1b[3;4H");
    let cursor = terminal.grid().cursor();
    let margins = (
        terminal.scroll_top,
        terminal.scroll_bottom,
        terminal.scroll_left,
        terminal.scroll_right,
    );
    assert_eq!(margins, (1, 4, 1, 7));
    terminal.resize_with_pixel_geometry(8, 5, 24, 20);
    assert_eq!(
        (
            terminal.scroll_top,
            terminal.scroll_bottom,
            terminal.scroll_left,
            terminal.scroll_right
        ),
        margins
    );
    assert_eq!(
        (terminal.grid().cursor().row, terminal.grid().cursor().col),
        (cursor.row, cursor.col)
    );
    assert_eq!(
        (terminal.cell_pixel_width, terminal.cell_pixel_height),
        (3, 4)
    );
}

#[test]
fn frozen_pixel_images_update_retention_footprints_when_cells_shrink() {
    let mut terminal = Terminal::new(8, 5);
    terminal.set_pixel_geometry(16, 15);
    terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[5;1HX");
    let original = terminal.graphics_placements().next().unwrap().clone();
    let pixels = std::sync::Arc::downgrade(&terminal.get_image(original.image_id).unwrap().data);
    assert_eq!((original.width, original.height), (Some(2), Some(2)));
    terminal.process(b"\x1b[?2026h");
    terminal.handle_graphics_command(b"a=d,d=A,q=2");
    assert!(terminal.get_image(original.image_id).is_none());
    // The frozen copy is the only remaining pixel owner. Its viewport may
    // shrink without retaining any captured history.
    terminal.presentation_grid_mut().set_scrollback_limit(0);
    terminal.set_pixel_geometry(8, 5);
    {
        let images = terminal.presentation_images();
        assert_eq!(images.len(), 1);
        let updated = &images[0].placement;
        assert_eq!((updated.width, updated.height), (Some(4), Some(6)));
        assert_eq!(
            (updated.pixel_width, updated.pixel_height),
            (Some(4), Some(6))
        );
        assert_eq!(
            (updated.source_width, updated.source_height),
            (original.source_width, original.source_height)
        );
    }
    terminal.resize(8, 2);
    {
        let images = terminal.presentation_images();
        assert_eq!(
            images.len(),
            1,
            "partly visible pixels must not be evicted using the old footprint"
        );
        assert_eq!(images[0].placement.row, -3);
        assert_eq!(images[0].placement.height, Some(6));
    }
    assert!(pixels.upgrade().is_some());
    terminal.process(b"\x1b[?2026l");
    assert!(pixels.upgrade().is_none());
}

fn populate(terminal: &mut Terminal, start: u32, letters: &[u8]) {
    for (index, letter) in letters.iter().enumerate() {
        if index > 0 {
            terminal.process(b"\r\n");
        }
        terminal.process(&[*letter]);
        terminal.handle_graphics_command(
            format!(
                "a=T,i={},p=1,c=1,r=1,C=1,f=32,s=1,v=1,q=2;AQIDBA==",
                start + index as u32
            )
            .as_bytes(),
        );
    }
}

#[test]
fn column_resize_keeps_coordinates_but_retires_unretained_image_roots() {
    for alternate_active in [false, true] {
        for columns in [2, 8, 16] {
            for rows in [2, 3, 5] {
                let mut terminal = Terminal::new(4, 3);
                terminal.set_scrollback_limits(100, 1024 * 1024);
                // One short root and one tall root share an origin. The tall
                // raster still intersects retained rows after history shrinks.
                for (id, height) in [(1, 1), (2, 12)] {
                    terminal.handle_graphics_command(
                        format!("a=T,i={id},p=1,c=2,r={height},C=1,f=32,s=1,v=1,q=2;AQIDBA==")
                            .as_bytes(),
                    );
                }
                terminal.process(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV");
                assert_eq!(terminal.grid().scrollback_len(), 9);
                terminal.handle_graphics_command(b"a=t,i=3,f=32,s=1,v=1,q=2;AQIDBA==");
                terminal.handle_graphics_command(b"a=p,i=3,P=1,Q=1,H=1,V=10,c=1,r=1,q=2");
                terminal.handle_graphics_command(b"a=T,i=4,c=2,r=2,C=1,f=32,s=1,v=1,q=2;AQIDBA==");
                let before: Vec<_> = terminal.graphics.placements(false).cloned().collect();
                if alternate_active {
                    terminal.enter_alt_screen();
                    populate(&mut terminal, 50, b"XYZ");
                }
                let alternate_before: Vec<_> =
                    terminal.graphics.placements(true).cloned().collect();
                terminal.resize(columns, rows);
                let oldest = -(terminal.grid.scrollback_len() as i64);
                let root_retained = -9 + 1 > oldest;
                for original in before {
                    let retained = if original.image_id == ImageId(3) {
                        root_retained
                    } else {
                        original.row + i64::from(original.height.unwrap_or(1)) > oldest
                    };
                    let actual = terminal
                        .graphics
                        .placements(false)
                        .find(|p| p.image_id == original.image_id);
                    assert_eq!(
                        actual.is_some(),
                        retained,
                        "cols={columns} rows={rows} oldest={oldest} image={original:?}"
                    );
                    if let Some(actual) = actual {
                        assert_eq!(format!("{actual:?}"), format!("{original:?}"));
                    }
                }
                assert_eq!(terminal.get_image(ImageId(3)).is_some(), root_retained);
                // Explicit Kitty payloads remain reusable; retired relatives
                // are cascade-owned, not independent reusable root caches.
                assert!(terminal.get_image(ImageId(1)).is_some());
                assert!(terminal.get_image(ImageId(2)).is_some());
                assert_eq!(
                    format!(
                        "{:?}",
                        terminal.graphics.placements(true).collect::<Vec<_>>()
                    ),
                    format!("{alternate_before:?}")
                );
                if columns > 4 {
                    assert!(!root_retained, "fixture must exercise history contraction");
                }
            }
        }
    }
}

#[test]
fn column_resize_releases_unretained_anonymous_pixels_without_resurrection() {
    for frozen_offset in [None, Some(0), Some(9)] {
        let mut terminal = Terminal::new(4, 3);
        terminal.set_pixel_geometry(8, 9);
        terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[H");
        let id = terminal.graphics_placements().next().unwrap().image_id;
        let pixels = std::sync::Arc::downgrade(&terminal.get_image(id).unwrap().data);
        terminal.process(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV");
        if let Some(offset) = frozen_offset {
            terminal.grid_mut().set_scroll_offset(offset);
            terminal.process(b"\x1b[?2026h");
        }
        assert!(pixels.upgrade().is_some());
        terminal.resize(16, 3);
        assert!(terminal.get_image(id).is_none());
        assert_eq!(pixels.upgrade().is_some(), frozen_offset == Some(9));
        if frozen_offset == Some(9) {
            let images = terminal.presentation_images();
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].placement.row, 0);
        } else {
            assert!(terminal.presentation_images().is_empty());
        }
        terminal.process(b"\x1b[?2026l");
        assert!(pixels.upgrade().is_none());
        terminal.resize(4, 3);
        terminal.process(b"\r\nmore\r\nhistory\r\n");
        assert!(terminal.graphics_placements().next().is_none());
    }
}

#[test]
fn height_resize_images_follow_retained_rows_on_both_screens() {
    for active_alternate in [false, true] {
        for limit in [0, 1, 20] {
            for primary_cursor in 0..5 {
                for alternate_cursor in [0, 4] {
                    for new_rows in [1, 3, 5, 7] {
                        let mut terminal = Terminal::new(8, 5);
                        terminal.set_scrollback_limits(limit, 1024 * 1024);
                        populate(&mut terminal, 1, b"ABCDEFGHI");
                        terminal.grid_mut().set_cursor(primary_cursor, 0);
                        terminal.enter_alt_screen();
                        populate(&mut terminal, 20, b"abcde");
                        terminal.grid_mut().set_cursor(alternate_cursor, 0);
                        if !active_alternate {
                            terminal.exit_alt_screen();
                        }
                        let before: Vec<_> = terminal
                            .graphics
                            .placements(false)
                            .chain(terminal.graphics.placements(true))
                            .cloned()
                            .collect();
                        terminal.resize(8, new_rows);
                        for original in before {
                            let (grid, cursor, base, start) = if original.alternate_screen {
                                (&terminal.alt_grid, alternate_cursor, b'a', 20)
                            } else {
                                (&terminal.grid, primary_cursor, b'A', 1)
                            };
                            let shift = 5usize.saturating_sub(new_rows).min(cursor) as i64;
                            let expected_row = original.row - shift;
                            let oldest = -(grid.scrollback_len() as i64);
                            let actual = terminal
                                .graphics
                                .placements(original.alternate_screen)
                                .find(|p| p.image_id == original.image_id);
                            if expected_row < oldest {
                                assert!(actual.is_none(), "evicted {original:?}");
                            } else {
                                let mut expected = original.clone();
                                expected.row = expected_row;
                                assert_eq!(
                                    format!("{:?}", actual.unwrap()),
                                    format!("{expected:?}")
                                );
                                if expected_row < new_rows as i64 {
                                    let letter =
                                        char::from(base + (original.image_id.0 - start) as u8);
                                    assert_eq!(
                                        grid.row_at_offset(expected_row as i32)
                                            .unwrap()
                                            .to_string(),
                                        letter.to_string()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn height_resize_uses_actual_archival_not_window_delta_or_history_growth() {
    // Removing blank tail rows and growing the viewport must not move images.
    let mut terminal = Terminal::new(8, 5);
    populate(&mut terminal, 1, b"ABC");
    let before = format!("{:?}", terminal.graphics_placements().collect::<Vec<_>>());
    for rows in [3, 7, 3] {
        terminal.resize(8, rows);
        assert_eq!(
            format!("{:?}", terminal.graphics_placements().collect::<Vec<_>>()),
            before
        );
    }

    // A byte cap can evict multiple old rows while two viewport rows move up.
    let mut terminal = Terminal::new(8, 5);
    populate(&mut terminal, 1, b"ABCDEFGHI");
    let budget = terminal.grid().memory_usage().scrollback_bytes;
    terminal.set_scrollback_limits(20, budget);
    terminal
        .grid_mut()
        .cell_mut(0, 0)
        .unwrap()
        .set_grapheme(&format!("E{}", "\u{301}".repeat(budget / 4)));
    terminal.resize(8, 3);
    assert!(terminal.grid().scrollback_len() < 6);
    let oldest = -(terminal.grid().scrollback_len() as i64);
    for id in 1..=9 {
        let row = i64::from(id) - 7;
        let image = terminal
            .graphics_placements()
            .find(|p| p.image_id == ImageId(id));
        assert_eq!(image.map(|p| p.row), (row >= oldest).then_some(row));
    }
}

#[test]
fn height_resize_keeps_relative_offsets_and_cascades_evicted_parents() {
    for limit in [0, 20] {
        let mut terminal = Terminal::new(8, 5);
        terminal.set_scrollback_limits(limit, 1024 * 1024);
        populate(&mut terminal, 1, b"ABCDE");
        terminal.handle_graphics_command(b"a=t,i=20,f=32,s=1,v=1,q=2;AQIDBA==");
        terminal.handle_graphics_command(b"a=p,i=20,P=1,Q=1,V=2,H=1,c=1,r=1,q=2");
        terminal.resize(8, 3);
        let images = terminal.presentation_images();
        let child = images.iter().find(|p| p.placement.image_id == ImageId(20));
        if limit == 0 {
            assert!(child.is_none());
            assert!(terminal.get_image(ImageId(20)).is_none());
        } else {
            assert_eq!(child.unwrap().placement.row, 0);
            assert_eq!(child.unwrap().placement.col, 2);
        }
    }

    // Raster rectangles may extend below the viewport; resize clipping must
    // not permanently crop or rescale their source pixels.
    let mut terminal = Terminal::new(8, 5);
    terminal.set_pixel_geometry(16, 15);
    terminal.process(b"\x1bPq\"1;1!4~-!4~-!4~\x1b\\");
    terminal.process(b"\x1b[5;1HX");
    let original = terminal.graphics_placements().next().unwrap().clone();
    assert_eq!(
        (original.height, original.pixel_height),
        (Some(6), Some(18))
    );
    let pixels = terminal.get_image(original.image_id).unwrap().data.clone();
    terminal.resize(8, 3);
    let mut expected = original;
    expected.row -= 2;
    assert_eq!(
        format!("{:?}", terminal.graphics_placements().next().unwrap()),
        format!("{expected:?}")
    );
    assert!(std::sync::Arc::ptr_eq(
        &pixels,
        &terminal.get_image(expected.image_id).unwrap().data
    ));
    terminal.resize(8, 6);
    assert_eq!(
        format!("{:?}", terminal.graphics_placements().next().unwrap()),
        format!("{expected:?}")
    );
}

#[test]
fn height_resize_frees_evicted_anonymous_images_after_frozen_history_releases() {
    let mut terminal = Terminal::new(8, 5);
    terminal.set_pixel_geometry(16, 15);
    terminal.set_scrollback_limits(0, 0);
    terminal.process(b"\x1bPq\"1;1!4~\x1b\\\x1b[3;1HC\r\nD\r\nE");
    let id = terminal.graphics_placements().next().unwrap().image_id;
    let pixels = std::sync::Arc::downgrade(&terminal.get_image(id).unwrap().data);
    terminal.process(b"\x1b[?2026h");
    terminal.resize(8, 2);
    assert_eq!(terminal.grid().scrollback_len(), 0);
    assert_eq!(terminal.graphics_memory_usage(), 0);
    assert!(terminal.get_image(id).is_none());
    assert_eq!(terminal.presentation_grid().scrollback_len(), 3);
    {
        let frozen = &terminal.presentation_images()[0];
        assert_eq!(frozen.placement.image_id, id);
        assert_eq!(frozen.placement.row, -3);
        assert_eq!(frozen.placement.pixel_height, Some(6));
    }
    assert!(pixels.upgrade().is_some());
    terminal.process(b"\x1b[?2026l");
    assert!(terminal.presentation_images().is_empty());
    assert!(pixels.upgrade().is_none());
}
