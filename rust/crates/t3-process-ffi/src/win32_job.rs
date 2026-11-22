//! Quarantined Win32 Job Object and process-metrics `kernel32` FFI bindings (`BUG-09`).
//!
//! All `unsafe` Win32 `extern "system"` calls are isolated inside this module behind
//! safe RAII wrappers ([`JobHandle`]) and `pub(crate)` safe process-telemetry helpers.

use std::io;
use std::process::Child;

use thiserror::Error;

/// Errors returned by [`JobHandle`] setup, process assignment, or termination.
#[derive(Debug, Error)]
pub enum Win32JobError {
    /// Win32 Job Objects are unavailable on non-Windows targets.
    #[error("Win32 Job Objects are only supported on Windows")]
    UnsupportedPlatform,
    /// `CreateJobObjectW` failed to allocate a kernel job handle.
    #[error("CreateJobObjectW failed: {0}")]
    CreateJobFailed(#[source] io::Error),
    /// `SetInformationJobObject` failed to apply `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    #[error("SetInformationJobObject failed: {0}")]
    SetLimitFailed(#[source] io::Error),
    /// `OpenProcess` failed when opening a target PID for job assignment.
    #[error("OpenProcess failed for pid {0}: {1}")]
    OpenProcessFailed(u32, #[source] io::Error),
    /// `AssignProcessToJobObject` failed to bind the process into the job.
    #[error("AssignProcessToJobObject failed: {0}")]
    AssignProcessFailed(#[source] io::Error),
    /// `TerminateJobObject` failed to terminate the job's process tree.
    #[error("TerminateJobObject failed: {0}")]
    TerminateJobFailed(#[source] io::Error),
}

#[cfg(windows)]
mod imp {
    use core::ffi::c_void;
    use std::io;
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::Child;
    use std::ptr::{NonNull, null, null_mut};

    use super::Win32JobError;
    use crate::resource_monitor_sampler::Win32ProcessMetrics;

    type Handle = *mut c_void;
    type Bool = i32;

    const JOB_KILL_ON_CLOSE: u32 = 0x0000_2000;
    const JOB_EXT_CLASS: i32 = 9;
    const PROCESS_ASSIGN_RIGHTS: u32 = 0x0100 | 0x0001;
    const PROCESS_QUERY_LIMITED_INFO: u32 = 0x1000;
    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const WIN32_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct JobBasicLimitInfo {
        time_limits: [i64; 2],
        limit_flags: u32,
        working_sets: [usize; 2],
        active_limit: u32,
        affinity: usize,
        classes: [u32; 2],
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct IoCounters {
        op_counts: [u64; 3],
        read_transfer_bytes: u64,
        write_transfer_bytes: u64,
        other_transfer_bytes: u64,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct JobExtendedLimitInfo {
        basic: JobBasicLimitInfo,
        io_info: IoCounters,
        memory_limits: [usize; 4],
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct ProcessMemCountersEx {
        header: [u32; 2],
        working_sets: [usize; 2],
        pool_and_pagefile: [usize; 6],
        private_usage: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        module_and_threads: [u32; 2],
        th32_parent_process_id: u32,
        pri_and_flags: [u32; 2],
        sz_exe_file: [u16; 260],
    }

    // SAFETY: Standard Win32 kernel32 ABI bindings; callers validate all handles and pointers.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attr: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(h: Handle, cls: i32, info: *const c_void, len: u32) -> Bool;
        fn AssignProcessToJobObject(h_job: Handle, h_proc: Handle) -> Bool;
        fn TerminateJobObject(h_job: Handle, code: u32) -> Bool;
        fn CloseHandle(h: Handle) -> Bool;
        fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> Handle;
        fn GetCurrentProcess() -> Handle;
        fn GetProcessTimes(
            h: Handle,
            c: *mut FileTime,
            e: *mut FileTime,
            k: *mut FileTime,
            u: *mut FileTime,
        ) -> Bool;
        fn K32GetProcessMemoryInfo(h: Handle, mem: *mut ProcessMemCountersEx, cb: u32) -> Bool;
        fn GetProcessIoCounters(h: Handle, io: *mut IoCounters) -> Bool;
        fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, len: *mut u32) -> Bool;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
        fn Process32FirstW(snap: Handle, lppe: *mut ProcessEntry32W) -> Bool;
        fn Process32NextW(snap: Handle, lppe: *mut ProcessEntry32W) -> Bool;
    }

    fn take_handle(raw: Handle) -> Option<OwnedHandle> {
        let invalid = std::ptr::without_provenance_mut::<c_void>(usize::MAX);
        if raw.is_null() || raw == invalid {
            None
        } else {
            // SAFETY: `raw` is non-null, not `INVALID_HANDLE_VALUE`, and newly returned by Win32.
            Some(unsafe { OwnedHandle::from_raw_handle(raw) })
        }
    }

    fn last_err<T>(ctor: fn(io::Error) -> Win32JobError) -> Result<T, Win32JobError> {
        Err(ctor(io::Error::last_os_error()))
    }

    /// RAII wrapper around a Win32 kernel Job Object configured with `KILL_ON_JOB_CLOSE`.
    #[derive(Debug)]
    pub struct JobHandle {
        raw: NonNull<c_void>,
    }

    // SAFETY: Win32 kernel HANDLEs are process-wide tokens safe to transfer across threads.
    unsafe impl Send for JobHandle {}

    // SAFETY: Kernel job operations (`AssignProcessToJobObject`, `TerminateJobObject`) are thread-safe.
    unsafe impl Sync for JobHandle {}

    impl JobHandle {
        /// Creates a new anonymous Win32 Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
        pub fn new_kill_on_close() -> Result<Self, Win32JobError> {
            // SAFETY: Null attributes and null name allocate an anonymous Win32 job handle.
            let Some(raw) = NonNull::new(unsafe { CreateJobObjectW(null_mut(), null()) }) else {
                return last_err(Win32JobError::CreateJobFailed);
            };
            let job = Self { raw };
            let mut info = JobExtendedLimitInfo::default();
            info.basic.limit_flags = JOB_KILL_ON_CLOSE;
            let len = u32::try_from(size_of::<JobExtendedLimitInfo>()).unwrap_or(0);
            let ptr = (&raw const info).cast::<c_void>();
            let jh = job.raw.as_ptr();
            // SAFETY: `jh` is a live job handle and `ptr` points to initialized `info`.
            if unsafe { SetInformationJobObject(jh, JOB_EXT_CLASS, ptr, len) } == 0 {
                return last_err(Win32JobError::SetLimitFailed);
            }
            Ok(job)
        }

        /// Assigns an existing `std::process::Child` handle to this Win32 Job Object.
        pub fn assign_child(&self, child: &Child) -> Result<(), Win32JobError> {
            // SAFETY: `self.raw` is live and `child.as_raw_handle()` is borrowed from a live `Child`.
            if unsafe { AssignProcessToJobObject(self.raw.as_ptr(), child.as_raw_handle()) } == 0 {
                return last_err(Win32JobError::AssignProcessFailed);
            }
            Ok(())
        }

        /// Opens `pid` with quota+terminate rights and assigns it to this Win32 Job Object.
        pub fn assign_pid(&self, pid: u32) -> Result<(), Win32JobError> {
            // SAFETY: `OpenProcess` accepts integer flags and returns NULL on failure.
            let Some(proc_h) = take_handle(unsafe { OpenProcess(PROCESS_ASSIGN_RIGHTS, 0, pid) })
            else {
                return Err(Win32JobError::OpenProcessFailed(
                    pid,
                    io::Error::last_os_error(),
                ));
            };
            // SAFETY: `self.raw` and `proc_h` are both live kernel handles.
            if unsafe { AssignProcessToJobObject(self.raw.as_ptr(), proc_h.as_raw_handle()) } == 0 {
                return last_err(Win32JobError::AssignProcessFailed);
            }
            Ok(())
        }

        /// Terminates all processes in this job object directly inside the Windows kernel.
        pub fn terminate(&self, exit_code: u32) -> Result<(), Win32JobError> {
            // SAFETY: `self.raw` is a live Win32 Job Object handle owned by `self`.
            if unsafe { TerminateJobObject(self.raw.as_ptr(), exit_code) } == 0 {
                return last_err(Win32JobError::TerminateJobFailed);
            }
            Ok(())
        }

        /// Returns the exact ABI struct size of `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`.
        pub const fn extended_limit_info_size() -> usize {
            size_of::<JobExtendedLimitInfo>()
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {
            // SAFETY: `self.raw` is exclusively owned by `self` and closed once via `CloseHandle`.
            unsafe {
                let _ = CloseHandle(self.raw.as_ptr());
            }
        }
    }

    fn ft_to_100ns(ft: FileTime) -> u64 {
        (u64::from(ft.high) << 32) | u64::from(ft.low)
    }

    pub(crate) fn snapshot_process_table() -> Vec<(u32, u32, String)> {
        // SAFETY: `CreateToolhelp32Snapshot` returns a snapshot handle or `INVALID_HANDLE_VALUE`.
        let Some(snap) = take_handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
        else {
            return Vec::new();
        };
        let mut entry = MaybeUninit::<ProcessEntry32W>::zeroed();
        let sz = u32::try_from(size_of::<ProcessEntry32W>()).unwrap_or(0);
        // SAFETY: `entry` is zeroed with `dw_size` initialized before `Process32FirstW`.
        if unsafe {
            (*entry.as_mut_ptr()).dw_size = sz;
            Process32FirstW(snap.as_raw_handle(), entry.as_mut_ptr())
        } == 0
        {
            return Vec::new();
        }
        let mut rows = Vec::new();
        loop {
            // SAFETY: `Process32FirstW` / `Process32NextW` succeeded and initialized `entry`.
            let cur = unsafe { entry.assume_init() };
            if cur.th32_process_id != 0 {
                let len = cur
                    .sz_exe_file
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(cur.sz_exe_file.len());
                let name = String::from_utf16_lossy(&cur.sz_exe_file[..len]);
                rows.push((cur.th32_process_id, cur.th32_parent_process_id, name));
            }
            // SAFETY: `snap` is live and `entry` points to `ProcessEntry32W`.
            if unsafe { Process32NextW(snap.as_raw_handle(), entry.as_mut_ptr()) } == 0 {
                break;
            }
        }
        rows
    }

    pub(crate) fn query_process_metrics(pid: u32) -> Option<Win32ProcessMetrics> {
        let owned = if pid == std::process::id() {
            None
        } else {
            // SAFETY: `OpenProcess` validates `pid` and returns NULL when inaccessible.
            Some(take_handle(unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFO, 0, pid)
            })?)
        };
        // SAFETY: `GetCurrentProcess` returns the pseudo-handle for the calling process.
        let ph = owned.as_ref().map_or_else(
            || unsafe { GetCurrentProcess() },
            AsRawHandle::as_raw_handle,
        );
        let [mut ct, mut ex, mut kt, mut ut] = [FileTime::default(); 4];
        // SAFETY: `ph` is valid and `ct`/`ex`/`kt`/`ut` are distinct stack `FILETIME` locals.
        if unsafe { GetProcessTimes(ph, &raw mut ct, &raw mut ex, &raw mut kt, &raw mut ut) } == 0 {
            return None;
        }
        let cb = u32::try_from(size_of::<ProcessMemCountersEx>()).unwrap_or(0);
        let mut mem = ProcessMemCountersEx {
            header: [cb, 0],
            ..Default::default()
        };
        // SAFETY: `ph` is valid and `mem.header[0]` holds exact struct byte size.
        let (rss, vmem) = if unsafe { K32GetProcessMemoryInfo(ph, &raw mut mem, cb) } != 0 {
            (mem.working_sets[1] as u64, mem.private_usage as u64)
        } else {
            (0, 0)
        };
        let mut io_c = IoCounters::default();
        // SAFETY: `ph` is valid and `io_c` refers to a stack local.
        let (io_r, io_w) = if unsafe { GetProcessIoCounters(ph, &raw mut io_c) } != 0 {
            (io_c.read_transfer_bytes, io_c.write_transfer_bytes)
        } else {
            (0, 0)
        };
        let mut buf = [0u16; 1024];
        let mut len = u32::try_from(buf.len()).unwrap_or(0);
        // SAFETY: `ph` is valid and `buf` has capacity `len` UTF-16 code units.
        let ok = unsafe { QueryFullProcessImageNameW(ph, 0, buf.as_mut_ptr(), &raw mut len) } != 0;
        let exe_path = (ok && len > 0).then(|| {
            String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())])
        });
        Some(Win32ProcessMetrics {
            start_time_ms: ft_to_100ns(ct).saturating_sub(WIN32_UNIX_EPOCH_100NS) / 10_000,
            cpu_time_ms: ft_to_100ns(kt).saturating_add(ft_to_100ns(ut)) / 10_000,
            resident_bytes: rss,
            virtual_bytes: vmem,
            io_read_bytes: io_r,
            io_write_bytes: io_w,
            exe_path,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use std::process::Child;

    use super::Win32JobError;

    fn err<T>() -> Result<T, Win32JobError> {
        Err(Win32JobError::UnsupportedPlatform)
    }

    /// Non-Windows placeholder handle returning [`Win32JobError::UnsupportedPlatform`].
    #[derive(Debug)]
    pub struct JobHandle(());

    impl JobHandle {
        pub fn new_kill_on_close() -> Result<Self, Win32JobError> {
            err()
        }
        pub fn assign_child(&self, _: &Child) -> Result<(), Win32JobError> {
            err()
        }
        pub fn assign_pid(&self, _: u32) -> Result<(), Win32JobError> {
            err()
        }
        pub fn terminate(&self, _: u32) -> Result<(), Win32JobError> {
            err()
        }
        pub const fn extended_limit_info_size() -> usize {
            0
        }
    }

    impl Drop for JobHandle {
        fn drop(&mut self) {}
    }
}

pub use imp::JobHandle;
#[cfg(windows)]
pub(crate) use imp::{query_process_metrics, snapshot_process_table};

/// Returns `true` when Win32 kernel Job Objects are supported on this build target.
pub const fn is_supported() -> bool {
    cfg!(windows)
}

/// Assigns an existing child process to a fresh kill-on-close [`JobHandle`] when supported.
pub fn supervise_existing_child(child: &Child) -> Result<JobHandle, Win32JobError> {
    let job = JobHandle::new_kill_on_close()?;
    job.assign_child(child)?;
    Ok(job)
}
