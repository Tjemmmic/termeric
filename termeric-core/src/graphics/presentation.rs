//! Read-only graphics state shared by live and synchronized presentations.

use super::*;

/// A bounded copy of presentation metadata and one active pixel Arc per image.
/// No pending transmissions, file policy, animation clocks, or mutable cache
/// state cross into the frozen frame.
pub(crate) struct GraphicsSnapshot {
    placements: VecDeque<ManagedImagePlacement>,
    virtual_placements: VecDeque<VirtualImagePlacement>,
    images: HashMap<ImageId, StoredImage>,
    alternate_screen: bool,
    cell_width: u32,
    cell_height: u32,
}

impl GraphicsSnapshot {
    pub(crate) fn capture(
        manager: &GraphicsManager,
        grid: &Grid,
        alternate_screen: bool,
        cell_width: u32,
        cell_height: u32,
    ) -> Self {
        let offset = i64::try_from(grid.scroll_offset()).unwrap_or(i64::MAX);
        let placements: VecDeque<_> = manager
            .placements
            .iter()
            .filter(|p| p.alternate_screen == alternate_screen)
            .cloned()
            .map(|mut p| {
                // Relative offsets are local to the parent, not screen rows.
                if p.relative.is_none() {
                    p.row = p.row.saturating_add(offset);
                }
                p
            })
            .collect();
        let virtual_placements: VecDeque<_> = manager
            .virtual_placements
            .iter()
            .filter(|p| p.alternate_screen == alternate_screen)
            .copied()
            .collect();
        let ids: HashSet<_> = placements
            .iter()
            .map(|p| p.image_id)
            .chain(virtual_placements.iter().map(|p| p.image_id))
            .collect();
        let images = ids
            .into_iter()
            .filter_map(|id| manager.current_image(id).map(|image| (id, image)))
            .collect();
        Self {
            placements,
            virtual_placements,
            images,
            alternate_screen,
            cell_width,
            cell_height,
        }
    }

    pub(crate) fn presentation_images(&self, grid: &Grid) -> Vec<PresentationImage> {
        PresentationView {
            placements: &self.placements,
            virtual_placements: &self.virtual_placements,
            image: &|id| self.images.get(&id).cloned(),
        }
        .presentation_images(
            grid,
            self.alternate_screen,
            self.cell_width,
            self.cell_height,
        )
    }

    /// Freeze content, not host display metrics. Font/scale changes must use
    /// current cell sizes for derived rectangles and retained pixel coverage.
    pub(crate) fn set_cell_pixel_geometry(&mut self, cell_width: u32, cell_height: u32) {
        self.cell_width = cell_width.max(1);
        self.cell_height = cell_height.max(1);
        for placement in &mut self.placements {
            placement.update_cell_footprint(self.cell_width, self.cell_height);
        }
    }

    pub(crate) fn resize(&mut self, same_columns: bool, archived: usize, history: usize) {
        let shift = if same_columns {
            i64::try_from(archived).unwrap_or(i64::MAX)
        } else {
            0
        };
        let oldest = -i64::try_from(history).unwrap_or(i64::MAX);
        self.placements.retain_mut(|p| {
            if p.relative.is_some() {
                return true;
            }
            p.row = p.row.saturating_sub(shift);
            p.row.saturating_add(i64::from(p.height.unwrap_or(1))) > oldest
        });
        // Keep the captured parent graph intact: an on-screen offset cannot
        // keep a child alive after its root's retained history disappears.
        loop {
            let retained: HashSet<_> = self
                .placements
                .iter()
                .map(|p| p.uid)
                .chain(self.virtual_placements.iter().map(|p| p.uid))
                .collect();
            let before = self.placements.len();
            self.placements
                .retain(|p| p.relative.is_none_or(|r| retained.contains(&r.parent)));
            if before == self.placements.len() {
                break;
            }
        }
        let used: HashSet<_> = self
            .placements
            .iter()
            .map(|p| p.image_id)
            .chain(self.virtual_placements.iter().map(|p| p.image_id))
            .collect();
        self.images.retain(|id, _| used.contains(id));
    }
}

