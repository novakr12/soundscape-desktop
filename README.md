# Soundscape Desktop

The Linux app for [Soundscape](https://github.com/novakr12/soundscape), the self-hosted music tracker. It shows your Soundscape server in its own window and scrobbles what any music player on your computer plays. The window is drawn by the [Servo](https://servo.org) browser engine.

![Soundscape Desktop showing a Soundscape server](screenshots/home.png)

You need a Soundscape server new enough to have desktop app support.

## Install

Download `soundscape-desktop-<version>-x86_64.flatpak` from the [releases page](https://github.com/novakr12/soundscape-desktop/releases) and run:

```
flatpak install --user soundscape-desktop-<version>-x86_64.flatpak
```

On first start, enter your server's address, log in and approve the connection. The app gets its own token, which you can see and revoke on the server's Connect page.

## What it does

- **Scrobbling** from any MPRIS player (Spotify, Strawberry, Rhythmbox, Elisa, VLC, …). A play counts after half the song or four minutes. Browsers are off by default; when on, only songs count and titles like "(Official Video)" are cleaned up.
- **Tray** with what's playing and today's stats, ♥ Love, Don't scrobble this one, Fix this title…, pause for an hour, players, friends listening and your Navidrome playlists.
- **Runs in the background** after you close the window, and can start when you log in. If a song is playing, the app's own player can carry it on so the page can be set aside.
- **Mini player** that stays on top and controls whatever is playing.
- **Notifications** for new followers, releases, leaderboard and milestones, a Monday recap, an optional streak reminder and your Yearscape. Clicking one opens what it's about.
- **Media keys**, **GNOME and KDE search** for your artists and songs, and an optional **Discord status**.
- **Offline queue**: plays wait while the server is unreachable and are sent later with their original times.

Everything is set in Soundscape's Settings page, under **This app**.

| Keys | Action |
|---|---|
| Ctrl+M | Mini player |
| Ctrl+, | Settings |
| Ctrl+R / F5 | Reload |
| Alt+← / Alt+→ | Back / forward |
| Alt+Home | Home |

Settings are stored in `~/.config/io.github.novakr12.SoundscapeDesktop/`. GNOME needs the AppIndicator extension to show the tray icon.

## Build from source

You need Rust, Python 3, a C/C++ compiler and the development files for GStreamer, fonts, D-Bus and your graphics stack. On Fedora:

```
sudo dnf install gcc gcc-c++ clang cmake m4 gperf python3 dbus-devel freetype-devel fontconfig-devel harfbuzz-devel libunwind-devel mesa-libEGL-devel wayland-devel libxkbcommon-devel libX11-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-bad-free-devel
```

The Ubuntu packages are listed in [`.github/workflows/ci.yml`](.github/workflows/ci.yml). Then:

```
cargo run --release
```

The first build compiles Servo and takes a while. `cargo test --workspace --release` runs the tests.

To build the Flatpak:

```
python3 flatpak/cargo-sources.py
flatpak-builder --user --install-deps-from=flathub --install --force-clean flatpak/build-dir flatpak/io.github.novakr12.SoundscapeDesktop.yml
```

## Releasing

Set the version in `Cargo.toml` and `core/Cargo.toml`, run `cargo update -w`, add a `<release>` to the top of the metainfo's `<releases>`, then push a matching tag (`git tag v0.10.0 && git push origin v0.10.0`). The release workflow builds the Flatpak and publishes it with the notes from the metainfo.

## License

[AGPL-3.0-or-later](LICENSE)
