//! Terminal-local state for Kitty and xterm keyboard protocols.

use bitflags::bitflags;
use std::collections::VecDeque;

/// Maximum saved keyboard-mode states per screen buffer.
///
/// Kitty requires a bounded stack and FIFO retirement when it fills. Sixteen
/// entries is deep enough for nested TUIs while keeping hostile input bounded.
pub(crate) const KITTY_KEYBOARD_STACK_DEPTH: usize = 16;

/// Xterm stores DEC user-defined keys by DEC function-key code 0 through 34.
pub(crate) const XTERM_USER_DEFINED_KEY_COUNT: usize = 35;
/// A single definition is deliberately much larger than the VT220 hardware
/// budget while preventing a control sequence from becoming permanent
/// unbounded terminal state.
pub(crate) const MAX_USER_DEFINED_KEY_BYTES: usize = 4 * 1024;
/// Aggregate decoded UDK storage per terminal owner.
pub(crate) const MAX_USER_DEFINED_KEYS_BYTES: usize = 64 * 1024;

/// Terminal-local DECUDK definitions used by xterm's VT220 keyboard profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XtermUserDefinedKeys {
    definitions: [Option<Box<[u8]>>; XTERM_USER_DEFINED_KEY_COUNT],
    bytes: usize,
}

impl Default for XtermUserDefinedKeys {
    fn default() -> Self {
        Self {
            definitions: std::array::from_fn(|_| None),
            bytes: 0,
        }
    }
}

impl XtermUserDefinedKeys {
    pub fn get(&self, code: u8) -> Option<&[u8]> {
        self.definitions
            .get(usize::from(code))
            .and_then(Option::as_deref)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u8, &[u8])> {
        self.definitions
            .iter()
            .enumerate()
            .filter_map(|(code, definition)| {
                Some((u8::try_from(code).ok()?, definition.as_deref()?))
            })
    }

    #[cfg(test)]
    pub(crate) const fn bytes(&self) -> usize {
        self.bytes
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Replace one definition if both the individual and aggregate decoded
    /// memory limits remain satisfied. Invalid/empty definitions are ignored,
    /// matching xterm's DECUDK parser.
    pub(super) fn define(&mut self, code: u16, definition: Vec<u8>) {
        let code = usize::from(code);
        if definition.is_empty()
            || definition.len() > MAX_USER_DEFINED_KEY_BYTES
            || code >= XTERM_USER_DEFINED_KEY_COUNT
        {
            return;
        }
        let previous = self.definitions[code]
            .as_ref()
            .map_or(0, |value| value.len());
        let next_bytes = self
            .bytes
            .saturating_sub(previous)
            .saturating_add(definition.len());
        if next_bytes > MAX_USER_DEFINED_KEYS_BYTES {
            return;
        }
        self.bytes = next_bytes;
        self.definitions[code] = Some(definition.into_boxed_slice());
    }
}

/// Xterm-compatible keyboard type selected by DEC private modes 1060/1061.
///
/// This is separate from application cursor/keypad modes and from negotiated
/// Kitty input. It controls historical function/editing-key mappings and
/// whether resource-zero's modifier permission bits are consulted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XtermKeyboardType {
    /// Contemporary xterm-compatible mappings. Resource zero is inert.
    #[default]
    Normal,
    /// Xterm's `oldXtermFKeys` profile selected by DECSET 1060.
    Legacy,
    /// Xterm's Sun/PC-to-VT220 keyboard profile selected by DECSET 1061.
    Vt220,
}

impl XtermKeyboardType {
    pub const fn uses_resource_zero(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

/// Opaque resource-zero state used by xterm's `modifyKeyboard` and
/// `formatKeyboard` controls.
///
/// `modifyKeyboard` is not an encoding mode like resources 1 through 7. Its
/// low four bits selectively allow modifier parameters for numeric keypad,
/// editing keypad, function, and other-special keys when xterm is configured
/// as a legacy or VT220 keyboard. It remains intentionally encoder-neutral in
/// the normal keyboard profile. Xterm also
/// stores and reports `formatKeyboard` without consulting it while encoding.
/// Keeping both complete 16-bit wire values makes queries restorable and
/// preserves xterm's `-1` disable representation as 65535 without assigning it
/// invented behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XtermKeyboardResource {
    modify: u16,
    format: u16,
}

impl XtermKeyboardResource {
    pub const fn new(modify: u16, format: u16) -> Self {
        Self { modify, format }
    }

