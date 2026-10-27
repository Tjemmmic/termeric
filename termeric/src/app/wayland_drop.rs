//! Files dropped on a window under Wayland, where winit 0.30 reports no
//! drops (only X11 has `WindowEvent::DroppedFile`). Termeric takes the
//! compositor's drag-and-drop offers itself, on winit's own connection (the
//! windows' surfaces belong to it) and on a thread of its own: over a
//! Termeric window it accepts a list of files, and the files dropped there
//! reach that window as `WindowResult::FilesDropped`.

use super::{AppEvent, WindowKey, WindowResult};
use crate::deadline_io::{Deadline, PollReader};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::collections::HashMap;
use std::io::Read as _;
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use winit::event_loop::EventLoopProxy;

/// The one type a drop is taken as: files, as `file://` URIs.
const URI_LIST: &str = "text/uri-list";
/// A list longer than this is refused rather than read on.
const MAX_URI_LIST_BYTES: u64 = 1024 * 1024;
/// How long the program the files come from has to send their list.
const URI_LIST_DEADLINE: Duration = Duration::from_secs(2);

/// The surfaces of this process's windows, by the address of their
/// `wl_surface`, so a drag over one is known to be over that window.
type Surfaces = Arc<Mutex<HashMap<usize, WindowKey>>>;

/// The running listener. Dropping it stops the thread and waits for it,
/// which must happen before winit closes its connection (in
/// `ApplicationHandler::exiting`): the thread reads from that connection.
pub(super) struct WaylandDrop {
    surfaces: Surfaces,
    /// Closing it wakes the thread to stop.
    stop: Option<std::io::PipeWriter>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WaylandDrop {
    /// Start listening when the event loop runs on Wayland; `None` on X11,
    /// where winit reports drops itself.
    pub(super) fn start(
        event_loop: &impl HasDisplayHandle,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = event_loop.display_handle().ok()?.as_raw() else {
            return None;
        };
        // Safety: winit's connection outlives this backend, which the thread
        // alone holds and drops before `Drop` returns, and this listener is
        // dropped before the event loop ends (see the type's documentation).
        let backend = unsafe {
            wayland_backend::client::Backend::from_foreign_display(display.display.as_ptr().cast())
        };
        let connection = Connection::from_backend(backend);
        let (stop_reader, stop) = match std::io::pipe() {
            Ok(pipe) => pipe,
            Err(error) => {
                log::warn!("Files can't be dropped on windows: {error}");
                return None;
            }
        };
        let surfaces = Surfaces::default();
        let listener_surfaces = surfaces.clone();
        let thread = std::thread::Builder::new()
            .name("termeric-wayland-drop".into())
            .spawn(move || {
                if let Err(error) = listen(&connection, listener_surfaces, proxy, &stop_reader) {
                    log::warn!("Files can no longer be dropped on windows: {error}");
                }
            });
        match thread {
            Ok(thread) => Some(Self {
                surfaces,
                stop: Some(stop),
                thread: Some(thread),
            }),
            Err(error) => {
                log::warn!("Files can't be dropped on windows: {error}");
                None
            }
        }
    }

    /// Take drops over `window` for the window `key` names.
    pub(super) fn register(&self, window: &winit::window::Window, key: WindowKey) {
        if let Some(surface) = surface_address(window) {
            lock(&self.surfaces).insert(surface, key);
        }
    }

    /// Stop taking drops for the window `key` names, before its surface
    /// goes and its address can be another's.
    pub(super) fn unregister(&self, key: WindowKey) {
        lock(&self.surfaces).retain(|_, registered| *registered != key);
    }
}

impl Drop for WaylandDrop {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("The Wayland drop listener panicked");
        }
    }
}

fn surface_address(window: &winit::window::Window) -> Option<usize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Wayland(handle) => Some(handle.surface.as_ptr() as usize),
        _ => None,
    }
}

