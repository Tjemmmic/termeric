//! Shared local daemon state without PTYs or external processes.
use super::*;

pub(super) const DOMAIN: &str = "11111111111111111111111111111111";

/// A daemon's state with no domains, as it starts.
pub(super) fn empty_state() -> SharedState {
    Arc::new(Mutex::new(ServerState::new(
        false,
        XtermKeyboardType::Normal,
        None,
    )))
}

/// A daemon's state with one domain, [`DOMAIN`], that has no tabs.
pub(super) fn state() -> SharedState {
    let domain = DomainRuntime {
        id: DOMAIN.into(),
        name: "topology-fixture".into(),
        created_at: String::new(),
        active_tab: 0,
        next_tab: 0,
        next_pane: 0,
        next_split: 0,
        topology_revision: 1,
        topology_subscribers: Vec::new(),
        tabs: Vec::new(),
    };
    let state = empty_state();
    state.lock().unwrap().domains.insert(DOMAIN.into(), domain);
    state
}
