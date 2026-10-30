//! Smoke test against the real emulator cores. Skipped unless `CMC_REAL_CORES` names a folder
//! holding Linux builds from the libretro buildbot (`azahar_libretro.so`, `melondsds_libretro.so`,
//! `mupen64plus_next_libretro.so`; any subset). No games are needed: each core is loaded, initialised
//! and shut down, which exercises the environment negotiation, the options we ask for, the logging
//! thread of the core and unloading the library. A junk ROM must be refused cleanly.
#![cfg(target_os = "linux")]

use horizon_cmc_emu::core::{Core, CoreError, Frontend};
use horizon_cmc_emu::retro::PixelFormat;
use horizon_cmc_emu::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder {
    lines: Mutex<Vec<(u32, String)>>,
}

impl Frontend for Recorder {
    fn video(&self, _: &[u8], _: u32, _: u32, _: usize, _: PixelFormat) {}
    fn audio(&self, _: &[i16]) {}
    fn input(&self, _: u32, _: u32, _: u32, _: u32) -> i16 {
        0
    }
    fn log(&self, level: u32, message: &str) {
        self.lines
            .lock()
            .unwrap()
            .push((level, message.to_string()));
    }
}

fn real_core(system: GameSystem) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("CMC_REAL_CORES")?);
    // the core and the folders live in a scratch copy, as in the game
    let src = system
        .core_stems()
        .iter()
        .map(|s| dir.join(format!("{s}.so")))
        .find(|p| p.is_file())?;
    Some(src)
}

fn smoke(system: GameSystem, expect_options: &[(&str, &str)]) {
    let Some(lib) = real_core(system) else {
        eprintln!("skipping {system:?}: no real core in CMC_REAL_CORES");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "cmc-emu-real-{}-{}",
        system.id(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let dirs = Dirs::new(&root);
    dirs.ensure_folders().unwrap();
    let rec = Arc::new(Recorder::default());
    let copy = instances::CoreCopy::acquire(&dirs, &lib).unwrap();

    {
        let mut core = Core::load(
            copy.path(),
            &dirs.system(),
            &dirs.saves(),
            CoreOptions::new(system.default_options()),
            rec.clone(),
            "Alexander123",
        )
        .expect("load the real core");
        let info = core.init().expect("retro_init");
        eprintln!(
            "{system:?}: {} {} (need_fullpath={}, extensions {})",
            info.name, info.version, info.need_fullpath, info.extensions
        );

        let junk = dirs.roms().join("junk.bin");
        std::fs::write(&junk, vec![0xAB; 4096]).unwrap();
        match core.load_game(&junk, None) {
            Err(CoreError::RomRefused(msg)) => eprintln!("{system:?}: junk ROM refused: {msg:?}"),
            other => panic!("a junk ROM must be refused, got {other:?}"),
        }
        // dropping the core unloads the game (never loaded), deinitialises and unloads the library
    }
    std::thread::sleep(std::time::Duration::from_millis(300)); // threads the core was still joining

    let lines = rec.lines.lock().unwrap();
    for (key, value) in expect_options {
        let want = format!("Core option {key} = {value}");
        assert!(
            lines.iter().any(|(_, m)| *m == want),
            "{system:?}: expected '{want}' in the log; options seen: {:?}",
            lines
                .iter()
                .filter(|(_, m)| m.starts_with("Core option"))
                .collect::<Vec<_>>()
        );
    }
    assert!(
        !lines.iter().any(|(_, m)| m.contains("needs 'Software'")),
        "the core ended up in a GPU renderer: {:?}",
        lines.iter().filter(|(l, _)| *l >= 2).collect::<Vec<_>>()
    );
    eprintln!("{system:?}: {} log lines, clean unload", lines.len());
}

#[test]
fn azahar_loads_picks_software_and_unloads() {
    smoke(
        GameSystem::N3ds,
        &[
            ("citra_graphics_api", "Software"),
            ("citra_layout_option", "default"),
            ("citra_analog_function", "c_stick"),
        ],
    );
}

#[test]
fn melonds_loads_picks_software_and_unloads() {
    smoke(
        GameSystem::Nds,
        &[
            ("melonds_render_mode", "software"),
            ("melonds_screen_layout1", "top-bottom"),
        ],
    );
}

#[test]
fn mupen64plus_next_loads_picks_angrylion_and_unloads() {
    smoke(GameSystem::N64, &[("mupen64plus-rdp-plugin", "angrylion")]);
}
