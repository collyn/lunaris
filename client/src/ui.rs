use sdl2::render::Canvas;
use sdl2::render::RenderTarget;
use sdl2::pixels::Color;
use sdl2::rect::Rect;

// Settings Options definitions
pub const RESOLUTIONS: &[(&str, u32, u32)] = &[
    ("720p", 1280, 720),
    ("1080p", 1920, 1080),
    ("1440p", 2560, 1440),
];

pub const FPSS: &[(&str, u32)] = &[
    ("60", 60),
    ("75", 75),
    ("100", 100),
    ("120", 120),
    ("240", 240),
];

pub const CODECS: &[(&str, &str)] = &[
    ("H.264", "h264"),
    ("H.265", "h265"),
    ("AV1", "av1"),
];

pub const BITRATES: &[(&str, u32)] = &[
    ("2M", 2000),
    ("4M", 4000),
    ("8M", 8000),
    ("16M", 16000),
    ("20M", 20000),
];

// Tailored space-dark colors matching the agent dashboard theme
#[allow(dead_code)]
pub const BG_PRIMARY: Color = Color::RGBA(8, 12, 20, 255);       // #080c14
#[allow(dead_code)]
pub const BG_SECONDARY: Color = Color::RGBA(15, 22, 38, 255);    // #0f1626
#[allow(dead_code)]
pub const BG_TERTIARY: Color = Color::RGBA(23, 32, 51, 255);     // #172033
pub const ACCENT_CYAN: Color = Color::RGBA(0, 240, 255, 255);     // #00f0ff
pub const ACCENT_PURPLE: Color = Color::RGBA(157, 78, 237, 255);  // #9d4edd
pub const TEXT_MAIN: Color = Color::RGBA(241, 245, 249, 255);     // #f1f5f9
pub const TEXT_MUTED: Color = Color::RGBA(148, 163, 184, 255);    // #94a3b8
pub const STATUS_ONLINE: Color = Color::RGBA(0, 255, 148, 255);   // #00ff94
#[allow(dead_code)]
pub const ERROR_COLOR: Color = Color::RGBA(239, 68, 68, 255);     // #ef4444


pub fn draw_rounded_rect<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    rect: Rect,
    radius: i32,
    color: Color,
) {
    canvas.set_draw_color(color);
    let x = rect.x();
    let y = rect.y();
    let w = rect.width() as i32;
    let h = rect.height() as i32;
    let r = radius;

    // Draw straight lines of borders
    let _ = canvas.draw_line((x + r, y), (x + w - r - 1, y)); // Top
    let _ = canvas.draw_line((x + r, y + h - 1), (x + w - r - 1, y + h - 1)); // Bottom
    let _ = canvas.draw_line((x, y + r), (x, y + h - r - 1)); // Left
    let _ = canvas.draw_line((x + w - 1, y + r), (x + w - 1, y + h - r - 1)); // Right

    // Draw corners
    let mut px = -1;
    let mut py = -1;
    for dy in 0..=r {
        let cy_offset = r - dy;
        let dx = ((r * r - cy_offset * cy_offset) as f32).sqrt().round() as i32;

        // Draw corner points
        let _ = canvas.draw_point((x + r - dx, y + dy));
        let _ = canvas.draw_point((x + w - r - 1 + dx, y + dy));
        let _ = canvas.draw_point((x + r - dx, y + h - 1 - dy));
        let _ = canvas.draw_point((x + w - r - 1 + dx, y + h - 1 - dy));

        if px != -1 && py != -1 {
            let _ = canvas.draw_line((x + r - px, y + py), (x + r - dx, y + dy));
            let _ = canvas.draw_line((x + w - r - 1 + px, y + py), (x + w - r - 1 + dx, y + dy));
            let _ = canvas.draw_line((x + r - px, y + h - 1 - py), (x + r - dx, y + h - 1 - dy));
            let _ = canvas.draw_line((x + w - r - 1 + px, y + h - 1 - py), (x + w - r - 1 + dx, y + h - 1 - dy));
        }
        px = dx;
        py = dy;
    }
}

