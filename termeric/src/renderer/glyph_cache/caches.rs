//! Bounded caches of shaped runs and laid-out UI text.

use super::*;

/// Count and owned-payload bounds are independent. Hash-table bucket/control
/// storage is bounded by the entry ceiling, not included in payload accounting.
pub(super) struct ShapedRunCache {
    pub(super) entries: HashMap<LigatureKey, (LigatureRun, usize)>,
    pub(super) bytes: usize,
    pub(super) byte_limit: usize,
    pub(super) skipped: u64,
}

impl ShapedRunCache {
    pub(super) fn new(byte_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            byte_limit,
            skipped: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn get(&self, key: &LigatureKey) -> Option<&LigatureRun> {
        self.entries.get(key).map(|(run, _)| run)
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&LigatureRun) -> bool) {
        self.entries.retain(|_, (run, bytes)| {
            if keep(run) {
                true
            } else {
                self.bytes -= *bytes;
                false
            }
        });
    }

    /// Preserve the established whole-cache eviction policy. An oversized
    /// run bypasses caching without evicting useful entries or truncating text.
    /// Return only capacity evictions, not replacement of an identical key.
    pub(super) fn insert(&mut self, key: LigatureKey, run: &LigatureRun) -> usize {
        let key_bytes = Self::key_bytes(&key);
        let minimum_bytes = key_bytes.saturating_add(
            run.glyphs
                .len()
                .saturating_mul(std::mem::size_of::<ShapedGlyph>()),
        );
        if minimum_bytes > self.byte_limit {
            self.skipped = self.skipped.saturating_add(1);
            return 0;
        }
        let stored = run.clone();
        let bytes = key_bytes.saturating_add(
            stored
                .glyphs
                .capacity()
                .saturating_mul(std::mem::size_of::<ShapedGlyph>()),
        );
        if bytes > self.byte_limit {
            self.skipped = self.skipped.saturating_add(1);
            return 0;
        }
        // Remove the old key too: equal keys can own different capacities.
        if let Some((_, (_, old_bytes))) = self.entries.remove_entry(&key) {
            self.bytes -= old_bytes;
        }
        let mut evicted = 0;
        if self.entries.len() >= MAX_SHAPED_RUNS || self.bytes > self.byte_limit - bytes {
            evicted = self.entries.len();
            self.clear();
        }
        self.entries.insert(key, (stored, bytes));
        self.bytes += bytes;
        evicted
    }

    pub(super) fn key_bytes(key: &LigatureKey) -> usize {
        std::mem::size_of::<LigatureKey>()
            .saturating_add(std::mem::size_of::<(LigatureRun, usize)>())
            .saturating_add(key.text.capacity())
    }
}

/// Everything a wrapped UI paragraph's layout depends on.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct UiLayoutKey {
    pub(super) spans: Vec<(Box<str>, UiFace)>,
    pub(super) size: (u32, u32),
    pub(super) width_bits: Option<u32>,
}

impl UiLayoutKey {
    pub(super) fn matches(
        &self,
        spans: &[TextSpan<'_>],
        size: (u32, u32),
        width_bits: Option<u32>,
    ) -> bool {
        self.size == size
            && self.width_bits == width_bits
            && self.spans.len() == spans.len()
            && self
                .spans
                .iter()
                .zip(spans)
                .all(|((text, face), span)| **text == *span.text && *face == span.face)
    }
}

pub(super) fn ui_layout_hash(
    spans: &[TextSpan<'_>],
    size: (u32, u32),
    width_bits: Option<u32>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for span in spans {
        span.text.hash(&mut hasher);
        span.face.hash(&mut hasher);
    }
    size.hash(&mut hasher);
    width_bits.hash(&mut hasher);
    hasher.finish()
}

/// Most wrapped UI paragraphs kept between frames.
pub(super) const MAX_UI_LAYOUTS: usize = 2048;

/// Most paragraph text, in bytes, kept between frames.
pub(super) const MAX_UI_LAYOUT_BYTES: usize = 4 * 1024 * 1024;

/// Wrapped UI paragraphs by content, bounded by count and by text size.
/// Lookups hash the borrowed spans and compare in place, so an unchanged
/// paragraph costs no allocation per frame. When full, the least recently
/// used paragraphs go first: those on screen stay, while stale ones, such as
/// every frame's version of a streamed answer, do not pile up.
#[derive(Default)]
pub(super) struct UiLayoutCache {
    pub(super) entries: HashMap<u64, Vec<UiLayoutEntry>>,
    pub(super) count: usize,
    pub(super) bytes: usize,
    /// Advances on every use, so each entry's stamp is unique.
    pub(super) clock: u64,
}

pub(super) struct UiLayoutEntry {
    pub(super) key: UiLayoutKey,
    pub(super) layout: Arc<TextLayout>,
    pub(super) bytes: usize,
    pub(super) last_used: u64,
}

impl UiLayoutCache {
    pub(super) fn get(
        &mut self,
        spans: &[TextSpan<'_>],
        size: (u32, u32),
        width_bits: Option<u32>,
    ) -> Option<Arc<TextLayout>> {
        let hash = ui_layout_hash(spans, size, width_bits);
        let entry = self
            .entries
            .get_mut(&hash)?
            .iter_mut()
            .find(|entry| entry.key.matches(spans, size, width_bits))?;
        self.clock += 1;
        entry.last_used = self.clock;
        Some(Arc::clone(&entry.layout))
    }

    pub(super) fn insert(
        &mut self,
        spans: &[TextSpan<'_>],
        size: (u32, u32),
        width_bits: Option<u32>,
        layout: Arc<TextLayout>,
    ) {
        let bytes: usize = spans.iter().map(|span| span.text.len()).sum();
        if self.count >= MAX_UI_LAYOUTS || self.bytes + bytes > MAX_UI_LAYOUT_BYTES {
            self.evict(MAX_UI_LAYOUTS / 2, MAX_UI_LAYOUT_BYTES / 2);
        }
        self.clock += 1;
        let key = UiLayoutKey {
            spans: spans
                .iter()
                .map(|span| (Box::from(span.text), span.face))
                .collect(),
            size,
            width_bits,
        };
        self.entries
            .entry(ui_layout_hash(spans, size, width_bits))
            .or_default()
            .push(UiLayoutEntry {
                key,
                layout,
                bytes,
                last_used: self.clock,
            });
        self.count += 1;
        self.bytes += bytes;
    }

    /// Drops the least recently used paragraphs until at most `count` of
    /// them, holding at most `bytes` of text, remain.
    pub(super) fn evict(&mut self, count: usize, bytes: usize) {
        let mut ages: Vec<(u64, usize)> = self
            .entries
            .values()
            .flatten()
            .map(|entry| (entry.last_used, entry.bytes))
            .collect();
        ages.sort_unstable_by_key(|(last_used, _)| *last_used);
        let (mut remaining_count, mut remaining_bytes) = (self.count, self.bytes);
        let mut cutoff = None;
        for (last_used, size) in ages {
            if remaining_count <= count && remaining_bytes <= bytes {
                break;
            }
            remaining_count -= 1;
            remaining_bytes -= size;
            cutoff = Some(last_used);
        }
        let Some(cutoff) = cutoff else {
            return;
        };
        self.entries.retain(|_, entries| {
            entries.retain(|entry| entry.last_used > cutoff);
            !entries.is_empty()
        });
        self.count = remaining_count;
        self.bytes = remaining_bytes;
    }
}
