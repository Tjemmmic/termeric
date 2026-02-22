//! Graphics system for inline images
//!
//! Implements the Kitty Graphics Protocol for displaying images in the terminal.
//! Images are stored in GPU-friendly format and referenced by cell positions.

mod kitty;
mod image;

pub use kitty::{KittyCommand, KittyAction, KittyTransmission, KittyFormat, KittyPlacement};
pub use image::{ImageStorage, ImageId, StoredImage, ImagePlacement};

use std::collections::HashMap;

/// Maximum number of images to keep in storage
const MAX_IMAGES: usize = 256;

/// Maximum total image memory (64 MB)
const MAX_IMAGE_MEMORY: usize = 64 * 1024 * 1024;

/// Graphics manager for handling inline images
#[derive(Debug)]
pub struct GraphicsManager {
    /// Image storage
    storage: ImageStorage,
    /// Pending image data during chunked transmission
    pending: HashMap<u32, PendingImage>,
    /// Images placed on the terminal grid
    placements: Vec<ImagePlacement>,
}

/// Image being received in chunks
#[derive(Debug)]
struct PendingImage {
    /// Image ID (assigned by terminal or client)
    id: ImageId,
    /// Data format
    format: KittyFormat,
    /// Expected dimensions (if provided)
    width: Option<u32>,
    height: Option<u32>,
    /// Accumulated data chunks
    data: Vec<u8>,
    /// Whether more chunks are expected
    more_chunks: bool,
}

impl GraphicsManager {
    /// Create a new graphics manager
    pub fn new() -> Self {
        Self {
            storage: ImageStorage::new(MAX_IMAGES, MAX_IMAGE_MEMORY),
            pending: HashMap::new(),
            placements: Vec::new(),
        }
    }

    /// Handle a Kitty graphics command
    pub fn handle_command(&mut self, cmd: KittyCommand) -> Option<GraphicsResponse> {
        match cmd.action {
            KittyAction::Transmit | KittyAction::TransmitAndDisplay => {
                self.handle_transmission(cmd)
            }
            KittyAction::Display => {
                self.handle_display(cmd)
            }
            KittyAction::Delete => {
                self.handle_delete(cmd)
            }
            KittyAction::Query => {
                Some(GraphicsResponse::ok(cmd.image_id.unwrap_or(0)))
            }
            KittyAction::FrameComposition => {
                // Animation frames - not implemented yet
                log::trace!("Kitty animation frame composition not implemented");
                None
            }
        }
    }

    /// Handle image transmission
    fn handle_transmission(&mut self, cmd: KittyCommand) -> Option<GraphicsResponse> {
        let image_id = cmd.image_id.unwrap_or(0);
        let placement_id = cmd.placement_id;

        // Check if this is a continuation of a chunked transmission
        if let Some(pending) = self.pending.get_mut(&image_id) {
            if let Some(data) = cmd.data {
                pending.data.extend_from_slice(&data);
            }
            pending.more_chunks = cmd.more_chunks;

            if !cmd.more_chunks {
                // Final chunk received, process the complete image
                let pending = self.pending.remove(&image_id)?;
                return self.finalize_image(pending, cmd.action, placement_id, cmd.placement);
            }
            return None;
        }

        // Start a new transmission
        let data = cmd.data.unwrap_or_default();
        let format = cmd.transmission.as_ref()
            .map(|t| t.format)
            .unwrap_or(KittyFormat::Rgba);
        let width = cmd.transmission.as_ref().and_then(|t| t.width);
        let height = cmd.transmission.as_ref().and_then(|t| t.height);

        if cmd.more_chunks {
            // Start chunked transmission
            self.pending.insert(image_id, PendingImage {
                id: ImageId(image_id),
                format,
                width,
                height,
                data,
                more_chunks: true,
            });
            return None;
        }

        // Single-chunk transmission, process immediately
        let pending = PendingImage {
            id: ImageId(image_id),
            format,
            width,
            height,
            data,
            more_chunks: false,
        };

        self.finalize_image(pending, cmd.action, placement_id, cmd.placement)
    }

    /// Finalize image after all chunks received
    fn finalize_image(
        &mut self,
        pending: PendingImage,
        action: KittyAction,
        placement_id: Option<u32>,
        placement: Option<KittyPlacement>,
    ) -> Option<GraphicsResponse> {
        // Decode the image
        let decoded = match self.decode_image(&pending) {
            Ok(img) => img,
            Err(e) => {
                log::warn!("Failed to decode image: {}", e);
                return Some(GraphicsResponse::error(pending.id.0, &e));
            }
        };

        // Store the image
        let stored = StoredImage {
            id: pending.id,
            width: decoded.width,
            height: decoded.height,
            data: decoded.data,
        };

        if let Err(e) = self.storage.store(stored) {
            log::warn!("Failed to store image: {}", e);
            return Some(GraphicsResponse::error(pending.id.0, &e));
        }

        // If this is transmit-and-display, create placement
        if action == KittyAction::TransmitAndDisplay {
            if let Some(placement) = placement {
                self.add_placement(pending.id, placement_id, placement);
            } else {
                // Default placement at cursor
                self.add_placement(pending.id, placement_id, KittyPlacement::default());
            }
        }

        Some(GraphicsResponse::ok(pending.id.0))
    }

