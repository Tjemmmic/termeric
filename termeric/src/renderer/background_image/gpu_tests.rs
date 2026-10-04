//! Offscreen pixel checks of the background image's upload, fit, opacity,
//! and replacement through the production pipelines.

use super::*;
use crate::renderer::{image::ImagePipeline, rect::RectPipeline};
use anyhow::{Result, ensure};
use wgpu::*;

const CLEAR: [f32; 4] = [0.02, 0.03, 0.04, 1.0];
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

#[derive(Clone, Copy, Debug)]
struct Case {
    size: (u32, u32),
    viewport: (f32, f32, f32, f32),
    fit: BackgroundImageFit,
    opacity: f32,
    foreground: bool,
}

struct ReferenceImage {
    width: u32,
    height: u32,
    encoded: Vec<[f32; 4]>,
}

// The renderer filters and blends sRGB-encoded values directly (see
// `ENCODED_RGBA_FORMAT`), so the oracle works on normalized stored values.
fn to_unit(value: u8) -> f32 {
    f32::from(value) / 255.0
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl ReferenceImage {
    fn from_decoded(decoded: &DecodedBackgroundImage) -> Self {
        Self {
            width: decoded.width,
            height: decoded.height,
            encoded: decoded
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| {
                    [
                        to_unit(pixel[0]),
                        to_unit(pixel[1]),
                        to_unit(pixel[2]),
                        to_unit(pixel[3]),
                    ]
                })
                .collect(),
        }
    }

    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let x0 = x.floor();
        let y0 = y.floor();
        let tx = x - x0;
        let ty = y - y0;
        let at = |x: f32, y: f32| {
            self.encoded[y.clamp(0.0, self.height as f32 - 1.0) as usize * self.width as usize
                + x.clamp(0.0, self.width as f32 - 1.0) as usize]
        };
        let samples = [
            at(x0, y0),
            at(x0 + 1.0, y0),
            at(x0, y0 + 1.0),
            at(x0 + 1.0, y0 + 1.0),
        ];
        std::array::from_fn(|channel| {
            let top = samples[0][channel] * (1.0 - tx) + samples[1][channel] * tx;
            let bottom = samples[2][channel] * (1.0 - tx) + samples[3][channel] * tx;
            top * (1.0 - ty) + bottom * ty
        })
    }
}

/// Inverse-map pixel centers into the source rather than reusing the production
/// destination/UV geometry. A broken fit/crop in that code must fail this oracle.
fn reference_frame(source: Option<&ReferenceImage>, case: Case) -> Vec<u8> {
    let (left, top, width, height) = case.viewport;
    let mut pixels = Vec::with_capacity((case.size.0 * case.size.1 * 4) as usize);
    for y in 0..case.size.1 {
        for x in 0..case.size.0 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut color = CLEAR;
            if let Some(source) = source
                && case.opacity > 0.0
                && width > 0.0
                && height > 0.0
                && px >= left
                && px < left + width
                && py >= top
                && py < top + height
            {
                let scale_x = width / source.width as f32;
                let scale_y = height / source.height as f32;
                let (sx, sy) = match case.fit {
                    BackgroundImageFit::Stretch => (scale_x, scale_y),
                    BackgroundImageFit::Cover => (scale_x.max(scale_y), scale_x.max(scale_y)),
                    BackgroundImageFit::Contain => (scale_x.min(scale_y), scale_x.min(scale_y)),
                    BackgroundImageFit::Center => (1.0, 1.0),
                };
                let u = (px - left - width / 2.0) / sx + source.width as f32 / 2.0;
                let v = (py - top - height / 2.0) / sy + source.height as f32 / 2.0;
                if u >= 0.0 && u < source.width as f32 && v >= 0.0 && v < source.height as f32 {
                    let sample = source.sample(u - 0.5, v - 0.5);
                    let alpha = sample[3] * case.opacity;
                    for channel in 0..3 {
                        color[channel] = sample[channel] * alpha + CLEAR[channel] * (1.0 - alpha);
                    }
                }
            }
            if case.foreground
                && px >= left + 4.0
                && px < left + 12.0
                && py >= top + 4.0
                && py < top + 12.0
            {
                color = [0.0, 1.0, 1.0, 1.0];
            }
            pixels.extend([to_byte(color[0]), to_byte(color[1]), to_byte(color[2]), 255]);
        }
    }
    pixels
}

