# Soundscape Desktop

A Linux desktop app for [Soundscape](https://github.com/novakr12/soundscape), the self-hosted music tracker. It opens your own Soundscape server in its own window and connects to it with a token of its own.

Coming next: scrobbling from any Linux music player, a tray icon, desktop notifications and a Flatpak.

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

## Useful commands

| Command | What it does |
|---|---|
| `cargo run` | Start the app |
| `cargo test` | Run the tests |
| `cargo clippy --all-targets -- -D warnings` | Lint |
| `cargo fmt` | Format the code |
| `cargo build --release` | Build an optimised binary in `target/release/soundscape-desktop` |

## License

[AGPL-3.0-or-later](LICENSE), like Soundscape.