pub fn fill_rounded_rect<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    rect: Rect,
    radius: i32,
    color: Color,
) {
    canvas.set_draw_color(color);
    let x = rect.x();
    let y = rect.y();
    let w = rect.width() as i32;
    let h = rect.height() as i32;
    let r = radius;

    // Center body
    let _ = canvas.fill_rect(Rect::new(x, y + r, w as u32, (h - 2 * r) as u32));
    
    // Top and bottom slices
    let _ = canvas.fill_rect(Rect::new(x + r, y, (w - 2 * r) as u32, r as u32));
    let _ = canvas.fill_rect(Rect::new(x + r, y + h - r, (w - 2 * r) as u32, r as u32));

    // Fill corners
    for dy in 0..r {
        let cy_offset = r - dy;
        let dx = ((r * r - cy_offset * cy_offset) as f32).sqrt() as i32;

        let _ = canvas.draw_line((x + r - dx, y + dy), (x + r, y + dy));
        let _ = canvas.draw_line((x + w - r, y + dy), (x + w - r + dx - 1, y + dy));
        let _ = canvas.draw_line((x + r - dx, y + h - 1 - dy), (x + r, y + h - 1 - dy));
        let _ = canvas.draw_line((x + w - r, y + h - 1 - dy), (x + w - r + dx - 1, y + h - 1 - dy));
    }
}

pub fn fill_rounded_gradient_rect<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    rect: Rect,
    radius: i32,
    color_start: Color,
    color_end: Color,
) {
    let w = rect.width() as i32;
    let h = rect.height() as i32;
    let r = radius;
    for x in 0..w {
        let t = x as f32 / w as f32;
        let color = Color::RGBA(
            (color_start.r as f32 * (1.0 - t) + color_end.r as f32 * t) as u8,
            (color_start.g as f32 * (1.0 - t) + color_end.g as f32 * t) as u8,
            (color_start.b as f32 * (1.0 - t) + color_end.b as f32 * t) as u8,
            (color_start.a as f32 * (1.0 - t) + color_end.a as f32 * t) as u8,
        );
        canvas.set_draw_color(color);
        
        let dy = if x < r {
            let cx_offset = r - x;
            let term = r * r - cx_offset * cx_offset;
            if term >= 0 {
                r - (term as f32).sqrt() as i32
            } else {
                0
            }
        } else if x >= w - r {
            let cx_offset = r - (w - 1 - x);
            let term = r * r - cx_offset * cx_offset;
            if term >= 0 {
                r - (term as f32).sqrt() as i32
            } else {
                0
            }
        } else {
            0
        };
        
        let _ = canvas.draw_line(
            (rect.x() + x, rect.y() + dy),
            (rect.x() + x, rect.y() + h - 1 - dy),
        );
    }
}

use std::sync::OnceLock;
use rusttype::{Font, Scale, point};

pub fn get_font() -> &'static Font<'static> {
    static FONT: OnceLock<Font<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        let font_data = include_bytes!("assets/SpaceGrotesk-Medium.ttf");
        Font::try_from_bytes(font_data as &[u8]).expect("Failed to parse bundled SpaceGrotesk font")
    })
}

pub fn get_text_width(text: &str, size: f32) -> i32 {
    let font = get_font();
    let scale = Scale::uniform(size);
    let glyphs: Vec<_> = font.layout(text, scale, point(0.0, 0.0)).collect();
    if glyphs.is_empty() {
        return 0;
    }
    let last_glyph = &glyphs[glyphs.len() - 1];
    let scale_factor = scale.x / font.units_per_em() as f32;
    let width = last_glyph.position().x + last_glyph.unpositioned().h_metrics().advance_width as f32 * scale_factor;
    width as i32
}

