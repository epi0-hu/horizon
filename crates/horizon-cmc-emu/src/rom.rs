//! Finds, reads and identifies the player's ROM files (N64, DS and 3DS) in `consolemaniacraft/roms`.

use crate::paths::Dirs;
use crate::system::GameSystem;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::UNIX_EPOCH;

static SHA1_CACHE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

#[derive(Clone, Debug, PartialEq)]
pub struct Rom {
    pub path: PathBuf,
    pub file_name: String,
    pub title: String,
    pub size: u64,
}

/// What a flashed cartridge remembers about its game.
#[derive(Clone, Debug, PartialEq)]
pub struct RomRef {
    pub rom_file: String,
    pub sha1: String,
}

pub fn is_rom_name(name: &str, system: GameSystem) -> bool {
    let n = name.to_lowercase();
    system.rom_extensions().iter().any(|e| n.ends_with(e))
}

pub fn is_zip(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".zip"))
}

fn is_candidate(p: &Path, system: GameSystem) -> bool {
    p.is_file()
        && (is_rom_name(&p.file_name().unwrap_or_default().to_string_lossy(), system)
            || (system.zips_allowed() && is_zip(p)))
}

/// All ROMs for `system` in the roms folder, sorted by title.
pub fn scan(dirs: &Dirs, system: GameSystem) -> Vec<Rom> {
    let mut out = Vec::new();
    let Ok(files) = fs::read_dir(dirs.roms()) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = files.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if !is_candidate(&p, system) {
            continue;
        }
        let Ok(Some(title)) = title(&p, system) else {
            continue;
        }; // None: a zip without a ROM for this system
        let Ok(meta) = fs::metadata(&p) else { continue };
        let file = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let title = if title.trim().is_empty() {
            strip_extension(&file)
        } else {
            title
        };
        out.push(Rom {
            path: p,
            file_name: file,
            title,
            size: meta.len(),
        });
    }
    out.sort_by_key(|r| r.title.to_lowercase());
    out
}

