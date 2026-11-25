use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use cxx_qt_lib::{QQmlApplicationEngine, QUrl};

pub mod audio;
pub mod av1_rtp;
pub mod bridge;
pub mod decoder;
pub mod deeplink;
pub mod display_gpu;
pub mod files;
pub mod gamepad;
pub mod luma_tap;
pub mod pointer_in_picture;
pub mod input;
pub mod protocol;
pub mod secure_fs;
pub mod server_cert;
pub mod session_info;
pub mod share;
pub mod single_instance;

use bridge::{AppArgs, PendingDashboardEvent, APP_ARGS};

pub use deeplink::parse_deeplink_url;

/// Whether Qt Quick can render with OpenGL. In Qt 6 OpenGL is a built-in
/// RHI backend of Qt Quick, not a plugin (the old `scenegraph/libqsgopengl.so`
/// probe never matched a Qt 6 install, so native GPU presentation was turned
/// off), and only the "software" adaptation rules it out.
fn qt_opengl_scenegraph_available() -> bool {
    !std::env::var("QT_QUICK_BACKEND")
        .map(|v| v.eq_ignore_ascii_case("software"))
        .unwrap_or(false)
}

fn linux_nvidia_cuda_present() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    std::path::Path::new("/dev/nvidiactl").exists()
        || std::path::Path::new("/proc/driver/nvidia/version").exists()
}

fn push_event(event: PendingDashboardEvent) {
    bridge::PENDING_EVENTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(event);
}

/// The message a second copy hands to the running one.
fn single_instance_message(args: &[String]) -> String {
    if args.len() >= 2 && args[1].starts_with("lunaris://") {
        format!("CONNECT {}", args[1])
    } else {
        "FOCUS".to_string()
    }
}

/// A message from a second copy of the app (its sender passed the
/// single-instance check).
fn handle_instance_message(msg: &str) {
    if msg == "FOCUS" {
        info!("Single-instance: Received FOCUS command");
        push_event(PendingDashboardEvent::DeepLinkReceived { url: String::new() });
    } else if let Some(url) = msg.strip_prefix("CONNECT ") {
        // Same checks as a link on the command line; a bad line is
        // ignored (the listener must not panic).
        let url = url.trim();
        // A trusted server's ticket is traded in the background
        // (`Redeeming`): this thread stays free for the next message.
        match deeplink::judge(url, false) {
            deeplink::Verdict::Connect(args) => {
                info!("Single-instance: link to {} (host {})", args.server_url, args.host_id);
                *bridge::ACTIVE_CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = Some(args);
                push_event(PendingDashboardEvent::DeepLinkReceived { url: "lunaris://connect".to_string() });
            }
            deeplink::Verdict::Redeeming => {
                info!("Single-instance: link to a trusted server, trading its ticket");
            }
            deeplink::Verdict::NeedsConfirmation => {
                info!("Single-instance: link to an unknown server, asking the user");
                // Empty url = focus; the window then finds the pending link.
                push_event(PendingDashboardEvent::DeepLinkReceived { url: String::new() });
            }
            deeplink::Verdict::Invalid => {
                warn!("Single-instance: ignored an invalid link");
            }
        }
    }
}

/// True when another copy of the app (this user's) took this one's link
/// and this copy should exit. The channel is private to the user
/// (single_instance.rs): a listener that is not this user's app never gets
/// the link, and this copy then runs on its own.
fn handle_single_instance() -> bool {
    // Development/testing: run a second, independent client.
    if std::env::var_os("LUNARIS_ALLOW_SECOND_INSTANCE").is_some() {
        return false;
    }
    let args: Vec<String> = std::env::args().collect();
    let message = single_instance_message(&args);
    let dir = app_settings_dir();

    match single_instance::send_to_running(&dir, &message) {
        Ok(true) => {
            info!("Sent activation command to running instance. Exiting.");
            return true;
        }
        Ok(false) => {}
        Err(e) => {
            // Never the link to it: this copy is a window of its own.
            warn!("Single-instance: the running listener failed the check ({}); running on our own", e);
            return false;
        }
    }

    single_instance::listen(&dir, handle_instance_message);
    false
}