pub fn draw_text<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    text: &str,
    x: i32,
    y: i32,
    color: Color,
    size: f32,
) {
    let font = get_font();
    let scale = Scale::uniform(size);
    let v_metrics = font.v_metrics(scale);
    let glyphs: Vec<_> = font.layout(text, scale, point(x as f32, y as f32 + v_metrics.ascent)).collect();
    
    for glyph in glyphs {
        if let Some(bounding_box) = glyph.pixel_bounding_box() {
            glyph.draw(|gx, gy, gv| {
                if gv > 0.05 {
                    let px = bounding_box.min.x + gx as i32;
                    let py = bounding_box.min.y + gy as i32;
                    let alpha = (color.a as f32 * gv) as u8;
                    canvas.set_draw_color(Color::RGBA(color.r, color.g, color.b, alpha));
                    let _ = canvas.draw_point((px, py));
                }
            });
        }
    }
}

pub fn draw_text_with_shadow<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    text: &str,
    x: i32,
    y: i32,
    color: Color,
    size: f32,
) {
    // Drop shadow
    draw_text(canvas, text, x + 1, y + 1, Color::RGBA(0, 0, 0, 150), size);
    // Main text
    draw_text(canvas, text, x, y, color, size);
}

// Collapsed trigger notch dimensions
pub fn get_trigger_rect(win_w: i32) -> Rect {
    let cx = win_w / 2;
    Rect::new(cx - 25, 0, 50, 14)
}

// Menu pill dimensions
pub fn get_menu_rect(win_w: i32, y_offset: i32) -> Rect {
    let cx = win_w / 2;
    Rect::new(cx - 200, y_offset, 400, 48)
}

// Menu buttons layouts
pub fn get_menu_buttons(win_w: i32, y_offset: i32) -> Vec<(Rect, &'static str)> {
    let cx = win_w / 2;
    let button_w: i32 = 68;
    let button_h: i32 = 36;
    let gap: i32 = 10;
    let start_x = cx - 200 + 10;
    vec![
        (Rect::new(start_x, y_offset + 6, button_w as u32, button_h as u32), "Exit"),
        (Rect::new(start_x + (button_w + gap) * 1, y_offset + 6, button_w as u32, button_h as u32), "FS"),
        (Rect::new(start_x + (button_w + gap) * 2, y_offset + 6, button_w as u32, button_h as u32), "Lock"),
        (Rect::new(start_x + (button_w + gap) * 3, y_offset + 6, button_w as u32, button_h as u32), "Stats"),
        (Rect::new(start_x + (button_w + gap) * 4, y_offset + 6, button_w as u32, button_h as u32), "Settings"),
    ]
}

