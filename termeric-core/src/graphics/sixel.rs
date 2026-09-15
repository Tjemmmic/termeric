//! Bounded DEC Sixel decoder.
//!
//! Supported controls are data bytes `?`-`~`, repeat (`!`), raster attributes
//! (`"`), color select/definition (`#`), carriage return (`$`), and graphics
//! newline (`-`). DCS macro and raster-attribute pixel aspects are retained as
//! bounded display geometry while RGB/HLS pixels remain compact RGBA source
//! data.

use super::{DecodedImage, MAX_IMAGE_DIMENSION, ProtocolError, checked_pixel_bytes};

const MAX_SIXEL_BYTES: usize = 16 * 1024 * 1024;
const MAX_REPEAT: u32 = 1_000_000;
const MAX_DRAW_OPERATIONS: u64 = 64 * 1024 * 1024;
const MAX_PALETTE_REGISTERS: usize = 1024;

#[derive(Debug, Clone)]
pub(crate) struct SixelCommand<'a> {
    background_transparent: bool,
    aspect: PixelAspect,
    data: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelAspect {
    vertical: u32,
    horizontal: u32,
}

pub(crate) struct DecodedSixel {
    pub(crate) image: DecodedImage,
    pub(crate) display_width: u32,
    pub(crate) display_height: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Bounds {
    width: u32,
    height: u32,
    declared_width: u32,
    declared_height: u32,
}

struct Canvas {
    width: u32,
    pixels: Vec<u8>,
    palette: Vec<[u8; 4]>,
    current_color: usize,
}

impl<'a> SixelCommand<'a> {
    pub(crate) fn parse(payload: &'a [u8]) -> Result<Self, ProtocolError> {
        if payload.len() > MAX_SIXEL_BYTES {
            return Err(ProtocolError::too_large("Sixel payload is too large"));
        }
        let Some(q) = payload.iter().position(|byte| *byte == b'q') else {
            return Err(ProtocolError::invalid("Sixel DCS is missing q"));
        };
        let header = &payload[..q];
        if !header
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
        {
            return Err(ProtocolError::invalid("invalid Sixel DCS parameters"));
        }
        let parameters = parse_parameter_list(header, 3)?;
        let background = parameters.get(1).copied().unwrap_or(0);
        if background > 2 {
            return Err(ProtocolError::invalid("invalid Sixel background selector"));
        }
        let background_transparent = background == 1;
        let aspect = macro_pixel_aspect(parameters.first().copied())?;
        Ok(Self {
            background_transparent,
            aspect,
            data: &payload[q + 1..],
        })
    }

    pub(crate) fn decode(&self) -> Result<DecodedSixel, ProtocolError> {
        let (bounds, aspect) = walk(self.data, self.aspect, None)?;
        let width = bounds.width.max(bounds.declared_width).max(1);
        let height = bounds.height.max(bounds.declared_height).max(1);
        let (display_width, display_height) = aspect.display_size(width, height)?;
        let bytes = checked_pixel_bytes(width, height, 4)?;
        let mut palette = default_palette();
        let background = if self.background_transparent {
            [0, 0, 0, 0]
        } else {
            palette[0]
        };
        let mut pixels = vec![0; bytes];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&background);
        }
        let mut canvas = Canvas {
            width,
            pixels,
            palette: std::mem::take(&mut palette),
            current_color: 0,
        };
        walk(self.data, self.aspect, Some(&mut canvas))?;
        Ok(DecodedSixel {
            image: DecodedImage {
                width,
                height,
                data: canvas.pixels,
            },
            display_width,
            display_height,
        })
    }
}

impl PixelAspect {
    fn new(vertical: u32, horizontal: u32) -> Self {
        Self {
            vertical: vertical.max(1),
            horizontal: horizontal.max(1),
        }
    }

