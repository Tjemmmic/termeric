//! Placement geometry: display sizes, cropping, and placeholder
//! presentation.

use super::*;

pub(super) fn placement_intersects(placement: &ImagePlacement, col: u32, row: i64) -> bool {
    col >= placement.col
        && col < placement.col.saturating_add(placement.width.unwrap_or(1))
        && row >= placement.row
        && row
            < placement
                .row
                .saturating_add(i64::from(placement.height.unwrap_or(1)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KittyDisplayGeometry {
    pub(super) columns: u32,
    pub(super) rows: u32,
    pub(super) pixel_width: Option<u32>,
    pub(super) pixel_height: Option<u32>,
}

pub(super) fn resolve_kitty_display_geometry(
    columns: Option<u32>,
    rows: Option<u32>,
    source_width: u32,
    source_height: u32,
    cell_width: u32,
    cell_height: u32,
) -> KittyDisplayGeometry {
    let cell_width = cell_width.max(1);
    let cell_height = cell_height.max(1);
    let source_width = source_width.max(1);
    let source_height = source_height.max(1);
    let scaled = |extent: u32, numerator: u32, denominator: u32| {
        u64::from(extent)
            .saturating_mul(u64::from(numerator))
            .div_ceil(u64::from(denominator))
            .min(u64::from(u32::MAX)) as u32
    };

    match (columns, rows) {
        (Some(columns), Some(rows)) => KittyDisplayGeometry {
            columns,
            rows,
            pixel_width: None,
            pixel_height: None,
        },
        (Some(columns), None) => {
            let pixel_width = columns.saturating_mul(cell_width).max(1);
            let pixel_height = scaled(pixel_width, source_height, source_width).max(1);
            KittyDisplayGeometry {
                columns,
                rows: pixel_height.div_ceil(cell_height).max(1),
                pixel_width: Some(pixel_width),
                pixel_height: Some(pixel_height),
            }
        }
        (None, Some(rows)) => {
            let pixel_height = rows.saturating_mul(cell_height).max(1);
            let pixel_width = scaled(pixel_height, source_width, source_height).max(1);
            KittyDisplayGeometry {
                columns: pixel_width.div_ceil(cell_width).max(1),
                rows,
                pixel_width: Some(pixel_width),
                pixel_height: Some(pixel_height),
            }
        }
        (None, None) => KittyDisplayGeometry {
            columns: source_width.div_ceil(cell_width).max(1),
            rows: source_height.div_ceil(cell_height).max(1),
            pixel_width: Some(source_width),
            pixel_height: Some(source_height),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlacementRect {
    pub(super) left: u32,
    pub(super) right: u32,
    pub(super) top: i64,
    pub(super) bottom: i64,
}

impl PlacementRect {
    pub(super) fn intersection(self, other: Self) -> Option<Self> {
        let intersection = Self {
            left: self.left.max(other.left),
            right: self.right.min(other.right),
            top: self.top.max(other.top),
            bottom: self.bottom.min(other.bottom),
        };
        (intersection.left < intersection.right && intersection.top < intersection.bottom)
            .then_some(intersection)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn placeholder_presentation(
    image: &StoredImage,
    prototype: &VirtualImagePlacement,
    image_col: u32,
    image_row: u32,
    screen_col: u32,
    screen_row: i64,
    cell_width: u32,
    cell_height: u32,
) -> Option<PresentationImage> {
    if image.width == 0 || image.height == 0 {
        return None;
    }
    let columns = prototype
        .columns
        .unwrap_or_else(|| image.width.saturating_add(cell_width - 1) / cell_width)
        .max(1);
    let rows = prototype
        .rows
        .unwrap_or_else(|| image.height.saturating_add(cell_height - 1) / cell_height)
        .max(1);
    if image_col >= columns || image_row >= rows {
        return None;
    }

    let box_width = f64::from(columns) * f64::from(cell_width);
    let box_height = f64::from(rows) * f64::from(cell_height);
    let scale = (box_width / f64::from(image.width)).min(box_height / f64::from(image.height));
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let drawn_width = f64::from(image.width) * scale;
    let drawn_height = f64::from(image.height) * scale;
    let image_left = (box_width - drawn_width) / 2.0;
    let image_top = (box_height - drawn_height) / 2.0;
    let cell_left = f64::from(image_col) * f64::from(cell_width);
    let cell_top = f64::from(image_row) * f64::from(cell_height);
    let visible_left = cell_left.max(image_left);
    let visible_top = cell_top.max(image_top);
    let visible_right = (cell_left + f64::from(cell_width)).min(image_left + drawn_width);
    let visible_bottom = (cell_top + f64::from(cell_height)).min(image_top + drawn_height);
    if visible_right <= visible_left || visible_bottom <= visible_top {
        return None;
    }

    let u_min = ((visible_left - image_left) / drawn_width).clamp(0.0, 1.0);
    let v_min = ((visible_top - image_top) / drawn_height).clamp(0.0, 1.0);
    let u_max = ((visible_right - image_left) / drawn_width).clamp(0.0, 1.0);
    let v_max = ((visible_bottom - image_top) / drawn_height).clamp(0.0, 1.0);
    Some(PresentationImage {
        placement: ImagePlacement {
            image_id: image.id,
            placement_id: prototype.placement_id,
            col: screen_col,
            row: screen_row,
            alternate_screen: prototype.alternate_screen,
            source_x: 0,
            source_y: 0,
            source_width: Some(image.width),
            source_height: Some(image.height),
            x_offset: 0,
            y_offset: 0,
            width: Some(1),
            height: Some(1),
            pixel_width: None,
            pixel_height: None,
            z_index: -1,
        },
        image: image.clone(),
        pixel_rect: Some([
            (visible_left - cell_left) as f32,
            (visible_top - cell_top) as f32,
            (visible_right - visible_left) as f32,
            (visible_bottom - visible_top) as f32,
        ]),
        source_uv: Some([u_min as f32, v_min as f32, u_max as f32, v_max as f32]),
    })
}

pub(super) fn merge_adjacent_placeholder_presentation(
    previous: Option<&mut PresentationImage>,
    next: &PresentationImage,
    cell_width: u32,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let (Some(previous_rect), Some(next_rect), Some(previous_uv), Some(next_uv)) = (
        previous.pixel_rect,
        next.pixel_rect,
        previous.source_uv,
        next.source_uv,
    ) else {
        return false;
    };
    let same_row = previous.placement.row == next.placement.row;
    let same_image = previous.image.id == next.image.id
        && previous.image.revision == next.image.revision
        && previous.placement.placement_id == next.placement.placement_id;
    let same_vertical_geometry = previous_rect[1] == next_rect[1]
        && previous_rect[3] == next_rect[3]
        && previous_uv[1] == next_uv[1]
        && previous_uv[3] == next_uv[3];
    let cell_width = cell_width as f32;
    let previous_left = previous.placement.col as f32 * cell_width + previous_rect[0];
    let previous_right = previous_left + previous_rect[2];
    let next_left = next.placement.col as f32 * cell_width + next_rect[0];
    let contiguous_pixels = (previous_right - next_left).abs() <= 0.01;
    let contiguous_texture = (previous_uv[2] - next_uv[0]).abs() <= f32::EPSILON * 8.0;
    if !same_row
        || !same_image
        || !same_vertical_geometry
        || !contiguous_pixels
        || !contiguous_texture
    {
        return false;
    }

    let next_right = next_left + next_rect[2];
    previous.pixel_rect.as_mut().unwrap()[2] = next_right - previous_left;
    previous.source_uv.as_mut().unwrap()[2] = next_uv[2];
    previous.placement.width = Some(
        next.placement
            .col
            .saturating_sub(previous.placement.col)
            .saturating_add(1),
    );
    true
}

pub(super) fn placement_rect(placement: &ImagePlacement) -> PlacementRect {
    PlacementRect {
        left: placement.col,
        right: placement.col.saturating_add(placement.width.unwrap_or(1)),
        top: placement.row,
        bottom: placement
            .row
            .saturating_add(i64::from(placement.height.unwrap_or(1))),
    }
}

pub(super) fn crop_placement(
    placement: &ImagePlacement,
    requested: PlacementRect,
    cell_width: u32,
    cell_height: u32,
) -> Option<ImagePlacement> {
    let bounds = placement_rect(placement);
    let crop = bounds.intersection(requested)?;
    let width = bounds.right - bounds.left;
    let height = u32::try_from(bounds.bottom - bounds.top).ok()?;
    let left = crop.left - bounds.left;
    let right = crop.right - bounds.left;
    let top = u32::try_from(crop.top - bounds.top).ok()?;
    let bottom = u32::try_from(crop.bottom - bounds.top).ok()?;

    let source_width = placement.source_width.unwrap_or(0);
    let source_height = placement.source_height.unwrap_or(0);
    let horizontal = crop_axis(
        source_width,
        placement.pixel_width,
        placement.x_offset,
        cell_width,
        left..right,
        width,
    )?;
    let vertical = crop_axis(
        source_height,
        placement.pixel_height,
        placement.y_offset,
        cell_height,
        top..bottom,
        height,
    )?;

    let mut result = placement.clone();
    result.col = crop.left;
    result.row = crop.top;
    result.source_x = result.source_x.saturating_add(horizontal.source_start);
    result.source_y = result.source_y.saturating_add(vertical.source_start);
    result.source_width = Some(horizontal.source_extent);
    result.source_height = Some(vertical.source_extent);
    result.width = Some(crop.right - crop.left);
    result.height = Some(u32::try_from(crop.bottom - crop.top).ok()?);
    result.x_offset = horizontal.offset;
    result.y_offset = vertical.offset;
    result.pixel_width = horizontal.pixel_extent;
    result.pixel_height = vertical.pixel_extent;
    result.update_cell_footprint(cell_width, cell_height);
    Some(result)
}

pub(super) struct AxisCrop {
    pub(super) source_start: u32,
    pub(super) source_extent: u32,
    pub(super) offset: u32,
    pub(super) pixel_extent: Option<u32>,
}

/// Cell bounds are not proportional slices of a pixel-sized image: its last
/// cell may be only partly covered. Intersect in destination pixels first,
/// then map to source texels using the existing outward rounding policy.
pub(super) fn crop_axis(
    source_extent: u32,
    pixel_extent: Option<u32>,
    offset: u32,
    cell_pixels: u32,
    cells: std::ops::Range<u32>,
    total_cells: u32,
) -> Option<AxisCrop> {
    let (start, end, total, offset, pixel_extent) = if let Some(pixels) = pixel_extent {
        let cell_pixels = u64::from(cell_pixels.max(1));
        let cell_start = u64::from(cells.start) * cell_pixels;
        let cell_end = u64::from(cells.end) * cell_pixels;
        let image_start = u64::from(offset);
        let image_end = image_start + u64::from(pixels);
        let start = cell_start.max(image_start);
        let end = cell_end.min(image_end);
        if start >= end {
            return None;
        }
        (
            start - image_start,
            end - image_start,
            u64::from(pixels),
            u32::try_from(start - cell_start).ok()?,
            Some(u32::try_from(end - start).ok()?),
        )
    } else {
        (
            u64::from(cells.start),
            u64::from(cells.end),
            u64::from(total_cells),
            if cells.start == 0 { offset } else { 0 },
            None,
        )
    };
    if total == 0 || start >= end {
        return None;
    }
    let source_start = (u64::from(source_extent) * start / total) as u32;
    let source_end = (u64::from(source_extent) * end).div_ceil(total) as u32;
    Some(AxisCrop {
        source_start,
        source_extent: source_end.saturating_sub(source_start),
        offset,
        pixel_extent,
    })
}

pub(super) fn relative_negative_presentation(
    mut placement: ImagePlacement,
    image: &StoredImage,
    col: i64,
    cell_width: u32,
    cell_height: u32,
) -> Option<PresentationImage> {
    let source_x = placement.source_x.min(image.width);
    let source_y = placement.source_y.min(image.height);
    let source_width = placement
        .source_width
        .unwrap_or(image.width.saturating_sub(source_x))
        .min(image.width.saturating_sub(source_x));
    let source_height = placement
        .source_height
        .unwrap_or(image.height.saturating_sub(source_y))
        .min(image.height.saturating_sub(source_y));
    if source_width == 0 || source_height == 0 || image.width == 0 || image.height == 0 {
        return None;
    }
    let aspect = f64::from(source_width) / f64::from(source_height);
    let (width, height) = match (placement.pixel_width, placement.pixel_height) {
        (Some(width), Some(height)) => (f64::from(width), f64::from(height)),
        (Some(width), None) => {
            let width = f64::from(width);
            (width, width / aspect)
        }
        (None, Some(height)) => {
            let height = f64::from(height);
            (height * aspect, height)
        }
        (None, None) => match (placement.width, placement.height) {
            (Some(columns), Some(rows)) => (
                f64::from(columns) * f64::from(cell_width),
                f64::from(rows) * f64::from(cell_height),
            ),
            (Some(columns), None) => {
                let width = f64::from(columns) * f64::from(cell_width);
                (width, width / aspect)
            }
            (None, Some(rows)) => {
                let height = f64::from(rows) * f64::from(cell_height);
                (height * aspect, height)
            }
            (None, None) => (f64::from(source_width), f64::from(source_height)),
        },
    };
    let x = (col as f64).mul_add(f64::from(cell_width), f64::from(placement.x_offset));
    let y = f64::from(placement.y_offset);
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return None;
    }
    placement.col = 0;
    placement.x_offset = 0;
    placement.y_offset = 0;
    Some(PresentationImage {
        placement,
        image: image.clone(),
        pixel_rect: Some([x as f32, y as f32, width as f32, height as f32]),
        source_uv: Some([
            source_x as f32 / image.width as f32,
            source_y as f32 / image.height as f32,
            source_x.saturating_add(source_width) as f32 / image.width as f32,
            source_y.saturating_add(source_height) as f32 / image.height as f32,
        ]),
    })
}
