//! VTE escape sequence handler

use super::color::{ansi_bright_color, ansi_color};
use crate::event::{
    ClipboardRequest, DynamicColorOperation, DynamicColorTarget, NotificationRequest, TerminalEvent,
};
use crate::grid::{CellAttrs, Color, ColorSpec};
use crate::term::{
    MAX_NOTIFICATION_BODY_BYTES, MAX_NOTIFICATION_TITLE_BYTES, MAX_TITLE_BYTES,
    ReportedWorkingDirectory, Terminal, TerminalMode, XtermKeyResource,
};
use base64::Engine as _;
use vte::{Params, Perform};

const MAX_OSC_DIRECTORY_BYTES: usize = 4_096;
pub const MAX_OSC52_DECODED_BYTES: usize = 1024 * 1024;
const MAX_OSC52_ENCODED_BYTES: usize = MAX_OSC52_DECODED_BYTES.div_ceil(3) * 4;
pub(super) const MAX_OSC_SEQUENCE_BYTES: usize = 16 * 1024 * 1024 + 64;
const MAX_XTGETTCAP_REQUEST_BYTES: usize = 4_096;
const MAX_XTGETTCAP_NAMES: usize = 64;
const MAX_XTGETTCAP_NAME_BYTES: usize = 64;
const MAX_XTGETTCAP_RESPONSE_BYTES: usize = 8_192;
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?1;2;4;8c";
const SECONDARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[>0;1;0c";
const XTVERSION: &[u8] = b"\x1bP>|Termeric(1)\x1b\\";

fn parse_osc_title(params: &[&[u8]]) -> Option<String> {
    let mut bytes = Vec::new();
    for (index, param) in params.iter().enumerate() {
        let separator = usize::from(index > 0);
        if bytes
            .len()
            .saturating_add(separator)
            .saturating_add(param.len())
            > MAX_TITLE_BYTES
        {
            return None;
        }
        if index > 0 {
            bytes.push(b';');
        }
        bytes.extend_from_slice(param);
    }
    let title = std::str::from_utf8(&bytes).ok()?;
    Some(
        title
            .chars()
            .filter(|character| !character.is_control())
            .collect(),
    )
}

fn parse_notification_text(params: &[&[u8]], limit: usize) -> Option<String> {
    let mut bytes = Vec::new();
    for (index, param) in params.iter().enumerate() {
        let separator = usize::from(index > 0);
        if bytes
            .len()
            .saturating_add(separator)
            .saturating_add(param.len())
            > limit
        {
            return None;
        }
        if index > 0 {
            bytes.push(b';');
        }
        bytes.extend_from_slice(param);
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    (!text
        .chars()
        .any(|character| matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)))
    .then(|| text.to_string())
}

fn parse_osc_directory(params: &[&[u8]]) -> Option<ReportedWorkingDirectory> {
    let mut bytes = Vec::new();
    for (index, param) in params.iter().enumerate() {
        let separator = usize::from(index > 0);
        if bytes
            .len()
            .saturating_add(separator)
            .saturating_add(param.len())
            > MAX_OSC_DIRECTORY_BYTES
        {
            return None;
        }
        if index > 0 {
            bytes.push(b';');
        }
        bytes.extend_from_slice(param);
    }

    let uri = std::str::from_utf8(&bytes).ok()?;
    let prefix = uri.get(..7)?;
    if !prefix.eq_ignore_ascii_case("file://") {
        return None;
    }
    let location = &uri[7..];
    let slash = location.find('/')?;
    let host = &location[..slash];
    if host
        .bytes()
        .any(|byte| byte.is_ascii_control() || matches!(byte, b'@' | b'?' | b'#' | b'%'))
    {
        return None;
    }

    let encoded_path = &location.as_bytes()[slash..];
    let mut decoded = Vec::with_capacity(encoded_path.len());
    let mut index = 0;
    while index < encoded_path.len() {
        if encoded_path[index] == b'%' {
            let high = ascii_hex_value(*encoded_path.get(index + 1)?)?;
            let low = ascii_hex_value(*encoded_path.get(index + 2)?)?;
            decoded.push(((high << 4) | low) as u8);
            index += 3;
        } else {
            decoded.push(encoded_path[index]);
            index += 1;
        }
    }
    if decoded.len() > MAX_OSC_DIRECTORY_BYTES
        || decoded
            .iter()
            .any(|byte| *byte == 0 || byte.is_ascii_control())
    {
        return None;
    }
    let path = String::from_utf8(decoded).ok()?;
    if !path.starts_with('/') {
        return None;
    }

    Some(ReportedWorkingDirectory::new(
        uri.to_string(),
        (!host.is_empty()).then(|| host.to_string()),
        path,
    ))
}

fn ascii_hex_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(byte - b'A' + 10)),
        _ => None,
    }
}

fn parse_clipboard_selection(value: &[u8]) -> Option<ClipboardRequest> {
    if value.is_empty() || value.contains(&b'c') || value.contains(&b's') {
        Some(ClipboardRequest::Clipboard)
    } else if value.contains(&b'p') {
        Some(ClipboardRequest::Primary)
    } else {
        None
    }
}

fn parse_osc52(params: &[&[u8]], reply_uses_8bit_controls: bool) -> Option<TerminalEvent> {
    let selection = parse_clipboard_selection(params.get(1).copied().unwrap_or_default())?;
    let data = *params.get(2)?;
    if data == b"?" {
        return Some(TerminalEvent::ClipboardRequest {
            selection,
            reply_uses_8bit_controls,
        });
    }
    if data.len() > MAX_OSC52_ENCODED_BYTES {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(data))
        .ok()?;
    if decoded.len() > MAX_OSC52_DECODED_BYTES {
        return None;
    }
    let text = String::from_utf8(decoded).ok()?;
    Some(TerminalEvent::ClipboardWrite { selection, text })
}

fn parse_palette_index(value: &[u8]) -> Option<u8> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn dynamic_default_target(command: u32) -> Option<DynamicColorTarget> {
    match command {
        10 => Some(DynamicColorTarget::Foreground),
        11 => Some(DynamicColorTarget::Background),
        12 => Some(DynamicColorTarget::Cursor),
        _ => None,
    }
}

fn parse_xterm_color(value: &[u8]) -> Option<Color> {
    let value = std::str::from_utf8(value).ok()?;
    if let Some(rgb) = value.strip_prefix("rgb:") {
        let mut components = rgb.split('/');
        let red = normalize_hex_component(components.next()?)?;
        let green = normalize_hex_component(components.next()?)?;
        let blue = normalize_hex_component(components.next()?)?;
        if components.next().is_some() {
            return None;
        }
        return Some(Color::rgb(red, green, blue));
    }
    let hex = value.strip_prefix('#')?;
    if !matches!(hex.len(), 3 | 6 | 9 | 12) {
        return None;
    }
    let component_len = hex.len() / 3;
    Some(Color::rgb(
        normalize_hex_component(&hex[..component_len])?,
        normalize_hex_component(&hex[component_len..component_len * 2])?,
        normalize_hex_component(&hex[component_len * 2..])?,
    ))
}

fn normalize_hex_component(component: &str) -> Option<u8> {
    if !(1..=4).contains(&component.len()) {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let maximum = 16_u32.pow(component.len() as u32) - 1;
    Some(((value * 255 + maximum / 2) / maximum) as u8)
}

/// Handler for VTE callbacks that updates terminal state
pub struct Handler<'a> {
    terminal: &'a mut Terminal,
}

impl<'a> Handler<'a> {
    pub fn new(terminal: &'a mut Terminal) -> Self {
        Self { terminal }
    }

    pub(super) fn begin_graphics_apc(&mut self) {
        self.terminal.begin_graphics_apc();
    }

    pub(super) fn push_graphics_apc_byte(&mut self, byte: u8) {
        self.terminal.push_graphics_apc_byte(byte);
    }

    pub(super) fn finish_graphics_apc(&mut self) {
        self.terminal.finish_graphics_apc();
    }

    pub(super) fn begin_graphics_relay_apc(&mut self) {
        self.terminal.begin_graphics_relay_apc();
    }

    pub(super) fn push_graphics_relay_apc_byte(&mut self, byte: u8) {
        self.terminal.push_graphics_relay_apc_byte(byte);
    }

    pub(super) fn finish_graphics_relay_apc(&mut self) {
        self.terminal.finish_graphics_relay_apc();
    }

    pub(super) fn handle_parameterless_xterm_key_control(&mut self, final_byte: u8) {
        match final_byte {
            b'm' => {
                self.terminal.reset_xterm_keyboard_modify();
                for resource in XtermKeyResource::ALL {
                    self.terminal.reset_xterm_key_mode(resource);
                }
                self.terminal.set_xterm_modify_other_keys(0, 0);
            }
            b'f' => {
                self.terminal.reset_xterm_keyboard_format();
                for resource in XtermKeyResource::ALL {
                    self.terminal.reset_xterm_key_format(resource);
                }
                self.terminal.set_xterm_other_key_format(0);
            }
            // Xterm assigns omitted Ps in the disable control to resource 2.
            b'n' => self
                .terminal
                .set_xterm_key_mode(XtermKeyResource::Function, u16::MAX),
            _ => unreachable!("only xterm m/f/n controls are intercepted"),
        }
    }

    pub(super) fn cancel_graphics_apc(&mut self) {
        self.terminal.cancel_graphics_apc();
    }

    pub(super) fn cancel_graphics_relay_apc(&mut self) {
        self.terminal.cancel_graphics_relay_apc();
    }