    fn display_size(self, width: u32, height: u32) -> Result<(u32, u32), ProtocolError> {
        let (display_width, display_height) = if self.vertical >= self.horizontal {
            (
                width,
                scale_dimension(height, self.vertical, self.horizontal)?,
            )
        } else {
            (
                scale_dimension(width, self.horizontal, self.vertical)?,
                height,
            )
        };
        if display_width > MAX_IMAGE_DIMENSION || display_height > MAX_IMAGE_DIMENSION {
            return Err(ProtocolError::too_large(
                "Sixel display dimensions exceed limit",
            ));
        }
        Ok((display_width.max(1), display_height.max(1)))
    }
}

fn macro_pixel_aspect(parameter: Option<u32>) -> Result<PixelAspect, ProtocolError> {
    let vertical = match parameter {
        None | Some(0 | 1 | 5 | 6) => 2,
        Some(2) => 5,
        Some(3 | 4) => 3,
        Some(7..=9) => 1,
        Some(_) => return Err(ProtocolError::invalid("invalid Sixel macro parameter")),
    };
    Ok(PixelAspect::new(vertical, 1))
}

fn scale_dimension(value: u32, numerator: u32, denominator: u32) -> Result<u32, ProtocolError> {
    let product = u64::from(value)
        .checked_mul(u64::from(numerator))
        .ok_or_else(|| ProtocolError::too_large("Sixel pixel aspect overflow"))?;
    let denominator = u64::from(denominator);
    let scaled = product
        .checked_add(denominator / 2)
        .ok_or_else(|| ProtocolError::too_large("Sixel pixel aspect overflow"))?
        / denominator;
    u32::try_from(scaled.max(1))
        .map_err(|_| ProtocolError::too_large("Sixel pixel aspect overflow"))
}

fn walk(
    data: &[u8],
    initial_aspect: PixelAspect,
    mut canvas: Option<&mut Canvas>,
) -> Result<(Bounds, PixelAspect), ProtocolError> {
    let mut index = 0;
    let mut x = 0_u32;
    let mut y = 0_u32;
    let mut bounds = Bounds::default();
    let mut operations = 0_u64;
    let mut saw_pixels = false;
    let mut aspect = initial_aspect;

    while index < data.len() {
        match data[index] {
            b'?'..=b'~' => {
                draw_sixel(
                    data[index],
                    1,
                    &mut x,
                    y,
                    &mut bounds,
                    &mut operations,
                    canvas.as_deref_mut(),
                )?;
                saw_pixels = true;
                index += 1;
            }
            b'!' => {
                index += 1;
                let repeat = parse_decimal(data, &mut index)?;
                if repeat == 0 || repeat > MAX_REPEAT {
                    return Err(ProtocolError::too_large("invalid Sixel repeat count"));
                }
                let Some(&sixel @ b'?'..=b'~') = data.get(index) else {
                    return Err(ProtocolError::invalid("Sixel repeat has no data byte"));
                };
                draw_sixel(
                    sixel,
                    repeat,
                    &mut x,
                    y,
                    &mut bounds,
                    &mut operations,
                    canvas.as_deref_mut(),
                )?;
                saw_pixels = true;
                index += 1;
            }
            b'$' => {
                x = 0;
                index += 1;
            }
            b'-' => {
                x = 0;
                y = y
                    .checked_add(6)
                    .ok_or_else(|| ProtocolError::too_large("Sixel height overflow"))?;
                if y >= MAX_IMAGE_DIMENSION {
                    return Err(ProtocolError::too_large("Sixel height exceeds limit"));
                }
                index += 1;
            }
            b'"' => {
                if saw_pixels {
                    return Err(ProtocolError::invalid(
                        "Sixel raster attributes must precede pixel data",
                    ));
                }
                index += 1;
                let values = parse_inline_parameters(data, &mut index, 4)?;
                let pan = values.first().copied().unwrap_or(1);
                let pad = values.get(1).copied().unwrap_or(1);
                aspect = PixelAspect::new(pan, pad);
                bounds.declared_width = values.get(2).copied().unwrap_or(0);
                bounds.declared_height = values.get(3).copied().unwrap_or(0);
                if bounds.declared_width > MAX_IMAGE_DIMENSION
                    || bounds.declared_height > MAX_IMAGE_DIMENSION
                {
                    return Err(ProtocolError::too_large(
                        "Sixel raster dimensions exceed limit",
                    ));
                }
            }
            b'#' => {
                index += 1;
                let register = parse_decimal(data, &mut index)? as usize;
                if register >= MAX_PALETTE_REGISTERS {
                    return Err(ProtocolError::too_large(
                        "Sixel palette register exceeds limit",
                    ));
                }
                if let Some(canvas) = canvas.as_deref_mut() {
                    canvas.current_color = register;
                }
                if data.get(index) == Some(&b';') {
                    index += 1;
                    let mode = parse_decimal(data, &mut index)?;
                    expect_separator(data, &mut index)?;
                    let first = parse_decimal(data, &mut index)?;
                    expect_separator(data, &mut index)?;
                    let second = parse_decimal(data, &mut index)?;
                    expect_separator(data, &mut index)?;
                    let third = parse_decimal(data, &mut index)?;
                    let color = match mode {
                        1 => hls(first, second, third)?,
                        2 => rgb_percent(first, second, third)?,
                        _ => {
                            return Err(ProtocolError::unsupported("Sixel palette color space"));
                        }
                    };
                    if let Some(canvas) = canvas.as_deref_mut() {
                        if canvas.palette.len() <= register {
                            canvas.palette.resize(register + 1, [0, 0, 0, 255]);
                        }
                        canvas.palette[register] = color;
                    }
                }
            }
            0x00..=0x20 => index += 1,
            _ => return Err(ProtocolError::invalid("unsupported Sixel control byte")),
        }
    }
    if bounds.width.max(bounds.declared_width) == 0
        || bounds.height.max(bounds.declared_height) == 0
    {
        return Err(ProtocolError::invalid("Sixel image contains no raster"));
    }
    checked_pixel_bytes(
        bounds.width.max(bounds.declared_width),
        bounds.height.max(bounds.declared_height),
        4,
    )?;
    aspect.display_size(
        bounds.width.max(bounds.declared_width),
        bounds.height.max(bounds.declared_height),
    )?;
    Ok((bounds, aspect))
}

