//! Raw Win32 `ConPTY` `kernel32` FFI declarations, RAII handles, and command/env encoding.

use core::ffi::c_void;
use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::process::Child;
use std::ptr::{NonNull, null, null_mut};
use std::time::Duration;

use crate::pty::{PtyError, PtySpawnConfig};

pub(crate) type Handle = *mut c_void;
pub(crate) type Hpc = *mut c_void;
pub(crate) type Bool = i32;
pub(crate) type Hresult = i32;

pub(crate) const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
pub(crate) const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
pub(crate) const CREATE_SUSPENDED: u32 = 0x0000_0004;
pub(crate) const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
pub(crate) const WAIT_OBJECT_0: u32 = 0x0000_0000;
pub(crate) const WAIT_TIMEOUT: u32 = 0x0000_0102;
pub(crate) const INFINITE: u32 = 0xFFFF_FFFF;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Coord {
    pub(crate) x: i16,
    pub(crate) y: i16,
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct StartupInfoW {
    pub(crate) cb: u32,
    pub(crate) lp_reserved: *mut u16,
    pub(crate) lp_desktop: *mut u16,
    pub(crate) lp_title: *mut u16,
    pub(crate) dw_x: u32,
    pub(crate) dw_y: u32,
    pub(crate) dw_x_size: u32,
    pub(crate) dw_y_size: u32,
    pub(crate) dw_x_count_chars: u32,
    pub(crate) dw_y_count_chars: u32,
    pub(crate) dw_fill_attribute: u32,
    pub(crate) dw_flags: u32,
    pub(crate) w_show_window: u16,
    pub(crate) cb_reserved2: u16,
    pub(crate) lp_reserved2: *mut u8,
    pub(crate) h_std_input: Handle,
    pub(crate) h_std_output: Handle,
    pub(crate) h_std_error: Handle,
}

#[repr(C)]
pub(crate) struct StartupInfoExW {
    pub(crate) startup_info: StartupInfoW,
    pub(crate) lp_attribute_list: *mut c_void,
}

#[repr(C)]
pub(crate) struct ProcessInformation {
    pub(crate) h_process: Handle,
    pub(crate) h_thread: Handle,
    pub(crate) dw_process_id: u32,
    pub(crate) dw_thread_id: u32,
}

#[link(name = "kernel32")]
// SAFETY: Kernel32 Win32 APIs declared with exact C ABI signatures and called under checked invariants.
unsafe extern "system" {
    pub(crate) fn CreatePipe(
        h_read_pipe: *mut Handle,
        h_write_pipe: *mut Handle,
        lp_pipe_attributes: *const c_void,
        n_size: u32,
    ) -> Bool;
    pub(crate) fn CreatePseudoConsole(
        size: Coord,
        h_input: Handle,
        h_output: Handle,
        dw_flags: u32,
        ph_pc: *mut Hpc,
    ) -> Hresult;
    pub(crate) fn ResizePseudoConsole(h_pc: Hpc, size: Coord) -> Hresult;
    pub(crate) fn ClosePseudoConsole(h_pc: Hpc);
    fn InitializeProcThreadAttributeList(
        lp_attribute_list: *mut c_void,
        dw_attribute_count: u32,
        dw_flags: u32,
        lp_size: *mut usize,
    ) -> Bool;
    fn UpdateProcThreadAttribute(
        lp_attribute_list: *mut c_void,
        dw_flags: u32,
        attribute: usize,
        lp_value: *const c_void,
        cb_size: usize,
        lp_previous_value: *mut c_void,
        lp_return_size: *mut usize,
    ) -> Bool;
    fn DeleteProcThreadAttributeList(lp_attribute_list: *mut c_void);
    pub(crate) fn CreateProcessW(
        lp_application_name: *const u16,
        lp_command_line: *mut u16,
        lp_process_attributes: *const c_void,
        lp_thread_attributes: *const c_void,
        b_inherit_handles: Bool,
        dw_creation_flags: u32,
        lp_environment: *const c_void,
        lp_current_directory: *const u16,
        lp_startup_info: *mut StartupInfoExW,
        lp_process_information: *mut ProcessInformation,
    ) -> Bool;
    pub(crate) fn WaitForSingleObject(h_handle: Handle, dw_milliseconds: u32) -> u32;
    pub(crate) fn GetExitCodeProcess(h_process: Handle, lp_exit_code: *mut u32) -> Bool;
    pub(crate) fn ResumeThread(h_thread: Handle) -> u32;
    pub(crate) fn TerminateProcess(h_process: Handle, u_exit_code: u32) -> Bool;
}

pub(crate) fn coord_from(cols: u16, rows: u16) -> Result<Coord, PtyError> {
    let x = i16::try_from(cols).map_err(|_| PtyError::InvalidDimensions { cols, rows })?;
    let y = i16::try_from(rows).map_err(|_| PtyError::InvalidDimensions { cols, rows })?;
    Ok(Coord { x, y })
}

pub(crate) fn create_pipe_pair() -> Result<(OwnedHandle, OwnedHandle), PtyError> {
    let mut read_raw: Handle = null_mut();
    let mut write_raw: Handle = null_mut();
    // SAFETY: `read_raw` and `write_raw` are valid stack pointers for `CreatePipe`.
    if unsafe { CreatePipe(&raw mut read_raw, &raw mut write_raw, null(), 0) } == 0 {
        return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
    }
    // SAFETY: On `CreatePipe` success, both handles are newly allocated, non-null, and uniquely owned.
    unsafe {
        Ok((
            OwnedHandle::from_raw_handle(read_raw),
            OwnedHandle::from_raw_handle(write_raw),
        ))
    }
}

#[derive(Debug)]
pub(crate) struct PseudoConsoleHandle(pub(crate) NonNull<c_void>);

// SAFETY: Windows HPCON handles are process-global opaque handles serialized by `Mutex`.
unsafe impl Send for PseudoConsoleHandle {}
// SAFETY: Access to `PseudoConsoleHandle` is always serialized through a `Mutex`.
unsafe impl Sync for PseudoConsoleHandle {}

impl PseudoConsoleHandle {
    pub(crate) const fn as_ptr(&self) -> Hpc {
        self.0.as_ptr()
    }
}

impl Drop for PseudoConsoleHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` was created by `CreatePseudoConsole` and is closed once.
        unsafe { ClosePseudoConsole(self.0.as_ptr()) };
    }
}

pub(crate) struct AttrList {
    _buf: Vec<usize>,
    pub(crate) ptr: *mut c_void,
}

impl AttrList {
    pub(crate) fn new_pseudoconsole(h_pc: &PseudoConsoleHandle) -> Result<Self, PtyError> {
        let mut size = 0_usize;
        // SAFETY: Query required byte size (`lp_attribute_list` is intentionally null).
        let _ = unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &raw mut size) };
        if size == 0 {
            return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
        }
        let words = size.div_ceil(size_of::<usize>());
        let mut buf = vec![0_usize; words];
        let ptr = buf.as_mut_ptr().cast::<c_void>();
        // SAFETY: `buf` is pointer-aligned and provides at least `size` writable bytes.
        if unsafe { InitializeProcThreadAttributeList(ptr, 1, 0, &raw mut size) } == 0 {
            return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
        }
        let guard = Self { _buf: buf, ptr };
        // SAFETY: `guard.ptr` is initialized and `h_pc.as_ptr()` is passed directly as HPCON value.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                guard.ptr,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                h_pc.as_ptr(),
                size_of::<Hpc>(),
                null_mut(),
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(PtyError::SpawnFailed(io::Error::last_os_error()));
        }
        Ok(guard)
    }
}

impl Drop for AttrList {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` was initialized by `InitializeProcThreadAttributeList`.
        unsafe { DeleteProcThreadAttributeList(self.ptr) };
    }
}

