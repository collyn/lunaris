use ffmpeg_next::sys as ffi;

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

pub struct HardwareDecoder {
    codec_ctx: *mut ffi::AVCodecContext,
    hw_device_ctx: *mut ffi::AVBufferRef,
    sws_ctx: *mut ffi::SwsContext,
    last_width: i32,
    last_height: i32,
    last_format: i32,
}

unsafe impl Send for HardwareDecoder {}

impl HardwareDecoder {
    pub fn new(codec_type: CodecType) -> Result<Self, anyhow::Error> {
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
            return Err(anyhow::anyhow!("Failed to find FFmpeg decoder for codec {:?}", codec_id));
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
        let hw_device_type = if cfg!(target_os = "linux") {
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
        } else if cfg!(target_os = "windows") {
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
        } else if cfg!(target_os = "macos") {
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX
        } else {
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE
        };

        if hw_device_type != ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            #[allow(unused_mut)]
            let mut err = unsafe {
                ffi::av_hwdevice_ctx_create(
                    &mut hw_device_ctx,
                    hw_device_type,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                )
            };

            #[cfg(target_os = "windows")]
            if err < 0 {
                err = unsafe {
                    ffi::av_hwdevice_ctx_create(
                        &mut hw_device_ctx,
                        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        0,
                    )
                };
            }

            if err >= 0 && !hw_device_ctx.is_null() {
                unsafe {
                    (*codec_ctx).hw_device_ctx = ffi::av_buffer_ref(hw_device_ctx);
                    let name = std::ffi::CStr::from_ptr(ffi::av_hwdevice_get_type_name(hw_device_type))
                        .to_string_lossy();
                    println!("Successfully initialized GPU hardware decoding context: {}", name);
                }
            } else {
                println!("Failed to create GPU hardware decoding context (err code: {}). Falling back to software decoding.", err);
            }
        } else {
            println!("No supported GPU hardware decoding API found for this platform. Falling back to software decoding.");
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
            sws_ctx: std::ptr::null_mut(),
            last_width: 0,
            last_height: 0,
            last_format: -1,
        })
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<YUVFrame>, anyhow::Error> {
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

            if recv_ret == ffi::AVERROR(ffmpeg_next::util::error::EAGAIN) || recv_ret == ffi::AVERROR_EOF {
                unsafe {
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                }
                break;
            } else if recv_ret < 0 {
                unsafe {
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                }
                return Err(anyhow::anyhow!("avcodec_receive_frame failed: {}", recv_ret));
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

    fn process_frame(&mut self, gpu_frame: *mut ffi::AVFrame) -> Result<YUVFrame, anyhow::Error> {
        let mut cpu_frame = gpu_frame;
        let mut is_hw_frame = false;

        unsafe {
            if !(*gpu_frame).hw_frames_ctx.is_null() {
                let temp_frame = ffi::av_frame_alloc();
                if temp_frame.is_null() {
                    ffi::av_frame_free(&mut (gpu_frame as *mut _));
                    return Err(anyhow::anyhow!("Failed to allocate temp frame for HW-to-CPU copy"));
                }

                let transfer_err = ffi::av_hwframe_transfer_data(temp_frame, gpu_frame, 0);
                if transfer_err >= 0 {
                    (*temp_frame).width = (*gpu_frame).width;
                    (*temp_frame).height = (*gpu_frame).height;
                    cpu_frame = temp_frame;
                    is_hw_frame = true;
                } else {
                    ffi::av_frame_free(&mut (temp_frame as *mut _));
                }
            }
        }

        let width = unsafe { (*cpu_frame).width };
        let height = unsafe { (*cpu_frame).height };
        let format = unsafe { (*cpu_frame).format };

        if self.sws_ctx.is_null() || self.last_width != width || self.last_height != height || self.last_format != format {
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
                    ffi::SWS_BILINEAR,
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
            return Err(anyhow::anyhow!("Failed to allocate output destination frame"));
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
                return Err(anyhow::anyhow!("av_frame_get_buffer failed: {}", buffer_err));
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
        }

        let y_size = (width * height) as usize;
        let u_size = ((width / 2) * (height / 2)) as usize;
        let v_size = u_size;

        let mut y_vec = vec![0u8; y_size];
        let mut u_vec = vec![0u8; u_size];
        let mut v_vec = vec![0u8; v_size];

        unsafe {
            let mut src_ptr = (*dst_frame).data[0];
            let src_stride = (*dst_frame).linesize[0] as usize;
            let mut dst_ptr = y_vec.as_mut_ptr();
            for _ in 0..height {
                std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, width as usize);
                src_ptr = src_ptr.add(src_stride);
                dst_ptr = dst_ptr.add(width as usize);
            }

            let mut src_ptr = (*dst_frame).data[1];
            let src_stride = (*dst_frame).linesize[1] as usize;
            let mut dst_ptr = u_vec.as_mut_ptr();
            let uv_h = height / 2;
            let uv_w = width / 2;
            for _ in 0..uv_h {
                std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, uv_w as usize);
                src_ptr = src_ptr.add(src_stride);
                dst_ptr = dst_ptr.add(uv_w as usize);
            }

            let mut src_ptr = (*dst_frame).data[2];
            let src_stride = (*dst_frame).linesize[2] as usize;
            let mut dst_ptr = v_vec.as_mut_ptr();
            for _ in 0..uv_h {
                std::ptr::copy_nonoverlapping(src_ptr, dst_ptr, uv_w as usize);
                src_ptr = src_ptr.add(src_stride);
                dst_ptr = dst_ptr.add(uv_w as usize);
            }
        }

        unsafe {
            ffi::av_frame_free(&mut (dst_frame as *mut _));
            if is_hw_frame {
                ffi::av_frame_free(&mut (cpu_frame as *mut _));
            }
            ffi::av_frame_free(&mut (gpu_frame as *mut _));
        }

        Ok(YUVFrame {
            width,
            height,
            y: y_vec,
            u: u_vec,
            v: v_vec,
            y_stride: width,
            u_stride: width / 2,
            v_stride: width / 2,
        })
    }
}

impl Drop for HardwareDecoder {
    fn drop(&mut self) {
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
