use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use resvg::tiny_skia::{
    Color, FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, Rect, SpreadMode,
    Transform,
};
use resvg::usvg;
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

const SHUFFLE: &str = r#"<path d="M3 7h3.5c2 0 3.2 1 4.3 2.7l2.4 4.6c1.1 1.7 2.3 2.7 4.3 2.7H20" /><path d="M3 17h3.5c1.3 0 2.3-.4 3.1-1.2M13.4 8.2c.8-.8 1.8-1.2 3.1-1.2H20" /><path d="M17.5 4.5 20 7l-2.5 2.5M17.5 14.5 20 17l-2.5 2.5" />"#;
const PREVIOUS: &str = r#"<rect x="5" y="5" width="2.5" height="14" rx="1" /><path d="M19 6.2v11.6a1 1 0 0 1-1.55.83L9.2 13.1a1.3 1.3 0 0 1 0-2.2l8.25-5.53A1 1 0 0 1 19 6.2z" />"#;
const PLAY: &str = r#"<path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" />"#;
const PAUSE: &str = r#"<rect x="6" y="5" width="4" height="14" rx="1" /><rect x="14" y="5" width="4" height="14" rx="1" />"#;
const NEXT: &str = r#"<rect x="16.5" y="5" width="2.5" height="14" rx="1" /><path d="M5 6.2v11.6a1 1 0 0 0 1.55.83l8.25-5.53a1.3 1.3 0 0 0 0-2.2L6.55 5.37A1 1 0 0 0 5 6.2z" />"#;
const REPEAT: &str = r#"<path d="M4 11V9.5A2.5 2.5 0 0 1 6.5 7H19M16.5 4.5 19 7l-2.5 2.5" /><path d="M20 13v1.5a2.5 2.5 0 0 1-2.5 2.5H5M7.5 19.5 5 17l2.5-2.5" />"#;
const REPEAT_ONE: &str = r#"<path d="M11.3 10.8 12.5 10v4" stroke-width="1.6" />"#;
const HEART: &str =
    r#"<path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z" />"#;

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
    let Ok(event_loop) = EventLoop::<Event>::with_user_event()
        .with_any_thread(true)
        .build()
    else {
        let _ = sender.send(None);
        return;
    };
    let proxy = event_loop.create_proxy();
    let _ = sender.send(Some(proxy.clone()));
    let dark = match crate::theme::chosen(std::env::var("SOUNDSCAPE_THEME").ok().as_deref()) {
        Some(theme) => theme == servo::Theme::Dark,
        None => {
            let watcher = proxy.clone();
            crate::theme::watch(move |theme| {
                let _ = watcher.send_event(Event::Dark(theme == servo::Theme::Dark));
            });
            crate::theme::current() == Some(servo::Theme::Dark)
        }
    };
    let mut player = Player {
        app,
        proxy,
        shown: None,
        fonts: None,
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
        OPEN.store(true, Ordering::Relaxed);
        let app = self.app.clone();
        async_runtime::spawn(async move { mini::refresh(&app).await });
    }

    fn close(&mut self) {
        OPEN.store(false, Ordering::Relaxed);
        self.dragging = None;
        self.shown = None;
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
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let size = shown.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if shown.surface.resize(width, height).is_err() {
            return;
        }
        let Some(mut canvas) = Pixmap::new(size.width, size.height) else {
            return;
        };
        let scale = shown.window.scale_factor() as f32;
        let spots = draw(&mut canvas, self, scale);
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let Ok(mut buffer) = shown.surface.buffer_mut() else {
            return;
        };
        for (out, pixel) in buffer.iter_mut().zip(canvas.pixels()) {
            *out = (u32::from(pixel.red()) << 16)
                | (u32::from(pixel.green()) << 8)
                | u32::from(pixel.blue());
        }
        let _ = buffer.present();
        self.spots = spots;
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
            self.redraw();
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + TICK));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

