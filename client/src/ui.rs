use sdl2::render::Canvas;
use sdl2::render::RenderTarget;
use sdl2::pixels::Color;
use sdl2::rect::Rect;
use font8x8::UnicodeFonts;

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

// Draw a linear gradient rectangle horizontally from color_start to color_end
fn draw_gradient_rect<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    rect: Rect,
    color_start: Color,
    color_end: Color,
) {
    let w = rect.width() as i32;
    let h = rect.height() as i32;
    for x in 0..w {
        let t = x as f32 / w as f32;
        let r = (color_start.r as f32 * (1.0 - t) + color_end.r as f32 * t) as u8;
        let g = (color_start.g as f32 * (1.0 - t) + color_end.g as f32 * t) as u8;
        let b = (color_start.b as f32 * (1.0 - t) + color_end.b as f32 * t) as u8;
        let a = (color_start.a as f32 * (1.0 - t) + color_end.a as f32 * t) as u8;
        canvas.set_draw_color(Color::RGBA(r, g, b, a));
        let _ = canvas.draw_line((rect.x() + x, rect.y()), (rect.x() + x, rect.y() + h - 1));
    }
}

// Draw text on canvas using font8x8
pub fn draw_text<T: RenderTarget>(
    canvas: &mut Canvas<T>,
    text: &str,
    x: i32,
    y: i32,
    color: Color,
    scale: u32,
) {
    canvas.set_draw_color(color);
    let mut current_x = x;
    for c in text.chars() {
        if let Some(glyph) = font8x8::BASIC_FONTS.get(c) {
            for row in 0..8 {
                let byte = glyph[row];
                for col in 0..8 {
                    if (byte & (1 << col)) != 0 {
                        let px = current_x + col as i32 * scale as i32;
                        let py = y + row as i32 * scale as i32;
                        let _ = canvas.fill_rect(Rect::new(px, py, scale, scale));
                    }
                }
            }
        }
        current_x += (8 * scale) as i32 + 2; // character width + spacing
    }
}

// Collapsed trigger notch dimensions
pub fn get_trigger_rect(win_w: i32) -> Rect {
    let cx = win_w / 2;
    Rect::new(cx - 25, 0, 50, 14)
}

// Menu pill dimensions
pub fn get_menu_rect(win_w: i32, y_offset: i32) -> Rect {
    let cx = win_w / 2;
    Rect::new(cx - 180, y_offset, 360, 38)
}

