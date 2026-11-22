use crate::host::{async_runtime, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use crate::scrobble::Track;
use crate::AppState;
use std::io::Read;
use std::time::Duration;
use tokio::sync::Notify;

pub const MINI: &str = "mini";
const LARGEST_COVER: u64 = 3 * 1024 * 1024;
const EVERY: Duration = Duration::from_secs(1);
pub const UNKNOWN_ALBUM: &str = "Unknown album";
static WAKE: Notify = Notify::const_new();

pub fn wake() {
    WAKE.notify_one();
}

#[derive(Debug, PartialEq)]
pub struct Asked {
    pub action: String,
    pub seconds: Option<f64>,
}

fn shown(app: &AppHandle) -> bool {
    app.host().exists(MINI)
}

pub async fn refresh(app: &AppHandle) {
    if shown(app) {
        let view = get_mini(app.clone()).await;
        app.host().mini_view(&view);
    }
}

pub fn act(app: &AppHandle, asked: Asked) {
    let app = app.clone();
    async_runtime::spawn(async move {
        match mini_action(app.clone(), asked.action, asked.seconds).await {
            Ok(view) => app.host().mini_view(&view),
            Err(problem) => app.host().mini_problem(&problem),
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        refresh(&app).await;
    });
}

pub async fn run(app: AppHandle) {
    loop {
        if !shown(&app) {
            WAKE.notified().await;
        } else {
            let wait = if crate::native_link::spectrum(&app).is_some() {
                crate::native::SPECTRUM_EVERY
            } else {
                EVERY
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = WAKE.notified() => {}
            }
        }
        refresh(&app).await;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub bus: String,
    pub name: String,
    pub track: Track,
    pub art: Option<String>,
    pub playing: bool,
    pub position_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MiniView {
    pub kind: &'static str,
    pub title: String,
    pub artist: String,
    pub source: String,
    pub art: Option<String>,
    pub playing: bool,
    pub position_ms: Option<u64>,
    pub length_ms: Option<u64>,
    pub can_skip: bool,
    pub can_next: bool,
    pub can_love: bool,
    pub loved: bool,
    pub shuffle: Option<bool>,
    pub repeat: Option<String>,
    pub can_seek: bool,
    pub visualizer: Option<crate::native::Visualizer>,
    pub spectrum: Option<Vec<f32>>,
    pub accent: Option<u32>,
}

pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut bits = 0u32;
    let mut count = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
        }
    }
    Some(out)
}

fn local_cover_path(art: &str) -> Option<std::path::PathBuf> {
    let url = url::Url::parse(art).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > LARGEST_COVER {
        return None;
    }
    Some(path)
}

fn read_local_picture(art: &str) -> Option<Vec<u8>> {
    let path = local_cover_path(art)?;
    let bytes = std::fs::read(&path).ok()?;
    (bytes.len() as u64 <= LARGEST_COVER && image_type(&bytes).is_some()).then_some(bytes)
}

pub async fn picture(app: &AppHandle, art: &str) -> Option<Vec<u8>> {
    if let Some(data) = art.strip_prefix("data:") {
        let (kind, encoded) = data.split_once(',')?;
        return kind
            .ends_with(";base64")
            .then(|| unbase64(encoded))
            .flatten();
    }
    if art.starts_with("file://") {
        return read_local_picture(art);
    }
    let address = url::Url::parse(art).ok()?;
    if !matches!(address.scheme(), "http" | "https") || address.as_str().len() > 4096 {
        return None;
    }
    let request = crate::server::client().get(address.clone());
    let response = match crate::server::credentials(app) {
        Some((server, token)) if server.origin() == address.origin() => {
            crate::server::answer(request, &token).await.ok()?
        }
        _ => request.timeout(Duration::from_secs(10)).send().await.ok()?,
    };
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > LARGEST_COVER)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    (bytes.len() as u64 <= LARGEST_COVER).then(|| bytes.to_vec())
}

fn image_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

