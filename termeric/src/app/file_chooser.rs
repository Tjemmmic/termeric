//! The system's own file chooser (`crate::file_chooser`), opened for one
//! window and answered back to it: asked off the event loop on Linux, shown
//! as a sheet from it on macOS.

use super::*;
use crate::file_chooser::{FileChoice, FileRequest};

/// What a window opened the file chooser for, so its answer reaches the
/// flow that asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileChooserPurpose {
    /// Settings → Appearance → Background Image.
    BackgroundImage,
    /// Settings → Plugins → Install a plugin….
    PluginFolder,
    /// The context picker's Choose Files…, for its request `request_id`.
    ContextFiles { request_id: u64 },
}

impl App {
    /// Show the chooser for `request` and answer it back to this window as
    /// `purpose`. While one is open, another request does nothing. Returns
    /// false when no chooser could be started, so the caller can ask
    /// another way.
    pub(super) fn open_file_chooser(
        &mut self,
        purpose: FileChooserPurpose,
        request: FileRequest,
    ) -> bool {
        if self.file_chooser_open {
            return true;
        }
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            return false;
        };
        let window = self.window_key();
        let answer = move |result| {
            let _ = proxy.send_event(AppEvent::WindowResult {
                window,
                result: WindowResult::FileChosen { purpose, result },
            });
        };
        match self.start_file_chooser(request, answer) {
            Ok(()) => {
                self.file_chooser_open = true;
                true
            }
            Err(error) => {
                log::warn!("Could not start the file chooser: {error}");
                false
            }
        }
    }

    /// The portal blocks until the person answers, so it is asked on a
    /// thread of its own.
    #[cfg(target_os = "linux")]
    fn start_file_chooser(
        &self,
        request: FileRequest,
        answer: impl FnOnce(std::result::Result<FileChoice, String>) + Send + 'static,
    ) -> std::result::Result<(), String> {
        std::thread::Builder::new()
            .name("termeric-file-chooser".into())
            .spawn(move || answer(crate::file_chooser::choose(&request)))
            .map(drop)
            .map_err(|error| error.to_string())
    }

    /// An `NSOpenPanel` sheet on this window, shown from the event loop.
    #[cfg(target_os = "macos")]
    fn start_file_chooser(
        &self,
        request: FileRequest,
        answer: impl FnOnce(std::result::Result<FileChoice, String>) + 'static,
    ) -> std::result::Result<(), String> {
        let window = self.window.as_ref().ok_or("the window is not open")?;
        crate::file_chooser::present(window, request, answer)
    }

    /// Hand the chooser's answer to the flow that opened it.
    pub(super) fn handle_file_chosen(
        &mut self,
        purpose: FileChooserPurpose,
        result: std::result::Result<FileChoice, String>,
    ) {
        self.file_chooser_open = false;
        match purpose {
            FileChooserPurpose::BackgroundImage => self.handle_background_image_chosen(result),
            FileChooserPurpose::PluginFolder => self.handle_plugin_folder_chosen(result),
            FileChooserPurpose::ContextFiles { request_id } => {
                self.handle_context_files_chosen(request_id, result);
            }
        }
    }
}
