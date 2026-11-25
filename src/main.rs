mod mini_draw;
mod mini_window;
mod parent;

use soundscape_core::host::{Manager, async_runtime};
use soundscape_core::instance::{self, Claim};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::platform::x11::WindowAttributesExtX11;
use winit::window::{Icon, WindowAttributes};

const APP_ID: &str = soundscape_core::IDENTIFIER;

pub fn named(attributes: WindowAttributes, instance: &str) -> WindowAttributes {
    let attributes = WindowAttributesExtWayland::with_name(attributes, APP_ID, instance);
    WindowAttributesExtX11::with_name(attributes, APP_ID, instance)
}

pub fn window_icon(accent: Option<u32>) -> Option<Icon> {
    soundscape_core::tray::window_picture(accent)
        .and_then(|shown| Icon::from_rgba(shown.rgba, shown.width, shown.height).ok())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let background = args
        .iter()
        .any(|arg| arg == soundscape_core::autostart::BACKGROUND_FLAG);
    let waiting: std::sync::Arc<std::sync::OnceLock<soundscape_core::host::AppHandle>> =
        Default::default();
    let caller = waiting.clone();
    let instance = match async_runtime::block_on(instance::claim(Box::new(move || {
        if let Some(app) = caller.get() {
            soundscape_core::show(app);
        }
    }))) {
        Claim::Running(connection) => {
            if !background {
                async_runtime::block_on(instance::wake(&connection));
            }
            return;
        }
        Claim::First(connection) => connection,
    };
    let dir = soundscape_core::config_dir()
        .unwrap_or_else(|| std::env::temp_dir().join(soundscape_core::IDENTIFIER));
    let saved = soundscape_core::config::Config::load(&dir);
    let running = soundscape_core::host::Running {
        cpu_rendering: saved.cpu_rendering,
        faster_scripts: saved.faster_scripts,
    };
    let (stop, stopped) = std::sync::mpsc::channel();
    let quitter = stop.clone();
    if let Ok(mut signals) = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ]) {
        std::thread::spawn(move || {
            if signals.forever().next().is_some() {
                let _ = quitter.send(());
            }
        });
    }
    let host = parent::WindowHost::new(waiting.clone(), stop, running);
    let app = soundscape_core::launch(Box::new(host.clone()), dir);
    let _ = waiting.set(app.clone());
    if !soundscape_core::starts_hidden(&args, &app.state::<soundscape_core::AppState>().config()) {
        soundscape_core::show(&app);
    }
    let _ = stopped.recv();
    soundscape_core::shutdown(&app);
    host.stop_window();
    if let Some(connection) = instance {
        let _ = async_runtime::block_on(connection.release_name(soundscape_core::IDENTIFIER));
    }
}
