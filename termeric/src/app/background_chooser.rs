//! Choosing the background image from Settings: the desktop's own file
//! chooser, or a typed path where the desktop offers none.

use super::file_chooser::FileChooserPurpose;
use super::*;
use crate::portal::{FileChoice, FileFilter};
use crate::ui::dialog::{DialogBlock, DialogSpec, FieldSpec};

/// What the chooser offers: the formats the background image decodes.
const IMAGES: FileFilter = FileFilter {
    name: "PNG and JPEG images",
    patterns: &["*.png", "*.PNG", "*.jpg", "*.JPG", "*.jpeg", "*.JPEG"],
};
/// The path prompt's answer.
const USE_IMAGE: &str = "use-background-image";
/// The longest path the prompt takes, in bytes.
const MAX_PATH_BYTES: usize = 4096;

impl App {
    /// Open the desktop's file chooser for a background image, else ask for
    /// its path.
    pub(super) fn choose_background_image(&mut self) {
        let opened = self.open_file_chooser(FileChooserPurpose::BackgroundImage, || {
            crate::portal::choose_file("Choose a background image", &IMAGES)
        });
        if !opened {
            self.ask_background_image_path(String::new(), None);
        }
    }

    /// Use the chosen image, or ask for a path when no chooser could open.
    pub(super) fn handle_background_image_chosen(
        &mut self,
        result: std::result::Result<FileChoice, String>,
    ) {
        match result {
            Ok(FileChoice::Chosen(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    self.apply_settings_action(SettingsAction::SetBackgroundImagePath(Some(path)));
                }
            }
            Ok(FileChoice::Cancelled) => {}
            Err(error) => {
                log::info!("No file chooser ({error}); asking for a path instead");
                let current = self
                    .config
                    .window
                    .background_image
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                self.ask_background_image_path(current, None);
            }
        }
    }

    fn ask_background_image_path(&mut self, value: String, error: Option<&str>) {
        let spec = DialogSpec::prompt(
            "Choose a background image",
            vec![DialogBlock::text(
                "Type the full path of a PNG or JPEG image, or one starting with ~/.",
            )],
            FieldSpec::new("Path", value, MAX_PATH_BYTES)
                .with_error(error.map(crate::safe_text::capitalized)),
            USE_IMAGE,
            "Use image",
        );
        let request_id = self.dialog.show(spec);
        self.dialog_flow = Some(DialogFlow::BackgroundImagePath { request_id });
    }

    /// Answer the path prompt. Returns whether the answer was its.
    pub(super) fn handle_background_image_path_choice(
        &mut self,
        request_id: u64,
        choice_id: &str,
    ) -> bool {
        let pending = matches!(
            self.dialog_flow,
            Some(DialogFlow::BackgroundImagePath { request_id: pending }) if pending == request_id
        );
        if !pending || choice_id != USE_IMAGE {
            return false;
        }
        let Some(value) = self.dialog.field_value(request_id).map(ToOwned::to_owned) else {
            return false;
        };
        self.dialog.dismiss(request_id);
        self.dialog_flow = None;
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        match background_image_path(&value, home.as_deref()) {
            Ok(path) => {
                self.apply_settings_action(SettingsAction::SetBackgroundImagePath(Some(path)));
            }
            Err(error) => self.ask_background_image_path(value, Some(&error)),
        }
        true
    }
}

/// The image file `typed` names: a full path, or one under `home` written
/// with `~/`.
fn background_image_path(
    typed: &str,
    home: Option<&std::path::Path>,
) -> std::result::Result<PathBuf, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("type the path of a PNG or JPEG image".into());
    }
    let path = match typed.strip_prefix("~/") {
        Some(rest) => home
            .ok_or("there is no home folder to start ~/ from")?
            .join(rest),
        None => PathBuf::from(typed),
    };
    if !path.is_absolute() {
        return Err("use a full path, or one starting with ~/".into());
    }
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_path_must_be_a_full_or_home_path_to_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("sky.png");
        std::fs::write(&image, b"").unwrap();
        let home = Some(directory.path());

        assert_eq!(
            background_image_path(&image.display().to_string(), None),
            Ok(image.clone())
        );
        assert_eq!(background_image_path("  ~/sky.png ", home), Ok(image));
        for (typed, error) in [
            ("", "type the path"),
            ("sky.png", "use a full path"),
            ("~/missing.png", "is not a file"),
        ] {
            let refused = background_image_path(typed, home).unwrap_err();
            assert!(refused.contains(error), "{typed:?}: {refused}");
        }
        let folder = background_image_path(&directory.path().display().to_string(), home);
        assert!(folder.unwrap_err().contains("is not a file"));
    }
}
