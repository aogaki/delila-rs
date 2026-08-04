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

// ---------------------------------------------------------------------------
// 5. Integrated-event builder (pure function over a SortedChunk)
// ---------------------------------------------------------------------------
//
// One BuiltEvent row carries BOTH ThGEM detectors plus the LaBr3 gamma:
// completeness is an offline cut, never a build condition (TODO 66 §5.2).
// A missing piece is NaN (times/derived) or 0 (energies) — 4-fold analysis is
// `!isnan(x1) && !isnan(y1)`, plane correlation is `!isnan(dt_trig)`, and the
// arm-efficiency map falls out of n_arms for free. Trigger-only events are
// therefore emitted too; dropping them would destroy the efficiency
// measurement, so don't.
//
// Matching semantics are the offline ground truth (tmp/test.cpp — the code
// that produced the 15,644 four-fold reference count on run0023, and
// macros/grid_resolution.C): per arm, the NEAREST hit to that detector's own
// trigger within ±window_ns (inclusive), via lower_bound predecessor/successor
// comparison. NOT LiveEventBuilder.C's first-found-backward rule. The X/Y
// convention is x = t_XR - t_XL, y = t_YU - t_YD in raw ns — LiveEventBuilder's
// X=(t4-t5) is that macro's deliberate 90-degree rotation; do not copy it, and
// do not bake the mm calibration or the -70 ns TOF offset in here (analysis
// constants live in macros).
//
// Cross-plane pairing (the "integrated event"): both trigger channels are
// scanned as one time-ordered sequence; a trigger pairs with the IMMEDIATELY
// NEXT trigger when that one belongs to the other detector and lies within
// window_ns (greedy, first-come — genuine coincidences sit ~ns apart while
// the window is ~us, so nearest-pair refinement would change nothing real).
// Same-detector triggers never merge (two ch1 triggers = two events). The
// event's anchor is the EARLIER trigger of the pair; dt_trig is ALWAYS
// t(det2) - t(det1) regardless of which side anchored.
//
// Chunk-boundary determinism (H10): pairing and arm matching run over the
// FULL chunk (context included), but an event is emitted only when its anchor
// falls in [core_start, core_end). A pair spanning core_end is emitted by the
// chunk that owns the anchor; in the next chunk the anchor reappears as
// lookback context (lookback >= window guarantees it), pairs again, and is
// gated out by core_start — so the partner never anchors a duplicate.
// Requirement on the caller: lookback_ns >= window_ns and
// safe_horizon_ns >= 2*window_ns (trivially true at 50 ms vs ~us windows).

struct BuiltEvent {
  // Times are digitizer-clock ns. NaN = that piece is absent from the event.
  double trig1_t = std::numeric_limits<double>::quiet_NaN();
  double trig2_t = std::numeric_limits<double>::quiet_NaN();
  double labr_t = std::numeric_limits<double>::quiet_NaN();
  // Positions in raw ns: xN = dt_xrN - dt_xlN, yN = dt_yuN - dt_ydN — valid
  // only when BOTH arms of that axis matched.
  float x1 = std::numeric_limits<float>::quiet_NaN();
  float y1 = std::numeric_limits<float>::quiet_NaN();
  float x2 = std::numeric_limits<float>::quiet_NaN();
  float y2 = std::numeric_limits<float>::quiet_NaN();
  // Per-arm deltas dt = t(arm) - t(own trigger); NaN = arm not matched.
  float dt_xl1 = std::numeric_limits<float>::quiet_NaN();
  float dt_xr1 = std::numeric_limits<float>::quiet_NaN();
  float dt_yu1 = std::numeric_limits<float>::quiet_NaN();
  float dt_yd1 = std::numeric_limits<float>::quiet_NaN();
  float dt_xl2 = std::numeric_limits<float>::quiet_NaN();
  float dt_xr2 = std::numeric_limits<float>::quiet_NaN();
  float dt_yu2 = std::numeric_limits<float>::quiet_NaN();
  float dt_yd2 = std::numeric_limits<float>::quiet_NaN();
  // Plane-to-plane: t(det2 trigger) - t(det1 trigger); NaN unless both fired.
  float dt_trig = std::numeric_limits<float>::quiet_NaN();
  // Raw TOFs: t(trigger) - t(labr), no analysis offset applied.
  float tof1 = std::numeric_limits<float>::quiet_NaN();
  float tof2 = std::numeric_limits<float>::quiet_NaN();
  uint16_t e_trig1 = 0, e_trig2 = 0, e_labr = 0;
  uint16_t e_xl1 = 0, e_xr1 = 0, e_yu1 = 0, e_yd1 = 0;
  uint16_t e_xl2 = 0, e_xr2 = 0, e_yu2 = 0, e_yd2 = 0;
  uint32_t e_sum1 = 0, e_sum2 = 0;  // sum of PRESENT arms (4x65535 > u16)
  uint8_t n_arms1 = 0, n_arms2 = 0;  // 0..4 matched arms (LaBr not counted)
};

