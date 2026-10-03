//! A still picture of the dragged tab, copied out of the frame that drew it,
//! for the pointer to carry once the tab leaves its window.

use super::Renderer;
use super::types::DraggedTabPicture;
use crate::ui::geometry::Bounds;

/// How a surface stores the four channels of a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChannelOrder {
    Rgba,
    Bgra,
}

impl ChannelOrder {
    /// The order of an eight-bit surface format; wider formats are not read
    /// back.
    fn of(format: wgpu::TextureFormat) -> Option<Self> {
        match format {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
                Some(Self::Rgba)
            }
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                Some(Self::Bgra)
            }
            _ => None,
        }
    }
}

/// What the surface's alpha channel means to the compositor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoredAlpha {
    /// Colors are already multiplied by alpha.
    Premultiplied,
    /// Colors are independent of alpha.
    Straight,
    /// The compositor ignores alpha: the window is opaque.
    Ignored,
}

impl StoredAlpha {
    fn of(mode: wgpu::CompositeAlphaMode) -> Self {
        match mode {
            wgpu::CompositeAlphaMode::PreMultiplied => Self::Premultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied => Self::Straight,
            _ => Self::Ignored,
        }
    }
}

/// The dragged tab's pixels on their way back from the GPU.
pub(super) struct PendingTabCopy {
    buffer: wgpu::Buffer,
    layout: CopiedRows,
    origin: (u32, u32),
}

/// How the copied rows lie in the readback buffer.
#[derive(Clone, Copy, Debug)]
struct CopiedRows {
    width: u32,
    height: u32,
    /// Each row is padded to the copy alignment.
    stride: u32,
    order: ChannelOrder,
    alpha: StoredAlpha,
    corner_radius: f32,
}

impl Renderer {
    /// Copies the dragged tab out of the drawn frame when the app asked for
    /// its picture, the tab was drawn, and the surface can be read back.
    pub(super) fn copy_dragged_tab(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) -> Option<PendingTabCopy> {
        if !self.tab_drag.capture || !self.surface_readable {
            return None;
        }
        let order = ChannelOrder::of(self.config.format)?;
        let (x, y, width, height) =
            pixel_bounds(self.dragged_tab_surface?, texture.width(), texture.height())?;
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Dragged tab picture"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Some(PendingTabCopy {
            buffer,
            layout: CopiedRows {
                width,
                height,
                stride,
                order,
                alpha: StoredAlpha::of(self.config.alpha_mode),
                corner_radius: self.tab_chrome_corner_radius(),
            },
            origin: (x, y),
        })
    }

    /// Waits for a copy submitted with this frame and keeps its picture for
    /// the app. Happens once per drag, so the short wait is not felt.
    pub(super) fn finish_dragged_tab_copy(
        &mut self,
        copy: PendingTabCopy,
        submission: wgpu::SubmissionIndex,
    ) {
        let slice = copy.buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let _ = self.device.poll(wgpu::Maintain::wait_for(submission));
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::warn!("Could not read the dragged tab's picture: {error}");
                return;
            }
            Err(_) => {
                log::warn!("The dragged tab's picture was not ready after its frame");
                return;
            }
        }
        let rgba = straight_rgba(&slice.get_mapped_range(), copy.layout);
        copy.buffer.unmap();
        log::debug!(
            "Pictured the dragged tab at {}x{} pixels",
            copy.layout.width,
            copy.layout.height
        );
        self.dragged_tab_picture = Some(DraggedTabPicture {
            rgba,
            width: copy.layout.width,
            height: copy.layout.height,
            origin: copy.origin,
        });
    }

    /// The picture copied from the last frame that asked for one.
    pub fn take_dragged_tab_picture(&mut self) -> Option<DraggedTabPicture> {
        self.dragged_tab_picture.take()
    }
}

/// The whole pixels a drawn rectangle covers within a `width` × `height`
/// texture, as (x, y, width, height); `None` when none are covered.
fn pixel_bounds(rect: Bounds, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let clamp = |value: f32, limit: u32| value.round().clamp(0.0, limit as f32) as u32;
    let (left, top) = (clamp(rect.x, width), clamp(rect.y, height));
    let (right, bottom) = (clamp(rect.right(), width), clamp(rect.bottom(), height));
    (right > left && bottom > top).then(|| (left, top, right - left, bottom - top))
}

/// Straight-alpha RGBA rows from copied surface rows, with the tab's corners
/// cut round so the tab bar behind them does not show.
fn straight_rgba(bytes: &[u8], layout: CopiedRows) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((layout.width * layout.height * 4) as usize);
    for row in 0..layout.height {
        let start = (row * layout.stride) as usize;
        let pixels = &bytes[start..start + (layout.width * 4) as usize];
        for (column, &[first, g, third, a]) in pixels.as_chunks::<4>().0.iter().enumerate() {
            let (r, b) = match layout.order {
                ChannelOrder::Rgba => (first, third),
                ChannelOrder::Bgra => (third, first),
            };
            let [r, g, b, a] = match layout.alpha {
                StoredAlpha::Premultiplied => unpremultiply([r, g, b, a]),
                StoredAlpha::Straight => [r, g, b, a],
                StoredAlpha::Ignored => [r, g, b, u8::MAX],
            };
            let coverage = corner_coverage(
                column as f32 + 0.5,
                row as f32 + 0.5,
                layout.width as f32,
                layout.height as f32,
                layout.corner_radius,
            );
            rgba.extend_from_slice(&[r, g, b, (f32::from(a) * coverage).round() as u8]);
        }
    }
    rgba
}

