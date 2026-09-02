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

struct CachedGpuRow {
    signature: u64,
    rects: RectBatch,
    text: TextBatch,
    rect_vertices: GrowableBuffer,
    rect_indices: GrowableBuffer,
    text_vertices: GrowableBuffer,
    text_indices: GrowableBuffer,
    last_used_frame: u64,
}

impl CachedGpuRow {
    fn new(signature: u64, rects: RectBatch, text: TextBatch, frame: u64) -> Self {
        Self {
            signature,
            rects,
            text,
            rect_vertices: GrowableBuffer::new("Cached Row Rect Vertices", BufferUsages::VERTEX),
            rect_indices: GrowableBuffer::new("Cached Row Rect Indices", BufferUsages::INDEX),
            text_vertices: GrowableBuffer::new("Cached Row Text Vertices", BufferUsages::VERTEX),
            text_indices: GrowableBuffer::new("Cached Row Text Indices", BufferUsages::INDEX),
            last_used_frame: frame,
        }
    }

    fn upload(&mut self, device: &Device, queue: &Queue) {
        self.rect_vertices
            .upload(device, queue, bytemuck::cast_slice(&self.rects.vertices));
        self.rect_indices
            .upload(device, queue, bytemuck::cast_slice(&self.rects.indices));
        self.text_vertices
            .upload(device, queue, bytemuck::cast_slice(&self.text.vertices));
        self.text_indices
            .upload(device, queue, bytemuck::cast_slice(&self.text.indices));
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RowCacheMetrics {
    pub frame_hits: u64,
    pub frame_misses: u64,
    pub total_hits: u64,
    pub total_misses: u64,
    pub cached_rows: usize,
    pub buffer_allocations: u64,
}

#[derive(Default)]
pub(super) struct TerminalRowCache {
    rows: HashMap<RowCacheKey, CachedGpuRow>,
    frame_rows: Vec<RowCacheKey>,
    frame: u64,
    metrics: RowCacheMetrics,
}

impl TerminalRowCache {
    pub(super) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        self.frame_rows.clear();
        self.metrics.frame_hits = 0;
        self.metrics.frame_misses = 0;
    }

    pub(super) fn reuse(&mut self, key: RowCacheKey, signature: u64) -> bool {
        let Some(row) = self.rows.get_mut(&key) else {
            self.metrics.frame_misses += 1;
            self.metrics.total_misses += 1;
            return false;
        };
        if row.signature != signature {
            self.metrics.frame_misses += 1;
            self.metrics.total_misses += 1;
            return false;
        }
        row.last_used_frame = self.frame;
        self.frame_rows.push(key);
        self.metrics.frame_hits += 1;
        self.metrics.total_hits += 1;
        true
    }

    pub(super) fn referenced_atlas_pages(&self, key: RowCacheKey) -> Vec<u16> {
        self.rows
            .get(&key)
            .map(|row| row.text.atlas_pages.clone())
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
        let mut row = self.rows.remove(&key).unwrap_or_else(|| {
            CachedGpuRow::new(
                signature,
                RectBatch::default(),
                TextBatch::default(),
                self.frame,
            )
        });
        row.signature = signature;
        row.rects = rects;
        row.text = text;
        row.last_used_frame = self.frame;
        row.upload(device, queue);
        self.rows.insert(key, row);
        self.frame_rows.push(key);
    }

    pub(super) fn finish_frame(&mut self) {
        let oldest = self.frame.saturating_sub(RETAIN_FRAMES);
        if self.rows.len() > MAX_CACHE_ROWS {
            self.rows.retain(|_, row| row.last_used_frame >= oldest);
        }
        self.metrics.cached_rows = self.rows.len();
        self.metrics.buffer_allocations = self
            .rows
            .values()
            .map(|row| {
                row.rect_vertices.stats().allocations
                    + row.rect_indices.stats().allocations
                    + row.text_vertices.stats().allocations
                    + row.text_indices.stats().allocations
            })
            .sum();
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.frame_rows.clear();
        self.metrics.cached_rows = 0;
    }

    pub(super) fn metrics(&self) -> RowCacheMetrics {
        self.metrics
    }

    #[cfg(test)]
    pub(super) fn cpu_batches_match(
        &self,
        key: RowCacheKey,
        rects: &RectBatch,
        text: &TextBatch,
    ) -> bool {
        self.rows
            .get(&key)
            .is_some_and(|row| &row.rects == rects && &row.text == text)
    }

    pub(super) fn draw_rects<'a>(&'a self, pipeline: &'a RectPipeline, pass: &mut RenderPass<'a>) {
        for key in &self.frame_rows {
            let Some(row) = self.rows.get(key) else {
                continue;
            };
            let (Some(vertices), Some(indices)) =
                (row.rect_vertices.buffer(), row.rect_indices.buffer())
            else {
                continue;
            };
            pipeline.draw_external(pass, vertices, indices, row.rects.indices.len() as u32);
        }
    }

    pub(super) fn draw_text<'a>(&'a self, pipeline: &'a TextPipeline, pass: &mut RenderPass<'a>) {
        for key in &self.frame_rows {
            let Some(row) = self.rows.get(key) else {
                continue;
            };
            let (Some(vertices), Some(indices)) =
                (row.text_vertices.buffer(), row.text_indices.buffer())
            else {
                continue;
            };
            pipeline.draw_external(pass, vertices, indices, row.text.indices.len() as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::rect::RectVertex;
    use crate::renderer::text::{QueuedChar, QueuedContent};

    fn rect_batch() -> RectBatch {
        RectBatch {
            vertices: vec![RectVertex {
                position: [1.0, 2.0],
                color: [0.1, 0.2, 0.3, 1.0],
                rect_center: [2.0, 3.0],
                rect_half_size: [1.0, 1.0],
                corner_radius: 0.0,
                _pad: 0.0,
            }],
            indices: vec![0],
        }
    }

    fn text_batch() -> TextBatch {
        TextBatch {
            queued: vec![QueuedChar {
                content: QueuedContent::Char('x'),
                x: 1.0,
                y: 2.0,
                color: [1.0; 4],
                style: Default::default(),
                clip: None,
            }],
            ..TextBatch::default()
        }
    }

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
        cache
            .rows
            .insert(first, CachedGpuRow::new(10, rect_batch(), text_batch(), 0));
        cache
            .rows
            .insert(second, CachedGpuRow::new(20, rect_batch(), text_batch(), 0));

        cache.begin_frame();
        assert!(cache.reuse(first, 10));
        assert!(!cache.reuse(second, 21));
        assert_eq!(cache.metrics().frame_hits, 1);
        assert_eq!(cache.metrics().frame_misses, 1);
    }

    #[test]
    fn cached_rows_retain_their_atlas_page_references() {
        let mut batch = text_batch();
        batch.atlas_pages = vec![1, 3];
        let row = CachedGpuRow::new(1, rect_batch(), batch, 0);
        let key = RowCacheKey {
            grid_id: 7,
            visual_row: 2,
        };
        let mut cache = TerminalRowCache::default();
        cache.rows.insert(key, row);
        assert_eq!(cache.referenced_atlas_pages(key), vec![1, 3]);
    }

    #[test]
    fn cached_cpu_batches_match_forced_full_batches() {
        let rects = rect_batch();
        let text = text_batch();
        let key = RowCacheKey {
            grid_id: 1,
            visual_row: 0,
        };
        let mut cache = TerminalRowCache::default();
        cache
            .rows
            .insert(key, CachedGpuRow::new(1, rects.clone(), text.clone(), 0));
        assert!(cache.cpu_batches_match(key, &rects, &text));
    }
}
