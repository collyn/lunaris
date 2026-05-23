#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod protocol;

#[tauri::command]
fn log_from_frontend(level: String, message: String) {
    let log_line = format!("[Frontend {}] {}\n", level, message);
    print!("{}", log_line);
    
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open("/home/huy/Projects/lunaris/client_frontend_logs.txt")
    {
        use std::io::Write;
        let _ = write!(file, "{}", log_line);
    }
}

#[tauri::command]
fn launch_native_client(
    host_id: String,
    server_url: String,
    token: String,
    res: String,
    fps: String,
    bitrate: String,
    codec: String,
    app_id: Option<u32>,
) -> Result<(), String> {
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = current_exe.parent().ok_or("Failed to get parent dir")?;
    
    #[cfg(target_os = "windows")]
    let client_bin_name = "client.exe";
    #[cfg(not(target_os = "windows"))]
    let client_bin_name = "client";
    
    let client_path = dir.join(client_bin_name);
    
    if !client_path.exists() {
        return Err(format!("Native client binary not found at {:?}", client_path));
    }
    
    println!("Spawning native client: {:?}", client_path);
    let mut cmd = std::process::Command::new(client_path);
    cmd.args(&[
        "--host-id", &host_id,
        "--server", &server_url,
        "--token", &token,
        "--res", &res,
        "--fps", &fps,
        "--bitrate", &bitrate,
        "--codec", &codec,
    ]);

    if let Some(id) = app_id {
        cmd.args(&["--app-id", &id.to_string()]);
    }

    cmd.spawn().map_err(|e| e.to_string())?;
        
    Ok(())
}

fn main() {
    // Try registering custom URI scheme handler with the OS
    if let Err(e) = protocol::register_protocol() {
        eprintln!("Failed to register protocol handler: {:?}", e);
    }

    let args: Vec<String> = std::env::args().collect();
    let mut is_external = false;
    let mut host_id = String::new();
    let mut server_url = String::new();
    let mut token = String::new();
    let mut host_name = String::new();
    let mut codec_support = String::new();
    let mut res = String::new();
    let mut fps = String::new();
    let mut bitrate = String::new();
    let mut codec = String::new();

    // Check if launched via deep-link protocol: lunaris://connect?host_id=...&server=...&token=...
    if args.len() > 1 {
        let link_arg = &args[1];
        if link_arg.starts_with("lunaris://") {
            if let Ok(parsed_url) = url::Url::parse(link_arg) {
                for (k, v) in parsed_url.query_pairs() {
                    match k.as_ref() {
                        "host_id" => host_id = v.into_owned(),
                        "server" => server_url = v.into_owned(),
                        "token" => token = v.into_owned(),
                        "host_name" => host_name = v.into_owned(),
                        "codec_support" => codec_support = v.into_owned(),
                        "res" => res = v.into_owned(),
                        "fps" => fps = v.into_owned(),
                        "bitrate" => bitrate = v.into_owned(),
                        "codec" => codec = v.into_owned(),
                        _ => {}
                    }
                }

                if !host_id.is_empty() && !server_url.is_empty() && !token.is_empty() {
                    is_external = true;
                }
            }
        }
    }

    let init_script = if is_external {
        let mut server_host = String::new();
        if let Ok(u) = url::Url::parse(&server_url) {
            if let Some(h) = u.host_str() {
                server_host = h.to_string();
                if let Some(p) = u.port() {
                    server_host.push_str(&format!(":{}", p));
                }
            }
        }
        if server_host.is_empty() {
            server_host = server_url.clone();
        }

        format!(
            r#"
            (function() {{
                localStorage.setItem('lunaris_token', '{}');
                localStorage.setItem('lunaris_server_host', '{}');
                localStorage.setItem('lunaris_auto_launch_host_id', '{}');
                localStorage.setItem('lunaris_auto_launch_host_name', '{}');
                localStorage.setItem('lunaris_auto_launch_codec_support', '{}');
                localStorage.setItem('lunaris_stream_res', '{}');
                localStorage.setItem('lunaris_stream_fps', '{}');
                localStorage.setItem('lunaris_stream_bitrate', '{}');
                localStorage.setItem('lunaris_stream_codec', '{}');
            }})();
            "#,
            token.replace("'", "\\'"),
            server_host.replace("'", "\\'"),
            host_id.replace("'", "\\'"),
            host_name.replace("'", "\\'"),
            codec_support.replace("'", "\\'"),
            res.replace("'", "\\'"),
            fps.replace("'", "\\'"),
            bitrate.replace("'", "\\'"),
            codec.replace("'", "\\'"),
        )
    } else {
        String::new()
    };

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![log_from_frontend, launch_native_client])
        .setup(move |app| {
            // Webview always loads the local secure app index.html
            let url = tauri::WebviewUrl::App("index.html".into());

            // General utility script to override window.console and pipe it to Rust print stdout
            let mut script = r#"
                (function() {
                    if (window.__TAURI__) {
                        const invoke = window.__TAURI__.core.invoke;
                        const originalLog = console.log;
                        const originalError = console.error;
                        const originalWarn = console.warn;

                        console.log = function(...args) {
                            originalLog.apply(console, args);
                            invoke('log_from_frontend', { level: 'info', message: args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ') }).catch(() => {});
                        };
                        console.error = function(...args) {
                            originalError.apply(console, args);
                            invoke('log_from_frontend', { level: 'error', message: args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ') }).catch(() => {});
                        };
                        console.warn = function(...args) {
                            originalWarn.apply(console, args);
                            invoke('log_from_frontend', { level: 'warn', message: args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' ') }).catch(() => {});
                        };
                        window.onerror = function(message, source, lineno, colno, error) {
                            invoke('log_from_frontend', { level: 'panic', message: `${message} at ${source}:${lineno}:${colno}` }).catch(() => {});
                        };
                    }
                })();
            "#.to_string();

            if !init_script.is_empty() {
                script.push_str(&init_script);
            }

            let _window = tauri::WebviewWindowBuilder::new(app, "main", url)
                .title("Lunaris Player Client")
                .inner_size(1280.0, 720.0)
                .resizable(true)
                .fullscreen(false)
                .initialization_script(&script)
                .build()?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