    pub(super) fn handle_dcs(&mut self, payload: &[u8]) {
        if let Some(state) = payload.strip_prefix(b"1$t") {
            self.terminal.restore_presentation_state(1, state);
            return;
        }
        if let Some(state) = payload.strip_prefix(b"2$t") {
            self.terminal.restore_presentation_state(2, state);
            return;
        }
        if let Some(request) = payload.strip_prefix(b"$q") {
            let status = (request.len() <= 32)
                .then(|| self.terminal.decrqss_status(request))
                .flatten();
            let mut response =
                Vec::with_capacity(status.as_ref().map_or(8, |value| value.len() + 8));
            response.extend_from_slice(b"\x1bP");
            response.extend_from_slice(if status.is_some() { b"1$r" } else { b"0$r" });
            if let Some(status) = status {
                response.extend_from_slice(&status);
            }
            response.extend_from_slice(b"\x1b\\");
            self.terminal.push_event(TerminalEvent::PtyWrite(response));
            return;
        }
        if let Some((clear_existing, definitions)) = decudk_payload(payload) {
            // DECUDK was introduced at VT220 conformance level. Xterm accepts
            // the second lock parameter but reports/behaves as always
            // unlocked, so only the first clear/preserve parameter is stateful.
            if self.terminal.dec_conformance_level() >= 2 {
                if clear_existing {
                    self.terminal.clear_xterm_user_defined_keys();
                }
                for definition in definitions.split(|byte| *byte == b';') {
                    let Some(slash) = definition.iter().position(|byte| *byte == b'/') else {
                        continue;
                    };
                    let Ok(code) = std::str::from_utf8(&definition[..slash])
                        .ok()
                        .and_then(|value| value.parse::<u16>().ok())
                        .ok_or(())
                    else {
                        continue;
                    };
                    let encoded = &definition[slash + 1..];
                    if encoded.len() > crate::term::MAX_USER_DEFINED_KEY_BYTES * 2 {
                        continue;
                    }
                    let Some(value) = decode_hex(encoded) else {
                        continue;
                    };
                    self.terminal.define_xterm_user_defined_key(code, value);
                }
            }
            return;
        }
        if let Some(q) = payload.iter().position(|byte| *byte == b'q')
            && payload[..q]
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b';')
        {
            self.terminal.handle_sixel_command(payload);
            return;
        }
        let Some(requests) = payload.strip_prefix(b"+q") else {
            return;
        };
        let response = xtgettcap_response(requests, self.terminal.term_name().as_bytes());
        self.terminal.push_event(TerminalEvent::PtyWrite(response));
    }
}

/// Recognize the parameter/final portion of DECUDK and return xterm's
/// clear-before-define policy plus its definition string.
fn decudk_payload(payload: &[u8]) -> Option<(bool, &[u8])> {
    let final_index = payload.iter().position(|byte| *byte == b'|')?;
    let parameters = &payload[..final_index];
    if !parameters
        .iter()
        .all(|byte| byte.is_ascii_digit() || *byte == b';')
    {
        return None;
    }
    let first = parameters
        .split(|byte| *byte == b';')
        .next()
        .unwrap_or_default();
    let first = if first.is_empty() {
        0
    } else {
        std::str::from_utf8(first).ok()?.parse::<u16>().ok()?
    };
    Some((first == 0, &payload[final_index + 1..]))
}

fn decode_hex(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)? as u8;
            let low = (pair[1] as char).to_digit(16)? as u8;
            Some((high << 4) | low)
        })
        .collect()
}

fn encode_hex(bytes: &[u8], output: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize]);
        output.push(HEX[(byte & 0x0f) as usize]);
    }
}

/// A terminfo name, xterm's termcap alias where one exists, and the value
/// exactly as the entry declares it: numbers in decimal, strings unexpanded.
type TerminfoCapability = (&'static [u8], Option<&'static [u8]>, &'static [u8]);

/// Capabilities XTGETTCAP answers from Termeric's own version-controlled
/// terminfo entry (`assets/terminfo/xterm-termeric.terminfo`). Keeping this
/// table independent of the host terminfo database makes XTGETTCAP
/// deterministic over SSH and in containers, and a test holds every row to
/// the entry so the two cannot drift apart.
const TERMINFO_CAPABILITIES: &[TerminfoCapability] = &[
    (b"colors", Some(b"Co"), b"256"),
    (b"RGB", None, b"8"),
    (b"indn", None, b"\x1b[%p1%dS"),
    // Extensions programs such as Neovim and tmux look up before using
    // styled underlines, cursor shapes, the clipboard, synchronized output,
    // bracketed paste, and focus reports.
    (b"Smulx", None, b"\x1b[4:%p1%dm"),
    (
        b"Setulc",
        None,
        b"\x1b[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m",
    ),
    (b"Ss", None, b"\x1b[%p1%d q"),
    (b"Se", None, b"\x1b[ q"),
    (b"Ms", None, b"\x1b]52;%p1%s;%p2%s\x1b\\"),
    (b"Sync", None, b"\x1b[?2026%?%p1%{1}%-%tl%eh%;"),
    (b"BE", None, b"\x1b[?2004h"),
    (b"BD", None, b"\x1b[?2004l"),
    (b"PS", None, b"\x1b[200~"),
    (b"PE", None, b"\x1b[201~"),
    (b"fe", None, b"\x1b[?1004h"),
    (b"fd", None, b"\x1b[?1004l"),
    (b"kxIN", None, b"\x1b[I"),
    (b"kxOUT", None, b"\x1b[O"),
    // Unmodified and shifted special keys, limited to what the input
    // encoder can emit.
    (b"kbs", Some(b"kb"), b"\x7f"),
    (b"kcbt", Some(b"kB"), b"\x1b[Z"),
    (b"kcub1", Some(b"kl"), b"\x1bOD"),
    (b"kcud1", Some(b"kd"), b"\x1bOB"),
    (b"kcuf1", Some(b"kr"), b"\x1bOC"),
    (b"kcuu1", Some(b"ku"), b"\x1bOA"),
    (b"kLFT", Some(b"#4"), b"\x1b[1;2D"),
    (b"kRIT", Some(b"%i"), b"\x1b[1;2C"),
    (b"kind", Some(b"kF"), b"\x1b[1;2B"),
    (b"kri", Some(b"kR"), b"\x1b[1;2A"),
    (b"kdch1", Some(b"kD"), b"\x1b[3~"),
    (b"kend", Some(b"@7"), b"\x1bOF"),
    (b"kent", Some(b"@8"), b"\x1bOM"),
    (b"kich1", Some(b"kI"), b"\x1b[2~"),
    (b"khome", Some(b"kh"), b"\x1bOH"),
    (b"knp", Some(b"kN"), b"\x1b[6~"),
    (b"kpp", Some(b"kP"), b"\x1b[5~"),
    (b"kDC", None, b"\x1b[3;2~"),
    (b"kEND", None, b"\x1b[1;2F"),
    (b"kHOM", None, b"\x1b[1;2H"),
    (b"kIC", None, b"\x1b[2;2~"),
    (b"kNXT", None, b"\x1b[6;2~"),
    (b"kPRV", None, b"\x1b[5;2~"),
    (b"kf1", Some(b"k1"), b"\x1bOP"),
    (b"kf2", Some(b"k2"), b"\x1bOQ"),
    (b"kf3", Some(b"k3"), b"\x1bOR"),
    (b"kf4", Some(b"k4"), b"\x1bOS"),
    (b"kf5", Some(b"k5"), b"\x1b[15~"),
    (b"kf6", Some(b"k6"), b"\x1b[17~"),
    (b"kf7", Some(b"k7"), b"\x1b[18~"),
    (b"kf8", Some(b"k8"), b"\x1b[19~"),
    (b"kf9", Some(b"k9"), b"\x1b[20~"),
    (b"kf10", Some(b"k;"), b"\x1b[21~"),
    (b"kf11", Some(b"F1"), b"\x1b[23~"),
    (b"kf12", Some(b"F2"), b"\x1b[24~"),
];

/// Return only capabilities that are part of Termeric's own terminal
/// contract, plus the terminal's name and the operating-system query Fish 4
/// uses.
fn xtgettcap_value<'a>(name: &[u8], term_name: &'a [u8]) -> Option<&'a [u8]> {
    match name {
        b"TN" | b"name" => Some(term_name),
        b"query-os-name" => Some(b"Linux"),
        _ => TERMINFO_CAPABILITIES
            .iter()
            .find(|(terminfo, termcap, _)| *terminfo == name || *termcap == Some(name))
            .map(|(_, _, value)| *value),
    }
}

fn xtgettcap_response(requests: &[u8], term_name: &[u8]) -> Vec<u8> {
    let mut response = b"\x1bP1+r".to_vec();
    let mut found = 0_usize;

    if requests.len() <= MAX_XTGETTCAP_REQUEST_BYTES {
        for request in requests
            .split(|byte| *byte == b';')
            .take(MAX_XTGETTCAP_NAMES)
        {
            if request.is_empty()
                || request.len() > MAX_XTGETTCAP_NAME_BYTES * 2
                || !request.iter().all(u8::is_ascii_hexdigit)
            {
                break;
            }
            let Some(name) = decode_hex(request) else {
                break;
            };
            let Some(value) = xtgettcap_value(&name, term_name) else {
                break;
            };

            let field_bytes = usize::from(found > 0)
                .saturating_add(request.len())
                .saturating_add(1)
                .saturating_add(value.len().saturating_mul(2));
            if response.len().saturating_add(field_bytes).saturating_add(2)
                > MAX_XTGETTCAP_RESPONSE_BYTES
            {
                break;
            }
            if found > 0 {
                response.push(b';');
            }
            response.extend_from_slice(request);
            response.push(b'=');
            encode_hex(value, &mut response);
            found += 1;
        }
    }

    if found == 0 {
        response.truncate(2);
        response.extend_from_slice(b"0+r");
    }
    response.extend_from_slice(b"\x1b\\");
    response
}

