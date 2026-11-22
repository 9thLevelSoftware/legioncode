//! Tests for `native/resource-monitor` protocol v3 engine, codec, CPU deltas, and OS sampler.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use legion_process_ffi::resource_monitor::{
    CpuDeltaTracker, ExternalProcessV3, HistoryRecorder, IoSemantics, MAX_HISTORY_SNAPSHOTS,
    MAX_PROCESS_NAME_BYTES, MonitorEventV3, MonitorRequestV3, NdjsonV3Codec, ProcessNodeMetrics,
    ProcessSampler, ProcessTableEntryV3, RESOURCE_MONITOR_PROTOCOL_VERSION, RawProcessSample,
    ResourceMonitorEngine, ResourceSampleV3, clamp_sample_interval, matches_external_identity,
    platform_io_semantics, select_tracked_pids, truncate_utf8,
};

struct MockSampler {
    table: Vec<ProcessTableEntryV3>,
    samples: HashMap<u32, RawProcessSample>,
}

impl ProcessSampler for MockSampler {
    fn process_table(&mut self) -> Vec<ProcessTableEntryV3> {
        self.table.clone()
    }

    fn sample_candidates(&mut self, roots: &[u32]) -> (usize, Vec<RawProcessSample>) {
        let mut out: Vec<RawProcessSample> = self
            .samples
            .values()
            .filter(|s| roots.contains(&s.pid) || roots.contains(&s.ppid))
            .cloned()
            .collect();
        out.sort_by_key(|s| s.pid);
        (self.samples.len(), out)
    }
}

fn make_raw(pid: u32, parent_pid: u32, start_ms: u64, cpu_ms: u64) -> RawProcessSample {
    RawProcessSample {
        pid,
        ppid: parent_pid,
        start_time_ms: start_ms,
        run_time_ms: 1_000,
        name: format!("proc-{pid}"),
        command: format!("proc-{pid} --flag"),
        status: "Run".to_owned(),
        cpu_time_ms: cpu_ms,
        resident_bytes: 1024 * 1024,
        virtual_bytes: 4 * 1024 * 1024,
        io_read_bytes: 100,
        io_write_bytes: 200,
    }
}