struct BuilderConfig {
  // Detector 1 (--xy1-ch T,XL,XR,YU,YD). -1 = channel unused.
  int trig1 = -1, xl1 = -1, xr1 = -1, yu1 = -1, yd1 = -1;
  // Detector 2 (--xy2-ch); trig2 = -1 => single-detector mode.
  int trig2 = -1, xl2 = -1, xr2 = -1, yu2 = -1, yd2 = -1;
  int labr_ch = -1;  // --gamma-ch; -1 => tof1/tof2/labr_t stay NaN
  double window_ns = 500.0;
};

namespace detail {

// Per-channel column view: times ascending (inherited from the sorted chunk)
// with the matching energies alongside.
struct ChView {
  std::vector<double> t;
  std::vector<uint16_t> e;
};

// Nearest hit to t0 within ±window (inclusive): lower_bound successor, then
// take the predecessor when it is STRICTLY closer (grid_resolution.C's rule —
// ties go to the successor). dt = t(hit) - t0, signed.
inline bool nearest_in(const ChView& v, double t0, double window, double& dt,
                       uint16_t& energy) {
  if (v.t.empty()) return false;
  std::size_t idx;
  auto it = std::lower_bound(v.t.begin(), v.t.end(), t0);
  if (it == v.t.end()) {
    idx = v.t.size() - 1;
  } else {
    idx = static_cast<std::size_t>(it - v.t.begin());
    if (idx > 0 && (t0 - v.t[idx - 1]) < (v.t[idx] - t0)) idx = idx - 1;
  }
  double d = v.t[idx] - t0;
  double ad = d < 0 ? -d : d;
  if (ad > window) return false;
  dt = d;
  energy = v.e[idx];
  return true;
}

}  // namespace detail