fn parse_args() -> Option<AppArgs> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return None;
    }

    // Check if deep linked: lunaris://connect?host_id=...&server=...&ticket=...
    if args[1].starts_with("lunaris://") {
        // Blocking: the app starts for this one session (`APP_ARGS`); the
        // single-instance listener already runs on its own thread.
        return match deeplink::judge(&args[1], true) {
            deeplink::Verdict::Connect(a) => Some(a),
            // Not when blocking; the window opens and the session starts
            // when the ticket is traded.
            deeplink::Verdict::Redeeming => None,
            // The window opens and asks (main.qml).
            deeplink::Verdict::NeedsConfirmation => None,
            deeplink::Verdict::Invalid => {
                warn!("Ignored an invalid lunaris:// link");
                None
            }
        };
    }

    // The sign-in token of the developer command line: from the
    // environment (not on the command line, where other users of this
    // computer see it), taken out so that no program started later inherits it.
    let env_token = std::env::var("LUNARIS_TOKEN").ok().filter(|t| !t.trim().is_empty());
    std::env::remove_var("LUNARIS_TOKEN");
    let (parsed, warnings) = parse_cli_args(&args, env_token, || {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        Some(line)
    });
    for w in warnings {
        warn!("{}", w);
        eprintln!("lunaris: {}", w);
    }
    parsed
}

/// Shown when the token is given with `--token <value>`.
const TOKEN_ARG_WARNING: &str = "--token puts the sign-in token on the command line, where other users of this computer can read it; set LUNARIS_TOKEN or use --token - (read from standard input) instead";

