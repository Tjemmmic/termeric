//! Kitty Graphics Protocol parser
//!
//! Parses APC (Application Program Command) sequences for the Kitty graphics protocol.
//! Format: APC G <key>=<value>,<key>=<value>;[base64 data] ST

use std::collections::HashMap;

/// Kitty graphics command
#[derive(Debug, Clone)]
pub struct KittyCommand {
    /// Action to perform
    pub action: KittyAction,
    /// Image ID (i=)
    pub image_id: Option<u32>,
    /// Placement ID (p=)
    pub placement_id: Option<u32>,
    /// Image transmission info
    pub transmission: Option<KittyTransmission>,
    /// Placement parameters
    pub placement: Option<KittyPlacement>,
    /// Base64 decoded data
    pub data: Option<Vec<u8>>,
    /// Whether more data chunks follow (m=1)
    pub more_chunks: bool,
}

/// Action type for the command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyAction {
    /// Transmit image data (a=t)
    Transmit,
    /// Display a transmitted image (a=p)
    Display,
    /// Transmit and display (a=T)
    TransmitAndDisplay,
    /// Delete an image (a=d)
    Delete,
    /// Query terminal capabilities (a=q)
    Query,
    /// Animation frame composition (a=f, a=c, a=a)
    FrameComposition,
}

/// Image transmission parameters
#[derive(Debug, Clone)]
pub struct KittyTransmission {
    /// Data format (f=)
    pub format: KittyFormat,
    /// Image width (s=)
    pub width: Option<u32>,
    /// Image height (v=)
    pub height: Option<u32>,
    /// Transmission medium (t=)
    pub medium: KittyMedium,
    /// Compression (o=)
    pub compression: KittyCompression,
}

/// Image data format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyFormat {
    /// 32-bit RGBA (f=32)
    #[default]
    Rgba,
    /// 24-bit RGB (f=24)
    Rgb,
    /// PNG encoded (f=100)
    Png,
}

/// Transmission medium
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyMedium {
    /// Direct data in payload (t=d)
    #[default]
    Direct,
    /// File path (t=f)
    File,
    /// Temporary file (t=t)
    TempFile,
    /// Shared memory (t=s)
    SharedMemory,
}

/// Compression method
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyCompression {
    /// No compression (o=)
    #[default]
    None,
    /// Zlib compression (o=z)
    Zlib,
}

/// Image placement parameters
#[derive(Debug, Clone, Default)]
pub struct KittyPlacement {
    /// X position in pixels within cell (X=)
    pub x_offset: Option<u32>,
    /// Y position in pixels within cell (Y=)
    pub y_offset: Option<u32>,
    /// Display width in columns (c=)
    pub width: Option<u32>,
    /// Display height in rows (r=)
    pub height: Option<u32>,
    /// Column position (x=)
    pub col: Option<u32>,
    /// Row position (y=)
    pub row: Option<u32>,
    /// Z-index for layering (z=)
    pub z_index: Option<i32>,
}