inline std::vector<BuiltEvent> build_events_from_chunk(const SortedChunk& chunk,
                                                       const BuilderConfig& cfg) {
  std::vector<BuiltEvent> out;
  if (chunk.hits.empty()) return out;

  // Distribute the sorted chunk into per-channel columns (sorted for free).
  detail::ChView vt1, vxl1, vxr1, vyu1, vyd1;
  detail::ChView vt2, vxl2, vxr2, vyu2, vyd2;
  detail::ChView vlabr;
  auto grab = [](detail::ChView& v, const ScalarHit& h) {
    v.t.push_back(h.timestamp_ns);
    v.e.push_back(h.energy);
  };
  for (const ScalarHit& h : chunk.hits) {
    int ch = static_cast<int>(h.channel);
    if (ch == cfg.trig1) grab(vt1, h);
    else if (ch == cfg.xl1) grab(vxl1, h);
    else if (ch == cfg.xr1) grab(vxr1, h);
    else if (ch == cfg.yu1) grab(vyu1, h);
    else if (ch == cfg.yd1) grab(vyd1, h);
    else if (ch == cfg.trig2) grab(vt2, h);
    else if (ch == cfg.xl2) grab(vxl2, h);
    else if (ch == cfg.xr2) grab(vxr2, h);
    else if (ch == cfg.yu2) grab(vyu2, h);
    else if (ch == cfg.yd2) grab(vyd2, h);
    else if (ch == cfg.labr_ch) grab(vlabr, h);
  }

  // Merge both trigger columns into one time-ordered sequence (ties: det1
  // first — deterministic).
  struct Trig {
    double t;
    uint16_t e;
    int det;  // 1 or 2
  };
  std::vector<Trig> trigs;
  trigs.reserve(vt1.t.size() + vt2.t.size());
  {
    std::size_t i = 0, j = 0;
    while (i < vt1.t.size() || j < vt2.t.size()) {
      bool take1 = j >= vt2.t.size() ||
                   (i < vt1.t.size() && vt1.t[i] <= vt2.t[j]);
      if (take1) {
        trigs.push_back({vt1.t[i], vt1.e[i], 1});
        ++i;
      } else {
        trigs.push_back({vt2.t[j], vt2.e[j], 2});
        ++j;
      }
    }
  }

  // Match one detector side of an event: arms nearest to that detector's own
  // trigger, x/y from complete axes, energy sum over present arms.
  auto fill_det = [&](const Trig& tr, const detail::ChView& xl,
                      const detail::ChView& xr, const detail::ChView& yu,
                      const detail::ChView& yd, double& trig_t, uint16_t& e_trig,
                      float& dxl, float& dxr, float& dyu, float& dyd, float& x,
                      float& y, uint16_t& exl, uint16_t& exr, uint16_t& eyu,
                      uint16_t& eyd, uint32_t& e_sum, uint8_t& n_arms) {
    trig_t = tr.t;
    e_trig = tr.e;
    double d = 0.0;
    uint16_t en = 0;
    if (detail::nearest_in(xl, tr.t, cfg.window_ns, d, en)) {
      dxl = static_cast<float>(d); exl = en; e_sum += en; ++n_arms;
    }
    if (detail::nearest_in(xr, tr.t, cfg.window_ns, d, en)) {
      dxr = static_cast<float>(d); exr = en; e_sum += en; ++n_arms;
    }
    if (detail::nearest_in(yu, tr.t, cfg.window_ns, d, en)) {
      dyu = static_cast<float>(d); eyu = en; e_sum += en; ++n_arms;
    }
    if (detail::nearest_in(yd, tr.t, cfg.window_ns, d, en)) {
      dyd = static_cast<float>(d); eyd = en; e_sum += en; ++n_arms;
    }
    if (!std::isnan(dxl) && !std::isnan(dxr)) x = dxr - dxl;
    if (!std::isnan(dyu) && !std::isnan(dyd)) y = dyu - dyd;
  };

  // Greedy pairing scan + emission gate on the ANCHOR time (H10 gates).
  std::size_t k = 0;
  while (k < trigs.size()) {
    const Trig& a = trigs[k];
    const Trig* b = nullptr;
    if (k + 1 < trigs.size() && trigs[k + 1].det != a.det &&
        trigs[k + 1].t - a.t <= cfg.window_ns) {
      b = &trigs[k + 1];
    }
    std::size_t consumed = b ? 2 : 1;
    if (a.t >= chunk.core_start && a.t < chunk.core_end) {
      BuiltEvent ev;
      const Trig* t1 = (a.det == 1) ? &a : b;
      const Trig* t2 = (a.det == 2) ? &a : b;
      if (t1) {
        fill_det(*t1, vxl1, vxr1, vyu1, vyd1, ev.trig1_t, ev.e_trig1, ev.dt_xl1,
                 ev.dt_xr1, ev.dt_yu1, ev.dt_yd1, ev.x1, ev.y1, ev.e_xl1,
                 ev.e_xr1, ev.e_yu1, ev.e_yd1, ev.e_sum1, ev.n_arms1);
      }
      if (t2) {
        fill_det(*t2, vxl2, vxr2, vyu2, vyd2, ev.trig2_t, ev.e_trig2, ev.dt_xl2,
                 ev.dt_xr2, ev.dt_yu2, ev.dt_yd2, ev.x2, ev.y2, ev.e_xl2,
                 ev.e_xr2, ev.e_yu2, ev.e_yd2, ev.e_sum2, ev.n_arms2);
      }
      if (t1 && t2) ev.dt_trig = static_cast<float>(t2->t - t1->t);
      // One gamma per event: nearest LaBr3 hit to the anchor; both TOFs come
      // from that same hit (so tof2 - tof1 == dt_trig by construction).
      double dl = 0.0;
      uint16_t el = 0;
      if (detail::nearest_in(vlabr, a.t, cfg.window_ns, dl, el)) {
        ev.labr_t = a.t + dl;
        ev.e_labr = el;
        if (t1) ev.tof1 = static_cast<float>(t1->t - ev.labr_t);
        if (t2) ev.tof2 = static_cast<float>(t2->t - ev.labr_t);
      }
      out.push_back(ev);
    }
    k += consumed;
  }
  return out;
}

// ---------------------------------------------------------------------------
// 6. Pipeline message types (in-band control markers)
// ---------------------------------------------------------------------------
//
// Control flows THROUGH the data path with sequence numbers, never around it:
// the Writer's SeqReorder then guarantees that a RunClose is processed only
// after every chunk of that run is written (a barrier for free) and that
// Shutdown pills drain all real work first.

enum class Ctrl : uint8_t { None, RunOpen, RunClose, Shutdown };

// Receiver -> Sorter. None carries a decoded batch; markers carry no hits.
struct SorterMsg {
  Ctrl ctrl = Ctrl::None;
  std::vector<ScalarHit> hits;
  uint32_t run_number = 0;  // RunClose: from the EOS envelope
};

// Sorter -> Workers -> Writer. Every message (data AND control) gets a seq.
struct WorkMsg {
  Ctrl ctrl = Ctrl::None;
  uint64_t seq = 0;
  SortedChunk chunk;             // None: the sorted chunk
  std::vector<BuiltEvent> built; // filled by a worker when building is on
  uint32_t run_number = 0;       // RunClose
};

// Receiver -> Display (bounded tee). Data may be dropped-and-counted; control
// markers must use the blocking push (see Channel notes).
struct DisplayMsg {
  Ctrl ctrl = Ctrl::None;
  std::vector<ScalarHit> hits;
};

}  // namespace eb
}  // namespace rootsink

#endif  // ROOTSINK_EB_CORE_HPP
