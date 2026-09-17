//! Stable command identities and keyboard/assistive browse focus.

use crate::config::CustomCommand;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommandKey {
    name: String,
    command: String,
    alias: Option<String>,
    occurrence: usize,
}

impl CommandKey {
    fn matches(&self, command: &CustomCommand) -> bool {
        self.name == command.name && self.command == command.command && self.alias == command.alias
    }

    pub fn at(commands: &[CustomCommand], index: usize) -> Option<Self> {
        let command = commands.get(index)?;
        let mut key = Self {
            name: command.name.clone(),
            command: command.command.clone(),
            alias: command.alias.clone(),
            occurrence: 0,
        };
        key.occurrence = commands[..index]
            .iter()
            .filter(|command| key.matches(command))
            .count();
        Some(key)
    }
}

pub fn command_keys(commands: &[CustomCommand]) -> Vec<CommandKey> {
    let mut occurrences = std::collections::HashMap::new();
    commands
        .iter()
        .map(|command| {
            let identity = (&command.name, &command.command, &command.alias);
            let occurrence = occurrences.entry(identity).or_insert(0);
            let key = CommandKey {
                name: command.name.clone(),
                command: command.command.clone(),
                alias: command.alias.clone(),
                occurrence: *occurrence,
            };
            *occurrence += 1;
            key
        })
        .collect()
}

pub fn command_index(commands: &[CustomCommand], key: &CommandKey) -> Option<usize> {
    commands
        .iter()
        .enumerate()
        .filter(|(_, command)| key.matches(command))
        .nth(key.occurrence)
        .map(|(index, _)| index)
}

/// Resolved, frame-local control. Stored command focus uses `CommandKey`, not this index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandControl {
    New,
    Run(usize),
    Delete(usize),
    Close,
}

impl CommandControl {
    pub fn row(self) -> Option<usize> {
        match self {
            Self::Run(index) | Self::Delete(index) => Some(index),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CommandNavigationKey {
    Next,
    Previous,
    First,
    Last,
}

#[derive(Default)]
enum StoredControl {
    #[default]
    New,
    Run(CommandKey),
    Delete(CommandKey),
    Close,
}

#[derive(Default)]
pub(super) struct CommandNavigation {
    target: StoredControl,
    pub visible: bool,
}

impl CommandNavigation {
    pub fn resolve(&self, commands: &[CustomCommand]) -> Option<CommandControl> {
        Some(match &self.target {
            StoredControl::New => CommandControl::New,
            StoredControl::Close => CommandControl::Close,
            StoredControl::Run(key) => CommandControl::Run(command_index(commands, key)?),
            StoredControl::Delete(key) => CommandControl::Delete(command_index(commands, key)?),
        })
    }

    pub fn focus(&mut self, control: CommandControl, commands: &[CustomCommand]) -> bool {
        self.target = match control {
            CommandControl::New => StoredControl::New,
            CommandControl::Close => StoredControl::Close,
            CommandControl::Run(index) => {
                let Some(key) = CommandKey::at(commands, index) else {
                    return false;
                };
                StoredControl::Run(key)
            }
            CommandControl::Delete(index) => {
                let Some(key) = CommandKey::at(commands, index) else {
                    return false;
                };
                StoredControl::Delete(key)
            }
        };
        self.visible = true;
        true
    }

    pub fn navigate(
        &mut self,
        key: CommandNavigationKey,
        commands: &[CustomCommand],
    ) -> CommandControl {
        // Two fixed controls plus Run/Delete per row, without allocating a second list.
        let last = commands.len().saturating_mul(2).saturating_add(1);
        let current = match self.resolve(commands).unwrap_or(CommandControl::New) {
            CommandControl::New => 0,
            CommandControl::Run(index) => 1 + index * 2,
            CommandControl::Delete(index) => 2 + index * 2,
            CommandControl::Close => last,
        };
        let next = match key {
            CommandNavigationKey::First => 0,
            CommandNavigationKey::Last => last,
            CommandNavigationKey::Next => {
                if current >= last {
                    0
                } else {
                    current + 1
                }
            }
            CommandNavigationKey::Previous => {
                if current == 0 {
                    last
                } else {
                    current - 1
                }
            }
        };
        let control = if next == 0 {
            CommandControl::New
        } else if next == last {
            CommandControl::Close
        } else if next % 2 == 1 {
            CommandControl::Run((next - 1) / 2)
        } else {
            CommandControl::Delete((next - 2) / 2)
        };
        self.focus(control, commands);
        control
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> CustomCommand {
        CustomCommand {
            name: name.into(),
            command: "inert".into(),
            alias: None,
        }
    }

    #[test]
    fn command_navigation_wraps_every_control_without_an_allocated_focus_list() {
        let commands = vec![command("a"), command("b")];
        let mut navigation = CommandNavigation::default();
        assert!(!navigation.visible);
        assert_eq!(navigation.resolve(&commands), Some(CommandControl::New));
        for expected in [
            CommandControl::Run(0),
            CommandControl::Delete(0),
            CommandControl::Run(1),
            CommandControl::Delete(1),
            CommandControl::Close,
            CommandControl::New,
        ] {
            assert_eq!(
                navigation.navigate(CommandNavigationKey::Next, &commands),
                expected
            );
        }
        for expected in [
            CommandControl::Close,
            CommandControl::Delete(1),
            CommandControl::Run(1),
            CommandControl::Delete(0),
            CommandControl::Run(0),
            CommandControl::New,
        ] {
            assert_eq!(
                navigation.navigate(CommandNavigationKey::Previous, &commands),
                expected
            );
        }
        assert_eq!(
            navigation.navigate(CommandNavigationKey::Last, &commands),
            CommandControl::Close
        );
        assert_eq!(
            navigation.navigate(CommandNavigationKey::First, &commands),
            CommandControl::New
        );
        assert_eq!(
            navigation.navigate(CommandNavigationKey::Next, &[]),
            CommandControl::Close
        );
        assert_eq!(
            navigation.navigate(CommandNavigationKey::Next, &[]),
            CommandControl::New
        );
    }

    #[test]
    fn command_focus_tracks_exact_tuples_occurrences_and_rejects_changed_values() {
        let mut commands = vec![command("a"), command("same"), command("same")];
        let keys = command_keys(&commands);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(Some(key.clone()), CommandKey::at(&commands, index));
            assert_eq!(command_index(&commands, key), Some(index));
        }
        let mut navigation = CommandNavigation::default();
        assert!(navigation.focus(CommandControl::Delete(2), &commands));
        commands.insert(0, command("before"));
        assert_eq!(
            navigation.resolve(&commands),
            Some(CommandControl::Delete(3))
        );
        assert!(!navigation.focus(CommandControl::Run(usize::MAX), &commands));
        assert_eq!(
            navigation.resolve(&commands),
            Some(CommandControl::Delete(3))
        );
        commands[3].alias = Some("changed".into());
        assert_eq!(navigation.resolve(&commands), None);
        commands[3].alias = None;
        commands[3].command = "changed".into();
        assert_eq!(navigation.resolve(&commands), None);
        commands[3].command = "inert".into();
        commands.remove(1);
        assert_eq!(
            navigation.resolve(&commands),
            Some(CommandControl::Delete(2))
        );
        commands.remove(2);
        assert_eq!(navigation.resolve(&commands), None);
    }
}
