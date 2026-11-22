use servo::{ClipboardDelegate, StringRequest, WebView};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use winit::raw_window_handle::RawDisplayHandle;

const BRIDGE_SCRIPT: &str = r#"(() => {
  if (window.top !== window || window.soundscapeDesktopCopy) return;
  const prefix = NONCE + ":copy:";
  window.soundscapeDesktopCopy = (text) => {
    console.debug(prefix + JSON.stringify(String(text ?? "")));
    return true;
  };
})();"#;

const TEXT_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

const READ_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];

const MAX_READ_BYTES: u64 = 16 * 1024 * 1024;
const MIN_MANAGER_RELEASE: u32 = 4;

pub fn page_script(nonce: &str) -> String {
    BRIDGE_SCRIPT.replacen(
        "NONCE",
        &serde_json::to_string(nonce).unwrap_or_default(),
        1,
    )
}

pub fn copied(nonce: &str, message: &str) -> Option<String> {
    let rest = message.strip_prefix(nonce)?.strip_prefix(":copy:")?;
    serde_json::from_str::<String>(rest).ok()
}

pub fn pick_text_mime(offered: &[String]) -> Option<&str> {
    for preferred in READ_MIMES {
        if let Some(found) = offered.iter().find(|mime| mime.as_str() == *preferred) {
            return Some(found.as_str());
        }
    }
    offered
        .iter()
        .find(|mime| mime.starts_with("text/plain"))
        .map(String::as_str)
}

enum Cmd {
    SetText(String),
    GetText(StringRequest),
    Clear,
    Poke,
    Stop,
}

struct SeatState {
    seat: WlSeat,
    data_device: Option<WlDataDevice>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    touch: Option<WlTouch>,
    focused: bool,
    latest_serial: Option<u32>,
    selection: Option<(WlDataOffer, Vec<String>)>,
    own_source: Option<(ObjectId, String)>,
}

impl SeatState {
    fn new(seat: WlSeat) -> Self {
        Self {
            seat,
            data_device: None,
            keyboard: None,
            pointer: None,
            touch: None,
            focused: false,
            latest_serial: None,
            selection: None,
            own_source: None,
        }
    }

    fn cleanup(&mut self) {
        if let Some((offer, _)) = self.selection.take() {
            offer.destroy();
        }
        self.own_source = None;
        if let Some(keyboard) = self.keyboard.take()
            && keyboard.version() >= 3
        {
            keyboard.release();
        }
        if let Some(pointer) = self.pointer.take()
            && pointer.version() >= 3
        {
            pointer.release();
        }
        if let Some(touch) = self.touch.take()
            && touch.version() >= 3
        {
            touch.release();
        }
        if let Some(device) = self.data_device.take()
            && device.version() >= 2
        {
            device.release();
        }
        if self.seat.version() >= 5 {
            self.seat.release();
        }
    }
}

struct WaylandState {
    registry: Option<WlRegistry>,
    manager: Option<WlDataDeviceManager>,
    seats: HashMap<u32, SeatState>,
    last_input_seat: Option<u32>,
    pending_offers: HashMap<ObjectId, Vec<String>>,
    dnd_offer: Option<WlDataOffer>,
    sources: HashMap<ObjectId, (WlDataSource, String)>,
}

impl WaylandState {
    fn new() -> Self {
        Self {
            registry: None,
            manager: None,
            seats: HashMap::new(),
            last_input_seat: None,
            pending_offers: HashMap::new(),
            dnd_offer: None,
            sources: HashMap::new(),
        }
    }

    fn seat_id_for_set(&self) -> Option<u32> {
        if let Some((id, _)) = self
            .seats
            .iter()
            .find(|(_, seat)| seat.focused && seat.data_device.is_some())
        {
            return Some(*id);
        }
        if let Some(id) = self.last_input_seat
            && self
                .seats
                .get(&id)
                .is_some_and(|seat| seat.data_device.is_some())
        {
            return Some(id);
        }
        self.seats
            .iter()
            .find(|(_, seat)| seat.data_device.is_some())
            .map(|(id, _)| *id)
    }

