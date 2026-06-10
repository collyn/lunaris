use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

pub mod audio;
pub mod bridge;
pub mod decoder;
pub mod input;
pub mod protocol;

use bridge::{AppArgs, PendingDashboardEvent, APP_ARGS};

pub fn parse_deeplink_url(url_str: &str) -> Option<AppArgs> {
    if !url_str.starts_with("lunaris://") {
        return None;
    }
    if let Ok(parsed_url) = Url::parse(url_str) {
        let mut host_id = String::new();
        let mut server_url = String::new();
        let mut token = String::new();

        let mut width = 1920; // Default resolution
        let mut height = 1080;
        let mut fps = 60;
        let mut bitrate = 8000;
        let mut codec = "h264".to_string();
        let mut app_id: Option<u32> = None;
        let mut mouse_queue_limit = 256;
        let mut host_name = "Desktop • Host".to_string();
        let mut disable_cuda = false;
        let mut input_protocol = "webrtc".to_string();
        let mut encoder: Option<String> = None;
        let mut display_id: Option<String> = None;
        let mut virtual_display = false;
        let mut render_backend: Option<String> = None;

        for (k, v) in parsed_url.query_pairs() {
            match k.as_ref() {
                "host_id" => host_id = v.into_owned(),
                "server" => server_url = v.into_owned(),
                "token" => token = v.into_owned(),
                "host_name" => host_name = v.into_owned(),
                "input_protocol" => input_protocol = v.into_owned().to_lowercase(),
                "encoder" => {
                    let value = v.into_owned().to_lowercase();
                    if !value.is_empty() && value != "auto" {
                        encoder = Some(value);
                    }
                }
                "display" | "display_id" => {
                    let value = v.into_owned();
                    if !value.is_empty() && value != "default" {
                        display_id = Some(value);
                    }
                }
                "virtual_display" => {
                    virtual_display = v.as_ref() == "1" || v.as_ref().eq_ignore_ascii_case("true");
                }
                "render_backend" | "gpu_backend" | "decode_backend" => {
                    render_backend = Some(v.into_owned());
                }
                "app_id" => {
                    if let Ok(id) = v.parse::<u32>() {
                        app_id = Some(id);
                    }
                }
                "res" => {
                    let parts: Vec<&str> = v.split('x').collect();
                    if parts.len() == 2 {
                        if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                            width = w;
                            height = h;
                        }
                    }
                }
                "fps" => {
                    if let Ok(f) = v.parse::<u32>() {
                        fps = f;
                    }
                }
                "bitrate" => {
                    if let Ok(b) = v.parse::<u32>() {
                        bitrate = b;
                    }
                }
                "codec" => {
                    codec = v.into_owned().to_lowercase();
                }
                "mouse_queue_limit" => {
                    if let Ok(limit) = v.parse::<u32>() {
                        mouse_queue_limit = limit;
                    }
                }
                "disable_cuda" => {
                    if let Ok(val) = v.parse::<bool>() {
                        disable_cuda = val;
                    } else if v.as_ref() == "1" || v.as_ref() == "true" {
                        disable_cuda = true;
                    }
                }
                _ => {}
            }
        }

        let render_backend = render_backend
            .map(|value| bridge::normalize_render_backend(&value, disable_cuda))
            .unwrap_or_else(|| bridge::render_backend_from_disable_cuda(disable_cuda));
        disable_cuda = bridge::render_backend_disables_cuda(&render_backend);

        if !host_id.is_empty() && !server_url.is_empty() && !token.is_empty() {
            return Some(AppArgs {
                host_id,
                server_url,
                token,
                width,
                height,
                fps,
                bitrate,
                codec,
                app_id,
                mouse_queue_limit,
                host_name,
                disable_cuda,
                render_backend,
                input_protocol,
                encoder,
                display_id,
                virtual_display,
            });
        }
    }
    None
}