/// Find the ROM a cartridge was flashed with: by file name (checking the hash), then by hash.
pub fn find(dirs: &Dirs, cart: &RomRef, system: GameSystem) -> io::Result<Option<PathBuf>> {
    let roms = dirs.roms();
    // The name must be a plain file name inside the roms folder.
    let plain = Path::new(&cart.rom_file)
        .file_name()
        .is_some_and(|n| n == Path::new(&cart.rom_file).as_os_str());
    if plain {
        let by_name = roms.join(&cart.rom_file);
        if by_name.is_file()
            && (cart.sha1.is_empty() || cart.sha1.eq_ignore_ascii_case(&sha1(&by_name, system)?))
        {
            return Ok(Some(by_name));
        }
    }
    if cart.sha1.is_empty() {
        return Ok(None);
    }
    let Ok(files) = fs::read_dir(&roms) else {
        return Ok(None);
    };
    for f in files.flatten() {
        let p = f.path();
        if is_candidate(&p, system) && cart.sha1.eq_ignore_ascii_case(&sha1(&p, system)?) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

fn first_rom_entry(zip: &mut zip::ZipArchive<File>, system: GameSystem) -> Option<usize> {
    (0..zip.len()).find(|&i| {
        zip.by_index(i)
            .is_ok_and(|e| !e.is_dir() && is_rom_name(e.name(), system))
    })
}

/// The ROM's bytes for a zip (extracted into memory); `None` for a plain ROM file, which the core
/// loader reads itself.
pub fn load_if_zipped(p: &Path, system: GameSystem) -> io::Result<Option<Vec<u8>>> {
    if !is_zip(p) || !system.zips_allowed() {
        return Ok(None);
    }
    let mut zip = zip::ZipArchive::new(File::open(p)?).map_err(io::Error::other)?;
    let idx = first_rom_entry(&mut zip, system)
        .ok_or_else(|| io::Error::other(format!("No ROM inside {}", p.display())))?;
    let mut entry = zip.by_index(idx).map_err(io::Error::other)?;
    if entry.size() > system.max_zip_rom_bytes() {
        return Err(io::Error::other("ROM is too large"));
    }
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// SHA-1 of the ROM's bytes (of the extracted ROM for zips), streamed and cached per file version.
pub fn sha1(p: &Path, system: GameSystem) -> io::Result<String> {
    let meta = fs::metadata(p)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    let key = format!(
        "{}|{}|{mtime}",
        std::path::absolute(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .display(),
        meta.len()
    );
    if let Some(h) = SHA1_CACHE.lock().unwrap().get(&key) {
        return Ok(h.clone());
    }
    let mut md = Sha1::new();
    if is_zip(p) && system.zips_allowed() {
        let mut zip = zip::ZipArchive::new(File::open(p)?).map_err(io::Error::other)?;
        let idx = first_rom_entry(&mut zip, system)
            .ok_or_else(|| io::Error::other(format!("No ROM inside {}", p.display())))?;
        feed(&mut md, zip.by_index(idx).map_err(io::Error::other)?)?;
    } else {
        feed(&mut md, File::open(p)?)?;
    }
    let hex: String = md.finalize().iter().map(|b| format!("{b:02x}")).collect();
    SHA1_CACHE.lock().unwrap().insert(key, hex.clone());
    Ok(hex)
}

fn feed(md: &mut Sha1, mut r: impl Read) -> io::Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        md.update(&buf[..n]);
    }
}

/// `len` bytes at `offset` of the ROM (inside a zip if needed); shorter at the end of the file,
/// `None` if a zip holds no ROM for this system.
fn read_range(
    p: &Path,
    system: GameSystem,
    offset: u64,
    len: usize,
) -> io::Result<Option<Vec<u8>>> {
    if is_zip(p) && system.zips_allowed() {
        let mut zip = zip::ZipArchive::new(File::open(p)?).map_err(io::Error::other)?;
        let Some(idx) = first_rom_entry(&mut zip, system) else {
            return Ok(None);
        };
        let mut entry = zip.by_index(idx).map_err(io::Error::other)?;
        io::copy(&mut entry.by_ref().take(offset), &mut io::sink())?;
        let mut buf = Vec::new();
        entry.take(len as u64).read_to_end(&mut buf)?;
        return Ok(Some(buf));
    }
    let mut f = File::open(p)?;
    let size = f.metadata()?.len();
    if offset >= size {
        return Ok(Some(Vec::new()));
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    f.take(len as u64).read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// The game's title from the ROM, "" if it has none, `None` if this isn't a ROM for `system`.
fn title(p: &Path, system: GameSystem) -> io::Result<Option<String>> {
    Ok(match system {
        GameSystem::N64 => read_range(p, system, 0, 0x40)?.map(|h| internal_title(&h)),
        GameSystem::Nds => read_range(p, system, 0, 0x200)?.map(|h| ds_title(p, &h)),
        GameSystem::N3ds => Some(three_ds_title(p)?),
    })
}

/// The 20-character game title from an N64 cartridge header, in any of the three byte orders.
pub fn internal_title(header: &[u8]) -> String {
    if header.len() < 0x34 {
        return String::new();
    }
    let mut h = header.to_vec();
    let magic = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
    match magic {
        0x3780_4012 => {
            // .v64: 16-bit byte swapped
            for c in h.chunks_exact_mut(2) {
                c.swap(0, 1);
            }
        }
        0x4012_3780 => {
            // .n64: little-endian 32-bit words
            for c in h.chunks_exact_mut(4) {
                c.reverse();
            }
        }
        0x8037_1240 => {}
        _ => return String::new(),
    }
    printable(&h[0x20..0x34])
}

/// A DS game's name: the English title from the ROM's banner ("New Super Mario Bros."), without the
/// publisher line; falls back to the 12-character header title ("NEW MARIO").
fn ds_title(p: &Path, header: &[u8]) -> String {
    let fallback = if header.len() >= 12 {
        printable(&header[..12])
    } else {
        String::new()
    };
    if header.len() < 0x6C {
        return fallback;
    }
    let banner =
        u32::from_le_bytes([header[0x68], header[0x69], header[0x6A], header[0x6B]]) as u64;
    if banner < 0x200 {
        return fallback;
    }
    match read_range(p, GameSystem::Nds, banner + 0x340, 0x100) {
        Ok(Some(english)) => {
            let t = banner_title(&english);
            if t.trim().is_empty() { fallback } else { t }
        }
        _ => fallback,
    }
}

/// Decode a 128-character UTF-16LE banner title; its last line is the publisher.
pub fn banner_title(utf16: &[u8]) -> String {
    if utf16.len() < 2 {
        return String::new();
    }
    let units: Vec<u16> = utf16
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    let raw = String::from_utf16_lossy(&units);
    let mut lines: Vec<&str> = raw
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() > 1 {
        lines.pop();
    }
    lines
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

const MEDIA_UNIT: u64 = 0x200;
/// Magic, version and the first two of the SMDH's 16 application titles (Japanese, English).
const SMDH_TITLE_BYTES: usize = 8 + 2 * 0x200;

fn has_magic(b: &[u8], at: usize, magic: &str) -> bool {
    b.get(at..at + magic.len())
        .is_some_and(|s| s == magic.as_bytes())
}

fn u16_le(b: &[u8], at: usize) -> u64 {
    u16::from_le_bytes([b[at], b[at + 1]]) as u64
}

fn u32_le(b: &[u8], at: usize) -> u64 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as u64
}

/// A 3DS game's English title from its SMDH icon data: in the ExeFS "icon" file of the first NCCH
/// partition (.3ds/.cci are an NCSD image around it, .cxi is the NCCH itself), or in a homebrew
/// .3dsx's extended header. "" when there is none or the dump is still encrypted.
fn three_ds_title(p: &Path) -> io::Result<String> {
    let sys = GameSystem::N3ds;
    let Some(mut head) = read_range(p, sys, 0, 0x200)? else {
        return Ok(String::new());
    };
    if head.len() < 0x28 {
        return Ok(String::new());
    }
    if has_magic(&head, 0, "3DSX") {
        if u16_le(&head, 4) <= 0x20 {
            return Ok(String::new()); // no extended header, so no SMDH
        }
        return Ok(smdh_title(
            read_range(p, sys, u32_le(&head, 0x20), SMDH_TITLE_BYTES)?.as_deref(),
        ));
    }
    let mut ncch = 0;
    if head.len() == 0x200 && has_magic(&head, 0x100, "NCSD") {
        ncch = u32_le(&head, 0x120) * MEDIA_UNIT;
        match read_range(p, sys, ncch, 0x200)? {
            Some(h) => head = h,
            None => return Ok(String::new()),
        }
    }
    if head.len() < 0x200 || !has_magic(&head, 0x100, "NCCH") {
        return Ok(String::new());
    }
    let no_crypto = head[0x18F] & 0x04 != 0;
    let exefs = ncch + u32_le(&head, 0x1A0) * MEDIA_UNIT;
    if !no_crypto || exefs == ncch {
        return Ok(String::new());
    }
    let Some(files) = read_range(p, sys, exefs, 10 * 16)? else {
        return Ok(String::new());
    };
    for entry in files.chunks_exact(16) {
        if entry.starts_with(b"icon\0") {
            let off = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as u64;
            return Ok(smdh_title(
                read_range(p, sys, exefs + 0x200 + off, SMDH_TITLE_BYTES)?.as_deref(),
            ));
        }
    }
    Ok(String::new())
}

/// The English short title of an SMDH (Japanese if it has no English one).
pub fn smdh_title(smdh: Option<&[u8]>) -> String {
    let Some(smdh) = smdh.filter(|s| s.len() >= SMDH_TITLE_BYTES && has_magic(s, 0, "SMDH")) else {
        return String::new();
    };
    for lang in [1usize, 0] {
        let off = 8 + lang * 0x200;
        let units: Vec<u16> = smdh[off..off + 0x80]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        let raw = String::from_utf16_lossy(&units);
        let t: String = raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    String::new()
}

fn printable(raw: &[u8]) -> String {
    let s: String = raw
        .iter()
        .map(|&c| {
            if (0x20..0x7F).contains(&c) {
                c as char
            } else {
                ' '
            }
        })
        .collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_extension(f: &str) -> String {
    match f.rfind('.') {
        Some(dot) if dot > 0 => f[..dot].to_string(),
        _ => f.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn le32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn put(b: &mut [u8], at: usize, s: &str) {
        b[at..at + s.len()].copy_from_slice(s.as_bytes());
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn smdh(english: &str, japanese: &str) -> Vec<u8> {
        let mut s = vec![0u8; 0x36C0];
        put(&mut s, 0, "SMDH");
        let (j, e) = (utf16(japanese), utf16(english));
        s[8..8 + j.len()].copy_from_slice(&j);
        s[8 + 0x200..8 + 0x200 + e.len()].copy_from_slice(&e);
        s
    }

    fn ncch(english: &str, japanese: &str, decrypted: bool) -> Vec<u8> {
        let mut n = vec![0u8; 0x400 + 0x200 + 0x200 + 0x36C0];
        put(&mut n, 0x100, "NCCH");
        if decrypted {
            n[0x18F] = 0x04;
        }
        le32(&mut n, 0x1A0, 0x400 / 0x200);
        let blocks = ((n.len() - 0x400) / 0x200) as u32;
        le32(&mut n, 0x1A4, blocks);
        put(&mut n, 0x400, ".code");
        le32(&mut n, 0x400 + 12, 0x10);
        put(&mut n, 0x400 + 16, "icon");
        le32(&mut n, 0x400 + 24, 0x200);
        le32(&mut n, 0x400 + 28, 0x36C0);
        let s = smdh(english, japanese);
        n[0x400 + 0x200 + 0x200..].copy_from_slice(&s);
        n
    }

    fn temp_dirs(name: &str) -> Dirs {
        let d = std::env::temp_dir().join(format!("cmc-emu-rom-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let dirs = Dirs::new(d);
        dirs.ensure_folders().unwrap();
        dirs
    }

    #[test]
    fn n64_titles_in_all_three_byte_orders() {
        let mut z64 = vec![0u8; 0x40];
        z64[..4].copy_from_slice(&[0x80, 0x37, 0x12, 0x40]);
        put(&mut z64, 0x20, "SUPER GAME 64");
        assert_eq!(internal_title(&z64), "SUPER GAME 64");
        let mut v64 = z64.clone();
        v64.chunks_exact_mut(2).for_each(|c| c.swap(0, 1));
        assert_eq!(internal_title(&v64), "SUPER GAME 64");
        let mut n64 = z64.clone();
        n64.chunks_exact_mut(4).for_each(|c| c.reverse());
        assert_eq!(internal_title(&n64), "SUPER GAME 64");
        assert_eq!(internal_title(&[0u8; 0x40]), "");
    }

    #[test]
    fn ds_banner_title_drops_the_publisher() {
        assert_eq!(
            banner_title(&utf16("New Super Mario Bros.\nNintendo")),
            "New Super Mario Bros."
        );
        assert_eq!(banner_title(&utf16("Only One Line")), "Only One Line");
        assert_eq!(
            banner_title(&utf16("Line A\nLine B\nPublisher")),
            "Line A Line B"
        );
    }

    #[test]
    fn three_ds_titles_from_ncsd_ncch_3dsx_and_japanese_only() {
        let dirs = temp_dirs("3ds");
        let roms = dirs.roms();
        let ncch = ncch("Fake Adventure 3D", "フェイク", true);
        let mut ncsd = vec![0u8; 0x4000 + ncch.len()];
        put(&mut ncsd, 0x100, "NCSD");
        le32(&mut ncsd, 0x120, 0x4000 / 0x200);
        ncsd[0x4000..].copy_from_slice(&ncch);
        fs::write(roms.join("adventure.3ds"), &ncsd).unwrap();
        fs::write(roms.join("adventure_cxi.cxi"), &ncch).unwrap();
        let mut locked = ncsd.clone();
        locked[0x4000 + 0x18F] = 0; // NoCrypto off: still encrypted, no title
        fs::write(roms.join("Locked Game.cci"), &locked).unwrap();
        fs::write(roms.join("japanese.cxi"), ncch_jp()).unwrap();
        let mut x = vec![0u8; 0x100 + 0x36C0];
        put(&mut x, 0, "3DSX");
        x[4] = 0x2C;
        le32(&mut x, 0x20, 0x100);
        x[0x100..].copy_from_slice(&smdh("Homebrew Thing\nby someone", ""));
        fs::write(roms.join("hb.3dsx"), &x).unwrap();
        // zips are not used for the 3DS
        let mut z = zip::ZipWriter::new(File::create(roms.join("zipped.zip")).unwrap());
        z.start_file("game.3ds", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(&ncsd).unwrap();
        z.finish().unwrap();

        let found: Vec<(String, String)> = scan(&dirs, GameSystem::N3ds)
            .into_iter()
            .map(|r| (r.file_name, r.title))
            .collect();
        assert!(found.contains(&("adventure.3ds".into(), "Fake Adventure 3D".into())));
        assert!(found.contains(&("adventure_cxi.cxi".into(), "Fake Adventure 3D".into())));
        assert!(found.contains(&("hb.3dsx".into(), "Homebrew Thing by someone".into())));
        assert!(found.contains(&("Locked Game.cci".into(), "Locked Game".into()))); // falls back to the file name
        assert!(found.contains(&("japanese.cxi".into(), "フェイク".into())));
        assert!(!found.iter().any(|(f, _)| f == "zipped.zip"));
        assert_eq!(
            load_if_zipped(&roms.join("zipped.zip"), GameSystem::N3ds).unwrap(),
            None
        );
    }

    fn ncch_jp() -> Vec<u8> {
        ncch("", "フェイク", true)
    }

    #[test]
    fn zipped_roms_hash_like_the_plain_file_and_are_found_by_hash() {
        let dirs = temp_dirs("zip");
        let mut rom = vec![0u8; 0x1000 + 0x400];
        put(&mut rom, 0, "NEW MARIO");
        rom[0x68..0x6C].copy_from_slice(&0x1000u32.to_le_bytes());
        let banner = utf16("New Super Mario Bros.\nNintendo");
        rom[0x1000 + 0x340..0x1000 + 0x340 + banner.len()].copy_from_slice(&banner);
        let nds = dirs.roms().join("nsmb.nds");
        fs::write(&nds, &rom).unwrap();
        let zp = dirs.roms().join("zipped.zip");
        let mut z = zip::ZipWriter::new(File::create(&zp).unwrap());
        z.start_file("inner/game.nds", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(&rom).unwrap();
        z.finish().unwrap();

        let sha = sha1(&nds, GameSystem::Nds).unwrap();
        assert_eq!(sha, sha1(&zp, GameSystem::Nds).unwrap());
        assert_eq!(load_if_zipped(&zp, GameSystem::Nds).unwrap().unwrap(), rom);
        let titles: Vec<String> = scan(&dirs, GameSystem::Nds)
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(titles, ["New Super Mario Bros.", "New Super Mario Bros."]);
        // renamed file: found through its hash; wrong hash under the right name is not accepted
        let by_hash = find(
            &dirs,
            &RomRef {
                rom_file: "renamed.nds".into(),
                sha1: sha.clone(),
            },
            GameSystem::Nds,
        )
        .unwrap();
        assert!(by_hash.is_some());
        assert_eq!(
            find(
                &dirs,
                &RomRef {
                    rom_file: "nsmb.nds".into(),
                    sha1: "00".into()
                },
                GameSystem::Nds
            )
            .unwrap(),
            None
        );
        assert_eq!(
            find(
                &dirs,
                &RomRef {
                    rom_file: "../evil.nds".into(),
                    sha1: String::new()
                },
                GameSystem::Nds
            )
            .unwrap(),
            None
        );
    }
}
