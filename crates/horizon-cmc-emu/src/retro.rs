//! The parts of libretro.h we use: constants and the C structs the core reads and writes.

use std::ffi::{c_char, c_uint, c_void};

pub const API_VERSION: c_uint = 1;

pub const ENV_EXPERIMENTAL: u32 = 0x10000;

pub const ENV_SET_ROTATION: u32 = 1;
pub const ENV_GET_OVERSCAN: u32 = 2;
pub const ENV_GET_CAN_DUPE: u32 = 3;
pub const ENV_SET_MESSAGE: u32 = 6;
pub const ENV_SHUTDOWN: u32 = 7;
pub const ENV_SET_PERFORMANCE_LEVEL: u32 = 8;
pub const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
pub const ENV_SET_PIXEL_FORMAT: u32 = 10;
pub const ENV_SET_INPUT_DESCRIPTORS: u32 = 11;
pub const ENV_GET_VARIABLE: u32 = 15;
pub const ENV_SET_VARIABLES: u32 = 16;
pub const ENV_GET_VARIABLE_UPDATE: u32 = 17;
pub const ENV_SET_SUPPORT_NO_GAME: u32 = 18;
pub const ENV_GET_LIBRETRO_PATH: u32 = 19;
pub const ENV_GET_RUMBLE_INTERFACE: u32 = 23;
pub const ENV_GET_LOG_INTERFACE: u32 = 27;
pub const ENV_GET_CORE_ASSETS_DIRECTORY: u32 = 30;
pub const ENV_GET_SAVE_DIRECTORY: u32 = 31;
pub const ENV_SET_SYSTEM_AV_INFO: u32 = 32;
pub const ENV_SET_SUBSYSTEM_INFO: u32 = 34;
pub const ENV_SET_CONTROLLER_INFO: u32 = 35;
pub const ENV_SET_MEMORY_MAPS: u32 = 36 | ENV_EXPERIMENTAL;
pub const ENV_SET_GEOMETRY: u32 = 37;
pub const ENV_GET_USERNAME: u32 = 38;
pub const ENV_GET_LANGUAGE: u32 = 39;
pub const ENV_SET_SUPPORT_ACHIEVEMENTS: u32 = 42 | ENV_EXPERIMENTAL;
pub const ENV_SET_SERIALIZATION_QUIRKS: u32 = 44;
pub const ENV_GET_AUDIO_VIDEO_ENABLE: u32 = 47 | ENV_EXPERIMENTAL;
pub const ENV_GET_FASTFORWARDING: u32 = 49 | ENV_EXPERIMENTAL;
pub const ENV_GET_TARGET_REFRESH_RATE: u32 = 50 | ENV_EXPERIMENTAL;
pub const ENV_GET_INPUT_BITMASKS: u32 = 51 | ENV_EXPERIMENTAL;
pub const ENV_GET_CORE_OPTIONS_VERSION: u32 = 52;
pub const ENV_SET_CORE_OPTIONS_DISPLAY: u32 = 55;
pub const ENV_GET_DISK_CONTROL_INTERFACE_VERSION: u32 = 57;
pub const ENV_GET_MESSAGE_INTERFACE_VERSION: u32 = 59;
pub const ENV_SET_MESSAGE_EXT: u32 = 60;
pub const ENV_GET_INPUT_MAX_USERS: u32 = 61;
pub const ENV_SET_MINIMUM_AUDIO_LATENCY: u32 = 63;
pub const ENV_SET_FASTFORWARDING_OVERRIDE: u32 = 64;
pub const ENV_SET_CONTENT_INFO_OVERRIDE: u32 = 65;
pub const ENV_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK: u32 = 69;
pub const ENV_SET_VARIABLE: u32 = 70;
pub const ENV_GET_JIT_CAPABLE: u32 = 74;
pub const ENV_GET_CLEAR_ALL_THREAD_WAITS_CB: u32 = 3 | ENV_EXPERIMENTAL;

pub const LOG_DEBUG: c_uint = 0;
pub const LOG_INFO: c_uint = 1;
pub const LOG_WARN: c_uint = 2;
pub const LOG_ERROR: c_uint = 3;

pub const MEMORY_SAVE_RAM: c_uint = 0;

pub const DEVICE_JOYPAD: c_uint = 1;
pub const DEVICE_ANALOG: c_uint = 5;
pub const DEVICE_POINTER: c_uint = 6;
pub const ANALOG_INDEX_LEFT: c_uint = 0;
pub const ANALOG_INDEX_RIGHT: c_uint = 1;
pub const ANALOG_X: c_uint = 0;
pub const ANALOG_Y: c_uint = 1;
pub const POINTER_X: c_uint = 0;
pub const POINTER_Y: c_uint = 1;
pub const POINTER_PRESSED: c_uint = 2;
pub const POINTER_COUNT: c_uint = 3;
pub const JOYPAD_MASK: c_uint = 256;

/// RetroPad button ids (bit i of [`crate::PadState`]'s button mask).
pub mod pad {
    pub const B: u32 = 0;
    pub const Y: u32 = 1;
    pub const SELECT: u32 = 2;
    pub const START: u32 = 3;
    pub const UP: u32 = 4;
    pub const DOWN: u32 = 5;
    pub const LEFT: u32 = 6;
    pub const RIGHT: u32 = 7;
    pub const A: u32 = 8;
    pub const X: u32 = 9;
    pub const L: u32 = 10;
    pub const R: u32 = 11;
    pub const L2: u32 = 12;
    pub const R2: u32 = 13;
    pub const L3: u32 = 14;
    pub const R3: u32 = 15;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb1555,
    Xrgb8888,
    Rgb565,
}

impl PixelFormat {
    pub fn from_raw(v: u32) -> Option<PixelFormat> {
        match v {
            0 => Some(PixelFormat::Rgb1555),
            1 => Some(PixelFormat::Xrgb8888),
            2 => Some(PixelFormat::Rgb565),
            _ => None,
        }
    }
}

/// `retro_game_info`
#[repr(C)]
pub struct GameInfo {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

/// `retro_system_info`
#[repr(C)]
pub struct SystemInfoRaw {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

/// `retro_game_geometry`
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Geometry {
    pub base_width: c_uint,
    pub base_height: c_uint,
    pub max_width: c_uint,
    pub max_height: c_uint,
    pub aspect_ratio: f32,
}

/// `retro_system_av_info`
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct AvInfoRaw {
    pub geometry: Geometry,
    pub fps: f64,
    pub sample_rate: f64,
}

/// `retro_variable`
#[repr(C)]
pub struct Variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

pub type EnvironmentFn = unsafe extern "C" fn(c_uint, *mut c_void) -> bool;
pub type VideoRefreshFn = unsafe extern "C" fn(*const c_void, c_uint, c_uint, usize);
pub type AudioSampleFn = unsafe extern "C" fn(i16, i16);
pub type AudioBatchFn = unsafe extern "C" fn(*const i16, usize) -> usize;
pub type InputPollFn = unsafe extern "C" fn();
pub type InputStateFn = unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16;
/// `retro_log_printf_t` is variadic in C. On x86-64 the extra integer arguments arrive in the same
/// registers and stack slots as plain integer parameters, so the log callback takes six of them.
pub type LogFn =
    unsafe extern "C" fn(c_uint, *const c_char, usize, usize, usize, usize, usize, usize);
pub type SimpleBoolFn = unsafe extern "C" fn(c_uint, *mut c_void) -> bool;
pub type RumbleFn = unsafe extern "C" fn(c_uint, c_uint, u16) -> bool;