fn qt_opengl_scenegraph_available() -> bool {
    if std::env::var("LUNARIS_ASSUME_QT_OPENGL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return true;
    }

    // Search common Qt plugin directories for the OpenGL scenegraph plugin.
    let mut roots = Vec::new();
    if let Ok(path) = std::env::var("QT_PLUGIN_PATH") {
        roots.extend(
            path.split(':')
                .filter(|p| !p.is_empty())
                .map(std::path::PathBuf::from),
        );
    }
    // Try pkg-config for the Qt6 GUI plugin directory.
    if let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--variable=plugindir", "Qt6Gui"])
        .output()
    {
        if output.status.success() {
            let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !dir.is_empty() {
                roots.push(std::path::PathBuf::from(&dir));
            }
        }
    }
    // Common hardcoded paths.
    roots.push(std::path::PathBuf::from(
        "/usr/lib/x86_64-linux-gnu/qt6/plugins",
    ));
    roots.push(std::path::PathBuf::from("/usr/lib/qt6/plugins"));
    roots.push(std::path::PathBuf::from("/usr/local/lib/qt6/plugins"));

    // Also try ldconfig to find Qt libraries and infer the plugin path.
    if let Ok(output) = std::process::Command::new("ldconfig")
        .args(["-p"])
        .output()
    {
        let libs = String::from_utf8_lossy(&output.stdout);
        for line in libs.lines() {
            if line.contains("libQt6Gui.so") {
                // Extract the directory from something like:
                //   libQt6Gui.so.6 (libc6,x86-64) => /usr/lib/x86_64-linux-gnu/libQt6Gui.so.6
                if let Some(idx) = line.find("=>") {
                    let lib_path = line[idx + 2..].trim();
                    if let Some(parent) = std::path::Path::new(lib_path).parent() {
                        // lib dir → plugin dir is usually ../qt6/plugins
                        let plugin_root = parent.join("qt6").join("plugins");
                        if plugin_root.exists() {
                            roots.push(plugin_root);
                        }
                    }
                }
            }
        }
    }

    roots.iter().any(|root| {
        let scenegraph = root.join("scenegraph");
        [
            scenegraph.join("libqsgopengl.so"),
            scenegraph.join("libqsgopengladaptation.so"),
            scenegraph.join("libqtquick2plugin.so"),
        ]
        .iter()
        .any(|path| path.exists())
    })
}

fn linux_nvidia_cuda_present() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    std::path::Path::new("/dev/nvidiactl").exists()
        || std::path::Path::new("/proc/driver/nvidia/version").exists()
}

fn handle_single_instance() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let message = if args.len() >= 2 && args[1].starts_with("lunaris://") {
        format!("CONNECT {}\n", args[1])
    } else {
        "FOCUS\n".to_string()
    };

    // Try to connect to the existing instance
    if let Ok(mut stream) = TcpStream::connect("127.0.0.1:28435") {
        let _ = stream.write_all(message.as_bytes());
        let _ = stream.flush();
        info!("Sent activation command to running instance. Exiting.");
        return true; // Should exit
    }

    // No running instance found, start listener thread
    std::thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:28435") {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind single-instance listener: {:?}", e);
                return;
            }
        };

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };

            let mut buffer = [0; 4096];
            let n = match stream.read(&mut buffer) {
                Ok(n) => n,
                Err(_) => continue,
            };

            let msg = String::from_utf8_lossy(&buffer[..n]);
            let msg = msg.trim();

            if msg == "FOCUS" {
                info!("Single-instance: Received FOCUS command");
                bridge::PENDING_EVENTS.lock().unwrap().push(
                    PendingDashboardEvent::DeepLinkReceived {
                        url: "".to_string(),
                    },
                );
            } else if msg.starts_with("CONNECT ") {
                let url = msg["CONNECT ".len()..].to_string();
                info!(
                    "Single-instance: Received CONNECT command with url: {}",
                    url
                );

                if let Some(args) = parse_deeplink_url(&url) {
                    let mut active_config_lock = bridge::ACTIVE_CONFIG.lock().unwrap();
                    *active_config_lock = Some(args);
                }

                bridge::PENDING_EVENTS
                    .lock()
                    .unwrap()
                    .push(PendingDashboardEvent::DeepLinkReceived { url });
            }
        }
    });

    false
}

