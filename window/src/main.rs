extern crate servo_allocator;

mod clipboard;
mod font;
mod glide;
mod host;
mod keys;
mod memory;
mod pickers;
mod profile;
mod scroll;
mod software;
mod theme;
mod valve;

use euclid::Scale;
use pickers::{Answer, Ask, Picked};
use profile::Profile;
use servo::{
    CompositionEvent, CompositionState, CreateNewWebViewRequest, Cursor, DevicePoint,
    DeviceVector2D, EmbedderControl, EmbedderControlId, EventLoopWaker, FilePicker, ImeEvent,
    InputEvent, MouseButton, MouseButtonAction, MouseButtonEvent, MouseLeftViewportEvent,
    MouseMoveEvent, NavigationRequest, Opts, RenderingContext, Scroll, SelectElement, Servo,
    ServoBuilder, SimpleDialog, Theme, UserContentManager, UserScript, WebView, WebViewBuilder,
    WebViewDelegate, WebViewPoint, WebViewVector, WheelDelta, WheelEvent, WheelMode,
    WindowRenderingContext,
};
use soundscape_protocol::{self as ipc, FromWindow, Start, ToWindow};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{
    ElementState, Ime, MouseButton as WinitButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamed};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::WindowAttributesExtX11;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{CursorIcon, Fullscreen, Icon, Window, WindowAttributes, WindowId};

const FRAME_COUNTER: &str = include_str!("frame_counter.js");
const FORM_ENTER: &str = include_str!("form_enter.js");
const CONTROLS: &str = include_str!("controls.css");
const SVG_FIX: &str = include_str!("svg_fix.js");
const LOGO_MOTION: &str = include_str!("logo_motion.js");
const LOGO_STYLE: &str = ".animated .logo-line, .animated .logo-area, .animated .logo-dot, .lockup.animated .wordmark, .lockup.animated .tagline, .lockup.animated .app-version { animation: none !important; }\n";
const VALVE: &str = include_str!("valve.js");
const COVERS: &str = include_str!("covers.js");
const ICON_PNG: &[u8] = include_bytes!("../../icons/128x128.png");
const LINE_HEIGHT: f64 = 76.0;
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(3);
const POLL_EVERY: Duration = Duration::from_millis(400);
const APP_ID: &str = soundscape_protocol::IDENTIFIER;

pub enum Wake {
    Servo,
    Push,
    Park(u64, bool),
    Quit,
    Answer(u64, Answer),
    Call(host::Call),
}

#[derive(Clone)]
struct Waker(EventLoopProxy<Wake>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        let _ = self.0.send_event(Wake::Servo);
    }
}

enum Waiting {
    Dialog(SimpleDialog),
    Files(FilePicker),
}

fn add_font(scripts: &UserContentManager, family: Option<&str>) -> Rc<UserScript> {
    let script = Rc::new(UserScript::new(font::page_script(family), None));
    scripts.add_script(script.clone());
    script
}

struct Shell {
    window: RefCell<Option<Rc<Window>>>,
    lingering: Cell<Option<std::time::Instant>>,
    painted: Cell<bool>,
    scripts: Rc<UserContentManager>,
    theme: Cell<Theme>,
    icon: Cell<Option<u32>>,
    desktop_font: RefCell<Option<String>>,
    font_script: RefCell<Rc<UserScript>>,
    native_code: RefCell<Option<String>>,
    native_script: RefCell<Option<Rc<UserScript>>>,
    size: Cell<PhysicalSize<u32>>,
    maximized: Cell<bool>,
    servo: Servo,
    context: Rc<WindowRenderingContext>,
    clipboard: Option<Rc<clipboard::WaylandClipboard>>,
    webview: RefCell<Option<WebView>>,
    openers: RefCell<Vec<WebView>>,
    select: RefCell<Option<SelectElement>>,
    waiting: RefCell<HashMap<u64, Waiting>>,
    asked: Cell<u64>,
    typing: RefCell<Option<EmbedderControlId>>,
    composing: Cell<bool>,
    smooth: Cell<bool>,
    memory_limit: Cell<u32>,
    keep_window: Cell<i64>,
    armed: Cell<bool>,
    loaded_at: Cell<std::time::Instant>,
    hidden_since: Cell<Option<Instant>>,
    hide_gen: Cell<u64>,
    parked: RefCell<Option<Url>>,
    asking: Cell<Option<u64>>,
    park_after: Duration,
    storage: Option<std::path::PathBuf>,
    glide: RefCell<glide::Glide>,
    fling: RefCell<glide::Fling>,
    link: Rc<host::Link>,
    nonce: String,
    debug: bool,
    proxy: EventLoopProxy<Wake>,
    held: Cell<ModifiersState>,
    pointer: Cell<PhysicalPosition<f64>>,
    over_slider: Cell<bool>,
}

fn notice() -> Url {
    soundscape_protocol::setup::page("", None)
}

pub fn icon(cursor: Cursor) -> CursorIcon {
    match cursor {
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Move => CursorIcon::Move,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::EResize => CursorIcon::EResize,
        Cursor::NResize => CursorIcon::NResize,
        Cursor::NeResize => CursorIcon::NeResize,
        Cursor::NwResize => CursorIcon::NwResize,
        Cursor::SResize => CursorIcon::SResize,
        Cursor::SeResize => CursorIcon::SeResize,
        Cursor::SwResize => CursorIcon::SwResize,
        Cursor::WResize => CursorIcon::WResize,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
        _ => CursorIcon::Default,
    }
}

pub fn button(button: WinitButton) -> MouseButton {
    match button {
        WinitButton::Left => MouseButton::Primary,
        WinitButton::Middle => MouseButton::Auxiliary,
        WinitButton::Right => MouseButton::Secondary,
        WinitButton::Back => MouseButton::Back,
        WinitButton::Forward => MouseButton::Forward,
        WinitButton::Other(number) => MouseButton::Other(number),
    }
}

pub fn wheel(delta: MouseScrollDelta, scale: f64) -> (f64, f64) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (
            x as f64 * LINE_HEIGHT * scale,
            y as f64 * LINE_HEIGHT * scale,
        ),
        MouseScrollDelta::PixelDelta(moved) => (moved.x, moved.y),
    }
}

