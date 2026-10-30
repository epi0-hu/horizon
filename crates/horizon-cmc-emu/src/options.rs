//! Core option values. The core declares its options (key, allowed values, default); for every key
//! we pick the first of our preferred values that the core actually offers, and fall back to the
//! core's own default otherwise.

use std::collections::{BTreeMap, HashMap};

/// For each option key, the preferred values in order.
pub type Prefs = BTreeMap<String, Vec<String>>;

fn prefs(entries: &[(&str, &[&str])]) -> Prefs {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
        .collect()
}

/// Mupen64Plus-Next in software: Angrylion RDP, no GPU context.
pub fn mupen_defaults() -> Prefs {
    prefs(&[
        ("mupen64plus-rdp-plugin", &["angrylion"]),
        ("mupen64plus-rsp-plugin", &["parallel", "cxd4"]),
        ("mupen64plus-angrylion-multithread", &["all threads"]),
        ("mupen64plus-angrylion-vioverlay", &["Filtered"]),
        ("mupen64plus-angrylion-sync", &["Low"]),
        ("mupen64plus-angrylion-overscan", &["enabled"]),
        (
            "mupen64plus-cpucore",
            &["dynamic_recompiler", "cached_interpreter"],
        ),
        ("mupen64plus-pak1", &["memory"]),
        ("mupen64plus-astick-deadzone", &["10"]),
        ("mupen64plus-astick-sensitivity", &["100"]),
    ])
}

/// melonDS DS keys first (software renderer, both screens stacked with no gap so the frame is
/// exactly top screen over bottom screen, touch from the pointer device, no drawn cursor, JIT
/// without fast memory, which relies on catching segfaults). Legacy melonDS and DeSmuME keys
/// follow; keys a core doesn't declare are ignored.
pub fn melonds_defaults() -> Prefs {
    prefs(&[
        ("melonds_render_mode", &["software"]),
        // Off by default: melonDS's render thread overflowed the JVM launcher's 1 MB stack. Rust
        // threads of the core get their own stacks, so this can be turned on later.
        ("melonds_threaded_renderer", &["disabled"]),
        ("melonds_console_mode", &["ds"]),
        ("melonds_boot_mode", &["direct"]),
        ("melonds_number_of_screen_layouts", &["1"]),
        ("melonds_screen_layout1", &["top-bottom"]),
        ("melonds_screen_gap", &["0"]),
        ("melonds_show_cursor", &["disabled"]),
        ("melonds_jit_enable", &["enabled"]),
        ("melonds_jit_fast_memory", &["disabled"]),
        ("melonds_mic_input", &["blow", "noise"]),
        ("melonds_mic_input_active", &["hold"]),
        ("melonds_show_unsupported_features", &["disabled"]),
        ("melonds_firmware_language", &["auto", "en"]),
        ("melonds_touch_mode", &["touch", "Touch", "auto"]),
        ("melonds_screen_layout", &["Top/Bottom"]),
        ("melonds_boot_directly", &["enabled"]),
        ("desmume_pointer_type", &["touch"]),
        ("desmume_screens_layout", &["top/bottom"]),
        ("desmume_screens_gap", &["0"]),
        ("desmume_cpu_mode", &["jit", "interpreter"]),
        ("desmume_pointer_mouse", &["disabled"]),
    ])
}

/// Azahar: the software renderer (the core's "Auto" asks for OpenGL or Vulkan, which we refuse),
/// the default top-over-bottom layout at native size so the frame is 400x480, the right stick as
/// the C-stick, and saves inside our saves folder (`saves/Azahar`).
pub fn azahar_defaults() -> Prefs {
    prefs(&[
        ("citra_graphics_api", &["Software"]),
        ("citra_layout_option", &["default"]),
        ("citra_swap_screen", &["Top"]),
        ("citra_resolution_factor", &["1"]),
        ("citra_render_3d", &["off"]),
        ("citra_use_cpu_jit", &["enabled"]),
        ("citra_use_shader_jit", &["enabled"]),
        ("citra_analog_function", &["c_stick"]),
        ("citra_enable_touch_touchscreen", &["enabled"]),
        ("citra_enable_mouse_touchscreen", &["disabled"]),
        ("citra_enable_touch_pointer_timeout", &["enabled"]),
        ("citra_enable_motion", &["disabled"]),
        ("citra_input_type", &["none"]),
        ("citra_audio_emulation", &["hle"]),
        ("citra_use_libretro_save_path", &["LibRetro Default"]),
        ("citra_use_virtual_sd", &["enabled"]),
    ])
}

