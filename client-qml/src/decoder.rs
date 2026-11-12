use ffmpeg_next::sys as ffi;

// SWS_BILINEAR is 2 in FFmpeg. We define it locally because on some platforms/FFmpeg versions
// bindgen generates it as a global constant, while on others it is an enum variant (SwsFlags::SWS_BILINEAR).
const SWS_BILINEAR: i32 = 2;
const DRM_FORMAT_NV12: u32 = 0x3231_564e;
const DRM_FORMAT_R8: u32 = 0x2020_3852; // 'R8  '
const DRM_FORMAT_GR88: u32 = 0x3838_5247; // 'GR88'

#[cfg(target_os = "linux")]
extern "C" {
    fn dup(fd: i32) -> i32;
    fn close(fd: i32) -> i32;
}

struct SendRawFrame {
    frame: *mut ffi::AVFrame,
    decoder_ptr: usize,
}
unsafe impl Send for SendRawFrame {}

static ACTIVE_CUDA_FRAME: std::sync::Mutex<Option<SendRawFrame>> = std::sync::Mutex::new(None);

pub fn clear_active_cuda_frame() {
    let mut lock = ACTIVE_CUDA_FRAME.lock().unwrap();
    if let Some(SendRawFrame { frame, .. }) = lock.take() {
        unsafe {
            ffi::av_frame_free(&mut (frame as *mut _));
        }
    }
}

