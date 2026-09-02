//! Bounded local background-image loading and viewport geometry.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{ImageFormat, ImageReader, Limits};
use termeric_core::{ImageId, StoredImage};

use super::Renderer;
use super::image::ImageKey;
use crate::config::BackgroundImageFit;

/// Encoded input is read through a hard cap before any decoder sees it.
pub(crate) const MAX_BACKGROUND_IMAGE_ENCODED_BYTES: u64 = 16 * 1024 * 1024;
/// Retained decoded RGBA memory is capped independently of compression ratio.
pub(crate) const MAX_BACKGROUND_IMAGE_DECODED_BYTES: u64 = 64 * 1024 * 1024;
/// Matches WGPU's portable default 2D texture dimension limit.
pub(crate) const MAX_BACKGROUND_IMAGE_DIMENSION: u32 = 8192;

const BACKGROUND_IMAGE_KEY: ImageKey = (0, ImageId(0));

#[derive(Debug, Clone)]
pub(crate) struct DecodedBackgroundImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct BackgroundGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub u_min: f32,
    pub v_min: f32,
    pub u_max: f32,
    pub v_max: f32,
}

pub(super) struct BackgroundImageState {
    image: Option<StoredImage>,
    fit: BackgroundImageFit,
    opacity: f32,
    revision: u64,
}

impl Default for BackgroundImageState {
    fn default() -> Self {
        Self {
            image: None,
            fit: BackgroundImageFit::Cover,
            opacity: 0.2,
            revision: 0,
        }
    }
}

/// Resolve relative user paths beside the persistent configuration file.
pub(crate) fn resolve_background_image_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    crate::config::Config::config_path()
        .parent()
        .map_or_else(|| path.to_path_buf(), |parent| parent.join(path))
}

/// Read and decode one static PNG or JPEG under explicit CPU-memory limits.
pub(crate) fn load_background_image(path: &Path) -> Result<DecodedBackgroundImage, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_BACKGROUND_IMAGE_ENCODED_BYTES {
        return Err(format!(
            "{} is {} bytes; background images are limited to {} encoded bytes",
            path.display(),
            metadata.len(),
            MAX_BACKGROUND_IMAGE_ENCODED_BYTES
        ));
    }

    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BACKGROUND_IMAGE_ENCODED_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if encoded.len() as u64 > MAX_BACKGROUND_IMAGE_ENCODED_BYTES {
        return Err(format!(
            "{} changed while loading and exceeded the {} byte encoded limit",
            path.display(),
            MAX_BACKGROUND_IMAGE_ENCODED_BYTES
        ));
    }

    let mut reader = ImageReader::new(Cursor::new(&encoded))
        .with_guessed_format()
        .map_err(|error| format!("could not identify {}: {error}", path.display()))?;
    let format = reader
        .format()
        .ok_or_else(|| format!("{} has no recognized image format", path.display()))?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err(format!(
            "{} uses unsupported format {format:?}; use a static PNG or JPEG",
            path.display()
        ));
    }
    if format == ImageFormat::Png && png_has_animation_control(&encoded) {
        return Err(format!(
            "{} is animated PNG; background images must be static",
            path.display()
        ));
    }

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_BACKGROUND_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_BACKGROUND_IMAGE_DIMENSION);
    // Decoder allocations can briefly exceed the retained RGBA output. They
    // remain bounded while allowing one intermediate conversion buffer.
    limits.max_alloc = Some(MAX_BACKGROUND_IMAGE_DECODED_BYTES * 2);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    let width = decoded.width();
    let height = decoded.height();
    let decoded_bytes = decoded_rgba_bytes(width, height)
        .ok_or_else(|| format!("{} has overflowing decoded dimensions", path.display()))?;
    if decoded_bytes > MAX_BACKGROUND_IMAGE_DECODED_BYTES {
        return Err(format!(
            "{} decodes to {} RGBA bytes; the limit is {}",
            path.display(),
            decoded_bytes,
            MAX_BACKGROUND_IMAGE_DECODED_BYTES
        ));
    }
    let rgba = decoded.into_rgba8().into_raw();
    if rgba.len() as u64 != decoded_bytes {
        return Err(format!(
            "{} produced inconsistent RGBA data",
            path.display()
        ));
    }
    Ok(DecodedBackgroundImage {
        width,
        height,
        rgba: rgba.into(),
    })
}

fn decoded_rgba_bytes(width: u32, height: u32) -> Option<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
}

fn png_has_animation_control(encoded: &[u8]) -> bool {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if !encoded.starts_with(SIGNATURE) {
        return false;
    }
    let mut offset = SIGNATURE.len();
    while offset.saturating_add(12) <= encoded.len() {
        let length = u32::from_be_bytes([
            encoded[offset],
            encoded[offset + 1],
            encoded[offset + 2],
            encoded[offset + 3],
        ]) as usize;
        let chunk_end = match offset
            .checked_add(12)
            .and_then(|header| header.checked_add(length))
        {
            Some(end) if end <= encoded.len() => end,
            _ => return false,
        };
        let chunk_type = &encoded[offset + 4..offset + 8];
        if chunk_type == b"acTL" {
            return true;
        }
        if chunk_type == b"IEND" {
            return false;
        }
        offset = chunk_end;
    }
    false
}

