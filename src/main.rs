mod font;
mod glide;
mod host;
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
use soundscape_core::host::Manager;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{
    ElementState, Ime, MouseButton as WinitButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamed};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::WindowAttributesExtX11;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{CursorIcon, Icon, Window, WindowAttributes, WindowId, WindowLevel};

const FRAME_COUNTER: &str = include_str!("frame_counter.js");
const FORM_ENTER: &str = include_str!("form_enter.js");
const CONTROLS: &str = include_str!("controls.css");
const SVG_FIX: &str = include_str!("svg_fix.js");
const LOGO_MOTION: &str = include_str!("logo_motion.js");
const VALVE: &str = include_str!("valve.js");
const COVERS: &str = include_str!("covers.js");
const MINI_PAGE: &str = include_str!("mini.html");
const LINE_HEIGHT: f64 = 76.0;
const APP_ID: &str = soundscape_core::IDENTIFIER;

pub enum Wake {
    Servo,
    Poll,
    Push,
    Park(bool),
    Quit,
    Theme(Theme),
    Font(Option<String>),
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

struct Mini {
    webview: WebView,
    context: Rc<WindowRenderingContext>,
    window: Window,
    pointer: Cell<PhysicalPosition<f64>>,
}

impl Mini {
    fn point(&self) -> WebViewPoint {
        let at = self.pointer.get();
        WebViewPoint::Device(DevicePoint::new(at.x as f32, at.y as f32))
    }
}

struct MiniPage {
    shell: Weak<Shell>,
}

impl WebViewDelegate for MiniPage {
    fn notify_new_frame_ready(&self, _: WebView) {
        if let Some(mini) = self.shell.upgrade().and_then(|shell| shell.mini()) {
            mini.window.request_redraw();
        }
    }

    fn request_navigation(&self, _: WebView, request: NavigationRequest) {
        if request.url.scheme() == "data" {
            request.allow();
        } else {
            request.deny();
        }
    }

    fn notify_cursor_changed(&self, _: WebView, cursor: Cursor) {
        if let Some(mini) = self.shell.upgrade().and_then(|shell| shell.mini()) {
            mini.window.set_cursor(icon(cursor));
        }
    }

    fn show_console_message(&self, _: WebView, _: servo::ConsoleLogLevel, message: String) {
        if let (Some(shell), Some(asked)) =
            (self.shell.upgrade(), soundscape_core::mini::asked(&message))
        {
            soundscape_core::mini::act(&shell.app, asked);
        }
    }
}

fn add_font(scripts: &UserContentManager, family: Option<&str>) -> Rc<UserScript> {
    let script = Rc::new(UserScript::new(font::page_script(family), None));
    scripts.add_script(script.clone());
    script
}

fn mini_page() -> Url {
    Url::parse(&format!(
        "data:text/html;base64,{}",
        soundscape_core::mini::base64(MINI_PAGE.as_bytes())
    ))
    .expect("mini player page")
}

struct Shell {
    window: RefCell<Option<Rc<Window>>>,
    mini: RefCell<Option<Rc<Mini>>>,
    scripts: Rc<UserContentManager>,
    theme: Cell<Theme>,
    desktop_font: RefCell<Option<String>>,
    font_script: RefCell<Rc<UserScript>>,
    size: Cell<PhysicalSize<u32>>,
    maximized: Cell<bool>,
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
    memory_limit: Cell<u32>,
    armed: Cell<bool>,
    loaded_at: Cell<std::time::Instant>,
    hidden_since: Cell<Option<std::time::Instant>>,
    parked: RefCell<Option<Url>>,
    asking: Cell<bool>,
    park_after: std::time::Duration,
    storage: Option<std::path::PathBuf>,
    glide: RefCell<glide::Glide>,
    fling: RefCell<glide::Fling>,
    server: Option<Url>,
    app: soundscape_core::host::AppHandle,
    seen: std::sync::Arc<std::sync::Mutex<host::Seen>>,
    nonce: String,
    debug: bool,
    proxy: EventLoopProxy<Wake>,
    held: Cell<ModifiersState>,
    pointer: Cell<PhysicalPosition<f64>>,
}

fn notice() -> Url {
    soundscape_core::setup::page("", None)
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
    fn push_valve(&self) {
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(valve::flag_script(self.armed.get()), |_| {});
        }
    }