pub fn history_script(back: bool, forward: bool) -> String {
    format!(
        "window.soundscapeDesktopHistory={{back:{back},forward:{forward}}};window.dispatchEvent(new CustomEvent(\"soundscape-desktop-history\",{{detail:window.soundscapeDesktopHistory}}));"
    )
}

fn nonce() -> String {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    format!("soundscape{:x}{:x}", std::process::id(), time)
}

impl Shell {
    fn push_valve(&self) {
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(valve::flag_script(self.armed.get()), |_| {});
            self.push_history(webview.can_go_back(), webview.can_go_forward());
        }
    }

    fn push_history(&self, back: bool, forward: bool) {
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(history_script(back, forward), |_| {});
        }
    }

    fn server(&self) -> Option<Url> {
        self.link.server.borrow().clone()
    }

    fn allowed(&self, url: &Url) -> bool {
        use soundscape_protocol::server::{Navigation, classify};
        if soundscape_protocol::setup::asked(url).is_some() {
            self.hand_over(url);
            return false;
        }
        match classify(self.server().as_ref(), url) {
            Navigation::Allow => true,
            Navigation::Block => false,
            _ => {
                self.hand_over(url);
                false
            }
        }
    }

    fn hand_over(&self, url: &Url) {
        self.link.send(FromWindow::Follow {
            url: url.to_string(),
        });
    }

    fn note_url(&self) {
        if let Some(url) = self.webview().and_then(|webview| webview.url()) {
            self.link.url(&url);
        }
    }

    fn set_hidden(&self, hidden: bool) {
        self.link.hidden(hidden);
    }

    fn finished(&self) -> bool {
        let keep = self.keep_window.get();
        self.window().is_none()
            && keep != soundscape_protocol::KEEP_WINDOW_ALWAYS
            && self
                .lingering
                .get()
                .is_none_or(|since| since.elapsed() >= Duration::from_secs(keep.max(0) as u64))
    }

    fn linger(&self) {
        self.lingering.set(Some(std::time::Instant::now()));
    }

    fn quit(&self, event_loop: &ActiveEventLoop) {
        software::settled(self.storage.as_deref());
        self.remember_window();
        self.drop_window();
        self.lingering.set(None);
        self.link.send(FromWindow::Done);
        std::thread::spawn(|| {
            std::thread::sleep(SHUTDOWN_LIMIT);
            std::process::exit(0);
        });
        self.close();
        event_loop.exit();
    }

    fn obey(&self, call: host::Call, event_loop: &ActiveEventLoop) {
        match call {
            host::Call::Eval(script) => {
                if let Some(webview) = self.webview() {
                    webview.evaluate_javascript(script, |_| {});
                }
            }
            host::Call::Evaluate(script, reply) => match self.webview() {
                Some(webview) => webview.evaluate_javascript(script, move |result| {
                    reply(result.ok().as_ref().and_then(host::text));
                }),
                None => reply(None),
            },
            host::Call::Show => self.show_window(event_loop),
            host::Call::Hide => self.hide_window(),
            host::Call::Focus => {
                if let Some(window) = self.window() {
                    window.focus_window();
                }
            }
            host::Call::Navigate(url) => {
                self.parked.borrow_mut().take();
                self.hide_gen.set(self.hide_gen.get().wrapping_add(1));
                self.asking.set(None);
                if let Some(webview) = self.webview() {
                    webview.load(url);
                }
            }
            host::Call::Setup => {
                if let Some(webview) = self.webview() {
                    webview.load(notice());
                }
                self.show_window(event_loop);
            }
            host::Call::Server(server) => *self.link.server.borrow_mut() = server,
            host::Call::Native(script) => self.set_native(script),
            host::Call::Smooth(on) => self.smooth.set(on),
            host::Call::Memory(mb) => {
                self.memory_limit.set(mb);
                self.check_memory();
            }
            host::Call::KeepWindow(seconds) => self.keep_window.set(seconds),
            host::Call::Theme(dark) => {
                if theme::chosen(std::env::var("SOUNDSCAPE_THEME").ok().as_deref()).is_none() {
                    let next = theme::from_dark(dark);
                    self.theme.set(next);
                    self.servo.set_preference(
                        "shell_background_color_rgba",
                        theme::backdrop(next).into(),
                    );
                    if let Some(webview) = self.webview() {
                        webview.notify_theme_change(next);
                    }
                }
            }
            host::Call::Font(family) => {
                if std::env::var_os("SOUNDSCAPE_FONT").is_none() {
                    self.change_font(family);
                }
            }
            host::Call::Icon(accent) => {
                self.icon.set(accent);
                if let Some(window) = self.window() {
                    window.set_window_icon(window_icon(accent));
                }
            }
            host::Call::Fullscreen(on) => self.set_fullscreen(on),
            host::Call::Exit => self.quit(event_loop),
        }
    }

    fn set_native(&self, script: Option<String>) {
        if *self.native_code.borrow() == script {
            return;
        }
        if let Some(old) = self.native_script.borrow_mut().take() {
            self.scripts.remove_script(old);
        }
        if let Some(code) = script.clone() {
            let fresh = Rc::new(UserScript::new(code, None));
            self.scripts.add_script(fresh.clone());
            *self.native_script.borrow_mut() = Some(fresh);
        }
        *self.native_code.borrow_mut() = script;
    }

    fn check_hidden(&self) {
        let hide_gen = self.hide_gen.get();
        let hidden_for = self.hidden_since.get().map(|since| since.elapsed());
        if self.asking.get() == Some(hide_gen)
            || !valve::should_park(hidden_for, self.parked.borrow().is_some(), self.park_after)
        {
            return;
        }
        let Some(webview) = self.webview() else {
            return;
        };
        self.asking.set(Some(hide_gen));
        let proxy = self.proxy.clone();
        webview.evaluate_javascript(valve::PLAYING, move |result| {
            let playing = !matches!(result, Ok(servo::JSValue::Boolean(false)));
            let _ = proxy.send_event(Wake::Park(hide_gen, playing));
        });
    }

    fn park(&self, hide_gen: u64, playing: bool) {
        if self.asking.get() == Some(hide_gen) {
            self.asking.set(None);
        }
        if hide_gen != self.hide_gen.get()
            || self.hidden_since.get().is_none()
            || self.parked.borrow().is_some()
        {
            return;
        }
        if playing {
            self.hidden_since.set(Some(Instant::now()));
            return;
        }
        let Some(webview) = self.webview() else {
            return;
        };
        let Some(url) = webview.url() else {
            return;
        };
        if self.debug {
            eprintln!("resting the page while hidden: {url}");
        }
        *self.parked.borrow_mut() = Some(url);
        webview.load(valve::resting_page());
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        unsafe {
            libc::malloc_trim(0);
        }
    }

    fn change_font(&self, family: Option<String>) {
        if *self.desktop_font.borrow() == family {
            return;
        }
        let fresh = add_font(&self.scripts, family.as_deref());
        let old = self.font_script.replace(fresh);
        self.scripts.remove_script(old);
        let restyle = font::restyle_script(family.as_deref());
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(restyle, |_| {});
        }
        *self.desktop_font.borrow_mut() = family;
    }

    fn shown(&self) {
        self.hidden_since.set(None);
        self.hide_gen.set(self.hide_gen.get().wrapping_add(1));
        self.asking.set(None);
        let parked = self.parked.borrow_mut().take();
        if let (Some(url), Some(webview)) = (parked, self.webview()) {
            if self.debug {
                eprintln!("bringing the page back: {url}");
            }
            webview.load(url);
        }
    }

    fn check_memory(&self) {
        let armed = valve::should_arm(
            self.memory_limit.get(),
            valve::own_resident_mb(),
            self.loaded_at.get().elapsed(),
        );
        if armed == self.armed.replace(armed) {
            return;
        }
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(valve::flag_script(armed), |_| {});
        }
    }

    fn remember_window(&self) {
        if let Some(window) = self.window() {
            self.note_size(&window);
        }
        let size = self.size.get();
        self.link.send(FromWindow::WindowState {
            width: size.width,
            height: size.height,
            maximized: self.maximized.get(),
        });
    }

    fn note_size(&self, window: &Window) {
        if window.fullscreen().is_some() {
            return;
        }
        let maximized = window.is_maximized();
        self.maximized.set(maximized);
        if !maximized {
            self.size.set(window.inner_size());
        }
    }

    fn close(&self) {
        self.select.borrow_mut().take();
        self.waiting.borrow_mut().clear();
        self.openers.borrow_mut().clear();
        self.webview.borrow_mut().take();
        if let Some(clipboard) = self.clipboard.as_ref() {
            clipboard.shutdown();
        }
    }

    fn webview(&self) -> Option<WebView> {
        self.webview.borrow().clone()
    }

    fn window(&self) -> Option<Rc<Window>> {
        self.window.borrow().clone()
    }

    fn redraw(&self) {
        if let Some(window) = self.window() {
            window.request_redraw();
        }
    }

    fn hide_window(&self) {
        self.set_hidden(true);
        self.linger();
        if self.hidden_since.get().is_none() {
            self.hidden_since.set(Some(Instant::now()));
            self.hide_gen.set(self.hide_gen.get().wrapping_add(1));
        }
        if self.window().is_some() {
            self.remember_window();
        }
        self.openers.borrow_mut().clear();
        self.drop_window();
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        unsafe {
            libc::malloc_trim(0);
        }
    }

    fn drop_window(&self) {
        self.set_hidden(true);
        let Some(window) = self.window.borrow_mut().take() else {
            return;
        };
        if let Some(webview) = self.webview() {
            webview.hide();
        }
        let _ = self.context.take_window();
        drop(window);
    }

    fn show_window(&self, event_loop: &ActiveEventLoop) {
        self.set_hidden(false);
        if self.window().is_none() {
            let Ok(window) = event_loop.create_window(attributes(
                Some(self.size.get()),
                self.maximized.get(),
                self.icon.get(),
            )) else {
                return;
            };
            let Ok(handle) = window.window_handle() else {
                return;
            };
            let size = window.inner_size();
            if self.context.set_window(handle, size).is_err() {
                return;
            }
            if let Some(webview) = self.webview() {
                webview.resize(size);
                webview.set_hidpi_scale_factor(Scale::new(window.scale_factor() as f32));
                webview.show();
                if let Some(title) = webview
                    .page_title()
                    .filter(|title| !title.trim().is_empty())
                {
                    window.set_title(&title);
                }
            }
            *self.window.borrow_mut() = Some(Rc::new(window));
        }
        if let Some(window) = self.window() {
            window.focus_window();
            window.request_redraw();
        }
        self.shown();
    }

    fn point(&self) -> WebViewPoint {
        let at = self.pointer.get();
        WebViewPoint::Device(DevicePoint::new(at.x as f32, at.y as f32))
    }

    fn send(&self, event: InputEvent) {
        if let Some(webview) = self.webview() {
            webview.notify_input_event(event);
        }
    }

    fn wait(&self, waiting: Waiting, ask: Ask, message: String) {
        let id = self.asked.get() + 1;
        self.asked.set(id);
        self.waiting.borrow_mut().insert(id, waiting);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let answer = pickers::ask(&ask, &message);
            let _ = proxy.send_event(Wake::Answer(id, answer));
        });
    }

    fn answer(&self, id: u64, answer: Answer) {
        let Some(waiting) = self.waiting.borrow_mut().remove(&id) else {
            return;
        };
        match waiting {
            Waiting::Dialog(SimpleDialog::Prompt(mut prompt)) => {
                if answer.ok {
                    prompt.set_current_value(&answer.text);
                    prompt.confirm();
                } else {
                    prompt.dismiss();
                }
            }
            Waiting::Dialog(dialog) => {
                if answer.ok {
                    dialog.confirm();
                } else {
                    dialog.dismiss();
                }
            }
            Waiting::Files(mut picker) => {
                let chosen = pickers::paths(&answer.text);
                if answer.ok && !chosen.is_empty() {
                    picker.select(&chosen);
                    picker.submit();
                } else {
                    picker.dismiss();
                }
            }
        }
    }

    fn go(&self, url: &Url) {
        if self.allowed(url)
            && let Some(webview) = self.webview()
        {
            webview.load(url.clone());
        }
    }

    fn shortcut(&self, key: &WinitKey) -> bool {
        let Some(webview) = self.webview() else {
            return false;
        };
        let held = self.held.get();
        match key {
            WinitKey::Named(WinitNamed::F5) => webview.reload(),
            WinitKey::Named(WinitNamed::F11) => self.set_fullscreen(None),
            WinitKey::Character(letter)
                if held.control_key() && held.shift_key() && letter.eq_ignore_ascii_case("m") =>
            {
                if let Ok(callback) = servo_base::generic_channel::GenericCallback::new(|result| {
                    if let Ok(result) = result {
                        eprintln!("{}", memory::summary(&result));
                    }
                }) {
                    self.servo.create_memory_report(callback);
                }
            }
            WinitKey::Character(letter)
                if held.control_key() && letter.eq_ignore_ascii_case("r") =>
            {
                webview.reload()
            }
            WinitKey::Character(letter)
                if held.control_key() && letter.eq_ignore_ascii_case("m") =>
            {
                self.link.send(FromWindow::Shortcut {
                    name: "mini".into(),
                });
            }
            WinitKey::Character(letter) if held.control_key() && letter.as_str() == "," => {
                self.link.send(FromWindow::Shortcut {
                    name: "settings".into(),
                })
            }
            WinitKey::Named(WinitNamed::Home) if held.alt_key() => {
                if let Some(server) = self.server() {
                    webview.load(server);
                }
            }
            WinitKey::Named(WinitNamed::ArrowLeft) if held.alt_key() => {
                if webview.can_go_back() {
                    webview.go_back(1);
                }
            }
            WinitKey::Named(WinitNamed::ArrowRight) if held.alt_key() => {
                if webview.can_go_forward() {
                    webview.go_forward(1);
                }
            }
            _ => return false,
        }
        true
    }

    fn compose(&self, state: CompositionState, data: String) {
        self.send(InputEvent::Ime(ImeEvent::Composition(CompositionEvent {
            state,
            data,
        })));
    }

    fn is_server_page(&self, webview: &WebView) -> bool {
        match (self.server(), webview.url()) {
            (Some(server), Some(current)) => current.origin() == server.origin(),
            _ => false,
        }
    }

    fn set_fullscreen(&self, on: Option<bool>) {
        let Some(window) = self.window() else {
            return;
        };
        let next = on.unwrap_or_else(|| window.fullscreen().is_none());
        window.set_fullscreen(next.then_some(Fullscreen::Borderless(None)));
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(
                format!(
                    "window.dispatchEvent(new CustomEvent(\"soundscape-fullscreen\", {{ detail: {next} }})), undefined"
                ),
                |_| {},
            );
        }
    }
}

