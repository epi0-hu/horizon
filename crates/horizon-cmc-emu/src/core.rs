//! A minimal libretro frontend: loads a core library, answers its environment requests and carries
//! video, audio and input between the core and a [`Frontend`].
//!
//! libretro callbacks are plain C function pointers with no user-data argument, so each loaded core
//! gets a *slot* with its own set of pre-built trampolines (`t_env::<N>` ...) that look the core's
//! state up by slot number. They work on any thread and any stack, so cores that run on coroutines
//! with heap stacks (Mupen64Plus-Next's libco) or log from threads of their own (Azahar) need
//! nothing special.
//!
//! A [`Core`] must be created and used on one thread (the emulation thread); callbacks arrive
//! on it, or, for logging, on whichever thread the core likes.

use crate::guard;
use crate::options::CoreOptions;
use crate::retro::*;
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_uint, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Receives the core's output. Called on the emulation thread (log lines: any thread). Must not panic.
pub trait Frontend: Send + Sync + 'static {
    fn video(&self, data: &[u8], width: u32, height: u32, pitch: usize, format: PixelFormat);
    fn audio(&self, interleaved_stereo: &[i16]);
    fn input(&self, port: u32, device: u32, index: u32, id: u32) -> i16;
    fn log(&self, level: u32, message: &str) {
        eprintln!("[core:{level}] {message}");
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SystemInfo {
    pub name: String,
    pub version: String,
    pub extensions: String,
    pub need_fullpath: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvInfo {
    pub base_width: u32,
    pub base_height: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub aspect: f32,
    pub fps: f64,
    pub sample_rate: f64,
}

impl From<AvInfoRaw> for AvInfo {
    fn from(a: AvInfoRaw) -> Self {
        AvInfo {
            base_width: a.geometry.base_width,
            base_height: a.geometry.base_height,
            max_width: a.geometry.max_width,
            max_height: a.geometry.max_height,
            aspect: a.geometry.aspect_ratio,
            fps: a.fps,
            sample_rate: a.sample_rate,
        }
    }
}

#[derive(Debug)]
pub enum CoreError {
    Load(String),
    MissingSymbol(&'static str),
    ApiVersion(u32),
    /// The core said no to the ROM; carries its last warning or error line, if any.
    RomRefused(Option<String>),
    Io(std::io::Error),
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Load(e) => write!(f, "could not load the core: {e}"),
            CoreError::MissingSymbol(s) => write!(f, "core is missing libretro symbol {s}"),
            CoreError::ApiVersion(v) => write!(f, "unsupported libretro API version {v}"),
            CoreError::RomRefused(Some(m)) => write!(f, "the core refused the ROM: {m}"),
            CoreError::RomRefused(None) => write!(f, "the core refused the ROM"),
            CoreError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CoreError {}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Io(e)
    }
}

// ---- per-core shared state and the slot table --------------------------------------------------

struct Strings {
    system_dir: CString,
    save_dir: CString,
    lib_path: CString,
    username: CString,
}

struct Shared {
    frontend: Arc<dyn Frontend>,
    options: Mutex<CoreOptions>,
    pixel_format: AtomicU32,
    av: Mutex<Option<AvInfo>>,
    shutdown: AtomicBool,
    last_message: Mutex<Option<String>>,
    strings: Strings,
    /// Strings handed to the core for option values; pointers stay valid until the core is gone.
    value_cache: Mutex<Vec<CString>>,
    log_fn: LogFn,
}

const SLOTS: usize = 16;
static REGISTRY: [Mutex<Option<Arc<Shared>>>; SLOTS] = [const { Mutex::new(None) }; SLOTS];

fn slot(n: usize) -> Option<Arc<Shared>> {
    REGISTRY[n].lock().ok()?.clone()
}

/// Run a callback body without letting a panic unwind into C.
fn guarded<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

unsafe fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the core passes NUL-terminated strings.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

unsafe extern "C" fn t_env<const N: usize>(cmd: c_uint, data: *mut c_void) -> bool {
    guarded(false, || match slot(N) {
        // SAFETY: `data` is what the libretro API says it is for `cmd`.
        Some(s) => unsafe { s.environment(cmd, data) },
        None => false,
    })
}

unsafe extern "C" fn t_video<const N: usize>(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    guarded((), || {
        // NULL = "duplicate the previous frame"; -1 = a hardware-rendered frame, which we refuse.
        if data.is_null() || data as isize == -1 || width == 0 || height == 0 {
            return;
        }
        let Some(s) = slot(N) else { return };
        let format = PixelFormat::from_raw(s.pixel_format.load(Ordering::Relaxed))
            .unwrap_or(PixelFormat::Rgb1555);
        // SAFETY: the core guarantees `height` rows of `pitch` bytes at `data` for the call.
        let bytes =
            unsafe { std::slice::from_raw_parts(data as *const u8, pitch * height as usize) };
        s.frontend.video(bytes, width, height, pitch, format);
    })
}

unsafe extern "C" fn t_audio_sample<const N: usize>(left: i16, right: i16) {
    guarded((), || {
        if let Some(s) = slot(N) {
            s.frontend.audio(&[left, right]);
        }
    })
}

unsafe extern "C" fn t_audio_batch<const N: usize>(data: *const i16, frames: usize) -> usize {
    guarded(frames, || {
        if !data.is_null() && frames > 0 {
            if let Some(s) = slot(N) {
                // SAFETY: `frames` interleaved stereo frames at `data` for the call.
                s.frontend
                    .audio(unsafe { std::slice::from_raw_parts(data, frames * 2) });
            }
        }
        frames
    })
}

unsafe extern "C" fn t_input_poll() {}

unsafe extern "C" fn t_input_state<const N: usize>(
    port: c_uint,
    device: c_uint,
    index: c_uint,
    id: c_uint,
) -> i16 {
    guarded(0, || {
        slot(N).map_or(0, |s| s.frontend.input(port, device, index, id))
    })
}

unsafe extern "C" fn t_log<const N: usize>(
    level: c_uint,
    fmt: *const c_char,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) {
    guarded((), || {
        let Some(s) = slot(N) else { return };
        // SAFETY: a NUL-terminated format string.
        let Some(f) = (unsafe { cstr(fmt) }) else {
            return;
        };
        let text = if cfg!(target_arch = "x86_64") {
            crate::format::format_c(&f, &[a0, a1, a2, a3, a4, a5], &|p| unsafe {
                cstr(p as *const c_char)
            })
        } else {
            f
        };
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if level >= LOG_WARN {
            *s.last_message.lock().unwrap() = Some(text.to_string());
        }
        s.frontend.log(level, text);
    })
}

unsafe extern "C" fn ret_true(_: c_uint, _: *mut c_void) -> bool {
    true
}

unsafe extern "C" fn rumble(_: c_uint, _: c_uint, _: u16) -> bool {
    true
}

struct Trampolines {
    env: EnvironmentFn,
    video: VideoRefreshFn,
    audio_sample: AudioSampleFn,
    audio_batch: AudioBatchFn,
    input_poll: InputPollFn,
    input_state: InputStateFn,
    log: LogFn,
}

macro_rules! trampolines {
    ($n:expr; $($i:literal)*) => {
        match $n {
            $($i => Trampolines {
                env: t_env::<$i>,
                video: t_video::<$i>,
                audio_sample: t_audio_sample::<$i>,
                audio_batch: t_audio_batch::<$i>,
                input_poll: t_input_poll,
                input_state: t_input_state::<$i>,
                log: t_log::<$i>,
            },)*
            _ => unreachable!("slot {} out of range", $n),
        }
    };
}

fn trampolines(n: usize) -> Trampolines {
    trampolines!(n; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
}

impl Shared {
    fn warn(&self, msg: String) {
        self.frontend.log(LOG_WARN, &msg);
    }

    fn cache(&self, s: &str) -> *const c_char {
        let c = CString::new(s.replace('\0', "")).unwrap_or_default();
        let mut cache = self.value_cache.lock().unwrap();
        // Same text, same pointer: cores compare option values by address now and then.
        if let Some(existing) = cache.iter().find(|e| **e == c) {
            return existing.as_ptr();
        }
        let p = c.as_ptr();
        cache.push(c);
        p
    }

    /// Answer one `retro_environment_t` request.
    ///
    /// # Safety
    /// `data` must be what libretro.h specifies for `cmd`.
    unsafe fn environment(&self, cmd: u32, data: *mut c_void) -> bool {
        let has = !data.is_null();
        // SAFETY (all blocks below): writes/reads of the type libretro.h defines for `cmd`.
        match cmd {
            ENV_GET_LOG_INTERFACE => {
                if !has {
                    return false;
                }
                unsafe { *(data as *mut LogFn) = self.log_fn };
                true
            }
            ENV_GET_CLEAR_ALL_THREAD_WAITS_CB => {
                if !has {
                    return false;
                }
                unsafe { *(data as *mut SimpleBoolFn) = ret_true };
                true
            }
            ENV_GET_RUMBLE_INTERFACE => {
                if !has {
                    return false;
                }
                unsafe { *(data as *mut RumbleFn) = rumble };
                true
            }
            ENV_GET_CAN_DUPE => {
                if has {
                    unsafe { *(data as *mut bool) = true };
                }
                true
            }
            ENV_GET_OVERSCAN => {
                if has {
                    unsafe { *(data as *mut bool) = false };
                }
                true
            }
            ENV_GET_SYSTEM_DIRECTORY | ENV_GET_CORE_ASSETS_DIRECTORY => {
                self.put_str(data, self.strings.system_dir.as_ptr())
            }
            ENV_GET_SAVE_DIRECTORY => self.put_str(data, self.strings.save_dir.as_ptr()),
            ENV_GET_LIBRETRO_PATH => self.put_str(data, self.strings.lib_path.as_ptr()),
            ENV_GET_USERNAME => self.put_str(data, self.strings.username.as_ptr()),
            ENV_SET_PIXEL_FORMAT => {
                if !has {
                    return false;
                }
                let fmt = unsafe { *(data as *const u32) };
                if PixelFormat::from_raw(fmt).is_none() {
                    return false;
                }
                self.pixel_format.store(fmt, Ordering::Relaxed);
                true
            }
            ENV_GET_VARIABLE => {
                if !has {
                    return false;
                }
                let var = unsafe { &mut *(data as *mut Variable) };
                let value =
                    unsafe { cstr(var.key) }.and_then(|k| self.options.lock().unwrap().get(&k));
                match value {
                    Some(v) => {
                        var.value = self.cache(&v);
                        true
                    }
                    None => {
                        var.value = std::ptr::null();
                        false
                    }
                }
            }
            ENV_SET_VARIABLES => {
                if !has {
                    return false;
                }
                let mut p = data as *const Variable;
                let mut opts = self.options.lock().unwrap();
                loop {
                    let var = unsafe { &*p };
                    let Some(key) = (unsafe { cstr(var.key) }) else {
                        break;
                    };
                    opts.define_legacy(&key, &unsafe { cstr(var.value) }.unwrap_or_default());
                    p = unsafe { p.add(1) };
                }
                for (k, v) in opts.resolved() {
                    self.frontend
                        .log(LOG_INFO, &format!("Core option {k} = {v}"));
                }
                for w in opts.warnings() {
                    self.warn(w);
                }
                true
            }
            ENV_SET_VARIABLE => {
                if !has {
                    return true; // "is SET_VARIABLE supported?" query
                }
                let var = unsafe { &*(data as *const Variable) };
                if let (Some(k), Some(v)) = (unsafe { cstr(var.key) }, unsafe { cstr(var.value) }) {
                    self.options.lock().unwrap().set_from_core(&k, &v);
                }
                true
            }
            ENV_GET_VARIABLE_UPDATE => {
                if has {
                    unsafe {
                        *(data as *mut bool) = self.options.lock().unwrap().consume_updated()
                    };
                }
                true
            }
            // Version 0 makes cores fall back to the plain SET_VARIABLES list, which is all we need.
            ENV_GET_CORE_OPTIONS_VERSION
            | ENV_GET_LANGUAGE
            | ENV_GET_MESSAGE_INTERFACE_VERSION
            | ENV_GET_DISK_CONTROL_INTERFACE_VERSION => self.put_u32(data, 0),
            ENV_GET_INPUT_MAX_USERS => self.put_u32(data, 1),
            ENV_GET_AUDIO_VIDEO_ENABLE => self.put_u32(data, 0b11),
            ENV_GET_FASTFORWARDING => {
                if has {
                    unsafe { *(data as *mut bool) = false };
                }
                true
            }
            ENV_GET_TARGET_REFRESH_RATE => {
                if has {
                    unsafe { *(data as *mut f32) = 60.0 };
                }
                true
            }
            ENV_GET_JIT_CAPABLE => {
                if has {
                    unsafe { *(data as *mut bool) = true };
                }
                true
            }
            ENV_GET_INPUT_BITMASKS => true,
            ENV_SET_GEOMETRY => {
                if has {
                    let g = unsafe { *(data as *const Geometry) };
                    if let Some(av) = self.av.lock().unwrap().as_mut() {
                        av.base_width = g.base_width;
                        av.base_height = g.base_height;
                        av.max_width = g.max_width;
                        av.max_height = g.max_height;
                        av.aspect = g.aspect_ratio;
                    }
                }
                true
            }
            ENV_SET_SYSTEM_AV_INFO => {
                if has {
                    *self.av.lock().unwrap() = Some(unsafe { *(data as *const AvInfoRaw) }.into());
                }
                true
            }
            ENV_SET_MESSAGE | ENV_SET_MESSAGE_EXT => {
                if has {
                    // Both structs start with the message text.
                    if let Some(m) = unsafe { cstr(*(data as *const *const c_char)) }
                        .filter(|m| !m.trim().is_empty())
                    {
                        *self.last_message.lock().unwrap() = Some(m.clone());
                        self.frontend.log(LOG_INFO, &m);
                    }
                }
                true
            }
            ENV_SHUTDOWN => {
                self.shutdown.store(true, Ordering::Relaxed);
                true
            }
            ENV_SET_ROTATION
            | ENV_SET_PERFORMANCE_LEVEL
            | ENV_SET_INPUT_DESCRIPTORS
            | ENV_SET_SUPPORT_NO_GAME
            | ENV_SET_SUBSYSTEM_INFO
            | ENV_SET_CONTROLLER_INFO
            | ENV_SET_MEMORY_MAPS
            | ENV_SET_SUPPORT_ACHIEVEMENTS
            | ENV_SET_SERIALIZATION_QUIRKS
            | ENV_SET_CORE_OPTIONS_DISPLAY
            | ENV_SET_MINIMUM_AUDIO_LATENCY
            | ENV_SET_CONTENT_INFO_OVERRIDE
            | ENV_SET_FASTFORWARDING_OVERRIDE
            | ENV_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK => true,
            // Deliberately unsupported: hardware rendering (we want the software renderer), VFS,
            // perf counters, LEDs, keyboard, disks, poll-type override.
            _ => false,
        }
    }

    fn put_str(&self, data: *mut c_void, s: *const c_char) -> bool {
        if data.is_null() {
            return false;
        }
        // SAFETY: the request's data is a `const char **`.
        unsafe { *(data as *mut *const c_char) = s };
        true
    }

    fn put_u32(&self, data: *mut c_void, v: u32) -> bool {
        if !data.is_null() {
            // SAFETY: the request's data is an `unsigned *`.
            unsafe { *(data as *mut u32) = v };
        }
        true
    }
}

// ---- the core ----------------------------------------------------------------------------------

type VoidFn = unsafe extern "C" fn();
type SetFn<T> = unsafe extern "C" fn(T);

struct Api {
    init: VoidFn,
    deinit: VoidFn,
    api_version: unsafe extern "C" fn() -> c_uint,
    get_system_info: SetFn<*mut SystemInfoRaw>,
    get_system_av_info: SetFn<*mut AvInfoRaw>,
    set_environment: SetFn<EnvironmentFn>,
    set_video_refresh: SetFn<VideoRefreshFn>,
    set_audio_sample: SetFn<AudioSampleFn>,
    set_audio_sample_batch: SetFn<AudioBatchFn>,
    set_input_poll: SetFn<InputPollFn>,
    set_input_state: SetFn<InputStateFn>,
    set_controller_port_device: unsafe extern "C" fn(c_uint, c_uint),
    reset: VoidFn,
    run: VoidFn,
    load_game: unsafe extern "C" fn(*const GameInfo) -> bool,
    unload_game: VoidFn,
    get_memory_data: unsafe extern "C" fn(c_uint) -> *mut c_void,
    get_memory_size: unsafe extern "C" fn(c_uint) -> usize,
}

pub struct Core {
    api: Api,
    tramps: Trampolines,
    shared: Arc<Shared>,
    slot: usize,
    library_path: PathBuf,
    initialized: bool,
    game_loaded: bool,
    system_info: Option<SystemInfo>,
    /// The ROM as given to the core (it may treat the buffer as scratch) and the path string.
    rom_data: Vec<u8>,
    rom_path: Option<CString>,
    /// Must outlive everything above that points into the library; dropped last.
    library: Option<Library>,
}

impl Core {
    /// Load the core library at `library_path` (normally a private copy, see `instances`).
    pub fn load(
        library_path: &Path,
        system_dir: &Path,
        save_dir: &Path,
        options: CoreOptions,
        frontend: Arc<dyn Frontend>,
        username: &str,
    ) -> Result<Core, CoreError> {
        // SAFETY: loading a core runs its initialisers; that is the point of a core.
        let library =
            unsafe { Library::new(library_path) }.map_err(|e| CoreError::Load(e.to_string()))?;
        let api = Api {
            init: sym(&library, "retro_init", b"retro_init\0")?,
            deinit: sym(&library, "retro_deinit", b"retro_deinit\0")?,
            api_version: sym(&library, "retro_api_version", b"retro_api_version\0")?,
            get_system_info: sym(
                &library,
                "retro_get_system_info",
                b"retro_get_system_info\0",
            )?,
            get_system_av_info: sym(
                &library,
                "retro_get_system_av_info",
                b"retro_get_system_av_info\0",
            )?,
            set_environment: sym(
                &library,
                "retro_set_environment",
                b"retro_set_environment\0",
            )?,
            set_video_refresh: sym(
                &library,
                "retro_set_video_refresh",
                b"retro_set_video_refresh\0",
            )?,
            set_audio_sample: sym(
                &library,
                "retro_set_audio_sample",
                b"retro_set_audio_sample\0",
            )?,
            set_audio_sample_batch: sym(
                &library,
                "retro_set_audio_sample_batch",
                b"retro_set_audio_sample_batch\0",
            )?,
            set_input_poll: sym(&library, "retro_set_input_poll", b"retro_set_input_poll\0")?,
            set_input_state: sym(
                &library,
                "retro_set_input_state",
                b"retro_set_input_state\0",
            )?,
            set_controller_port_device: sym(
                &library,
                "retro_set_controller_port_device",
                b"retro_set_controller_port_device\0",
            )?,
            reset: sym(&library, "retro_reset", b"retro_reset\0")?,
            run: sym(&library, "retro_run", b"retro_run\0")?,
            load_game: sym(&library, "retro_load_game", b"retro_load_game\0")?,
            unload_game: sym(&library, "retro_unload_game", b"retro_unload_game\0")?,
            get_memory_data: sym(
                &library,
                "retro_get_memory_data",
                b"retro_get_memory_data\0",
            )?,
            get_memory_size: sym(
                &library,
                "retro_get_memory_size",
                b"retro_get_memory_size\0",
            )?,
        };

        let to_c =
            |p: &Path| CString::new(p.to_string_lossy().replace('\0', "")).unwrap_or_default();
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let username = if username.trim().is_empty() {
            "Steve"
        } else {
            username
        };

        // Claim a slot and build the state its trampolines read.
        let mut options = Some(options);
        let mut frontend = Some(frontend);
        for n in 0..SLOTS {
            let mut g = REGISTRY[n].lock().unwrap();
            if g.is_some() {
                continue;
            }
            let tramps = trampolines(n);
            let shared = Arc::new(Shared {
                frontend: frontend.take().expect("the frontend is taken once"),
                options: Mutex::new(options.take().expect("options are taken once")),
                pixel_format: AtomicU32::new(0),
                av: Mutex::new(None),
                shutdown: AtomicBool::new(false),
                last_message: Mutex::new(None),
                strings: Strings {
                    system_dir: to_c(&abs(system_dir)),
                    save_dir: to_c(&abs(save_dir)),
                    lib_path: to_c(&abs(library_path)),
                    username: CString::new(username.replace('\0', "")).unwrap_or_default(),
                },
                value_cache: Mutex::new(Vec::new()),
                log_fn: tramps.log,
            });
            *g = Some(shared.clone());
            return Ok(Core {
                api,
                tramps,
                shared,
                slot: n,
                library_path: library_path.to_path_buf(),
                initialized: false,
                game_loaded: false,
                system_info: None,
                rom_data: Vec::new(),
                rom_path: None,
                library: Some(library),
            });
        }
        Err(CoreError::Load(format!(
            "more than {SLOTS} cores are running at once"
        )))
    }

    pub fn library_path(&self) -> &Path {
        &self.library_path
    }

    /// Install the callbacks and call `retro_init`.
    pub fn init(&mut self) -> Result<SystemInfo, CoreError> {
        // SAFETY: calling the core's libretro entry points in the order the API prescribes, with
        // callbacks that match the signatures it expects.
        unsafe {
            let v = (self.api.api_version)();
            if v != API_VERSION {
                return Err(CoreError::ApiVersion(v));
            }
            // Windows: keep the core's exception handlers on this thread (melonDS's crashed the JVM
            // and would crash anything that raises access violations on purpose).
            if let Some(lib) = &self.library {
                guard::install_for_library(lib);
            }
            (self.api.set_environment)(self.tramps.env);
            (self.api.set_video_refresh)(self.tramps.video);
            (self.api.set_audio_sample)(self.tramps.audio_sample);
            (self.api.set_audio_sample_batch)(self.tramps.audio_batch);
            (self.api.set_input_poll)(self.tramps.input_poll);
            (self.api.set_input_state)(self.tramps.input_state);
            (self.api.init)();
            self.initialized = true;

            let mut raw = std::mem::MaybeUninit::<SystemInfoRaw>::zeroed();
            (self.api.get_system_info)(raw.as_mut_ptr());
            let raw = raw.assume_init();
            let info = SystemInfo {
                name: cstr(raw.library_name).unwrap_or_default(),
                version: cstr(raw.library_version).unwrap_or_default(),
                extensions: cstr(raw.valid_extensions).unwrap_or_default(),
                need_fullpath: raw.need_fullpath,
            };
            self.system_info = Some(info.clone());
            Ok(info)
        }
    }

    /// Load a game. `rom` is the ROM's bytes (a zip's extracted contents); with `None` the file is
    /// read here. Cores that ask for a path only (need_fullpath, like Azahar with its multi-gigabyte
    /// 3DS games) get the path and no data at all.
    pub fn load_game(
        &mut self,
        rom_path: &Path,
        rom: Option<Vec<u8>>,
    ) -> Result<AvInfo, CoreError> {
        let need_fullpath = self.system_info.as_ref().is_some_and(|i| i.need_fullpath);
        let path_only = need_fullpath && rom.is_none();
        let bytes = match rom {
            Some(r) => r,
            None if path_only => Vec::new(),
            None => std::fs::read(rom_path)?,
        };
        let size = bytes.len();
        self.rom_data = bytes;
        if !path_only && self.rom_data.is_empty() {
            // A core that wants the data may touch the buffer even for an empty ROM: never hand it NULL.
            self.rom_data = vec![0];
        }
        let abs = std::path::absolute(rom_path).unwrap_or_else(|_| rom_path.to_path_buf());
        let path = CString::new(abs.to_string_lossy().replace('\0', "")).unwrap_or_default();
        let info = GameInfo {
            path: path.as_ptr(),
            data: if path_only {
                std::ptr::null()
            } else {
                self.rom_data.as_ptr() as *const c_void
            },
            size,
            meta: std::ptr::null(),
        };
        self.rom_path = Some(path);
        // SAFETY: `info` and the buffers it points to live for the call (and the core may keep the
        // data pointer, so `rom_data` stays in `self`).
        let ok = unsafe { (self.api.load_game)(&info) };
        if !ok {
            let msg = self.shared.last_message.lock().unwrap().clone();
            return Err(CoreError::RomRefused(msg));
        }
        self.game_loaded = true;
        // SAFETY: plain libretro calls after a successful load.
        let av = unsafe {
            (self.api.set_controller_port_device)(0, DEVICE_JOYPAD);
            let mut av = AvInfoRaw::default();
            (self.api.get_system_av_info)(&mut av);
            av
        };
        let av: AvInfo = av.into();
        *self.shared.av.lock().unwrap() = Some(av);
        Ok(av)
    }

    /// Run one frame: the core calls back with video, audio and input.
    pub fn run(&mut self) {
        // SAFETY: a game is loaded; the core runs on this thread.
        unsafe { (self.api.run)() }
    }

    pub fn reset(&mut self) {
        // SAFETY: as `run`.
        unsafe { (self.api.reset)() }
    }

    /// The current geometry and timing (it changes when the core calls SET_GEOMETRY).
    pub fn av_info(&self) -> Option<AvInfo> {
        *self.shared.av.lock().unwrap()
    }

    pub fn system_info(&self) -> Option<&SystemInfo> {
        self.system_info.as_ref()
    }

    /// The core asked to be shut down (RETRO_ENVIRONMENT_SHUTDOWN).
    pub fn shutdown_requested(&self) -> bool {
        self.shared.shutdown.load(Ordering::Relaxed)
    }

    /// The last warning, error or message line the core printed.
    pub fn last_message(&self) -> Option<String> {
        self.shared.last_message.lock().unwrap().clone()
    }

    /// Size of the core's battery-backed memory (EEPROM / SRAM / FlashRAM / Controller Pak), 0 if none.
    pub fn save_ram_size(&self) -> usize {
        // SAFETY: valid after init.
        let size = unsafe { (self.api.get_memory_size)(MEMORY_SAVE_RAM) };
        // SAFETY: as above.
        if size == 0 || unsafe { (self.api.get_memory_data)(MEMORY_SAVE_RAM) }.is_null() {
            0
        } else {
            size
        }
    }

    /// A copy of the core's save memory.
    pub fn read_save_ram(&self) -> Vec<u8> {
        let size = self.save_ram_size();
        if size == 0 {
            return Vec::new();
        }
        // SAFETY: the core owns `size` bytes at the pointer while the game is loaded.
        unsafe {
            std::slice::from_raw_parts(
                (self.api.get_memory_data)(MEMORY_SAVE_RAM) as *const u8,
                size,
            )
        }
        .to_vec()
    }

    /// Copy `data` into the core's save memory (as much as fits); returns the bytes copied.
    pub fn write_save_ram(&mut self, data: &[u8]) -> usize {
        let size = self.save_ram_size();
        let n = size.min(data.len());
        if n > 0 {
            // SAFETY: as `read_save_ram`; `n` bytes are inside both buffers.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    (self.api.get_memory_data)(MEMORY_SAVE_RAM) as *mut u8,
                    n,
                )
            };
        }
        n
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        // SAFETY: the reverse of load_game and init.
        unsafe {
            if self.game_loaded {
                (self.api.unload_game)();
            }
            if self.initialized {
                (self.api.deinit)();
            }
        }
        // From here on callbacks (from threads the core hasn't joined) find nothing and do nothing.
        if let Ok(mut g) = REGISTRY[self.slot].lock() {
            *g = None;
        }
        guard::forget_current_thread();
        self.library.take(); // unload the library last
    }
}

fn sym<T: Copy>(lib: &Library, name: &'static str, raw: &[u8]) -> Result<T, CoreError> {
    // SAFETY: the type `T` is the libretro signature of `name`.
    unsafe { lib.get::<T>(raw) }
        .map(|s| *s)
        .map_err(|_| CoreError::MissingSymbol(name))
}