    /// Current `modifyKeyboard` bit mask/wire value.
    pub const fn modify(self) -> u16 {
        self.modify
    }

    /// Current opaque `formatKeyboard` wire value.
    pub const fn format(self) -> u16 {
        self.format
    }

    pub const fn with_modify(mut self, modify: u16) -> Self {
        self.modify = modify;
        self
    }

    pub const fn with_format(mut self, format: u16) -> Self {
        self.format = format;
        self
    }
}

/// Output format selected by xterm's `formatCursorKeys`,
/// `formatFunctionKeys`, `formatKeypadKeys`, `formatModifierKeys`, and
/// `formatSpecialKeys`, and `formatOtherKeys` resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum XtermKeyFormat {
    /// Function-key-27 form: `CSI 27 ; modifier ; codepoint ~`.
    #[default]
    FunctionKey27 = 0,
    /// CSI-u form: `CSI codepoint ; modifier u`.
    CsiU = 1,
}

impl XtermKeyFormat {
    /// Convert the bounded XTFMTKEYS resource value into a format.
    pub const fn from_parameter(parameter: u16) -> Option<Self> {
        match parameter {
            0 => Some(Self::FunctionKey27),
            1 => Some(Self::CsiU),
            _ => None,
        }
    }

    /// Numeric value reported by XTQFMTKEYS.
    pub const fn parameter(self) -> u8 {
        self as u8
    }
}

/// Xterm's shared parameterized-key behavior for cursor, function, keypad,
/// and other special-key resources.
///
/// Xterm represents the disabled state internally as `-1` and reports that
/// through its unsigned CSI parameter path as 65535. Keeping it as an enum
/// makes the terminal state bounded while preserving that wire behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XtermKeyMode {
    Disabled,
    Original,
    Control,
    Parameter,
    Private,
    Extended,
}

impl XtermKeyMode {
    pub const fn from_parameter(parameter: u16) -> Option<Self> {
        match parameter {
            u16::MAX => Some(Self::Disabled),
            0 => Some(Self::Original),
            1 => Some(Self::Control),
            2 => Some(Self::Parameter),
            3 => Some(Self::Private),
            4 => Some(Self::Extended),
            _ => None,
        }
    }

    pub const fn parameter(self) -> u16 {
        match self {
            Self::Disabled => u16::MAX,
            Self::Original => 0,
            Self::Control => 1,
            Self::Parameter => 2,
            Self::Private => 3,
            Self::Extended => 4,
        }
    }
}

/// Xterm's key resources that share one mode-and-format shape under
/// XTMODKEYS/XTFMTKEYS and their queries. Resources 0 (`modifyKeyboard`) and
/// 4 (`modifyOtherKeys`) keep their own state and are not listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XtermKeyResource {
    /// `modifyCursorKeys`/`formatCursorKeys`, resource 1.
    Cursor,
    /// `modifyFunctionKeys`/`formatFunctionKeys`, resource 2.
    Function,
    /// `modifyKeypadKeys`/`formatKeypadKeys`, resource 3. Modes -1 and 0
    /// through 3 keep the conventional keypad path; mode 4 reports the keypad
    /// keysym in the selected extended format.
    Keypad,
    /// Xterm's reserved resource 5: negotiable state with no defined encoder
    /// behavior.
    String,
    /// `modifyModifierKeys`/`formatModifierKeys`, resource 6. Modifier keys
    /// stay silent in modes -1 and 0 through 3; mode 4 reports the modifier
    /// keysym in the selected extended format.
    Modifier,
    /// `modifySpecialKeys`/`formatSpecialKeys`, resource 7. Modes -1 and 0
    /// through 3 keep each key's conventional path; mode 4 reports the
    /// special keysym in the selected extended format.
    Special,
}

impl XtermKeyResource {
    pub const ALL: [Self; 6] = [
        Self::Cursor,
        Self::Function,
        Self::Keypad,
        Self::String,
        Self::Modifier,
        Self::Special,
    ];