fn parse_args() -> Option<AppArgs> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return None;
    }

    // Check if deep linked: lunaris://connect?host_id=...&server=...&token=...
    if args[1].starts_with("lunaris://") {
        return parse_deeplink_url(&args[1]);
    }

    // Fallback to normal CLI arguments: client --host-id ID --server URL --token TOKEN ...
    let mut host_id = String::new();
    let mut server_url = String::new();
    let mut token = String::new();

    let mut width = 1280; // Changed default window size to QML's preferred size 1280x720
    let mut height = 720;
    let mut fps = 60;
    let mut bitrate = 8000;
    let mut codec = "h264".to_string();
    let mut app_id: Option<u32> = None;
    let mut mouse_queue_limit = 256;
    let mut host_name = "Desktop • Host".to_string();
    let mut disable_cuda = false;
    let mut input_protocol = "webrtc".to_string();
    let mut encoder: Option<String> = None;
    let mut display_id: Option<String> = None;
    let mut virtual_display = false;
    let mut render_backend: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--disable-cuda" {
            disable_cuda = true;
            if render_backend.is_none() {
                render_backend = Some(bridge::RENDER_BACKEND_SOFTWARE.to_string());
            }
            i += 1;
            continue;
        }
        if args[i] == "--virtual-display" {
            virtual_display = true;
            i += 1;
            continue;
        }
        if i + 1 >= args.len() {
            // Check if it is a lone flag or deep link already handled, else break
            if args[i].starts_with("lunaris://") {
                // Ignore as it was not parsed correctly or we've done it
            }
            break;
        }
        match args[i].as_str() {
            "--host-id" => {
                host_id = args[i + 1].clone();
                i += 2;
            }
            "--server" => {
                server_url = args[i + 1].clone();
                i += 2;
            }
            "--token" => {
                token = args[i + 1].clone();
                i += 2;
            }
            "--host-name" => {
                host_name = args[i + 1].clone();
                i += 2;
            }
            "--input-protocol" => {
                input_protocol = args[i + 1].clone().to_lowercase();
                i += 2;
            }
            "--render-backend" | "--gpu-backend" | "--decode-backend" => {
                render_backend = Some(args[i + 1].clone());
                i += 2;
            }
            "--encoder" => {
                let value = args[i + 1].clone().to_lowercase();
                encoder = if value.is_empty() || value == "auto" {
                    None
                } else {
                    Some(value)
                };
                i += 2;
            }
            "--display" | "--display-id" => {
                let value = args[i + 1].clone();
                display_id = if value.is_empty() || value == "default" {
                    None
                } else {
                    Some(value)
                };
                i += 2;
            }
            "--app-id" => {
                if let Ok(id) = args[i + 1].parse::<u32>() {
                    app_id = Some(id);
                }
                i += 2;
            }
            "--res" => {
                let parts: Vec<&str> = args[i + 1].split('x').collect();
                if parts.len() == 2 {
                    if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                        width = w;
                        height = h;
                    }
                }
                i += 2;
            }
            "--fps" => {
                if let Ok(f) = args[i + 1].parse::<u32>() {
                    fps = f;
                }
                i += 2;
            }
            "--bitrate" => {
                if let Ok(b) = args[i + 1].parse::<u32>() {
                    bitrate = b;
                }
                i += 2;
            }
            "--codec" => {
                codec = args[i + 1].clone().to_lowercase();
                i += 2;
            }
            "--mouse-queue-limit" => {
                if let Ok(limit) = args[i + 1].parse::<u32>() {
                    mouse_queue_limit = limit;
                }
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let render_backend = render_backend
        .map(|value| bridge::normalize_render_backend(&value, disable_cuda))
        .unwrap_or_else(|| bridge::render_backend_from_disable_cuda(disable_cuda));
    disable_cuda = bridge::render_backend_disables_cuda(&render_backend);

    if !host_id.is_empty() && !server_url.is_empty() && !token.is_empty() {
        Some(AppArgs {
            host_id,
            server_url,
            token,
            width,
            height,
            fps,
            bitrate,
            codec,
            app_id,
            mouse_queue_limit,
            host_name,
            disable_cuda,
            render_backend,
            input_protocol,
            encoder,
            display_id,
            virtual_display,
        })
    } else {
        None
    }
}

