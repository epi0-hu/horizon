//! Player one's input as the core sees it: a libretro RetroPad plus a pointer (the touch screen).
//! Written by the client while the player plays, read by the emulation thread.
//!
//! Mupen64Plus-Next: RetroPad B = N64 A, Y = N64 B, L2 = Z, L = L, R = R, Start = Start, left analog
//! = control stick, right analog = C-buttons, D-pad = D-pad. melonDS DS maps the RetroPad one to
//! one (L3 = microphone) and reads the touch screen from the pointer. Azahar maps it one to one
//! too, with L2/R2 = ZL/ZR, left analog = Circle Pad, right analog = C-stick.

use crate::retro::*;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

#[derive(Debug, Default)]
pub struct PadState {
    buttons: AtomicU32,
    /// left x, left y, right x, right y as four packed i16
    axes: AtomicU64,
    /// pointer x, y (i16 each, libretro range), bit 32 = pressed
    pointer: AtomicU64,
}

impl PadState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, button_mask: u32, lx: i16, ly: i16, rx: i16, ry: i16) {
        self.buttons.store(button_mask, Relaxed);
        let a = (lx as u16 as u64)
            | ((ly as u16 as u64) << 16)
            | ((rx as u16 as u64) << 32)
            | ((ry as u16 as u64) << 48);
        self.axes.store(a, Relaxed);
    }

    /// Touch position in libretro pointer coordinates (-0x7FFF..0x7FFF across the whole video frame).
    pub fn set_pointer(&self, x: i16, y: i16, pressed: bool) {
        let p = (x as u16 as u64) | ((y as u16 as u64) << 16) | if pressed { 1 << 32 } else { 0 };
        self.pointer.store(p, Relaxed);
    }

    pub fn buttons(&self) -> u32 {
        self.buttons.load(Relaxed)
    }

    pub fn clear(&self) {
        self.buttons.store(0, Relaxed);
        self.axes.store(0, Relaxed);
        self.pointer.store(0, Relaxed);
    }

    /// What the core asks for: `retro_input_state_t(port, device, index, id)`.
    pub fn query(&self, port: u32, device: u32, index: u32, id: u32) -> i16 {
        if port != 0 {
            return 0;
        }
        match device {
            DEVICE_JOYPAD => {
                let b = self.buttons();
                if id == JOYPAD_MASK {
                    (b & 0xFFFF) as i16
                } else if id < 16 {
                    ((b >> id) & 1) as i16
                } else {
                    0
                }
            }
            DEVICE_ANALOG if index <= ANALOG_INDEX_RIGHT && id <= ANALOG_Y => {
                let shift = (index * 2 + id) * 16;
                (self.axes.load(Relaxed) >> shift) as u16 as i16
            }
            DEVICE_POINTER if index == 0 => {
                let p = self.pointer.load(Relaxed);
                match id {
                    POINTER_X => p as u16 as i16,
                    POINTER_Y => (p >> 16) as u16 as i16,
                    POINTER_PRESSED | POINTER_COUNT => ((p >> 32) & 1) as i16,
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_axes_and_pointer() {
        let p = PadState::new();
        p.set((1 << pad::A) | (1 << pad::L2), 0x7FFF, -0x7FFF, 0x4000, -1);
        assert_eq!(p.query(0, DEVICE_JOYPAD, 0, pad::A), 1);
        assert_eq!(p.query(0, DEVICE_JOYPAD, 0, pad::B), 0);
        assert_eq!(
            p.query(0, DEVICE_JOYPAD, 0, JOYPAD_MASK) as u16,
            (1 << 8) | (1 << 12)
        );
        assert_eq!(
            p.query(0, DEVICE_ANALOG, ANALOG_INDEX_LEFT, ANALOG_X),
            0x7FFF
        );
        assert_eq!(
            p.query(0, DEVICE_ANALOG, ANALOG_INDEX_LEFT, ANALOG_Y),
            -0x7FFF
        );
        assert_eq!(
            p.query(0, DEVICE_ANALOG, ANALOG_INDEX_RIGHT, ANALOG_X),
            0x4000
        );
        assert_eq!(p.query(0, DEVICE_ANALOG, ANALOG_INDEX_RIGHT, ANALOG_Y), -1);
        p.set_pointer(-100, 200, true);
        assert_eq!(p.query(0, DEVICE_POINTER, 0, POINTER_X), -100);
        assert_eq!(p.query(0, DEVICE_POINTER, 0, POINTER_Y), 200);
        assert_eq!(p.query(0, DEVICE_POINTER, 0, POINTER_PRESSED), 1);
        assert_eq!(p.query(0, DEVICE_POINTER, 1, POINTER_X), 0); // only pointer 0
        assert_eq!(p.query(1, DEVICE_JOYPAD, 0, pad::A), 0); // only port 0
        p.clear();
        assert_eq!(p.query(0, DEVICE_JOYPAD, 0, pad::A), 0);
    }
}