    fn server(&self) -> Option<Url> {
        self.app
            .state::<soundscape_core::AppState>()
            .server()
            .or_else(|| self.server.clone())
    }

    fn allowed(&self, url: &Url) -> bool {
        use soundscape_core::server::Navigation;
        if soundscape_core::setup::follow(&self.app, url) {
            return false;
        }
        match soundscape_core::server::classify(self.server().as_ref(), url) {
            Navigation::Allow => true,
            Navigation::Block => false,
            Navigation::AppSettings => {
                let _ = soundscape_core::open_settings(&self.app);
                false
            }
            Navigation::External => {
                soundscape_core::links::open(&self.app, url);
                false
            }
            Navigation::Connected { .. } => {
                soundscape_core::connected_through(&self.app, url);
                false
            }
        }
    }

    fn note_url(&self) {
        let Some(url) = self.webview().and_then(|webview| webview.url()) else {
            return;
        };
        if let Ok(mut seen) = self.seen.lock()
            && seen.url.as_ref() != Some(&url)
        {
            seen.url = Some(url);
        }
    }

    fn set_hidden(&self, hidden: bool) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.hidden = hidden;
        }
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
                if let Some(webview) = self.webview() {
                    webview.load(url);
                }
            }
            host::Call::OpenMini => self.open_mini(event_loop),
            host::Call::MiniEval(script) => {
                if let Some(mini) = self.mini() {
                    mini.webview.evaluate_javascript(script, |_| {});
                }
            }
            host::Call::CloseMini => self.close_mini(),
            host::Call::Setup => {
                if let Some(webview) = self.webview() {
                    webview.load(notice());
                }
            }
            host::Call::Main(work) => work(),
            host::Call::Smooth(on) => self.smooth.set(on),
            host::Call::Memory(mb) => {
                self.memory_limit.set(mb);
                self.check_memory();
            }
            host::Call::Restart => {
                RESTART.store(true, std::sync::atomic::Ordering::Relaxed);
                software::settled(self.storage.as_deref());
                self.close();
                event_loop.exit();
            }
            host::Call::Exit => {
                software::settled(self.storage.as_deref());
                self.close();
                event_loop.exit();
            }
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

    fn change_font(&self, family: Option<String>) {
        if *self.desktop_font.borrow() == family {
            return;
        }
        let fresh = add_font(&self.scripts, family.as_deref());
        let old = self.font_script.replace(fresh);
        self.scripts.remove_script(old);
        let restyle = font::restyle_script(family.as_deref());
        if let Some(webview) = self.webview() {
            webview.evaluate_javascript(restyle.clone(), |_| {});
        }
        if let Some(mini) = self.mini() {
            mini.webview.evaluate_javascript(restyle, |_| {});
        }
        *self.desktop_font.borrow_mut() = family;
    }

    fn mini(&self) -> Option<Rc<Mini>> {
        self.mini.borrow().clone()
    }

    fn open_mini(&self, event_loop: &ActiveEventLoop) {
        if let Some(mini) = self.mini() {
            mini.window.focus_window();
            return;
        }
        let Some(me) = self.me() else {
            return;
        };
        let attributes = Window::default_attributes()
            .with_title("Soundscape mini player")
            .with_inner_size(LogicalSize::new(420.0, 178.0))
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_window_icon(window_icon());
        let attributes = named(attributes, "mini");
        let Ok(window) = event_loop.create_window(attributes) else {
            return;
        };
        let (Ok(display), Ok(handle)) = (event_loop.display_handle(), window.window_handle())
        else {
            return;
        };
        let Ok(context) = WindowRenderingContext::new(display, handle, window.inner_size()) else {
            return;
        };
        let context = Rc::new(context);
        let mini_scripts = Rc::new(UserContentManager::new(&self.servo));
        add_font(&mini_scripts, self.desktop_font.borrow().as_deref());
        let webview = WebViewBuilder::new(&self.servo, context.clone())
            .url(mini_page())
            .user_content_manager(mini_scripts)
            .hidpi_scale_factor(Scale::new(window.scale_factor() as f32))
            .delegate(Rc::new(MiniPage {
                shell: Rc::downgrade(&me),
            }))
            .build();
        webview.notify_theme_change(self.theme.get());
        webview.set_focused(true);
        *self.mini.borrow_mut() = Some(Rc::new(Mini {
            webview,
            context,
            window,
            pointer: Cell::new(PhysicalPosition::new(0.0, 0.0)),
        }));
        if let Ok(mut seen) = self.seen.lock() {
            seen.mini = true;
        }
    }

    fn close_mini(&self) {
        let Some(mini) = self.mini.borrow_mut().take() else {
            return;
        };
        if let Ok(mut seen) = self.seen.lock() {
            seen.mini = false;
        }
        let _ = mini.context.take_window();
        drop(mini);
    }

    fn mini_event(&self, mini: &Mini, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close_mini(),
            WindowEvent::RedrawRequested => {
                mini.webview.paint();
                mini.context.present();
            }
            WindowEvent::Resized(size) => mini.webview.resize(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                mini.webview
                    .set_hidpi_scale_factor(Scale::new(scale_factor as f32));
            }
            WindowEvent::Focused(focused) => mini.webview.set_focused(focused),
            WindowEvent::ModifiersChanged(modifiers) => self.held.set(modifiers.state()),
            WindowEvent::KeyboardInput { event, .. } => {
                mini.webview
                    .notify_input_event(InputEvent::Keyboard(keys::event(&event, self.held.get())));
            }
            WindowEvent::CursorMoved { position, .. } => {
                mini.pointer.set(position);
                mini.webview
                    .notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(mini.point())));
            }
            WindowEvent::CursorLeft { .. } => {
                mini.webview
                    .notify_input_event(InputEvent::MouseLeftViewport(
                        MouseLeftViewportEvent::default(),
                    ));
            }
            WindowEvent::MouseInput {
                state,
                button: pressed,
                ..
            } => {
                mini.webview
                    .notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                        if state == ElementState::Pressed {
                            MouseButtonAction::Down
                        } else {
                            MouseButtonAction::Up
                        },
                        button(pressed),
                        mini.point(),
                    )));
            }
            _ => {}
        }
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

    fn remember_window(&self) {
        if let Some(window) = self.window() {
            self.note_size(&window);
        }
        let size = self.size.get();
        let maximized = self.maximized.get();
        self.app
            .state::<soundscape_core::AppState>()
            .update(|config| {
                config.window_size = Some((size.width, size.height));
                config.maximized = maximized;
            });
    }

    fn note_size(&self, window: &Window) {
        let maximized = window.is_maximized();
        self.maximized.set(maximized);
        if !maximized {
            self.size.set(window.inner_size());
        }
    }

    fn close(&self) {
        self.remember_window();
        self.close_mini();
        self.select.borrow_mut().take();
        self.waiting.borrow_mut().clear();
        self.openers.borrow_mut().clear();
        self.webview.borrow_mut().take();
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
        if self.hidden_since.get().is_none() {
            self.hidden_since.set(Some(std::time::Instant::now()));
        }
        if self.window().is_some() {
            self.remember_window();
        }
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
            let Ok(window) =
                event_loop.create_window(attributes(Some(self.size.get()), self.maximized.get()))
            else {
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
                let _ = soundscape_core::mini::open(&self.app);
            }
            WinitKey::Character(letter) if held.control_key() && letter.as_str() == "," => {
                soundscape_core::settings_page::open(&self.app)
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
        if let Ok(mut seen) = self.seen.lock() {
            seen.url = Some(url);
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

    fn show_console_message(&self, _: WebView, level: servo::ConsoleLogLevel, message: String) {
        if let Some(asked) = soundscape_core::native_link::asked(&message) {
            soundscape_core::native_link::from_page(&self.app, asked);
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
        soundscape_core::media::poke();
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
        let saved = launched.app.state::<soundscape_core::AppState>().config();
        let saved_size = saved
            .window_size
            .filter(|(width, height)| *width >= 200 && *height >= 200)
            .map(|(width, height)| PhysicalSize::new(width, height));
        let window = event_loop
            .create_window(attributes(saved_size, saved.maximized))
            .expect("window");
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
        scripts.add_script(Rc::new(UserScript::new(
            soundscape_core::links::SAME_WINDOW.into(),
            None,
        )));
        if let Some(native) = soundscape_core::native_link::script_for(&launched.app) {
            scripts.add_script(Rc::new(UserScript::new(native, None)));
        }
        let desktop_font = font::desktop();
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
            mini: RefCell::new(None),
            scripts: scripts.clone(),
            theme: Cell::new(profile.theme),
            desktop_font: RefCell::new(desktop_font),
            font_script: RefCell::new(font_script),
            size: Cell::new(size),
            maximized: Cell::new(saved.maximized),
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
            memory_limit: Cell::new(profile.memory_limit),
            armed: Cell::new(false),
            loaded_at: Cell::new(std::time::Instant::now()),
            hidden_since: Cell::new(None),
            parked: RefCell::new(None),
            asking: Cell::new(false),
            park_after: valve::park_after(std::env::var("SOUNDSCAPE_PARK_SECONDS").ok().as_deref()),
            storage: profile.storage.clone(),
            glide: RefCell::new(glide::Glide::default()),
            fling: RefCell::new(glide::Fling::default()),
            server: profile.server.clone(),
            app: launched.app.clone(),
            seen: launched.seen.clone(),
            nonce: nonce(),
            debug: profile::wants(std::env::var("SOUNDSCAPE_DEBUG").ok().as_deref()),
            proxy: proxy.clone(),
            held: Cell::new(ModifiersState::empty()),
            pointer: Cell::new(PhysicalPosition::new(0.0, 0.0)),
        });
        ME.with(|me| *me.borrow_mut() = Rc::downgrade(&shell));
        let start = profile::given_server()
            .and(profile.server.clone())
            .or_else(|| soundscape_core::first_page(&launched.app))
            .or_else(|| profile.server.clone())
            .unwrap_or_else(notice);
        if let Ok(mut seen) = launched.seen.lock() {
            seen.url = Some(start.clone());
        }
        let webview = WebViewBuilder::new(&shell.servo, shell.context.clone())
            .url(start)
            .hidpi_scale_factor(Scale::new(
                shell.window().map_or(1.0, |window| window.scale_factor()) as f32,
            ))
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
        if std::env::var_os("SOUNDSCAPE_FONT").is_none() {
            let sender = proxy.clone();
            font::watch(move |family| {
                let _ = sender.send_event(Wake::Font(family));
            });
        }
        shell
    }
}

impl App {
    fn begin(&mut self, event_loop: &ActiveEventLoop) {
        let App::Starting(proxy, profile, launched) = std::mem::replace(self, App::Stopped) else {
            return;
        };
        if let Ok(mut seen) = launched.seen.lock() {
            seen.hidden = false;
        }
        *self = App::Running(App::start(event_loop, proxy, profile, launched));
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let App::Starting(_, _, launched) = self
            && !launched.hidden
        {
            self.begin(event_loop);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, wake: Wake) {
        if let App::Starting(..) = self {
            let wanted = match &wake {
                Wake::Quit | Wake::Call(host::Call::Exit) => {
                    event_loop.exit();
                    return;
                }
                Wake::Call(host::Call::Restart) => {
                    RESTART.store(true, std::sync::atomic::Ordering::Relaxed);
                    event_loop.exit();
                    return;
                }
                Wake::Call(
                    host::Call::Show
                    | host::Call::Focus
                    | host::Call::Navigate(_)
                    | host::Call::Setup
                    | host::Call::OpenMini,
                ) => true,
                _ => false,
            };
            if !wanted {
                if let Wake::Call(host::Call::Evaluate(_, reply)) = wake {
                    reply(None);
                }
                return;
            }
            self.begin(event_loop);
        }
        let App::Running(shell) = self else {
            return;
        };
        match wake {
            Wake::Servo => shell.servo.spin_event_loop(),
            Wake::Poll => {
                shell.note_url();
                shell.check_memory();
                shell.check_hidden();
            }
            Wake::Push => shell.push_valve(),
            Wake::Call(call) => shell.obey(call, event_loop),
            Wake::Park(playing) => shell.park(playing),
            Wake::Quit => {
                software::settled(shell.storage.as_deref());
                shell.close();
                event_loop.exit();
            }
            Wake::Theme(theme) => {
                shell.theme.set(theme);
                shell
                    .servo
                    .set_preference("shell_background_color_rgba", theme::backdrop(theme).into());
                if let Some(webview) = shell.webview() {
                    webview.notify_theme_change(theme);
                }
                if let Some(mini) = shell.mini() {
                    mini.webview.notify_theme_change(theme);
                }
            }
            Wake::Answer(id, answer) => shell.answer(id, answer),
            Wake::Font(family) => shell.change_font(family),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let App::Running(shell) = self else {
            return;
        };
        shell.servo.spin_event_loop();
        if let Some(mini) = shell.mini().filter(|mini| mini.window.id() == id) {
            shell.mini_event(&mini, event);
            return;
        }
        let Some(window) = shell.window().filter(|window| window.id() == id) else {
            return;
        };
        let Some(webview) = shell.webview() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested if soundscape_core::window_closed(&shell.app) => {
                shell.hide_window();
            }
            WindowEvent::CloseRequested => {
                software::settled(shell.storage.as_deref());
                shell.close();
                event_loop.exit();
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

fn attributes(size: Option<PhysicalSize<u32>>, maximized: bool) -> WindowAttributes {
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
        .with_window_icon(window_icon())
}

fn named(attributes: WindowAttributes, instance: &str) -> WindowAttributes {
    let attributes = WindowAttributesExtWayland::with_name(attributes, APP_ID, instance);
    WindowAttributesExtX11::with_name(attributes, APP_ID, instance)
}

fn window_icon() -> Option<Icon> {
    soundscape_core::tray::window_picture()
        .and_then(|shown| Icon::from_rgba(shown.rgba, shown.width, shown.height).ok())
}

struct Launched {
    hidden: bool,
    app: soundscape_core::host::AppHandle,
    seen: std::sync::Arc<std::sync::Mutex<host::Seen>>,
}

fn main() {
    use soundscape_core::host::async_runtime;
    use soundscape_core::instance::{self, Claim};
    let args: Vec<String> = std::env::args().collect();
    let background = args
        .iter()
        .any(|arg| arg == soundscape_core::autostart::BACKGROUND_FLAG);
    let waiting: std::sync::Arc<std::sync::OnceLock<soundscape_core::host::AppHandle>> =
        Default::default();
    let caller = waiting.clone();
    let instance = match async_runtime::block_on(instance::claim(Box::new(move || {
        if let Some(app) = caller.get() {
            soundscape_core::show(app);
        }
    }))) {
        Claim::Running(connection) => {
            if !background {
                async_runtime::block_on(instance::wake(&connection));
            }
            return;
        }
        Claim::First(connection) => connection,
    };
    let mut profile = profile::load();
    let config_dir = profile.config.clone();
    profile.cpu_rendering =
        software::guard(profile.storage.as_deref(), profile.cpu_rendering, |now| {
            if let Some(dir) = &config_dir {
                let mut config = soundscape_core::config::Config::load(dir);
                config.cpu_rendering = now;
                let _ = config.save(dir);
            }
        });
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
    let seen = std::sync::Arc::new(std::sync::Mutex::new(host::Seen::default()));
    let dir = profile
        .config
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(soundscape_core::IDENTIFIER));
    let app = soundscape_core::launch(
        Box::new(host::ServoHost {
            proxy: proxy.clone(),
            seen: seen.clone(),
            running: soundscape_core::host::Running {
                cpu_rendering: profile.cpu_rendering,
                faster_scripts: profile.faster_scripts,
            },
        }),
        dir,
        soundscape_core::host::Running {
            cpu_rendering: profile.cpu_rendering,
            faster_scripts: profile.faster_scripts,
        },
    );
    let hidden =
        soundscape_core::starts_hidden(&args, &app.state::<soundscape_core::AppState>().config());
    let _ = waiting.set(app.clone());
    if hidden && let Ok(mut held) = seen.lock() {
        held.hidden = true;
    }
    let launched = Launched { hidden, app, seen };
    let mut app = App::Starting(proxy, profile, launched);
    if let Err(error) = event_loop.run_app(&mut app) {
        eprintln!("Soundscape stopped: {error}");
    }
    drop(app);
    software::settled(storage.as_deref());
    if let Some(connection) = instance {
        let _ = async_runtime::block_on(connection.release_name(soundscape_core::IDENTIFIER));
    }
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
