//! Resident-set-size snapshots for OOM regression tests + paint-session
//! tracing (Sprint 23 / T1, ADR-041 amendment).
//!
//! The [`current`] helper returns a [`RssSnapshot`] on Linux (read from
//! `/proc/self/status` via [`procfs`]) and `None` everywhere else.
//! Sized in **bytes** at the public surface so callers don't have to
//! reason about kB↔MB conversions; conversion to display units happens
//! at the call site.
//!
//! The Sprint 23 OOM investigation traces RSS deltas around the
//! `Tool::PaintLayer` entry frame and across paint sessions. The
//! regression test in
//! [`super::layers::tests::sixteen_smu_four_layer_stack_fits_cpu_budget`]
//! pins the CPU layer-stack budget; the GPU-resident composite RT +
//! mask array land separately in the renderer fix.
//!
//! ## Why Linux-only?
//!
//! `procfs` is Linux-specific. Windows / macOS could in principle read
//! their equivalent (`GetProcessMemoryInfo` / `mach_task_self`), but
//! the original OOM was reported on Linux and the regression we're
//! pinning is the same shape on every OS the editor runs on. The
//! reading just isn't there on other platforms in this sprint; a
//! follow-up can add `sysinfo` if cross-platform numbers become
//! load-bearing.

/// Snapshot of one process's resident-set + peak virtual size, in
/// bytes. Carries a label so traced sequences are self-documenting.
#[derive(Debug, Clone)]
pub struct RssSnapshot {
    pub label: String,
    /// Current resident-set bytes (`VmRSS` on Linux). The closest
    /// proxy for "how much RAM is this process actually pinning right
    /// now."
    pub rss_bytes: u64,
    /// High-water-mark virtual size since process start
    /// (`VmPeak` on Linux). Useful for catching transient spikes
    /// between two `rss_bytes` snapshots.
    pub vm_peak_bytes: u64,
}

impl RssSnapshot {
    /// Convenience MB accessor for logging / assertions.
    pub fn rss_mb(&self) -> u64 {
        self.rss_bytes / (1024 * 1024)
    }

    /// Convenience MB accessor for the peak.
    pub fn vm_peak_mb(&self) -> u64 {
        self.vm_peak_bytes / (1024 * 1024)
    }
}

/// Read the current process's [`RssSnapshot`].
///
/// Returns `None` on non-Linux targets or when `/proc/self/status` is
/// unreadable (e.g. a sandboxed harness without `/proc` mounted).
/// Callers should treat `None` as "skip this assertion" rather than a
/// failure — the regression test is Linux-only by intent.
#[cfg(target_os = "linux")]
pub fn current(label: impl Into<String>) -> Option<RssSnapshot> {
    let proc = procfs::process::Process::myself().ok()?;
    let status = proc.status().ok()?;
    // procfs returns kB; convert to bytes for the public surface.
    let rss_bytes = status.vmrss?.saturating_mul(1024);
    let vm_peak_bytes = status.vmpeak?.saturating_mul(1024);
    Some(RssSnapshot {
        label: label.into(),
        rss_bytes,
        vm_peak_bytes,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn current(_label: impl Into<String>) -> Option<RssSnapshot> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn current_returns_plausible_snapshot_on_linux() {
        let snap = current("baseline").expect("Linux harness reads /proc/self/status");
        // Any running test process holds at least a few MB resident.
        // The hard floor is "non-zero" — even a hello-world is > 1 MB.
        assert!(
            snap.rss_bytes > 1024 * 1024,
            "rss should be > 1 MB; got {} B",
            snap.rss_bytes
        );
        assert!(
            snap.vm_peak_bytes >= snap.rss_bytes,
            "VmPeak >= VmRSS by definition; got peak={} rss={}",
            snap.vm_peak_bytes,
            snap.rss_bytes,
        );
        // The label round-trips.
        assert_eq!(snap.label, "baseline");
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn current_returns_none_off_linux() {
        assert!(current("off-linux").is_none());
    }

    #[test]
    fn mb_accessor_floors_division() {
        let snap = RssSnapshot {
            label: "test".into(),
            rss_bytes: 1024 * 1024 * 7 + 1024,
            vm_peak_bytes: 1024 * 1024 * 13,
        };
        assert_eq!(snap.rss_mb(), 7);
        assert_eq!(snap.vm_peak_mb(), 13);
    }
}