    fn cleanup(&mut self) {
        for (_, mut seat) in self.seats.drain() {
            seat.cleanup();
        }
        for (_, (source, _)) in self.sources.drain() {
            source.destroy();
        }
        if let Some(offer) = self.dnd_offer.take() {
            offer.destroy();
        }
        self.pending_offers.clear();
        if let Some(manager) = self.manager.take()
            && manager.version() >= MIN_MANAGER_RELEASE
        {
            manager.release();
        }
        self.registry.take();
    }
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == WlDataDeviceManager::interface().name && state.manager.is_none() {
                    let manager: WlDataDeviceManager = registry.bind(name, version.min(3), qh, ());
                    for (&seat_name, seat_state) in &mut state.seats {
                        if seat_state.data_device.is_none() {
                            seat_state.data_device =
                                Some(manager.get_data_device(&seat_state.seat, qh, seat_name));
                        }
                    }
                    state.manager = Some(manager);
                } else if interface == WlSeat::interface().name {
                    let seat: WlSeat = registry.bind(name, version.min(9), qh, name);
                    let mut seat_state = SeatState::new(seat);
                    if let Some(manager) = state.manager.as_ref() {
                        seat_state.data_device =
                            Some(manager.get_data_device(&seat_state.seat, qh, name));
                    }
                    state.seats.insert(name, seat_state);
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(mut seat) = state.seats.remove(&name) {
                    seat.cleanup();
                }
                if state.last_input_seat == Some(name) {
                    state.last_input_seat = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: wayland_client::protocol::wl_data_device_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, u32> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        seat_name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(seat_state) = state.seats.get_mut(seat_name) else {
            return;
        };
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let caps = wl_seat::Capability::from_bits_truncate(capabilities.into());
            let has_keyboard = caps.contains(wl_seat::Capability::Keyboard);
            if has_keyboard && seat_state.keyboard.is_none() {
                seat_state.keyboard = Some(seat.get_keyboard(qh, *seat_name));
            } else if !has_keyboard && let Some(keyboard) = seat_state.keyboard.take() {
                if keyboard.version() >= 3 {
                    keyboard.release();
                }
                seat_state.focused = false;
            }

            let has_pointer = caps.contains(wl_seat::Capability::Pointer);
            if has_pointer && seat_state.pointer.is_none() {
                seat_state.pointer = Some(seat.get_pointer(qh, *seat_name));
            } else if !has_pointer
                && let Some(pointer) = seat_state.pointer.take()
                && pointer.version() >= 3
            {
                pointer.release();
            }

            let has_touch = caps.contains(wl_seat::Capability::Touch);
            if has_touch && seat_state.touch.is_none() {
                seat_state.touch = Some(seat.get_touch(qh, *seat_name));
            } else if !has_touch
                && let Some(touch) = seat_state.touch.take()
                && touch.version() >= 3
            {
                touch.release();
            }
        }
    }
}

