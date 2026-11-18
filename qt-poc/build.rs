use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("io.github.novakr12.soundscape").qml_file("qml/main.qml"),
    )
    .qt_module("Gui")
    .qt_module("Quick")
    .files(["src/app.rs", "src/share.rs"])
    .include_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/cpp"))
    .cpp_file("cpp/share.cpp")
    .build();
}
