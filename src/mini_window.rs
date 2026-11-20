use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
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
const HEIGHT: f32 = 178.0;
const PAD: f32 = 14.0;
const COVER: f32 = 96.0;
const GAP: f32 = 4.0;
const LINE: f32 = 20.0;
const SMALL_LINE: f32 = 17.0;
const SEEK_ROW: f32 = 18.0;
const BUTTON_HEIGHT: f32 = 30.0;
const BUTTON_WIDTH: f32 = 38.0;
const MODE_WIDTH: f32 = 32.0;
const BUTTON_GAP: f32 = 6.0;
const TICK: Duration = Duration::from_millis(250);

const SHUFFLE: &str = r#"<path d="M3 7h3.5c2 0 3.2 1 4.3 2.7l2.4 4.6c1.1 1.7 2.3 2.7 4.3 2.7H20" /><path d="M3 17h3.5c1.3 0 2.3-.4 3.1-1.2M13.4 8.2c.8-.8 1.8-1.2 3.1-1.2H20" /><path d="M17.5 4.5 20 7l-2.5 2.5M17.5 14.5 20 17l-2.5 2.5" />"#;
const PREVIOUS: &str = r#"<path d="M6 5h2v14H6zM20 5v14L9 12z" />"#;
const PLAY: &str = r#"<path d="M7 4v16l13-8z" />"#;
const PAUSE: &str = r#"<path d="M6 4h4v16H6zM14 4h4v16h-4z" />"#;
const NEXT: &str = r#"<path d="M16 5h2v14h-2zM4 5v14l11-7z" />"#;
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
        let art = view.art.clone();
        self.view = view;
        self.received = Instant::now();
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
        let share = ((self.pointer.x as f32 - rect.left()) / rect.width()).clamp(0.0, 1.0);
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
                let pointing = self.dragging.is_some() || self.spot_at(position).is_some();
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
                        self.ask(action, None);
                    }
                    None => {}
                },
                ElementState::Released => {
                    if let Some(at) = self.dragging.take() {
                        self.view.position_ms = Some(at);
                        self.received = Instant::now();
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
    field: Color,
    border: Color,
}

fn colors(dark: bool) -> Colors {
    let rgb =
        |value: u32| Color::from_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255);
    if dark {
        Colors {
            background: rgb(0x0a0e14),
            text: rgb(0xeeeeee),
            muted: rgb(0x9aa3ae),
            accent: rgb(0x1db151),
            error: rgb(0xff8a80),
            field: rgb(0x151b23),
            border: rgb(0x2c333d),
        }
    } else {
        Colors {
            background: rgb(0xffffff),
            text: rgb(0x111111),
            muted: rgb(0x666666),
            accent: rgb(0x18a34a),
            error: rgb(0xc62828),
            field: rgb(0xf4f4f2),
            border: rgb(0xd6d5ce),
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

struct Fonts {
    regular: Vec<FontVec>,
    bold: Vec<FontVec>,
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
        let pick = |weight: fontdb::Weight| {
            let mut found = Vec::new();
            let mut take = |families: &[fontdb::Family]| {
                let query = fontdb::Query {
                    families,
                    weight,
                    ..fontdb::Query::default()
                };
                if let Some(id) = database.query(&query)
                    && let Some(Some(font)) = database.with_face_data(id, |data, index| {
                        FontVec::try_from_vec_and_index(data.to_vec(), index).ok()
                    })
                {
                    found.push(font);
                }
            };
            take(&families);
            for name in fallbacks {
                take(&[fontdb::Family::Name(name)]);
            }
            if found.is_empty()
                && let Some(face) = database.faces().next()
                && let Some(Some(font)) = database.with_face_data(face.id, |data, index| {
                    FontVec::try_from_vec_and_index(data.to_vec(), index).ok()
                })
            {
                found.push(font);
            }
            found
        };
        Fonts {
            regular: pick(fontdb::Weight::NORMAL),
            bold: pick(fontdb::Weight::BOLD),
        }
    }
}

struct Glyph<'a> {
    font: &'a FontVec,
    id: ab_glyph::GlyphId,
    x: f32,
}

fn lay_out<'a>(fonts: &'a [FontVec], text: &str, size: f32) -> (Vec<Glyph<'a>>, f32) {
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
        let scaled = font.as_scaled(PxScale::from(size));
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

fn fitted(fonts: &[FontVec], text: &str, size: f32, room: f32) -> String {
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

fn text(
    canvas: &mut Pixmap,
    fonts: &[FontVec],
    words: &str,
    size: f32,
    at: (f32, f32),
    color: Color,
) {
    let (glyphs, _) = lay_out(fonts, words, size);
    for glyph in glyphs {
        let placed = glyph
            .id
            .with_scale_and_position(PxScale::from(size), ab_glyph::point(at.0 + glyph.x, at.1));
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

fn baseline(fonts: &[FontVec], size: f32, top: f32, line: f32) -> f32 {
    let Some(font) = fonts.first() else {
        return top + line * 0.75;
    };
    let scaled = font.as_scaled(PxScale::from(size));
    let height = scaled.ascent() - scaled.descent();
    top + (line - height) / 2.0 + scaled.ascent()
}

fn draw(canvas: &mut Pixmap, player: &Player, scale: f32) -> Vec<(Spot, Rect, bool)> {
    let palette = colors(player.dark);
    canvas.fill(palette.background);
    let mut spots = Vec::new();
    let k = |value: f32| value * scale;
    let height = canvas.height() as f32 / scale;
    let cover_top = (height - COVER) / 2.0;
    if let Some(rect) = Rect::from_xywh(k(PAD), k(cover_top), k(COVER), k(COVER)) {
        match player
            .cover
            .as_ref()
            .and_then(|(_, picture)| picture.as_ref())
        {
            Some(picture) => {
                if let Some(path) = rounded(rect, k(8.0)) {
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
            None => fill(canvas, rect, k(8.0), palette.field),
        }
    }
    let Some(fonts) = player.fonts.as_ref() else {
        return spots;
    };
    let view = &player.view;
    let active = player.active();
    let left = PAD + COVER + PAD;
    let room = WIDTH - left - PAD;
    let up_next = view.up_next.as_ref().filter(|_| active);
    let mut total = LINE + GAP + LINE + GAP + SEEK_ROW + GAP + BUTTON_HEIGHT;
    if up_next.is_some() {
        total += GAP + SMALL_LINE;
    }
    if player.problem.is_some() {
        total += GAP + SMALL_LINE;
    }
    let mut top = ((height - total) / 2.0).max(0.0);
    let title = if active {
        view.title.as_str()
    } else {
        "Nothing playing"
    };
    let title = fitted(&fonts.bold, title, k(14.0), k(room));
    text(
        canvas,
        &fonts.bold,
        &title,
        k(14.0),
        (k(left), k(baseline(&fonts.bold, 14.0, top, LINE))),
        palette.text,
    );
    top += LINE + GAP;
    let artist = if active {
        format!("{} · {}", view.artist, view.source)
    } else {
        "Start a song in any player.".into()
    };
    let artist = fitted(&fonts.regular, &artist, k(14.0), k(room));
    text(
        canvas,
        &fonts.regular,
        &artist,
        k(14.0),
        (k(left), k(baseline(&fonts.regular, 14.0, top, LINE))),
        palette.muted,
    );
    top += LINE + GAP;

    let length = player.length();
    let at = player.position();
    let line = baseline(&fonts.regular, 12.0, top, SEEK_ROW);
    let now = clock(at);
    let total_time = clock(length);
    let now_width = lay_out(&fonts.regular, &now, 12.0).1;
    let total_width = lay_out(&fonts.regular, &total_time, 12.0).1;
    text(
        canvas,
        &fonts.regular,
        &now,
        k(12.0),
        (k(left), k(line)),
        palette.muted,
    );
    text(
        canvas,
        &fonts.regular,
        &total_time,
        k(12.0),
        (k(left + room - total_width), k(line)),
        palette.muted,
    );
    let bar_left = left + now_width + 6.0;
    let bar_width = (room - now_width - total_width - 12.0).max(10.0);
    let seekable = player.seekable();
    let dim = if seekable { 1.0 } else { 0.6 };
    if let Some(rect) = Rect::from_xywh(k(bar_left), k(top + 7.0), k(bar_width), k(4.0)) {
        fill(canvas, rect, k(2.0), faded(palette.border, dim));
    }
    let share = if length > 0 {
        (at as f32 / length as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if share > 0.0
        && let Some(rect) = Rect::from_xywh(k(bar_left), k(top + 7.0), k(bar_width * share), k(4.0))
    {
        fill(canvas, rect, k(2.0), faded(palette.accent, dim));
    }
    if let Some(rect) = Rect::from_xywh(k(bar_left), k(top + 2.0), k(bar_width), k(14.0)) {
        spots.push((Spot::Seek, rect, seekable));
    }
    top += SEEK_ROW + GAP;

    let repeat = view.repeat.as_deref().unwrap_or("off");
    let mut buttons: Vec<(&'static str, f32, String, bool, bool, bool)> = Vec::new();
    if let Some(on) = view.shuffle {
        buttons.push(("shuffle", MODE_WIDTH, SHUFFLE.into(), false, true, on));
    }
    buttons.push((
        "previous",
        BUTTON_WIDTH,
        PREVIOUS.into(),
        true,
        view.can_skip,
        false,
    ));
    buttons.push((
        "toggle",
        BUTTON_WIDTH,
        if view.playing { PAUSE } else { PLAY }.into(),
        true,
        active,
        false,
    ));
    buttons.push((
        "next",
        BUTTON_WIDTH,
        NEXT.into(),
        true,
        view.can_next,
        false,
    ));
    if view.repeat.is_some() {
        let shapes = if repeat == "one" {
            format!("{REPEAT}{REPEAT_ONE}")
        } else {
            REPEAT.into()
        };
        buttons.push(("repeat", MODE_WIDTH, shapes, false, true, repeat != "off"));
    }
    buttons.push((
        "love",
        BUTTON_WIDTH,
        HEART.into(),
        view.loved,
        view.can_love,
        view.loved,
    ));
    let mut x = left;
    for (action, width, shapes, solid, enabled, pressed) in buttons {
        let opacity = if enabled { 1.0 } else { 0.45 };
        let Some(rect) = Rect::from_xywh(k(x), k(top), k(width), k(BUTTON_HEIGHT)) else {
            continue;
        };
        let border = if pressed {
            palette.accent
        } else {
            palette.border
        };
        fill(canvas, rect, k(8.0), faded(border, opacity));
        if let Some(inner) = Rect::from_xywh(
            rect.left() + k(1.0),
            rect.top() + k(1.0),
            rect.width() - k(2.0),
            rect.height() - k(2.0),
        ) {
            fill(canvas, inner, k(7.0), faded(palette.field, opacity));
        }
        let mode = matches!(action, "shuffle" | "repeat");
        let tint = if pressed {
            palette.accent
        } else if mode {
            palette.muted
        } else {
            palette.text
        };
        icon(
            canvas,
            &shapes,
            solid,
            faded(tint, opacity),
            (k(x + width / 2.0), k(top + BUTTON_HEIGHT / 2.0)),
            k(16.0),
        );
        spots.push((Spot::Button(action), rect, enabled));
        x += width + BUTTON_GAP;
    }
    top += BUTTON_HEIGHT;
    if let Some(next) = up_next {
        top += GAP;
        let words = fitted(
            &fonts.regular,
            &format!("Up next: {next}"),
            k(12.0),
            k(room),
        );
        text(
            canvas,
            &fonts.regular,
            &words,
            k(12.0),
            (k(left), k(baseline(&fonts.regular, 12.0, top, SMALL_LINE))),
            palette.muted,
        );
        top += SMALL_LINE;
    }
    if let Some(problem) = &player.problem {
        top += GAP;
        let words = fitted(&fonts.regular, problem, k(12.0), k(room));
        text(
            canvas,
            &fonts.regular,
            &words,
            k(12.0),
            (k(left), k(baseline(&fonts.regular, 12.0, top, SMALL_LINE))),
            palette.error,
        );
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
