//! The live-domain graphics relay: snapshots of the image namespace and
//! their replay into another manager.

use super::*;

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
    crate::checksum::fnv1a64(bytes)
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

impl GraphicsManager {
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
        let cell_pixels = (self.cell_pixel_width, self.cell_pixel_height);
        *self = Self::new();
        self.allow_file_transmission = allow_file_transmission;
        // A relay replaces decoded content, not the receiving host's display
        // geometry. Keep this on failure cleanup as well as import begin.
        self.set_cell_pixel_geometry(cell_pixels.0, cell_pixels.1);
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
            .map(|placement| {
                let mut placement = relay_managed_placement(placement);
                // The sender may have different cells, or this host may have
                // changed font/scale while the fragmented import was pending.
                placement.update_cell_footprint(self.cell_pixel_width, self.cell_pixel_height);
                placement
            })
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
}