impl Dispatch<WlKeyboard, u32> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        seat_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(seat_state) = state.seats.get_mut(seat_name) else {
            return;
        };
        match event {
            wl_keyboard::Event::Enter { serial, .. } => {
                seat_state.focused = true;
                seat_state.latest_serial = Some(serial);
                state.last_input_seat = Some(*seat_name);
            }
            wl_keyboard::Event::Leave { .. } => {
                seat_state.focused = false;
            }
            wl_keyboard::Event::Key { serial, .. }
            | wl_keyboard::Event::Modifiers { serial, .. } => {
                seat_state.latest_serial = Some(serial);
                state.last_input_seat = Some(*seat_name);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, u32> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        seat_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(seat_state) = state.seats.get_mut(seat_name) else {
            return;
        };
        match event {
            wl_pointer::Event::Enter { serial, .. } | wl_pointer::Event::Button { serial, .. } => {
                seat_state.latest_serial = Some(serial);
                state.last_input_seat = Some(*seat_name);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlTouch, u32> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlTouch,
        event: wl_touch::Event,
        seat_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(seat_state) = state.seats.get_mut(seat_name) else {
            return;
        };
        if let wl_touch::Event::Down { serial, .. } = event {
            seat_state.latest_serial = Some(serial);
            state.last_input_seat = Some(*seat_name);
        }
    }
}

impl Dispatch<WlDataDevice, u32> for WaylandState {
    event_created_child!(WaylandState, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ())
    ]);

    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        seat_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.pending_offers.insert(id.id(), Vec::new());
            }
            wl_data_device::Event::Enter { id, .. } => {
                if let Some(old_dnd) = state.dnd_offer.take() {
                    old_dnd.destroy();
                }
                if let Some(offer) = id {
                    state.pending_offers.remove(&offer.id());
                    state.dnd_offer = Some(offer);
                }
            }
            wl_data_device::Event::Leave => {
                if let Some(dnd_offer) = state.dnd_offer.take() {
                    dnd_offer.destroy();
                }
            }
            wl_data_device::Event::Selection { id } => {
                let Some(seat_state) = state.seats.get_mut(seat_name) else {
                    if let Some(offer) = id {
                        state.pending_offers.remove(&offer.id());
                        offer.destroy();
                    }
                    return;
                };
                if let Some((old_offer, _)) = seat_state.selection.take() {
                    old_offer.destroy();
                }
                if let Some(offer) = id {
                    let mimes = state.pending_offers.remove(&offer.id()).unwrap_or_default();
                    seat_state.selection = Some((offer, mimes));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataOffer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some(mimes) = state.pending_offers.get_mut(&offer.id())
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for WaylandState {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                let Some((_, text)) = state.sources.get(&source.id()) else {
                    return;
                };
                let bytes = text.as_bytes().to_vec();
                std::thread::spawn(move || {
                    let mut file = std::fs::File::from(fd);
                    let _ = file.write_all(&bytes);
                });
            }
            wl_data_source::Event::Cancelled => {
                let id = source.id();
                for seat_state in state.seats.values_mut() {
                    if seat_state
                        .own_source
                        .as_ref()
                        .is_some_and(|(own_id, _)| own_id == &id)
                    {
                        seat_state.own_source = None;
                    }
                }
                if let Some((removed, _)) = state.sources.remove(&id) {
                    removed.destroy();
                } else {
                    source.destroy();
                }
            }
            _ => {}
        }
    }
}

