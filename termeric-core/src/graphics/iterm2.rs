//! Safe subset of iTerm2's original OSC 1337 inline-file protocol.

use base64::{Engine as _, engine::general_purpose};

use super::{DecodedImage, ProtocolError, decode_png};

pub(crate) const MAX_ITERM2_ENCODED_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILENAME_BYTES: usize = 4 * 1024;
const MAX_DISPLAY_PIXELS: u32 = 65_535;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Iterm2Dimension {
    Auto,
    Cells(u32),
    Pixels(u32),
    Percent(u32),
}

#[derive(Debug, Clone)]
pub(crate) struct Iterm2Command<'a> {
    pub(crate) inline: bool,
    declared_size: Option<usize>,
    width: Iterm2Dimension,
    height: Iterm2Dimension,
    preserve_aspect_ratio: bool,
    encoded: &'a [u8],
}

impl<'a> Iterm2Command<'a> {
    pub(crate) fn parse(params: &'a [&'a [u8]]) -> Result<Self, ProtocolError> {
        let Some(first) = params.first() else {
            return Err(ProtocolError::invalid("empty OSC 1337 command"));
        };
        let Some(mut metadata) = first.strip_prefix(b"File=") else {
            return Err(ProtocolError::unsupported(
                "only OSC 1337 File inline images are supported",
            ));
        };

        let mut inline = false;
        let mut declared_size = None;
        let mut width = Iterm2Dimension::Auto;
        let mut height = Iterm2Dimension::Auto;
        let mut preserve_aspect_ratio = true;
        let mut encoded = None;

        for index in 0..params.len() {
            if index > 0 {
                metadata = params[index];
            }
            let (field, data) = split_at_byte(metadata, b':')
                .map_or((metadata, None), |(field, data)| (field, Some(data)));
            if !field.is_empty() {
                let (key, value) = split_at_byte(field, b'=')
                    .ok_or_else(|| ProtocolError::invalid("invalid OSC 1337 File metadata"))?;
                match key {
                    b"inline" => inline = value == b"1",
                    b"size" => declared_size = Some(parse_usize(value, "size")?),
                    b"width" => width = parse_dimension(value)?,
                    b"height" => height = parse_dimension(value)?,
                    b"preserveAspectRatio" => preserve_aspect_ratio = value != b"0",
                    b"name" => validate_filename(value)?,
                    _ => {
                        // Unknown metadata is deliberately ignored like iTerm2,
                        // but it cannot enable downloads or other OSC commands.
                    }
                }
            }
            if let Some(data) = data {
                if index + 1 != params.len() {
                    return Err(ProtocolError::invalid(
                        "inline image data must be the final OSC parameter",
                    ));
                }
                encoded = Some(data);
            }
        }
        let encoded =
            encoded.ok_or_else(|| ProtocolError::invalid("inline image data is missing"))?;
        if encoded.len() > MAX_ITERM2_ENCODED_BYTES {
            return Err(ProtocolError::too_large(
                "OSC 1337 image payload is too large",
            ));
        }
        Ok(Self {
            inline,
            declared_size,
            width,
            height,
            preserve_aspect_ratio,
            encoded,
        })
    }

    pub(crate) fn decode(&self) -> Result<DecodedImage, ProtocolError> {
        let data = general_purpose::STANDARD
            .decode(self.encoded)
            .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(self.encoded))
            .map_err(|_| ProtocolError::invalid("invalid OSC 1337 base64"))?;
        if let Some(expected) = self.declared_size
            && data.len() != expected
        {
            return Err(ProtocolError::invalid(
                "OSC 1337 size does not match decoded bytes",
            ));
        }
        if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(ProtocolError::unsupported(
                "only static PNG inline images are supported",
            ));
        }
        decode_png(data)
    }

    pub(crate) fn display_size(
        &self,
        source_width: u32,
        source_height: u32,
        terminal_cells: (u32, u32),
        cell_pixels: (u32, u32),
    ) -> Result<(u32, u32), ProtocolError> {
        let width = resolve_dimension(self.width, terminal_cells.0, cell_pixels.0)?;
        let height = resolve_dimension(self.height, terminal_cells.1, cell_pixels.1)?;
        let (width, height) = match (width, height, self.preserve_aspect_ratio) {
            (None, None, _) => (source_width, source_height),
            (Some(width), None, true) => (width, scaled_other(width, source_height, source_width)?),
            (None, Some(height), true) => {
                (scaled_other(height, source_width, source_height)?, height)
            }
            (Some(width), Some(height), true) => {
                let scale =
                    (width as f64 / source_width as f64).min(height as f64 / source_height as f64);
                (
                    (source_width as f64 * scale).round().max(1.0) as u32,
                    (source_height as f64 * scale).round().max(1.0) as u32,
                )
            }
            (Some(width), None, false) => (width, source_height),
            (None, Some(height), false) => (source_width, height),
            (Some(width), Some(height), false) => (width, height),
        };
        if width == 0 || height == 0 || width > MAX_DISPLAY_PIXELS || height > MAX_DISPLAY_PIXELS {
            return Err(ProtocolError::too_large(
                "OSC 1337 display dimensions exceed limit",
            ));
        }
        Ok((width, height))
    }
}

fn split_at_byte(data: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let index = data.iter().position(|byte| *byte == separator)?;
    Some((&data[..index], &data[index + 1..]))
}

