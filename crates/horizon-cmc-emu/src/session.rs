//! One running game: a thread that loads the core and the ROM, then calls `retro_run` at the
//! game's frame rate until it is switched off. Used for powered CMC-64 consoles (one per console
//! block) and for the open CMC-DS / CMC-3DS handheld.

use crate::audio::AudioRing;
use crate::core::{AvInfo, Core, CoreError, Frontend};
use crate::frame::FrameBuffer;
use crate::instances::CoreCopy;
use crate::options::{CoreOptions, Prefs};
use crate::pad::PadState;
use crate::paths::Dirs;
use crate::retro::{LOG_ERROR, LOG_INFO, LOG_WARN, PixelFormat};
use crate::rom::{self, RomRef};
use crate::system::GameSystem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Cores run on this thread's stack (melonDS DS recurses deeply and keeps big buffers on the
/// stack). 64 MB is only reserved address space; pages are committed as used.
const EMU_STACK_BYTES: usize = 64 << 20;
const SAVE_INTERVAL: Duration = Duration::from_secs(5);

pub type LogSink = Arc<dyn Fn(u32, &str) + Send + Sync>;

#[derive(Clone)]
pub struct SessionConfig {
    pub system: GameSystem,
    pub dirs: Dirs,
    /// The ROM the cartridge was flashed with.
    pub rom: RomRef,
    /// Battery saves go to `saves/<save_key>.srm`; one per cartridge, so saves travel with it.
    pub save_key: String,
    pub title: String,
    /// A configured core file name inside `cores/`; empty = the system's usual core.
    pub core_file: String,
    /// Core option preferences, already adjusted for this session (e.g. render threads).
    pub options: Prefs,
    /// Reported as the frontend's user name (the DS firmware's owner name).
    pub username: String,
    /// CRT scanline darkness for the TV; `None` for LCDs.
    pub scanlines: Option<f32>,
    /// Size of the chunks the game's sound engine pulls; sets how much audio the pacing keeps queued.
    pub audio_chunk_millis: u32,
    /// Where the core's log lines go (default: stderr).
    pub log: Option<LogSink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Booting,
    Running,
    Error,
    Stopped,
}

/// Why a game didn't start or stopped; the game turns it into a message on the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No emulator core file; carries the file name to put in `cores/`.
    NoCore(String),
    /// The ROM the cartridge names isn't in the roms folder.
    NoRom(String),
    /// The core file wouldn't load (wrong file or architecture).
    BadCore(String),
    /// The core refused the ROM.
    BadRom(String),
    /// The emulator thread panicked or failed; carries a short description.
    Crashed(String),
}

impl Failure {
    /// The suffix of the translation key, e.g. `screen.consolemaniacraft.tv.<key>`.
    pub fn key(&self) -> &'static str {
        match self {
            Failure::NoCore(_) => "no_core",
            Failure::NoRom(_) => "no_rom",
            Failure::BadCore(_) => "bad_core",
            Failure::BadRom(_) => "bad_rom",
            Failure::Crashed(_) => "crashed",
        }
    }

    pub fn arg(&self) -> &str {
        match self {
            Failure::NoCore(a)
            | Failure::NoRom(a)
            | Failure::BadCore(a)
            | Failure::BadRom(a)
            | Failure::Crashed(a) => a,
        }
    }
}

/// What the core talks to: video into the frame buffer, audio into the ring, input from the pad.
struct Link {
    frames: Arc<FrameBuffer>,
    pad: Arc<PadState>,
    ring: OnceLock<Arc<AudioRing>>,
    controlled: AtomicBool,
    epoch: Instant,
    /// Nanoseconds since `epoch` of the first picture, plus one (0 = none yet).
    first_frame: AtomicU64,
    log: Option<LogSink>,
}

impl Frontend for Link {
    fn video(&self, data: &[u8], width: u32, height: u32, pitch: usize, format: PixelFormat) {
        self.frames.submit(data, width, height, pitch, format);
        if self.first_frame.load(Ordering::Relaxed) == 0 {
            self.first_frame.store(
                self.epoch.elapsed().as_nanos() as u64 + 1,
                Ordering::Relaxed,
            );
        }
    }