impl WebViewDelegate for Shell {
    fn notify_new_frame_ready(&self, _: WebView) {
        self.redraw();
    }

    fn notify_load_status_changed(&self, _: WebView, status: servo::LoadStatus) {
        if matches!(status, servo::LoadStatus::Complete) {
            self.loaded_at.set(std::time::Instant::now());
            self.armed.set(false);
            let _ = self.proxy.send_event(Wake::Push);
        }
    }

    fn notify_page_title_changed(&self, _: WebView, title: Option<String>) {
        let title = title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Soundscape".into());
        if let Some(window) = self.window() {
            window.set_title(&title);
        }
    }

    fn notify_cursor_changed(&self, _: WebView, cursor: Cursor) {
        if let Some(window) = self.window() {
            window.set_cursor_visible(!matches!(cursor, Cursor::None));
            window.set_cursor(icon(cursor));
        }
    }

    fn request_navigation(&self, _: WebView, request: NavigationRequest) {
        if self.allowed(&request.url) {
            request.allow();
        } else {
            request.deny();
        }
    }

    fn notify_url_changed(&self, _: WebView, url: Url) {
        self.link.url(&url);
    }

    fn notify_history_changed(&self, _: WebView, entries: Vec<Url>, current: usize) {
        let back = current > 0;
        let forward = current
            .checked_add(1)
            .is_some_and(|next| next < entries.len());
        self.push_history(back, forward);
    }