#[allow(clippy::too_many_arguments)]
fn draw_sixel(
    sixel: u8,
    repeat: u32,
    x: &mut u32,
    y: u32,
    bounds: &mut Bounds,
    operations: &mut u64,
    canvas: Option<&mut Canvas>,
) -> Result<(), ProtocolError> {
    let end_x = x
        .checked_add(repeat)
        .ok_or_else(|| ProtocolError::too_large("Sixel width overflow"))?;
    if end_x > MAX_IMAGE_DIMENSION || y.saturating_add(6) > MAX_IMAGE_DIMENSION {
        return Err(ProtocolError::too_large("Sixel dimensions exceed limit"));
    }
    *operations = operations.saturating_add(u64::from(repeat) * 6);
    if *operations > MAX_DRAW_OPERATIONS {
        return Err(ProtocolError::too_large("Sixel operation budget exceeded"));
    }
    bounds.width = bounds.width.max(end_x);
    bounds.height = bounds.height.max(y + 6);
    if let Some(canvas) = canvas {
        let bits = sixel - b'?';
        let color = canvas
            .palette
            .get(canvas.current_color)
            .copied()
            .unwrap_or([0, 0, 0, 255]);
        for column in *x..end_x {
            for bit in 0..6 {
                if bits & (1 << bit) == 0 {
                    continue;
                }
                let row = y + bit;
                let pixel = (u64::from(row) * u64::from(canvas.width) + u64::from(column)) * 4;
                let pixel = usize::try_from(pixel)
                    .map_err(|_| ProtocolError::too_large("Sixel pixel offset overflow"))?;
                canvas.pixels[pixel..pixel + 4].copy_from_slice(&color);
            }
        }
    }
    *x = end_x;
    Ok(())
}