    /// The resource number used on the wire.
    pub const fn number(self) -> u16 {
        match self {
            Self::Cursor => 1,
            Self::Function => 2,
            Self::Keypad => 3,
            Self::String => 5,
            Self::Modifier => 6,
            Self::Special => 7,
        }
    }

    pub(crate) const fn from_number(number: u16) -> Option<Self> {
        match number {
            1 => Some(Self::Cursor),
            2 => Some(Self::Function),
            3 => Some(Self::Keypad),
            5 => Some(Self::String),
            6 => Some(Self::Modifier),
            7 => Some(Self::Special),
            _ => None,
        }
    }

    /// Xterm starts cursor and function keys in parameter mode (2) and every
    /// other resource in the original mode (0).
    pub const fn default_setting(self) -> XtermKeySetting {
        let mode = match self {
            Self::Cursor | Self::Function => XtermKeyMode::Parameter,
            Self::Keypad | Self::String | Self::Modifier | Self::Special => XtermKeyMode::Original,
        };
        XtermKeySetting::new(mode, XtermKeyFormat::FunctionKey27)
    }
}

/// One key resource's terminal-local mode and output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XtermKeySetting {
    mode: XtermKeyMode,
    format: XtermKeyFormat,
}

impl XtermKeySetting {
    pub const fn new(mode: XtermKeyMode, format: XtermKeyFormat) -> Self {
        Self { mode, format }
    }

    pub const fn mode(self) -> XtermKeyMode {
        self.mode
    }

    pub const fn format(self) -> XtermKeyFormat {
        self.format
    }

    pub const fn with_mode(mut self, mode: XtermKeyMode) -> Self {
        self.mode = mode;
        self
    }

    pub const fn with_format(mut self, format: XtermKeyFormat) -> Self {
        self.format = format;
        self
    }
}

// `XtermKeyResources` indexes its array by discriminant and fills it in
// `ALL` order, so the two orders must agree.
const _: () = {
    let mut index = 0;
    while index < XtermKeyResource::ALL.len() {
        assert!(XtermKeyResource::ALL[index] as usize == index);
        index += 1;
    }
};

/// Every resource in [`XtermKeyResource`], each starting at its xterm
/// default. The state applies across both screen buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XtermKeyResources([XtermKeySetting; XtermKeyResource::ALL.len()]);

impl Default for XtermKeyResources {
    fn default() -> Self {
        Self(XtermKeyResource::ALL.map(XtermKeyResource::default_setting))
    }
}

impl XtermKeyResources {
    pub fn get(&self, resource: XtermKeyResource) -> XtermKeySetting {
        self.0[resource as usize]
    }

    /// A copy with one resource's setting replaced.
    pub const fn with(mut self, resource: XtermKeyResource, setting: XtermKeySetting) -> Self {
        self.0[resource as usize] = setting;
        self
    }

    /// Out-of-range parameters leave the mode unchanged.
    pub(super) fn set_mode(&mut self, resource: XtermKeyResource, parameter: u16) {
        if let Some(mode) = XtermKeyMode::from_parameter(parameter) {
            self.0[resource as usize].mode = mode;
        }
    }

    /// Out-of-range parameters leave the format unchanged.
    pub(super) fn set_format(&mut self, resource: XtermKeyResource, parameter: u16) {
        if let Some(format) = XtermKeyFormat::from_parameter(parameter) {
            self.0[resource as usize].format = format;
        }
    }

    pub(super) fn reset_mode(&mut self, resource: XtermKeyResource) {
        self.0[resource as usize].mode = resource.default_setting().mode;
    }

    pub(super) fn reset_format(&mut self, resource: XtermKeyResource) {
        self.0[resource as usize].format = resource.default_setting().format;
    }
}

/// Xterm's terminal-local `modifyOtherKeys` input policy.
///
/// Levels zero through three are the values accepted by `XTMODKEYS` for
/// resource 4. The modifier mask is the optional xterm subparameter whose
/// single-bit matches retain their legacy encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XtermModifyOtherKeys {
    level: u8,
    modifier_mask: u8,
    format: XtermKeyFormat,
}

