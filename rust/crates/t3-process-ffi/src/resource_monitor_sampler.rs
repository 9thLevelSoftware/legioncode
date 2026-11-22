//! Lightweight OS process sampler for protocol v3 without subprocess spawning.
//!
//! Reads real process metrics directly from OS APIs (`kernel32` via [`crate::win32_job`]
//! on Windows, `/proc` on Linux, and `std::process`/`std::env` fallback on other targets).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::resource_monitor_events::ProcessTableEntryV3;
use crate::resource_monitor_history::MAX_PROCESS_NAME_BYTES;
use crate::resource_monitor_history::truncate_utf8;
#[cfg(any(windows, target_os = "linux"))]
use crate::resource_monitor_history::unix_time_ms;
#[cfg(not(windows))]
use crate::resource_monitor_history::{MAX_PROCESS_COMMAND_BYTES, MAX_PROCESS_STATUS_BYTES};

#[cfg(windows)]
pub(crate) struct Win32ProcessMetrics {
    pub start_time_ms: u64,
    pub cpu_time_ms: u64,
    pub resident_bytes: u64,
    pub virtual_bytes: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub exe_path: Option<String>,
}

/// Raw per-process metrics yielded by a [`ProcessSampler`] before CPU-delta calculation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProcessSample {
    /// OS process identifier.
    pub pid: u32,
    /// Parent process identifier (`0` if unknown).
    pub ppid: u32,
    /// Process start timestamp in Unix milliseconds.
    pub start_time_ms: u64,
    /// Process run duration in milliseconds.
    pub run_time_ms: u64,
    /// Process executable/image name.
    pub name: String,
    /// Full command line (or image name when unavailable).
    pub command: String,
    /// Process state string (`"Run"`, `"Sleep"`, etc.).
    pub status: String,
    /// Cumulative CPU time (user + kernel) in milliseconds.
    pub cpu_time_ms: u64,
    /// Resident working set memory in bytes.
    pub resident_bytes: u64,
    /// Virtual / private memory in bytes.
    pub virtual_bytes: u64,
    /// Cumulative bytes read.
    pub io_read_bytes: u64,
    /// Cumulative bytes written.
    pub io_write_bytes: u64,
}

/// Abstract process telemetry source used by [`crate::resource_monitor::ResourceMonitorEngine`].
pub trait ProcessSampler {
    /// Returns the full process table sorted by `pid`.
    fn process_table(&mut self) -> Vec<ProcessTableEntryV3>;

    /// Captures raw process metrics for `roots` and their descendant candidates plus total scanned count.
    fn sample_candidates(&mut self, roots: &[u32]) -> (usize, Vec<RawProcessSample>);
}

/// Lightweight real-OS sampler reading host process metrics without spawning subprocesses.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsProcessSampler;

impl OsProcessSampler {
    /// Creates a new [`OsProcessSampler`].
    pub const fn new() -> Self {
        Self
    }
}

fn collect_reachable_pids(rows: &[(u32, u32)], roots: &[u32]) -> HashSet<u32> {
    let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, ppid) in rows {
        children_by_parent.entry(ppid).or_default().push(pid);
    }
    let mut candidates = HashSet::new();
    let mut queue: VecDeque<u32> = roots.iter().copied().collect();
    while let Some(pid) = queue.pop_front() {
        if !candidates.insert(pid) {
            continue;
        }
        if let Some(children) = children_by_parent.get(&pid) {
            queue.extend(children.iter().copied());
        }
    }
    candidates
}

