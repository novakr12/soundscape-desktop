use crate::config::Config;
use crate::{friends, media, mini, music, native, scrobble, scrobbler, today};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use url::Url;

pub struct AppState {
    pub(crate) dir: PathBuf,
    pub(crate) config: Mutex<Config>,
    pub(crate) signed_out: AtomicBool,
    pub(crate) status: Mutex<scrobbler::Status>,
    pub(crate) friends: Mutex<Vec<friends::Friend>>,
    pub(crate) playlists: Mutex<Vec<music::Playlist>>,
    pub(crate) loved: Mutex<Option<(scrobble::Track, bool)>>,
    pub(crate) skip_request: Mutex<Option<scrobble::Track>>,
    pub(crate) skipped: Mutex<Option<scrobble::Track>>,
    pub(crate) today: Mutex<Option<today::Today>>,
    pub(crate) mini: Mutex<Option<mini::Source>>,
    pub(crate) preview: Mutex<Option<media::Preview>>,
    pub(crate) update: Mutex<Option<String>>,
    pub(crate) native: Mutex<Option<native::Player>>,
}

impl AppState {
    pub fn new(dir: PathBuf, config: Config) -> AppState {
        AppState {
            dir,
            config: Mutex::new(config),
            signed_out: AtomicBool::new(false),
            status: Mutex::new(scrobbler::Status::default()),
            friends: Mutex::new(Vec::new()),
            playlists: Mutex::new(Vec::new()),
            loved: Mutex::new(None),
            skip_request: Mutex::new(None),
            skipped: Mutex::new(None),
            today: Mutex::new(None),
            mini: Mutex::new(None),
            preview: Mutex::new(None),
            update: Mutex::new(None),
            native: Mutex::new(None),
        }
    }

    pub fn config(&self) -> Config {
        self.config.lock().expect("config lock").clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Config)) -> Config {
        let mut config = self.config.lock().expect("config lock");
        change(&mut config);
        if let Err(error) = config.save(&self.dir) {
            eprintln!("Couldn't save settings: {error}");
        }
        config.clone()
    }

    pub fn server(&self) -> Option<Url> {
        self.config()
            .server
            .and_then(|server| Url::parse(&server).ok())
    }

    pub fn signed_out(&self) -> bool {
        self.signed_out.load(Ordering::Relaxed)
    }

    pub fn set_signed_out(&self, value: bool) {
        self.signed_out.store(value, Ordering::Relaxed);
    }

    pub fn status(&self) -> scrobbler::Status {
        self.status.lock().expect("status lock").clone()
    }

    pub fn friends(&self) -> Vec<friends::Friend> {
        self.friends.lock().expect("friends lock").clone()
    }

    pub fn set_friends(&self, friends: Vec<friends::Friend>) -> bool {
        let mut current = self.friends.lock().expect("friends lock");
        if *current == friends {
            return false;
        }
        *current = friends;
        true
    }

    pub fn playlists(&self) -> Vec<music::Playlist> {
        self.playlists.lock().expect("playlists lock").clone()
    }

    pub fn set_playlists(&self, playlists: Vec<music::Playlist>) -> bool {
        let mut current = self.playlists.lock().expect("playlists lock");
        if *current == playlists {
            return false;
        }
        *current = playlists;
        true
    }

    pub fn loved_for(&self, track: &scrobble::Track) -> Option<bool> {
        match &*self.loved.lock().expect("loved lock") {
            Some((known, loved)) if known.same_song(track) => Some(*loved),
            _ => None,
        }
    }

    pub fn set_loved(&self, track: &scrobble::Track, loved: bool) {
        *self.loved.lock().expect("loved lock") = Some((track.clone(), loved));
    }

    pub fn request_skip(&self, track: scrobble::Track) {
        *self.skip_request.lock().expect("skip lock") = Some(track);
    }

    pub fn take_skip_request(&self) -> Option<scrobble::Track> {
        self.skip_request.lock().expect("skip lock").take()
    }

    pub fn set_skipped(&self, track: Option<scrobble::Track>) {
        *self.skipped.lock().expect("skip lock") = track;
    }

    pub fn was_skipped(&self, track: &scrobble::Track) -> bool {
        self.skipped
            .lock()
            .expect("skip lock")
            .as_ref()
            .is_some_and(|skipped| skipped.same_song(track))
    }

    pub fn mini(&self) -> Option<mini::Source> {
        self.mini.lock().expect("mini lock").clone()
    }

    pub fn set_mini(&self, source: Option<mini::Source>) {
        *self.mini.lock().expect("mini lock") = source;
    }

    pub fn preview(&self) -> Option<media::Preview> {
        self.preview.lock().expect("preview lock").clone()
    }

    pub fn set_preview(&self, preview: Option<media::Preview>) {
        *self.preview.lock().expect("preview lock") = preview;
    }

    pub fn update_available(&self) -> Option<String> {
        self.update.lock().expect("update lock").clone()
    }

    pub fn set_update(&self, version: Option<String>) -> bool {
        let mut current = self.update.lock().expect("update lock");
        if *current == version {
            return false;
        }
        *current = version;
        true
    }

    pub fn today(&self) -> Option<today::Today> {
        *self.today.lock().expect("today lock")
    }

    pub fn set_today(&self, today: Option<today::Today>) -> bool {
        let mut current = self.today.lock().expect("today lock");
        if *current == today {
            return false;
        }
        *current = today;
        true
    }

    pub fn set_status(&self, status: scrobbler::Status) -> bool {
        let mut current = self.status.lock().expect("status lock");
        if *current == status {
            return false;
        }
        *current = status;
        true
    }
}
