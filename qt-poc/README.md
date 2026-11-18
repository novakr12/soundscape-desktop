# Soundscape Qt preview

A test of the desktop app on Qt WebEngine, the Chromium engine that Qt ships, instead of WebKitGTK. It only opens your Soundscape server in a window, so you can compare scrolling smoothness and memory with the Tauri app. Tray, scrobbling, MPRIS and the rest stay in the Tauri app for now.

The window is QML, and the Rust side (through [CXX-Qt](https://github.com/KDAB/cxx-qt)) handles the server setting and the memory count.

## Run it with Flatpak

```bash
cd qt-poc/flatpak
flatpak install --user flathub org.kde.Sdk//6.10 org.kde.Platform//6.10 io.qt.qtwebengine.BaseApp//6.10 org.freedesktop.Sdk.Extension.rust-stable
flatpak-builder --user --install --force-clean build-dir io.github.novakr12.SoundscapeQtPreview.yml
flatpak run --env=SOUNDSCAPE_SHOW_FPS=1 io.github.novakr12.SoundscapeQtPreview
```

The build downloads Rust crates, so it needs the network. The first start asks for your server address and remembers it. `--env=SOUNDSCAPE_SERVER=https://…` overrides it for one run.

## What the counters show

With `SOUNDSCAPE_SHOW_FPS=1` there are two counters:

- Top right, the same one as in the Tauri app: how often the page can draw. Compare this number between the two apps.
- Top left, from Qt: frames actually put on screen in the last second, and the memory of the app and its Chromium helper processes together. While nothing moves Qt draws nothing, so it can read 0 or 1. While scrolling it should match your screen.

## Run it without Flatpak

With Qt 6 and Qt WebEngine development packages and Rust installed:

```bash
cd qt-poc
cargo run --release
```

Set `QMAKE` to the Qt 6 `qmake` if the build picks up the wrong one.