impl Perform for Handler<'_> {
    /// Print a character to the terminal
    fn print(&mut self, c: char) {
        self.terminal.write_translated_character(c);
    }

    /// Execute a C0/C1 control character
    fn execute(&mut self, byte: u8) {
        match byte {
            0x00 => {} // NUL - ignore
            0x07 => {
                // BEL - bell
                log::debug!("Bell");
                self.terminal.push_event(TerminalEvent::Bell);
            }
            0x08 => {
                // BS - backspace
                self.terminal.cursor_backward(1);
            }
            0x09 => {
                // HT - horizontal tab
                self.terminal.horizontal_tab();
            }
            0x0A..=0x0C => {
                // LF, VT, FF - line feed
                self.terminal.linefeed();
            }
            0x0D => {
                // CR - carriage return
                self.terminal.carriage_return();
            }
            0x0E => {
                // SO - shift out (select G1 charset)
                self.terminal.invoke_gl(1);
            }
            0x0F => {
                // SI - shift in (select G0 charset)
                self.terminal.invoke_gl(0);
            }
            _ => {
                log::trace!("Unknown C0 control: 0x{:02X}", byte);
            }
        }
    }

    /// Handle a CSI sequence
    fn csi_dispatch(
        &mut self,
        raw_params: &Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        // VTE bounds the complete parameter/subparameter payload to 32 u16s.
        // Keep the flattened compatibility view on the stack: CSI is common
        // enough that allocating a Vec for every sequence dominates otherwise
        // inexpensive terminal output.
        let mut flattened_params = [0_u16; 32];
        let mut flattened_len = 0;
        for value in raw_params.iter().flatten().copied() {
            let Some(slot) = flattened_params.get_mut(flattened_len) else {
                return;
            };
            *slot = value;
            flattened_len += 1;
        }
        let params = &flattened_params[..flattened_len];
        let scalar_params = raw_params.iter().all(|param| param.len() <= 1);

        let private = intermediates.first() == Some(&b'?');

        macro_rules! param {
            ($idx:expr, $default:expr) => {
                params
                    .get($idx)
                    .copied()
                    .filter(|&v| v != 0)
                    .unwrap_or($default)
            };
        }

        match action {
            // XTFMTKEYS for xterm's keyboard (0), cursor (1), function (2), keypad (3),
            // other-key (4), reserved string-key (5), modifier-key (6), and special-key (7) format
            // resources. Resources 1-7 use bounded 0/1 formats; xterm stores
            // resource 0 opaquely and does not consult it while encoding.
            'f' if !ignore && intermediates == b">" && scalar_params => match params {
                [0] => self.terminal.reset_xterm_keyboard_format(),
                [0, format] => self.terminal.set_xterm_keyboard_format(*format),
                [4] => self.terminal.set_xterm_other_key_format(0),
                [4, format] => self.terminal.set_xterm_other_key_format(*format),
                [resource] => {
                    if let Some(resource) = XtermKeyResource::from_number(*resource) {
                        self.terminal.reset_xterm_key_format(resource);
                    }
                }
                [resource, format] => {
                    if let Some(resource) = XtermKeyResource::from_number(*resource) {
                        self.terminal.set_xterm_key_format(resource, *format);
                    }
                }
                _ => {}
            },

            // Cursor movement
            'A' => {
                // CUU - Cursor Up
                let n = param!(0, 1) as usize;
                self.terminal.cursor_up(n);
            }
            'B' => {
                // CUD - Cursor Down
                let n = param!(0, 1) as usize;
                self.terminal.cursor_down(n);
            }
            'C' => {
                // CUF - Cursor Forward
                let n = param!(0, 1) as usize;
                self.terminal.cursor_forward(n);
            }
            'D' => {
                // CUB - Cursor Back
                let n = param!(0, 1) as usize;
                self.terminal.cursor_backward(n);
            }
            'E' => {
                // CNL - Cursor Next Line
                let n = param!(0, 1) as usize;
                self.terminal.cursor_down(n);
                self.terminal.carriage_return();
            }
            'F' => {
                // CPL - Cursor Previous Line
                let n = param!(0, 1) as usize;
                self.terminal.cursor_up(n);
                self.terminal.carriage_return();
            }
            'G' | '`' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // CHA/HPA - Cursor Horizontal Absolute.
                self.terminal.set_cursor_column(param!(0, 1) as usize);
            }
            'H' | 'f' if intermediates.is_empty() && scalar_params && params.len() <= 2 => {
                // CUP/HVP - Cursor Position
                let row = param!(0, 1) as usize;
                let col = param!(1, 1) as usize;
                self.terminal.set_cursor_position(row, col);
            }
            'd' => {
                // VPA - Vertical Position Absolute.
                self.terminal.set_cursor_row(param!(0, 1) as usize);
            }
            'a' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // HPR - Horizontal Position Relative.
                self.terminal
                    .horizontal_position_relative(param!(0, 1) as usize);
            }
            'e' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // VPR - Vertical Position Relative.
                self.terminal
                    .vertical_position_relative(param!(0, 1) as usize);
            }
            'I' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // CHT - Cursor Forward Tabulation.
                self.terminal.cursor_forward_tabs(param!(0, 1) as usize);
            }
            'Z' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // CBT - Cursor Backward Tabulation.
                self.terminal.cursor_backward_tabs(param!(0, 1) as usize);
            }

            // Erasing
            'J' if intermediates == b"?" && scalar_params && params.len() <= 1 => {
                // DECSED - Selective Erase in Display.
                self.terminal.selective_erase_in_display(param!(0, 0));
            }
            'J' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // ED - Erase in Display
                let mode = param!(0, 0);
                self.terminal.erase_in_display(mode);
            }
            'K' if intermediates == b"?" && scalar_params && params.len() <= 1 => {
                // DECSEL - Selective Erase in Line.
                self.terminal.selective_erase_in_line(param!(0, 0));
            }
            'K' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // EL - Erase in Line
                let mode = param!(0, 0);
                self.terminal.erase_in_line(mode);
            }
            'X' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // ECH - Erase Character.
                self.terminal.erase_chars(param!(0, 1) as usize);
            }
            'v' if !ignore && intermediates == b"$" && scalar_params && params.len() <= 8 => {
                // DECCRA - Copy Rectangular Area.
                self.terminal.copy_rectangular_area(
                    param!(0, 1) as usize,
                    param!(1, 1) as usize,
                    param!(2, 0) as usize,
                    param!(3, 0) as usize,
                    param!(4, 1) as usize,
                    param!(5, 1) as usize,
                    param!(6, 1) as usize,
                    param!(7, 1) as usize,
                );
            }
            'x' if !ignore && intermediates == b"$" && scalar_params && params.len() <= 5 => {
                // DECFRA - Fill Rectangular Area.
                self.terminal.fill_rectangular_area(
                    params.first().copied().unwrap_or(0),
                    param!(1, 1) as usize,
                    param!(2, 1) as usize,
                    param!(3, 0) as usize,
                    param!(4, 0) as usize,
                );
            }
            'z' if !ignore && intermediates == b"$" && scalar_params && params.len() <= 4 => {
                // DECERA - Erase Rectangular Area.
                self.terminal.erase_rectangular_area(
                    param!(0, 1) as usize,
                    param!(1, 1) as usize,
                    param!(2, 0) as usize,
                    param!(3, 0) as usize,
                );
            }
            '{' if !ignore && intermediates == b"$" && scalar_params && params.len() <= 4 => {
                // DECSERA - Selective Erase Rectangular Area.
                self.terminal.selective_erase_rectangular_area(
                    param!(0, 1) as usize,
                    param!(1, 1) as usize,
                    param!(2, 0) as usize,
                    param!(3, 0) as usize,
                );
            }
            'x' if !ignore && intermediates == b"*" && scalar_params && params.len() <= 1 => {
                // DECSACE - Select Attribute Change Extent.
                self.terminal
                    .select_attribute_change_extent(params.first().copied().unwrap_or(0));
            }
            'r' if !ignore && intermediates == b"$" && scalar_params => {
                // DECCARA - Change Attributes in Rectangular Area.
                self.terminal.change_attributes_area(
                    param!(0, 1) as usize,
                    param!(1, 1) as usize,
                    param!(2, 0) as usize,
                    param!(3, 0) as usize,
                    params.get(4..).unwrap_or_default(),
                );
            }
            't' if !ignore && intermediates == b"$" && scalar_params => {
                // DECRARA - Reverse Attributes in Rectangular Area.
                self.terminal.reverse_attributes_area(
                    param!(0, 1) as usize,
                    param!(1, 1) as usize,
                    param!(2, 0) as usize,
                    param!(3, 0) as usize,
                    params.get(4..).unwrap_or_default(),
                );
            }
            't' if !ignore
                && intermediates.is_empty()
                && scalar_params
                && !params.is_empty()
                && params.len() <= 3 =>
            {
                match params {
                    [operation @ (14 | 16 | 18)] => {
                        self.terminal.report_text_area_geometry(*operation);
                    }
                    [operation @ (20 | 21)] => {
                        self.terminal.report_application_title(*operation);
                    }
                    [operation @ (22 | 23)] => {
                        self.terminal.update_title_stack(*operation == 22, 0, 0);
                    }
                    [operation @ (22 | 23), target] => {
                        self.terminal
                            .update_title_stack(*operation == 22, *target, 0);
                    }
                    [operation @ (22 | 23), target, slot] => {
                        self.terminal
                            .update_title_stack(*operation == 22, *target, *slot);
                    }
                    _ => {}
                }
            }
            'y' if !ignore && intermediates == b"*" && scalar_params && params.len() <= 6 => {
                // DECRQCRA - Request Checksum of Rectangular Area.
                self.terminal.request_checksum_rectangular_area(
                    params.first().copied().unwrap_or(0),
                    params.get(1).copied().unwrap_or(0) as usize,
                    param!(2, 1) as usize,
                    param!(3, 1) as usize,
                    param!(4, 0) as usize,
                    param!(5, 0) as usize,
                );
            }
            'w' if !ignore && intermediates == b"$" && scalar_params && params.len() <= 1 => {
                // DECRQPSR - Request Presentation State Report.
                self.terminal
                    .request_presentation_state(params.first().copied().unwrap_or(0));
            }

            // Xterm title-stack position report. Keep this ahead of SU.
            'S' if !ignore
                && intermediates == b"#"
                && scalar_params
                && matches!(params, [] | [0]) =>
            {
                self.terminal.report_title_stack_position();
            }

            // Scrolling
            'S' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // SU - Scroll Up
                let n = param!(0, 1) as usize;
                self.terminal.scroll_up(n);
            }
            'T' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                // SD - Scroll Down
                let n = param!(0, 1) as usize;
                self.terminal.scroll_down(n);
            }

            // REP - Repeat the preceding graphic character.
            'b' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                self.terminal.repeat_last_graphic(param!(0, 1) as usize);
            }

            // XTQFMTKEYS for xterm's keyboard/cursor/function/keypad/other/
            // modifier/special-key resources. Keep this ahead of TBC because the private
            // introducer selects a keyboard query rather than tab clearing.
            'g' if !ignore
                && intermediates == b"?"
                && scalar_params
                && matches!(params, [0] | [1] | [2] | [3] | [4] | [5] | [6] | [7]) =>
            {
                match params[0] {
                    0 => self.terminal.report_xterm_keyboard_format(),
                    4 => self.terminal.report_xterm_other_key_format(),
                    number => {
                        if let Some(resource) = XtermKeyResource::from_number(number) {
                            self.terminal.report_xterm_key_format(resource);
                        }
                    }
                }
            }

            // TBC - Clear the current or every horizontal tab stop.
            'g' if intermediates.is_empty() && scalar_params && params.len() <= 1 => {
                match params.first().copied().unwrap_or(0) {
                    0 => self.terminal.clear_tab_stop(),
                    3 => self.terminal.clear_tab_stops(),
                    _ => {}
                }
            }

            // DECST8C - Restore default stops at columns 9, 17, ...
            'W' if intermediates == b"?"
                && scalar_params
                && params.len() == 1
                && params[0] == 5 =>
            {
                self.terminal.reset_tab_stops();
            }

            // Modes
            'h' => {
                // SM - Set Mode
                self.handle_mode(params, private, true);
            }
            'l' => {
                // RM - Reset Mode
                self.handle_mode(params, private, false);
            }

            // XTMODKEYS/XTQMODKEYS for xterm's keyboard (0), cursor (1), function (2),
            // keypad (3), other-key (4), reserved string-key (5), modifier-key (6), and special-key
            // (7) resources.
            // Keep this ahead of SGR: the private introducers are protocol
            // selectors, not rendition parameters.
            'm' if !ignore && intermediates == b">" => {
                let parameters: Vec<_> = raw_params.iter().collect();
                match parameters.as_slice() {
                    [[0]] => self.terminal.reset_xterm_keyboard_modify(),
                    [[0], [value]] => self.terminal.set_xterm_keyboard_modify(*value),
                    // Omitted Pv restores xterm's default (disabled).
                    [[4]] => self.terminal.set_xterm_modify_other_keys(0, 0),
                    [[4], [level]] => {
                        self.terminal.set_xterm_modify_other_keys(*level, 0);
                    }
                    // Xterm's documented `CSI > 4 : mask m` form updates
                    // only the modifier mask of the current level.
                    [[4, mask]] => {
                        let level = self.terminal.xterm_modify_other_keys().level();
                        self.terminal
                            .set_xterm_modify_other_keys(u16::from(level), *mask);
                    }
                    // Xterm documents the mask as a subparameter. Accept the
                    // value-attached form used by its current parser too, but
                    // reject all wider shapes.
                    [[4], [level, mask]] | [[4, mask], [level]] => {
                        self.terminal.set_xterm_modify_other_keys(*level, *mask);
                    }
                    [[resource]] => {
                        if let Some(resource) = XtermKeyResource::from_number(*resource) {
                            self.terminal.reset_xterm_key_mode(resource);
                        }
                    }
                    [[resource], [mode]] => {
                        if let Some(resource) = XtermKeyResource::from_number(*resource) {
                            self.terminal.set_xterm_key_mode(resource, *mode);
                        }
                    }
                    _ => {}
                }
            }
            'm' if !ignore
                && intermediates == b"?"
                && scalar_params
                && matches!(params, [0] | [1] | [2] | [3] | [4] | [5] | [6] | [7]) =>
            {
                match params[0] {
                    0 => self.terminal.report_xterm_keyboard_modify(),
                    4 => self.terminal.report_xterm_modify_other_keys(),
                    number => {
                        if let Some(resource) = XtermKeyResource::from_number(number) {
                            self.terminal.report_xterm_key_mode(resource);
                        }
                    }
                }
            }
            // SGR - Select Graphic Rendition
            'm' => {
                // SGR has no private/intermediate prefix. Xterm keyboard
                // protocols also use final `m` (for example `CSI > 4 m` to
                // reset modifyOtherKeys); treating those parameters as SGR
                // would incorrectly enable underline on application exit.
                if intermediates.is_empty() {
                    self.handle_sgr_raw(raw_params);
                } else {
                    log::trace!(
                        "Unhandled private CSI m: params={:?}, intermediates={:?}",
                        params,
                        intermediates
                    );
                }
            }

            // Xterm DEC private-mode save. Keyboard types 1060/1061 share one
            // saved value because they are mutually exclusive views of the
            // same global terminal state.
            's' if intermediates == b"?" && scalar_params => {
                if params.iter().any(|mode| matches!(*mode, 1060 | 1061)) {
                    self.terminal.save_xterm_keyboard_type();
                }
            }
            // Cursor save/restore and Kitty progressive keyboard negotiation.
            's' if intermediates.is_empty() && scalar_params => {
                if self
                    .terminal
                    .mode()
                    .contains(TerminalMode::LEFT_RIGHT_MARGIN)
                {
                    if params.len() <= 2 {
                        let left = param!(0, 1) as usize;
                        let right = param!(1, self.terminal.cols() as u16) as usize;
                        self.terminal.set_left_right_margins(left, right);
                    }
                } else if params.len() <= 1 && params.first().copied().unwrap_or(0) == 0 {
                    // SCOSC - Save Cursor Position. CSI s is only this alias
                    // while DECLRMM is disabled.
                    self.terminal.save_cursor();
                }
            }
            'u' => {
                if intermediates == b"?"
                    && scalar_params
                    && params.len() <= 1
                    && params.first().copied().unwrap_or(0) == 0
                {
                    let flags = self.terminal.kitty_keyboard_flags().bits();
                    self.terminal.push_event(TerminalEvent::PtyWrite(
                        format!("\x1b[?{flags}u").into_bytes(),
                    ));
                } else if intermediates == b"=" && scalar_params && params.len() <= 2 {
                    let flags = params.first().copied().unwrap_or(0);
                    let mode = params
                        .get(1)
                        .copied()
                        .filter(|value| *value != 0)
                        .unwrap_or(1);
                    self.terminal.set_kitty_keyboard_flags(flags, mode);
                } else if intermediates == b">" && scalar_params && params.len() <= 1 {
                    self.terminal
                        .push_kitty_keyboard_flags(params.first().copied().unwrap_or(0));
                } else if intermediates == b"<" && scalar_params && params.len() <= 1 {
                    self.terminal.pop_kitty_keyboard_flags(
                        params
                            .first()
                            .copied()
                            .filter(|value| *value != 0)
                            .unwrap_or(1) as usize,
                    );
                } else if intermediates.is_empty() {
                    // SCORC - Restore Cursor Position
                    self.terminal.restore_cursor();
                }
            }

            // XTVERSION. The static value is a terminal protocol revision, not
            // the package semantic version or an xterm identity claim.
            'q' if !ignore && intermediates == b"> " => {
                // Kitty multiple cursors. Preserve colon-delimited parameter
                // groups; flattening would lose coordinate/color structure.
                let parameters = raw_params
                    .iter()
                    .map(|parameter| parameter.to_vec())
                    .collect::<Vec<_>>();
                self.terminal.handle_multiple_cursor_csi(&parameters);
            }
            'q' if intermediates == b">"
                && params.len() <= 1
                && params.first().copied().unwrap_or(0) == 0 =>
            {
                self.terminal
                    .push_event(TerminalEvent::PtyWrite(XTVERSION.to_vec()));
            }
            // DECSCUSR - application-selected cursor shape and blink policy.
            'q' if intermediates == b" " && scalar_params && params.len() <= 1 => {
                self.terminal
                    .set_cursor_style(params.first().copied().unwrap_or(0));
            }
            // DECSCA - Select Character Protection Attribute.
            'q' if intermediates == b"\"" && scalar_params && params.len() <= 1 => {
                self.terminal
                    .set_character_protection(params.first().copied().unwrap_or(0));
            }

            // DECRQM - Request ANSI or DEC private mode. Only singular,
            // scalar requests are valid; unknown modes receive status 0.
            'p' if scalar_params
                && params.len() <= 1
                && matches!(intermediates, [b'$'] | [b'?', b'$']) =>
            {
                let private = intermediates == b"?$";
                let number = params.first().copied().unwrap_or(0);
                let status = self.terminal.mode_status(private, number);
                let prefix = if private { "?" } else { "" };
                self.terminal.push_event(TerminalEvent::PtyWrite(
                    format!("\x1b[{prefix}{number};{status}$y").into_bytes(),
                ));
            }
            // DECSTR - Soft Terminal Reset. The sequence is parameterless;
            // reject plural, nonzero, and subparameter forms rather than
            // interpreting a malformed mode query as a reset.
            'p' if intermediates == b"!"
                && scalar_params
                && params.len() <= 1
                && params.first().copied().unwrap_or(0) == 0 =>
            {
                self.terminal.soft_reset();
            }
            // DECSCL - Select Conformance Level. The first parameter selects
            // DEC levels 1-5 as 61-65. The optional second parameter selects
            // 8-bit (0/2/default) or 7-bit (1) reply controls.
            'p' if intermediates == b"\"" && scalar_params && (1..=2).contains(&params.len()) => {
                self.terminal
                    .select_conformance_level(params[0], params.get(1).copied());
            }

            // Xterm's disable form for keyboard, cursor, function, keypad, other-key,
            // reserved string-key, modifier-key, and special-key resources.
            'n' if !ignore
                && intermediates == b">"
                && scalar_params
                && matches!(params, [0] | [1] | [2] | [3] | [4] | [5] | [6] | [7]) =>
            {
                match params[0] {
                    0 => self.terminal.set_xterm_keyboard_modify(u16::MAX),
                    4 => {
                        // Xterm stores -1, but modifyOtherKeys levels 0 and -1
                        // are behaviorally equivalent and resource 4's bounded
                        // public state uses 0 through 3.
                        self.terminal.set_xterm_modify_other_keys(0, 0);
                    }
                    number => {
                        if let Some(resource) = XtermKeyResource::from_number(number) {
                            self.terminal.set_xterm_key_mode(resource, u16::MAX);
                        }
                    }
                }
            }

            // Device status report
            'n' => {
                if private {
                    log::trace!("Unhandled private device status report: {:?}", params);
                } else {
                    match param!(0, 0) {
                        5 => {
                            // DSR - Device Status Report: respond with "OK"
                            log::trace!("Device status report requested");
                            self.terminal
                                .push_event(crate::event::TerminalEvent::PtyWrite(
                                    b"\x1b[0n".to_vec(),
                                ));
                        }
                        6 => {
                            // CPR - Cursor Position Report
                            let cursor = self.terminal.grid().cursor();
                            let row = cursor.row + 1; // 1-indexed
                            let col = cursor.col + 1; // 1-indexed
                            let response = format!("\x1b[{};{}R", row, col);
                            log::trace!("Cursor position report: row={}, col={}", row, col);
                            self.terminal
                                .push_event(crate::event::TerminalEvent::PtyWrite(
                                    response.into_bytes(),
                                ));
                        }
                        _ => {}
                    }
                }
            }

            // Primary/Secondary device attributes
            'c' => {
                let request = params.len() <= 1 && params.first().copied().unwrap_or(0) == 0;
                let response = match intermediates {
                    [] if request => Some(PRIMARY_DEVICE_ATTRIBUTES),
                    [b'>'] if request => Some(SECONDARY_DEVICE_ATTRIBUTES),
                    _ => None,
                };
                if let Some(response) = response {
                    self.terminal
                        .push_event(crate::event::TerminalEvent::PtyWrite(response.to_vec()));
                }
            }

            // Insert/Delete
            '@' => {
                // ICH - Insert Characters
                let n = param!(0, 1) as usize;
                self.terminal.insert_chars(n);
            }
            'P' => {
                // DCH - Delete Characters
                let n = param!(0, 1) as usize;
                self.terminal.delete_chars(n);
            }
            'L' => {
                // IL - Insert Lines
                let n = param!(0, 1) as usize;
                self.terminal.insert_lines(n);
            }
            'M' => {
                // DL - Delete Lines
                let n = param!(0, 1) as usize;
                self.terminal.delete_lines(n);
            }

            // Xterm DEC private-mode restore for the shared keyboard-type slot.
            'r' if intermediates == b"?" && scalar_params => {
                for mode in params {
                    if matches!(*mode, 1060 | 1061) {
                        self.terminal.restore_xterm_keyboard_type();
                    }
                }
            }

            // Scroll region
            'r' if intermediates.is_empty() && scalar_params && params.len() <= 2 => {
                // DECSTBM - Set Top and Bottom Margins
                let top = param!(0, 1) as usize;
                let bottom = param!(1, self.terminal.rows() as u16) as usize;
                self.terminal.set_scroll_region(top, bottom);
            }

            _ => {
                log::trace!(
                    "Unhandled CSI: params={:?}, intermediates={:?}, action='{}'",
                    params,
                    intermediates,
                    action
                );
            }
        }
    }

    /// Handle an escape sequence
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (byte, intermediates) {
            // RIS - Full Reset
            (b'c', []) => {
                self.terminal.reset();
            }

            // DECSC - Save Cursor
            (b'7', []) => {
                self.terminal.save_cursor();
            }

            // DECRC - Restore Cursor
            (b'8', []) => {
                self.terminal.restore_cursor();
            }

            // DECID - obsolete VT100 alias for primary device attributes.
            (b'Z', []) => {
                self.terminal
                    .push_event(crate::event::TerminalEvent::PtyWrite(
                        PRIMARY_DEVICE_ATTRIBUTES.to_vec(),
                    ));
            }

            // S7C1T/S8C1T select only the representation of terminal-generated
            // C1 replies. Input acceptance is independent of this preference.
            (b'F', [b' ']) => self.terminal.set_8bit_controls(false),
            (b'G', [b' ']) => self.terminal.set_8bit_controls(true),

            // IND - Index (move down, possibly scroll)
            (b'D', []) => {
                self.terminal.linefeed();
            }

            // NEL - Next Line
            (b'E', []) => {
                self.terminal.carriage_return();
                self.terminal.linefeed();
            }

            // HTS - Horizontal Tab Set
            (b'H', []) => {
                self.terminal.set_tab_stop();
            }

            // RI - Reverse Index (move up, possibly scroll)
            (b'M', []) => {
                self.terminal.reverse_index();
            }

            // DECKPAM - Application Keypad Mode
            (b'=', []) => {
                self.terminal
                    .set_mode(TerminalMode::KEYPAD_APPLICATION, true);
            }

            // DECKPNM - Normal Keypad Mode
            (b'>', []) => {
                self.terminal
                    .set_mode(TerminalMode::KEYPAD_APPLICATION, false);
            }

            // VT220 single and locking shifts. SS2/SS3 affect exactly the next
            // graphic character; LS2/LS3 persist until another GL invocation.
            (b'N', []) => self.terminal.single_shift(2),
            (b'O', []) => self.terminal.single_shift(3),
            (b'n', []) => self.terminal.invoke_gl(2),
            (b'o', []) => self.terminal.invoke_gl(3),
            (b'~', []) => self.terminal.invoke_gr(1),
            (b'}', []) => self.terminal.invoke_gr(2),
            (b'|', []) => self.terminal.invoke_gr(3),

            // Resident 94-character-set designations. Unsupported optional ROM
            // and later national sets leave the register unchanged.
            (designator @ (b'A' | b'B' | b'0'), [b'(']) => {
                self.terminal.designate_character_set(0, designator);
            }
            (designator @ (b'A' | b'B' | b'0'), [b')']) => {
                self.terminal.designate_character_set(1, designator);
            }
            (designator @ (b'A' | b'B' | b'0'), [b'*']) => {
                self.terminal.designate_character_set(2, designator);
            }
            (designator @ (b'A' | b'B' | b'0'), [b'+']) => {
                self.terminal.designate_character_set(3, designator);
            }

            _ => {
                log::trace!(
                    "Unhandled ESC: intermediates={:?}, byte=0x{:02X} ('{}')",
                    intermediates,
                    byte,
                    byte as char
                );
            }
        }
    }

    /// Handle an OSC (Operating System Command) sequence
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }

        let Some(command) = std::str::from_utf8(params[0])
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            return;
        };

        match command {
            // Set both icon and window labels.
            0 => {
                if let Some(title) = parse_osc_title(&params[1..]) {
                    self.terminal.set_application_title_and_icon(title);
                }
            }

            // Set icon label.
            1 => {
                if let Some(icon) = parse_osc_title(&params[1..]) {
                    self.terminal.set_application_icon_name(icon);
                }
            }

            // Set window title.
            2 => {
                if let Some(title) = parse_osc_title(&params[1..]) {
                    self.terminal.set_application_window_title(title);
                }
            }

            // Set color palette
            4 => {
                for pair in params[1..].as_chunks::<2>().0 {
                    let Some(index) = parse_palette_index(pair[0]) else {
                        continue;
                    };
                    if pair[1] == b"?" {
                        self.terminal.push_event(TerminalEvent::DynamicColor(
                            DynamicColorOperation::Query {
                                target: DynamicColorTarget::Palette(index),
                                reply_uses_8bit_controls: self.terminal.uses_8bit_controls(),
                            },
                        ));
                    } else if let Some(color) = parse_xterm_color(pair[1]) {
                        self.terminal.push_event(TerminalEvent::DynamicColor(
                            DynamicColorOperation::Set {
                                target: DynamicColorTarget::Palette(index),
                                color,
                            },
                        ));
                    }
                }
            }

            // Shell-reported current working directory. Keep the URI and its
            // decoded path terminal-local; the application separately decides
            // whether the authority is safe to use as a local filesystem path.
            7 => {
                if let Some(directory) = parse_osc_directory(&params[1..]) {
                    self.terminal.set_reported_working_directory(directory);
                }
            }

            10..=12 => {
                let target = dynamic_default_target(command).expect("matched dynamic color OSC");
                if let Some(value) = params.get(1) {
                    let operation = if *value == b"?" {
                        Some(DynamicColorOperation::Query {
                            target,
                            reply_uses_8bit_controls: self.terminal.uses_8bit_controls(),
                        })
                    } else {
                        parse_xterm_color(value)
                            .map(|color| DynamicColorOperation::Set { target, color })
                    };
                    if let Some(operation) = operation {
                        self.terminal
                            .push_event(TerminalEvent::DynamicColor(operation));
                    }
                }
            }

            104 => {
                if params.len() == 1 {
                    self.terminal.push_event(TerminalEvent::DynamicColor(
                        DynamicColorOperation::ResetPalette,
                    ));
                } else {
                    for value in &params[1..] {
                        if let Some(index) = parse_palette_index(value) {
                            self.terminal.push_event(TerminalEvent::DynamicColor(
                                DynamicColorOperation::Reset(DynamicColorTarget::Palette(index)),
                            ));
                        }
                    }
                }
            }

            // Kitty/xterm mouse pointer shapes. Keep the payload intact so
            // comma-separated stack/query operations remain atomic.
            22 => {
                if params.len() == 2 {
                    self.terminal.handle_pointer_shape_osc(params[1]);
                }
            }

            110..=112 => {
                if let Some(target) = dynamic_default_target(command - 100) {
                    self.terminal.push_event(TerminalEvent::DynamicColor(
                        DynamicColorOperation::Reset(target),
                    ));
                }
            }

            // Hyperlink (OSC 8)
            // Format: OSC 8 ; params ; uri ST
            // params may contain id=value pairs separated by colons
            // Empty uri ends the hyperlink
            8 => {
                log::trace!("Hyperlink OSC 8: {:?}", params);
                // Strip the command "8" from params before passing
                if params.len() > 1 {
                    self.handle_osc_hyperlink(&params[1..]);
                }
            }

            // Clipboard (OSC 52)
            // Format: OSC 52 ; <selection> ; <data> ST
            // selection: c = clipboard, p = primary, etc.
            // data: base64 encoded content, or ? to query
            52 => {
                if let Some(event) = parse_osc52(params, self.terminal.uses_8bit_controls()) {
                    self.terminal.push_event(event);
                }
            }

            // Kitty text sizing and explicit cell-width protocol.
            66 => self.terminal.handle_osc66_multicell(&params[1..]),

            // Desktop notification (OSC 9) - iTerm2 style
            // Format: OSC 9 ; message ST
            9 => {
                if params.len() > 1
                    && let Some(message) = parse_notification_text(
                        params.get(1..).unwrap_or_default(),
                        MAX_NOTIFICATION_BODY_BYTES,
                    )
                {
                    log::info!("Desktop notification: {message:?}");
                    self.terminal
                        .push_event(TerminalEvent::Notification(NotificationRequest {
                            title: None,
                            body: message.to_string(),
                            ..NotificationRequest::default()
                        }));
                }
            }

            // Bounded, capability-negotiated Kitty desktop notifications.
            99 => self.terminal.handle_osc99_notification(&params[1..]),

            // Shell integration (OSC 133)
            // Used for prompt detection, command tracking
            133 => {
                // params[0] is "133", the rest are the actual parameters
                if params.len() > 1 {
                    self.terminal.handle_osc_133(&params[1..]);
                }
            }

            // Safe original iTerm2 inline-image subset. File downloads,
            // multipart transfers, animation, and non-PNG formats are rejected
            // by the protocol adapter without producing filesystem effects.
            1337 => {
                if params.len() > 1 {
                    self.terminal.handle_iterm2_image(&params[1..]);
                }
            }

            // Kitty terminal file transfer. Core validates and decodes the
            // wire command; the embedding application owns consent and every
            // filesystem effect.
            5113 => {
                match crate::file_transfer::parse_kitty_file_transfer(
                    &params[1..],
                    self.terminal.uses_8bit_controls(),
                ) {
                    Ok(command) => self
                        .terminal
                        .push_event(TerminalEvent::KittyFileTransfer(command)),
                    Err(error) => {
                        if let Some(id) = error.id {
                            let reply = crate::file_transfer::KittyFileTransferStatus {
                                id,
                                file_id: error.file_id.unwrap_or_default(),
                                code: crate::file_transfer::KittyFileTransferStatusCode::Invalid,
                                message: Some(error.message),
                                name: None,
                                size: None,
                                transmission_type:
                                    crate::file_transfer::KittyFileTransmissionType::Simple,
                            }
                            .canonical_reply();
                            self.terminal.push_event(TerminalEvent::PtyWrite(reply));
                        }
                    }
                }
            }

            // Desktop notification (OSC 777) - rxvt-unicode style
            // Format: OSC 777 ; notify ; title ; body ST
            777 => {
                if params.get(1) == Some(&b"notify".as_slice()) {
                    let title = match params.get(2) {
                        Some(title) => {
                            let Some(title) =
                                parse_notification_text(&[title], MAX_NOTIFICATION_TITLE_BYTES)
                            else {
                                return;
                            };
                            Some(title)
                        }
                        None => None,
                    };
                    let body = parse_notification_text(
                        params.get(3..).unwrap_or_default(),
                        MAX_NOTIFICATION_BODY_BYTES,
                    );
                    if let Some(body) = body {
                        log::info!("Desktop notification: {title:?} - {body:?}");
                        self.terminal.push_event(TerminalEvent::Notification(
                            NotificationRequest {
                                title,
                                body,
                                ..NotificationRequest::default()
                            },
                        ));
                    }
                }
            }

            _ => {
                log::trace!("Unhandled OSC {}: {:?}", command, params);
            }
        }
    }
}

