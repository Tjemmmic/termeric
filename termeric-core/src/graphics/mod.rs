//! Bounded static-image adapters for Kitty, Sixel, and iTerm2 protocols.

mod image;
mod iterm2;
mod kitty;
mod placeholder;
mod sixel;

pub use image::{ImageId, ImagePlacement, ImageStorage, PresentationImage, StoredImage};
pub(crate) use iterm2::Iterm2Command;
pub use kitty::{
    KittyAction, KittyCommand, KittyCompression, KittyFormat, KittyFrame, KittyMedium,
    KittyParseError, KittyPlacement, KittyTransmission,
};
pub use placeholder::IMAGE_PLACEHOLDER;
pub(crate) use sixel::SixelCommand;

use crate::grid::Grid;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use placeholder::PlaceholderDecoder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::io::{Seek, SeekFrom};
use std::ops::{Deref, DerefMut};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(target_vendor = "apple")]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::io::FromRawFd;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_IMAGES: usize = 256;
/// A placement is roughly one renderer instance plus terminal lifecycle
/// metadata. Bound it independently from decoded images because one reusable
/// image may otherwise create an unlimited number of placements.
pub const MAX_IMAGE_PLACEMENTS: usize = 1_024;
/// Virtual placements are inert prototypes rather than renderer instances,
/// but they remain independently bounded because clients may create them
/// without adding any visible cells.
pub const MAX_VIRTUAL_IMAGE_PLACEMENTS: usize = 1_024;
/// Relative placements may be chained, but resolution work must remain
/// bounded. The protocol requires support for at least eight ancestors.
pub const MAX_RELATIVE_PLACEMENT_DEPTH: usize = 32;
pub const MAX_ANIMATION_FRAMES_PER_IMAGE: usize = 1_024;
pub const MAX_ANIMATION_FRAMES_TOTAL: usize = 4_096;
pub const MAX_ANIMATION_MEMORY: usize = 5 * MAX_IMAGE_MEMORY;
const DEFAULT_ANIMATION_GAP_MS: u32 = 40;
const GRAPHICS_RELAY_VERSION: u32 = 1;
pub const MAX_GRAPHICS_RELAY_METADATA_BYTES: usize = 4 * 1024 * 1024;
const RELAY_PIXEL_CHUNK_BYTES: usize = 3 * 1024;
const RELAY_METADATA_CHUNK_BYTES: usize = 24 * 1024;
pub(crate) const MAX_IMAGE_MEMORY: usize = 64 * 1024 * 1024;
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
pub struct GraphicsManager {
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

/// Cheap, immutable capture of the complete decoded graphics namespace used
/// by a live-domain terminal. Pixel buffers remain reference counted while a
/// relay emits bounded protocol chunks outside the daemon's state lock.
#[derive(Debug, Clone)]
pub struct GraphicsRelaySnapshot {
    images: Vec<RelayImage>,
    metadata: RelayMetadata,
}

#[derive(Debug, Clone)]
struct RelayImage {
    id: ImageId,
    width: u32,
    height: u32,
    data: Arc<[u8]>,
    frames: Vec<AnimationFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayMetadata {
    version: u32,
    images: Vec<RelayImageMetadata>,
    placements: Vec<RelayPlacement>,
    virtual_placements: Vec<RelayVirtualPlacement>,
    image_numbers: Vec<RelayImageNumber>,
    protocol_images: Vec<u32>,
    next_anonymous_id: u32,
    next_client_id: u32,
    next_revision: u64,
    next_placement_uid: u64,
    animations: Vec<RelayAnimation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayImageMetadata {
    id: u32,
    width: u32,
    height: u32,
    revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayPlacement {
    uid: u64,
    image_id: u32,
    placement_id: Option<u32>,
    col: u32,
    row: i64,
    alternate_screen: bool,
    source_x: u32,
    source_y: u32,
    source_width: Option<u32>,
    source_height: Option<u32>,
    x_offset: u32,
    y_offset: u32,
    width: Option<u32>,
    height: Option<u32>,
    pixel_width: Option<u32>,
    pixel_height: Option<u32>,
    z_index: i32,
    relative: Option<RelayRelativePlacement>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct RelayRelativePlacement {
    parent: u64,
    horizontal_offset: i32,
    vertical_offset: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayVirtualPlacement {
    uid: u64,
    image_id: u32,
    placement_id: Option<u32>,
    columns: Option<u32>,
    rows: Option<u32>,
    alternate_screen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayImageNumber {
    number: u32,
    image_ids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayAnimation {
    image_id: u32,
    root_gap_ms: u32,
    frame_revisions: Vec<u64>,
    frame_gaps_ms: Vec<u32>,
    current_frame: usize,
    playback: u8,
    max_loops: Option<u32>,
    completed_loops: u32,
    shown_elapsed_ms: u64,
}

impl From<&ManagedImagePlacement> for RelayPlacement {
    fn from(managed: &ManagedImagePlacement) -> Self {
        let placement = &managed.placement;
        Self {
            uid: managed.uid.0,
            image_id: placement.image_id.0,
            placement_id: placement.placement_id,
            col: placement.col,
            row: placement.row,
            alternate_screen: placement.alternate_screen,
            source_x: placement.source_x,
            source_y: placement.source_y,
            source_width: placement.source_width,
            source_height: placement.source_height,
            x_offset: placement.x_offset,
            y_offset: placement.y_offset,
            width: placement.width,
            height: placement.height,
            pixel_width: placement.pixel_width,
            pixel_height: placement.pixel_height,
            z_index: placement.z_index,
            relative: managed.relative.map(|relative| RelayRelativePlacement {
                parent: relative.parent.0,
                horizontal_offset: relative.horizontal_offset,
                vertical_offset: relative.vertical_offset,
            }),
        }
    }
}

impl From<&VirtualImagePlacement> for RelayVirtualPlacement {
    fn from(placement: &VirtualImagePlacement) -> Self {
        Self {
            uid: placement.uid.0,
            image_id: placement.image_id.0,
            placement_id: placement.placement_id,
            columns: placement.columns,
            rows: placement.rows,
            alternate_screen: placement.alternate_screen,
        }
    }
}

impl AnimatedImage {
    fn new(now: Instant) -> Self {
        Self {
            frames: Vec::new(),
            root_gap_ms: 0,
            current_frame: 0,
            playback: AnimationPlayback::Stopped,
            max_loops: None,
            completed_loops: 0,
            shown_since: now,
        }
    }

    fn frame_count(&self) -> usize {
        self.frames.len() + 1
    }

    fn gap(&self, frame: usize) -> Option<u32> {
        if frame == 0 {
            Some(self.root_gap_ms)
        } else {
            self.frames.get(frame - 1).map(|frame| frame.gap_ms)
        }
    }

    fn has_timed_frame(&self) -> bool {
        self.root_gap_ms > 0 || self.frames.iter().any(|frame| frame.gap_ms > 0)
    }
}

impl GraphicsRelaySnapshot {
    /// Emit a replay stream as small, independently framed APC strings. The
    /// callback can write each string directly to a bounded domain frame and
    /// return false to stop after an I/O failure.
    pub fn for_each_escape_chunk(
        &self,
        mut emit: impl FnMut(String) -> bool,
    ) -> Result<(), String> {
        let metadata = serde_json::to_vec(&self.metadata)
            .map_err(|error| format!("serialize graphics relay metadata: {error}"))?;
        if metadata.len() > MAX_GRAPHICS_RELAY_METADATA_BYTES {
            return Err("graphics relay metadata exceeds its resource limit".into());
        }
        let checksum = relay_checksum(&metadata);
        if !emit("\x1b[?2026h".into()) {
            return Ok(());
        }
        if !emit(format!(
            "\x1b_TG;begin;{};{checksum:016x}\x1b\\",
            metadata.len()
        )) {
            return Ok(());
        }

        for image in &self.images {
            if !emit_relay_pixels(
                image.id,
                image.width,
                image.height,
                None,
                0,
                &image.data,
                &mut emit,
            ) {
                return Ok(());
            }
            for (index, frame) in image.frames.iter().enumerate() {
                let frame_number = u32::try_from(index + 2).unwrap_or(u32::MAX);
                if !emit_relay_pixels(
                    image.id,
                    image.width,
                    image.height,
                    Some(frame_number),
                    frame.gap_ms,
                    &frame.data,
                    &mut emit,
                ) {
                    return Ok(());
                }
            }
        }

        for chunk in metadata.chunks(RELAY_METADATA_CHUNK_BYTES) {
            if !emit(format!("\x1b_TG;chunk;{}\x1b\\", STANDARD.encode(chunk))) {
                return Ok(());
            }
        }
        if !emit("\x1b_TG;end\x1b\\".into()) {
            return Ok(());
        }
        emit("\x1b[?2026l".into());
        Ok(())
    }
}

fn emit_relay_pixels(
    image_id: ImageId,
    width: u32,
    height: u32,
    frame_number: Option<u32>,
    gap_ms: u32,
    data: &[u8],
    emit: &mut impl FnMut(String) -> bool,
) -> bool {
    let chunks = data.chunks(RELAY_PIXEL_CHUNK_BYTES);
    let chunk_count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let more = usize::from(index + 1 < chunk_count);
        let controls = if index == 0 {
            if let Some(frame_number) = frame_number {
                format!(
                    "a=f,f=32,s={width},v={height},i={},r={frame_number},z={gap_ms},X=1,q=2,m={more}",
                    image_id.0
                )
            } else {
                format!(
                    "a=t,f=32,s={width},v={height},i={},q=2,m={more}",
                    image_id.0
                )
            }
        } else {
            let action = if frame_number.is_some() { 'f' } else { 't' };
            format!("a={action},i={},q=2,m={more}", image_id.0)
        };
        if !emit(format!("\x1b_G{controls};{}\x1b\\", STANDARD.encode(chunk))) {
            return false;
        }
    }
    true
}

fn relay_checksum(bytes: &[u8]) -> u64 {
    // FNV-1a is used as an inexpensive corruption/truncation check. The
    // authenticated Unix/SSH transport provides trust; this is not a MAC.
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn validate_relay_relative_placements(
    placements: &[RelayPlacement],
    all_uids: &HashSet<u64>,
) -> Result<(), String> {
    let parents: HashMap<_, _> = placements
        .iter()
        .filter_map(|placement| {
            placement
                .relative
                .map(|relative| (placement.uid, relative.parent))
        })
        .collect();
    if parents
        .iter()
        .any(|(uid, parent)| uid == parent || !all_uids.contains(parent))
    {
        return Err("graphics relay metadata contains an invalid relative parent".into());
    }
    for placement in placements {
        let mut uid = placement.uid;
        let mut visited = HashSet::new();
        for depth in 0..=MAX_RELATIVE_PLACEMENT_DEPTH {
            if !visited.insert(uid) {
                return Err("graphics relay metadata contains a relative placement cycle".into());
            }
            let Some(parent) = parents.get(&uid).copied() else {
                break;
            };
            if depth == MAX_RELATIVE_PLACEMENT_DEPTH {
                return Err("graphics relay relative placement chain is too deep".into());
            }
            uid = parent;
        }
    }
    Ok(())
}

fn relay_managed_placement(placement: &RelayPlacement) -> ManagedImagePlacement {
    ManagedImagePlacement {
        uid: PlacementUid(placement.uid),
        placement: ImagePlacement {
            image_id: ImageId(placement.image_id),
            placement_id: placement.placement_id,
            col: placement.col,
            row: placement.row,
            alternate_screen: placement.alternate_screen,
            source_x: placement.source_x,
            source_y: placement.source_y,
            source_width: placement.source_width,
            source_height: placement.source_height,
            x_offset: placement.x_offset,
            y_offset: placement.y_offset,
            width: placement.width,
            height: placement.height,
            pixel_width: placement.pixel_width,
            pixel_height: placement.pixel_height,
            z_index: placement.z_index,
        },
        relative: placement.relative.map(|relative| RelativePlacement {
            parent: PlacementUid(relative.parent),
            horizontal_offset: relative.horizontal_offset,
            vertical_offset: relative.vertical_offset,
        }),
    }
}

/// Non-PTY side effects of a successfully handled graphics command.
#[derive(Debug, Clone, Default)]
pub struct GraphicsCommandResult {
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
        }
    }

    pub fn handle_command(
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
    pub fn set_file_transmission_allowed(&mut self, allowed: bool) {
        self.allow_file_transmission = allowed;
    }

    pub fn file_transmission_allowed(&self) -> bool {
        self.allow_file_transmission
    }

    pub(crate) fn relay_snapshot(&self, now: Instant) -> GraphicsRelaySnapshot {
        let images = self
            .storage
            .iter_lru()
            .map(|image| RelayImage {
                id: image.id,
                width: image.width,
                height: image.height,
                data: Arc::clone(&image.data),
                frames: self
                    .animations
                    .get(&image.id)
                    .map_or_else(Vec::new, |animation| animation.frames.clone()),
            })
            .collect();
        let image_metadata = self
            .storage
            .iter_lru()
            .map(|image| RelayImageMetadata {
                id: image.id.0,
                width: image.width,
                height: image.height,
                revision: image.revision,
            })
            .collect();
        let placements = self.placements.iter().map(RelayPlacement::from).collect();
        let virtual_placements = self
            .virtual_placements
            .iter()
            .map(RelayVirtualPlacement::from)
            .collect();
        let mut image_numbers: Vec<_> = self
            .image_numbers
            .iter()
            .map(|(number, ids)| RelayImageNumber {
                number: *number,
                image_ids: ids.iter().map(|id| id.0).collect(),
            })
            .collect();
        image_numbers.sort_by_key(|entry| entry.number);
        let mut protocol_images: Vec<_> = self.protocol_images.iter().map(|id| id.0).collect();
        protocol_images.sort_unstable();
        let mut animations: Vec<_> = self
            .animations
            .iter()
            .map(|(image_id, animation)| RelayAnimation {
                image_id: image_id.0,
                root_gap_ms: animation.root_gap_ms,
                frame_revisions: animation
                    .frames
                    .iter()
                    .map(|frame| frame.revision)
                    .collect(),
                frame_gaps_ms: animation.frames.iter().map(|frame| frame.gap_ms).collect(),
                current_frame: animation.current_frame,
                playback: match animation.playback {
                    AnimationPlayback::Stopped => 1,
                    AnimationPlayback::Loading => 2,
                    AnimationPlayback::Running => 3,
                },
                max_loops: animation.max_loops,
                completed_loops: animation.completed_loops,
                shown_elapsed_ms: u64::try_from(
                    now.saturating_duration_since(animation.shown_since)
                        .as_millis(),
                )
                .unwrap_or(u64::MAX),
            })
            .collect();
        animations.sort_by_key(|animation| animation.image_id);
        GraphicsRelaySnapshot {
            images,
            metadata: RelayMetadata {
                version: GRAPHICS_RELAY_VERSION,
                images: image_metadata,
                placements,
                virtual_placements,
                image_numbers,
                protocol_images,
                next_anonymous_id: self.next_anonymous_id,
                next_client_id: self.next_client_id,
                next_revision: self.next_revision,
                next_placement_uid: self.next_placement_uid,
                animations,
            },
        }
    }

    pub(crate) fn reset_for_relay_snapshot(&mut self) {
        let allow_file_transmission = self.allow_file_transmission;
        *self = Self::new();
        self.allow_file_transmission = allow_file_transmission;
    }

    pub(crate) fn apply_relay_metadata(
        &mut self,
        bytes: &[u8],
        now: Instant,
    ) -> Result<(), String> {
        if bytes.len() > MAX_GRAPHICS_RELAY_METADATA_BYTES {
            return Err("graphics relay metadata exceeds its resource limit".into());
        }
        let metadata: RelayMetadata = serde_json::from_slice(bytes)
            .map_err(|error| format!("decode graphics relay metadata: {error}"))?;
        if metadata.version != GRAPHICS_RELAY_VERSION {
            return Err(format!(
                "unsupported graphics relay metadata version {}",
                metadata.version
            ));
        }
        if metadata.images.len() != self.storage.len()
            || metadata.placements.len() > MAX_IMAGE_PLACEMENTS
            || metadata.virtual_placements.len() > MAX_VIRTUAL_IMAGE_PLACEMENTS
            || metadata.animations.len() > MAX_IMAGES
        {
            return Err("graphics relay metadata exceeds namespace limits".into());
        }

        let image_ids: HashSet<_> = self.storage.iter().map(|image| image.id).collect();
        let mut revisions = HashMap::new();
        let mut storage_order = Vec::with_capacity(metadata.images.len());
        for image in &metadata.images {
            let image_id = ImageId(image.id);
            let Some(decoded) = self.storage.get(image_id) else {
                return Err("graphics relay metadata references a missing image".into());
            };
            if decoded.width != image.width || decoded.height != image.height {
                return Err("graphics relay image geometry does not match decoded pixels".into());
            }
            if revisions.insert(image_id, image.revision).is_some() {
                return Err("graphics relay metadata repeats an image".into());
            }
            storage_order.push(image_id);
        }

        let mut animation_ids = HashSet::new();
        for animation in &metadata.animations {
            let image_id = ImageId(animation.image_id);
            if !image_ids.contains(&image_id) || !animation_ids.insert(image_id) {
                return Err("graphics relay metadata contains an invalid animation image".into());
            }
            let decoded_frames = self
                .animations
                .get(&image_id)
                .map_or(0, |decoded| decoded.frames.len());
            if decoded_frames != animation.frame_revisions.len()
                || decoded_frames != animation.frame_gaps_ms.len()
                || animation.current_frame > decoded_frames
                || !(1..=3).contains(&animation.playback)
            {
                return Err(
                    "graphics relay animation metadata does not match decoded frames".into(),
                );
            }
        }
        if self.animations.iter().any(|(image_id, animation)| {
            !animation.frames.is_empty() && !animation_ids.contains(image_id)
        }) {
            return Err("graphics relay metadata omits decoded animation frames".into());
        }

        let mut placement_uids = HashSet::new();
        for placement in &metadata.placements {
            if placement.uid == 0
                || !placement_uids.insert(placement.uid)
                || !image_ids.contains(&ImageId(placement.image_id))
            {
                return Err("graphics relay metadata contains an invalid placement".into());
            }
        }
        for placement in &metadata.virtual_placements {
            if placement.uid == 0
                || !placement_uids.insert(placement.uid)
                || !image_ids.contains(&ImageId(placement.image_id))
            {
                return Err("graphics relay metadata contains an invalid virtual placement".into());
            }
        }
        validate_relay_relative_placements(&metadata.placements, &placement_uids)?;

        let mut image_number_by_id = HashMap::new();
        let mut image_numbers = HashMap::new();
        for entry in &metadata.image_numbers {
            if entry.number == 0
                || entry.image_ids.is_empty()
                || image_numbers.contains_key(&entry.number)
            {
                return Err(
                    "graphics relay metadata contains an invalid image-number alias".into(),
                );
            }
            let mut ids = VecDeque::new();
            for raw_id in &entry.image_ids {
                let image_id = ImageId(*raw_id);
                if !image_ids.contains(&image_id)
                    || image_number_by_id.insert(image_id, entry.number).is_some()
                {
                    return Err(
                        "graphics relay metadata contains an invalid image-number target".into(),
                    );
                }
                ids.push_back(image_id);
            }
            image_numbers.insert(entry.number, ids);
        }
        let protocol_images: HashSet<_> = metadata
            .protocol_images
            .iter()
            .copied()
            .map(ImageId)
            .collect();
        if protocol_images.len() != metadata.protocol_images.len()
            || protocol_images
                .iter()
                .any(|image_id| !image_ids.contains(image_id))
        {
            return Err("graphics relay metadata contains an invalid anonymous image".into());
        }

        self.storage
            .restore_relay_metadata(&storage_order, &revisions)?;
        self.placements = metadata
            .placements
            .iter()
            .map(relay_managed_placement)
            .collect();
        self.virtual_placements = metadata
            .virtual_placements
            .iter()
            .map(|placement| VirtualImagePlacement {
                uid: PlacementUid(placement.uid),
                image_id: ImageId(placement.image_id),
                placement_id: placement.placement_id,
                columns: placement.columns,
                rows: placement.rows,
                alternate_screen: placement.alternate_screen,
            })
            .collect();
        self.image_numbers = image_numbers;
        self.image_number_by_id = image_number_by_id;
        self.protocol_images = protocol_images;
        self.next_anonymous_id = metadata.next_anonymous_id;
        self.next_client_id = metadata.next_client_id;
        self.next_revision = metadata.next_revision;
        self.next_placement_uid = metadata.next_placement_uid;

        let mut restored_animations = HashMap::new();
        let mut animation_memory = 0usize;
        for metadata_animation in metadata.animations {
            let image_id = ImageId(metadata_animation.image_id);
            let mut animation = self
                .animations
                .remove(&image_id)
                .unwrap_or_else(|| AnimatedImage::new(now));
            for ((frame, revision), gap_ms) in animation
                .frames
                .iter_mut()
                .zip(metadata_animation.frame_revisions)
                .zip(metadata_animation.frame_gaps_ms)
            {
                frame.revision = revision;
                frame.gap_ms = gap_ms;
                animation_memory = animation_memory.saturating_add(frame.data.len());
            }
            animation.root_gap_ms = metadata_animation.root_gap_ms;
            animation.current_frame = metadata_animation.current_frame;
            animation.playback = match metadata_animation.playback {
                1 => AnimationPlayback::Stopped,
                2 => AnimationPlayback::Loading,
                3 => AnimationPlayback::Running,
                _ => unreachable!("validated relay playback state"),
            };
            animation.max_loops = metadata_animation.max_loops;
            animation.completed_loops = metadata_animation.completed_loops;
            animation.shown_since = now
                .checked_sub(Duration::from_millis(metadata_animation.shown_elapsed_ms))
                .unwrap_or(now);
            restored_animations.insert(image_id, animation);
        }
        if animation_memory > MAX_ANIMATION_MEMORY {
            return Err("graphics relay animation memory exceeds its resource limit".into());
        }
        self.animations = restored_animations;
        self.animation_memory = animation_memory;
        Ok(())
    }

    fn start_transmission(
        &mut self,
        mut cmd: KittyCommand,
        alternate_screen: bool,
    ) -> GraphicsCommandResult {
        if self.pending_frame.is_some() {
            return failure(
                &cmd,
                ProtocolError::invalid("an animation frame transmission is already pending"),
            );
        }
        if let Err(error) = prepare_transmission_data(&mut cmd, self.allow_file_transmission) {
            return failure(&cmd, error);
        }
        if cmd.data.len() > MAX_TRANSMISSION_BYTES {
            return failure(&cmd, ProtocolError::too_large("transmission is too large"));
        }
        let response_number = cmd.image_number.filter(|number| *number != 0);
        let explicit_id = cmd.image_id.filter(|id| *id != 0).map(ImageId);
        let storage_id = if let Some(id) = explicit_id {
            id
        } else if response_number.is_some() {
            match self.allocate_client_id() {
                Ok(id) => id,
                Err(error) => return failure(&cmd, error),
            }
        } else {
            self.allocate_anonymous_id()
        };
        let response_id = explicit_id.map_or(storage_id.0, |id| id.0);
        let response_id = if explicit_id.is_none() && response_number.is_none() {
            0
        } else {
            response_id
        };
        if cmd.placement.virtual_placement && response_id == 0 {
            return failure(
                &cmd,
                ProtocolError::invalid("Unicode placeholder placement requires an image ID"),
            );
        }
        let pending = PendingImage {
            storage_id,
            response_id,
            response_number,
            placement_id: cmd.placement_id.filter(|id| *id != 0 && response_id != 0),
            transmission: cmd.transmission,
            action: cmd.action,
            placement: cmd.placement,
            data: cmd.data,
            quiet: cmd.quiet,
            alternate_screen,
        };
        if cmd.more_chunks {
            self.pending = Some(pending);
            return GraphicsCommandResult::default();
        }
        self.finalize_image(pending, false)
    }

    fn continue_transmission(&mut self, cmd: KittyCommand) -> GraphicsCommandResult {
        let Some(mut pending) = self.pending.take() else {
            return failure(&cmd, ProtocolError::invalid("no transmission is pending"));
        };
        if cmd.transmission.medium != KittyMedium::Direct {
            return pending_failure(
                pending,
                ProtocolError::invalid("continuation chunks must use direct transmission"),
            );
        }
        if cmd.image_id.is_some() && cmd.image_id != Some(pending.response_id) {
            return GraphicsCommandResult {
                response: response_fields(
                    pending.response_id,
                    pending.response_number,
                    pending.placement_id,
                    pending.quiet,
                    Err(ProtocolError::invalid("chunk interrupted by another image")),
                ),
                cursor_movement: None,
            };
        }
        let Some(new_len) = pending.data.len().checked_add(cmd.data.len()) else {
            return pending_failure(pending, ProtocolError::too_large("transmission overflow"));
        };
        if new_len > MAX_TRANSMISSION_BYTES {
            return pending_failure(
                pending,
                ProtocolError::too_large("transmission is too large"),
            );
        }
        pending.data.extend_from_slice(&cmd.data);
        if cmd.more_chunks {
            self.pending = Some(pending);
            GraphicsCommandResult::default()
        } else {
            self.finalize_image(pending, false)
        }
    }

    fn start_frame_transmission(
        &mut self,
        mut cmd: KittyCommand,
        now: Instant,
    ) -> GraphicsCommandResult {
        if self.pending.is_some() {
            return failure(
                &cmd,
                ProtocolError::invalid("an image transmission is already pending"),
            );
        }
        if let Err(error) = prepare_transmission_data(&mut cmd, self.allow_file_transmission) {
            return failure(&cmd, error);
        }
        if cmd.data.len() > MAX_TRANSMISSION_BYTES {
            return failure(&cmd, ProtocolError::too_large("transmission is too large"));
        }
        let Some(image_id) = self.resolve_image_reference(&cmd) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation frame image was not found"),
            );
        };
        let Some(image) = self.storage.get(image_id) else {
            return failure(
                &cmd,
                ProtocolError::not_found("animation frame image was not found"),
            );
        };
        if cmd.transmission.format != KittyFormat::Png {
            cmd.transmission.width = cmd.transmission.width.or(Some(image.width));
            cmd.transmission.height = cmd.transmission.height.or(Some(image.height));
        }
        let pending = PendingFrame {
            image_id,
            response_id: image_id.0,
            response_number: cmd.image_number.filter(|number| *number != 0),
            transmission: cmd.transmission,
            frame: cmd.frame,
            data: cmd.data,
            quiet: cmd.quiet,
        };
        if cmd.more_chunks {
            self.pending_frame = Some(pending);
            return GraphicsCommandResult::default();
        }
        self.finalize_frame_transmission(pending, now)
    }

    fn continue_frame_transmission(
        &mut self,
        cmd: KittyCommand,
        now: Instant,
    ) -> GraphicsCommandResult {
        let Some(mut pending) = self.pending_frame.take() else {
            return failure(
                &cmd,
                ProtocolError::invalid("no frame transmission is pending"),
            );
        };
        if cmd.transmission.medium != KittyMedium::Direct {
            return pending_frame_failure(
                pending,
                ProtocolError::invalid("continuation chunks must use direct transmission"),
            );
        }
        if cmd.image_id.is_some_and(|id| id != pending.response_id)
            || cmd
                .image_number
                .is_some_and(|number| Some(number) != pending.response_number)
        {
            return pending_frame_failure(
                pending,
                ProtocolError::invalid("chunk interrupted by another image"),
            );
        }
        let Some(new_len) = pending.data.len().checked_add(cmd.data.len()) else {
            return pending_frame_failure(
                pending,
                ProtocolError::too_large("transmission overflow"),
            );
        };
        if new_len > MAX_TRANSMISSION_BYTES {
            return pending_frame_failure(
                pending,
                ProtocolError::too_large("transmission is too large"),
            );
        }
        pending.data.extend_from_slice(&cmd.data);
        if cmd.quiet != 0 {
            pending.quiet = cmd.quiet;
        }
        if cmd.more_chunks {
            self.pending_frame = Some(pending);
            GraphicsCommandResult::default()
        } else {
            self.finalize_frame_transmission(pending, now)
        }
    }

    fn finalize_frame_transmission(
        &mut self,
        pending: PendingFrame,
        now: Instant,
    ) -> GraphicsCommandResult {
        let decoded = match decode_payload(&pending.transmission, &pending.data) {
            Ok(decoded) => decoded,
            Err(error) => return pending_frame_failure(pending, error),
        };
        let frame_number =
            match self.apply_frame_update(pending.image_id, pending.frame, decoded, now) {
                Ok(frame_number) => frame_number,
                Err(error) => return pending_frame_failure(pending, error),
            };
        self.storage.touch(pending.image_id);
        let response = frame_response_fields(
            pending.response_id,
            pending.response_number,
            Some(frame_number),
            pending.quiet,
            Ok(()),
        );
        GraphicsCommandResult {
            response,
            cursor_movement: None,
        }
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
                for pixel in canvas.chunks_exact_mut(4) {
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

        if pending.action == KittyAction::TransmitAndDisplay
            && let Err(error) = validate_cursor_policy(&pending.placement)
        {
            return pending_failure(pending, error);
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
        if let Err(error) = validate_cursor_policy(&cmd.placement) {
            return failure(&cmd, error);
        }
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
        let movement = (relative.is_none() && !placement.do_not_move_cursor)
            .then_some((placement.width.unwrap_or(1), placement.height.unwrap_or(1)));
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
        self.push_placement(ManagedImagePlacement {
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
                width: placement.width,
                height: placement.height,
                pixel_width: None,
                pixel_height: None,
                z_index: placement.z_index.unwrap_or(0),
            },
        });
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
        if self.virtual_placements.len() >= MAX_VIRTUAL_IMAGE_PLACEMENTS {
            if let Some(uid) = self
                .virtual_placements
                .front()
                .map(|placement| placement.uid)
            {
                self.remove_placement_groups([uid], false);
            }
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

    /// Build renderer-owned instances for ordinary placements and for the
    /// visible Unicode placeholder cells backed by virtual placements.
    pub(crate) fn presentation_images(
        &self,
        grid: &Grid,
        alternate_screen: bool,
        cell_width: u32,
        cell_height: u32,
    ) -> Vec<PresentationImage> {
        let mut placeholder_presentations = Vec::new();
        let mut virtual_origins: HashMap<PlacementUid, (i64, i64)> = HashMap::new();
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        let row_offset = i64::try_from(grid.scroll_offset()).unwrap_or(i64::MAX);
        let mut placeholder_count = 0usize;
        if self
            .virtual_placements
            .iter()
            .any(|placement| placement.alternate_screen == alternate_screen)
        {
            for visual_row in 0..grid.rows() {
                let mut decoder = PlaceholderDecoder::default();
                for col in 0..grid.cols() {
                    let Some(cell) = grid.cell_scrolled(visual_row, col) else {
                        decoder = PlaceholderDecoder::default();
                        continue;
                    };
                    let Some(decoded) = decoder.decode(&cell) else {
                        continue;
                    };
                    let Some(prototype) = self.virtual_placement_for(
                        ImageId(decoded.image_id),
                        decoded.placement_id,
                        alternate_screen,
                    ) else {
                        continue;
                    };
                    let Some(image) = self.current_image(prototype.image_id) else {
                        continue;
                    };
                    let screen_col = u32::try_from(col).unwrap_or(u32::MAX);
                    let screen_row = i64::try_from(visual_row)
                        .unwrap_or(i64::MAX)
                        .saturating_sub(row_offset);
                    let Some(presentation) = placeholder_presentation(
                        &image,
                        prototype,
                        decoded.image_col,
                        decoded.image_row,
                        screen_col,
                        screen_row,
                        cell_width,
                        cell_height,
                    ) else {
                        continue;
                    };
                    virtual_origins
                        .entry(prototype.uid)
                        .and_modify(|origin| {
                            origin.0 = origin.0.min(i64::from(screen_col));
                            origin.1 = origin.1.min(screen_row);
                        })
                        .or_insert((i64::from(screen_col), screen_row));
                    if placeholder_count < MAX_IMAGE_PLACEMENTS {
                        placeholder_presentations.push(presentation);
                        placeholder_count += 1;
                    }
                }
            }
        }

        let placements_by_uid: HashMap<PlacementUid, &ManagedImagePlacement> = self
            .placements
            .iter()
            .map(|placement| (placement.uid, placement))
            .collect();
        let mut result: Vec<PresentationImage> = self
            .placements
            .iter()
            .filter(|placement| placement.alternate_screen == alternate_screen)
            .filter_map(|placement| {
                let image = self.current_image(placement.image_id)?;
                let (col, row) = self.resolved_placement_position(
                    placement.uid,
                    &placements_by_uid,
                    &virtual_origins,
                )?;
                let mut resolved = placement.placement.clone();
                resolved.row = row;
                if col < 0 {
                    relative_negative_presentation(resolved, &image, col, cell_width, cell_height)
                } else {
                    resolved.col = u32::try_from(col).ok()?;
                    Some(PresentationImage::from_placement(&resolved, &image))
                }
            })
            .collect();
        result.extend(placeholder_presentations);
        result
    }

    fn resolved_placement_position(
        &self,
        mut uid: PlacementUid,
        placements_by_uid: &HashMap<PlacementUid, &ManagedImagePlacement>,
        virtual_origins: &HashMap<PlacementUid, (i64, i64)>,
    ) -> Option<(i64, i64)> {
        let mut horizontal = 0i64;
        let mut vertical = 0i64;
        for _ in 0..=MAX_RELATIVE_PLACEMENT_DEPTH {
            if let Some(placement) = placements_by_uid.get(&uid) {
                if let Some(relative) = placement.relative {
                    horizontal = horizontal.checked_add(i64::from(relative.horizontal_offset))?;
                    vertical = vertical.checked_add(i64::from(relative.vertical_offset))?;
                    uid = relative.parent;
                    continue;
                }
                return Some((
                    i64::from(placement.col).checked_add(horizontal)?,
                    placement.row.checked_add(vertical)?,
                ));
            }
            let (col, row) = *virtual_origins.get(&uid)?;
            return Some((col.checked_add(horizontal)?, row.checked_add(vertical)?));
        }
        None
    }

    fn virtual_placement_for(
        &self,
        image_id: ImageId,
        placement_id: Option<u32>,
        alternate_screen: bool,
    ) -> Option<&VirtualImagePlacement> {
        self.virtual_placements.iter().find(|placement| {
            placement.image_id == image_id
                && placement.alternate_screen == alternate_screen
                && placement_id.is_none_or(|id| placement.placement_id == Some(id))
        })
    }

    pub fn get_image(&self, id: ImageId) -> Option<&StoredImage> {
        self.storage.get(id)
    }

    pub fn clear_screen(&mut self, alternate_screen: bool) {
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
    pub fn clear_screen_all(&mut self, alternate_screen: bool) {
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
                if let Some(fragment) = crop_placement(&placement, fragment) {
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
                let mut copy = crop_placement(placement, intersection)?;
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

    pub fn abort_pending(&mut self) {
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
        let placement_uids_before = self.placement_uids();
        let count = count as i64;
        if preserve_history {
            let history_top = -(history_lines as i64);
            self.placements.retain_mut(|placement| {
                if placement.alternate_screen != alternate_screen {
                    return true;
                }
                if placement.relative.is_some() {
                    return true;
                }
                placement.row = placement.row.saturating_sub(count);
                placement
                    .row
                    .saturating_add(i64::from(placement.height.unwrap_or(1)))
                    > history_top
            });
            self.cascade_implicitly_removed(placement_uids_before);
            self.remove_unplaced_protocol_images();
            return;
        }
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
                clip_placement_top(placement, clipped_rows, rows);
                placement.row = top;
                true
            }
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
                    clip_placement_bottom(placement, visible_rows, rows);
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
    pub fn image_count(&self) -> usize {
        self.storage.len()
    }

    /// Number of live placements across the primary and alternate screens.
    pub fn placement_count(&self) -> usize {
        self.placements.len() + self.virtual_placements.len()
    }

    /// Recompute the grid footprint of pixel-addressed placements after the
    /// drawable cell size changes. Their pixel extent remains exact.
    pub(crate) fn set_cell_pixel_geometry(&mut self, cell_width: u32, cell_height: u32) {
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);
        for placement in &mut self.placements {
            if !self.protocol_images.contains(&placement.image_id) {
                continue;
            }
            if let Some(pixel_width) = placement.pixel_width {
                placement.width = Some(pixel_width.div_ceil(cell_width).max(1));
            }
            if let Some(pixel_height) = placement.pixel_height {
                placement.height = Some(pixel_height.div_ceil(cell_height).max(1));
            }
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

fn placement_intersects(placement: &ImagePlacement, col: u32, row: i64) -> bool {
    col >= placement.col
        && col < placement.col.saturating_add(placement.width.unwrap_or(1))
        && row >= placement.row
        && row
            < placement
                .row
                .saturating_add(i64::from(placement.height.unwrap_or(1)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlacementRect {
    left: u32,
    right: u32,
    top: i64,
    bottom: i64,
}

impl PlacementRect {
    fn intersection(self, other: Self) -> Option<Self> {
        let intersection = Self {
            left: self.left.max(other.left),
            right: self.right.min(other.right),
            top: self.top.max(other.top),
            bottom: self.bottom.min(other.bottom),
        };
        (intersection.left < intersection.right && intersection.top < intersection.bottom)
            .then_some(intersection)
    }
}

#[allow(clippy::too_many_arguments)]
fn placeholder_presentation(
    image: &StoredImage,
    prototype: &VirtualImagePlacement,
    image_col: u32,
    image_row: u32,
    screen_col: u32,
    screen_row: i64,
    cell_width: u32,
    cell_height: u32,
) -> Option<PresentationImage> {
    if image.width == 0 || image.height == 0 {
        return None;
    }
    let columns = prototype
        .columns
        .unwrap_or_else(|| image.width.saturating_add(cell_width - 1) / cell_width)
        .max(1);
    let rows = prototype
        .rows
        .unwrap_or_else(|| image.height.saturating_add(cell_height - 1) / cell_height)
        .max(1);
    if image_col >= columns || image_row >= rows {
        return None;
    }

    let box_width = f64::from(columns) * f64::from(cell_width);
    let box_height = f64::from(rows) * f64::from(cell_height);
    let scale = (box_width / f64::from(image.width)).min(box_height / f64::from(image.height));
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let drawn_width = f64::from(image.width) * scale;
    let drawn_height = f64::from(image.height) * scale;
    let image_left = (box_width - drawn_width) / 2.0;
    let image_top = (box_height - drawn_height) / 2.0;
    let cell_left = f64::from(image_col) * f64::from(cell_width);
    let cell_top = f64::from(image_row) * f64::from(cell_height);
    let visible_left = cell_left.max(image_left);
    let visible_top = cell_top.max(image_top);
    let visible_right = (cell_left + f64::from(cell_width)).min(image_left + drawn_width);
    let visible_bottom = (cell_top + f64::from(cell_height)).min(image_top + drawn_height);
    if visible_right <= visible_left || visible_bottom <= visible_top {
        return None;
    }

    let u_min = ((visible_left - image_left) / drawn_width).clamp(0.0, 1.0);
    let v_min = ((visible_top - image_top) / drawn_height).clamp(0.0, 1.0);
    let u_max = ((visible_right - image_left) / drawn_width).clamp(0.0, 1.0);
    let v_max = ((visible_bottom - image_top) / drawn_height).clamp(0.0, 1.0);
    Some(PresentationImage {
        placement: ImagePlacement {
            image_id: image.id,
            placement_id: prototype.placement_id,
            col: screen_col,
            row: screen_row,
            alternate_screen: prototype.alternate_screen,
            source_x: 0,
            source_y: 0,
            source_width: Some(image.width),
            source_height: Some(image.height),
            x_offset: 0,
            y_offset: 0,
            width: Some(1),
            height: Some(1),
            pixel_width: None,
            pixel_height: None,
            z_index: -1,
        },
        image: image.clone(),
        pixel_rect: Some([
            (visible_left - cell_left) as f32,
            (visible_top - cell_top) as f32,
            (visible_right - visible_left) as f32,
            (visible_bottom - visible_top) as f32,
        ]),
        source_uv: Some([u_min as f32, v_min as f32, u_max as f32, v_max as f32]),
    })
}

fn placement_rect(placement: &ImagePlacement) -> PlacementRect {
    PlacementRect {
        left: placement.col,
        right: placement.col.saturating_add(placement.width.unwrap_or(1)),
        top: placement.row,
        bottom: placement
            .row
            .saturating_add(i64::from(placement.height.unwrap_or(1))),
    }
}

fn crop_placement(placement: &ImagePlacement, requested: PlacementRect) -> Option<ImagePlacement> {
    let bounds = placement_rect(placement);
    let crop = bounds.intersection(requested)?;
    let width = bounds.right - bounds.left;
    let height = u32::try_from(bounds.bottom - bounds.top).ok()?;
    let left = crop.left - bounds.left;
    let right = crop.right - bounds.left;
    let top = u32::try_from(crop.top - bounds.top).ok()?;
    let bottom = u32::try_from(crop.bottom - bounds.top).ok()?;

    let scaled_range = |extent: u32, start: u32, end: u32, total: u32| {
        let start = (u64::from(extent) * u64::from(start) / u64::from(total)) as u32;
        let end = (u64::from(extent) * u64::from(end)).div_ceil(u64::from(total)) as u32;
        (start, end.saturating_sub(start))
    };
    let source_width = placement.source_width.unwrap_or(0);
    let source_height = placement.source_height.unwrap_or(0);
    let (source_left, source_width) = scaled_range(source_width, left, right, width);
    let (source_top, source_height) = scaled_range(source_height, top, bottom, height);

    let mut result = placement.clone();
    result.col = crop.left;
    result.row = crop.top;
    result.source_x = result.source_x.saturating_add(source_left);
    result.source_y = result.source_y.saturating_add(source_top);
    result.source_width = Some(source_width);
    result.source_height = Some(source_height);
    result.width = Some(crop.right - crop.left);
    result.height = Some(u32::try_from(crop.bottom - crop.top).ok()?);
    if left > 0 {
        result.x_offset = 0;
    }
    if top > 0 {
        result.y_offset = 0;
    }
    if let Some(pixel_width) = placement.pixel_width {
        result.pixel_width = Some(scaled_range(pixel_width, left, right, width).1);
    }
    if let Some(pixel_height) = placement.pixel_height {
        result.pixel_height = Some(scaled_range(pixel_height, top, bottom, height).1);
    }
    Some(result)
}

fn relative_negative_presentation(
    mut placement: ImagePlacement,
    image: &StoredImage,
    col: i64,
    cell_width: u32,
    cell_height: u32,
) -> Option<PresentationImage> {
    let source_x = placement.source_x.min(image.width);
    let source_y = placement.source_y.min(image.height);
    let source_width = placement
        .source_width
        .unwrap_or(image.width.saturating_sub(source_x))
        .min(image.width.saturating_sub(source_x));
    let source_height = placement
        .source_height
        .unwrap_or(image.height.saturating_sub(source_y))
        .min(image.height.saturating_sub(source_y));
    if source_width == 0 || source_height == 0 || image.width == 0 || image.height == 0 {
        return None;
    }
    let aspect = f64::from(source_width) / f64::from(source_height);
    let (width, height) = match (placement.pixel_width, placement.pixel_height) {
        (Some(width), Some(height)) => (f64::from(width), f64::from(height)),
        (Some(width), None) => {
            let width = f64::from(width);
            (width, width / aspect)
        }
        (None, Some(height)) => {
            let height = f64::from(height);
            (height * aspect, height)
        }
        (None, None) => match (placement.width, placement.height) {
            (Some(columns), Some(rows)) => (
                f64::from(columns) * f64::from(cell_width),
                f64::from(rows) * f64::from(cell_height),
            ),
            (Some(columns), None) => {
                let width = f64::from(columns) * f64::from(cell_width);
                (width, width / aspect)
            }
            (None, Some(rows)) => {
                let height = f64::from(rows) * f64::from(cell_height);
                (height * aspect, height)
            }
            (None, None) => (f64::from(source_width), f64::from(source_height)),
        },
    };
    let x = (col as f64).mul_add(f64::from(cell_width), f64::from(placement.x_offset));
    let y = f64::from(placement.y_offset);
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return None;
    }
    placement.col = 0;
    placement.x_offset = 0;
    placement.y_offset = 0;
    Some(PresentationImage {
        placement,
        image: image.clone(),
        pixel_rect: Some([x as f32, y as f32, width as f32, height as f32]),
        source_uv: Some([
            source_x as f32 / image.width as f32,
            source_y as f32 / image.height as f32,
            source_x.saturating_add(source_width) as f32 / image.width as f32,
            source_y.saturating_add(source_height) as f32 / image.height as f32,
        ]),
    })
}

fn clip_placement_top(placement: &mut ImagePlacement, clipped_rows: u32, total_rows: u32) {
    let source_height = placement.source_height.unwrap_or(0);
    let clipped_pixels = ((source_height as u64 * clipped_rows as u64) / total_rows as u64) as u32;
    placement.source_y = placement.source_y.saturating_add(clipped_pixels);
    placement.source_height = Some(source_height.saturating_sub(clipped_pixels));
    placement.height = Some(total_rows - clipped_rows);
    if let Some(pixel_height) = placement.pixel_height {
        placement.pixel_height = Some(
            ((u64::from(pixel_height) * u64::from(total_rows - clipped_rows))
                / u64::from(total_rows)) as u32,
        );
    }
}

fn clip_placement_bottom(placement: &mut ImagePlacement, visible_rows: u32, total_rows: u32) {
    let source_height = placement.source_height.unwrap_or(0);
    placement.source_height =
        Some(((source_height as u64 * visible_rows as u64) / total_rows as u64) as u32);
    placement.height = Some(visible_rows);
    if let Some(pixel_height) = placement.pixel_height {
        placement.pixel_height = Some(
            ((u64::from(pixel_height) * u64::from(visible_rows)) / u64::from(total_rows)) as u32,
        );
    }
}

fn validate_cursor_policy(placement: &KittyPlacement) -> Result<(), ProtocolError> {
    if placement.virtual_placement
        || placement.parent_image_id.is_some()
        || placement.do_not_move_cursor
        || placement.width.is_some() && placement.height.is_some()
    {
        Ok(())
    } else {
        Err(ProtocolError::unsupported(
            "C=1 or both c and r are required for exact cursor movement",
        ))
    }
}

fn prepare_transmission_data(
    cmd: &mut KittyCommand,
    allow_file_transmission: bool,
) -> Result<(), ProtocolError> {
    match cmd.transmission.medium {
        KittyMedium::Direct => {
            if cmd.transmission.data_offset.is_some() {
                return Err(ProtocolError::invalid(
                    "O is only valid for local transmission media",
                ));
            }
            Ok(())
        }
        KittyMedium::File => {
            if !allow_file_transmission {
                return Err(ProtocolError::permission_denied(
                    "ordinary file transmission is disabled by host policy",
                ));
            }
            if cmd.more_chunks {
                return Err(ProtocolError::invalid(
                    "ordinary-file transmission cannot be chunked",
                ));
            }
            #[cfg(unix)]
            {
                cmd.data = read_ordinary_file(
                    &cmd.data,
                    cmd.transmission.data_offset.unwrap_or(0),
                    cmd.transmission.data_size,
                )?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                Err(ProtocolError::unsupported(
                    "ordinary-file transmission is only supported on Unix",
                ))
            }
        }
        KittyMedium::SharedMemory => {
            if cmd.more_chunks {
                return Err(ProtocolError::invalid(
                    "shared-memory transmission cannot be chunked",
                ));
            }
            #[cfg(unix)]
            {
                cmd.data = read_and_unlink_shared_memory(
                    &cmd.data,
                    cmd.transmission.data_offset.unwrap_or(0),
                    cmd.transmission.data_size,
                )?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                Err(ProtocolError::unsupported(
                    "shared-memory transmission is only supported on Unix",
                ))
            }
        }
        KittyMedium::TempFile => {
            if cmd.more_chunks {
                return Err(ProtocolError::invalid(
                    "temporary-file transmission cannot be chunked",
                ));
            }
            #[cfg(unix)]
            {
                cmd.data = read_and_remove_temporary_file(
                    &cmd.data,
                    cmd.transmission.data_offset.unwrap_or(0),
                    cmd.transmission.data_size,
                )?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                Err(ProtocolError::unsupported(
                    "temporary-file transmission is only supported on Unix",
                ))
            }
        }
    }
}

#[cfg(unix)]
fn read_ordinary_file(
    encoded_path: &[u8],
    offset: u64,
    requested_size: Option<u64>,
) -> Result<Vec<u8>, ProtocolError> {
    let path = Path::new(std::ffi::OsStr::from_bytes(encoded_path));
    if encoded_path.is_empty() || !path.is_absolute() {
        return Err(ProtocolError::invalid(
            "ordinary-file path must be absolute",
        ));
    }

    // Resolve symlinks as Kitty specifies, then apply policy to the final
    // target. Device and kernel pseudo-filesystems must never become an
    // alternate path around regular-file validation.
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| filesystem_error(error, "ordinary file is unavailable"))?;
    if is_sensitive_file_root(&canonical_path) {
        return Err(ProtocolError::permission_denied(
            "ordinary file is inside a protected system filesystem",
        ));
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&canonical_path)
        .map_err(|error| filesystem_error(error, "ordinary file could not be opened"))?;
    let opened = file
        .metadata()
        .map_err(|error| filesystem_error(error, "ordinary file metadata is unavailable"))?;
    if !opened.file_type().is_file() {
        return Err(ProtocolError::invalid(
            "ordinary-file medium requires a regular file",
        ));
    }
    ensure_ordinary_file_unchanged(&canonical_path, &opened)?;

    let available = opened
        .len()
        .checked_sub(offset)
        .ok_or_else(|| ProtocolError::invalid("ordinary-file offset exceeds file size"))?;
    let size = requested_size.unwrap_or(available).min(available);
    if size > MAX_TRANSMISSION_BYTES as u64 {
        return Err(ProtocolError::too_large("ordinary-file data is too large"));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| ProtocolError::io("ordinary file could not be positioned"))?;
    let mut data = Vec::with_capacity(size as usize);
    file.by_ref()
        .take(size)
        .read_to_end(&mut data)
        .map_err(|_| ProtocolError::io("ordinary file could not be read"))?;
    if data.len() as u64 != size {
        return Err(ProtocolError::io(
            "ordinary file ended before the requested range",
        ));
    }
    ensure_ordinary_file_unchanged(&canonical_path, &opened)?;
    Ok(data)
}

#[cfg(unix)]
fn ensure_ordinary_file_unchanged(
    path: &Path,
    opened: &std::fs::Metadata,
) -> Result<(), ProtocolError> {
    let current_path = std::fs::canonicalize(path)
        .map_err(|error| filesystem_error(error, "ordinary file changed while being read"))?;
    if is_sensitive_file_root(&current_path) {
        return Err(ProtocolError::permission_denied(
            "ordinary file moved into a protected system filesystem",
        ));
    }
    let current = std::fs::metadata(&current_path)
        .map_err(|error| filesystem_error(error, "ordinary file changed while being read"))?;
    if !current.file_type().is_file() || !same_file(opened, &current) {
        return Err(ProtocolError::invalid(
            "ordinary file changed while being read",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn is_sensitive_file_root(path: &Path) -> bool {
    [Path::new("/dev"), Path::new("/proc"), Path::new("/sys")]
        .into_iter()
        .any(|root| path.starts_with(root))
}

#[cfg(unix)]
fn read_and_remove_temporary_file(
    encoded_path: &[u8],
    offset: u64,
    requested_size: Option<u64>,
) -> Result<Vec<u8>, ProtocolError> {
    let path = Path::new(std::ffi::OsStr::from_bytes(encoded_path));
    if encoded_path.is_empty() || !path.is_absolute() {
        return Err(ProtocolError::invalid(
            "temporary-file path must be absolute",
        ));
    }
    if !encoded_path
        .windows(b"tty-graphics-protocol".len())
        .any(|window| window == b"tty-graphics-protocol")
    {
        return Err(ProtocolError::invalid(
            "temporary-file path is missing its safety marker",
        ));
    }

    let parent = path
        .parent()
        .ok_or_else(|| ProtocolError::invalid("temporary-file path has no parent"))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|error| filesystem_error(error, "temporary-file directory is unavailable"))?;
    if !allowed_temporary_roots().contains(&canonical_parent) {
        return Err(ProtocolError::invalid(
            "temporary file is outside an allowed temporary directory",
        ));
    }

    let before = std::fs::symlink_metadata(path)
        .map_err(|error| filesystem_error(error, "temporary file is unavailable"))?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(ProtocolError::invalid(
            "temporary-file medium requires a regular non-symlink file",
        ));
    }

    let mut file = std::fs::File::open(path)
        .map_err(|error| filesystem_error(error, "temporary file could not be opened"))?;
    let opened = file
        .metadata()
        .map_err(|error| filesystem_error(error, "temporary file metadata is unavailable"))?;
    if !opened.file_type().is_file() || !same_file(&before, &opened) {
        return Err(ProtocolError::invalid(
            "temporary file changed before it could be read",
        ));
    }

    let read_result = (|| {
        let available = opened
            .len()
            .checked_sub(offset)
            .ok_or_else(|| ProtocolError::invalid("temporary-file offset exceeds file size"))?;
        let size = requested_size.unwrap_or(available).min(available);
        if size > MAX_TRANSMISSION_BYTES as u64 {
            return Err(ProtocolError::too_large("temporary-file data is too large"));
        }
        // S is a read ceiling. A short local object supplies all remaining
        // bytes, matching ordinary read-to-EOF semantics in Kitty's loader.

        file.seek(SeekFrom::Start(offset))
            .map_err(|_| ProtocolError::io("temporary file could not be positioned"))?;
        let mut data = Vec::with_capacity(size as usize);
        file.by_ref()
            .take(size)
            .read_to_end(&mut data)
            .map_err(|_| ProtocolError::io("temporary file could not be read"))?;
        if data.len() as u64 != size {
            return Err(ProtocolError::io(
                "temporary file ended before the requested range",
            ));
        }
        Ok(data)
    })();

    let cleanup_result = remove_temporary_file_if_unchanged(path, &opened);
    match (read_result, cleanup_result) {
        (_, Err(error)) => Err(error),
        (result, Ok(())) => result,
    }
}

#[cfg(unix)]
fn read_and_unlink_shared_memory(
    name: &[u8],
    offset: u64,
    requested_size: Option<u64>,
) -> Result<Vec<u8>, ProtocolError> {
    const MAX_SHARED_MEMORY_NAME_BYTES: usize = 255;

    if name.len() < 2
        || name.len() > MAX_SHARED_MEMORY_NAME_BYTES
        || name[0] != b'/'
        || name[1..].contains(&b'/')
    {
        return Err(ProtocolError::invalid(
            "shared-memory name must be one bounded POSIX name",
        ));
    }
    let name = std::ffi::CString::new(name)
        .map_err(|_| ProtocolError::invalid("shared-memory name contains NUL"))?;
    // SAFETY: `name` is a live NUL-terminated C string, the flags do not
    // create an object, and the returned descriptor is checked before it is
    // transferred into `File` ownership.
    let descriptor = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC, 0) };
    if descriptor < 0 {
        return Err(filesystem_error(
            std::io::Error::last_os_error(),
            "shared-memory object is unavailable",
        ));
    }
    // SAFETY: `descriptor` is a newly returned owned descriptor and this is
    // its sole transfer into a type that closes it on drop.
    let mut file = unsafe { std::fs::File::from_raw_fd(descriptor) };
    let metadata = file
        .metadata()
        .map_err(|error| filesystem_error(error, "shared-memory metadata is unavailable"))?;
    // SAFETY: `geteuid` has no preconditions and does not access Rust memory.
    let effective_uid = unsafe { libc::geteuid() };
    #[cfg(target_vendor = "apple")]
    let valid_object_type = true;
    #[cfg(not(target_vendor = "apple"))]
    let valid_object_type = metadata.file_type().is_file();
    if !valid_object_type || metadata.uid() != effective_uid || metadata.mode() & 0o077 != 0 {
        return Err(ProtocolError::invalid(
            "shared-memory object must be a private regular object owned by this user",
        ));
    }

    unlink_shared_memory_if_unchanged(&name, &metadata)?;

    let available = metadata
        .len()
        .checked_sub(offset)
        .ok_or_else(|| ProtocolError::invalid("shared-memory offset exceeds object size"))?;
    let size = requested_size.unwrap_or(available).min(available);
    if size > MAX_TRANSMISSION_BYTES as u64 {
        return Err(ProtocolError::too_large("shared-memory data is too large"));
    }
    // S is a read ceiling. A short object supplies all remaining bytes.

    read_shared_memory_range(&mut file, offset, size)
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn read_shared_memory_range(
    file: &mut std::fs::File,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, ProtocolError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| ProtocolError::io("shared-memory object could not be positioned"))?;
    let mut data = Vec::with_capacity(size as usize);
    file.by_ref()
        .take(size)
        .read_to_end(&mut data)
        .map_err(|_| ProtocolError::io("shared-memory object could not be read"))?;
    if data.len() as u64 != size {
        return Err(ProtocolError::io(
            "shared-memory object ended before the requested range",
        ));
    }
    Ok(data)
}

#[cfg(target_vendor = "apple")]
fn read_shared_memory_range(
    file: &mut std::fs::File,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, ProtocolError> {
    if size == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: sysconf has no pointer arguments or Rust-memory preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(ProtocolError::io("shared-memory page size is unavailable"));
    }
    let page_size = page_size as u64;
    let page_start = offset / page_size * page_size;
    let delta = usize::try_from(offset - page_start)
        .map_err(|_| ProtocolError::too_large("shared-memory offset is too large"))?;
    let size = usize::try_from(size)
        .map_err(|_| ProtocolError::too_large("shared-memory data is too large"))?;
    let map_size = size
        .checked_add(delta)
        .ok_or_else(|| ProtocolError::too_large("shared-memory mapping size overflow"))?;
    let page_start = libc::off_t::try_from(page_start)
        .map_err(|_| ProtocolError::too_large("shared-memory offset is too large"))?;
    // SAFETY: the descriptor is open and validated, the aligned range is
    // within the metadata length checked above, and MAP_FAILED is handled.
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            map_size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            page_start,
        )
    };
    if address == libc::MAP_FAILED {
        return Err(filesystem_error(
            std::io::Error::last_os_error(),
            "shared-memory object could not be mapped",
        ));
    }
    let mut data = vec![0; size];
    // SAFETY: the mapping covers `delta + size` bytes and `data` owns `size`
    // initialized writable bytes. These regions do not overlap.
    unsafe {
        std::ptr::copy_nonoverlapping((address as *const u8).add(delta), data.as_mut_ptr(), size);
        libc::munmap(address, map_size);
    }
    Ok(data)
}

#[cfg(unix)]
fn unlink_shared_memory_if_unchanged(
    name: &std::ffi::CStr,
    opened: &std::fs::Metadata,
) -> Result<(), ProtocolError> {
    // SAFETY: `name` is a live NUL-terminated C string and the returned
    // descriptor is checked before ownership is transferred to `File`.
    let descriptor = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC, 0) };
    if descriptor < 0 {
        return Err(filesystem_error(
            std::io::Error::last_os_error(),
            "shared-memory object disappeared before cleanup",
        ));
    }
    // SAFETY: `descriptor` is a newly returned owned descriptor and this is
    // its sole transfer into a type that closes it on drop.
    let current = unsafe { std::fs::File::from_raw_fd(descriptor) };
    let current_metadata = current
        .metadata()
        .map_err(|error| filesystem_error(error, "shared-memory metadata is unavailable"))?;
    if !same_file(opened, &current_metadata) {
        return Err(ProtocolError::invalid(
            "shared-memory object changed before cleanup",
        ));
    }
    // Unlink before reading. POSIX keeps both open descriptors valid until
    // drop, while removing the attacker-visible name for every later failure.
    // SAFETY: `name` remains a live NUL-terminated C string for this call.
    if unsafe { libc::shm_unlink(name.as_ptr()) } != 0 {
        return Err(filesystem_error(
            std::io::Error::last_os_error(),
            "shared-memory object could not be unlinked",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn allowed_temporary_roots() -> Vec<PathBuf> {
    [
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/dev/shm"),
    ]
    .into_iter()
    .filter_map(|root| std::fs::canonicalize(root).ok())
    .fold(Vec::new(), |mut roots, root| {
        if !roots.contains(&root) {
            roots.push(root);
        }
        roots
    })
}

#[cfg(unix)]
fn remove_temporary_file_if_unchanged(
    path: &Path,
    opened: &std::fs::Metadata,
) -> Result<(), ProtocolError> {
    let current = std::fs::symlink_metadata(path)
        .map_err(|error| filesystem_error(error, "temporary file disappeared before cleanup"))?;
    if current.file_type().is_symlink()
        || !current.file_type().is_file()
        || !same_file(&current, opened)
    {
        return Err(ProtocolError::invalid(
            "temporary file changed before cleanup",
        ));
    }
    std::fs::remove_file(path)
        .map_err(|error| filesystem_error(error, "temporary file could not be removed"))
}

#[cfg(unix)]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(unix)]
fn filesystem_error(error: std::io::Error, message: &'static str) -> ProtocolError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ProtocolError::not_found(message),
        std::io::ErrorKind::PermissionDenied => ProtocolError::permission_denied(message),
        _ => ProtocolError::io(message),
    }
}

impl Default for GraphicsManager {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct DecodedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data: Vec<u8>,
}

fn decode_image(pending: &PendingImage) -> Result<DecodedImage, ProtocolError> {
    decode_payload(&pending.transmission, &pending.data)
}

fn decode_payload(
    transmission: &KittyTransmission,
    encoded: &[u8],
) -> Result<DecodedImage, ProtocolError> {
    let data = match transmission.compression {
        KittyCompression::None => encoded.to_vec(),
        KittyCompression::Zlib => {
            let expected_size = if transmission.format == KittyFormat::Png {
                let size = transmission.data_size.ok_or_else(|| {
                    ProtocolError::invalid("S is required for compressed PNG data")
                })?;
                if size > MAX_TRANSMISSION_BYTES as u64 {
                    return Err(ProtocolError::too_large("declared data size is too large"));
                }
                Some(size)
            } else {
                None
            };
            let decoder = ZlibDecoder::new(encoded);
            let mut data = Vec::new();
            decoder
                .take((MAX_TRANSMISSION_BYTES + 1) as u64)
                .read_to_end(&mut data)
                .map_err(|_| ProtocolError::invalid("invalid zlib stream"))?;
            if data.len() > MAX_TRANSMISSION_BYTES {
                return Err(ProtocolError::too_large("decompressed data is too large"));
            }
            if let Some(expected) = expected_size {
                if data.len() as u64 != expected {
                    return Err(ProtocolError::invalid("decompressed size does not match S"));
                }
            }
            data
        }
    };

    match transmission.format {
        KittyFormat::Rgba => decode_raw(data, transmission, 4),
        KittyFormat::Rgb => decode_raw(data, transmission, 3),
        KittyFormat::Png => decode_png(data),
    }
}

pub(crate) fn checked_pixel_bytes(
    width: u32,
    height: u32,
    channels: usize,
) -> Result<usize, ProtocolError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(ProtocolError::invalid("invalid image dimensions"));
    }
    let bytes = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| ProtocolError::too_large("image dimensions overflow"))?;
    if bytes > MAX_IMAGE_MEMORY {
        return Err(ProtocolError::too_large("decoded image is too large"));
    }
    Ok(bytes)
}

fn decode_raw(
    data: Vec<u8>,
    transmission: &KittyTransmission,
    channels: usize,
) -> Result<DecodedImage, ProtocolError> {
    let width = transmission
        .width
        .ok_or_else(|| ProtocolError::invalid("s is required for raw pixels"))?;
    let height = transmission
        .height
        .ok_or_else(|| ProtocolError::invalid("v is required for raw pixels"))?;
    let expected = checked_pixel_bytes(width, height, channels)?;
    if data.len() != expected {
        return Err(ProtocolError::invalid("raw pixel data size mismatch"));
    }
    let data = if channels == 4 {
        data
    } else {
        let mut rgba = Vec::with_capacity(checked_pixel_bytes(width, height, 4)?);
        for pixel in data.chunks_exact(3) {
            rgba.extend_from_slice(pixel);
            rgba.push(255);
        }
        rgba
    };
    Ok(DecodedImage {
        width,
        height,
        data,
    })
}

pub(crate) fn decode_png(data: Vec<u8>) -> Result<DecodedImage, ProtocolError> {
    let limits = png::Limits {
        bytes: MAX_IMAGE_MEMORY,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(data), limits);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| ProtocolError::invalid("invalid PNG"))?;
    if reader.info().animation_control.is_some() {
        return Err(ProtocolError::unsupported("animated PNG"));
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| ProtocolError::too_large("PNG dimensions overflow"))?;
    if output_size > MAX_IMAGE_MEMORY {
        return Err(ProtocolError::too_large("decoded PNG is too large"));
    }
    let mut pixels = vec![0; output_size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| ProtocolError::invalid("invalid PNG pixels"))?;
    pixels.truncate(info.buffer_size());
    checked_pixel_bytes(info.width, info.height, 4)?;
    let mut rgba = Vec::with_capacity(checked_pixel_bytes(info.width, info.height, 4)?);
    match info.color_type {
        png::ColorType::Rgba => rgba = pixels,
        png::ColorType::Rgb => {
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
        }
        png::ColorType::Grayscale => {
            for value in pixels {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(ProtocolError::invalid("PNG palette expansion failed"));
        }
    }
    Ok(DecodedImage {
        width: info.width,
        height: info.height,
        data: rgba,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ProtocolError {
    code: &'static str,
    pub(crate) message: String,
}

impl ProtocolError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "EINVAL",
            message: message.into(),
        }
    }
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: "ENOTSUP",
            message: message.into(),
        }
    }
    pub(crate) fn permission_denied(message: impl Into<String>) -> Self {
        Self {
            code: "EPERM",
            message: message.into(),
        }
    }
    pub(crate) fn too_large(message: impl Into<String>) -> Self {
        Self {
            code: "E2BIG",
            message: message.into(),
        }
    }
    fn no_space(message: impl Into<String>) -> Self {
        Self {
            code: "ENOSPC",
            message: message.into(),
        }
    }
    fn parent_missing(message: impl Into<String>) -> Self {
        Self {
            code: "ENOPARENT",
            message: message.into(),
        }
    }
    fn cycle(message: impl Into<String>) -> Self {
        Self {
            code: "ECYCLE",
            message: message.into(),
        }
    }
    fn too_deep(message: impl Into<String>) -> Self {
        Self {
            code: "ETOODEEP",
            message: message.into(),
        }
    }
    fn io(message: impl Into<String>) -> Self {
        Self {
            code: "EIO",
            message: message.into(),
        }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "ENOENT",
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphicsResponse {
    pub image_id: u32,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub frame_number: Option<u32>,
    pub ok: bool,
    pub error_code: Option<&'static str>,
    pub error: Option<String>,
}

impl GraphicsResponse {
    pub fn parse_error(message: &str) -> Self {
        Self {
            image_id: 0,
            image_number: None,
            placement_id: None,
            frame_number: None,
            ok: false,
            error_code: Some("EINVAL"),
            error: Some(message.to_owned()),
        }
    }

    pub fn too_large(message: &str) -> Self {
        Self {
            image_id: 0,
            image_number: None,
            placement_id: None,
            frame_number: None,
            ok: false,
            error_code: Some("E2BIG"),
            error: Some(message.to_owned()),
        }
    }

    pub fn to_escape_sequence(&self) -> String {
        let mut controls = if self.image_id != 0 || self.image_number.is_none() {
            format!("i={}", self.image_id)
        } else {
            String::new()
        };
        if let Some(image_number) = self.image_number.filter(|number| *number != 0) {
            if !controls.is_empty() {
                controls.push(',');
            }
            controls.push_str(&format!("I={image_number}"));
        }
        if self.image_id != 0 {
            if let Some(placement_id) = self.placement_id.filter(|id| *id != 0) {
                controls.push_str(&format!(",p={placement_id}"));
            }
        }
        if let Some(frame_number) = self.frame_number.filter(|frame| *frame != 0) {
            controls.push_str(&format!(",r={frame_number}"));
        }
        if self.ok {
            format!("\x1b_G{controls};OK\x1b\\")
        } else {
            let code = self.error_code.unwrap_or("EINVAL");
            let message: String = self
                .error
                .as_deref()
                .unwrap_or("graphics error")
                .chars()
                .filter(|character| !character.is_control())
                .take(160)
                .collect();
            format!("\x1b_G{controls};{code}:{message}\x1b\\")
        }
    }
}

fn response_for(cmd: &KittyCommand, result: Result<(), ProtocolError>) -> Option<GraphicsResponse> {
    response_fields(
        cmd.image_id.unwrap_or(0),
        cmd.image_number,
        cmd.placement_id,
        cmd.quiet,
        result,
    )
}

fn response_for_resolved(
    cmd: &KittyCommand,
    image_id: u32,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    response_fields(
        image_id,
        cmd.image_number,
        cmd.placement_id,
        cmd.quiet,
        result,
    )
}

fn response_fields(
    image_id: u32,
    image_number: Option<u32>,
    placement_id: Option<u32>,
    quiet: u8,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    let placement_id = placement_id.filter(|value| image_id != 0 && *value != 0);
    let is_error = result.is_err();
    let image_number = image_number.filter(|number| *number != 0);
    if quiet == 2
        || (quiet == 1 && !is_error)
        || (image_id == 0 && image_number.is_none() && placement_id.is_none())
    {
        return None;
    }
    let (ok, error_code, error) = match result {
        Ok(()) => (true, None, None),
        Err(error) => (false, Some(error.code), Some(error.message)),
    };
    Some(GraphicsResponse {
        image_id,
        image_number,
        placement_id,
        frame_number: None,
        ok,
        error_code,
        error,
    })
}

fn frame_response_fields(
    image_id: u32,
    image_number: Option<u32>,
    frame_number: Option<u32>,
    quiet: u8,
    result: Result<(), ProtocolError>,
) -> Option<GraphicsResponse> {
    let mut response = response_fields(image_id, image_number, None, quiet, result)?;
    response.frame_number = frame_number.filter(|frame| *frame != 0);
    Some(response)
}

fn failure(cmd: &KittyCommand, error: ProtocolError) -> GraphicsCommandResult {
    GraphicsCommandResult {
        response: response_for(cmd, Err(error)),
        cursor_movement: None,
    }
}

fn pending_failure(pending: PendingImage, error: ProtocolError) -> GraphicsCommandResult {
    GraphicsCommandResult {
        response: response_fields(
            pending.response_id,
            pending.response_number,
            pending.placement_id,
            pending.quiet,
            Err(error),
        ),
        cursor_movement: None,
    }
}

fn pending_frame_failure(pending: PendingFrame, error: ProtocolError) -> GraphicsCommandResult {
    GraphicsCommandResult {
        response: frame_response_fields(
            pending.response_id,
            pending.response_number,
            pending.frame.frame_number,
            pending.quiet,
            Err(error),
        ),
        cursor_movement: None,
    }
}

fn animation_deadline(animation: &AnimatedImage) -> Option<Instant> {
    if animation.playback == AnimationPlayback::Stopped
        || animation.frames.is_empty()
        || !animation.has_timed_frame()
    {
        return None;
    }
    let last_frame = animation.frame_count() - 1;
    if animation.current_frame == last_frame {
        if animation.playback == AnimationPlayback::Loading {
            return None;
        }
        if animation
            .max_loops
            .is_some_and(|limit| animation.completed_loops.saturating_add(1) >= limit)
        {
            return None;
        }
    }
    animation
        .shown_since
        .checked_add(Duration::from_millis(u64::from(
            animation.gap(animation.current_frame)?,
        )))
}

fn advance_animation_frame(animation: &mut AnimatedImage, deadline: Instant) -> bool {
    let last_frame = animation.frame_count() - 1;
    if animation.current_frame < last_frame {
        animation.current_frame += 1;
    } else {
        if animation.playback != AnimationPlayback::Running
            || animation
                .max_loops
                .is_some_and(|limit| animation.completed_loops.saturating_add(1) >= limit)
        {
            return false;
        }
        animation.completed_loops = animation.completed_loops.saturating_add(1);
        animation.current_frame = 0;
    }
    animation.shown_since = deadline;
    true
}

fn validate_frame_rectangle(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    frame_width: u32,
    frame_height: u32,
) -> Result<(), ProtocolError> {
    if width == 0
        || height == 0
        || x.checked_add(width).is_none_or(|right| right > frame_width)
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > frame_height)
    {
        return Err(ProtocolError::invalid(
            "animation rectangle lies outside the frame",
        ));
    }
    Ok(())
}

fn rectangles_overlap(
    source_x: u32,
    source_y: u32,
    destination_x: u32,
    destination_y: u32,
    width: u32,
    height: u32,
) -> bool {
    source_x < destination_x.saturating_add(width)
        && destination_x < source_x.saturating_add(width)
        && source_y < destination_y.saturating_add(height)
        && destination_y < source_y.saturating_add(height)
}

#[allow(clippy::too_many_arguments)]
fn compose_rgba_overlay(
    destination: &mut [u8],
    destination_width: u32,
    destination_height: u32,
    source: &[u8],
    source_width: u32,
    source_height: u32,
    destination_x: u32,
    destination_y: u32,
    replace: bool,
) -> Result<(), ProtocolError> {
    let expected_source = checked_pixel_bytes(source_width, source_height, 4)?;
    let expected_destination = checked_pixel_bytes(destination_width, destination_height, 4)?;
    if source.len() != expected_source || destination.len() != expected_destination {
        return Err(ProtocolError::invalid("invalid animation pixel buffer"));
    }
    let width = source_width.min(destination_width.saturating_sub(destination_x));
    let height = source_height.min(destination_height.saturating_sub(destination_y));
    compose_rgba_rectangle(
        destination,
        destination_width,
        source,
        source_width,
        0,
        0,
        destination_x,
        destination_y,
        width,
        height,
        replace,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compose_rgba_rectangle(
    destination: &mut [u8],
    destination_width: u32,
    source: &[u8],
    source_width: u32,
    source_x: u32,
    source_y: u32,
    destination_x: u32,
    destination_y: u32,
    width: u32,
    height: u32,
    replace: bool,
) {
    let destination_stride = destination_width as usize * 4;
    let source_stride = source_width as usize * 4;
    for row in 0..height as usize {
        for column in 0..width as usize {
            let source_offset =
                (source_y as usize + row) * source_stride + (source_x as usize + column) * 4;
            let destination_offset = (destination_y as usize + row) * destination_stride
                + (destination_x as usize + column) * 4;
            let source_pixel = &source[source_offset..source_offset + 4];
            let destination_pixel = &mut destination[destination_offset..destination_offset + 4];
            if replace {
                destination_pixel.copy_from_slice(source_pixel);
            } else {
                blend_source_over(destination_pixel, source_pixel);
            }
        }
    }
}

fn blend_source_over(destination: &mut [u8], source: &[u8]) {
    let source_alpha = u32::from(source[3]);
    if source_alpha == 0 {
        return;
    }
    if source_alpha == 255 {
        destination.copy_from_slice(source);
        return;
    }
    let destination_alpha = u32::from(destination[3]);
    let inverse_source_alpha = 255 - source_alpha;
    let output_alpha = source_alpha + (destination_alpha * inverse_source_alpha + 127) / 255;
    for channel in 0..3 {
        let source_premultiplied = u32::from(source[channel]) * source_alpha;
        let destination_premultiplied =
            u32::from(destination[channel]) * destination_alpha * inverse_source_alpha / 255;
        destination[channel] =
            ((source_premultiplied + destination_premultiplied + output_alpha / 2) / output_alpha)
                as u8;
    }
    destination[3] = output_alpha as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(value: &[u8]) -> KittyCommand {
        KittyCommand::parse(value).unwrap()
    }

    fn raw_command(controls: &str, pixels: &[u8]) -> KittyCommand {
        command(format!("{controls};{}", STANDARD.encode(pixels)).as_bytes())
    }

    #[test]
    fn raw_query_validates_without_storing() {
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(b"a=q,f=32,s=1,v=1,i=7;AQIDBA=="), false);
        assert!(result.response.unwrap().ok);
        assert!(manager.storage.is_empty());
    }

    #[test]
    fn disabled_file_media_and_animations_report_truthfully() {
        let mut manager = GraphicsManager::new();
        let media = manager.handle_command(command(b"a=q,t=f,i=1;L3RtcC9pbWFnZQ=="), false);
        assert_eq!(media.response.unwrap().error_code, Some("EPERM"));
        assert!(!manager.file_transmission_allowed());
        let animation = manager.handle_command(command(b"a=f,i=1"), false);
        assert_eq!(animation.response.unwrap().error_code, Some("ENOENT"));
    }

    #[test]
    fn animation_frames_load_edit_blend_replace_and_inherit_a_base() {
        let mut manager = GraphicsManager::new();
        let root = [
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
        ];
        manager.handle_command(raw_command("a=t,f=32,s=2,v=2,i=7", &root), false);

        let second = [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255];
        let loaded =
            manager.handle_command(raw_command("a=f,f=32,s=2,v=2,i=7,z=75", &second), false);
        let response = loaded.response.unwrap();
        assert!(response.ok);
        assert_eq!(response.frame_number, Some(2));
        assert!(response.to_escape_sequence().contains(",r=2;OK"));
        assert_eq!(manager.animations[&ImageId(7)].frames[0].gap_ms, 75);

        // Transparent source-over leaves the destination unchanged.
        manager.handle_command(
            raw_command("a=f,f=32,s=1,v=1,i=7,r=2,x=1", &[250, 0, 0, 0]),
            false,
        );
        assert_eq!(
            &manager.frame_pixels(ImageId(7), 1).unwrap()[4..8],
            &[4, 5, 6, 255]
        );

        // Replacement mode copies transparent pixels exactly.
        manager.handle_command(
            raw_command("a=f,f=32,s=1,v=1,i=7,r=2,x=1,X=1", &[250, 0, 0, 0]),
            false,
        );
        assert_eq!(
            &manager.frame_pixels(ImageId(7), 1).unwrap()[4..8],
            &[250, 0, 0, 0]
        );

        // A new frame based on frame two preserves that frame outside the
        // transmitted rectangle and receives the default 40 ms gap.
        let inherited = manager.handle_command(
            raw_command(
                "a=f,f=32,s=1,v=1,i=7,c=2,x=0,y=1,X=1",
                &[200, 201, 202, 255],
            ),
            false,
        );
        assert_eq!(inherited.response.unwrap().frame_number, Some(3));
        let third = manager.frame_pixels(ImageId(7), 2).unwrap();
        assert_eq!(
            &third[..8],
            &manager.frame_pixels(ImageId(7), 1).unwrap()[..8]
        );
        assert_eq!(&third[8..12], &[200, 201, 202, 255]);
        assert_eq!(manager.animations[&ImageId(7)].frames[1].gap_ms, 40);

        let colored = manager.handle_command(
            raw_command(
                "a=f,f=32,s=1,v=1,i=7,x=1,y=1,X=1,Y=16909060,z=-1",
                &[9, 8, 7, 6],
            ),
            false,
        );
        assert_eq!(colored.response.unwrap().frame_number, Some(4));
        let fourth = manager.frame_pixels(ImageId(7), 3).unwrap();
        assert_eq!(&fourth[..4], &[1, 2, 3, 4]);
        assert_eq!(&fourth[12..], &[9, 8, 7, 6]);
        assert_eq!(manager.animations[&ImageId(7)].frames[2].gap_ms, 0);
        assert_eq!(manager.memory_usage(), root.len() + second.len() * 3);

        let oversized = manager.handle_command(
            raw_command(
                "a=f,f=32,s=3,v=1,i=7",
                &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            false,
        );
        assert_eq!(oversized.response.unwrap().error_code, Some("EINVAL"));
        assert_eq!(manager.animations[&ImageId(7)].frame_count(), 4);
    }

    #[test]
    fn animation_frame_chunks_retain_first_command_metadata() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=9", &[0, 0, 0, 255]), false);
        let encoded = STANDARD.encode([10, 20, 30, 255]);
        let split = 4;
        let first = format!("a=f,f=32,s=1,v=1,i=9,z=61,m=1;{}", &encoded[..split]);
        assert!(
            manager
                .handle_command(command(first.as_bytes()), false)
                .response
                .is_none()
        );
        let last = format!("m=0;{}", &encoded[split..]);
        let response = manager
            .handle_command(command(last.as_bytes()), false)
            .response
            .unwrap();
        assert_eq!(response.frame_number, Some(2));
        assert_eq!(manager.animations[&ImageId(9)].frames[0].gap_ms, 61);
        assert_eq!(
            manager.frame_pixels(ImageId(9), 1).unwrap().as_ref(),
            &[10, 20, 30, 255]
        );

        let interrupted = format!(
            "a=f,f=32,s=1,v=1,i=9,m=1;{}",
            &STANDARD.encode([1, 2, 3, 4])[..4]
        );
        manager.handle_command(command(interrupted.as_bytes()), false);
        let error = manager.handle_command(command(b"a=a,i=9,s=1"), false);
        assert_eq!(error.response.unwrap().error_code, Some("EINVAL"));
        assert!(manager.pending_frame.is_none());

        let static_chunk = format!(
            "a=t,f=32,s=1,v=1,i=10,m=1;{}",
            &STANDARD.encode([1, 2, 3, 4])[..4]
        );
        manager.handle_command(command(static_chunk.as_bytes()), false);
        let error =
            manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=9", &[4, 3, 2, 1]), false);
        assert_eq!(error.response.unwrap().error_code, Some("EINVAL"));
        assert!(manager.pending.is_none());
        assert_eq!(manager.animations[&ImageId(9)].frame_count(), 2);
    }

    #[test]
    fn frame_composition_is_bounded_and_supports_replace_and_source_over() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(
            raw_command("a=t,f=32,s=2,v=1,i=11", &[0, 0, 255, 255, 0, 255, 0, 255]),
            false,
        );
        manager.handle_command(
            raw_command("a=f,f=32,s=2,v=1,i=11,X=1", &[255, 0, 0, 128, 0, 0, 0, 0]),
            false,
        );
        let missing = manager.handle_command(command(b"a=c,f=0,i=11,c=1"), false);
        assert_eq!(missing.response.unwrap().error_code, Some("ENOENT"));
        let composed = manager.handle_command(
            command(b"a=c,f=0,i=11,r=2,c=1,w=1,h=1,X=0,Y=0,x=0,y=0"),
            false,
        );
        assert!(composed.response.unwrap().ok);
        let root = manager.frame_pixels(ImageId(11), 0).unwrap();
        assert_eq!(root[3], 255);
        assert!(root[0] >= 127 && root[2] >= 127);

        manager.handle_command(
            command(b"a=c,f=0,i=11,r=2,c=1,w=1,h=1,X=1,Y=0,x=1,y=0,C=1"),
            false,
        );
        assert_eq!(
            &manager.frame_pixels(ImageId(11), 0).unwrap()[4..8],
            &[0, 0, 0, 0]
        );

        let overlap = manager.handle_command(
            command(b"a=c,f=0,i=11,r=1,c=1,w=1,h=1,X=0,Y=0,x=0,y=0"),
            false,
        );
        assert_eq!(overlap.response.unwrap().error_code, Some("EINVAL"));
        let overflow = manager.handle_command(
            command(b"a=c,f=0,i=11,r=2,c=1,w=2,h=1,X=1,Y=0,x=0,y=0"),
            false,
        );
        assert_eq!(overflow.response.unwrap().error_code, Some("EINVAL"));
    }

    #[test]
    fn animation_control_uses_exact_deadlines_loading_and_finite_loops() {
        let mut manager = GraphicsManager::new();
        let started = Instant::now();
        manager.handle_command_at(
            raw_command("a=T,f=32,s=1,v=1,i=13,c=1,r=1", &[1, 2, 3, 255]),
            false,
            started,
        );
        manager.handle_command_at(
            raw_command("a=f,f=32,s=1,v=1,i=13,z=-1", &[4, 5, 6, 255]),
            false,
            started,
        );
        manager.handle_command_at(
            raw_command("a=f,f=32,s=1,v=1,i=13,z=20", &[7, 8, 9, 255]),
            false,
            started,
        );
        assert!(
            manager
                .handle_command_at(command(b"a=a,i=13,r=1,z=10,s=3,v=3"), false, started)
                .response
                .is_none()
        );
        assert_eq!(
            manager.next_animation_deadline(false),
            Some(started + Duration::from_millis(10))
        );
        assert_eq!(manager.next_animation_deadline(true), None);
        manager.handle_command_at(
            command(b"a=a,i=13,s=3"),
            false,
            started + Duration::from_millis(5),
        );
        assert_eq!(
            manager.next_animation_deadline(false),
            Some(started + Duration::from_millis(10))
        );
        assert!(!manager.advance_animations(false, started + Duration::from_millis(9)));
        assert!(manager.advance_animations(false, started + Duration::from_millis(10)));
        assert_eq!(manager.animations[&ImageId(13)].current_frame, 2);
        assert_eq!(
            manager.next_animation_deadline(false),
            Some(started + Duration::from_millis(30))
        );
        assert!(manager.advance_animations(false, started + Duration::from_millis(30)));
        assert_eq!(manager.animations[&ImageId(13)].current_frame, 0);
        assert!(manager.advance_animations(false, started + Duration::from_millis(40)));
        assert_eq!(manager.animations[&ImageId(13)].current_frame, 2);
        assert_eq!(manager.next_animation_deadline(false), None);

        manager.handle_command_at(command(b"a=a,i=13,c=1,s=2"), false, started);
        assert!(manager.advance_animations(false, started + Duration::from_millis(10)));
        assert_eq!(manager.animations[&ImageId(13)].current_frame, 2);
        assert_eq!(manager.next_animation_deadline(false), None);
    }

    #[test]
    fn frame_deletion_promotes_root_and_uppercase_removes_last_frame_image() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(
            raw_command("a=T,f=32,s=1,v=1,i=15,c=1,r=1", &[1, 1, 1, 255]),
            false,
        );
        manager.handle_command(
            raw_command("a=f,f=32,s=1,v=1,i=15,z=88", &[2, 2, 2, 255]),
            false,
        );
        manager.handle_command(command(b"a=d,d=f,i=15,r=1"), false);
        assert_eq!(
            manager.frame_pixels(ImageId(15), 0).unwrap().as_ref(),
            &[2, 2, 2, 255]
        );
        assert_eq!(manager.animations[&ImageId(15)].root_gap_ms, 88);
        assert_eq!(manager.memory_usage(), 4);
        assert_eq!(manager.image_count(), 1);

        manager.handle_command(command(b"a=d,d=f,i=15"), false);
        assert_eq!(manager.image_count(), 1);
        manager.handle_command(command(b"a=d,d=F,i=15"), false);
        assert_eq!(manager.image_count(), 0);
        assert_eq!(manager.placement_count(), 0);
        assert_eq!(manager.memory_usage(), 0);
    }

    #[test]
    fn animation_frame_count_is_bounded_without_losing_existing_frames() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=17", &[0, 0, 0, 255]), false);
        for value in 1..MAX_ANIMATION_FRAMES_PER_IMAGE {
            let pixel = [value as u8, 0, 0, 255];
            let result =
                manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=17", &pixel), false);
            assert!(result.response.unwrap().ok);
        }
        assert_eq!(
            manager.animations[&ImageId(17)].frame_count(),
            MAX_ANIMATION_FRAMES_PER_IMAGE
        );
        let rejected =
            manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=17", &[9, 9, 9, 255]), false);
        assert_eq!(rejected.response.unwrap().error_code, Some("ENOSPC"));
        assert_eq!(
            manager.animations[&ImageId(17)].frame_count(),
            MAX_ANIMATION_FRAMES_PER_IMAGE
        );
    }

    #[test]
    fn replacing_an_image_id_releases_all_animation_storage() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=19", &[1, 2, 3, 4]), false);
        manager.handle_command(raw_command("a=f,f=32,s=1,v=1,i=19", &[5, 6, 7, 8]), false);
        assert_eq!(manager.memory_usage(), 8);
        assert!(manager.animations.contains_key(&ImageId(19)));

        manager.handle_command(
            raw_command("a=t,f=32,s=1,v=1,i=19", &[9, 10, 11, 12]),
            false,
        );
        assert_eq!(manager.memory_usage(), 4);
        assert!(!manager.animations.contains_key(&ImageId(19)));
    }

    #[cfg(unix)]
    #[test]
    fn animation_frames_use_the_supported_temporary_file_medium() {
        use std::io::Write;

        let mut manager = GraphicsManager::new();
        manager.handle_command(raw_command("a=t,f=32,s=1,v=1,i=21", &[1, 2, 3, 4]), false);
        let mut file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-animation-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        file.write_all(&[5, 6, 7, 8]).unwrap();
        file.flush().unwrap();
        let path = file.path().to_path_buf();
        let payload = format!("a=f,t=t,f=32,s=1,v=1,i=21;{}", encoded_path(&path));
        let response = manager
            .handle_command(command(payload.as_bytes()), false)
            .response
            .unwrap();
        assert!(response.ok);
        assert_eq!(response.frame_number, Some(2));
        assert_eq!(
            manager.frame_pixels(ImageId(21), 1).unwrap().as_ref(),
            &[5, 6, 7, 8]
        );
        assert!(!path.exists());
    }

    #[cfg(unix)]
    fn encoded_path(path: &std::path::Path) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use std::os::unix::ffi::OsStrExt;

        STANDARD.encode(path.as_os_str().as_bytes())
    }

    #[cfg(unix)]
    struct TestSharedMemory {
        name: std::ffi::CString,
        file: std::fs::File,
    }

    #[cfg(unix)]
    impl TestSharedMemory {
        fn new(data: &[u8]) -> Self {
            #[cfg(not(target_vendor = "apple"))]
            use std::io::Write;
            #[cfg(target_vendor = "apple")]
            use std::os::unix::io::AsRawFd;
            use std::os::unix::io::FromRawFd;
            use std::sync::atomic::{AtomicU64, Ordering};

            static NEXT_NAME: AtomicU64 = AtomicU64::new(1);
            loop {
                let sequence = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
                let name = std::ffi::CString::new(format!(
                    "/termeric-kitty-test-{}-{sequence}",
                    std::process::id()
                ))
                .unwrap();
                // SAFETY: `name` is a live C string, creation uses private
                // permissions, and a successful descriptor is transferred
                // exactly once into `File` ownership below.
                let descriptor = unsafe {
                    libc::shm_open(
                        name.as_ptr(),
                        libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_CLOEXEC,
                        0o600,
                    )
                };
                if descriptor < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                        continue;
                    }
                    panic!(
                        "could not create test shared memory: {}",
                        std::io::Error::last_os_error()
                    );
                }
                // SAFETY: this is the sole owner transfer for the newly
                // returned descriptor.
                let file = unsafe { std::fs::File::from_raw_fd(descriptor) };
                file.set_len(data.len() as u64).unwrap();
                #[cfg(not(target_vendor = "apple"))]
                {
                    let mut writer = &file;
                    writer.write_all(data).unwrap();
                    writer.flush().unwrap();
                }
                #[cfg(target_vendor = "apple")]
                if !data.is_empty() {
                    // SAFETY: the private object has just been sized to the
                    // mapping length and remains open for the mapping lifetime.
                    let address = unsafe {
                        libc::mmap(
                            std::ptr::null_mut(),
                            data.len(),
                            libc::PROT_READ | libc::PROT_WRITE,
                            libc::MAP_SHARED,
                            file.as_raw_fd(),
                            0,
                        )
                    };
                    assert_ne!(address, libc::MAP_FAILED);
                    // SAFETY: both regions cover exactly `data.len()` bytes
                    // and cannot overlap; the mapping is released afterwards.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr(),
                            address as *mut u8,
                            data.len(),
                        );
                        libc::munmap(address, data.len());
                    }
                }
                return Self { name, file };
            }
        }

        fn encoded_name(&self) -> String {
            use base64::{Engine as _, engine::general_purpose::STANDARD};

            STANDARD.encode(self.name.as_bytes())
        }

        fn is_linked(&self) -> bool {
            use std::os::unix::io::FromRawFd;

            // SAFETY: `name` is a live C string and any successful descriptor
            // is immediately transferred to `File` for closure.
            let descriptor =
                unsafe { libc::shm_open(self.name.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC, 0) };
            if descriptor < 0 {
                false
            } else {
                // SAFETY: this is the sole owner transfer for the descriptor.
                drop(unsafe { std::fs::File::from_raw_fd(descriptor) });
                true
            }
        }
    }

