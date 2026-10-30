//! Video output: each core frame is converted to RGBA8 on the emulation thread (optionally with the
//! lines doubled to draw CRT scanlines) and the render thread copies the newest one into a texture.

use crate::retro::PixelFormat;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
struct Front {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

pub struct FrameBuffer {
    back: Mutex<Vec<u8>>,
    front: Mutex<Front>,
    sequence: AtomicU64,
    /// (scanlines on, brightness 0..=256 of the in-between rows)
    scanlines: Mutex<(bool, u32)>,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        FrameBuffer {
            back: Mutex::new(Vec::new()),
            front: Mutex::new(Front::default()),
            sequence: AtomicU64::new(0),
            scanlines: Mutex::new((false, 150)),
        }
    }
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// CRT scanlines belong on the TV; the handhelds have LCDs. `darkness` 0 = none, 1 = black gaps.
    pub fn set_scanlines(&self, enabled: bool, darkness: f32) {
        let level = (256.0 * (1.0 - darkness)).round().clamp(0.0, 256.0) as u32;
        *self.scanlines.lock().unwrap() = (enabled, level);
    }

    /// Increases with every submitted frame.
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// Emulation thread, from the core's video callback. `src` holds `height` rows of `pitch` bytes.
    pub fn submit(&self, src: &[u8], width: u32, height: u32, pitch: usize, format: PixelFormat) {
        let (scanlines, dim) = *self.scanlines.lock().unwrap();
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 {
            return;
        }
        let double = scanlines && h <= 300;
        let out_h = if double { h * 2 } else { h };
        let mut back = std::mem::take(&mut *self.back.lock().unwrap());
        back.resize(w * out_h * 4, 0);

        let bpp = if format == PixelFormat::Xrgb8888 {
            4
        } else {
            2
        };
        for y in 0..h {
            let row_start = y * pitch;
            let Some(row) = src.get(row_start..row_start + w * bpp) else {
                break;
            };
            let o = (if double { y * 2 } else { y }) * w * 4;
            {
                let dst = &mut back[o..o + w * 4];
                match format {
                    PixelFormat::Xrgb8888 => {
                        for (px, d) in row.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                            // memory order B, G, R, X -> R, G, B, A
                            d.copy_from_slice(&[px[2], px[1], px[0], 255]);
                        }
                    }
                    PixelFormat::Rgb565 => {
                        for (px, d) in row.chunks_exact(2).zip(dst.chunks_exact_mut(4)) {
                            let p = u16::from_le_bytes([px[0], px[1]]) as u32;
                            let (r, g, b) = ((p >> 11) & 0x1F, (p >> 5) & 0x3F, p & 0x1F);
                            d.copy_from_slice(&[
                                ((r << 3) | (r >> 2)) as u8,
                                ((g << 2) | (g >> 4)) as u8,
                                ((b << 3) | (b >> 2)) as u8,
                                255,
                            ]);
                        }
                    }
                    PixelFormat::Rgb1555 => {
                        for (px, d) in row.chunks_exact(2).zip(dst.chunks_exact_mut(4)) {
                            let p = u16::from_le_bytes([px[0], px[1]]) as u32;
                            let (r, g, b) = ((p >> 10) & 0x1F, (p >> 5) & 0x1F, p & 0x1F);
                            d.copy_from_slice(&[
                                ((r << 3) | (r >> 2)) as u8,
                                ((g << 3) | (g >> 2)) as u8,
                                ((b << 3) | (b >> 2)) as u8,
                                255,
                            ]);
                        }
                    }
                }
            }
            if double {
                let (head, tail) = back.split_at_mut(o + w * 4);
                for (s, d) in head[o..]
                    .chunks_exact(4)
                    .zip(tail[..w * 4].chunks_exact_mut(4))
                {
                    d.copy_from_slice(&darken(s, dim));
                }
            } else if scanlines && y & 1 == 1 {
                for d in back[o..o + w * 4].chunks_exact_mut(4) {
                    let v = darken(d, (dim + 256) / 2);
                    d.copy_from_slice(&v);
                }
            }
        }

        let mut front = self.front.lock().unwrap();
        let old = std::mem::replace(&mut front.rgba, back);
        front.width = width;
        front.height = out_h as u32;
        drop(front);
        *self.back.lock().unwrap() = old;
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// Render thread: hands the newest frame to `reader` (kept short, it holds a lock). `None` if
    /// no frame has been produced yet.
    pub fn read<R>(&self, reader: impl FnOnce(&[u8], u32, u32) -> R) -> Option<R> {
        let front = self.front.lock().unwrap();
        if front.width == 0 || front.height == 0 {
            return None;
        }
        Some(reader(&front.rgba, front.width, front.height))
    }
}

fn darken(px: &[u8], level: u32) -> [u8; 4] {
    [
        ((px[0] as u32 * level) >> 8) as u8,
        ((px[1] as u32 * level) >> 8) as u8,
        ((px[2] as u32 * level) >> 8) as u8,
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrgb_becomes_rgba() {
        let fb = FrameBuffer::new();
        // two pixels: red then green, memory order B G R X
        let src = [0, 0, 255, 0, 0, 255, 0, 0];
        fb.submit(&src, 2, 1, 8, PixelFormat::Xrgb8888);
        fb.read(|p, w, h| {
            assert_eq!((w, h), (2, 1));
            assert_eq!(p, &[255, 0, 0, 255, 0, 255, 0, 255]);
        })
        .unwrap();
        assert_eq!(fb.sequence(), 1);
    }

    #[test]
    fn scanlines_double_the_lines_and_dim_the_new_ones() {
        let fb = FrameBuffer::new();
        fb.set_scanlines(true, 0.5);
        let src = [200, 200, 200, 0];
        fb.submit(&src, 1, 1, 4, PixelFormat::Xrgb8888);
        fb.read(|p, w, h| {
            assert_eq!((w, h), (1, 2));
            assert_eq!(&p[0..4], &[200, 200, 200, 255]);
            assert_eq!(&p[4..8], &[100, 100, 100, 255]);
        })
        .unwrap();
    }

    #[test]
    fn rgb565_expands_to_full_range() {
        let fb = FrameBuffer::new();
        let white = 0xFFFFu16.to_le_bytes();
        fb.submit(&white, 1, 1, 2, PixelFormat::Rgb565);
        fb.read(|p, _, _| assert_eq!(p, &[255, 255, 255, 255]))
            .unwrap();
    }

    #[test]
    fn nothing_to_read_before_the_first_frame() {
        assert!(FrameBuffer::new().read(|_, _, _| ()).is_none());
    }
}
