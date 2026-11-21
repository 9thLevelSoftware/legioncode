//! Byte formatting, doctor repair planning, and byte-for-byte CLI report rendering.
//!
//! Mirrors `apps/server/src/cli/doctor.ts:16-21` and `apps/server/src/cli/doctor.ts:68-136`.

use super::DatabaseInspection;

/// Reclaimable-bytes threshold (`1 GiB`) above which compaction is recommended even when
/// incremental auto-vacuum is already enabled (`DATABASE_COMPACTION_RECOMMENDED_BYTES`).
pub const DATABASE_COMPACTION_RECOMMENDED_BYTES: u64 = 1024 * 1024 * 1024;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// Identifier for a candidate doctor repair action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorRepairId {
    /// Compact and enable incremental auto-vacuum on `statev2.sqlite`.
    Database,
    /// Delete the obsolete pre-0.1.0 `state.sqlite` database.
    Legacy,
}

/// Planned repair surfaced by [`plan_doctor_repairs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRepair {
    /// Machine-readable repair identifier (`"database"` or `"legacy"` in TypeScript).
    pub id: DoctorRepairId,
    /// User-facing repair title.
    pub title: &'static str,
    /// Multi-line indented repair description matching `cli/doctor.ts:93-113`.
    pub description: String,
    /// Whether `t3 doctor --fix` selects this repair automatically.
    pub recommended: bool,
}

/// Rendering options controlling the trailing prompt guidance in [`render_doctor_report`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    /// Equivalent to `--fix`.
    pub fix: bool,
    /// Equivalent to `--dry-run`.
    pub dry_run: bool,
    /// Equivalent to an interactive TTY session (`cli/doctor.ts:130`).
    pub interactive: bool,
}

/// Formats a byte count using `cli/doctor.ts:16-21` rules:
/// `>= 1 GiB` with 2 decimals, `>= 1 MiB` with 1 decimal, otherwise ceiling KiB.
///
/// # Examples
///
/// ```
/// use t3_persistence::maintenance::format_bytes;
///
/// assert_eq!(format_bytes(0), "0 KiB");
/// assert_eq!(format_bytes(1), "1 KiB");
/// assert_eq!(format_bytes(1024), "1 KiB");
/// assert_eq!(format_bytes(1025), "2 KiB");
/// assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
/// assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GiB");
/// ```
pub fn format_bytes(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.2} GiB", u64_to_f64(bytes) / 1_073_741_824.0)
    } else if bytes >= MIB {
        format!("{:.1} MiB", u64_to_f64(bytes) / 1_048_576.0)
    } else {
        format!("{} KiB", bytes.div_ceil(KIB))
    }
}

fn u64_to_f64(value: u64) -> f64 {
    let [b0, b1, b2, b3, b4, b5, b6, b7] = value.to_le_bytes();
    let lo = u32::from_le_bytes([b0, b1, b2, b3]);
    let hi = u32::from_le_bytes([b4, b5, b6, b7]);
    f64::from(hi) * 4_294_967_296.0 + f64::from(lo)
}