impl Handler<'_> {
    /// Handle SGR with raw VTE Params (preserves colon-separated subparameters)
    fn handle_sgr_raw(&mut self, params: &Params) {
        // Check for empty params (reset all)
        if params.iter().next().is_none() {
            self.terminal.reset_attrs();
            return;
        }

        // VTE stores at most 32 parameters and subparameters. Borrow its
        // groups into fixed stack storage so every SGR does not allocate an
        // outer Vec plus one Vec per semicolon group.
        let mut groups = [&[][..]; 32];
        let mut group_count = 0;
        for group in params {
            let Some(slot) = groups.get_mut(group_count) else {
                return;
            };
            *slot = group;
            group_count += 1;
        }
        let groups = &groups[..group_count];
        let mut group_index = 0;

        // Process each semicolon-separated parameter group. Extended colors
        // consume following groups in the common `38;2;r;g;b` / `48;5;n`
        // forms, while colon forms stay within one group.
        while group_index < groups.len() {
            let subparams = groups[group_index];
            // Each subparams is a colon-separated group (e.g., [4, 3] for "4:3")
            let values = subparams;

            if values.is_empty() {
                group_index += 1;
                continue;
            }

            let primary = values[0];

            match primary {
                // Reset
                0 => self.terminal.reset_attrs(),

                // Basic attributes
                1 => self.add_attr(CellAttrs::BOLD),
                2 => self.add_attr(CellAttrs::DIM),
                3 => self.add_attr(CellAttrs::ITALIC),

                // Underline with optional style subparameter
                4 => {
                    if values.len() > 1 {
                        // Has subparameter: 4:0 (none), 4:1 (single), 4:2 (double), 4:3 (curly), etc.
                        self.terminal.set_underline_style(values[1]);
                    } else {
                        // Plain "4" means single underline
                        self.terminal.set_underline_style(1);
                    }
                }

                5 => self.add_attr(CellAttrs::BLINK),
                6 => self.add_attr(CellAttrs::RAPID_BLINK),
                7 => self.add_attr(CellAttrs::INVERSE),
                8 => self.add_attr(CellAttrs::HIDDEN),
                9 => self.add_attr(CellAttrs::STRIKE),

                // Double underline (alternate code)
                21 => self.terminal.set_underline_style(2),

                // Reset attributes
                22 => self.remove_attr(CellAttrs::BOLD | CellAttrs::DIM),
                23 => self.remove_attr(CellAttrs::ITALIC),
                24 => {
                    // Reset all underline styles
                    self.remove_attr(CellAttrs::ALL_UNDERLINES);
                }
                25 => self.remove_attr(CellAttrs::BLINK | CellAttrs::RAPID_BLINK),
                27 => self.remove_attr(CellAttrs::INVERSE),
                28 => self.remove_attr(CellAttrs::HIDDEN),
                29 => self.remove_attr(CellAttrs::STRIKE),

                // Foreground colors (30-37)
                30..=37 => self.terminal.set_fg(
                    ansi_color(primary - 30),
                    ColorSpec::Indexed((primary - 30) as u8),
                ),

                // Extended foreground color (38:2:R:G:B or 38:5:N)
                38 => {
                    let parsed = if values.len() > 1 {
                        self.parse_extended_color_spec_from_subparams(&values[1..])
                            .map(|color| (color, 0))
                    } else {
                        self.parse_semicolon_extended_color(groups, group_index)
                    };
                    if let Some(((color, spec), consumed)) = parsed {
                        self.terminal.set_fg(color, spec);
                        group_index += consumed;
                    }
                }

                // Default foreground
                39 => self
                    .terminal
                    .set_fg(Color::default_fg(), ColorSpec::Default),

                // Background colors (40-47)
                40..=47 => self.terminal.set_bg(
                    ansi_color(primary - 40),
                    ColorSpec::Indexed((primary - 40) as u8),
                ),

                // Extended background color (48:2:R:G:B or 48:5:N)
                48 => {
                    let parsed = if values.len() > 1 {
                        self.parse_extended_color_spec_from_subparams(&values[1..])
                            .map(|color| (color, 0))
                    } else {
                        self.parse_semicolon_extended_color(groups, group_index)
                    };
                    if let Some(((color, spec), consumed)) = parsed {
                        self.terminal.set_bg(color, spec);
                        group_index += consumed;
                    }
                }

                // Default background
                49 => self
                    .terminal
                    .set_bg(Color::default_bg(), ColorSpec::Default),

                // Extended underline color (58:2:R:G:B or 58:5:N)
                58 => {
                    let parsed = if values.len() > 1 {
                        self.parse_extended_color_spec_from_subparams(&values[1..])
                            .map(|color| (color, 0))
                    } else {
                        self.parse_semicolon_extended_color(groups, group_index)
                    };
                    if let Some(((color, spec), consumed)) = parsed {
                        self.terminal.set_underline_color_spec(color, spec);
                        group_index += consumed;
                    }
                }

                // Reset underline color
                59 => self.terminal.reset_underline_color(),

                // Bright foreground colors (90-97)
                90..=97 => self.terminal.set_fg(
                    ansi_bright_color(primary - 90),
                    ColorSpec::Indexed((primary - 90 + 8) as u8),
                ),

                // Bright background colors (100-107)
                100..=107 => self.terminal.set_bg(
                    ansi_bright_color(primary - 100),
                    ColorSpec::Indexed((primary - 100 + 8) as u8),
                ),

                _ => {
                    log::trace!("Unknown SGR parameter: {}", primary);
                }
            }
            group_index += 1;
        }
    }

    /// Parse the groups following a semicolon-form 38/48/58 parameter.
    /// Returns the color and number of additional groups consumed.
    fn parse_semicolon_extended_color(
        &self,
        groups: &[&[u16]],
        start: usize,
    ) -> Option<((Color, ColorSpec), usize)> {
        let mode = *groups.get(start + 1)?.first()?;
        match mode {
            2 => {
                let r = *groups.get(start + 2)?.first()?;
                let g = *groups.get(start + 3)?.first()?;
                let b = *groups.get(start + 4)?.first()?;
                if r > 255 || g > 255 || b > 255 {
                    return None;
                }
                let color = Color::rgb(r as u8, g as u8, b as u8);
                Some(((color, ColorSpec::TrueColor(color)), 4))
            }
            5 => {
                let index = *groups.get(start + 2)?.first()?;
                if index > 255 {
                    return None;
                }
                let index = index as u8;
                Some((
                    (super::color::color_256(index), ColorSpec::Indexed(index)),
                    2,
                ))
            }
            _ => None,
        }
    }

    /// Parse extended color with its spec from colon-separated subparameters
    fn parse_extended_color_spec_from_subparams(
        &self,
        subparams: &[u16],
    ) -> Option<(Color, ColorSpec)> {
        if subparams.is_empty() {
            return None;
        }

        match subparams[0] {
            2 => {
                // RGB color: 2:R:G:B (may have additional empty : separators)
                // Some terminals send 2::R:G:B (with colorspace placeholder)
                let rgb = if subparams.len() >= 5 {
                    &subparams[subparams.len() - 3..]
                } else {
                    subparams.get(1..4)?
                };
                if rgb.iter().any(|component| *component > 255) {
                    return None;
                }
                let color = Color::rgb(rgb[0] as u8, rgb[1] as u8, rgb[2] as u8);
                Some((color, ColorSpec::TrueColor(color)))
            }
            5 => {
                // 256-color palette: 5:N
                if subparams.len() > 1 {
                    if subparams[1] > 255 {
                        return None;
                    }
                    let idx = subparams[1] as u8;
                    Some((super::color::color_256(idx), ColorSpec::Indexed(idx)))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Handle mode setting/resetting
    fn handle_mode(&mut self, params: &[u16], private: bool, enable: bool) {
        for &mode in params {
            if private {
                // DEC private modes
                match mode {
                    1 => {
                        // DECCKM - Application cursor keys
                        self.terminal
                            .set_mode(TerminalMode::CURSOR_KEYS_APPLICATION, enable);
                    }
                    6 => {
                        // DECOM - Origin mode
                        self.terminal.set_mode(TerminalMode::ORIGIN, enable);
                        self.terminal.set_cursor_position(1, 1);
                    }
                    7 => {
                        // DECAWM - Autowrap
                        self.terminal.set_mode(TerminalMode::AUTOWRAP, enable);
                    }
                    69 => {
                        // DECLRMM - Enable or disable DECSLRM interpretation.
                        self.terminal.set_left_right_margin_mode(enable);
                    }
                    12 => {
                        // ATT610 - Cursor blinking
                        self.terminal.set_cursor_blinking(enable);
                    }
                    25 => {
                        // DECTCEM - Cursor visible
                        self.terminal.set_cursor_visible(enable);
                    }
                    9 => {
                        // X10 mouse tracking (press only)
                        self.terminal.set_mode(TerminalMode::MOUSE_X10, enable);
                    }
                    1000 => {
                        // VT200 normal mouse tracking (press and release)
                        self.terminal.set_mode(TerminalMode::MOUSE_TRACKING, enable);
                    }
                    1002 => {
                        // Button event mouse tracking
                        self.terminal
                            .set_mode(TerminalMode::MOUSE_BUTTON_EVENT, enable);
                    }
                    1003 => {
                        // Any event mouse tracking
                        self.terminal
                            .set_mode(TerminalMode::MOUSE_ANY_EVENT, enable);
                    }
                    1004 => {
                        // Focus events
                        self.terminal.set_mode(TerminalMode::FOCUS_EVENTS, enable);
                    }
                    1005 => {
                        // UTF-8 mouse encoding
                        self.terminal.set_mode(TerminalMode::MOUSE_UTF8, enable);
                    }
                    1006 => {
                        // SGR mouse encoding
                        self.terminal.set_mode(TerminalMode::MOUSE_SGR, enable);
                    }
                    1007 => {
                        // Alternate scroll mode
                        self.terminal
                            .set_mode(TerminalMode::ALTERNATE_SCROLL, enable);
                    }
                    1060 => {
                        // Xterm oldXtermFKeys/legacy keyboard type.
                        self.terminal.set_xterm_keyboard_type(
                            crate::term::XtermKeyboardType::Legacy,
                            enable,
                        );
                    }
                    1061 => {
                        // Xterm Sun/PC-to-VT220 keyboard type.
                        self.terminal
                            .set_xterm_keyboard_type(crate::term::XtermKeyboardType::Vt220, enable);
                    }
                    47 | 1047 => {
                        // Alternate screen buffer without cursor save/restore.
                        if enable {
                            self.terminal.enter_alt_screen();
                        } else {
                            self.terminal.exit_alt_screen();
                        }
                    }
                    1048 => {
                        // Save/restore cursor and rendition state.
                        if enable {
                            self.terminal.save_cursor();
                        } else {
                            self.terminal.restore_cursor();
                        }
                    }
                    1049 => {
                        // Save cursor/rendition, use a cleared alternate screen,
                        // then restore the primary state when leaving.
                        if enable {
                            self.terminal.save_cursor();
                            self.terminal.enter_alt_screen();
                        } else {
                            self.terminal.exit_alt_screen();
                            self.terminal.restore_cursor();
                        }
                    }
                    2004 => {
                        // Bracketed paste mode
                        self.terminal
                            .set_mode(TerminalMode::BRACKETED_PASTE, enable);
                    }
                    2026 => {
                        // Synchronized output mode
                        // When enabled, terminal should buffer output until disabled
                        self.terminal
                            .set_mode(TerminalMode::SYNCHRONIZED_OUTPUT, enable);
                        log::trace!("Synchronized output mode: {}", enable);
                    }
                    _ => {
                        log::trace!("Unknown private mode {}: {}", mode, enable);
                    }
                }
            } else {
                // ANSI modes
                match mode {
                    4 => {
                        // IRM - Insert mode
                        self.terminal.set_mode(TerminalMode::INSERT, enable);
                    }
                    20 => {
                        // LNM - Linefeed/Newline mode
                        self.terminal
                            .set_mode(TerminalMode::LINEFEED_NEWLINE, enable);
                    }
                    _ => {
                        log::trace!("Unknown ANSI mode {}: {}", mode, enable);
                    }
                }
            }
        }
    }

    fn add_attr(&mut self, attr: CellAttrs) {
        self.terminal.add_attrs(attr);
    }

    fn remove_attr(&mut self, attr: CellAttrs) {
        self.terminal.remove_attrs(attr);
    }

    /// Handle OSC 8 hyperlink
    ///
    /// Format: OSC 8 ; params ; uri ST
    /// - params may contain id=value pairs separated by colons
    /// - Empty uri ends the hyperlink
    fn handle_osc_hyperlink(&mut self, params: &[&[u8]]) {
        // OSC 8 should have at least 2 parameters after the command:
        // params[0] is "8" (command), params[1] is params, params[2] is URI
        // But the params slice we receive has the command already stripped
        // So params[0] is params (may be empty), params[1] is URI

        if params.len() < 2 {
            log::trace!("OSC 8: Invalid format, need at least 2 params");
            return;
        }

        let params_str = std::str::from_utf8(params[0]).unwrap_or("");
        let uri_str = std::str::from_utf8(params[1]).unwrap_or("");

        // If URI is empty, end the hyperlink
        if uri_str.is_empty() {
            log::trace!("OSC 8: End hyperlink");
            self.terminal.reset_hyperlink();
            return;
        }

        // Parse the id parameter from params string
        // Format: key=value:key=value:...
        let mut link_id: Option<String> = None;
        if !params_str.is_empty() {
            for param in params_str.split(':') {
                if let Some(value) = param.strip_prefix("id=") {
                    link_id = Some(value.to_string());
                    break;
                }
            }
        }

        log::trace!("OSC 8: Set hyperlink url={} id={:?}", uri_str, link_id);
        self.terminal.set_hyperlink(uri_str.to_string(), link_id);
    }
}

#[cfg(test)]
mod osc_tests {
    use super::*;

    fn events(bytes: &[u8]) -> Vec<TerminalEvent> {
        let mut terminal = Terminal::new(20, 4);
        terminal.process(bytes);
        terminal.drain_events()
    }

    #[test]
    fn device_attributes_are_byte_exact_and_reject_non_requests() {
        assert_eq!(
            events(b"\x1b[c\x1b[0c\x1bZ\x1b[>c\x1b[>0c"),
            vec![
                TerminalEvent::PtyWrite(PRIMARY_DEVICE_ATTRIBUTES.to_vec()),
                TerminalEvent::PtyWrite(PRIMARY_DEVICE_ATTRIBUTES.to_vec()),
                TerminalEvent::PtyWrite(PRIMARY_DEVICE_ATTRIBUTES.to_vec()),
                TerminalEvent::PtyWrite(SECONDARY_DEVICE_ATTRIBUTES.to_vec()),
                TerminalEvent::PtyWrite(SECONDARY_DEVICE_ATTRIBUTES.to_vec()),
            ]
        );
        assert!(events(b"\x1b[1c\x1b[>1c\x1b[?0c").is_empty());
    }

    #[test]
    fn title_accepts_bel_and_st_preserves_semicolons_and_rejects_bad_input() {
        assert_eq!(
            events(b"\x1b]0;build;tests\x07"),
            vec![TerminalEvent::TitleChanged("build;tests".into())]
        );
        assert_eq!(
            events(b"\x1b]2;editor\x1b\\"),
            vec![TerminalEvent::TitleChanged("editor".into())]
        );
        assert_eq!(
            events(b"\x1b]2;\x07"),
            vec![TerminalEvent::TitleChanged(String::new())]
        );
        assert_eq!(events(b"\x1b]2;bad\xff\x07"), Vec::new());
        assert_eq!(events(b"\x1b]not-a-command;title\x07"), Vec::new());

        let mut oversized = b"\x1b]2;".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', MAX_TITLE_BYTES + 1));
        oversized.push(7);
        let oversized_events = events(&oversized);
        assert!(
            oversized_events.is_empty(),
            "oversized title emitted {oversized_events:?}"
        );

        let first = vec![b'a'; MAX_TITLE_BYTES / 2];
        let second = vec![b'b'; MAX_TITLE_BYTES - first.len()];
        assert!(parse_osc_title(&[&first, &second]).is_none());
    }

    #[test]
    fn kitty_file_transfer_dispatch_is_typed_bounded_and_mode_captured() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process(
            b"\x1b G\x1b]5113;ac=file;id=request;fid=f1;ft=regular;sz=5;n=L3RtcC9maWxl;d=aGVsbG8=\x1b\\",
        );
        let events = terminal.drain_events();
        let TerminalEvent::KittyFileTransfer(command) = &events[0] else {
            panic!("expected typed file-transfer event: {events:?}");
        };
        assert_eq!(command.action, crate::KittyFileTransferAction::File);
        assert_eq!(command.id, "request");
        assert_eq!(command.file_id, "f1");
        assert_eq!(command.name.as_deref(), Some("/tmp/file"));
        assert_eq!(&*command.data, b"hello");
        assert!(command.reply_uses_8bit_controls);

        terminal.process(b"\x1b]5113;ac=send;id=one;id=two\x1b\\");
        let events = terminal.drain_events();
        assert!(matches!(
            events.as_slice(),
            [TerminalEvent::PtyWrite(reply)]
                if reply.starts_with(b"\x9d5113;ac=status;id=one;")
                    && reply.ends_with(b"\x9c")
        ));
    }

    #[test]
    fn osc_zero_and_one_retain_separate_bounded_application_labels() {
        let mut terminal = Terminal::new(20, 4);
        terminal.process(b"\x1b]0;shared;label\x1b\\");
        assert_eq!(terminal.application_icon_name(), "shared;label");
        assert_eq!(terminal.application_window_title(), "shared;label");
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::TitleChanged("shared;label".into())]
        );

        terminal.process(b"\x1b]1;icon;only\x07");
        assert_eq!(terminal.application_icon_name(), "icon;only");
        assert_eq!(terminal.application_window_title(), "shared;label");
        assert!(terminal.drain_events().is_empty());

        terminal.process(b"\x1b]2;window;only\x07");
        assert_eq!(terminal.application_icon_name(), "icon;only");
        assert_eq!(terminal.application_window_title(), "window;only");
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::TitleChanged("window;only".into())]
        );
    }

    #[test]
    fn legacy_notifications_are_bounded_safe_and_preserve_body_semicolons() {
        assert_eq!(
            events(b"\x1b]9;build;complete\x1b\\"),
            vec![TerminalEvent::Notification(NotificationRequest {
                title: None,
                body: "build;complete".into(),
                ..NotificationRequest::default()
            })]
        );
        assert_eq!(
            events(b"\x1b]777;notify;build;line;two\x07"),
            vec![TerminalEvent::Notification(NotificationRequest {
                title: Some("build".into()),
                body: "line;two".into(),
                ..NotificationRequest::default()
            })]
        );
        assert!(parse_notification_text(&[b"line\nfeed"], 64).is_none());
        assert!(events(b"\x1b]9\x1b\\").is_empty());

        let mut oversized = b"\x1b]9;".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', MAX_NOTIFICATION_BODY_BYTES + 1));
        oversized.extend_from_slice(b"\x1b\\");
        assert!(events(&oversized).is_empty());
    }

    #[test]
    fn osc7_retains_only_bounded_valid_file_directory_reports() {
        let mut terminal = Terminal::new(20, 4);
        terminal.process(b"\x1b]7;file://localhost/work/a%20b;semi/%E7%95%8C\x07");
        let directory = terminal.reported_working_directory().unwrap();
        assert_eq!(
            directory.uri(),
            "file://localhost/work/a%20b;semi/%E7%95%8C"
        );
        assert_eq!(directory.host(), Some("localhost"));
        assert_eq!(directory.path(), "/work/a b;semi/界");

        for invalid in [
            b"\x1b]7;https://localhost/replaced\x07".as_slice(),
            b"\x1b]7;file://user@localhost/replaced\x07",
            b"\x1b]7;file://localhost/invalid%2\x07",
            b"\x1b]7;file://localhost/control%0Apath\x07",
            b"\x1b]7;file://localhost/%FF\x07",
            b"\x1b]7;relative/path\x07",
        ] {
            terminal.process(invalid);
            assert_eq!(
                terminal.reported_working_directory().unwrap().path(),
                "/work/a b;semi/界"
            );
        }

        let mut oversized = b"\x1b]7;file:///".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', MAX_OSC_DIRECTORY_BYTES));
        oversized.push(7);
        terminal.process(&oversized);
        assert_eq!(
            terminal.reported_working_directory().unwrap().path(),
            "/work/a b;semi/界"
        );

        terminal.process(b"\x1bc");
        assert_eq!(
            terminal.reported_working_directory().unwrap().path(),
            "/work/a b;semi/界",
            "RIS must not discard shell/application directory metadata"
        );

        let mut local = Terminal::new(20, 4);
        local.process(b"\x1b]7;file:///tmp/raw path#query?literal\x1b\\");
        let directory = local.reported_working_directory().unwrap();
        assert_eq!(directory.host(), None);
        assert_eq!(directory.path(), "/tmp/raw path#query?literal");
    }

    #[test]
    fn osc52_parses_queries_writes_empty_fields_and_rejects_malformed_payloads() {
        assert_eq!(
            events(b"\x1b]52;;?\x07"),
            vec![TerminalEvent::ClipboardRequest {
                selection: ClipboardRequest::Clipboard,
                reply_uses_8bit_controls: false,
            }]
        );
        assert_eq!(
            events(b"\x1b]52;p;SGVsbG8=\x1b\\"),
            vec![TerminalEvent::ClipboardWrite {
                selection: ClipboardRequest::Primary,
                text: "Hello".into(),
            }]
        );
        assert_eq!(
            events(b"\x1b]52;c;SGVsbG8\x07"),
            vec![TerminalEvent::ClipboardWrite {
                selection: ClipboardRequest::Clipboard,
                text: "Hello".into(),
            }]
        );
        assert!(events(b"\x1b]52;c;%%%\x07").is_empty());
        assert!(events(b"\x1b]52;c;/w==\x07").is_empty());
        assert!(events(b"\x1b]52;x;SGVsbG8=\x07").is_empty());

        let mut oversized = b"\x1b]52;c;".to_vec();
        oversized.extend(std::iter::repeat_n(b'A', MAX_OSC52_ENCODED_BYTES + 1));
        oversized.push(7);
        assert!(events(&oversized).is_empty());
    }

    #[test]
    fn dynamic_colors_parse_set_query_and_reset_families() {
        assert_eq!(
            events(
                concat!(
                    "\x1b]4;1;rgb:f/0/8;2;?\x07",
                    "\x1b]10;#123456\x1b\\",
                    "\x1b]11;?\x07",
                    "\x1b]104;1;2\x07",
                    "\x1b]112\x1b\\"
                )
                .as_bytes()
            ),
            vec![
                TerminalEvent::DynamicColor(DynamicColorOperation::Set {
                    target: DynamicColorTarget::Palette(1),
                    color: Color::rgb(255, 0, 136),
                }),
                TerminalEvent::DynamicColor(DynamicColorOperation::Query {
                    target: DynamicColorTarget::Palette(2),
                    reply_uses_8bit_controls: false,
                }),
                TerminalEvent::DynamicColor(DynamicColorOperation::Set {
                    target: DynamicColorTarget::Foreground,
                    color: Color::rgb(18, 52, 86),
                }),
                TerminalEvent::DynamicColor(DynamicColorOperation::Query {
                    target: DynamicColorTarget::Background,
                    reply_uses_8bit_controls: false,
                }),
                TerminalEvent::DynamicColor(DynamicColorOperation::Reset(
                    DynamicColorTarget::Palette(1),
                )),
                TerminalEvent::DynamicColor(DynamicColorOperation::Reset(
                    DynamicColorTarget::Palette(2),
                )),
                TerminalEvent::DynamicColor(DynamicColorOperation::Reset(
                    DynamicColorTarget::Cursor,
                )),
            ]
        );
        assert_eq!(
            events(b"\x1b]104\x07"),
            vec![TerminalEvent::DynamicColor(
                DynamicColorOperation::ResetPalette
            )]
        );
    }

    #[test]
    fn xterm_color_components_normalize_one_to_four_hex_digits() {
        assert_eq!(parse_xterm_color(b"#f08"), Some(Color::rgb(255, 0, 136)));
        assert_eq!(
            parse_xterm_color(b"rgb:ffff/8000/0000"),
            Some(Color::rgb(255, 128, 0))
        );
        assert_eq!(parse_xterm_color(b"rgb:12/34"), None);
        assert_eq!(parse_xterm_color(b"not-a-color"), None);
    }
}

