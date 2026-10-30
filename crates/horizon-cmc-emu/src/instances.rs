//! Private copies of the core library, one per running game.
//!
//! The OS loads a library once per path, and a libretro core keeps everything (the emulated machine,
//! its threads, its coroutine) in global variables. Loading a separate copy of the file gives each
//! game a completely independent emulator.

use crate::paths::Dirs;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A copy of a core file in `cores/.running/`, removed when dropped.
pub struct CoreCopy {
    path: PathBuf,
}

fn running_dir(dirs: &Dirs) -> PathBuf {
    dirs.cores().join(".running")
}

impl CoreCopy {
    pub fn acquire(dirs: &Dirs, core: &Path) -> io::Result<CoreCopy> {
        let dir = running_dir(dirs);
        fs::create_dir_all(&dir)?;
        let name = core
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (stem, ext) = match name.rfind('.') {
            Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
            _ => (name.as_str(), ""),
        };
        for _ in 0..64 {
            let copy = dir.join(format!(
                "{stem}.{}{ext}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            // That name may still be loaded by a game that hasn't finished shutting down: try the next.
            if fs::copy(core, &copy).is_ok() {
                return Ok(CoreCopy { path: copy });
            }
        }
        Err(io::Error::other(format!(
            "could not make a private copy of {name}"
        )))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CoreCopy {
    fn drop(&mut self) {
        // Still in use (Windows keeps loaded DLLs locked)? Then it is removed on the next launch.
        let _ = fs::remove_file(&self.path);
    }
}

/// Remove copies left behind by a crash or an unclean exit.
pub fn cleanup(dirs: &Dirs) {
    if let Ok(files) = fs::read_dir(running_dir(dirs)) {
        for f in files.flatten() {
            let _ = fs::remove_file(f.path());
        }
    }
}
