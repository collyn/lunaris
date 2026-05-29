use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let mut moc_path = "moc".to_string();

    // 1. Try querying via QMAKE env var or standard qmake commands
    let qmake_candidates = vec![
        std::env::var("QMAKE").unwrap_or_default(),
        "qmake6".to_string(),
        "qmake".to_string(),
    ];

    for qmake in qmake_candidates {
        if qmake.is_empty() {
            continue;
        }
        for query_key in &["QT_INSTALL_LIBEXECS", "QT_INSTALL_BINS"] {
            if let Ok(output) = std::process::Command::new(&qmake)
                .args(&["-query", query_key])
                .output()
            {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let candidate = std::path::Path::new(&path_str).join("moc");
                    #[cfg(windows)]
                    let candidate = candidate.with_extension("exe");
                    
                    if candidate.exists() {
                        moc_path = candidate.to_string_lossy().to_string();
                        break;
                    }
                }
            }
        }
        if moc_path != "moc" {
            break;
        }
    }

    // 2. Try standard fallback paths if still not found
    if moc_path == "moc" {
        if std::path::Path::new("/usr/lib/qt6/libexec/moc").exists() {
            moc_path = "/usr/lib/qt6/libexec/moc".to_string();
        } else if let Ok(qt_root) = std::env::var("QT_ROOT_DIR") {
            let candidate = std::path::Path::new(&qt_root).join("bin/moc");
            #[cfg(windows)]
            let candidate = candidate.with_extension("exe");
            if candidate.exists() {
                moc_path = candidate.to_string_lossy().to_string();
            }
        } else {
            // Check Homebrew macOS standard path
            let brew_moc = std::path::Path::new("/opt/homebrew/opt/qt/libexec/moc");
            if brew_moc.exists() {
                moc_path = brew_moc.to_string_lossy().to_string();
            } else {
                let brew_moc_bin = std::path::Path::new("/opt/homebrew/opt/qt/bin/moc");
                if brew_moc_bin.exists() {
                    moc_path = brew_moc_bin.to_string_lossy().to_string();
                }
            }
        }
    }

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
