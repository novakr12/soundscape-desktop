use crate::host::{Manager, Running, WebviewWindow};
use std::sync::OnceLock;

static RUNNING: OnceLock<Running> = OnceLock::new();

pub fn remember(running: Running) {
    let _ = RUNNING.set(running);
}

pub fn active() -> bool {
    RUNNING.get().is_some_and(|running| running.cpu_rendering)
}

pub fn faster_scripts() -> bool {
    RUNNING.get().is_some_and(|running| running.faster_scripts)
}

pub fn smooth_scrolling(window: &WebviewWindow, on: bool) {
    window.app_handle().host().smooth_scrolling(on);
}
