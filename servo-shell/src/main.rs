mod bridge;
mod glide;
mod keys;
mod memory;
mod pickers;
mod profile;
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
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton as WinitButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamed};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{CursorIcon, Window, WindowId};

const FRAME_COUNTER: &str = include_str!("frame_counter.js");
const FORM_ENTER: &str = include_str!("form_enter.js");
const CONTROLS: &str = include_str!("controls.css");
const SVG_FIX: &str = include_str!("svg_fix.js");
const LOGO_MOTION: &str = include_str!("logo_motion.js");
const VALVE: &str = include_str!("valve.js");
const COVERS: &str = include_str!("covers.js");
const LINE_HEIGHT: f64 = 76.0;
const APP_ID: &str = "soundscape-servo";

enum Wake {
    Servo,
    Poll,
    Push,
    Park(bool),
    Quit,
    Theme(Theme),
    Answer(u64, Answer),
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

struct Shell {
    window: Window,
    servo: Servo,
    context: Rc<WindowRenderingContext>,
    webview: RefCell<Option<WebView>>,
    openers: RefCell<Vec<WebView>>,
    select: RefCell<Option<SelectElement>>,
    waiting: RefCell<HashMap<u64, Waiting>>,
    asked: Cell<u64>,
    typing: RefCell<Option<EmbedderControlId>>,
    composing: Cell<bool>,
    smooth: Cell<bool>,
    cpu: Cell<bool>,
    cpu_now: bool,
    memory_limit: Cell<u32>,
    armed: Cell<bool>,
    loaded_at: Cell<std::time::Instant>,
    hidden_since: Cell<Option<std::time::Instant>>,
    parked: RefCell<Option<Url>>,
    asking: Cell<bool>,
    park_after: std::time::Duration,
    storage: Option<std::path::PathBuf>,
    glide: RefCell<glide::Glide>,
    server: Option<Url>,
    nonce: String,
    debug: bool,
    proxy: EventLoopProxy<Wake>,
    held: Cell<ModifiersState>,
    pointer: Cell<PhysicalPosition<f64>>,
}

pub fn internal(server: Option<&Url>, url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => server.is_some_and(|server| server.origin() == url.origin()),
        "mailto" | "tel" => false,
        _ => true,
    }
}

fn open_outside(url: &Url) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url.as_str())
        .spawn();
}

