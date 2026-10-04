//! Live-domain control requests that wait on the daemon: deferring them and
//! completing each with its topology result.

use super::*;

impl Application {
    pub(super) fn defer_live_domain_control(
        &mut self,
        request: &crate::control::protocol::Request,
        responder: ControlResponder,
    ) -> Option<ControlResponder> {
        use crate::control::protocol::Request;

        let Some(request_id) = request.request_id() else {
            return Some(responder);
        };
        if let Some(pending) = self
            .pending_domain_controls
            .iter_mut()
            .find(|pending| pending.request.request_id() == Some(request_id))
        {
            if pending.request != *request {
                let _ = responder.send(Err(
                    "GUI control request ID was already used for a different action".into(),
                ));
            } else if pending.responders.len() >= MAX_PENDING_DOMAIN_WAITERS {
                let _ = responder.send(Err(
                    "Too many clients are waiting for this live-domain control request".into(),
                ));
            } else {
                pending.responders.push(responder);
            }
            return None;
        }
        if let Some(replay) = self.control_replays.replay(request) {
            let _ = responder.send(replay);
            return None;
        }
        if self.control_launches.contains(request_id) {
            let _ = responder.send(Err("Launch request ID remains retained but its mutation replay expired; query launch-status or use a new request ID".into()));
            return None;
        }

        let (window_id, control, context) = match request {
            Request::CreateTab { window_id, .. } => {
                let Ok((native_id, window)) = self.live_control_window(*window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_create_tab() {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) | Err(_) => return Some(responder),
                }
            }
            Request::SplitPane {
                window_id,
                tab_id,
                pane_id,
                direction,
                ..
            } => {
                let Ok((native_id, window)) = self.live_control_window(*window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_split(*tab_id, *pane_id, *direction) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) | Err(_) => return Some(responder),
                }
            }
            Request::CloseTab {
                window_id, tab_id, ..
            } => {
                let Ok((native_id, window)) = self.live_control_window(*window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_close_tab(*tab_id) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::ClosePane {
                window_id,
                tab_id,
                pane_id,
                ..
            } => {
                let Ok((native_id, window)) = self.live_control_window(*window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_close_pane(*tab_id, *pane_id) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::ResizeDivider {
                window_id,
                tab_id,
                divider_id,
                ratio_millis,
                ..
            } => {
                let Ok((native_id, window)) = self.live_control_window(*window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_resize_divider(*tab_id, *divider_id, *ratio_millis)
                {
                    Ok(Some((control, previous_ratio_millis))) => (
                        native_id,
                        control,
                        PendingDomainContext::ResizeDivider {
                            previous_ratio_millis,
                        },
                    ),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::MoveTab {
                source_window_id,
                tab_id,
                destination_window_id,
                destination_index,
                ..
            } if source_window_id == destination_window_id => {
                let Ok((native_id, window)) = self.live_control_window(*source_window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_move_tab(*tab_id, *destination_index) {
                    Ok(Some((control, changed))) => (
                        native_id,
                        control,
                        PendingDomainContext::MoveTab { changed },
                    ),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            Request::MovePane {
                source_window_id,
                source_tab_id,
                pane_id,
                destination_window_id,
                destination_tab_id,
                target_pane_id,
                direction,
                ..
            } if source_window_id == destination_window_id => {
                let Ok((native_id, window)) = self.live_control_window(*source_window_id) else {
                    return Some(responder);
                };
                match window.control_live_domain_move_pane(
                    *source_tab_id,
                    *pane_id,
                    *destination_tab_id,
                    *target_pane_id,
                    *direction,
                ) {
                    Ok(Some(control)) => (native_id, control, PendingDomainContext::None),
                    Ok(None) => return Some(responder),
                    Err(error) => {
                        let result = Err(bounded_control_error(&error));
                        self.control_replays.record(request.clone(), result.clone());
                        let _ = responder.send(result);
                        return None;
                    }
                }
            }
            _ => return Some(responder),
        };
        if self.pending_domain_controls.len() >= MAX_PENDING_DOMAIN_CONTROLS {
            let result = Err("Too many live-domain control requests are pending".into());
            self.control_replays.record(request.clone(), result.clone());
            let _ = responder.send(result);
            return None;
        }
        let result = self
            .windows
            .get_mut(&window_id)
            .expect("resolved live control window remains present")
            .start_live_domain_control(control.clone());
        if let Err(error) = result {
            let result = Err(bounded_control_error(&error));
            self.control_replays.record(request.clone(), result.clone());
            let _ = responder.send(result);
            return None;
        }
        self.pending_domain_controls.push(PendingDomainControl {
            request: request.clone(),
            window_id,
            control,
            context,
            responders: vec![responder],
        });
        None
    }

    pub(super) fn complete_live_domain_control(
        &mut self,
        window_id: WindowId,
        control: &crate::app::LiveDomainControl,
        result: &std::result::Result<crate::domain::model::DomainSnapshot, String>,
    ) {
        use crate::control::protocol::{Request, Response};

        let Some(index) = self
            .pending_domain_controls
            .iter()
            .position(|pending| pending.window_id == window_id && pending.control == *control)
        else {
            return;
        };
        let pending = self.pending_domain_controls.remove(index);
        let response = match result {
            Err(error) => Err(bounded_control_error(error)),
            Ok(snapshot) => match &pending.request {
                Request::CreateTab {
                    request_id,
                    window_id: control_window_id,
                } => self
                    .windows
                    .get(&window_id)
                    .and_then(|window| {
                        window
                            .control_local_live_tab_id(&snapshot.id, snapshot.active_tab)
                    })
                    .map(|tab_id| Response::TabCreated {
                        process_id: std::process::id(),
                        request_id: request_id.clone(),
                        window_id: *control_window_id,
                        tab_id,
                    })
                    .ok_or_else(|| {
                        "Live-domain tab was created but its GUI stable ID is unavailable".into()
                    }),
                Request::SplitPane {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                    ..
                } => snapshot
                    .tabs
                    .iter()
                    .find(|tab| tab.id == snapshot.active_tab)
                    .and_then(|tab| {
                        self.windows.get(&window_id).and_then(|window| {
                            window.control_local_live_pane_id(
                                &snapshot.id,
                                tab.id,
                                tab.focused_pane,
                            )
                        })
                    })
                    .map(|pane_id| Response::PaneCreated {
                        process_id: std::process::id(),
                        request_id: request_id.clone(),
                        window_id: *control_window_id,
                        tab_id: *tab_id,
                        pane_id,
                    })
                    .ok_or_else(|| {
                        "Live-domain pane was created but its GUI stable ID is unavailable".into()
                    }),
                Request::CloseTab {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                } => match control {
                    crate::app::LiveDomainControl::CloseTab { remote_tab_id, .. }
                        if !snapshot
                            .tabs
                            .iter()
                            .any(|tab| tab.id == *remote_tab_id) =>
                    {
                        Ok(Response::TabCloseRequested {
                            process_id: std::process::id(),
                            request_id: request_id.clone(),
                            window_id: *control_window_id,
                            tab_id: *tab_id,
                        })
                    }
                    crate::app::LiveDomainControl::CloseTab { .. } => Err(
                        "Live-domain tab closure returned a snapshot that still contains the tab"
                            .into(),
                    ),
                    _ => Err("Pending live-domain tab closure lost its exact target".into()),
                },
                Request::ClosePane {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                    pane_id,
                } => match control {
                    crate::app::LiveDomainControl::ClosePane { remote_pane_id, .. }
                        if !snapshot
                            .tabs
                            .iter()
                            .any(|tab| tab.panes.iter().any(|pane| pane.id == *remote_pane_id)) =>
                    {
                        self.windows
                            .get(&window_id)
                            .ok_or_else(|| {
                                "GUI window disappeared after live-domain pane closure".into()
                            })
                            .and_then(|window| window.control_live_pane_status(*tab_id))
                            .map(|(remaining_panes, focused_pane_id)| Response::PaneClosed {
                                process_id: std::process::id(),
                                request_id: request_id.clone(),
                                window_id: *control_window_id,
                                tab_id: *tab_id,
                                pane_id: *pane_id,
                                remaining_panes,
                                focused_pane_id,
                            })
                    }
                    crate::app::LiveDomainControl::ClosePane { .. } => Err(
                        "Live-domain pane closure returned a snapshot that still contains the pane"
                            .into(),
                    ),
                    _ => Err("Pending live-domain pane closure lost its exact target".into()),
                },
                Request::ResizeDivider {
                    request_id,
                    window_id: control_window_id,
                    tab_id,
                    divider_id,
                    ratio_millis,
                } => match pending.context {
                    PendingDomainContext::ResizeDivider {
                        previous_ratio_millis,
                    } => self
                        .windows
                        .get(&window_id)
                        .ok_or_else(|| {
                            "GUI window disappeared after live-domain divider resize".into()
                        })
                        .and_then(|window| {
                            window.control_live_divider_ratio(*tab_id, *divider_id)
                        })
                        .and_then(|authoritative_ratio| {
                            if authoritative_ratio != *ratio_millis {
                                return Err(format!(
                                    "Live-domain divider resize returned ratio {authoritative_ratio}‰ instead of {ratio_millis}‰"
                                ));
                            }
                            Ok(Response::DividerResized {
                                process_id: std::process::id(),
                                request_id: request_id.clone(),
                                window_id: *control_window_id,
                                tab_id: *tab_id,
                                divider_id: *divider_id,
                                previous_ratio_millis,
                                ratio_millis: *ratio_millis,
                                changed: previous_ratio_millis != *ratio_millis,
                            })
                        }),
                    PendingDomainContext::None | PendingDomainContext::MoveTab { .. } => {
                        Err("Pending live-domain divider resize lost its previous ratio".into())
                    }
                },
                Request::MoveTab {
                    request_id,
                    source_window_id,
                    tab_id,
                    destination_window_id,
                    destination_index,
                } => match pending.context {
                    PendingDomainContext::MoveTab { changed } => self
                        .windows
                        .get(&window_id)
                        .ok_or_else(|| {
                            "GUI window disappeared after live-domain tab movement".into()
                        })
                        .and_then(|window| window.control_tab_visual_index(*tab_id))
                        .and_then(|authoritative_index| {
                            if authoritative_index != *destination_index {
                                return Err(format!(
                                    "Live-domain tab movement reconciled at position {authoritative_index} instead of {destination_index}"
                                ));
                            }
                            Ok(Response::TabMoved {
                                process_id: std::process::id(),
                                request_id: request_id.clone(),
                                source_window_id: *source_window_id,
                                source_tab_id: *tab_id,
                                destination_window_id: *destination_window_id,
                                destination_tab_id: *tab_id,
                                destination_index: *destination_index,
                                source_window_closing: false,
                                changed,
                            })
                        }),
                    PendingDomainContext::None
                    | PendingDomainContext::ResizeDivider { .. } => {
                        Err("Pending live-domain tab movement lost its source position".into())
                    }
                },
                Request::MovePane {
                    request_id,
                    source_window_id,
                    source_tab_id: source_local_tab_id,
                    pane_id,
                    destination_window_id,
                    destination_tab_id: destination_local_tab_id,
                    target_pane_id: target_local_pane_id,
                    ..
                } => match control {
                    crate::app::LiveDomainControl::MovePane {
                        domain_id,
                        remote_pane_id,
                        source_tab_id: source_remote_tab_id,
                        destination_tab_id: destination_remote_tab_id,
                        target_pane_id: target_remote_pane_id,
                        direction,
                    } => {
                        let source = snapshot
                            .tabs
                            .iter()
                            .find(|tab| tab.id == *source_remote_tab_id);
                        let destination = snapshot
                            .tabs
                            .iter()
                            .find(|tab| tab.id == *destination_remote_tab_id);
                        if source.is_none_or(|tab| {
                            tab.panes.iter().any(|pane| pane.id == *remote_pane_id)
                        }) {
                            Err("Live-domain pane movement did not remove the source route".into())
                        } else if destination.is_none_or(|tab| {
                            !tab.panes.iter().any(|pane| pane.id == *remote_pane_id)
                                || !tab
                                    .panes
                                    .iter()
                                    .any(|pane| pane.id == *target_remote_pane_id)
                                || !live_layout_has_exact_move_split(
                                    &tab.layout,
                                    *target_remote_pane_id,
                                    *remote_pane_id,
                                    *direction,
                                )
                        }) {
                            Err(
                                "Live-domain pane movement did not preserve the exact destination anchor and direction"
                                    .into(),
                            )
                        } else {
                            self.windows
                                .get(&window_id)
                                .ok_or_else(|| {
                                    "GUI window disappeared after live-domain pane movement".into()
                                })
                                .and_then(|window| {
                                    let target = window.control_local_live_pane_id(
                                        domain_id,
                                        *destination_remote_tab_id,
                                        *target_remote_pane_id,
                                    );
                                    if target != Some(*target_local_pane_id) {
                                        return Err(
                                            "Live-domain pane movement changed or lost the exact destination anchor"
                                                .into(),
                                        );
                                    }
                                    window
                                        .control_local_live_pane_id(
                                            domain_id,
                                            *destination_remote_tab_id,
                                            *remote_pane_id,
                                        )
                                        .ok_or_else(|| {
                                            "Moved live-domain pane has no destination-local stable ID"
                                                .into()
                                        })
                                })
                                .map(|destination_pane_id| Response::PaneMoved {
                                    process_id: std::process::id(),
                                    request_id: request_id.clone(),
                                    source_window_id: *source_window_id,
                                    source_tab_id: *source_local_tab_id,
                                    source_pane_id: *pane_id,
                                    destination_window_id: *destination_window_id,
                                    destination_tab_id: *destination_local_tab_id,
                                    destination_pane_id,
                                    target_pane_id: *target_local_pane_id,
                                })
                        }
                    }
                    _ => Err("Pending live-domain pane movement lost its exact route".into()),
                },
                _ => Err("Pending live-domain control request has an invalid operation".into()),
            },
        };
        self.control_replays
            .record_terminal(pending.request, response.clone());
        for responder in pending.responders {
            let _ = responder.send(response.clone());
        }
    }
}