    fn audio(&self, interleaved_stereo: &[i16]) {
        if let Some(r) = self.ring.get() {
            r.write_stereo(interleaved_stereo);
        }
    }

    fn input(&self, port: u32, device: u32, index: u32, id: u32) -> i16 {
        if self.controlled.load(Ordering::Relaxed) {
            self.pad.query(port, device, index, id)
        } else {
            0
        }
    }

    fn log(&self, level: u32, message: &str) {
        match &self.log {
            Some(sink) => sink(level, message),
            None => eprintln!("[core:{level}] {message}"),
        }
    }
}

pub struct Session {
    pub system: GameSystem,
    pub frames: Arc<FrameBuffer>,
    /// Written by the game while the player holds the controller / handheld.
    pub pad: Arc<PadState>,
    link: Arc<Link>,
    state: AtomicU8,
    failure: Mutex<Option<Failure>>,
    stop: AtomicBool,
    paused: AtomicBool,
    fps_bits: AtomicU64,
    title: Mutex<String>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Session {
    /// Start the emulation thread; the session is `Booting` until the first frame can run.
    pub fn start(cfg: SessionConfig) -> Arc<Session> {
        let frames = Arc::new(FrameBuffer::new());
        if let Some(darkness) = cfg.scanlines {
            frames.set_scanlines(true, darkness);
        }
        let pad = Arc::new(PadState::new());
        let link = Arc::new(Link {
            frames: frames.clone(),
            pad: pad.clone(),
            ring: OnceLock::new(),
            controlled: AtomicBool::new(false),
            epoch: Instant::now(),
            first_frame: AtomicU64::new(0),
            log: cfg.log.clone(),
        });
        let session = Arc::new(Session {
            system: cfg.system,
            frames,
            pad,
            link,
            state: AtomicU8::new(State::Booting as u8),
            failure: Mutex::new(None),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            fps_bits: AtomicU64::new(0),
            title: Mutex::new(cfg.title.clone()),
            thread: Mutex::new(None),
        });
        let runner = session.clone();
        let handle = std::thread::Builder::new()
            .name(format!("{} emulator", cfg.system.machine_name()))
            .stack_size(EMU_STACK_BYTES)
            .spawn(move || runner.thread_main(cfg))
            .expect("spawn the emulation thread");
        *session.thread.lock().unwrap() = Some(handle);
        session
    }

    pub fn state(&self) -> State {
        match self.state.load(Ordering::Acquire) {
            0 => State::Booting,
            1 => State::Running,
            2 => State::Error,
            _ => State::Stopped,
        }
    }

    pub fn failure(&self) -> Option<Failure> {
        self.failure.lock().unwrap().clone()
    }

    pub fn title(&self) -> String {
        self.title.lock().unwrap().clone()
    }

    pub fn fps(&self) -> f64 {
        f64::from_bits(self.fps_bits.load(Ordering::Relaxed))
    }

    /// Time since the first picture arrived, for the TV's power-on animation.
    pub fn first_frame_elapsed(&self) -> Option<Duration> {
        match self.link.first_frame.load(Ordering::Relaxed) {
            0 => None,
            t => Some(
                self.link
                    .epoch
                    .elapsed()
                    .saturating_sub(Duration::from_nanos(t - 1)),
            ),
        }
    }

    /// The audio ring the game's sound engine pulls from; `None` until the game is running.
    pub fn audio_ring(&self) -> Option<Arc<AudioRing>> {
        self.link.ring.get().cloned()
    }

    /// Freeze the game while Minecraft is paused.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// True while the local player holds this game's controller; input only reaches a controlled game.
    pub fn set_controlled(&self, controlled: bool) {
        self.link.controlled.store(controlled, Ordering::Relaxed);
    }

    /// Ask the emulation thread to save and shut down. Doesn't block.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Wait (bounded) for the thread to finish, e.g. when leaving the world.
    pub fn join(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let handle = self.thread.lock().unwrap().take();
        let Some(h) = handle else { return };
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if h.is_finished() {
            let _ = h.join();
        } else {
            *self.thread.lock().unwrap() = Some(h);
        }
    }

    fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Release);
    }

    fn fail(&self, f: Failure) {
        *self.failure.lock().unwrap() = Some(f);
        self.set_state(State::Error);
    }

    fn log(&self, level: u32, msg: &str) {
        self.link.log(level, msg);
    }

    // ---- emulation thread ----------------------------------------------------------------------

    fn thread_main(self: Arc<Self>, cfg: SessionConfig) {
        let name = cfg.system.machine_name();
        match catch_unwind(AssertUnwindSafe(|| self.run(&cfg))) {
            Ok(Ok(())) => {}
            Ok(Err(f)) => self.fail(f),
            Err(panic) => {
                let what = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                self.log(LOG_ERROR, &format!("The {name} crashed: {what}"));
                self.fail(Failure::Crashed("panic".into()));
            }
        }
        if self.state() != State::Error {
            self.set_state(State::Stopped);
        }
    }

    fn run(&self, cfg: &SessionConfig) -> Result<(), Failure> {
        let system = cfg.system;
        let Some(core_path) = cfg.dirs.find_core(system, &cfg.core_file) else {
            return Err(Failure::NoCore(Dirs::default_core_file_name(system)));
        };
        let rom_path = match rom::find(&cfg.dirs, &cfg.rom, system) {
            Ok(Some(p)) => p,
            _ => return Err(Failure::NoRom(cfg.rom.rom_file.clone())),
        };
        if self.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let zipped = rom::load_if_zipped(&rom_path, system).map_err(|e| {
            self.log(LOG_ERROR, &format!("{e}"));
            Failure::BadRom(file_name(&rom_path))
        })?;

        // Each game gets its own copy of the core file: a library loaded twice from one path is
        // the same library, and libretro cores keep their state in globals.
        let copy = CoreCopy::acquire(&cfg.dirs, &core_path).map_err(|e| {
            self.log(
                LOG_ERROR,
                &format!(
                    "Could not copy the libretro core {}: {e}",
                    core_path.display()
                ),
            );
            Failure::BadCore(file_name(&core_path))
        })?;
        let saves = cfg.dirs.saves();
        let mut core = Core::load(
            copy.path(),
            &cfg.dirs.system(),
            &saves,
            CoreOptions::new(cfg.options.clone()),
            self.link.clone(),
            &cfg.username,
        )
        .map_err(|e| {
            self.log(
                LOG_ERROR,
                &format!(
                    "Could not load the libretro core {}: {e}",
                    core_path.display()
                ),
            );
            Failure::BadCore(file_name(&core_path))
        })?;

        let info = core.init().map_err(|e| {
            self.log(
                LOG_ERROR,
                &format!(
                    "Could not start the libretro core {}: {e}",
                    core_path.display()
                ),
            );
            Failure::BadCore(file_name(&core_path))
        })?;
        self.log(
            LOG_INFO,
            &format!(
                "Loaded core {} {} for {}",
                info.name,
                info.version,
                file_name(&rom_path)
            ),
        );

        let av = core.load_game(&rom_path, zipped).map_err(|e| {
            self.log(LOG_ERROR, &format!("{e}"));
            match e {
                CoreError::RomRefused(_) => Failure::BadRom(file_name(&rom_path)),
                _ => Failure::BadCore(file_name(&core_path)),
            }
        })?;
        self.log(
            LOG_INFO,
            &format!(
                "Game running at {}x{} {:.2} fps, {} Hz audio",
                av.base_width, av.base_height, av.fps, av.sample_rate
            ),
        );

        let save_file = saves.join(format!("{}.srm", cfg.save_key));
        let mut saver = SaveRam::new(save_file);
        saver.load(&mut core, |m| self.log(LOG_INFO, m));

        let rate = if av.sample_rate > 0.0 {
            av.sample_rate.round() as u32
        } else {
            44100
        };
        let chunk = (rate * cfg.audio_chunk_millis.max(10) / 1000).max(256) as usize;
        let ring = Arc::new(AudioRing::new(rate, 1000));
        let _ = self.link.ring.set(ring.clone());
        self.set_state(State::Running);

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.main_loop(&mut core, &mut saver, &av, &ring, chunk * 5 / 2, system)
        }));
        // Save, then unload the core, whatever happened.
        saver.write(&mut core, true, |m| self.log(LOG_WARN, m));
        drop(core);
        drop(copy);
        outcome.unwrap_or_else(|_| Err(Failure::Crashed("panic".into())))
    }

    fn main_loop(
        &self,
        core: &mut Core,
        saver: &mut SaveRam,
        av: &AvInfo,
        ring: &AudioRing,
        ring_target: usize,
        system: GameSystem,
    ) -> Result<(), Failure> {
        let fps = if av.fps > 1.0 { av.fps } else { 60.0 };
        // Nudge the frame time so the sample ring stays near its target fill.
        let frame = Duration::from_secs_f64(1.0 / fps);
        let mut next = Instant::now();
        let mut last_save = next;
        let mut fps_start = next;
        let mut fps_frames = 0u32;
        let mut total = 0u64;
        let title = self.title();

        while !self.stop.load(Ordering::Relaxed) && !core.shutdown_requested() {
            if self.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(20));
                next = Instant::now();
                continue;
            }
            core.run();
            total += 1;
            if total == 1 || total == 600 {
                let a = core.av_info().unwrap_or(*av);
                self.log(
                    LOG_INFO,
                    &format!(
                        "{title}: frame {total} done ({}x{})",
                        a.base_width, a.base_height
                    ),
                );
            }

            let now = Instant::now();
            fps_frames += 1;
            if now - fps_start >= Duration::from_secs(1) {
                let f = f64::from(fps_frames) / (now - fps_start).as_secs_f64();
                self.fps_bits.store(f.to_bits(), Ordering::Relaxed);
                fps_frames = 0;
                fps_start = now;
            }
            if now - last_save >= SAVE_INTERVAL {
                saver.write(core, false, |m| self.log(LOG_WARN, m));
                last_save = now;
            }

            let err = (ring.buffered() as f64 - ring_target as f64) / ring_target.max(1) as f64;
            next += frame.mul_f64(1.0 + 0.03 * err.clamp(-1.0, 1.0));
            let t = Instant::now();
            if next > t + Duration::from_millis(1) {
                std::thread::sleep(next - t);
            } else if t > next + frame * 5 {
                next = Instant::now(); // fell behind (slow CPU or a hitch); don't try to catch up
            }
        }
        let _ = system;
        Ok(())
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---- battery saves -----------------------------------------------------------------------------