#[cfg(windows)]
impl ProcessSampler for OsProcessSampler {
    fn process_table(&mut self) -> Vec<ProcessTableEntryV3> {
        let mut entries = crate::win32_job::snapshot_process_table()
            .into_iter()
            .map(|(pid, ppid, name)| ProcessTableEntryV3 {
                pid,
                ppid,
                name: truncate_utf8(name, MAX_PROCESS_NAME_BYTES),
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.pid);
        entries
    }

    fn sample_candidates(&mut self, roots: &[u32]) -> (usize, Vec<RawProcessSample>) {
        let table = crate::win32_job::snapshot_process_table();
        let scanned_count = table.len();
        let pairs: Vec<(u32, u32)> = table.iter().map(|&(pid, ppid, _)| (pid, ppid)).collect();
        let candidate_pids = collect_reachable_pids(&pairs, roots);
        let row_by_pid: HashMap<u32, (u32, String)> = table
            .into_iter()
            .map(|(pid, ppid, name)| (pid, (ppid, name)))
            .collect();

        let now_unix_ms = unix_time_ms();
        let mut samples = candidate_pids
            .into_iter()
            .filter_map(|pid| {
                let (ppid, name) = row_by_pid.get(&pid)?;
                let m = crate::win32_job::query_process_metrics(pid)?;
                Some(RawProcessSample {
                    pid,
                    ppid: *ppid,
                    start_time_ms: m.start_time_ms,
                    run_time_ms: now_unix_ms.saturating_sub(m.start_time_ms),
                    name: name.clone(),
                    command: m.exe_path.unwrap_or_else(|| name.clone()),
                    status: "Run".to_owned(),
                    cpu_time_ms: m.cpu_time_ms,
                    resident_bytes: m.resident_bytes,
                    virtual_bytes: m.virtual_bytes,
                    io_read_bytes: m.io_read_bytes,
                    io_write_bytes: m.io_write_bytes,
                })
            })
            .collect::<Vec<_>>();
        samples.sort_by_key(|sample| sample.pid);
        (scanned_count, samples)
    }
}

#[cfg(target_os = "linux")]
mod linux_proc {
    use std::fs;
    use std::path::Path;

    use super::{
        MAX_PROCESS_COMMAND_BYTES, MAX_PROCESS_NAME_BYTES, MAX_PROCESS_STATUS_BYTES,
        ProcessTableEntryV3, RawProcessSample, truncate_utf8, unix_time_ms,
    };

    const LINUX_JIFFY_MS: u64 = 10;

    fn parse_proc_stat_fields(stat: &[u8]) -> Option<(String, String, u32, u64, u64)> {
        let open = stat.iter().position(|&b| b == b'(')?;
        let close = stat.iter().rposition(|&b| b == b')')?;
        let name_bytes = stat.get(open + 1..close)?;
        let tail = std::str::from_utf8(stat.get(close + 1..)?.trim_ascii_start()).ok()?;
        let mut fields = tail.split_ascii_whitespace();
        let state = fields.next()?;
        let ppid = fields.next()?.parse::<u32>().ok()?;
        let utime = fields.nth(9)?.parse::<u64>().ok()?;
        let stime = fields.next()?.parse::<u64>().ok()?;
        let start_ticks = fields.nth(5)?.parse::<u64>().ok()?;
        Some((
            truncate_utf8(
                String::from_utf8_lossy(name_bytes).into_owned(),
                MAX_PROCESS_NAME_BYTES,
            ),
            truncate_utf8(state.to_owned(), MAX_PROCESS_STATUS_BYTES),
            ppid,
            utime.saturating_add(stime).saturating_mul(LINUX_JIFFY_MS),
            start_ticks.saturating_mul(LINUX_JIFFY_MS),
        ))
    }

    fn uptime_ms() -> u64 {
        let Ok(raw) = fs::read_to_string("/proc/uptime") else {
            return 0;
        };
        let secs = raw
            .split_ascii_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if !secs.is_finite() || secs <= 0.0 {
            return 0;
        }
        #[expect(clippy::cast_possible_truncation, reason = "uptime in ms fits in u64")]
        #[expect(clippy::cast_sign_loss, reason = "checked secs > 0.0 above")]
        let ms = (secs * 1_000.0) as u64;
        ms
    }

