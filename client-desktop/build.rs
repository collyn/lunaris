//! Windows: the program's icon and its manifest.
//!
//! The manifest asks for no administrator rights (`asInvoker`): the app only
//! shows streams. Earlier versions were also the host agent and asked for
//! them, to send input to elevated windows of this computer; that is
//! lunaris-agent's now.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=windows/app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icons/icon.ico");
    res.set_manifest_file("windows/app.manifest");
    res.set("ProductName", "Lunaris Client");
    res.set("FileDescription", "Lunaris Client");
    if let Err(e) = res.compile() {
        panic!("The Windows resources (icon, manifest) could not be compiled: {}", e);
    }
}