fn lock(surfaces: &Surfaces) -> std::sync::MutexGuard<'_, HashMap<usize, WindowKey>> {
    surfaces
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Dispatch drag-and-drop events until `stop` closes or the connection
/// fails.
fn listen(
    connection: &Connection,
    surfaces: Surfaces,
    proxy: EventLoopProxy<AppEvent>,
    stop: &std::io::PipeReader,
) -> Result<(), String> {
    let (globals, mut queue) = registry_queue_init::<Listener>(connection)
        .map_err(|error| format!("could not list the compositor's globals: {error}"))?;
    let handle = queue.handle();
    let manager = globals
        .bind::<WlDataDeviceManager, _, _>(&handle, 1..=3, ())
        .map_err(|error| format!("the compositor has no drag and drop: {error}"))?;
    log::debug!(
        "Taking files dropped on windows (wl_data_device_manager version {})",
        manager.version()
    );
    let mut listener = Listener {
        connection: connection.clone(),
        surfaces,
        proxy,
        manager,
        devices: HashMap::new(),
        drag: None,
    };
    for global in globals.contents().clone_list() {
        if global.interface == WlSeat::interface().name {
            listener.add_seat(globals.registry(), global.name, &handle);
        }
    }
    loop {
        dispatch(&mut queue, &mut listener)?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        if wait(guard.connection_fd(), stop)? == Ready::Stop {
            return Ok(());
        }
        match guard.read() {
            Ok(_) => {}
            Err(wayland_backend::client::WaylandError::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("the connection failed: {error}")),
        }
    }
}

/// Handle what has arrived and send what that asked.
fn dispatch(queue: &mut EventQueue<Listener>, listener: &mut Listener) -> Result<(), String> {
    match queue.dispatch_pending(listener) {
        Ok(_) => {}
        // A message that didn't parse is lost; the next can still be read.
        Err(wayland_client::DispatchError::BadMessage {
            sender_id,
            interface,
            opcode,
        }) => log::debug!("Skipped a malformed {interface} event {opcode} from {sender_id}"),
        Err(error) => return Err(format!("the connection failed: {error}")),
    }
    match queue.flush() {
        Ok(()) => Ok(()),
        Err(wayland_backend::client::WaylandError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(())
        }
        Err(error) => Err(format!("the connection failed: {error}")),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Ready {
    Events,
    Stop,
}

/// Wait until the compositor sends something or `stop` closes.
fn wait(connection: BorrowedFd<'_>, stop: &std::io::PipeReader) -> Result<Ready, String> {
    let mut descriptors = [
        libc::pollfd {
            fd: connection.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: stop.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // Safety: both descriptors stay open for the call.
        let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, -1) };
        if ready >= 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("could not wait for the compositor: {error}"));
        }
    }
    if descriptors[1].revents != 0 {
        Ok(Ready::Stop)
    } else if descriptors[0].revents & libc::POLLIN != 0 {
        Ok(Ready::Events)
    } else {
        Err("the compositor closed the connection".into())
    }
}

struct Listener {
    connection: Connection,
    surfaces: Surfaces,
    proxy: EventLoopProxy<AppEvent>,
    manager: WlDataDeviceManager,
    /// Each seat's data device, by the seat's registry name.
    devices: HashMap<u32, WlDataDevice>,
    /// The drag over one of this process's surfaces, from its enter to its
    /// leave or drop.
    drag: Option<Drag>,
}

struct Drag {
    offer: WlDataOffer,
    /// The window the files would be dropped on, when the drag offers files
    /// and is over a Termeric window.
    window: Option<WindowKey>,
}

/// What a drag's source offered, as its events arrive.
#[derive(Default)]
struct OfferData {
    /// The source offers `text/uri-list`.
    files: AtomicBool,
    /// The compositor settled on copying, which a drop needs from version 3.
    copy: AtomicBool,
}

impl Listener {
    fn add_seat(&mut self, registry: &WlRegistry, name: u32, handle: &QueueHandle<Self>) {
        let seat = registry.bind::<WlSeat, _, _>(name, 1, handle, ());
        let device = self.manager.get_data_device(&seat, handle, ());
        self.devices.insert(name, device);
    }

    fn remove_seat(&mut self, name: u32) {
        if let Some(device) = self.devices.remove(&name)
            && device.version() >= 2
        {
            device.release();
        }
    }

    fn enter(&mut self, serial: u32, surface: &impl Proxy, offer: Option<WlDataOffer>) {
        self.end_drag();
        let Some(offer) = offer else {
            return;
        };
        let offers_files = offer
            .data::<OfferData>()
            .is_some_and(|data| data.files.load(Ordering::Acquire));
        let window = offers_files
            .then(|| {
                lock(&self.surfaces)
                    .get(&(surface.id().as_ptr() as usize))
                    .copied()
            })
            .flatten();
        offer.accept(serial, window.map(|_| URI_LIST.to_string()));
        if offer.version() >= 3 {
            let actions = if window.is_some() {
                DndAction::Copy
            } else {
                DndAction::empty()
            };
            offer.set_actions(actions, actions);
        }
        self.drag = Some(Drag { offer, window });
    }