// Draw the header notch menu
pub fn draw_menu<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    win_w: i32,
    y_offset: i32,
    show_menu: bool,
    fullscreen: bool,
    pointer_locked: bool,
    show_stats: bool,
    mx: i32,
    my: i32,
) {
    let collapsed_bg = Color::RGBA(15, 23, 42, 200); // Sleek translucent dark glass
    let menu_bg = Color::RGBA(15, 23, 42, 225); // Rich translucent space slate glass
    let border_color_collapsed = ACCENT_PURPLE;

    if !show_menu && y_offset <= -35 {
        // Draw collapsed trigger notch
        let trig = get_trigger_rect(win_w);
        let is_hovered = mx >= trig.x() && mx <= trig.x() + trig.width() as i32
            && my >= trig.y() && my <= trig.y() + trig.height() as i32;

        let radius = 6;
        fill_rounded_rect(canvas, trig, radius, collapsed_bg);
        draw_rounded_rect(canvas, trig, radius, if is_hovered { ACCENT_CYAN } else { border_color_collapsed });
        
        // Draw a clean vector chevron down (V)
        let cx = trig.x() + 25;
        let cy = trig.y() + 5;
        canvas.set_draw_color(if is_hovered { ACCENT_CYAN } else { ACCENT_PURPLE });
        let _ = canvas.draw_line((cx - 4, cy), (cx, cy + 4));
        let _ = canvas.draw_line((cx, cy + 4), (cx + 4, cy));
    } else {
        // Draw expanded menu pill (radius 12)
        let menu_rect = get_menu_rect(win_w, y_offset);
        let menu_radius = 12;
        
        // 1. Draw Drop Shadow
        let shadow_rect = Rect::new(menu_rect.x(), menu_rect.y() + 3, menu_rect.width(), menu_rect.height());
        fill_rounded_rect(canvas, shadow_rect, menu_radius, Color::RGBA(0, 0, 0, 120));

        // 2. Draw Menu Background & Border
        fill_rounded_rect(canvas, menu_rect, menu_radius, menu_bg);
        draw_rounded_rect(canvas, menu_rect, menu_radius, Color::RGBA(255, 255, 255, 20)); // Subtle white outline

        let buttons = get_menu_buttons(win_w, y_offset);
        for &(rect, label) in &buttons {
            let mut is_active = false;
            if label == "FS" && fullscreen {
                is_active = true;
            } else if label == "Lock" && pointer_locked {
                is_active = true;
            } else if label == "Stats" && show_stats {
                is_active = true;
            }

            let is_hovered = mx >= rect.x() && mx <= rect.x() + rect.width() as i32
                && my >= rect.y() && my <= rect.y() + rect.height() as i32;

            let button_radius = 6;

            if is_active {
                // Active buttons: Rounded linear gradient background, dark text
                fill_rounded_gradient_rect(canvas, rect, button_radius, ACCENT_CYAN, ACCENT_PURPLE);
                draw_rounded_rect(canvas, rect, button_radius, TEXT_MAIN);
                
                // Draw vector icon and label (dark color)
                draw_button_contents(canvas, rect, label, Color::RGB(8, 12, 20), pointer_locked);
            } else {
                // Inactive buttons: Translucent white glass, border glows cyan if hovered
                if is_hovered {
                    fill_rounded_rect(canvas, rect, button_radius, Color::RGBA(255, 255, 255, 20)); // Hover glass opacity
                    draw_rounded_rect(canvas, rect, button_radius, ACCENT_CYAN); // Glowing border
                    draw_button_contents(canvas, rect, label, TEXT_MAIN, pointer_locked);
                } else {
                    fill_rounded_rect(canvas, rect, button_radius, Color::RGBA(255, 255, 255, 10)); // Subtle glass opacity
                    draw_rounded_rect(canvas, rect, button_radius, Color::RGBA(255, 255, 255, 12)); // Muted border
                    draw_button_contents(canvas, rect, label, Color::RGBA(241, 245, 249, 180), pointer_locked);
                }
            }
        }
    }
}