    fn request_create_new(&self, _: WebView, request: CreateNewWebViewRequest) {
        let Some(shell) = self.me() else {
            return;
        };
        let opener = Rc::new(Opener {
            shell: Rc::downgrade(&shell),
            done: Cell::new(false),
        });
        let webview = request
            .builder(self.context.clone())
            .delegate(opener)
            .build();
        self.openers.borrow_mut().push(webview);
    }

    fn show_embedder_control(&self, webview: WebView, control: EmbedderControl) {
        match control {
            EmbedderControl::SelectElement(select) => {
                let rows = pickers::rows(select.options(), &select.selected_options());
                let script = pickers::picker_script(&self.nonce, &rows, select.position());
                *self.select.borrow_mut() = Some(select);
                webview.evaluate_javascript(script, |_| {});
            }
            EmbedderControl::SimpleDialog(dialog) => {
                let ask = match &dialog {
                    SimpleDialog::Alert(_) => Ask::Alert,
                    SimpleDialog::Confirm(_) => Ask::Confirm,
                    SimpleDialog::Prompt(prompt) => Ask::Prompt(prompt.current_value().to_string()),
                };
                let message = dialog.message().to_string();
                self.wait(Waiting::Dialog(dialog), ask, message);
            }
            EmbedderControl::FilePicker(picker) => {
                let ask = Ask::Files {
                    many: picker.allow_select_multiple(),
                };
                self.wait(Waiting::Files(picker), ask, String::new());
            }
            EmbedderControl::InputMethod(control) => {
                *self.typing.borrow_mut() = Some(control.id());
                let place = control.position();
                if let Some(window) = self.window() {
                    window.set_ime_allowed(true);
                    window.set_ime_cursor_area(
                        PhysicalPosition::new(place.min.x, place.min.y),
                        PhysicalSize::new(
                            place.width().max(1) as u32,
                            place.height().max(1) as u32,
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    fn hide_embedder_control(&self, webview: WebView, id: EmbedderControlId) {
        let closing_select = self
            .select
            .borrow()
            .as_ref()
            .is_some_and(|select| select.id() == id);
        if closing_select {
            self.select.borrow_mut().take();
            webview.evaluate_javascript(pickers::CLOSE_PICKER, |_| {});
        }
        let closing_typing = self.typing.borrow().is_some_and(|typing| typing == id);
        if closing_typing {
            self.typing.borrow_mut().take();
            self.composing.set(false);
            if let Some(window) = self.window() {
                window.set_ime_allowed(false);
            }
        }
    }

    fn show_console_message(
        &self,
        webview: WebView,
        level: servo::ConsoleLogLevel,
        message: String,
    ) {
        if message.starts_with(soundscape_protocol::NATIVE_ASKED) {
            if self.is_server_page(&webview) {
                self.link.send(FromWindow::Page { message });
            }
            return;
        }
        if let Some(text) = clipboard::copied(&self.nonce, &message) {
            if self.is_server_page(&webview) {
                let delegate = webview.clipboard_delegate();
                delegate.set_text(webview, text);
            }
            return;
        }
        if let Some(over) = scroll::over_slider(&self.nonce, &message) {
            self.over_slider.set(over);
            return;
        }
        let Some(picked) = pickers::picked(&self.nonce, &message) else {
            if self.debug {
                eprintln!("page {level:?}: {message}");
            }
            return;
        };
        let Some(mut select) = self.select.borrow_mut().take() else {
            return;
        };
        if let Picked::Option(id) = picked {
            select.select(vec![id]);
        }
        select.submit();
    }

    fn notify_media_session_event(&self, _: WebView, _: servo::MediaSessionEvent) {
        self.link.send(FromWindow::Poke);
    }

    fn notify_crashed(&self, _: WebView, reason: String, _: Option<String>) {
        eprintln!("The page crashed: {reason}");
    }
}

#[used]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static MALLOC_CONF: &[u8; 77] =
    b"narenas:4,dirty_decay_ms:5000,muzzy_decay_ms:5000,lg_tcache_max:13,thp:never\0";

thread_local! {
    static ME: RefCell<Weak<Shell>> = const { RefCell::new(Weak::new()) };
}

impl Shell {
    fn me(&self) -> Option<Rc<Shell>> {
        ME.with(|me| me.borrow().upgrade())
    }
}

struct Opener {
    shell: Weak<Shell>,
    done: Cell<bool>,
}

impl Opener {
    fn follow(&self, webview: &WebView, url: &Url) {
        if url.scheme() == "about" {
            return;
        }
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        if !self.done.replace(true) {
            shell.go(url);
        }
        shell
            .openers
            .borrow_mut()
            .retain(|opener| opener.id() != webview.id());
    }
}

impl WebViewDelegate for Opener {
    fn request_navigation(&self, webview: WebView, request: NavigationRequest) {
        if request.url.scheme() == "about" {
            request.allow();
            return;
        }
        let url = request.url.clone();
        request.deny();
        self.follow(&webview, &url);
    }

    fn notify_url_changed(&self, webview: WebView, url: Url) {
        self.follow(&webview, &url);
    }
}

enum App {
    Starting(EventLoopProxy<Wake>, Profile, Launched),
    Running(Rc<Shell>),
    Stopped,
}

impl App {
    fn start(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<Wake>,
        profile: Profile,
        launched: Launched,
    ) -> Rc<Shell> {
        let start = &launched.start;
        let saved_size = start
            .size
            .filter(|(width, height)| *width >= 200 && *height >= 200)
            .map(|(width, height)| PhysicalSize::new(width, height));
        let window = event_loop
            .create_window(attributes(saved_size, start.maximized, start.icon))
            .expect("window");
        let display = event_loop.display_handle().expect("display");
        let raw_display = display.as_raw();
        let handle = window.window_handle().expect("window handle");
        let context = Rc::new(
            WindowRenderingContext::new(display, handle, window.inner_size()).expect("drawing"),
        );
        let _ = context.make_current();
        let clipboard = clipboard::WaylandClipboard::new(raw_display);
        let mut opts = Opts::default();
        if let Some(dir) = &profile.storage {
            let _ = std::fs::create_dir_all(dir);
            opts.config_dir = Some(dir.clone());
        }
        let preferences = servo::Preferences {
            dom_async_clipboard_enabled: true,
            dom_gamepad_enabled: false,
            dom_webxr_enabled: false,
            dom_webxr_glwindow_enabled: false,
            dom_webxr_hands_enabled: false,
            dom_webxr_openxr_enabled: false,
            dom_sharedworker_enabled: false,
            network_http_cache_size: 512,
            layout_threads: 2,
            thread_pool_workers_max: 2,
            thread_pool_fallback_workers: 2,
            thread_pool_async_runtime_workers_max: 2,
            thread_pool_canvas_workers: 0,
            thread_pool_webrender_workers_max: 2,
            js_wasm_enabled: false,
            js_asmjs_enabled: false,
            js_mem_gc_empty_chunk_count_min: 0,
            js_mem_gc_high_frequency_low_limit_mb: 32,
            js_mem_gc_high_frequency_high_limit_mb: 128,
            js_mem_gc_high_frequency_heap_growth_max: 150,
            js_mem_gc_high_frequency_heap_growth_min: 125,
            js_mem_gc_low_frequency_heap_growth: 125,
            js_mem_gc_malloc_threshold_base_mb: 16,
            session_history_max_length: 0,
            expose_servointernals_globally: true,
            shell_background_color_rgba: theme::backdrop(profile.theme),
            js_ion_enabled: profile.faster_scripts,
            js_baseline_jit_enabled: profile.faster_scripts,
            ..Default::default()
        };
        let servo = ServoBuilder::default()
            .preferences(preferences)
            .opts(opts)
            .event_loop_waker(Box::new(Waker(proxy.clone())))
            .build();
        servo.setup_logging();
        let nonce = nonce();
        let scripts = Rc::new(UserContentManager::new(&servo));
        scripts.add_script(Rc::new(UserScript::new(
            clipboard::page_script(&nonce),
            None,
        )));
        scripts.add_script(Rc::new(UserScript::new(scroll::page_script(&nonce), None)));
        scripts.add_script(Rc::new(UserScript::new(FORM_ENTER.into(), None)));
        scripts.add_script(Rc::new(UserScript::new(COVERS.into(), None)));
        scripts.add_script(Rc::new(UserScript::new(VALVE.into(), None)));
        if !profile::wants(std::env::var("SOUNDSCAPE_PLAIN_SVG").ok().as_deref()) {
            scripts.add_script(Rc::new(UserScript::new(SVG_FIX.into(), None)));
            scripts.add_script(Rc::new(UserScript::new(LOGO_MOTION.into(), None)));
            if let Ok(place) = Url::parse("soundscape://logo_motion.css") {
                scripts.add_stylesheet(Rc::new(servo::user_contents::UserStyleSheet::new(
                    LOGO_STYLE.into(),
                    place,
                )));
            }
        }
        if let Ok(place) = Url::parse("soundscape://controls.css") {
            scripts.add_stylesheet(Rc::new(servo::user_contents::UserStyleSheet::new(
                CONTROLS.into(),
                place,
            )));
        }
        scripts.add_script(Rc::new(UserScript::new(
            soundscape_protocol::SAME_WINDOW.into(),
            None,
        )));
        let native_code = start.native.clone();
        let native_script = native_code.as_ref().map(|native| {
            let script = Rc::new(UserScript::new(native.clone(), None));
            scripts.add_script(script.clone());
            script
        });
        let desktop_font = std::env::var("SOUNDSCAPE_FONT")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("sans-serif"))
            .or_else(|| start.font.clone());
        let font_script = add_font(&scripts, desktop_font.as_deref());
        if let Some(extra) = std::env::var_os("SOUNDSCAPE_EXTRA_SCRIPT")
            .and_then(|path| std::fs::read_to_string(path).ok())
        {
            scripts.add_script(Rc::new(UserScript::new(extra, None)));
        }
        if profile.show_fps {
            scripts.add_script(Rc::new(UserScript::new(FRAME_COUNTER.into(), None)));
        }
        let size = saved_size.unwrap_or_else(|| window.inner_size());
        let shell = Rc::new(Shell {
            window: RefCell::new(Some(Rc::new(window))),
            lingering: Cell::new(None),
            painted: Cell::new(false),
            scripts: scripts.clone(),
            theme: Cell::new(profile.theme),
            icon: Cell::new(start.icon),
            desktop_font: RefCell::new(desktop_font),
            font_script: RefCell::new(font_script),
            native_code: RefCell::new(native_code),
            native_script: RefCell::new(native_script),
            size: Cell::new(size),
            maximized: Cell::new(start.maximized),
            servo,
            context,
            clipboard: clipboard.clone(),
            webview: RefCell::new(None),
            openers: RefCell::new(Vec::new()),
            select: RefCell::new(None),
            waiting: RefCell::new(HashMap::new()),
            asked: Cell::new(0),
            typing: RefCell::new(None),
            composing: Cell::new(false),
            smooth: Cell::new(start.smooth),
            memory_limit: Cell::new(start.memory_limit),
            keep_window: Cell::new(start.keep_window),
            armed: Cell::new(false),
            loaded_at: Cell::new(std::time::Instant::now()),
            hidden_since: Cell::new(None),
            hide_gen: Cell::new(0),
            parked: RefCell::new(None),
            asking: Cell::new(None),
            park_after: valve::park_after(std::env::var("SOUNDSCAPE_PARK_SECONDS").ok().as_deref()),
            storage: profile.storage.clone(),
            glide: RefCell::new(glide::Glide::default()),
            fling: RefCell::new(glide::Fling::default()),
            link: launched.link.clone(),
            nonce,
            debug: profile::wants(std::env::var("SOUNDSCAPE_DEBUG").ok().as_deref()),
            proxy: proxy.clone(),
            held: Cell::new(ModifiersState::empty()),
            pointer: Cell::new(PhysicalPosition::new(0.0, 0.0)),
            over_slider: Cell::new(false),
        });
        ME.with(|me| *me.borrow_mut() = Rc::downgrade(&shell));
        let first = Url::parse(&start.url).unwrap_or_else(|_| notice());
        shell.link.url(&first);
        let mut builder = WebViewBuilder::new(&shell.servo, shell.context.clone())
            .url(first)
            .hidpi_scale_factor(Scale::new(
                shell.window().map_or(1.0, |window| window.scale_factor()) as f32,
            ))
            .user_content_manager(scripts)
            .delegate(shell.clone());
        if let Some(clipboard) = clipboard {
            builder = builder.clipboard_delegate(clipboard);
        }
        let webview = builder.build();
        webview.set_focused(true);
        *shell.webview.borrow_mut() = Some(webview.clone());
        webview.notify_theme_change(profile.theme);
        shell.set_hidden(false);
        shell
    }
}

impl App {
    fn begin(&mut self, event_loop: &ActiveEventLoop) {
        let App::Starting(proxy, profile, launched) = std::mem::replace(self, App::Stopped) else {
            return;
        };
        *self = App::Running(App::start(event_loop, proxy, profile, launched));
    }

    fn end_if_finished(&self, event_loop: &ActiveEventLoop) {
        if let App::Running(shell) = self
            && shell.finished()
        {
            shell.quit(event_loop);
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.begin(event_loop);
        self.end_if_finished(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, wake: Wake) {
        if let App::Starting(_, profile, launched) = self {
            if matches!(wake, Wake::Quit | Wake::Call(host::Call::Exit)) {
                software::settled(profile.storage.as_deref());
                launched.link.send(FromWindow::Done);
                event_loop.exit();
                return;
            }
            self.begin(event_loop);
        }
        self.wake(event_loop, wake);
        self.end_if_finished(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.window_changed(event_loop, id, event);
        self.end_if_finished(event_loop);
    }
}

struct Runner {
    app: App,
    storage: Option<std::path::PathBuf>,
    settled_at: Option<Instant>,
    next_poll: Instant,
}

impl ApplicationHandler<Wake> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.app.resumed(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, wake: Wake) {
        self.app.user_event(event_loop, wake);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.app.window_event(event_loop, id, event);
        if let App::Running(shell) = &self.app {
            self.settled_at =
                software::settle_deadline(self.settled_at, Instant::now(), shell.painted.get());
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if let Some(deadline) = self.settled_at
            && now >= deadline
        {
            software::settled(self.storage.as_deref());
            self.settled_at = None;
        }
        let mut earliest = self.settled_at;
        if let App::Running(shell) = &self.app {
            let unparked = shell.parked.borrow().is_none();
            if unparked && now >= self.next_poll {
                shell.note_url();
                shell.check_memory();
                shell.check_hidden();
                self.next_poll = now + POLL_EVERY;
            }
            self.app.end_if_finished(event_loop);
            if event_loop.exiting() {
                return;
            }
            let mut wait_until = if shell.parked.borrow().is_none() {
                Some(self.next_poll)
            } else {
                None
            };
            if shell.window().is_none()
                && shell.keep_window.get() != soundscape_protocol::KEEP_WINDOW_ALWAYS
                && let Some(since) = shell.lingering.get()
            {
                let linger_until =
                    since + Duration::from_secs(shell.keep_window.get().max(0) as u64);
                wait_until = Some(wait_until.map_or(linger_until, |at| at.min(linger_until)));
            }
            if let Some(target) = wait_until {
                earliest = Some(earliest.map_or(target, |at| at.min(target)));
            }
        }
        event_loop.set_control_flow(match earliest {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

impl App {
    fn wake(&mut self, event_loop: &ActiveEventLoop, wake: Wake) {
        let App::Running(shell) = self else {
            if let Wake::Call(host::Call::Evaluate(_, reply)) = wake {
                reply(None);
            }
            return;
        };
        match wake {
            Wake::Servo => shell.servo.spin_event_loop(),
            Wake::Push => shell.push_valve(),
            Wake::Park(hide_gen, playing) => shell.park(hide_gen, playing),
            Wake::Call(call) => shell.obey(call, event_loop),
            Wake::Quit => shell.quit(event_loop),
            Wake::Answer(id, answer) => shell.answer(id, answer),
        }
    }

    fn window_changed(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let App::Running(shell) = self else {
            return;
        };
        shell.servo.spin_event_loop();
        let Some(window) = shell.window().filter(|window| window.id() == id) else {
            return;
        };
        let Some(webview) = shell.webview() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                shell.hide_window();
                shell.link.send(FromWindow::Close);
            }
            WindowEvent::RedrawRequested => {
                let now = std::time::Instant::now();
                let coasted = shell.fling.borrow_mut().tick(now);
                if let Some((x, y)) = coasted {
                    let (left, top) = shell.fling.borrow().point();
                    webview.notify_scroll_event(
                        Scroll::Delta(WebViewVector::Device(DeviceVector2D::new(
                            x as f32, y as f32,
                        ))),
                        WebViewPoint::Device(DevicePoint::new(left as f32, top as f32)),
                    );
                }
                let moved = shell.glide.borrow_mut().tick(now);
                if moved.is_none() && coasted.is_some() {
                    shell.servo.spin_event_loop();
                }
                if let Some((x, y)) = moved {
                    let (left, top) = shell.glide.borrow().point();
                    webview.notify_scroll_event(
                        Scroll::Delta(WebViewVector::Device(DeviceVector2D::new(
                            x as f32, y as f32,
                        ))),
                        WebViewPoint::Device(DevicePoint::new(left as f32, top as f32)),
                    );
                    shell.servo.spin_event_loop();
                }
                webview.paint();
                shell.context.present();
                shell.painted.set(true);
                if shell.glide.borrow().active() || shell.fling.borrow().active() {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                webview.resize(size);
                shell.note_size(&window);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                webview.set_hidpi_scale_factor(Scale::new(scale_factor as f32));
            }
            WindowEvent::Focused(focused) => {
                webview.set_focused(focused);
                if focused {
                    shell.shown();
                } else if let Some(clipboard) = shell.clipboard.as_ref() {
                    clipboard.poke();
                }
            }
            WindowEvent::Occluded(hidden) => {
                if hidden {
                    if shell.hidden_since.get().is_none() {
                        shell.hidden_since.set(Some(Instant::now()));
                        shell.hide_gen.set(shell.hide_gen.get().wrapping_add(1));
                    }
                } else {
                    shell.shown();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => shell.held.set(modifiers.state()),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed && shell.shortcut(&event.logical_key) {
                    return;
                }
                shell.send(InputEvent::Keyboard(keys::event(&event, shell.held.get())));
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                if text.is_empty() {
                    return;
                }
                if !shell.composing.replace(true) {
                    shell.compose(CompositionState::Start, String::new());
                }
                shell.compose(CompositionState::Update, text);
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                if !shell.composing.replace(false) {
                    shell.compose(CompositionState::Start, String::new());
                }
                shell.compose(CompositionState::End, text);
            }
            WindowEvent::CursorMoved { position, .. } => {
                shell.pointer.set(position);
                shell.send(InputEvent::MouseMove(MouseMoveEvent::new(shell.point())));
            }
            WindowEvent::CursorLeft { .. } => {
                shell.over_slider.set(false);
                shell.send(InputEvent::MouseLeftViewport(
                    MouseLeftViewportEvent::default(),
                ));
            }
            WindowEvent::MouseInput {
                state,
                button: pressed,
                ..
            } => {
                let down = state == ElementState::Pressed;
                if down {
                    shell.fling.borrow_mut().stop();
                }
                match pressed {
                    WinitButton::Back if down && webview.can_go_back() => {
                        webview.go_back(1);
                    }
                    WinitButton::Forward if down && webview.can_go_forward() => {
                        webview.go_forward(1);
                    }
                    WinitButton::Back | WinitButton::Forward => {}
                    other => shell.send(InputEvent::MouseButton(MouseButtonEvent::new(
                        if down {
                            MouseButtonAction::Down
                        } else {
                            MouseButtonAction::Up
                        },
                        button(other),
                        shell.point(),
                    ))),
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (x, y) = wheel(delta, window.scale_factor());
                if shell.debug {
                    eprintln!("wheel {delta:?} {phase:?}");
                }
                if shell.smooth.get()
                    && !shell.over_slider.get()
                    && matches!(delta, MouseScrollDelta::PixelDelta(..))
                    && !shell.held.get().control_key()
                {
                    let now = std::time::Instant::now();
                    let at = shell.pointer.get();
                    let mut fling = shell.fling.borrow_mut();
                    if phase == TouchPhase::Started {
                        fling.stop();
                    }
                    if x != 0.0 || y != 0.0 {
                        fling.touch(-x, -y, (at.x, at.y), now);
                        webview.notify_scroll_event(
                            Scroll::Delta(WebViewVector::Device(DeviceVector2D::new(
                                -x as f32, -y as f32,
                            ))),
                            WebViewPoint::Device(DevicePoint::new(at.x as f32, at.y as f32)),
                        );
                    }
                    if phase == TouchPhase::Ended {
                        fling.release(now);
                    }
                    drop(fling);
                    window.request_redraw();
                    return;
                }
                if shell.smooth.get()
                    && !shell.over_slider.get()
                    && matches!(delta, MouseScrollDelta::LineDelta(..))
                    && !shell.held.get().control_key()
                {
                    let at = shell.pointer.get();
                    shell
                        .glide
                        .borrow_mut()
                        .add(-x, -y, (at.x, at.y), std::time::Instant::now());
                    window.request_redraw();
                    return;
                }
                let mode = WheelMode::DeltaPixel;
                shell.send(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x, y, z: 0.0, mode },
                    shell.point(),
                )));
            }
            _ => {}
        }
    }
}

fn attributes(
    size: Option<PhysicalSize<u32>>,
    maximized: bool,
    icon: Option<u32>,
) -> WindowAttributes {
    let attributes = Window::default_attributes()
        .with_title("Soundscape")
        .with_min_inner_size(LogicalSize::new(380.0, 480.0));
    let attributes = named(attributes, "soundscape");
    let attributes = match size {
        Some(size) => attributes.with_inner_size(size),
        None => attributes.with_inner_size(LogicalSize::new(1200.0, 820.0)),
    };
    attributes
        .with_maximized(maximized)
        .with_window_icon(window_icon(icon))
}

fn named(attributes: WindowAttributes, instance: &str) -> WindowAttributes {
    let attributes = WindowAttributesExtWayland::with_name(attributes, APP_ID, instance);
    WindowAttributesExtX11::with_name(attributes, APP_ID, instance)
}

fn window_icon(accent: Option<u32>) -> Option<Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(frame.buffer_size());
    if let Some(accent) = accent {
        soundscape_protocol::tint(&mut pixels, accent);
    }
    Icon::from_rgba(pixels, frame.width, frame.height).ok()
}

struct Launched {
    start: Start,
    link: Rc<host::Link>,
}

fn protocol_out() -> host::Out {
    use std::os::fd::FromRawFd;
    let min_fd: libc::c_int = 3;
    let copy = unsafe { libc::fcntl(1, libc::F_DUPFD_CLOEXEC, min_fd) };
    let out: Box<dyn std::io::Write + Send> = if copy >= 0 {
        unsafe { libc::dup2(2, 1) };
        Box::new(unsafe { std::fs::File::from_raw_fd(copy) })
    } else {
        Box::new(std::io::stdout())
    };
    std::sync::Arc::new(std::sync::Mutex::new(out))
}

fn main() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    let out = protocol_out();
    let mut input = std::io::BufReader::new(std::io::stdin());
    let start = loop {
        match ipc::read::<ToWindow>(&mut input) {
            None => return,
            Some(Some(ToWindow::Start(start))) => break start,
            Some(_) => {}
        }
    };
    let server = start
        .server
        .as_deref()
        .and_then(|text| Url::parse(text).ok());
    let link = Rc::new(host::Link::new(out.clone(), server));
    let mut profile = profile::from_start(&start);
    let crashed = out.clone();
    profile.cpu_rendering = software::guard(
        profile.storage.as_deref(),
        profile.cpu_rendering,
        move |on| {
            host::tell(&crashed, &FromWindow::CpuRendering { on });
        },
    );
    let ours = software::prepare(profile.cpu_rendering);
    let storage = profile.storage.clone();
    if profile::wants(std::env::var("SOUNDSCAPE_DEBUG").ok().as_deref()) {
        for name in &ours {
            eprintln!("set {name}={}", std::env::var(name).unwrap_or_default());
        }
    }
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .expect("event loop");
    let proxy = event_loop.create_proxy();
    let quitter = proxy.clone();
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ]) {
        std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                let _ = quitter.send_event(Wake::Quit);
            }
        });
    }
    let listener = proxy.clone();
    let answers = out.clone();
    std::thread::spawn(move || {
        while let Some(message) = ipc::read::<ToWindow>(&mut input) {
            let Some(call) = message.and_then(|message| host::call_for(message, &answers)) else {
                continue;
            };
            if listener.send_event(Wake::Call(call)).is_err() {
                return;
            }
        }
        let _ = listener.send_event(Wake::Quit);
    });
    let mut runner = Runner {
        app: App::Starting(proxy, profile, Launched { start, link }),
        storage: storage.clone(),
        settled_at: Some(Instant::now() + software::SETTLED_AFTER),
        next_poll: Instant::now() + POLL_EVERY,
    };
    if let Err(error) = event_loop.run_app(&mut runner) {
        eprintln!("The Soundscape window stopped: {error}");
    }
    drop(runner);
    software::settled(storage.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolls_a_notch_at_a_time() {
        assert_eq!(
            wheel(MouseScrollDelta::LineDelta(0.0, -1.0), 1.0),
            (0.0, -76.0)
        );
        assert_eq!(
            wheel(MouseScrollDelta::LineDelta(0.0, 1.0), 2.0),
            (0.0, 152.0)
        );
        assert_eq!(
            wheel(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(1.5, -3.0)),
                2.0
            ),
            (1.5, -3.0)
        );
    }

    #[test]
    fn maps_cursors_and_buttons() {
        assert_eq!(icon(Cursor::Pointer), CursorIcon::Pointer);
        assert_eq!(icon(Cursor::Text), CursorIcon::Text);
        assert_eq!(icon(Cursor::Default), CursorIcon::Default);
        assert!(matches!(button(WinitButton::Left), MouseButton::Primary));
        assert!(matches!(button(WinitButton::Right), MouseButton::Secondary));
        assert!(matches!(
            button(WinitButton::Other(7)),
            MouseButton::Other(7)
        ));
    }

    #[test]
    fn tells_the_page_when_back_and_forward_are_ready() {
        assert_eq!(
            history_script(true, false),
            "window.soundscapeDesktopHistory={back:true,forward:false};window.dispatchEvent(new CustomEvent(\"soundscape-desktop-history\",{detail:window.soundscapeDesktopHistory}));"
        );
        assert_eq!(
            history_script(false, true),
            "window.soundscapeDesktopHistory={back:false,forward:true};window.dispatchEvent(new CustomEvent(\"soundscape-desktop-history\",{detail:window.soundscapeDesktopHistory}));"
        );
    }

    #[test]
    fn drives_lockup_reveal_without_css_keyframe_races() {
        assert!(LOGO_STYLE.contains(".lockup.animated .wordmark"));
        assert!(LOGO_STYLE.contains(".lockup.animated .tagline"));
        assert!(LOGO_STYLE.contains(".lockup.animated .app-version"));
        assert!(LOGO_STYLE.contains("animation: none !important;"));
        assert!(LOGO_MOTION.contains("clearLockup"));
        assert!(!LOGO_MOTION.contains("clipPath"));
    }
}
