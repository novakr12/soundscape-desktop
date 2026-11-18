mod app;
mod share;
mod system;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    share::ffi::share_gl_contexts();
    let mut application = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from(
            "qrc:/qt/qml/io/github/novakr12/soundscape/qml/main.qml",
        ));
    }
    if let Some(application) = application.as_mut() {
        application.exec();
    }
}
