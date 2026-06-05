# Fix H265, Stabilize FPS, Display Selection & Virtual Display

## Vấn đề hiện tại

1. **H265 không hoạt động** trên cả Linux và Windows
2. **FPS không ổn định** — nhảy 40-55 thay vì ổn định 60
3. **FPS > 60 không hoạt động** — chọn 90fps vẫn thấy 60fps
4. **Không có tùy chọn chọn màn hình** — tất cả Linux backend capture toàn bộ root window

---

## Phase 1: H265 Encoder Fix

### [MODIFY] [ffmpeg.rs](file:///home/huy/Projects/lunaris-media/src/encode/ffmpeg.rs)

**Root cause**: HW encoders thiếu `repeat-headers` cho HEVC → decoder không thể decode sau PLI/reconnect. VAAPI/QSV/AMF thiếu profile và options.

Thay đổi `set_encoder_options()` (L499-580):

| Encoder | Thêm option | Mục đích |
|---------|-------------|----------|
| NVENC | `repeat_vps_sps_pps=1` | Emit VPS/SPS/PPS mỗi IDR |
| VAAPI | `profile=main` (H265), `profile=high` (H264) | Thiếu profile → encoder có thể chọn sai |
| QSV | `forced_idr=1`, `repeat_pps=1`, `profile` | Cần IDR + repeat headers |
| AMF | `usage=ultralowlatency`, `rc=cbr`, `header_insertion_mode=idr`, `profile` | Hoàn toàn thiếu options |
| VideoToolbox | `realtime=1`, `profile` | Hoàn toàn thiếu options |

---

## Phase 2: FPS Stabilization

### [MODIFY] [pipeline.rs](file:///home/huy/Projects/lunaris-media/src/pipeline.rs)

**Root cause**: Pipeline loop không có timer — phụ thuộc capture speed. Encoding đồng bộ chặn capture.

Thay đổi main loop (L261-334):
- Thay `capture.next_frame()` trực tiếp trong `select!` bằng `tokio::time::interval` timer
- Dùng `MissedTickBehavior::Skip` — nếu encode chậm hơn 1 tick thì bỏ tick thay vì dồn
- Reset interval khi `SetFps` command

```rust
let mut frame_interval = tokio::time::interval(target_interval);
frame_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

loop {
    tokio::select! {
        _ = frame_interval.tick() => {
            let frame_result = capture.next_frame().await;
            // ... encode + send ...
        }
        Some(cmd) = self.command_rx.recv() => { ... }
    }
}
```

### [MODIFY] [linux_x11.rs](file:///home/huy/Projects/lunaris-media/src/capture/linux_x11.rs)

**Root cause**: X11 capture có frame pacing riêng **CHỒNG CHÉO** với pipeline → double pacing.

Loại bỏ frame pacing khỏi `next_frame()` (L478-486):
```diff
-    let target_interval = Duration::from_nanos(1_000_000_000 / self.fps as u64);
-    let elapsed = self.last_frame_time.elapsed();
-    if elapsed < target_interval {
-        tokio::time::sleep(target_interval - elapsed).await;
-    } else {
-        tokio::task::yield_now().await;
-    }
-    self.last_frame_time = std::time::Instant::now();
+    // Frame pacing handled by pipeline.rs interval timer
+    tokio::task::yield_now().await;
```

---

## Phase 3: Support FPS > 60

### Phân tích

FPS bị giới hạn bởi **refresh rate của monitor**:
- NvFBC push model: chỉ capture frame mới khi GPU render → giới hạn bởi vsync
- X11 XShm: capture root window → chỉ có frame mới ở tần số refresh
- Để stream 90fps, **display phải chạy ≥90Hz**

### [MODIFY] [pipeline.rs](file:///home/huy/Projects/lunaris-media/src/pipeline.rs)

Thêm logic tự động thay đổi display refresh rate trước khi bắt đầu capture:

```rust
// Before capture.start(), check if display refresh rate is sufficient
if self.config.fps > 60 {
    if let Ok(displays) = capture.list_displays().await {
        if let Some(display) = displays.iter().find(|d| d.id == display_id || d.is_primary) {
            if (display.refresh_rate as u32) < self.config.fps {
                log::info!("Target FPS {} > display refresh rate {}, attempting to change...",
                    self.config.fps, display.refresh_rate);
                Self::try_set_refresh_rate(&display.id, self.config.fps);
            }
        }
    }
}
```

### [NEW] Thêm hàm `try_set_refresh_rate` trong `pipeline.rs`

Dùng `xrandr` để thay đổi refresh rate:
```rust
fn try_set_refresh_rate(display_id: &str, target_fps: u32) {
    // xrandr --output DP-1 --rate 90
    let output = std::process::Command::new("xrandr")
        .args(["--output", display_id, "--rate", &target_fps.to_string()])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            log::info!("Changed display {} refresh rate to {}Hz", display_id, target_fps);
        }
        _ => {
            log::warn!("Failed to change refresh rate to {}Hz. FPS will be limited to display's refresh rate.", target_fps);
        }
    }
}
```

> [!IMPORTANT]
> Chỉ hoạt động trên Linux X11. Windows cần dùng `ChangeDisplaySettingsW` API (sẽ thêm sau).
> Monitor phải hỗ trợ refresh rate mục tiêu (vd: 90Hz, 120Hz, 144Hz).

---

## Phase 4: Multi-Display Selection

### Hiện trạng

