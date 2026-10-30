//! Keeps a core's Windows exception handlers on the emulation thread.
//!
//! melonDS registers a process-wide *first* vectored exception handler (for its JIT's fast memory)
//! whose first action is to dereference `NDS::Current`, a `thread_local` that is null on every
//! thread but the emulator's. Nothing else raises access violations in RetroArch, but a managed
//! runtime (the JVM did, and the Rust game's own guard pages and stack probes do) raises them on its
//! own threads, and the handler then faults inside itself until the thread's stack is gone
//! (STATUS_STACK_OVERFLOW, 0xC00000FD).
//!
//! The fix: patch the core's import-table slots for `AddVectoredExceptionHandler` and
//! `RemoveVectoredExceptionHandler`. A handler the core registers is kept in a list of the
//! *registering thread*, and one handler of ours (registered once for the process) forwards an
//! exception to the list of the thread it happened on. On every other thread the list is empty, so
//! the answer is EXCEPTION_CONTINUE_SEARCH and the exception goes on to its real owner.
//!
//! Linux would need the same for melonDS's SIGSEGV `sigaction`; that is not done.

/// Parsing a mapped PE image (RVAs are offsets into the slice). Platform independent.
pub mod pe {
    fn u16_at(b: &[u8], o: u64) -> Option<u16> {
        let o = usize::try_from(o).ok()?;
        Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
    }

    fn u32_at(b: &[u8], o: u64) -> Option<u32> {
        let o = usize::try_from(o).ok()?;
        Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    }

    fn u64_at(b: &[u8], o: u64) -> Option<u64> {
        let o = usize::try_from(o).ok()?;
        Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
    }

    /// Offset of the PE32+ optional header, if `image` starts with a 64-bit PE.
    fn optional_header(image: &[u8]) -> Option<u64> {
        if u16_at(image, 0)? != 0x5A4D {
            return None; // "MZ"
        }
        let nt = u32_at(image, 0x3C)? as u64;
        if u32_at(image, nt)? != 0x0000_4550 {
            return None; // "PE\0\0"
        }
        let opt = nt + 24;
        (u16_at(image, opt)? == 0x20B).then_some(opt)
    }

    pub fn size_of_image(image: &[u8]) -> Option<u64> {
        Some(u32_at(image, optional_header(image)? + 56)? as u64)
    }

    fn c_string_equals(image: &[u8], offset: u64, s: &str) -> bool {
        let Ok(start) = usize::try_from(offset) else {
            return false;
        };
        let want = s.as_bytes();
        image
            .get(start..start + want.len() + 1)
            .is_some_and(|got| &got[..want.len()] == want && got[want.len()] == 0)
    }

