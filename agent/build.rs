fn main() {
    // Qt GUI is only built when the "gui" feature is enabled.
    // CLI-only builds (`--no-default-features`) skip Qt entirely.
    let gui_enabled = std::env::var("CARGO_FEATURE_GUI").is_ok();
    if !gui_enabled {
        return;
    }

    cxx_qt_build::CxxQtBuilder::new()
        .qt_module("Gui")
        .qt_module("Qml")
        .qt_module("Quick")
        .qt_module("Widgets")
        .qml_module(cxx_qt_build::QmlModule {
            uri: "com.lunaris.agent",
            version_major: 1,
            version_minor: 0,
            rust_files: &["src/agent_gui.rs"],
            qml_files: &[] as &[&str],
            qrc_files: &[] as &[&str],
        })
        .cc_builder(|cc| {
            cc.include("src");
            cc.file("src/agent_gui.cpp");
        })
        .qrc("qml/resources.qrc")
        .build();

    println!("cargo:rerun-if-changed=qml/AgentWindow.qml");
    println!("cargo:rerun-if-changed=qml/resources.qrc");
    println!("cargo:rerun-if-changed=src/agent_gui.cpp");
    println!("cargo:rerun-if-changed=src/agent_gui.h");
}