    /// Decode image data based on format
    fn decode_image(&self, pending: &PendingImage) -> Result<DecodedImage, String> {
        match pending.format {
            KittyFormat::Rgba => {
                // Raw RGBA data
                let width = pending.width.ok_or("Width required for raw RGBA")?;
                let height = pending.height.ok_or("Height required for raw RGBA")?;
                let expected = (width * height * 4) as usize;
                if pending.data.len() != expected {
                    return Err(format!(
                        "RGBA data size mismatch: expected {}, got {}",
                        expected, pending.data.len()
                    ));
                }
                Ok(DecodedImage {
                    width,
                    height,
                    data: pending.data.clone(),
                })
            }
            KittyFormat::Rgb => {
                // Raw RGB data - convert to RGBA
                let width = pending.width.ok_or("Width required for raw RGB")?;
                let height = pending.height.ok_or("Height required for raw RGB")?;
                let expected = (width * height * 3) as usize;
                if pending.data.len() != expected {
                    return Err(format!(
                        "RGB data size mismatch: expected {}, got {}",
                        expected, pending.data.len()
                    ));
                }
                let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                for chunk in pending.data.chunks(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                Ok(DecodedImage {
                    width,
                    height,
                    data: rgba,
                })
            }
            KittyFormat::Png => {
                // PNG encoded data - needs image crate (deferred to renderer)
                // For now, store the PNG data as-is and let the renderer decode it
                Err("PNG decoding deferred to renderer".to_string())
            }
        }
    }

    /// Handle display command
    fn handle_display(&mut self, cmd: KittyCommand) -> Option<GraphicsResponse> {
        let image_id = ImageId(cmd.image_id?);

        if !self.storage.contains(image_id) {
            return Some(GraphicsResponse::error(image_id.0, "Image not found"));
        }

        let placement = cmd.placement.unwrap_or_default();
        self.add_placement(image_id, cmd.placement_id, placement);

        Some(GraphicsResponse::ok(image_id.0))
    }

    /// Handle delete command
    fn handle_delete(&mut self, cmd: KittyCommand) -> Option<GraphicsResponse> {
        if let Some(image_id) = cmd.image_id {
            // Delete specific image
            self.storage.remove(ImageId(image_id));
            self.placements.retain(|p| p.image_id != ImageId(image_id));
        } else {
            // Delete all images
            self.storage.clear();
            self.placements.clear();
        }
        None
    }

    /// Add an image placement
    fn add_placement(&mut self, image_id: ImageId, placement_id: Option<u32>, placement: KittyPlacement) {
        let placement = ImagePlacement {
            image_id,
            placement_id,
            col: placement.col.unwrap_or(0),
            row: placement.row.unwrap_or(0),
            x_offset: placement.x_offset.unwrap_or(0),
            y_offset: placement.y_offset.unwrap_or(0),
            width: placement.width,
            height: placement.height,
            z_index: placement.z_index.unwrap_or(0),
        };
        self.placements.push(placement);
    }

    /// Get all image placements for rendering
    pub fn placements(&self) -> &[ImagePlacement] {
        &self.placements
    }

    /// Get image data for rendering
    pub fn get_image(&self, id: ImageId) -> Option<&StoredImage> {
        self.storage.get(id)
    }

    /// Clear placements that have scrolled off screen
    pub fn cleanup_placements(&mut self, min_row: i32, max_row: i32) {
        self.placements.retain(|p| {
            let row = p.row as i32;
            row >= min_row && row <= max_row
        });
    }

    /// Check if there are any images to render
    pub fn has_images(&self) -> bool {
        !self.placements.is_empty()
    }
}

impl Default for GraphicsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Decoded image data
struct DecodedImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

/// Response to send back to the terminal
#[derive(Debug, Clone)]
pub struct GraphicsResponse {
    /// Image ID
    pub image_id: u32,
    /// Whether the operation succeeded
    pub ok: bool,
    /// Error message if failed
    pub error: Option<String>,
}

impl GraphicsResponse {
    fn ok(image_id: u32) -> Self {
        Self {
            image_id,
            ok: true,
            error: None,
        }
    }

    fn error(image_id: u32, msg: &str) -> Self {
        Self {
            image_id,
            ok: false,
            error: Some(msg.to_string()),
        }
    }

    /// Format as Kitty graphics response
    pub fn to_escape_sequence(&self) -> String {
        if self.ok {
            format!("\x1b_Gi={};OK\x1b\\", self.image_id)
        } else {
            let msg = self.error.as_deref().unwrap_or("Unknown error");
            format!("\x1b_Gi={};ENOENT:{}\x1b\\", self.image_id, msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graphics_manager_new() {
        let manager = GraphicsManager::new();
        assert!(!manager.has_images());
        assert!(manager.placements().is_empty());
    }
}
