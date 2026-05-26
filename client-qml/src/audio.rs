use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;
use tracing::{info, warn};

pub fn setup_audio() -> Option<mpsc::UnboundedSender<Vec<f32>>> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let config = device.default_output_config().ok()?;
    
    let config_channels = config.channels();
    let config_sample_rate = config.sample_rate().0;
    info!("Initializing CPAL audio output device: {} channels, {} Hz", config_channels, config_sample_rate);
    
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let mut audio_buffer = Vec::<f32>::new();
    
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Drain incoming audio buffers
            while let Ok(mut samples) = rx.try_recv() {
                audio_buffer.append(&mut samples);
            }
            
            let target_channels = config_channels as usize;
            let target_sample_rate = config_sample_rate as f64;
            let ratio = 48000.0 / target_sample_rate;
            let output_frames = data.len() / target_channels;
            
            let available_input_frames = audio_buffer.len() / 2;
            
            let mut source_frame_ptr = 0.0f64;
            for i in 0..output_frames {
                let idx = source_frame_ptr as usize;
                
                // If we ran out of input frames, pad the remaining output channels with silence (0.0)
                if idx >= available_input_frames {
                    let out_idx = i * target_channels;
                    for c in 0..target_channels {
                        data[out_idx + c] = 0.0;
                    }
                    continue;
                }
                
                let fract = (source_frame_ptr - idx as f64) as f32;
                
                let l1 = audio_buffer[idx * 2];
                let r1 = audio_buffer[idx * 2 + 1];
                
                // Boundary check for linear interpolation
                let (l2, r2) = if idx + 1 < available_input_frames {
                    (audio_buffer[(idx + 1) * 2], audio_buffer[(idx + 1) * 2 + 1])
                } else {
                    (l1, r1)
                };
                
                let left = l1 + (l2 - l1) * fract;
                let right = r1 + (r2 - r1) * fract;
                
                let out_idx = i * target_channels;
                if target_channels == 1 {
                    data[out_idx] = (left + right) * 0.5;
                } else if target_channels == 2 {
                    data[out_idx] = left;
                    data[out_idx + 1] = right;
                } else {
                    data[out_idx] = left;
                    data[out_idx + 1] = right;
                    for c in 2..target_channels {
                        data[out_idx + c] = 0.0;
                    }
                }
                
                source_frame_ptr += ratio;
            }
            
            let consumed_frames = source_frame_ptr.floor() as usize;
            if consumed_frames * 2 <= audio_buffer.len() {
                audio_buffer.drain(..consumed_frames * 2);
            } else {
                audio_buffer.clear();
            }
        },
        |err| warn!("an error occurred on cpal stream: {}", err),
        None
    ).ok()?;

    if stream.play().is_ok() {
        Box::leak(Box::new(stream));
        Some(tx)
    } else {
        None
    }
}
