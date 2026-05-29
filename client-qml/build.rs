use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let moc_path = if std::path::Path::new("/usr/lib/qt6/libexec/moc").exists() {
        "/usr/lib/qt6/libexec/moc".to_string()
    } else {
        "moc".to_string()
    };

    let current_dir = std::env::current_dir().unwrap();
    let header_path = current_dir.join("src/gpu_video_item.h");
    let moc_cpp = format!("{}/moc_gpu_video_item.cpp", out_dir);
    let moc_status = std::process::Command::new(&moc_path)
        .arg(&header_path)
        .arg("-f")
        .arg("gpu_video_item.h")
        .arg("-o")
        .arg(&moc_cpp)
        .status();

    match moc_status {
        Ok(status) if status.success() => {
            println!("cargo:warning=Successfully ran moc on src/gpu_video_item.h");
        }
        _ => {
            panic!(
                "Failed to execute Qt6 moc on src/gpu_video_item.h using path {}",
                moc_path
            );
        }
    }

    CxxQtBuilder::new()
        // Link standard Qt modules and extra modules needed for video/audio
        .qt_module("Gui")
        .qt_module("Qml")
        .qt_module("Quick")
        .qt_module("Multimedia")
        .qt_module("OpenGL")
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
            cc.file("src/gpu_video_item.cpp");
            cc.file(&moc_cpp);
            cc.include("src");
            cc.include(&out_dir);
        })
        // Register resources.qrc
        .qrc("qml/resources.qrc")
        .build();

    // Force rerun if QML files change
    println!("cargo:rerun-if-changed=src/gpu_video_item.h");
    println!("cargo:rerun-if-changed=qml/main.qml");
    println!("cargo:rerun-if-changed=qml/LunarisMenuBar.qml");
    println!("cargo:rerun-if-changed=qml/Settings.qml");
    println!("cargo:rerun-if-changed=qml/resources.qrc");
}