#[cfg(test)]
mod terminfo_tests {
    use super::*;
    use std::collections::HashMap;

    const TERMINFO_SOURCE: &str = include_str!("../../../assets/terminfo/xterm-termeric.terminfo");

    /// Undo terminfo source escaping, so a string compares byte for byte
    /// with what XTGETTCAP sends.
    fn decode_terminfo_string(text: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut characters = text.bytes();
        while let Some(byte) = characters.next() {
            match byte {
                b'\\' => match characters.next().expect("escape is complete") {
                    b'E' | b'e' => bytes.push(0x1b),
                    b'n' => bytes.push(b'\n'),
                    b'r' => bytes.push(b'\r'),
                    b't' => bytes.push(b'\t'),
                    b'b' => bytes.push(0x08),
                    b'f' => bytes.push(0x0c),
                    b's' => bytes.push(b' '),
                    digit @ b'0'..=b'7' => {
                        let mut value = u32::from(digit - b'0');
                        for _ in 0..2 {
                            let digit = characters.next().expect("octal escapes have 3 digits");
                            value = value * 8 + u32::from(digit - b'0');
                        }
                        bytes.push(value as u8);
                    }
                    literal => bytes.push(literal),
                },
                b'^' => {
                    let control = characters.next().expect("control escape is complete");
                    bytes.push(if control == b'?' {
                        0x7f
                    } else {
                        control & 0x1f
                    });
                }
                byte => bytes.push(byte),
            }
        }
        bytes
    }

