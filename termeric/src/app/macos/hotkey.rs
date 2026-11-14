//! The quick-access terminal's system-wide hotkey, through Carbon's
//! `RegisterEventHotKey`: the one system-wide hotkey API that needs no
//! accessibility permission. Its events arrive on the main run loop, which
//! a Cocoa app runs, and are handed to the event loop as
//! `AppEvent::QuickAccessHotkey`.

use crate::config::quick_access::{Hotkey, hotkey_key_code};
use std::ffi::c_void;

type OsStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerProc =
    extern "C" fn(next: *mut c_void, event: *mut c_void, user_data: *mut c_void) -> OsStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        type_count: u32,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        installed: *mut EventHandlerRef,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        registered: *mut EventHotKeyRef,
    ) -> OsStatus;
}

/// `kEventClassKeyboard` and `kEventHotKeyPressed`.
const KEYBOARD_EVENTS: u32 = u32::from_be_bytes(*b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
/// Carbon's modifier bits: `cmdKey`, `shiftKey`, `optionKey`, `controlKey`.
const COMMAND: u32 = 1 << 8;
const SHIFT: u32 = 1 << 9;
const OPTION: u32 = 1 << 11;
const CONTROL: u32 = 1 << 12;
/// The hotkey's signature, "TMRC".
const SIGNATURE: u32 = u32::from_be_bytes(*b"TMRC");

extern "C" fn hotkey_pressed(
    _next: *mut c_void,
    _event: *mut c_void,
    _user_data: *mut c_void,
) -> OsStatus {
    super::super::post_quick_access_hotkey();
    0
}

/// Carbon's modifier bits for `hotkey`.
fn modifiers(hotkey: &Hotkey) -> u32 {
    [
        (hotkey.super_key, COMMAND),
        (hotkey.shift, SHIFT),
        (hotkey.alt, OPTION),
        (hotkey.ctrl, CONTROL),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(0, |bits, (_, bit)| bits | bit)
}

/// Listen for `hotkey` for as long as the process runs. Fails when another
/// app holds the same hotkey or the key is unknown.
pub(super) fn register(hotkey: &Hotkey) -> Result<(), String> {
    let key_code = hotkey_key_code(&hotkey.key)
        .ok_or_else(|| format!("unknown hotkey key {:?}", hotkey.key))?;
    let pressed = EventTypeSpec {
        event_class: KEYBOARD_EVENTS,
        event_kind: HOT_KEY_PRESSED,
    };
    let mut handler: EventHandlerRef = std::ptr::null_mut();
    // SAFETY: the event-type list holds one spec for the call; the handler is
    // a function with Carbon's handler signature that lives for the process;
    // `handler` is writable. The handler is never removed: it lasts as long
    // as the process.
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            hotkey_pressed,
            1,
            &pressed,
            std::ptr::null_mut(),
            &mut handler,
        )
    };
    if status != 0 {
        return Err(format!("could not listen for hotkeys (status {status})"));
    }
    let mut registered: EventHotKeyRef = std::ptr::null_mut();
    // SAFETY: `registered` is writable; the hotkey stays registered for the
    // process's life.
    let status = unsafe {
        RegisterEventHotKey(
            u32::from(key_code),
            modifiers(hotkey),
            EventHotKeyId {
                signature: SIGNATURE,
                id: 1,
            },
            GetApplicationEventTarget(),
            0,
            &mut registered,
        )
    };
    if status != 0 {
        return Err(format!(
            "the hotkey is taken by another app or the system (status {status})"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_become_carbon_bits() {
        let hotkey = Hotkey {
            key: "`".into(),
            ctrl: true,
            alt: false,
            shift: true,
            super_key: false,
        };
        assert_eq!(modifiers(&hotkey), CONTROL | SHIFT);
    }
}
