use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use rsmpeg::{
    avcodec::{AVCodec, AVCodecContext},
    avformat::AVFormatContextOutput,
    avutil::{AVFrame, AVRational},
    ffi,
};

use super::{Source, SourceHandle};

/// VP8 RTP source implemented directly with FFmpeg via `rsmpeg`.
#[derive(Debug, Clone, Copy)]
pub struct RsmpegVp8Source {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for RsmpegVp8Source {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            fps: 30,
        }
    }
}

impl Source for RsmpegVp8Source {
    fn name(&self) -> &'static str {
        "rsmpeg-vp8"
    }

    fn start(&self, target_addr: SocketAddr) -> Result<Box<dyn SourceHandle>> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let url = format!("rtp://{target_addr}");
        let url_c =
            std::ffi::CString::new(url.clone()).context("Failed to build RTP URL CString")?;

        let width = self.width;
        let height = self.height;
        let fps = self.fps;

        thread::spawn(move || {
            if let Err(e) = run_rtp_stream(url_c.as_c_str(), width, height, fps, stop_clone.clone())
            {
                eprintln!("rsmpeg VP8 RTP stream error: {e:?}");
            }
            stop_clone.store(true, Ordering::Relaxed);
        });

        Ok(Box::new(RsmpegHandle { stop }))
    }

    fn sdp(&self, listen_addr: SocketAddr) -> String {
        format!(
            "v=0\r\n\
             o=- 0 0 IN IP4 127.0.0.1\r\n\
             s=rsmpeg VP8 test stream\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=video {} RTP/AVP 96\r\n\
             a=rtpmap:96 VP8/90000\r\n",
            listen_addr.port()
        )
    }
}

struct RsmpegHandle {
    stop: Arc<AtomicBool>,
}

impl SourceHandle for RsmpegHandle {
    fn stop(self: Box<Self>) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run_rtp_stream(
    url: &std::ffi::CStr,
    width: u32,
    height: u32,
    fps: u32,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let width = width as i32;
    let height = height as i32;
    let fps = fps as i32;

    let mut output = AVFormatContextOutput::builder()
        .filename(url)
        .format_name(c"rtp")
        .build()
        .context("Failed to create RTP output context")?;

    let codec = AVCodec::find_encoder(ffi::AV_CODEC_ID_VP8)
        .ok_or_else(|| anyhow!("VP8 encoder not found"))?;

    let mut stream = output.new_stream();
    let stream_index = stream.index;

    let mut codec_context = AVCodecContext::new(&codec);
    codec_context.set_width(width);
    codec_context.set_height(height);
    codec_context.set_time_base(AVRational { num: 1, den: fps });
    codec_context.set_framerate(AVRational { num: fps, den: 1 });
    codec_context.set_pix_fmt(ffi::AV_PIX_FMT_YUV420P);
    codec_context.set_bit_rate(1_000_000);
    codec_context.set_gop_size(fps);
    codec_context.set_max_b_frames(0);

    codec_context
        .open(None)
        .context("Failed to open VP8 encoder")?;

    let codecpar = codec_context.extract_codecpar();
    stream.set_codecpar(codecpar);
    stream.set_time_base(codec_context.time_base);
    let stream_time_base = stream.time_base;

    drop(stream);

    let mut options = None;
    output
        .write_header(&mut options)
        .context("Failed to write RTP header")?;

    let frame_duration = Duration::from_secs(1) / fps as u32;
    let start = Instant::now();
    let mut frame_count: i64 = 0;

    while !stop.load(Ordering::Relaxed) {
        let mut frame = AVFrame::new();
        frame.set_width(width);
        frame.set_height(height);
        frame.set_format(ffi::AV_PIX_FMT_YUV420P);
        frame.set_pts(frame_count);
        frame
            .alloc_buffer()
            .context("Failed to allocate frame buffer")?;
        frame
            .make_writable()
            .context("Failed to make frame writable")?;

        fill_test_pattern(&mut frame, frame_count as f32 / fps as f32);

        codec_context
            .send_frame(Some(&frame))
            .context("Failed to send frame to encoder")?;

        loop {
            match codec_context.receive_packet() {
                Ok(mut packet) => {
                    packet.set_stream_index(stream_index);
                    packet.rescale_ts(codec_context.time_base, stream_time_base);
                    output
                        .interleaved_write_frame(&mut packet)
                        .context("Failed to write RTP packet")?;
                }
                Err(rsmpeg::error::RsmpegError::EncoderDrainError) => break,
                Err(e) => return Err(e.into()),
            }
        }

        frame_count += 1;

        let expected = start + frame_duration * frame_count as u32;
        let now = Instant::now();
        if expected > now {
            thread::sleep(expected - now);
        }
    }

    codec_context
        .send_frame(None)
        .context("Failed to flush encoder")?;
    loop {
        match codec_context.receive_packet() {
            Ok(mut packet) => {
                packet.set_stream_index(stream_index);
                packet.rescale_ts(codec_context.time_base, stream_time_base);
                output
                    .interleaved_write_frame(&mut packet)
                    .context("Failed to write flush packet")?;
            }
            Err(rsmpeg::error::RsmpegError::EncoderDrainError) => break,
            Err(e) => return Err(e.into()),
        }
    }

    output
        .write_trailer()
        .context("Failed to write RTP trailer")?;

    Ok(())
}

fn fill_test_pattern(frame: &mut AVFrame, t: f32) {
    let width = frame.width as usize;
    let height = frame.height as usize;

    let frame_ptr = frame.as_mut_ptr();
    let data = unsafe { (*frame_ptr).data };
    let linesize = unsafe { (*frame_ptr).linesize };

    let y_stride = linesize[0] as usize;
    let u_stride = linesize[1] as usize;
    let v_stride = linesize[2] as usize;

    let y_plane = unsafe { std::slice::from_raw_parts_mut(data[0], y_stride * height) };
    let u_plane = unsafe { std::slice::from_raw_parts_mut(data[1], u_stride * (height / 2)) };
    let v_plane = unsafe { std::slice::from_raw_parts_mut(data[2], v_stride * (height / 2)) };

    let bar_x = ((t * 60.0) as usize) % width;

    for y in 0..height {
        for x in 0..width {
            let dx = (x as i32 - bar_x as i32).abs();
            let in_bar = dx < 40;

            let bg = ((x * 255) / width) as u8;
            let hue = (t * 30.0 + (x as f32 / width as f32) * 360.0) % 360.0;
            let (r, g, b) = if in_bar {
                hsv_to_rgb(hue, 0.8, 0.9)
            } else {
                (bg, bg, bg)
            };

            let (yy, uu, vv) = rgb_to_yuv(r, g, b);
            y_plane[y * y_stride + x] = yy;

            if y % 2 == 0 && x % 2 == 0 {
                u_plane[(y / 2) * u_stride + (x / 2)] = uu;
                v_plane[(y / 2) * v_stride + (x / 2)] = vv;
            }
        }
    }
}

fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r = r as f32;
    let g = g as f32;
    let b = b as f32;

    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = -0.169 * r - 0.331 * g + 0.5 * b + 128.0;
    let v = 0.5 * r - 0.419 * g - 0.081 * b + 128.0;

    (
        y.clamp(0.0, 255.0) as u8,
        u.clamp(0.0, 255.0) as u8,
        v.clamp(0.0, 255.0) as u8,
    )
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}
