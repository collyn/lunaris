fn main() {
    cxx_qt_build::CxxQtBuilder::new()
        .qt_module("Gui")
        .qt_module("Qml")
        .qt_module("Quick")
        .qt_module("Widgets")
        .qml_module(cxx_qt_build::QmlModule {
            uri: "com.lunaris.approval",
            version_major: 1,
            version_minor: 0,
            rust_files: &["src/bridge.rs"],
            qml_files: &[] as &[&str],
            qrc_files: &[] as &[&str],
        })
        .cc_builder(|cc| {
            cc.include("src");
            cc.file("src/qt_app.cpp");
        })
        .qrc("qml/resources.qrc")
        .build();

    for f in [
        "qml/Main.qml",
        "qml/ApprovalDialog.qml",
        "qml/Theme.qml",
        "qml/qmldir",
        "qml/resources.qrc",
        "qml/icons/icon.png",
        "src/qt_app.cpp",
        "src/qt_app.h",
    ] {
        println!("cargo:rerun-if-changed={f}");
    }
}
