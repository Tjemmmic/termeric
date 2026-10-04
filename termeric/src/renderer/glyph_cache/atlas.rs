//! The paged glyph atlas allocator: shelves on fixed-size pages, page
//! pinning for the frame in flight, and least-recently-used eviction.

use super::*;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AtlasPage {
    pub(super) cursor_x: u32,
    pub(super) cursor_y: u32,
    pub(super) row_height: u32,
    pub(super) occupied_pixels: u64,
    pub(super) last_used_frame: u64,
    pub(super) pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AtlasSlot {
    pub(super) page: u16,
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) evicted_page: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AtlasAllocationError {
    /// The glyph can never fit in a page with this atlas configuration.
    TooLarge,
    /// Capacity exists only on pages that are still referenced by this frame.
    FramePinned,
}

#[derive(Debug)]
pub(super) struct PagedAtlasAllocator {
    pub(super) page_size: u32,
    pub(super) max_pages: u32,
    pub(super) pages: Vec<AtlasPage>,
    pub(super) frame: u64,
    pub(super) generation: u64,
    pub(super) evictions: u64,
    pub(super) allocation_failures: u64,
    /// A frame-pinned miss may have placed the visible replacement glyph in a
    /// persistent row batch. Advance the generation at the next frame boundary
    /// so that batch is rebuilt after the current pins have been released.
    pub(super) retry_on_next_frame: bool,
    pub(super) protect_page_zero: bool,
}

impl PagedAtlasAllocator {
    pub(super) fn new(page_size: u32, max_pages: u32) -> Self {
        Self::with_page_zero_policy(page_size, max_pages, true)
    }

    pub(super) fn recyclable(page_size: u32, max_pages: u32) -> Self {
        Self::with_page_zero_policy(page_size, max_pages, false)
    }

    pub(super) fn with_page_zero_policy(
        page_size: u32,
        max_pages: u32,
        protect_page_zero: bool,
    ) -> Self {
        assert!(page_size > 0);
        assert!(max_pages > 0);
        Self {
            page_size,
            max_pages,
            pages: vec![AtlasPage {
                pinned: protect_page_zero,
                ..AtlasPage::default()
            }],
            frame: 0,
            generation: 0,
            evictions: 0,
            allocation_failures: 0,
            retry_on_next_frame: false,
            protect_page_zero,
        }
    }

    pub(super) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        if self.retry_on_next_frame {
            self.generation = self.generation.wrapping_add(1);
            self.retry_on_next_frame = false;
        }
        for (index, page) in self.pages.iter_mut().enumerate() {
            // Page zero contains the permanent visible replacement glyph.
            page.pinned = self.protect_page_zero && index == 0;
        }
    }

    pub(super) fn pin(&mut self, page: u16) {
        if let Some(state) = self.pages.get_mut(page as usize) {
            state.pinned = true;
            state.last_used_frame = self.frame;
        }
    }

    pub(super) fn allocate(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<AtlasSlot, AtlasAllocationError> {
        if width == 0 || height == 0 {
            return Ok(AtlasSlot {
                page: 0,
                x: 0,
                y: 0,
                evicted_page: None,
            });
        }
        if width.saturating_add(ATLAS_PADDING) > self.page_size
            || height.saturating_add(ATLAS_PADDING) > self.page_size
        {
            self.allocation_failures += 1;
            return Err(AtlasAllocationError::TooLarge);
        }

        for page in 0..self.pages.len() {
            if let Some((x, y)) = self.allocate_on_page(page, width, height) {
                return Ok(AtlasSlot {
                    page: page as u16,
                    x,
                    y,
                    evicted_page: None,
                });
            }
        }

        if self.pages.len() < self.max_pages as usize {
            self.pages.push(AtlasPage::default());
            let page = self.pages.len() - 1;
            let (x, y) = self
                .allocate_on_page(page, width, height)
                .expect("a validated glyph fits on a fresh atlas page");
            return Ok(AtlasSlot {
                page: page as u16,
                x,
                y,
                evicted_page: None,
            });
        }

        let victim = self
            .pages
            .iter()
            .enumerate()
            .skip(usize::from(self.protect_page_zero))
            .filter(|(_, page)| !page.pinned)
            .min_by_key(|(_, page)| page.last_used_frame)
            .map(|(index, _)| index);
        let Some(victim) = victim else {
            self.allocation_failures += 1;
            self.retry_on_next_frame = true;
            return Err(AtlasAllocationError::FramePinned);
        };

        self.pages[victim] = AtlasPage::default();
        self.evictions += 1;
        self.generation = self.generation.wrapping_add(1);
        let (x, y) = self
            .allocate_on_page(victim, width, height)
            .expect("a validated glyph fits on a recycled atlas page");
        Ok(AtlasSlot {
            page: victim as u16,
            x,
            y,
            evicted_page: Some(victim as u16),
        })
    }

    pub(super) fn allocate_on_page(
        &mut self,
        page_index: usize,
        width: u32,
        height: u32,
    ) -> Option<(u32, u32)> {
        let page = &mut self.pages[page_index];
        let mut x = page.cursor_x;
        let mut y = page.cursor_y;
        let mut row_height = page.row_height;
        if x.saturating_add(width).saturating_add(ATLAS_PADDING) > self.page_size {
            x = 0;
            y = y.saturating_add(row_height).saturating_add(ATLAS_PADDING);
            row_height = 0;
        }
        if y.saturating_add(height).saturating_add(ATLAS_PADDING) > self.page_size {
            return None;
        }
        page.cursor_x = x + width + ATLAS_PADDING;
        page.cursor_y = y;
        page.row_height = row_height.max(height);
        page.occupied_pixels += u64::from(width) * u64::from(height);
        page.last_used_frame = self.frame;
        page.pinned = true;
        Some((x, y))
    }

    pub(super) fn reset(&mut self) {
        self.pages.clear();
        self.pages.push(AtlasPage {
            pinned: self.protect_page_zero,
            ..AtlasPage::default()
        });
        self.retry_on_next_frame = false;
        self.generation = self.generation.wrapping_add(1);
    }

    pub(super) fn metrics(
        &self,
        hits: u64,
        misses: u64,
        bytes_per_pixel: u64,
    ) -> GlyphAtlasMetrics {
        GlyphAtlasMetrics {
            pages: self.pages.len() as u32,
            color_pages: 0,
            max_pages: self.max_pages,
            max_color_pages: 0,
            capacity_bytes: u64::from(self.page_size)
                * u64::from(self.page_size)
                * u64::from(self.max_pages)
                * bytes_per_pixel,
            occupied_pixels: self.pages.iter().map(|page| page.occupied_pixels).sum(),
            hits,
            misses,
            evictions: self.evictions,
            allocation_failures: self.allocation_failures,
        }
    }
}
