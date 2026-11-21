use crate::host::{Manager, WebviewWindow};

pub fn smooth_scrolling(window: &WebviewWindow, on: bool) {
    window.app_handle().host().smooth_scrolling(on);
}
