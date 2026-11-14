use crate::scrobble::Track;
use crate::AppState;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const MINI: &str = "mini";
const LARGEST_COVER: u64 = 3 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub bus: String,
    pub name: String,
    pub track: Track,
    pub art: Option<String>,
    pub playing: bool,
    pub position_ms: Option<u64>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
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
    pub can_love: bool,
    pub loved: bool,
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

static COVER: Mutex<Option<(String, Option<String>)>> = Mutex::new(None);

pub fn cover(art: &str) -> Option<String> {
    if art.starts_with("https://") || art.starts_with("http://") {
        return Some(art.to_string());
    }
    let path = url::Url::parse(art)
        .ok()
        .filter(|url| url.scheme() == "file")?
        .to_file_path()
        .ok()?;
    let mut cached = COVER.lock().expect("cover lock");
    if let Some((known, data)) = &*cached {
        if known == art {
            return data.clone();
        }
    }
    let data = std::fs::metadata(&path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= LARGEST_COVER)
        .and_then(|_| std::fs::read(&path).ok())
        .and_then(|bytes| {
            image_type(&bytes).map(|kind| format!("data:{kind};base64,{}", base64(&bytes)))
        });
    *cached = Some((art.to_string(), data.clone()));
    data
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
    if let Some(preview) = preview_shown(&state) {
        return MiniView {
            kind: "preview",
            title: preview.title,
            artist: preview.artist,
            source: preview.album.unwrap_or_else(|| "Song preview".into()),
            art: preview
                .cover
                .filter(|cover| cover.starts_with("https://") || cover.starts_with("http://")),
            playing: preview.playing,
            position_ms: Some((preview.position * 1000.0) as u64),
            length_ms: preview.length.map(|length| (length * 1000.0) as u64),
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
        source: source.name.clone(),
        art: source.art.as_deref().and_then(cover),
        playing: source.playing,
        position_ms: source.position_ms,
        length_ms: source.track.length_ms,
        can_skip: true,
        can_love: current && actions.can_love,
        loved: current && actions.loved,
    }
}

#[tauri::command]
pub fn get_mini(app: AppHandle) -> MiniView {
    view(&app)
}

#[tauri::command]
pub async fn mini_action(app: AppHandle, action: String) -> Result<MiniView, String> {
    let state = app.state::<AppState>();
    let preview = preview_shown(&state).is_some();
    match (action.as_str(), preview) {
        ("toggle", true) => crate::media::control(&app, "toggle"),
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
    Ok(view(&app))
}

pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MINI) {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, MINI, WebviewUrl::App("mini.html".into()))
        .title("Soundscape mini player")
        .inner_size(400.0, 132.0)
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
    fn encodes_covers_as_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xD8, 0x00]), "/9gA");
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
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 1, 2, 3]).unwrap();
        let url = url::Url::from_file_path(&png).unwrap();
        assert_eq!(
            cover(url.as_str()).as_deref(),
            Some("data:image/png;base64,iVBORwECAw==")
        );
        let text = dir.join("notes.txt");
        std::fs::write(&text, "not an image").unwrap();
        assert_eq!(
            cover(url::Url::from_file_path(&text).unwrap().as_str()),
            None
        );
        assert_eq!(cover("ftp://example.com/a.jpg"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