fn verify(actual: &[u8], expected: &[u8], case: Case) -> Result<()> {
    ensure!(
        actual.len() == expected.len(),
        "background readback length mismatch: {case:?}"
    );
    if let Some((offset, (actual, expected))) = actual
        .iter()
        .zip(expected)
        .enumerate()
        .find(|(_, (actual, expected))| actual.abs_diff(**expected) > 3)
    {
        anyhow::bail!(
            "background pixel ({}, {}) channel {} was {actual}, expected {expected}: {case:?}",
            offset / 4 % case.size.0 as usize,
            offset / 4 / case.size.0 as usize,
            offset % 4
        );
    }
    Ok(())
}

fn snapshot(
    device: &Device,
    queue: &Queue,
    state: &BackgroundImageState,
    images: &mut ImagePipeline,
    case: Case,
) -> Result<Vec<u8>> {
    images.resize(queue, case.size.0, case.size.1);
    state.queue(device, queue, images, case.viewport);
    images.upload(device, queue);
    let mut rects = RectPipeline::new(device, FORMAT)?;
    rects.resize(queue, case.size.0, case.size.1);
    if case.foreground {
        rects.push_rect(
            case.viewport.0 + 4.0,
            case.viewport.1 + 4.0,
            8.0,
            8.0,
            [0.0, 1.0, 1.0, 1.0],
        );
    }
    rects.upload(device, queue);
    let clear = wgpu::Color {
        r: f64::from(CLEAR[0]),
        g: f64::from(CLEAR[1]),
        b: f64::from(CLEAR[2]),
        a: 1.0,
    };
    crate::renderer::headless_tests::render_offscreen(
        device,
        queue,
        FORMAT,
        case.size,
        clear,
        |pass| {
            images.draw_below_text(pass);
            rects.draw_layer(pass, 0);
        },
    )
}

