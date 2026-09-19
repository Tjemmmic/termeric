//! Parser-to-GPU coverage for destructive image crops at partial cell edges.
//! The expected framebuffer uses pixel copies, never placement/crop helpers.

use super::{
    HEADLESS_IMAGE_HEIGHT, HEADLESS_IMAGE_WIDTH, ImagePipeline, render_headless_image_state,
};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use termeric_core::Terminal;
use wgpu::{Device, Queue, TextureFormat};

const WIDTH: usize = HEADLESS_IMAGE_WIDTH as usize;
const HEIGHT: usize = HEADLESS_IMAGE_HEIGHT as usize;
const BLACK: [u8; 4] = [0, 0, 0, 255];

#[derive(Clone, Copy, Debug)]
enum Fixture {
    Kitty,
    Sixel,
    Iterm2,
    OffsetKitty,
}

const FIXTURES: [Fixture; 4] = [
    Fixture::Kitty,
    Fixture::Sixel,
    Fixture::Iterm2,
    Fixture::OffsetKitty,
];

#[derive(Clone, Copy, Debug)]
enum Edit {
    Up,
    Down,
    Copy,
    Erase,
}

const EDITS: [Edit; 4] = [Edit::Up, Edit::Down, Edit::Copy, Edit::Erase];

fn source_pixel(x: usize, y: usize) -> [u8; 4] {
    match (x < 8, y < 4) {
        (true, true) => [255, 0, 0, 255],
        (false, true) => [0, 255, 0, 255],
        (true, false) => [0, 0, 255, 255],
        (false, false) => [255, 255, 0, 255],
    }
}

fn source_pixels() -> Vec<u8> {
    (0..6)
        .flat_map(|y| (0..14).flat_map(move |x| source_pixel(x, y)))
        .collect()
}

impl Fixture {
    fn offset(self) -> usize {
        if matches!(self, Self::OffsetKitty) {
            3
        } else {
            0
        }
    }

    fn wire(self) -> Result<Vec<u8>> {
        let image = match self {
            Self::Kitty | Self::OffsetKitty => format!(
                "\x1b_Ga=T,f=32,s=14,v=6,i=7,C=1,q=2,X={0},Y={0};{1}\x1b\\",
                self.offset(),
                STANDARD.encode(source_pixels())
            ),
            // Four independent palette bands, with 1:1 display pixels. The
            // final two source rows and final six columns are partial cells.
            Self::Sixel => concat!(
                "\x1bP7q\"1;1;14;6",
                "#1;2;100;0;0!8N$#2;2;0;100;0!8?!6N$",
                "#3;2;0;0;100!8o$#4;2;100;100;0!8?!6o\x1b\\"
            )
            .into(),
            Self::Iterm2 => {
                let image = ::image::RgbaImage::from_raw(14, 6, source_pixels()).unwrap();
                let mut png = std::io::Cursor::new(Vec::new());
                image.write_to(&mut png, ::image::ImageFormat::Png)?;
                format!(
                    "\x1b]1337;File=inline=1;width=14px;height=6px;preserveAspectRatio=0:{}\x07",
                    STANDARD.encode(png.into_inner())
                )
            }
        };
        Ok(format!("\x1b[2;1H{image}").into_bytes())
    }

    fn terminal(self) -> Result<Terminal> {
        let mut terminal = Terminal::new(8, HEADLESS_IMAGE_HEIGHT as usize / 4);
        terminal.set_pixel_geometry(HEADLESS_IMAGE_WIDTH, HEADLESS_IMAGE_HEIGHT);
        // Real incremental parser input, including split DCS/APC/OSC framing.
        for byte in self.wire()? {
            terminal.process(&[byte]);
        }
        Ok(terminal)
    }

    fn region_bottom(self) -> usize {
        if self.offset() == 0 { 12 } else { 16 }
    }

    fn down_pixels(self) -> usize {
        if self.offset() == 0 { 4 } else { 8 }
    }

