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
        #[cfg(target_os = "linux")]
        let mut disable_cuda = true;
        #[cfg(not(target_os = "linux"))]
        let mut disable_cuda = false;
        let mut input_protocol = "webrtc".to_string();
        let mut encoder: Option<String> = None;
        let mut display_id: Option<String> = None;
        let mut virtual_display = false;

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
                input_protocol,
                encoder,
                display_id,
                virtual_display,
            });
        }
    }
    None
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
    #[cfg(target_os = "linux")]
    let mut disable_cuda = true;
    #[cfg(not(target_os = "linux"))]
    let mut disable_cuda = false;
    let mut input_protocol = "webrtc".to_string();
    let mut encoder: Option<String> = None;
    let mut display_id: Option<String> = None;
    let mut virtual_display = false;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--disable-cuda" {
            disable_cuda = true;
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
            "--encoder" => {
                let value = args[i + 1].clone().to_lowercase();
                encoder = if value.is_empty() || value == "auto" { None } else { Some(value) };
                i += 2;
            }
            "--display" | "--display-id" => {
                let value = args[i + 1].clone();
                display_id = if value.is_empty() || value == "default" { None } else { Some(value) };
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
    let rust_log = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info,client_qml=debug,bridge=debug".into());
    let rust_log = if rust_log.contains("webrtc_srtp") {
        rust_log
    } else {
        format!("{},webrtc_srtp=warn", rust_log)
    };
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

    // Select Qt Quick RHI backend:
    // - If CUDA-GL interop is needed → force OpenGL
    // - If user already set QSG_RHI_BACKEND → respect their choice
    // - Otherwise → prefer Vulkan (OpenGL/GLX can fail on some NVIDIA setups)
    let cuda_enabled = APP_ARGS.get().map_or(false, |a| !a.disable_cuda);
    if cuda_enabled {
        std::env::set_var("QSG_RHI_BACKEND", "opengl");
    } else if std::env::var("QSG_RHI_BACKEND").is_err() {
        std::env::set_var("QSG_RHI_BACKEND", "vulkan");
    }

    // 1. Create QGuiApplication
    let mut app = QGuiApplication::new();

    // Register our custom QML video rendering item
    bridge::qobject::register_gpu_video_item_type();

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