pub(super) fn background_geometry(
    fit: BackgroundImageFit,
    image_size: (u32, u32),
    viewport: (f32, f32, f32, f32),
) -> Option<BackgroundGeometry> {
    let (image_width, image_height) = (image_size.0 as f32, image_size.1 as f32);
    let (viewport_x, viewport_y, viewport_width, viewport_height) = viewport;
    if image_width <= 0.0 || image_height <= 0.0 || viewport_width <= 0.0 || viewport_height <= 0.0
    {
        return None;
    }

    let full = |x, y, width, height| BackgroundGeometry {
        x,
        y,
        width,
        height,
        u_min: 0.0,
        v_min: 0.0,
        u_max: 1.0,
        v_max: 1.0,
    };
    Some(match fit {
        BackgroundImageFit::Stretch => {
            full(viewport_x, viewport_y, viewport_width, viewport_height)
        }
        BackgroundImageFit::Contain => {
            let scale = (viewport_width / image_width).min(viewport_height / image_height);
            let width = image_width * scale;
            let height = image_height * scale;
            full(
                viewport_x + (viewport_width - width) * 0.5,
                viewport_y + (viewport_height - height) * 0.5,
                width,
                height,
            )
        }
        BackgroundImageFit::Cover => {
            let image_aspect = image_width / image_height;
            let viewport_aspect = viewport_width / viewport_height;
            let (u_min, v_min, u_max, v_max) = if image_aspect > viewport_aspect {
                let visible = viewport_aspect / image_aspect;
                let inset = (1.0 - visible) * 0.5;
                (inset, 0.0, 1.0 - inset, 1.0)
            } else {
                let visible = image_aspect / viewport_aspect;
                let inset = (1.0 - visible) * 0.5;
                (0.0, inset, 1.0, 1.0 - inset)
            };
            BackgroundGeometry {
                x: viewport_x,
                y: viewport_y,
                width: viewport_width,
                height: viewport_height,
                u_min,
                v_min,
                u_max,
                v_max,
            }
        }
        BackgroundImageFit::Center => {
            let width = image_width.min(viewport_width);
            let height = image_height.min(viewport_height);
            let u_visible = width / image_width;
            let v_visible = height / image_height;
            BackgroundGeometry {
                x: viewport_x + (viewport_width - width) * 0.5,
                y: viewport_y + (viewport_height - height) * 0.5,
                width,
                height,
                u_min: (1.0 - u_visible) * 0.5,
                v_min: (1.0 - v_visible) * 0.5,
                u_max: 1.0 - (1.0 - u_visible) * 0.5,
                v_max: 1.0 - (1.0 - v_visible) * 0.5,
            }
        }
    })
}

impl Renderer {
    pub(crate) fn set_background_image_layout(&mut self, fit: BackgroundImageFit, opacity: f32) {
        self.background_image.fit = fit;
        self.background_image.opacity = opacity.clamp(0.0, 1.0);
    }

    pub(crate) fn install_background_image(&mut self, decoded: DecodedBackgroundImage) {
        self.background_image.revision = self.background_image.revision.wrapping_add(1).max(1);
        let mut image = StoredImage {
            id: BACKGROUND_IMAGE_KEY.1,
            width: decoded.width,
            height: decoded.height,
            data: decoded.rgba,
            revision: self.background_image.revision,
        };
        self.background_image_pipeline.upload_image(
            &self.device,
            &self.queue,
            BACKGROUND_IMAGE_KEY,
            &image,
        );
        // The dedicated GPU cache owns the steady-state pixels. A renderer
        // recreation starts a fresh bounded decode, so retaining a second CPU
        // RGBA copy for the lifetime of every window is unnecessary.
        image.data = Arc::from([]);
        self.background_image.image = Some(image);
    }

    pub(crate) fn clear_background_image(&mut self) {
        self.background_image.image = None;
        self.background_image_pipeline
            .remove_image(BACKGROUND_IMAGE_KEY);
        self.background_image_pipeline.clear();
    }

