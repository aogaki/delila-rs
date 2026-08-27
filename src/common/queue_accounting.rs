//! Byte/item accounting for the unbounded inter-task channels (TODO 68).
//!
//! The Merger and Recorder deliberately buffer in unbounded channels — the
//! "never drop a hit" rule forbids back-pressure drops, so a slow disk turns
//! into RAM growth. This gauge makes that growth observable (`queue_bytes` in
//! [`crate::common::ComponentMetrics`]) and drives the soft/hard backlog
//! watermarks that let the Operator run a drain-first stop *before* the OOM
//! killer decides for us.
//!
//! Design: two cumulative counter pairs (enqueued/dequeued), depth derived by
//! `saturating_sub` — the same self-balancing pattern the Monitor uses
//! (`recv - proc`), which cannot go negative under racing reads. Do NOT copy
//! the Reader's `queue_length` pattern (increment-only, never decremented).
//!
//! ⚠️ Reset semantics: the cumulative counters are *lifetime* counters. They
//! must NOT be cleared on run Start/Reset — channel contents straddle run
//! boundaries (the Merger drains stale data only on the Running transition),
//! and clearing while items sit in the channel corrupts the derived depth.
//! Only the peak is per-run: call [`QueueAccounting::reset_peak`], which
//! re-bases the peak at the *current* depth, not zero.

use std::sync::atomic::{AtomicU64, Ordering};

/// Cumulative enqueue/dequeue accounting for one channel.
///
/// All operations are `Relaxed`: the counters are monotonic and independently
/// read; a momentarily stale depth is fine for a 1-Hz status poll.
#[derive(Debug, Default)]
pub struct QueueAccounting {
    enqueued_items: AtomicU64,
    dequeued_items: AtomicU64,
    enqueued_bytes: AtomicU64,
    dequeued_bytes: AtomicU64,
    /// High-water mark of `depth_bytes()`, updated on enqueue.
    peak_bytes: AtomicU64,
}

impl QueueAccounting {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one item of `bytes` entering the channel (call before `send`).
    pub fn on_enqueue(&self, bytes: u64) {
        self.enqueued_items.fetch_add(1, Ordering::Relaxed);
        self.enqueued_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.peak_bytes
            .fetch_max(self.depth_bytes(), Ordering::Relaxed);
    }

    /// Record one item of `bytes` leaving the channel — including items that
    /// are subsequently discarded, and undo-paths when a `send` fails: the
    /// gauge tracks physical channel occupancy, not message fate.
    pub fn on_dequeue(&self, bytes: u64) {
        self.dequeued_items.fetch_add(1, Ordering::Relaxed);
        self.dequeued_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Current channel depth in bytes (approximate under concurrency, never
    /// negative).
    pub fn depth_bytes(&self) -> u64 {
        self.enqueued_bytes
            .load(Ordering::Relaxed)
            .saturating_sub(self.dequeued_bytes.load(Ordering::Relaxed))
    }

    /// Current channel depth in items (approximate, never negative).
    pub fn depth_items(&self) -> u64 {
        self.enqueued_items
            .load(Ordering::Relaxed)
            .saturating_sub(self.dequeued_items.load(Ordering::Relaxed))
    }

    /// High-water mark of `depth_bytes()` since the last [`reset_peak`].
    ///
    /// [`reset_peak`]: QueueAccounting::reset_peak
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes.load(Ordering::Relaxed)
    }

    /// Start a new per-run peak window. The peak re-bases at the *current*
    /// depth — never zero — because a backlog carried across the run boundary
    /// is still occupying RAM and must stay visible.
    pub fn reset_peak(&self) {
        self.peak_bytes.store(self.depth_bytes(), Ordering::Relaxed);
    }
}

/// Backlog severity for a channel depth against the configured watermarks.
///
/// - `0` — OK (below soft, or soft disabled)
/// - `1` — soft limit exceeded (warn; run keeps going)
/// - `2` — hard limit exceeded (drain-first stop material)
///
/// A limit of `0` disables that threshold. A misconfigured `hard < soft`
/// still reports `2` above hard — the more severe verdict wins.
pub fn backlog_level(depth_bytes: u64, soft_limit_bytes: u64, hard_limit_bytes: u64) -> u8 {
    if hard_limit_bytes > 0 && depth_bytes >= hard_limit_bytes {
        2
    } else if soft_limit_bytes > 0 && depth_bytes >= soft_limit_bytes {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_tracks_enqueue_dequeue() {
        let q = QueueAccounting::new();
        q.on_enqueue(100);
        q.on_enqueue(50);
        assert_eq!(q.depth_bytes(), 150);
        assert_eq!(q.depth_items(), 2);
        q.on_dequeue(100);
        assert_eq!(q.depth_bytes(), 50);
        assert_eq!(q.depth_items(), 1);
        q.on_dequeue(50);
        assert_eq!(q.depth_bytes(), 0);
        assert_eq!(q.depth_items(), 0);
    }

    #[test]
    fn depth_never_negative_under_racing_read_order() {
        // A reader may observe dequeued before the matching enqueued update;
        // saturating_sub must clamp at zero rather than wrap.
        let q = QueueAccounting::new();
        q.on_dequeue(500); // pathological order
        assert_eq!(q.depth_bytes(), 0);
        assert_eq!(q.depth_items(), 0);
    }

    #[test]
    fn peak_is_monotonic_within_a_window() {
        let q = QueueAccounting::new();
        q.on_enqueue(100);
        q.on_enqueue(100);
        q.on_dequeue(100);
        q.on_enqueue(10);
        // Peak saw 200 even though depth is now 110.
        assert_eq!(q.peak_bytes(), 200);
        assert_eq!(q.depth_bytes(), 110);
    }

    #[test]
    fn reset_peak_rebases_at_current_depth_not_zero() {
        // The exact bug the reset semantics prevent: a backlog straddling a
        // run boundary must remain visible in the new run's peak.
        let q = QueueAccounting::new();
        q.on_enqueue(300);
        q.on_enqueue(300);
        q.on_dequeue(300);
        assert_eq!(q.peak_bytes(), 600);
        q.reset_peak();
        assert_eq!(q.peak_bytes(), 300, "peak re-bases at live depth");
        q.on_enqueue(50);
        assert_eq!(q.peak_bytes(), 350);
    }

    #[test]
    fn backlog_level_boundaries() {
        let soft = 100;
        let hard = 200;
        assert_eq!(backlog_level(0, soft, hard), 0);
        assert_eq!(backlog_level(99, soft, hard), 0);
        assert_eq!(backlog_level(100, soft, hard), 1, "at soft = exceeded");
        assert_eq!(backlog_level(199, soft, hard), 1);
        assert_eq!(backlog_level(200, soft, hard), 2, "at hard = exceeded");
        assert_eq!(backlog_level(u64::MAX, soft, hard), 2);
    }

    #[test]
    fn backlog_level_zero_disables_a_threshold() {
        assert_eq!(backlog_level(u64::MAX, 0, 0), 0, "both disabled");
        assert_eq!(backlog_level(500, 0, 200), 2, "soft disabled, hard live");
        assert_eq!(backlog_level(150, 0, 200), 0, "soft disabled below hard");
        assert_eq!(
            backlog_level(u64::MAX, 100, 0),
            1,
            "hard disabled caps at 1"
        );
    }

    #[test]
    fn backlog_level_hard_below_soft_still_reports_hard() {
        // Misconfiguration: hard < soft. The severe verdict wins above hard.
        assert_eq!(backlog_level(150, 200, 100), 2);
        assert_eq!(backlog_level(50, 200, 100), 0);
    }
}