    fn read_kv_kb(proc_dir: &Path) -> (u64, u64) {
        let Ok(status) = fs::read_to_string(proc_dir.join("status")) else {
            return (0, 0);
        };
        let (mut rss, mut vsize) = (0u64, 0u64);
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                rss = rest
                    .split_ascii_whitespace()
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
                    .saturating_mul(1024);
            } else if let Some(rest) = line.strip_prefix("VmSize:") {
                vsize = rest
                    .split_ascii_whitespace()
                    .next()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0)
                    .saturating_mul(1024);
            }
        }
        (rss, vsize)
    }

    fn read_io_bytes(proc_dir: &Path) -> (u64, u64) {
        let Ok(io_text) = fs::read_to_string(proc_dir.join("io")) else {
            return (0, 0);
        };
        let (mut read_bytes, mut write_bytes) = (0u64, 0u64);
        for line in io_text.lines() {
            if let Some(rest) = line.strip_prefix("read_bytes:") {
                read_bytes = rest.trim().parse::<u64>().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("write_bytes:") {
                write_bytes = rest.trim().parse::<u64>().unwrap_or(0);
            }
        }
        (read_bytes, write_bytes)
    }

    pub(super) fn read_process_table() -> Vec<ProcessTableEntryV3> {
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        let mut rows = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
                if pid == 0 {
                    return None;
                }
                let stat = fs::read(entry.path().join("stat")).ok()?;
                let (name, _, parent_pid, _, _) = parse_proc_stat_fields(&stat)?;
                Some(ProcessTableEntryV3 {
                    pid,
                    ppid: parent_pid,
                    name,
                })
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| row.pid);
        rows
    }

    pub(super) fn read_process_sample(pid: u32) -> Option<RawProcessSample> {
        let proc_dir = Path::new("/proc").join(pid.to_string());
        let stat = fs::read(proc_dir.join("stat")).ok()?;
        let (name, status, parent_pid, cpu_time_ms, start_ticks_ms) =
            parse_proc_stat_fields(&stat)?;
        let now_unix = unix_time_ms();
        let up_ms = uptime_ms();
        let start_time_ms = now_unix
            .saturating_sub(up_ms)
            .saturating_add(start_ticks_ms);
        let (resident_bytes, virtual_bytes) = read_kv_kb(&proc_dir);
        let (io_read_bytes, io_write_bytes) = read_io_bytes(&proc_dir);
        let command = fs::read(proc_dir.join("cmdline"))
            .ok()
            .map(|raw| {
                raw.split(|&b| b == 0)
                    .filter(|part| !part.is_empty())
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|cmd| !cmd.is_empty())
            .unwrap_or_else(|| name.clone());
        Some(RawProcessSample {
            pid,
            ppid: parent_pid,
            start_time_ms,
            run_time_ms: up_ms.saturating_sub(start_ticks_ms),
            name,
            command: truncate_utf8(command, MAX_PROCESS_COMMAND_BYTES),
            status,
            cpu_time_ms,
            resident_bytes,
            virtual_bytes,
            io_read_bytes,
            io_write_bytes,
        })
    }
}

#[cfg(target_os = "linux")]
impl ProcessSampler for OsProcessSampler {
    fn process_table(&mut self) -> Vec<ProcessTableEntryV3> {
        linux_proc::read_process_table()
    }

    fn sample_candidates(&mut self, roots: &[u32]) -> (usize, Vec<RawProcessSample>) {
        let table = self.process_table();
        let scanned_count = table.len();
        let pairs: Vec<(u32, u32)> = table.iter().map(|r| (r.pid, r.ppid)).collect();
        let mut samples = collect_reachable_pids(&pairs, roots)
            .into_iter()
            .filter_map(linux_proc::read_process_sample)
            .collect::<Vec<_>>();
        samples.sort_by_key(|s| s.pid);
        (scanned_count, samples)
    }
}

#[cfg(all(not(windows), not(target_os = "linux")))]
impl ProcessSampler for OsProcessSampler {
    fn process_table(&mut self) -> Vec<ProcessTableEntryV3> {
        let pid = std::process::id();
        let name = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "t3".to_owned());
        vec![ProcessTableEntryV3 {
            pid,
            ppid: 0,
            name: truncate_utf8(name, MAX_PROCESS_NAME_BYTES),
        }]
    }

    fn sample_candidates(&mut self, roots: &[u32]) -> (usize, Vec<RawProcessSample>) {
        let _ = collect_reachable_pids(&[], roots);
        let pid = std::process::id();
        if !roots.contains(&pid) {
            return (1, Vec::new());
        }
        let name = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "t3".to_owned());
        let command = std::env::args_os()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        (
            1,
            vec![RawProcessSample {
                pid,
                ppid: 0,
                start_time_ms: 0,
                run_time_ms: 0,
                name: truncate_utf8(name.clone(), MAX_PROCESS_NAME_BYTES),
                command: truncate_utf8(
                    if command.is_empty() { name } else { command },
                    MAX_PROCESS_COMMAND_BYTES,
                ),
                status: truncate_utf8("Run".to_owned(), MAX_PROCESS_STATUS_BYTES),
                cpu_time_ms: 0,
                resident_bytes: 0,
                virtual_bytes: 0,
                io_read_bytes: 0,
                io_write_bytes: 0,
            }],
        )
    }
}
