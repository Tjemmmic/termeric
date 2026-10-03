//! In-app discovery and attachment for local and saved-SSH live domains.

use crate::config::DomainsConfig;
use crate::domain::model::DomainSummary;
use crate::domain::{DomainAuthority, DomainTarget};
use crate::ui::command_palette::palette::{CommandSource, RegisteredCommand};
use crate::ui::command_palette::{CommandPalette, PaletteLayout, PaletteOwner, PaletteResult};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainPickerCommand {
    Discover {
        generation: u64,
        authority: DomainAuthority,
    },
    PrepareWindow {
        generation: u64,
        target: DomainTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainPickerPrepared {
    Ignored,
    Updated,
    OpenWindow {
        target: DomainTarget,
        snapshot: crate::domain::model::DomainSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryAction {
    Discover {
        authority: DomainAuthority,
        label: String,
    },
    PrepareWindow {
        target: DomainTarget,
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthorityEntry {
    id: String,
    title: String,
    detail: String,
    authority: DomainAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DomainPickerStage {
    Authorities,
    Loading {
        generation: u64,
        authority: DomainAuthority,
        label: String,
    },
    Preparing {
        generation: u64,
        target: DomainTarget,
        label: String,
    },
    Domains {
        authority: DomainAuthority,
        label: String,
    },
    Error {
        authority: DomainAuthority,
        label: String,
    },
}

pub struct DomainPicker {
    palette: CommandPalette,
    authorities: Vec<AuthorityEntry>,
    actions: Vec<(String, EntryAction)>,
    stage: DomainPickerStage,
    generation: u64,
}

impl DomainPicker {
    pub fn new() -> Self {
        let mut palette = CommandPalette::new();
        palette.set_presentation("Search hosts…", "host");
        Self {
            palette,
            authorities: Vec::new(),
            actions: Vec::new(),
            stage: DomainPickerStage::Authorities,
            generation: 0,
        }
    }

    pub fn open(&mut self, config: &DomainsConfig) {
        self.generation = self.generation.wrapping_add(1);
        self.authorities = vec![AuthorityEntry {
            id: "domain-picker.authority.local".into(),
            title: "Local domains".into(),
            detail: "This computer".into(),
            authority: DomainAuthority::Local,
        }];
        self.authorities
            .extend(config.ssh_hosts.iter().map(|(name, profile)| {
                let title = profile.description.as_ref().map_or_else(
                    || name.clone(),
                    |description| {
                        format!(
                            "{name} — {}",
                            crate::safe_text::truncate_with_ellipsis(description, 40)
                        )
                    },
                );
                AuthorityEntry {
                    id: format!("domain-picker.authority.ssh.{name}"),
                    title,
                    detail: crate::safe_text::truncate_with_ellipsis(&profile.destination, 48),
                    authority: DomainAuthority::Ssh {
                        destination: profile.destination.clone(),
                    },
                }
            }));
        self.show_authorities();
    }

    pub fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.palette.close();
        self.actions.clear();
    }

    pub fn back_or_close(&mut self) {
        if matches!(self.stage, DomainPickerStage::Authorities) {
            self.close();
        } else {
            self.generation = self.generation.wrapping_add(1);
            self.show_authorities();
        }
    }

    pub fn can_filter(&self) -> bool {
        matches!(
            self.stage,
            DomainPickerStage::Authorities | DomainPickerStage::Domains { .. }
        )
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            DomainPickerStage::Loading { .. } | DomainPickerStage::Preparing { .. }
        )
    }

    pub fn is_actionable(&self, id: &str) -> bool {
        self.actions.iter().any(|(entry_id, _)| entry_id == id)
    }

    pub fn execute_command(&mut self, id: &str) -> Option<DomainPickerCommand> {
        let result = self.palette.execute_command(id)?;
        self.resolve_palette_result(result)
    }

    pub fn activate(&mut self) -> Option<DomainPickerCommand> {
        let result = self.palette.execute();
        self.resolve_palette_result(result)
    }

    pub fn handle_click(
        &mut self,
        x: f32,
        y: f32,
        layout: &PaletteLayout,
    ) -> Option<DomainPickerCommand> {
        if !self.can_filter() && layout.input_contains(x, y) {
            return None;
        }
        let result = self.palette.handle_click(x, y, layout)?;
        self.resolve_palette_result(result)
    }

    pub fn complete(
        &mut self,
        generation: u64,
        result: Result<Vec<DomainSummary>, String>,
    ) -> bool {
        let DomainPickerStage::Loading {
            generation: pending,
            authority,
            label,
        } = &self.stage
        else {
            return false;
        };
        if *pending != generation || !self.palette.is_active() {
            return false;
        }
        let authority = authority.clone();
        let label = label.clone();
        match result {
            Ok(domains) => self.show_domains(authority, label, domains),
            Err(error) => self.show_error(authority, label, &error),
        }
        true
    }

    pub fn complete_preparation(
        &mut self,
        generation: u64,
        result: Result<crate::domain::model::DomainSnapshot, String>,
    ) -> DomainPickerPrepared {
        let DomainPickerStage::Preparing {
            generation: pending,
            target,
            label,
        } = &self.stage
        else {
            return DomainPickerPrepared::Ignored;
        };
        if *pending != generation || !self.palette.is_active() {
            return DomainPickerPrepared::Ignored;
        }
        let target = target.clone();
        let label = label.clone();
        match result {
            Ok(snapshot) if snapshot.id == target.domain_id => {
                self.close();
                DomainPickerPrepared::OpenWindow { target, snapshot }
            }
            Ok(_) => {
                self.show_error(
                    target.authority,
                    label,
                    "Domain server returned a mismatched snapshot",
                );
                DomainPickerPrepared::Updated
            }
            Err(error) => {
                self.show_error(target.authority, label, &error);
                DomainPickerPrepared::Updated
            }
        }
    }

    fn show_authorities(&mut self) {
        self.stage = DomainPickerStage::Authorities;
        self.actions.clear();
        let commands = self
            .authorities
            .iter()
            .map(|entry| {
                let id = entry.id.clone();
                self.actions.push((
                    id.clone(),
                    EntryAction::Discover {
                        authority: entry.authority.clone(),
                        label: entry.title.clone(),
                    },
                ));
                command(id, entry.title.clone(), entry.detail.clone())
            })
            .collect();
        self.palette
            .set_presentation("Search local or saved hosts…", "host");
        self.palette.replace_all_commands(commands);
        self.palette.open();
    }

    fn begin_discovery(
        &mut self,
        authority: DomainAuthority,
        label: String,
    ) -> DomainPickerCommand {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.stage = DomainPickerStage::Loading {
            generation,
            authority: authority.clone(),
            label: label.clone(),
        };
        self.actions.clear();
        self.palette.set_presentation(
            &format!(
                "Connecting to {}…",
                crate::safe_text::truncate_with_ellipsis(&label, 36)
            ),
            "domain",
        );
        self.palette.replace_all_commands(vec![command(
            "domain-picker.loading".into(),
            "Discovering live domains…".into(),
            crate::safe_text::truncate_with_ellipsis(&label, 48),
        )]);
        self.palette.open();
        DomainPickerCommand::Discover {
            generation,
            authority,
        }
    }

    fn show_domains(
        &mut self,
        authority: DomainAuthority,
        label: String,
        domains: Vec<DomainSummary>,
    ) {
        self.stage = DomainPickerStage::Domains {
            authority: authority.clone(),
            label: label.clone(),
        };
        self.actions.clear();
        let commands = if domains.is_empty() {
            vec![command(
                "domain-picker.empty".into(),
                "No live domains".into(),
                "Create one with `termeric domain create`".into(),
            )]
        } else {
            let mut seen = HashSet::new();
            domains
                .into_iter()
                .filter(|domain| seen.insert(domain.id.clone()))
                .map(|domain| {
                    let id = format!("domain-picker.domain.{}", domain.id);
                    self.actions.push((
                        id.clone(),
                        EntryAction::PrepareWindow {
                            target: DomainTarget {
                                authority: authority.clone(),
                                domain_id: domain.id.clone(),
                            },
                            label: domain.name.clone(),
                        },
                    ));
                    command(
                        id,
                        crate::safe_text::truncate_with_ellipsis(&domain.name, 48),
                        format!(
                            "{} · {} tabs · {} panes · {} attached",
                            crate::safe_text::truncate_with_ellipsis(&domain.id, 28),
                            domain.tabs,
                            domain.panes,
                            domain.attachments
                        ),
                    )
                })
                .collect()
        };
        self.palette.set_presentation(
            &format!(
                "Search domains on {}…",
                crate::safe_text::truncate_with_ellipsis(&label, 30)
            ),
            "domain",
        );
        self.palette.replace_all_commands(commands);
        self.palette.open();
    }

    fn show_error(&mut self, authority: DomainAuthority, label: String, error: &str) {
        self.stage = DomainPickerStage::Error {
            authority,
            label: label.clone(),
        };
        self.actions.clear();
        self.palette.set_presentation(
            &format!(
                "Could not reach {}",
                crate::safe_text::truncate_with_ellipsis(&label, 36)
            ),
            "result",
        );
        self.palette.replace_all_commands(vec![command(
            "domain-picker.error".into(),
            "Domain discovery failed".into(),
            crate::safe_text::truncate_with_ellipsis(error, 64),
        )]);
        self.palette.open();
    }

    fn begin_preparation(&mut self, target: DomainTarget, label: String) -> DomainPickerCommand {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.stage = DomainPickerStage::Preparing {
            generation,
            target: target.clone(),
            label: label.clone(),
        };
        self.actions.clear();
        self.palette.set_presentation(
            &format!(
                "Preparing {}…",
                crate::safe_text::truncate_with_ellipsis(&label, 40)
            ),
            "domain",
        );
        self.palette.replace_all_commands(vec![command(
            "domain-picker.preparing".into(),
            "Loading authoritative domain state…".into(),
            crate::safe_text::truncate_with_ellipsis(&target.domain_id, 48),
        )]);
        self.palette.open();
        DomainPickerCommand::PrepareWindow { generation, target }
    }

    fn resolve_palette_result(&mut self, result: PaletteResult) -> Option<DomainPickerCommand> {
        let PaletteResult::RunAction(id) = result else {
            if matches!(result, PaletteResult::Cancelled) {
                self.close();
            }
            return None;
        };
        let action = self
            .actions
            .iter()
            .find(|(entry_id, _)| entry_id == &id)
            .map(|(_, action)| action.clone());
        match action {
            Some(EntryAction::Discover { authority, label }) => {
                Some(self.begin_discovery(authority, label))
            }
            Some(EntryAction::PrepareWindow { target, label }) => {
                Some(self.begin_preparation(target, label))
            }
            None => {
                self.palette.open();
                None
            }
        }
    }
}

impl PaletteOwner for DomainPicker {
    fn palette(&self) -> &CommandPalette {
        &self.palette
    }

    fn palette_mut(&mut self) -> &mut CommandPalette {
        &mut self.palette
    }

    fn query_editable(&self) -> bool {
        self.can_filter()
    }
}

fn command(id: String, title: String, category: String) -> RegisteredCommand {
    RegisteredCommand {
        id,
        title,
        category,
        shortcut: None,
        icon: None,
        source: CommandSource::Core,
        destructive: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshHostProfile;
    use std::collections::BTreeMap;

    fn summary(id: &str, name: &str) -> DomainSummary {
        DomainSummary {
            id: id.into(),
            name: name.into(),
            workspace_id: format!("workspace-{id}"),
            tabs: 2,
            panes: 3,
            attachments: 1,
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn snapshot(id: &str) -> crate::domain::model::DomainSnapshot {
        crate::domain::model::DomainSnapshot {
            id: id.into(),
            name: "Compile services".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            workspace: crate::domain::model::LiveWorkspace {
                id: format!("workspace-{id}"),
                name: "Compile services".into(),
                tabs: Vec::new(),
                active_tab: 0,
            },
        }
    }

    #[test]
    fn saved_authority_discovery_opens_a_pinned_remote_target() {
        let config = DomainsConfig {
            ssh_hosts: BTreeMap::from([(
                "build".into(),
                SshHostProfile {
                    destination: "developer@build.example".into(),
                    description: Some("Build farm".into()),
                },
            )]),
        };
        let mut picker = DomainPicker::new();
        picker.open(&config);
        assert_eq!(picker.build_info().total_count, 2);
        picker.palette_mut().select_next();
        let Some(DomainPickerCommand::Discover {
            generation,
            authority,
        }) = picker.activate()
        else {
            panic!("expected saved authority discovery");
        };
        assert_eq!(
            authority,
            DomainAuthority::Ssh {
                destination: "developer@build.example".into(),
            }
        );
        assert!(picker.complete(
            generation,
            Ok(vec![summary("domain-7", "Compile services")])
        ));
        assert_eq!(picker.build_info().item_noun, "domain");
        assert_eq!(picker.build_info().total_count, 1);
        let Some(DomainPickerCommand::PrepareWindow { generation, target }) = picker.activate()
        else {
            panic!("expected authenticated snapshot preparation");
        };
        assert_eq!(
            target,
            DomainTarget {
                authority: DomainAuthority::Ssh {
                    destination: "developer@build.example".into(),
                },
                domain_id: "domain-7".into(),
            }
        );
        assert_eq!(
            picker.complete_preparation(generation, Ok(snapshot("domain-7"))),
            DomainPickerPrepared::OpenWindow {
                target,
                snapshot: snapshot("domain-7"),
            }
        );
        assert!(!picker.is_active());
    }

    #[test]
    fn stale_results_are_ignored_and_escape_returns_to_authorities() {
        let mut picker = DomainPicker::new();
        picker.open(&DomainsConfig::default());
        let Some(DomainPickerCommand::Discover { generation, .. }) = picker.activate() else {
            panic!("expected local discovery");
        };
        picker.back_or_close();
        assert!(!picker.complete(generation, Ok(vec![summary("stale", "Stale")])));
        assert_eq!(picker.build_info().item_noun, "host");
        assert!(picker.is_active());
        picker.back_or_close();
        assert!(!picker.is_active());
    }

    #[test]
    fn failures_and_empty_hosts_remain_bounded_navigable_states() {
        let mut picker = DomainPicker::new();
        picker.open(&DomainsConfig::default());
        let Some(DomainPickerCommand::Discover { generation, .. }) = picker.activate() else {
            panic!("expected local discovery");
        };
        assert!(picker.complete(generation, Err("socket unavailable".repeat(100))));
        assert_eq!(picker.build_info().total_count, 1);
        picker.back_or_close();
        let Some(DomainPickerCommand::Discover { generation, .. }) = picker.activate() else {
            panic!("expected local rediscovery");
        };
        assert!(picker.complete(generation, Ok(Vec::new())));
        assert_eq!(picker.build_info().total_count, 1);
        assert!(picker.activate().is_none());
        assert!(picker.is_active());
    }

    #[test]
    fn shared_palette_hit_geometry_selects_the_clicked_saved_host() {
        let config = DomainsConfig {
            ssh_hosts: BTreeMap::from([(
                "build".into(),
                SshHostProfile {
                    destination: "developer@build.example".into(),
                    description: None,
                },
            )]),
        };
        let mut picker = DomainPicker::new();
        picker.open(&config);
        let info = picker.build_info();
        let layout = PaletteLayout::calculate(900.0, 700.0, 10.0, 20.0, info.animation, 2);
        let x = layout.panel_x + layout.padding;
        let y = layout.list_y + layout.row_height * 1.5;
        assert!(matches!(
            picker.handle_click(x, y, &layout),
            Some(DomainPickerCommand::Discover {
                authority: DomainAuthority::Ssh { destination },
                ..
            }) if destination == "developer@build.example"
        ));
    }

    #[test]
    fn stale_or_mismatched_prepared_snapshots_cannot_open_a_window() {
        let mut picker = DomainPicker::new();
        picker.open(&DomainsConfig::default());
        let Some(DomainPickerCommand::Discover { generation, .. }) = picker.activate() else {
            panic!("expected discovery");
        };
        picker.complete(
            generation,
            Ok(vec![summary("domain-7", "Compile services")]),
        );
        let Some(DomainPickerCommand::PrepareWindow { generation, .. }) = picker.activate() else {
            panic!("expected preparation");
        };
        assert_eq!(
            picker.complete_preparation(generation.wrapping_add(1), Ok(snapshot("domain-7"))),
            DomainPickerPrepared::Ignored
        );
        assert_eq!(
            picker.complete_preparation(generation, Ok(snapshot("wrong-domain"))),
            DomainPickerPrepared::Updated
        );
        assert!(picker.is_active());
    }

    #[test]
    fn semantic_command_ids_follow_host_names_and_domain_ids_not_row_order() {
        let config = DomainsConfig {
            ssh_hosts: BTreeMap::from([
                (
                    "build".into(),
                    SshHostProfile {
                        destination: "developer@build.example".into(),
                        description: None,
                    },
                ),
                (
                    "staging".into(),
                    SshHostProfile {
                        destination: "developer@staging.example".into(),
                        description: None,
                    },
                ),
            ]),
        };
        let mut picker = DomainPicker::new();
        picker.open(&config);
        assert_eq!(
            picker.palette().command_ids().collect::<Vec<_>>(),
            [
                "domain-picker.authority.local",
                "domain-picker.authority.ssh.build",
                "domain-picker.authority.ssh.staging",
            ]
        );

        // Select local by identity, not by its sorted display row.
        assert!(
            picker
                .palette_mut()
                .select_command("domain-picker.authority.local")
        );
        let Some(DomainPickerCommand::Discover { generation, .. }) = picker.activate() else {
            panic!("expected local discovery");
        };
        assert!(picker.complete(
            generation,
            Ok(vec![
                summary("domain-z", "Zed"),
                summary("domain-a", "Alpha"),
                summary("domain-a", "Duplicate must be ignored"),
            ])
        ));
        let ids = picker.palette().command_ids().collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "domain-picker.domain.domain-a",
                "domain-picker.domain.domain-z"
            ]
        );
    }

    #[test]
    fn pending_stages_reject_filter_mutation() {
        let mut picker = DomainPicker::new();
        picker.open(&DomainsConfig::default());
        assert!(picker.set_query("local"));
        let info = picker.build_info();
        let layout = PaletteLayout::calculate(800.0, 600.0, 10.0, 20.0, 1.0, info.entries.len());
        let view = layout.input_viewport(&info.input, info.cursor).unwrap();
        assert!(
            picker
                .handle_click(view.text_x + 20.0, view.text_y, &layout)
                .is_none()
        );
        assert_eq!(picker.build_info().cursor, 2);
        let Some(DomainPickerCommand::Discover { .. }) = picker.activate() else {
            panic!("expected local discovery");
        };
        assert!(picker.is_busy());
        // Even a retained presentation value must stay read-only while loading.
        picker.palette.accessibility_set_input("read-only");
        let before_cursor = picker.build_info().cursor;
        assert!(
            picker
                .handle_click(view.text_x, view.text_y, &layout)
                .is_none()
        );
        assert_eq!(picker.build_info().cursor, before_cursor);
        let before = picker.build_info().input;
        picker.insert_query_text('x'.encode_utf8(&mut [0; 4]));
        picker.edit_query(crate::ui::text_input::TextInputEdit::Backspace);
        assert!(!picker.set_query("changed"));
        assert_eq!(picker.build_info().input, before);
    }
}