    fn copy_origin(self) -> (usize, usize) {
        if self.offset() == 0 { (8, 8) } else { (16, 12) }
    }

    fn edit(self, terminal: &mut Terminal, edit: Edit) {
        let command = match edit {
            Edit::Up => format!("\x1b[2;{}r\x1b[S", self.region_bottom() / 4),
            Edit::Down => format!(
                "\x1b[2;{}r\x1b[{}T",
                self.region_bottom() / 4,
                self.down_pixels() / 4
            ),
            Edit::Copy => {
                let (x, y) = self.copy_origin();
                format!("\x1b[{0};{1};{0};{1};1;6;5;1$v", y / 4 + 1, x / 8 + 1)
            }
            Edit::Erase => "\x1b[2;1;2;1$z".into(),
        };
        terminal.process(command.as_bytes());
    }
}

fn blank_frame() -> Vec<u8> {
    BLACK.repeat(WIDTH * HEIGHT)
}

fn initial_frame(fixture: Fixture) -> Vec<u8> {
    let mut frame = blank_frame();
    for y in 0..6 {
        for x in 0..14 {
            let position = ((y + 4 + fixture.offset()) * WIDTH + x + fixture.offset()) * 4;
            frame[position..position + 4].copy_from_slice(&source_pixel(x, y));
        }
    }
    frame
}

fn edited_frame(fixture: Fixture, edit: Edit) -> Vec<u8> {
    let source = initial_frame(fixture);
    let mut frame = source.clone();
    match edit {
        Edit::Up | Edit::Down => {
            for y in 4..fixture.region_bottom() {
                let from = match edit {
                    Edit::Up => Some(y + 4),
                    _ => y.checked_sub(fixture.down_pixels()),
                }
                .filter(|y| (4..fixture.region_bottom()).contains(y));
                for x in 0..WIDTH {
                    let color = from
                        .map(|y| &source[(y * WIDTH + x) * 4..][..4])
                        .unwrap_or(&BLACK);
                    frame[(y * WIDTH + x) * 4..][..4].copy_from_slice(color);
                }
            }
        }
        Edit::Copy => {
            let (left, top) = fixture.copy_origin();
            for y in 0..4 {
                for x in 0..8 {
                    frame[((20 + y) * WIDTH + 32 + x) * 4..][..4]
                        .copy_from_slice(&source[((top + y) * WIDTH + left + x) * 4..][..4]);
                }
            }
        }
        Edit::Erase => {
            for y in 4..8 {
                for x in 0..8 {
                    frame[(y * WIDTH + x) * 4..][..4].copy_from_slice(&BLACK);
                }
            }
        }
    }
    frame
}

fn verify(actual: &[u8], expected: &[u8], label: &str) -> Result<()> {
    ensure!(
        actual.len() == WIDTH * HEIGHT * 4 && actual.len() == expected.len(),
        "{label}: wrong framebuffer length"
    );
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        ensure!(
            actual.abs_diff(*expected) <= 3,
            "{label}: pixel ({}, {}) channel {} was {actual}, expected {expected}",
            index / 4 % WIDTH,
            index / 4 / WIDTH,
            index % 4
        );
    }
    Ok(())
}

