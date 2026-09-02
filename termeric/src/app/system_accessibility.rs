//! Native desktop accessibility preference monitoring.
//!
//! Linux exposes reduced-motion and contrast preferences through the XDG
//! Desktop Portal Settings interface. All D-Bus work stays on one background
//! thread; the event-loop thread only receives small immutable snapshots.

use super::AppEvent;
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SystemAccessibilityPreferences {
    pub reduce_motion: Option<bool>,
    pub high_contrast: Option<bool>,
}

pub(super) fn start_monitor(proxy: EventLoopProxy<AppEvent>) {
    #[cfg(target_os = "linux")]
    if let Err(error) = std::thread::Builder::new()
        .name("termeric-accessibility".into())
        .spawn(move || monitor_linux(proxy))
    {
        log::warn!("Could not start the desktop accessibility monitor: {error}");
    }

    #[cfg(not(target_os = "linux"))]
    let _ = proxy;
}

#[cfg(target_os = "linux")]
fn monitor_linux(proxy: EventLoopProxy<AppEvent>) {
    if let Err(error) = monitor_linux_inner(&proxy) {
        let _ = proxy.send_event(AppEvent::SystemAccessibilityUnavailable(error));
    }
}

#[cfg(target_os = "linux")]
fn monitor_linux_inner(proxy: &EventLoopProxy<AppEvent>) -> Result<(), String> {
    use zbus::blocking::{Connection, Proxy};

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const NAMESPACE: &str = "org.freedesktop.appearance";

    let connection = Connection::session()
        .map_err(|error| format!("desktop portal session bus is unavailable: {error}"))?;
    let portal = Proxy::new(&connection, DESTINATION, PATH, INTERFACE)
        .map_err(|error| format!("desktop portal settings interface is unavailable: {error}"))?;

    // Subscribe before taking the initial snapshot so a change cannot be lost
    // between ReadOne and signal registration.
    let signals = portal
        .receive_signal_with_args("SettingChanged", &[(0, NAMESPACE)])
        .map_err(|error| format!("desktop portal settings signals are unavailable: {error}"))?;
    let mut preferences = SystemAccessibilityPreferences {
        reduce_motion: read_preference(&portal, NAMESPACE, "reduced-motion"),
        high_contrast: read_preference(&portal, NAMESPACE, "contrast"),
    };
    if proxy
        .send_event(AppEvent::SystemAccessibilityChanged(preferences))
        .is_err()
    {
        return Ok(());
    }

    for message in signals {
        let (namespace, key, value): (String, String, zbus::zvariant::OwnedValue) = message
            .body()
            .deserialize()
            .map_err(|error| format!("invalid desktop portal settings signal: {error}"))?;
        if namespace != NAMESPACE {
            continue;
        }
        let Ok(value) = u32::try_from(value) else {
            log::debug!("Ignoring non-integer desktop accessibility preference {key:?}");
            continue;
        };
        if !apply_portal_setting(&mut preferences, &key, value) {
            continue;
        }
        if proxy
            .send_event(AppEvent::SystemAccessibilityChanged(preferences))
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_preference(portal: &zbus::blocking::Proxy<'_>, namespace: &str, key: &str) -> Option<bool> {
    let value: zbus::zvariant::OwnedValue = match portal.call("ReadOne", &(namespace, key)) {
        Ok(value) => value,
        Err(error) => {
            log::debug!("Desktop accessibility preference {key:?} is unavailable: {error}");
            return None;
        }
    };
    match u32::try_from(value) {
        Ok(value) => Some(portal_preference(value)),
        Err(error) => {
            log::debug!("Desktop accessibility preference {key:?} is not an integer: {error}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
const fn portal_preference(value: u32) -> bool {
    value == 1
}

#[cfg(target_os = "linux")]
fn apply_portal_setting(
    preferences: &mut SystemAccessibilityPreferences,
    key: &str,
    value: u32,
) -> bool {
    let preference = Some(portal_preference(value));
    match key {
        "reduced-motion" => preferences.reduce_motion = preference,
        "contrast" => preferences.high_contrast = preference,
        _ => return false,
    }
    true
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{SystemAccessibilityPreferences, apply_portal_setting, portal_preference};

    #[test]
    fn portal_preferences_enable_only_the_standardized_one_value() {
        assert!(!portal_preference(0));
        assert!(portal_preference(1));
        assert!(!portal_preference(2));
        assert!(!portal_preference(u32::MAX));
    }

    #[test]
    fn portal_signal_updates_only_the_named_accessibility_preference() {
        let mut preferences = SystemAccessibilityPreferences {
            reduce_motion: Some(false),
            high_contrast: None,
        };
        assert!(apply_portal_setting(&mut preferences, "contrast", 1));
        assert_eq!(preferences.reduce_motion, Some(false));
        assert_eq!(preferences.high_contrast, Some(true));

        assert!(!apply_portal_setting(&mut preferences, "accent-color", 1));
        assert_eq!(preferences.high_contrast, Some(true));
        assert!(apply_portal_setting(&mut preferences, "reduced-motion", 0,));
        assert_eq!(preferences.reduce_motion, Some(false));
    }
}