fn validate_filename(encoded: &[u8]) -> Result<(), ProtocolError> {
    if encoded.len() > MAX_FILENAME_BYTES.div_ceil(3) * 4 {
        return Err(ProtocolError::too_large("OSC 1337 filename is too large"));
    }
    let filename = general_purpose::STANDARD
        .decode(encoded)
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encoded))
        .map_err(|_| ProtocolError::invalid("invalid OSC 1337 filename"))?;
    if filename.len() > MAX_FILENAME_BYTES || filename.contains(&0) {
        return Err(ProtocolError::invalid("unsafe OSC 1337 filename metadata"));
    }
    Ok(())
}

fn parse_dimension(value: &[u8]) -> Result<Iterm2Dimension, ProtocolError> {
    if value == b"auto" || value.is_empty() {
        return Ok(Iterm2Dimension::Auto);
    }
    if let Some(value) = value.strip_suffix(b"px") {
        return Ok(Iterm2Dimension::Pixels(parse_u32(
            value,
            "pixel dimension",
        )?));
    }
    if let Some(value) = value.strip_suffix(b"%") {
        let percent = parse_u32(value, "percent dimension")?;
        if percent > 1000 {
            return Err(ProtocolError::too_large("OSC 1337 percentage exceeds 1000"));
        }
        return Ok(Iterm2Dimension::Percent(percent));
    }
    Ok(Iterm2Dimension::Cells(parse_u32(value, "cell dimension")?))
}

fn resolve_dimension(
    dimension: Iterm2Dimension,
    terminal_cells: u32,
    cell_pixels: u32,
) -> Result<Option<u32>, ProtocolError> {
    let value = match dimension {
        Iterm2Dimension::Auto => return Ok(None),
        Iterm2Dimension::Cells(cells) => cells.checked_mul(cell_pixels),
        Iterm2Dimension::Pixels(pixels) => Some(pixels),
        Iterm2Dimension::Percent(percent) => terminal_cells
            .checked_mul(cell_pixels)
            .and_then(|pixels| pixels.checked_mul(percent))
            .map(|pixels| pixels.div_ceil(100)),
    }
    .ok_or_else(|| ProtocolError::too_large("OSC 1337 display dimension overflow"))?;
    if value == 0 || value > MAX_DISPLAY_PIXELS {
        return Err(ProtocolError::too_large(
            "OSC 1337 display dimension exceeds limit",
        ));
    }
    Ok(Some(value))
}

fn scaled_other(value: u32, other: u32, source: u32) -> Result<u32, ProtocolError> {
    let scaled = u64::from(value)
        .checked_mul(u64::from(other))
        .map(|value| value.div_ceil(u64::from(source)))
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| ProtocolError::too_large("OSC 1337 aspect ratio overflow"))?;
    Ok(scaled.max(1))
}

fn parse_u32(value: &[u8], name: &str) -> Result<u32, ProtocolError> {
    let value = std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ProtocolError::invalid(format!("invalid OSC 1337 {name}")))?;
    Ok(value)
}

fn parse_usize(value: &[u8], name: &str) -> Result<usize, ProtocolError> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| ProtocolError::invalid(format!("invalid OSC 1337 {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();
        }
        bytes
    }

    #[test]
    fn independent_imgcat_fixture_decodes_png_and_mixed_dimensions() {
        let png = png();
        let encoded = general_purpose::STANDARD.encode(&png);
        let first = b"File=name=dGVzdC5wbmc=";
        let inline = b"inline=1";
        let width = b"width=4";
        let height = b"height=20px";
        let preserve = b"preserveAspectRatio=0";
        let final_part = format!("size={}:{}", png.len(), encoded);
        let params = [
            first.as_slice(),
            inline.as_slice(),
            width.as_slice(),
            height.as_slice(),
            preserve.as_slice(),
            final_part.as_bytes(),
        ];
        let command = Iterm2Command::parse(&params).unwrap();
        let decoded = command.decode().unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 1));
        assert_eq!(
            command.display_size(2, 1, (80, 24), (10, 20)).unwrap(),
            (40, 20)
        );
    }

    #[test]
    fn aspect_ratio_and_percent_dimensions_are_bounded() {
        let params = [
            b"File=inline=1".as_slice(),
            b"width=50%".as_slice(),
            b"height=auto:AA==".as_slice(),
        ];
        let command = Iterm2Command::parse(&params).unwrap();
        assert_eq!(
            command.display_size(100, 50, (80, 24), (10, 20)).unwrap(),
            (400, 200)
        );
    }

    #[test]
    fn downloads_malformed_base64_non_png_and_oversize_metadata_are_rejected() {
        let download = [b"File=inline=0:AA==".as_slice()];
        assert!(!Iterm2Command::parse(&download).unwrap().inline);
        let malformed = [b"File=inline=1:not-base64".as_slice()];
        assert!(Iterm2Command::parse(&malformed).unwrap().decode().is_err());
        let text = general_purpose::STANDARD.encode(b"plain text");
        let payload = format!("File=inline=1:{text}");
        assert!(
            Iterm2Command::parse(&[payload.as_bytes()])
                .unwrap()
                .decode()
                .is_err()
        );
        let huge = [
            b"File=inline=1".as_slice(),
            b"width=999999px:AA==".as_slice(),
        ];
        assert!(
            Iterm2Command::parse(&huge)
                .unwrap()
                .display_size(1, 1, (80, 24), (10, 20))
                .is_err()
        );
    }
}