pub fn notice() -> Url {
    let page = "<!doctype html><meta charset=utf-8><body style=\"font:16px system-ui,sans-serif;padding:32px;color-scheme:light dark\"><h1>Soundscape on Servo</h1><p>No server address was found. Sign in with the Soundscape desktop app once, or start this test build with the address, for example:</p><pre>soundscape-servo https://music.example.com</pre></body>";
    let encoded: String = url::form_urlencoded::byte_serialize(page.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    Url::parse(&format!("data:text/html;charset=utf-8,{encoded}")).expect("notice page")
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

fn nonce() -> String {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    format!("soundscape{:x}{:x}", std::process::id(), time)
}

impl Shell {
    fn push_settings(&self) {
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(
                bridge::settings_script(&bridge::Shown {
                    version: env!("CARGO_PKG_VERSION"),
                    smooth_scrolling: self.smooth.get(),
                    cpu_rendering: self.cpu.get(),
                    cpu_rendering_now: self.cpu_now,
                    memory_limit: self.memory_limit.get(),
                }),
                |_| {},
            );
            webview.evaluate_javascript(valve::flag_script(self.armed.get()), |_| {});
        }
    }

    fn check_hidden(&self) {
        let hidden_for = self.hidden_since.get().map(|since| since.elapsed());
        if self.asking.get()
            || !valve::should_park(hidden_for, self.parked.borrow().is_some(), self.park_after)
        {
            return;
        }
        let Some(webview) = self.webview() else {
            return;
        };
        self.asking.set(true);
        let proxy = self.proxy.clone();
        webview.evaluate_javascript(valve::PLAYING, move |result| {
            let playing = !matches!(result, Ok(servo::JSValue::Boolean(false)));
            let _ = proxy.send_event(Wake::Park(playing));
        });
    }

    fn park(&self, playing: bool) {
        self.asking.set(false);
        if self.hidden_since.get().is_none() || self.parked.borrow().is_some() {
            return;
        }
        if playing {
            self.hidden_since.set(Some(std::time::Instant::now()));
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
    }

    fn shown(&self) {
        self.hidden_since.set(None);
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

    fn take_commands(&self) {
        let Some(webview) = self.webview() else {
            return;
        };
        webview.evaluate_javascript(bridge::TAKE_COMMANDS, |result| {
            let Ok(servo::JSValue::String(json)) = result else {
                return;
            };
            let Some(shell) = ME.with(|me| me.borrow().upgrade()) else {
                return;
            };
            for command in bridge::commands(&json) {
                match command {
                    bridge::Command::CpuRendering(on) => {
                        shell.cpu.set(on);
                        let mut own = bridge::load(shell.storage.as_deref());
                        own.cpu_rendering = Some(on);
                        bridge::save(shell.storage.as_deref(), &own);
                        let _ = shell.proxy.send_event(Wake::Push);
                    }
                    bridge::Command::MemoryLimit(mb) => {
                        let mb = valve::allowed(mb);
                        shell.memory_limit.set(mb);
                        let mut own = bridge::load(shell.storage.as_deref());
                        own.memory_limit = Some(mb);
                        bridge::save(shell.storage.as_deref(), &own);
                        let _ = shell.proxy.send_event(Wake::Push);
                    }
                    bridge::Command::Restart => {
                        RESTART.store(true, std::sync::atomic::Ordering::Relaxed);
                        let _ = shell.proxy.send_event(Wake::Quit);
                    }
                    bridge::Command::SmoothScrolling(on) => {
                        shell.smooth.set(on);
                        let mut own = bridge::load(shell.storage.as_deref());
                        own.smooth_scrolling = Some(on);
                        bridge::save(shell.storage.as_deref(), &own);
                        let _ = shell.proxy.send_event(Wake::Push);
                    }
                }
            }
        });
    }

    fn close(&self) {
        self.select.borrow_mut().take();
        self.waiting.borrow_mut().clear();
        self.openers.borrow_mut().clear();
        self.webview.borrow_mut().take();
    }

    fn webview(&self) -> Option<WebView> {
        self.webview.borrow().clone()
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
        if internal(self.server.as_ref(), url) {
            if let Some(webview) = self.webview() {
                webview.load(url.clone());
            }
        } else {
            open_outside(url);
        }
    }

    fn shortcut(&self, key: &WinitKey) -> bool {
        let Some(webview) = self.webview() else {
            return false;
        };
        let held = self.held.get();
        match key {
            WinitKey::Named(WinitNamed::F5) => webview.reload(),
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
}

impl WebViewDelegate for Shell {
    fn notify_new_frame_ready(&self, _: WebView) {
        self.window.request_redraw();
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
        self.window.set_title(&title);
    }

    fn notify_cursor_changed(&self, _: WebView, cursor: Cursor) {
        self.window
            .set_cursor_visible(!matches!(cursor, Cursor::None));
        self.window.set_cursor(icon(cursor));
    }

    fn request_navigation(&self, _: WebView, request: NavigationRequest) {
        if internal(self.server.as_ref(), &request.url) {
            request.allow();
        } else {
            open_outside(&request.url);
            request.deny();
        }
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
                self.window.set_ime_allowed(true);
                self.window.set_ime_cursor_area(
                    PhysicalPosition::new(place.min.x, place.min.y),
                    PhysicalSize::new(place.width().max(1) as u32, place.height().max(1) as u32),
                );
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
            self.window.set_ime_allowed(false);
        }
    }

    fn show_console_message(&self, _: WebView, level: servo::ConsoleLogLevel, message: String) {
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

    fn notify_crashed(&self, _: WebView, reason: String, _: Option<String>) {
        eprintln!("The page crashed: {reason}");
    }
}

#[unsafe(export_name = "_rjem_malloc_conf")]
pub static MALLOC_CONF: &[u8; 60] =
    b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0\0";

static RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    Starting(EventLoopProxy<Wake>, Profile),
    Running(Rc<Shell>),
    Stopped,
}

impl App {
    fn start(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<Wake>,
        profile: Profile,
    ) -> Rc<Shell> {
        let attributes = Window::default_attributes()
            .with_title("Soundscape")
            .with_inner_size(LogicalSize::new(1200.0, 820.0))
            .with_min_inner_size(LogicalSize::new(380.0, 480.0))
            .with_name(APP_ID, "");
        let window = event_loop.create_window(attributes).expect("window");
        let display = event_loop.display_handle().expect("display");
        let handle = window.window_handle().expect("window handle");
        let context = Rc::new(
            WindowRenderingContext::new(display, handle, window.inner_size()).expect("drawing"),
        );
        let _ = context.make_current();
        let mut opts = Opts::default();
        if let Some(dir) = &profile.storage {
            let _ = std::fs::create_dir_all(dir);
            opts.config_dir = Some(dir.clone());
        }
        let preferences = servo::Preferences {
            js_mem_gc_high_frequency_heap_growth_max: 150,
            js_mem_gc_high_frequency_heap_growth_min: 120,
            js_mem_gc_low_frequency_heap_growth: 120,
            js_mem_gc_malloc_threshold_base_mb: 16,
            session_history_max_length: 0,
            expose_servointernals_globally: true,
            shell_background_color_rgba: theme::backdrop(profile.theme),
            ..Default::default()
        };
        let servo = ServoBuilder::default()
            .preferences(preferences)
            .opts(opts)
            .event_loop_waker(Box::new(Waker(proxy.clone())))
            .build();
        servo.setup_logging();
        let scripts = Rc::new(UserContentManager::new(&servo));
        scripts.add_script(Rc::new(UserScript::new(FORM_ENTER.into(), None)));
        scripts.add_script(Rc::new(UserScript::new(COVERS.into(), None)));
        scripts.add_script(Rc::new(UserScript::new(VALVE.into(), None)));
        if !profile::wants(std::env::var("SOUNDSCAPE_PLAIN_SVG").ok().as_deref()) {
            scripts.add_script(Rc::new(UserScript::new(SVG_FIX.into(), None)));
            scripts.add_script(Rc::new(UserScript::new(LOGO_MOTION.into(), None)));
        }
        if let Ok(place) = Url::parse("soundscape://controls.css") {
            scripts.add_stylesheet(Rc::new(servo::user_contents::UserStyleSheet::new(
                CONTROLS.into(),
                place,
            )));
        }
        if let Some(extra) = std::env::var_os("SOUNDSCAPE_EXTRA_SCRIPT")
            .and_then(|path| std::fs::read_to_string(path).ok())
        {
            scripts.add_script(Rc::new(UserScript::new(extra, None)));
        }
        if profile.show_fps {
            scripts.add_script(Rc::new(UserScript::new(FRAME_COUNTER.into(), None)));
        }
        let shell = Rc::new(Shell {
            window,
            servo,
            context,
            webview: RefCell::new(None),
            openers: RefCell::new(Vec::new()),
            select: RefCell::new(None),
            waiting: RefCell::new(HashMap::new()),
            asked: Cell::new(0),
            typing: RefCell::new(None),
            composing: Cell::new(false),
            smooth: Cell::new(profile.smooth_scrolling),
            cpu: Cell::new(profile.cpu_rendering),
            memory_limit: Cell::new(profile.memory_limit),
            armed: Cell::new(false),
            loaded_at: Cell::new(std::time::Instant::now()),
            hidden_since: Cell::new(None),
            parked: RefCell::new(None),
            asking: Cell::new(false),
            park_after: valve::park_after(std::env::var("SOUNDSCAPE_PARK_SECONDS").ok().as_deref()),
            cpu_now: profile.cpu_rendering,
            storage: profile.storage.clone(),
            glide: RefCell::new(glide::Glide::default()),
            server: profile.server.clone(),
            nonce: nonce(),
            debug: profile::wants(std::env::var("SOUNDSCAPE_DEBUG").ok().as_deref()),
            proxy: proxy.clone(),
            held: Cell::new(ModifiersState::empty()),
            pointer: Cell::new(PhysicalPosition::new(0.0, 0.0)),
        });
        ME.with(|me| *me.borrow_mut() = Rc::downgrade(&shell));
        let start = profile.server.clone().unwrap_or_else(notice);
        let webview = WebViewBuilder::new(&shell.servo, shell.context.clone())
            .url(start)
            .hidpi_scale_factor(Scale::new(shell.window.scale_factor() as f32))
            .user_content_manager(scripts)
            .delegate(shell.clone())
            .build();
        webview.set_focused(true);
        *shell.webview.borrow_mut() = Some(webview.clone());
        webview.notify_theme_change(profile.theme);
        match theme::chosen(std::env::var("SOUNDSCAPE_THEME").ok().as_deref()) {
            Some(_) => {}
            None => {
                let sender = proxy.clone();
                theme::watch(move |theme| {
                    let _ = sender.send_event(Wake::Theme(theme));
                });
            }
        }
        shell
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let App::Starting(..) = self {
            let App::Starting(proxy, profile) = std::mem::replace(self, App::Stopped) else {
                return;
            };
            *self = App::Running(App::start(event_loop, proxy, profile));
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, wake: Wake) {
        let App::Running(shell) = self else {
            return;
        };
        match wake {
            Wake::Servo => shell.servo.spin_event_loop(),
            Wake::Poll => {
                shell.take_commands();
                shell.check_memory();
                shell.check_hidden();
            }
            Wake::Push => shell.push_settings(),
            Wake::Park(playing) => shell.park(playing),
            Wake::Quit => {
                software::settled(shell.storage.as_deref());
                shell.close();
                event_loop.exit();
            }
            Wake::Theme(theme) => {
                shell
                    .servo
                    .set_preference("shell_background_color_rgba", theme::backdrop(theme).into());
                if let Some(webview) = shell.webview() {
                    webview.notify_theme_change(theme);
                }
            }
            Wake::Answer(id, answer) => shell.answer(id, answer),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let App::Running(shell) = self else {
            return;
        };
        shell.servo.spin_event_loop();
        let Some(webview) = shell.webview() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                software::settled(shell.storage.as_deref());
                shell.close();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                let moved = shell.glide.borrow_mut().tick(std::time::Instant::now());
                if let Some((x, y)) = moved {
                    let (left, top) = shell.glide.borrow().point();
                    webview.notify_scroll_event(
                        Scroll::Delta(WebViewVector::Device(DeviceVector2D::new(
                            x as f32, y as f32,
                        ))),
                        WebViewPoint::Device(DevicePoint::new(left as f32, top as f32)),
                    );
                }
                webview.paint();
                shell.context.present();
                if shell.glide.borrow().active() {
                    shell.window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => webview.resize(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                webview.set_hidpi_scale_factor(Scale::new(scale_factor as f32));
            }
            WindowEvent::Focused(focused) => {
                webview.set_focused(focused);
                if focused {
                    shell.shown();
                }
            }
            WindowEvent::Occluded(hidden) => {
                if hidden {
                    if shell.hidden_since.get().is_none() {
                        shell.hidden_since.set(Some(std::time::Instant::now()));
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
            WindowEvent::MouseWheel { delta, .. } => {
                let (x, y) = wheel(delta, shell.window.scale_factor());
                if shell.debug {
                    eprintln!("wheel {delta:?}");
                }
                if shell.smooth.get()
                    && matches!(delta, MouseScrollDelta::LineDelta(..))
                    && !shell.held.get().control_key()
                {
                    let at = shell.pointer.get();
                    shell
                        .glide
                        .borrow_mut()
                        .add(-x, -y, (at.x, at.y), std::time::Instant::now());
                    shell.window.request_redraw();
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

fn main() {
    let mut profile = profile::load();
    profile.cpu_rendering = software::guard(profile.storage.as_deref(), profile.cpu_rendering);
    let ours = software::prepare(profile.cpu_rendering);
    let settling = profile.storage.clone();
    std::thread::spawn(move || {
        std::thread::sleep(software::SETTLED_AFTER);
        software::settled(settling.as_deref());
    });
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
    let poller = proxy.clone();
    std::thread::spawn(move || {
        while poller.send_event(Wake::Poll).is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    });
    let mut app = App::Starting(proxy, profile);
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("Soundscape stopped: {error}");
    }
    drop(app);
    software::settled(storage.as_deref());
    if RESTART.load(std::sync::atomic::Ordering::Relaxed)
        && let Ok(me) = std::env::current_exe()
    {
        let mut again = std::process::Command::new(me);
        again.args(std::env::args().skip(1));
        for name in ours {
            again.env_remove(name);
        }
        let _ = again.spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_server_inside_and_the_rest_outside() {
        let server = Url::parse("https://music.example.com").unwrap();
        let inside = |text: &str| internal(Some(&server), &Url::parse(text).unwrap());
        assert!(inside("https://music.example.com/albums"));
        assert!(!inside("https://www.last.fm/user/x"));
        assert!(!inside("http://music.example.com/"));
        assert!(!inside("mailto:a@example.com"));
        assert!(inside("about:blank"));
        assert!(inside("data:text/html,x"));
        assert!(!internal(None, &Url::parse("https://a.example").unwrap()));
    }

    #[test]
    fn explains_a_missing_server() {
        let page = notice();
        assert_eq!(page.scheme(), "data");
        assert!(!page.as_str().contains(' '));
        assert!(page.as_str().contains("Servo"));
    }

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
}