/// The developer command line: `client --host-id ID --server URL [--token -] ...`
/// with the token from `env_token` (LUNARIS_TOKEN), or `--token -` (one
/// line from `read_stdin`), or (deprecated, warned about) `--token TOKEN`.
/// Returns the session and the warnings for the user.
fn parse_cli_args(
    args: &[String],
    env_token: Option<String>,
    read_stdin: impl FnOnce() -> Option<String>,
) -> (Option<AppArgs>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut token_arg: Option<String> = None;
    let mut host_id = String::new();
    let mut server_url = String::new();

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
                token_arg = Some(args[i + 1].clone());
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

    let token = match token_arg.as_deref() {
        Some("-") => read_stdin().map(|t| t.trim().to_string()).unwrap_or_default(),
        Some(t) => {
            warnings.push(TOKEN_ARG_WARNING.to_string());
            t.to_string()
        }
        None => env_token.map(|t| t.trim().to_string()).unwrap_or_default(),
    };

    let parsed = if !host_id.is_empty() && !server_url.is_empty() && !token.is_empty() {
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
            source_id: None,
            // Typed by the user on this computer's command line.
            untrusted: false,
        })
    } else {
        None
    };
    (parsed, warnings)
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
        // webrtc 0.17 logged from webrtc_srtp, 0.21 from rtc_srtp.
        format!("{},webrtc_srtp=warn,rtc_srtp=warn", rust_log)
    };
    if !rust_log.contains("interceptor::nack") {
        rust_log = format!("{},interceptor::nack=error,rtc_interceptor::nack=error", rust_log);
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

    // The server earlier versions were signed in to becomes a trusted one
    // (before the link on the command line is judged).
    deeplink::migrate_signed_in_server();

    let parsed_args = parse_args();
    if let Some(args) = parsed_args {
        let mut shown = args.clone();
        shown.token = "<redacted>".to_string();
        info!("App configurations loaded: {:?}", shown);
        if APP_ARGS.set(args).is_err() {
            error!("Failed to set static APP_ARGS configuration OnceLock.");
            std::process::exit(1);
        }
    } else if !std::env::args().nth(1).is_some_and(|a| a.starts_with("lunaris://")) {
        // Nothing to show: the client opens a computer from the Lunaris
        // server's web page (Connect, "Open in the Lunaris app").
        eprintln!("lunaris-client: open a computer from your Lunaris server's web page (\"Open in the Lunaris app\").");
        return;
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
            // (Not QT_QUICK_BACKEND=opengl: Qt 6 takes that as the name of a
            // scenegraph plugin, fails to load it and logs an error.)

            let qsg_backend = std::env::var("QSG_RHI_BACKEND").unwrap_or_default();
            let opengl_active = qsg_backend.eq_ignore_ascii_case("opengl");

            if opengl_active {
                // CUDA-GL: enable by default on NVIDIA hardware unless explicitly
                // disabled.  The runtime will self-disable if CUDA init fails.
                if !cuda_gl_disabled {
                    // Interop needs the OpenGL context on the NVIDIA GPU: not
                    // when the displays run on another one.
                    let nvidia = linux_nvidia_cuda_present()
                        && crate::display_gpu::display_on_nvidia_or_unknown();
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
                info!(
                    "GPU presentation: QT_QUICK_BACKEND=software — \
                     native present disabled, GPU decode + CPU/QVideoSink"
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

    // 1. Create the Qt application (a QApplication: QtWidgets).
    bridge::qobject::create_widgets_application();

    // Register our custom QML video rendering item
    bridge::qobject::register_gpu_video_item_type();
    bridge::qobject::register_native_video_item_types();


    let mut engine = QQmlApplicationEngine::new();

    // 3. Load QML resources
    if let Some(engine_mut) = engine.as_mut() {
        engine_mut.load(&QUrl::from("qrc:/main.qml"));
    } else {
        error!("Failed to access QQmlApplicationEngine mutably.");
        std::process::exit(1);
    }

    // 4. Exec event loop
    bridge::qobject::exec_widgets_application();

    // After an update the window quit: start the new version.
}

/// `~/.lunaris` (Windows: `%USERPROFILE%\.lunaris`), where the app keeps its settings.
fn app_settings_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = home.join(".lunaris");
    let _ = secure_fs::create_private_dir(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<String> {
        std::iter::once("client").chain(a.iter().copied()).map(String::from).collect()
    }

    #[test]
    fn cli_token_comes_from_the_environment() {
        let (a, w) = parse_cli_args(&argv(&["--host-id", "h", "--server", "http://s"]), Some("envtok".into()), || None);
        let a = a.unwrap();
        assert_eq!(a.token, "envtok");
        assert!(!a.untrusted);
        assert!(w.is_empty());
        // No token at all: no session.
        let (a, _) = parse_cli_args(&argv(&["--host-id", "h", "--server", "http://s"]), None, || None);
        assert!(a.is_none());
    }

    #[test]
    fn cli_token_from_stdin() {
        let (a, w) = parse_cli_args(&argv(&["--host-id", "h", "--server", "http://s", "--token", "-"]), Some("env".into()), || Some("stdintok\n".into()));
        assert_eq!(a.unwrap().token, "stdintok");
        assert!(w.is_empty());
    }

    #[test]
    fn cli_token_argument_still_works_with_a_warning() {
        let (a, w) = parse_cli_args(&argv(&["--host-id", "h", "--server", "http://s", "--token", "argtok", "--fps", "30"]), None, || None);
        let a = a.unwrap();
        assert_eq!(a.token, "argtok");
        assert_eq!(a.fps, 30);
        assert_eq!(w, vec![TOKEN_ARG_WARNING.to_string()]);
    }

    #[test]
    fn single_instance_messages() {
        assert_eq!(single_instance_message(&argv(&[])), "FOCUS");
        assert_eq!(single_instance_message(&argv(&["lunaris://connect?x=1"])), "CONNECT lunaris://connect?x=1");
        assert_eq!(single_instance_message(&argv(&["--host-id", "h"])), "FOCUS");
    }
}