fn unpremultiply([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    if a == 0 {
        return [0; 4];
    }
    let alpha = u32::from(a);
    let undo = |channel: u8| ((u32::from(channel) * 255 + alpha / 2) / alpha).min(255) as u8;
    [undo(r), undo(g), undo(b), a]
}

/// How much of the pixel centered at (`x`, `y`) lies inside a `width` ×
/// `height` rectangle with corners of `radius`, antialiased over one pixel.
fn corner_coverage(x: f32, y: f32, width: f32, height: f32, radius: f32) -> f32 {
    let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    let dx = (radius - x).max(x - (width - radius)).max(0.0);
    let dy = (radius - y).max(y - (height - radius)).max(0.0);
    if dx == 0.0 || dy == 0.0 {
        return 1.0;
    }
    let outside = dx.hypot(dy) - radius;
    (0.5 - outside).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(width: u32, height: u32, stride: u32) -> CopiedRows {
        CopiedRows {
            width,
            height,
            stride,
            order: ChannelOrder::Bgra,
            alpha: StoredAlpha::Premultiplied,
            corner_radius: 0.0,
        }
    }

    #[test]
    fn only_eight_bit_surfaces_are_read_back() {
        assert_eq!(
            ChannelOrder::of(wgpu::TextureFormat::Bgra8UnormSrgb),
            Some(ChannelOrder::Bgra)
        );
        assert_eq!(
            ChannelOrder::of(wgpu::TextureFormat::Rgba8Unorm),
            Some(ChannelOrder::Rgba)
        );
        assert_eq!(ChannelOrder::of(wgpu::TextureFormat::Rgba16Float), None);
    }

    #[test]
    fn a_drawn_rect_covers_whole_pixels_inside_the_frame() {
        assert_eq!(
            pixel_bounds(Bounds::new(10.4, 4.0, 100.2, 28.0), 800, 600),
            Some((10, 4, 101, 28))
        );
        assert_eq!(
            pixel_bounds(Bounds::new(-20.0, 4.0, 100.0, 28.0), 800, 600),
            Some((0, 4, 80, 28))
        );
        assert_eq!(
            pixel_bounds(Bounds::new(790.0, 4.0, 100.0, 28.0), 800, 600),
            Some((790, 4, 10, 28))
        );
        assert_eq!(
            pixel_bounds(Bounds::new(900.0, 4.0, 100.0, 28.0), 800, 600),
            None
        );
        assert_eq!(
            pixel_bounds(Bounds::new(10.0, 4.0, 0.2, 28.0), 800, 600),
            None
        );
    }

    #[test]
    fn copied_rows_drop_their_padding_and_come_out_rgba() {
        // Two pixels per row, padded to three.
        let bytes = [
            30, 20, 10, 255, 60, 50, 40, 255, 0, 0, 0, 0, //
            3, 2, 1, 255, 6, 5, 4, 255, 9, 9, 9, 9,
        ];
        assert_eq!(
            straight_rgba(&bytes, layout(2, 2, 12)),
            [
                10, 20, 30, 255, 40, 50, 60, 255, //
                1, 2, 3, 255, 4, 5, 6, 255,
            ]
        );
    }

    #[test]
    fn a_translucent_window_gives_a_translucent_picture_in_straight_alpha() {
        let bytes = [0, 64, 128, 128];
        assert_eq!(straight_rgba(&bytes, layout(1, 1, 4)), [255, 128, 0, 128]);
        let opaque = CopiedRows {
            alpha: StoredAlpha::Ignored,
            ..layout(1, 1, 4)
        };
        assert_eq!(straight_rgba(&bytes, opaque), [128, 64, 0, 255]);
        assert_eq!(unpremultiply([0, 0, 0, 0]), [0; 4]);
        assert_eq!(unpremultiply([200, 10, 255, 255]), [200, 10, 255, 255]);
    }

    #[test]
    fn the_corners_are_cut_round_and_the_edges_kept() {
        let (width, height, radius) = (40.0, 20.0, 6.0);
        // The very corner lies outside the curve.
        assert_eq!(corner_coverage(0.5, 0.5, width, height, radius), 0.0);
        assert_eq!(corner_coverage(39.5, 19.5, width, height, radius), 0.0);
        // Along the straight edges and inside, everything stays.
        assert_eq!(corner_coverage(20.5, 0.5, width, height, radius), 1.0);
        assert_eq!(corner_coverage(0.5, 10.5, width, height, radius), 1.0);
        assert_eq!(corner_coverage(6.5, 6.5, width, height, radius), 1.0);
        // On the curve, partly.
        let edge = corner_coverage(1.5, 2.5, width, height, radius);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
        // No radius keeps every pixel.
        assert_eq!(corner_coverage(0.5, 0.5, width, height, 0.0), 1.0);
    }
}
