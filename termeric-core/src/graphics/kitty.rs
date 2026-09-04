//! Strict parser for the supported Kitty graphics protocol subset.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use std::collections::HashMap;
use std::fmt;

/// Bound the control section independently from image data.
pub const MAX_CONTROL_BYTES: usize = 4 * 1024;
/// Kitty's reference client currently emits 128-KiB directly transmitted,
/// base64-encoded chunks even though the published protocol recommends 4 KiB.
pub const MAX_ENCODED_CHUNK_BYTES: usize = 128 * 1024;
/// Maximum bytes following APC `G`, including control data and its separator.
pub const MAX_APC_BYTES: usize = MAX_CONTROL_BYTES + 1 + MAX_ENCODED_CHUNK_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyParseError(pub String);

impl fmt::Display for KittyParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Kitty graphics command.
#[derive(Debug, Clone)]
pub struct KittyCommand {
    pub action: KittyAction,
    pub image_id: Option<u32>,
    /// Client image number (`I=`). Unlike IDs, numbers are not unique and
    /// resolve to the newest retained image carrying that number.
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub transmission: KittyTransmission,
    pub placement: KittyPlacement,
    pub frame: KittyFrame,
    pub data: Vec<u8>,
    pub more_chunks: bool,
    /// q=0 reports all results, q=1 suppresses success, q=2 suppresses all replies.
    pub quiet: u8,
    /// Deletion selector (d=), defaulting to all visible placements/images.
    pub delete: char,
    /// Whether a semicolon and data field were present.
    pub has_data: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyAction {
    Transmit,
    Display,
    TransmitAndDisplay,
    Delete,
    Query,
    TransmitFrame,
    AnimationControl,
    FrameComposition,
}

/// Action-dependent animation controls. Kitty intentionally reuses several
/// control keys between frame loading, playback control, and composition.
#[derive(Debug, Clone, Copy, Default)]
pub struct KittyFrame {
    /// `r=`: edited/affected/source frame number, depending on the action.
    pub frame_number: Option<u32>,
    /// `c=`: base/current/destination frame number, depending on the action.
    pub other_frame_number: Option<u32>,
    pub gap: Option<i32>,
    /// Frame-load destination or composition destination coordinates (`x/y`).
    pub x: u32,
    pub y: u32,
    /// Composition rectangle (`w/h`).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Composition source coordinates (`X/Y`).
    pub source_x: u32,
    pub source_y: u32,
    /// Frame-load background color (`Y=` as `0xRRGGBBAA`).
    pub background_color: Option<u32>,
    /// Replacement instead of source-over alpha blending.
    pub replace: bool,
    /// Playback state (`s=1/2/3`) and loop count (`v=`).
    pub animation_state: Option<u8>,
    pub loop_count: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct KittyTransmission {
    pub format: KittyFormat,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub medium: KittyMedium,
    pub compression: KittyCompression,
    /// Local-media read ceiling, or expected decompressed PNG size (`S=`).
    pub data_size: Option<u64>,
    /// Byte offset into local media (`O=`).
    pub data_offset: Option<u64>,
    /// Client usage hints (`N=`). Hints may be ignored by the storage policy.
    pub usage_hints: u32,
}

impl Default for KittyTransmission {
    fn default() -> Self {
        Self {
            format: KittyFormat::Rgba,
            width: None,
            height: None,
            medium: KittyMedium::Direct,
            compression: KittyCompression::None,
            data_size: None,
            data_offset: None,
            usage_hints: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyFormat {
    #[default]
    Rgba,
    Rgb,
    Png,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyMedium {
    #[default]
    Direct,
    File,
    TempFile,
    SharedMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KittyCompression {
    #[default]
    None,
    Zlib,
}

#[derive(Debug, Clone, Default)]
pub struct KittyPlacement {
    /// Source rectangle in image pixels (x/y/w/h).
    pub source_x: Option<u32>,
    pub source_y: Option<u32>,
    pub source_width: Option<u32>,
    pub source_height: Option<u32>,
    /// Pixel offset in the cursor cell (X/Y).
    pub x_offset: Option<u32>,
    pub y_offset: Option<u32>,
    /// Destination size in cells (c/r).
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub z_index: Option<i32>,
    /// C=1 prevents the normal cursor movement after placement.
    pub do_not_move_cursor: bool,
    /// U=1 creates an invisible prototype for Unicode placeholder cells.
    pub virtual_placement: bool,
    /// Parent image and placement identifiers for relative placement (P/Q).
    pub parent_image_id: Option<u32>,
    pub parent_placement_id: Option<u32>,
    /// Cell displacement from the parent's top-left origin (H/V).
    pub horizontal_offset: i32,
    pub vertical_offset: i32,
    /// Terminal-owned placement origin, never parsed from protocol x/y.
    pub(crate) col: Option<u32>,
    pub(crate) row: Option<u32>,
}

impl KittyCommand {
    /// Return whether a command explicitly requests that all replies be
    /// suppressed. This deliberately needs only the bounded control prefix so
    /// malformed and oversized commands can still honor `q=2`.
    pub fn suppresses_response(payload: &[u8]) -> bool {
        let control_end = payload
            .iter()
            .position(|byte| *byte == b';')
            .unwrap_or(payload.len())
            .min(MAX_CONTROL_BYTES);
        payload[..control_end]
            .split(|byte| *byte == b',')
            .any(|field| field == b"q=2")
    }

    /// Parse the bytes following APC `G`.
    pub fn parse(payload: &[u8]) -> Result<Self, KittyParseError> {
        let (control, encoded, has_data) = match payload.iter().position(|byte| *byte == b';') {
            Some(pos) => (&payload[..pos], &payload[pos + 1..], true),
            None => (payload, &[][..], false),
        };
        if control.len() > MAX_CONTROL_BYTES {
            return Err(KittyParseError("control data exceeds 4096 bytes".into()));
        }
        let control = std::str::from_utf8(control)
            .map_err(|_| KittyParseError("control data is not UTF-8".into()))?;
        let params = parse_params(control)?;
        const SUPPORTED_KEYS: &str = "aiIpNfsvtoSOmqdxywhXYcrzCUPQHV";
        if let Some(key) = params
            .keys()
            .find(|key| key.len() != 1 || !SUPPORTED_KEYS.contains(key.as_str()))
        {
            return Err(KittyParseError(format!("unsupported control {key}")));
        }

        let action = match params.get("a").map(String::as_str) {
            None | Some("t") => KittyAction::Transmit,
            Some("T") => KittyAction::TransmitAndDisplay,
            Some("p") => KittyAction::Display,
            Some("d") => KittyAction::Delete,
            Some("q") => KittyAction::Query,
            Some("f") => KittyAction::TransmitFrame,
            Some("a") => KittyAction::AnimationControl,
            Some("c") => KittyAction::FrameComposition,
            Some(value) => return Err(KittyParseError(format!("unsupported action {value}"))),
        };

        let format = match params.get("f").map(String::as_str) {
            None | Some("32") => KittyFormat::Rgba,
            Some("0") if action == KittyAction::FrameComposition => KittyFormat::Rgba,
            Some("24") => KittyFormat::Rgb,
            Some("100") => KittyFormat::Png,
            Some(value) => return Err(KittyParseError(format!("unsupported format {value}"))),
        };
        let medium = match params.get("t").map(String::as_str) {
            None | Some("d") => KittyMedium::Direct,
            Some("f") => KittyMedium::File,
            Some("t") => KittyMedium::TempFile,
            Some("s") => KittyMedium::SharedMemory,
            Some(value) => return Err(KittyParseError(format!("unsupported medium {value}"))),
        };
        let compression = match params.get("o").map(String::as_str) {
            None => KittyCompression::None,
            Some("z") => KittyCompression::Zlib,
            Some(value) => return Err(KittyParseError(format!("unsupported compression {value}"))),
        };
        let more_chunks = match params.get("m").map(String::as_str) {
            None | Some("0") => false,
            Some("1") => true,
            Some(value) => return Err(KittyParseError(format!("invalid m={value}"))),
        };
        let quiet = number::<u8>(&params, "q")?.unwrap_or(0);
        if quiet > 2 {
            return Err(KittyParseError("q must be 0, 1, or 2".into()));
        }
        let delete = match params.get("d") {
            None => 'a',
            Some(value) if value.chars().count() == 1 => value.chars().next().unwrap(),
            Some(_) => return Err(KittyParseError("d must be one character".into())),
        };
        if params.contains_key("i") && params.contains_key("I") {
            return Err(KittyParseError(
                "i and I cannot be specified together".into(),
            ));
        }

        if encoded.len() > MAX_ENCODED_CHUNK_BYTES {
            return Err(KittyParseError("direct data chunk exceeds 128 KiB".into()));
        }
        if more_chunks && encoded.len() % 4 != 0 {
            return Err(KittyParseError(
                "non-final base64 chunk length must be divisible by four".into(),
            ));
        }
        let data = if has_data {
            STANDARD
                .decode(encoded)
                .or_else(|_| STANDARD_NO_PAD.decode(encoded))
                .map_err(|_| KittyParseError("invalid base64 data".into()))?
        } else {
            Vec::new()
        };

        let cursor_policy = number::<u8>(&params, "C")?.unwrap_or(0);
        if cursor_policy > 1 {
            return Err(KittyParseError("C must be 0 or 1".into()));
        }
        let virtual_placement = number::<u8>(&params, "U")?.unwrap_or(0);
        if virtual_placement > 1 {
            return Err(KittyParseError("U must be 0 or 1".into()));
        }
        let frame_replace = match action {
            KittyAction::TransmitFrame => number::<u8>(&params, "X")?.unwrap_or(0),
            KittyAction::FrameComposition => cursor_policy,
            _ => 0,
        };
        if frame_replace > 1 {
            return Err(KittyParseError(
                "animation composition mode must be 0 or 1".into(),
            ));
        }
        let animation_state = if action == KittyAction::AnimationControl {
            number::<u8>(&params, "s")?.filter(|value| *value != 0)
        } else {
            None
        };
        if animation_state.is_some_and(|value| !(1..=3).contains(&value)) {
            return Err(KittyParseError("animation s must be 1, 2, or 3".into()));
        }

        let frame = KittyFrame {
            frame_number: number::<u32>(&params, "r")?.filter(|value| *value != 0),
            other_frame_number: number::<u32>(&params, "c")?.filter(|value| *value != 0),
            gap: number::<i32>(&params, "z")?.filter(|value| *value != 0),
            x: number::<u32>(&params, "x")?.unwrap_or(0),
            y: number::<u32>(&params, "y")?.unwrap_or(0),
            width: number::<u32>(&params, "w")?.filter(|value| *value != 0),
            height: number::<u32>(&params, "h")?.filter(|value| *value != 0),
            source_x: if action == KittyAction::FrameComposition {
                number::<u32>(&params, "X")?.unwrap_or(0)
            } else {
                0
            },
            source_y: if action == KittyAction::FrameComposition {
                number::<u32>(&params, "Y")?.unwrap_or(0)
            } else {
                0
            },
            background_color: (action == KittyAction::TransmitFrame)
                .then(|| number::<u32>(&params, "Y"))
                .transpose()?
                .flatten(),
            replace: frame_replace == 1,
            animation_state,
            loop_count: if action == KittyAction::AnimationControl {
                number::<u32>(&params, "v")?.filter(|value| *value != 0)
            } else {
                None
            },
        };

        Ok(Self {
            action,
            image_id: number(&params, "i")?,
            image_number: number(&params, "I")?,
            placement_id: number(&params, "p")?,
            transmission: KittyTransmission {
                format,
                width: number(&params, "s")?,
                height: number(&params, "v")?,
                medium,
                compression,
                data_size: number::<u64>(&params, "S")?.filter(|value| *value != 0),
                data_offset: number::<u64>(&params, "O")?.filter(|value| *value != 0),
                usage_hints: number::<u32>(&params, "N")?.unwrap_or(0),
            },
            placement: KittyPlacement {
                source_x: number(&params, "x")?,
                source_y: number(&params, "y")?,
                source_width: number::<u32>(&params, "w")?.filter(|value| *value != 0),
                source_height: number::<u32>(&params, "h")?.filter(|value| *value != 0),
                x_offset: number(&params, "X")?,
                y_offset: number(&params, "Y")?,
                width: number::<u32>(&params, "c")?.filter(|value| *value != 0),
                height: number::<u32>(&params, "r")?.filter(|value| *value != 0),
                z_index: number(&params, "z")?,
                do_not_move_cursor: cursor_policy == 1,
                virtual_placement: virtual_placement == 1,
                parent_image_id: number::<u32>(&params, "P")?.filter(|value| *value != 0),
                parent_placement_id: number::<u32>(&params, "Q")?.filter(|value| *value != 0),
                horizontal_offset: number::<i32>(&params, "H")?.unwrap_or(0),
                vertical_offset: number::<i32>(&params, "V")?.unwrap_or(0),
                col: None,
                row: None,
            },
            frame,
            data,
            more_chunks,
            quiet,
            delete,
            has_data,
        })
    }
}

fn parse_params(value: &str) -> Result<HashMap<String, String>, KittyParseError> {
    let mut params = HashMap::new();
    if value.is_empty() {
        return Ok(params);
    }
    for pair in value.split(',') {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| KittyParseError(format!("invalid control {pair}")))?;
        if key.chars().count() != 1 || value.is_empty() {
            return Err(KittyParseError(format!("invalid control {pair}")));
        }
        if params.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(KittyParseError(format!("duplicate control {key}")));
        }
    }
    Ok(params)
}

fn number<T>(params: &HashMap<String, String>, key: &str) -> Result<Option<T>, KittyParseError>
where
    T: std::str::FromStr,
{
    params
        .get(key)
        .map(|value| {
            value
                .parse()
                .map_err(|_| KittyParseError(format!("invalid {key}={value}")))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_static_png_and_source_rectangle_without_moving_cursor() {
        let cmd =
            KittyCommand::parse(b"a=T,f=100,i=7,p=9,x=2,y=3,w=4,h=5,X=6,Y=7,c=8,r=9,z=-2,C=1")
                .unwrap();
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.transmission.format, KittyFormat::Png);
        assert_eq!(cmd.placement.source_x, Some(2));
        assert_eq!(cmd.placement.source_y, Some(3));
        assert_eq!(cmd.placement.z_index, Some(-2));
        assert!(cmd.placement.do_not_move_cursor);
    }

    #[test]
    fn strictly_decodes_data_and_rejects_invalid_controls() {
        let cmd = KittyCommand::parse(b"a=T,f=32,s=1,v=1,i=1;AQIDBA==").unwrap();
        assert_eq!(cmd.data, vec![1, 2, 3, 4]);
        assert!(KittyCommand::parse(b"a=T,f=999;AQID").is_err());
        assert!(KittyCommand::parse(b"a=T,a=p;AQID").is_err());
        assert!(KittyCommand::parse(b"a=T;%%%=").is_err());
        assert!(KittyCommand::parse(b"m=1;AQI").is_err());
        assert!(KittyCommand::parse(b"a=p,L=1,i=2").is_err());
        assert_eq!(
            KittyCommand::parse(b"f=32,s=1,v=1;AQIDBA==")
                .unwrap()
                .action,
            KittyAction::Transmit
        );
    }

    #[test]
    fn accepts_current_kitty_chunk_size_and_unpadded_local_media_payloads() {
        let mut direct = b"a=T,q=2,f=32,s=128,v=192,m=1;".to_vec();
        direct.extend(std::iter::repeat_n(b'A', MAX_ENCODED_CHUNK_BYTES));
        let command = KittyCommand::parse(&direct).unwrap();
        assert_eq!(command.data.len(), 98_304);
        assert!(command.more_chunks);

        // Captured from kitten 0.48.2 temporary-file transfer. The reference
        // client uses raw standard base64, so the final `=` is omitted.
        let command = KittyCommand::parse(
            b"a=T,q=2,f=100,t=t,U=1,S=1482060;L2Rldi9zaG0va2l0dHktdHR5LWdyYXBoaWNzLXByb3RvY29sLTE4MTc4ODc1MDc",
        )
        .unwrap();
        assert_eq!(
            command.data,
            b"/dev/shm/kitty-tty-graphics-protocol-1817887507"
        );
    }

    #[test]
    fn response_suppression_survives_parse_errors_but_is_control_scoped() {
        assert!(KittyCommand::suppresses_response(b"a=T,q=2;%%%"));
        assert!(!KittyCommand::suppresses_response(b"a=T,q=1;q=2"));
        assert!(!KittyCommand::suppresses_response(b"a=T;payload,q=2"));
    }

    #[test]
    fn parses_local_media_size_and_offset() {
        let cmd = KittyCommand::parse(b"a=q,t=t,S=80,O=10;iW1wL2ltYWdl").unwrap();
        assert_eq!(cmd.transmission.medium, KittyMedium::TempFile);
        assert_eq!(cmd.transmission.data_size, Some(80));
        assert_eq!(cmd.transmission.data_offset, Some(10));

        let defaults = KittyCommand::parse(b"a=q,t=t,S=0,O=0;iW1wL2ltYWdl").unwrap();
        assert_eq!(defaults.transmission.data_size, None);
        assert_eq!(defaults.transmission.data_offset, None);
    }

    #[test]
    fn parses_image_numbers_and_ignorable_usage_hints_exclusively() {
        let cmd = KittyCommand::parse(b"a=T,I=13,N=1,f=32,s=1,v=1;AQIDBA==").unwrap();
        assert_eq!(cmd.image_id, None);
        assert_eq!(cmd.image_number, Some(13));
        assert_eq!(cmd.transmission.usage_hints, 1);

        assert!(KittyCommand::parse(b"a=t,i=7,I=13;AQIDBA==").is_err());
        assert!(KittyCommand::parse(b"a=t,I=not-a-number;AQIDBA==").is_err());
    }

    #[test]
    fn parses_only_the_defined_unicode_placeholder_flag() {
        let cmd = KittyCommand::parse(b"a=p,U=1,i=7,c=2,r=3").unwrap();
        assert!(cmd.placement.virtual_placement);
        assert!(KittyCommand::parse(b"a=p,U=2,i=7,c=2,r=3").is_err());
    }

    #[test]
    fn parses_signed_relative_placement_controls() {
        let cmd = KittyCommand::parse(b"a=p,i=7,p=8,P=9,Q=10,H=-11,V=12").unwrap();
        assert_eq!(cmd.placement.parent_image_id, Some(9));
        assert_eq!(cmd.placement.parent_placement_id, Some(10));
        assert_eq!(cmd.placement.horizontal_offset, -11);
        assert_eq!(cmd.placement.vertical_offset, 12);

        let defaults = KittyCommand::parse(b"a=p,i=7,P=0,Q=0").unwrap();
        assert_eq!(defaults.placement.parent_image_id, None);
        assert_eq!(defaults.placement.parent_placement_id, None);
        assert_eq!(defaults.placement.horizontal_offset, 0);
        assert_eq!(defaults.placement.vertical_offset, 0);
        assert!(KittyCommand::parse(b"a=p,i=7,H=2147483648").is_err());
    }

    #[test]
    fn parses_action_dependent_animation_controls() {
        let load = KittyCommand::parse(
            b"a=f,i=1,f=32,s=2,v=3,x=4,y=5,c=1,r=2,z=-1,X=1,Y=4278190335;AQIDBA==",
        )
        .unwrap();
        assert_eq!(load.action, KittyAction::TransmitFrame);
        assert_eq!(
            (load.transmission.width, load.transmission.height),
            (Some(2), Some(3))
        );
        assert_eq!((load.frame.x, load.frame.y), (4, 5));
        assert_eq!(
            (load.frame.frame_number, load.frame.other_frame_number),
            (Some(2), Some(1))
        );
        assert_eq!(load.frame.gap, Some(-1));
        assert!(load.frame.replace);
        assert_eq!(load.frame.background_color, Some(4_278_190_335));

        let control = KittyCommand::parse(b"a=a,i=1,s=3,v=4,r=2,c=3,z=48").unwrap();
        assert_eq!(control.action, KittyAction::AnimationControl);
        assert_eq!(control.frame.animation_state, Some(3));
        assert_eq!(control.frame.loop_count, Some(4));

        let compose =
            KittyCommand::parse(b"a=c,i=1,r=2,c=3,w=4,h=5,X=6,Y=7,x=8,y=9,C=1,f=0").unwrap();
        assert_eq!(compose.action, KittyAction::FrameComposition);
        assert_eq!((compose.frame.source_x, compose.frame.source_y), (6, 7));
        assert_eq!((compose.frame.x, compose.frame.y), (8, 9));
        assert!(compose.frame.replace);
        assert!(KittyCommand::parse(b"a=a,i=1,s=4").is_err());
        assert!(KittyCommand::parse(b"a=f,i=1,X=2").is_err());
        assert!(KittyCommand::parse(b"a=t,i=1,f=0").is_err());
    }
}