fn parse_decimal(data: &[u8], index: &mut usize) -> Result<u32, ProtocolError> {
    let start = *index;
    let mut value = 0_u32;
    while let Some(byte) = data.get(*index).filter(|byte| byte.is_ascii_digit()) {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u32::from(*byte - b'0')))
            .ok_or_else(|| ProtocolError::too_large("Sixel number overflow"))?;
        *index += 1;
    }
    if *index == start {
        return Err(ProtocolError::invalid("Sixel number is missing"));
    }
    Ok(value)
}

fn expect_separator(data: &[u8], index: &mut usize) -> Result<(), ProtocolError> {
    if data.get(*index) != Some(&b';') {
        return Err(ProtocolError::invalid(
            "Sixel parameter separator is missing",
        ));
    }
    *index += 1;
    Ok(())
}

fn parse_inline_parameters(
    data: &[u8],
    index: &mut usize,
    maximum: usize,
) -> Result<Vec<u32>, ProtocolError> {
    let mut values = Vec::new();
    while values.len() < maximum && data.get(*index).is_some_and(u8::is_ascii_digit) {
        values.push(parse_decimal(data, index)?);
        if data.get(*index) != Some(&b';') {
            break;
        }
        *index += 1;
    }
    Ok(values)
}

fn parse_parameter_list(data: &[u8], maximum: usize) -> Result<Vec<u32>, ProtocolError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<_> = data.split(|byte| *byte == b';').collect();
    if parts.len() > maximum {
        return Err(ProtocolError::invalid("too many Sixel DCS parameters"));
    }
    let mut values = Vec::new();
    for part in parts {
        if part.is_empty() {
            values.push(0);
            continue;
        }
        let text = std::str::from_utf8(part)
            .map_err(|_| ProtocolError::invalid("invalid Sixel parameter"))?;
        values.push(
            text.parse()
                .map_err(|_| ProtocolError::too_large("Sixel parameter overflow"))?,
        );
    }
    Ok(values)
}

fn rgb_percent(red: u32, green: u32, blue: u32) -> Result<[u8; 4], ProtocolError> {
    if red > 100 || green > 100 || blue > 100 {
        return Err(ProtocolError::invalid(
            "Sixel RGB components must be percentages",
        ));
    }
    let component = |value| ((value * 255 + 50) / 100) as u8;
    Ok([component(red), component(green), component(blue), 255])
}

fn hls(hue: u32, lightness: u32, saturation: u32) -> Result<[u8; 4], ProtocolError> {
    if hue > 360 || lightness > 100 || saturation > 100 {
        return Err(ProtocolError::invalid("invalid Sixel HLS component"));
    }
    // DEC's HLS color wheel uses blue at zero rather than the CSS/HSL red
    // origin, so rotate it by 240 degrees before conversion.
    let h = ((hue + 240) % 360) as f32 / 360.0;
    let l = lightness as f32 / 100.0;
    let s = saturation as f32 / 100.0;
    let (red, green, blue) = if s == 0.0 {
        (l, l, l)
    } else {
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        (
            hue_component(p, q, h + 1.0 / 3.0),
            hue_component(p, q, h),
            hue_component(p, q, h - 1.0 / 3.0),
        )
    };
    Ok([
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8,
        255,
    ])
}

fn hue_component(p: f32, q: f32, mut value: f32) -> f32 {
    if value < 0.0 {
        value += 1.0;
    }
    if value > 1.0 {
        value -= 1.0;
    }
    if value < 1.0 / 6.0 {
        p + (q - p) * 6.0 * value
    } else if value < 0.5 {
        q
    } else if value < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - value) * 6.0
    } else {
        p
    }
}

