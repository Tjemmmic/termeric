//! The `/` command menu: which command is being typed, which of the agent's
//! commands match it, and completing one.
//!
//! Commands come from the agent itself: Claude Code lists the ones it runs
//! without its terminal interface when it accepts the connection. Nothing
//! here knows any command by name.

use crate::agent_view::event::SlashCommand;
use crate::agent_view::session::AgentSession;

use super::{AgentViewState, Focus, Screen};

/// Most matches listed at once; typing narrows the rest.
pub const MAX_SHOWN: usize = 8;

/// The command menu's own state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandMenu {
    /// The highlighted match, by position.
    pub highlighted: usize,
    /// Escape hid the menu for this typed name; typing more shows it again.
    pub dismissed: Option<String>,
}

/// The name typed after a leading `/`, up to the caret, while the caret is
/// still in the message's first word.
pub fn typed_name(text: &str, caret: usize) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let name = rest.get(..caret.checked_sub(1)?)?;
    (!name.chars().any(char::is_whitespace)).then_some(name)
}

/// The command a message invokes, if it starts with one. Paths such as
/// `/etc/hosts` are not commands.
pub fn invoked_name(message: &str) -> Option<&str> {
    let rest = message.trim_start().strip_prefix('/')?;
    let name = rest.split(char::is_whitespace).next()?;
    let mut characters = name.chars();
    let first = characters.next()?;
    let starts_well = first.is_ascii_alphanumeric() || first == '_';
    let rest_valid = characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, ':' | '_' | '-'));
    (starts_well && rest_valid).then_some(name)
}

/// Commands whose names start with `typed`, then those that contain it,
/// each group alphabetical. Case is ignored.
pub fn matching<'a>(commands: &'a [SlashCommand], typed: &str) -> Vec<&'a SlashCommand> {
    let typed = typed.to_lowercase();
    let (mut starts, mut contains): (Vec<_>, Vec<_>) = commands
        .iter()
        .filter(|command| command.name.to_lowercase().contains(&typed))
        .partition(|command| command.name.to_lowercase().starts_with(&typed));
    starts.sort_by(|a, b| a.name.cmp(&b.name));
    contains.sort_by(|a, b| a.name.cmp(&b.name));
    starts.extend(contains);
    starts
}

/// `text` with its first word replaced by `/name` and a space, and where the
/// caret goes: after that space.
pub fn complete(text: &str, name: &str) -> (String, usize) {
    let first_word_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let rest = text[first_word_end..].trim_start_matches([' ', '\t']);
    let completed = format!("/{name} ");
    let caret = completed.len();
    (format!("{completed}{rest}"), caret)
}

/// The commands the agent offers here: the shown session's, else those of
/// any session that has connected, since they share one installation.
pub fn offered<'a>(state: &AgentViewState, sessions: &[&'a AgentSession]) -> &'a [SlashCommand] {
    // The tab's own session first: skills and commands can differ by folder.
    let shown = match state.screen {
        Screen::Sessions => None,
        _ => state
            .active
            .and_then(|id| sessions.iter().find(|session| session.id() == id)),
    };
    shown
        .into_iter()
        .chain(sessions.iter())
        .find_map(|session| session.transcript().handshake())
        .map_or(&[], |handshake| handshake.commands.as_slice())
}

/// The matches the menu shows now, or none while it is hidden: the message
/// box must have the keyboard, no other menu may be open, and the person
/// must not have dismissed it for what they typed.
pub fn shown<'a>(state: &AgentViewState, commands: &'a [SlashCommand]) -> Vec<&'a SlashCommand> {
    if !state.has_keyboard()
        || state.focus != Focus::Composer
        || state.menu.is_some()
        || state.screen == Screen::Sessions
    {
        return Vec::new();
    }
    let Some(composer) = state.composer() else {
        return Vec::new();
    };
    let Some(typed) = typed_name(composer.text(), composer.caret()) else {
        return Vec::new();
    };
    if state.commands.dismissed.as_deref() == Some(typed) {
        return Vec::new();
    }
    let mut matches = matching(commands, typed);
    matches.truncate(MAX_SHOWN);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> SlashCommand {
        SlashCommand {
            name: name.into(),
            description: String::new(),
            argument_hint: String::new(),
        }
    }

    #[test]
    fn the_typed_name_ends_at_the_caret_inside_the_first_word() {
        assert_eq!(typed_name("/com", 4), Some("com"));
        assert_eq!(typed_name("/compact now", 3), Some("co"));
        assert_eq!(typed_name("/", 1), Some(""));
        assert_eq!(typed_name("/compact now", 12), None);
        assert_eq!(typed_name("/com", 0), None);
        assert_eq!(typed_name("hello /com", 10), None);
    }

    #[test]
    fn commands_are_told_apart_from_paths() {
        assert_eq!(invoked_name("/compact"), Some("compact"));
        assert_eq!(invoked_name("  /plugin:skill do it"), Some("plugin:skill"));
        assert_eq!(invoked_name("/etc/hosts is broken"), None);
        assert_eq!(invoked_name("/"), None);
        assert_eq!(invoked_name("plain text"), None);
    }

    #[test]
    fn prefix_matches_come_before_other_matches() {
        let commands = [
            command("review"),
            command("compact"),
            command("security-review"),
            command("Context"),
        ];
        let names = |typed| {
            matching(&commands, typed)
                .into_iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names("re"), ["review", "security-review"]);
        assert_eq!(names("con"), ["Context"]);
        assert_eq!(names("").len(), 4);
        assert!(names("zzz").is_empty());
    }

    #[test]
    fn completing_replaces_the_first_word_and_keeps_the_rest() {
        assert_eq!(complete("/com", "compact"), ("/compact ".into(), 9));
        assert_eq!(
            complete("/re the auth code", "review"),
            ("/review the auth code".into(), 8)
        );
        assert_eq!(complete("/", "init"), ("/init ".into(), 6));
    }
}