// Menu buttons layouts
pub fn get_menu_buttons(win_w: i32, y_offset: i32) -> Vec<(Rect, &'static str)> {
    let cx = win_w / 2;
    vec![
        (Rect::new(cx - 170, y_offset + 5, 50, 28), "Exit"),
        (Rect::new(cx - 110, y_offset + 5, 40, 28), "FS"),
        (Rect::new(cx - 60, y_offset + 5, 50, 28), "Lock"),
        (Rect::new(cx, y_offset + 5, 60, 28), "Stats"),
        (Rect::new(cx + 70, y_offset + 5, 90, 28), "Settings"),
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
) {
    let collapsed_bg = Color::RGBA(15, 22, 38, 200); // BG_SECONDARY with alpha
    let menu_bg = Color::RGBA(15, 22, 38, 220); // BG_SECONDARY with alpha
    let border_color = ACCENT_CYAN;
    let border_color_collapsed = ACCENT_PURPLE;
    let text_color = TEXT_MAIN;

    if !show_menu && y_offset <= -35 {
        // Draw collapsed trigger notch
        let trig = get_trigger_rect(win_w);
        canvas.set_draw_color(collapsed_bg);
        let _ = canvas.fill_rect(trig);
        canvas.set_draw_color(border_color_collapsed);
        let _ = canvas.draw_rect(trig);
        draw_text(canvas, "V", trig.x() + 21, trig.y() + 3, ACCENT_CYAN, 1);
    } else {
        // Draw expanded menu pill
        let menu_rect = get_menu_rect(win_w, y_offset);
        canvas.set_draw_color(menu_bg);
        let _ = canvas.fill_rect(menu_rect);
        canvas.set_draw_color(border_color);
        let _ = canvas.draw_rect(menu_rect);

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

            if is_active {
                // Active buttons get a cyan-to-purple gradient background, dark text
                draw_gradient_rect(canvas, rect, ACCENT_CYAN, ACCENT_PURPLE);
                canvas.set_draw_color(TEXT_MAIN);
                let _ = canvas.draw_rect(rect);
                // Center text
                let label_len = label.len() as i32;
                let text_x = rect.x() + (rect.width() as i32 - label_len * 10) / 2;
                let text_y = rect.y() + 10;
                draw_text(canvas, label, text_x, text_y, Color::RGB(8, 12, 20), 1);
            } else {
                // Normal buttons get tertiary background, subtle white border, main text
                canvas.set_draw_color(Color::RGBA(23, 32, 51, 200)); // BG_TERTIARY with alpha
                let _ = canvas.fill_rect(rect);
                canvas.set_draw_color(Color::RGBA(255, 255, 255, 25)); // subtle white border
                let _ = canvas.draw_rect(rect);
                // Center text
                let label_len = label.len() as i32;
                let text_x = rect.x() + (rect.width() as i32 - label_len * 10) / 2;
                let text_y = rect.y() + 10;
                draw_text(canvas, label, text_x, text_y, text_color, 1);
            }
        }
    }
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
    let bg_color = Color::RGBA(15, 22, 38, 200); // BG_SECONDARY with alpha
    let border_color = STATUS_ONLINE; // Glowing neon green
    let text_color = TEXT_MAIN;
    let label_color = TEXT_MUTED;

    let rect = Rect::new(15, 50, 200, 85);
    canvas.set_draw_color(bg_color);
    let _ = canvas.fill_rect(rect);
    canvas.set_draw_color(border_color);
    let _ = canvas.draw_rect(rect);

    // Draw Title: "STATS" in cyan
    draw_text(canvas, "STATS", rect.x() + 10, rect.y() + 8, ACCENT_CYAN, 1);
    // Draw thin line divider in slate
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    let _ = canvas.draw_line((rect.x() + 5, rect.y() + 20), (rect.x() + rect.width() as i32 - 5, rect.y() + 20));

    let fps_val = format!("{}", fps);
    let res_val = format!("{}x{}", width, height);
    let codec_val = format!("{}", codec.to_uppercase());
    let bitrate_val = format!("{} Mbps", bitrate / 1000);

    // Render labels and values
    // FPS:
    draw_text(canvas, "FPS:", rect.x() + 10, rect.y() + 26, label_color, 1);
    draw_text(canvas, &fps_val, rect.x() + 75, rect.y() + 26, STATUS_ONLINE, 1);

    // Res:
    draw_text(canvas, "Res:", rect.x() + 10, rect.y() + 40, label_color, 1);
    draw_text(canvas, &res_val, rect.x() + 75, rect.y() + 40, text_color, 1);

    // Codec:
    draw_text(canvas, "Codec:", rect.x() + 10, rect.y() + 54, label_color, 1);
    draw_text(canvas, &codec_val, rect.x() + 75, rect.y() + 54, ACCENT_PURPLE, 1);

    // Bitrate:
    draw_text(canvas, "Bit:", rect.x() + 10, rect.y() + 68, label_color, 1);
    draw_text(canvas, &bitrate_val, rect.x() + 75, rect.y() + 68, text_color, 1);
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
) {
    let layout = get_settings_layout(win_w, win_h);
    let bg_color = Color::RGBA(15, 22, 38, 245); // BG_SECONDARY with high opacity
    let border_color = ACCENT_CYAN; // Cyan glow border
    let text_color = TEXT_MAIN;
    let label_color = TEXT_MUTED;
    let btn_bg_color = Color::RGBA(23, 32, 51, 200); // BG_TERTIARY
    let btn_border_color = Color::RGBA(255, 255, 255, 20); // subtle white border

    // Draw main modal container
    canvas.set_draw_color(bg_color);
    let _ = canvas.fill_rect(layout.modal_rect);
    canvas.set_draw_color(border_color);
    let _ = canvas.draw_rect(layout.modal_rect);

    // Title
    draw_text(
        canvas,
        "STREAM CONFIGURATION",
        layout.modal_rect.x() + 90,
        layout.modal_rect.y() + 15,
        ACCENT_CYAN,
        1,
    );

    // Draw thin line divider under title
    canvas.set_draw_color(Color::RGBA(255, 255, 255, 15));
    let _ = canvas.draw_line(
        (layout.modal_rect.x() + 15, layout.modal_rect.y() + 32),
        (layout.modal_rect.x() + layout.modal_rect.width() as i32 - 15, layout.modal_rect.y() + 32),
    );

    // Helper closure to draw settings option rows
    let draw_option_row = |canvas: &mut Canvas<T>, label: &str, btns: &[Rect], selected_idx: usize, labels: &[&str]| {
        let row_y = btns[0].y() + 5;
        draw_text(canvas, label, layout.modal_rect.x() + 20, row_y, label_color, 1);
        for (i, &rect) in btns.iter().enumerate() {
            let is_selected = i == selected_idx;
            if is_selected {
                draw_gradient_rect(canvas, rect, ACCENT_CYAN, ACCENT_PURPLE);
                canvas.set_draw_color(TEXT_MAIN);
                let _ = canvas.draw_rect(rect);
                let opt_label = labels[i];
                let label_len = opt_label.len() as i32;
                let text_x = rect.x() + (rect.width() as i32 - label_len * 10) / 2;
                draw_text(canvas, opt_label, text_x, rect.y() + 6, Color::RGB(8, 12, 20), 1);
            } else {
                canvas.set_draw_color(btn_bg_color);
                let _ = canvas.fill_rect(rect);
                canvas.set_draw_color(btn_border_color);
                let _ = canvas.draw_rect(rect);
                let opt_label = labels[i];
                let label_len = opt_label.len() as i32;
                let text_x = rect.x() + (rect.width() as i32 - label_len * 10) / 2;
                draw_text(canvas, opt_label, text_x, rect.y() + 6, label_color, 1);
            }
        }
    };

    // 1. Resolution row
    let res_labels: Vec<&str> = RESOLUTIONS.iter().map(|r| r.0).collect();
    draw_option_row(canvas, "Resolution:", &layout.res_btns, res_idx, &res_labels);

    // 2. FPS row
    let fps_labels: Vec<&str> = FPSS.iter().map(|f| f.0).collect();
    draw_option_row(canvas, "Frame Rate:", &layout.fps_btns, fps_idx, &fps_labels);

    // 3. Codec row
    let codec_labels: Vec<&str> = CODECS.iter().map(|c| c.0).collect();
    draw_option_row(canvas, "Video Codec:", &layout.codec_btns, codec_idx, &codec_labels);

    // 4. Bitrate row
    let bitrate_labels: Vec<&str> = BITRATES.iter().map(|b| b.0).collect();
    draw_option_row(canvas, "Bitrate:", &layout.bitrate_btns, bitrate_idx, &bitrate_labels);

    // Action buttons at the bottom

    // Apply button (Cyan-to-Purple gradient with dark text)
    draw_gradient_rect(canvas, layout.apply_btn, ACCENT_CYAN, ACCENT_PURPLE);
    canvas.set_draw_color(TEXT_MAIN);
    let _ = canvas.draw_rect(layout.apply_btn);
    // Center the text
    let apply_text = "Apply & Stream";
    let text_x = layout.apply_btn.x() + (layout.apply_btn.width() as i32 - apply_text.len() as i32 * 10) / 2;
    draw_text(canvas, apply_text, text_x, layout.apply_btn.y() + 10, Color::RGB(8, 12, 20), 1);

    // Cancel button (Secondary)
    canvas.set_draw_color(btn_bg_color);
    let _ = canvas.fill_rect(layout.cancel_btn);
    canvas.set_draw_color(btn_border_color);
    let _ = canvas.draw_rect(layout.cancel_btn);
    // Center the text
    let cancel_text = "Cancel";
    let text_x = layout.cancel_btn.x() + (layout.cancel_btn.width() as i32 - cancel_text.len() as i32 * 10) / 2;
    draw_text(canvas, cancel_text, text_x, layout.cancel_btn.y() + 10, text_color, 1);
}

