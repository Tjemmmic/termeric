//! What a Termeric on macOS sets up with AppKit itself, beyond what winit
//! does: the menu bar (`menu.rs`), the defaults a terminal needs, and each
//! window's colour space.

mod menu;

use super::AppEvent;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSUserDefaults};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::event_loop::{ActiveEventLoop, EventLoopBuilder};

/// Before the event loop is built: Termeric installs its own menu bar.
pub(crate) fn configure_event_loop(builder: &mut EventLoopBuilder<AppEvent>) {
    use winit::platform::macos::EventLoopBuilderExtMacOS;
    builder.with_default_menu(false);
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
/// else (wgpu's usual arrangement) a sublayer of it.
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
        .find_map(|layer| layer.downcast::<CAMetalLayer>().ok())
}
