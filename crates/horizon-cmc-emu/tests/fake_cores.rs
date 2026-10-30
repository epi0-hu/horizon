//! Runs the real frontend against small fake libretro cores written in C (`tests/fakes`), built
//! with `gcc` at test time. They stand in for the emulators:
//!
//! * `fakecore.c`: a Mupen64Plus-Next-like core that runs on a libco-style coroutine with its own
//!   heap stack and calls input, audio and log from there
//! * `fakeds.c`: a melonDS-like core: legacy option list, XRGB8888 256x384 frames, pointer touch
//! * `fakeds_deep.c`: the DS core using ~24 MB of stack on the emulation thread
//! * `fake3ds.c`: an Azahar-like core: path-only loading, 400x480 frames with the 320-wide touch
//!   screen centred in the lower half, Circle Pad / C-stick / ZL / ZR, and a log thread of its own
//!   that hands out heap strings it frees right after the call
//!
//! The cores keep a few facts in their save memory (test-only) so the tests can read what they saw.
#![cfg(target_os = "linux")]

use horizon_cmc_emu::retro::pad;
use horizon_cmc_emu::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

fn gcc_available() -> bool {
    Command::new("gcc")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Compile `tests/fakes/<src>` to a shared library once per test run.
fn build(src: &str) -> PathBuf {
    static BUILT: OnceLock<Mutex<std::collections::HashMap<String, PathBuf>>> = OnceLock::new();
    let map = BUILT.get_or_init(Default::default);
    let mut map = map.lock().unwrap();
    if let Some(p) = map.get(src) {
        return p.clone();
    }
    let out_dir = std::env::temp_dir().join(format!("cmc-emu-fakes-{}", std::process::id()));
    fs::create_dir_all(&out_dir).unwrap();
    let lib = out_dir.join(format!("{}.so", src.trim_end_matches(".c")));
    let status = Command::new("gcc")
        .args(["-shared", "-fPIC", "-O0", "-pthread", "-o"])
        .arg(&lib)
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fakes")
                .join(src),
        )
        .status()
        .expect("run gcc");
    assert!(status.success(), "gcc failed on {src}");
    map.insert(src.to_string(), lib.clone());
    lib
}

struct Setup {
    dirs: Dirs,
}