    pub(super) fn build_background_image_data(&mut self, search_bar_visible: bool) {
        self.background_image_pipeline.clear();
        let Some(image) = self.background_image.image.clone() else {
            return;
        };
        if self.background_image.opacity <= 0.0 {
            return;
        }
        let search_height = if search_bar_visible {
            self.cell_height + 16.0
        } else {
            0.0
        };
        let content_left = self.tab_bar_width();
        let content_top = self.tab_bar_height() + self.chrome_bar_height() + search_height;
        let Some(geometry) = background_geometry(
            self.background_image.fit,
            (image.width, image.height),
            (
                content_left,
                content_top,
                (self.size.0 as f32 - content_left).max(0.0),
                (self.size.1 as f32 - content_top).max(0.0),
            ),
        ) else {
            return;
        };
        self.background_image_pipeline.upload_image(
            &self.device,
            &self.queue,
            BACKGROUND_IMAGE_KEY,
            &image,
        );
        self.background_image_pipeline.push_image(
            BACKGROUND_IMAGE_KEY,
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.u_min,
            geometry.v_min,
            geometry.u_max,
            geometry.v_max,
            self.background_image.opacity,
            -1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
    }

    #[test]
    fn cover_contain_stretch_and_center_have_bounded_deterministic_geometry() {
        let viewport = (100.0, 40.0, 300.0, 200.0);
        let cover = background_geometry(BackgroundImageFit::Cover, (400, 100), viewport).unwrap();
        assert_eq!((cover.x, cover.y, cover.width, cover.height), viewport);
        assert_close(cover.u_min, 0.3125);
        assert_close(cover.u_max, 0.6875);

        let contain =
            background_geometry(BackgroundImageFit::Contain, (400, 100), viewport).unwrap();
        assert_eq!((contain.x, contain.y), (100.0, 102.5));
        assert_eq!((contain.width, contain.height), (300.0, 75.0));

        let stretch =
            background_geometry(BackgroundImageFit::Stretch, (400, 100), viewport).unwrap();
        assert_eq!(
            (stretch.x, stretch.y, stretch.width, stretch.height),
            viewport
        );
        assert_eq!(
            (stretch.u_min, stretch.v_min, stretch.u_max, stretch.v_max),
            (0.0, 0.0, 1.0, 1.0)
        );

        let center = background_geometry(BackgroundImageFit::Center, (500, 100), viewport).unwrap();
        assert_eq!(
            (center.x, center.y, center.width, center.height),
            (100.0, 90.0, 300.0, 100.0)
        );
        assert_close(center.u_min, 0.2);
        assert_close(center.u_max, 0.8);
    }

    #[test]
    fn local_loader_accepts_static_png_and_rejects_unsupported_or_oversized_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let png_path = directory.path().join("background.png");
        image::RgbaImage::from_pixel(2, 3, image::Rgba([10, 20, 30, 128]))
            .save(&png_path)
            .unwrap();
        let decoded = load_background_image(&png_path).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 3));
        assert_eq!(decoded.rgba.len(), 24);

        let jpeg_path = directory.path().join("background.jpg");
        image::RgbImage::from_pixel(3, 2, image::Rgb([40, 50, 60]))
            .save(&jpeg_path)
            .unwrap();
        let decoded = load_background_image(&jpeg_path).unwrap();
        assert_eq!((decoded.width, decoded.height), (3, 2));
        assert_eq!(decoded.rgba.len(), 24);

        let gif_path = directory.path().join("background.gif");
        image::RgbaImage::new(1, 1).save(&gif_path).unwrap();
        assert!(
            load_background_image(&gif_path)
                .unwrap_err()
                .contains("unsupported format")
        );

        let large_path = directory.path().join("too-large.png");
        let file = std::fs::File::create(&large_path).unwrap();
        file.set_len(MAX_BACKGROUND_IMAGE_ENCODED_BYTES + 1)
            .unwrap();
        assert!(
            load_background_image(&large_path)
                .unwrap_err()
                .contains("encoded bytes")
        );

        let wide_path = directory.path().join("too-wide.png");
        image::RgbaImage::new(MAX_BACKGROUND_IMAGE_DIMENSION + 1, 1)
            .save(&wide_path)
            .unwrap();
        assert!(load_background_image(&wide_path).is_err());

        assert!(
            decoded_rgba_bytes(4096, 4097).unwrap() > MAX_BACKGROUND_IMAGE_DECODED_BYTES,
            "decoded-size admission must reject compression bombs within the dimension ceiling"
        );
    }

    #[test]
    fn animation_control_is_detected_without_trusting_malformed_chunk_lengths() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&8_u32.to_be_bytes());
        png.extend_from_slice(b"acTL");
        png.extend_from_slice(&[0; 8]);
        png.extend_from_slice(&[0; 4]);
        assert!(png_has_animation_control(&png));

        let mut malformed = b"\x89PNG\r\n\x1a\n".to_vec();
        malformed.extend_from_slice(&u32::MAX.to_be_bytes());
        malformed.extend_from_slice(b"acTL");
        assert!(!png_has_animation_control(&malformed));
    }

    #[test]
    fn relative_sources_resolve_beside_the_persistent_config() {
        let relative = Path::new("images/background.png");
        assert_eq!(
            resolve_background_image_path(relative),
            crate::config::Config::config_path()
                .parent()
                .unwrap()
                .join(relative)
        );
        let absolute = std::env::temp_dir().join("termeric-background.png");
        assert_eq!(resolve_background_image_path(&absolute), absolute);
    }
}
