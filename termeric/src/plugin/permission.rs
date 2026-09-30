//! Declarative plugin capabilities and the window scope each grant covers.
//!
//! A permission names one capability and, for capabilities that act on
//! windows, a scope: the plugin's originating window (the default) or every
//! window this Termeric process owns (written with an `:all-windows` suffix).
//! An all-windows grant also covers the originating window.

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

const ALL_WINDOWS_SUFFIX: &str = ":all-windows";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Launch profile processes and event handlers.
    ProcessSpawn,
    /// Receive subscribed lifecycle events.
    EventSubscribe,
    /// Show informational messages and notices.
    UiActions,
    /// Receive bounded command text and output with `command-completed`.
    CommandOutput,
    /// Request native focus for any window (always process-wide).
    WindowFocus,
    /// Launch context, event context, and `query-workspace`.
    WorkspaceQuery,
    /// `select-tab` and `focus-pane`.
    Navigate,
    /// `query-terminal`.
    TerminalRead,
    /// `send-text`.
    TerminalInput,
    TabCreate,
    TabClose,
    TabMove,
    PaneCreate,
    PaneClose,
    PaneMove,
    /// `swap-panes`, `balance-panes`, `apply-pane-layout`, and `resize-divider`.
    PaneLayout,
    WindowClose,
}

impl Capability {
    pub const ALL: [Self; 17] = [
        Self::ProcessSpawn,
        Self::EventSubscribe,
        Self::UiActions,
        Self::CommandOutput,
        Self::WindowFocus,
        Self::WorkspaceQuery,
        Self::Navigate,
        Self::TerminalRead,
        Self::TerminalInput,
        Self::TabCreate,
        Self::TabClose,
        Self::TabMove,
        Self::PaneCreate,
        Self::PaneClose,
        Self::PaneMove,
        Self::PaneLayout,
        Self::WindowClose,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::ProcessSpawn => "process-spawn",
            Self::EventSubscribe => "event-subscribe",
            Self::UiActions => "ui-actions",
            Self::CommandOutput => "command-output",
            Self::WindowFocus => "window-focus",
            Self::WorkspaceQuery => "workspace-query",
            Self::Navigate => "navigate",
            Self::TerminalRead => "terminal-read",
            Self::TerminalInput => "terminal-input",
            Self::TabCreate => "tab-create",
            Self::TabClose => "tab-close",
            Self::TabMove => "tab-move",
            Self::PaneCreate => "pane-create",
            Self::PaneClose => "pane-close",
            Self::PaneMove => "pane-move",
            Self::PaneLayout => "pane-layout",
            Self::WindowClose => "window-close",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.name() == name)
    }

    /// Whether the capability acts on windows and therefore takes a scope.
    pub fn is_scoped(self) -> bool {
        !matches!(
            self,
            Self::ProcessSpawn
                | Self::EventSubscribe
                | Self::UiActions
                | Self::CommandOutput
                | Self::WindowFocus
        )
    }

    /// Whether the capability is exercised only through handler queries or
    /// actions, which need a persistent handler to receive their results.
    pub fn requires_persistent_handler(self) -> bool {
        !matches!(
            self,
            Self::ProcessSpawn
                | Self::EventSubscribe
                | Self::UiActions
                | Self::CommandOutput
                | Self::WorkspaceQuery
        )
    }

    /// Short user-facing description for install and update reviews.
    pub fn description(self) -> &'static str {
        match self {
            Self::ProcessSpawn => "Start processes",
            Self::EventSubscribe => "Receive lifecycle events",
            Self::UiActions => "Show messages and notices",
            Self::CommandOutput => "Read completed command text and output",
            Self::WindowFocus => "Focus any window",
            Self::WorkspaceQuery => "Inspect windows, tabs, and panes",
            Self::Navigate => "Select tabs and focus panes",
            Self::TerminalRead => "Read terminal text",
            Self::TerminalInput => "Type into terminals",
            Self::TabCreate => "Create tabs",
            Self::TabClose => "Close tabs",
            Self::TabMove => "Move tabs",
            Self::PaneCreate => "Create panes",
            Self::PaneClose => "Close panes",
            Self::PaneMove => "Move panes",
            Self::PaneLayout => "Arrange panes",
            Self::WindowClose => "Close windows",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Which windows a scoped capability may act on. The derived order is
/// breadth: an all-windows grant covers a window-scoped request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PermissionScope {
    /// Only the window whose event started the handler conversation.
    #[default]
    Window,
    /// Every live window owned by this Termeric process.
    AllWindows,
}

