//! Synthetic plugin hooks launch only raw local shell fixtures, never providers.
use super::*;
use crate::plugin::{Hook, PluginAction};

type RestartObservations = std::sync::Arc<std::sync::Mutex<Vec<(PluginContext, Option<String>)>>>;

#[derive(Debug)]
struct RestartFixture {
    directory: std::path::PathBuf,
    mode: &'static str,
    seen: RestartObservations,
}

impl Plugin for RestartFixture {
    fn id(&self) -> &str {
        "synthetic-capture"
    }
    fn name(&self) -> &str {
        "Local restart fixture"
    }
    fn on_hook(&mut self, hook: &Hook, ctx: &PluginContext) -> Option<PluginAction> {
        let profile = match hook {
            Hook::WorkspaceRestore { profile, .. } => profile.clone(),
            Hook::CommandInvoked { command_id } if command_id == "synthetic-capture.open" => None,
            _ => return None,
        };
        self.seen
            .lock()
            .unwrap()
            .push((ctx.clone(), profile.clone()));
        if self.mode == "configuration" {
            return Some(PluginAction::ShowNotice {
                title: "Synthetic configuration error".into(),
                lines: vec!["No launch".into()],
            });
        }
        let launch = PluginAction::OpenTab {
            program: if self.mode == "missing-binary" {
                self.directory.join("does-not-exist").display().to_string()
            } else {
                "/bin/sh".into()
            },
            args: vec![
                "-c".into(),
                "stty raw -echo; printf READY; cat > \"$CAPTURE_DIR/$$\"".into(),
            ],
            cwd: if self.mode == "bad-cwd" {
                Some(self.directory.join("missing-cwd").display().to_string())
            } else if self.mode == "wrong-existing-cwd" {
                Some("/".into())
            } else {
                ctx.cwd.clone()
            },
            title: Some("replacement fixture".into()),
            plugin_id: Some(
                if self.mode == "wrong-owner" {
                    "different-plugin"
                } else {
                    "synthetic-capture"
                }
                .into(),
            ),
            profile: if self.mode == "wrong-profile" {
                Some("different-profile".into())
            } else {
                profile
            },
            env: HashMap::from([("CAPTURE_DIR".into(), self.directory.display().to_string())]),
        };
        match self.mode {
            "unsupported" => Some(PluginAction::ShowMessage {
                message: "not a launch".into(),
            }),
            _ => Some(launch),
        }
    }
}

fn register_restart_fixture(
    app: &mut App,
    directory: &std::path::Path,
    mode: &'static str,
) -> RestartObservations {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    app.plugin_manager
        .register(Box::new(RestartFixture {
            directory: directory.to_path_buf(),
            mode,
            seen: seen.clone(),
        }))
        .unwrap();
    seen
}

#[test]
fn failed_restart_preserves_old_owners_layout_and_focus() {
    for mode in [
        "configuration",
        "missing-binary",
        "bad-cwd",
        "wrong-existing-cwd",
        "wrong-owner",
        "wrong-profile",
        "unsupported",
        "read-only",
        "live-domain",
    ] {
        let (mut app, directory, entry, captures) = harvest_rerun_capture_fixture();
        mark_capture_plugin_owner(&mut app, PaneId(0), "synthetic-capture");
        let seen = register_restart_fixture(&mut app, directory.path(), mode);
        let tab = app.tabs.as_mut().unwrap().active_tab_mut().unwrap();
        tab.set_plugin_profile_reference(
            PaneId(0),
            "synthetic-capture",
            Some("saved-profile".into()),
        );
        if mode == "read-only" {
            tab.pane_manager
                .as_mut()
                .unwrap()
                .get_pane_mut(PaneId(0))
                .unwrap()
                .read_only = true;
        }
        if mode == "live-domain" {
            tab.live_domain = Some(crate::tabs::LiveDomainTab {
                domain_id: "synthetic-not-connected".into(),
                tab_id: 1,
                pane_ids: HashMap::new(),
            });
        }
        let focused = app.active_harvest_source();
        app.restart_plugin("synthetic-capture");
        let tabs = app.tabs.as_ref().unwrap();
        assert_eq!(tabs.count(), 1, "{mode}");
        let tab = tabs.tab_by_id(entry.source.tab_id).unwrap();
        assert!(!tab.is_closing(), "{mode}");
        assert_eq!(tab.pane_manager.as_ref().unwrap().pane_count(), 3, "{mode}");
        assert_eq!(app.active_harvest_source(), focused, "{mode}");
        assert_eq!(
            seen.lock().unwrap().len(),
            usize::from(!matches!(mode, "read-only" | "live-domain")),
            "{mode}"
        );
        assert!(
            fenced_harvest_capture(&mut app, &captures)
                .iter()
                .all(Vec::is_empty),
            "{mode}"
        );
    }
}

