//! A sample ring between the emulation thread (producer) and the game's sound thread (consumer).
//! Stereo frames go in; they can be read back as stereo or downmixed to mono (a TV is one speaker
//! in the world, and only mono sources are positioned by the sound engine).

use std::sync::Mutex;

struct Inner {
    /// interleaved L, R
    buf: Vec<i16>,
    read: usize,
    write: usize,
    /// frames waiting
    count: usize,
    underruns: u64,
}

pub struct AudioRing {
    sample_rate: u32,
    inner: Mutex<Inner>,
}

impl AudioRing {
    pub fn new(sample_rate: u32, capacity_millis: u32) -> Self {
        let frames = (sample_rate as usize * capacity_millis as usize / 1000).max(4096);
        AudioRing {
            sample_rate,
            inner: Mutex::new(Inner {
                buf: vec![0; frames * 2],
                read: 0,
                write: 0,
                count: 0,
                underruns: 0,
            }),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames currently waiting to be played.
    pub fn buffered(&self) -> usize {
        self.inner.lock().unwrap().count
    }

    pub fn underruns(&self) -> u64 {
        self.inner.lock().unwrap().underruns
    }

    /// Append interleaved stereo; the oldest audio is dropped if the ring is full.
    pub fn write_stereo(&self, interleaved: &[i16]) {
        let mut g = self.inner.lock().unwrap();
        let cap = g.buf.len() / 2;
        for f in interleaved.chunks_exact(2) {
            let w = g.write;
            g.buf[w * 2] = f[0];
            g.buf[w * 2 + 1] = f[1];
            g.write = (w + 1) % cap;
            if g.count == cap {
                g.read = (g.read + 1) % cap;
            } else {
                g.count += 1;
            }
        }
    }

    /// Fill `out` with mono samples (L+R)/2; returns how many frames were available.
    pub fn read_mono(&self, out: &mut [i16]) -> usize {
        self.read_with(out.len(), |i, l, r| {
            out[i] = ((l as i32 + r as i32) >> 1) as i16
        })
    }

    /// Fill `out` (interleaved L, R) with up to `out.len() / 2` frames; returns the frames read.
    pub fn read_stereo(&self, out: &mut [i16]) -> usize {
        let n = out.len() / 2;
        self.read_with(n, |i, l, r| {
            out[i * 2] = l;
            out[i * 2 + 1] = r;
        })
    }

    fn read_with(&self, want: usize, mut put: impl FnMut(usize, i16, i16)) -> usize {
        let mut g = self.inner.lock().unwrap();
        let n = want.min(g.count);
        let cap = g.buf.len() / 2;
        for i in 0..n {
            let r = g.read;
            put(i, g.buf[r * 2], g.buf[r * 2 + 1]);
            g.read = (r + 1) % cap;
        }
        g.count -= n;
        if n < want {
            g.underruns += 1;
        }
        n
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap();
        g.read = 0;
        g.write = 0;
        g.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_downmix_and_underrun() {
        let r = AudioRing::new(1000, 10);
        r.write_stereo(&[100, 300, -50, 50]);
        assert_eq!(r.buffered(), 2);
        let mut out = [0i16; 3];
        assert_eq!(r.read_mono(&mut out), 2);
        assert_eq!(&out[..2], &[200, 0]);
        assert_eq!(r.underruns(), 1);
    }

    #[test]
    fn oldest_audio_is_dropped_when_full() {
        let r = AudioRing::new(1, 1); // 4096 frames minimum
        let n = 4096;
        let data: Vec<i16> = (0..(n + 3) * 2).map(|i| (i / 2) as i16).collect();
        r.write_stereo(&data);
        assert_eq!(r.buffered(), n);
        let mut out = [0i16; 2];
        r.read_stereo(&mut out);
        assert_eq!(out, [3, 3]); // the first three frames were overwritten
    }
}