/// One requested or granted capability with its scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginPermission {
    pub capability: Capability,
    pub scope: PermissionScope,
}

impl PluginPermission {
    /// User-facing label, e.g. "Create panes (all windows)".
    pub fn label(&self) -> String {
        let description = self.capability.description();
        if !self.capability.is_scoped() {
            return description.to_string();
        }
        match self.scope {
            PermissionScope::Window => format!("{description} (this window)"),
            PermissionScope::AllWindows => format!("{description} (all windows)"),
        }
    }
}

impl std::fmt::Display for PluginPermission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.capability.name())?;
        if self.scope == PermissionScope::AllWindows {
            formatter.write_str(ALL_WINDOWS_SUFFIX)?;
        }
        Ok(())
    }
}

impl std::str::FromStr for PluginPermission {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (name, scope) = match value.split_once(':') {
            None => (value, PermissionScope::Window),
            Some((name, "all-windows")) => (name, PermissionScope::AllWindows),
            Some((_, scope)) => bail!(
                "unknown scope {scope:?} in plugin permission {value:?}; the only scope suffix is {ALL_WINDOWS_SUFFIX}"
            ),
        };
        let Some(capability) = Capability::from_name(name) else {
            bail!("unknown plugin permission {value:?}");
        };
        if scope == PermissionScope::AllWindows && !capability.is_scoped() {
            bail!("plugin permission {name:?} does not take a window scope");
        }
        Ok(Self { capability, scope })
    }
}

/// A set of permissions with at most one scope per capability. Manifests use
/// it for requests and the registry for grants; both serialize as a list of
/// permission strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginPermissions(BTreeMap<Capability, PermissionScope>);

impl PluginPermissions {
    /// Build a set, rejecting a capability that appears more than once.
    pub fn from_permissions(
        permissions: impl IntoIterator<Item = PluginPermission>,
    ) -> Result<Self> {
        let mut set = BTreeMap::new();
        for permission in permissions {
            if set
                .insert(permission.capability, permission.scope)
                .is_some()
            {
                bail!(
                    "plugin permission {:?} is listed more than once",
                    permission.capability.name()
                );
            }
        }
        Ok(Self(set))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = PluginPermission> + '_ {
        self.0
            .iter()
            .map(|(&capability, &scope)| PluginPermission { capability, scope })
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.0.contains_key(&capability)
    }

    pub fn scope(&self, capability: Capability) -> Option<PermissionScope> {
        self.0.get(&capability).copied()
    }

    /// Whether this set covers `permission`: it holds the capability with at
    /// least the permission's scope.
    pub fn covers(&self, permission: PluginPermission) -> bool {
        self.scope(permission.capability)
            .is_some_and(|scope| scope >= permission.scope)
    }

    /// Set a capability's scope, replacing any existing entry.
    pub fn set(&mut self, permission: PluginPermission) {
        self.0.insert(permission.capability, permission.scope);
    }

    pub fn remove(&mut self, capability: Capability) -> bool {
        self.0.remove(&capability).is_some()
    }

    /// Permissions in this set that `other` does not cover.
    pub fn uncovered_by(&self, other: &Self) -> Vec<PluginPermission> {
        self.iter()
            .filter(|permission| !other.covers(*permission))
            .collect()
    }

    /// Capabilities present in both sets, each at the narrower scope.
    pub fn intersection(&self, other: &Self) -> Self {
        Self(
            self.0
                .iter()
                .filter_map(|(capability, scope)| {
                    other
                        .scope(*capability)
                        .map(|other_scope| (*capability, (*scope).min(other_scope)))
                })
                .collect(),
        )
    }
}

impl std::fmt::Display for PluginPermissions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut separator = "";
        for permission in self.iter() {
            write!(formatter, "{separator}{permission}")?;
            separator = ",";
        }
        Ok(())
    }
}

