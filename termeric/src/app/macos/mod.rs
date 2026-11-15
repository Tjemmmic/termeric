//! What a Termeric on macOS sets up with AppKit itself, beyond what winit
//! does: the menu bar (`menu.rs`), the defaults a terminal needs, each
//! window's colour space, and the quick-access terminal's place on screen
//! and hotkey (`hotkey.rs`).

mod hotkey;
mod menu;

use super::AppEvent;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSScreen, NSView, NSWindowCollectionBehavior};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event_loop::{ActiveEventLoop, EventLoopBuilder};

/// Before the event loop is built: Termeric installs its own menu bar. A
/// quick-access process is an accessory app: no Dock icon of its own beside
/// Termeric's, and its window may float over another app's full-screen
/// space.
pub(crate) fn configure_event_loop(builder: &mut EventLoopBuilder<AppEvent>, quick_access: bool) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    builder.with_default_menu(false);
    if quick_access {
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
}

/// Listen for the quick-access hotkey for as long as the process runs.
pub(super) fn register_quick_access_hotkey(hotkey: &crate::config::quick_access::Hotkey) {
    match hotkey::register(hotkey) {
        Ok(()) => log::info!("Listening for the quick-access hotkey"),
        Err(error) => log::warn!("The quick-access hotkey is unavailable: {error}"),
    }
}

/// A shown quick-access window is on every Space, including over a
/// full-screen app, wherever the person is.
pub(super) fn prepare_quick_access_window(window: &winit::window::Window) {
    let Some(native) = native_view(window).and_then(|view| view.window()) else {
        log::warn!("No AppKit window to place the quick-access terminal with");
        return;
    };
    native.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
}

/// How far below the top of the primary display windows may start, in
/// physical pixels: the menu bar, and the notch's height where there is one.
pub(super) fn primary_display_top_inset() -> u32 {
    let Some(main_thread) = MainThreadMarker::new() else {
        return 0;
    };
    let screens = NSScreen::screens(main_thread);
    let Some(primary) = screens.firstObject() else {
        return 0;
    };
    let (frame, visible) = (primary.frame(), primary.visibleFrame());
    let inset_points =
        (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);
    (inset_points.max(0.0) * primary.backingScaleFactor()).round() as u32
}

/// Once the app has finished launching (the first `resumed`), before any
/// window opens.
pub(super) fn finish_launching(event_loop: &ActiveEventLoop) {
    use winit::platform::macos::ActiveEventLoopExtMacOS;
    // Termeric has its own tabs; macOS would otherwise merge new windows
    // into native tabs when the person prefers tabs.
    event_loop.set_allows_automatic_window_tabbing(false);
    let Some(main_thread) = MainThreadMarker::new() else {
        log::warn!("The app finished launching off the main thread; no menu bar");
        return;
    };
    register_defaults();
    menu::install(main_thread);
}

/// Registered, not saved: a person's own `defaults write` still wins.
/// Holding a key repeats it, as a terminal needs, instead of opening the
/// accent picker; and AppKit adds no full-screen item of its own (with a key
/// equivalent that would bypass the input chain) to the View menu.
fn register_defaults() {
    let keys = [
        NSString::from_str("ApplePressAndHoldEnabled"),
        NSString::from_str("NSFullScreenMenuItemEverywhere"),
    ];
    let no = NSNumber::numberWithBool(false);
    let key_refs: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let value_refs: Vec<&AnyObject> = keys.iter().map(|_| &**no as &AnyObject).collect();
    let defaults = NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &value_refs);
    // SAFETY: the dictionary's keys are strings and its values property-list
    // objects (numbers), which is what registerDefaults: takes.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

/// After `window`'s renderer created its surface: give the Metal layer wgpu
/// draws into the sRGB colour space. Without one, Core Animation shows its
/// pixels as the display's own colours, which on a wide-gamut display makes
/// every theme colour more saturated than in other apps.
pub(super) fn prepare_window_surface(window: &winit::window::Window) {
    let Some(view) = native_view(window) else {
        log::warn!("No AppKit view to set the colour space on");
        return;
    };
    let Some(layer) = metal_layer(&view) else {
        log::warn!("No Metal layer found to set the colour space on");
        return;
    };
    // SAFETY: kCGColorSpaceSRGB is a constant CoreGraphics defines.
    let name = unsafe { objc2_core_graphics::kCGColorSpaceSRGB };
    match objc2_core_graphics::CGColorSpace::with_name(Some(name)) {
        Some(srgb) => layer.setColorspace(Some(&srgb)),
        None => log::warn!("CoreGraphics has no sRGB colour space"),
    }
}

/// The NSView winit made for `window`.
fn native_view(window: &winit::window::Window) -> Option<Retained<NSView>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: winit's AppKit handle points to the window's live NSView, which
    // is retained here for as long as it is used.
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
}

/// The CAMetalLayer wgpu draws into: the view's own layer when it is one,
/// else (wgpu's usual arrangement) a sublayer of it, the last one: a renderer
/// recreated after a lost device adds a new sublayer beside the old one.
fn metal_layer(view: &NSView) -> Option<Retained<CAMetalLayer>> {
    let root: Retained<CALayer> = view.layer()?;
    if let Ok(layer) = root.clone().downcast::<CAMetalLayer>() {
        return Some(layer);
    }
    // SAFETY: reading a layer's sublayers on the main thread, where the
    // window's layers are changed.
    let sublayers = unsafe { root.sublayers() }?;
    sublayers
        .iter()
        .filter_map(|layer| layer.downcast::<CAMetalLayer>().ok())
        .last()
}