/// A fresh consolemaniacraft folder with `core_src` installed as the core of `system`.
fn setup(name: &str, system: GameSystem, core_src: Option<&str>) -> Setup {
    let root = std::env::temp_dir().join(format!("cmc-emu-it-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dirs = Dirs::new(&root);
    dirs.ensure_folders().unwrap();
    if let Some(src) = core_src {
        let dest = dirs.cores().join(Dirs::default_core_file_name(system));
        fs::copy(build(src), dest).unwrap();
    }
    Setup { dirs }
}

fn write_rom(dirs: &Dirs, name: &str, bytes: &[u8]) -> RomRef {
    fs::write(dirs.roms().join(name), bytes).unwrap();
    let sha1 = rom::sha1(&dirs.roms().join(name), GameSystem::N64).unwrap();
    RomRef {
        rom_file: name.into(),
        sha1,
    }
}

fn config(s: &Setup, system: GameSystem, rom: RomRef, key: &str) -> SessionConfig {
    SessionConfig {
        system,
        dirs: s.dirs.clone(),
        rom,
        save_key: key.into(),
        title: "Test Game".into(),
        core_file: String::new(),
        options: system.default_options(),
        username: "Alexander123".into(),
        scanlines: None,
        audio_chunk_millis: 30,
        log: Some(Arc::new(|_, _| {})),
    }
}

fn wait_for(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_frames(session: &Session, n: u64) {
    let start = session.frames.sequence();
    wait_for("frames", Duration::from_secs(30), || {
        session.frames.sequence() >= start + n
    });
}

fn stop_and_read_save(s: &Setup, session: &Session, key: &str) -> Vec<u8> {
    session.request_stop();
    session.join(Duration::from_secs(10));
    assert_eq!(
        session.state(),
        State::Stopped,
        "failure: {:?}",
        session.failure()
    );
    fs::read(s.dirs.saves().join(format!("{key}.srm"))).expect("a save file was written on stop")
}

#[test]
fn n64_core_on_a_coroutine_stack_gets_input_audio_video_and_saves() {
    if !gcc_available() {
        return;
    }
    let s = setup("n64", GameSystem::N64, Some("fakecore.c"));
    let rom = write_rom(&s.dirs, "game.z64", &[0x80, 0x37, 0x12, 0x40, 1, 2, 3]);
    let mut cfg = config(&s, GameSystem::N64, rom, "cart-n64");
    cfg.scanlines = Some(0.45);
    let session = Session::start(cfg);

    wait_for("running", Duration::from_secs(20), || {
        session.state() == State::Running
    });
    session.set_controlled(true);
    session.pad.set(1 << pad::A, 0x7FFF, 0, 0, -0x7FFF);
    run_frames(&session, 40);

    // The fake draws R=255, G=x, B=y: pixel (10, 5). Scanlines double the lines: 320x480.
    session
        .frames
        .read(|px, w, h| {
            assert_eq!((w, h), (320, 480));
            let o = (5 * 2 * 320 + 10) * 4;
            assert_eq!(&px[o..o + 4], &[255, 10, 5, 255]);
            let dim = o + 320 * 4;
            assert!(
                px[dim + 1] < px[o + 1] + 1 && px[dim + 1] > 0,
                "the in-between line is a dimmed copy"
            );
        })
        .expect("a frame");
    let ring = session.audio_ring().expect("audio ring");
    assert!(ring.buffered() > 735, "audio arrived from the coroutine");
    assert!(session.first_frame_elapsed().is_some());

    let save = stop_and_read_save(&s, &session, "cart-n64");
    assert_eq!(save.len(), 2048);
    assert!(save[0] > 30, "the fake counts frames in its save memory");
    assert_eq!(
        save[1], 0x01,
        "RetroPad A (bit 8) reached the core on its coroutine stack"
    );
    assert_eq!(save[2], 0x7F, "left stick x");
    assert_eq!(
        save[3] as i8, -0x80i8,
        "right stick y (-0x7FFF >> 8 = -128)"
    );
}

#[test]
fn input_only_reaches_the_controlled_game() {
    if !gcc_available() {
        return;
    }
    let s = setup("uncontrolled", GameSystem::N64, Some("fakecore.c"));
    let rom = write_rom(&s.dirs, "game.z64", &[0x80, 0x37, 0x12, 0x40]);
    let session = Session::start(config(&s, GameSystem::N64, rom, "cart-idle"));
    wait_for("running", Duration::from_secs(20), || {
        session.state() == State::Running
    });
    session.pad.set(1 << pad::A, 0, 0, 0, 0); // pad is set, but nobody holds the controller
    run_frames(&session, 20);
    let save = stop_and_read_save(&s, &session, "cart-idle");
    assert_eq!(save[1], 0, "an uncontrolled game sees an idle pad");
}

#[test]
fn two_games_run_side_by_side_with_private_core_copies() {
    if !gcc_available() {
        return;
    }
    let s = setup("pair", GameSystem::N64, Some("fakecore.c"));
    let rom = write_rom(&s.dirs, "game.z64", &[0x80, 0x37, 0x12, 0x40]);
    let a = Session::start(config(&s, GameSystem::N64, rom.clone(), "cart-a"));
    let b = Session::start(config(&s, GameSystem::N64, rom, "cart-b"));
    wait_for("both running", Duration::from_secs(30), || {
        a.state() == State::Running && b.state() == State::Running
    });
    a.set_controlled(true);
    a.pad.set(1 << pad::A, 0, 0, 0, 0);
    b.pad.set(1 << pad::A, 0, 0, 0, 0); // set, but b is not controlled
    run_frames(&a, 30);
    run_frames(&b, 30);
    let copies = fs::read_dir(s.dirs.cores().join(".running"))
        .unwrap()
        .count();
    assert_eq!(copies, 2, "each game loaded its own copy of the core file");
    let sa = stop_and_read_save(&s, &a, "cart-a");
    let sb = stop_and_read_save(&s, &b, "cart-b");
    assert_eq!(sa[1], 1);
    assert_eq!(
        sb[1], 0,
        "the other console's globals are separate and it got no input"
    );
    wait_for("copies removed", Duration::from_secs(5), || {
        fs::read_dir(s.dirs.cores().join(".running"))
            .unwrap()
            .count()
            == 0
    });
}

#[test]
fn ds_core_options_touch_and_user_name() {
    if !gcc_available() {
        return;
    }
    let s = setup("ds", GameSystem::Nds, Some("fakeds.c"));
    let rom = write_rom(&s.dirs, "nsmb.nds", &vec![7u8; 1 << 20]);
    let session = Session::start(config(&s, GameSystem::Nds, rom, "card-ds"));
    wait_for("running", Duration::from_secs(20), || {
        session.state() == State::Running
    });
    session.set_controlled(true);
    // the middle of the bottom screen: u = 0.5 -> pointer x 0; frame y fraction 0.75 -> +0.5 * 0x7FFF
    session.pad.set((1 << pad::A) | (1 << pad::L3), 0, 0, 0, 0);
    session
        .pad
        .set_pointer(0, (0.5 * 0x7FFF as f64).round() as i16, true);
    run_frames(&session, 40);
    session
        .frames
        .read(|_, w, h| assert_eq!((w, h), (256, 384)))
        .unwrap();
    let save = stop_and_read_save(&s, &session, "card-ds");
    assert_eq!(save.len(), 512 * 1024);
    assert_eq!(
        (save[1], save[2]),
        (128, 96),
        "touch lands in the middle of the bottom screen"
    );
    assert_eq!(
        save[3], 0x41,
        "A and the microphone button (L3) reached the core"
    );
}

#[test]
fn a_core_using_24_mb_of_stack_runs_on_the_emulation_thread() {
    if !gcc_available() {
        return;
    }
    let s = setup("deep", GameSystem::Nds, Some("fakeds_deep.c"));
    let rom = write_rom(&s.dirs, "nsmb.nds", &vec![7u8; 1 << 20]);
    // SAFETY: only this test's core reads DEEP, and it is set before the core starts.
    unsafe { std::env::set_var("DEEP", "384") };
    let session = Session::start(config(&s, GameSystem::Nds, rom, "card-deep"));
    wait_for("running", Duration::from_secs(20), || {
        session.state() == State::Running
    });
    run_frames(&session, 5);
    stop_and_read_save(&s, &session, "card-deep");
}

#[test]
fn azahar_like_core_gets_a_path_only_touch_sticks_and_logs_from_its_own_thread() {
    if !gcc_available() {
        return;
    }
    let s = setup("3ds", GameSystem::N3ds, Some("fake3ds.c"));
    let rom = write_rom(&s.dirs, "adventure.3ds", &[0u8; 0x400]);
    let lines: Arc<Mutex<Vec<(u32, String)>>> = Arc::default();
    let sink = lines.clone();
    let mut cfg = config(&s, GameSystem::N3ds, rom, "card-3ds");
    cfg.log = Some(Arc::new(move |level, msg| {
        sink.lock().unwrap().push((level, msg.to_string()))
    }));
    let session = Session::start(cfg);
    wait_for("running", Duration::from_secs(20), || {
        session.state() == State::Running
    });
    session.set_controlled(true);
    // A quarter across and halfway down the 320x240 touch screen, as the handheld screen maps it
    // into Azahar's 400x480 frame: x 0.1 + 0.25*0.8, y 0.5 + 0.5*0.5.
    let screens = GameSystem::N3ds.screens().unwrap();
    let uv = screens.bottom_uv(400, 480);
    let (fx, fy) = (
        uv[0] + 0.25 * (uv[1] - uv[0]),
        uv[2] + 0.5 * (uv[3] - uv[2]),
    );
    session.pad.set(
        (1 << pad::A) | (1 << pad::L2) | (1 << pad::R2),
        0x7FFF,
        -0x7FFF,
        0x4000,
        0,
    );
    session.pad.set_pointer(
        ((fx as f64 * 2.0 - 1.0) * 0x7FFF as f64).round() as i16,
        ((fy as f64 * 2.0 - 1.0) * 0x7FFF as f64).round() as i16,
        true,
    );
    run_frames(&session, 60);
    session
        .frames
        .read(|_, w, h| assert_eq!((w, h), (400, 480)))
        .unwrap();
    let save = stop_and_read_save(&s, &session, "card-3ds");
    assert_eq!(
        save[0], 1,
        "the core was handed the path only: no data, size 0"
    );
    assert_eq!(
        (save[1], save[2]),
        (79, 120),
        "touch maps onto the centred touch screen"
    );
    assert_eq!(save[3], 0x31, "A, ZL and ZR");
    assert_eq!((save[4], save[6]), (1, 1), "Circle Pad right and up");
    assert_eq!(save[5], 0x40, "C-stick x");

    // The log thread kept writing while the game ran: every line that came through is intact.
    let lines = lines.lock().unwrap();
    let thread_lines: Vec<&String> = lines
        .iter()
        .map(|(_, m)| m)
        .filter(|m| m.contains("thread log"))
        .collect();
    assert!(
        thread_lines.len() > 100,
        "got {} lines from the core's thread",
        thread_lines.len()
    );
    for m in thread_lines {
        // "[ 12.000042] Core.ARM11 <Info> thread log 42 ..." : the two counters agree, nothing overwritten
        let seq: u32 = m.split("12.").nth(1).unwrap()[..6].parse().unwrap();
        let n: u32 = m
            .split("thread log ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(seq, n, "{m}");
        assert!(!m.contains("XXXX"), "read a freed string: {m}");
    }
    assert!(
        lines.iter().any(|(l, _)| *l >= retro::LOG_WARN),
        "warning-level lines come through"
    );
}

#[test]
fn a_missing_core_or_rom_is_reported_not_crashed() {
    if !gcc_available() {
        return;
    }
    let s = setup("missing", GameSystem::Nds, None);
    let rom = write_rom(&s.dirs, "nsmb.nds", &[0u8; 64]);
    let session = Session::start(config(&s, GameSystem::Nds, rom.clone(), "x"));
    wait_for("failure", Duration::from_secs(10), || {
        session.state() == State::Error
    });
    assert_eq!(
        session.failure(),
        Some(Failure::NoCore("melondsds_libretro.so".into()))
    );

    let s = setup("missing-rom", GameSystem::Nds, Some("fakeds.c"));
    let session = Session::start(config(
        &s,
        GameSystem::Nds,
        RomRef {
            rom_file: "gone.nds".into(),
            sha1: "ab".into(),
        },
        "x",
    ));
    wait_for("failure", Duration::from_secs(10), || {
        session.state() == State::Error
    });
    assert_eq!(session.failure(), Some(Failure::NoRom("gone.nds".into())));

    // a core that refuses the ROM: the DS fake wants data, an empty ROM file has none
    let s = setup("bad-rom", GameSystem::Nds, Some("fakeds.c"));
    fs::write(s.dirs.roms().join("empty.nds"), b"").unwrap();
    let session = Session::start(config(
        &s,
        GameSystem::Nds,
        RomRef {
            rom_file: "empty.nds".into(),
            sha1: String::new(),
        },
        "x",
    ));
    wait_for("failure", Duration::from_secs(10), || {
        session.state() == State::Error
    });
    assert_eq!(session.failure(), Some(Failure::BadRom("empty.nds".into())));

    // a file that is not a library
    let s = setup("bad-core", GameSystem::Nds, None);
    fs::write(
        s.dirs.cores().join("melondsds_libretro.so"),
        b"not a library",
    )
    .unwrap();
    let rom = write_rom(&s.dirs, "nsmb.nds", &[0u8; 64]);
    let session = Session::start(config(&s, GameSystem::Nds, rom, "x"));
    wait_for("failure", Duration::from_secs(10), || {
        session.state() == State::Error
    });
    assert_eq!(
        session.failure(),
        Some(Failure::BadCore("melondsds_libretro.so".into()))
    );
}

#[test]
fn pausing_freezes_the_game() {
    if !gcc_available() {
        return;
    }
    let s = setup("pause", GameSystem::N64, Some("fakecore.c"));
    let rom = write_rom(&s.dirs, "game.z64", &[0x80, 0x37, 0x12, 0x40]);
    let session = Session::start(config(&s, GameSystem::N64, rom, "cart-pause"));
    wait_for("running", Duration::from_secs(20), || {
        session.state() == State::Running
    });
    run_frames(&session, 5);
    session.set_paused(true);
    std::thread::sleep(Duration::from_millis(100));
    let frozen = session.frames.sequence();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(session.frames.sequence(), frozen);
    session.set_paused(false);
    run_frames(&session, 5);
    stop_and_read_save(&s, &session, "cart-pause");
}
