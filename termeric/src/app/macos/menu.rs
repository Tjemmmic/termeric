//! The menu bar. winit's default menu is turned off and this one installed
//! when the app finishes launching.
//!
//! Termeric's own items carry no key equivalents. AppKit would match a key
//! equivalent before the key reached winit, so a menu chord would run its
//! command past whatever holds the keyboard (a dialog, the palette) instead
//! of going through Termeric's one input chain, and AppKit has no documented
//! way to show a shortcut in a menu without it firing. Every ⌘ chord
//! therefore reaches winit's `keyDown` as a key press, and the menus are a
//! clickable list of commands. The system's app-wide items (Hide, Hide
//! Others, Quit, Minimize) keep their standard equivalents.

use crate::input::Action;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSWorkspace};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString, NSURL};
use std::cell::OnceCell;

/// Where "Termeric on GitHub" leads.
const PROJECT_PAGE: &str = "https://github.com/Tjemmmic/termeric";

/// One menu entry.
enum Entry {
    /// Runs one of Termeric's own actions in the focused window.
    Command(&'static str, Action),
    /// Sends `selector` to AppKit's responder chain, with an optional key
    /// equivalent (key and modifiers beyond ⌘).
    System(
        &'static str,
        Sel,
        Option<(&'static str, NSEventModifierFlags)>,
    ),
    /// Opens the project page.
    ProjectPage,
    Separator,
}

/// The menus, in order, as (title, role, entries).
fn menus() -> Vec<(&'static str, Role, Vec<Entry>)> {
    use Entry::{Command, ProjectPage, Separator, System};
    let none = NSEventModifierFlags::empty();
    vec![
        (
            "Termeric",
            Role::Application,
            vec![
                Command("About Termeric", Action::ShowAbout),
                Separator,
                Command("Settings…", Action::ToggleSettings),
                Separator,
                // The Services submenu is filled by macOS; see `Role`.
                Separator,
                System("Hide Termeric", sel!(hide:), Some(("h", none))),
                System(
                    "Hide Others",
                    sel!(hideOtherApplications:),
                    Some(("h", NSEventModifierFlags::Option)),
                ),
                System("Show All", sel!(unhideAllApplications:), None),
                Separator,
                // `terminate:` ends the app through winit's exiting(), which
                // saves every window (`Application::exiting`).
                System("Quit Termeric", sel!(terminate:), Some(("q", none))),
            ],
        ),
        (
            "Shell",
            Role::Plain,
            vec![
                Command("New Window", Action::NewWindow),
                Command("New Tab", Action::NewTab),
                Separator,
                Command("Split Right", Action::SplitVertical),
                Command("Split Down", Action::SplitHorizontal),
                Separator,
                Command("Rename Tab…", Action::RenameTab),
                Separator,
                Command("Close Pane", Action::ClosePane),
                Command("Close Tab", Action::CloseTab),
            ],
        ),
        (
            "Edit",
            Role::Plain,
            vec![
                Command("Copy", Action::Copy),
                Command("Paste", Action::Paste),
                Separator,
                Command("Find…", Action::ToggleSearch),
                Separator,
                Command("Clear Scrollback", Action::ClearScrollback),
            ],
        ),
        (
            "View",
            Role::Plain,
            vec![
                Command("Command Palette", Action::ToggleCommandPalette),
                Command("Change Theme…", Action::ChangeTheme),
                Separator,
                Command("Bigger", Action::IncreaseFontSize),
                Command("Smaller", Action::DecreaseFontSize),
                Command("Actual Size", Action::ResetFontSize),
                Separator,
                Command("Toggle Full Screen", Action::ToggleFullscreen),
            ],
        ),
        (
            "Window",
            Role::Windows,
            vec![
                System("Minimize", sel!(performMiniaturize:), Some(("m", none))),
                System("Zoom", sel!(performZoom:), None),
                Separator,
                Command("Next Tab", Action::NextTab),
                Command("Previous Tab", Action::PrevTab),
                Separator,
                System("Bring All to Front", sel!(arrangeInFront:), None),
            ],
        ),
        (
            "Help",
            Role::Help,
            vec![
                Command("Keyboard Shortcuts", Action::ToggleShortcutsPanel),
                ProjectPage,
            ],
        ),
    ]
}

/// What macOS does with a menu beyond showing it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Plain,
    /// The application menu; it also holds the Services submenu.
    Application,
    /// macOS lists the open windows in it.
    Windows,
    /// macOS adds its help search to it.
    Help,
}

thread_local! {
    /// The menus' target, kept alive for as long as the menus are: an item
    /// holds its target weakly.
    static TARGET: OnceCell<Retained<MenuTarget>> = const { OnceCell::new() };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and MenuTarget does
    // not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "TermericMenuTarget"]
    struct MenuTarget;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        // SAFETY: the signature is AppKit's action signature.
        #[unsafe(method(performCommand:))]
        fn perform_command(&self, sender: &NSMenuItem) {
            let commands = commands();
            if let Some(action) = usize::try_from(sender.tag())
                .ok()
                .and_then(|index| commands.get(index))
            {
                super::super::post_menu_command(action.clone());
            }
        }

        // SAFETY: the signature is AppKit's action signature.
        #[unsafe(method(openProjectPage:))]
        fn open_project_page(&self, _sender: Option<&AnyObject>) {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(PROJECT_PAGE)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        }
    }
);