fn nonblocking_pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return None;
    }
    Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn transfer_pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return None;
    }
    Some(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn drain_pipe(fd: &OwnedFd) {
    let mut buffer = [0u8; 64];
    loop {
        let read = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if read <= 0 {
            break;
        }
    }
}

fn fd_readable(fd: i32) -> bool {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
    ready > 0 && (pollfd.revents & libc::POLLIN) != 0
}

fn drain_wayland(queue: &mut EventQueue<WaylandState>, state: &mut WaylandState) {
    let _ = queue.dispatch_pending(state);
    for _ in 0..8 {
        if let Some(guard) = queue.prepare_read() {
            if fd_readable(guard.connection_fd().as_raw_fd()) {
                let _ = guard.read();
            } else {
                drop(guard);
                let _ = queue.dispatch_pending(state);
                break;
            }
        }
        if queue.dispatch_pending(state).unwrap_or(0) == 0 {
            break;
        }
    }
}

fn handle_command(
    cmd: Cmd,
    conn: &Connection,
    qh: &QueueHandle<WaylandState>,
    state: &mut WaylandState,
) -> bool {
    match cmd {
        Cmd::Stop => return false,
        Cmd::Poke => {}
        Cmd::SetText(text) => {
            let Some(manager) = state.manager.as_ref().cloned() else {
                return true;
            };
            let Some(seat_id) = state.seat_id_for_set() else {
                return true;
            };
            let Some(seat_state) = state.seats.get_mut(&seat_id) else {
                return true;
            };
            let Some(device) = seat_state.data_device.as_ref() else {
                return true;
            };
            let source = manager.create_data_source(qh, ());
            for mime in TEXT_MIMES {
                source.offer((*mime).to_string());
            }
            let serial = seat_state.latest_serial.unwrap_or(0);
            device.set_selection(Some(&source), serial);
            let id = source.id();
            seat_state.own_source = Some((id.clone(), text.clone()));
            state.sources.insert(id, (source, text));
            let _ = conn.flush();
        }
        Cmd::Clear => {
            let Some(seat_id) = state.seat_id_for_set() else {
                return true;
            };
            let Some(seat_state) = state.seats.get_mut(&seat_id) else {
                return true;
            };
            if let Some(device) = seat_state.data_device.as_ref() {
                device.set_selection(None, seat_state.latest_serial.unwrap_or(0));
            }
            seat_state.own_source = None;
            let _ = conn.flush();
        }
        Cmd::GetText(request) => {
            if let Some((_, text)) = state.seats.values().find_map(|seat| {
                seat.own_source
                    .as_ref()
                    .filter(|(source_id, _)| state.sources.contains_key(source_id))
            }) {
                request.success(text.clone());
                return true;
            }
            let selection = state
                .seat_id_for_set()
                .and_then(|seat_id| state.seats.get(&seat_id))
                .and_then(|seat| seat.selection.as_ref())
                .or_else(|| {
                    state
                        .seats
                        .values()
                        .find_map(|seat| seat.selection.as_ref())
                });
            let Some((offer, offered)) = selection else {
                request.success(String::new());
                return true;
            };
            let Some(mime) = pick_text_mime(offered) else {
                request.success(String::new());
                return true;
            };
            let Some((read_fd, write_fd)) = transfer_pipe() else {
                request.success(String::new());
                return true;
            };
            offer.receive(mime.to_string(), write_fd.as_fd());
            drop(write_fd);
            let _ = conn.flush();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let file = std::fs::File::from(read_fd);
                let _ = file.take(MAX_READ_BYTES).read_to_end(&mut bytes);
                request.success(String::from_utf8_lossy(&bytes).into_owned());
            });
        }
    }
    true
}

