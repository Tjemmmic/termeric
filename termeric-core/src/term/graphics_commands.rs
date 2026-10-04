//! Kitty, Sixel, and iTerm2 image commands and the images they keep.

use super::*;

impl Terminal {
    pub(crate) fn begin_graphics_apc(&mut self) {
        self.graphics_apc_data.clear();
        self.graphics_apc_active = true;
        self.graphics_apc_overflow = false;
    }

    pub(crate) fn push_graphics_apc_byte(&mut self, byte: u8) {
        if !self.graphics_apc_active || self.graphics_apc_overflow {
            return;
        }
        if self.graphics_apc_data.len() >= MAX_KITTY_APC_BYTES {
            self.graphics_apc_overflow = true;
        } else {
            self.graphics_apc_data.push(byte);
        }
    }

    pub(crate) fn finish_graphics_apc(&mut self) {
        if !self.graphics_apc_active {
            return;
        }
        self.graphics_apc_active = false;
        if self.graphics_apc_overflow {
            self.graphics_apc_overflow = false;
            if KittyCommand::suppresses_response(&self.graphics_apc_data) {
                self.graphics_apc_data.clear();
                return;
            }
            self.graphics_apc_data.clear();
            let response = GraphicsResponse::too_large("graphics APC is too large");
            self.push_event(TerminalEvent::PtyWrite(
                response.to_escape_sequence().into_bytes(),
            ));
            return;
        }
        let payload = std::mem::take(&mut self.graphics_apc_data);
        self.handle_graphics_command(&payload);
    }

    pub(crate) fn cancel_graphics_apc(&mut self) {
        self.graphics_apc_data.clear();
        self.graphics_apc_active = false;
        self.graphics_apc_overflow = false;
    }

    /// Handle one complete Kitty graphics APC command.
    pub fn handle_graphics_command(&mut self, payload: &[u8]) {
        match KittyCommand::parse(payload) {
            Ok(mut cmd) => {
                // Record cursor position for placement
                let cursor = self.grid().cursor();
                cmd.placement.col = Some(cursor.col as u32);
                cmd.placement.row = Some(cursor.row as u32);
                let result = self.graphics.handle_command(cmd, self.alt_screen_active);
                if let Some((cols, rows)) = result.cursor_movement {
                    let terminal_rows = self.rows;
                    let terminal_cols = self.cols;
                    self.grid_mut().set_cursor(
                        cursor
                            .row
                            .saturating_add(rows as usize)
                            .min(terminal_rows - 1),
                        cursor
                            .col
                            .saturating_add(cols as usize)
                            .min(terminal_cols - 1),
                    );
                }
                if let Some(response) = result.response {
                    log::trace!("Graphics response: {:?}", response);
                    // Emit the response as Kitty graphics ack back to the PTY,
                    // so apps probing for image-protocol support (e.g. `icat`
                    // in Kitty-compatible mode) get the expected reply.
                    let bytes = response.to_escape_sequence().into_bytes();
                    self.push_event(crate::event::TerminalEvent::PtyWrite(bytes));
                }
            }
            Err(error) => {
                log::debug!("Rejected Kitty graphics command: {error}");
                self.graphics.abort_pending();
                if KittyCommand::suppresses_response(payload) {
                    return;
                }
                let response = GraphicsResponse::parse_error(&error.to_string());
                self.push_event(TerminalEvent::PtyWrite(
                    response.to_escape_sequence().into_bytes(),
                ));
            }
        }
    }

    pub(crate) fn handle_sixel_command(&mut self, payload: &[u8]) {
        let cursor = self.grid().cursor();
        match self.graphics.handle_sixel(
            payload,
            cursor.col as u32,
            cursor.row as i64,
            (self.cell_pixel_width, self.cell_pixel_height),
            self.alt_screen_active,
        ) {
            Ok((cols, rows)) => {
                log::debug!("Accepted Sixel image: cursor advance {cols}x{rows} cells");
                self.move_after_sixel_image(cursor, rows);
            }
            Err(error) => log::debug!("Rejected Sixel image: {}", error.message),
        }
    }

    pub(crate) fn handle_iterm2_image(&mut self, params: &[&[u8]]) {
        let cursor = self.grid().cursor();
        match self.graphics.handle_iterm2(
            params,
            (cursor.col as u32, cursor.row as i64),
            (self.cols as u32, self.rows as u32),
            (self.cell_pixel_width, self.cell_pixel_height),
            self.alt_screen_active,
        ) {
            Ok(Some((cols, rows))) => {
                log::debug!("Accepted iTerm2 inline image: cursor advance {cols}x{rows} cells");
                self.move_after_inline_image(cursor, cols, rows);
            }
            Ok(None) => {}
            Err(error) => log::debug!("Rejected iTerm2 inline image: {}", error.message),
        }
    }

    fn move_after_inline_image(&mut self, cursor: crate::grid::Cursor, cols: u32, rows: u32) {
        let row = cursor
            .row
            .saturating_add(rows as usize)
            .min(self.rows.saturating_sub(1));
        let col = if cols == 0 {
            0
        } else {
            cursor
                .col
                .saturating_add(cols as usize)
                .min(self.cols.saturating_sub(1))
        };
        self.grid_mut().set_cursor(row, col);
    }

    fn move_after_sixel_image(&mut self, cursor: crate::grid::Cursor, rows: u32) {
        let row = cursor
            .row
            .saturating_add(rows as usize)
            .min(self.rows.saturating_sub(1));
        let col = cursor.col.min(self.cols.saturating_sub(1));
        self.grid_mut().set_cursor(row, col);
    }

    /// Get all image placements for rendering
    pub fn graphics_placements(&self) -> impl Iterator<Item = &ImagePlacement> {
        self.graphics.placements(self.alt_screen_active)
    }

    /// Image placements and decoded data corresponding to the presentation
    /// grid. Snapshot ownership keeps deletions and ID reuse during a
    /// synchronized update from leaking into an intermediate frame.
    pub fn presentation_images(&self) -> Vec<PresentationImage> {
        if let Some(snapshot) = &self.synchronized_output_snapshot {
            snapshot.graphics.presentation_images(&snapshot.grid)
        } else {
            self.graphics.presentation_images(
                self.grid(),
                self.alt_screen_active,
                self.cell_pixel_width,
                self.cell_pixel_height,
            )
        }
    }

    /// Next exact frame boundary for a visible Kitty animation.
    pub fn graphics_animation_deadline(&self) -> Option<Instant> {
        self.graphics
            .next_animation_deadline(self.alt_screen_active)
    }

    /// Advance visible Kitty animations through every elapsed frame boundary.
    pub fn advance_graphics_animations(&mut self, now: Instant) -> bool {
        self.graphics
            .advance_animations(self.alt_screen_active, now)
    }

    /// Get image data for rendering
    pub fn get_image(&self, id: ImageId) -> Option<&StoredImage> {
        self.graphics.get_image(id)
    }

    /// Check if there are images to render
    pub fn has_images(&self) -> bool {
        self.graphics.has_images(self.alt_screen_active)
    }

    /// Decoded image bytes retained by this terminal's bounded image cache.
    pub fn graphics_memory_usage(&self) -> usize {
        self.graphics.memory_usage()
    }

    /// Decoded images retained by this terminal's bounded image cache.
    pub fn graphics_image_count(&self) -> usize {
        self.graphics.image_count()
    }

    /// Live image placements across both screen buffers.
    pub fn graphics_placement_count(&self) -> usize {
        self.graphics.placement_count()
    }

    // === Shell integration methods ===
}
