//! The mod's folders inside the game directory: `<game dir>/consolemaniacraft/...`.

use crate::system::GameSystem;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Dirs {
    root: PathBuf,
}

/// The platform's dynamic library extension.
pub fn lib_extension() -> &'static str {
    if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

impl Dirs {
    /// `root` is `<game dir>/consolemaniacraft`.
    pub fn new(root: impl Into<PathBuf>) -> Dirs {
        Dirs { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cores(&self) -> PathBuf {
        self.root.join("cores")
    }

    pub fn roms(&self) -> PathBuf {
        self.root.join("roms")
    }

    pub fn saves(&self) -> PathBuf {
        self.root.join("saves")
    }

    pub fn system(&self) -> PathBuf {
        self.root.join("system")
    }

    /// The platform's file name for the recommended core of `system`.
    pub fn default_core_file_name(system: GameSystem) -> String {
        format!("{}.{}", system.core_stems()[0], lib_extension())
    }

    /// Find a core for `system`: the configured file name first, then the known cores in order of
    /// preference, then anything whose name starts like one of them (a renamed nightly).
    pub fn find_core(&self, system: GameSystem, configured: &str) -> Option<PathBuf> {
        let ext = lib_extension();
        if !configured.trim().is_empty() {
            let direct = self.cores().join(configured);
            if direct.is_file() {
                return Some(direct);
            }
        }
        for stem in system.core_stems() {
            let p = self.cores().join(format!("{stem}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        let mut all: Vec<PathBuf> = fs::read_dir(self.cores())
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        all.sort();
        for stem in system.core_stems() {
            let prefix = stem.replace("_libretro", "");
            for p in &all {
                let f = p
                    .file_name()
                    .map(|f| f.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if f.starts_with(&prefix) && f.ends_with(&format!(".{ext}")) {
                    return Some(p.clone());
                }
            }
        }
        None
    }

    /// Create the folders and (re)write the READMEs.
    pub fn ensure_folders(&self) -> io::Result<()> {
        for d in [self.cores(), self.roms(), self.saves(), self.system()] {
            fs::create_dir_all(d)?;
        }
        write_readme(&self.cores().join("README.txt"), CORES_README)?;
        write_readme(&self.roms().join("README.txt"), ROMS_README)
    }
}

/// (Re)write one of our README files when its text changed (e.g. after an update added a console).
fn write_readme(file: &Path, text: &str) -> io::Result<()> {
    if fs::read_to_string(file).is_ok_and(|t| t == text) {
        return Ok(());
    }
    fs::write(file, text)
}

const CORES_README: &str = "\
ConsoleManiaCraft — emulator cores
==================================
Put the libretro cores here. Download them from the libretro buildbot
(Windows: https://buildbot.libretro.com/nightly/windows/x86_64/latest/) and unzip them into this folder.

CMC-64 (Nintendo 64):   mupen64plus_next_libretro.dll   (from mupen64plus_next_libretro.dll.zip)
CMC-DS (Nintendo DS):   melondsds_libretro.dll          (from melondsds_libretro.dll.zip)
CMC-3DS (Nintendo 3DS): azahar_libretro.dll             (from azahar_libretro.dll.zip)

Linux uses .so and macOS .dylib instead of .dll.
The mod selects the software renderers (Angrylion for the N64, melonDS's and Azahar's own
software renderers for the DS and 3DS). The 3DS software renderer needs a fast CPU.
melonDS DS has a built-in BIOS and firmware, so no BIOS files are needed.
Azahar keeps its 3DS files (saves, SD card, system data) in consolemaniacraft/saves/Azahar.
Some 3DS games need system files dumped from your own 3DS (e.g. the shared fonts); put them
into consolemaniacraft/saves/Azahar/sysdata and nand the way Azahar expects.
";

const ROMS_README: &str = "\
ConsoleManiaCraft — ROMs
========================
Put your own game dumps here:
  Nintendo 64: .z64, .n64, .v64
  Nintendo DS: .nds
(a .zip containing one works too), and
  Nintendo 3DS: .3ds, .cci, .cxi, .3dsx (DECRYPTED dumps; no .cia, no .zip)
In game, right-click a Blank Cartridge, a Blank DS Game Card or a Blank 3DS Game Card to flash
one of these onto it.

In multiplayer every player who wants to see the game needs the same ROM in their own folder;
cartridges only store the file name and its SHA-1, never the game itself.
";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cmc-emu-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_cores_by_name_preference_and_prefix() {
        let dirs = Dirs::new(temp("paths"));
        dirs.ensure_folders().unwrap();
        let ext = lib_extension();
        assert_eq!(dirs.find_core(GameSystem::Nds, ""), None);
        let nightly = dirs.cores().join(format!("melondsds_libretro_2025.{ext}"));
        fs::write(&nightly, b"x").unwrap();
        assert_eq!(dirs.find_core(GameSystem::Nds, ""), Some(nightly.clone()));
        let legacy = dirs.cores().join(format!("desmume_libretro.{ext}"));
        fs::write(&legacy, b"x").unwrap();
        let best = dirs.cores().join(format!("melondsds_libretro.{ext}"));
        fs::write(&best, b"x").unwrap();
        assert_eq!(dirs.find_core(GameSystem::Nds, ""), Some(best));
        assert_eq!(
            dirs.find_core(GameSystem::Nds, &format!("desmume_libretro.{ext}")),
            Some(legacy)
        );
        assert_eq!(dirs.find_core(GameSystem::N3ds, ""), None);
        assert!(dirs.roms().join("README.txt").is_file());
    }
}