pub fn cover(art: &str) -> Option<String> {
    if art.starts_with("https://") || art.starts_with("http://") {
        return Some(art.to_string());
    }
    let path = local_cover_path(art)?;
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 12];
    let n = file.read(&mut header).ok()?;
    image_type(&header[..n]).map(|_| art.to_string())
}

pub fn shows_preview(preview: Option<&crate::media::Preview>, player_playing: bool) -> bool {
    preview.is_some_and(|preview| preview.playing || !player_playing)
}

fn preview_shown(state: &AppState) -> Option<crate::media::Preview> {
    let preview = state.preview();
    let player_playing = state.mini().is_some_and(|source| source.playing);
    shows_preview(preview.as_ref(), player_playing)
        .then_some(preview)
        .flatten()
}

pub fn view(app: &AppHandle) -> MiniView {
    let state = app.state::<AppState>();
    if let Some(polled) = preview_shown(&state) {
        let preview = crate::native_link::preview(app).unwrap_or(polled);
        return MiniView {
            kind: "preview",
            title: preview.title,
            artist: preview.artist,
            source: preview
                .album
                .filter(|album| !album.trim().is_empty())
                .unwrap_or_else(|| UNKNOWN_ALBUM.into()),
            art: preview
                .cover
                .filter(|cover| cover.starts_with("https://") || cover.starts_with("http://")),
            playing: preview.playing,
            position_ms: Some((preview.position * 1000.0) as u64),
            length_ms: preview.length.map(|length| (length * 1000.0) as u64),
            can_skip: preview.has_previous,
            can_next: preview.has_next,
            shuffle: preview.shuffle,
            repeat: preview.repeat,
            can_seek: preview.length.is_some(),
            visualizer: crate::native_link::visualizer(app),
            spectrum: crate::native_link::spectrum(app),
            ..MiniView::default()
        };
    }
    let Some(source) = state.mini() else {
        return MiniView {
            kind: "none",
            ..MiniView::default()
        };
    };
    let status = state.status();
    let current = status
        .listening
        .as_ref()
        .is_some_and(|(track, _)| track.same_song(&source.track));
    let actions = crate::tray::song_actions(&status, state.loved_for(&source.track), false);
    MiniView {
        kind: "player",
        title: source.track.title.clone(),
        artist: source.track.artist.clone(),
        source: source
            .track
            .album
            .clone()
            .filter(|album| !album.trim().is_empty())
            .unwrap_or_else(|| UNKNOWN_ALBUM.into()),
        art: source.art.as_deref().and_then(cover),
        playing: source.playing,
        position_ms: source.position_ms,
        length_ms: source.track.length_ms,
        can_skip: true,
        can_next: true,
        can_love: current && actions.can_love,
        loved: current && actions.loved,
        ..MiniView::default()
    }
}

async fn with_modes(app: &AppHandle, mut view: MiniView) -> MiniView {
    if view.kind != "player" {
        return view;
    }
    let Some(source) = app.state::<AppState>().mini() else {
        return view;
    };
    let modes = crate::mpris::modes(&source.bus).await;
    view.shuffle = modes.shuffle;
    view.repeat = modes.repeat.map(str::to_string);
    view.can_seek = modes.can_seek && view.length_ms.is_some();
    view
}

pub async fn get_mini(app: AppHandle) -> MiniView {
    MiniView {
        accent: Some(crate::native_link::accent()),
        ..with_modes(&app, view(&app)).await
    }
}

fn seek_seconds(seconds: Option<f64>) -> f64 {
    seconds.filter(|s| s.is_finite()).unwrap_or(0.0).max(0.0)
}