impl KittyCommand {
    /// Parse a Kitty graphics APC sequence
    ///
    /// Expected format after "G": key=value,key=value;[base64data]
    pub fn parse(payload: &[u8]) -> Option<Self> {
        // Find the control portion and optional data portion
        let (control, data_part) = if let Some(pos) = payload.iter().position(|&b| b == b';') {
            (&payload[..pos], Some(&payload[pos + 1..]))
        } else {
            (payload, None)
        };

        // Parse key=value pairs
        let control_str = std::str::from_utf8(control).ok()?;
        let params = parse_params(control_str);

        // Determine action
        let action = match params.get("a").map(|s| s.as_str()) {
            Some("t") => KittyAction::Transmit,
            Some("T") => KittyAction::TransmitAndDisplay,
            Some("p") => KittyAction::Display,
            Some("d") => KittyAction::Delete,
            Some("q") => KittyAction::Query,
            Some("f") | Some("c") | Some("a") => KittyAction::FrameComposition,
            _ => KittyAction::TransmitAndDisplay, // Default
        };

        // Parse IDs
        let image_id = params.get("i").and_then(|s| s.parse().ok());
        let placement_id = params.get("p").and_then(|s| s.parse().ok());

        // Parse transmission parameters
        let transmission = if action == KittyAction::Transmit || action == KittyAction::TransmitAndDisplay {
            Some(KittyTransmission {
                format: match params.get("f").map(|s| s.as_str()) {
                    Some("24") => KittyFormat::Rgb,
                    Some("100") => KittyFormat::Png,
                    _ => KittyFormat::Rgba,
                },
                width: params.get("s").and_then(|s| s.parse().ok()),
                height: params.get("v").and_then(|s| s.parse().ok()),
                medium: match params.get("t").map(|s| s.as_str()) {
                    Some("f") => KittyMedium::File,
                    Some("t") => KittyMedium::TempFile,
                    Some("s") => KittyMedium::SharedMemory,
                    _ => KittyMedium::Direct,
                },
                compression: match params.get("o").map(|s| s.as_str()) {
                    Some("z") => KittyCompression::Zlib,
                    _ => KittyCompression::None,
                },
            })
        } else {
            None
        };

        // Parse placement parameters
        let placement = Some(KittyPlacement {
            x_offset: params.get("X").and_then(|s| s.parse().ok()),
            y_offset: params.get("Y").and_then(|s| s.parse().ok()),
            width: params.get("c").and_then(|s| s.parse().ok()),
            height: params.get("r").and_then(|s| s.parse().ok()),
            col: params.get("x").and_then(|s| s.parse().ok()),
            row: params.get("y").and_then(|s| s.parse().ok()),
            z_index: params.get("z").and_then(|s| s.parse().ok()),
        });

        // Check if more chunks follow
        let more_chunks = params.get("m").map(|s| s == "1").unwrap_or(false);

        // Decode base64 data if present
        let data = if let Some(data_part) = data_part {
            decode_base64(data_part)
        } else {
            None
        };

        Some(Self {
            action,
            image_id,
            placement_id,
            transmission,
            placement,
            data,
            more_chunks,
        })
    }
}

/// Parse comma-separated key=value pairs
fn parse_params(s: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in s.split(',') {
        if let Some((key, value)) = pair.split_once('=') {
            params.insert(key.to_string(), value.to_string());
        }
    }
    params
}

/// Decode base64 data
fn decode_base64(data: &[u8]) -> Option<Vec<u8>> {
    // Simple base64 decoder
    const DECODE_TABLE: [i8; 256] = {
        let mut table = [-1i8; 256];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < 64 {
            table[chars[i] as usize] = i as i8;
            i += 1;
        }
        table
    };

    let data: Vec<u8> = data.iter()
        .copied()
        .filter(|&b| b != b'\n' && b != b'\r' && b != b' ')
        .collect();

    if data.is_empty() {
        return Some(Vec::new());
    }

    let mut result = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;

    for &byte in &data {
        if byte == b'=' {
            continue;
        }

        let value = DECODE_TABLE[byte as usize];
        if value < 0 {
            return None; // Invalid character
        }

        buffer = (buffer << 6) | (value as u32);
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let payload = b"a=T,f=100,i=1";
        let cmd = KittyCommand::parse(payload).unwrap();

        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.image_id, Some(1));
        assert!(matches!(
            cmd.transmission.as_ref().unwrap().format,
            KittyFormat::Png
        ));
    }

    #[test]
    fn test_parse_with_data() {
        let payload = b"a=T,f=32,s=2,v=2,i=1;AQIDBA=="; // base64 for [1,2,3,4]
        let cmd = KittyCommand::parse(payload).unwrap();

        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.image_id, Some(1));
        assert_eq!(cmd.data, Some(vec![1, 2, 3, 4]));
    }

    #[test]
    fn test_parse_placement() {
        let payload = b"a=p,i=1,x=10,y=5,c=20,r=10";
        let cmd = KittyCommand::parse(payload).unwrap();

        assert_eq!(cmd.action, KittyAction::Display);
        let placement = cmd.placement.unwrap();
        assert_eq!(placement.col, Some(10));
        assert_eq!(placement.row, Some(5));
        assert_eq!(placement.width, Some(20));
        assert_eq!(placement.height, Some(10));
    }

    #[test]
    fn test_decode_base64() {
        // "Hello" in base64 is "SGVsbG8="
        let result = decode_base64(b"SGVsbG8=").unwrap();
        assert_eq!(result, b"Hello");

        // Empty string
        let result = decode_base64(b"").unwrap();
        assert!(result.is_empty());

        // With whitespace
        let result = decode_base64(b"SGVs\nbG8=").unwrap();
        assert_eq!(result, b"Hello");
    }
}