/// Computes the repair actions applicable to `status`, matching `cli/doctor.ts:86-116`.
///
/// # Examples
///
/// ```
/// use t3_persistence::maintenance::{DatabaseInspection, DoctorRepairId, plan_doctor_repairs};
///
/// let clean = DatabaseInspection {
///     database_bytes: 16_384,
///     reclaimable_bytes: 0,
///     incremental_vacuum_enabled: true,
///     legacy_database_bytes: None,
///     legacy_only_records: None,
/// };
/// assert!(plan_doctor_repairs(&clean).is_empty());
///
/// let needs_vacuum = DatabaseInspection {
///     incremental_vacuum_enabled: false,
///     ..clean
/// };
/// let repairs = plan_doctor_repairs(&needs_vacuum);
/// assert_eq!(repairs.len(), 1);
/// assert_eq!(repairs[0].id, DoctorRepairId::Database);
/// assert!(repairs[0].recommended);
/// ```
pub fn plan_doctor_repairs(status: &DatabaseInspection) -> Vec<DoctorRepair> {
    let mut repairs = Vec::new();
    if !(status.incremental_vacuum_enabled && status.reclaimable_bytes == 0) {
        let lead = if status.incremental_vacuum_enabled {
            "Returns unused space to disk."
        } else {
            "Returns unused space to disk and turns on automatic shrinking."
        };
        repairs.push(DoctorRepair {
            id: DoctorRepairId::Database,
            title: "Reclaim database space",
            description: format!(
                "{lead}\n  Rewrites the whole {} database, which can take more than an hour on a large database or slow disk.\n  Pauses the background service, if installed, while it runs and may need temporary disk space.",
                format_bytes(status.database_bytes),
            ),
            recommended: !status.incremental_vacuum_enabled
                || status.reclaimable_bytes >= DATABASE_COMPACTION_RECOMMENDED_BYTES,
        });
    }

    if let (Some(legacy_bytes), Some(records)) =
        (status.legacy_database_bytes, status.legacy_only_records)
        && records.projects == 0
        && records.threads == 0
    {
        repairs.push(DoctorRepair {
            id: DoctorRepairId::Legacy,
            title: "Delete the database from before 0.1.0",
            description: format!(
                "Frees {}.\n  Warning: after this, a T3 Code version before 0.1.0 starts with no threads or settings, so you can no longer roll back.",
                format_bytes(legacy_bytes),
            ),
            recommended: false,
        });
    }

    repairs
}

/// Renders the doctor inspection and repair report matching `cli/doctor.ts:68-136` byte-for-byte.
///
/// Each line corresponds to one `Console.log(...)` call joined with `"\n"`.
///
/// # Examples
///
/// ```
/// use t3_persistence::maintenance::{DatabaseInspection, DoctorOptions, render_doctor_report};
///
/// let clean = DatabaseInspection {
///     database_bytes: 16_384,
///     reclaimable_bytes: 0,
///     incremental_vacuum_enabled: true,
///     legacy_database_bytes: None,
///     legacy_only_records: None,
/// };
/// assert_eq!(
///     render_doctor_report(&clean, DoctorOptions::default()),
///     "Database: 16 KiB, 0 KiB reclaimable.\nNothing to repair.",
/// );
/// ```
pub fn render_doctor_report(status: &DatabaseInspection, options: DoctorOptions) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Database: {}, {} reclaimable.",
        format_bytes(status.database_bytes),
        format_bytes(status.reclaimable_bytes),
    ));

    if let Some(legacy_bytes) = status.legacy_database_bytes {
        lines.push(format!(
            "Database from before 0.1.0: {}, kept for rolling back.",
            format_bytes(legacy_bytes),
        ));
        match status.legacy_only_records {
            None => {
                lines.push(
                    "  Could not check for missing projects or threads. Keep the old database."
                        .to_owned(),
                );
            }
            Some(records) if records.projects > 0 || records.threads > 0 => {
                lines.push(format!(
                    "  The current database is missing legacy records. Projects missing: {}. Threads missing: {}. Keep the old database for recovery.",
                    records.projects, records.threads,
                ));
            }
            Some(_) => {}
        }
    }

    let repairs = plan_doctor_repairs(status);
    if repairs.is_empty() {
        lines.push("Nothing to repair.".to_owned());
        return lines.join("\n");
    }

    for repair in &repairs {
        let suffix = if repair.recommended {
            " (recommended)"
        } else {
            ""
        };
        lines.push(format!(
            "\n{}{}\n  {}",
            repair.title, suffix, repair.description
        ));
    }

    if options.dry_run {
        lines.push("\nDry run: nothing changed.".to_owned());
    } else if !options.fix && !options.interactive {
        lines.push(
            "\nRun in a terminal to choose repairs, or pass --fix for the recommended ones."
                .to_owned(),
        );
    }

    lines.join("\n")
}