    /// Every numeric and string capability in the source entry, by name.
    fn terminfo_source_values() -> HashMap<String, Vec<u8>> {
        let mut lines = TERMINFO_SOURCE
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'));
        let names = lines.next().expect("the entry starts with its names");
        assert!(names.starts_with("xterm-termeric|"), "{names}");
        let mut values = HashMap::new();
        for line in lines {
            let field = line.strip_suffix(',').expect("fields end with a comma");
            let (name, value) = if let Some((name, value)) = field.split_once('=') {
                (name, decode_terminfo_string(value))
            } else if let Some((name, value)) = field.split_once('#') {
                (name, value.as_bytes().to_vec())
            } else {
                continue;
            };
            assert!(
                values.insert(name.to_string(), value).is_none(),
                "{name} is declared twice"
            );
        }
        values
    }

    #[test]
    fn every_xtgettcap_answer_matches_the_terminfo_entry() {
        let declared = terminfo_source_values();
        assert_eq!(
            declared.get("kbs").map(Vec::as_slice),
            Some(b"\x7f".as_slice())
        );
        for (name, termcap, value) in TERMINFO_CAPABILITIES {
            let name = std::str::from_utf8(name).unwrap();
            assert_eq!(
                declared.get(name).map(Vec::as_slice),
                Some(*value),
                "XTGETTCAP {name} differs from the terminfo entry"
            );
            assert_eq!(
                xtgettcap_value(name.as_bytes(), b"xterm-termeric"),
                Some(*value)
            );
            if let Some(termcap) = termcap {
                assert_eq!(xtgettcap_value(termcap, b"xterm-termeric"), Some(*value));
            }
        }
    }

