use crate::system;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        type App = super::AppRust;

        #[qinvokable]
        fn server(self: &App) -> QString;

        #[qinvokable]
        #[cxx_name = "saveServer"]
        fn save_server(self: &App, text: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "showFps"]
        fn show_fps(self: &App) -> bool;

        #[qinvokable]
        #[cxx_name = "frameCounter"]
        fn frame_counter(self: &App) -> QString;

        #[qinvokable]
        #[cxx_name = "memoryMb"]
        fn memory_mb(self: &App) -> i32;

        #[qinvokable]
        #[cxx_name = "isOwnPage"]
        fn is_own_page(self: &App, address: &QString) -> bool;
    }
}

use cxx_qt_lib::QString;

#[derive(Default)]
pub struct AppRust;

impl qobject::App {
    fn server(&self) -> QString {
        let chosen = std::env::var("SOUNDSCAPE_SERVER")
            .ok()
            .and_then(|text| system::normalize_server(&text))
            .or_else(system::saved_server);
        QString::from(chosen.unwrap_or_default().as_str())
    }

    fn save_server(&self, text: &QString) -> QString {
        match system::normalize_server(&String::from(text)) {
            Some(server) if system::save_server(&server) => QString::from(server.as_str()),
            _ => QString::default(),
        }
    }

    fn show_fps(&self) -> bool {
        system::wants_frame_counter(std::env::var("SOUNDSCAPE_SHOW_FPS").ok().as_deref())
    }

    fn frame_counter(&self) -> QString {
        QString::from(system::FRAME_COUNTER)
    }

    fn memory_mb(&self) -> i32 {
        i32::try_from(system::memory_mb()).unwrap_or(i32::MAX)
    }

    fn is_own_page(&self, address: &QString) -> bool {
        let server = String::from(&self.server());
        system::is_own_page(&server, &String::from(address))
    }
}
