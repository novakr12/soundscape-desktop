use crate::host::AppHandle;
use crate::scrobbler::Status;

pub const LOVE_TEXT: &str = "♥ Love this song";
pub const SKIP_TEXT: &str = "Don't scrobble this one";
pub const MINI: &str = "mini-player";
pub const PAUSE: &str = "pause-hour";
pub const PAUSE_TEXT: &str = "Pause scrobbling for an hour";
pub const FIX: &str = "fix-title";
pub const FIX_TEXT: &str = "Fix this title…";
pub const UPDATE: &str = "update-available";

#[derive(Debug, PartialEq, Eq)]
pub struct SongActions {
    pub love_text: &'static str,
    pub loved: bool,
    pub can_love: bool,
    pub skip_text: &'static str,
    pub can_skip: bool,
}

pub fn song_actions(status: &Status, loved: Option<bool>, skipped: bool) -> SongActions {
    let playing = status.listening.is_some() && status.connected && !status.signed_out;
    let loved = playing && loved == Some(true);
    let (skip_text, can_skip) = match (playing, status.counted, skipped) {
        (false, _, _) => (SKIP_TEXT, false),
        (true, true, true) => ("Won't be scrobbled", false),
        (true, true, false) => ("Already scrobbled", false),
        (true, false, _) => (SKIP_TEXT, true),
    };
    SongActions {
        love_text: if loved { "♥ Loved" } else { LOVE_TEXT },
        loved,
        can_love: playing,
        skip_text,
        can_skip,
    }
}

pub fn refresh(app: &AppHandle) {
    let _ = app;
}
