// eb_core.hpp — multithreaded event-builder core for root_sink (TODO 66 Phase 1).
//
// This header holds every piece of the Receiver → Sorter → Workers → Writer
// pipeline that can exist without ROOT or ZMQ: the inter-thread channel, the
// sequence reorderer, the sorted-chunk splitter (a faithful port of the frozen
// Rust reference in src/event_builder/chunk_builder.rs, including the TODO 58
// H10 chunk-boundary rules), and the data-time-driven Sorter. Everything here
// is unit-testable with a plain `g++ -std=c++17 -pthread` build — see
// test_eb_core.cpp. The thread spawning and the ROOT/ZMQ wiring live in
// root_sink.cxx.
//
// Design constraints inherited from the project rules (CLAUDE.md / TODO 66 §5):
//   * Record-path channels are UNBOUNDED — data is never dropped between the
//     ZMQ socket and the ROOT file. Only the display tee may be bounded, and
//     its drops must be counted by the caller (Monitor-exception policy).
//   * Chunks deliberately carry duplicated context hits (the lookback overlap
//     and the post-core tail). Consumers that must see each hit exactly once
//     (the hits tree) use core_range(); consumers that need coincidence
//     context (the builder) scan the full chunk. Do not "fix" the overlap.
//
// License: BSD-3-Clause (same as delila-rs).

#ifndef ROOTSINK_EB_CORE_HPP
#define ROOTSINK_EB_CORE_HPP

#include <algorithm>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <limits>
#include <map>
#include <mutex>
#include <utility>
#include <vector>

#include "sink_core.hpp"  // rootsink::ScalarHit

namespace rootsink {
namespace eb {

// ---------------------------------------------------------------------------
// 1. Channel<T> — the one inter-thread queue (mutex + condvar + deque)
// ---------------------------------------------------------------------------
//
// capacity == 0 means unbounded (the record path: push never blocks, nothing
// is ever dropped). A bounded channel (the display tee) offers two flavors:
//   * try_push — returns false when full WITHOUT consuming the value, so the
//     caller can count the drop and keep the original (Monitor-exception
//     bookkeeping).
//   * push     — blocks until space; reserved for control markers (RunOpen /
//     RunClose / Shutdown), which must never be dropped even on the display
//     path (a lost RunOpen would leave stale matcher clocks across runs).
// Message granularity is a whole batch/chunk, so lock traffic is a few dozen
// operations per second — a lock-free structure would buy nothing here (KISS).
template <class T>
class Channel {
 public:
  explicit Channel(std::size_t capacity = 0) : cap_(capacity) {}

  // Blocking push. Unbounded: appends immediately. Bounded: waits for space.
  // After close() the value is silently discarded — producers may still be
  // running during shutdown and have nowhere else to put it.
  void push(T&& v) {
    std::unique_lock<std::mutex> lk(m_);
    if (cap_ > 0) cv_push_.wait(lk, [&] { return q_.size() < cap_ || closed_; });
    if (closed_) return;
    q_.push_back(std::move(v));
    cv_pop_.notify_one();
  }

  // Non-blocking push for the bounded display data path. Returns false when
  // full and leaves `v` UNTOUCHED (no move happened) so the caller can count
  // the drop. On a closed channel the value is discarded and true is returned:
  // shutdown-time discards are not display drops and must not inflate the
  // counter.
  bool try_push(T&& v) {
    std::lock_guard<std::mutex> lk(m_);
    if (closed_) return true;
    if (cap_ > 0 && q_.size() >= cap_) return false;
    q_.push_back(std::move(v));
    cv_pop_.notify_one();
    return true;
  }

  // Blocking pop. Returns false only when the channel is closed AND drained —
  // the normal thread-exit signal for abnormal termination. (Orderly shutdown
  // flows through in-band Ctrl::Shutdown markers instead.)
  bool pop(T& out) {
    std::unique_lock<std::mutex> lk(m_);
    cv_pop_.wait(lk, [&] { return !q_.empty() || closed_; });
    if (q_.empty()) return false;  // closed and drained
    out = std::move(q_.front());
    q_.pop_front();
    if (cap_ > 0) cv_push_.notify_one();
    return true;
  }

  // Pop with a timeout, for loops that must wake periodically (the Display
  // loop's ProcessEvents tick, the Writer's autosave timer). Returns false on
  // timeout OR on closed-and-drained; callers treat false as "no work now"
  // and check their own exit conditions.
  bool pop_for(T& out, int timeout_ms) {
    std::unique_lock<std::mutex> lk(m_);
    cv_pop_.wait_for(lk, std::chrono::milliseconds(timeout_ms),
                     [&] { return !q_.empty() || closed_; });
    if (q_.empty()) return false;
    out = std::move(q_.front());
    q_.pop_front();
    if (cap_ > 0) cv_push_.notify_one();
    return true;
  }