    /// RVA of the import address table slot for `function` (imported by name), if any.
    pub fn find_import_slot(image: &[u8], function: &str) -> Option<u64> {
        let opt = optional_header(image)?;
        let import_rva = u32_at(image, opt + 112 + 8)? as u64;
        if import_rva == 0 {
            return None;
        }
        let mut d = import_rva;
        loop {
            let original_thunk = u32_at(image, d)? as u64;
            let name_rva = u32_at(image, d + 12)?;
            let first_thunk = u32_at(image, d + 16)? as u64;
            if name_rva == 0 && first_thunk == 0 {
                return None;
            }
            d += 20;
            if original_thunk == 0 {
                continue; // names already overwritten by the loader
            }
            let mut i = 0;
            loop {
                let entry = u64_at(image, original_thunk + i * 8)?;
                if entry == 0 {
                    break;
                }
                if entry >> 63 == 0 && c_string_equals(image, (entry & 0x7FFF_FFFF) + 2, function) {
                    return Some(first_thunk + i * 8);
                }
                i += 1;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A tiny mapped image importing `Other` and `AddVectoredExceptionHandler` from one DLL.
        fn image() -> Vec<u8> {
            let mut b = vec![0u8; 0x800];
            let put32 =
                |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
            let put64 =
                |b: &mut Vec<u8>, o: usize, v: u64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
            b[0..2].copy_from_slice(b"MZ");
            put32(&mut b, 0x3C, 0x80);
            b[0x80..0x84].copy_from_slice(b"PE\0\0");
            let opt = 0x80 + 24;
            b[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
            put32(&mut b, opt + 56, 0x800); // SizeOfImage
            put32(&mut b, opt + 112 + 8, 0x200); // import directory RVA
            // descriptor 0
            put32(&mut b, 0x200, 0x300); // OriginalFirstThunk
            put32(&mut b, 0x200 + 12, 0x600); // Name
            put32(&mut b, 0x200 + 16, 0x500); // FirstThunk
            // names
            put64(&mut b, 0x300, 0x400);
            put64(&mut b, 0x308, 0x420);
            put64(&mut b, 0x310, 1 << 63 | 7); // by ordinal
            b[0x402..0x402 + 6].copy_from_slice(b"Other\0");
            let n = b"AddVectoredExceptionHandler\0";
            b[0x422..0x422 + n.len()].copy_from_slice(n);
            b
        }

        #[test]
        fn finds_the_slot_of_a_named_import() {
            let img = image();
            assert_eq!(size_of_image(&img), Some(0x800));
            assert_eq!(find_import_slot(&img, "Other"), Some(0x500));
            assert_eq!(
                find_import_slot(&img, "AddVectoredExceptionHandler"),
                Some(0x508)
            );
        }

        #[test]
        fn missing_function_and_garbage_are_none() {
            let img = image();
            assert_eq!(find_import_slot(&img, "NoSuchFunction"), None);
            assert_eq!(find_import_slot(&img, "Add"), None); // prefixes don't match
            assert_eq!(find_import_slot(&[0u8; 64], "x"), None);
            assert_eq!(size_of_image(b"MZ"), None);
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn install_for_library(_lib: &libloading::Library) {}

#[cfg(not(windows))]
pub(crate) fn forget_current_thread() {}

#[cfg(windows)]
pub(crate) use win::{forget_current_thread, install_for_library};

#[cfg(windows)]
mod win {
    use super::pe;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_POINTERS, PVECTORED_EXCEPTION_HANDLER,
    };
    use windows_sys::Win32::System::Memory::{
        MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualProtect, VirtualQuery,
    };

    const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

    thread_local! {
        /// (fake handle, handler) the core registered from this thread.
        static HANDLERS: RefCell<Vec<(usize, PVECTORED_EXCEPTION_HANDLER)>> = const { RefCell::new(Vec::new()) };
    }
    static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(1);
    /// Our one real handler, registered the first time a core registers one.
    static SCOPED: OnceLock<usize> = OnceLock::new();

    unsafe extern "system" fn scoped_handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        HANDLERS
            .try_with(|h| {
                let Ok(list) = h.try_borrow() else { return 0 };
                for (_, f) in list.iter() {
                    if let Some(f) = f {
                        // SAFETY: the handler the core itself registered, called on its own thread.
                        if unsafe { f(info) } == EXCEPTION_CONTINUE_EXECUTION {
                            return EXCEPTION_CONTINUE_EXECUTION;
                        }
                    }
                }
                0 // EXCEPTION_CONTINUE_SEARCH
            })
            .unwrap_or(0)
    }

    unsafe extern "system" fn hook_add(
        _first: u32,
        handler: PVECTORED_EXCEPTION_HANDLER,
    ) -> *mut c_void {
        SCOPED.get_or_init(|| {
            // SAFETY: registering a handler with the OS.
            unsafe { AddVectoredExceptionHandler(1, Some(scoped_handler)) as usize }
        });
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let _ = HANDLERS.try_with(|h| h.borrow_mut().push((handle, handler)));
        handle as *mut c_void
    }

    unsafe extern "system" fn hook_remove(handle: *mut c_void) -> u32 {
        let handle = handle as usize;
        let removed = HANDLERS.try_with(|h| {
            let mut list = h.borrow_mut();
            let before = list.len();
            list.retain(|(hd, _)| *hd != handle);
            before != list.len()
        });
        u32::from(removed.unwrap_or(false))
    }

    /// Drop this thread's handlers (their code is about to be unloaded with the core).
    pub(crate) fn forget_current_thread() {
        let _ = HANDLERS.try_with(|h| h.borrow_mut().clear());
    }

    /// Patch the core's import slots. Quietly does nothing when the core doesn't import them.
    pub(crate) fn install_for_library(lib: &libloading::Library) {
        // SAFETY: reading the mapped module's headers and import table, and rewriting two IAT slots.
        unsafe {
            let Ok(sym) = lib.get::<*const c_void>(b"retro_init\0") else {
                return;
            };
            let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            if VirtualQuery(
                *sym,
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            ) == 0
            {
                return;
            }
            let base = mbi.AllocationBase as *const u8;
            let Some(size) = pe::size_of_image(std::slice::from_raw_parts(base, 4096)) else {
                return;
            };
            let image = std::slice::from_raw_parts(base, size as usize);
            if let Some(slot) = pe::find_import_slot(image, "AddVectoredExceptionHandler") {
                patch(base, slot, hook_add as *const () as usize);
                if let Some(slot) = pe::find_import_slot(image, "RemoveVectoredExceptionHandler") {
                    patch(base, slot, hook_remove as *const () as usize);
                }
            }
        }
    }

    unsafe fn patch(base: *const u8, rva: u64, target: usize) {
        // SAFETY: `rva` is an import address table slot inside the loaded image.
        unsafe {
            let slot = base.add(rva as usize) as *mut usize;
            let mut old = 0u32;
            if VirtualProtect(slot as *const c_void, 8, PAGE_READWRITE, &mut old) != 0 {
                slot.write(target);
                VirtualProtect(slot as *const c_void, 8, old, &mut old);
            }
        }
    }
}