    #[cfg(unix)]
    impl Drop for TestSharedMemory {
        fn drop(&mut self) {
            // SAFETY: `name` remains a live C string. Failure is expected when
            // the graphics handler already performed the required unlink.
            unsafe {
                libc::shm_unlink(self.name.as_ptr());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn temporary_file_range_is_loaded_deleted_and_queryable() {
        use std::io::Write;

        let mut image_file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        image_file
            .write_all(&[90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        image_file.flush().unwrap();
        let image_path = image_file.path().to_path_buf();
        let payload = format!(
            "a=T,t=t,f=32,s=2,v=1,S=8,O=4,i=41,c=2,r=1,C=1;{}",
            encoded_path(&image_path)
        );

        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(!image_path.exists());
        assert_eq!(
            manager.get_image(ImageId(41)).unwrap().data.as_ref(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );

        let mut query_file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        query_file.write_all(&[9, 8, 7, 6]).unwrap();
        query_file.flush().unwrap();
        let query_path = query_file.path().to_path_buf();
        let query = format!(
            "a=q,t=t,f=32,s=1,v=1,S=4,i=42;{}",
            encoded_path(&query_path)
        );
        let result = manager.handle_command(command(query.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(!query_path.exists());
        assert!(manager.get_image(ImageId(42)).is_none());

        let mut unsupported_file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        unsupported_file.write_all(&[1, 2, 3, 4]).unwrap();
        unsupported_file.flush().unwrap();
        let unsupported_path = unsupported_file.path().to_path_buf();
        let unsupported = format!(
            "a=q,t=t,f=32,s=1,v=1,S=4,o=z,i=48;{}",
            encoded_path(&unsupported_path)
        );
        let result = manager.handle_command(command(unsupported.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
        assert!(!unsupported_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn opted_in_ordinary_file_range_is_loaded_without_deletion() {
        use std::io::Write;

        let mut image_file = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
        image_file
            .write_all(&[90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        image_file.flush().unwrap();
        let image_path = image_file.path().to_path_buf();
        let payload = format!(
            "a=T,t=f,f=32,s=2,v=1,S=8,O=4,i=59,c=2,r=1,C=1;{}",
            encoded_path(&image_path)
        );

        let mut manager = GraphicsManager::new();
        manager.set_file_transmission_allowed(true);
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(manager.file_transmission_allowed());
        assert!(image_path.exists());
        assert_eq!(
            manager.get_image(ImageId(59)).unwrap().data.as_ref(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );

        let query = format!(
            "a=q,t=f,f=32,s=1,v=1,S=4,O=4,i=60;{}",
            encoded_path(&image_path)
        );
        let result = manager.handle_command(command(query.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(image_path.exists());
        assert!(manager.get_image(ImageId(60)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn opted_in_ordinary_files_follow_safe_symlinks_but_reject_unsafe_targets() {
        use std::io::Write;

        let mut target = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
        target.write_all(&[1, 2, 3, 4]).unwrap();
        target.flush().unwrap();
        let directory = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let link = directory.path().join("image-link");
        std::os::unix::fs::symlink(target.path(), &link).unwrap();

        let mut manager = GraphicsManager::new();
        manager.set_file_transmission_allowed(true);
        let payload = format!("a=q,t=f,f=32,s=1,v=1,S=4,i=61;{}", encoded_path(&link));
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(link.exists());
        assert!(target.path().exists());

        let relative = command(b"a=q,t=f,f=32,s=1,v=1,S=4,i=62;aW1hZ2U=");
        let result = manager.handle_command(relative, false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));

        let directory_payload = format!(
            "a=q,t=f,f=32,s=1,v=1,S=4,i=63;{}",
            encoded_path(directory.path())
        );
        let result = manager.handle_command(command(directory_payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));

        let other = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
        let replacement =
            ensure_ordinary_file_unchanged(target.path(), &other.as_file().metadata().unwrap())
                .unwrap_err();
        assert_eq!(replacement.code, "EINVAL");

        if Path::new("/proc/self/status").exists() {
            let protected = format!(
                "a=q,t=f,f=32,s=1,v=1,S=4,i=64;{}",
                encoded_path(Path::new("/proc/self/status"))
            );
            let result = manager.handle_command(command(protected.as_bytes()), false);
            assert_eq!(result.response.unwrap().error_code, Some("EPERM"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn oversized_ordinary_file_is_rejected_without_deletion() {
        let file = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
        file.as_file()
            .set_len(MAX_TRANSMISSION_BYTES as u64 + 1)
            .unwrap();
        let path = file.path().to_path_buf();
        let payload = format!("a=q,t=f,f=32,s=1,v=1,i=65;{}", encoded_path(&path));

        let mut manager = GraphicsManager::new();
        manager.set_file_transmission_allowed(true);
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
        assert!(path.exists());
        assert!(manager.storage.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn local_media_policy_rejects_unsafe_paths_and_special_files() {
        let ordinary = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        let ordinary_path = ordinary.path().to_path_buf();
        let payload = format!("a=q,t=f,f=32,s=1,v=1,i=43;{}", encoded_path(&ordinary_path));
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EPERM"));
        assert!(ordinary_path.exists());

        let unmarked = tempfile::Builder::new()
            .prefix("termeric-image-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        let unmarked_path = unmarked.path().to_path_buf();
        let payload = format!("a=q,t=t,f=32,s=1,v=1,i=44;{}", encoded_path(&unmarked_path));
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
        assert!(unmarked_path.exists());

        let directory = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let directory_path = directory.path().to_path_buf();
        let payload = format!(
            "a=q,t=t,f=32,s=1,v=1,i=45;{}",
            encoded_path(&directory_path)
        );
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
        assert!(directory_path.exists());

        let target = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
        let symlink_seed = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        let symlink_path = symlink_seed.path().to_path_buf();
        drop(symlink_seed);
        std::os::unix::fs::symlink(target.path(), &symlink_path).unwrap();
        let payload = format!("a=q,t=t,f=32,s=1,v=1,i=46;{}", encoded_path(&symlink_path));
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
        assert!(symlink_path.exists());
        std::fs::remove_file(&symlink_path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn oversized_safe_temporary_file_is_rejected_and_removed() {
        let file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        file.as_file()
            .set_len(MAX_TRANSMISSION_BYTES as u64 + 1)
            .unwrap();
        let path = file.path().to_path_buf();
        let payload = format!("a=q,t=t,f=32,s=1,v=1,i=47;{}", encoded_path(&path));

        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
        assert!(!path.exists());
        assert!(manager.storage.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn shared_memory_range_is_loaded_unlinked_and_queryable() {
        let image = TestSharedMemory::new(&[90, 91, 92, 93, 1, 2, 3, 4, 5, 6, 7, 8]);
        let payload = format!(
            "a=T,t=s,f=32,s=2,v=1,S=8,O=4,i=49,c=2,r=1,C=1;{}",
            image.encoded_name()
        );
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(!image.is_linked());
        assert_eq!(
            manager.get_image(ImageId(49)).unwrap().data.as_ref(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );

        let query = TestSharedMemory::new(&[9, 8, 7, 6]);
        let payload = format!("a=q,t=s,f=32,s=1,v=1,S=4,i=50;{}", query.encoded_name());
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(!query.is_linked());
        assert!(manager.get_image(ImageId(50)).is_none());

        let unsupported = TestSharedMemory::new(&[1, 2, 3, 4]);
        let payload = format!(
            "a=q,t=s,f=32,s=1,v=1,S=4,o=z,i=51;{}",
            unsupported.encoded_name()
        );
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
        assert!(!unsupported.is_linked());
    }

    #[cfg(unix)]
    #[test]
    fn compressed_local_media_uses_format_specific_size_semantics() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let raw_pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&raw_pixels).unwrap();
        let compressed_raw = compressor.finish().unwrap();
        let mut raw_file = tempfile::Builder::new()
            .prefix("tty-graphics-protocol-")
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        raw_file.write_all(&compressed_raw).unwrap();
        raw_file.flush().unwrap();
        let raw_path = raw_file.path().to_path_buf();
        let payload = format!(
            "a=t,t=t,f=32,s=2,v=1,S={},o=z,i=55;{}",
            compressed_raw.len(),
            encoded_path(&raw_path)
        );
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(!raw_path.exists());
        assert_eq!(
            manager.get_image(ImageId(55)).unwrap().data.as_ref(),
            raw_pixels
        );

        let mut ordinary_raw = tempfile::NamedTempFile::new_in(std::env::temp_dir()).unwrap();
        ordinary_raw.write_all(&compressed_raw).unwrap();
        ordinary_raw.flush().unwrap();
        let ordinary_path = ordinary_raw.path().to_path_buf();
        manager.set_file_transmission_allowed(true);
        let payload = format!(
            "a=t,t=f,f=32,s=2,v=1,S={},o=z,i=66;{}",
            compressed_raw.len(),
            encoded_path(&ordinary_path)
        );
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(ordinary_path.exists());
        assert_eq!(
            manager.get_image(ImageId(66)).unwrap().data.as_ref(),
            raw_pixels
        );

        let mut png_data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_data, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[9, 8, 7, 6]).unwrap();
        }
        png_data.extend(std::iter::repeat_n(0, 4_096));
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::best());
        compressor.write_all(&png_data).unwrap();
        let compressed_png = compressor.finish().unwrap();
        assert!(compressed_png.len() < png_data.len());
        let png_object = TestSharedMemory::new(&compressed_png);
        let payload = format!(
            "a=t,t=s,f=100,S={},o=z,i=56;{}",
            png_data.len(),
            png_object.encoded_name()
        );
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
        assert!(!png_object.is_linked());
        assert_eq!(
            manager.get_image(ImageId(56)).unwrap().data.as_ref(),
            [9, 8, 7, 6]
        );

        let oversized = TestSharedMemory::new(&[0]);
        let payload = format!(
            "a=q,t=s,f=100,S={},o=z,i=57;{}",
            MAX_TRANSMISSION_BYTES as u64 + 1,
            oversized.encoded_name()
        );
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
        assert!(!oversized.is_linked());

        let direct_png = STANDARD.encode(compressed_png);
        let payload = format!("a=q,f=100,S={},o=z,i=58;{direct_png}", png_data.len());
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert!(result.response.unwrap().ok);
    }

    #[cfg(unix)]
    #[test]
    fn shared_memory_policy_rejects_public_and_invalid_names() {
        use std::os::unix::fs::PermissionsExt;

        let public = TestSharedMemory::new(&[1, 2, 3, 4]);
        public
            .file
            .set_permissions(std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let payload = format!("a=q,t=s,f=32,s=1,v=1,S=4,i=52;{}", public.encoded_name());
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
        assert!(public.is_linked());

        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let invalid = STANDARD.encode(b"/nested/name");
        let payload = format!("a=q,t=s,f=32,s=1,v=1,S=4,i=53;{invalid}");
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    }

    #[cfg(unix)]
    #[test]
    fn oversized_private_shared_memory_is_rejected_and_unlinked() {
        let object = TestSharedMemory::new(&[]);
        object
            .file
            .set_len(MAX_TRANSMISSION_BYTES as u64 + 1)
            .unwrap();
        let payload = format!("a=q,t=s,f=32,s=1,v=1,i=54;{}", object.encoded_name());

        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(payload.as_bytes()), false);
        assert_eq!(result.response.unwrap().error_code, Some("E2BIG"));
        assert!(!object.is_linked());
        assert!(manager.storage.is_empty());
    }

    #[test]
    fn chunk_continuation_without_repeated_id_works() {
        let mut manager = GraphicsManager::new();
        assert!(
            manager
                .handle_command(command(b"a=t,f=32,s=2,v=1,i=3,m=1;AQID"), false)
                .response
                .is_none()
        );
        let result = manager.handle_command(command(b"m=0;BAUGBwg="), false);
        assert!(result.response.unwrap().ok);
        assert_eq!(manager.get_image(ImageId(3)).unwrap().data.len(), 8);
    }

    #[test]
    fn quiet_policy_and_placement_replacement_are_enforced() {
        let mut manager = GraphicsManager::new();
        let result =
            manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=2,p=4,q=1,C=1;AQIDBA=="), false);
        assert!(result.response.is_none());
        manager.handle_command(command(b"a=p,i=2,p=4,C=1"), false);
        assert_eq!(manager.placements(false).count(), 1);
    }

    #[test]
    fn png_and_zlib_are_decoded_within_bounds() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3, 4]).unwrap();
        }
        let encoded = STANDARD.encode(&png_bytes);
        let mut manager = GraphicsManager::new();
        let payload = format!("a=t,f=100,i=8;{encoded}");
        assert!(
            manager
                .handle_command(command(payload.as_bytes()), false)
                .response
                .unwrap()
                .ok
        );
        assert_eq!(
            manager.get_image(ImageId(8)).unwrap().data.as_ref(),
            [1, 2, 3, 4]
        );

        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&[9, 8, 7, 6]).unwrap();
        let encoded = STANDARD.encode(compressor.finish().unwrap());
        let payload = format!("a=q,f=32,s=1,v=1,o=z,i=9;{encoded}");
        assert!(
            manager
                .handle_command(command(payload.as_bytes()), false)
                .response
                .unwrap()
                .ok
        );
    }

    #[test]
    fn checked_dimensions_reject_overflow_before_allocation() {
        let mut manager = GraphicsManager::new();
        let result =
            manager.handle_command(command(b"a=q,f=32,s=4294967295,v=4294967295,i=1;"), false);
        assert_eq!(result.response.unwrap().error_code, Some("EINVAL"));
    }

    #[test]
    fn anonymous_images_coexist_and_retransmission_invalidates_content() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(command(b"a=T,f=32,s=1,v=1,C=1;AQIDBA=="), false);
        manager.handle_command(command(b"a=T,f=32,s=1,v=1,C=1;BQYHCA=="), false);
        assert_eq!(manager.placements(false).count(), 2);

        manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=4,C=1;AQIDBA=="), false);
        let first_revision = manager.get_image(ImageId(4)).unwrap().revision;
        manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=4,C=1;BQYHCA=="), false);
        assert!(manager.get_image(ImageId(4)).unwrap().revision > first_revision);
        assert_eq!(
            manager
                .placements(false)
                .filter(|placement| placement.image_id == ImageId(4))
                .count(),
            1
        );
    }

    #[test]
    fn unicode_virtual_placements_are_invisible_bounded_prototypes() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(command(b"a=t,i=42,f=32,s=1,v=1;AQIDBA=="), false);
        let placed = manager.handle_command(command(b"a=p,U=1,i=42,p=7,c=2,r=3"), false);
        assert_eq!(placed.cursor_movement, None);
        assert!(placed.response.unwrap().ok);
        assert_eq!(manager.placements(false).count(), 0);
        assert_eq!(manager.placement_count(), 1);

        // Coordinate and all-visible deletion selectors cannot affect an
        // invisible virtual prototype.
        manager.handle_command(command(b"a=d,d=a"), false);
        manager.handle_command(command(b"a=d,d=z,z=-1"), false);
        assert_eq!(manager.virtual_placements.len(), 1);

        // Reusing an explicit placement ID updates rather than duplicates the
        // virtual placement, while anonymous prototypes remain independently
        // bounded.
        manager.handle_command(command(b"a=p,U=1,i=42,p=7,c=4,r=5"), false);
        assert_eq!(manager.virtual_placements.len(), 1);
        assert_eq!(manager.virtual_placements[0].columns, Some(4));
        manager.handle_command(command(b"a=d,d=i,i=42,p=7"), false);
        assert!(manager.virtual_placements.is_empty());
        assert!(manager.get_image(ImageId(42)).is_some());

        for placement_id in 1..=MAX_VIRTUAL_IMAGE_PLACEMENTS + 1 {
            let payload = format!("a=p,U=1,i=42,p={placement_id},c=1,r=1");
            manager.handle_command(command(payload.as_bytes()), false);
        }
        assert_eq!(
            manager.virtual_placements.len(),
            MAX_VIRTUAL_IMAGE_PLACEMENTS
        );
        assert_eq!(
            manager.virtual_placements.front().unwrap().placement_id,
            Some(2)
        );

        manager.handle_command(command(b"a=t,i=42,f=32,s=1,v=1;BQYHCA=="), false);
        assert!(manager.virtual_placements.is_empty());
    }

    #[test]
    fn unicode_virtual_placements_are_pruned_with_storage_eviction() {
        let mut manager = GraphicsManager::new();
        manager.storage = ImageStorage::new(1, 1024);
        manager.handle_command(command(b"a=t,i=42,f=32,s=1,v=1;AQIDBA=="), false);
        manager.handle_command(command(b"a=p,U=1,i=42,c=1,r=1"), false);
        assert_eq!(manager.virtual_placements.len(), 1);
        manager.handle_command(command(b"a=t,i=43,f=32,s=1,v=1;BQYHCA=="), false);
        assert!(manager.get_image(ImageId(42)).is_none());
        assert!(manager.virtual_placements.is_empty());
    }

    #[test]
    fn unicode_virtual_placements_follow_number_and_range_deletion() {
        let mut manager = GraphicsManager::new();
        let numbered = manager.handle_command(command(b"a=t,I=13,f=32,s=1,v=1;AQIDBA=="), false);
        let numbered_id = ImageId(numbered.response.unwrap().image_id);
        manager.handle_command(command(b"a=p,U=1,I=13,p=7,c=1,r=1"), false);
        manager.handle_command(command(b"a=d,d=n,I=13,p=7"), false);
        assert!(manager.virtual_placements.is_empty());
        assert!(manager.get_image(numbered_id).is_some());
        manager.handle_command(command(b"a=p,U=1,I=13,p=7,c=1,r=1"), false);
        manager.handle_command(command(b"a=d,d=N,I=13,p=7"), false);
        assert!(manager.get_image(numbered_id).is_none());

        for id in [40, 41] {
            let payload = format!("a=t,i={id},f=32,s=1,v=1;AQIDBA==");
            manager.handle_command(command(payload.as_bytes()), false);
            let payload = format!("a=p,U=1,i={id},c=1,r=1");
            manager.handle_command(command(payload.as_bytes()), false);
        }
        manager.handle_command(command(b"a=d,d=R,x=40,y=40"), false);
        assert!(manager.get_image(ImageId(40)).is_none());
        assert!(manager.get_image(ImageId(41)).is_some());
        assert_eq!(manager.virtual_placements.len(), 1);
        assert_eq!(manager.virtual_placements[0].image_id, ImageId(41));
    }

    #[test]
    fn unicode_placeholder_presentations_fit_and_crop_each_visible_cell() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(
            command(b"a=t,i=42,f=32,s=2,v=2;AQIDBAUGBwgJCgsMDQ4PEA=="),
            false,
        );
        manager.handle_command(command(b"a=p,U=1,i=42,p=7,c=2,r=2"), false);
        let mut grid = Grid::new(3, 2);
        for row in 0..2 {
            for col in 0..2 {
                let cell = grid.cell_mut(row, col).unwrap();
                cell.set_grapheme(match (row, col) {
                    (0, 0) => "\u{10eeee}\u{0305}\u{0305}",
                    (0, 1) => "\u{10eeee}",
                    (1, 0) => "\u{10eeee}\u{030d}",
                    _ => "\u{10eeee}",
                });
                cell.fg_spec = crate::grid::ColorSpec::Indexed(42);
                cell.underline_color = Some(crate::grid::Color::rgb(1, 2, 3));
                cell.underline_color_spec = Some(crate::grid::ColorSpec::Indexed(7));
            }
        }

        let presentations = manager.presentation_images(&grid, false, 1, 1);
        assert_eq!(presentations.len(), 4);
        for (index, presentation) in presentations.iter().enumerate() {
            assert_eq!(presentation.placement.placement_id, Some(7));
            assert_eq!(presentation.placement.z_index, -1);
            assert_eq!(presentation.pixel_rect, Some([0.0, 0.0, 1.0, 1.0]));
            let col = (index % 2) as f32;
            let row = (index / 2) as f32;
            assert_eq!(
                presentation.source_uv,
                Some([col / 2.0, row / 2.0, (col + 1.0) / 2.0, (row + 1.0) / 2.0])
            );
        }

        let prototype = VirtualImagePlacement {
            uid: PlacementUid(1),
            image_id: ImageId(42),
            placement_id: None,
            columns: Some(1),
            rows: Some(1),
            alternate_screen: false,
        };
        let letterboxed = placeholder_presentation(
            manager.get_image(ImageId(42)).unwrap(),
            &prototype,
            0,
            0,
            0,
            0,
            2,
            4,
        )
        .unwrap();
        assert_eq!(letterboxed.pixel_rect, Some([0.0, 1.0, 2.0, 2.0]));
        assert_eq!(letterboxed.source_uv, Some([0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn relative_placements_follow_named_parents_and_delete_as_groups() {
        let mut manager = GraphicsManager::new();
        for image_id in 1..=3 {
            let payload = format!("a=t,i={image_id},f=32,s=1,v=1;AQIDBA==");
            assert!(
                manager
                    .handle_command(command(payload.as_bytes()), false)
                    .response
                    .unwrap()
                    .ok
            );
        }
        let mut parent = command(b"a=p,i=1,p=1,c=1,r=1,C=1");
        parent.placement.col = Some(2);
        parent.placement.row = Some(3);
        manager.handle_command(parent, false);

        let child = manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=-1,V=2,c=1,r=1"), false);
        assert_eq!(child.cursor_movement, None);
        assert!(child.response.unwrap().ok);
        assert!(
            manager
                .handle_command(command(b"a=p,i=3,p=3,P=2,Q=2,H=4,V=-1,c=1,r=1"), false,)
                .response
                .unwrap()
                .ok
        );

        let grid = Grid::new(12, 8);
        let positions = |manager: &GraphicsManager| {
            manager
                .presentation_images(&grid, false, 1, 1)
                .into_iter()
                .map(|image| {
                    (
                        image.placement.image_id,
                        image.placement.col,
                        image.placement.row,
                    )
                })
                .collect::<HashSet<_>>()
        };
        assert_eq!(
            positions(&manager),
            HashSet::from([(ImageId(1), 2, 3), (ImageId(2), 1, 5), (ImageId(3), 5, 4),])
        );

        let mut moved_parent = command(b"a=p,i=1,p=1,c=1,r=1,C=1");
        moved_parent.placement.col = Some(6);
        moved_parent.placement.row = Some(1);
        manager.handle_command(moved_parent, false);
        assert_eq!(
            positions(&manager),
            HashSet::from([(ImageId(1), 6, 1), (ImageId(2), 5, 3), (ImageId(3), 9, 2),])
        );

        let cycle = manager.handle_command(command(b"a=p,i=1,p=1,P=3,Q=3,c=1,r=1,C=1"), false);
        assert_eq!(cycle.response.unwrap().error_code, Some("ECYCLE"));
        let missing = manager.handle_command(command(b"a=p,i=1,p=9,P=99,Q=1"), false);
        assert_eq!(missing.response.unwrap().error_code, Some("ENOPARENT"));
        assert!(positions(&manager).contains(&(ImageId(1), 6, 1)));

        manager.handle_command(command(b"a=d,d=i,i=2,p=2"), false);
        assert_eq!(positions(&manager), HashSet::from([(ImageId(1), 6, 1)]));
        assert!(manager.get_image(ImageId(2)).is_some());
        assert!(manager.get_image(ImageId(3)).is_none());
    }

    #[test]
    fn relative_placements_clip_negative_columns_and_reject_virtual_children() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;AQIDBA=="), false);
        manager.handle_command(command(b"a=t,i=2,f=32,s=2,v=1;AQIDBAUGBwg="), false);
        manager.handle_command(command(b"a=p,i=1,p=1,c=1,r=1,C=1"), false);
        manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=-1,c=2,r=1"), false);
        let grid = Grid::new(4, 2);
        let child = manager
            .presentation_images(&grid, false, 1, 1)
            .into_iter()
            .find(|image| image.placement.image_id == ImageId(2))
            .unwrap();
        assert_eq!((child.placement.col, child.placement.width), (0, Some(2)));
        assert_eq!(child.pixel_rect, Some([-1.0, 0.0, 2.0, 1.0]));
        assert_eq!(child.source_uv, Some([0.0, 0.0, 1.0, 1.0]));

        manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=-1"), false);
        let natural = manager
            .presentation_images(&grid, false, 1, 1)
            .into_iter()
            .find(|image| image.placement.image_id == ImageId(2))
            .unwrap();
        assert_eq!(natural.placement.width, None);
        assert_eq!(natural.pixel_rect, Some([-1.0, 0.0, 2.0, 1.0]));

        let invalid = manager.handle_command(command(b"a=p,i=2,p=3,U=1,P=1,Q=1,c=1,r=1"), false);
        assert_eq!(invalid.response.unwrap().error_code, Some("EINVAL"));
    }

    #[test]
    fn relative_placement_depth_is_bounded_above_the_required_minimum() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;AQIDBA=="), false);
        manager.handle_command(command(b"a=p,i=1,p=1,c=1,r=1,C=1"), false);
        for placement_id in 2..=MAX_RELATIVE_PLACEMENT_DEPTH as u32 + 1 {
            let payload = format!(
                "a=p,i=1,p={placement_id},P=1,Q={},H=1,c=1,r=1",
                placement_id - 1
            );
            assert!(
                manager
                    .handle_command(command(payload.as_bytes()), false)
                    .response
                    .unwrap()
                    .ok
            );
        }
        let too_deep = format!(
            "a=p,i=1,p={},P=1,Q={},H=1,c=1,r=1",
            MAX_RELATIVE_PLACEMENT_DEPTH + 2,
            MAX_RELATIVE_PLACEMENT_DEPTH + 1
        );
        let response = manager
            .handle_command(command(too_deep.as_bytes()), false)
            .response
            .unwrap();
        assert_eq!(response.error_code, Some("ETOODEEP"));
    }

    #[test]
    fn relative_placements_follow_scrolling_and_screen_ownership() {
        let mut manager = GraphicsManager::new();
        for image_id in 1..=2 {
            let payload = format!("a=t,i={image_id},f=32,s=1,v=1;AQIDBA==");
            manager.handle_command(command(payload.as_bytes()), false);
        }
        let mut parent = command(b"a=p,i=1,p=1,c=1,r=1,C=1");
        parent.placement.col = Some(2);
        parent.placement.row = Some(3);
        manager.handle_command(parent, false);
        manager.handle_command(command(b"a=p,i=2,p=2,P=1,Q=1,H=1,V=1,c=1,r=1"), false);
        let grid = Grid::new(8, 6);
        manager.scroll_up(false, 0..6, 0..8, 1, true, 8);
        let child = manager
            .presentation_images(&grid, false, 1, 1)
            .into_iter()
            .find(|image| image.placement.image_id == ImageId(2))
            .unwrap();
        assert_eq!((child.placement.col, child.placement.row), (3, 3));

        let cross_screen = manager.handle_command(command(b"a=p,i=2,p=9,P=1,Q=1,c=1,r=1"), true);
        assert_eq!(cross_screen.response.unwrap().error_code, Some("ENOPARENT"));

        manager.clear_screen(false);
        assert!(manager.presentation_images(&grid, false, 1, 1).is_empty());
    }

    #[test]
    fn relative_descendants_are_pruned_when_the_parent_hits_the_placement_cap() {
        let mut manager = GraphicsManager::new();
        for image_id in 1..=2 {
            let payload = format!("a=t,i={image_id},f=32,s=1,v=1;AQIDBA==");
            manager.handle_command(command(payload.as_bytes()), false);
        }
        manager.handle_command(command(b"a=p,i=1,p=1,c=1,r=1,C=1"), false);
        manager.handle_command(command(b"a=p,i=2,p=1,P=1,Q=1,c=1,r=1,C=1"), false);
        for placement_id in 2..=MAX_IMAGE_PLACEMENTS as u32 {
            let payload = format!("a=p,i=1,p={placement_id},c=1,r=1,C=1");
            manager.handle_command(command(payload.as_bytes()), false);
        }
        assert!(manager.placement_count() <= MAX_IMAGE_PLACEMENTS);
        assert!(manager.get_image(ImageId(2)).is_none());
        assert!(
            manager
                .placements(false)
                .all(|placement| placement.image_id != ImageId(2))
        );
    }

    #[test]
    fn image_numbers_allocate_ids_resolve_newest_and_fall_back_after_deletion() {
        let mut manager = GraphicsManager::new();
        let first = manager.handle_command(command(b"a=t,I=13,N=1,f=32,s=1,v=1;AQIDBA=="), false);
        let first = first.response.unwrap();
        assert!(first.ok);
        assert_eq!(first.image_id, 1);
        assert_eq!(first.image_number, Some(13));
        assert_eq!(first.to_escape_sequence(), "\x1b_Gi=1,I=13;OK\x1b\\");

        let second = manager.handle_command(command(b"a=t,I=13,f=32,s=1,v=1;BQYHCA=="), false);
        let second = second.response.unwrap();
        assert_eq!(second.image_id, 2);
        assert_eq!(manager.image_count(), 2);
        assert_eq!(manager.newest_image_with_number(13), Some(ImageId(2)));

        let displayed = manager.handle_command(command(b"a=p,I=13,p=7,c=1,r=1,C=1"), false);
        let displayed = displayed.response.unwrap();
        assert_eq!((displayed.image_id, displayed.image_number), (2, Some(13)));
        assert_eq!(
            manager.placements(false).next().unwrap().image_id,
            ImageId(2)
        );

        manager.handle_command(command(b"a=d,d=N,I=13,p=7"), false);
        assert!(manager.get_image(ImageId(2)).is_none());
        assert_eq!(manager.newest_image_with_number(13), Some(ImageId(1)));

        manager.handle_command(command(b"a=p,I=13,p=8,c=1,r=1,C=1"), false);
        assert_eq!(
            manager.placements(false).next().unwrap().image_id,
            ImageId(1)
        );

        // Reusing an explicit ID replaces the old image and removes its
        // previous number association.
        manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;CQoLDA=="), false);
        assert_eq!(manager.newest_image_with_number(13), None);
        let missing = manager.handle_command(command(b"a=p,I=13,c=1,r=1,C=1"), false);
        assert_eq!(missing.response.unwrap().error_code, Some("ENOENT"));
    }

    #[test]
    fn numbered_chunking_preserves_allocated_response_identity() {
        let mut manager = GraphicsManager::new();
        let first = manager.handle_command(command(b"a=t,I=44,f=32,s=2,v=1,m=1;AQID"), false);
        assert!(first.response.is_none());
        let final_chunk = manager.handle_command(command(b"m=0;BAUGBwg="), false);
        let response = final_chunk.response.unwrap();
        assert_eq!((response.image_id, response.image_number), (1, Some(44)));
        assert_eq!(response.to_escape_sequence(), "\x1b_Gi=1,I=44;OK\x1b\\");
        assert_eq!(manager.newest_image_with_number(44), Some(ImageId(1)));
    }

    #[test]
    fn image_number_indices_follow_id_allocation_and_storage_eviction() {
        let mut manager = GraphicsManager::new();
        manager.storage = ImageStorage::new(2, 1024);
        manager.handle_command(command(b"a=t,i=1,f=32,s=1,v=1;AQIDBA=="), false);
        let numbered = manager.handle_command(command(b"a=t,I=20,f=32,s=1,v=1;BQYHCA=="), false);
        assert_eq!(numbered.response.unwrap().image_id, 2);
        manager.handle_command(command(b"a=t,I=30,f=32,s=1,v=1;CQoLDA=="), false);
        assert_eq!(manager.image_count(), 2);
        assert_eq!(manager.newest_image_with_number(20), Some(ImageId(2)));

        manager.handle_command(command(b"a=t,I=40,f=32,s=1,v=1;DQ4PEA=="), false);
        assert_eq!(manager.newest_image_with_number(20), None);
        assert!(!manager.image_number_by_id.contains_key(&ImageId(2)));
    }

    #[test]
    fn queries_require_ids_and_reject_image_numbers_truthfully() {
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(b"a=q,I=9,f=32,s=1,v=1;AQIDBA=="), false);
        let response = result.response.unwrap();
        assert_eq!(response.image_number, Some(9));
        assert_eq!(response.error_code, Some("EINVAL"));
        assert!(
            response
                .to_escape_sequence()
                .starts_with("\x1b_GI=9;EINVAL:")
        );
        assert!(manager.storage.is_empty());
    }

    #[test]
    fn supported_delete_selectors_remove_only_matching_placements() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(
            command(b"a=T,f=32,s=1,v=1,i=1,p=1,c=2,r=2,z=-1,C=1;AQIDBA=="),
            false,
        );
        manager.handle_command(
            command(b"a=T,f=32,s=1,v=1,i=2,p=2,c=1,r=1,z=2,C=1;AQIDBA=="),
            false,
        );
        manager.handle_command(command(b"a=d,d=z,z=-1"), false);
        assert_eq!(manager.placements(false).count(), 1);
        assert!(manager.get_image(ImageId(1)).is_some());
        manager.handle_command(command(b"a=d,d=I,i=2,p=2"), false);
        assert!(manager.get_image(ImageId(2)).is_none());
    }

    #[test]
    fn referenced_images_evict_after_unplaced_cache_entries() {
        let mut manager = GraphicsManager::new();
        manager.storage = ImageStorage::new(2, 1024);

        manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=1,C=1;AQIDBA=="), false);
        manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=2;BQYHCA=="), false);
        manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=3;CQoLDA=="), false);

        assert!(manager.get_image(ImageId(1)).is_some());
        assert!(manager.get_image(ImageId(2)).is_none());
        assert!(manager.get_image(ImageId(3)).is_some());
        assert_eq!(manager.placements(false).count(), 1);

        // Once the placement disappears, image 1 becomes an ordinary cached
        // transmission and is again eligible for oldest-first eviction.
        manager.handle_command(command(b"a=d,d=a"), false);
        manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=4;DQ4PEA=="), false);
        assert!(manager.get_image(ImageId(1)).is_none());
        assert!(manager.get_image(ImageId(3)).is_some());
        assert!(manager.get_image(ImageId(4)).is_some());
    }

    #[test]
    fn repeated_display_of_one_image_has_a_bounded_fifo_placement_window() {
        let mut manager = GraphicsManager::new();
        manager.handle_command(command(b"a=t,f=32,s=1,v=1,i=1;AQIDBA=="), false);

        for placement_id in 1..=MAX_IMAGE_PLACEMENTS as u32 + 7 {
            let value = format!("a=p,i=1,p={placement_id},C=1");
            let result = manager.handle_command(command(value.as_bytes()), false);
            assert!(result.response.unwrap().ok);
        }

        let placements: Vec<_> = manager.placements(false).collect();
        assert_eq!(placements.len(), MAX_IMAGE_PLACEMENTS);
        assert_eq!(placements.first().unwrap().placement_id, Some(8));
        assert_eq!(
            placements.last().unwrap().placement_id,
            Some(MAX_IMAGE_PLACEMENTS as u32 + 7)
        );
        assert_eq!(manager.image_count(), 1);

        manager.handle_command(command(b"a=p,i=1,p=8,C=1"), false);
        assert_eq!(manager.placement_count(), MAX_IMAGE_PLACEMENTS);
        assert_eq!(
            manager.placements(false).last().unwrap().placement_id,
            Some(8)
        );
    }

    #[test]
    fn placement_never_acknowledges_an_inexact_cursor_policy() {
        let mut manager = GraphicsManager::new();
        let result = manager.handle_command(command(b"a=T,f=32,s=1,v=1,i=5,c=1;AQIDBA=="), false);
        assert_eq!(result.response.unwrap().error_code, Some("ENOTSUP"));
        assert!(manager.get_image(ImageId(5)).is_none());
    }

    #[test]
    fn pixel_protocol_placements_clip_reflow_and_release_unreachable_pixels() {
        let mut manager = GraphicsManager::new();
        manager
            .handle_sixel(b"q\"1;1!4~", 0, 0, (2, 3), false)
            .unwrap();
        manager
            .handle_sixel(b"q\"1;1!4~", 0, 0, (2, 3), true)
            .unwrap();
        assert_eq!(manager.image_count(), 2);
        assert_eq!(manager.placement_count(), 2);
        assert_eq!(manager.memory_usage(), 2 * 4 * 6 * 4);

        manager.scroll_up(false, 0..2, 0..usize::MAX, 1, false, 0);
        let clipped = manager.placements(false).next().unwrap();
        assert_eq!(clipped.height, Some(1));
        assert_eq!(clipped.pixel_height, Some(3));
        assert_eq!(clipped.source_y, 3);
        assert_eq!(clipped.source_height, Some(3));

        manager.set_cell_pixel_geometry(4, 6);
        let primary = manager.placements(false).next().unwrap();
        assert_eq!((primary.width, primary.height), (Some(1), Some(1)));
        assert_eq!(
            (primary.pixel_width, primary.pixel_height),
            (Some(4), Some(3))
        );

        manager.clear_screen(false);
        assert_eq!(manager.image_count(), 1);
        assert_eq!(manager.placement_count(), 1);
        manager.clear_screen(true);
        assert_eq!(manager.image_count(), 0);
        assert_eq!(manager.memory_usage(), 0);
    }

    #[test]
    fn sixel_aspect_changes_display_footprint_without_expanding_source_storage() {
        let mut manager = GraphicsManager::new();
        let movement = manager
            .handle_sixel(b"7q\"2;1;2;6!2~", 3, 4, (2, 3), false)
            .unwrap();
        assert_eq!(movement, (0, 4));
        assert_eq!(manager.memory_usage(), 2 * 6 * 4);
        let placement = manager.placements(false).next().unwrap();
        assert_eq!((placement.col, placement.row), (3, 4));
        assert_eq!((placement.width, placement.height), (Some(1), Some(4)));
        assert_eq!(
            (placement.pixel_width, placement.pixel_height),
            (Some(2), Some(12))
        );
        let image = manager.get_image(placement.image_id).unwrap();
        assert_eq!((image.width, image.height), (2, 6));
    }

    #[test]
    fn static_pixel_protocols_share_scroll_resize_screen_and_cleanup_lifecycle() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();
        }
        let encoded = STANDARD.encode(png_bytes);
        let data = format!("preserveAspectRatio=0:{encoded}");
        let params = [
            b"File=inline=1".as_slice(),
            b"width=4px".as_slice(),
            b"height=6px".as_slice(),
            data.as_bytes(),
        ];

        let mut manager = GraphicsManager::new();
        for alternate_screen in [false, true] {
            manager
                .handle_sixel(b"7q\"1;1;4;6!4~", 0, 0, (2, 3), alternate_screen)
                .unwrap();
            manager
                .handle_iterm2(&params, (0, 0), (80, 24), (2, 3), alternate_screen)
                .unwrap();
        }
        assert_eq!(manager.image_count(), 4);
        assert_eq!(manager.placements(false).count(), 2);
        assert_eq!(manager.placements(true).count(), 2);

        manager.scroll_up(false, 0..2, 0..usize::MAX, 1, false, 0);
        let clipped: Vec<_> = manager.placements(false).collect();
        assert_eq!(clipped.len(), 2);
        assert!(clipped.iter().all(|placement| placement.height == Some(1)));
        assert_eq!(manager.placements(true).count(), 2);

        manager.set_cell_pixel_geometry(4, 6);
        assert!(
            manager
                .placements(false)
                .all(|placement| { placement.width == Some(1) && placement.height == Some(1) })
        );

        manager.clear_screen(false);
        assert_eq!(manager.image_count(), 2);
        assert_eq!(manager.placement_count(), 2);
        assert_eq!(manager.placements(false).count(), 0);
        assert_eq!(manager.placements(true).count(), 2);
        manager.clear_screen(true);
        assert_eq!(manager.image_count(), 0);
        assert_eq!(manager.placement_count(), 0);
        assert_eq!(manager.memory_usage(), 0);
    }

    #[test]
    fn rectangular_text_operations_crop_erase_and_copy_image_coverage() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let pixels = STANDARD.encode(vec![255; 4 * 4 * 4]);
        let command_text = format!("a=T,f=32,s=4,v=4,i=1,c=4,r=4,C=1;{pixels}");
        let mut manager = GraphicsManager::new();
        manager.handle_command(command(command_text.as_bytes()), false);
        manager.erase_rect(false, 1..3, 1..3);

        let fragments: Vec<_> = manager.placements(false).collect();
        assert_eq!(fragments.len(), 4);
        assert_eq!(
            fragments
                .iter()
                .map(|placement| { placement.width.unwrap_or(1) * placement.height.unwrap_or(1) })
                .sum::<u32>(),
            12
        );
        for row in 1..3 {
            for col in 1..3 {
                assert!(
                    fragments
                        .iter()
                        .all(|placement| !placement_intersects(placement, col, row))
                );
            }
        }
        assert!(manager.get_image(ImageId(1)).is_some());

        let mut copied = GraphicsManager::new();
        let command_text = format!("a=T,f=32,s=4,v=4,i=2,c=2,r=2,C=1;{pixels}");
        copied.handle_command(command(command_text.as_bytes()), false);
        copied.copy_rect(false, 0..2, 0..2, 1, 2);
        let placements: Vec<_> = copied.placements(false).collect();
        assert_eq!(placements.len(), 2);
        assert!(placements.iter().any(|placement| {
            placement.col == 0
                && placement.row == 0
                && placement.width == Some(2)
                && placement.height == Some(2)
        }));
        assert!(placements.iter().any(|placement| {
            placement.col == 2
                && placement.row == 1
                && placement.width == Some(2)
                && placement.height == Some(2)
        }));
        assert!(copied.get_image(ImageId(2)).is_some());
    }

    #[test]
    fn all_protocols_share_one_bounded_cache_and_normalized_placement_model() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();
        }
        let encoded = STANDARD.encode(png_bytes);
        let data = format!("height=10px:{encoded}");
        let params = [b"File=inline=1".as_slice(), data.as_bytes()];

        let mut manager = GraphicsManager::new();
        manager
            .handle_sixel(b"q\"1;1!4~", 1, 2, (10, 10), false)
            .unwrap();
        manager
            .handle_iterm2(&params, (3, 4), (80, 24), (10, 10), false)
            .unwrap();

        let placements: Vec<_> = manager.placements(false).collect();
        assert_eq!(placements.len(), 2);
        assert!(placements.iter().all(|placement| {
            placement.source_width.is_some()
                && placement.source_height.is_some()
                && placement.pixel_width.is_some()
                && placement.pixel_height.is_some()
        }));
        assert_eq!(manager.image_count(), 2);
        assert!(manager.memory_usage() <= MAX_IMAGE_MEMORY);
    }
}
