//! Bounded static-image adapters for Kitty, Sixel, and iTerm2 protocols.

mod animation;
mod decode;
mod image;
mod iterm2;
mod kitty;
mod placeholder;
mod placement_geometry;
mod presentation;
mod relay;
mod response;
mod sixel;
mod transmission;

pub(crate) use decode::{DecodedImage, checked_pixel_bytes, decode_png};
pub use image::{ImageId, ImagePlacement, ImageStorage, PresentationImage, StoredImage};
pub(crate) use iterm2::Iterm2Command;
pub(crate) use kitty::{
    KittyAction, KittyCommand, KittyCompression, KittyFormat, KittyFrame, KittyMedium,
    KittyPlacement, KittyTransmission, MAX_APC_BYTES as MAX_KITTY_APC_BYTES,
};
pub use placeholder::IMAGE_PLACEHOLDER;
pub(crate) use presentation::GraphicsSnapshot;
pub use relay::GraphicsRelaySnapshot;
pub(crate) use response::{GraphicsResponse, ProtocolError};
pub(crate) use sixel::SixelCommand;

use crate::grid::Grid;
use animation::{
    advance_animation_frame, animation_deadline, compose_rgba_overlay, compose_rgba_rectangle,
    rectangles_overlap, validate_frame_rectangle,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use decode::{decode_image, decode_payload};
use flate2::read::ZlibDecoder;
use placeholder::PlaceholderDecoder;
use placement_geometry::{
    PlacementRect, crop_placement, merge_adjacent_placeholder_presentation,
    placeholder_presentation, placement_intersects, placement_rect, relative_negative_presentation,
    resolve_kitty_display_geometry,
};
use response::{
    failure, frame_response_fields, pending_failure, pending_frame_failure, response_fields,
    response_for, response_for_resolved,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};
use std::io::{Seek, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use transmission::prepare_transmission_data;

pub const MAX_IMAGES: usize = 256;
/// A placement is roughly one renderer instance plus terminal lifecycle
/// metadata. Bound it independently from decoded images because one reusable
/// image may otherwise create an unlimited number of placements.
pub const MAX_IMAGE_PLACEMENTS: usize = 1_024;
/// Virtual placements are inert prototypes rather than renderer instances,
/// but they remain independently bounded because clients may create them
/// without adding any visible cells.
pub(crate) const MAX_VIRTUAL_IMAGE_PLACEMENTS: usize = 1_024;
/// Relative placements may be chained, but resolution work must remain
/// bounded. The protocol requires support for at least eight ancestors.
pub(crate) const MAX_RELATIVE_PLACEMENT_DEPTH: usize = 32;
pub(crate) const MAX_ANIMATION_FRAMES_PER_IMAGE: usize = 1_024;
pub(crate) const MAX_ANIMATION_FRAMES_TOTAL: usize = 4_096;
pub(crate) const MAX_ANIMATION_MEMORY: usize = 5 * MAX_IMAGE_MEMORY;
const DEFAULT_ANIMATION_GAP_MS: u32 = 40;
const GRAPHICS_RELAY_VERSION: u32 = 1;
pub(crate) const MAX_GRAPHICS_RELAY_METADATA_BYTES: usize = 4 * 1024 * 1024;
const RELAY_PIXEL_CHUNK_BYTES: usize = 3 * 1024;
const RELAY_METADATA_CHUNK_BYTES: usize = 24 * 1024;
pub const MAX_IMAGE_MEMORY: usize = 64 * 1024 * 1024;
pub(crate) const MAX_IMAGE_DIMENSION: u32 = 16_384;
pub(crate) const MAX_TRANSMISSION_BYTES: usize = MAX_IMAGE_MEMORY;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProtocolPlacement {
    pub col: u32,
    pub row: i64,
    pub columns: u32,
    pub rows: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub alternate_screen: bool,
}

#[derive(Debug)]
pub(crate) struct GraphicsManager {
    storage: ImageStorage,
    pending: Option<PendingImage>,
    pending_frame: Option<PendingFrame>,
    placements: VecDeque<ManagedImagePlacement>,
    virtual_placements: VecDeque<VirtualImagePlacement>,
    /// Ordinary-file transmission lets terminal applications request reads
    /// from persistent host paths. Keep it behind an explicit host policy;
    /// temporary files and POSIX shared memory have narrower lifecycles.
    allow_file_transmission: bool,
    /// Image numbers are non-unique aliases. Each number retains creation
    /// order so commands can resolve the newest image that has not been
    /// deleted or evicted.
    image_numbers: HashMap<u32, VecDeque<ImageId>>,
    image_number_by_id: HashMap<ImageId, u32>,
    /// Sixel and OSC 1337 do not expose reusable image IDs. Track their
    /// anonymous cache entries so removing the last placement also releases
    /// decoded pixels instead of retaining unreachable data.
    protocol_images: HashSet<ImageId>,
    next_anonymous_id: u32,
    next_client_id: u32,
    next_revision: u64,
    next_placement_uid: u64,
    animations: HashMap<ImageId, AnimatedImage>,
    animation_memory: usize,
    cell_pixel_width: u32,
    cell_pixel_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PlacementUid(u64);

#[derive(Debug, Clone, Copy)]
struct RelativePlacement {
    parent: PlacementUid,
    horizontal_offset: i32,
    vertical_offset: i32,
}

#[derive(Debug, Clone)]
struct ManagedImagePlacement {
    uid: PlacementUid,
    placement: ImagePlacement,
    relative: Option<RelativePlacement>,
}

impl Deref for ManagedImagePlacement {
    type Target = ImagePlacement;

    fn deref(&self) -> &Self::Target {
        &self.placement
    }
}

impl DerefMut for ManagedImagePlacement {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.placement
    }
}

#[derive(Debug, Clone, Copy)]
struct VirtualImagePlacement {
    uid: PlacementUid,
    image_id: ImageId,
    placement_id: Option<u32>,
    columns: Option<u32>,
    rows: Option<u32>,
    alternate_screen: bool,
}

#[derive(Debug)]
struct PendingImage {
    storage_id: ImageId,
    response_id: u32,
    response_number: Option<u32>,
    placement_id: Option<u32>,
    transmission: KittyTransmission,
    action: KittyAction,
    placement: KittyPlacement,
    data: Vec<u8>,
    quiet: u8,
    alternate_screen: bool,
}

#[derive(Debug)]
struct PendingFrame {
    image_id: ImageId,
    response_id: u32,
    response_number: Option<u32>,
    transmission: KittyTransmission,
    frame: KittyFrame,
    data: Vec<u8>,
    quiet: u8,
}

#[derive(Debug, Clone)]
struct AnimationFrame {
    data: Arc<[u8]>,
    revision: u64,
    gap_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimationPlayback {
    Stopped,
    Loading,
    Running,
}

#[derive(Debug)]
struct AnimatedImage {
    frames: Vec<AnimationFrame>,
    root_gap_ms: u32,
    current_frame: usize,
    playback: AnimationPlayback,
    /// `None` loops forever; `Some(n)` permits `n` completed wraps.
    max_loops: Option<u32>,
    completed_loops: u32,
    shown_since: Instant,
}

/// Non-PTY side effects of a successfully handled graphics command.
#[derive(Debug, Clone, Default)]
pub(crate) struct GraphicsCommandResult {
    pub response: Option<GraphicsResponse>,
    pub cursor_movement: Option<(u32, u32)>,
}

impl GraphicsManager {
    pub fn new() -> Self {
        Self {
            storage: ImageStorage::new(MAX_IMAGES, MAX_IMAGE_MEMORY),
            pending: None,
            pending_frame: None,
            placements: VecDeque::new(),
            virtual_placements: VecDeque::new(),
            allow_file_transmission: false,
            image_numbers: HashMap::new(),
            image_number_by_id: HashMap::new(),
            protocol_images: HashSet::new(),
            next_anonymous_id: u32::MAX,
            next_client_id: 1,
            next_revision: 1,
            next_placement_uid: 1,
            animations: HashMap::new(),
            animation_memory: 0,
            cell_pixel_width: 8,
            cell_pixel_height: 16,
        }
    }

    pub(crate) fn handle_command(
        &mut self,
        cmd: KittyCommand,
        alternate_screen: bool,
    ) -> GraphicsCommandResult {
        self.handle_command_at(cmd, alternate_screen, Instant::now())
    }

    fn handle_command_at(
        &mut self,
        cmd: KittyCommand,
        alternate_screen: bool,
        now: Instant,
    ) -> GraphicsCommandResult {
        if self.pending_frame.is_some()
            && matches!(
                cmd.action,
                KittyAction::Transmit | KittyAction::TransmitFrame
            )
        {
            return self.continue_frame_transmission(cmd, now);
        }
        if let Some(pending) = self.pending_frame.take() {
            return pending_frame_failure(
                pending,
                ProtocolError::invalid("frame transmission was interrupted by another command"),
            );
        }
        if self.pending.is_some()
            && matches!(
                cmd.action,
                KittyAction::Transmit | KittyAction::TransmitAndDisplay
            )
        {
            return self.continue_transmission(cmd);
        }
        if let Some(pending) = self.pending.take() {
            return pending_failure(
                pending,
                ProtocolError::invalid("image transmission was interrupted by another command"),
            );
        }
        match cmd.action {
            KittyAction::Transmit | KittyAction::TransmitAndDisplay => {
                self.start_transmission(cmd, alternate_screen)
            }
            KittyAction::Display => self.handle_display(cmd, alternate_screen),
            KittyAction::Delete => self.handle_delete(cmd, alternate_screen),
            KittyAction::Query => self.handle_query(cmd),
            KittyAction::TransmitFrame => self.start_frame_transmission(cmd, now),
            KittyAction::AnimationControl => self.handle_animation_control(cmd, now),
            KittyAction::FrameComposition => self.handle_frame_composition(cmd),
        }
    }

    /// Permit or deny Kitty's persistent ordinary-file transmission medium.
    /// This host policy is intentionally independent of terminal reset state.
    pub(crate) fn set_file_transmission_allowed(&mut self, allowed: bool) {
        self.allow_file_transmission = allowed;
    }

    pub(crate) fn file_transmission_allowed(&self) -> bool {
        self.allow_file_transmission
    }

    fn apply_frame_update(
        &mut self,
        image_id: ImageId,
        frame_command: KittyFrame,
        decoded: DecodedImage,
        now: Instant,
    ) -> Result<u32, ProtocolError> {
        let (image_width, image_height) = self
            .storage
            .get(image_id)
            .map(|image| (image.width, image.height))
            .ok_or_else(|| ProtocolError::not_found("animation image was evicted"))?;
        if decoded.width > image_width || decoded.height > image_height {
            return Err(ProtocolError::invalid(
                "transmitted frame rectangle is larger than the image",
            ));
        }
        let existing_count = self
            .animations
            .get(&image_id)
            .map_or(1, AnimatedImage::frame_count);
        let target = frame_command
            .frame_number
            .and_then(|number| usize::try_from(number.saturating_sub(1)).ok())
            .filter(|index| *index < existing_count)
            .unwrap_or(existing_count);
        let is_new = target == existing_count;

        if is_new {
            if existing_count >= MAX_ANIMATION_FRAMES_PER_IMAGE {
                return Err(ProtocolError::no_space(
                    "animation frame count reached the per-image limit",
                ));
            }
            let total_frames: usize = self
                .animations
                .values()
                .map(|animation| animation.frames.len())
                .sum();
            if total_frames >= MAX_ANIMATION_FRAMES_TOTAL {
                return Err(ProtocolError::no_space(
                    "animation frame count reached the terminal limit",
                ));
            }
            let frame_bytes = checked_pixel_bytes(image_width, image_height, 4)?;
            if self.animation_memory.saturating_add(frame_bytes) > MAX_ANIMATION_MEMORY {
                return Err(ProtocolError::no_space(
                    "animation frame memory limit exceeded",
                ));
            }
        }

        let mut canvas = if is_new {
            if let Some(base) = frame_command.other_frame_number {
                self.frame_pixels(image_id, base.saturating_sub(1) as usize)
                    .ok_or_else(|| ProtocolError::invalid("base animation frame was not found"))?
                    .as_ref()
                    .to_vec()
            } else {
                let color = frame_command.background_color.unwrap_or(0).to_be_bytes();
                let mut canvas = vec![0; checked_pixel_bytes(image_width, image_height, 4)?];
                for pixel in canvas.as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&color);
                }
                canvas
            }
        } else {
            self.frame_pixels(image_id, target)
                .ok_or_else(|| ProtocolError::not_found("animation frame was not found"))?
                .as_ref()
                .to_vec()
        };
        compose_rgba_overlay(
            &mut canvas,
            image_width,
            image_height,
            &decoded.data,
            decoded.width,
            decoded.height,
            frame_command.x,
            frame_command.y,
            frame_command.replace,
        )?;
        let revision = self.allocate_revision();
        let data: Arc<[u8]> = canvas.into();

        if target == 0 {
            if !self.storage.replace_pixels(image_id, data, revision) {
                return Err(ProtocolError::invalid(
                    "root animation frame geometry changed unexpectedly",
                ));
            }
            let animation = self
                .animations
                .entry(image_id)
                .or_insert_with(|| AnimatedImage::new(now));
            if let Some(gap) = frame_command.gap {
                animation.root_gap_ms = gap.max(0) as u32;
            }
        } else if is_new {
            let gap_ms = frame_command
                .gap
                .map_or(DEFAULT_ANIMATION_GAP_MS, |gap| gap.max(0) as u32);
            self.animation_memory = self.animation_memory.saturating_add(data.len());
            self.animations
                .entry(image_id)
                .or_insert_with(|| AnimatedImage::new(now))
                .frames
                .push(AnimationFrame {
                    data,
                    revision,
                    gap_ms,
                });
        } else {
            let animation = self
                .animations
                .get_mut(&image_id)
                .ok_or_else(|| ProtocolError::not_found("animation frame was not found"))?;
            let frame = animation
                .frames
                .get_mut(target - 1)
                .ok_or_else(|| ProtocolError::not_found("animation frame was not found"))?;
            frame.data = data;
            frame.revision = revision;
            if let Some(gap) = frame_command.gap {
                frame.gap_ms = gap.max(0) as u32;
            }
        }
        Ok(u32::try_from(target + 1).unwrap_or(u32::MAX))
    }

    fn frame_pixels(&self, image_id: ImageId, frame: usize) -> Option<Arc<[u8]>> {
        if frame == 0 {
            return self.storage.get(image_id).map(|image| image.data.clone());
        }
        self.animations
            .get(&image_id)?
            .frames
            .get(frame - 1)
            .map(|frame| frame.data.clone())
    }

    fn handle_animation_control(
        &mut self,
        cmd: KittyCommand,
        now: Instant,
    ) -> GraphicsCommandResult {
        let Some(image_id) = self.resolve_image_reference(&cmd) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation image was not found"),
            );
        };
        let animation = self
            .animations
            .entry(image_id)
            .or_insert_with(|| AnimatedImage::new(now));

        if let Some(frame_number) = cmd.frame.other_frame_number {
            let frame = frame_number.saturating_sub(1) as usize;
            if frame < animation.frame_count() && frame != animation.current_frame {
                animation.current_frame = frame;
                animation.shown_since = now;
            }
        }
        if let Some(gap) = cmd.frame.gap {
            let frame = cmd
                .frame
                .frame_number
                .map(|frame| frame.saturating_sub(1) as usize)
                .unwrap_or(animation.current_frame);
            if frame == 0 {
                animation.root_gap_ms = gap.max(0) as u32;
            } else if let Some(frame) = animation.frames.get_mut(frame - 1) {
                frame.gap_ms = gap.max(0) as u32;
            }
        }
        if let Some(loop_count) = cmd.frame.loop_count {
            animation.max_loops = if loop_count == 1 {
                None
            } else {
                Some(loop_count.saturating_sub(1))
            };
        }
        if let Some(state) = cmd.frame.animation_state {
            let was_stopped = animation.playback == AnimationPlayback::Stopped;
            animation.playback = match state {
                1 => AnimationPlayback::Stopped,
                2 => AnimationPlayback::Loading,
                3 => AnimationPlayback::Running,
                _ => unreachable!("the parser validates animation state"),
            };
            animation.completed_loops = 0;
            if was_stopped && animation.playback != AnimationPlayback::Stopped {
                animation.shown_since = now;
            }
        }
        // Kitty animation-control commands acknowledge failures, but do not
        // emit a success response.
        GraphicsCommandResult::default()
    }

    fn handle_frame_composition(&mut self, cmd: KittyCommand) -> GraphicsCommandResult {
        let Some(image_id) = self.resolve_image_reference(&cmd) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation image was not found"),
            );
        };
        let Some(root) = self.storage.get(image_id) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation image was evicted"),
            );
        };
        let (image_width, image_height) = (root.width, root.height);
        let Some(source_number) = cmd.frame.frame_number else {
            return failure(
                &cmd,
                ProtocolError::not_found("source animation frame was not found"),
            );
        };
        let Some(destination_number) = cmd.frame.other_frame_number else {
            return failure(
                &cmd,
                ProtocolError::not_found("destination animation frame was not found"),
            );
        };
        let source_index = source_number.saturating_sub(1) as usize;
        let destination_index = destination_number.saturating_sub(1) as usize;
        let Some(source) = self.frame_pixels(image_id, source_index) else {
            return failure(
                &cmd,
                ProtocolError::not_found("source animation frame was not found"),
            );
        };
        let Some(destination) = self.frame_pixels(image_id, destination_index) else {
            return failure(
                &cmd,
                ProtocolError::not_found("destination animation frame was not found"),
            );
        };
        let width = cmd.frame.width.unwrap_or(image_width);
        let height = cmd.frame.height.unwrap_or(image_height);
        if let Err(error) = validate_frame_rectangle(
            cmd.frame.source_x,
            cmd.frame.source_y,
            width,
            height,
            image_width,
            image_height,
        )
        .and_then(|()| {
            validate_frame_rectangle(
                cmd.frame.x,
                cmd.frame.y,
                width,
                height,
                image_width,
                image_height,
            )
        }) {
            return failure(&cmd, error);
        }
        if source_index == destination_index
            && rectangles_overlap(
                cmd.frame.source_x,
                cmd.frame.source_y,
                cmd.frame.x,
                cmd.frame.y,
                width,
                height,
            )
        {
            return failure(
                &cmd,
                ProtocolError::invalid("same-frame composition rectangles overlap"),
            );
        }

        let mut composed = destination.as_ref().to_vec();
        compose_rgba_rectangle(
            &mut composed,
            image_width,
            &source,
            image_width,
            cmd.frame.source_x,
            cmd.frame.source_y,
            cmd.frame.x,
            cmd.frame.y,
            width,
            height,
            cmd.frame.replace,
        );
        let revision = self.allocate_revision();
        let data: Arc<[u8]> = composed.into();
        if destination_index == 0 {
            if !self.storage.replace_pixels(image_id, data, revision) {
                return failure(
                    &cmd,
                    ProtocolError::invalid("root animation frame geometry changed unexpectedly"),
                );
            }
        } else if let Some(frame) = self
            .animations
            .get_mut(&image_id)
            .and_then(|animation| animation.frames.get_mut(destination_index - 1))
        {
            frame.data = data;
            frame.revision = revision;
        } else {
            return failure(
                &cmd,
                ProtocolError::not_found("destination animation frame was not found"),
            );
        }
        self.storage.touch(image_id);
        GraphicsCommandResult {
            response: response_for_resolved(&cmd, image_id.0, Ok(())),
            cursor_movement: None,
        }
    }

    fn allocate_revision(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        revision
    }

    fn handle_query(&mut self, mut cmd: KittyCommand) -> GraphicsCommandResult {
        if cmd.image_id.filter(|id| *id != 0).is_none() || cmd.image_number.is_some() {
            return failure(
                &cmd,
                ProtocolError::invalid("query requires a nonzero image ID, not an image number"),
            );
        }
        if let Err(error) = prepare_transmission_data(&mut cmd, self.allow_file_transmission) {
            return failure(&cmd, error);
        }
        let pending = PendingImage {
            storage_id: ImageId(0),
            response_id: cmd.image_id.unwrap_or(0),
            response_number: None,
            placement_id: cmd.placement_id,
            transmission: cmd.transmission,
            action: KittyAction::Query,
            placement: cmd.placement,
            data: cmd.data,
            quiet: cmd.quiet,
            alternate_screen: false,
        };
        self.finalize_image(pending, true)
    }

    fn finalize_image(&mut self, pending: PendingImage, query_only: bool) -> GraphicsCommandResult {
        let decoded = match decode_image(&pending) {
            Ok(image) => image,
            Err(error) => return pending_failure(pending, error),
        };
        if query_only {
            return GraphicsCommandResult {
                response: response_fields(
                    pending.response_id,
                    pending.response_number,
                    pending.placement_id,
                    pending.quiet,
                    Ok(()),
                ),
                cursor_movement: None,
            };
        }

        if pending.response_id != 0 {
            self.disassociate_image_number(pending.storage_id);
            self.remove_animation(pending.storage_id);
            self.storage.remove(pending.storage_id);
            let roots: Vec<PlacementUid> = self
                .placements
                .iter()
                .filter(|placement| placement.image_id == pending.storage_id)
                .map(|placement| placement.uid)
                .chain(
                    self.virtual_placements
                        .iter()
                        .filter(|placement| placement.image_id == pending.storage_id)
                        .map(|placement| placement.uid),
                )
                .collect();
            self.remove_placement_groups(roots, false);
        }
        self.protocol_images.remove(&pending.storage_id);
        let stored = StoredImage {
            id: pending.storage_id,
            width: decoded.width,
            height: decoded.height,
            data: decoded.data.into(),
            revision: self.next_revision,
        };
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        self.refresh_placed_image_recency();
        if let Err(message) = self.storage.store(stored) {
            return pending_failure(pending, ProtocolError::too_large(message));
        }
        if let Some(number) = pending.response_number {
            self.associate_image_number(pending.storage_id, number);
        }
        self.remove_evicted_placements();

        let movement = if pending.action == KittyAction::TransmitAndDisplay {
            match self.add_placement(
                pending.storage_id,
                pending.placement_id,
                pending.placement.clone(),
                pending.alternate_screen,
            ) {
                Ok(movement) => Some(movement),
                Err(error) => return pending_failure(pending, error),
            }
        } else {
            None
        };
        let placement_kind = if pending.placement.virtual_placement {
            "Unicode placeholder image"
        } else {
            "image"
        };
        log::debug!(
            "Accepted Kitty {placement_kind}: id={}, {}x{} pixels",
            pending.storage_id.0,
            decoded.width,
            decoded.height
        );
        GraphicsCommandResult {
            response: response_fields(
                pending.response_id,
                pending.response_number,
                pending.placement_id,
                pending.quiet,
                Ok(()),
            ),
            cursor_movement: movement.flatten(),
        }
    }

    fn handle_display(
        &mut self,
        cmd: KittyCommand,
        alternate_screen: bool,
    ) -> GraphicsCommandResult {
        let Some(id) = self.resolve_image_reference(&cmd) else {
            if cmd.image_id.is_some_and(|id| id != 0)
                || cmd.image_number.is_some_and(|number| number != 0)
            {
                return failure(&cmd, ProtocolError::not_found("image not found"));
            }
            return failure(
                &cmd,
                ProtocolError::invalid("display requires an image ID or image number"),
            );
        };
        self.storage.touch(id);
        let movement = match self.add_placement(
            id,
            cmd.placement_id.filter(|placement_id| *placement_id != 0),
            cmd.placement.clone(),
            alternate_screen,
        ) {
            Ok(movement) => movement,
            Err(error) => return failure(&cmd, error),
        };
        GraphicsCommandResult {
            response: response_for_resolved(&cmd, id.0, Ok(())),
            cursor_movement: movement,
        }
    }

    fn handle_delete(
        &mut self,
        cmd: KittyCommand,
        alternate_screen: bool,
    ) -> GraphicsCommandResult {
        let delete_data = cmd.delete.is_ascii_uppercase();
        if cmd.delete.eq_ignore_ascii_case(&'f') {
            return self.handle_frame_delete(cmd, delete_data);
        }
        match cmd.delete.to_ascii_lowercase() {
            'a' => {
                let roots: Vec<PlacementUid> = self
                    .placements
                    .iter()
                    .filter(|placement| placement.alternate_screen == alternate_screen)
                    .map(|placement| placement.uid)
                    .collect();
                self.remove_placement_groups(roots, delete_data);
            }
            'i' => {
                let Some(id) = cmd.image_id.filter(|id| *id != 0).map(ImageId) else {
                    return failure(&cmd, ProtocolError::invalid("d=i requires an image ID"));
                };
                let placement_id = cmd.placement_id.filter(|value| *value != 0);
                let roots: Vec<PlacementUid> = self
                    .placements
                    .iter()
                    .filter(|placement| {
                        placement.image_id == id
                            && placement_id
                                .is_none_or(|value| placement.placement_id == Some(value))
                    })
                    .map(|placement| placement.uid)
                    .chain(
                        self.virtual_placements
                            .iter()
                            .filter(|placement| {
                                placement.image_id == id
                                    && placement_id
                                        .is_none_or(|value| placement.placement_id == Some(value))
                            })
                            .map(|placement| placement.uid),
                    )
                    .collect();
                self.remove_placement_groups(roots, delete_data);
                if delete_data {
                    self.remove_image_if_unplaced(id);
                }
            }
            'n' => {
                let Some(number) = cmd.image_number.filter(|number| *number != 0) else {
                    return failure(
                        &cmd,
                        ProtocolError::invalid("d=n requires an image number in I"),
                    );
                };
                let Some(id) = self.newest_image_with_number(number) else {
                    // Kitty deletion is idempotent when the selected image is
                    // already absent.
                    return GraphicsCommandResult {
                        response: response_for(&cmd, Ok(())),
                        cursor_movement: None,
                    };
                };
                let placement_id = cmd.placement_id.filter(|value| *value != 0);
                let roots: Vec<PlacementUid> = self
                    .placements
                    .iter()
                    .filter(|placement| {
                        placement.image_id == id
                            && placement_id
                                .is_none_or(|value| placement.placement_id == Some(value))
                    })
                    .map(|placement| placement.uid)
                    .chain(
                        self.virtual_placements
                            .iter()
                            .filter(|placement| {
                                placement.image_id == id
                                    && placement_id
                                        .is_none_or(|value| placement.placement_id == Some(value))
                            })
                            .map(|placement| placement.uid),
                    )
                    .collect();
                self.remove_placement_groups(roots, delete_data);
                if delete_data {
                    self.remove_image_if_unplaced(id);
                }
            }
            'p' => {
                let (Some(col), Some(row)) = (cmd.placement.source_x, cmd.placement.source_y)
                else {
                    return failure(&cmd, ProtocolError::invalid("d=p requires x and y"));
                };
                self.delete_matching(delete_data, |placement| {
                    placement_intersects(
                        placement,
                        col.saturating_sub(1),
                        i64::from(row.saturating_sub(1)),
                    )
                });
            }
            'c' => {
                let col = cmd.placement.col.unwrap_or(0);
                let row = cmd.placement.row.unwrap_or(0);
                self.delete_matching(delete_data, |placement| {
                    placement_intersects(placement, col, i64::from(row))
                });
            }
            'q' => {
                let (Some(col), Some(row), Some(z_index)) = (
                    cmd.placement.source_x,
                    cmd.placement.source_y,
                    cmd.placement.z_index,
                ) else {
                    return failure(&cmd, ProtocolError::invalid("d=q requires x, y, and z"));
                };
                self.delete_matching(delete_data, |placement| {
                    placement.z_index == z_index
                        && placement_intersects(
                            placement,
                            col.saturating_sub(1),
                            i64::from(row.saturating_sub(1)),
                        )
                });
            }
            'x' => {
                let Some(col) = cmd.placement.source_x else {
                    return failure(&cmd, ProtocolError::invalid("d=x requires x"));
                };
                let col = col.saturating_sub(1);
                self.delete_matching(delete_data, |placement| {
                    col >= placement.col
                        && col < placement.col.saturating_add(placement.width.unwrap_or(1))
                });
            }
            'y' => {
                let Some(row) = cmd.placement.source_y else {
                    return failure(&cmd, ProtocolError::invalid("d=y requires y"));
                };
                let row = i64::from(row.saturating_sub(1));
                self.delete_matching(delete_data, |placement| {
                    row >= placement.row
                        && row
                            < placement
                                .row
                                .saturating_add(i64::from(placement.height.unwrap_or(1)))
                });
            }
            'z' => {
                let Some(z_index) = cmd.placement.z_index else {
                    return failure(&cmd, ProtocolError::invalid("d=z requires z"));
                };
                self.delete_matching(delete_data, |placement| placement.z_index == z_index);
            }
            'r' => {
                let (Some(first), Some(last)) = (cmd.placement.source_x, cmd.placement.source_y)
                else {
                    return failure(&cmd, ProtocolError::invalid("d=r requires x and y"));
                };
                let virtual_ids: Vec<ImageId> = self
                    .virtual_placements
                    .iter()
                    .filter(|placement| {
                        placement.image_id.0 >= first && placement.image_id.0 <= last
                    })
                    .map(|placement| placement.image_id)
                    .collect();
                let roots: Vec<PlacementUid> = self
                    .placements
                    .iter()
                    .filter(|placement| {
                        placement.image_id.0 >= first && placement.image_id.0 <= last
                    })
                    .map(|placement| placement.uid)
                    .chain(
                        self.virtual_placements
                            .iter()
                            .filter(|placement| {
                                placement.image_id.0 >= first && placement.image_id.0 <= last
                            })
                            .map(|placement| placement.uid),
                    )
                    .collect();
                self.remove_placement_groups(roots, delete_data);
                if delete_data {
                    for id in virtual_ids {
                        self.remove_image_if_unplaced(id);
                    }
                }
            }
            _ => return failure(&cmd, ProtocolError::unsupported("deletion selector")),
        }
        self.abort_pending();
        GraphicsCommandResult {
            response: response_for(&cmd, Ok(())),
            cursor_movement: None,
        }
    }

    fn handle_frame_delete(
        &mut self,
        cmd: KittyCommand,
        delete_image_if_last_frame: bool,
    ) -> GraphicsCommandResult {
        let Some(image_id) = self.resolve_image_reference(&cmd) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation image was not found"),
            );
        };
        let frame_count = self
            .animations
            .get(&image_id)
            .map_or(1, AnimatedImage::frame_count);
        if frame_count == 1 {
            if delete_image_if_last_frame {
                self.remove_image_completely(image_id);
            }
            self.abort_pending();
            return GraphicsCommandResult {
                response: response_for_resolved(&cmd, image_id.0, Ok(())),
                cursor_movement: None,
            };
        }

        let target = cmd
            .frame
            .frame_number
            .unwrap_or(1)
            .clamp(1, frame_count as u32) as usize
            - 1;
        if target == 0 {
            let promoted = self
                .animations
                .get_mut(&image_id)
                .expect("multiple frames require animation metadata")
                .frames
                .remove(0);
            self.animation_memory = self.animation_memory.saturating_sub(promoted.data.len());
            let promoted_gap = promoted.gap_ms;
            if !self
                .storage
                .replace_pixels(image_id, promoted.data, promoted.revision)
            {
                return failure(
                    &cmd,
                    ProtocolError::invalid("root animation frame geometry changed unexpectedly"),
                );
            }
            let animation = self
                .animations
                .get_mut(&image_id)
                .expect("animation metadata remains after promotion");
            animation.root_gap_ms = promoted_gap;
            if animation.current_frame > 0 {
                animation.current_frame -= 1;
            }
        } else {
            let animation = self
                .animations
                .get_mut(&image_id)
                .expect("multiple frames require animation metadata");
            let removed = animation.frames.remove(target - 1);
            self.animation_memory = self.animation_memory.saturating_sub(removed.data.len());
            if animation.current_frame > target {
                animation.current_frame -= 1;
            } else if animation.current_frame == target {
                animation.current_frame = target.min(animation.frame_count() - 1);
            }
        }
        self.abort_pending();
        GraphicsCommandResult {
            response: response_for_resolved(&cmd, image_id.0, Ok(())),
            cursor_movement: None,
        }
    }

    fn add_placement(
        &mut self,
        image_id: ImageId,
        placement_id: Option<u32>,
        placement: KittyPlacement,
        alternate_screen: bool,
    ) -> Result<Option<(u32, u32)>, ProtocolError> {
        if placement.virtual_placement && placement.parent_image_id.is_some() {
            return Err(ProtocolError::invalid(
                "virtual placement cannot also be relative",
            ));
        }
        let placement_id = placement_id.filter(|id| *id != 0);
        let existing_uid = placement_id.and_then(|id| {
            self.placement_uid_by_client_reference(image_id, Some(id), alternate_screen)
        });
        let relative = if let Some(parent_image_id) = placement.parent_image_id {
            let parent = self
                .placement_uid_by_client_reference(
                    ImageId(parent_image_id),
                    placement.parent_placement_id,
                    alternate_screen,
                )
                .ok_or_else(|| {
                    ProtocolError::parent_missing("relative placement parent was not found")
                })?;
            if existing_uid == Some(parent) {
                return Err(ProtocolError::invalid(
                    "relative placement cannot be its own parent",
                ));
            }
            self.validate_relative_ancestry(existing_uid, parent)?;
            Some(RelativePlacement {
                parent,
                horizontal_offset: placement.horizontal_offset,
                vertical_offset: placement.vertical_offset,
            })
        } else {
            None
        };
        let uid = existing_uid.unwrap_or_else(|| self.allocate_placement_uid());

        if placement.virtual_placement {
            self.placements.retain(|existing| existing.uid != uid);
            self.virtual_placements
                .retain(|existing| existing.uid != uid);
            self.push_virtual_placement(VirtualImagePlacement {
                uid,
                image_id,
                placement_id,
                columns: placement.width,
                rows: placement.height,
                alternate_screen,
            });
            return Ok(None);
        }
        self.placements.retain(|existing| existing.uid != uid);
        self.virtual_placements
            .retain(|existing| existing.uid != uid);
        let (image_width, image_height) = self
            .storage
            .get(image_id)
            .map(|image| (image.width, image.height))
            .unwrap_or((0, 0));
        let source_x = placement.source_x.unwrap_or(0).min(image_width);
        let source_y = placement.source_y.unwrap_or(0).min(image_height);
        let source_width = placement
            .source_width
            .unwrap_or(image_width.saturating_sub(source_x))
            .min(image_width.saturating_sub(source_x));
        let source_height = placement
            .source_height
            .unwrap_or(image_height.saturating_sub(source_y))
            .min(image_height.saturating_sub(source_y));
        let display = resolve_kitty_display_geometry(
            placement.width,
            placement.height,
            source_width,
            source_height,
            self.cell_pixel_width,
            self.cell_pixel_height,
        );
        let movement = (relative.is_none() && !placement.do_not_move_cursor)
            .then_some((display.columns, display.rows));
        let mut managed = ManagedImagePlacement {
            uid,
            relative,
            placement: ImagePlacement {
                image_id,
                placement_id,
                col: placement.col.unwrap_or(0),
                row: i64::from(placement.row.unwrap_or(0)),
                alternate_screen,
                source_x,
                source_y,
                source_width: Some(source_width),
                source_height: Some(source_height),
                x_offset: placement.x_offset.unwrap_or(0),
                y_offset: placement.y_offset.unwrap_or(0),
                width: Some(display.columns),
                height: Some(display.rows),
                pixel_width: display.pixel_width,
                pixel_height: display.pixel_height,
                z_index: placement.z_index.unwrap_or(0),
            },
        };
        managed.update_cell_footprint(self.cell_pixel_width, self.cell_pixel_height);
        self.push_placement(managed);
        Ok(movement)
    }

    fn allocate_placement_uid(&mut self) -> PlacementUid {
        let uid = PlacementUid(self.next_placement_uid);
        self.next_placement_uid = self.next_placement_uid.wrapping_add(1).max(1);
        uid
    }

    fn placement_uid_by_client_reference(
        &self,
        image_id: ImageId,
        placement_id: Option<u32>,
        alternate_screen: bool,
    ) -> Option<PlacementUid> {
        let physical = self
            .placements
            .iter()
            .filter(|placement| {
                placement.image_id == image_id
                    && placement.alternate_screen == alternate_screen
                    && placement_id.is_none_or(|id| placement.placement_id == Some(id))
            })
            .map(|placement| placement.uid);
        let virtuals = self
            .virtual_placements
            .iter()
            .filter(|placement| {
                placement.image_id == image_id
                    && placement.alternate_screen == alternate_screen
                    && placement_id.is_none_or(|id| placement.placement_id == Some(id))
            })
            .map(|placement| placement.uid);
        physical.chain(virtuals).min_by_key(|uid| uid.0)
    }

    fn validate_relative_ancestry(
        &self,
        child: Option<PlacementUid>,
        mut ancestor: PlacementUid,
    ) -> Result<(), ProtocolError> {
        for depth in 1..=MAX_RELATIVE_PLACEMENT_DEPTH {
            if child == Some(ancestor) {
                return Err(ProtocolError::cycle(
                    "relative placement would create a cycle",
                ));
            }
            let Some(relative) = self.relative_placement(ancestor) else {
                return Ok(());
            };
            if depth == MAX_RELATIVE_PLACEMENT_DEPTH {
                return Err(ProtocolError::too_deep(
                    "relative placement ancestry exceeds the supported depth",
                ));
            }
            ancestor = relative.parent;
        }
        unreachable!()
    }

    fn relative_placement(&self, uid: PlacementUid) -> Option<RelativePlacement> {
        self.placements
            .iter()
            .find(|placement| placement.uid == uid)
            .and_then(|placement| placement.relative)
    }

    fn remove_placement_groups(
        &mut self,
        roots: impl IntoIterator<Item = PlacementUid>,
        free_root_images: bool,
    ) {
        let roots: HashSet<PlacementUid> = roots.into_iter().collect();
        if roots.is_empty() {
            return;
        }
        let mut removed = roots.clone();
        loop {
            let before = removed.len();
            for placement in &self.placements {
                if placement
                    .relative
                    .is_some_and(|relative| removed.contains(&relative.parent))
                {
                    removed.insert(placement.uid);
                }
            }
            if removed.len() == before {
                break;
            }
        }

        let removed_images: HashSet<ImageId> = self
            .placements
            .iter()
            .filter(|placement| removed.contains(&placement.uid))
            .map(|placement| placement.image_id)
            .chain(
                self.virtual_placements
                    .iter()
                    .filter(|placement| removed.contains(&placement.uid))
                    .map(|placement| placement.image_id),
            )
            .collect();
        let descendant_images: HashSet<ImageId> = self
            .placements
            .iter()
            .filter(|placement| removed.contains(&placement.uid) && !roots.contains(&placement.uid))
            .map(|placement| placement.image_id)
            .collect();
        self.placements
            .retain(|placement| !removed.contains(&placement.uid));
        self.virtual_placements
            .retain(|placement| !removed.contains(&placement.uid));

        let images_to_free = if free_root_images {
            removed_images
        } else {
            descendant_images
        };
        for image_id in images_to_free {
            self.remove_image_if_unplaced(image_id);
        }
    }

    fn placement_uids(&self) -> HashSet<PlacementUid> {
        self.placements
            .iter()
            .map(|placement| placement.uid)
            .chain(
                self.virtual_placements
                    .iter()
                    .map(|placement| placement.uid),
            )
            .collect()
    }

    fn cascade_implicitly_removed(&mut self, before: HashSet<PlacementUid>) {
        let after = self.placement_uids();
        self.remove_placement_groups(before.difference(&after).copied(), false);
    }

    fn push_virtual_placement(&mut self, placement: VirtualImagePlacement) {
        if let Some(placement_id) = placement.placement_id {
            self.virtual_placements.retain(|existing| {
                existing.image_id != placement.image_id
                    || existing.placement_id != Some(placement_id)
            });
        }
        if self.virtual_placements.len() >= MAX_VIRTUAL_IMAGE_PLACEMENTS
            && let Some(uid) = self
                .virtual_placements
                .front()
                .map(|placement| placement.uid)
        {
            self.remove_placement_groups([uid], false);
        }
        self.virtual_placements.push_back(placement);
    }

    /// Keep referenced images newer than retained transmit-only cache entries
    /// before admitting another decoded image. Placements are semantic
    /// references even when currently in scrollback or the inactive screen;
    /// dropping their pixels first would silently destroy terminal content.
    fn refresh_placed_image_recency(&mut self) {
        for placement in &self.placements {
            self.storage.touch(placement.image_id);
        }
        for placement in &self.virtual_placements {
            self.storage.touch(placement.image_id);
        }
    }

    fn push_placement(&mut self, placement: ManagedImagePlacement) {
        let evicted = (self.placements.len() >= MAX_IMAGE_PLACEMENTS)
            .then(|| {
                self.placements
                    .front()
                    .map(|placement| (placement.uid, placement.image_id))
            })
            .flatten();
        if let Some((uid, _)) = evicted {
            self.remove_placement_groups([uid], false);
        }
        self.placements.push_back(placement);
        // Sixel/iTerm2 images are anonymous and cannot be redisplayed by ID.
        // Release pixels if the placement cap removed their final reference.
        if let Some(image_id) = evicted.map(|(_, image_id)| image_id)
            && self.protocol_images.contains(&image_id)
            && !self
                .placements
                .iter()
                .any(|placement| placement.image_id == image_id)
        {
            self.remove_animation(image_id);
            self.storage.remove(image_id);
            self.protocol_images.remove(&image_id);
        }
    }

    fn allocate_anonymous_id(&mut self) -> ImageId {
        loop {
            let id = ImageId(self.next_anonymous_id);
            self.next_anonymous_id = self.next_anonymous_id.saturating_sub(1).max(1);
            if !self.storage.contains(id) {
                return id;
            }
        }
    }

    fn allocate_client_id(&mut self) -> Result<ImageId, ProtocolError> {
        let first = self.next_client_id.max(1);
        let mut candidate = first;
        loop {
            let id = ImageId(candidate);
            self.next_client_id = candidate.wrapping_add(1).max(1);
            if !self.storage.contains(id) {
                return Ok(id);
            }
            candidate = self.next_client_id;
            if candidate == first {
                return Err(ProtocolError::too_large(
                    "no free client image ID is available",
                ));
            }
        }
    }

    fn resolve_image_reference(&self, cmd: &KittyCommand) -> Option<ImageId> {
        if let Some(id) = cmd.image_id.filter(|id| *id != 0).map(ImageId) {
            return self.storage.contains(id).then_some(id);
        }
        cmd.image_number
            .filter(|number| *number != 0)
            .and_then(|number| self.newest_image_with_number(number))
    }

    fn newest_image_with_number(&self, number: u32) -> Option<ImageId> {
        self.image_numbers.get(&number).and_then(|ids| {
            ids.iter()
                .rev()
                .copied()
                .find(|id| self.storage.contains(*id))
        })
    }

    fn associate_image_number(&mut self, id: ImageId, number: u32) {
        self.disassociate_image_number(id);
        self.image_number_by_id.insert(id, number);
        self.image_numbers.entry(number).or_default().push_back(id);
    }

    fn disassociate_image_number(&mut self, id: ImageId) {
        let Some(number) = self.image_number_by_id.remove(&id) else {
            return;
        };
        let mut remove_number = false;
        if let Some(ids) = self.image_numbers.get_mut(&number) {
            ids.retain(|candidate| *candidate != id);
            remove_number = ids.is_empty();
        }
        if remove_number {
            self.image_numbers.remove(&number);
        }
    }

    fn prune_image_numbers(&mut self) {
        let stale: Vec<ImageId> = self
            .image_number_by_id
            .keys()
            .copied()
            .filter(|id| !self.storage.contains(*id))
            .collect();
        for id in stale {
            self.disassociate_image_number(id);
        }
    }

    fn remove_evicted_placements(&mut self) {
        let evicted_animations: Vec<ImageId> = self
            .animations
            .keys()
            .copied()
            .filter(|image_id| !self.storage.contains(*image_id))
            .collect();
        for image_id in evicted_animations {
            self.remove_animation(image_id);
        }
        let roots: Vec<PlacementUid> = self
            .placements
            .iter()
            .filter(|placement| !self.storage.contains(placement.image_id))
            .map(|placement| placement.uid)
            .chain(
                self.virtual_placements
                    .iter()
                    .filter(|placement| !self.storage.contains(placement.image_id))
                    .map(|placement| placement.uid),
            )
            .collect();
        self.remove_placement_groups(roots, false);
        self.protocol_images
            .retain(|image_id| self.storage.contains(*image_id));
        self.prune_image_numbers();
    }

    fn remove_image_if_unplaced(&mut self, id: ImageId) {
        if !self
            .placements
            .iter()
            .any(|placement| placement.image_id == id)
            && !self
                .virtual_placements
                .iter()
                .any(|placement| placement.image_id == id)
        {
            self.remove_animation(id);
            self.storage.remove(id);
            self.protocol_images.remove(&id);
            self.disassociate_image_number(id);
        }
    }

    fn remove_unplaced_protocol_images(&mut self) {
        let unplaced: Vec<ImageId> = self
            .protocol_images
            .iter()
            .copied()
            .filter(|id| {
                !self
                    .placements
                    .iter()
                    .any(|placement| placement.image_id == *id)
            })
            .collect();
        for id in unplaced {
            self.remove_animation(id);
            self.storage.remove(id);
            self.protocol_images.remove(&id);
            self.disassociate_image_number(id);
        }
    }

    fn delete_matching(&mut self, delete_data: bool, predicate: impl Fn(&ImagePlacement) -> bool) {
        let roots: Vec<PlacementUid> = self
            .placements
            .iter()
            .filter(|placement| predicate(placement))
            .map(|placement| placement.uid)
            .collect();
        self.remove_placement_groups(roots, delete_data);
    }

    fn remove_animation(&mut self, image_id: ImageId) {
        if let Some(animation) = self.animations.remove(&image_id) {
            let bytes = animation
                .frames
                .iter()
                .map(|frame| frame.data.len())
                .sum::<usize>();
            self.animation_memory = self.animation_memory.saturating_sub(bytes);
        }
    }

    fn remove_image_completely(&mut self, image_id: ImageId) {
        let roots: Vec<PlacementUid> = self
            .placements
            .iter()
            .filter(|placement| placement.image_id == image_id)
            .map(|placement| placement.uid)
            .chain(
                self.virtual_placements
                    .iter()
                    .filter(|placement| placement.image_id == image_id)
                    .map(|placement| placement.uid),
            )
            .collect();
        self.remove_placement_groups(roots, false);
        self.remove_animation(image_id);
        self.storage.remove(image_id);
        self.protocol_images.remove(&image_id);
        self.disassociate_image_number(image_id);
    }

    pub fn placements(&self, alternate_screen: bool) -> impl Iterator<Item = &ImagePlacement> {
        self.placements
            .iter()
            .filter(move |placement| placement.alternate_screen == alternate_screen)
            .map(|placement| &placement.placement)
    }

    fn visible_animation_ids(&self, alternate_screen: bool) -> HashSet<ImageId> {
        self.placements
            .iter()
            .filter(|placement| placement.alternate_screen == alternate_screen)
            .map(|placement| placement.image_id)
            .chain(
                self.virtual_placements
                    .iter()
                    .filter(|placement| placement.alternate_screen == alternate_screen)
                    .map(|placement| placement.image_id),
            )
            .collect()
    }

    /// Earliest time at which a visible animation on this screen can change.
    pub(crate) fn next_animation_deadline(&self, alternate_screen: bool) -> Option<Instant> {
        self.visible_animation_ids(alternate_screen)
            .into_iter()
            .filter_map(|image_id| self.animations.get(&image_id))
            .filter_map(animation_deadline)
            .min()
    }

    /// Advance visible animations whose frame deadline has elapsed.
    pub(crate) fn advance_animations(&mut self, alternate_screen: bool, now: Instant) -> bool {
        let visible = self.visible_animation_ids(alternate_screen);
        let mut changed = false;
        for image_id in visible {
            let Some(animation) = self.animations.get_mut(&image_id) else {
                continue;
            };
            // A long-suspended window can have many elapsed frame boundaries.
            // Catch up deterministically while keeping one wake bounded.
            for _ in 0..MAX_ANIMATION_FRAMES_TOTAL {
                let Some(deadline) = animation_deadline(animation) else {
                    break;
                };
                if deadline > now {
                    break;
                }
                if !advance_animation_frame(animation, deadline) {
                    break;
                }
                changed = true;
            }
            if animation_deadline(animation).is_some_and(|deadline| deadline <= now) {
                animation.shown_since = now;
            }
        }
        changed
    }

    fn current_image(&self, image_id: ImageId) -> Option<StoredImage> {
        let root = self.storage.get(image_id)?;
        let Some(animation) = self.animations.get(&image_id) else {
            return Some(root.clone());
        };
        if animation.current_frame == 0 {
            return Some(root.clone());
        }
        let frame = animation.frames.get(animation.current_frame - 1)?;
        Some(StoredImage {
            id: root.id,
            width: root.width,
            height: root.height,
            data: frame.data.clone(),
            revision: frame.revision,
        })
    }

    /// Resolve current placements against the displayed grid.
    pub(crate) fn presentation_images(
        &self,
        grid: &Grid,
        alternate_screen: bool,
        cell_width: u32,
        cell_height: u32,
    ) -> Vec<PresentationImage> {
        presentation::PresentationView {
            placements: &self.placements,
            virtual_placements: &self.virtual_placements,
            image: &|id| self.current_image(id),
        }
        .presentation_images(grid, alternate_screen, cell_width, cell_height)
    }

    pub fn get_image(&self, id: ImageId) -> Option<&StoredImage> {
        self.storage.get(id)
    }

    pub(crate) fn clear_screen(&mut self, alternate_screen: bool) {
        let roots: Vec<PlacementUid> = self
            .placements
            .iter()
            .filter(|placement| placement.alternate_screen == alternate_screen)
            .map(|placement| placement.uid)
            .collect();
        self.remove_placement_groups(roots, false);
        self.remove_unplaced_protocol_images();
    }

    /// Clear the complete graphics namespace for one screen buffer. Unlike an
    /// ordinary viewport erase, alternate-screen replacement also discards
    /// invisible Unicode virtual-placement prototypes.
    pub(crate) fn clear_screen_all(&mut self, alternate_screen: bool) {
        let roots: Vec<PlacementUid> = self
            .placements
            .iter()
            .filter(|placement| placement.alternate_screen == alternate_screen)
            .map(|placement| placement.uid)
            .chain(
                self.virtual_placements
                    .iter()
                    .filter(|placement| placement.alternate_screen == alternate_screen)
                    .map(|placement| placement.uid),
            )
            .collect();
        self.remove_placement_groups(roots, false);
        self.remove_unplaced_protocol_images();
    }

    /// Remove the portions of image placements covered by a cell rectangle.
    /// Placements are split into the remaining top/bottom/left/right strips so
    /// a text-area erase does not discard unrelated pixels outside the area.
    pub(crate) fn erase_rect(
        &mut self,
        alternate_screen: bool,
        rows: std::ops::Range<usize>,
        columns: std::ops::Range<usize>,
    ) {
        self.erase_rect_placements(alternate_screen, rows, columns);
        self.remove_unplaced_protocol_images();
    }

    fn erase_rect_placements(
        &mut self,
        alternate_screen: bool,
        rows: std::ops::Range<usize>,
        columns: std::ops::Range<usize>,
    ) {
        let placement_uids_before = self.placement_uids();
        let erase = PlacementRect {
            left: u32::try_from(columns.start).unwrap_or(u32::MAX),
            right: u32::try_from(columns.end).unwrap_or(u32::MAX),
            top: i64::try_from(rows.start).unwrap_or(i64::MAX),
            bottom: i64::try_from(rows.end).unwrap_or(i64::MAX),
        };
        if erase.left >= erase.right || erase.top >= erase.bottom {
            return;
        }

        let placements = std::mem::take(&mut self.placements);
        let mut rebuilt = VecDeque::new();
        for placement in placements {
            if placement.relative.is_some() {
                rebuilt.push_back(placement);
                continue;
            }
            let bounds = placement_rect(&placement);
            let Some(intersection) = bounds.intersection(erase) else {
                rebuilt.push_back(placement);
                continue;
            };
            if placement.alternate_screen != alternate_screen {
                rebuilt.push_back(placement);
                continue;
            }

            let fragments = [
                PlacementRect {
                    left: bounds.left,
                    right: bounds.right,
                    top: bounds.top,
                    bottom: intersection.top,
                },
                PlacementRect {
                    left: bounds.left,
                    right: bounds.right,
                    top: intersection.bottom,
                    bottom: bounds.bottom,
                },
                PlacementRect {
                    left: bounds.left,
                    right: intersection.left,
                    top: intersection.top,
                    bottom: intersection.bottom,
                },
                PlacementRect {
                    left: intersection.right,
                    right: bounds.right,
                    top: intersection.top,
                    bottom: intersection.bottom,
                },
            ];
            let mut retained_original = false;
            for fragment in fragments {
                if let Some(fragment) = crop_placement(
                    &placement,
                    fragment,
                    self.cell_pixel_width,
                    self.cell_pixel_height,
                ) {
                    let preserve_identity = !retained_original;
                    let uid = if preserve_identity {
                        retained_original = true;
                        placement.uid
                    } else {
                        self.allocate_placement_uid()
                    };
                    rebuilt.push_back(ManagedImagePlacement {
                        uid,
                        relative: preserve_identity.then_some(placement.relative).flatten(),
                        placement: fragment,
                    });
                }
            }
        }
        while rebuilt.len() > MAX_IMAGE_PLACEMENTS {
            rebuilt.pop_front();
        }
        self.placements = rebuilt;
        self.cascade_implicitly_removed(placement_uids_before);
    }

    /// Copy image-cell coverage along with DECCRA text cells. Absence of image
    /// data in the source clears corresponding destination image coverage.
    pub(crate) fn copy_rect(
        &mut self,
        alternate_screen: bool,
        source_rows: std::ops::Range<usize>,
        source_columns: std::ops::Range<usize>,
        destination_top: usize,
        destination_left: usize,
    ) {
        let placement_uids_before = self.placement_uids();
        let source = PlacementRect {
            left: u32::try_from(source_columns.start).unwrap_or(u32::MAX),
            right: u32::try_from(source_columns.end).unwrap_or(u32::MAX),
            top: i64::try_from(source_rows.start).unwrap_or(i64::MAX),
            bottom: i64::try_from(source_rows.end).unwrap_or(i64::MAX),
        };
        if source.left >= source.right || source.top >= source.bottom {
            return;
        }
        let destination = PlacementRect {
            left: u32::try_from(destination_left).unwrap_or(u32::MAX),
            right: u32::try_from(destination_left)
                .unwrap_or(u32::MAX)
                .saturating_add(source.right - source.left),
            top: i64::try_from(destination_top).unwrap_or(i64::MAX),
            bottom: i64::try_from(destination_top)
                .unwrap_or(i64::MAX)
                .saturating_add(source.bottom - source.top),
        };
        if source == destination {
            return;
        }

        let copied: Vec<ImagePlacement> = self
            .placements
            .iter()
            .filter(|placement| placement.alternate_screen == alternate_screen)
            .filter(|placement| placement.relative.is_none())
            .filter_map(|placement| {
                let intersection = placement_rect(placement).intersection(source)?;
                let mut copy = crop_placement(
                    placement,
                    intersection,
                    self.cell_pixel_width,
                    self.cell_pixel_height,
                )?;
                copy.col = destination
                    .left
                    .saturating_add(intersection.left - source.left);
                copy.row = destination
                    .top
                    .saturating_add(intersection.top - source.top);
                Some(copy)
            })
            .collect();

        self.erase_rect_placements(
            alternate_screen,
            destination.top as usize..destination.bottom as usize,
            destination.left as usize..destination.right as usize,
        );
        for placement in copied {
            let uid = self.allocate_placement_uid();
            self.placements.push_back(ManagedImagePlacement {
                uid,
                placement,
                relative: None,
            });
        }
        while self.placements.len() > MAX_IMAGE_PLACEMENTS {
            self.placements.pop_front();
        }
        self.cascade_implicitly_removed(placement_uids_before);
        self.remove_unplaced_protocol_images();
    }

    pub(crate) fn abort_pending(&mut self) {
        self.pending = None;
        self.pending_frame = None;
    }

    pub fn scroll_up(
        &mut self,
        alternate_screen: bool,
        rows: std::ops::Range<usize>,
        columns: std::ops::Range<usize>,
        count: usize,
        preserve_history: bool,
        history_lines: usize,
    ) {
        if preserve_history {
            self.scroll_history_up(alternate_screen, count, history_lines, None);
            return;
        }
        let placement_uids_before = self.placement_uids();
        let count = count as i64;
        let top = rows.start as i64;
        let bottom = rows.end as i64;
        let left = columns.start as u32;
        let right = u32::try_from(columns.end).unwrap_or(u32::MAX);
        self.placements.retain_mut(|placement| {
            if placement.relative.is_some() {
                return true;
            }
            let rows = placement.height.unwrap_or(1);
            let placement_bottom = placement.row.saturating_add(i64::from(rows));
            let placement_right = placement.col.saturating_add(placement.width.unwrap_or(1));
            if placement.alternate_screen != alternate_screen
                || placement.row < top
                || placement_bottom > bottom
                || placement.col < left
                || placement_right > right
            {
                return true;
            }
            if placement.row >= top + count {
                placement.row -= count;
                true
            } else {
                let clipped_rows = u32::try_from(top + count - placement.row).unwrap_or(u32::MAX);
                if clipped_rows >= rows {
                    return false;
                }
                let mut retained = placement_rect(placement);
                retained.top = top + count;
                let Some(mut cropped) = crop_placement(
                    placement,
                    retained,
                    self.cell_pixel_width,
                    self.cell_pixel_height,
                ) else {
                    return false;
                };
                cropped.row = top;
                placement.placement = cropped;
                true
            }
        });
        self.cascade_implicitly_removed(placement_uids_before);
        self.remove_unplaced_protocol_images();
    }

    /// Archive full-width primary rows above a fixed footer. The moving area
    /// includes retained history, so crossing row zero must not crop an image.
    /// As with other margin scrolling, placements crossing the fixed boundary
    /// stay stationary; relative children continue to resolve from their parent.
    pub(crate) fn scroll_archival_region_up(
        &mut self,
        bottom: usize,
        count: usize,
        history_lines: usize,
    ) {
        self.scroll_history_up(false, count, history_lines, Some(bottom));
    }

    /// Retire ordinary placements wholly outside retained history without
    /// moving surviving coordinates. Parent removal cascades to relatives;
    /// anonymous protocol pixels are freed when their last placement is gone.
    /// Reusable Kitty data and virtual prototypes keep their normal cache life.
    pub(crate) fn prune_scrollback(&mut self, alternate_screen: bool, history_lines: usize) {
        self.scroll_history_up(alternate_screen, 0, history_lines, None);
    }

    /// Follow actual row archival on a same-column resize, not the window's
    /// height delta (blank-tail trimming and cursor pinning can differ).
    /// Relative children resolve from their translated parent; pixel/source
    /// geometry and below-viewport clipping remain unchanged.
    pub(crate) fn adjust_for_height_resize(
        &mut self,
        alternate_screen: bool,
        archived_rows: usize,
        history_lines: usize,
    ) {
        self.scroll_history_up(alternate_screen, archived_rows, history_lines, None);
    }

    fn scroll_history_up(
        &mut self,
        alternate_screen: bool,
        count: usize,
        history_lines: usize,
        fixed_boundary: Option<usize>,
    ) {
        let placement_uids_before = self.placement_uids();
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        let history_top = -i64::try_from(history_lines).unwrap_or(i64::MAX);
        self.placements.retain_mut(|placement| {
            if placement.alternate_screen != alternate_screen || placement.relative.is_some() {
                return true;
            }
            let height = i64::from(placement.height.unwrap_or(1));
            if fixed_boundary.is_some_and(|bottom| {
                placement.row.saturating_add(height) > i64::try_from(bottom).unwrap_or(i64::MAX)
            }) {
                return true;
            }
            placement.row = placement.row.saturating_sub(count);
            placement.row.saturating_add(height) > history_top
        });
        self.cascade_implicitly_removed(placement_uids_before);
        self.remove_unplaced_protocol_images();
    }

    pub fn scroll_down(
        &mut self,
        alternate_screen: bool,
        rows: std::ops::Range<usize>,
        columns: std::ops::Range<usize>,
        count: usize,
    ) {
        let placement_uids_before = self.placement_uids();
        let count = count as i64;
        let top = rows.start as i64;
        let bottom = rows.end as i64;
        let left = columns.start as u32;
        let right = u32::try_from(columns.end).unwrap_or(u32::MAX);
        self.placements.retain_mut(|placement| {
            if placement.relative.is_some() {
                return true;
            }
            let rows = placement.height.unwrap_or(1);
            let placement_bottom = placement.row.saturating_add(i64::from(rows));
            let placement_right = placement.col.saturating_add(placement.width.unwrap_or(1));
            if placement.alternate_screen != alternate_screen
                || placement.row < top
                || placement_bottom > bottom
                || placement.col < left
                || placement_right > right
            {
                return true;
            }
            let row = placement.row.saturating_add(count);
            if row >= bottom {
                false
            } else {
                placement.row = row;
                let visible_rows = u32::try_from(bottom - row).unwrap_or(u32::MAX).min(rows);
                if visible_rows < rows {
                    let mut retained = placement_rect(placement);
                    retained.bottom = bottom;
                    let Some(cropped) = crop_placement(
                        placement,
                        retained,
                        self.cell_pixel_width,
                        self.cell_pixel_height,
                    ) else {
                        return false;
                    };
                    placement.placement = cropped;
                }
                true
            }
        });
        self.cascade_implicitly_removed(placement_uids_before);
        self.remove_unplaced_protocol_images();
    }

    pub fn has_images(&self, alternate_screen: bool) -> bool {
        self.placements(alternate_screen).next().is_some()
            || self
                .virtual_placements
                .iter()
                .any(|placement| placement.alternate_screen == alternate_screen)
    }

    /// Store one decoded RGBA image from any protocol in the shared bounded
    /// cache and placement representation consumed by the renderer.
    pub(crate) fn store_protocol_image(
        &mut self,
        decoded: DecodedImage,
        placement: ProtocolPlacement,
    ) -> Result<ImageId, ProtocolError> {
        let image_id = self.allocate_anonymous_id();
        let width = decoded.width;
        let height = decoded.height;
        self.refresh_placed_image_recency();
        self.storage
            .store(StoredImage {
                id: image_id,
                width,
                height,
                data: decoded.data.into(),
                revision: self.next_revision,
            })
            .map_err(ProtocolError::too_large)?;
        self.protocol_images.insert(image_id);
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
        self.remove_evicted_placements();
        let uid = self.allocate_placement_uid();
        self.push_placement(ManagedImagePlacement {
            uid,
            relative: None,
            placement: ImagePlacement {
                image_id,
                placement_id: None,
                col: placement.col,
                row: placement.row,
                alternate_screen: placement.alternate_screen,
                source_x: 0,
                source_y: 0,
                source_width: Some(width),
                source_height: Some(height),
                x_offset: 0,
                y_offset: 0,
                width: Some(placement.columns.max(1)),
                height: Some(placement.rows.max(1)),
                pixel_width: Some(placement.pixel_width.max(1)),
                pixel_height: Some(placement.pixel_height.max(1)),
                z_index: 0,
            },
        });
        Ok(image_id)
    }

    /// Decoded image bytes currently retained by all graphics protocols.
    pub fn memory_usage(&self) -> usize {
        self.storage
            .memory_usage()
            .saturating_add(self.animation_memory)
    }

    /// Number of decoded images currently retained by all protocols.
    pub(crate) fn image_count(&self) -> usize {
        self.storage.len()
    }

    /// Number of live placements across the primary and alternate screens.
    pub(crate) fn placement_count(&self) -> usize {
        self.placements.len() + self.virtual_placements.len()
    }

    /// Recompute the grid footprint of pixel-addressed placements after the
    /// drawable cell size changes. Their pixel extent remains exact.
    pub(crate) fn set_cell_pixel_geometry(&mut self, cell_width: u32, cell_height: u32) {
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        self.cell_pixel_width = cell_width;
        self.cell_pixel_height = cell_height;
        for placement in &mut self.placements {
            placement.update_cell_footprint(cell_width, cell_height);
        }
    }

    pub(crate) fn handle_sixel(
        &mut self,
        payload: &[u8],
        col: u32,
        row: i64,
        cell_pixels: (u32, u32),
        alternate_screen: bool,
    ) -> Result<(u32, u32), ProtocolError> {
        let decoded = SixelCommand::parse(payload)?.decode()?;
        let columns = decoded.display_width.div_ceil(cell_pixels.0.max(1)).max(1);
        let rows = decoded.display_height.div_ceil(cell_pixels.1.max(1)).max(1);
        let placement = ProtocolPlacement {
            col,
            row,
            columns,
            rows,
            pixel_width: decoded.display_width,
            pixel_height: decoded.display_height,
            alternate_screen,
        };
        self.store_protocol_image(decoded.image, placement)?;
        Ok((0, rows))
    }

    pub(crate) fn handle_iterm2(
        &mut self,
        params: &[&[u8]],
        cursor: (u32, i64),
        terminal_cells: (u32, u32),
        cell_pixels: (u32, u32),
        alternate_screen: bool,
    ) -> Result<Option<(u32, u32)>, ProtocolError> {
        let command = Iterm2Command::parse(params)?;
        if !command.inline {
            return Err(ProtocolError::unsupported("file downloads are disabled"));
        }
        let decoded = command.decode()?;
        let (pixel_width, pixel_height) =
            command.display_size(decoded.width, decoded.height, terminal_cells, cell_pixels)?;
        let columns = pixel_width.div_ceil(cell_pixels.0.max(1)).max(1);
        let rows = pixel_height.div_ceil(cell_pixels.1.max(1)).max(1);
        self.store_protocol_image(
            decoded,
            ProtocolPlacement {
                col: cursor.0,
                row: cursor.1,
                columns,
                rows,
                pixel_width,
                pixel_height,
                alternate_screen,
            },
        )?;
        Ok(Some((0, rows)))
    }
}

impl Default for GraphicsManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
