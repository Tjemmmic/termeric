//! The macOS file chooser: an `NSOpenPanel` sheet on the window that asks.
//! AppKit panels live on the main thread, so the sheet is shown from the
//! event loop and answers through its completion handler instead of a
//! blocked thread.

use super::{FileChoice, FileRequest, Pick, filter_extensions};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSView};
use objc2_foundation::{NSArray, NSString, NSURL};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::path::PathBuf;

/// Show the chooser `request` describes as a sheet on `window`, and call
/// `answer` once with what the person did. An error means no sheet could be
/// shown, and `answer` is not called; the caller should ask another way.
pub(crate) fn present(
    window: &winit::window::Window,
    request: FileRequest,
    answer: impl FnOnce(Result<FileChoice, String>) + 'static,
) -> Result<(), String> {
    let main_thread =
        MainThreadMarker::new().ok_or("the file chooser can only be shown from the main thread")?;
    let handle = window
        .window_handle()
        .map_err(|error| format!("the window has no native handle: {error}"))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("the window is not an AppKit window".into());
    };
    // SAFETY: winit's AppKit handle points to the window's NSView, which
    // lives as long as the window, and the window outlives this call.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let native_window = view
        .window()
        .ok_or("the window's view is not in a window")?;

    let panel = NSOpenPanel::openPanel(main_thread);
    panel.setMessage(Some(&NSString::from_str(request.title)));
    panel.setResolvesAliases(true);
    match &request.pick {
        Pick::File(filter) => {
            // A sheet shows no title, but accessibility names the panel by it.
            panel.setTitle(Some(&NSString::from_str(filter.name)));
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            let extensions: Vec<Retained<NSString>> = filter_extensions(filter)
                .into_iter()
                .map(NSString::from_str)
                .collect();
            if !extensions.is_empty() {
                // allowedContentTypes would need UniformTypeIdentifiers;
                // extensions still work on every supported macOS.
                #[allow(deprecated)]
                panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&extensions)));
            }
        }
        Pick::Folder { start } => {
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            panel.setCanCreateDirectories(true);
            panel.setAllowsMultipleSelection(false);
            set_start(&panel, start.as_deref());
        }
        Pick::Files { start } => {
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(true);
            set_start(&panel, start.as_deref());
        }
    }

    // AppKit calls the handler once; the cell makes the one-time answer
    // callable from the Fn block AppKit takes.
    let answer = RefCell::new(Some(answer));
    let answering = panel.clone();
    let handler = block2::RcBlock::new(move |response: isize| {
        let Some(answer) = answer.borrow_mut().take() else {
            return;
        };
        answer(if response == NSModalResponseOK {
            chosen_paths(&answering)
        } else {
            Ok(FileChoice::Cancelled)
        });
    });
    panel.beginSheetModalForWindow_completionHandler(&native_window, &handler);
    Ok(())
}

fn set_start(panel: &NSOpenPanel, start: Option<&std::path::Path>) {
    if let Some(start) = start.and_then(|start| start.to_str()) {
        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(start), true);
        panel.setDirectoryURL(Some(&url));
    }
}

/// The local paths the panel's answer names.
fn chosen_paths(panel: &NSOpenPanel) -> Result<FileChoice, String> {
    let urls = panel.URLs();
    let paths: Vec<PathBuf> = urls
        .iter()
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect();
    if paths.is_empty() || paths.len() != urls.len() {
        return Err("the file chooser returned something that isn't a local file".into());
    }
    Ok(FileChoice::Chosen(paths))
}
