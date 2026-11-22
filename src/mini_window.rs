use crate::mini_draw::{
    Fonts, Icon, Icons, baseline, centered, circle, clock, colors, cover_pixmap, faded, fill,
    fitted, rounded, text,
};
use resvg::tiny_skia::{
    FillRule, FilterQuality, Paint, Pattern, Pixmap, Rect, SpreadMode, Transform,
};
use soundscape_core::host::{AppHandle, async_runtime};
use soundscape_core::mini::{self, Asked, MiniView};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::wayland::EventLoopBuilderExtWayland;
use winit::platform::x11::EventLoopBuilderExtX11;
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 172.0;
const PAD: f32 = 16.0;
const COVER: f32 = 56.0;
const PLAY_SIZE: f32 = 38.0;
const BUTTON: f32 = 32.0;
const SKIP_STEP: f32 = 46.0;
const MODE_STEP: f32 = 92.0;
const TIME_WIDTH: f32 = 44.0;
const SEEK_ARRIVED_MS: u64 = 1500;
const SEEK_GIVE_UP: Duration = Duration::from_secs(4);
const TOGGLE_GIVE_UP: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);

enum Event {
    Open,
    Close,
    View(Box<MiniView>),
    Problem(String),
    Cover(String, Option<Pixmap>),
    Dark(bool),
}

struct Handle {
    proxy: Mutex<EventLoopProxy<Event>>,
}

static HANDLE: OnceLock<Option<Handle>> = OnceLock::new();
static OPEN: AtomicBool = AtomicBool::new(false);

fn handle(app: &AppHandle) -> Option<&'static Handle> {
    HANDLE
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            let app = app.clone();
            std::thread::Builder::new()
                .name("mini player".into())
                .spawn(move || run(app, sender))
                .ok()?;
            let proxy = receiver.recv().ok()??;
            Some(Handle {
                proxy: Mutex::new(proxy),
            })
        })
        .as_ref()
}

fn send(event: Event) {
    if let Some(Some(handle)) = HANDLE.get()
        && let Ok(proxy) = handle.proxy.lock()
    {
        let _ = proxy.send_event(event);
    }
}

pub fn open(app: &AppHandle) -> bool {
    let Some(handle) = handle(app) else {
        return false;
    };
    handle
        .proxy
        .lock()
        .is_ok_and(|proxy| proxy.send_event(Event::Open).is_ok())
}