#[derive(Debug, Default)]
pub struct CoreOptions {
    preferences: Prefs,
    values: HashMap<String, String>,
    updated: bool,
}

impl CoreOptions {
    pub fn new(preferences: Prefs) -> Self {
        CoreOptions {
            preferences,
            values: HashMap::new(),
            updated: false,
        }
    }

    /// A legacy `retro_variable` value: `"Description; default|other|..."`.
    pub fn define_legacy(&mut self, key: &str, spec: &str) {
        let list = spec.split_once(';').map_or(spec, |(_, l)| l).trim();
        let opts: Vec<&str> = if list.is_empty() {
            Vec::new()
        } else {
            list.split('|').collect()
        };
        if let Some(v) = self.choose(key, &opts) {
            self.values.insert(key.to_string(), v);
        }
    }

    fn choose(&self, key: &str, opts: &[&str]) -> Option<String> {
        if let Some(prefs) = self.preferences.get(key) {
            for p in prefs {
                if opts.is_empty() || opts.contains(&p.as_str()) {
                    return Some(p.clone());
                }
            }
        }
        opts.first().map(|s| s.to_string())
    }

    pub fn set_from_core(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
    }

    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(v) = self.values.get(key) {
            return Some(v.clone());
        }
        self.preferences.get(key).and_then(|p| p.first().cloned())
    }

    pub fn consume_updated(&mut self) -> bool {
        std::mem::take(&mut self.updated)
    }

    /// Our preferred settings that the core ended up with, for the log.
    pub fn resolved(&self) -> Vec<(String, String)> {
        self.preferences
            .keys()
            .filter_map(|k| self.values.get(k).map(|v| (k.clone(), v.clone())))
            .collect()
    }

    /// Problems the player should hear about: a core that isn't rendering in software.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if let Some(r) = self
            .values
            .get("melonds_render_mode")
            .filter(|r| *r != "software")
        {
            w.push(format!(
                "melonDS DS is set to the '{r}' renderer; the handheld needs 'software'."
            ));
        }
        if let Some(a) = self
            .values
            .get("citra_graphics_api")
            .filter(|a| *a != "Software")
        {
            w.push(format!(
                "Azahar is set to the '{a}' graphics API; the CMC-3DS needs 'Software'."
            ));
        }
        if let Some(r) = self
            .values
            .get("mupen64plus-rdp-plugin")
            .filter(|r| *r != "angrylion")
        {
            w.push(format!(
                "This core build does not offer the Angrylion software renderer (got '{r}'). \
                 The TV needs a software-rendered frame; please use an official Mupen64Plus-Next build."
            ));
        }
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_first_preferred_value_the_core_offers() {
        let mut o = CoreOptions::new(mupen_defaults());
        o.define_legacy("mupen64plus-rsp-plugin", "RSP; hle|cxd4");
        assert_eq!(o.get("mupen64plus-rsp-plugin").as_deref(), Some("cxd4")); // parallel not offered
        o.define_legacy("mupen64plus-rdp-plugin", "RDP; gliden64|angrylion");
        assert_eq!(
            o.get("mupen64plus-rdp-plugin").as_deref(),
            Some("angrylion")
        );
    }

    #[test]
    fn falls_back_to_the_cores_default() {
        let mut o = CoreOptions::new(Prefs::new());
        o.define_legacy("x", "Desc; first|second");
        assert_eq!(o.get("x").as_deref(), Some("first"));
        assert_eq!(o.get("unknown"), None);
    }

    #[test]
    fn warns_when_a_core_is_not_in_software() {
        let mut o = CoreOptions::new(azahar_defaults());
        o.define_legacy("citra_graphics_api", "API; Auto|Vulkan");
        assert!(o.warnings().iter().any(|w| w.contains("Azahar")));
    }
}