pub fn clear_active_cuda_frame_for_decoder(decoder_ptr: usize) {
    let mut lock = ACTIVE_CUDA_FRAME.lock().unwrap();
    if let Some(ref r) = *lock {
        if r.decoder_ptr == decoder_ptr {
            let taken = lock.take();
            if let Some(SendRawFrame { frame, .. }) = taken {
                unsafe {
                    ffi::av_frame_free(&mut (frame as *mut _));
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CodecType {
    H264,
    H265,
    AV1,
}

pub struct YUVFrame {
    pub width: i32,
    pub height: i32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub y_stride: i32,
    pub u_stride: i32,
    pub v_stride: i32,
}

pub enum DecodedFrame {
    CpuYuv(YUVFrame),
    /// Packed BGRX for formats QVideoSink can't take without losing chroma
    /// (4:4:4 streams).
    CpuBgrx(BgrxFrame),
    NativePresented,
}

pub struct BgrxFrame {
    pub width: i32,
    pub height: i32,
    pub data: Vec<u8>,
    pub stride: i32,
}

/// Matrix (BT.709 or not: BT.601) and range (full or studio) of a frame
/// from its tags; untagged frames are BT.709 studio range, as decoders
/// assume for HD video (and the agent's untagged encoders produce).
pub fn frame_color(colorspace: i32, color_range: i32, format: i32) -> (bool, bool) {
    let bt601 = colorspace == ffi::AVColorSpace::AVCOL_SPC_BT470BG as i32
        || colorspace == ffi::AVColorSpace::AVCOL_SPC_SMPTE170M as i32;
    let full = color_range == ffi::AVColorRange::AVCOL_RANGE_JPEG as i32
        || format == ffi::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32
        || format == ffi::AVPixelFormat::AV_PIX_FMT_YUVJ444P as i32;
    (!bt601, full)
}

pub struct HardwareDecoder {
    codec_ctx: *mut ffi::AVCodecContext,
    hw_device_ctx: *mut ffi::AVBufferRef,
    hw_device_type: ffi::AVHWDeviceType,
    hardware_decode_requested: bool,
    sws_ctx: *mut ffi::SwsContext,
    last_width: i32,
    last_height: i32,
    last_format: i32,
    /// YUV 4:4:4 -> BGRX converter and the (width, height, format, colorspace,
    /// range) it was made for.
    bgrx_sws_ctx: *mut ffi::SwsContext,
    bgrx_key: (i32, i32, i32, i32, i32),
    /// Why VAAPI frames went through the CPU was logged once.
    dmabuf_layout_logged: bool,
    /// (BT.709, full range) of the last frame, to log changes.
    last_color: Option<(bool, bool)>,
}

unsafe impl Send for HardwareDecoder {}

impl HardwareDecoder {
    fn linux_nvidia_cuda_present() -> bool {
        if !cfg!(target_os = "linux") {
            return false;
        }
        std::path::Path::new("/dev/nvidiactl").exists()
            || std::path::Path::new("/proc/driver/nvidia/version").exists()
    }

    pub fn new(codec_type: CodecType, disable_cuda: bool) -> Result<Self, anyhow::Error> {
        unsafe {
            ffi::av_log_set_level(ffi::AV_LOG_WARNING);
        }

        let codec_id = match codec_type {
            CodecType::H264 => ffi::AVCodecID::AV_CODEC_ID_H264,
            CodecType::H265 => ffi::AVCodecID::AV_CODEC_ID_HEVC,
            CodecType::AV1 => ffi::AVCodecID::AV_CODEC_ID_AV1,
        };

        let codec = unsafe { ffi::avcodec_find_decoder(codec_id) };
        if codec.is_null() {
            return Err(anyhow::anyhow!(
                "Failed to find FFmpeg decoder for codec {:?}",
                codec_id
            ));
        }

        let codec_ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
        if codec_ctx.is_null() {
            return Err(anyhow::anyhow!("Failed to allocate FFmpeg codec context"));
        }

        unsafe {
            (*codec_ctx).flags |= ffi::AV_CODEC_FLAG_LOW_DELAY as i32;
            (*codec_ctx).thread_count = 1;
        }

        let mut hw_device_ctx: *mut ffi::AVBufferRef = std::ptr::null_mut();
        let mut hw_device_type = ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE;
        let hardware_decode_requested = !disable_cuda;

        // (device type, device to open: None = FFmpeg's default)
        let mut candidates: Vec<(ffi::AVHWDeviceType, Option<String>)> = Vec::new();
        if hardware_decode_requested {
            let cuda_disabled = std::env::var("LUNARIS_DISABLE_CUDA").is_ok();
            // Use the same logic as cuda_gl_requested() — auto-enable on
            // Linux, respect explicit env var overrides.
            let cuda_gl_on = Self::cuda_gl_requested();

            if cfg!(target_os = "linux") {
                // Displays on another GPU than NVIDIA (e.g. an AMD card with
                // the NVIDIA one driving no monitor): CUDA frames can't reach
                // the app's OpenGL without a copy through the CPU, so decode
                // with VAAPI on the display GPU first (frames go to OpenGL as
                // DMA-BUFs). FFmpeg's default VAAPI device is renderD128,
                // often an unrelated GPU.
                let display = crate::display_gpu::display_gpu();
                let display_node = display
                    .as_ref()
                    .filter(|g| g.vendor != crate::display_gpu::Vendor::Nvidia)
                    .and_then(|g| g.render_node.clone());
                if let Some(node) = &display_node {
                    println!("Displays run on {:?} ({}): trying VAAPI decoding there first", display.as_ref().map(|g| g.vendor), node);
                    candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, Some(node.clone())));
                }
                let prefer_cuda_decode = !cuda_disabled
                    && (cuda_gl_on || Self::linux_nvidia_cuda_present());
                if prefer_cuda_decode {
                    candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, None));
                }
                candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, None));
                candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VDPAU, None));
                if !prefer_cuda_decode && !cuda_disabled {
                    candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, None));
                } else if cuda_disabled {
                    println!("CUDA hardware decoding is disabled.");
                }
            } else if cfg!(target_os = "windows") {
                if cuda_gl_on && !cuda_disabled {
                    candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, None));
                }
                candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, None));
                candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2, None));
                if !cuda_gl_on && !cuda_disabled {
                    candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA, None));
                }
            } else if cfg!(target_os = "macos") {
                candidates.push((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, None));
            }
        } else {
            println!("All hardware decoding is disabled (Software/FFmpeg mode).");
        }

        for (dev_type, device) in &candidates {
            let device_c = device.as_deref().and_then(|d| std::ffi::CString::new(d).ok());
            let err = unsafe {
                ffi::av_hwdevice_ctx_create(
                    &mut hw_device_ctx,
                    *dev_type,
                    device_c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                    std::ptr::null_mut(),
                    0,
                )
            };
            if err >= 0 && !hw_device_ctx.is_null() {
                hw_device_type = *dev_type;
                break;
            }
        }

        if hw_device_type != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            unsafe {
                (*codec_ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device_ctx);
                let name = std::ffi::CStr::from_ptr(ffi::av_hwdevice_get_type_name(hw_device_type))
                    .to_string_lossy();
                println!(
                    "Successfully initialized GPU hardware decoding context: {}",
                    name
                );
            }
        } else {
            if hardware_decode_requested {
                println!("Failed to create GPU hardware decoding context. Falling back to software decoding.");
            }
            // Software decoding: use slice threads. They decode the slices of
            // one frame in parallel without the extra frames of delay that
            // frame threading adds (x264 with zerolatency writes one slice per
            // encoder thread), which a 1080p60 4:4:4 stream needs.
            unsafe {
                (*codec_ctx).thread_type = ffi::FF_THREAD_SLICE as i32;
                (*codec_ctx).thread_count = 0;
            }
        }

        let open_err = unsafe { ffi::avcodec_open2(codec_ctx, codec, std::ptr::null_mut()) };
        if open_err < 0 {
            unsafe {
                if !hw_device_ctx.is_null() {
                    ffi::av_buffer_unref(&mut hw_device_ctx);
                }
                ffi::avcodec_free_context(&mut (codec_ctx as *mut _));
            }
            return Err(anyhow::anyhow!("Failed to open FFmpeg codec: {}", open_err));
        }

        Ok(HardwareDecoder {
            codec_ctx,
            hw_device_ctx,
            hw_device_type,
            hardware_decode_requested,
            sws_ctx: std::ptr::null_mut(),
            last_width: 0,
            last_height: 0,
            last_format: -1,
            bgrx_sws_ctx: std::ptr::null_mut(),
            bgrx_key: (0, 0, -1, -1, -1),
            dmabuf_layout_logged: false,
            last_color: None,
        })
    }

    pub fn decode_backend_label(&self) -> &'static str {
        match self.hw_device_type {
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA => "CUDA",
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA => "D3D11VA",
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2 => "DXVA2",
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI => "VAAPI",
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VDPAU => "VDPAU",
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX => "VideoToolbox",
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE => "Software",
            _ => "GPU",
        }
    }

    /// Whether the CUDA-GL native present path is requested.
    /// Explicit `LUNARIS_CLIENT_CUDA_GL=1` forces it on; `=0` forces it off.
    /// When unset, off: `lib.rs` sets it when OpenGL runs on an NVIDIA GPU.
    fn cuda_gl_requested() -> bool {
        match std::env::var("LUNARIS_CLIENT_CUDA_GL") {
            Ok(ref v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
            Ok(_) => false, // explicit "0" / "false" / etc.
            Err(_) => {
                // Not set: lib.rs found no OpenGL scenegraph or NVIDIA card,
                // or CUDA-GL was turned off (LUNARIS_CLIENT_CUDA_GL=0 is
                // cleared there). Interop only works with OpenGL on NVIDIA.
                false
            }
        }
    }

    /// Whether the VAAPI DMABUF-GL native present path is requested.
    /// Explicit `LUNARIS_CLIENT_DMABUF_GL=1` forces it on; `=0` forces it off.
    /// When unset, auto-enable on Linux (where VAAPI + EGL DMABUF may work),
    /// otherwise off.
    fn dmabuf_gl_requested() -> bool {
        match std::env::var("LUNARIS_CLIENT_DMABUF_GL") {
            Ok(ref v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
            Ok(_) => false,
            Err(_) => cfg!(target_os = "linux"),
        }
    }

    pub fn gpu_decode_enabled(&self) -> bool {
        self.hw_device_type != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE
    }

    pub fn present_backend_label(&self) -> &'static str {
        if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return "CPU/QVideoSink";
        }

        let direct_cuda_gl = self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA
            && Self::cuda_gl_requested()
            && !crate::bridge::qobject::cuda_gl_render_failed();

        if direct_cuda_gl {
            return "CUDA-GL";
        }

        if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
            && Self::dmabuf_gl_requested()
            && !crate::bridge::qobject::dmabuf_render_failed()
        {
            return "DMABUF/OpenGL";
        }

        if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
            && !crate::bridge::qobject::d3d11_render_failed()
        {
            return "D3D11";
        }

        "CPU/QVideoSink"
    }

    pub fn fallback_reason(&self) -> String {
        if !self.hardware_decode_requested {
            return "Software backend selected".to_string();
        }
        if !self.gpu_decode_enabled() {
            return "No GPU decoder available".to_string();
        }
        if self.present_backend_label() == "CPU/QVideoSink" {
            if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA
                && Self::cuda_gl_requested()
                && crate::bridge::qobject::cuda_gl_render_failed()
            {
                return "CUDA-GL present failed; using CPU/QVideoSink".to_string();
            }
            if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI {
                if crate::bridge::qobject::dmabuf_render_failed() {
                    return "DMABUF present failed; using CPU/QVideoSink".to_string();
                }
                if !Self::dmabuf_gl_requested() {
                    return "DMABUF/OpenGL not enabled; using CPU/QVideoSink".to_string();
                }
            }
            if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
                && crate::bridge::qobject::d3d11_render_failed()
            {
                return "D3D11 direct present off (LUNARIS_D3D11_PRESENT=1) or failed; using CPU/QVideoSink".to_string();
            }
            return "GPU decode with CPU/QVideoSink present".to_string();
        }
        "None".to_string()
    }

    pub fn presentation_mode_label(&self) -> String {
        format!(
            "{} decode + {} present",
            self.decode_backend_label(),
            self.present_backend_label()
        )
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<DecodedFrame>, anyhow::Error> {
        let packet = unsafe { ffi::av_packet_alloc() };
        if packet.is_null() {
            return Err(anyhow::anyhow!("Failed to allocate FFmpeg packet"));
        }

        unsafe {
            let ret = ffi::av_new_packet(packet, data.len() as i32);
            if ret < 0 {
                ffi::av_packet_free(&mut (packet as *mut _));
                return Err(anyhow::anyhow!("av_new_packet failed: {}", ret));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), (*packet).data, data.len());
        }

        let send_ret = unsafe { ffi::avcodec_send_packet(self.codec_ctx, packet) };
        unsafe {
            ffi::av_packet_free(&mut (packet as *mut _));
        }

        if send_ret < 0 {
            return Err(anyhow::anyhow!("avcodec_send_packet failed: {}", send_ret));
        }

        let mut decoded_frames = Vec::new();

        loop {
            let gpu_frame = unsafe { ffi::av_frame_alloc() };
            if gpu_frame.is_null() {
                return Err(anyhow::anyhow!("Failed to allocate FFmpeg frame"));
            }

            let recv_ret = unsafe { ffi::avcodec_receive_frame(self.codec_ctx, gpu_frame) };

            if recv_ret == ffi::AVERROR(ffmpeg_next::util::error::EAGAIN)
                || recv_ret == ffi::AVERROR_EOF
            {
                unsafe {
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                }
                break;
            } else if recv_ret < 0 {
                unsafe {
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                }
                return Err(anyhow::anyhow!(
                    "avcodec_receive_frame failed: {}",
                    recv_ret
                ));
            }

            match self.process_frame(gpu_frame) {
                Ok(frame) => {
                    decoded_frames.push(frame);
                }
                Err(e) => {
                    eprintln!("Failed to process decoded frame: {:?}", e);
                }
            }
        }

        Ok(decoded_frames)
    }

    fn process_frame(
        &mut self,
        gpu_frame: *mut ffi::AVFrame,
    ) -> Result<DecodedFrame, anyhow::Error> {
        let format = unsafe { (*gpu_frame).format };
        // The renderers convert YCbCr with the stream's own matrix and range.
        let (bt709, full_range) =
            unsafe { frame_color((*gpu_frame).colorspace as i32, (*gpu_frame).color_range as i32, format) };
        if self.last_color != Some((bt709, full_range)) {
            self.last_color = Some((bt709, full_range));
            println!("Video colours: {} {} range", if bt709 { "BT.709" } else { "BT.601" }, if full_range { "full" } else { "studio" });
        }
        crate::bridge::qobject::set_video_color(bt709, full_range);
        let is_cuda = format == ffi::AVPixelFormat::AV_PIX_FMT_CUDA as i32;
        let cuda_gl_failed = crate::bridge::qobject::cuda_gl_render_failed();
        if cuda_gl_failed {
            clear_active_cuda_frame_for_decoder(self as *const _ as usize);
        }
        let use_direct_cuda_gl = is_cuda && Self::cuda_gl_requested() && !cuda_gl_failed;

        if use_direct_cuda_gl {
            unsafe {
                let y_ptr = (*gpu_frame).data[0] as u64;
                let uv_ptr = (*gpu_frame).data[1] as u64;
                let y_stride = (*gpu_frame).linesize[0] as i32;
                let uv_stride = (*gpu_frame).linesize[1] as i32;
                let width = (*gpu_frame).width as i32;
                let height = (*gpu_frame).height as i32;

                let mut cuda_ctx = 0u64;
                if !self.hw_device_ctx.is_null() {
                    let hw_device_ctx_ptr =
                        (*self.hw_device_ctx).data as *mut ffi::AVHWDeviceContext;
                    if !hw_device_ctx_ptr.is_null() {
                        let hwctx = (*hw_device_ctx_ptr).hwctx;
                        if !hwctx.is_null() {
                            cuda_ctx = *(hwctx as *mut *mut std::ffi::c_void) as u64;
                        }
                    }
                }

                let mut lock = ACTIVE_CUDA_FRAME.lock().unwrap();
                let decoder_ptr = self as *const _ as usize;
                if let Some(SendRawFrame {
                    frame: old_frame, ..
                }) = lock.replace(SendRawFrame {
                    frame: gpu_frame,
                    decoder_ptr,
                }) {
                    ffi::av_frame_free(&mut (old_frame as *mut _));
                }

                crate::bridge::qobject::deliver_cuda_frame(
                    cuda_ctx, y_ptr, y_stride, uv_ptr, uv_stride, width, height,
                );
            }

            return Ok(DecodedFrame::NativePresented);
        }

        if self.try_present_dmabuf(gpu_frame)? {
            return Ok(DecodedFrame::NativePresented);
        }

        if self.try_present_d3d11(gpu_frame)? {
            return Ok(DecodedFrame::NativePresented);
        }

        let mut cpu_frame = gpu_frame;
        let mut is_hw_frame = false;

        unsafe {
            if !(*gpu_frame).hw_frames_ctx.is_null() {
                let temp_frame = ffi::av_frame_alloc();
                if temp_frame.is_null() {
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                    return Err(anyhow::anyhow!(
                        "Failed to allocate temp frame for HW-to-CPU copy"
                    ));
                }

                let transfer_err = ffi::av_hwframe_transfer_data(temp_frame, gpu_frame, 0);
                if transfer_err >= 0 {
                    (*temp_frame).width = (*gpu_frame).width;
                    (*temp_frame).height = (*gpu_frame).height;
                    cpu_frame = temp_frame;
                    is_hw_frame = true;
                } else {
                    ffi::av_frame_free(&mut (temp_frame as *mut _));
                    if is_cuda {
                        ffi::av_frame_free(&mut (gpu_frame as *mut _));
                        return Err(anyhow::anyhow!(
                            "CUDA decoded frame transfer failed: {}",
                            transfer_err
                        ));
                    }
                }
            } else if is_cuda {
                ffi::av_frame_free(&mut (gpu_frame as *mut _));
                return Err(anyhow::anyhow!(
                    "CUDA decoded frame did not include hw_frames_ctx for CPU transfer"
                ));
            }
        }

        let width = unsafe { (*cpu_frame).width };
        let height = unsafe { (*cpu_frame).height };
        let format = unsafe { (*cpu_frame).format };

        if Self::is_yuv444(format) {
            let result = self.convert_to_bgrx(cpu_frame);
            unsafe {
                if is_hw_frame {
                    ffi::av_frame_free(&mut (cpu_frame as *mut _));
                }
                ffi::av_frame_free(&mut (gpu_frame as *mut _));
            }
            return result.map(DecodedFrame::CpuBgrx);
        }

        let y_size = (width * height) as usize;
        let u_size = ((width / 2) * (height / 2)) as usize;
        let v_size = u_size;

        let mut y_vec = vec![0u8; y_size];
        let mut u_vec = vec![0u8; u_size];
        let mut v_vec = vec![0u8; v_size];

        let is_yuv420p = format == ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32
            || format == ffi::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32;

        if is_yuv420p {
            unsafe {
                let src_stride = (*cpu_frame).linesize[0] as usize;
                if src_stride == width as usize {
                    std::ptr::copy_nonoverlapping((*cpu_frame).data[0], y_vec.as_mut_ptr(), y_size);
                } else {
                    let mut src_ptr = (*cpu_frame).data[0];
                    let mut dst_ptr = y_vec.as_mut_ptr();
                    for _ in 0..height {
                        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, width as usize);
                        src_ptr = src_ptr.add(src_stride);
                        dst_ptr = dst_ptr.add(width as usize);
                    }
                }

                let src_stride = (*cpu_frame).linesize[1] as usize;
                let uv_h = height / 2;
                let uv_w = width / 2;
                if src_stride == uv_w as usize {
                    std::ptr::copy_nonoverlapping((*cpu_frame).data[1], u_vec.as_mut_ptr(), u_size);
                } else {
                    let mut src_ptr = (*cpu_frame).data[1];
                    let mut dst_ptr = u_vec.as_mut_ptr();
                    for _ in 0..uv_h {
                        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, uv_w as usize);
                        src_ptr = src_ptr.add(src_stride);
                        dst_ptr = dst_ptr.add(uv_w as usize);
                    }
                }

                let src_stride = (*cpu_frame).linesize[2] as usize;
                if src_stride == uv_w as usize {
                    std::ptr::copy_nonoverlapping((*cpu_frame).data[2], v_vec.as_mut_ptr(), v_size);
                } else {
                    let mut src_ptr = (*cpu_frame).data[2];
                    let mut dst_ptr = v_vec.as_mut_ptr();
                    for _ in 0..uv_h {
                        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, uv_w as usize);
                        src_ptr = src_ptr.add(src_stride);
                        dst_ptr = dst_ptr.add(uv_w as usize);
                    }
                }
            }
        } else {
            if self.sws_ctx.is_null()
                || self.last_width != width
                || self.last_height != height
                || self.last_format != format
            {
                if !self.sws_ctx.is_null() {
                    unsafe {
                        ffi::sws_freeContext(self.sws_ctx);
                    }
                }

                self.sws_ctx = unsafe {
                    ffi::sws_getContext(
                        width,
                        height,
                        std::mem::transmute(format),
                        width,
                        height,
                        ffi::AVPixelFormat::AV_PIX_FMT_YUV420P,
                        SWS_BILINEAR,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    )
                };

                if self.sws_ctx.is_null() {
                    unsafe {
                        if is_hw_frame {
                            ffi::av_frame_free(&mut (cpu_frame as *mut _));
                        }
                        ffi::av_frame_free(&mut (gpu_frame as *mut _));
                    }
                    return Err(anyhow::anyhow!("Failed to initialize sws_scale context"));
                }

                self.last_width = width;
                self.last_height = height;
                self.last_format = format;
            }

            let dst_frame = unsafe { ffi::av_frame_alloc() };
            if dst_frame.is_null() {
                unsafe {
                    if is_hw_frame {
                        ffi::av_frame_free(&mut (cpu_frame as *mut _));
                    }
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                }
                return Err(anyhow::anyhow!(
                    "Failed to allocate output destination frame"
                ));
            }

            unsafe {
                (*dst_frame).width = width;
                (*dst_frame).height = height;
                (*dst_frame).format = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P as i32;

                let buffer_err = ffi::av_frame_get_buffer(dst_frame, 32);
                if buffer_err < 0 {
                    ffi::av_frame_free(&mut (dst_frame as *mut _));
                    if is_hw_frame {
                        ffi::av_frame_free(&mut (cpu_frame as *mut _));
                    }
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                    return Err(anyhow::anyhow!(
                        "av_frame_get_buffer failed: {}",
                        buffer_err
                    ));
                }

                ffi::sws_scale(
                    self.sws_ctx,
                    (*cpu_frame).data.as_ptr() as *const *const u8,
                    (*cpu_frame).linesize.as_ptr(),
                    0,
                    height,
                    (*dst_frame).data.as_mut_ptr() as *mut *mut u8,
                    (*dst_frame).linesize.as_mut_ptr(),
                );

                let src_stride = (*dst_frame).linesize[0] as usize;
                if src_stride == width as usize {
                    std::ptr::copy_nonoverlapping((*dst_frame).data[0], y_vec.as_mut_ptr(), y_size);
                } else {
                    let mut src_ptr = (*dst_frame).data[0];
                    let mut dst_ptr = y_vec.as_mut_ptr();
                    for _ in 0..height {
                        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, width as usize);
                        src_ptr = src_ptr.add(src_stride);
                        dst_ptr = dst_ptr.add(width as usize);
                    }
                }

                let src_stride = (*dst_frame).linesize[1] as usize;
                let uv_h = height / 2;
                let uv_w = width / 2;
                if src_stride == uv_w as usize {
                    std::ptr::copy_nonoverlapping((*dst_frame).data[1], u_vec.as_mut_ptr(), u_size);
                } else {
                    let mut src_ptr = (*dst_frame).data[1];
                    let mut dst_ptr = u_vec.as_mut_ptr();
                    for _ in 0..uv_h {
                        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, uv_w as usize);
                        src_ptr = src_ptr.add(src_stride);
                        dst_ptr = dst_ptr.add(uv_w as usize);
                    }
                }

                let src_stride = (*dst_frame).linesize[2] as usize;
                if src_stride == uv_w as usize {
                    std::ptr::copy_nonoverlapping((*dst_frame).data[2], v_vec.as_mut_ptr(), v_size);
                } else {
                    let mut src_ptr = (*dst_frame).data[2];
                    let mut dst_ptr = v_vec.as_mut_ptr();
                    for _ in 0..uv_h {
                        std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, uv_w as usize);
                        src_ptr = src_ptr.add(src_stride);
                        dst_ptr = dst_ptr.add(uv_w as usize);
                    }
                }

                ffi::av_frame_free(&mut (dst_frame as *mut _));
            }
        }

        unsafe {
            if is_hw_frame {
                ffi::av_frame_free(&mut (cpu_frame as *mut _));
            }
            ffi::av_frame_free(&mut (gpu_frame as *mut _));
        }

        Ok(DecodedFrame::CpuYuv(YUVFrame {
            width,
            height,
            y: y_vec,
            u: u_vec,
            v: v_vec,
            y_stride: width,
            u_stride: width / 2,
            v_stride: width / 2,
        }))
    }

    fn is_yuv444(format: i32) -> bool {
        format == ffi::AVPixelFormat::AV_PIX_FMT_YUV444P as i32
            || format == ffi::AVPixelFormat::AV_PIX_FMT_YUVJ444P as i32
            || format == ffi::AVPixelFormat::AV_PIX_FMT_NV24 as i32
    }

    /// Converts a 4:4:4 frame to packed BGRX with the frame's own matrix and
    /// range, keeping the full chroma resolution (the YUV420P path would
    /// throw away exactly what a 4:4:4 stream is for).
    fn convert_to_bgrx(&mut self, frame: *mut ffi::AVFrame) -> Result<BgrxFrame, anyhow::Error> {
        let (width, height, format, colorspace, range) = unsafe {
            (
                (*frame).width,
                (*frame).height,
                (*frame).format,
                (*frame).colorspace as i32,
                (*frame).color_range as i32,
            )
        };
        let key = (width, height, format, colorspace, range);
        if self.bgrx_sws_ctx.is_null() || self.bgrx_key != key {
            unsafe {
                if !self.bgrx_sws_ctx.is_null() {
                    ffi::sws_freeContext(self.bgrx_sws_ctx);
                }
                self.bgrx_sws_ctx = ffi::sws_getContext(
                    width,
                    height,
                    std::mem::transmute(format),
                    width,
                    height,
                    ffi::AVPixelFormat::AV_PIX_FMT_BGR0,
                    SWS_BILINEAR,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                if self.bgrx_sws_ctx.is_null() {
                    return Err(anyhow::anyhow!("Failed to create the 4:4:4 -> BGRX converter"));
                }
                // Unspecified colourspace: the agent's encoders tag BT.709.
                let matrix = match colorspace {
                    c if c == ffi::AVColorSpace::AVCOL_SPC_BT470BG as i32
                        || c == ffi::AVColorSpace::AVCOL_SPC_SMPTE170M as i32 => ffi::SWS_CS_ITU601,
                    _ => ffi::SWS_CS_ITU709,
                } as i32;
                let full_range = range == ffi::AVColorRange::AVCOL_RANGE_JPEG as i32
                    || format == ffi::AVPixelFormat::AV_PIX_FMT_YUVJ444P as i32;
                let coefficients = ffi::sws_getCoefficients(matrix);
                ffi::sws_setColorspaceDetails(
                    self.bgrx_sws_ctx,
                    coefficients,
                    full_range as i32,
                    coefficients,
                    1,
                    0,
                    1 << 16,
                    1 << 16,
                );
            }
            self.bgrx_key = key;
        }

        let stride = width * 4;
        let mut data = vec![0u8; (stride * height) as usize];
        unsafe {
            let dst_planes: [*mut u8; 4] = [data.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()];
            let dst_strides: [i32; 4] = [stride, 0, 0, 0];
            ffi::sws_scale(
                self.bgrx_sws_ctx,
                (*frame).data.as_ptr() as *const *const u8,
                (*frame).linesize.as_ptr(),
                0,
                height,
                dst_planes.as_ptr(),
                dst_strides.as_ptr(),
            );
        }
        Ok(BgrxFrame { width, height, data, stride })
    }

    #[cfg(target_os = "linux")]
    fn try_present_dmabuf(&mut self, gpu_frame: *mut ffi::AVFrame) -> Result<bool, anyhow::Error> {
        if self.hw_device_type != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
            || !Self::dmabuf_gl_requested()
            || crate::bridge::qobject::dmabuf_render_failed()
        {
            if self.hw_device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI && !self.dmabuf_layout_logged {
                self.dmabuf_layout_logged = true;
                println!(
                    "VAAPI frames go through the CPU: DMA-BUF present requested={} failed={}",
                    Self::dmabuf_gl_requested(),
                    crate::bridge::qobject::dmabuf_render_failed()
                );
            }
            return Ok(false);
        }

        unsafe {
            let mapped_frame = ffi::av_frame_alloc();
            if mapped_frame.is_null() {
                return Ok(false);
            }

            // Without a destination format FFmpeg maps the surface into
            // ordinary memory (NV12); ask for DMA-BUF handles instead.
            (*mapped_frame).format = ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
            let map_err =
                ffi::av_hwframe_map(mapped_frame, gpu_frame, ffi::AV_HWFRAME_MAP_READ as i32);
            if map_err < 0 {
                if !self.dmabuf_layout_logged {
                    self.dmabuf_layout_logged = true;
                    println!("VAAPI frames can't be mapped as DMA-BUFs ({}); copying through the CPU", map_err);
                }
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            if (*mapped_frame).format != ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32
                || (*mapped_frame).data[0].is_null()
            {
                if !self.dmabuf_layout_logged {
                    self.dmabuf_layout_logged = true;
                    println!("VAAPI frames go through the CPU: mapped format {} is not DRM PRIME", (*mapped_frame).format);
                }
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            let desc = (*mapped_frame).data[0] as *const ffi::AVDRMFrameDescriptor;
            if desc.is_null() || (*desc).nb_layers < 1 || (*desc).nb_objects < 1 {
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            // NV12 comes as one layer with two planes, or (Mesa, which
            // exports separate layers) as an R8 layer for Y and a GR88 layer
            // for UV. Either way the renderer imports each plane on its own.
            let layer = (*desc).layers[0];
            let (plane0, plane1) = if layer.format == DRM_FORMAT_NV12 && layer.nb_planes >= 2 {
                (layer.planes[0], layer.planes[1])
            } else if (*desc).nb_layers >= 2
                && layer.format == DRM_FORMAT_R8
                && layer.nb_planes >= 1
                && (*desc).layers[1].format == DRM_FORMAT_GR88
                && (*desc).layers[1].nb_planes >= 1
            {
                (layer.planes[0], (*desc).layers[1].planes[0])
            } else {
                if !self.dmabuf_layout_logged {
                    self.dmabuf_layout_logged = true;
                    println!(
                        "VAAPI frames can't be shown as DMA-BUFs (layers={}, first format={:#x}, planes={}); copying through the CPU",
                        (*desc).nb_layers,
                        layer.format,
                        layer.nb_planes
                    );
                }
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            };
            if plane0.object_index < 0
                || plane1.object_index < 0
                || plane0.object_index as usize >= (*desc).objects.len()
                || plane1.object_index as usize >= (*desc).objects.len()
                || plane0.object_index >= (*desc).nb_objects
                || plane1.object_index >= (*desc).nb_objects
            {
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            let object0 = (*desc).objects[plane0.object_index as usize];
            let object1 = (*desc).objects[plane1.object_index as usize];
            let fd0 = dup(object0.fd);
            let fd1 = dup(object1.fd);
            if fd0 < 0 || fd1 < 0 {
                if fd0 >= 0 {
                    close(fd0);
                }
                if fd1 >= 0 {
                    close(fd1);
                }
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            let modifier = object0.format_modifier;
            let delivered = crate::bridge::qobject::deliver_dmabuf_frame(
                fd0,
                fd1,
                DRM_FORMAT_NV12,
                modifier,
                plane0.offset as i32,
                plane0.pitch as i32,
                plane1.offset as i32,
                plane1.pitch as i32,
                (*gpu_frame).width as i32,
                (*gpu_frame).height as i32,
            );
            ffi::av_frame_free(&mut (mapped_frame as *mut _));
            if delivered {
                ffi::av_frame_free(&mut (gpu_frame as *mut _));
                return Ok(true);
            }
        }

        Ok(false)
    }

    #[cfg(not(target_os = "linux"))]
    fn try_present_dmabuf(&mut self, _gpu_frame: *mut ffi::AVFrame) -> Result<bool, anyhow::Error> {
        Ok(false)
    }

    #[cfg(target_os = "windows")]
    fn try_present_d3d11(&mut self, gpu_frame: *mut ffi::AVFrame) -> Result<bool, anyhow::Error> {
        if self.hw_device_type != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
            || crate::bridge::qobject::d3d11_render_failed()
        {
            return Ok(false);
        }

        unsafe {
            let format = (*gpu_frame).format;
            if format != ffi::AVPixelFormat::AV_PIX_FMT_D3D11 as i32
                && format != ffi::AVPixelFormat::AV_PIX_FMT_D3D11VA_VLD as i32
            {
                return Ok(false);
            }
            let texture_ptr = (*gpu_frame).data[0] as u64;
            if texture_ptr == 0 {
                return Ok(false);
            }
            let array_index = (*gpu_frame).data[1] as isize as i32;
            let delivered = crate::bridge::qobject::deliver_d3d11_frame(
                texture_ptr,
                array_index,
                (*gpu_frame).width as i32,
                (*gpu_frame).height as i32,
                format as u32,
            );
            if delivered {
                ffi::av_frame_free(&mut (gpu_frame as *mut _));
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(not(target_os = "windows"))]
    fn try_present_d3d11(&mut self, _gpu_frame: *mut ffi::AVFrame) -> Result<bool, anyhow::Error> {
        Ok(false)
    }
}

impl Drop for HardwareDecoder {
    fn drop(&mut self) {
        let decoder_ptr = self as *const _ as usize;
        clear_active_cuda_frame_for_decoder(decoder_ptr);
        unsafe {
            if !self.codec_ctx.is_null() {
                ffi::avcodec_free_context(&mut self.codec_ctx);
            }
            if !self.hw_device_ctx.is_null() {
                ffi::av_buffer_unref(&mut self.hw_device_ctx);
            }
            if !self.sws_ctx.is_null() {
                ffi::sws_freeContext(self.sws_ctx);
            }
            if !self.bgrx_sws_ctx.is_null() {
                ffi::sws_freeContext(self.bgrx_sws_ctx);
            }
        }
    }
}

#[cfg(test)]
mod color_tests {
    use super::{ffi, frame_color};

    #[test]
    fn tags_choose_matrix_and_range() {
        let nv12 = ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
        let unspec = ffi::AVColorSpace::AVCOL_SPC_UNSPECIFIED as i32;
        let mpeg = ffi::AVColorRange::AVCOL_RANGE_MPEG as i32;
        let jpeg = ffi::AVColorRange::AVCOL_RANGE_JPEG as i32;
        let no_range = ffi::AVColorRange::AVCOL_RANGE_UNSPECIFIED as i32;
        // Untagged: BT.709 studio.
        assert_eq!(frame_color(unspec, no_range, nv12), (true, false));
        assert_eq!(frame_color(ffi::AVColorSpace::AVCOL_SPC_BT709 as i32, jpeg, nv12), (true, true));
        assert_eq!(frame_color(ffi::AVColorSpace::AVCOL_SPC_SMPTE170M as i32, mpeg, nv12), (false, false));
        assert_eq!(frame_color(unspec, no_range, ffi::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32), (true, true));
    }
}
