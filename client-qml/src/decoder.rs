use ffmpeg_next::sys as ffi;

// SWS_BILINEAR is 2 in FFmpeg. We define it locally because on some platforms/FFmpeg versions
// bindgen generates it as a global constant, while on others it is an enum variant (SwsFlags::SWS_BILINEAR).
const SWS_BILINEAR: i32 = 2;
const DRM_FORMAT_NV12: u32 = 0x3231_564e;

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
    NativePresented,
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

        let mut candidates = Vec::new();
        if hardware_decode_requested {
            let cuda_disabled = std::env::var("LUNARIS_DISABLE_CUDA").is_ok();
            let cuda_gl_requested = std::env::var("LUNARIS_CLIENT_CUDA_GL")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

            if cfg!(target_os = "linux") {
                let prefer_cuda_decode = !cuda_disabled
                    && (cuda_gl_requested || Self::linux_nvidia_cuda_present());
                if prefer_cuda_decode {
                    candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA);
                }
                candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI);
                candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VDPAU);
                if !prefer_cuda_decode && !cuda_disabled {
                    candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA);
                } else if cuda_disabled {
                    println!("CUDA hardware decoding is disabled.");
                }
            } else if cfg!(target_os = "windows") {
                if cuda_gl_requested && !cuda_disabled {
                    candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA);
                }
                candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
                candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2);
                if !cuda_gl_requested && !cuda_disabled {
                    candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA);
                }
            } else if cfg!(target_os = "macos") {
                candidates.push(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX);
            }
        } else {
            println!("All hardware decoding is disabled (Software/FFmpeg mode).");
        }

        for &dev_type in &candidates {
            let err = unsafe {
                ffi::av_hwdevice_ctx_create(
                    &mut hw_device_ctx,
                    dev_type,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            if err >= 0 && !hw_device_ctx.is_null() {
                hw_device_type = dev_type;
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
            println!("Failed to create GPU hardware decoding context. Falling back to software decoding.");
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

    fn cuda_gl_requested() -> bool {
        std::env::var("LUNARIS_CLIENT_CUDA_GL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn dmabuf_gl_requested() -> bool {
        std::env::var("LUNARIS_CLIENT_DMABUF_GL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
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
                return "D3D11 present failed; using CPU/QVideoSink".to_string();
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

    #[cfg(target_os = "linux")]
    fn try_present_dmabuf(&mut self, gpu_frame: *mut ffi::AVFrame) -> Result<bool, anyhow::Error> {
        if self.hw_device_type != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
            || !Self::dmabuf_gl_requested()
            || crate::bridge::qobject::dmabuf_render_failed()
        {
            return Ok(false);
        }

        unsafe {
            let mapped_frame = ffi::av_frame_alloc();
            if mapped_frame.is_null() {
                return Ok(false);
            }

            let map_err =
                ffi::av_hwframe_map(mapped_frame, gpu_frame, ffi::AV_HWFRAME_MAP_READ as i32);
            if map_err < 0 {
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            if (*mapped_frame).format != ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32
                || (*mapped_frame).data[0].is_null()
            {
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            let desc = (*mapped_frame).data[0] as *const ffi::AVDRMFrameDescriptor;
            if desc.is_null() || (*desc).nb_layers < 1 || (*desc).nb_objects < 1 {
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            let layer = (*desc).layers[0];
            if layer.format != DRM_FORMAT_NV12 || layer.nb_planes < 2 {
                ffi::av_frame_free(&mut (mapped_frame as *mut _));
                return Ok(false);
            }

            let plane0 = layer.planes[0];
            let plane1 = layer.planes[1];
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
                layer.format,
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
        }
    }
}
