//! The live-domain graphics relay: exporting and importing image state.

use super::*;

const MAX_GRAPHICS_RELAY_APC_BYTES: usize = 64 * 1024;
fn graphics_relay_checksum(bytes: &[u8]) -> u64 {
    crate::checksum::fnv1a64(bytes)
}

impl Terminal {
    /// Permit one authenticated live-domain relay to restore decoded graphics
    /// state. The permission survives the replica's leading RIS, consumes
    /// itself on success or failure, and is never enabled for ordinary PTYs.
    pub fn set_graphics_relay_snapshot_allowed(&mut self, allowed: bool) {
        self.graphics_relay_import_allowed = allowed;
        if !allowed {
            self.graphics_relay_apc_data.clear();
            self.graphics_relay_apc_active = false;
            self.graphics_relay_apc_overflow = false;
            self.pending_graphics_relay_import = None;
        }
    }

    /// Capture the complete decoded graphics namespace without copying pixel
    /// buffers. Animation clocks are advanced to the atomic attachment point.
    pub fn graphics_relay_snapshot(&mut self, now: Instant) -> GraphicsRelaySnapshot {
        self.graphics
            .advance_animations(self.alt_screen_active, now);
        self.graphics.relay_snapshot(now)
    }

    pub(crate) fn begin_graphics_relay_apc(&mut self) {
        self.graphics_relay_apc_data.clear();
        self.graphics_relay_apc_active = self.graphics_relay_import_allowed;
        self.graphics_relay_apc_overflow = false;
    }

    pub(crate) fn push_graphics_relay_apc_byte(&mut self, byte: u8) {
        if !self.graphics_relay_apc_active || self.graphics_relay_apc_overflow {
            return;
        }
        if self.graphics_relay_apc_data.len() >= MAX_GRAPHICS_RELAY_APC_BYTES {
            self.graphics_relay_apc_data.clear();
            self.graphics_relay_apc_overflow = true;
        } else {
            self.graphics_relay_apc_data.push(byte);
        }
    }

    pub(crate) fn finish_graphics_relay_apc(&mut self) {
        if !self.graphics_relay_apc_active {
            return;
        }
        self.graphics_relay_apc_active = false;
        if self.graphics_relay_apc_overflow {
            self.graphics_relay_apc_overflow = false;
            self.fail_graphics_relay_import("graphics relay APC exceeds its resource limit");
            return;
        }
        let payload = std::mem::take(&mut self.graphics_relay_apc_data);
        if let Err(error) = self.handle_graphics_relay_apc(&payload) {
            self.fail_graphics_relay_import(&error);
        }
    }

    pub(crate) fn cancel_graphics_relay_apc(&mut self) {
        self.graphics_relay_apc_data.clear();
        self.graphics_relay_apc_active = false;
        self.graphics_relay_apc_overflow = false;
        if self.pending_graphics_relay_import.is_some() {
            self.fail_graphics_relay_import("graphics relay APC was interrupted");
        }
    }

    fn handle_graphics_relay_apc(&mut self, payload: &[u8]) -> Result<(), String> {
        let payload = std::str::from_utf8(payload)
            .map_err(|_| "graphics relay APC is not ASCII".to_string())?;
        let mut fields = payload.split(';');
        if fields.next() != Some("G") {
            return Err("graphics relay APC has an invalid namespace".into());
        }
        match fields.next() {
            Some("begin") => {
                let expected_bytes = fields
                    .next()
                    .and_then(|value| value.parse::<usize>().ok())
                    .ok_or_else(|| "graphics relay begin has an invalid length".to_string())?;
                let expected_checksum = fields
                    .next()
                    .and_then(|value| u64::from_str_radix(value, 16).ok())
                    .ok_or_else(|| "graphics relay begin has an invalid checksum".to_string())?;
                if fields.next().is_some() || expected_bytes > MAX_GRAPHICS_RELAY_METADATA_BYTES {
                    return Err("graphics relay begin exceeds its resource limit".into());
                }
                self.graphics.reset_for_relay_snapshot();
                self.pending_graphics_relay_import = Some(PendingGraphicsRelayImport {
                    expected_bytes,
                    expected_checksum,
                    metadata: Vec::new(),
                    started_at: Instant::now(),
                });
            }
            Some("chunk") => {
                let encoded = fields
                    .next()
                    .ok_or_else(|| "graphics relay chunk omits data".to_string())?;
                if fields.next().is_some() {
                    return Err("graphics relay chunk has trailing fields".into());
                }
                let decoded = STANDARD
                    .decode(encoded)
                    .map_err(|_| "graphics relay chunk is not valid base64".to_string())?;
                let pending = self
                    .pending_graphics_relay_import
                    .as_mut()
                    .ok_or_else(|| "graphics relay chunk arrived before begin".to_string())?;
                let new_len = pending
                    .metadata
                    .len()
                    .checked_add(decoded.len())
                    .ok_or_else(|| "graphics relay metadata length overflow".to_string())?;
                if new_len > pending.expected_bytes || new_len > MAX_GRAPHICS_RELAY_METADATA_BYTES {
                    return Err("graphics relay metadata exceeds its declared length".into());
                }
                pending.metadata.extend_from_slice(&decoded);
            }
            Some("end") => {
                if fields.next().is_some() {
                    return Err("graphics relay end has trailing fields".into());
                }
                let pending = self
                    .pending_graphics_relay_import
                    .take()
                    .ok_or_else(|| "graphics relay end arrived before begin".to_string())?;
                if pending.metadata.len() != pending.expected_bytes
                    || graphics_relay_checksum(&pending.metadata) != pending.expected_checksum
                {
                    return Err("graphics relay metadata is truncated or corrupt".into());
                }
                self.graphics
                    .apply_relay_metadata(&pending.metadata, pending.started_at)?;
                self.graphics
                    .advance_animations(self.alt_screen_active, Instant::now());
                log::debug!(
                    "Restored live-domain graphics snapshot: {} images, {} placements, {} decoded bytes",
                    self.graphics.image_count(),
                    self.graphics.placement_count(),
                    self.graphics.memory_usage()
                );
                self.graphics_relay_import_allowed = false;
            }
            _ => return Err("graphics relay APC has an invalid operation".into()),
        }
        Ok(())
    }

    fn fail_graphics_relay_import(&mut self, message: &str) {
        log::warn!("Rejected live-domain graphics snapshot: {message}");
        self.pending_graphics_relay_import = None;
        self.graphics_relay_import_allowed = false;
        self.graphics.reset_for_relay_snapshot();
    }
}
