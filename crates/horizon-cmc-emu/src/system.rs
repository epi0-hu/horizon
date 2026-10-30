//! The consoles the mod emulates, and what differs between them.

use crate::options::Prefs;

/// A console. The CMC-64 is a console block on a TV; the other two are handhelds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameSystem {
    /// Nintendo 64: Mupen64Plus-Next with the Angrylion software renderer.
    N64,
    /// Nintendo DS: melonDS DS.
    Nds,
    /// Nintendo 3DS: Azahar with its software renderer.
    N3ds,
}

impl GameSystem {
    pub const ALL: [GameSystem; 3] = [GameSystem::N64, GameSystem::Nds, GameSystem::N3ds];

    pub fn id(self) -> &'static str {
        match self {
            GameSystem::N64 => "n64",
            GameSystem::Nds => "nds",
            GameSystem::N3ds => "3ds",
        }
    }

    pub fn machine_name(self) -> &'static str {
        match self {
            GameSystem::N64 => "CMC-64",
            GameSystem::Nds => "CMC-DS",
            GameSystem::N3ds => "CMC-3DS",
        }
    }

    pub fn is_handheld(self) -> bool {
        self != GameSystem::N64
    }

    /// Core file names (without the platform's extension) that can run this system, best first.
    pub fn core_stems(self) -> &'static [&'static str] {
        match self {
            GameSystem::N64 => &["mupen64plus_next_libretro"],
            GameSystem::Nds => &[
                "melondsds_libretro",
                "melonds_libretro",
                "desmume_libretro",
                "desmume2015_libretro",
            ],
            GameSystem::N3ds => &["azahar_libretro"],
        }
    }

    /// ROM file extensions (lower case, with the dot).
    pub fn rom_extensions(self) -> &'static [&'static str] {
        match self {
            GameSystem::N64 => &[".z64", ".n64", ".v64"],
            GameSystem::Nds => &[".nds", ".dsi"],
            // Decrypted dumps only (Azahar can't play encrypted ones); .cia is an installer, not a game.
            GameSystem::N3ds => &[".3ds", ".cci", ".cxi", ".3dsx", ".zcci", ".zcxi", ".z3dsx"],
        }
    }

    /// Zipped ROMs are unpacked into memory. Not for the 3DS: its games are up to 4 GB and Azahar
    /// reads them from the file itself.
    pub fn zips_allowed(self) -> bool {
        self != GameSystem::N3ds
    }

    /// The largest ROM extracted from a zip into memory.
    pub fn max_zip_rom_bytes(self) -> u64 {
        match self {
            GameSystem::N64 => 128 << 20,
            GameSystem::Nds => 1 << 30,
            GameSystem::N3ds => 0,
        }
    }

    /// Core option preferences that make the core render in software and fit the mod's screens.
    pub fn default_options(self) -> Prefs {
        match self {
            GameSystem::N64 => crate::options::mupen_defaults(),
            GameSystem::Nds => crate::options::melonds_defaults(),
            GameSystem::N3ds => crate::options::azahar_defaults(),
        }
    }

    /// Frame height at which the video is two screens stacked: (top, bottom) screen sizes in the
    /// core's frame, used to cut the picture into screens.
    pub fn screens(self) -> Option<Screens> {
        match self {
            GameSystem::N64 => None,
            GameSystem::Nds => Some(Screens {
                top: (256, 192),
                bottom: (256, 192),
            }),
            GameSystem::N3ds => Some(Screens {
                top: (400, 240),
                bottom: (320, 240),
            }),
        }
    }
}

/// Where a handheld's two screens sit in the core's frame (top over bottom, bottom centred).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screens {
    pub top: (u32, u32),
    pub bottom: (u32, u32),
}

impl Screens {
    /// The top screen as (u0, u1, v0, v1) fractions of a `w`×`h` frame.
    pub fn top_uv(&self, _w: u32, h: u32) -> [f32; 4] {
        let rows = self.top.1.min(h / 2).max(1);
        [0.0, 1.0, 0.0, rows as f32 / h.max(1) as f32]
    }

    /// The touch screen as (u0, u1, v0, v1) fractions of a `w`×`h` frame. The DS's two screens fill
    /// the frame's width; the 3DS's narrower bottom screen is centred under the wide top one.
    pub fn bottom_uv(&self, w: u32, h: u32) -> [f32; 4] {
        let rows = self.bottom.1.min(h / 2).max(1);
        let v0 = (h.saturating_sub(rows)) as f32 / h.max(1) as f32;
        let frac = (self.bottom.0 as f32 / self.top.0.max(1) as f32).min(1.0);
        let _ = w;
        let u0 = (1.0 - frac) / 2.0;
        [u0, 1.0 - u0, v0, 1.0]
    }
}
