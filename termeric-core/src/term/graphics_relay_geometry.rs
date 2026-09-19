use super::*;

fn image(terminal: &mut Terminal, controls: &str) {
    let pixels = STANDARD.encode([255; 6 * 6 * 4]);
    terminal.process(
        format!("\x1b_Ga=T,f=32,s=6,v=6,i=7,C=1,q=2,{controls};{pixels}\x1b\\").as_bytes(),
    );
}

fn relay(source: &mut Terminal, replica: &mut Terminal, change_before_commit: bool) {
    replica.set_graphics_relay_snapshot_allowed(true);
    source
        .graphics_relay_snapshot(Instant::now())
        .for_each_escape_chunk(|chunk| {
            if change_before_commit && chunk.starts_with("\x1b_TG;end") {
                replica.set_pixel_geometry(16, 24);
            }
            for fragment in chunk.as_bytes().chunks(3) {
                replica.process(fragment);
            }
            true
        })
        .unwrap();
    assert!(!replica.graphics_relay_import_allowed);
    assert!(replica.drain_events().is_empty());
}

#[test]
fn relay_reset_success_and_failure_keep_host_metrics_for_future_images() {
    for failure in [false, true] {
        let mut replica = Terminal::new(8, 8);
        replica.set_pixel_geometry(32, 32);
        if failure {
            replica.set_graphics_relay_snapshot_allowed(true);
            replica.process(b"\x1b_TG;begin;1;0\x1b\\\x1b_TG;end\x1b\\");
            assert!(!replica.graphics_relay_import_allowed);
        } else {
            relay(&mut Terminal::new(8, 8), &mut replica, false);
        }
        image(&mut replica, "p=1");
        let p = replica.graphics_placements().next().unwrap();
        assert_eq!((p.width, p.height), (Some(2), Some(2)), "failure={failure}");
        assert_eq!((p.pixel_width, p.pixel_height), (Some(6), Some(6)));
        replica.process(b"\x1b[16t");
        assert!(
            matches!(&replica.drain_events()[0], TerminalEvent::PtyWrite(reply) if reply == b"\x1b[6;4;4t")
        );
    }
}

#[test]
fn relay_import_recomputes_pixel_coverage_for_current_host_on_both_screens() {
    for change_before_commit in [false, true] {
        let mut source = Terminal::new(8, 8);
        source.set_pixel_geometry(64, 128);
        for alternate in [false, true] {
            if alternate {
                source.process(b"\x1b[?1049h");
                source.process(b"\x1b_Ga=p,i=7,p=1,X=3,Y=3,C=1,q=2\x1b\\");
            } else {
                image(&mut source, "p=1,X=3,Y=3");
            }
            source.process(b"\x1b_Ga=p,i=7,p=2,c=2,r=3,C=1,q=2\x1b\\");
            source.process(b"\x1b_Ga=p,i=7,p=3,P=7,Q=2,H=-1,V=1,c=1,r=1,C=1,q=2\x1b\\");
        }
        for alternate in [false, true] {
            assert_eq!(source.graphics.placements(alternate).count(), 3);
        }
        let mut replica = Terminal::new(8, 8);
        replica.set_pixel_geometry(32, 32);
        relay(&mut source, &mut replica, change_before_commit);
        for alternate in [false, true] {
            let placements: Vec<_> = replica.graphics.placements(alternate).collect();
            assert_eq!(placements.len(), 3);
            let p = placements
                .iter()
                .find(|p| p.placement_id == Some(1))
                .unwrap();
            assert_eq!(
                (p.width, p.height),
                if change_before_commit {
                    (Some(5), Some(3))
                } else {
                    (Some(3), Some(3))
                }
            );
            assert_eq!(
                (p.pixel_width, p.pixel_height, p.x_offset, p.y_offset),
                (Some(6), Some(6), 3, 3)
            );
            for (id, width, height) in [(2, 2, 3), (3, 1, 1)] {
                let p = placements
                    .iter()
                    .find(|p| p.placement_id == Some(id))
                    .unwrap();
                assert_eq!((p.width, p.height), (Some(width), Some(height)));
                assert_eq!((p.pixel_width, p.pixel_height), (None, None));
            }
        }
        assert_eq!(
            replica.get_image(ImageId(7)).unwrap().data,
            source.get_image(ImageId(7)).unwrap().data
        );
    }
}

#[test]
fn restored_pixel_images_crop_correctly_without_an_extra_host_resize() {
    for source_cell in [(4, 4), (8, 16)] {
        let mut source = Terminal::new(8, 8);
        source.set_pixel_geometry(source_cell.0 * 8, source_cell.1 * 8);
        source.process(b"\x1b[2;1H");
        image(&mut source, "p=1");
        let mut replica = Terminal::new(8, 8);
        replica.set_pixel_geometry(32, 32);
        relay(&mut source, &mut replica, false);
        replica.process(b"\x1b[2;3r\x1b[S");
        let images = replica.presentation_images();
        assert_eq!(images.len(), 1, "source cells={source_cell:?}");
        let p = &images[0].placement;
        assert_eq!(
            (p.row, p.source_y, p.source_height, p.pixel_height),
            (1, 4, Some(2), Some(2))
        );
        assert_eq!((p.width, p.height), (Some(2), Some(1)));
    }
}