pub fn run() {
    if handle_single_instance() {
        return;
    }
    // Disable GStreamer device provider features that cause periodic thread stalls
    // and critical GLib log spam on Linux.
    std::env::set_var(
        "GST_PLUGIN_FEATURE_RANK",
        "pipewiredeviceprovider:NONE,pulsedeviceprovider:NONE,v4l2deviceprovider:NONE,alsadeviceprovider:NONE,jackdeviceprovider:NONE"
    );

    // Init standard tracing logger. Keep SRTP duplicate-packet notices out of the
    // default INFO stream; AV1 packet loss/retransmit can otherwise flood stdout.
    let rust_log =
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info,client_qml=debug,bridge=debug".into());
    let mut rust_log = if rust_log.contains("webrtc_srtp") {
        rust_log
    } else {
        format!("{},webrtc_srtp=warn", rust_log)
    };
    if !rust_log.contains("interceptor::nack") {
        rust_log = format!("{},interceptor::nack=error", rust_log);
    }
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(rust_log))
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    info!("Starting Lunaris QML Player Client...");

    // Try registering custom URI scheme
    if let Err(e) = protocol::register_protocol() {
        warn!("Failed to auto-register protocol handler: {:?}", e);
    }

    let parsed_args = parse_args();
    if let Some(args) = parsed_args {
        info!("App configurations loaded: {:?}", args);
        if APP_ARGS.set(args).is_err() {
            error!("Failed to set static APP_ARGS configuration OnceLock.");
            std::process::exit(1);
        }
    } else {
        info!("No stream configurations provided on command-line. Starting in Launcher Dashboard mode.");
    }

    // ── GPU present auto-configuration ──────────────────────────────────────
    // Try to enable native GPU present (CUDA-GL, DMABUF-GL, or D3D11) by
    // default whenever GPU decode is active.  The per-frame code in
    // decoder.rs gracefully falls back to CPU/QVideoSink when native present
    // fails at runtime, so we can be optimistic here.
    //
    // Explicit env vars (LUNARIS_CLIENT_CUDA_GL=0, LUNARIS_CLIENT_CPU_PRESENT=1,
    // --disable-cuda) still act as escape hatches.

    let gpu_mode_enabled = APP_ARGS.get().map_or(true, |a| a.gpu_mode_enabled());
    let force_cpu_present = std::env::var("LUNARIS_CLIENT_CPU_PRESENT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // Respect explicit user overrides.
    let cuda_gl_explicit = std::env::var("LUNARIS_CLIENT_CUDA_GL").ok();
    let dmabuf_gl_explicit = std::env::var("LUNARIS_CLIENT_DMABUF_GL").ok();
    let cuda_gl_disabled = cuda_gl_explicit
        .as_deref()
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false);

    if gpu_mode_enabled && !force_cpu_present {
        let opengl_available = !cfg!(target_os = "linux") || qt_opengl_scenegraph_available();

        if opengl_available {
            // Set Qt to use the OpenGL RHI backend.
            if std::env::var("QSG_RHI_BACKEND").is_err() {
                std::env::set_var("QSG_RHI_BACKEND", "opengl");
            }
            if std::env::var("QT_QUICK_BACKEND").is_err() {
                std::env::set_var("QT_QUICK_BACKEND", "opengl");
            }

            let qsg_backend = std::env::var("QSG_RHI_BACKEND").unwrap_or_default();
            let qt_backend = std::env::var("QT_QUICK_BACKEND").unwrap_or_default();
            let opengl_active = qsg_backend.eq_ignore_ascii_case("opengl")
                && qt_backend.eq_ignore_ascii_case("opengl");

            if opengl_active {
                // CUDA-GL: enable by default on NVIDIA hardware unless explicitly
                // disabled.  The runtime will self-disable if CUDA init fails.
                if !cuda_gl_disabled {
                    let nvidia = linux_nvidia_cuda_present();
                    let cuda_gl_on = cuda_gl_explicit
                        .as_deref()
                        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                        .unwrap_or(nvidia); // auto-on for NVIDIA
                    if cuda_gl_on {
                        std::env::set_var("LUNARIS_CLIENT_CUDA_GL", "1");
                    } else if cuda_gl_explicit.is_none() {
                        std::env::remove_var("LUNARIS_CLIENT_CUDA_GL");
                    }
                } else {
                    std::env::remove_var("LUNARIS_CLIENT_CUDA_GL");
                }

                // DMABUF-GL: enable by default on Linux (VAAPI → DMABUF path).
                // Safe — the per-frame code falls back gracefully.
                let dmabuf_on = dmabuf_gl_explicit
                    .as_deref()
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(cfg!(target_os = "linux"));
                if dmabuf_on {
                    std::env::set_var("LUNARIS_CLIENT_DMABUF_GL", "1");
                } else if dmabuf_gl_explicit.is_none() {
                    std::env::remove_var("LUNARIS_CLIENT_DMABUF_GL");
                }

                let nvidia = linux_nvidia_cuda_present();
                info!(
                    "GPU presentation: OpenGL scenegraph active — \
                     NVIDIA={} CUDA-GL={} DMABUF-GL={}",
                    nvidia,
                    std::env::var("LUNARIS_CLIENT_CUDA_GL").unwrap_or_default() == "1",
                    std::env::var("LUNARIS_CLIENT_DMABUF_GL").unwrap_or_default() == "1",
                );
            } else {
                // User overrode QSG_RHI_BACKEND to something non-OpenGL.
                std::env::remove_var("LUNARIS_CLIENT_CUDA_GL");
                std::env::remove_var("LUNARIS_CLIENT_DMABUF_GL");
                info!(
                    "GPU presentation: QSG_RHI_BACKEND={} (not OpenGL) — \
                     native present disabled, GPU decode + CPU/QVideoSink",
                    qsg_backend,
                );
            }
        } else {
            // No OpenGL scenegraph plugin found.
            std::env::remove_var("LUNARIS_CLIENT_CUDA_GL");
            std::env::remove_var("LUNARIS_CLIENT_DMABUF_GL");
            if cfg!(target_os = "linux") {
                // Still set QSG_RHI_BACKEND to opengl on Linux — Qt may
                // find the plugin at runtime even if our filesystem check
                // missed it.
                if std::env::var("QSG_RHI_BACKEND").is_err() {
                    std::env::set_var("QSG_RHI_BACKEND", "opengl");
                }
                if std::env::var("QT_QUICK_BACKEND").is_err() {
                    std::env::set_var("QT_QUICK_BACKEND", "opengl");
                }
                info!(
                    "GPU presentation: OpenGL scenegraph plugin not found on disk, \
                     but QSG_RHI_BACKEND=opengl set — Qt may locate it at runtime. \
                     GPU decode + CPU/QVideoSink present until proven otherwise."
                );
            }

            if cfg!(target_os = "windows") && std::env::var("QSG_RHI_BACKEND").is_err() {
                std::env::set_var("QSG_RHI_BACKEND", "d3d11");
            }
        }
    } else {
        // GPU mode is disabled or CPU-present forced — wipe native present env vars.
        std::env::remove_var("LUNARIS_CLIENT_CUDA_GL");
        std::env::remove_var("LUNARIS_CLIENT_DMABUF_GL");
        if force_cpu_present {
            info!("GPU presentation: forced CPU-only via LUNARIS_CLIENT_CPU_PRESENT");
        } else {
            info!("GPU presentation: disabled via --disable-cuda / software render backend");
        }
    }

    // 1. Create QGuiApplication
    let mut app = QGuiApplication::new();

    // Register our custom QML video rendering item
    bridge::qobject::register_gpu_video_item_type();
    bridge::qobject::register_native_video_item_types();

    // 2. Create QQmlApplicationEngine
    let mut engine = QQmlApplicationEngine::new();

    // 3. Load QML resources
    if let Some(engine_mut) = engine.as_mut() {
        engine_mut.load(&QUrl::from("qrc:/main.qml"));
    } else {
        error!("Failed to access QQmlApplicationEngine mutably.");
        std::process::exit(1);
    }

    // 4. Exec event loop
    if let Some(app_mut) = app.as_mut() {
        app_mut.exec();
    } else {
        error!("Failed to execute QGuiApplication event loop.");
        std::process::exit(1);
    }

    // Clean up local agent if running
    bridge::stop_local_agent();
}
