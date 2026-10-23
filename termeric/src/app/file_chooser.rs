//! The desktop's own file chooser, opened off the event loop for one window
//! and answered back to it.

use super::*;
use crate::portal::FileChoice;

/// What a window opened the file chooser for, so its answer reaches the
/// flow that asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileChooserPurpose {
    /// Settings → Appearance → Background Image.
    BackgroundImage,
    /// Settings → Plugins → Install a plugin….
    PluginFolder,
}

impl App {
    /// Show the chooser `ask` opens, off the event loop, and answer it back
    /// to this window as `purpose`. While one is open, another request does
    /// nothing. Returns false when no chooser could be started, so the
    /// caller can ask another way.
    pub(super) fn open_file_chooser(
        &mut self,
        purpose: FileChooserPurpose,
        ask: impl FnOnce() -> std::result::Result<FileChoice, String> + Send + 'static,
    ) -> bool {
        if self.file_chooser_open {
            return true;
        }
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            return false;
        };
        let window_key = self.window_key();
        let spawn = std::thread::Builder::new()
            .name("termeric-file-chooser".into())
            .spawn(move || {
                let result = ask();
                let _ = proxy.send_event(AppEvent::WindowResult {
                    window: window_key,
                    result: WindowResult::FileChosen { purpose, result },
                });
            });
        match spawn {
            Ok(_) => {
                self.file_chooser_open = true;
                true
            }
            Err(error) => {
                log::warn!("Could not start the file chooser: {error}");
                false
            }
        }
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
        }
    }
}
