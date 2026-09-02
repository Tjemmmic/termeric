#![no_main]

use libfuzzer_sys::fuzz_target;
use termeric_core::{Terminal, graphics::MAX_IMAGE_PLACEMENTS};

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_MEMORY: usize = 64 * 1024 * 1024;
const MAX_IMAGES: usize = 256;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, payload)) = data.split_first() else {
        return;
    };
    let payload = &payload[..payload.len().min(MAX_INPUT_BYTES)];
    let mut sequence = Vec::with_capacity(payload.len() + 16);
    match selector % 4 {
        0 => {
            sequence.extend_from_slice(b"\x1bP");
            sequence.extend_from_slice(payload);
            sequence.extend_from_slice(b"\x1b\\");
        }
        1 => {
            sequence.extend_from_slice(b"\x1b]1337;");
            sequence.extend_from_slice(payload.strip_suffix(b"\n").unwrap_or(payload));
            sequence.push(0x07);
        }
        2 => {
            sequence.extend_from_slice(b"\x1b_G");
            sequence.extend_from_slice(payload.strip_suffix(b"\n").unwrap_or(payload));
            sequence.extend_from_slice(b"\x1b\\");
        }
        _ => sequence.extend_from_slice(payload),
    }

    let mut terminal = Terminal::new(80, 24);
    terminal.set_pixel_geometry(800, 480);
    for chunk in sequence.chunks((selector as usize % 31) + 1) {
        terminal.process(chunk);
    }

    assert!(terminal.graphics_memory_usage() <= MAX_IMAGE_MEMORY);
    assert!(terminal.graphics_image_count() <= MAX_IMAGES);
    assert!(terminal.graphics_placement_count() <= MAX_IMAGE_PLACEMENTS);
    for placement in terminal.graphics_placements() {
        assert!(terminal.get_image(placement.image_id).is_some());
        assert!(placement.width.unwrap_or(1) > 0);
        assert!(placement.height.unwrap_or(1) > 0);
    }
});