fn cover_pixmap(bytes: &[u8], size: u32) -> Option<Pixmap> {
    let size = size.max(1);
    let picture = image::load_from_memory(bytes).ok()?;
    let filled = picture.resize_to_fill(size, size, image::imageops::FilterType::Triangle);
    let rgba = filled.to_rgba8();
    let mut pixmap = Pixmap::new(rgba.width(), rgba.height())?;
    for (out, pixel) in pixmap.pixels_mut().iter_mut().zip(rgba.pixels()) {
        let [red, green, blue, alpha] = pixel.0;
        let premultiply = |channel: u8| ((u16::from(channel) * u16::from(alpha)) / 255) as u8;
        if let Some(color) = resvg::tiny_skia::PremultipliedColorU8::from_rgba(
            premultiply(red),
            premultiply(green),
            premultiply(blue),
            alpha,
        ) {
            *out = color;
        }
    }
    Some(pixmap)
}

struct Colors {
    background: Color,
    text: Color,
    muted: Color,
    accent: Color,
    error: Color,
}

fn colors(dark: bool) -> Colors {
    let rgb =
        |value: u32| Color::from_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255);
    if dark {
        Colors {
            background: rgb(0x111111),
            text: rgb(0xeeeeee),
            muted: rgb(0x999999),
            accent: rgb(0x1db954),
            error: rgb(0xff7b6b),
        }
    } else {
        Colors {
            background: rgb(0xffffff),
            text: rgb(0x111111),
            muted: rgb(0x666666),
            accent: rgb(0x1db954),
            error: rgb(0xc0392b),
        }
    }
}

fn faded(color: Color, opacity: f32) -> Color {
    let mut color = color;
    color.apply_opacity(opacity);
    color
}

fn hex(color: Color) -> String {
    let color = color.to_color_u8();
    format!(
        "#{:02x}{:02x}{:02x}",
        color.red(),
        color.green(),
        color.blue()
    )
}

pub fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn rounded(rect: Rect, radius: f32) -> Option<resvg::tiny_skia::Path> {
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let (left, top, right, bottom) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let mut path = PathBuilder::new();
    path.move_to(left + radius, top);
    path.line_to(right - radius, top);
    path.quad_to(right, top, right, top + radius);
    path.line_to(right, bottom - radius);
    path.quad_to(right, bottom, right - radius, bottom);
    path.line_to(left + radius, bottom);
    path.quad_to(left, bottom, left, bottom - radius);
    path.line_to(left, top + radius);
    path.quad_to(left, top, left + radius, top);
    path.close();
    path.finish()
}

fn fill(canvas: &mut Pixmap, rect: Rect, radius: f32, color: Color) {
    let Some(path) = rounded(rect, radius) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    canvas.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn icon(
    canvas: &mut Pixmap,
    shapes: &str,
    solid: bool,
    color: Color,
    center: (f32, f32),
    size: f32,
) {
    let color = hex(color);
    let look = if solid {
        format!(r#"fill="{color}" stroke="none""#)
    } else {
        format!(
            r#"fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round""#
        )
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24"><g {look}>{shapes}</g></svg>"#
    );
    let Ok(tree) = usvg::Tree::from_str(&svg, &usvg::Options::default()) else {
        return;
    };
    let zoom = size / 24.0;
    let transform = Transform::from_row(
        zoom,
        0.0,
        0.0,
        zoom,
        center.0 - size / 2.0,
        center.1 - size / 2.0,
    );
    resvg::render(&tree, transform, &mut canvas.as_mut());
}

type Face = FontRef<'static>;

struct Fonts {
    regular: Vec<Face>,
    bold: Vec<Face>,
}

fn mapped(path: &std::path::Path) -> Option<&'static [u8]> {
    let file = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(&Box::leak(Box::new(map))[..])
}

impl Fonts {
    fn load() -> Fonts {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let desktop = crate::font::desktop();
        let mut families: Vec<fontdb::Family> = Vec::new();
        if let Some(name) = desktop.as_deref() {
            families.push(fontdb::Family::Name(name));
        }
        for name in [
            "Cantarell",
            "Noto Sans",
            "DejaVu Sans",
            "Liberation Sans",
            "Inter",
        ] {
            families.push(fontdb::Family::Name(name));
        }
        let fallbacks = [
            "Noto Sans CJK JP",
            "Noto Sans CJK SC",
            "Droid Sans Fallback",
            "DejaVu Sans",
        ];
        let mut files: std::collections::HashMap<std::path::PathBuf, Option<&'static [u8]>> =
            std::collections::HashMap::new();
        let mut open = |id: fontdb::ID| -> Option<Face> {
            let face = database.face(id)?;
            let fontdb::Source::File(path) = &face.source else {
                return None;
            };
            let data = (*files.entry(path.clone()).or_insert_with(|| mapped(path)))?;
            FontRef::try_from_slice_and_index(data, face.index).ok()
        };
        let mut pick = |weight: fontdb::Weight| {
            let mut found: Vec<Face> = Vec::new();
            let mut chosen: Vec<fontdb::ID> = Vec::new();
            let mut ids = Vec::new();
            ids.push(database.query(&fontdb::Query {
                families: &families,
                weight,
                ..fontdb::Query::default()
            }));
            for name in fallbacks {
                ids.push(database.query(&fontdb::Query {
                    families: &[fontdb::Family::Name(name)],
                    weight,
                    ..fontdb::Query::default()
                }));
            }
            if ids.iter().all(Option::is_none) {
                ids.push(database.faces().next().map(|face| face.id));
            }
            for id in ids.into_iter().flatten() {
                if chosen.contains(&id) {
                    continue;
                }
                chosen.push(id);
                if let Some(face) = open(id) {
                    found.push(face);
                }
            }
            found
        };
        let regular = pick(fontdb::Weight::NORMAL);
        let bold = pick(fontdb::Weight::BOLD);
        Fonts { regular, bold }
    }
}

struct Glyph<'a> {
    font: &'a Face,
    id: ab_glyph::GlyphId,
    x: f32,
}