    #[test]
    fn terminfo_extensions_drive_the_features_they_declare() {
        // The bytes `tput` expands each extension to, as
        // scripts/check_terminfo.py checks them.
        let mut terminal = Terminal::new(20, 2);
        terminal.process(b"\x1b[4:3m\x1b[58:2::18:52:86mA\x1b[4:0mB");
        let styled = terminal.grid().cell(0, 0).unwrap();
        assert_eq!(styled.attrs, CellAttrs::CURLY_UNDERLINE);
        assert_eq!(styled.underline_color, Some(Color::rgb(18, 52, 86)));
        assert!(!terminal.grid().cell(0, 1).unwrap().attrs.has_underline());

        terminal.process(b"\x1b[5 q");
        assert!(terminal.cursor_style().is_some());
        terminal.process(b"\x1b[ q");
        assert_eq!(terminal.cursor_style(), None);

        terminal.process(b"\x1b[?2026h");
        assert!(terminal.is_synchronized_output());
        terminal.process(b"\x1b[?2026l");
        assert!(!terminal.is_synchronized_output());

        terminal.process(b"\x1b[?2004h\x1b[?1004h");
        assert!(terminal.mode().contains(TerminalMode::BRACKETED_PASTE));
        assert!(terminal.mode().contains(TerminalMode::FOCUS_EVENTS));
        terminal.process(b"\x1b[?2004l\x1b[?1004l");
        assert!(!terminal.mode().contains(TerminalMode::BRACKETED_PASTE));
        assert!(!terminal.mode().contains(TerminalMode::FOCUS_EVENTS));

        terminal.drain_events();
        terminal.process(b"\x1b]52;c;aGk=\x1b\\");
        assert_eq!(
            terminal.drain_events(),
            vec![TerminalEvent::ClipboardWrite {
                selection: ClipboardRequest::Clipboard,
                text: "hi".into(),
            }]
        );
    }
}