impl XtermModifyOtherKeys {
    /// Construct a validated policy. Values outside xterm's documented
    /// level and four-bit modifier-mask ranges are rejected.
    pub const fn new(level: u8, modifier_mask: u8) -> Option<Self> {
        if level <= 3 && modifier_mask <= 0x0f {
            Some(Self {
                level,
                modifier_mask,
                format: XtermKeyFormat::FunctionKey27,
            })
        } else {
            None
        }
    }

    /// Disabled (0), user (1), program (2), or all-key (3) behavior.
    pub const fn level(self) -> u8 {
        self.level
    }

    /// Modifier bits factored out when they are the only active modifier.
    /// Bits are Shift=1, Alt=2, Control=4, and Super/Meta=8.
    pub const fn modifier_mask(self) -> u8 {
        self.modifier_mask
    }

    /// Active `formatOtherKeys` encoding selected through XTFMTKEYS.
    pub const fn format(self) -> XtermKeyFormat {
        self.format
    }

    /// Return this policy with a specific validated output format.
    pub const fn with_format(mut self, format: XtermKeyFormat) -> Self {
        self.format = format;
        self
    }

    pub(super) fn set(&mut self, level: u16, modifier_mask: u16) {
        let Ok(level) = u8::try_from(level) else {
            return;
        };
        let Ok(modifier_mask) = u8::try_from(modifier_mask) else {
            return;
        };
        if level <= 3 && modifier_mask <= 0x0f {
            self.level = level;
            self.modifier_mask = modifier_mask;
        }
    }

    pub(super) fn set_format(&mut self, parameter: u16) {
        if let Some(format) = XtermKeyFormat::from_parameter(parameter) {
            self.format = format;
        }
    }
}

bitflags! {
    /// Progressive enhancements negotiated by the child application.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct KittyKeyboardFlags: u8 {
        const DISAMBIGUATE_ESCAPE_CODES = 1;
        const REPORT_EVENT_TYPES = 1 << 1;
        const REPORT_ALTERNATE_KEYS = 1 << 2;
        const REPORT_ALL_KEYS = 1 << 3;
        const REPORT_ASSOCIATED_TEXT = 1 << 4;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct KittyKeyboardState {
    flags: KittyKeyboardFlags,
    stack: VecDeque<KittyKeyboardFlags>,
}

impl KittyKeyboardState {
    pub(super) fn flags(&self) -> KittyKeyboardFlags {
        self.flags
    }

    pub(super) fn set(&mut self, requested: u16, mode: u16) {
        let requested = KittyKeyboardFlags::from_bits_truncate(requested as u8);
        match mode {
            1 => self.flags = requested,
            2 => self.flags.insert(requested),
            3 => self.flags.remove(requested),
            _ => {}
        }
    }

    pub(super) fn push(&mut self, requested: u16) {
        if self.stack.len() == KITTY_KEYBOARD_STACK_DEPTH {
            self.stack.pop_front();
        }
        self.stack.push_back(self.flags);
        self.flags = KittyKeyboardFlags::from_bits_truncate(requested as u8);
    }

    pub(super) fn pop(&mut self, count: usize) {
        for _ in 0..count {
            let Some(flags) = self.stack.pop_back() else {
                self.flags = KittyKeyboardFlags::empty();
                return;
            };
            self.flags = flags;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_add_and_remove_ignore_unknown_flags_and_invalid_modes() {
        let mut state = KittyKeyboardState::default();
        state.set(0xff, 1);
        assert_eq!(state.flags().bits(), 31);
        state.set(2, 3);
        assert_eq!(state.flags().bits(), 29);
        state.set(2, 2);
        assert_eq!(state.flags().bits(), 31);
        state.set(1, 99);
        assert_eq!(state.flags().bits(), 31);
    }

    #[test]
    fn overflow_retires_oldest_entry_and_over_pop_resets_flags() {
        let mut state = KittyKeyboardState::default();
        state.set(31, 1);
        for requested in 0..=KITTY_KEYBOARD_STACK_DEPTH {
            state.push((requested % 32) as u16);
        }
        assert_eq!(state.stack.len(), KITTY_KEYBOARD_STACK_DEPTH);

        state.pop(KITTY_KEYBOARD_STACK_DEPTH);
        // The initial 31 was the oldest entry and cannot be restored after overflow.
        assert!(state.flags().is_empty());
        state.pop(1);
        assert!(state.flags().is_empty());
    }
}
