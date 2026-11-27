#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `lunaris-approval-helper`: the optional tray program of the person at a
//! computer whose Lunaris agent runs as a daemon (the Windows service; on
//! Linux `--cli`, e.g. a systemd user service), which cannot show anything
//! itself. It runs no agent: it connects to the agent's local channel (the
//! approval-channel crate), tries again every few seconds while there is
//! none, and shows the agent's "accept this connection?" questions,
//! sending the answers back. Without it the agent works the same, only the
//! password lets anyone in.
//!
//! Needs Qt 6 (Gui, Qml, Quick, QuickControls2, Widgets, Qt.labs.platform);
//! the agent needs none of it. Started at login: on Windows by the agent's
//! installer for every user (HKLM Run value `LunarisAgentApproval`); on
//! Linux by `lunaris-approval-helper.desktop` in /etc/xdg/autostart. It
//! exits at once where there is nothing to answer for (no agent service on
//! Windows, no desktop session on Linux) and starts Qt only once an agent
//! lets it answer. On Windows it must be in the agent's folder: the agent
//! only lets a helper from its own folder answer.

mod bridge;
mod helper;
mod pending;
mod tray;

fn main() -> std::process::ExitCode {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
    if !approval_channel::agent_may_listen() {
        tracing::info!("No Lunaris agent to answer for on this computer; exiting");
        return std::process::ExitCode::SUCCESS;
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new().name("approval-helper".into()).spawn(move || {
        match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt.block_on(helper::run(move || {
                let _ = ready_tx.send(());
            })),
            Err(e) => tracing::error!("No async runtime: {}", e),
        }
    });
    if spawned.is_err() {
        return std::process::ExitCode::FAILURE;
    }
    // Qt only once there is an agent to answer for.
    if ready_rx.recv().is_err() {
        return std::process::ExitCode::FAILURE;
    }
    unsafe {
        bridge::qobject::create_widgets_application();
        bridge::qobject::create_tray();
    }
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&cxx_qt_lib::QUrl::from("qrc:/Main.qml"));
    }
    unsafe { bridge::qobject::exec_widgets_application() };
    std::process::ExitCode::SUCCESS
}