pub fn close() {
    send(Event::Close);
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

pub fn show(view: &MiniView) {
    if is_open() {
        send(Event::View(Box::new(view.clone())));
    }
}

pub fn problem(text: &str) {
    if is_open() {
        send(Event::Problem(text.to_string()));
    }
}

fn run(app: AppHandle, sender: mpsc::Sender<Option<EventLoopProxy<Event>>>) {
    let mut builder = EventLoop::<Event>::with_user_event();
    EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
    EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
    let Ok(event_loop) = builder.build() else {
        let _ = sender.send(None);
        return;
    };
    let proxy = event_loop.create_proxy();
    let _ = sender.send(Some(proxy.clone()));
    let dark = match soundscape_core::theme::chosen_dark(
        std::env::var("SOUNDSCAPE_THEME").ok().as_deref(),
    ) {
        Some(dark) => dark,
        None => {
            let watcher = proxy.clone();
            let mut rx = soundscape_core::theme::subscribe();
            async_runtime::spawn(async move {
                while rx.changed().await.is_ok() {
                    if let Some(dark) = *rx.borrow_and_update()
                        && watcher.send_event(Event::Dark(dark)).is_err()
                    {
                        break;
                    }
                }
            });
            async_runtime::block_on(soundscape_core::theme::current_dark()).unwrap_or(false)
        }
    };
    let mut player = Player {
        app,
        proxy,
        shown: None,
        fonts: None,
        icons: Icons::default(),
        canvas: None,
        next_tick: None,
        view: MiniView::default(),
        received: Instant::now(),
        problem: None,
        cover: None,
        dark,
        pointer: PhysicalPosition::new(0.0, 0.0),
        dragging: None,
        seek_hold: None,
        toggle_hold: None,
        hover: None,
        spots: Vec::new(),
    };
    let _ = event_loop.run_app(&mut player);
}

struct Shown {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    _context: softbuffer::Context<Rc<Window>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Spot {
    Button(&'static str),
    Seek,
}

struct Player {
    app: AppHandle,
    proxy: EventLoopProxy<Event>,
    shown: Option<Shown>,
    fonts: Option<Fonts>,
    icons: Icons,
    canvas: Option<Pixmap>,
    next_tick: Option<Instant>,
    view: MiniView,
    received: Instant,
    problem: Option<String>,
    cover: Option<(String, Option<Pixmap>)>,
    dark: bool,
    pointer: PhysicalPosition<f64>,
    dragging: Option<u64>,
    seek_hold: Option<(u64, Instant)>,
    toggle_hold: Option<(bool, Instant)>,
    hover: Option<Spot>,
    spots: Vec<(Spot, Rect, bool)>,
}

impl Player {
    fn open(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(shown) = &self.shown {
            shown.window.focus_window();
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Soundscape mini player")
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_window_icon(crate::window_icon());
        let Ok(window) = event_loop.create_window(crate::named(attributes, "mini")) else {
            return;
        };
        let window = Rc::new(window);
        let Ok(context) = softbuffer::Context::new(window.clone()) else {
            return;
        };
        let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else {
            return;
        };
        if self.fonts.is_none() {
            self.fonts = Some(Fonts::load());
        }
        self.shown = Some(Shown {
            window,
            surface,
            _context: context,
        });
        self.problem = None;
        self.next_tick = None;
        OPEN.store(true, Ordering::Relaxed);
        soundscape_core::mini::wake();
        let app = self.app.clone();
        async_runtime::spawn(async move { mini::refresh(&app).await });
    }

    fn close(&mut self) {
        OPEN.store(false, Ordering::Relaxed);
        self.dragging = None;
        self.next_tick = None;
        self.canvas = None;
        self.shown = None;
        self.fonts = None;
        self.icons = Icons::default();
        self.cover = None;
        self.spots.clear();
        self.spots.shrink_to_fit();
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        unsafe {
            libc::malloc_trim(0);
        }
    }

    fn redraw(&self) {
        if let Some(shown) = &self.shown {
            shown.window.request_redraw();
        }
    }

    fn take_view(&mut self, view: MiniView) {
        let mut view = view;
        let art = view.art.clone();
        if view.title != self.view.title || view.kind != self.view.kind {
            self.seek_hold = None;
        }
        let mut since = Instant::now();
        if let Some((wanted, asked)) = self.toggle_hold {
            if view.playing == wanted || asked.elapsed() >= TOGGLE_GIVE_UP {
                self.toggle_hold = None;
            } else {
                view.playing = wanted;
            }
        }
        if let Some((target, asked)) = self.seek_hold {
            let waited = asked.elapsed();
            let expected = target
                + if view.playing {
                    waited.as_millis() as u64
                } else {
                    0
                };
            let arrived = view
                .position_ms
                .is_some_and(|at| at.abs_diff(expected) < SEEK_ARRIVED_MS);
            if arrived || waited >= SEEK_GIVE_UP {
                self.seek_hold = None;
            } else {
                view.position_ms = Some(target);
                since = asked;
            }
        }
        let same_song = view.title == self.view.title && view.kind == self.view.kind;
        if self.seek_hold.is_none() && same_song && view.playing && self.view.playing {
            let shown = self.position();
            if let Some(at) = view.position_ms
                && at < shown
                && shown - at < SEEK_ARRIVED_MS
            {
                view.position_ms = Some(shown);
                since = Instant::now();
            }
        }
        self.view = view;
        self.received = since;
        self.problem = None;
        match art {
            Some(art) if self.cover.as_ref().map(|(known, _)| known) != Some(&art) => {
                self.cover = Some((art.clone(), None));
                let size = self.scale() * COVER;
                let app = self.app.clone();
                let proxy = self.proxy.clone();
                async_runtime::spawn(async move {
                    let picture = mini::picture(&app, &art)
                        .await
                        .and_then(|bytes| cover_pixmap(&bytes, size.round() as u32));
                    let _ = proxy.send_event(Event::Cover(art, picture));
                });
            }
            Some(_) => {}
            None => self.cover = None,
        }
        self.redraw();
    }

    fn scale(&self) -> f32 {
        self.shown
            .as_ref()
            .map_or(1.0, |shown| shown.window.scale_factor() as f32)
    }

    fn active(&self) -> bool {
        !matches!(self.view.kind, "none" | "")
    }

    fn length(&self) -> u64 {
        self.view.length_ms.unwrap_or(0)
    }

    fn seekable(&self) -> bool {
        self.view.can_seek && self.length() > 0
    }

    fn position(&self) -> u64 {
        if let Some(at) = self.dragging {
            return at;
        }
        let mut at = self.view.position_ms.unwrap_or(0);
        if self.view.playing {
            at += self.received.elapsed().as_millis() as u64;
        }
        match self.length() {
            0 => at,
            length => at.min(length),
        }
    }

    fn spot_at(&self, at: PhysicalPosition<f64>) -> Option<(Spot, Rect)> {
        let (x, y) = (at.x as f32, at.y as f32);
        self.spots
            .iter()
            .find(|(_, rect, enabled)| {
                *enabled
                    && x >= rect.left()
                    && x <= rect.right()
                    && y >= rect.top()
                    && y <= rect.bottom()
            })
            .map(|(spot, rect, _)| (*spot, *rect))
    }

    fn seek_point(&self, rect: Rect) -> u64 {
        let inset = 7.0 * self.scale();
        let share = ((self.pointer.x as f32 - rect.left() - inset) / (rect.width() - 2.0 * inset))
            .clamp(0.0, 1.0);
        (share as f64 * self.length() as f64) as u64
    }

    fn ask(&self, action: &str, seconds: Option<f64>) {
        mini::act(
            &self.app,
            Asked {
                action: action.into(),
                seconds,
            },
        );
    }

    fn paint(&mut self) {
        let Some((size, scale)) = self.shown.as_mut().and_then(|shown| {
            let size = shown.window.inner_size();
            let (width, height) = (NonZeroU32::new(size.width)?, NonZeroU32::new(size.height)?);
            shown.surface.resize(width, height).ok()?;
            Some((size, shown.window.scale_factor() as f32))
        }) else {
            return;
        };
        let Some(mut canvas) = self
            .canvas
            .take()
            .filter(|pixmap| pixmap.width() == size.width && pixmap.height() == size.height)
            .or_else(|| Pixmap::new(size.width, size.height))
        else {
            return;
        };
        let mut icons = std::mem::take(&mut self.icons);
        let mut fonts = self.fonts.take();
        let spots = draw(&mut canvas, &mut icons, fonts.as_mut(), self, scale);
        self.icons = icons;
        self.fonts = fonts;
        if let Some(shown) = self.shown.as_mut()
            && let Ok(mut buffer) = shown.surface.buffer_mut()
        {
            for (out, pixel) in buffer.iter_mut().zip(canvas.pixels()) {
                *out = (u32::from(pixel.red()) << 16)
                    | (u32::from(pixel.green()) << 8)
                    | u32::from(pixel.blue());
            }
            let _ = buffer.present();
            self.spots = spots;
        }
        self.canvas = Some(canvas);
        if self.view.playing && self.dragging.is_none() {
            self.next_tick = Some(Instant::now() + TICK);
        } else {
            self.next_tick = None;
        }
    }
}

impl ApplicationHandler<Event> for Player {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Open => self.open(event_loop),
            Event::Close => self.close(),
            Event::View(view) => self.take_view(*view),
            Event::Problem(text) => {
                self.problem = Some(text);
                self.redraw();
            }
            Event::Cover(art, picture) => {
                if self.cover.as_ref().map(|(known, _)| known) == Some(&art) {
                    self.cover = Some((art, picture));
                    self.redraw();
                }
            }
            Event::Dark(dark) => {
                self.dark = dark;
                self.redraw();
            }
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close(),
            WindowEvent::CursorLeft { .. } => {
                if self.hover.take().is_some() {
                    self.redraw();
                }
            }
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::ScaleFactorChanged { .. } => {
                self.cover = None;
                let view = self.view.clone();
                self.take_view(view);
            }
            WindowEvent::Resized(_) => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = position;
                if self.dragging.is_some()
                    && let Some((_, rect, _)) =
                        self.spots.iter().find(|(spot, _, _)| *spot == Spot::Seek)
                {
                    self.dragging = Some(self.seek_point(*rect));
                    self.redraw();
                }
                let over = self.spot_at(position).map(|(spot, _)| spot);
                if over != self.hover {
                    self.hover = over;
                    self.redraw();
                }
                let pointing = self.dragging.is_some() || over.is_some();
                if let Some(shown) = &self.shown {
                    shown.window.set_cursor(if pointing {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    });
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => match self.spot_at(self.pointer) {
                    Some((Spot::Seek, rect)) => {
                        self.dragging = Some(self.seek_point(rect));
                        self.redraw();
                    }
                    Some((Spot::Button(action), _)) => {
                        self.problem = None;
                        if action == "toggle" {
                            let at = self.position();
                            self.view.position_ms = Some(at);
                            self.received = Instant::now();
                            self.view.playing = !self.view.playing;
                            self.toggle_hold = Some((self.view.playing, self.received));
                            self.redraw();
                        }
                        self.ask(action, None);
                    }
                    None => {}
                },
                ElementState::Released => {
                    if let Some(at) = self.dragging.take() {
                        self.view.position_ms = Some(at);
                        self.received = Instant::now();
                        self.seek_hold = Some((at, self.received));
                        self.ask("seek", Some(at as f64 / 1000.0));
                        self.redraw();
                    }
                }
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.shown.is_some() && self.view.playing && self.dragging.is_none() {
            let now = Instant::now();
            match self.next_tick {
                Some(due) if now < due => {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(due));
                }
                _ => {
                    self.redraw();
                    let due = now + TICK;
                    self.next_tick = Some(due);
                    event_loop.set_control_flow(ControlFlow::WaitUntil(due));
                }
            }
        } else {
            self.next_tick = None;
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn draw(
    canvas: &mut Pixmap,
    icons: &mut Icons,
    fonts: Option<&mut Fonts>,
    player: &Player,
    scale: f32,
) -> Vec<(Spot, Rect, bool)> {
    let palette = colors(player.dark);
    canvas.fill(palette.background);
    let mut spots = Vec::new();
    let k = |value: f32| value * scale;
    let view = &player.view;
    let active = player.active();
    let hovered = |spot: Spot| player.hover == Some(spot);
    let mut button =
        |canvas: &mut Pixmap, spot: Spot, center: (f32, f32), size: f32, enabled: bool| {
            let rect = Rect::from_xywh(
                k(center.0 - size / 2.0),
                k(center.1 - size / 2.0),
                k(size),
                k(size),
            );
            if let Some(rect) = rect {
                if enabled && hovered(spot) {
                    fill(canvas, rect, k(size / 2.0), faded(palette.muted, 0.15));
                }
                spots.push((spot, rect, enabled));
            }
        };

    if let Some(rect) = Rect::from_xywh(k(PAD), k(PAD), k(COVER), k(COVER)) {
        match player
            .cover
            .as_ref()
            .and_then(|(_, picture)| picture.as_ref())
        {
            Some(picture) => {
                if let Some(path) = rounded(rect, k(4.0)) {
                    let paint = Paint {
                        anti_alias: true,
                        shader: Pattern::new(
                            picture.as_ref(),
                            SpreadMode::Pad,
                            FilterQuality::Bilinear,
                            1.0,
                            Transform::from_scale(
                                rect.width() / picture.width() as f32,
                                rect.height() / picture.height() as f32,
                            )
                            .post_translate(rect.left(), rect.top()),
                        ),
                        ..Paint::default()
                    };
                    canvas.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
            None => fill(canvas, rect, k(4.0), faded(palette.muted, 0.15)),
        }
    }

    let love_center = (WIDTH - PAD - BUTTON / 2.0, PAD + COVER / 2.0);
    let shown_love = active && (view.can_love || view.loved);
    if shown_love {
        button(
            canvas,
            Spot::Button("love"),
            love_center,
            BUTTON,
            view.can_love,
        );
        let tint = if view.loved {
            palette.accent
        } else {
            palette.muted
        };
        icons.draw(
            canvas,
            Icon::Heart,
            view.loved,
            tint,
            (k(love_center.0), k(love_center.1)),
            k(18.0),
        );
    }

    let Some(fonts) = fonts else {
        return spots;
    };
    let left = PAD + COVER + 12.0;
    let room = WIDTH - left - PAD - if shown_love { BUTTON + 8.0 } else { 0.0 };
    let top = PAD + (COVER - 44.0) / 2.0;
    let title = if active {
        view.title.as_str()
    } else {
        "Nothing playing"
    };
    let (second, tint) = match &player.problem {
        Some(problem) => (problem.clone(), palette.error),
        None if active => (format!("{} · {}", view.artist, view.source), palette.muted),
        None => ("Start a song in any player.".into(), palette.muted),
    };
    fonts.ensure_fallbacks_for(title);
    fonts.ensure_fallbacks_for(&second);
    let title = fitted(&fonts.bold, title, k(16.0), k(room));
    text(
        canvas,
        &fonts.bold,
        &title,
        k(16.0),
        (k(left), k(baseline(&fonts.bold, 16.0, top, 22.0))),
        palette.text,
    );
    let second = fitted(&fonts.regular, &second, k(15.0), k(room));
    text(
        canvas,
        &fonts.regular,
        &second,
        k(15.0),
        (k(left), k(baseline(&fonts.regular, 15.0, top + 22.0, 22.0))),
        tint,
    );

    let middle = WIDTH / 2.0;
    let row = PAD + COVER + 14.0 + PLAY_SIZE / 2.0;
    if let Some(on) = view.shuffle {
        let center = (middle - MODE_STEP, row);
        button(canvas, Spot::Button("shuffle"), center, BUTTON, true);
        let tint = if on { palette.accent } else { palette.muted };
        icons.draw(
            canvas,
            Icon::Shuffle,
            false,
            tint,
            (k(center.0), k(center.1)),
            k(18.0),
        );
        if on {
            circle(
                canvas,
                (k(center.0), k(center.1 + 13.0)),
                k(2.0),
                palette.accent,
            );
        }
    }
    let previous = (middle - SKIP_STEP, row);
    button(
        canvas,
        Spot::Button("previous"),
        previous,
        BUTTON,
        view.can_skip,
    );
    icons.draw(
        canvas,
        Icon::Previous,
        true,
        faded(palette.text, if view.can_skip { 1.0 } else { 0.4 }),
        (k(previous.0), k(previous.1)),
        k(20.0),
    );
    let toggle = (middle, row);
    button(canvas, Spot::Button("toggle"), toggle, PLAY_SIZE, active);
    let grow = if active && hovered(Spot::Button("toggle")) {
        1.06
    } else {
        1.0
    };
    circle(
        canvas,
        (k(toggle.0), k(toggle.1)),
        k(PLAY_SIZE / 2.0 * grow),
        faded(palette.text, if active { 1.0 } else { 0.4 }),
    );
    icons.draw(
        canvas,
        if view.playing {
            Icon::Pause
        } else {
            Icon::Play
        },
        true,
        palette.background,
        (k(toggle.0), k(toggle.1)),
        k(20.0),
    );
    let next = (middle + SKIP_STEP, row);
    button(canvas, Spot::Button("next"), next, BUTTON, view.can_next);
    icons.draw(
        canvas,
        Icon::Next,
        true,
        faded(palette.text, if view.can_next { 1.0 } else { 0.4 }),
        (k(next.0), k(next.1)),
        k(20.0),
    );
    if let Some(mode) = view.repeat.as_deref() {
        let center = (middle + MODE_STEP, row);
        button(canvas, Spot::Button("repeat"), center, BUTTON, true);
        let on = mode != "off";
        let tint = if on { palette.accent } else { palette.muted };
        let icon = if mode == "one" {
            Icon::RepeatOne
        } else {
            Icon::Repeat
        };
        icons.draw(
            canvas,
            icon,
            false,
            tint,
            (k(center.0), k(center.1)),
            k(18.0),
        );
        if on {
            circle(
                canvas,
                (k(center.0), k(center.1 + 13.0)),
                k(2.0),
                palette.accent,
            );
        }
    }

    let seek_row = row + PLAY_SIZE / 2.0 + 12.0 + 10.0;
    let length = player.length();
    let at = player.position();
    centered(
        canvas,
        &fonts.regular,
        &clock(at),
        k(12.0),
        (k(PAD + TIME_WIDTH / 2.0), k(seek_row)),
        palette.muted,
    );
    centered(
        canvas,
        &fonts.regular,
        &clock(length),
        k(12.0),
        (k(WIDTH - PAD - TIME_WIDTH / 2.0), k(seek_row)),
        palette.muted,
    );
    let bar_left = PAD + TIME_WIDTH + 4.0;
    let bar_width = WIDTH - 2.0 * (PAD + TIME_WIDTH + 4.0);
    let seekable = player.seekable();
    let share = if length > 0 {
        (at as f32 / length as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if let Some(rect) = Rect::from_xywh(k(bar_left), k(seek_row - 2.0), k(bar_width), k(4.0)) {
        fill(canvas, rect, k(2.0), faded(palette.muted, 0.35));
    }
    if share > 0.0
        && let Some(rect) =
            Rect::from_xywh(k(bar_left), k(seek_row - 2.0), k(bar_width * share), k(4.0))
    {
        fill(canvas, rect, k(2.0), palette.accent);
    }
    if seekable {
        let knob = (k(bar_left + bar_width * share), k(seek_row));
        circle(canvas, knob, k(7.0), palette.background);
        circle(canvas, knob, k(5.0), palette.accent);
    }
    if let Some(rect) = Rect::from_xywh(
        k(bar_left - 7.0),
        k(seek_row - 10.0),
        k(bar_width + 14.0),
        k(20.0),
    ) {
        spots.push((Spot::Seek, rect, seekable));
    }
    spots
}
