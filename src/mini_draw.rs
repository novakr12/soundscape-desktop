use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use resvg::tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
use resvg::usvg;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SHUFFLE: &str = r#"<path d="M3 7h3.5c2 0 3.2 1 4.3 2.7l2.4 4.6c1.1 1.7 2.3 2.7 4.3 2.7H20" /><path d="M3 17h3.5c1.3 0 2.3-.4 3.1-1.2M13.4 8.2c.8-.8 1.8-1.2 3.1-1.2H20" /><path d="M17.5 4.5 20 7l-2.5 2.5M17.5 14.5 20 17l-2.5 2.5" />"#;
const PREVIOUS: &str = r#"<rect x="5" y="5" width="2.5" height="14" rx="1" /><path d="M19 6.2v11.6a1 1 0 0 1-1.55.83L9.2 13.1a1.3 1.3 0 0 1 0-2.2l8.25-5.53A1 1 0 0 1 19 6.2z" />"#;
const PLAY: &str = r#"<path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z" />"#;
const PAUSE: &str = r#"<rect x="6" y="5" width="4" height="14" rx="1" /><rect x="14" y="5" width="4" height="14" rx="1" />"#;
const NEXT: &str = r#"<rect x="16.5" y="5" width="2.5" height="14" rx="1" /><path d="M5 6.2v11.6a1 1 0 0 0 1.55.83l8.25-5.53a1.3 1.3 0 0 0 0-2.2L6.55 5.37A1 1 0 0 0 5 6.2z" />"#;
const REPEAT: &str = r#"<path d="M4 11V9.5A2.5 2.5 0 0 1 6.5 7H19M16.5 4.5 19 7l-2.5 2.5" /><path d="M20 13v1.5a2.5 2.5 0 0 1-2.5 2.5H5M7.5 19.5 5 17l2.5-2.5" />"#;
const REPEAT_ONE: &str = concat!(
    r#"<path d="M4 11V9.5A2.5 2.5 0 0 1 6.5 7H19M16.5 4.5 19 7l-2.5 2.5" /><path d="M20 13v1.5a2.5 2.5 0 0 1-2.5 2.5H5M7.5 19.5 5 17l2.5-2.5" />"#,
    r#"<path d="M11.3 10.8 12.5 10v4" stroke-width="1.6" />"#
);
const HEART: &str =
    r#"<path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z" />"#;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Shuffle,
    Previous,
    Play,
    Pause,
    Next,
    Repeat,
    RepeatOne,
    Heart,
}

impl Icon {
    fn shapes(self) -> &'static str {
        match self {
            Icon::Shuffle => SHUFFLE,
            Icon::Previous => PREVIOUS,
            Icon::Play => PLAY,
            Icon::Pause => PAUSE,
            Icon::Next => NEXT,
            Icon::Repeat => REPEAT,
            Icon::RepeatOne => REPEAT_ONE,
            Icon::Heart => HEART,
        }
    }
}

#[derive(Default)]
pub struct Icons {
    trees: HashMap<(Icon, bool, [u8; 4]), usvg::Tree>,
}

impl Icons {
    fn tree(&mut self, icon: Icon, solid: bool, color: Color) -> Option<&usvg::Tree> {
        let rgba = color.to_color_u8();
        let key = (
            icon,
            solid,
            [rgba.red(), rgba.green(), rgba.blue(), rgba.alpha()],
        );
        match self.trees.entry(key) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(entry) => {
                let color = format!("#{:02x}{:02x}{:02x}", rgba.red(), rgba.green(), rgba.blue());
                let opacity = f32::from(rgba.alpha()) / 255.0;
                let look = if solid {
                    format!(r#"fill="{color}" stroke="none" opacity="{opacity:.3}""#)
                } else {
                    format!(
                        r#"fill="none" stroke="{color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" opacity="{opacity:.3}""#
                    )
                };
                let shapes = icon.shapes();
                let svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24"><g {look}>{shapes}</g></svg>"#
                );
                let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()?;
                Some(entry.insert(tree))
            }
        }
    }

    pub fn draw(
        &mut self,
        canvas: &mut Pixmap,
        icon: Icon,
        solid: bool,
        color: Color,
        center: (f32, f32),
        size: f32,
    ) {
        let Some(tree) = self.tree(icon, solid, color) else {
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
        resvg::render(tree, transform, &mut canvas.as_mut());
    }
}