// Helper to draw vector icons and text inside menu buttons
fn draw_button_contents<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    rect: Rect,
    label: &str,
    color: Color,
    pointer_locked: bool,
) {
    let ix = rect.x() + (rect.width() as i32 - 12) / 2;
    let iy = rect.y() + 4;
    
    // Draw vector icon
    canvas.set_draw_color(color);
    match label {
        "Exit" => {
            // Exit: door bracket on the left, arrow pointing right out of it
            let _ = canvas.draw_line((ix + 2, iy), (ix + 2, iy + 11));
            let _ = canvas.draw_line((ix + 2, iy), (ix + 6, iy));
            let _ = canvas.draw_line((ix + 2, iy + 11), (ix + 6, iy + 11));
            let _ = canvas.draw_line((ix + 4, iy + 6), (ix + 11, iy + 6));
            let _ = canvas.draw_line((ix + 11, iy + 6), (ix + 8, iy + 3));
            let _ = canvas.draw_line((ix + 11, iy + 6), (ix + 8, iy + 9));
        }
        "FS" => {
            // Fullscreen: Corner brackets
            let _ = canvas.draw_line((ix, iy), (ix + 3, iy));
            let _ = canvas.draw_line((ix, iy), (ix, iy + 3));
            let _ = canvas.draw_line((ix + 11, iy), (ix + 8, iy));
            let _ = canvas.draw_line((ix + 11, iy), (ix + 11, iy + 3));
            let _ = canvas.draw_line((ix, iy + 11), (ix + 3, iy + 11));
            let _ = canvas.draw_line((ix, iy + 11), (ix, iy + 8));
            let _ = canvas.draw_line((ix + 11, iy + 11), (ix + 8, iy + 11));
            let _ = canvas.draw_line((ix + 11, iy + 11), (ix + 11, iy + 8));
        }
        "Lock" => {
            // Pointer Lock: Padlock that shifts open/close
            let _ = canvas.draw_rect(Rect::new(ix + 2, iy + 5, 8, 7));
            if pointer_locked {
                // Locked: Closed Shackle
                let _ = canvas.draw_line((ix + 4, iy + 4), (ix + 4, iy + 2));
                let _ = canvas.draw_line((ix + 4, iy + 2), (ix + 7, iy + 2));
                let _ = canvas.draw_line((ix + 7, iy + 2), (ix + 7, iy + 4));
                // Center keyhole
                let _ = canvas.fill_rect(Rect::new(ix + 5, iy + 7, 2, 2));
                let _ = canvas.draw_line((ix + 6, iy + 9), (ix + 6, iy + 10));
            } else {
                // Unlocked: Open Shackle
                let _ = canvas.draw_line((ix + 4, iy + 4), (ix + 4, iy + 1));
                let _ = canvas.draw_line((ix + 4, iy + 1), (ix + 7, iy + 1));
                let _ = canvas.draw_line((ix + 7, iy + 1), (ix + 7, iy + 3));
                // Keyhole line
                let _ = canvas.draw_line((ix + 5, iy + 8), (ix + 7, iy + 8));
            }
        }
        "Stats" => {
            // Stats: filled bar chart
            let _ = canvas.fill_rect(Rect::new(ix, iy + 8, 3, 4));
            let _ = canvas.fill_rect(Rect::new(ix + 4, iy + 4, 3, 8));
            let _ = canvas.fill_rect(Rect::new(ix + 8, iy, 3, 12));
        }
        "Settings" => {
            // Settings: Sliders
            let _ = canvas.draw_line((ix, iy + 3), (ix + 11, iy + 3));
            let _ = canvas.fill_rect(Rect::new(ix + 2, iy + 1, 3, 5));
            let _ = canvas.draw_line((ix, iy + 8), (ix + 11, iy + 8));
            let _ = canvas.fill_rect(Rect::new(ix + 7, iy + 6, 3, 5));
        }
        _ => {}
    }

    // Draw text label centered below icon
    let display_text = match label {
        "Exit" => "EXIT",
        "FS" => "FULL",
        "Lock" => "LOCK",
        "Stats" => "STATS",
        "Settings" => "CONFIG",
        other => other,
    };
    
    let text_size = 11.0;
    let text_w = get_text_width(display_text, text_size);
    let text_x = rect.x() + (rect.width() as i32 - text_w) / 2;
    let text_y = rect.y() + 22;
    
    draw_text_with_shadow(canvas, display_text, text_x, text_y, color, text_size);
}