#[test]
fn successful_restart_retires_only_source_and_preserves_cwd_profile_and_siblings() {
    for mode in [
        "single",
        "mixed-shell-tab",
        "mixed-plugin-tab",
        "nonzero-single",
        "multiple-plugins",
    ] {
        let split = mode != "single";
        let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(split);
        let source = if mode == "nonzero-single" {
            PaneId(2)
        } else {
            PaneId(0)
        };
        mark_capture_plugin_owner(&mut app, source, "synthetic-capture");
        if mode == "multiple-plugins" {
            mark_capture_plugin_owner(&mut app, PaneId(1), "synthetic-capture");
        }
        let source_cwd = tempfile::tempdir().unwrap();
        let sibling_cwd = tempfile::tempdir().unwrap();
        {
            let tabs = app.tabs.as_mut().unwrap();
            let origin = tabs.origin_for_pane(entry.source.tab_id, source).unwrap();
            tabs.terminal_for_origin_mut(origin).unwrap().process(
                format!("\x1b]7;file://localhost{}\x07", source_cwd.path().display()).as_bytes(),
            );
            let tab = tabs.active_tab_mut().unwrap();
            tab.set_plugin_profile_reference(
                source,
                "synthetic-capture",
                Some("saved-profile".into()),
            );
            if mode == "mixed-plugin-tab" {
                tab.tab_type = crate::tabs::TabType::Plugin {
                    plugin_id: "synthetic-capture".into(),
                };
            }
            if let Some(panes) = &mut tab.pane_manager {
                for id in panes.pane_ids().into_iter().filter(|id| *id != source) {
                    panes.get_pane_mut(id).unwrap().terminal.process(
                        format!(
                            "\x1b]7;file://localhost{}\x07",
                            sibling_cwd.path().display()
                        )
                        .as_bytes(),
                    );
                }
            }
        }
        if mode == "nonzero-single" {
            app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 0)
                .unwrap();
            app.close_pane_for(Requester::Control, entry.source.tab_id as u64, 1)
                .unwrap();
        }
        let captures = current_harvest_capture_routes(&mut app, directory.path());
        let surviving = captures
            .into_iter()
            .filter(|(origin, _)| origin.pane_id != source)
            .collect::<Vec<_>>();
        let seen = register_restart_fixture(&mut app, directory.path(), "success");
        app.restart_plugin("synthetic-capture");
        let replacement = app.tabs.as_ref().unwrap().active_tab_id().unwrap();
        assert_ne!(replacement, entry.source.tab_id);
        let tabs = app.tabs.as_ref().unwrap();
        let old = tabs.tab_by_id(entry.source.tab_id).unwrap();
        if surviving.is_empty() {
            assert!(old.is_closing(), "{mode}");
        } else {
            assert!(!old.is_closing(), "{mode}");
            let panes = old.pane_manager.as_ref().unwrap();
            assert_eq!(panes.pane_count(), surviving.len(), "{mode}");
            assert!(panes.get_pane(source).is_none());
            for (origin, path) in &surviving {
                assert_eq!(
                    path.file_name().unwrap().to_string_lossy(),
                    panes
                        .get_pane(origin.pane_id)
                        .unwrap()
                        .pty
                        .child_id()
                        .unwrap()
                        .to_string()
                );
            }
        }
        let tab = tabs.active_tab().unwrap();
        assert_eq!(
            tab.pane_plugin_launch(tab.single_pane_id()),
            Some(("synthetic-capture", Some("saved-profile")))
        );
        assert_eq!(
            tab.pane_working_directory(tab.single_pane_id()).as_deref(),
            source_cwd.path().to_str()
        );
        let observations = seen.lock().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].0.cwd.as_deref(), source_cwd.path().to_str());
        assert_eq!(observations[0].1.as_deref(), Some("saved-profile"));
        drop(observations);
        let mut all_captures = surviving;
        all_captures.extend(current_harvest_capture_routes(&mut app, directory.path()));
        assert!(
            fenced_harvest_capture(&mut app, &all_captures)
                .iter()
                .all(Vec::is_empty)
        );
    }
}

#[test]
fn restart_without_existing_owner_opens_one_fresh_terminal() {
    let (mut app, directory, entry, _) = harvest_rerun_capture_fixture_in(false);
    register_restart_fixture(&mut app, directory.path(), "success");
    app.restart_plugin("synthetic-capture");
    assert_eq!(app.tabs.as_ref().unwrap().count(), 2);
    assert!(
        !app.tabs
            .as_ref()
            .unwrap()
            .tab_by_id(entry.source.tab_id)
            .unwrap()
            .is_closing()
    );
    assert_ne!(
        app.tabs.as_ref().unwrap().active_tab_id(),
        Some(entry.source.tab_id)
    );
    let captures = current_harvest_capture_routes(&mut app, directory.path());
    assert!(
        fenced_harvest_capture(&mut app, &captures)
            .iter()
            .all(Vec::is_empty)
    );
}
