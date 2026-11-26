# Soundscape Desktop

Linux desktop app for [Soundscape](https://github.com/novakr12/soundscape), rendered with the [Servo](https://servo.org) engine.

![Soundscape Desktop showing a Soundscape server](screenshots/home.png)

## Features

- **Native background playback**: GStreamer player with ReplayGain, crossfade, 10-band equalizer, sleep timer, visualizer, queue editing, and saving queues as Navidrome playlists — keeps playing after closing the window.
- **MPRIS scrobbling & media keys**: scrobbles from any MPRIS player (browsers opt-in) with offline play queueing, plus system media keys.
- **Tray & mini player**: live track info, stats, love/skip/fix actions, playlists, friends, and an always-on-top mini player (GNOME needs the AppIndicator extension).
- **Desktop integration**: borderless fullscreen (`F11`), native notifications, GNOME/KDE search, accent-colored icons, and optional Discord Rich Presence.
- **Remote sync**: control and hand off playback across all your Soundscape devices and listening parties.

Configure app options in Soundscape under **Settings → This app**. Config is stored in `~/.config/io.github.novakr12.SoundscapeDesktop/`, and credentials go in the desktop keyring when available.

## Install

Download `soundscape-desktop-<version>-x86_64.flatpak` from [Releases](https://github.com/novakr12/soundscape-desktop/releases) and install it:

```bash
flatpak install --user soundscape-desktop-<version>-x86_64.flatpak
```

On first launch, enter your Soundscape server address, log in, and approve the connection. To match the Flatpak launcher icon to your accent color, download [`flatpak/icon-color.sh`](flatpak/icon-color.sh) and run `bash icon-color.sh <color|#rrggbb>` (`green` resets).

## Shortcuts

| Keys | Action |
|---|---|
| `F11` | Toggle fullscreen |
| `Ctrl+M` | Toggle mini player |
| `Ctrl+,` | Open settings |
| `Ctrl+R` / `F5` | Reload |
| `Alt+←` / `Alt+→` | Back / forward |
| `Alt+Home` | Home |

## Build from source

Install Rust, Python 3, a C/C++ compiler, and system development libraries. On Fedora:

```bash
sudo dnf install gcc gcc-c++ clang cmake m4 gperf python3 dbus-devel freetype-devel fontconfig-devel harfbuzz-devel libunwind-devel mesa-libEGL-devel wayland-devel libxkbcommon-devel libX11-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-bad-free-devel gstreamer1-plugins-good
```

See [`.github/workflows/ci.yml`](.github/workflows/ci.yml) for Ubuntu packages. Build and run (the first build compiles Servo):

```bash
cargo run --release
cargo test --workspace --release
```

Build the Flatpak locally:

```bash
python3 flatpak/cargo-sources.py
flatpak-builder --user --install-deps-from=flathub --install --force-clean flatpak/build-dir flatpak/io.github.novakr12.SoundscapeDesktop.yml
```

## Releasing

Bump `version` in `Cargo.toml`, `core/Cargo.toml`, `protocol/Cargo.toml`, and `window/Cargo.toml`, run `cargo update -w`, prepend a `<release>` entry in `flatpak/io.github.novakr12.SoundscapeDesktop.metainfo.xml`, and push a matching `v<version>` tag.

## License

[AGPL-3.0-or-later](LICENSE)