// Draw stats panel
pub fn draw_stats<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    fps: u32,
    codec: &str,
    width: u32,
    height: u32,
    bitrate: u32,
) {
    let bg_color = Color::RGBA(11, 17, 30, 210); // Sleek translucent dark glass
    let border_color = Color::RGBA(0, 240, 255, 100); // Subtle glowing cyan border
    let text_color = TEXT_MAIN;
    let label_color = TEXT_MUTED;

    let rect = Rect::new(15, 60, 220, 95); // Slightly larger for layout comfort
    let radius = 8;
    fill_rounded_rect(canvas, rect, radius, bg_color);
    draw_rounded_rect(canvas, rect, radius, border_color);

    // Draw small glowing green dot (online state indicator)
    let dot_x = rect.x() + 15;
    let dot_y = rect.y() + 14;
    // Outer glow ring
    canvas.set_draw_color(Color::RGBA(0, 255, 148, 100));
    let _ = canvas.draw_rect(Rect::new(dot_x - 2, dot_y - 2, 7, 7));
    // Solid center
    canvas.set_draw_color(STATUS_ONLINE);
    let _ = canvas.fill_rect(Rect::new(dot_x - 1, dot_y - 1, 5, 5));

    // Title: "STATS" in cyan with shadow
    draw_text_with_shadow(canvas, "STATS", rect.x() + 30, rect.y() + 10, ACCENT_CYAN, 12.0);
    
    // Draw thin line divider in dark slate
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    let _ = canvas.draw_line(
        (rect.x() + 10, rect.y() + 24),
        (rect.x() + rect.width() as i32 - 10, rect.y() + 24)
    );

    let fps_val = format!("{}", fps);
    let res_val = format!("{}x{}", width, height);
    let codec_val = format!("{}", codec.to_uppercase());
    let bitrate_val = format!("{} Mbps", bitrate / 1000);

    let start_y = rect.y() + 30;
    let row_h = 14;

    // Render labels and values with drop shadow
    draw_text_with_shadow(canvas, "FPS", rect.x() + 15, start_y, label_color, 11.0);
    draw_text_with_shadow(canvas, &fps_val, rect.x() + 85, start_y, STATUS_ONLINE, 11.0);

    draw_text_with_shadow(canvas, "Res", rect.x() + 15, start_y + row_h, label_color, 11.0);
    draw_text_with_shadow(canvas, &res_val, rect.x() + 85, start_y + row_h, text_color, 11.0);

    draw_text_with_shadow(canvas, "Codec", rect.x() + 15, start_y + row_h * 2, label_color, 11.0);
    draw_text_with_shadow(canvas, &codec_val, rect.x() + 85, start_y + row_h * 2, ACCENT_PURPLE, 11.0);

    draw_text_with_shadow(canvas, "Bitrate", rect.x() + 15, start_y + row_h * 3, label_color, 11.0);
    draw_text_with_shadow(canvas, &bitrate_val, rect.x() + 85, start_y + row_h * 3, text_color, 11.0);
}

// Layout coordinate helper for settings modal
pub struct SettingsLayout {
    pub modal_rect: Rect,
    pub res_btns: Vec<Rect>,
    pub fps_btns: Vec<Rect>,
    pub codec_btns: Vec<Rect>,
    pub bitrate_btns: Vec<Rect>,
    pub apply_btn: Rect,
    pub cancel_btn: Rect,
}

pub fn get_settings_layout(win_w: i32, win_h: i32) -> SettingsLayout {
    let cx = win_w / 2;
    let cy = win_h / 2;
    
    let modal_rect = Rect::new(cx - 200, cy - 160, 400, 320);

    // Row positions
    let res_y = cy - 90;
    let fps_y = cy - 40;
    let codec_y = cy + 10;
    let bitrate_y = cy + 60;

    // Resolutions buttons (3 options: 720p, 1080p, 1440p)
    let res_btns = vec![
        Rect::new(cx - 60, res_y, 45, 22),
        Rect::new(cx - 10, res_y, 50, 22),
        Rect::new(cx + 45, res_y, 50, 22),
    ];

    // FPS buttons (5 options: 60, 75, 100, 120, 240)
    let fps_btns = vec![
        Rect::new(cx - 60, fps_y, 25, 22),
        Rect::new(cx - 30, fps_y, 25, 22),
        Rect::new(cx, fps_y, 35, 22),
        Rect::new(cx + 40, fps_y, 35, 22),
        Rect::new(cx + 80, fps_y, 35, 22),
    ];

    // Codec buttons (3 options: H.264, H.265, AV1)
    let codec_btns = vec![
        Rect::new(cx - 60, codec_y, 50, 22),
        Rect::new(cx - 5, codec_y, 50, 22),
        Rect::new(cx + 50, codec_y, 35, 22),
    ];

    // Bitrates buttons (5 options: 2M, 4M, 8M, 16M, 20M)
    let bitrate_btns = vec![
        Rect::new(cx - 60, bitrate_y, 25, 22),
        Rect::new(cx - 30, bitrate_y, 25, 22),
        Rect::new(cx, bitrate_y, 25, 22),
        Rect::new(cx + 30, bitrate_y, 30, 22),
        Rect::new(cx + 65, bitrate_y, 30, 22),
    ];

    let apply_btn = Rect::new(cx - 150, cy + 110, 140, 30);
    let cancel_btn = Rect::new(cx + 10, cy + 110, 140, 30);

    SettingsLayout {
        modal_rect,
        res_btns,
        fps_btns,
        codec_btns,
        bitrate_btns,
        apply_btn,
        cancel_btn,
    }
}

