//! The GPU that drives this computer's displays (Linux).
//!
//! The app's OpenGL context (and so the video it draws) lives on that GPU. On
//! a machine with several GPUs it is often not the NVIDIA card: then CUDA
//! decoded frames cannot be handed to OpenGL (CUDA-GL interop needs the GL
//! context on the NVIDIA GPU), and decoding with VAAPI on the display GPU lets
//! frames go to OpenGL without a copy.

/// PCI vendor of a GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

#[derive(Debug, Clone)]
pub struct DisplayGpu {
    pub vendor: Vendor,
    /// `/dev/dri/renderD*` of that GPU, when it has one.
    pub render_node: Option<String>,
}

/// The GPU of the first DRM card with a connected display (`LUNARIS_RENDER_NODE`
/// overrides the render node). None when it can't be told (no DRM, not Linux).
pub fn display_gpu() -> Option<DisplayGpu> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let card = cards_with_connected_display().into_iter().next()?;
    let device = format!("/sys/class/drm/{}/device", card);
    let vendor = match std::fs::read_to_string(format!("{}/vendor", device))
        .ok()?
        .trim()
    {
        "0x10de" => Vendor::Nvidia,
        "0x1002" => Vendor::Amd,
        "0x8086" => Vendor::Intel,
        _ => Vendor::Other,
    };
    let render_node = std::env::var("LUNARIS_RENDER_NODE")
        .ok()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && std::path::Path::new(n).exists())
        .or_else(|| render_node_of(&device));
    Some(DisplayGpu { vendor, render_node })
}

/// True unless the displays are known to run on a GPU other than NVIDIA.
pub fn display_on_nvidia_or_unknown() -> bool {
    display_gpu().map_or(true, |g| g.vendor == Vendor::Nvidia)
}

/// `cardN` names with at least one connected connector, sorted.
fn cards_with_connected_display() -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir("/sys/class/drm") {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => return Vec::new(),
    };
    names.sort();
    let mut cards: Vec<String> = Vec::new();
    for name in names {
        // Connectors look like "card2-DP-2".
        let Some((card, _)) = name.split_once('-') else { continue };
        if !card.starts_with("card") || cards.iter().any(|c| c == card) {
            continue;
        }
        let status = std::fs::read_to_string(format!("/sys/class/drm/{}/status", name)).unwrap_or_default();
        if status.trim() == "connected" {
            cards.push(card.to_string());
        }
    }
    cards
}

fn render_node_of(device_dir: &str) -> Option<String> {
    let mut nodes: Vec<String> = std::fs::read_dir(format!("{}/drm", device_dir))
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("renderD"))
        .collect();
    nodes.sort();
    nodes.first().map(|n| format!("/dev/dri/{}", n))
}
