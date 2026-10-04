//! Decoding transmitted pixels: raw RGB(A), zlib, and PNG.

use super::*;

pub(crate) struct DecodedImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data: Vec<u8>,
}

pub(super) fn decode_image(pending: &PendingImage) -> Result<DecodedImage, ProtocolError> {
    decode_payload(&pending.transmission, &pending.data)
}

pub(super) fn decode_payload(
    transmission: &KittyTransmission,
    encoded: &[u8],
) -> Result<DecodedImage, ProtocolError> {
    let data = match transmission.compression {
        KittyCompression::None => encoded.to_vec(),
        KittyCompression::Zlib => {
            let expected_size = if transmission.format == KittyFormat::Png {
                let size = transmission.data_size.ok_or_else(|| {
                    ProtocolError::invalid("S is required for compressed PNG data")
                })?;
                if size > MAX_TRANSMISSION_BYTES as u64 {
                    return Err(ProtocolError::too_large("declared data size is too large"));
                }
                Some(size)
            } else {
                None
            };
            let decoder = ZlibDecoder::new(encoded);
            let mut data = Vec::new();
            decoder
                .take((MAX_TRANSMISSION_BYTES + 1) as u64)
                .read_to_end(&mut data)
                .map_err(|_| ProtocolError::invalid("invalid zlib stream"))?;
            if data.len() > MAX_TRANSMISSION_BYTES {
                return Err(ProtocolError::too_large("decompressed data is too large"));
            }
            if let Some(expected) = expected_size
                && data.len() as u64 != expected
            {
                return Err(ProtocolError::invalid("decompressed size does not match S"));
            }
            data
        }
    };

    match transmission.format {
        KittyFormat::Rgba => decode_raw(data, transmission, 4),
        KittyFormat::Rgb => decode_raw(data, transmission, 3),
        KittyFormat::Png => decode_png(data),
    }
}

pub(crate) fn checked_pixel_bytes(
    width: u32,
    height: u32,
    channels: usize,
) -> Result<usize, ProtocolError> {
    if width == 0 || height == 0 || width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(ProtocolError::invalid("invalid image dimensions"));
    }
    let bytes = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| ProtocolError::too_large("image dimensions overflow"))?;
    if bytes > MAX_IMAGE_MEMORY {
        return Err(ProtocolError::too_large("decoded image is too large"));
    }
    Ok(bytes)
}

fn decode_raw(
    data: Vec<u8>,
    transmission: &KittyTransmission,
    channels: usize,
) -> Result<DecodedImage, ProtocolError> {
    let width = transmission
        .width
        .ok_or_else(|| ProtocolError::invalid("s is required for raw pixels"))?;
    let height = transmission
        .height
        .ok_or_else(|| ProtocolError::invalid("v is required for raw pixels"))?;
    let expected = checked_pixel_bytes(width, height, channels)?;
    if data.len() != expected {
        return Err(ProtocolError::invalid("raw pixel data size mismatch"));
    }
    let data = if channels == 4 {
        data
    } else {
        let mut rgba = Vec::with_capacity(checked_pixel_bytes(width, height, 4)?);
        for pixel in data.as_chunks::<3>().0 {
            rgba.extend_from_slice(pixel);
            rgba.push(255);
        }
        rgba
    };
    Ok(DecodedImage {
        width,
        height,
        data,
    })
}

pub(crate) fn decode_png(data: Vec<u8>) -> Result<DecodedImage, ProtocolError> {
    let limits = png::Limits {
        bytes: MAX_IMAGE_MEMORY,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(data), limits);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| ProtocolError::invalid("invalid PNG"))?;
    if reader.info().animation_control.is_some() {
        return Err(ProtocolError::unsupported("animated PNG"));
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| ProtocolError::too_large("PNG dimensions overflow"))?;
    if output_size > MAX_IMAGE_MEMORY {
        return Err(ProtocolError::too_large("decoded PNG is too large"));
    }
    let mut pixels = vec![0; output_size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| ProtocolError::invalid("invalid PNG pixels"))?;
    pixels.truncate(info.buffer_size());
    checked_pixel_bytes(info.width, info.height, 4)?;
    let mut rgba = Vec::with_capacity(checked_pixel_bytes(info.width, info.height, 4)?);
    match info.color_type {
        png::ColorType::Rgba => rgba = pixels,
        png::ColorType::Rgb => {
            for pixel in pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
        }
        png::ColorType::Grayscale => {
            for value in pixels {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in pixels.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(ProtocolError::invalid("PNG palette expansion failed"));
        }
    }
    Ok(DecodedImage {
        width: info.width,
        height: info.height,
        data: rgba,
    })
}
