use crate::config::Config;
use crate::friends::Friend;
use crate::host::{async_runtime, AppHandle, Manager};
use crate::music::Playlist;
use crate::scrobbler::{PlayerChoice, Status};
use crate::{scrobbler, AppState};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem, SubMenu};
use ksni::{Handle, Icon, ToolTip, TrayMethods};
use std::sync::OnceLock;

const LONGEST: usize = 60;

pub const OPEN: &str = "tray-open";
pub const MUSIC_OPEN: &str = "music-open";
pub const LOVE: &str = "love";
pub const SKIP: &str = "skip-scrobble";
pub const LOVE_TEXT: &str = "♥ Love this song";
pub const SKIP_TEXT: &str = "Don't scrobble this one";
pub const MINI: &str = "mini-player";
pub const PAUSE: &str = "pause-hour";
pub const PAUSE_TEXT: &str = "Pause scrobbling for an hour";
pub const FIX: &str = "fix-title";
pub const FIX_TEXT: &str = "Fix this title…";
pub const UPDATE: &str = "update-available";
pub const SCROBBLING: &str = "tray-scrobbling";
pub const CONNECT: &str = "connect-app";
pub const SETTINGS: &str = "settings";
pub const QUIT: &str = "quit";

const SMALL_ICON: &[u8] = include_bytes!("../../icons/32x32.png");
const LARGE_ICON: &[u8] = include_bytes!("../../icons/128x128.png");

static HANDLE: OnceLock<Handle<SoundscapeTray>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
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

fn short(text: &str) -> String {
    if text.chars().count() <= LONGEST {
        text.to_string()
    } else {
        format!(
            "{}…",
            text.chars()
                .take(LONGEST - 1)
                .collect::<String>()
                .trim_end()
        )
    }
}

pub fn minutes_left(minutes: u64) -> String {
    if minutes == 1 {
        "1 min left".into()
    } else {
        format!("{minutes} min left")
    }
}

pub fn pause_text(status: &Status) -> String {
    match status.paused_minutes {
        Some(minutes) => format!("Resume scrobbling ({})", minutes_left(minutes)),
        None => PAUSE_TEXT.into(),
    }
}

pub fn tooltip(status: &Status, today: Option<&str>) -> String {
    let song = match &status.listening {
        Some((track, player)) => short(&format!("{} — {} ({player})", track.title, track.artist)),
        None => "Soundscape".into(),
    };
    match today {
        Some(today) => format!("{song}\n{today}"),
        None => song,
    }
}

pub fn listening_text(status: &Status) -> String {
    match &status.listening {
        Some((track, _)) => short(&format!("♫ {} — {}", track.title, track.artist)),
        None => "Nothing playing".into(),
    }
}

