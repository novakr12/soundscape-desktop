# Soundscape Desktop

A Linux desktop app for [Soundscape](https://github.com/novakr12/soundscape), the self-hosted music tracker. It opens your own Soundscape server in its own window, connects to it with a token of its own, sends what any music player on your computer plays, and shows your Soundscape notifications on the desktop.

![Soundscape Desktop showing a Soundscape server](screenshots/home.png)

## What you need

- A Soundscape server new enough to have desktop app support (the `/api/info` address answers).
- To build it yourself: Rust and the WebKitGTK development libraries.

On Fedora:

```
sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel openssl-devel gcc
```

On Ubuntu or Debian:

```
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev build-essential
```

Install Rust from [rustup.rs](https://rustup.rs) if you don't have it.

## Run it

```
cd src-tauri
cargo run
```

On first start it asks for your server's address, for example `https://music.example.com`. You log in on the next page and approve the connection; the app gets its own token, which shows up on Soundscape's Connect page as "Desktop app on" your computer's name. Use the Soundscape menu to reload, change server or sign out. Signing out revokes the token and clears the app's cookies.

Settings live in `~/.config/io.github.novakr12.SoundscapeDesktop/config.json`, readable only by you.

## Scrobbling

The app picks up what Linux music players are playing through MPRIS, the standard every player supports: Spotify, Strawberry, Rhythmbox, Elisa, Lollypop, Amberol, VLC, mpv with the MPRIS plugin, and video sites in your browser.

- "Listening now" is sent as soon as a song starts. A play counts once you've heard half the song or four minutes, whichever comes first, the same rule as ListenBrainz and Last.fm. Songs under 30 seconds and paused time don't count, and a song you play again counts again.
- Browsers are off by default, so YouTube videos don't end up in your history. Turn any player on or off under Soundscape → Players, or from the tray.
- When a browser is on, only songs count: videos titled "Artist - Song", topic and VEVO channels, and music sites that send the album, like YouTube Music. Titles are cleaned up, so "Radiohead - Creep (Official Video)" is scrobbled as Creep by Radiohead. Other videos and anything longer than 20 minutes are left out.
- While a song plays, the tray and the Soundscape menu offer **♥ Love this song**, which marks it on your server (and on Last.fm, if your server sends there), and **Don't scrobble this one**, which keeps the current song out of your history. If you play it again later, it counts again.
- Untagged music files are tidied up: file endings and track numbers are dropped ("03 - Talisman.flac" becomes Talisman), underscores become spaces, and when there's no artist, "Artist - Song" in the name supplies it. Titles like "1999" or "7 rings" are left alone, and "Unknown Album" isn't sent.
- **Pause scrobbling for an hour**, in the tray, the Soundscape menu and Settings, stops sending for 60 minutes and then turns itself back on, even across a restart. While it's paused, the same item says how long is left and resumes right away.
- When a player sends a wrong or messy name, choose **Fix this title…** in the tray or the Soundscape menu. Settings shows exactly what the player sends; type the right artist and title, or tick the box to fix the artist on every song (handy for channels like "RadioheadVEVO"). The fix applies from then on, to now playing and scrobbles; plays already sent keep their old name. Fixes are listed in Settings, each with a Remove button, and stay on your computer.
- When the server can't be reached, plays wait in `waiting.jsonl` next to the settings and are sent with their original times once it's back.
- Closing the window keeps the app running in the background, so it keeps scrobbling. Open it again from your apps, or quit from the menu or the tray. Turn this off with Soundscape → Keep running when closed.
- The tray icon shows the song playing and how many plays are waiting. GNOME only shows tray icons with the AppIndicator extension (`sudo dnf install gnome-shell-extension-appindicator` on Fedora, then turn it on in Extensions); everything in the tray is also in the window's Soundscape menu.

## Notifications and settings

New followers, friends passing you on the weekly leaderboard and play milestones show up as desktop notifications. The app checks about once a minute. The first check after you connect only notes what's already there, so you don't get a burst of old ones.

On Monday or Tuesday a notification sums up last week: plays, listening time, your most played artist and your place among friends. It comes once per week; turn it off in Settings.

Soundscape → Friends listening, also in the tray, shows who you follow is playing right now. Click a friend to open their profile.

Your artists and songs show up when you search from GNOME's Activities overview or KDE's KRunner, with their play counts. Pick one to open it in the app, or in GNOME click the app icon to open the full search. This works while the app is running, which it usually is in the background. In GNOME you can turn it off under Settings → Search.

Previews and full songs you play in the window work with your keyboard's media keys and your desktop's media controls (play, pause and seeking), and show up there with their cover. This needs a server new enough to describe its previews to the system.

Settings (Ctrl+, or from the menu and the tray) has everything in one place: your account, scrobbling and players, plays waiting to be sent with a Send now button, notifications, starting when you log in, and keeping the app running when the window is closed.

Starting when you log in opens no window; the app scrobbles in the background until you open it from your apps. Outside Flatpak this writes `~/.config/autostart/io.github.novakr12.SoundscapeDesktop.desktop`; inside Flatpak it asks your desktop for permission instead.

## Search from your desktop without Flatpak

The Flatpak sets search up for you. When you run the app from source, copy the two files yourself. GNOME only reads search providers from system folders and needs the app's `.desktop` file installed:

```
sudo install -Dm644 flatpak/io.github.novakr12.SoundscapeDesktop.search-provider.ini /usr/local/share/gnome-shell/search-providers/io.github.novakr12.SoundscapeDesktop.search-provider.ini
sudo install -Dm644 flatpak/io.github.novakr12.SoundscapeDesktop.desktop /usr/local/share/applications/io.github.novakr12.SoundscapeDesktop.desktop
install -Dm644 flatpak/io.github.novakr12.SoundscapeDesktop.krunner.desktop ~/.local/share/krunner/dbusplugins/io.github.novakr12.SoundscapeDesktop.desktop
```

Log out and back in for GNOME to pick it up.

## Flatpak

The `flatpak` folder holds everything to build the app as a Flatpak: the manifest, the desktop entry, the store listing, and `cargo-sources.py`, which lists every Rust crate so the build can run without network access. The tray library comes from Flathub's shared modules, a git submodule in `flatpak/shared-modules`.

To build and install it yourself (install `flatpak-builder` first; on Fedora `sudo dnf install flatpak-builder`):

```
git submodule update --init
python3 flatpak/cargo-sources.py
flatpak-builder --user --install-deps-from=flathub --install --force-clean flatpak/build-dir flatpak/io.github.novakr12.SoundscapeDesktop.yml
flatpak run io.github.novakr12.SoundscapeDesktop
```

CI builds the same Flatpak on every push and keeps it as `soundscape-desktop.flatpak` under the run's artifacts, which installs with `flatpak install --user soundscape-desktop.flatpak`.

The Flatpak may talk to music players (`org.mpris.MediaPlayer2.*`), the tray (`org.kde.StatusNotifierWatcher`) and notifications (`org.freedesktop.Notifications`), show its own media player for media keys (`org.mpris.MediaPlayer2.SoundscapeDesktop`), play sound (for song previews) and use the network. It can't see your files.

## Releases

Each release on GitHub has a ready-made Flatpak bundle. Download `soundscape-desktop-<version>-x86_64.flatpak` and install it with:

```
flatpak install --user soundscape-desktop-<version>-x86_64.flatpak
```

Flatpak fetches the GNOME runtime from Flathub if you don't have it yet. `SHA256SUMS` next to it lets you check the download with `sha256sum -c SHA256SUMS`.

To make a release:

1. Set the new version in `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, run `cargo update -p soundscape-desktop` in `src-tauri`, and add a `<release>` with notes at the top of the metainfo's `<releases>`.
2. Commit and push, then tag the commit and push the tag:

   ```
   git tag v0.2.0
   git push origin v0.2.0
   ```

The release workflow checks that the tag, `Cargo.toml` and the metainfo agree, builds the Flatpak, and publishes the release with the notes from the metainfo.

## Useful commands

| Command | What it does |
|---|---|
| `cargo run` | Start the app |
| `cargo test` | Run the tests (`dbus-run-session -- cargo test` also runs the one that talks to a real player over D-Bus) |
| `cargo clippy --all-targets -- -D warnings` | Lint |
| `cargo fmt` | Format the code |
| `cargo build --release` | Build an optimised binary in `target/release/soundscape-desktop` |

## License

[AGPL-3.0-or-later](LICENSE), like Soundscape.
