# ConsoleManiaCraft on Horizon (port notes)

ConsoleManiaCraft (C.M.C.) is a Fabric mod for Minecraft 26.3 that plays the player's own games on in-world
consoles: a CRT TV with the **CMC-64** (Nintendo 64, Mupen64Plus-Next), the **CMC-DS** handheld (Nintendo DS,
melonDS DS) and the **CMC-3DS** handheld (Nintendo 3DS, Azahar). The Java mod is in `epi0-hu/cmc-epi`. Java mods
don't run in Horizon, so this is a port: the same behaviour, written in Rust, inside this fork.

Rule of thumb carried over from the mod: only cores that can render in **software** fit (the frame has to arrive in
memory). Hardware-render requests are refused and Minecraft's GPU context is never shared.

## Status

| Part | State |
|---|---|
| `crates/horizon-cmc-emu`: libretro frontend, ROM library, saves, options, sessions, audio ring, Windows exception-handler guard | **Done**, 22 unit + 8 integration tests, clippy clean on Linux and `x86_64-pc-windows-gnu` |
| Real cores (Mupen64Plus-Next, melonDS DS, Azahar) | Smoke test written (`tests/real_cores.rs`), **not run yet**: `CMC_REAL_CORES=<folder with the .so/.dll cores> cargo test -p horizon-cmc-emu --test real_cores -- --nocapture` |
| Game side (items, blocks, screens, picture, sound, input) | Designed below, not started |

## The emulator crate

`horizon-cmc-emu` knows nothing about Horizon. A `Session` runs one core on its own 64 MB-stack thread:

- `session.frames` (`FrameBuffer`): the newest picture as RGBA8, with optional CRT scanlines (TV only).
  `frames.sequence()` changes with every new picture, so the renderer uploads only when it has to.
- `session.audio_ring()` (`AudioRing`): the core's stereo sound; read it as stereo or mono.
- `session.pad` (`PadState`): RetroPad buttons, two sticks and a pointer (the touch screen), written by the game.
  Input only reaches a session after `set_controlled(true)`.
- `state()`, `failure()` (no core, no ROM, bad core, bad ROM, crashed: data, not text), `fps()`, `set_paused`,
  `request_stop`, `join`.
- `RomLibrary` functions (`rom::scan`, `find`, `sha1`) for N64 `.z64/.n64/.v64`, DS `.nds/.dsi`, decrypted 3DS
  `.3ds/.cci/.cxi/.3dsx`, with titles from the N64 header, the DS banner and the 3DS SMDH.
- Battery saves: `saves/<save_key>.srm` every 5 s when changed and on stop (Azahar keeps its own files under
  `saves/Azahar/`).
- Each running game loads a private copy of its core (`cores/.running/`): libretro cores keep global state.

What changed against the Java version: callbacks are plain `extern "C"` trampolines, one set per loaded core
(`core.rs`), so cores that run on a coroutine stack (Mupen's libco) or log from their own threads (Azahar) need no
hand-assembled stubs and no stack guard. The log stub copies nothing because the trampoline reads the string while the
core's thread still owns it. The Windows exception-handler scoping is kept (`guard.rs`): melonDS registers a process-wide
first-chance handler that faults on every thread but its own.

Platforms: x86-64 for the cores themselves (the buildbot has no useful aarch64 ones). Core log lines print their
integer arguments on x86-64 only; floats print `?`.

## The game side (not started)

Everything below lives in `horizon-client` (which forbids `unsafe`), using only the safe API above.

### Picture
Horizon already reserves atlas regions that are written at run time: maps (`MAP_SLOTS` 128x128, patched through
`map_patches`), the minimap, skins, world and pack icons, the keyboard's text (`hud.rs`, `HudAtlas::slots`). The emulator
picture uses the same mechanism: reserve a few `horizon/cmc-frame<i>` slots (512x512 holds the biggest frame, Azahar's
400x480), upload with a `Patch` when `frames.sequence()` changed.

- **Handhelds**: draw the slot with `HudBatch::rect_uv` (what `native_sprite` does), cut into the two screens with
  `GameSystem::screens()` (`Screens::top_uv/bottom_uv`: DS 256x192 + 256x192; 3DS 400x240 over a 320x240 touch screen
  centred in the lower half of the frame).
- **TV**: maps in item frames are already drawn in the world from an atlas rect (`maps.rs`, `map_vertices`); the curved
  CRT glass is the same thing with a curved quad grid.

### Sound
`Audio` (`horizon-client-render/src/audio.rs`) is Horizon's own mixer over `cpal`. It needs a streaming PCM source: pulls
from an `AudioRing`, mono and positioned at the TV, stereo and unpositioned for a handheld. Pacing keeps the ring near
2.5 chunks (already done in `Session`).

### Input
The handheld screen and the controller screen poll the keyboard / SDL gamepad into `PadState` (the key maps are in the
Java mod's `CmcConfig` and are unchanged). Mouse = stylus on the touch screen: pointer coordinates span the whole frame;
for the 3DS the touch screen is `bottom_uv`'s rectangle of it. A tap must be held for about 50 ms so an emulated frame
sees it.

### Items and blocks
Blocks: CRT TV, CMC-64 console, cartridge shelf. Items: AV cable, controller, cartridges, CMC-DS, CMC-3DS and the two card
kinds. Horizon's content mods give blocks and items from data (`data/<id>/horizon/block|item`), and native mods get hooks
for item use (API 13), block entities (API 15-20) and container screens (API 19-20). Neither draws a block entity with a
live texture or opens an arbitrary full-screen UI, so these two need a small, hook-shaped addition. Whether the rest is a
content mod or built into the game is still open.

### Verification the game side needs
Touching `game.rs`/`game/` selects about 25 parity harnesses, a client-lib file about 103
(`python3 tools/parity.py --changed origin/main`), and `tools/pr_ready.py` before every PR.
