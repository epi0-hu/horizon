//! ConsoleManiaCraft's emulator side: a libretro frontend that runs the Nintendo 64, DS and 3DS
//! cores in software, plus the ROM library, battery saves and option handling around it.
//!
//! The game's own crates forbid unsafe code; everything that has to touch a native core lives
//! here, in `core` (loading the library and the callbacks) and `guard` (Windows exception
//! handlers). The rest is safe code.
//!
//! The crate knows nothing about Minecraft or Horizon: the game feeds a [`Session`] its input and
//! takes the picture from [`Session::frames`] and the sound from [`Session::audio_ring`].
#![deny(unsafe_op_in_unsafe_fn)]

pub mod audio;
pub mod core;
pub mod format;
pub mod frame;
pub mod guard;
pub mod instances;
pub mod options;
pub mod pad;
pub mod paths;
pub mod retro;
pub mod rom;
pub mod session;
pub mod system;

pub use audio::AudioRing;
pub use core::{AvInfo, Core, CoreError, Frontend, SystemInfo};
pub use frame::FrameBuffer;
pub use options::{CoreOptions, Prefs};
pub use pad::PadState;
pub use paths::Dirs;
pub use rom::{Rom, RomRef};
pub use session::{Failure, Session, SessionConfig, State};
pub use system::{GameSystem, Screens};