pub async fn mini_action(
    app: AppHandle,
    action: String,
    seconds: Option<f64>,
) -> Result<MiniView, String> {
    let state = app.state::<AppState>();
    let preview = preview_shown(&state).is_some();
    match (action.as_str(), preview) {
        ("toggle" | "next" | "previous" | "shuffle" | "repeat", true) => {
            crate::media::control(&app, &action)
        }
        ("seek", true) => crate::media::control_at(&app, "position", seek_seconds(seconds)),
        ("shuffle" | "repeat" | "seek", false) => {
            if let Some(source) = state.mini() {
                let done = match action.as_str() {
                    "shuffle" => crate::mpris::toggle_shuffle(&source.bus).await,
                    "repeat" => crate::mpris::cycle_repeat(&source.bus).await,
                    _ => {
                        crate::mpris::seek_to(&source.bus, (seek_seconds(seconds) * 1000.0) as u64)
                            .await
                    }
                };
                done.map_err(|error| format!("{} didn't respond: {error}", source.name))?;
            }
        }
        ("love", false) => crate::love::toggle(app.clone()).await,
        ("toggle" | "next" | "previous", false) => {
            let method = match action.as_str() {
                "toggle" => "PlayPause",
                "next" => "Next",
                _ => "Previous",
            };
            if let Some(source) = state.mini() {
                crate::mpris::control(&source.bus, method)
                    .await
                    .map_err(|error| format!("{} didn't respond: {error}", source.name))?;
            }
        }
        _ => {}
    }
    Ok(with_modes(&app, view(&app)).await)
}

pub fn open(app: &AppHandle) -> crate::host::Result<()> {
    if let Some(window) = app.get_webview_window(MINI) {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, MINI, WebviewUrl::App("mini.html".into()))
        .title("Soundscape mini player")
        .inner_size(420.0, 178.0)
        .resizable(false)
        .always_on_top(true)
        .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paused_preview_stays_until_a_player_plays() {
        let mut preview = crate::media::parse(r#"{"title":"Teardrop","artist":"Massive Attack","album":"30-second preview","cover":null,"playing":true,"position":3,"length":30}"#).unwrap();
        assert!(shows_preview(Some(&preview), true));
        preview.playing = false;
        assert!(shows_preview(Some(&preview), false));
        assert!(!shows_preview(Some(&preview), true));
        assert!(!shows_preview(None, false));
    }

    #[test]
    fn seeks_only_to_real_places_in_the_song() {
        assert_eq!(seek_seconds(Some(12.5)), 12.5);
        assert_eq!(seek_seconds(Some(-3.0)), 0.0);
        assert_eq!(seek_seconds(Some(f64::NAN)), 0.0);
        assert_eq!(seek_seconds(None), 0.0);
    }

    #[test]
    fn decodes_base64_data_urls() {
        assert_eq!(unbase64("").as_deref(), Some(&b""[..]));
        assert_eq!(unbase64("Zg==").as_deref(), Some(&b"f"[..]));
        assert_eq!(unbase64("Zm8=").as_deref(), Some(&b"fo"[..]));
        assert_eq!(unbase64("Zm9v").as_deref(), Some(&b"foo"[..]));
        assert_eq!(unbase64("Zm9vYmFy").as_deref(), Some(&b"foobar"[..]));
        assert_eq!(unbase64("/9gA").as_deref(), Some(&[0xFF, 0xD8, 0x00][..]));
        assert_eq!(unbase64("not*base64"), None);
    }

    #[test]
    fn shows_web_covers_and_reads_local_ones() {
        assert_eq!(
            cover("https://cdn.example.com/a.jpg").as_deref(),
            Some("https://cdn.example.com/a.jpg")
        );
        let dir = std::env::temp_dir().join(format!("soundscape-cover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("cover.png");
        let bytes = [0x89, b'P', b'N', b'G', 1, 2, 3];
        std::fs::write(&png, bytes).unwrap();
        let url = url::Url::from_file_path(&png).unwrap();
        assert_eq!(cover(url.as_str()).as_deref(), Some(url.as_str()));
        assert_eq!(
            read_local_picture(url.as_str()).as_deref(),
            Some(&bytes[..])
        );
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not an image").unwrap();
        let text_url = url::Url::from_file_path(&text).unwrap();
        assert_eq!(cover(text_url.as_str()), None);
        assert_eq!(read_local_picture(text_url.as_str()), None);
        assert_eq!(cover("ftp://example.com/a.jpg"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