impl Serialize for PluginPermissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(|permission| permission.to_string()))
    }
}

impl<'de> Deserialize<'de> for PluginPermissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        let permissions = names
            .iter()
            .map(|name| name.parse::<PluginPermission>())
            .collect::<Result<Vec<_>>>()
            .map_err(serde::de::Error::custom)?;
        Self::from_permissions(permissions).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission(value: &str) -> PluginPermission {
        value.parse().unwrap()
    }

    fn set(values: &[&str]) -> PluginPermissions {
        PluginPermissions::from_permissions(values.iter().map(|value| permission(value))).unwrap()
    }

    #[test]
    fn every_capability_round_trips_by_name_with_its_valid_scopes() {
        for capability in Capability::ALL {
            let window = permission(capability.name());
            assert_eq!(window.capability, capability);
            assert_eq!(window.scope, PermissionScope::Window);
            assert_eq!(window.to_string(), capability.name());
            let scoped = format!("{}:all-windows", capability.name());
            if capability.is_scoped() {
                let all = permission(&scoped);
                assert_eq!(all.scope, PermissionScope::AllWindows);
                assert_eq!(all.to_string(), scoped);
            } else {
                let error = scoped.parse::<PluginPermission>().unwrap_err();
                assert!(error.to_string().contains("does not take a window scope"));
            }
        }
    }

    #[test]
    fn unknown_names_removed_names_and_unknown_scopes_are_rejected() {
        for removed in [
            "terminal-text",
            "terminal-query",
            "workspace-actions",
            "workspace-topology",
            "process-query",
            "process-actions",
            "process-window-focus",
            "filesystem-read",
            "filesystem-write",
            "network",
            "clipboard",
            "secrets",
            "camera",
        ] {
            let error = removed.parse::<PluginPermission>().unwrap_err();
            assert!(
                error.to_string().contains("unknown plugin permission"),
                "{removed}: {error}"
            );
        }
        let error = "pane-create:window"
            .parse::<PluginPermission>()
            .unwrap_err();
        assert!(error.to_string().contains("unknown scope"));
        assert!(
            "pane-create:all-windows:extra"
                .parse::<PluginPermission>()
                .is_err()
        );
    }

    #[test]
    fn duplicate_capabilities_are_rejected_in_sets_and_serialized_lists() {
        let error = PluginPermissions::from_permissions([
            permission("pane-create"),
            permission("pane-create:all-windows"),
        ])
        .unwrap_err();
        assert!(error.to_string().contains("more than once"));

        #[derive(Debug, Deserialize)]
        struct Holder {
            #[allow(dead_code)]
            permissions: PluginPermissions,
        }
        let error = toml::from_str::<Holder>(r#"permissions = ["navigate", "navigate"]"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("more than once"), "{error}");
    }

    #[test]
    fn coverage_is_scope_aware_and_intersection_keeps_the_narrower_scope() {
        let requests = set(&["process-spawn", "pane-create:all-windows", "navigate"]);
        let broad = set(&[
            "process-spawn",
            "pane-create:all-windows",
            "navigate:all-windows",
        ]);
        let narrow = set(&["process-spawn", "pane-create"]);

        assert!(requests.uncovered_by(&broad).is_empty());
        assert_eq!(
            requests.uncovered_by(&narrow),
            vec![
                permission("navigate"),
                permission("pane-create:all-windows")
            ]
        );
        assert_eq!(
            broad.uncovered_by(&requests),
            vec![permission("navigate:all-windows")]
        );
        assert_eq!(
            broad.intersection(&narrow),
            set(&["process-spawn", "pane-create"])
        );
        assert_eq!(
            requests.to_string(),
            "process-spawn,navigate,pane-create:all-windows"
        );
    }

    #[test]
    fn labels_name_the_scope_only_for_scoped_capabilities() {
        assert_eq!(
            permission("pane-create:all-windows").label(),
            "Create panes (all windows)"
        );
        assert_eq!(
            permission("pane-create").label(),
            "Create panes (this window)"
        );
        assert_eq!(permission("process-spawn").label(), "Start processes");
        assert_eq!(permission("window-focus").label(), "Focus any window");
    }
}