pub fn status_text(status: &Status, config: &Config) -> String {
    if !config.connected() {
        "Not connected to a server".into()
    } else if status.signed_out {
        "Signed out. Connect again from the window".into()
    } else if !config.scrobbling {
        "Scrobbling is paused".into()
    } else if let Some(minutes) = status.paused_minutes {
        format!("Scrobbling paused · {}", minutes_left(minutes))
    } else {
        match status.waiting {
            0 => "All plays sent".into(),
            1 => "1 play waiting to be sent".into(),
            count => format!("{count} plays waiting to be sent"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub listening: String,
    pub today: Option<String>,
    pub status: String,
    pub tooltip: String,
    pub scrobbling: bool,
    pub can_connect: bool,
    pub actions: SongActions,
    pub pause: String,
    pub can_pause: bool,
    pub players: Vec<PlayerChoice>,
    pub friends: Vec<Friend>,
    pub playlists: Vec<Playlist>,
    pub update: Option<String>,
}

pub fn view(app: &AppHandle) -> View {
    let state = app.state::<AppState>();
    let status = state.status();
    let config = state.config();
    let today = state.today().map(|today| crate::today::text(&today));
    let track = status.listening.as_ref().map(|(track, _)| track);
    let actions = song_actions(
        &status,
        track.and_then(|track| state.loved_for(track)),
        track.is_some_and(|track| state.was_skipped(track)),
    );
    View {
        listening: listening_text(&status),
        tooltip: tooltip(&status, today.as_deref()),
        today,
        status: status_text(&status, &config),
        scrobbling: config.scrobbling,
        can_connect: !config.connected() || status.signed_out,
        actions,
        pause: pause_text(&status),
        can_pause: config.scrobbling && config.connected(),
        players: status.players.clone(),
        friends: state.friends(),
        playlists: state.playlists(),
        update: state.update_available(),
    }
}

pub struct SoundscapeTray {
    app: AppHandle,
    view: View,
    icons: Vec<Icon>,
}

pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn picture(bytes: &[u8], accent: Option<u32>) -> Option<Picture> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(frame.buffer_size());
    if let Some(accent) = accent {
        soundscape_protocol::tint(&mut pixels, accent);
    }
    Some(Picture {
        width: frame.width,
        height: frame.height,
        rgba: pixels,
    })
}

pub fn window_picture(accent: Option<u32>) -> Option<Picture> {
    picture(LARGE_ICON, accent)
}

fn icon(bytes: &[u8], accent: Option<u32>) -> Option<Icon> {
    let shown = picture(bytes, accent)?;
    Some(Icon {
        width: shown.width as i32,
        height: shown.height as i32,
        data: shown
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|[red, green, blue, alpha]| [*alpha, *red, *green, *blue])
            .collect(),
    })
}

pub fn icons(accent: Option<u32>) -> Vec<Icon> {
    [SMALL_ICON, LARGE_ICON]
        .into_iter()
        .filter_map(|bytes| icon(bytes, accent))
        .collect()
}

pub fn recolor(accent: Option<u32>) {
    let Some(handle) = HANDLE.get() else {
        return;
    };
    let handle = handle.clone();
    let fresh = icons(accent);
    async_runtime::spawn(async move {
        handle
            .update(move |tray: &mut SoundscapeTray| tray.icons = fresh)
            .await;
    });
}

fn item(label: impl Into<String>, id: &str, enabled: bool) -> MenuItem<SoundscapeTray> {
    let id = id.to_string();
    StandardItem {
        label: label.into(),
        enabled,
        activate: Box::new(move |tray: &mut SoundscapeTray| handle(&tray.app, &id)),
        ..Default::default()
    }
    .into()
}

fn note(label: impl Into<String>) -> MenuItem<SoundscapeTray> {
    StandardItem {
        label: label.into(),
        enabled: false,
        ..Default::default()
    }
    .into()
}

fn check(
    label: impl Into<String>,
    id: &str,
    enabled: bool,
    checked: bool,
) -> MenuItem<SoundscapeTray> {
    let id = id.to_string();
    CheckmarkItem {
        label: label.into(),
        enabled,
        checked,
        activate: Box::new(move |tray: &mut SoundscapeTray| handle(&tray.app, &id)),
        ..Default::default()
    }
    .into()
}

fn submenu(
    label: impl Into<String>,
    items: Vec<MenuItem<SoundscapeTray>>,
) -> MenuItem<SoundscapeTray> {
    SubMenu {
        label: label.into(),
        submenu: items,
        ..Default::default()
    }
    .into()
}

fn players_items(players: &[PlayerChoice]) -> Vec<MenuItem<SoundscapeTray>> {
    if players.is_empty() {
        return vec![note("No players open")];
    }
    players
        .iter()
        .map(|player| {
            check(
                short(&player.name),
                &format!("player:{}", player.id),
                true,
                player.enabled,
            )
        })
        .collect()
}

fn friends_items(friends: &[Friend]) -> Vec<MenuItem<SoundscapeTray>> {
    if friends.is_empty() {
        return vec![note("Nobody you follow is playing")];
    }
    friends
        .iter()
        .map(|friend| {
            submenu(
                short(&crate::friends::label(friend)),
                vec![
                    item(
                        "▶ Play this song",
                        &format!("friend-play:{}", friend.user),
                        friend.track_id.is_some(),
                    ),
                    item(
                        format!("Open {}'s profile", friend.user),
                        &format!("friend:{}", friend.user),
                        true,
                    ),
                ],
            )
        })
        .collect()
}

fn music_items(playlists: &[Playlist]) -> Vec<MenuItem<SoundscapeTray>> {
    let mut items = vec![item("Open Music", MUSIC_OPEN, true)];
    if playlists.is_empty() {
        return items;
    }
    items.push(MenuItem::Separator);
    items.push(note("Play a playlist"));
    items.extend(playlists.iter().map(|playlist| {
        item(
            short(&format!("▶ {}", crate::music::label(playlist))),
            &format!("music-playlist:{}", playlist.id),
            true,
        )
    }));
    items
}

pub fn menu(view: &View) -> Vec<MenuItem<SoundscapeTray>> {
    let mut items = vec![item("Open Soundscape", OPEN, true)];
    if let Some(version) = &view.update {
        items.push(item(crate::updates::menu_text(version), UPDATE, true));
    }
    items.extend([MenuItem::Separator, note(view.listening.clone())]);
    if let Some(today) = &view.today {
        items.push(note(today.clone()));
    }
    items.extend([
        check(
            view.actions.love_text,
            LOVE,
            view.actions.can_love,
            view.actions.loved,
        ),
        item(view.actions.skip_text, SKIP, view.actions.can_skip),
        item(FIX_TEXT, FIX, true),
        note(view.status.clone()),
        item("Connect this app…", CONNECT, view.can_connect),
        MenuItem::Separator,
        check("Scrobbling", SCROBBLING, true, view.scrobbling),
        item(view.pause.clone(), PAUSE, view.can_pause),
        submenu("Players", players_items(&view.players)),
        submenu("Friends listening", friends_items(&view.friends)),
        MenuItem::Separator,
        submenu("Music", music_items(&view.playlists)),
        item("Mini player", MINI, true),
        item("Settings…", SETTINGS, true),
        item("Quit", QUIT, true),
    ]);
    items
}

impl ksni::Tray for SoundscapeTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "soundscape-desktop".into()
    }

    fn title(&self) -> String {
        "Soundscape".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        let mut lines = self.view.tooltip.splitn(2, '\n');
        ToolTip {
            title: lines.next().unwrap_or("Soundscape").to_string(),
            description: lines.next().unwrap_or_default().to_string(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        menu(&self.view)
    }
}

pub fn create(app: &AppHandle) {
    let tray = SoundscapeTray {
        app: app.clone(),
        view: view(app),
        icons: icons(app.state::<AppState>().config().icon_accent),
    };
    async_runtime::spawn(async move {
        let spawned = tray
            .disable_dbus_name(crate::autostart::in_flatpak())
            .assume_sni_available(true)
            .spawn()
            .await;
        match spawned {
            Ok(handle) => {
                let _ = HANDLE.set(handle);
            }
            Err(error) => eprintln!("No tray: {error}"),
        }
    });
}

pub fn refresh(app: &AppHandle) {
    let Some(handle) = HANDLE.get() else {
        return;
    };
    let fresh = view(app);
    let handle = handle.clone();
    async_runtime::spawn(async move {
        handle
            .update(move |tray: &mut SoundscapeTray| {
                if tray.view != fresh {
                    tray.view = fresh;
                }
            })
            .await;
    });
}

fn open_friend(app: &AppHandle, user: &str, play: bool) {
    let state = app.state::<AppState>();
    let Some(server) = state.server() else { return };
    let Some(friend) = state
        .friends()
        .into_iter()
        .find(|friend| friend.user == user)
    else {
        return;
    };
    let target = if play {
        crate::friends::song(&server, &friend)
    } else {
        crate::friends::page(&server, &friend)
    };
    if let Some(target) = target {
        let _ = crate::open_main(app, target);
    }
}

fn open_music(app: &AppHandle, playlist: Option<&str>) {
    let Some(server) = app.state::<AppState>().server() else {
        return;
    };
    let target = match playlist {
        Some(id) => crate::music::playlist_page(&server, id),
        None => crate::music::page(&server),
    };
    if let Some(target) = target {
        let _ = crate::open_main(app, target);
    }
}

pub fn handle(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match id {
        OPEN => crate::show(app),
        SETTINGS => crate::settings_page::open(app),
        CONNECT => crate::connect_app(app),
        SCROBBLING => {
            state.update(|config| config.scrobbling = !config.scrobbling);
            refresh(app);
        }
        LOVE => {
            async_runtime::spawn(crate::love::toggle(app.clone()));
        }
        MINI => {
            let _ = crate::mini::open(app);
        }
        PAUSE => crate::toggle_pause(app),
        UPDATE => crate::updates::open_downloads(app),
        FIX => crate::settings_page::open(app),
        SKIP => {
            if let Some((track, _)) = state.status().listening {
                state.request_skip(track);
            }
            refresh(app);
        }
        QUIT => app.exit(0),
        MUSIC_OPEN => open_music(app, None),
        other => {
            if let Some(id) = other.strip_prefix("music-playlist:") {
                open_music(app, Some(id));
            } else if let Some(user) = other.strip_prefix("friend-play:") {
                open_friend(app, user, true);
            } else if let Some(user) = other.strip_prefix("friend:") {
                open_friend(app, user, false);
            } else if let Some(player) = other.strip_prefix("player:") {
                state.update(|config| {
                    let now = scrobbler::enabled(&config.players, player);
                    config.players.insert(player.to_string(), !now);
                });
                refresh(app);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::Track;

    fn connected() -> Config {
        Config {
            server: Some("https://a.example/".into()),
            user: Some("u".into()),
            token: Some("t".into()),
            ..Config::default()
        }
    }

    #[test]
    fn offers_love_and_skip_only_while_a_song_plays() {
        let mut status = Status {
            connected: true,
            ..Status::default()
        };
        let idle = song_actions(&status, None, false);
        assert!(!idle.can_love && !idle.can_skip);
        status.listening = Some((
            Track {
                artist: "Air".into(),
                title: "Alone in Kyoto".into(),
                album: None,
                length_ms: None,
            },
            "Strawberry".into(),
        ));
        let fresh = song_actions(&status, None, false);
        assert_eq!(
            (fresh.love_text, fresh.loved, fresh.can_love),
            (LOVE_TEXT, false, true)
        );
        assert_eq!((fresh.skip_text, fresh.can_skip), (SKIP_TEXT, true));
        assert_eq!(
            song_actions(&status, Some(true), false).love_text,
            "♥ Loved"
        );
        status.counted = true;
        assert_eq!(
            song_actions(&status, None, true).skip_text,
            "Won't be scrobbled"
        );
        assert_eq!(
            song_actions(&status, None, false).skip_text,
            "Already scrobbled"
        );
        assert!(!song_actions(&status, None, false).can_skip);
        status.signed_out = true;
        assert!(!song_actions(&status, Some(true), false).can_love);
    }

    #[test]
    fn says_what_is_playing_in_a_short_line() {
        let mut status = Status::default();
        assert_eq!(listening_text(&status), "Nothing playing");
        status.listening = Some((
            Track {
                artist: "Radiohead".into(),
                title: "Airbag".into(),
                album: None,
                length_ms: None,
            },
            "Strawberry".into(),
        ));
        assert_eq!(listening_text(&status), "♫ Airbag — Radiohead");
        status.listening = Some((
            Track {
                artist: "A".repeat(80),
                title: "T".into(),
                album: None,
                length_ms: None,
            },
            "x".into(),
        ));
        assert_eq!(listening_text(&status).chars().count(), LONGEST);
    }

    #[test]
    fn explains_the_state_of_scrobbling() {
        let mut status = Status::default();
        assert_eq!(
            status_text(&status, &Config::default()),
            "Not connected to a server"
        );
        assert_eq!(status_text(&status, &connected()), "All plays sent");
        status.waiting = 1;
        assert_eq!(
            status_text(&status, &connected()),
            "1 play waiting to be sent"
        );
        status.waiting = 12;
        assert_eq!(
            status_text(&status, &connected()),
            "12 plays waiting to be sent"
        );
        let paused = Config {
            scrobbling: false,
            ..connected()
        };
        assert_eq!(status_text(&status, &paused), "Scrobbling is paused");
        status.signed_out = true;
        assert!(status_text(&status, &connected()).starts_with("Signed out"));
    }

    fn labels(items: &[MenuItem<SoundscapeTray>]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                MenuItem::Standard(item) => item.label.clone(),
                MenuItem::Checkmark(item) => item.label.clone(),
                MenuItem::SubMenu(item) => item.label.clone(),
                MenuItem::Separator => "-".into(),
                MenuItem::RadioGroup(_) => "radio".into(),
            })
            .collect()
    }

    fn quiet() -> View {
        let status = Status::default();
        View {
            listening: listening_text(&status),
            today: None,
            status: status_text(&status, &connected()),
            tooltip: tooltip(&status, None),
            scrobbling: true,
            can_connect: false,
            actions: song_actions(&status, None, false),
            pause: pause_text(&status),
            can_pause: true,
            players: Vec::new(),
            friends: Vec::new(),
            playlists: Vec::new(),
            update: None,
        }
    }

    #[test]
    fn lays_out_the_tray_menu_like_before() {
        assert_eq!(
            labels(&menu(&quiet())),
            [
                "Open Soundscape",
                "-",
                "Nothing playing",
                LOVE_TEXT,
                SKIP_TEXT,
                FIX_TEXT,
                "All plays sent",
                "Connect this app…",
                "-",
                "Scrobbling",
                PAUSE_TEXT,
                "Players",
                "Friends listening",
                "-",
                "Music",
                "Mini player",
                "Settings…",
                "Quit",
            ]
        );
        let busy = View {
            today: Some("Today: 3 plays".into()),
            update: Some("1.0.0".into()),
            ..quiet()
        };
        let shown = labels(&menu(&busy));
        assert_eq!(shown[1], crate::updates::menu_text("1.0.0"));
        assert_eq!(shown[4], "Today: 3 plays");
    }

    #[test]
    fn fills_the_submenus() {
        let friend = Friend {
            user: "ana".into(),
            title: "Airbag".into(),
            artist: "Radiohead".into(),
            url: String::new(),
            track_id: None,
        };
        let playlist = Playlist {
            id: "p1".into(),
            name: "Morning".into(),
            song_count: 3,
        };
        let MenuItem::SubMenu(friends) = &friends_items(std::slice::from_ref(&friend))[0] else {
            panic!("friend submenu");
        };
        assert_eq!(
            labels(&friends.submenu),
            ["▶ Play this song", "Open ana's profile"]
        );
        let MenuItem::Standard(play) = &friends.submenu[0] else {
            panic!("play item");
        };
        assert!(!play.enabled);
        assert_eq!(
            labels(&friends_items(&[])),
            ["Nobody you follow is playing"]
        );
        assert_eq!(labels(&music_items(&[])), ["Open Music"]);
        assert_eq!(labels(&music_items(&[playlist])).len(), 4);
        assert_eq!(labels(&players_items(&[])), ["No players open"]);
    }

    #[test]
    fn draws_the_icon_from_the_bundled_pictures() {
        let shown = icons(None);
        assert_eq!(shown.len(), 2);
        assert_eq!((shown[0].width, shown[0].height), (32, 32));
        assert_eq!(shown[0].data.len(), 32 * 32 * 4);
    }
}
