//! Asynchronous orchestration for the in-app live-domain picker.

use super::{App, AppEvent, EVENT_LOOP_PROXY, RedrawReason};
use crate::domain::protocol::{Request, Response};
use crate::ui::domain_picker::{DomainPickerCommand, DomainPickerPrepared};

impl App {
    /// Live-domain commands to hide from the palette: attaching until a
    /// local daemon runs or an SSH host is configured, and cancelling an
    /// upload unless one runs.
    pub(super) fn unavailable_domain_commands(&self) -> Vec<String> {
        let mut hidden = Vec::new();
        let daemon_running = crate::domain::paths::discover_runtime_directory()
            .join(crate::domain::paths::SOCKET_NAME)
            .exists();
        if !daemon_running && self.config.domains.ssh_hosts.is_empty() {
            hidden.push("core.attach-domain".to_owned());
        }
        if self.active_file_upload.is_none() {
            hidden.push("core.cancel-file-upload".to_owned());
        }
        hidden
    }

    pub(super) fn open_domain_picker(&mut self) {
        self.ime.clear_preedit();
        self.command_palette.close();
        self.domain_picker.open(&self.config.domains);
        self.request_redraw(RedrawReason::Action);
    }

    pub(super) fn handle_domain_picker_command(&mut self, command: DomainPickerCommand) {
        self.ime.clear_preedit();
        match command {
            DomainPickerCommand::Discover {
                generation,
                authority,
            } => self.discover_domains(generation, authority),
            DomainPickerCommand::PrepareWindow { generation, target } => {
                self.prepare_domain_window(generation, target)
            }
        }
    }

    pub(super) fn handle_domain_picker_loaded(
        &mut self,
        generation: u64,
        result: Result<Vec<crate::domain::model::DomainSummary>, String>,
    ) {
        if self.domain_picker.complete(generation, result) {
            self.ime.clear_preedit();
            self.request_redraw(RedrawReason::WindowEvent);
        }
    }

    pub(super) fn handle_domain_picker_prepared(
        &mut self,
        generation: u64,
        result: Result<crate::domain::model::DomainSnapshot, String>,
    ) {
        match self.domain_picker.complete_preparation(generation, result) {
            DomainPickerPrepared::Ignored => return,
            DomainPickerPrepared::Updated => {}
            DomainPickerPrepared::OpenWindow { target, snapshot } => {
                self.requested_window_launch = Some(crate::cli::WindowLaunch {
                    force_new: true,
                    domain: Some(target),
                    domain_snapshot: Some(snapshot),
                    ..crate::cli::WindowLaunch::default()
                });
            }
        }
        self.ime.clear_preedit();
        self.request_redraw(RedrawReason::WindowEvent);
    }

    pub(super) fn close_domain_picker(&mut self) {
        self.ime.clear_preedit();
        self.domain_picker.close();
    }

    pub(super) fn back_or_close_domain_picker(&mut self) {
        self.ime.clear_preedit();
        self.domain_picker.back_or_close();
    }