fn em(font: &Face, size: f32) -> PxScale {
    let units = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / units)
}

fn lay_out<'a>(fonts: &'a [Face], text: &str, size: f32) -> (Vec<Glyph<'a>>, f32) {
    let mut glyphs = Vec::new();
    let mut x = 0.0;
    let mut previous: Option<(usize, ab_glyph::GlyphId)> = None;
    for letter in text.chars() {
        let chosen = fonts
            .iter()
            .position(|font| font.glyph_id(letter).0 != 0)
            .unwrap_or(0);
        let Some(font) = fonts.get(chosen) else {
            break;
        };
        let scaled = font.as_scaled(em(font, size));
        let id = font.glyph_id(letter);
        if let Some((before, before_id)) = previous
            && before == chosen
        {
            x += scaled.kern(before_id, id);
        }
        glyphs.push(Glyph { font, id, x });
        x += scaled.h_advance(id);
        previous = Some((chosen, id));
    }
    (glyphs, x)
}

fn fitted(fonts: &[Face], text: &str, size: f32, room: f32) -> String {
    if lay_out(fonts, text, size).1 <= room {
        return text.to_string();
    }
    let mut kept: Vec<char> = text.chars().collect();
    while !kept.is_empty() {
        kept.pop();
        let trimmed: String = kept.iter().collect::<String>().trim_end().to_string() + "…";
        if lay_out(fonts, &trimmed, size).1 <= room {
            return trimmed;
        }
    }
    "…".into()
}

fn blend(canvas: &mut Pixmap, x: i32, y: i32, color: Color, coverage: f32) {
    if x < 0 || y < 0 || x >= canvas.width() as i32 || y >= canvas.height() as i32 {
        return;
    }
    let index = (y as u32 * canvas.width() + x as u32) as usize;
    let alpha = (coverage.clamp(0.0, 1.0) * color.alpha()).clamp(0.0, 1.0);
    let pixels = canvas.pixels_mut();
    let below = pixels[index];
    let mix = |over: f32, under: u8| {
        (over * 255.0 * alpha + f32::from(under) * (1.0 - alpha)).round() as u8
    };
    let out_alpha = (alpha * 255.0 + f32::from(below.alpha()) * (1.0 - alpha)).round() as u8;
    let red = mix(color.red(), below.red()).min(out_alpha);
    let green = mix(color.green(), below.green()).min(out_alpha);
    let blue = mix(color.blue(), below.blue()).min(out_alpha);
    if let Some(mixed) =
        resvg::tiny_skia::PremultipliedColorU8::from_rgba(red, green, blue, out_alpha)
    {
        pixels[index] = mixed;
    }
}

