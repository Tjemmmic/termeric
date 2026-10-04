//! Persistent terminal-row geometry and GPU buffers.

use std::collections::HashMap;

use super::buffer::GrowableBuffer;
use super::rect::{RectBatch, RectPipeline};
use super::text::{TextBatch, TextPipeline};
use wgpu::{BufferUsages, Device, Queue, RenderPass};

const MAX_CACHE_ROWS: usize = 4_096;
const RETAIN_FRAMES: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RowCacheKey {
    pub grid_id: usize,
    pub visual_row: usize,
}

/// A row's uploaded geometry. The CPU batches are dropped once uploaded:
/// drawing needs only the index counts, and frame pinning the atlas pages.
struct CachedGpuRow {
    signature: u64,
    rect_index_count: u32,
    text_index_count: u32,
    atlas_pages: Vec<u16>,
    rect_vertices: GrowableBuffer,
    rect_indices: GrowableBuffer,
    text_vertices: GrowableBuffer,
    text_indices: GrowableBuffer,
    last_used_frame: u64,
}

impl CachedGpuRow {
    fn new(signature: u64, frame: u64) -> Self {
        Self {
            signature,
            rect_index_count: 0,
            text_index_count: 0,
            atlas_pages: Vec::new(),
            rect_vertices: GrowableBuffer::new("Cached Row Rect Vertices", BufferUsages::VERTEX),
            rect_indices: GrowableBuffer::new("Cached Row Rect Indices", BufferUsages::INDEX),
            text_vertices: GrowableBuffer::new("Cached Row Text Vertices", BufferUsages::VERTEX),
            text_indices: GrowableBuffer::new("Cached Row Text Indices", BufferUsages::INDEX),
            last_used_frame: frame,
        }
    }

    fn upload(&mut self, rects: &RectBatch, text: TextBatch, device: &Device, queue: &Queue) {
        self.rect_vertices
            .upload(device, queue, bytemuck::cast_slice(&rects.vertices));
        self.rect_indices
            .upload(device, queue, bytemuck::cast_slice(&rects.indices));
        self.text_vertices
            .upload(device, queue, bytemuck::cast_slice(&text.vertices));
        self.text_indices
            .upload(device, queue, bytemuck::cast_slice(&text.indices));
        self.rect_index_count = rects.indices.len() as u32;
        self.text_index_count = text.indices.len() as u32;
        self.atlas_pages = text.atlas_pages;
    }
}

#[derive(Default)]
pub(super) struct TerminalRowCache {
    rows: HashMap<RowCacheKey, CachedGpuRow>,
    frame_rows: Vec<RowCacheKey>,
    frame: u64,
}

impl TerminalRowCache {
    pub(super) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        self.frame_rows.clear();
    }

    /// Draw the row from cache this frame if its signature is unchanged.
    pub(super) fn reuse(&mut self, key: RowCacheKey, signature: u64) -> bool {
        let Some(row) = self.rows.get_mut(&key) else {
            return false;
        };
        if row.signature != signature {
            return false;
        }
        row.last_used_frame = self.frame;
        self.frame_rows.push(key);
        true
    }

    pub(super) fn referenced_atlas_pages(&self, key: RowCacheKey) -> Vec<u16> {
        self.rows
            .get(&key)
            .map(|row| row.atlas_pages.clone())
            .unwrap_or_default()
    }

    pub(super) fn replace(
        &mut self,
        key: RowCacheKey,
        signature: u64,
        rects: RectBatch,
        text: TextBatch,
        device: &Device,
        queue: &Queue,
    ) {
        let mut row = self
            .rows
            .remove(&key)
            .unwrap_or_else(|| CachedGpuRow::new(signature, self.frame));
        row.signature = signature;
        row.last_used_frame = self.frame;
        row.upload(&rects, text, device, queue);
        self.rows.insert(key, row);
        self.frame_rows.push(key);
    }

    pub(super) fn finish_frame(&mut self) {
        if self.rows.len() > MAX_CACHE_ROWS {
            let oldest = self.frame.saturating_sub(RETAIN_FRAMES);
            self.rows.retain(|_, row| row.last_used_frame >= oldest);
        }
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.frame_rows.clear();
    }

    pub(super) fn draw_rects(&self, pipeline: &RectPipeline, pass: &mut RenderPass<'_>) {
        for key in &self.frame_rows {
            let Some(row) = self.rows.get(key) else {
                continue;
            };
            let (Some(vertices), Some(indices)) =
                (row.rect_vertices.buffer(), row.rect_indices.buffer())
            else {
                continue;
            };
            pipeline.draw_external(pass, vertices, indices, row.rect_index_count);
        }
    }

    pub(super) fn draw_text(&self, pipeline: &TextPipeline, pass: &mut RenderPass<'_>) {
        for key in &self.frame_rows {
            let Some(row) = self.rows.get(key) else {
                continue;
            };
            let (Some(vertices), Some(indices)) =
                (row.text_vertices.buffer(), row.text_indices.buffer())
            else {
                continue;
            };
            pipeline.draw_external(pass, vertices, indices, row.text_index_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_signature_hits_and_one_change_misses_only_that_row() {
        let mut cache = TerminalRowCache::default();
        let first = RowCacheKey {
            grid_id: 1,
            visual_row: 0,
        };
        let second = RowCacheKey {
            grid_id: 1,
            visual_row: 1,
        };
        cache.rows.insert(first, CachedGpuRow::new(10, 0));
        cache.rows.insert(second, CachedGpuRow::new(20, 0));

        cache.begin_frame();
        assert!(cache.reuse(first, 10));
        assert!(!cache.reuse(second, 21));
        assert_eq!(cache.frame_rows, [first]);
    }

    #[test]
    fn cached_rows_retain_their_atlas_page_references() {
        let mut row = CachedGpuRow::new(1, 0);
        row.atlas_pages = vec![1, 3];
        let key = RowCacheKey {
            grid_id: 7,
            visual_row: 2,
        };
        let mut cache = TerminalRowCache::default();
        cache.rows.insert(key, row);
        assert_eq!(cache.referenced_atlas_pages(key), vec![1, 3]);
    }
}