fn default_palette() -> Vec<[u8; 4]> {
    vec![
        [0, 0, 0, 255],
        [51, 51, 204, 255],
        [204, 51, 51, 255],
        [51, 204, 51, 255],
        [204, 51, 204, 255],
        [51, 204, 204, 255],
        [204, 204, 51, 255],
        [204, 204, 204, 255],
        [102, 102, 102, 255],
        [102, 102, 255, 255],
        [255, 102, 102, 255],
        [102, 255, 102, 255],
        [255, 102, 255, 255],
        [102, 255, 255, 255],
        [255, 255, 102, 255],
        [255, 255, 255, 255],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_encoder_fixture_decodes_repeat_raster_and_rgb_palette() {
        let decoded = SixelCommand::parse(b"0;1;0q\"1;1;4;6#1;2;100;0;0!4~")
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.image.width, decoded.image.height), (4, 6));
        assert_eq!((decoded.display_width, decoded.display_height), (4, 6));
        assert_eq!(decoded.image.data.len(), 4 * 6 * 4);
        assert!(
            decoded
                .image
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [255, 0, 0, 255])
        );
    }

    #[test]
    fn transparent_background_and_graphics_newline_are_preserved() {
        let decoded = SixelCommand::parse(b"0;1q#2;2;0;100;0@?-$@?")
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.image.width, decoded.image.height), (2, 12));
        assert_eq!((decoded.display_width, decoded.display_height), (2, 24));
        assert_eq!(&decoded.image.data[..4], &[0, 255, 0, 255]);
        assert!(decoded.image.data[4..8].iter().all(|value| *value == 0));
    }

    #[test]
    fn undefined_background_selectors_are_rejected() {
        assert!(SixelCommand::parse(b"0;3q~").is_err());
        assert!(SixelCommand::parse(b"0;4294967295q~").is_err());
        assert!(SixelCommand::parse(b"0;2q~").is_ok());
    }

    #[test]
    fn macro_and_raster_pixel_aspects_produce_bounded_display_geometry() {
        let default_macro = SixelCommand::parse(b"q!2~").unwrap().decode().unwrap();
        assert_eq!(
            (default_macro.image.width, default_macro.image.height),
            (2, 6)
        );
        assert_eq!(
            (default_macro.display_width, default_macro.display_height),
            (2, 12)
        );

        for (parameter, vertical) in [
            (0, 2),
            (1, 2),
            (2, 5),
            (3, 3),
            (4, 3),
            (5, 2),
            (6, 2),
            (7, 1),
            (8, 1),
            (9, 1),
        ] {
            let payload = format!("{parameter}q!2~");
            let decoded = SixelCommand::parse(payload.as_bytes())
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(
                (decoded.display_width, decoded.display_height),
                (2, 6 * vertical),
                "macro parameter {parameter}"
            );
        }

        let tall = SixelCommand::parse(b"7q\"2;1;2;6!2~")
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((tall.image.width, tall.image.height), (2, 6));
        assert_eq!((tall.display_width, tall.display_height), (2, 12));
        assert_eq!(tall.image.data.len(), 2 * 6 * 4);

        let wide = SixelCommand::parse(b"0q\"1;2;2;6!2~")
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((wide.image.width, wide.image.height), (2, 6));
        assert_eq!((wide.display_width, wide.display_height), (4, 6));

        let near_square = SixelCommand::parse(b"0q\"1000;999;2;6!2~")
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(
            (near_square.display_width, near_square.display_height),
            (2, 6)
        );
    }

    #[test]
    fn malformed_dimensions_repeats_aspects_and_palettes_fail_before_allocation() {
        for payload in [
            b"q!999999999~".as_slice(),
            b"0;0;0;0q~",
            b"10q~",
            b"q\"999999;1;1;1~",
            b"q#999999~",
            b"q\"1;1;999999;1~",
            b"q!12",
        ] {
            assert!(
                SixelCommand::parse(payload)
                    .and_then(|command| command.decode())
                    .is_err()
            );
        }
    }

    #[test]
    fn dec_hls_hue_origin_is_blue() {
        assert_eq!(hls(0, 50, 100).unwrap(), [0, 0, 255, 255]);
        assert_eq!(hls(120, 50, 100).unwrap(), [255, 0, 0, 255]);
        assert_eq!(hls(240, 50, 100).unwrap(), [0, 255, 0, 255]);
    }
}
