# Soundscape Desktop

A Linux desktop app for [Soundscape](https://github.com/novakr12/soundscape), the self-hosted music tracker. It opens your own Soundscape server in its own window, connects to it with a token of its own, and sends what any music player on your computer plays.

Coming next: desktop notifications, settings and a Flatpak.

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
- When the server can't be reached, plays wait in `waiting.jsonl` next to the settings and are sent with their original times once it's back.
- Closing the window keeps the app running in the background, so it keeps scrobbling. Open it again from your apps, or quit from the menu or the tray. Turn this off with Soundscape → Keep running when closed.
- The tray icon shows the song playing and how many plays are waiting. GNOME only shows tray icons with the AppIndicator extension (`sudo dnf install gnome-shell-extension-appindicator` on Fedora, then turn it on in Extensions); everything in the tray is also in the window's Soundscape menu.

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
