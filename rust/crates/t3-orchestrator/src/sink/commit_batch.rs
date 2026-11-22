//! Focused-thread-priority queuing and entity snapshot coalescing for provider updates
//! (`db51b6719f` / #39502).

use std::collections::{HashMap, HashSet, VecDeque};

use t3_contracts::ids::ThreadId;

/// Maximum snapshots taken per batch transaction.
pub const MAX_BATCH_EVENTS: usize = 64;

/// Maximum aggregate payload byte budget per batch transaction (`1 MiB`).
pub const MAX_BATCH_BYTES: usize = 1024 * 1024;

/// Waiting longer than this promotes a subagent stream to main-thread rank to avoid starvation.
pub const STARVATION_PROMOTION_MS: i64 = 30_000;

/// Maximum snapshots held across active stream queues before backpressure engages.
pub const MAX_QUEUED_SNAPSHOTS: usize = 4 * MAX_BATCH_EVENTS;

/// Priority metadata for a provider update stream writing snapshots for `thread_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamPriority {
    /// Owning thread of the provider stream.
    pub thread_id: ThreadId,
    /// True when the stream runs for a subagent/delegated task rather than a main thread.
    pub delegated: bool,
}

/// Snapshot queue entry emitted by a provider runtime before batched persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedSnapshotJob<T> {
    /// Stable caller/job identifier for correlating kept and superseded outcomes.
    pub job_id: u64,
    /// Entity deduplication key (`<eventType>:<entityId>`).
    pub entity_key: String,
    /// Owning thread ID.
    pub thread_id: ThreadId,
    /// Timestamp when enqueued in epoch milliseconds.
    pub queued_at_ms: i64,
    /// Estimated encoded JSON byte size.
    pub bytes: usize,
    /// Latest full entity snapshot payload.
    pub payload: T,
}

/// Kept snapshot unit paired with earlier superseded jobs of the same entity in the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoalescedBatchUnit<T> {
    /// Kept snapshot job to apply in the batch transaction.
    pub job: QueuedSnapshotJob<T>,
    /// Superseded job IDs settled alongside `job` without writing redundant events.
    pub superseded_job_ids: Vec<u64>,
}

/// Batch stop reason explaining why collection stopped taking queued snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStopReason {
    /// All ready streams drained cleanly.
    Ready,
    /// Reached [`MAX_BATCH_EVENTS`] or [`MAX_BATCH_BYTES`].
    Limit,
}

/// Result of scheduling one priority-ranked, coalesced batch across streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledSnapshotBatch<T> {
    /// Coalesced units ready to persist in batch order.
    pub units: Vec<CoalescedBatchUnit<T>>,
    /// Number of raw queued snapshots dequeued to produce `units`.
    pub dequeued_count: usize,
    /// Whether collection drained or stopped at a size limit.
    pub reason: BatchStopReason,
}

#[derive(Debug, Clone)]
struct StreamBucket<T> {
    priority: Option<ProviderStreamPriority>,
    jobs: VecDeque<QueuedSnapshotJob<T>>,
    served_at: u64,
    finishing: bool,
}

/// Pure priority scheduler and per-stream entity coalescer mirroring `ProviderCommitBatch`.
#[derive(Debug, Default)]
pub struct ProviderSnapshotScheduler<T> {
    streams: HashMap<u64, StreamBucket<T>>,
    servings: u64,
    queued_total: usize,
}

