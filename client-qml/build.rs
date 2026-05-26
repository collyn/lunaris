use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new()
        // Link standard Qt modules and extra modules needed for video/audio
        .qt_module("Gui")
        .qt_module("Qml")
        .qt_module("Quick")
        .qt_module("Multimedia")
        // Define the QML module with URI matching the import in QML
        .qml_module(QmlModule {
            uri: "com.lunaris.client",
            version_major: 1,
            version_minor: 0,
            rust_files: &["src/bridge.rs"],
            qml_files: &[] as &[&str],
            qrc_files: &[] as &[&str],
        })
        // Manually compiled C++ helpers
        .cc_builder(|cc| {
            cc.file("src/video_helper.cpp");
            cc.include("src");
        })
        // Register resources.qrc
        .qrc("qml/resources.qrc")
        .build();

    // Force rerun if QML files change
    println!("cargo:rerun-if-changed=qml/main.qml");
    println!("cargo:rerun-if-changed=qml/LunarisMenuBar.qml");
    println!("cargo:rerun-if-changed=qml/Settings.qml");
    println!("cargo:rerun-if-changed=qml/resources.qrc");
}