    fn end_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            drag.offer.destroy();
        }
    }

    /// Read the dropped files' list and hand it to the window it was
    /// dropped on.
    fn drop_files(&mut self) {
        let Some(Drag { offer, window }) = self.drag.take() else {
            return;
        };
        let copying = offer.version() < 3
            || offer
                .data::<OfferData>()
                .is_some_and(|data| data.copy.load(Ordering::Acquire));
        let Some(window) = window.filter(|_| copying) else {
            offer.destroy();
            return;
        };
        let result = self.receive(&offer);
        if offer.version() >= 3 {
            offer.finish();
        }
        offer.destroy();
        let _ = self.proxy.send_event(AppEvent::WindowResult {
            window,
            result: WindowResult::FilesDropped(result),
        });
    }

    /// The files the source sent for `offer`, read here: the list is short,
    /// and nothing else this thread handles waits meanwhile.
    fn receive(&self, offer: &WlDataOffer) -> Result<Vec<PathBuf>, String> {
        let (mut reader, writer) =
            std::io::pipe().map_err(|error| format!("could not ask for the files: {error}"))?;
        offer.receive(URI_LIST.to_string(), writer.as_fd());
        drop(writer);
        self.connection
            .flush()
            .map_err(|error| format!("could not ask for the files: {error}"))?;
        let mut list = Vec::new();
        PollReader::new(
            &mut reader,
            Deadline::within(URI_LIST_DEADLINE),
            "the program the files came from didn't send them in time",
        )
        .take(MAX_URI_LIST_BYTES + 1)
        .read_to_end(&mut list)
        .map_err(|error| format!("could not read the files: {error}"))?;
        if list.len() as u64 > MAX_URI_LIST_BYTES {
            return Err("too many files were dropped at once".into());
        }
        local_files(&list)
    }
}

/// The local files a `text/uri-list` (RFC 2483) names, in order: one URI a
/// line, `#` starting a comment.
fn local_files(list: &[u8]) -> Result<Vec<PathBuf>, String> {
    let text = std::str::from_utf8(list).map_err(|_| "the list of files wasn't text")?;
    let uris: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let files: Vec<PathBuf> = uris
        .iter()
        .filter_map(|uri| crate::portal::local_path(uri))
        .collect();
    if !files.is_empty() {
        Ok(files)
    } else if uris.is_empty() {
        Err("no files were dropped".into())
    } else {
        Err("only files on this computer can be dropped".into())
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Listener {
    fn event(
        listener: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name, interface, ..
            } if interface == WlSeat::interface().name => {
                listener.add_seat(registry, name, handle);
            }
            wl_registry::Event::GlobalRemove { name } => listener.remove_seat(name),
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for Listener {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDeviceManager, ()> for Listener {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for Listener {
    fn event(
        listener: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Enter {
                serial,
                surface,
                id,
                ..
            } => listener.enter(serial, &surface, id),
            wl_data_device::Event::Leave => listener.end_drag(),
            wl_data_device::Event::Drop => listener.drop_files(),
            // The clipboard's offers are arboard's business, on its own
            // connection.
            wl_data_device::Event::Selection { id: Some(offer) } => offer.destroy(),
            _ => {}
        }
    }

    wayland_client::event_created_child!(Listener, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferData::default()),
    ]);
}

impl Dispatch<WlDataOffer, OfferData> for Listener {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        data: &OfferData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } if mime_type == URI_LIST => {
                data.files.store(true, Ordering::Release);
            }
            wl_data_offer::Event::Action {
                dnd_action: WEnum::Value(action),
            } => data
                .copy
                .store(action == DndAction::Copy, Ordering::Release),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_list_names_its_local_files_in_order() {
        let list = b"# from a file manager\r\nfile:///home/me/notes.txt\r\nfile://localhost/tmp/a%20b\r\n\r\n";
        assert_eq!(
            local_files(list),
            Ok(vec![
                PathBuf::from("/home/me/notes.txt"),
                PathBuf::from("/tmp/a b")
            ])
        );
    }

    #[test]
    fn files_elsewhere_are_left_out_and_a_list_of_only_those_is_refused() {
        assert_eq!(
            local_files(b"https://example.com/a\nfile:///tmp/kept\n"),
            Ok(vec![PathBuf::from("/tmp/kept")])
        );
        assert_eq!(
            local_files(b"file://otherhost/tmp/x\r\nhttps://example.com/a\r\n"),
            Err("only files on this computer can be dropped".into())
        );
        assert_eq!(
            local_files(b"# nothing\r\n"),
            Err("no files were dropped".into())
        );
        assert_eq!(
            local_files(b"file:///tmp/\xff"),
            Err("the list of files wasn't text".into())
        );
    }
}
