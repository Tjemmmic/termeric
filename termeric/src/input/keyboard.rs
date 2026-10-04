//! Mode-aware keyboard input to PTY byte conversion.

use termeric_core::term::{
    KittyKeyboardFlags, TerminalMode, XtermKeyFormat, XtermKeyMode, XtermKeyResource,
    XtermKeyResources, XtermKeySetting, XtermKeyboardResource, XtermKeyboardType,
    XtermModifyOtherKeys,
};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, NativeKey, PhysicalKey};

/// One terminal's negotiated xterm keyboard state. The default is xterm's
/// startup profile, which sends the familiar VT/xterm sequences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct XtermKeyProfile {
    keyboard_type: XtermKeyboardType,
    keyboard_resource: XtermKeyboardResource,
    other_keys: XtermModifyOtherKeys,
    resources: XtermKeyResources,
}

impl XtermKeyProfile {
    fn of(terminal: &termeric_core::Terminal) -> Self {
        Self {
            keyboard_type: terminal.xterm_keyboard_type(),
            keyboard_resource: terminal.xterm_keyboard_resource(),
            other_keys: terminal.xterm_modify_other_keys(),
            resources: terminal.xterm_key_resources(),
        }
    }

    fn resource(&self, resource: XtermKeyResource) -> XtermKeySetting {
        self.resources.get(resource)
    }

    fn other_keys_active(&self) -> bool {
        self.other_keys.level() != 0
    }

    /// The modifiers a key reports. The historical keyboard types drop them
    /// unless `modifyOtherKeys` is on or resource 0 enables `category_bit`.
    fn modifiers(&self, category_bit: u16, modifiers: ModifiersState) -> ModifiersState {
        if !self.keyboard_type.uses_resource_zero()
            || self.other_keys_active()
            || category_bit != 0 && self.keyboard_resource.modify() & category_bit != 0
        {
            modifiers
        } else {
            ModifiersState::empty()
        }
    }
}

/// The protocol one key event is encoded with. Kitty's negotiated protocol
/// takes precedence over every xterm resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyboardProtocol {
    Xterm(XtermKeyProfile),
    Kitty(KittyKeyboardFlags),
}

impl KeyboardProtocol {
    fn new(kitty_flags: KittyKeyboardFlags, xterm: XtermKeyProfile) -> Self {
        if kitty_flags.is_empty() {
            Self::Xterm(xterm)
        } else {
            Self::Kitty(kitty_flags)
        }
    }

    /// Terminal modes come with every event because panes can be in
    /// different application modes.
    fn encode(
        self,
        event: &KeyEvent,
        modifiers: ModifiersState,
        alt_graph_active: bool,
        mode: TerminalMode,
    ) -> Option<Vec<u8>> {
        match self {
            Self::Xterm(profile) => (event.state == ElementState::Pressed).then(|| {
                encode_xterm(
                    &event.logical_key,
                    event.physical_key,
                    event.text.as_deref(),
                    modifiers,
                    alt_graph_active,
                    mode,
                    profile,
                )
            })?,
            Self::Kitty(flags) => encode_kitty(event, modifiers, alt_graph_active, mode, flags),
        }
    }
}

/// Encode against one terminal's complete negotiated keyboard state. DECUDK
/// is checked before the ordinary xterm resource encoders, while Kitty's
/// explicitly negotiated protocol retains precedence.
pub fn key_to_bytes_for_terminal(
    event: &KeyEvent,
    modifiers: ModifiersState,
    alt_graph_active: bool,
    terminal: &termeric_core::Terminal,
) -> Option<Vec<u8>> {
    if let Some(definition) = encode_xterm_user_defined_key(&event.logical_key, modifiers, terminal)
    {
        return Some(definition);
    }
    KeyboardProtocol::new(
        terminal.kitty_keyboard_flags(),
        XtermKeyProfile::of(terminal),
    )
    .encode(event, modifiers, alt_graph_active, terminal.mode())
}

fn encode_xterm_user_defined_key(
    logical_key: &Key,
    modifiers: ModifiersState,
    terminal: &termeric_core::Terminal,
) -> Option<Vec<u8>> {
    if !terminal.kitty_keyboard_flags().is_empty() {
        return None;
    }
    let code = xterm_user_defined_key_code(logical_key, modifiers, terminal.xterm_keyboard_type())?;
    terminal.xterm_user_defined_key(code).map(<[u8]>::to_vec)
}