// Draw the settings modal
pub fn draw_settings<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    win_w: i32,
    win_h: i32,
    res_idx: usize,
    fps_idx: usize,
    codec_idx: usize,
    bitrate_idx: usize,
    mx: i32,
    my: i32,
) {
    let layout = get_settings_layout(win_w, win_h);
    let bg_color = Color::RGBA(11, 17, 30, 240); // Sleek translucent dark glass (high opacity)
    let border_color = Color::RGBA(0, 240, 255, 120); // Cyan border glow
    let text_color = TEXT_MAIN;
    let label_color = TEXT_MUTED;

    // Draw main modal container (radius 12)
    let modal_radius = 12;
    fill_rounded_rect(canvas, layout.modal_rect, modal_radius, bg_color);
    draw_rounded_rect(canvas, layout.modal_rect, modal_radius, border_color);

    // Title
    let title_text = "STREAM CONFIGURATION";
    let title_w = get_text_width(title_text, 14.0);
    let title_x = layout.modal_rect.x() + (layout.modal_rect.width() as i32 - title_w) / 2;
    draw_text_with_shadow(
        canvas,
        title_text,
        title_x,
        layout.modal_rect.y() + 15,
        ACCENT_CYAN,
        14.0,
    );

    // Draw thin line divider under title
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    let _ = canvas.draw_line(
        (layout.modal_rect.x() + 15, layout.modal_rect.y() + 32),
        (layout.modal_rect.x() + layout.modal_rect.width() as i32 - 15, layout.modal_rect.y() + 32),
    );

    // Helper closure to draw settings option rows
    let draw_option_row = |canvas: &mut Canvas<T>, label: &str, btns: &[Rect], selected_idx: usize, labels: &[&str], mx: i32, my: i32| {
        let row_y = btns[0].y() + 5;
        draw_text_with_shadow(canvas, label, layout.modal_rect.x() + 20, row_y - 2, label_color, 11.0);
        for (i, &rect) in btns.iter().enumerate() {
            let is_selected = i == selected_idx;
            let is_hovered = mx >= rect.x() && mx <= rect.x() + rect.width() as i32
                && my >= rect.y() && my <= rect.y() + rect.height() as i32;

            let btn_radius = 4;
            let opt_label = labels[i];
            let opt_size = 11.0;
            let text_w = get_text_width(opt_label, opt_size);
            let text_x = rect.x() + (rect.width() as i32 - text_w) / 2;
            let text_y = rect.y() + 6;

            if is_selected {
                fill_rounded_gradient_rect(canvas, rect, btn_radius, ACCENT_CYAN, ACCENT_PURPLE);
                draw_rounded_rect(canvas, rect, btn_radius, TEXT_MAIN);
                draw_text(canvas, opt_label, text_x, text_y, Color::RGB(8, 12, 20), opt_size);
            } else {
                if is_hovered {
                    fill_rounded_rect(canvas, rect, btn_radius, Color::RGBA(35, 48, 77, 220));
                    draw_rounded_rect(canvas, rect, btn_radius, ACCENT_CYAN);
                    draw_text_with_shadow(canvas, opt_label, text_x, text_y, TEXT_MAIN, opt_size);
                } else {
                    fill_rounded_rect(canvas, rect, btn_radius, Color::RGBA(23, 32, 51, 160));
                    draw_rounded_rect(canvas, rect, btn_radius, Color::RGBA(255, 255, 255, 20));
                    draw_text_with_shadow(canvas, opt_label, text_x, text_y, label_color, opt_size);
                }
            }
        }
    };

    // 1. Resolution row
    let res_labels: Vec<&str> = RESOLUTIONS.iter().map(|r| r.0).collect();
    draw_option_row(canvas, "Resolution:", &layout.res_btns, res_idx, &res_labels, mx, my);

    // 2. FPS row
    let fps_labels: Vec<&str> = FPSS.iter().map(|f| f.0).collect();
    draw_option_row(canvas, "Frame Rate:", &layout.fps_btns, fps_idx, &fps_labels, mx, my);

    // 3. Codec row
    let codec_labels: Vec<&str> = CODECS.iter().map(|c| c.0).collect();
    draw_option_row(canvas, "Video Codec:", &layout.codec_btns, codec_idx, &codec_labels, mx, my);

    // 4. Bitrate row
    let bitrate_labels: Vec<&str> = BITRATES.iter().map(|b| b.0).collect();
    draw_option_row(canvas, "Bitrate:", &layout.bitrate_btns, bitrate_idx, &bitrate_labels, mx, my);

    // Action buttons at the bottom

    // Apply button (Cyan-to-Purple gradient with hover indicator)
    let apply_hovered = mx >= layout.apply_btn.x() && mx <= layout.apply_btn.x() + layout.apply_btn.width() as i32
        && my >= layout.apply_btn.y() && my <= layout.apply_btn.y() + layout.apply_btn.height() as i32;
    fill_rounded_gradient_rect(canvas, layout.apply_btn, 6, ACCENT_CYAN, ACCENT_PURPLE);
    if apply_hovered {
        draw_rounded_rect(canvas, layout.apply_btn, 6, ACCENT_CYAN);
    } else {
        draw_rounded_rect(canvas, layout.apply_btn, 6, TEXT_MAIN);
    }
    let apply_text = "Apply & Stream";
    let apply_size = 12.0;
    let apply_w = get_text_width(apply_text, apply_size);
    let text_x = layout.apply_btn.x() + (layout.apply_btn.width() as i32 - apply_w) / 2;
    draw_text(canvas, apply_text, text_x, layout.apply_btn.y() + 9, Color::RGB(8, 12, 20), apply_size);

    // Cancel button (Secondary)
    let cancel_hovered = mx >= layout.cancel_btn.x() && mx <= layout.cancel_btn.x() + layout.cancel_btn.width() as i32
        && my >= layout.cancel_btn.y() && my <= layout.cancel_btn.y() + layout.cancel_btn.height() as i32;
    if cancel_hovered {
        fill_rounded_rect(canvas, layout.cancel_btn, 6, Color::RGBA(35, 48, 77, 220));
        draw_rounded_rect(canvas, layout.cancel_btn, 6, ACCENT_CYAN);
    } else {
        fill_rounded_rect(canvas, layout.cancel_btn, 6, Color::RGBA(23, 32, 51, 160));
        draw_rounded_rect(canvas, layout.cancel_btn, 6, Color::RGBA(255, 255, 255, 20));
    }
    let cancel_text = "Cancel";
    let cancel_size = 12.0;
    let cancel_w = get_text_width(cancel_text, cancel_size);
    let text_x = layout.cancel_btn.x() + (layout.cancel_btn.width() as i32 - cancel_w) / 2;
    if cancel_hovered {
        draw_text_with_shadow(canvas, cancel_text, text_x, layout.cancel_btn.y() + 9, TEXT_MAIN, cancel_size);
    } else {
        draw_text_with_shadow(canvas, cancel_text, text_x, layout.cancel_btn.y() + 9, text_color, cancel_size);
    }
}

