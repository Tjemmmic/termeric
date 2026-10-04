//! Shared local daemon state without PTYs or external processes.
use super::*;

pub(super) const DOMAIN: &str = "11111111111111111111111111111111";

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
    Arc::new(Mutex::new(ServerState {
        domains: HashMap::from([(DOMAIN.into(), domain)]),
        allow_screen_checksum_reports: false,
        xterm_keyboard_type: XtermKeyboardType::Normal,
        shutdown: Arc::new(AtomicBool::new(false)),
        wake_socket: None,
        next_attachment: AtomicU64::new(1),
        next_topology_subscription: AtomicU64::new(1),
        active_uploads: 0,
        upload_ids_in_flight: HashMap::new(),
        upload_records: HashMap::new(),
        upload_janitor_running: false,
    }))
}
