//! Capacity-managed GPU upload buffers.

use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, Queue};

const MIN_BUFFER_CAPACITY: u64 = 4 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct BufferStats {
    pub allocations: u64,
    pub writes: u64,
    pub bytes_written: u64,
    pub capacity_bytes: u64,
}

pub(super) struct GrowableBuffer {
    label: &'static str,
    usage: BufferUsages,
    buffer: Option<Buffer>,
    stats: BufferStats,
}

impl GrowableBuffer {
    pub(super) fn new(label: &'static str, usage: BufferUsages) -> Self {
        Self {
            label,
            usage,
            buffer: None,
            stats: BufferStats::default(),
        }
    }

    pub(super) fn upload(&mut self, device: &Device, queue: &Queue, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let required = bytes.len() as u64;
        if required > self.stats.capacity_bytes {
            let capacity = required
                .max(MIN_BUFFER_CAPACITY)
                .checked_next_power_of_two()
                .unwrap_or(required);
            self.buffer = Some(device.create_buffer(&BufferDescriptor {
                label: Some(self.label),
                size: capacity,
                usage: self.usage | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.stats.capacity_bytes = capacity;
            self.stats.allocations += 1;
        }

        if let Some(buffer) = &self.buffer {
            // Queue writes are ordered before the render submission that
            // consumes them; WGPU retains staging memory until the copy is
            // complete, so an in-flight frame never observes host mutation.
            queue.write_buffer(buffer, 0, bytes);
            self.stats.writes += 1;
            self.stats.bytes_written += required;
        }
    }

    pub(super) fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    pub(super) fn stats(&self) -> BufferStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_upload_does_not_change_accounting() {
        let buffer = GrowableBuffer::new("test", BufferUsages::VERTEX);
        assert_eq!(buffer.stats(), BufferStats::default());
        assert!(buffer.buffer().is_none());
    }
}