    fn discover_domains(&mut self, generation: u64, authority: crate::domain::DomainAuthority) {
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.handle_domain_picker_loaded(
                generation,
                Err("Domain discovery requires a native window".into()),
            );
            return;
        };
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            self.handle_domain_picker_loaded(
                generation,
                Err("Domain discovery event channel is unavailable".into()),
            );
            return;
        };
        let spawn = std::thread::Builder::new()
            .name("termeric-domain-discovery".into())
            .spawn(move || {
                let result = authority
                    .client()
                    .and_then(|client| client.request(Request::ListDomains))
                    .and_then(|response| match response {
                        Response::Domains { domains } => Ok(domains),
                        _ => anyhow::bail!("domain server returned an invalid list response"),
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = proxy.send_event(AppEvent::DomainPickerLoaded {
                    window_id,
                    generation,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.handle_domain_picker_loaded(
                generation,
                Err(format!("Could not start domain discovery: {error}")),
            );
        }
    }

    fn prepare_domain_window(&mut self, generation: u64, target: crate::domain::DomainTarget) {
        let Some(window_id) = self.window.as_ref().map(|window| window.id()) else {
            self.handle_domain_picker_prepared(
                generation,
                Err("Domain attachment requires a native window".into()),
            );
            return;
        };
        let Some(proxy) = EVENT_LOOP_PROXY.get().cloned() else {
            self.handle_domain_picker_prepared(
                generation,
                Err("Domain attachment event channel is unavailable".into()),
            );
            return;
        };
        let spawn = std::thread::Builder::new()
            .name("termeric-domain-prepare".into())
            .spawn(move || {
                let result = target
                    .authority
                    .client()
                    .and_then(|client| {
                        client.request(Request::Describe {
                            domain: target.domain_id,
                        })
                    })
                    .and_then(|response| match response {
                        Response::Domain { domain } => Ok(domain),
                        _ => anyhow::bail!("domain server returned an invalid describe response"),
                    })
                    .map_err(|error| format!("{error:#}"));
                let _ = proxy.send_event(AppEvent::DomainPickerPrepared {
                    window_id,
                    generation,
                    result,
                });
            });
        if let Err(error) = spawn {
            self.handle_domain_picker_prepared(
                generation,
                Err(format!("Could not prepare domain attachment: {error}")),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::command_palette::PaletteOwner;

    #[test]
    fn live_domain_commands_show_only_where_they_can_act() {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        // Nothing to cancel without an upload.
        assert!(
            app.unavailable_domain_commands()
                .contains(&"core.cancel-file-upload".to_owned())
        );
        // A saved SSH host is somewhere to attach, whatever runs locally.
        app.config.domains.ssh_hosts.insert(
            "build".into(),
            crate::config::SshHostProfile {
                destination: "build.example".into(),
                description: None,
            },
        );
        assert!(
            !app.unavailable_domain_commands()
                .contains(&"core.attach-domain".to_owned())
        );
    }

    #[test]
    fn domain_picker_stage_changes_retire_composition_without_external_discovery() {
        use winit::event::Ime;
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.command_palette.open();
        app.handle_ime_event(Ime::Preedit("old palette".into(), None));
        app.open_domain_picker();
        assert!(app.ime.preedit().is_none());
        app.handle_ime_event(Ime::Preedit("discovery".into(), None));
        let command = app.domain_picker.activate().unwrap();
        // Headless orchestration rejects before constructing a client or
        // worker, so this cannot connect to a daemon or saved SSH host.
        app.handle_domain_picker_command(command);
        assert!(app.ime.preedit().is_none());
        assert!(!app.domain_picker.can_filter());
        app.back_or_close_domain_picker();
        assert!(app.domain_picker.can_filter());
        app.handle_ime_event(Ime::Preedit("closing".into(), None));
        app.back_or_close_domain_picker();
        assert!(!app.domain_picker.is_active());
        assert!(app.ime.preedit().is_none());
        app.open_domain_picker();
        app.handle_ime_event(Ime::Preedit("closing again".into(), None));
        app.close_domain_picker();
        assert!(!app.domain_picker.is_active());
        assert!(app.ime.preedit().is_none());
        assert!(app.requested_window_launch.is_none());
    }

    #[test]
    fn prepared_picker_snapshot_becomes_an_explicit_window_launch() {
        let mut app = App::new_headless(crate::config::Config::default()).unwrap();
        app.domain_picker.open(&app.config.domains);
        let Some(DomainPickerCommand::Discover { generation, .. }) = app.domain_picker.activate()
        else {
            panic!("expected local discovery");
        };
        app.domain_picker.complete(
            generation,
            Ok(vec![crate::domain::model::DomainSummary {
                id: "domain-7".into(),
                name: "Build".into(),
                tabs: 1,
                panes: 1,
                attachments: 0,
                created_at: "2026-01-01T00:00:00Z".into(),
            }]),
        );
        let Some(DomainPickerCommand::PrepareWindow { generation, .. }) =
            app.domain_picker.activate()
        else {
            panic!("expected attachment preparation");
        };
        let snapshot = crate::domain::model::DomainSnapshot {
            id: "domain-7".into(),
            name: "Build".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            active_tab: 0,
            tabs: Vec::new(),
        };
        app.handle_domain_picker_prepared(generation, Ok(snapshot.clone()));
        let launch = app.requested_window_launch.take().unwrap();
        assert!(launch.force_new);
        assert_eq!(launch.domain.unwrap().domain_id, "domain-7");
        assert_eq!(launch.domain_snapshot, Some(snapshot));
    }
}