pub(super) struct PresentationView<'a> {
    pub(super) placements: &'a VecDeque<ManagedImagePlacement>,
    pub(super) virtual_placements: &'a VecDeque<VirtualImagePlacement>,
    pub(super) image: &'a dyn Fn(ImageId) -> Option<StoredImage>,
}

impl PresentationView<'_> {
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
                    let Some(image) = (self.image)(prototype.image_id) else {
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
                    if !merge_adjacent_placeholder_presentation(
                        placeholder_presentations.last_mut(),
                        &presentation,
                        cell_width,
                    ) && placeholder_presentations.len() < MAX_IMAGE_PLACEMENTS
                    {
                        placeholder_presentations.push(presentation);
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
                let image = (self.image)(placement.image_id)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(manager: &mut GraphicsManager, command: &[u8], alternate: bool) {
        let result = manager.handle_command(KittyCommand::parse(command).unwrap(), alternate);
        assert!(result.response.is_none_or(|response| response.ok));
    }

    #[test]
    fn snapshot_captures_only_referenced_active_pixels_from_its_screen() {
        let mut manager = GraphicsManager::new();
        send(&mut manager, b"a=t,i=1,f=32,s=1,v=1;AQIDBA==", false);
        send(
            &mut manager,
            b"a=T,i=2,c=1,r=1,C=1,f=32,s=1,v=1;AQIDBA==",
            false,
        );
        send(&mut manager, b"a=f,i=2,f=32,s=1,v=1;BQYHCA==", false);
        send(&mut manager, b"a=a,i=2,s=1,c=2", false);
        send(
            &mut manager,
            b"a=T,U=1,i=3,c=1,r=1,f=32,s=1,v=1;AQIDBA==",
            false,
        );
        send(
            &mut manager,
            b"a=T,i=4,c=1,r=1,C=1,f=32,s=1,v=1;AQIDBA==",
            true,
        );
        let active = Arc::downgrade(&manager.current_image(ImageId(2)).unwrap().data);
        let root = Arc::downgrade(&manager.get_image(ImageId(2)).unwrap().data);
        let unused = Arc::downgrade(&manager.get_image(ImageId(1)).unwrap().data);
        let snapshot = GraphicsSnapshot::capture(&manager, &Grid::new(4, 3), false, 10, 20);
        assert_eq!(snapshot.placements.len(), 1);
        assert_eq!(snapshot.virtual_placements.len(), 1);
        assert_eq!(snapshot.images.len(), 2);
        assert_eq!(&*snapshot.images[&ImageId(2)].data, &[5, 6, 7, 8]);
        assert!(Arc::ptr_eq(
            &snapshot.images[&ImageId(2)].data,
            &active.upgrade().unwrap()
        ));
        drop(manager);
        assert!(active.upgrade().is_some());
        assert!(root.upgrade().is_none());
        assert!(unused.upgrade().is_none());
        drop(snapshot);
        assert!(active.upgrade().is_none());
    }

    #[test]
    fn frozen_resize_retires_relative_chains_with_their_parent() {
        for same_columns in [false, true] {
            let mut manager = GraphicsManager::new();
            for id in 1..=3 {
                send(
                    &mut manager,
                    format!("a=t,i={id},f=32,s=1,v=1;AQIDBA==").as_bytes(),
                    false,
                );
            }
            send(&mut manager, b"a=p,i=1,p=1,c=1,r=1,C=1", false);
            send(&mut manager, b"a=p,i=2,p=1,P=1,Q=1,V=3,c=1,r=1", false);
            send(&mut manager, b"a=p,i=3,p=1,P=2,Q=1,V=1,c=1,r=1", false);
            manager.scroll_history_up(false, 2, 2, None);
            let pixels: Vec<_> = (1..=3)
                .map(|id| Arc::downgrade(&manager.get_image(ImageId(id)).unwrap().data))
                .collect();
            let mut snapshot = GraphicsSnapshot::capture(&manager, &Grid::new(4, 3), false, 10, 20);
            drop(manager);
            assert_eq!(snapshot.placements.len(), 3);
            assert!(pixels.iter().all(|p| p.upgrade().is_some()));
            snapshot.resize(same_columns, 1, 1);
            assert!(snapshot.placements.is_empty());
            assert!(snapshot.images.is_empty());
            assert!(pixels.iter().all(|p| p.upgrade().is_none()));
        }
    }
}
