//! Unified metrics framework for DELILA components
//!
//! # Design Principles (KISS)
//! - Lock-free atomic counters for hot path (zero overhead on data path)
//! - Simple snapshot mechanism for reporting
//! - Each component can extend with custom fields if needed

use std::sync::atomic::{AtomicU64, Ordering};

/// Common atomic counters used across all pipeline components
///
/// This provides the core metrics that every component tracks:
/// - Received: items coming in
/// - Processed/Sent: items going out successfully
/// - Dropped: items lost due to backpressure
///
/// All operations use Relaxed ordering for maximum performance.
/// Statistics are eventually consistent, which is acceptable for monitoring.
#[derive(Debug)]
pub struct AtomicCounters {
    /// Batches/items received from upstream
    pub received: AtomicU64,
    /// Batches/items successfully processed/sent
    pub processed: AtomicU64,
    /// Batches/items dropped due to backpressure
    pub dropped: AtomicU64,
    /// Events received (sum of all events in batches)
    pub events_received: AtomicU64,
    /// Events processed/sent
    pub events_processed: AtomicU64,
    /// Bytes transferred
    pub bytes: AtomicU64,
}

impl AtomicCounters {
    /// Create new zeroed counters
    pub fn new() -> Self {
        Self {
            received: AtomicU64::new(0),
            processed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            events_received: AtomicU64::new(0),
            events_processed: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }

    /// Increment received batch counter
    #[inline]
    pub fn inc_received(&self) {
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment received counter by n
    #[inline]
    pub fn add_received(&self, n: u64) {
        self.received.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment processed counter
    #[inline]
    pub fn inc_processed(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment processed counter by n
    #[inline]
    pub fn add_processed(&self, n: u64) {
        self.processed.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment dropped counter
    #[inline]
    pub fn inc_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Add to events received
    #[inline]
    pub fn add_events_received(&self, n: u64) {
        self.events_received.fetch_add(n, Ordering::Relaxed);
    }

    /// Add to events processed
    #[inline]
    pub fn add_events_processed(&self, n: u64) {
        self.events_processed.fetch_add(n, Ordering::Relaxed);
    }

    /// Add to bytes counter
    #[inline]
    pub fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }

    /// Take a snapshot of current values
    pub fn snapshot(&self) -> CounterSnapshot {
        CounterSnapshot {
            received: self.received.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            events_received: self.events_received.load(Ordering::Relaxed),
            events_processed: self.events_processed.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
        }
    }

    /// Reset all counters to zero
    pub fn reset(&self) {
        self.received.store(0, Ordering::Relaxed);
        self.processed.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.events_received.store(0, Ordering::Relaxed);
        self.events_processed.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
    }
}

impl Default for AtomicCounters {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of counter values at a point in time
#[derive(Debug, Clone, Copy, Default)]
pub struct CounterSnapshot {
    pub received: u64,
    pub processed: u64,
    pub dropped: u64,
    pub events_received: u64,
    pub events_processed: u64,
    pub bytes: u64,
}

impl CounterSnapshot {
    /// Calculate rate between two snapshots given elapsed seconds
    pub fn rate_from(&self, prev: &CounterSnapshot, elapsed_secs: f64) -> RateSnapshot {
        if elapsed_secs <= 0.0 {
            return RateSnapshot::default();
        }

        RateSnapshot {
            received_rate: (self.received.saturating_sub(prev.received)) as f64 / elapsed_secs,
            processed_rate: (self.processed.saturating_sub(prev.processed)) as f64 / elapsed_secs,
            events_rate: (self.events_processed.saturating_sub(prev.events_processed)) as f64
                / elapsed_secs,
            bytes_rate: (self.bytes.saturating_sub(prev.bytes)) as f64 / elapsed_secs,
        }
    }
}

/// Rate calculations from counter snapshots
#[derive(Debug, Clone, Copy, Default)]
pub struct RateSnapshot {
    /// Batches received per second
    pub received_rate: f64,
    /// Batches processed per second
    pub processed_rate: f64,
    /// Events processed per second
    pub events_rate: f64,
    /// Bytes per second
    pub bytes_rate: f64,
}

impl RateSnapshot {
    /// Format bytes rate as human-readable string (KB/s, MB/s, etc.)
    pub fn format_bytes_rate(&self) -> String {
        if self.bytes_rate >= 1_000_000_000.0 {
            format!("{:.2} GB/s", self.bytes_rate / 1_000_000_000.0)
        } else if self.bytes_rate >= 1_000_000.0 {
            format!("{:.2} MB/s", self.bytes_rate / 1_000_000.0)
        } else if self.bytes_rate >= 1_000.0 {
            format!("{:.2} KB/s", self.bytes_rate / 1_000.0)
        } else {
            format!("{:.0} B/s", self.bytes_rate)
        }
    }

    /// Format events rate as human-readable string (K/s, M/s, etc.)
    pub fn format_events_rate(&self) -> String {
        if self.events_rate >= 1_000_000.0 {
            format!("{:.2} M/s", self.events_rate / 1_000_000.0)
        } else if self.events_rate >= 1_000.0 {
            format!("{:.2} K/s", self.events_rate / 1_000.0)
        } else {
            format!("{:.0} /s", self.events_rate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_counters_new() {
        let counters = AtomicCounters::new();
        assert_eq!(counters.received.load(Ordering::Relaxed), 0);
        assert_eq!(counters.processed.load(Ordering::Relaxed), 0);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_inc_methods() {
        let counters = AtomicCounters::new();
        counters.inc_received();
        counters.inc_received();
        counters.inc_processed();
        counters.inc_dropped();

        assert_eq!(counters.received.load(Ordering::Relaxed), 2);
        assert_eq!(counters.processed.load(Ordering::Relaxed), 1);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_add_methods() {
        let counters = AtomicCounters::new();
        counters.add_received(10);
        counters.add_processed(5);
        counters.add_events_received(100);
        counters.add_events_processed(95);
        counters.add_bytes(1000);

        let snap = counters.snapshot();
        assert_eq!(snap.received, 10);
        assert_eq!(snap.processed, 5);
        assert_eq!(snap.events_received, 100);
        assert_eq!(snap.events_processed, 95);
        assert_eq!(snap.bytes, 1000);
    }

    #[test]
    fn test_snapshot() {
        let counters = AtomicCounters::new();
        counters.add_received(100);
        counters.add_processed(90);
        counters.inc_dropped();

        let snap = counters.snapshot();
        assert_eq!(snap.received, 100);
        assert_eq!(snap.processed, 90);
        assert_eq!(snap.dropped, 1);
    }

    #[test]
    fn test_reset() {
        let counters = AtomicCounters::new();
        counters.add_received(100);
        counters.add_processed(50);
        counters.reset();

        let snap = counters.snapshot();
        assert_eq!(snap.received, 0);
        assert_eq!(snap.processed, 0);
    }

    #[test]
    fn test_rate_calculation() {
        let prev = CounterSnapshot {
            received: 100,
            processed: 90,
            dropped: 1,
            events_received: 1000,
            events_processed: 900,
            bytes: 10000,
        };

        let current = CounterSnapshot {
            received: 200,
            processed: 180,
            dropped: 2,
            events_received: 2000,
            events_processed: 1800,
            bytes: 20000,
        };

        let rate = current.rate_from(&prev, 1.0);
        assert_eq!(rate.received_rate, 100.0);
        assert_eq!(rate.processed_rate, 90.0);
        assert_eq!(rate.events_rate, 900.0);
        assert_eq!(rate.bytes_rate, 10000.0);
    }

    #[test]
    fn test_rate_with_elapsed_time() {
        let prev = CounterSnapshot::default();
        let current = CounterSnapshot {
            received: 100,
            processed: 80,
            dropped: 0,
            events_received: 1000,
            events_processed: 800,
            bytes: 10000,
        };

        // 2 second interval
        let rate = current.rate_from(&prev, 2.0);
        assert_eq!(rate.received_rate, 50.0);
        assert_eq!(rate.processed_rate, 40.0);
        assert_eq!(rate.events_rate, 400.0);
        assert_eq!(rate.bytes_rate, 5000.0);
    }

    #[test]
    fn test_rate_zero_elapsed() {
        let prev = CounterSnapshot::default();
        let current = CounterSnapshot {
            received: 100,
            ..Default::default()
        };

        let rate = current.rate_from(&prev, 0.0);
        assert_eq!(rate.received_rate, 0.0);
        assert_eq!(rate.bytes_rate, 0.0);
    }

    #[test]
    fn test_format_bytes_rate() {
        let rate = RateSnapshot {
            bytes_rate: 500.0,
            ..Default::default()
        };
        assert_eq!(rate.format_bytes_rate(), "500 B/s");

        let rate = RateSnapshot {
            bytes_rate: 1500.0,
            ..Default::default()
        };
        assert_eq!(rate.format_bytes_rate(), "1.50 KB/s");

        let rate = RateSnapshot {
            bytes_rate: 1_500_000.0,
            ..Default::default()
        };
        assert_eq!(rate.format_bytes_rate(), "1.50 MB/s");

        let rate = RateSnapshot {
            bytes_rate: 1_500_000_000.0,
            ..Default::default()
        };
        assert_eq!(rate.format_bytes_rate(), "1.50 GB/s");
    }

    #[test]
    fn test_format_events_rate() {
        let rate = RateSnapshot {
            events_rate: 500.0,
            ..Default::default()
        };
        assert_eq!(rate.format_events_rate(), "500 /s");

        let rate = RateSnapshot {
            events_rate: 1500.0,
            ..Default::default()
        };
        assert_eq!(rate.format_events_rate(), "1.50 K/s");

        let rate = RateSnapshot {
            events_rate: 1_500_000.0,
            ..Default::default()
        };
        assert_eq!(rate.format_events_rate(), "1.50 M/s");
    }
}
