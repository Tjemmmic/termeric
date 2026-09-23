//! "Change Theme…": a palette quick pick that previews each theme as it is
//! highlighted, keeps the chosen one, and restores the original on cancel.

use std::path::PathBuf;

use super::App;
use crate::config::theme::Theme;
use crate::ui::command_palette::palette::{
    CommandSource, PickKind, RegisteredCommand, theme_display_name,
};

/// The theme in effect before the picker opened and the one on screen now.
pub(super) struct ThemePreview {
    original_name: String,
    original_path: Option<PathBuf>,
    shown: String,
}

/// Picker rows: every built-in and custom theme, grouped by origin. The
/// current theme is labelled so it can be found without scrolling.
pub(super) fn theme_pick_items(current: &str) -> Vec<RegisteredCommand> {
    let builtin = Theme::list_available().iter().map(|name| {
        let root = Theme::load_builtin(name).is_ok_and(|theme| theme.meta.author == "Termeric");
        (name.to_string(), if root { "Termeric" } else { "Guest" })
    });
    let custom = Theme::list_custom()
        .into_iter()
        .map(|name| (name, "Custom"));
    builtin
        .chain(custom)
        .map(|(name, origin)| RegisteredCommand {
            title: theme_display_name(&name),
            category: if name == current {
                format!("Current · {origin}")
            } else {
                origin.into()
            },
            id: name,
            shortcut: None,
            icon: None,
            source: CommandSource::Core,
            destructive: false,
        })
        .collect()
}

impl App {
    pub(super) fn open_theme_picker(&mut self) {
        let current = self.config.theme.name.clone();
        self.command_palette.begin_pick(
            PickKind::Theme,
            theme_pick_items(&current),
            "Search themes…",
            "theme",
            Some(&current),
        );
        self.theme_preview = Some(ThemePreview {
            original_name: current.clone(),
            original_path: self.config.theme.path.clone(),
            shown: current,
        });
    }

    /// Follow the picker once per frame: preview the highlighted theme, or
    /// restore the original when the picker closed without a choice.
    pub(super) fn sync_theme_preview(&mut self) {
        let Some(preview) = &self.theme_preview else {
            return;
        };
        if self.command_palette.active_pick() == Some(PickKind::Theme) {
            let Some(highlighted) = self.command_palette.highlighted_id() else {
                return;
            };
            if highlighted == preview.shown {
                return;
            }
            let highlighted = highlighted.to_owned();
            if let Ok((theme, source)) = Theme::resolve(&highlighted, None) {
                self.apply_theme(theme, source);
            }
            if let Some(preview) = &mut self.theme_preview {
                preview.shown = highlighted;
            }
            return;
        }
        let Some(preview) = self.theme_preview.take() else {
            return;
        };
        if preview.shown != preview.original_name
            && let Ok((theme, source)) =
                Theme::resolve(&preview.original_name, preview.original_path.as_deref())
        {
            self.apply_theme(theme, source);
        }
    }

    /// Keep the picked theme and save it to the configuration.
    pub(super) fn commit_theme(&mut self, name: &str) {
        if let Err(error) = self.adopt_theme(name) {
            self.toasts.error(format!("Theme failed: {error}"));
            return;
        }
        match self.save_config() {
            Ok(message) => {
                log::info!("Theme changed to {name}: {message}");
                self.toasts
                    .info(format!("Theme: {}", theme_display_name(name)));
            }
            Err(error) => {
                log::error!("Theme applied but save failed: {error}");
                self.toasts
                    .warning(format!("Theme applied but not saved: {error}"));
            }
        }
    }

    /// Make `name` the configured theme in memory and end any preview.
    fn adopt_theme(&mut self, name: &str) -> anyhow::Result<()> {
        self.theme_preview = None;
        let (theme, source) = Theme::resolve(name, None)?;
        self.config.theme.name = name.to_string();
        self.config.theme.path = None;
        self.apply_theme(theme, source);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn picker_lists_every_builtin_theme_by_origin_and_marks_the_current_one() {
        let items = theme_pick_items("turnip");
        assert!(items.len() >= Theme::list_available().len());
        let item = |id: &str| items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item("turnip").category, "Current · Termeric");
        assert_eq!(item("turmeric").category, "Termeric");
        assert_eq!(item("dracula").category, "Guest");
        assert_eq!(item("catppuccin-mocha").title, "Catppuccin Mocha");
    }

    #[test]
    fn highlighting_previews_and_cancelling_restores_the_original_theme() {
        let mut app = App::new_headless(Config::default()).unwrap();
        let original = app.config.theme.name.clone();
        app.open_theme_picker();
        assert_eq!(
            app.command_palette.highlighted_id(),
            Some(original.as_str())
        );
        app.command_palette.select_command("dracula");
        app.sync_theme_preview();
        assert_eq!(app.theme.meta.name, "Dracula");
        assert_eq!(app.config.theme.name, original, "previews are not saved");

        app.command_palette.close();
        app.sync_theme_preview();
        assert!(app.theme_preview.is_none());
        assert_eq!(
            app.theme.meta.name,
            Theme::resolve(&original, None).unwrap().0.meta.name
        );
    }

    #[test]
    fn picking_adopts_the_chosen_theme_and_ends_the_preview() {
        let mut app = App::new_headless(Config::default()).unwrap();
        app.open_theme_picker();
        app.command_palette.select_command("nord");
        app.sync_theme_preview();
        let result = app.command_palette.execute();
        let crate::ui::command_palette::PaletteResult::Picked { kind, id } = result else {
            panic!("expected a pick, got {result:?}");
        };
        assert_eq!((kind, id.as_str()), (PickKind::Theme, "nord"));
        // `commit_theme` also saves the user's configuration file, so the
        // in-memory adoption it performs is exercised directly.
        app.adopt_theme(&id).unwrap();
        app.sync_theme_preview();
        assert!(app.theme_preview.is_none());
        assert_eq!(app.config.theme.name, "nord");
        assert_eq!(app.theme.meta.name, "Nord");
    }
}