impl<T> ProviderSnapshotScheduler<T> {
    /// Creates an empty scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            streams: HashMap::new(),
            servings: 0,
            queued_total: 0,
        }
    }

    /// Registers or updates metadata for stream `stream_id`.
    pub fn register_stream(&mut self, stream_id: u64, priority: Option<ProviderStreamPriority>) {
        self.streams
            .entry(stream_id)
            .or_insert_with(|| StreamBucket {
                priority,
                jobs: VecDeque::new(),
                served_at: 0,
                finishing: false,
            });
    }

    /// Marks stream `stream_id` as finishing a control/EOF barrier so its queued tail flushes ahead
    /// of non-finishing streams in the same priority tier.
    pub fn mark_finishing(&mut self, stream_id: u64, finishing: bool) {
        if let Some(bucket) = self.streams.get_mut(&stream_id) {
            bucket.finishing = finishing;
        }
    }

    /// Returns total queued snapshots currently held across all streams.
    #[must_use]
    pub const fn queued_count(&self) -> usize {
        self.queued_total
    }

    /// Returns `true` when the shared backlog is below [`MAX_QUEUED_SNAPSHOTS`].
    #[must_use]
    pub const fn has_backlog_capacity(&self) -> bool {
        self.queued_total < MAX_QUEUED_SNAPSHOTS
    }

    /// Enqueues a snapshot job onto `stream_id`, returning `false` when the backlog is full.
    pub fn enqueue(&mut self, stream_id: u64, job: QueuedSnapshotJob<T>) -> bool {
        if !self.has_backlog_capacity() {
            return false;
        }
        let default_thread = job.thread_id.clone();
        let bucket = self
            .streams
            .entry(stream_id)
            .or_insert_with(|| StreamBucket {
                priority: Some(ProviderStreamPriority {
                    thread_id: default_thread,
                    delegated: false,
                }),
                jobs: VecDeque::new(),
                served_at: 0,
                finishing: false,
            });
        bucket.jobs.push_back(job);
        self.queued_total += 1;
        true
    }

    /// Selects and coalesces the next batch at `now_ms`.
    ///
    /// Ranks streams by: 1) viewed/focused threads (`0`), 2) main threads (`1`),
    /// 3) delegated subagents (`2`), promoting any stream waiting `>= STARVATION_PROMOTION_MS`
    /// to at most rank `1`, then `finishing` streams (`0`), then oldest `served_at`.
    pub fn next_batch<F>(&mut self, now_ms: i64, is_viewed: F) -> Option<ScheduledSnapshotBatch<T>>
    where
        F: Fn(&ThreadId) -> bool,
    {
        let mut ready: Vec<(u64, (u8, u8, u64))> = self
            .streams
            .iter()
            .filter_map(|(&id, bucket)| {
                let oldest = bucket.jobs.front()?;
                Some((
                    id,
                    stream_rank(bucket, oldest.queued_at_ms, now_ms, &is_viewed),
                ))
            })
            .collect();
        if ready.is_empty() {
            return None;
        }
        ready.sort_by_key(|(id, rank)| (*rank, *id));

        let mut units = Vec::new();
        let mut bytes = 0_usize;
        let mut dequeued_count = 0_usize;
        let mut reason = BatchStopReason::Ready;

        for (stream_id, _) in ready {
            let Some(bucket) = self.streams.get_mut(&stream_id) else {
                continue;
            };
            let mut take = 0_usize;
            while take < bucket.jobs.len() && take < MAX_BATCH_EVENTS.saturating_sub(units.len()) {
                let next_bytes = bucket.jobs[take].bytes;
                if (units.len() + take > 0) && bytes.saturating_add(next_bytes) > MAX_BATCH_BYTES {
                    break;
                }
                bytes = bytes.saturating_add(next_bytes);
                take += 1;
            }
            if take < bucket.jobs.len() {
                reason = BatchStopReason::Limit;
            }
            if take == 0 {
                break;
            }
            let taken: Vec<QueuedSnapshotJob<T>> = bucket.jobs.drain(..take).collect();
            self.queued_total = self.queued_total.saturating_sub(take);
            dequeued_count += take;
            units.extend(coalesce_stream_snapshots(taken));
            self.servings += 1;
            bucket.served_at = self.servings;
            if units.len() >= MAX_BATCH_EVENTS || bytes >= MAX_BATCH_BYTES {
                break;
            }
        }

        Some(ScheduledSnapshotBatch {
            units,
            dequeued_count,
            reason,
        })
    }
}

fn stream_rank<F>(
    bucket: &StreamBucket<impl Sized>,
    head_queued_at_ms: i64,
    now_ms: i64,
    is_viewed: &F,
) -> (u8, u8, u64)
where
    F: Fn(&ThreadId) -> bool,
{
    let tier = match &bucket.priority {
        Some(p) if is_viewed(&p.thread_id) => 0,
        Some(p) if p.delegated => 2,
        None | Some(_) => 1,
    };
    let effective_tier = if now_ms.saturating_sub(head_queued_at_ms) >= STARVATION_PROMOTION_MS {
        tier.min(1)
    } else {
        tier
    };
    let finishing_rank = u8::from(!bucket.finishing);
    (effective_tier, finishing_rank, bucket.served_at)
}

/// Drops earlier snapshots for the same `entity_key` within a single stream slice only when no
/// distinct entity makes its first appearance in between, preserving entity first-seen ordering.
pub fn coalesce_stream_snapshots<T>(jobs: Vec<QueuedSnapshotJob<T>>) -> Vec<CoalescedBatchUnit<T>> {
    if jobs.len() <= 1 {
        return jobs
            .into_iter()
            .map(|job| CoalescedBatchUnit {
                job,
                superseded_job_ids: Vec::new(),
            })
            .collect();
    }

    let mut first_seen: HashMap<&str, usize> = HashMap::with_capacity(jobs.len());
    for (idx, job) in jobs.iter().enumerate() {
        first_seen.entry(job.entity_key.as_str()).or_insert(idx);
    }

    let mut survivor: Vec<usize> = (0..jobs.len()).collect();
    let mut next_of_key: HashMap<&str, usize> = HashMap::with_capacity(jobs.len());
    for idx in (0..jobs.len()).rev() {
        let key = jobs[idx].entity_key.as_str();
        if let Some(next) = next_of_key.insert(key, idx) {
            let mut replaced = true;
            for item in &jobs[(idx + 1)..next] {
                if first_seen
                    .get(item.entity_key.as_str())
                    .is_some_and(|&pos| pos > idx)
                {
                    replaced = false;
                    break;
                }
            }
            if replaced {
                survivor[idx] = survivor[next];
            }
        }
    }

    let mut superseded: HashMap<usize, Vec<u64>> = HashMap::new();
    for (idx, job) in jobs.iter().enumerate() {
        let kept = survivor[idx];
        if kept != idx {
            superseded.entry(kept).or_default().push(job.job_id);
        }
    }

    let mut seen_kept: HashSet<usize> = HashSet::new();
    let mut out = Vec::with_capacity(jobs.len());
    for (idx, job) in jobs.into_iter().enumerate() {
        if survivor[idx] == idx && seen_kept.insert(idx) {
            out.push(CoalescedBatchUnit {
                job,
                superseded_job_ids: superseded.remove(&idx).unwrap_or_default(),
            });
        }
    }
    out
}
