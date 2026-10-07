//! The background image: its viewport geometry, GPU state, and drawing.

use std::sync::Arc;

use termeric_core::{ImageId, StoredImage};

use super::Renderer;
use super::image::{ImageKey, ImageQuad};
use crate::config::BackgroundImageFit;

mod load;
pub(crate) use load::{
    BackgroundImageStamp, DecodedBackgroundImage, load_background_image,
    resolve_background_image_path,
};

const BACKGROUND_IMAGE_KEY: ImageKey = (0, ImageId(0));

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

pub(super) fn background_geometry(
    fit: BackgroundImageFit,
    image_size: (u32, u32),
    viewport: (f32, f32, f32, f32),
) -> Option<ImageQuad> {
    let (image_width, image_height) = (image_size.0 as f32, image_size.1 as f32);
    let (viewport_x, viewport_y, viewport_width, viewport_height) = viewport;
    if image_width <= 0.0 || image_height <= 0.0 || viewport_width <= 0.0 || viewport_height <= 0.0
    {
        return None;
    }

    let full = |x, y, width, height| ImageQuad {
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
            ImageQuad {
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
            ImageQuad {
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
        pipeline.push_image(BACKGROUND_IMAGE_KEY, geometry, self.opacity, -1);
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
}