impl MenuTarget {
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(());
        // SAFETY: NSObject's init has this signature.
        unsafe { msg_send![super(this), init] }
    }
}

/// Every command a menu runs, in menu order; an item's tag is its index.
fn commands() -> Vec<Action> {
    menus()
        .into_iter()
        .flat_map(|(_, _, entries)| entries)
        .filter_map(|entry| match entry {
            Entry::Command(_, action) => Some(action),
            _ => None,
        })
        .collect()
}

/// Build the menu bar and make it the app's.
pub(super) fn install(main_thread: MainThreadMarker) {
    let app = NSApplication::sharedApplication(main_thread);
    let target = TARGET.with(|cell| cell.get_or_init(|| MenuTarget::new(main_thread)).clone());
    let bar = NSMenu::new(main_thread);
    let mut command_index = 0;
    for (title, role, entries) in menus() {
        let title = NSString::from_str(title);
        let menu = NSMenu::initWithTitle(NSMenu::alloc(main_thread), &title);
        if role == Role::Application {
            // The Services submenu goes after Settings and its separator,
            // between the two separators the entries leave for it.
            let services =
                NSMenu::initWithTitle(NSMenu::alloc(main_thread), &NSString::from_str("Services"));
            let mut separators = 0;
            for entry in entries {
                let is_separator = matches!(entry, Entry::Separator);
                menu.addItem(&item(main_thread, entry, &target, &mut command_index));
                if is_separator {
                    separators += 1;
                    if separators == 2 {
                        let services_item = plain_item(main_thread, "Services", None, "");
                        services_item.setSubmenu(Some(&services));
                        menu.addItem(&services_item);
                    }
                }
            }
            app.setServicesMenu(Some(&services));
        } else {
            for entry in entries {
                menu.addItem(&item(main_thread, entry, &target, &mut command_index));
            }
        }
        match role {
            Role::Windows => app.setWindowsMenu(Some(&menu)),
            Role::Help => app.setHelpMenu(Some(&menu)),
            Role::Plain | Role::Application => {}
        }
        let holder = plain_item(main_thread, "", None, "");
        holder.setSubmenu(Some(&menu));
        bar.addItem(&holder);
    }
    app.setMainMenu(Some(&bar));
}

/// The menu item for `entry`; a command takes the next command index as its
/// tag.
fn item(
    main_thread: MainThreadMarker,
    entry: Entry,
    target: &MenuTarget,
    command_index: &mut isize,
) -> Retained<NSMenuItem> {
    match entry {
        Entry::Separator => NSMenuItem::separatorItem(main_thread),
        Entry::Command(title, _) => {
            let item = plain_item(main_thread, title, Some(sel!(performCommand:)), "");
            // SAFETY: the target is a MenuTarget, which answers
            // performCommand:, and outlives the menus (`TARGET`).
            unsafe { item.setTarget(Some(target)) };
            item.setTag(*command_index);
            *command_index += 1;
            item
        }
        Entry::ProjectPage => {
            let item = plain_item(
                main_thread,
                "Termeric on GitHub",
                Some(sel!(openProjectPage:)),
                "",
            );
            // SAFETY: as above, for openProjectPage:.
            unsafe { item.setTarget(Some(target)) };
            item
        }
        Entry::System(title, action, equivalent) => {
            let (key, modifiers) = equivalent.unwrap_or(("", NSEventModifierFlags::empty()));
            let item = plain_item(main_thread, title, Some(action), key);
            if equivalent.is_some() {
                item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | modifiers);
            }
            // No target: the action goes to the responder chain (the window,
            // then the application), where AppKit implements it.
            item
        }
    }
}

fn plain_item(
    main_thread: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    // SAFETY: the action, when given, is answered by the item's target or by
    // AppKit's responder chain.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(main_thread),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Termeric's own items never take a key from the input chain, and the
    /// app-wide system items keep theirs.
    #[test]
    fn only_system_items_have_key_equivalents() {
        let mut equivalents = Vec::new();
        for (_, _, entries) in menus() {
            for entry in entries {
                if let Entry::System(title, _, Some((key, _))) = entry {
                    equivalents.push((title, key));
                }
            }
        }
        assert_eq!(
            equivalents,
            [
                ("Hide Termeric", "h"),
                ("Hide Others", "h"),
                ("Quit Termeric", "q"),
                ("Minimize", "m"),
            ]
        );
    }

    #[test]
    fn every_menu_command_has_a_tag_of_its_own() {
        let commands = commands();
        assert!(commands.len() > 10);
        assert!(matches!(commands[0], Action::ShowAbout));
    }
}