  // Wake every blocked producer and consumer. Pending items remain poppable
  // (drain-then-false semantics in pop()).
  void close() {
    std::lock_guard<std::mutex> lk(m_);
    closed_ = true;
    cv_pop_.notify_all();
    cv_push_.notify_all();
  }

  // Instantaneous depth — status-line gauge only (racy by nature, fine).
  std::size_t size() const {
    std::lock_guard<std::mutex> lk(m_);
    return q_.size();
  }

 private:
  mutable std::mutex m_;
  std::condition_variable cv_pop_, cv_push_;
  std::deque<T> q_;
  std::size_t cap_;  // 0 = unbounded
  bool closed_ = false;
};

// ---------------------------------------------------------------------------
// 2. SeqReorder<T> — restore Sorter emission order after the worker pool
// ---------------------------------------------------------------------------
//
// Workers finish out of order; the Writer pushes each result in with its
// Sorter-assigned sequence number and pops back the contiguous prefix. This
// makes the output deterministic in the worker count (same input → same tree,
// --workers 1 or 8) and turns in-band control markers into barriers for free:
// a RunClose with seq k cannot pop before every chunk with seq < k.
//
// NOT thread-safe — it lives entirely inside the Writer thread.
template <class T>
class SeqReorder {
 public:
  void push(uint64_t seq, T&& v) { buf_.emplace(seq, std::move(v)); }

  // True while the next contiguous sequence number (starting at 0) is
  // buffered; `out` receives that item.
  bool pop_ready(T& out) {
    auto it = buf_.find(next_);
    if (it == buf_.end()) return false;
    out = std::move(it->second);
    buf_.erase(it);
    ++next_;
    return true;
  }

  std::size_t pending() const { return buf_.size(); }

 private:
  std::map<uint64_t, T> buf_;
  uint64_t next_ = 0;
};

// ---------------------------------------------------------------------------
// 3. SortedChunk + split / flush / core_range
// ---------------------------------------------------------------------------
//
// Port of src/event_builder/chunk_builder.rs (SortedChunk, sort_and_split,
// sort_and_flush) — the frozen Rust reference that carries the TODO 58 H10
// chunk-boundary fixes. Semantics preserved exactly; see the Rust file for
// the original derivation.

struct SortedChunk {
  std::vector<ScalarHit> hits;  // ascending timestamp_ns; includes context
  // Emit floor [ns] (H10): triggers before this were already emitted by the
  // previous chunk — they are present here only as backward-window context
  // carried in by the lookback overlap. First chunk of a run: -infinity.
  double core_start = -std::numeric_limits<double>::infinity();
  // Emit ceiling [ns]: triggers at or after this belong to the next chunk,
  // but their hits remain visible as coincidence partners in this one.
  double core_end = std::numeric_limits<double>::infinity();
};

namespace detail {
inline bool ts_less(const ScalarHit& a, const ScalarHit& b) {
  // Plain < is safe: timestamps come off the wire as finite f64 (the digitizer
  // never produces NaN), so total_cmp-style NaN ordering is not needed here.
  return a.timestamp_ns < b.timestamp_ns;
}
}  // namespace detail

// Sort `buffer` and split off a chunk whose core ends safe_horizon_ns before
// the newest hit. On success (returns true): `chunk` takes ALL of the buffer's
// hits, and `buffer` is replaced by the retained tail starting at
// core_end - lookback_ns — the [core_end - lookback, core_end) overlap rides
// along purely as context; the next chunk's core_start (== this core_end)
// keeps those hits from emitting twice (H10). On failure (empty buffer, or
// core_end would not clear the earliest hit) returns false with `buffer`
// sorted but otherwise untouched.
inline bool sort_and_split(std::vector<ScalarHit>& buffer, double safe_horizon_ns,
                           double lookback_ns, double core_start,
                           SortedChunk& chunk) {
  if (buffer.empty()) return false;
  std::sort(buffer.begin(), buffer.end(), detail::ts_less);
  double earliest = buffer.front().timestamp_ns;
  double latest = buffer.back().timestamp_ns;
  double core_end = latest - safe_horizon_ns;
  if (core_end <= earliest) return false;  // not enough spread yet
  // No core progress since the previous split (no newer data arrived): refuse.
  // Without this guard, a caller looping "split until false" would carve
  // zero-width chunks out of the retained lookback tail forever. (The Rust
  // original relies on its caller's span check for this; here the invariant
  // "a chunk always advances the core" lives in the primitive itself.)
  if (core_end <= core_start) return false;

  auto split = std::lower_bound(
      buffer.begin(), buffer.end(), core_end - lookback_ns,
      [](const ScalarHit& h, double v) { return h.timestamp_ns < v; });
  std::vector<ScalarHit> retained(split, buffer.end());

  chunk.hits = std::move(buffer);
  chunk.core_start = core_start;
  chunk.core_end = core_end;
  buffer = std::move(retained);
  return true;
}

// EOS / shutdown flush: everything left becomes one final chunk with
// core_end = +infinity. The core_start floor still applies — lookback hits
// carried into this buffer were already emitted by the previous chunk.
// Returns false when there is nothing to flush.
inline bool sort_and_flush(std::vector<ScalarHit>& buffer, double core_start,
                           SortedChunk& chunk) {
  if (buffer.empty()) return false;
  std::sort(buffer.begin(), buffer.end(), detail::ts_less);
  chunk.hits = std::move(buffer);
  buffer.clear();
  chunk.core_start = core_start;
  chunk.core_end = std::numeric_limits<double>::infinity();
  return true;
}

// [first, last) index range of the hits owned by this chunk's core region
// [core_start, core_end). THE dedup rule for exactly-once consumers: over a
// run's chunk sequence (chained core_start == previous core_end, final flush
// at +inf), every hit falls in exactly one chunk's core range — the Writer
// fills the hits tree from this range only, never from the full chunk.
inline std::pair<std::size_t, std::size_t> core_range(const SortedChunk& c) {
  auto lo = std::lower_bound(
      c.hits.begin(), c.hits.end(), c.core_start,
      [](const ScalarHit& h, double v) { return h.timestamp_ns < v; });
  auto hi = std::lower_bound(
      lo, c.hits.end(), c.core_end,
      [](const ScalarHit& h, double v) { return h.timestamp_ns < v; });
  return {static_cast<std::size_t>(lo - c.hits.begin()),
          static_cast<std::size_t>(hi - c.hits.begin())};
}

// ---------------------------------------------------------------------------
// 4. Sorter — accumulate, sort, emit chunks on DATA-TIME advance
// ---------------------------------------------------------------------------
//
// The single owner of the disorder buffer. Emission is driven purely by data
// time (the watermark advancing), never by hit count or wall clock: the rate
// can rise or fall and chunks keep a constant time span, and a silent beam
// pause simply pauses emission (EOS flushes the remainder). One split per
// push_batch is sufficient — sort_and_split advances core_end all the way to
// max_ts - safe_horizon, so the emit condition cannot re-fire until new data
// arrives.
class Sorter {
 public:
  struct Config {
    double chunk_span_ns = 100e6;    // emit when the core grows this much
    double safe_horizon_ns = 50e6;   // disorder absorption (>= max disorder)
    double lookback_ns = 1000.0;     // context carry (>= coincidence window)
  };

