//! Synthetic frame generator — the Stage-1 stand-in for the smithay
//! compositor. Produces XRGB8888 frames at the configured rate: scrolling
//! color bars, a bouncing box, and a binary frame counter, so the output is
//! visibly alive and verifiable by eye.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::{ArgbFrame, WaylandUrlConfig};

/// Spawn the generator task. Frames go to `frame_tx`; when the encoder side
/// falls behind, fresh frames are dropped instead of queueing stale video.
/// The task exits when `cancel` is cancelled (or `frame_tx` closes).
pub fn spawn(
    config: &WaylandUrlConfig,
    frame_tx: mpsc::Sender<ArgbFrame>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let mut pattern = Pattern::new(config.width, config.height);
    let period = Duration::from_secs_f64(1.0 / f64::from(config.fps));
    let mut ticker = tokio::time::interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    tokio::spawn(async move {
        let origin = Instant::now();
        let mut dropped = 0u64;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    let frame = pattern.render(origin.elapsed().as_micros() as u64);
                    if frame_tx.try_send(frame).is_err() {
                        dropped += 1;
                        if dropped == 1 || dropped.is_multiple_of(100) {
                            tracing::debug!("wayland pattern: {dropped} frames dropped (encoder busy)");
                        }
                    }
                }
            }
        }
    })
}

struct Pattern {
    width: u32,
    height: u32,
    frame: u64,
    box_x: i32,
    box_y: i32,
    box_dx: i32,
    box_dy: i32,
}

impl Pattern {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame: 0,
            box_x: 0,
            box_y: 0,
            box_dx: 3,
            box_dy: 2,
        }
    }

    fn render(&mut self, pts_us: u64) -> ArgbFrame {
        let (width, height) = (self.width as usize, self.height as usize);
        let stride = width * 4;
        let mut data = vec![0u8; stride * height];

        // Scrolling 75% color bars (white, yellow, cyan, green, magenta,
        // red, blue, black), shifting one pixel left per frame.
        const BARS: [(u8, u8, u8); 8] = [
            (191, 191, 191),
            (191, 191, 0),
            (0, 191, 191),
            (0, 191, 0),
            (191, 0, 191),
            (191, 0, 0),
            (0, 0, 191),
            (0, 0, 0),
        ];
        let bar_w = width / 8;
        let phase = (self.frame as usize) % width;
        for row in data.chunks_exact_mut(stride) {
            for (x, px) in row.chunks_exact_mut(4).enumerate() {
                let (r, g, b) = BARS[((x + phase) / bar_w) % 8];
                px[0] = b;
                px[1] = g;
                px[2] = r;
                px[3] = 0xff;
            }
        }

        // Bouncing box.
        let box_w = (width / 8) as i32;
        let box_h = (height / 8) as i32;
        self.box_x += self.box_dx;
        self.box_y += self.box_dy;
        if self.box_x <= 0 || self.box_x + box_w >= width as i32 {
            self.box_dx = -self.box_dx;
            self.box_x = self.box_x.clamp(0, width as i32 - box_w);
        }
        if self.box_y <= 0 || self.box_y + box_h >= height as i32 {
            self.box_dy = -self.box_dy;
            self.box_y = self.box_y.clamp(0, height as i32 - box_h);
        }
        for y in self.box_y..self.box_y + box_h {
            let row = &mut data[y as usize * stride..(y as usize + 1) * stride];
            for x in self.box_x..self.box_x + box_w {
                let px = &mut row[x as usize * 4..x as usize * 4 + 4];
                px[0] = 0xff;
                px[1] = 0xff;
                px[2] = 0xff;
                px[3] = 0xff;
            }
        }

        // Binary frame counter strip at the bottom: 32 cells, one bit each.
        let cell_w = width / 32;
        let cell_h = (height / 24).max(1);
        for bit in 0..32 {
            let on = (self.frame >> bit) & 1 == 1;
            if !on {
                continue;
            }
            let x0 = bit * cell_w;
            for y in height - cell_h..height {
                let row = &mut data[y * stride..(y + 1) * stride];
                for x in x0..x0 + cell_w {
                    let px = &mut row[x * 4..x * 4 + 4];
                    px[0] = 0xff;
                    px[1] = 0xff;
                    px[2] = 0xff;
                    px[3] = 0xff;
                }
            }
        }

        self.frame += 1;
        ArgbFrame {
            width: self.width,
            height: self.height,
            stride: stride as u32,
            data,
            pts_us,
        }
    }
}