pub fn cover_pixmap(bytes: &[u8], size: u32) -> Option<Pixmap> {
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

pub struct Colors {
    pub background: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub error: Color,
}

pub fn colors(dark: bool) -> Colors {
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

pub fn faded(color: Color, opacity: f32) -> Color {
    let mut color = color;
    color.apply_opacity(opacity);
    color
}

pub fn clock(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

pub fn rounded(rect: Rect, radius: f32) -> Option<resvg::tiny_skia::Path> {
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

pub fn fill(canvas: &mut Pixmap, rect: Rect, radius: f32, color: Color) {
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

pub fn circle(canvas: &mut Pixmap, center: (f32, f32), radius: f32, color: Color) {
    if let Some(rect) = Rect::from_xywh(
        center.0 - radius,
        center.1 - radius,
        radius * 2.0,
        radius * 2.0,
    ) {
        fill(canvas, rect, radius, color);
    }
}

pub struct Face {
    path: PathBuf,
    index: u32,
    mmap: Arc<memmap2::Mmap>,
}

impl Face {
    fn as_font(&self) -> Option<FontRef<'_>> {
        FontRef::try_from_slice_and_index(&self.mmap, self.index).ok()
    }
}

pub struct Fonts {
    pub regular: Vec<Face>,
    pub bold: Vec<Face>,
    fallbacks_loaded: bool,
}

const PRIMARY_FAMILIES: [&str; 5] = [
    "Cantarell",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Inter",
];

const FALLBACK_FAMILIES: [&str; 4] = [
    "Noto Sans CJK JP",
    "Noto Sans CJK SC",
    "Droid Sans Fallback",
    "DejaVu Sans",
];

fn read_font(path: &Path) -> Option<Arc<memmap2::Mmap>> {
    let file = std::fs::File::open(path).ok()?;
    // Safety: system font files on disk are immutable for the lifetime of the mini player window.
    let mmap = unsafe { memmap2::Mmap::map(&file).ok()? };
    Some(Arc::new(mmap))
}

fn open_face(
    database: &fontdb::Database,
    id: fontdb::ID,
    files: &mut HashMap<PathBuf, Option<Arc<memmap2::Mmap>>>,
) -> Option<Face> {
    let face = database.face(id)?;
    let fontdb::Source::File(path) = &face.source else {
        return None;
    };
    let mmap = files
        .entry(path.clone())
        .or_insert_with(|| read_font(path))
        .clone()?;
    FontRef::try_from_slice_and_index(&mmap, face.index).ok()?;
    Some(Face {
        path: path.clone(),
        index: face.index,
        mmap,
    })
}

fn append_unique(target: &mut Vec<Face>, face: Face) {
    if !target
        .iter()
        .any(|known| known.path == face.path && known.index == face.index)
    {
        target.push(face);
    }
}

impl Fonts {
    pub fn load() -> Fonts {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let desktop = soundscape_core::font::desktop();
        let mut families: Vec<fontdb::Family> = Vec::new();
        if let Some(name) = desktop.as_deref() {
            families.push(fontdb::Family::Name(name));
        }
        for name in PRIMARY_FAMILIES {
            families.push(fontdb::Family::Name(name));
        }
        let mut files: HashMap<PathBuf, Option<Arc<memmap2::Mmap>>> = HashMap::new();
        let mut pick = |weight: fontdb::Weight| {
            let mut found: Vec<Face> = Vec::new();
            let id = database
                .query(&fontdb::Query {
                    families: &families,
                    weight,
                    ..fontdb::Query::default()
                })
                .or_else(|| database.faces().next().map(|face| face.id));
            if let Some(id) = id
                && let Some(face) = open_face(&database, id, &mut files)
            {
                append_unique(&mut found, face);
            }
            found
        };
        let regular = pick(fontdb::Weight::NORMAL);
        let bold = pick(fontdb::Weight::BOLD);
        Fonts {
            regular,
            bold,
            fallbacks_loaded: false,
        }
    }

    fn has_glyph(&self, ch: char) -> bool {
        self.regular
            .iter()
            .filter_map(Face::as_font)
            .any(|font| font.glyph_id(ch).0 != 0)
    }

    pub fn ensure_fallbacks_for(&mut self, text: &str) {
        if self.fallbacks_loaded
            || text
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .all(|ch| self.has_glyph(ch))
        {
            return;
        }
        self.fallbacks_loaded = true;
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let mut files: HashMap<PathBuf, Option<Arc<memmap2::Mmap>>> = HashMap::new();
        for weight in [fontdb::Weight::NORMAL, fontdb::Weight::BOLD] {
            for name in FALLBACK_FAMILIES {
                if let Some(id) = database.query(&fontdb::Query {
                    families: &[fontdb::Family::Name(name)],
                    weight,
                    ..fontdb::Query::default()
                }) && let Some(face) = open_face(&database, id, &mut files)
                {
                    let target = if weight == fontdb::Weight::BOLD {
                        &mut self.bold
                    } else {
                        &mut self.regular
                    };
                    append_unique(target, face);
                }
            }
        }
    }
}

struct Glyph<'a> {
    font: FontRef<'a>,
    id: ab_glyph::GlyphId,
    x: f32,
}

fn em(font: &FontRef<'_>, size: f32) -> PxScale {
    let units = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / units)
}

fn lay_out<'a>(fonts: &'a [Face], text: &str, size: f32) -> (Vec<Glyph<'a>>, f32) {
    let refs: Vec<FontRef<'a>> = fonts.iter().filter_map(Face::as_font).collect();
    let mut glyphs = Vec::new();
    let mut x = 0.0;
    let mut previous: Option<(usize, ab_glyph::GlyphId)> = None;
    for letter in text.chars() {
        let chosen = refs
            .iter()
            .position(|font| font.glyph_id(letter).0 != 0)
            .unwrap_or(0);
        let Some(font) = refs.get(chosen).cloned() else {
            break;
        };
        let id = font.glyph_id(letter);
        let glyph_x = {
            let scaled = font.as_scaled(em(&font, size));
            if let Some((before, before_id)) = previous
                && before == chosen
            {
                x += scaled.kern(before_id, id);
            }
            let glyph_x = x;
            x += scaled.h_advance(id);
            glyph_x
        };
        glyphs.push(Glyph {
            font,
            id,
            x: glyph_x,
        });
        previous = Some((chosen, id));
    }
    (glyphs, x)
}

pub fn fitted(fonts: &[Face], text: &str, size: f32, room: f32) -> String {
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

pub fn text(
    canvas: &mut Pixmap,
    fonts: &[Face],
    words: &str,
    size: f32,
    at: (f32, f32),
    color: Color,
) {
    let (glyphs, _) = lay_out(fonts, words, size);
    for glyph in glyphs {
        let placed = glyph
            .id
            .with_scale_and_position(em(&glyph.font, size), ab_glyph::point(at.0 + glyph.x, at.1));
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

pub fn baseline(fonts: &[Face], size: f32, top: f32, line: f32) -> f32 {
    let Some(font) = fonts.iter().find_map(Face::as_font) else {
        return top + line * 0.75;
    };
    let scaled = font.as_scaled(em(&font, size));
    let height = scaled.ascent() - scaled.descent();
    top + (line - height) / 2.0 + scaled.ascent()
}

pub fn centered(
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

    #[test]
    fn caches_parsed_svg_icons() {
        let mut icons = Icons::default();
        let mut canvas = Pixmap::new(32, 32).unwrap();
        let white = Color::from_rgba8(255, 255, 255, 255);
        icons.draw(&mut canvas, Icon::Play, true, white, (16.0, 16.0), 20.0);
        icons.draw(&mut canvas, Icon::Play, true, white, (16.0, 16.0), 20.0);
        icons.draw(
            &mut canvas,
            Icon::RepeatOne,
            false,
            white,
            (16.0, 16.0),
            18.0,
        );
        assert_eq!(icons.trees.len(), 2);
    }
}