| Backend | `list_displays()` | `start()` dùng `display_id` |
|---------|-------------------|--------------------------|
| X11 | ❌ Hardcoded "default" | ❌ Ignored |
| NvFBC | ❌ Hardcoded "default" | ❌ Ignored |
| DRM/KMS | ❌ First CRTC only | ❌ Ignored |
| PipeWire | ⚠️ Portal picker | ⚠️ Logged only |
| Windows DXGI | ✅ Enumerates all | ✅ Full support |

### [MODIFY] [linux_x11.rs](file:///home/huy/Projects/lunaris-media/src/capture/linux_x11.rs)

1. **`list_displays()`**: Thay hardcoded "default" bằng gọi `parse_xrandr_output()` (code đã có nhưng là dead code)
2. **`start()`**: Lưu offset (x, y) của display được chọn, dùng offset trong `XShmGetImage`/`XGetImage` để capture đúng vùng

```rust
async fn list_displays(&self) -> Result<Vec<DisplayInfo>, MediaError> {
    let output = std::process::Command::new("xrandr")
        .arg("--query")
        .output()
        .map_err(|e| MediaError::CaptureError(format!("xrandr failed: {}", e)))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut displays = Self::parse_xrandr_output(&text);
    if displays.is_empty() {
        displays.push(DisplayInfo {
            id: "default".into(), name: "Default X11 Display".into(),
            width: self.width, height: self.height,
            refresh_rate: 60.0, is_primary: true,
        });
    }
    Ok(displays)
}
```

Thêm fields `capture_x`, `capture_y` vào struct, set trong `start()` dựa trên display position từ xrandr.

### [MODIFY] [linux_nvfbc.rs](file:///home/huy/Projects/lunaris-media/src/capture/linux_nvfbc.rs)

1. **`list_displays()`**: Dùng NvFBC status API enumerate outputs
2. **`start()`**: Dùng `NVFBC_TRACKING_OUTPUT` + output ID thay vì `NVFBC_TRACKING_DEFAULT`

```rust
// In custom_start_nvfbc():
if display_id != "default" {
    params.eTrackingType = NVFBC_TRACKING_OUTPUT;
    params.dwOutputId = display_id.parse().unwrap_or(0);
} else {
    params.eTrackingType = NVFBC_TRACKING_DEFAULT;
}
```

---

## Phase 5: Virtual Display (xrandr)

### [NEW] [virtual_display.rs](file:///home/huy/Projects/lunaris-media/src/capture/virtual_display.rs)

Module quản lý virtual display, hoạt động giống Sunshine:
- Tạo virtual display khi stream bắt đầu (nếu client yêu cầu)
- Xóa virtual display khi stream kết thúc

```rust
pub struct VirtualDisplay {
    output_name: String,
    mode_name: String,
    active: bool,
}

impl VirtualDisplay {
    /// Tạo virtual display bằng xrandr
    pub fn create(width: u32, height: u32, fps: u32) -> Result<Self, MediaError> {
        // 1. cvt <width> <height> <fps> → get modeline
        // 2. xrandr --newmode "custom" <modeline>
        // 3. Find VIRTUAL output (xrandr --query | grep "VIRTUAL.*disconnected")
        // 4. xrandr --addmode VIRTUAL1 "custom"
        // 5. xrandr --output VIRTUAL1 --mode "custom"
    }

    /// Xóa virtual display
    pub fn destroy(&mut self) -> Result<(), MediaError> {
        // xrandr --output VIRTUAL1 --off
        // xrandr --delmode VIRTUAL1 "custom"
        // xrandr --rmmode "custom"
    }
}
```

### [MODIFY] [pipeline.rs](file:///home/huy/Projects/lunaris-media/src/pipeline.rs)

Thêm `StreamConfig.virtual_display: bool` option. Nếu `true`, tạo virtual display trước khi capture:

```rust
let _virtual_display = if self.config.virtual_display {
    match VirtualDisplay::create(self.config.width, self.config.height, self.config.fps) {
        Ok(vd) => {
            log::info!("Created virtual display: {}", vd.output_name);
            Some(vd)
        }
        Err(e) => {
            log::warn!("Failed to create virtual display: {}", e);
            None
        }
    }
} else {
    None
};
// On drop, virtual display is automatically destroyed
```

> [!WARNING]
> Virtual display trên Linux **cần cấu hình Xorg** với `Option "VirtualHeads" "1"` (hoặc dùng `video=` kernel param cho output rỗng).
> Đây không phải plug-and-play — cần restart X server lần đầu. Sẽ thêm hướng dẫn setup cho user.

> [!IMPORTANT]
> **Windows virtual display** cần driver riêng (IddSampleDriver). Sẽ implement ở phase sau.

---

## Open Questions

1. **Phase 5 (Virtual Display)**: Virtual display trên Linux yêu cầu cấu hình Xorg trước. Có muốn tôi tự động tạo file `/etc/X11/xorg.conf.d/10-virtual-heads.conf` không? Hay chỉ cần hướng dẫn user?

2. **Priority**: Phase 1-2 (H265 + FPS) quan trọng nhất. Phase 3-5 có thể làm sau. Bạn muốn làm tất cả hay chia nhỏ?

---

## Verification Plan

### Build Check
```bash
cd /home/huy/Projects/lunaris-media && cargo build --release
cd /home/huy/Projects/lunaris && cargo build --release -p agent
```

### Manual Verification
- Test H265 streaming từ web → agent Linux
- Test H264 để đảm bảo không regression
- Kiểm tra FPS counter ổn định ~60 sau fix
- Test chọn 90fps trên monitor 144Hz
- Test chọn display cụ thể (multi-monitor setup)