pub(super) fn run(device: &Device, queue: &Queue) -> Result<()> {
    let mut frames = 0;
    for (fixture, relayed) in FIXTURES
        .into_iter()
        .flat_map(|fixture| [(fixture, false), (fixture, true)])
    {
        for edit in EDITS {
            let mut terminal = fixture.terminal()?;
            if relayed {
                let mut replica = Terminal::new(8, HEADLESS_IMAGE_HEIGHT as usize / 4);
                replica.set_pixel_geometry(HEADLESS_IMAGE_WIDTH, HEADLESS_IMAGE_HEIGHT);
                replica.set_graphics_relay_snapshot_allowed(true);
                terminal
                    .graphics_relay_snapshot(std::time::Instant::now())
                    .for_each_escape_chunk(|chunk| {
                        for fragment in chunk.as_bytes().chunks(3) {
                            replica.process(fragment);
                        }
                        true
                    })
                    .map_err(anyhow::Error::msg)?;
                ensure!(
                    replica.drain_events().is_empty(),
                    "relay emitted unexpected side effects"
                );
                terminal = replica;
            }
            let images = terminal.presentation_images();
            ensure!(
                images.len() == 1,
                "{fixture:?}: parser did not produce exactly one image"
            );
            let image = &images[0].image;
            ensure!(
                image.data.as_ref() == source_pixels(),
                "{fixture:?}: decoded pixels differ"
            );
            let key = (1, image.id);
            let mut pipeline = ImagePipeline::new(device, TextureFormat::Rgba8UnormSrgb)?;
            pipeline.resize(queue, HEADLESS_IMAGE_WIDTH, HEADLESS_IMAGE_HEIGHT);
            for phase in ["initial", "edited", "frozen after hidden erase", "released"] {
                let expected = match phase {
                    "initial" => initial_frame(fixture),
                    "edited" => {
                        fixture.edit(&mut terminal, edit);
                        edited_frame(fixture, edit)
                    }
                    "frozen after hidden erase" => {
                        terminal.process(b"\x1b[?2026h\x1b[2J");
                        edited_frame(fixture, edit)
                    }
                    _ => {
                        terminal.process(b"\x1b[?2026l");
                        blank_frame()
                    }
                };
                let presentations = terminal.presentation_images();
                for presentation in &presentations {
                    ensure!(
                        presentation.placement.image_id == image.id
                            && presentation.image.id == image.id
                            && presentation.image.revision == image.revision
                            && std::sync::Arc::ptr_eq(&presentation.image.data, &image.data),
                        "{fixture:?}/{edit:?}/{phase}: crop replaced the source image"
                    );
                    pipeline.upload_image(device, queue, key, &presentation.image);
                }
                let pixels = render_headless_image_state(
                    device,
                    queue,
                    &mut pipeline,
                    key,
                    &presentations,
                    terminal.presentation_grid().scroll_offset(),
                )?;
                verify(
                    &pixels,
                    &expected,
                    &format!("{fixture:?}/relayed={relayed}/{edit:?}/{phase}"),
                )?;
                frames += 1;
            }
        }
    }
    ensure!(frames == 128, "partial-cell image coverage lost scenarios");
    eprintln!(
        "parser-to-GPU partial-cell image crops: {frames} complete-frame checks passed (Kitty/Sixel/iTerm2, local/relayed, offsets, scroll/copy/erase, frozen/released)"
    );
    Ok(())
}

#[test]
fn fragmented_protocol_fixtures_decode_independent_quadrants() {
    for fixture in FIXTURES {
        let terminal = fixture.terminal().unwrap();
        let images = terminal.presentation_images();
        assert_eq!(images.len(), 1, "{fixture:?}");
        assert_eq!((images[0].image.width, images[0].image.height), (14, 6));
        assert_eq!(images[0].image.data.as_ref(), source_pixels());
        assert_eq!(images[0].placement.row, 1);
    }
}

#[test]
fn pixel_oracle_rejects_missing_pixels_stale_content_and_overpaint() {
    for fixture in FIXTURES {
        for edit in EDITS {
            let expected = edited_frame(fixture, edit);
            verify(&expected, &expected, "control").unwrap();
            assert!(verify(&blank_frame(), &expected, "missing").is_err());
            assert!(verify(&initial_frame(fixture), &expected, "stale").is_err());
            let mut wrong = expected.clone();
            wrong[(WIDTH * HEIGHT - 1) * 4] = 255;
            assert!(verify(&wrong, &expected, "overpaint").is_err());
            let colored = expected
                .as_chunks::<4>()
                .0
                .iter()
                .position(|pixel| *pixel != BLACK)
                .unwrap();
            wrong = expected.clone();
            wrong[colored * 4..colored * 4 + 4].copy_from_slice(&BLACK);
            assert!(verify(&wrong, &expected, "missing crop pixel").is_err());
        }
    }
}