/// Battery saves: `retro_get_memory_data(SAVE_RAM)` goes to a `.srm` file, every few seconds if it
/// changed and on stop.
struct SaveRam {
    file: PathBuf,
    last_hash: Option<u64>,
}

fn fnv1a(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl SaveRam {
    fn new(file: PathBuf) -> Self {
        SaveRam {
            file,
            last_hash: None,
        }
    }

    fn load(&mut self, core: &mut Core, log: impl Fn(&str)) {
        if core.save_ram_size() == 0 {
            return;
        }
        let Ok(data) = std::fs::read(&self.file) else {
            return;
        };
        let n = core.write_save_ram(&data);
        self.last_hash = Some(fnv1a(&core.read_save_ram()));
        log(&format!(
            "Loaded {n} bytes of save data from {}",
            self.file.file_name().unwrap_or_default().to_string_lossy()
        ));
    }

    fn write(&mut self, core: &mut Core, force: bool, warn: impl Fn(&str)) {
        if core.save_ram_size() == 0 {
            return;
        }
        let data = core.read_save_ram();
        let hash = fnv1a(&data);
        if !force && Some(hash) == self.last_hash {
            return;
        }
        if Some(hash) == self.last_hash && self.file.exists() {
            return;
        }
        let tmp = self.file.with_extension("srm.tmp");
        let result = std::fs::write(&tmp, &data).and_then(|()| std::fs::rename(&tmp, &self.file));
        match result {
            Ok(()) => self.last_hash = Some(hash),
            Err(e) => warn(&format!(
                "Could not write the save file {}: {e}",
                self.file.display()
            )),
        }
    }
}