pub(crate) fn headless_background_render_smoke(device: &Device, queue: &Queue) -> Result<()> {
    let dir = tempfile::tempdir()?;
    for extension in ["png", "jpg"] {
        let path = dir.path().join(format!("background.{extension}"));
        let input = ::image::RgbaImage::from_fn(96, 48, |x, y| {
            let colors = [[210, 40, 20], [30, 180, 50], [30, 60, 220]];
            let mut color = colors[(x / 32) as usize];
            if y >= 24 {
                color.rotate_left(1);
            }
            let alpha = if x / 32 == 1 {
                128
            } else if x >= 64 && y >= 24 {
                0
            } else {
                255
            };
            ::image::Rgba([color[0], color[1], color[2], alpha])
        });
        if extension == "jpg" {
            ::image::DynamicImage::ImageRgba8(input)
                .to_rgb8()
                .save(&path)?;
        } else {
            input.save(&path)?;
        }
        let decoded = load_background_image(&path).map_err(anyhow::Error::msg)?;
        let source = ReferenceImage::from_decoded(&decoded);
        let cpu_pixels = Arc::downgrade(&decoded.rgba);
        let mut state = BackgroundImageState::default();
        let mut images = ImagePipeline::new(device, FORMAT)?;
        state.install(decoded, device, queue, &mut images);
        ensure!(
            cpu_pixels.upgrade().is_none(),
            "background retained its CPU RGBA allocation"
        );
        ensure!(
            images.retained_image_count() == 1,
            "background upload did not retain exactly one texture"
        );
        for (size, viewport) in [
            ((256, 192), (0.0, 24.0, 256.0, 168.0)),
            ((256, 192), (32.0, 56.0, 224.0, 136.0)),
            ((192, 128), (32.0, 24.0, 160.0, 104.0)),
            ((128, 96), (32.0, 24.0, 48.0, 32.0)),
        ] {
            for fit in [
                BackgroundImageFit::Cover,
                BackgroundImageFit::Contain,
                BackgroundImageFit::Stretch,
                BackgroundImageFit::Center,
            ] {
                for opacity in [0.0, 0.35, 1.0] {
                    let case = Case {
                        size,
                        viewport,
                        fit,
                        opacity,
                        foreground: true,
                    };
                    state.fit = fit;
                    state.opacity = opacity;
                    let actual = snapshot(device, queue, &state, &mut images, case)?;
                    verify(&actual, &reference_frame(Some(&source), case), case)?;
                }
            }
        }
        // Replacing the same key must refresh its pixels and retain one GPU
        // texture; a subsequent frame must not depend on the discarded RGBA.
        ::image::RgbImage::from_pixel(64, 32, ::image::Rgb([200, 180, 20])).save(&path)?;
        let decoded = load_background_image(&path).map_err(anyhow::Error::msg)?;
        let replacement = ReferenceImage::from_decoded(&decoded);
        let case = Case {
            size: (128, 96),
            viewport: (32.0, 24.0, 96.0, 72.0),
            fit: BackgroundImageFit::Stretch,
            opacity: 1.0,
            foreground: false,
        };
        state.fit = case.fit;
        state.opacity = case.opacity;
        state.install(decoded, device, queue, &mut images);
        ensure!(
            images.retained_image_count() == 1,
            "replacement leaked a cached texture"
        );
        for _ in 0..2 {
            verify(
                &snapshot(device, queue, &state, &mut images, case)?,
                &reference_frame(Some(&replacement), case),
                case,
            )?;
        }
        state.clear(&mut images);
        ensure!(
            images.retained_image_count() == 0 && state.image.is_none(),
            "clearing retained background resources"
        );
        verify(
            &snapshot(device, queue, &state, &mut images, case)?,
            &reference_frame(None, case),
            case,
        )?;
        // Recreate the dedicated pipeline and reload, as renderer recreation
        // does. This is not a simulation of a real GPU/device-loss event.
        images = ImagePipeline::new(device, FORMAT)?;
        state.install(
            load_background_image(&path).map_err(anyhow::Error::msg)?,
            device,
            queue,
            &mut images,
        );
        verify(
            &snapshot(device, queue, &state, &mut images, case)?,
            &reference_frame(Some(&replacement), case),
            case,
        )?;
    }
    Ok(())
}

#[test]
fn background_pixel_oracle_rejects_missing_image_and_chrome_bleed() {
    let source = ReferenceImage {
        width: 2,
        height: 2,
        encoded: vec![[1.0, 0.0, 0.0, 1.0]; 4],
    };
    let case = Case {
        size: (64, 64),
        viewport: (16.0, 16.0, 32.0, 32.0),
        fit: BackgroundImageFit::Stretch,
        opacity: 1.0,
        foreground: true,
    };
    let expected = reference_frame(Some(&source), case);
    assert!(verify(&reference_frame(None, case), &expected, case).is_err());
    let mut corrupted = expected.clone();
    corrupted[0] = 255;
    assert!(verify(&corrupted, &expected, case).is_err());
    let mut corrupted = expected.clone();
    corrupted[(22 * 64 + 22) * 4] = 255;
    assert!(verify(&corrupted, &expected, case).is_err());
}

#[test]
fn background_pixel_oracle_distinguishes_fits_and_opacity() {
    let source = ReferenceImage {
        width: 48,
        height: 16,
        encoded: (0..48 * 16)
            .map(|index| [if index % 48 < 16 { 1.0 } else { 0.0 }, 0.3, 0.0, 1.0])
            .collect(),
    };
    let case = Case {
        size: (64, 64),
        viewport: (8.0, 8.0, 32.0, 32.0),
        fit: BackgroundImageFit::Cover,
        opacity: 1.0,
        foreground: false,
    };
    let expected = reference_frame(Some(&source), case);
    for fit in [
        BackgroundImageFit::Contain,
        BackgroundImageFit::Stretch,
        BackgroundImageFit::Center,
    ] {
        assert!(
            verify(
                &reference_frame(Some(&source), Case { fit, ..case }),
                &expected,
                case
            )
            .is_err()
        );
    }
    assert!(
        verify(
            &reference_frame(
                Some(&source),
                Case {
                    opacity: 0.35,
                    ..case
                }
            ),
            &expected,
            case
        )
        .is_err()
    );
}