fn push_quoted_win_arg(out: &mut String, arg: &OsStr) -> Result<(), PtyError> {
    let text = arg.to_str().ok_or_else(|| {
        PtyError::SpawnFailed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "argument contains invalid UTF-8/surrogate codepoints",
        ))
    })?;
    if text.contains('\0') {
        return Err(PtyError::SpawnFailed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "argument contains interior NUL byte",
        )));
    }
    let needs_quotes = text.is_empty() || text.chars().any(|c| matches!(c, ' ' | '\t' | '"'));
    if !needs_quotes {
        out.push_str(text);
        return Ok(());
    }
    out.push('"');
    let mut backslashes = 0_usize;
    for ch in text.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..=(backslashes * 2) {
                    out.push('\\');
                }
                backslashes = 0;
                out.push('"');
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(ch);
            }
        }
    }
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    Ok(())
}

pub(crate) fn build_command_line(config: &PtySpawnConfig) -> Result<Vec<u16>, PtyError> {
    let mut line = String::new();
    push_quoted_win_arg(&mut line, &config.program)?;
    for arg in &config.args {
        line.push(' ');
        push_quoted_win_arg(&mut line, arg)?;
    }
    let mut wide: Vec<u16> = OsStr::new(&line).encode_wide().collect();
    wide.push(0);
    Ok(wide)
}

pub(crate) fn build_env_block(config: &PtySpawnConfig) -> Vec<u16> {
    let mut pairs: Vec<(&str, &str)> = config
        .env
        .as_map()
        .iter()
        .filter(|(k, v)| !k.is_empty() && !k.contains(['=', '\0']) && !v.contains('\0'))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    pairs.sort_by_key(|a| a.0.to_ascii_uppercase());
    let mut block = Vec::new();
    for (k, v) in pairs {
        block.extend(k.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(v.encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

pub(crate) fn wait_child_timeout(child: &Child, timeout: Duration) -> io::Result<bool> {
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: `child.as_raw_handle()` is borrowed from a live `Child` handle.
    match unsafe { WaitForSingleObject(child.as_raw_handle(), ms) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}