fn run_wayland_thread(
    raw_display_addr: usize,
    rx: mpsc::Receiver<Cmd>,
    wake_read: OwnedFd,
    ready: mpsc::SyncSender<bool>,
) {
    let backend = unsafe {
        wayland_client::backend::Backend::from_foreign_display(
            (raw_display_addr as *mut std::ffi::c_void).cast(),
        )
    };
    let conn = Connection::from_backend(backend);
    let mut queue = conn.new_event_queue::<WaylandState>();
    let qh = queue.handle();
    let mut state = WaylandState::new();
    state.registry = Some(conn.display().get_registry(&qh, ()));
    if queue.roundtrip(&mut state).is_err() || queue.roundtrip(&mut state).is_err() {
        state.cleanup();
        let _ = ready.send(false);
        return;
    }
    if state.manager.is_none() {
        state.cleanup();
        let _ = ready.send(false);
        return;
    }
    let _ = ready.send(true);

    'outer: loop {
        let _ = queue.dispatch_pending(&mut state);
        drain_pipe(&wake_read);
        while let Ok(cmd) = rx.try_recv() {
            drain_wayland(&mut queue, &mut state);
            if !handle_command(cmd, &conn, &qh, &mut state) {
                break 'outer;
            }
        }
        let _ = conn.flush();

        let guard = loop {
            if let Some(guard) = queue.prepare_read() {
                break guard;
            }
            if queue.dispatch_pending(&mut state).is_err() {
                break 'outer;
            }
        };
        let mut fds = [
            libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled = loop {
            let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if ret < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break ret;
        };
        if polled < 0 {
            drop(guard);
            break;
        }
        if (fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0
            && (fds[0].revents & libc::POLLIN) == 0
        {
            drop(guard);
            break;
        }
        if (fds[0].revents & libc::POLLIN) != 0 {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
        } else {
            drop(guard);
        }
    }

    state.cleanup();
    let _ = conn.flush();
}

struct Worker {
    tx: mpsc::Sender<Cmd>,
    wake_write: OwnedFd,
    handle: JoinHandle<()>,
}

pub struct WaylandClipboard {
    worker: Mutex<Option<Worker>>,
}

impl WaylandClipboard {
    pub fn new(raw: RawDisplayHandle) -> Option<Rc<Self>> {
        let RawDisplayHandle::Wayland(handle) = raw else {
            return None;
        };
        let raw_display_addr = handle.display.as_ptr() as usize;
        let (wake_read, wake_write) = nonblocking_pipe()?;
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("soundscape-clipboard".into())
            .spawn(move || run_wayland_thread(raw_display_addr, rx, wake_read, ready_tx))
            .ok()?;
        match ready_rx.recv() {
            Ok(true) => Some(Rc::new(Self {
                worker: Mutex::new(Some(Worker {
                    tx,
                    wake_write,
                    handle: thread,
                })),
            })),
            _ => {
                let _ = thread.join();
                None
            }
        }
    }

    fn send(&self, cmd: Cmd) {
        let Ok(guard) = self.worker.lock() else {
            return;
        };
        if let Some(worker) = guard.as_ref()
            && worker.tx.send(cmd).is_ok()
        {
            let byte = [1u8; 1];
            unsafe {
                libc::write(worker.wake_write.as_raw_fd(), byte.as_ptr().cast(), 1);
            }
        }
    }

    pub fn poke(&self) {
        self.send(Cmd::Poke);
    }

    pub fn shutdown(&self) {
        let worker = match self.worker.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => None,
        };
        if let Some(worker) = worker {
            let _ = worker.tx.send(Cmd::Stop);
            let byte = [1u8; 1];
            unsafe {
                libc::write(worker.wake_write.as_raw_fd(), byte.as_ptr().cast(), 1);
            }
            let _ = worker.handle.join();
        }
    }
}

impl Drop for WaylandClipboard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl ClipboardDelegate for WaylandClipboard {
    fn clear(&self, _webview: WebView) {
        self.send(Cmd::Clear);
    }

    fn get_text(&self, _webview: WebView, request: StringRequest) {
        self.send(Cmd::GetText(request));
    }

    fn set_text(&self, _webview: WebView, new_contents: String) {
        self.send(Cmd::SetText(new_contents));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_utf8_text_mime_first() {
        assert_eq!(
            pick_text_mime(&["STRING".into(), "text/plain;charset=utf-8".into()]),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(
            pick_text_mime(&["text/plain".into(), "UTF8_STRING".into()]),
            Some("UTF8_STRING")
        );
        assert_eq!(
            pick_text_mime(&["text/plain;charset=iso-8859-1".into()]),
            Some("text/plain;charset=iso-8859-1")
        );
        assert_eq!(pick_text_mime(&["image/png".into()]), None);
    }

    #[test]
    fn parses_page_copy_messages() {
        assert_eq!(
            copied("abc", r#"abc:copy:"hello world""#),
            Some("hello world".into())
        );
        assert_eq!(
            copied("abc", "abc:copy:\"  padded\\nlines  \""),
            Some("  padded\nlines  ".into())
        );
        assert_eq!(copied("abc", r#"other:copy:"hello""#), None);
        assert_eq!(copied("abc", "abc:copy:not-json"), None);
    }

    #[test]
    fn builds_page_copy_script() {
        let script = page_script("nonce-42");
        assert!(script.contains("\"nonce-42\""));
        assert!(script.contains("window.soundscapeDesktopCopy"));
    }
}