#[test]
fn decodes_and_encodes_all_v3_requests_and_events() -> Result<(), String> {
    let lines = [
        r#"{"version":3,"type":"configure","rootPid":42,"sampleIntervalMs":1000,"externalProcesses":[{"pid":7}]}"#,
        r#"{"version":3,"type":"setExternalProcesses","processes":[{"pid":8,"startTimeMs":5000}]}"#,
        r#"{"version":3,"type":"setSampleInterval","sampleIntervalMs":500}"#,
        r#"{"version":3,"type":"setStreaming","enabled":true}"#,
        r#"{"version":3,"type":"sampleNow","requestId":"req-1"}"#,
        r#"{"version":3,"type":"processTable","requestId":"pt-1"}"#,
        r#"{"version":3,"type":"readHistory","requestId":"h-1","windowMs":60000}"#,
        r#"{"version":3,"type":"shutdown"}"#,
    ];
    for raw in lines {
        let req = NdjsonV3Codec::decode_request_line(raw)
            .map_err(|e| e.to_string())?
            .ok_or("expected request")?;
        assert_eq!(req.version(), RESOURCE_MONITOR_PROTOCOL_VERSION);
        let encoded = NdjsonV3Codec::encode_request_line(&req).map_err(|e| e.to_string())?;
        assert!(encoded.ends_with('\n'));
        let reparsed = NdjsonV3Codec::decode_request_line(&encoded).map_err(|e| e.to_string())?;
        assert_eq!(reparsed, Some(req));
    }

    let mut codec = NdjsonV3Codec::new();
    let partial = codec
        .push_bytes(b"{\"version\":3")
        .map_err(|e| e.to_string())?;
    assert_eq!(partial.len(), 0);
    let extracted = codec
        .push_bytes(b",\"type\":\"shutdown\"}\r\n  \n")
        .map_err(|e| e.to_string())?;
    assert_eq!(extracted, vec![r#"{"version":3,"type":"shutdown"}"#]);
    Ok(())
}

#[test]
fn rolling_cpu_delta_handles_intervals_multi_core_and_pid_reuse() {
    let mut tracker = CpuDeltaTracker::with_window(2);
    assert_eq!(tracker.record_and_compute(10, 1_000, 100, 10_000), 0.0);
    assert!((tracker.record_and_compute(10, 1_000, 350, 11_000) - 25.0).abs() < 1e-6);
    // Window=2 compares against base point (t=10_000, cpu=100) -> (1350-100)/(12_000-10_000)*100 = 62.5%
    assert!((tracker.record_and_compute(10, 1_000, 1_350, 12_000) - 62.5).abs() < 1e-6);

    // PID reuse changes start_time_ms -> resets baseline to 0.0%.
    assert_eq!(tracker.record_and_compute(10, 2_000, 50, 13_000), 0.0);
    // Non-monotonic timestamp resets cleanly without NaN/Inf.
    assert_eq!(tracker.record_and_compute(10, 2_000, 80, 13_000), 0.0);

    let keep = HashSet::from([99u32]);
    tracker.prune(&keep);
    assert_eq!(tracker.record_and_compute(10, 2_000, 100, 14_000), 0.0);
}

#[test]
fn tracks_process_trees_and_rejects_reused_parent_descendants() {
    let rows = vec![
        (20, 1, 5_000),
        (21, 20, 4_000),
        (22, 20, 5_100),
        (23, 21, 5_200),
    ];
    assert_eq!(
        select_tracked_pids(&rows, &HashSet::from([20])),
        HashSet::from([20, 22])
    );
    assert!(matches_external_identity(10_000, Some(10_999)));
    assert!(!matches_external_identity(10_000, Some(11_000)));
    assert_eq!(clamp_sample_interval(0), None);
    assert_eq!(clamp_sample_interval(1).map(|d| d.as_millis()), Some(250));
    assert_eq!(
        clamp_sample_interval(99_999).map(|d| d.as_millis()),
        Some(60_000)
    );

    let truncated = truncate_utf8(
        "é".repeat(MAX_PROCESS_NAME_BYTES),
        MAX_PROCESS_NAME_BYTES - 1,
    );
    assert!(truncated.len() < MAX_PROCESS_NAME_BYTES);
    assert!(truncated.is_char_boundary(truncated.len()));
}

#[test]
fn history_recorder_bounds_and_clock_rollback() {
    let mut history = HistoryRecorder::new();
    for seq in 0..=MAX_HISTORY_SNAPSHOTS {
        let seq_u64 = u64::try_from(seq).unwrap_or(0);
        history.record(&ResourceSampleV3 {
            version: 3,
            event_type: "snapshot".to_owned(),
            sequence: seq_u64,
            sampled_at_unix_ms: seq_u64 * 1_000,
            collection_duration_micros: 1,
            scanned_process_count: 1,
            retained_process_count: 0,
            inaccessible_process_count: 0,
            request_id: Some("req".to_owned()),
            external_processes: vec![ExternalProcessV3 {
                pid: 7,
                start_time_ms: Some(1_000),
            }],
            processes: Vec::new(),
        });
    }
    assert_eq!(history.len(), MAX_HISTORY_SNAPSHOTS);
    assert_eq!(
        history
            .read(10_000, MAX_HISTORY_SNAPSHOTS as u64 * 1_000)
            .len(),
        11
    );

    let mut rollback = HistoryRecorder::new();
    let sample = ResourceSampleV3 {
        version: 3,
        event_type: "snapshot".to_owned(),
        sequence: 1,
        sampled_at_unix_ms: 5_000,
        collection_duration_micros: 1,
        scanned_process_count: 1,
        retained_process_count: 1,
        inaccessible_process_count: 0,
        request_id: None,
        external_processes: Vec::new(),
        processes: vec![ProcessNodeMetrics {
            pid: 1,
            ppid: 0,
            start_time_ms: 0,
            run_time_ms: 0,
            name: "p".to_owned(),
            command: "p".to_owned(),
            status: "Run".to_owned(),
            cpu_percent: 0.0,
            cpu_time_ms: 0,
            resident_bytes: 0,
            virtual_bytes: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            io_semantics: IoSemantics::Storage,
        }],
    };
    rollback.record(&sample);
    assert_eq!(rollback.read(0, 2_000).len(), 0);
    rollback.record(&ResourceSampleV3 {
        sequence: 2,
        sampled_at_unix_ms: 2_000,
        ..sample
    });
    assert_eq!(rollback.len(), 1);
    assert_eq!(rollback.read(5_000, 2_000)[0].sequence, 2);
}

#[test]
fn stdio_session_processes_v3_commands_and_errors() -> Result<(), String> {
    let sampler = MockSampler {
        table: vec![ProcessTableEntryV3 {
            pid: 10,
            ppid: 1,
            name: "node".to_owned(),
        }],
        samples: HashMap::from([(10, make_raw(10, 1, 1_000, 250))]),
    };
    let mut engine = ResourceMonitorEngine::new(sampler);
    let input = concat!(
        "{\"version\":3,\"type\":\"sampleNow\",\"requestId\":\"early\"}\n",
        "{\"version\":2,\"type\":\"shutdown\"}\n",
        "not-json\n",
        "{\"version\":3,\"type\":\"configure\",\"rootPid\":10,\"sampleIntervalMs\":1000,\"externalProcesses\":[]}\n",
        "{\"version\":3,\"type\":\"sampleNow\",\"requestId\":\"s-1\"}\n",
        "{\"version\":3,\"type\":\"processTable\",\"requestId\":\"pt-1\"}\n",
        "{\"version\":3,\"type\":\"readHistory\",\"requestId\":\"h-1\",\"windowMs\":60000}\n",
        "{\"version\":3,\"type\":\"shutdown\"}\n"
    );
    let mut out = Vec::new();
    engine
        .run_stdio_session(Cursor::new(input), &mut out)
        .map_err(|e| e.to_string())?;
    assert!(engine.is_shutdown_requested());

    let text = String::from_utf8(out).map_err(|e| e.to_string())?;
    let events: Vec<MonitorEventV3> = text
        .lines()
        .map(|l| NdjsonV3Codec::decode_event_line(l).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    assert!(matches!(events[0], MonitorEventV3::Hello(_)));
    assert!(matches!(&events[1], MonitorEventV3::Error(e) if e.code == "not-configured"));
    assert!(
        matches!(&events[2], MonitorEventV3::Error(e) if e.code == "protocol-mismatch" && !e.recoverable)
    );
    assert!(matches!(&events[3], MonitorEventV3::Error(e) if e.code == "invalid-command"));
    assert!(
        matches!(&events[4], MonitorEventV3::Snapshot(s) if s.request_id.as_deref() == Some("s-1"))
    );
    assert!(matches!(&events[5], MonitorEventV3::ProcessTable(t) if t.processes.len() == 1));
    assert!(
        matches!(&events[6], MonitorEventV3::HistoryChunk(c) if c.done && c.snapshots.len() == 1)
    );
    let chunk_json = NdjsonV3Codec::encode_event_line(&events[6]).map_err(|e| e.to_string())?;
    assert!(chunk_json.contains(r#""type":"historyChunk""#));
    assert!(chunk_json.contains(r#""type":"snapshot""#));
    Ok(())
}

#[test]
fn read_history_chunks_at_32_snapshots_with_done_flag() {
    let sampler = MockSampler {
        table: Vec::new(),
        samples: HashMap::from([(10, make_raw(10, 1, 1_000, 100))]),
    };
    let mut engine = ResourceMonitorEngine::new(sampler);
    let _ = engine.handle_request(MonitorRequestV3::Configure {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        root_pid: 10,
        sample_interval_ms: 1_000,
        external_processes: Vec::new(),
    });
    for _ in 0..33 {
        let _ = engine.handle_request(MonitorRequestV3::SampleNow {
            version: RESOURCE_MONITOR_PROTOCOL_VERSION,
            request_id: "s".to_owned(),
        });
    }
    let chunks = engine.handle_request(MonitorRequestV3::ReadHistory {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        request_id: "chunked".to_owned(),
        window_ms: 60_000,
    });
    assert_eq!(chunks.len(), 2);
    assert!(
        matches!(&chunks[0], MonitorEventV3::HistoryChunk(c) if !c.done && c.snapshots.len() == 32)
    );
    assert!(
        matches!(&chunks[1], MonitorEventV3::HistoryChunk(c) if c.done && c.snapshots.len() == 1)
    );
}

#[test]
fn os_sampler_reads_real_current_process_metrics_without_subprocesses() -> Result<(), String> {
    let mut engine = ResourceMonitorEngine::os_default();
    let self_pid = std::process::id();
    let cfg_events = engine.handle_request(MonitorRequestV3::Configure {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        root_pid: self_pid,
        sample_interval_ms: 1_000,
        external_processes: Vec::new(),
    });
    assert_eq!(cfg_events.len(), 0);

    let events = engine.handle_request(MonitorRequestV3::SampleNow {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        request_id: "real-1".to_owned(),
    });
    let Some(MonitorEventV3::Snapshot(sample)) = events.first() else {
        return Err("expected snapshot event".to_owned());
    };
    let own = sample
        .processes
        .iter()
        .find(|p| p.pid == self_pid)
        .ok_or("missing current process in OS snapshot")?;
    assert_ne!(own.command, "");
    assert!(own.cpu_percent.is_finite());
    assert_eq!(own.io_semantics, platform_io_semantics());
    if cfg!(any(windows, target_os = "linux")) {
        assert!(own.resident_bytes > 0);
        assert!(sample.scanned_process_count > 0);
    }

    let pt = engine.handle_request(MonitorRequestV3::ProcessTable {
        version: RESOURCE_MONITOR_PROTOCOL_VERSION,
        request_id: "real-pt".to_owned(),
    });
    let Some(MonitorEventV3::ProcessTable(table)) = pt.first() else {
        return Err("expected processTable event".to_owned());
    };
    assert!(table.processes.iter().any(|entry| entry.pid == self_pid));
    assert!(
        table
            .processes
            .windows(2)
            .all(|pair| pair[0].pid < pair[1].pid)
    );
    Ok(())
}
