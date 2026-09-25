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

/// Version of the opened source, never a later pathname lookup. Unix identity
/// and change time also detect atomic replacements that preserve mtime/length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackgroundImageStamp {
    length: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl BackgroundImageStamp {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                )
            },
        }
    }

    pub(crate) fn for_path(path: &Path) -> Option<Self> {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| Self::from_metadata(&metadata))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedBackgroundImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    pub source_stamp: BackgroundImageStamp,
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
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // User-configured symlinks are allowed. A FIFO substituted at open
        // must not occupy the sole decoder worker waiting for a writer.
        options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    load_opened_background_image(file, path)
}

fn load_opened_background_image(
    file: std::fs::File,
    path: &Path,
) -> Result<DecodedBackgroundImage, String> {
    let metadata = file
        .metadata()
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

    let source_stamp = BackgroundImageStamp::from_metadata(&metadata);
    let mut encoded = Vec::with_capacity(metadata.len() as usize);
    (&file)
        .take(MAX_BACKGROUND_IMAGE_ENCODED_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if encoded.len() as u64 > MAX_BACKGROUND_IMAGE_ENCODED_BYTES {
        return Err(format!(
            "{} changed while loading and exceeded the {} byte encoded limit",
            path.display(),
            MAX_BACKGROUND_IMAGE_ENCODED_BYTES
        ));
    }
    let final_metadata = file
        .metadata()
        .map_err(|error| format!("could not recheck {}: {error}", path.display()))?;
    if source_stamp != BackgroundImageStamp::from_metadata(&final_metadata) {
        return Err(format!(
            "{} changed while reading; keeping the last good image",
            path.display()
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
        source_stamp,
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
        self.background_image.install(
            decoded,
            &self.device,
            &self.queue,
            &mut self.background_image_pipeline,
        );
    }

    pub(crate) fn clear_background_image(&mut self) {
        self.background_image
            .clear(&mut self.background_image_pipeline);
    }

    pub(super) fn build_background_image_data(&mut self) {
        let content_left = self.tab_bar_width();
        let content_top = self.content_top();
        let viewport = (
            content_left,
            content_top,
            self.content_width_for(self.size.0 as f32),
            (self.size.1 as f32 - content_top).max(0.0),
        );
        self.background_image.queue(
            &self.device,
            &self.queue,
            &mut self.background_image_pipeline,
            viewport,
        );
    }
}

impl BackgroundImageState {
    fn install(
        &mut self,
        decoded: DecodedBackgroundImage,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &mut super::image::ImagePipeline,
    ) {
        self.revision = self.revision.wrapping_add(1).max(1);
        let mut image = StoredImage {
            id: BACKGROUND_IMAGE_KEY.1,
            width: decoded.width,
            height: decoded.height,
            data: decoded.rgba,
            revision: self.revision,
        };
        pipeline.upload_image(device, queue, BACKGROUND_IMAGE_KEY, &image);
        // The dedicated GPU cache owns the steady-state pixels. A renderer
        // recreation starts a fresh bounded decode, so retaining a second CPU
        // RGBA copy for the lifetime of every window is unnecessary.
        image.data = Arc::from([]);
        self.image = Some(image);
    }

    fn clear(&mut self, pipeline: &mut super::image::ImagePipeline) {
        self.image = None;
        pipeline.remove_image(BACKGROUND_IMAGE_KEY);
        pipeline.clear();
    }

    fn queue(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &mut super::image::ImagePipeline,
        viewport: (f32, f32, f32, f32),
    ) {
        pipeline.clear();
        let Some(image) = self.image.as_ref() else {
            return;
        };
        if self.opacity <= 0.0 {
            return;
        }
        let Some(geometry) = background_geometry(self.fit, (image.width, image.height), viewport)
        else {
            return;
        };
        pipeline.upload_image(device, queue, BACKGROUND_IMAGE_KEY, image);
        pipeline.push_image(
            BACKGROUND_IMAGE_KEY,
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.u_min,
            geometry.v_min,
            geometry.u_max,
            geometry.v_max,
            self.opacity,
            -1,
        );
    }
}

#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
pub(crate) use gpu_tests::headless_background_render_smoke;

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

    #[cfg(unix)]
    #[test]
    fn opened_background_source_keeps_its_pixels_and_stamp_after_path_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let old_stamp = BackgroundImageStamp::from_metadata(&file.metadata().unwrap());
        let replacement = dir.path().join("replacement.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 50, 60, 255]))
            .save(&replacement)
            .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        // Renaming over the old inode changes its ctime/link count. Capture
        // the opened source after that transition, just as admission does.
        let expected_stamp = BackgroundImageStamp::from_metadata(&file.metadata().unwrap());
        let decoded = load_opened_background_image(file, &path).unwrap();
        assert_eq!(&decoded.rgba[..4], &[10, 20, 30, 255]);
        assert_eq!(decoded.source_stamp, expected_stamp);
        assert_ne!(Some(old_stamp), BackgroundImageStamp::for_path(&path));
        assert_ne!(
            Some(decoded.source_stamp),
            BackgroundImageStamp::for_path(&path)
        );

        let large = dir.path().join("oversized.png");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len(MAX_BACKGROUND_IMAGE_ENCODED_BYTES + 1)
            .unwrap();
        let oversized = std::fs::File::open(&large).unwrap();
        // Admission must inspect the handle, even when the display path now
        // names a different small, valid image.
        assert!(
            load_opened_background_image(oversized, &path)
                .unwrap_err()
                .contains("encoded bytes")
        );
    }

    #[cfg(unix)]
    #[test]
    fn background_symlink_tracks_atomic_replacement_with_preserved_mtime_and_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.png");
        let link = dir.path().join("configured.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let original = load_background_image(&link).unwrap();
        let replacement = dir.path().join("replacement.png");
        std::fs::copy(&path, &replacement).unwrap();
        std::fs::File::open(&replacement)
            .unwrap()
            .set_modified(original.source_stamp.modified.unwrap())
            .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        let latest = load_background_image(&link).unwrap();
        assert_eq!(original.rgba, latest.rgba);
        assert_eq!(original.source_stamp.modified, latest.source_stamp.modified);
        assert_eq!(original.source_stamp.length, latest.source_stamp.length);
        assert_ne!(original.source_stamp, latest.source_stamp);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn background_fifo_is_rejected_without_waiting_for_a_writer() {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.png");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name points to a live C string inside the private fixture.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let source = path.clone();
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(load_background_image(&source)).unwrap();
        });
        let mut rescued = false;
        let result = match receive.recv_timeout(Duration::from_millis(300)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                rescued = true;
                let _peer = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&path)
                    .unwrap();
                receive.recv_timeout(Duration::from_secs(2)).unwrap()
            }
            Err(error) => panic!("background worker failed: {error}"),
        };
        worker.join().unwrap();
        assert!(!rescued, "background worker waited for a FIFO writer");
        assert!(result.unwrap_err().contains("regular file"));
        assert!(path.exists());
        let valid = dir.path().join("valid.png");
        image::RgbaImage::new(1, 1).save(&valid).unwrap();
        assert!(load_background_image(&valid).is_ok());
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
