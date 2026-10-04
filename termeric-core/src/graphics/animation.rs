//! Animation timing and frame composition.

use super::*;

impl AnimatedImage {
    pub(super) fn new(now: Instant) -> Self {
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

    pub(super) fn frame_count(&self) -> usize {
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

pub(super) fn animation_deadline(animation: &AnimatedImage) -> Option<Instant> {
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

pub(super) fn advance_animation_frame(animation: &mut AnimatedImage, deadline: Instant) -> bool {
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

pub(super) fn validate_frame_rectangle(
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

pub(super) fn rectangles_overlap(
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
pub(super) fn compose_rgba_overlay(
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
pub(super) fn compose_rgba_rectangle(
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