fn text(canvas: &mut Pixmap, fonts: &[Face], words: &str, size: f32, at: (f32, f32), color: Color) {
    let (glyphs, _) = lay_out(fonts, words, size);
    for glyph in glyphs {
        let placed = glyph
            .id
            .with_scale_and_position(em(glyph.font, size), ab_glyph::point(at.0 + glyph.x, at.1));
        if let Some(outline) = glyph.font.outline_glyph(placed) {
            let bounds = outline.px_bounds();
            outline.draw(|x, y, coverage| {
                blend(
                    canvas,
                    bounds.min.x as i32 + x as i32,
                    bounds.min.y as i32 + y as i32,
                    color,
                    coverage,
                );
            });
        }
    }
}

fn baseline(fonts: &[Face], size: f32, top: f32, line: f32) -> f32 {
    let Some(font) = fonts.first() else {
        return top + line * 0.75;
    };
    let scaled = font.as_scaled(em(font, size));
    let height = scaled.ascent() - scaled.descent();
    top + (line - height) / 2.0 + scaled.ascent()
}

fn circle(canvas: &mut Pixmap, center: (f32, f32), radius: f32, color: Color) {
    if let Some(rect) = Rect::from_xywh(
        center.0 - radius,
        center.1 - radius,
        radius * 2.0,
        radius * 2.0,
    ) {
        fill(canvas, rect, radius, color);
    }
}

fn centered(
    canvas: &mut Pixmap,
    fonts: &[Face],
    words: &str,
    size: f32,
    center: (f32, f32),
    color: Color,
) {
    let width = lay_out(fonts, words, size).1;
    let line = baseline(fonts, size, center.1 - size, size * 2.0);
    text(
        canvas,
        fonts,
        words,
        size,
        (center.0 - width / 2.0, line),
        color,
    );
}

fn draw(canvas: &mut Pixmap, player: &Player, scale: f32) -> Vec<(Spot, Rect, bool)> {
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
        icon(
            canvas,
            HEART,
            view.loved,
            tint,
            (k(love_center.0), k(love_center.1)),
            k(18.0),
        );
    }

    let Some(fonts) = player.fonts.as_ref() else {
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
    let title = fitted(&fonts.bold, title, k(16.0), k(room));
    text(
        canvas,
        &fonts.bold,
        &title,
        k(16.0),
        (k(left), k(baseline(&fonts.bold, 16.0, top, 22.0))),
        palette.text,
    );
    let (second, tint) = match &player.problem {
        Some(problem) => (problem.clone(), palette.error),
        None if active => (format!("{} · {}", view.artist, view.source), palette.muted),
        None => ("Start a song in any player.".into(), palette.muted),
    };
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
        icon(
            canvas,
            SHUFFLE,
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
    icon(
        canvas,
        PREVIOUS,
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
    icon(
        canvas,
        if view.playing { PAUSE } else { PLAY },
        true,
        palette.background,
        (k(toggle.0), k(toggle.1)),
        k(20.0),
    );
    let next = (middle + SKIP_STEP, row);
    button(canvas, Spot::Button("next"), next, BUTTON, view.can_next);
    icon(
        canvas,
        NEXT,
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
        let shapes = if mode == "one" {
            format!("{REPEAT}{REPEAT_ONE}")
        } else {
            REPEAT.into()
        };
        icon(
            canvas,
            &shapes,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_times_like_the_page_did() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(61_500), "1:01");
        assert_eq!(clock(3_600_000), "60:00");
    }

    #[test]
    fn rounds_corners_inside_the_box() {
        let rect = Rect::from_xywh(0.0, 0.0, 40.0, 30.0).unwrap();
        let path = rounded(rect, 50.0).unwrap();
        let bounds = path.bounds();
        assert_eq!((bounds.width(), bounds.height()), (40.0, 30.0));
    }

    #[test]
    fn crops_covers_to_a_square() {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(40, 20, image::Rgba([200, 10, 10, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let picture = cover_pixmap(&bytes, 16).unwrap();
        assert_eq!((picture.width(), picture.height()), (16, 16));
        assert_eq!(picture.pixels()[0].red(), 200);
    }
}