  explicit Sorter(Config cfg) : cfg_(cfg) {}

  // Append a decoded batch (any internal order) and emit at most one chunk.
  template <class ChunkF>
  void push_batch(std::vector<ScalarHit>&& hits, ChunkF on_chunk) {
    for (const ScalarHit& h : hits) {
      if (h.timestamp_ns > max_ts_) max_ts_ = h.timestamp_ns;
      if (h.timestamp_ns < min_ts_) min_ts_ = h.timestamp_ns;
    }
    if (buffer_.empty()) {
      buffer_ = std::move(hits);
    } else {
      buffer_.insert(buffer_.end(), std::make_move_iterator(hits.begin()),
                     std::make_move_iterator(hits.end()));
    }
    if (buffer_.empty()) return;

    double candidate = max_ts_ - cfg_.safe_horizon_ns;
    // Floor: where the previous core ended, or (first chunk) the earliest hit
    // ever buffered this run. The emit test against the floor is what makes
    // emission data-time-driven.
    double floor = std::isinf(prev_core_end_) ? min_ts_ : prev_core_end_;
    if (candidate - floor < cfg_.chunk_span_ns) return;

    SortedChunk chunk;
    if (sort_and_split(buffer_, cfg_.safe_horizon_ns, cfg_.lookback_ns,
                       prev_core_end_, chunk)) {
      prev_core_end_ = chunk.core_end;
      on_chunk(std::move(chunk));
    }
  }

  // Emit everything left as a final chunk (RunClose / Shutdown).
  template <class ChunkF>
  void flush(ChunkF on_chunk) {
    SortedChunk chunk;
    if (sort_and_flush(buffer_, prev_core_end_, chunk)) {
      on_chunk(std::move(chunk));
    }
  }

  // Call at RunOpen. CRITICAL: prev_core_end_ MUST return to -infinity — the
  // digitizer clock restarts between runs, and a stale floor from run N's huge
  // timestamps would put every run-N+1 hit below core_start, silently emitting
  // nothing (no built events AND an empty hits tree) for the entire run.
  void reset() {
    buffer_.clear();
    max_ts_ = -std::numeric_limits<double>::infinity();
    min_ts_ = std::numeric_limits<double>::infinity();
    prev_core_end_ = -std::numeric_limits<double>::infinity();
  }

  std::size_t buffered() const { return buffer_.size(); }

 private:
  Config cfg_;
  std::vector<ScalarHit> buffer_;
  double max_ts_ = -std::numeric_limits<double>::infinity();
  double min_ts_ = std::numeric_limits<double>::infinity();
  double prev_core_end_ = -std::numeric_limits<double>::infinity();
};

}  // namespace eb
}  // namespace rootsink

#endif  // ROOTSINK_EB_CORE_HPP