/// Encode a text-only IME commit under Kitty's associated-text extension.
pub fn encode_ime_text(flags: KittyKeyboardFlags, text: &str) -> Vec<u8> {
    if !flags
        .contains(KittyKeyboardFlags::REPORT_ALL_KEYS | KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT)
    {
        return text.as_bytes().to_vec();
    }
    let codepoints = associated_text(text);
    if codepoints.is_empty() {
        Vec::new()
    } else {
        format!("\x1b[0;1;{codepoints}u").into_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittyEventType {
    Press = 1,
    Repeat = 2,
    Release = 3,
}

impl KittyEventType {
    fn from_event(event: &KeyEvent) -> Self {
        if event.state == ElementState::Released {
            Self::Release
        } else if event.repeat {
            Self::Repeat
        } else {
            Self::Press
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittyKey {
    Code(u32),
    Final(u8),
    Tilde(u16),
}

fn encode_kitty(
    event: &KeyEvent,
    modifiers: ModifiersState,
    alt_graph_active: bool,
    mode: TerminalMode,
    flags: KittyKeyboardFlags,
) -> Option<Vec<u8>> {
    let reports_events = flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES);
    if event.state == ElementState::Released && !reports_events {
        return None;
    }
    let event_type = KittyEventType::from_event(event);
    let effective_modifiers = if alt_graph_active && event.text.is_some() {
        modifiers & !(ModifiersState::ALT | ModifiersState::CONTROL)
    } else {
        modifiers
    };

    if let Some(keypad) = KeypadKey::from_physical(event.physical_key) {
        let keypad_is_non_text = mode.contains(TerminalMode::KEYPAD_APPLICATION);
        let encode_as_event = flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
            || (keypad_is_non_text && keypad != KeypadKey::Enter);
        if encode_as_event {
            return Some(encode_kitty_key(
                KittyKey::Code(keypad.kitty_code()),
                None,
                None,
                event.text.as_deref(),
                effective_modifiers,
                event_type,
                flags,
            ));
        }
        return (event.state == ElementState::Pressed)
            .then(|| keypad.encode(mode, event.text.as_deref(), modifiers));
    }

    if let Some(code) = kitty_modifier_code(event.physical_key, &event.logical_key) {
        return flags
            .contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
            .then(|| {
                encode_kitty_key(
                    KittyKey::Code(code),
                    None,
                    None,
                    None,
                    effective_modifiers,
                    event_type,
                    flags,
                )
            });
    }

    match &event.logical_key {
        Key::Character(logical) => {
            let should_escape = flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
                || (flags.contains(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES)
                    && (effective_modifiers.alt_key()
                        || effective_modifiers.control_key()
                        || effective_modifiers.super_key()));
            if !should_escape {
                return (event.state == ElementState::Pressed).then(|| {
                    encode_character(logical, event.text.as_deref(), modifiers, alt_graph_active)
                })?;
            }
            let (code, shifted, base) =
                kitty_character_codes(logical, event.physical_key, effective_modifiers.shift_key());
            Some(encode_kitty_key(
                KittyKey::Code(code),
                shifted,
                base,
                event.text.as_deref(),
                effective_modifiers,
                event_type,
                flags,
            ))
        }
        Key::Named(named) => encode_kitty_named(
            *named,
            event.physical_key,
            event.text.as_deref(),
            effective_modifiers,
            mode,
            event_type,
            flags,
        ),
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

fn encode_kitty_named(
    named: NamedKey,
    physical: PhysicalKey,
    text: Option<&str>,
    modifiers: ModifiersState,
    mode: TerminalMode,
    event_type: KittyEventType,
    flags: KittyKeyboardFlags,
) -> Option<Vec<u8>> {
    let all = flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS);
    let key = match named {
        NamedKey::Enter if all => KittyKey::Code(13),
        NamedKey::Tab if all => KittyKey::Code(9),
        NamedKey::Backspace if all => KittyKey::Code(127),
        NamedKey::Escape
            if all
                || flags.contains(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES)
                || flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES) =>
        {
            KittyKey::Code(27)
        }
        NamedKey::Space
            if all
                || (flags.contains(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES)
                    && (modifiers.alt_key()
                        || modifiers.control_key()
                        || modifiers.super_key())) =>
        {
            KittyKey::Code(32)
        }
        NamedKey::ArrowUp => KittyKey::Final(b'A'),
        NamedKey::ArrowDown => KittyKey::Final(b'B'),
        NamedKey::ArrowRight => KittyKey::Final(b'C'),
        NamedKey::ArrowLeft => KittyKey::Final(b'D'),
        NamedKey::Home => KittyKey::Final(b'H'),
        NamedKey::End => KittyKey::Final(b'F'),
        NamedKey::PageUp => KittyKey::Tilde(5),
        NamedKey::PageDown => KittyKey::Tilde(6),
        NamedKey::Insert => KittyKey::Tilde(2),
        NamedKey::Delete => KittyKey::Tilde(3),
        NamedKey::F1 => KittyKey::Final(b'P'),
        NamedKey::F2 => KittyKey::Final(b'Q'),
        NamedKey::F3 if flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES) => {
            KittyKey::Tilde(13)
        }
        NamedKey::F3 => KittyKey::Final(b'R'),
        NamedKey::F4 => KittyKey::Final(b'S'),
        NamedKey::F5 => KittyKey::Tilde(15),
        NamedKey::F6 => KittyKey::Tilde(17),
        NamedKey::F7 => KittyKey::Tilde(18),
        NamedKey::F8 => KittyKey::Tilde(19),
        NamedKey::F9 => KittyKey::Tilde(20),
        NamedKey::F10 => KittyKey::Tilde(21),
        NamedKey::F11 => KittyKey::Tilde(23),
        NamedKey::F12 => KittyKey::Tilde(24),
        _ if kitty_named_code(named).is_some() => KittyKey::Code(kitty_named_code(named)?),
        _ => {
            return (event_type != KittyEventType::Release)
                .then(|| encode_named(named, modifiers, mode))?;
        }
    };

    // Enter, Tab and Backspace deliberately remain recoverable legacy keys
    // unless REPORT_ALL_KEYS is active, and therefore have no release events.
    if !all && matches!(named, NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace) {
        return (event_type != KittyEventType::Release)
            .then(|| encode_named(named, modifiers, mode))?;
    }
    if matches!(key, KittyKey::Final(_) | KittyKey::Tilde(_))
        && !flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES)
        && extended_modifier_parameter(modifiers) == 1
    {
        return encode_named(named, modifiers, mode);
    }
    Some(encode_kitty_key(
        key,
        None,
        physical_base_code(physical),
        text,
        modifiers,
        event_type,
        flags,
    ))
}

fn encode_kitty_key(
    key: KittyKey,
    shifted: Option<u32>,
    base: Option<u32>,
    text: Option<&str>,
    modifiers: ModifiersState,
    event_type: KittyEventType,
    flags: KittyKeyboardFlags,
) -> Vec<u8> {
    let modifier = extended_modifier_parameter(modifiers);
    let report_event = flags.contains(KittyKeyboardFlags::REPORT_EVENT_TYPES);
    let associated = (flags.contains(KittyKeyboardFlags::REPORT_ALL_KEYS)
        && flags.contains(KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT)
        && event_type != KittyEventType::Release)
        .then(|| associated_text(text.unwrap_or_default()))
        .filter(|value| !value.is_empty());

    match key {
        KittyKey::Code(code) => {
            let key_field = kitty_key_field(code, shifted, base, flags);
            let mut sequence = format!("\x1b[{key_field}");
            if modifier != 1 || report_event || associated.is_some() {
                sequence.push(';');
                sequence.push_str(&modifier.to_string());
                if report_event {
                    sequence.push(':');
                    sequence.push_str(&(event_type as u8).to_string());
                }
            }
            if let Some(text) = associated {
                sequence.push(';');
                sequence.push_str(&text);
            }
            sequence.push('u');
            sequence.into_bytes()
        }
        KittyKey::Final(final_byte) => {
            if modifier == 1 && !report_event {
                return vec![0x1b, b'[', final_byte];
            }
            let event = if report_event {
                format!(":{}", event_type as u8)
            } else {
                String::new()
            };
            format!("\x1b[1;{modifier}{event}{}", final_byte as char).into_bytes()
        }
        KittyKey::Tilde(number) => {
            if modifier == 1 && !report_event {
                return format!("\x1b[{number}~").into_bytes();
            }
            let event = if report_event {
                format!(":{}", event_type as u8)
            } else {
                String::new()
            };
            format!("\x1b[{number};{modifier}{event}~").into_bytes()
        }
    }
}

fn kitty_key_field(
    code: u32,
    shifted: Option<u32>,
    base: Option<u32>,
    flags: KittyKeyboardFlags,
) -> String {
    if !flags.contains(KittyKeyboardFlags::REPORT_ALTERNATE_KEYS) {
        return code.to_string();
    }
    let shifted = shifted.filter(|value| *value != code);
    let base = base.filter(|value| *value != code);
    match (shifted, base) {
        (Some(shifted), Some(base)) => format!("{code}:{shifted}:{base}"),
        (Some(shifted), None) => format!("{code}:{shifted}"),
        (None, Some(base)) => format!("{code}::{base}"),
        (None, None) => code.to_string(),
    }
}

fn associated_text(text: &str) -> String {
    text.chars()
        .filter(|character| {
            let code = *character as u32;
            code >= 0x20 && !(0x7f..=0x9f).contains(&code)
        })
        .map(|character| (character as u32).to_string())
        .collect::<Vec<_>>()
        .join(":")
}

fn kitty_character_codes(
    logical: &str,
    physical: PhysicalKey,
    shifted_active: bool,
) -> (u32, Option<u32>, Option<u32>) {
    let logical_char = logical.chars().next().unwrap_or('\0');
    let unshifted = if logical_char.is_uppercase() || shifted_active {
        unshifted_character(logical_char).unwrap_or(logical_char)
    } else {
        logical_char
    };
    let shifted = shifted_active.then_some(logical_char as u32);
    (unshifted as u32, shifted, physical_base_code(physical))
}

fn unshifted_character(character: char) -> Option<char> {
    if character.is_uppercase() {
        return character.to_lowercase().next();
    }
    Some(match character {
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}

fn physical_base_code(physical: PhysicalKey) -> Option<u32> {
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    let character = match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Backquote => '`',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        KeyCode::Space => ' ',
        _ => return None,
    };
    Some(character as u32)
}

fn kitty_modifier_code(physical: PhysicalKey, logical: &Key) -> Option<u32> {
    if matches!(logical, Key::Named(NamedKey::AltGraph)) {
        return Some(57453);
    }
    if let PhysicalKey::Code(code) = physical {
        return match code {
            KeyCode::ShiftLeft => Some(57441),
            KeyCode::ControlLeft => Some(57442),
            KeyCode::AltLeft => Some(57443),
            KeyCode::SuperLeft => Some(57444),
            KeyCode::ShiftRight => Some(57447),
            KeyCode::ControlRight => Some(57448),
            KeyCode::AltRight => Some(57449),
            KeyCode::SuperRight => Some(57450),
            KeyCode::Hyper => Some(57445),
            KeyCode::Meta => Some(57446),
            _ => None,
        };
    }
    match logical {
        Key::Named(NamedKey::Hyper) => Some(57445),
        Key::Named(NamedKey::Meta) => Some(57446),
        _ => None,
    }
}

fn kitty_named_code(named: NamedKey) -> Option<u32> {
    Some(match named {
        NamedKey::CapsLock => 57358,
        NamedKey::ScrollLock => 57359,
        NamedKey::NumLock => 57360,
        NamedKey::PrintScreen => 57361,
        NamedKey::Pause => 57362,
        NamedKey::ContextMenu => 57363,
        NamedKey::F13 => 57376,
        NamedKey::F14 => 57377,
        NamedKey::F15 => 57378,
        NamedKey::F16 => 57379,
        NamedKey::F17 => 57380,
        NamedKey::F18 => 57381,
        NamedKey::F19 => 57382,
        NamedKey::F20 => 57383,
        NamedKey::F21 => 57384,
        NamedKey::F22 => 57385,
        NamedKey::F23 => 57386,
        NamedKey::F24 => 57387,
        NamedKey::F25 => 57388,
        NamedKey::F26 => 57389,
        NamedKey::F27 => 57390,
        NamedKey::F28 => 57391,
        NamedKey::F29 => 57392,
        NamedKey::F30 => 57393,
        NamedKey::F31 => 57394,
        NamedKey::F32 => 57395,
        NamedKey::F33 => 57396,
        NamedKey::F34 => 57397,
        NamedKey::F35 => 57398,
        NamedKey::MediaPlay => 57428,
        NamedKey::MediaPause => 57429,
        NamedKey::MediaPlayPause => 57430,
        NamedKey::MediaStop => 57432,
        NamedKey::MediaFastForward => 57433,
        NamedKey::MediaRewind => 57434,
        NamedKey::MediaTrackNext => 57435,
        NamedKey::MediaTrackPrevious => 57436,
        NamedKey::MediaRecord => 57437,
        NamedKey::AudioVolumeDown => 57438,
        NamedKey::AudioVolumeUp => 57439,
        NamedKey::AudioVolumeMute => 57440,
        _ => return None,
    })
}

/// Xterm's conventional sequences: everything but a `modifyOtherKeys` report.
fn encode_legacy_xterm(
    logical_key: &Key,
    physical_key: PhysicalKey,
    text: Option<&str>,
    modifiers: ModifiersState,
    alt_graph_active: bool,
    mode: TerminalMode,
    profile: XtermKeyProfile,
) -> Option<Vec<u8>> {
    if let Some(keypad) = KeypadKey::from_physical(physical_key) {
        let (keypad, modifiers) = keypad.profile_mapping(profile.keyboard_type, modifiers);
        return Some(keypad.encode_xterm(
            logical_key,
            mode,
            text,
            profile.modifiers(1, modifiers),
            profile.resource(XtermKeyResource::Keypad),
        ));
    }
    if alt_graph_active && text.is_some_and(|produced| !produced.is_empty()) {
        return text.map(|produced| produced.as_bytes().to_vec());
    }
    let special_modifiers = profile.modifiers(0, modifiers);
    if let Some(encoded) = encode_xterm_special_key(
        logical_key,
        physical_key,
        special_modifiers,
        profile.resource(XtermKeyResource::Special),
    ) {
        return Some(encoded);
    }

    match logical_key {
        Key::Character(character) => encode_character(character, text, modifiers, alt_graph_active),
        Key::Named(named) => encode_xterm_modifier_named(
            *named,
            physical_key,
            special_modifiers,
            profile.resource(XtermKeyResource::Modifier),
        )
        .or_else(|| encode_xterm_function_named(*named, modifiers, profile))
        .or_else(|| encode_xterm_misc_function_named(*named, modifiers, profile))
        .or_else(|| encode_xterm_cursor_named(*named, modifiers, mode, profile))
        .or_else(|| encode_named(*named, modifiers, mode)),
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

/// The xterm encoder: a `modifyOtherKeys` report when the profile asks for
/// one, otherwise the conventional sequences.
fn encode_xterm(
    logical_key: &Key,
    physical_key: PhysicalKey,
    text: Option<&str>,
    modifiers: ModifiersState,
    alt_graph_active: bool,
    mode: TerminalMode,
    profile: XtermKeyProfile,
) -> Option<Vec<u8>> {
    // AltGr is layout composition, not a Control+Alt shortcut. Preserve the
    // produced Unicode text exactly as in the legacy path.
    if alt_graph_active && text.is_some_and(|produced| !produced.is_empty()) {
        return encode_legacy_xterm(
            logical_key,
            physical_key,
            text,
            modifiers,
            true,
            mode,
            profile,
        );
    }
    // Xterm classifies special keysyms before its ordinary-key resource. In
    // mode 4, resource 7 therefore owns keys such as Backspace and Escape
    // even when modifyOtherKeys level 3 would otherwise select them.
    if let Some(encoded) = encode_xterm_special_key(
        logical_key,
        physical_key,
        modifiers,
        profile.resource(XtermKeyResource::Special),
    ) {
        return Some(encoded);
    }

    let policy = profile.other_keys;
    let modifier_bits = xterm_other_modifier_bits(modifiers);
    let factored = modifier_bits.count_ones() == 1 && modifier_bits & policy.modifier_mask() != 0;
    let effective_bits = if factored { 0 } else { modifier_bits };
    let key_code = xterm_other_key_code(logical_key);
    let should_encode = key_code.is_some()
        && match policy.level() {
            1 => match logical_key {
                Key::Named(NamedKey::Tab) => effective_bits != 0x01 && effective_bits != 0,
                Key::Named(NamedKey::Enter) => effective_bits != 0,
                // Preserve xterm's familiar single Shift, Control, and Alt
                // mappings in user mode. Encode combinations whose Shift bit
                // would otherwise be lost, plus Super/Meta which has no
                // unambiguous legacy representation.
                Key::Character(_) | Key::Named(NamedKey::Space) => {
                    effective_bits & 0x08 != 0 || effective_bits & (0x01 | 0x04) == (0x01 | 0x04)
                }
                _ => false,
            },
            2 => effective_bits != 0,
            3 => true,
            _ => false,
        };

    if should_encode {
        return Some(xterm_extended_key(
            policy.format(),
            key_code.expect("checked above"),
            u16::from(effective_bits) + 1,
        ));
    }

    encode_legacy_xterm(
        logical_key,
        physical_key,
        text,
        modifiers,
        alt_graph_active,
        mode,
        profile,
    )
}

fn encode_xterm_modifier_named(
    named: NamedKey,
    physical_key: PhysicalKey,
    modifiers: ModifiersState,
    policy: XtermKeySetting,
) -> Option<Vec<u8>> {
    if policy.mode() != XtermKeyMode::Extended {
        return None;
    }
    let key_code = xterm_modifier_code(named, physical_key)?;
    Some(xterm_extended_key(
        policy.format(),
        key_code,
        extended_modifier_parameter(modifiers),
    ))
}

fn xterm_modifier_code(named: NamedKey, physical_key: PhysicalKey) -> Option<u32> {
    if named == NamedKey::AltGraph {
        // Winit exposes ISO_Level3_Shift as AltGraph. Xterm maps its X11
        // keysym (0xfe03) into this private-use codepoint.
        return Some(0xe103);
    }

    match physical_key {
        PhysicalKey::Code(KeyCode::ShiftLeft) => Some(0xe2e1),
        PhysicalKey::Code(KeyCode::ShiftRight) => Some(0xe2e2),
        PhysicalKey::Code(KeyCode::ControlLeft) => Some(0xe2e3),
        PhysicalKey::Code(KeyCode::ControlRight) => Some(0xe2e4),
        PhysicalKey::Code(KeyCode::CapsLock) => Some(0xe2e5),
        PhysicalKey::Code(KeyCode::Meta) => Some(0xe2e7),
        PhysicalKey::Code(KeyCode::AltLeft) => Some(0xe2e9),
        PhysicalKey::Code(KeyCode::AltRight) => Some(0xe2ea),
        PhysicalKey::Code(KeyCode::SuperLeft) => Some(0xe2eb),
        PhysicalKey::Code(KeyCode::SuperRight) => Some(0xe2ec),
        PhysicalKey::Code(KeyCode::Hyper) => Some(0xe2ed),
        PhysicalKey::Code(KeyCode::NumLock) => Some(0xe27f),
        // Winit cannot distinguish left/right Meta or Hyper. Preserve the
        // available identity using xterm's left-hand keysym mapping.
        PhysicalKey::Unidentified(_) => match named {
            NamedKey::CapsLock => Some(0xe2e5),
            NamedKey::Meta => Some(0xe2e7),
            NamedKey::Hyper => Some(0xe2ed),
            NamedKey::NumLock => Some(0xe27f),
            _ => None,
        },
        _ => None,
    }
}

fn encode_xterm_special_key(
    logical_key: &Key,
    physical_key: PhysicalKey,
    modifiers: ModifiersState,
    policy: XtermKeySetting,
) -> Option<Vec<u8>> {
    if policy.mode() != XtermKeyMode::Extended {
        return None;
    }
    let key_code = xterm_special_code(logical_key, physical_key, modifiers)?;
    Some(xterm_extended_key(
        policy.format(),
        key_code,
        extended_modifier_parameter(modifiers),
    ))
}

fn xterm_special_code(
    logical_key: &Key,
    physical_key: PhysicalKey,
    modifiers: ModifiersState,
) -> Option<u32> {
    let keysym = match logical_key {
        Key::Unidentified(NativeKey::Xkb(keysym)) => *keysym,
        Key::Named(named) => xterm_special_named_keysym(*named, physical_key, modifiers)?,
        _ => return None,
    };
    xterm_special_keysym_code(keysym)
}

fn xterm_special_named_keysym(
    named: NamedKey,
    physical_key: PhysicalKey,
    modifiers: ModifiersState,
) -> Option<u32> {
    Some(match named {
        NamedKey::Backspace => 0xff08,
        NamedKey::Tab if modifiers.shift_key() => 0xfe20,
        NamedKey::Tab => 0xff09,
        NamedKey::Clear
            if !matches!(
                physical_key,
                PhysicalKey::Code(KeyCode::NumpadClear | KeyCode::NumpadClearEntry)
            ) =>
        {
            0xff0b
        }
        NamedKey::Enter => 0xff0d,
        // X11 commonly turns Control+Pause into the function-class Break
        // keysym, which is not owned by resource 7.
        NamedKey::Pause if !modifiers.control_key() => 0xff13,
        NamedKey::ScrollLock => 0xff14,
        NamedKey::Escape => 0xff1b,
        NamedKey::Compose => 0xff20,
        NamedKey::KanjiMode => 0xff21,
        NamedKey::NonConvert => 0xff22,
        NamedKey::Convert => 0xff23,
        NamedKey::Romaji => 0xff24,
        NamedKey::Hiragana => 0xff25,
        NamedKey::Katakana => 0xff26,
        NamedKey::HiraganaKatakana => 0xff27,
        NamedKey::Zenkaku => 0xff28,
        NamedKey::Hankaku => 0xff29,
        NamedKey::ZenkakuHankaku => 0xff2a,
        NamedKey::CodeInput => 0xff37,
        NamedKey::SingleCandidate => 0xff3c,
        NamedKey::AllCandidates => 0xff3d,
        NamedKey::PreviousCandidate => 0xff3e,

        // Unambiguous Winit identities derived from standardized XF86
        // keysyms. Aliases that collapse multiple different keysyms are
        // deliberately omitted because mode 4 must preserve exact identity.
        NamedKey::BrightnessUp => 0x1008_ff02,
        NamedKey::BrightnessDown => 0x1008_ff03,
        NamedKey::AudioVolumeDown => 0x1008_ff11,
        NamedKey::AudioVolumeMute => 0x1008_ff12,
        NamedKey::AudioVolumeUp => 0x1008_ff13,
        NamedKey::MediaPlay => 0x1008_ff14,
        NamedKey::MediaStop => 0x1008_ff15,
        NamedKey::MediaTrackPrevious => 0x1008_ff16,
        NamedKey::MediaTrackNext => 0x1008_ff17,
        NamedKey::BrowserHome => 0x1008_ff18,
        NamedKey::LaunchMail => 0x1008_ff19,
        NamedKey::BrowserSearch => 0x1008_ff1b,
        NamedKey::MediaRecord => 0x1008_ff1c,
        NamedKey::LaunchCalendar => 0x1008_ff20,
        NamedKey::BrowserBack => 0x1008_ff26,
        NamedKey::BrowserForward => 0x1008_ff27,
        NamedKey::WakeUp => 0x1008_ff2b,
        NamedKey::Eject => 0x1008_ff2c,
        NamedKey::LaunchScreenSaver => 0x1008_ff2d,
        NamedKey::LaunchWebBrowser => 0x1008_ff2e,
        NamedKey::MediaPause => 0x1008_ff31,
        NamedKey::LaunchApplication1 => 0x1008_ff33,
        NamedKey::MediaRewind => 0x1008_ff3e,
        NamedKey::Close => 0x1008_ff56,
        NamedKey::Copy => 0x1008_ff57,
        NamedKey::Cut => 0x1008_ff58,
        NamedKey::LaunchSpreadsheet => 0x1008_ff5c,
        NamedKey::LogOff => 0x1008_ff61,
        NamedKey::New => 0x1008_ff68,
        NamedKey::Open => 0x1008_ff6b,
        NamedKey::Paste => 0x1008_ff6d,
        NamedKey::LaunchPhone => 0x1008_ff6e,
        NamedKey::MailReply => 0x1008_ff72,
        NamedKey::Save => 0x1008_ff77,
        NamedKey::MailSend => 0x1008_ff7b,
        NamedKey::SpellCheck => 0x1008_ff7c,
        NamedKey::SplitScreenToggle => 0x1008_ff7d,
        NamedKey::LaunchMediaPlayer => 0x1008_ff87,
        NamedKey::LaunchWordProcessor => 0x1008_ff89,
        NamedKey::ZoomIn => 0x1008_ff8b,
        NamedKey::ZoomOut => 0x1008_ff8c,
        NamedKey::LaunchWebCam => 0x1008_ff8f,
        NamedKey::MailForward => 0x1008_ff90,
        NamedKey::LaunchMusicPlayer => 0x1008_ff92,
        NamedKey::MediaFastForward => 0x1008_ff97,
        NamedKey::RandomToggle => 0x1008_ff99,
        NamedKey::Subtitle => 0x1008_ff9a,
        NamedKey::MediaAudioTrack => 0x1008_ff9b,
        NamedKey::Hibernate => 0x1008_ffa8,
        NamedKey::VideoModeNext => 0x1008_fe22,
        _ => return None,
    })
}

fn xterm_special_keysym_code(keysym: u32) -> Option<u32> {
    let predefined = (0xfe01..=0xffff).contains(&keysym) || keysym >= 0x1000_0000;
    let belongs_to_another_resource = (0xfe01..=0xfe13).contains(&keysym)
        || (0xff50..=0xff5f).contains(&keysym)
        || (0xff60..=0xff6b).contains(&keysym)
        || matches!(keysym, 0xff7e | 0xff7f)
        || (0xff80..=0xffbd).contains(&keysym)
        || (0xffbe..=0xffe0).contains(&keysym)
        || (0xffe1..=0xffee).contains(&keysym)
        || keysym == 0xffff;
    if !predefined || belongs_to_another_resource {
        return None;
    }

    Some(match keysym {
        0xff08 => 0x08,
        0xff09 => 0x09,
        0xff0a => 0x0a,
        0xff0b => 0x0b,
        0xff0d => 0x0d,
        0xff13 => 0x13,
        0xff14 => 0x14,
        0xff15 => 0x15,
        0xff1b => 0x1b,
        0xfd00..=0xffff => keysym - 0x1d00,
        0x1000_0000..=0x1000_00ff => keysym - 0x1000_0000,
        0x1000_f000..=0x1000_ffff => keysym - 0x1000_f000 + 0xf0000,
        0x1004_f000..=0x1004_ffff => keysym - 0x1004_f000 + 0xf1000,
        0x1005_f000..=0x1005_ffff => keysym - 0x1005_f000 + 0xf2000,
        0x1008_1000..=0x1008_1fff => keysym - 0x1008_1000 + 0xf4000,
        0x1008_fe00..=0x1008_ffff => keysym - 0x1008_fe00 + 0xf6000,
        _ => return None,
    })
}

/// Xterm's extended key report in `format`: `CSI 27 ; m ; code ~` or
/// `CSI code ; m u`.
fn xterm_extended_key(format: XtermKeyFormat, code: u32, modifier: u16) -> Vec<u8> {
    match format {
        XtermKeyFormat::FunctionKey27 => format!("\x1b[27;{modifier};{code}~").into_bytes(),
        XtermKeyFormat::CsiU => format!("\x1b[{code};{modifier}u").into_bytes(),
    }
}

/// The modifier parameter of extended xterm and Kitty reports: one plus the
/// Shift, Alt, Control, and Super bits.
fn extended_modifier_parameter(modifiers: ModifiersState) -> u16 {
    u16::from(xterm_other_modifier_bits(modifiers)) + 1
}

fn xterm_other_modifier_bits(modifiers: ModifiersState) -> u8 {
    u8::from(modifiers.shift_key())
        | (u8::from(modifiers.alt_key()) << 1)
        | (u8::from(modifiers.control_key()) << 2)
        | (u8::from(modifiers.super_key()) << 3)
}

fn xterm_other_key_code(logical_key: &Key) -> Option<u32> {
    match logical_key {
        Key::Character(character) => {
            let mut characters = character.chars();
            let character = characters.next()?;
            (characters.next().is_none()).then_some(character as u32)
        }
        Key::Named(NamedKey::Space) => Some(32),
        Key::Named(NamedKey::Tab) => Some(9),
        Key::Named(NamedKey::Enter) => Some(13),
        Key::Named(NamedKey::Escape) => Some(27),
        Key::Named(NamedKey::Backspace) => Some(127),
        _ => None,
    }
}

fn encode_character(
    logical: &str,
    text: Option<&str>,
    modifiers: ModifiersState,
    alt_graph_active: bool,
) -> Option<Vec<u8>> {
    let ctrl = modifiers.control_key();
    let alt = modifiers.alt_key();

    // ModifiersState alone cannot distinguish AltGr from Ctrl+Alt. Track the
    // platform's explicit AltGraph key state so layout-generated Unicode is
    // not converted into Meta+control bytes while real Ctrl+Alt remains usable.
    if alt_graph_active {
        return text
            .or(Some(logical))
            .map(|value| value.as_bytes().to_vec());
    }

    if ctrl {
        let byte = control_byte(logical.chars().next()?)?;
        return Some(prefix_alt(vec![byte], alt));
    }

    let produced = text.unwrap_or(logical);
    if produced.is_empty() {
        None
    } else {
        Some(prefix_alt(produced.as_bytes().to_vec(), alt))
    }
}

fn control_byte(character: char) -> Option<u8> {
    match character {
        'a'..='z' => Some(character as u8 - b'a' + 1),
        'A'..='Z' => Some(character as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XtermLegacyFunctionKey {
    Final(u8),
    Tilde(u8),
}

fn xterm_function_number(named: NamedKey) -> Option<u8> {
    Some(match named {
        NamedKey::F1 => 1,
        NamedKey::F2 => 2,
        NamedKey::F3 => 3,
        NamedKey::F4 => 4,
        NamedKey::F5 => 5,
        NamedKey::F6 => 6,
        NamedKey::F7 => 7,
        NamedKey::F8 => 8,
        NamedKey::F9 => 9,
        NamedKey::F10 => 10,
        NamedKey::F11 => 11,
        NamedKey::F12 => 12,
        NamedKey::F13 => 13,
        NamedKey::F14 => 14,
        NamedKey::F15 => 15,
        NamedKey::F16 => 16,
        NamedKey::F17 => 17,
        NamedKey::F18 => 18,
        NamedKey::F19 => 19,
        NamedKey::F20 => 20,
        NamedKey::F21 => 21,
        NamedKey::F22 => 22,
        NamedKey::F23 => 23,
        NamedKey::F24 => 24,
        NamedKey::F25 => 25,
        NamedKey::F26 => 26,
        NamedKey::F27 => 27,
        NamedKey::F28 => 28,
        NamedKey::F29 => 29,
        NamedKey::F30 => 30,
        NamedKey::F31 => 31,
        NamedKey::F32 => 32,
        NamedKey::F33 => 33,
        NamedKey::F34 => 34,
        NamedKey::F35 => 35,
        _ => return None,
    })
}

fn xterm_dec_function_code(number: u8) -> Option<u8> {
    Some(match number {
        1..=5 => number + 10,
        6..=10 => number + 11,
        11..=14 => number + 12,
        15..=16 => number + 13,
        17..=20 => number + 14,
        _ => return None,
    })
}

fn xterm_user_defined_key_code(
    logical_key: &Key,
    modifiers: ModifiersState,
    keyboard_type: XtermKeyboardType,
) -> Option<u8> {
    if keyboard_type != XtermKeyboardType::Vt220 || !modifiers.shift_key() {
        return None;
    }
    let Key::Named(named) = logical_key else {
        return None;
    };
    if let Some(mut number) = xterm_function_number(*named) {
        if modifiers.control_key() {
            number = number.saturating_add(10);
        }
        return xterm_dec_function_code(number);
    }
    Some(match named {
        NamedKey::Find | NamedKey::Home => 1,
        NamedKey::Insert => 2,
        NamedKey::Delete => 3,
        NamedKey::Select | NamedKey::End => 4,
        NamedKey::PageUp => 5,
        NamedKey::PageDown => 6,
        NamedKey::Help => 28,
        NamedKey::ContextMenu => 29,
        _ => return None,
    })
}

fn xterm_legacy_function_key(number: u8) -> XtermLegacyFunctionKey {
    match number {
        1..=4 => XtermLegacyFunctionKey::Final(b'P' + number - 1),
        5 => XtermLegacyFunctionKey::Tilde(15),
        6 => XtermLegacyFunctionKey::Tilde(17),
        7 => XtermLegacyFunctionKey::Tilde(18),
        8 => XtermLegacyFunctionKey::Tilde(19),
        9 => XtermLegacyFunctionKey::Tilde(20),
        10 => XtermLegacyFunctionKey::Tilde(21),
        11 => XtermLegacyFunctionKey::Tilde(23),
        12 => XtermLegacyFunctionKey::Tilde(24),
        13 => XtermLegacyFunctionKey::Tilde(25),
        14 => XtermLegacyFunctionKey::Tilde(26),
        15 => XtermLegacyFunctionKey::Tilde(28),
        16 => XtermLegacyFunctionKey::Tilde(29),
        17 => XtermLegacyFunctionKey::Tilde(31),
        18 => XtermLegacyFunctionKey::Tilde(32),
        19 => XtermLegacyFunctionKey::Tilde(33),
        20 => XtermLegacyFunctionKey::Tilde(34),
        // Xterm assigns sequential DEC-style codes after F20.
        _ => XtermLegacyFunctionKey::Tilde(number + 21),
    }
}

fn encode_unmodified_xterm_function(number: u8) -> Vec<u8> {
    match xterm_legacy_function_key(number) {
        XtermLegacyFunctionKey::Final(final_byte) => vec![0x1b, b'O', final_byte],
        XtermLegacyFunctionKey::Tilde(number) => format!("\x1b[{number}~").into_bytes(),
    }
}

fn encode_xterm_function_named(
    named: NamedKey,
    mut modifiers: ModifiersState,
    profile: XtermKeyProfile,
) -> Option<Vec<u8>> {
    let policy = profile.resource(XtermKeyResource::Function);
    let keyboard_type = profile.keyboard_type;
    let mut number = xterm_function_number(named)?;
    let historical = keyboard_type.uses_resource_zero();
    if historical && modifiers.control_key() {
        // Xterm reserves Shift for VT220 UDKs and uses Control to reach the
        // next ten numbered function keys in both historical profiles.
        number = number.saturating_add(10);
        modifiers.set(ModifiersState::CONTROL, false);
    }
    let modifiers = profile.modifiers(4, modifiers);
    let modifier_bits = xterm_other_modifier_bits(modifiers);

    if policy.mode() == XtermKeyMode::Extended {
        // X11 F1 starts at keysym 0xffbe. Xterm's keysym-to-Unicode mapping
        // translates that contiguous F1-F35 range to U+E2BE..U+E2E0.
        let codepoint = 58_045 + u32::from(number);
        return Some(xterm_extended_key(
            policy.format(),
            codepoint,
            u16::from(modifier_bits) + 1,
        ));
    }

    if policy.mode() == XtermKeyMode::Disabled {
        if historical {
            return Some(encode_unmodified_xterm_function_profile(
                number,
                keyboard_type,
            ));
        }
        // Current xterm's default ctrlFKeys value is 10. In disabled mode it
        // consumes Shift/Control to synthesize higher numbered function keys,
        // then discards any remaining Alt/Meta modifier.
        let remapped =
            number + 10 * u8::from(modifiers.shift_key()) + 20 * u8::from(modifiers.control_key());
        return Some(encode_unmodified_xterm_function(remapped));
    }

    if modifier_bits == 0 {
        return Some(encode_unmodified_xterm_function_profile(
            number,
            keyboard_type,
        ));
    }

    let modifier = u16::from(modifier_bits) + 1;
    Some(
        match (
            xterm_profile_function_key(number, keyboard_type),
            policy.mode(),
        ) {
            (XtermLegacyFunctionKey::Final(final_byte), XtermKeyMode::Original) => {
                format!("\x1bO{modifier}{}", final_byte as char).into_bytes()
            }
            (XtermLegacyFunctionKey::Final(final_byte), XtermKeyMode::Control) => {
                format!("\x1b[{modifier}{}", final_byte as char).into_bytes()
            }
            (XtermLegacyFunctionKey::Final(final_byte), XtermKeyMode::Parameter) => {
                format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes()
            }
            (XtermLegacyFunctionKey::Final(final_byte), XtermKeyMode::Private) => {
                format!("\x1b[>1;{modifier}{}", final_byte as char).into_bytes()
            }
            (XtermLegacyFunctionKey::Tilde(number), XtermKeyMode::Private) => {
                format!("\x1b[>{number};{modifier}~").into_bytes()
            }
            (XtermLegacyFunctionKey::Tilde(number), _) => {
                format!("\x1b[{number};{modifier}~").into_bytes()
            }
            // Extended and Disabled were handled above.
            (_, XtermKeyMode::Disabled | XtermKeyMode::Extended) => unreachable!(),
        },
    )
}

fn xterm_profile_function_key(
    number: u8,
    keyboard_type: XtermKeyboardType,
) -> XtermLegacyFunctionKey {
    if keyboard_type == XtermKeyboardType::Legacy && (1..=4).contains(&number) {
        XtermLegacyFunctionKey::Tilde(number + 10)
    } else {
        xterm_legacy_function_key(number)
    }
}

fn encode_unmodified_xterm_function_profile(
    number: u8,
    keyboard_type: XtermKeyboardType,
) -> Vec<u8> {
    match xterm_profile_function_key(number, keyboard_type) {
        XtermLegacyFunctionKey::Final(final_byte) => vec![0x1b, b'O', final_byte],
        XtermLegacyFunctionKey::Tilde(number) => format!("\x1b[{number}~").into_bytes(),
    }
}

fn encode_xterm_misc_function_named(
    named: NamedKey,
    modifiers: ModifiersState,
    profile: XtermKeyProfile,
) -> Option<Vec<u8>> {
    let policy = profile.resource(XtermKeyResource::Function);
    let (number, codepoint) = match named {
        NamedKey::Help => (28, 0xe26a),
        NamedKey::ContextMenu => (29, 0xe267),
        _ => return None,
    };
    let modifier_bits = xterm_other_modifier_bits(profile.modifiers(8, modifiers));
    if policy.mode() == XtermKeyMode::Extended {
        return Some(xterm_extended_key(
            policy.format(),
            codepoint,
            u16::from(modifier_bits) + 1,
        ));
    }
    if modifier_bits == 0 || policy.mode() == XtermKeyMode::Disabled {
        return Some(format!("\x1b[{number}~").into_bytes());
    }
    let modifier = u16::from(modifier_bits) + 1;
    Some(match policy.mode() {
        XtermKeyMode::Private => format!("\x1b[>{number};{modifier}~").into_bytes(),
        XtermKeyMode::Original | XtermKeyMode::Control | XtermKeyMode::Parameter => {
            format!("\x1b[{number};{modifier}~").into_bytes()
        }
        XtermKeyMode::Disabled | XtermKeyMode::Extended => unreachable!(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XtermCursorKey {
    Final { final_byte: u8, extended_code: u32 },
    Tilde { number: u8, extended_code: u32 },
}

impl XtermCursorKey {
    fn from_named(named: NamedKey) -> Option<Self> {
        Some(match named {
            NamedKey::Home => Self::Final {
                final_byte: b'H',
                extended_code: 57_936,
            },
            NamedKey::ArrowLeft => Self::Final {
                final_byte: b'D',
                extended_code: 57_937,
            },
            NamedKey::ArrowUp => Self::Final {
                final_byte: b'A',
                extended_code: 57_938,
            },
            NamedKey::ArrowRight => Self::Final {
                final_byte: b'C',
                extended_code: 57_939,
            },
            NamedKey::ArrowDown => Self::Final {
                final_byte: b'B',
                extended_code: 57_940,
            },
            NamedKey::PageUp => Self::Tilde {
                number: 5,
                extended_code: 57_941,
            },
            NamedKey::PageDown => Self::Tilde {
                number: 6,
                extended_code: 57_942,
            },
            NamedKey::End => Self::Final {
                final_byte: b'F',
                extended_code: 57_943,
            },
            NamedKey::Insert => Self::Tilde {
                number: 2,
                extended_code: 57_955,
            },
            // Xterm maps the Delete keysym to DEL rather than the private-use
            // value produced by its generic keysym-to-Unicode mapping.
            NamedKey::Delete => Self::Tilde {
                number: 3,
                extended_code: 127,
            },
            NamedKey::Find => Self::Tilde {
                number: 1,
                extended_code: 0xe268,
            },
            NamedKey::Select => Self::Tilde {
                number: 4,
                extended_code: 0xe260,
            },
            _ => return None,
        })
    }
}

fn encode_xterm_cursor_named(
    named: NamedKey,
    modifiers: ModifiersState,
    mode: TerminalMode,
    profile: XtermKeyProfile,
) -> Option<Vec<u8>> {
    let policy = profile.resource(XtermKeyResource::Cursor);
    let keyboard_type = profile.keyboard_type;
    let named = match (keyboard_type, named) {
        (XtermKeyboardType::Vt220, NamedKey::Home) => NamedKey::Find,
        (XtermKeyboardType::Vt220, NamedKey::End) => NamedKey::Select,
        _ => named,
    };
    let key = XtermCursorKey::from_named(named)?;
    let modifiers = profile.modifiers(2, modifiers);
    if keyboard_type == XtermKeyboardType::Legacy
        && named == NamedKey::Delete
        && modifiers.is_empty()
    {
        return Some(vec![0x7f]);
    }
    let modifier_bits = xterm_other_modifier_bits(modifiers);

    if policy.mode() == XtermKeyMode::Extended {
        let code = match key {
            XtermCursorKey::Final { extended_code, .. }
            | XtermCursorKey::Tilde { extended_code, .. } => extended_code,
        };
        return Some(xterm_extended_key(
            policy.format(),
            code,
            u16::from(modifier_bits) + 1,
        ));
    }

    let application = mode.contains(TerminalMode::CURSOR_KEYS_APPLICATION);
    if modifier_bits == 0 || policy.mode() == XtermKeyMode::Disabled {
        return Some(match key {
            XtermCursorKey::Final { final_byte, .. } => {
                if application {
                    vec![0x1b, b'O', final_byte]
                } else {
                    vec![0x1b, b'[', final_byte]
                }
            }
            XtermCursorKey::Tilde { number, .. } => format!("\x1b[{number}~").into_bytes(),
        });
    }

    let modifier = u16::from(modifier_bits) + 1;
    Some(match (key, policy.mode()) {
        (XtermCursorKey::Final { final_byte, .. }, XtermKeyMode::Original) => {
            let introducer = if application { 'O' } else { '[' };
            format!("\x1b{introducer}{modifier}{}", final_byte as char).into_bytes()
        }
        (XtermCursorKey::Final { final_byte, .. }, XtermKeyMode::Control) => {
            format!("\x1b[{modifier}{}", final_byte as char).into_bytes()
        }
        (XtermCursorKey::Final { final_byte, .. }, XtermKeyMode::Parameter) => {
            format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes()
        }
        (XtermCursorKey::Final { final_byte, .. }, XtermKeyMode::Private) => {
            format!("\x1b[>1;{modifier}{}", final_byte as char).into_bytes()
        }
        (XtermCursorKey::Tilde { number, .. }, XtermKeyMode::Private) => {
            format!("\x1b[>{number};{modifier}~").into_bytes()
        }
        (XtermCursorKey::Tilde { number, .. }, _) => {
            format!("\x1b[{number};{modifier}~").into_bytes()
        }
        // Extended and Disabled were handled above.
        (_, XtermKeyMode::Disabled | XtermKeyMode::Extended) => unreachable!(),
    })
}

fn encode_named(named: NamedKey, modifiers: ModifiersState, mode: TerminalMode) -> Option<Vec<u8>> {
    let application_cursor = mode.contains(TerminalMode::CURSOR_KEYS_APPLICATION);
    let alt = modifiers.alt_key();
    let ctrl = modifiers.control_key();
    let shift = modifiers.shift_key();

    let bytes = match named {
        NamedKey::Enter => prefix_alt(vec![b'\r'], alt),
        NamedKey::Tab if shift => {
            if alt || ctrl {
                modified_final(b'Z', modifiers)
            } else {
                b"\x1b[Z".to_vec()
            }
        }
        NamedKey::Tab => prefix_alt(vec![b'\t'], alt),
        NamedKey::Backspace => prefix_alt(vec![if ctrl { 0x08 } else { 0x7f }], alt),
        NamedKey::Escape => prefix_alt(vec![0x1b], alt),
        NamedKey::Space => {
            let byte = if ctrl { 0x00 } else { b' ' };
            prefix_alt(vec![byte], alt)
        }
        NamedKey::ArrowUp => cursor_key(b'A', application_cursor, modifiers),
        NamedKey::ArrowDown => cursor_key(b'B', application_cursor, modifiers),
        NamedKey::ArrowRight => cursor_key(b'C', application_cursor, modifiers),
        NamedKey::ArrowLeft => cursor_key(b'D', application_cursor, modifiers),
        NamedKey::Home => cursor_key(b'H', application_cursor, modifiers),
        NamedKey::End => cursor_key(b'F', application_cursor, modifiers),
        NamedKey::PageUp => tilde_key(5, modifiers),
        NamedKey::PageDown => tilde_key(6, modifiers),
        NamedKey::Insert => tilde_key(2, modifiers),
        NamedKey::Delete => tilde_key(3, modifiers),
        _ => return None,
    };
    Some(bytes)
}

fn modifier_parameter(modifiers: ModifiersState) -> Option<u8> {
    let bits = u8::from(modifiers.shift_key())
        | (u8::from(modifiers.alt_key()) << 1)
        | (u8::from(modifiers.control_key()) << 2);
    (bits != 0).then_some(bits + 1)
}

fn cursor_key(final_byte: u8, application: bool, modifiers: ModifiersState) -> Vec<u8> {
    if modifier_parameter(modifiers).is_some() {
        modified_final(final_byte, modifiers)
    } else if application {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn modified_final(final_byte: u8, modifiers: ModifiersState) -> Vec<u8> {
    let parameter = modifier_parameter(modifiers).unwrap_or(1);
    format!("\x1b[1;{parameter}{}", final_byte as char).into_bytes()
}

fn tilde_key(number: u8, modifiers: ModifiersState) -> Vec<u8> {
    if let Some(parameter) = modifier_parameter(modifiers) {
        format!("\x1b[{number};{parameter}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

fn prefix_alt(mut bytes: Vec<u8>, alt: bool) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeypadKey {
    Digit(u8),
    Add,
    Comma,
    Decimal,
    Divide,
    Enter,
    Equal,
    Multiply,
    Subtract,
}

impl KeypadKey {
    fn from_physical(physical_key: PhysicalKey) -> Option<Self> {
        let PhysicalKey::Code(code) = physical_key else {
            return None;
        };
        match code {
            KeyCode::Numpad0 => Some(Self::Digit(0)),
            KeyCode::Numpad1 => Some(Self::Digit(1)),
            KeyCode::Numpad2 => Some(Self::Digit(2)),
            KeyCode::Numpad3 => Some(Self::Digit(3)),
            KeyCode::Numpad4 => Some(Self::Digit(4)),
            KeyCode::Numpad5 => Some(Self::Digit(5)),
            KeyCode::Numpad6 => Some(Self::Digit(6)),
            KeyCode::Numpad7 => Some(Self::Digit(7)),
            KeyCode::Numpad8 => Some(Self::Digit(8)),
            KeyCode::Numpad9 => Some(Self::Digit(9)),
            KeyCode::NumpadAdd => Some(Self::Add),
            KeyCode::NumpadComma => Some(Self::Comma),
            KeyCode::NumpadDecimal => Some(Self::Decimal),
            KeyCode::NumpadDivide => Some(Self::Divide),
            KeyCode::NumpadEnter => Some(Self::Enter),
            KeyCode::NumpadEqual => Some(Self::Equal),
            KeyCode::NumpadMultiply => Some(Self::Multiply),
            KeyCode::NumpadSubtract => Some(Self::Subtract),
            _ => None,
        }
    }

    fn encode(
        self,
        mode: TerminalMode,
        produced_text: Option<&str>,
        modifiers: ModifiersState,
    ) -> Vec<u8> {
        if mode.contains(TerminalMode::KEYPAD_APPLICATION) {
            return self.application_sequence(modifiers);
        }

        // Xterm's numeric-keypad path emits the selected numeric character
        // directly in DECKPNM. Modifier-bearing identity is available through
        // DECKPAM or modifyKeypadKeys mode 4.
        match self {
            Self::Digit(digit) => vec![b'0' + digit],
            Self::Add => vec![b'+'],
            Self::Comma => produced_text.unwrap_or(",").as_bytes().to_vec(),
            Self::Decimal => produced_text.unwrap_or(".").as_bytes().to_vec(),
            Self::Divide => vec![b'/'],
            Self::Enter => vec![b'\r'],
            Self::Equal => vec![b'='],
            Self::Multiply => vec![b'*'],
            Self::Subtract => vec![b'-'],
        }
    }

    fn profile_mapping(
        self,
        keyboard_type: XtermKeyboardType,
        mut modifiers: ModifiersState,
    ) -> (Self, ModifiersState) {
        if keyboard_type != XtermKeyboardType::Vt220 || self != Self::Add || modifiers.shift_key() {
            return (self, modifiers);
        }

        // VT220's numeric keypad has a Separator key where a PC keypad has
        // Add. Xterm also maps Control+Separator to Subtract and consumes the
        // Control modifier before encoding it.
        if modifiers.control_key() {
            modifiers.set(ModifiersState::CONTROL, false);
            (Self::Subtract, modifiers)
        } else {
            (Self::Comma, modifiers)
        }
    }

    fn encode_xterm(
        self,
        logical_key: &Key,
        mode: TerminalMode,
        produced_text: Option<&str>,
        modifiers: ModifiersState,
        policy: XtermKeySetting,
    ) -> Vec<u8> {
        if policy.mode() != XtermKeyMode::Extended {
            return self.encode(mode, produced_text, modifiers);
        }

        xterm_extended_key(
            policy.format(),
            self.xterm_codepoint(logical_key),
            extended_modifier_parameter(modifiers),
        )
    }

    fn kitty_code(self) -> u32 {
        match self {
            Self::Digit(digit) => 57399 + u32::from(digit),
            Self::Decimal => 57409,
            Self::Divide => 57410,
            Self::Multiply => 57411,
            Self::Subtract => 57412,
            Self::Add => 57413,
            Self::Enter => 57414,
            Self::Equal => 57415,
            Self::Comma => 57416,
        }
    }

    fn xterm_codepoint(self, logical_key: &Key) -> u32 {
        // With Num Lock off, X11 assigns distinct KP navigation keysyms to
        // the same physical digit/decimal keys. Winit exposes the current
        // logical identity on the event, allowing mode 4 to preserve it.
        if let Key::Named(named) = logical_key {
            let navigation = match (self, named) {
                (Self::Digit(0), NamedKey::Insert) => Some(0xe29e),
                (Self::Digit(1), NamedKey::End) => Some(0xe29c),
                (Self::Digit(2), NamedKey::ArrowDown) => Some(0xe299),
                (Self::Digit(3), NamedKey::PageDown) => Some(0xe29b),
                (Self::Digit(4), NamedKey::ArrowLeft) => Some(0xe296),
                (Self::Digit(5), NamedKey::Clear) => Some(0xe29d),
                (Self::Digit(6), NamedKey::ArrowRight) => Some(0xe298),
                (Self::Digit(7), NamedKey::Home) => Some(0xe295),
                (Self::Digit(8), NamedKey::ArrowUp) => Some(0xe297),
                (Self::Digit(9), NamedKey::PageUp) => Some(0xe29a),
                (Self::Decimal, NamedKey::Delete) => Some(0xe29f),
                _ => None,
            };
            if let Some(codepoint) = navigation {
                return codepoint;
            }
        }

        match self {
            // Xterm maps X11 keypad keysyms 0xff80..0xffff to the private-use
            // range by subtracting 0x1d00. These are the portable numeric
            // keypad identities exposed by Winit.
            Self::Enter => 0xe28d,
            Self::Multiply => 0xe2aa,
            Self::Add => 0xe2ab,
            Self::Comma => 0xe2ac,
            Self::Subtract => 0xe2ad,
            Self::Decimal => 0xe2ae,
            Self::Divide => 0xe2af,
            Self::Digit(digit) => 0xe2b0 + u32::from(digit),
            Self::Equal => 0xe2bd,
        }
    }

    fn application_sequence(self, modifiers: ModifiersState) -> Vec<u8> {
        let final_byte = match self {
            Self::Digit(digit) => b'p' + digit,
            Self::Add => b'k',
            Self::Comma => b'l',
            Self::Decimal => b'n',
            Self::Divide => b'o',
            Self::Enter => b'M',
            Self::Equal => b'X',
            Self::Multiply => b'j',
            Self::Subtract => b'm',
        };
        let mut sequence = vec![0x1b, b'O'];
        let modifier_bits = xterm_other_modifier_bits(modifiers);
        if modifier_bits != 0 {
            sequence.extend_from_slice((u16::from(modifier_bits) + 1).to_string().as_bytes());
        }
        sequence.push(final_byte);
        sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kitty_flags(bits: u8) -> KittyKeyboardFlags {
        KittyKeyboardFlags::from_bits_retain(bits)
    }

    fn mods(shift: bool, alt: bool, ctrl: bool) -> ModifiersState {
        let mut value = ModifiersState::empty();
        value.set(ModifiersState::SHIFT, shift);
        value.set(ModifiersState::ALT, alt);
        value.set(ModifiersState::CONTROL, ctrl);
        value
    }

    const UNIDENTIFIED: PhysicalKey =
        PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);

    fn ch(text: &str) -> Key {
        Key::Character(text.into())
    }

    /// The conventional xterm encoding under the startup profile.
    fn legacy(
        logical: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
        alt_graph_active: bool,
        mode: TerminalMode,
    ) -> Option<Vec<u8>> {
        encode_legacy_xterm(
            logical,
            UNIDENTIFIED,
            text,
            modifiers,
            alt_graph_active,
            mode,
            XtermKeyProfile::default(),
        )
    }

    fn other_keys(other_keys: XtermModifyOtherKeys) -> XtermKeyProfile {
        XtermKeyProfile {
            other_keys,
            ..XtermKeyProfile::default()
        }
    }

    fn named(key: NamedKey, modifiers: ModifiersState, mode: TerminalMode) -> Vec<u8> {
        encode_legacy_xterm(
            &Key::Named(key),
            PhysicalKey::Code(KeyCode::F24),
            None,
            modifiers,
            false,
            mode,
            XtermKeyProfile::default(),
        )
        .unwrap()
    }

    fn modify_other(
        logical: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
        level: u8,
        mask: u8,
    ) -> Vec<u8> {
        encode_xterm(
            logical,
            UNIDENTIFIED,
            text,
            modifiers,
            false,
            TerminalMode::default(),
            other_keys(XtermModifyOtherKeys::new(level, mask).unwrap()),
        )
        .unwrap()
    }

    fn cursor_key_with_policy(
        key: NamedKey,
        modifiers: ModifiersState,
        mode: TerminalMode,
        policy: XtermKeySetting,
    ) -> Vec<u8> {
        encode_legacy_xterm(
            &Key::Named(key),
            PhysicalKey::Code(KeyCode::F24),
            None,
            modifiers,
            false,
            mode,
            XtermKeyProfile {
                resources: XtermKeyResources::default().with(XtermKeyResource::Cursor, policy),
                ..XtermKeyProfile::default()
            },
        )
        .unwrap()
    }

    fn function_key_with_policy(
        key: NamedKey,
        modifiers: ModifiersState,
        policy: XtermKeySetting,
    ) -> Vec<u8> {
        encode_legacy_xterm(
            &Key::Named(key),
            PhysicalKey::Code(KeyCode::F24),
            None,
            modifiers,
            false,
            TerminalMode::default(),
            XtermKeyProfile {
                resources: XtermKeyResources::default().with(XtermKeyResource::Function, policy),
                ..XtermKeyProfile::default()
            },
        )
        .unwrap()
    }

    fn keypad_key_with_policy(
        key: KeyCode,
        modifiers: ModifiersState,
        mode: TerminalMode,
        policy: XtermKeySetting,
    ) -> Vec<u8> {
        keypad_key_with_logical_policy(
            Key::Character("logical-keypad-value".into()),
            key,
            modifiers,
            mode,
            policy,
        )
    }

    fn keypad_key_with_logical_policy(
        logical: Key,
        key: KeyCode,
        modifiers: ModifiersState,
        mode: TerminalMode,
        policy: XtermKeySetting,
    ) -> Vec<u8> {
        encode_legacy_xterm(
            &logical,
            PhysicalKey::Code(key),
            None,
            modifiers,
            false,
            mode,
            XtermKeyProfile {
                resources: XtermKeyResources::default().with(XtermKeyResource::Keypad, policy),
                ..XtermKeyProfile::default()
            },
        )
        .unwrap()
    }

    fn special_key_with_policy(
        logical: Key,
        physical: PhysicalKey,
        modifiers: ModifiersState,
        policy: XtermKeySetting,
    ) -> Option<Vec<u8>> {
        encode_legacy_xterm(
            &logical,
            physical,
            None,
            modifiers,
            false,
            TerminalMode::default(),
            XtermKeyProfile {
                resources: XtermKeyResources::default().with(XtermKeyResource::Special, policy),
                ..XtermKeyProfile::default()
            },
        )
    }

    fn profile_key(
        logical: NamedKey,
        physical: KeyCode,
        modifiers: ModifiersState,
        mode: TerminalMode,
        keyboard_type: XtermKeyboardType,
        modify_keyboard: u16,
    ) -> Vec<u8> {
        encode_xterm(
            &Key::Named(logical),
            PhysicalKey::Code(physical),
            None,
            modifiers,
            false,
            mode,
            XtermKeyProfile {
                keyboard_type,
                keyboard_resource: XtermKeyboardResource::new(modify_keyboard, 0),
                ..XtermKeyProfile::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn legacy_and_vt220_profiles_match_xterm_function_and_editing_keys() {
        use NamedKey::{Delete, End, F1, Home};
        use XtermKeyboardType::{Legacy, Normal, Vt220};
        let none = ModifiersState::empty();
        let alt = mods(false, true, false);
        let shift = mods(true, false, false);
        let ctrl = mods(false, false, true);
        let normal = TerminalMode::default();
        let application = TerminalMode::CURSOR_KEYS_APPLICATION;
        for (key, physical, modifiers, mode, keyboard_type, modify_keyboard, expected) in [
            (
                F1,
                KeyCode::F1,
                none,
                normal,
                Normal,
                0,
                b"\x1bOP".as_slice(),
            ),
            (F1, KeyCode::F1, none, normal, Legacy, 0, b"\x1b[11~"),
            (F1, KeyCode::F1, none, normal, Vt220, 0, b"\x1bOP"),
            // Both historical profiles reserve Shift for UDKs and consume
            // Control to select F(n+10), independently of the permission mask.
            (F1, KeyCode::F1, ctrl, normal, Legacy, 0, b"\x1b[23~"),
            (F1, KeyCode::F1, ctrl, normal, Vt220, 0, b"\x1b[23~"),
            (F1, KeyCode::F1, shift, normal, Legacy, 0, b"\x1b[11~"),
            (F1, KeyCode::F1, shift, normal, Legacy, 4, b"\x1b[11;2~"),
            // VT220 translates the PC Home/End identities to DEC Find/Select.
            (Home, KeyCode::Home, alt, application, Vt220, 0, b"\x1b[1~"),
            (End, KeyCode::End, alt, normal, Vt220, 2, b"\x1b[4;3~"),
            (Delete, KeyCode::Delete, none, normal, Legacy, 0, b"\x7f"),
            (Delete, KeyCode::Delete, none, normal, Vt220, 0, b"\x1b[3~"),
        ] {
            assert_eq!(
                profile_key(
                    key,
                    physical,
                    modifiers,
                    mode,
                    keyboard_type,
                    modify_keyboard
                ),
                expected,
                "{key:?} {modifiers:?} {keyboard_type:?} {modify_keyboard}"
            );
        }
    }

    #[test]
    fn vt220_shift_keys_use_decudk_definitions_before_xterm_resources() {
        let mut terminal = termeric_core::Terminal::new(80, 24);
        terminal.process(b"\x1b[?1061h\x1bP0;1|1/66696e64;17/736978;23/656c6576656e\x1b\\");
        let shift = mods(true, false, false);
        let shift_ctrl = mods(true, false, true);

        assert_eq!(
            encode_xterm_user_defined_key(&Key::Named(NamedKey::F6), shift, &terminal),
            Some(b"six".to_vec())
        );
        assert_eq!(
            encode_xterm_user_defined_key(&Key::Named(NamedKey::F1), shift_ctrl, &terminal),
            Some(b"eleven".to_vec())
        );
        assert_eq!(
            encode_xterm_user_defined_key(&Key::Named(NamedKey::Home), shift, &terminal),
            Some(b"find".to_vec())
        );
        assert_eq!(
            encode_xterm_user_defined_key(
                &Key::Named(NamedKey::F6),
                ModifiersState::empty(),
                &terminal,
            ),
            None
        );

        terminal.process(b"\x1b[?1060h");
        assert_eq!(
            encode_xterm_user_defined_key(&Key::Named(NamedKey::F6), shift, &terminal),
            None
        );
        terminal.process(b"\x1b[?1061h\x1b[=1u");
        assert_eq!(
            encode_xterm_user_defined_key(&Key::Named(NamedKey::F6), shift, &terminal),
            None
        );
    }

    #[test]
    fn modify_keyboard_bits_gate_each_historical_profile_category() {
        let alt = mods(false, true, false);
        let normal = TerminalMode::default();
        for keyboard_type in [XtermKeyboardType::Legacy, XtermKeyboardType::Vt220] {
            let end = if keyboard_type == XtermKeyboardType::Vt220 {
                b"\x1b[4~".as_slice()
            } else {
                b"\x1b[F"
            };
            for (key, physical, modify_keyboard, expected) in [
                (NamedKey::F5, KeyCode::F5, 0, b"\x1b[15~".as_slice()),
                (NamedKey::F5, KeyCode::F5, 4, b"\x1b[15;3~"),
                (NamedKey::Help, KeyCode::Help, 0, b"\x1b[28~"),
                (NamedKey::Help, KeyCode::Help, 8, b"\x1b[28;3~"),
                (NamedKey::End, KeyCode::End, 0, end),
            ] {
                assert_eq!(
                    profile_key(key, physical, alt, normal, keyboard_type, modify_keyboard),
                    expected,
                    "{keyboard_type:?} {key:?} {modify_keyboard}"
                );
            }
        }

        // VT220's keypad Add key is its Separator, and Control+Separator is
        // Subtract with the Control consumed.
        let keypad = TerminalMode::KEYPAD_APPLICATION;
        for (modifiers, modify_keyboard, expected) in [
            (alt, 0, b"\x1bOl".as_slice()),
            (alt, 1, b"\x1bO3l"),
            (mods(false, false, true), 0, b"\x1bOm"),
        ] {
            assert_eq!(
                profile_key(
                    NamedKey::Enter,
                    KeyCode::NumpadAdd,
                    modifiers,
                    keypad,
                    XtermKeyboardType::Vt220,
                    modify_keyboard,
                ),
                expected,
                "{modifiers:?} {modify_keyboard}"
            );
        }
    }

    #[test]
    fn xterm_function_key_modes_formats_remapping_and_codes_match_xterm() {
        use XtermKeyMode::{Control, Disabled, Original, Parameter, Private};
        let modified = mods(false, true, true);
        for (key, modifiers, mode, expected) in [
            (NamedKey::F1, modified, Disabled, b"\x1b[42~".as_slice()),
            (NamedKey::F1, modified, Original, b"\x1bO7P"),
            (NamedKey::F1, modified, Control, b"\x1b[7P"),
            (NamedKey::F1, modified, Parameter, b"\x1b[1;7P"),
            (NamedKey::F1, modified, Private, b"\x1b[>1;7P"),
            (NamedKey::F5, modified, Private, b"\x1b[>15;7~"),
            // Disabled mode uses xterm's default ctrlFKeys=10 remapping and
            // then discards any remaining Alt/Meta state.
            (
                NamedKey::F1,
                mods(true, false, false),
                Disabled,
                b"\x1b[23~",
            ),
            (
                NamedKey::F1,
                mods(false, false, true),
                Disabled,
                b"\x1b[42~",
            ),
            (NamedKey::F1, mods(true, false, true), Disabled, b"\x1b[52~"),
            (NamedKey::F1, mods(false, true, false), Disabled, b"\x1bOP"),
        ] {
            let policy = XtermKeyResource::Function.default_setting().with_mode(mode);
            assert_eq!(
                function_key_with_policy(key, modifiers, policy),
                expected,
                "{key:?} {modifiers:?} {mode:?}"
            );
        }

        for (key, expected) in [
            (NamedKey::F13, b"\x1b[25~".as_slice()),
            (NamedKey::F20, b"\x1b[34~".as_slice()),
            (NamedKey::F21, b"\x1b[42~".as_slice()),
            (NamedKey::F35, b"\x1b[56~".as_slice()),
        ] {
            assert_eq!(
                function_key_with_policy(
                    key,
                    ModifiersState::empty(),
                    XtermKeyResource::Function.default_setting()
                ),
                expected
            );
        }

        let extended = XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::FunctionKey27);
        let all_function_keys = [
            NamedKey::F1,
            NamedKey::F2,
            NamedKey::F3,
            NamedKey::F4,
            NamedKey::F5,
            NamedKey::F6,
            NamedKey::F7,
            NamedKey::F8,
            NamedKey::F9,
            NamedKey::F10,
            NamedKey::F11,
            NamedKey::F12,
            NamedKey::F13,
            NamedKey::F14,
            NamedKey::F15,
            NamedKey::F16,
            NamedKey::F17,
            NamedKey::F18,
            NamedKey::F19,
            NamedKey::F20,
            NamedKey::F21,
            NamedKey::F22,
            NamedKey::F23,
            NamedKey::F24,
            NamedKey::F25,
            NamedKey::F26,
            NamedKey::F27,
            NamedKey::F28,
            NamedKey::F29,
            NamedKey::F30,
            NamedKey::F31,
            NamedKey::F32,
            NamedKey::F33,
            NamedKey::F34,
            NamedKey::F35,
        ];
        for (index, key) in all_function_keys.into_iter().enumerate() {
            let number = index as u8 + 1;
            let codepoint = 58_046 + index as u32;
            assert_eq!(xterm_function_number(key), Some(number));
            assert_eq!(
                function_key_with_policy(key, ModifiersState::empty(), extended),
                format!("\x1b[27;1;{codepoint}~").into_bytes(),
                "extended {key:?}"
            );
        }
        let mut shift_super = mods(true, false, false);
        shift_super.insert(ModifiersState::SUPER);
        assert_eq!(
            function_key_with_policy(
                NamedKey::F13,
                shift_super,
                extended.with_format(XtermKeyFormat::CsiU),
            ),
            b"\x1b[58058;10u"
        );
    }

    #[test]
    fn xterm_cursor_key_modes_formats_and_codes_match_xterm() {
        use XtermKeyMode::{Control, Disabled, Original, Parameter, Private};
        let modified = mods(false, true, true);
        let normal = TerminalMode::default();
        let application = TerminalMode::CURSOR_KEYS_APPLICATION;
        for (key, mode, policy, expected) in [
            (NamedKey::ArrowUp, normal, Disabled, b"\x1b[A".as_slice()),
            (NamedKey::ArrowUp, normal, Original, b"\x1b[7A"),
            (NamedKey::ArrowUp, normal, Control, b"\x1b[7A"),
            (NamedKey::ArrowUp, normal, Parameter, b"\x1b[1;7A"),
            (NamedKey::ArrowUp, normal, Private, b"\x1b[>1;7A"),
            // Disabling strips modifiers but keeps DECCKM for cursor finals.
            (NamedKey::ArrowUp, application, Original, b"\x1bO7A"),
            (NamedKey::ArrowUp, application, Disabled, b"\x1bOA"),
            (NamedKey::PageUp, normal, Private, b"\x1b[>5;7~"),
        ] {
            let setting = XtermKeyResource::Cursor.default_setting().with_mode(policy);
            assert_eq!(
                cursor_key_with_policy(key, modified, mode, setting),
                expected,
                "{key:?} {mode:?} {policy:?}"
            );
        }

        let extended = XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::FunctionKey27);
        let mappings = [
            (NamedKey::Home, 57_936),
            (NamedKey::ArrowLeft, 57_937),
            (NamedKey::ArrowUp, 57_938),
            (NamedKey::ArrowRight, 57_939),
            (NamedKey::ArrowDown, 57_940),
            (NamedKey::PageUp, 57_941),
            (NamedKey::PageDown, 57_942),
            (NamedKey::End, 57_943),
            (NamedKey::Insert, 57_955),
            (NamedKey::Delete, 127),
        ];
        for (key, code) in mappings {
            assert_eq!(
                cursor_key_with_policy(
                    key,
                    ModifiersState::empty(),
                    TerminalMode::CURSOR_KEYS_APPLICATION,
                    extended,
                ),
                format!("\x1b[27;1;{code}~").into_bytes(),
                "key {key:?}"
            );
        }

        let mut shift_super = mods(true, false, false);
        shift_super.insert(ModifiersState::SUPER);
        assert_eq!(
            cursor_key_with_policy(
                NamedKey::Home,
                shift_super,
                TerminalMode::default(),
                extended.with_format(XtermKeyFormat::CsiU),
            ),
            b"\x1b[57936;10u"
        );
    }

    #[test]
    fn xterm_modify_other_keys_levels_masks_and_unicode_are_unambiguous() {
        let none = ModifiersState::empty();
        let tab = Key::Named(NamedKey::Tab);
        let backspace = Key::Named(NamedKey::Backspace);
        for (logical, text, modifiers, level, mask, expected) in [
            // User level preserves familiar one-modifier character mappings
            // but disambiguates Control+Shift and modified control keys.
            (
                ch("R"),
                Some("R"),
                mods(true, false, true),
                1,
                0,
                "\x1b[27;6;82~",
            ),
            (ch("x"), Some("x"), mods(false, true, false), 1, 0, "\x1bx"),
            (
                tab.clone(),
                None,
                mods(false, true, false),
                1,
                0,
                "\x1b[27;3;9~",
            ),
            (tab, None, mods(true, false, false), 1, 0, "\x1b[Z"),
            // Program level encodes every modified ordinary/control key, and
            // level three also encodes unmodified keys.
            (
                ch("i"),
                None,
                mods(false, false, true),
                2,
                0,
                "\x1b[27;5;105~",
            ),
            (
                backspace,
                None,
                mods(false, false, true),
                2,
                0,
                "\x1b[27;5;127~",
            ),
            (ch("λ"), Some("λ"), none, 2, 0, "λ"),
            (ch("λ"), Some("λ"), none, 3, 0, "\x1b[27;1;955~"),
            // A sole masked modifier keeps its legacy behavior. Super is
            // carried as xterm's Meta bit when it is not masked.
            (ch("!"), Some("!"), mods(true, false, false), 2, 1, "!"),
            (
                ch("x"),
                Some("x"),
                ModifiersState::SUPER,
                2,
                0,
                "\x1b[27;9;120~",
            ),
        ] {
            assert_eq!(
                modify_other(&logical, text, modifiers, level, mask),
                expected.as_bytes(),
                "{logical:?} {modifiers:?} level {level} mask {mask}"
            );
        }

        let altgr = encode_xterm(
            &ch("€"),
            UNIDENTIFIED,
            Some("€"),
            mods(false, true, true),
            true,
            TerminalMode::default(),
            other_keys(XtermModifyOtherKeys::new(2, 0).unwrap()),
        );
        assert_eq!(altgr, Some("€".as_bytes().to_vec()));

        // AltGr on a key that produced no text: below the level that encodes
        // it, xterm falls back to the legacy encoding, which must treat AltGr
        // as layout composition there too, not as Control+Alt.
        for level in [0, 1] {
            let unproduced = encode_xterm(
                &ch("e"),
                UNIDENTIFIED,
                None,
                mods(false, true, true),
                true,
                TerminalMode::default(),
                other_keys(XtermModifyOtherKeys::new(level, 0).unwrap()),
            );
            assert_eq!(
                unproduced,
                legacy(
                    &ch("e"),
                    None,
                    mods(false, true, true),
                    true,
                    TerminalMode::default()
                ),
                "level {level}"
            );
            assert_eq!(unproduced, Some(b"e".to_vec()), "level {level}");
        }
    }

    #[test]
    fn xterm_modify_other_keys_csi_u_format_preserves_codes_and_modifiers() {
        for (logical, physical, text, modifiers, level, expected) in [
            (
                ch("i"),
                PhysicalKey::Code(KeyCode::KeyI),
                None,
                mods(false, false, true),
                2,
                b"\x1b[105;5u".as_slice(),
            ),
            (
                ch("λ"),
                UNIDENTIFIED,
                Some("λ"),
                ModifiersState::empty(),
                3,
                b"\x1b[955;1u",
            ),
        ] {
            let policy = XtermModifyOtherKeys::new(level, 0)
                .unwrap()
                .with_format(XtermKeyFormat::CsiU);
            assert_eq!(
                encode_xterm(
                    &logical,
                    physical,
                    text,
                    modifiers,
                    false,
                    TerminalMode::default(),
                    other_keys(policy),
                ),
                Some(expected.to_vec()),
                "{logical:?}"
            );
        }
    }

    #[test]
    fn xterm_modifier_key_mode_four_matches_portable_xterm_keysym_codes() {
        let policy = XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::FunctionKey27);
        for (named, physical, code) in [
            (NamedKey::Shift, KeyCode::ShiftLeft, 0xe2e1),
            (NamedKey::Shift, KeyCode::ShiftRight, 0xe2e2),
            (NamedKey::Control, KeyCode::ControlLeft, 0xe2e3),
            (NamedKey::Control, KeyCode::ControlRight, 0xe2e4),
            (NamedKey::CapsLock, KeyCode::CapsLock, 0xe2e5),
            (NamedKey::Meta, KeyCode::Meta, 0xe2e7),
            (NamedKey::Alt, KeyCode::AltLeft, 0xe2e9),
            (NamedKey::Alt, KeyCode::AltRight, 0xe2ea),
            (NamedKey::Super, KeyCode::SuperLeft, 0xe2eb),
            (NamedKey::Super, KeyCode::SuperRight, 0xe2ec),
            (NamedKey::Hyper, KeyCode::Hyper, 0xe2ed),
            (NamedKey::NumLock, KeyCode::NumLock, 0xe27f),
        ] {
            assert_eq!(
                encode_xterm_modifier_named(
                    named,
                    PhysicalKey::Code(physical),
                    ModifiersState::empty(),
                    policy,
                ),
                Some(format!("\x1b[27;1;{code}~").into_bytes())
            );
        }
        assert_eq!(
            encode_xterm_modifier_named(
                NamedKey::AltGraph,
                PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                ModifiersState::empty(),
                policy,
            ),
            Some(b"\x1b[27;1;57603~".to_vec())
        );
    }

    #[test]
    fn xterm_modifier_keys_are_silent_before_mode_four_and_honor_csi_u() {
        for mode in [
            XtermKeyMode::Disabled,
            XtermKeyMode::Original,
            XtermKeyMode::Control,
            XtermKeyMode::Parameter,
            XtermKeyMode::Private,
        ] {
            assert_eq!(
                encode_xterm_modifier_named(
                    NamedKey::Shift,
                    PhysicalKey::Code(KeyCode::ShiftLeft),
                    ModifiersState::SHIFT,
                    XtermKeySetting::new(mode, XtermKeyFormat::FunctionKey27),
                ),
                None
            );
        }

        let mut modified = ModifiersState::SHIFT;
        modified.insert(ModifiersState::SUPER);
        assert_eq!(
            encode_xterm_modifier_named(
                NamedKey::Super,
                PhysicalKey::Code(KeyCode::SuperRight),
                modified,
                XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU),
            ),
            Some(b"\x1b[58092;10u".to_vec())
        );
        assert_eq!(
            encode_xterm_modifier_named(
                NamedKey::Meta,
                PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                ModifiersState::empty(),
                XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU),
            ),
            Some(b"\x1b[58087;1u".to_vec())
        );
    }

    #[test]
    fn xterm_special_mode_four_covers_control_compose_and_ime_keysyms() {
        let policy = XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::FunctionKey27);
        for (named, keysym_code) in [
            (NamedKey::Backspace, 0x08),
            (NamedKey::Tab, 0x09),
            (NamedKey::Clear, 0x0b),
            (NamedKey::Enter, 0x0d),
            (NamedKey::Pause, 0x13),
            (NamedKey::ScrollLock, 0x14),
            (NamedKey::Escape, 0x1b),
            (NamedKey::Compose, 0xe220),
            (NamedKey::KanjiMode, 0xe221),
            (NamedKey::NonConvert, 0xe222),
            (NamedKey::Convert, 0xe223),
            (NamedKey::Romaji, 0xe224),
            (NamedKey::Hiragana, 0xe225),
            (NamedKey::Katakana, 0xe226),
            (NamedKey::HiraganaKatakana, 0xe227),
            (NamedKey::Zenkaku, 0xe228),
            (NamedKey::Hankaku, 0xe229),
            (NamedKey::ZenkakuHankaku, 0xe22a),
            (NamedKey::CodeInput, 0xe237),
            (NamedKey::SingleCandidate, 0xe23c),
            (NamedKey::AllCandidates, 0xe23d),
            (NamedKey::PreviousCandidate, 0xe23e),
        ] {
            assert_eq!(
                special_key_with_policy(
                    Key::Named(named),
                    PhysicalKey::Code(KeyCode::F24),
                    ModifiersState::empty(),
                    policy,
                ),
                Some(format!("\x1b[27;1;{keysym_code}~").into_bytes())
            );
        }

        // Shift+Tab is the distinct ISO_Left_Tab keysym in xterm's special
        // category, not the ordinary Tab code with only a modifier change.
        assert_eq!(
            special_key_with_policy(
                Key::Named(NamedKey::Tab),
                PhysicalKey::Code(KeyCode::Tab),
                ModifiersState::SHIFT,
                policy,
            ),
            Some(b"\x1b[27;2;57632~".to_vec())
        );
    }

    #[test]
    fn xterm_special_mode_four_covers_unambiguous_xf86_named_keys() {
        let policy = XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU);
        for (named, codepoint) in [
            (NamedKey::BrightnessUp, 0xf6102),
            (NamedKey::BrightnessDown, 0xf6103),
            (NamedKey::AudioVolumeDown, 0xf6111),
            (NamedKey::AudioVolumeMute, 0xf6112),
            (NamedKey::AudioVolumeUp, 0xf6113),
            (NamedKey::MediaPlay, 0xf6114),
            (NamedKey::MediaStop, 0xf6115),
            (NamedKey::MediaTrackPrevious, 0xf6116),
            (NamedKey::MediaTrackNext, 0xf6117),
            (NamedKey::BrowserHome, 0xf6118),
            (NamedKey::LaunchMail, 0xf6119),
            (NamedKey::BrowserSearch, 0xf611b),
            (NamedKey::MediaRecord, 0xf611c),
            (NamedKey::LaunchCalendar, 0xf6120),
            (NamedKey::BrowserBack, 0xf6126),
            (NamedKey::BrowserForward, 0xf6127),
            (NamedKey::WakeUp, 0xf612b),
            (NamedKey::Eject, 0xf612c),
            (NamedKey::LaunchScreenSaver, 0xf612d),
            (NamedKey::LaunchWebBrowser, 0xf612e),
            (NamedKey::MediaPause, 0xf6131),
            (NamedKey::LaunchApplication1, 0xf6133),
            (NamedKey::MediaRewind, 0xf613e),
            (NamedKey::Close, 0xf6156),
            (NamedKey::Copy, 0xf6157),
            (NamedKey::Cut, 0xf6158),
            (NamedKey::LaunchSpreadsheet, 0xf615c),
            (NamedKey::LogOff, 0xf6161),
            (NamedKey::New, 0xf6168),
            (NamedKey::Open, 0xf616b),
            (NamedKey::Paste, 0xf616d),
            (NamedKey::LaunchPhone, 0xf616e),
            (NamedKey::MailReply, 0xf6172),
            (NamedKey::Save, 0xf6177),
            (NamedKey::MailSend, 0xf617b),
            (NamedKey::SpellCheck, 0xf617c),
            (NamedKey::SplitScreenToggle, 0xf617d),
            (NamedKey::LaunchMediaPlayer, 0xf6187),
            (NamedKey::LaunchWordProcessor, 0xf6189),
            (NamedKey::ZoomIn, 0xf618b),
            (NamedKey::ZoomOut, 0xf618c),
            (NamedKey::LaunchWebCam, 0xf618f),
            (NamedKey::MailForward, 0xf6190),
            (NamedKey::LaunchMusicPlayer, 0xf6192),
            (NamedKey::MediaFastForward, 0xf6197),
            (NamedKey::RandomToggle, 0xf6199),
            (NamedKey::Subtitle, 0xf619a),
            (NamedKey::MediaAudioTrack, 0xf619b),
            (NamedKey::Hibernate, 0xf61a8),
            (NamedKey::VideoModeNext, 0xf6022),
        ] {
            assert_eq!(
                special_key_with_policy(
                    Key::Named(named),
                    PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                    ModifiersState::ALT | ModifiersState::CONTROL,
                    policy,
                ),
                Some(format!("\x1b[{codepoint};7u").into_bytes())
            );
        }
    }

    #[test]
    fn xterm_special_raw_xkb_path_is_exact_bounded_and_category_safe() {
        // Winit preserves otherwise-unmapped XKB keysyms. Match xterm's
        // extended keysym conversion instead of reducing those to a guessed
        // named-key identity.
        assert_eq!(
            xterm_special_code(
                &Key::Unidentified(NativeKey::Xkb(0x1008_ffb8)),
                PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                ModifiersState::empty(),
            ),
            Some(0xf61b8)
        );
        assert_eq!(
            xterm_special_code(
                &Key::Unidentified(NativeKey::Xkb(0xfe20)),
                PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                ModifiersState::empty(),
            ),
            Some(0xe120)
        );
        for keysym in [
            0x20,
            0xfe03,
            0xff51,
            0xff61,
            0xff7f,
            0xffab,
            0xffbe,
            0xffe1,
            0xffff,
            0x1009_0000,
        ] {
            assert_eq!(
                xterm_special_keysym_code(keysym),
                None,
                "keysym {keysym:#x}"
            );
        }
    }

    #[test]
    fn xterm_special_modes_before_four_preserve_legacy_and_mode_four_wins_other_keys() {
        for mode in [
            XtermKeyMode::Disabled,
            XtermKeyMode::Original,
            XtermKeyMode::Control,
            XtermKeyMode::Parameter,
            XtermKeyMode::Private,
        ] {
            let policy = XtermKeySetting::new(mode, XtermKeyFormat::CsiU);
            assert_eq!(
                special_key_with_policy(
                    Key::Named(NamedKey::Backspace),
                    PhysicalKey::Code(KeyCode::Backspace),
                    ModifiersState::empty(),
                    policy,
                ),
                Some(vec![0x7f])
            );
            assert_eq!(
                special_key_with_policy(
                    Key::Named(NamedKey::Compose),
                    PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
                    ModifiersState::empty(),
                    policy,
                ),
                None
            );
        }

        let encoded = encode_xterm(
            &Key::Named(NamedKey::Backspace),
            PhysicalKey::Code(KeyCode::Backspace),
            None,
            ModifiersState::empty(),
            false,
            TerminalMode::default(),
            XtermKeyProfile {
                other_keys: XtermModifyOtherKeys::new(3, 0).unwrap(),
                resources: XtermKeyResources::default().with(
                    XtermKeyResource::Special,
                    XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::CsiU),
                ),
                ..XtermKeyProfile::default()
            },
        );
        assert_eq!(encoded, Some(b"\x1b[8;1u".to_vec()));
    }

    #[test]
    fn kitty_protocol_takes_precedence_over_xterm_resources() {
        let kitty = KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES;
        let resource = |resource: XtermKeyResource, mode| XtermKeyProfile {
            resources: XtermKeyResources::default()
                .with(resource, resource.default_setting().with_mode(mode)),
            ..XtermKeyProfile::default()
        };
        for profile in [
            XtermKeyProfile::default(),
            XtermKeyProfile {
                other_keys: XtermModifyOtherKeys::new(2, 0).unwrap(),
                ..XtermKeyProfile::default()
            },
            resource(XtermKeyResource::Cursor, XtermKeyMode::Private),
            resource(XtermKeyResource::Function, XtermKeyMode::Private),
            resource(XtermKeyResource::Keypad, XtermKeyMode::Extended),
            resource(XtermKeyResource::Modifier, XtermKeyMode::Extended),
            resource(XtermKeyResource::Special, XtermKeyMode::Extended),
            XtermKeyProfile {
                keyboard_type: XtermKeyboardType::Vt220,
                keyboard_resource: XtermKeyboardResource::new(15, 42),
                ..XtermKeyProfile::default()
            },
        ] {
            assert_eq!(
                KeyboardProtocol::new(kitty, profile),
                KeyboardProtocol::Kitty(kitty)
            );
            assert_eq!(
                KeyboardProtocol::new(KittyKeyboardFlags::empty(), profile),
                KeyboardProtocol::Xterm(profile)
            );
        }
    }

    #[test]
    fn kitty_csi_u_encodes_alternate_keys_events_and_associated_text() {
        let flags = kitty_flags(31);
        assert_eq!(
            encode_kitty_key(
                KittyKey::Code(97),
                Some(65),
                Some(97),
                Some("A"),
                mods(true, false, false),
                KittyEventType::Press,
                flags,
            ),
            b"\x1b[97:65;2:1;65u"
        );
        assert_eq!(
            encode_kitty_key(
                KittyKey::Code('с' as u32),
                None,
                Some('c' as u32),
                None,
                mods(false, false, true),
                KittyEventType::Release,
                flags,
            ),
            b"\x1b[1089::99;5:3u"
        );
    }

    #[test]
    fn kitty_functional_keys_report_press_repeat_and_release() {
        let flags = KittyKeyboardFlags::REPORT_EVENT_TYPES;
        for (event, expected) in [
            (KittyEventType::Press, b"\x1b[1;1:1A".as_slice()),
            (KittyEventType::Repeat, b"\x1b[1;1:2A".as_slice()),
            (KittyEventType::Release, b"\x1b[1;1:3A".as_slice()),
        ] {
            assert_eq!(
                encode_kitty_key(
                    KittyKey::Final(b'A'),
                    None,
                    None,
                    None,
                    ModifiersState::empty(),
                    event,
                    flags,
                ),
                expected
            );
        }
        assert_eq!(
            encode_kitty_key(
                KittyKey::Tilde(3),
                None,
                None,
                None,
                mods(false, true, true),
                KittyEventType::Repeat,
                flags,
            ),
            b"\x1b[3;7:2~"
        );
    }

    #[test]
    fn kitty_all_keys_covers_keypad_extended_functions_and_modifier_bits() {
        assert_eq!(KeypadKey::Digit(0).kitty_code(), 57399);
        assert_eq!(KeypadKey::Digit(9).kitty_code(), 57408);
        assert_eq!(KeypadKey::Enter.kitty_code(), 57414);
        assert_eq!(kitty_named_code(NamedKey::F35), Some(57398));
        assert_eq!(kitty_named_code(NamedKey::MediaPlayPause), Some(57430));
        assert_eq!(
            kitty_modifier_code(
                PhysicalKey::Code(KeyCode::ControlRight),
                &Key::Named(NamedKey::Control),
            ),
            Some(57448)
        );
        let mut modifiers = mods(true, true, true);
        modifiers.insert(ModifiersState::SUPER);
        assert_eq!(extended_modifier_parameter(modifiers), 16);
    }

    #[test]
    fn kitty_ime_text_is_encoded_only_when_both_required_flags_are_active() {
        assert_eq!(
            encode_ime_text(KittyKeyboardFlags::REPORT_ALL_KEYS, "日本"),
            "日本".as_bytes()
        );
        assert_eq!(
            encode_ime_text(
                KittyKeyboardFlags::REPORT_ALL_KEYS | KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT,
                "日\n本",
            ),
            b"\x1b[0;1;26085:26412u"
        );
    }

    #[test]
    fn every_supported_named_key_has_legacy_and_modified_sequences() {
        let plain = ModifiersState::empty();
        let modified = mods(true, true, true);
        let cases = [
            (NamedKey::Enter, b"\r".as_slice(), b"\x1b\r".as_slice()),
            (NamedKey::Tab, b"\t", b"\x1b[1;8Z"),
            (NamedKey::Backspace, b"\x7f", b"\x1b\x08"),
            (NamedKey::Escape, b"\x1b", b"\x1b\x1b"),
            (NamedKey::Space, b" ", b"\x1b\0"),
            (NamedKey::ArrowUp, b"\x1b[A", b"\x1b[1;8A"),
            (NamedKey::ArrowDown, b"\x1b[B", b"\x1b[1;8B"),
            (NamedKey::ArrowRight, b"\x1b[C", b"\x1b[1;8C"),
            (NamedKey::ArrowLeft, b"\x1b[D", b"\x1b[1;8D"),
            (NamedKey::Home, b"\x1b[H", b"\x1b[1;8H"),
            (NamedKey::End, b"\x1b[F", b"\x1b[1;8F"),
            (NamedKey::PageUp, b"\x1b[5~", b"\x1b[5;8~"),
            (NamedKey::PageDown, b"\x1b[6~", b"\x1b[6;8~"),
            (NamedKey::Insert, b"\x1b[2~", b"\x1b[2;8~"),
            (NamedKey::Delete, b"\x1b[3~", b"\x1b[3;8~"),
            (NamedKey::F1, b"\x1bOP", b"\x1b[1;8P"),
            (NamedKey::F2, b"\x1bOQ", b"\x1b[1;8Q"),
            (NamedKey::F3, b"\x1bOR", b"\x1b[1;8R"),
            (NamedKey::F4, b"\x1bOS", b"\x1b[1;8S"),
            (NamedKey::F5, b"\x1b[15~", b"\x1b[15;8~"),
            (NamedKey::F6, b"\x1b[17~", b"\x1b[17;8~"),
            (NamedKey::F7, b"\x1b[18~", b"\x1b[18;8~"),
            (NamedKey::F8, b"\x1b[19~", b"\x1b[19;8~"),
            (NamedKey::F9, b"\x1b[20~", b"\x1b[20;8~"),
            (NamedKey::F10, b"\x1b[21~", b"\x1b[21;8~"),
            (NamedKey::F11, b"\x1b[23~", b"\x1b[23;8~"),
            (NamedKey::F12, b"\x1b[24~", b"\x1b[24;8~"),
        ];

        for (key, expected_plain, expected_modified) in cases {
            assert_eq!(named(key, plain, TerminalMode::default()), expected_plain);
            assert_eq!(
                named(key, modified, TerminalMode::default()),
                expected_modified,
                "modified encoding for {key:?}"
            );
        }
    }

    #[test]
    fn application_cursor_mode_changes_arrows_home_and_end_only_when_unmodified() {
        let application = TerminalMode::CURSOR_KEYS_APPLICATION;
        for (key, final_byte) in [
            (NamedKey::ArrowUp, b'A'),
            (NamedKey::ArrowDown, b'B'),
            (NamedKey::ArrowRight, b'C'),
            (NamedKey::ArrowLeft, b'D'),
            (NamedKey::Home, b'H'),
            (NamedKey::End, b'F'),
        ] {
            assert_eq!(
                named(key, ModifiersState::empty(), application),
                [0x1b, b'O', final_byte]
            );
            assert_eq!(
                named(key, mods(true, false, false), application),
                [0x1b, b'[', b'1', b';', b'2', final_byte]
            );
        }
    }

    #[test]
    fn all_xterm_modifier_parameters_are_generated() {
        let modifier_cases = [
            (true, false, false, 2),
            (false, true, false, 3),
            (true, true, false, 4),
            (false, false, true, 5),
            (true, false, true, 6),
            (false, true, true, 7),
            (true, true, true, 8),
        ];
        for (shift, alt, ctrl, expected) in modifier_cases {
            assert_eq!(modifier_parameter(mods(shift, alt, ctrl)), Some(expected));
        }

        let final_keys = [
            (NamedKey::ArrowUp, b'A'),
            (NamedKey::ArrowDown, b'B'),
            (NamedKey::ArrowRight, b'C'),
            (NamedKey::ArrowLeft, b'D'),
            (NamedKey::Home, b'H'),
            (NamedKey::End, b'F'),
            (NamedKey::F1, b'P'),
            (NamedKey::F2, b'Q'),
            (NamedKey::F3, b'R'),
            (NamedKey::F4, b'S'),
        ];
        let tilde_keys = [
            (NamedKey::Insert, 2),
            (NamedKey::Delete, 3),
            (NamedKey::PageUp, 5),
            (NamedKey::PageDown, 6),
            (NamedKey::F5, 15),
            (NamedKey::F6, 17),
            (NamedKey::F7, 18),
            (NamedKey::F8, 19),
            (NamedKey::F9, 20),
            (NamedKey::F10, 21),
            (NamedKey::F11, 23),
            (NamedKey::F12, 24),
        ];
        for (shift, alt, ctrl, parameter) in modifier_cases {
            let modifiers = mods(shift, alt, ctrl);
            for (key, final_byte) in final_keys {
                assert_eq!(
                    named(key, modifiers, TerminalMode::default()),
                    format!("\x1b[1;{parameter}{}", final_byte as char).into_bytes()
                );
            }
            for (key, number) in tilde_keys {
                assert_eq!(
                    named(key, modifiers, TerminalMode::default()),
                    format!("\x1b[{number};{parameter}~").into_bytes()
                );
            }
        }
    }

    #[test]
    fn application_keypad_uses_physical_identity_and_normal_mode_restores_text() {
        let cases = [
            (KeyCode::Numpad0, b"0".as_slice(), b"\x1bOp".as_slice()),
            (KeyCode::Numpad1, b"1", b"\x1bOq"),
            (KeyCode::Numpad2, b"2", b"\x1bOr"),
            (KeyCode::Numpad3, b"3", b"\x1bOs"),
            (KeyCode::Numpad4, b"4", b"\x1bOt"),
            (KeyCode::Numpad5, b"5", b"\x1bOu"),
            (KeyCode::Numpad6, b"6", b"\x1bOv"),
            (KeyCode::Numpad7, b"7", b"\x1bOw"),
            (KeyCode::Numpad8, b"8", b"\x1bOx"),
            (KeyCode::Numpad9, b"9", b"\x1bOy"),
            (KeyCode::NumpadAdd, b"+", b"\x1bOk"),
            (KeyCode::NumpadComma, b",", b"\x1bOl"),
            (KeyCode::NumpadDecimal, b".", b"\x1bOn"),
            (KeyCode::NumpadDivide, b"/", b"\x1bOo"),
            (KeyCode::NumpadEnter, b"\r", b"\x1bOM"),
            (KeyCode::NumpadEqual, b"=", b"\x1bOX"),
            (KeyCode::NumpadMultiply, b"*", b"\x1bOj"),
            (KeyCode::NumpadSubtract, b"-", b"\x1bOm"),
        ];

        let policy = XtermKeyResource::Keypad.default_setting();
        let none = ModifiersState::empty();
        for (physical, normal, application) in cases {
            assert_eq!(
                keypad_key_with_policy(physical, none, TerminalMode::default(), policy),
                normal
            );
            assert_eq!(
                keypad_key_with_policy(physical, none, TerminalMode::KEYPAD_APPLICATION, policy),
                application
            );
        }
    }

    #[test]
    fn xterm_keypad_modes_formats_and_codes_match_xterm() {
        let control_alt = mods(false, true, true);
        for mode in [
            XtermKeyMode::Disabled,
            XtermKeyMode::Original,
            XtermKeyMode::Control,
            XtermKeyMode::Parameter,
            XtermKeyMode::Private,
        ] {
            let policy = XtermKeyResource::Keypad.default_setting().with_mode(mode);
            assert_eq!(
                keypad_key_with_policy(
                    KeyCode::Numpad7,
                    control_alt,
                    TerminalMode::default(),
                    policy,
                ),
                b"7",
                "normal mode {mode:?}"
            );
            assert_eq!(
                keypad_key_with_policy(
                    KeyCode::Numpad7,
                    control_alt,
                    TerminalMode::KEYPAD_APPLICATION,
                    policy,
                ),
                b"\x1bO7w",
                "application mode {mode:?}"
            );
        }

        let extended = XtermKeySetting::new(XtermKeyMode::Extended, XtermKeyFormat::FunctionKey27);
        let keys = [
            (KeyCode::NumpadEnter, 0xe28d),
            (KeyCode::NumpadMultiply, 0xe2aa),
            (KeyCode::NumpadAdd, 0xe2ab),
            (KeyCode::NumpadComma, 0xe2ac),
            (KeyCode::NumpadSubtract, 0xe2ad),
            (KeyCode::NumpadDecimal, 0xe2ae),
            (KeyCode::NumpadDivide, 0xe2af),
            (KeyCode::Numpad0, 0xe2b0),
            (KeyCode::Numpad1, 0xe2b1),
            (KeyCode::Numpad2, 0xe2b2),
            (KeyCode::Numpad3, 0xe2b3),
            (KeyCode::Numpad4, 0xe2b4),
            (KeyCode::Numpad5, 0xe2b5),
            (KeyCode::Numpad6, 0xe2b6),
            (KeyCode::Numpad7, 0xe2b7),
            (KeyCode::Numpad8, 0xe2b8),
            (KeyCode::Numpad9, 0xe2b9),
            (KeyCode::NumpadEqual, 0xe2bd),
        ];
        for (key, codepoint) in keys {
            assert_eq!(
                keypad_key_with_policy(
                    key,
                    ModifiersState::empty(),
                    TerminalMode::default(),
                    extended,
                ),
                format!("\x1b[27;1;{codepoint}~").into_bytes(),
                "extended {key:?}"
            );
        }

        for (key, logical, codepoint) in [
            (KeyCode::Numpad0, NamedKey::Insert, 0xe29e),
            (KeyCode::Numpad1, NamedKey::End, 0xe29c),
            (KeyCode::Numpad2, NamedKey::ArrowDown, 0xe299),
            (KeyCode::Numpad3, NamedKey::PageDown, 0xe29b),
            (KeyCode::Numpad4, NamedKey::ArrowLeft, 0xe296),
            (KeyCode::Numpad5, NamedKey::Clear, 0xe29d),
            (KeyCode::Numpad6, NamedKey::ArrowRight, 0xe298),
            (KeyCode::Numpad7, NamedKey::Home, 0xe295),
            (KeyCode::Numpad8, NamedKey::ArrowUp, 0xe297),
            (KeyCode::Numpad9, NamedKey::PageUp, 0xe29a),
            (KeyCode::NumpadDecimal, NamedKey::Delete, 0xe29f),
        ] {
            assert_eq!(
                keypad_key_with_logical_policy(
                    Key::Named(logical),
                    key,
                    ModifiersState::empty(),
                    TerminalMode::default(),
                    extended,
                ),
                format!("\x1b[27;1;{codepoint}~").into_bytes(),
                "Num Lock-off {key:?}"
            );
        }

        let mut shift_super = mods(true, false, false);
        shift_super.insert(ModifiersState::SUPER);
        assert_eq!(
            keypad_key_with_policy(
                KeyCode::NumpadEnter,
                shift_super,
                TerminalMode::KEYPAD_APPLICATION,
                extended.with_format(XtermKeyFormat::CsiU),
            ),
            b"\x1b[57997;10u"
        );
    }

    #[test]
    fn control_meta_altgr_and_unicode_text_do_not_conflict() {
        let space = Key::Named(NamedKey::Space);
        for (logical, text, modifiers, alt_graph_active, expected) in [
            (ch("C"), "C", mods(true, false, true), false, "\x03"),
            (ch("x"), "x", mods(false, true, false), false, "\x1bx"),
            (ch("€"), "€", mods(false, true, true), true, "€"),
            (space, "\u{a0}", mods(false, true, true), true, "\u{a0}"),
            (ch("x"), "x", mods(false, true, true), false, "\x1b\x18"),
            (ch("界"), "界", ModifiersState::empty(), false, "界"),
        ] {
            assert_eq!(
                legacy(
                    &logical,
                    Some(text),
                    modifiers,
                    alt_graph_active,
                    TerminalMode::default()
                ),
                Some(expected.as_bytes().to_vec()),
                "{logical:?} {modifiers:?} AltGr {alt_graph_active}"
            );
        }
    }

    #[test]
    fn traditional_control_character_aliases_are_complete() {
        for (character, expected) in [
            ('@', 0x00),
            ('2', 0x00),
            ('[', 0x1b),
            ('3', 0x1b),
            ('\\', 0x1c),
            ('4', 0x1c),
            (']', 0x1d),
            ('5', 0x1d),
            ('^', 0x1e),
            ('6', 0x1e),
            ('_', 0x1f),
            ('7', 0x1f),
            ('?', 0x7f),
            ('8', 0x7f),
        ] {
            assert_eq!(control_byte(character), Some(expected));
        }
        for (index, character) in ('a'..='z').enumerate() {
            assert_eq!(control_byte(character), Some(index as u8 + 1));
        }
        assert_eq!(control_byte('界'), None);
    }

    #[test]
    fn real_bash_readline_accepts_normal_and_application_home_from_the_encoder() {
        use std::time::{Duration, Instant};

        let mut environment = std::collections::HashMap::new();
        environment.insert("TERM".into(), "xterm-256color".into());
        environment.insert("PS1".into(), "termeric-readline> ".into());
        let launch = crate::tabs::CommandLaunch {
            program: "/bin/bash".into(),
            args: vec!["--noprofile".into(), "--norc".into(), "-i".into()],
            title: Some("readline".into()),
            env: environment,
            ..crate::tabs::CommandLaunch::default()
        };
        let mut tab =
            crate::tabs::Tab::new_with_command(0, 80, 24, &launch, 1_000, 8 * 1024 * 1024).unwrap();
        let home = |mode| {
            encode_legacy_xterm(
                &Key::Named(NamedKey::Home),
                PhysicalKey::Code(KeyCode::Home),
                None,
                ModifiersState::empty(),
                false,
                mode,
                XtermKeyProfile::default(),
            )
            .unwrap()
        };

        let prompt_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < prompt_deadline
            && !tab
                .focused_terminal()
                .unwrap()
                .rows_text(0, 23)
                .contains("termeric-readline>")
        {
            tab.poll_events(|_, _| {});
            std::thread::sleep(Duration::from_millis(2));
        }

        tab.write(b"printf 'NORMAL_TAIL\\n'").unwrap();
        tab.write(&home(termeric_core::term::TerminalMode::default()))
            .unwrap();
        tab.write(b"printf 'NORMAL_HEAD\\n'; ").unwrap();
        tab.write(b"\r").unwrap();

        tab.write(b"printf 'APP_TAIL\\n'").unwrap();
        tab.write(&home(
            termeric_core::term::TerminalMode::CURSOR_KEYS_APPLICATION,
        ))
        .unwrap();
        tab.write(b"printf 'APP_HEAD\\n'; ").unwrap();
        tab.write(b"\r").unwrap();

        let output_deadline = Instant::now() + Duration::from_secs(3);
        let mut output = String::new();
        while Instant::now() < output_deadline {
            tab.poll_events(|_, _| {});
            output = tab.focused_terminal().unwrap().rows_text(0, 23);
            if ["NORMAL_HEAD", "NORMAL_TAIL", "APP_HEAD", "APP_TAIL"]
                .iter()
                .all(|marker| output.matches(marker).count() >= 2)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        for marker in ["NORMAL_HEAD", "NORMAL_TAIL", "APP_HEAD", "APP_TAIL"] {
            assert!(
                output.matches(marker).count() >= 2,
                "readline omitted executed output for {marker}: {output}"
            );
        }
    }
}
