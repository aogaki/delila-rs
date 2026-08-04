// sink_core.hpp — pure logic for the DELILA `root_sink` tool.
//
// This header deliberately depends ONLY on `TDelila.hpp` (for its zero-dependency
// `tdelila::mp::Reader`) and the C++ standard library. It contains NO ROOT and NO
// ZMQ code, so every piece here is unit-testable with a plain `g++ -std=c++17`
// build (see tools/root_sink/README.md and the scratchpad self-test). The ROOT /
// libzmq wiring lives in root_sink.cxx.
//
// What lives here:
//   1. Envelope parser  — split a ZMQ message into Data / EndOfStream / Heartbeat.
//   2. Batch decoder    — walk an EventDataBatch into flat ScalarHit records.
//   3. CoincidenceMatcher — pure Δt logic (gamma vs ThGEM1/ThGEM2), no histograms.
//   3b. PositionMatcher — delay-line XY (trigger + XL/XR/YU/YD arms), no histograms.
//   4. RunState         — Idle→Writing state machine driven by Data/EOS.
//
// Wire format (empirically confirmed against rmp-serde 1.3.1, the repo's version):
//   Each ZMQ message is one Rust `Message` enum value, encoded by rmp-serde as a
//   fixmap(1)  { variant_name(str) : payload }:
//     Data        -> 0x81 0xa4 "Data"        + EventDataBatch (positional array(4))
//     EndOfStream -> 0x81 0xab "EndOfStream" + array(2): [source_id u32, run_number u32]
//     Heartbeat   -> 0x81 0xa9 "Heartbeat"   + payload (skipped entirely)
//   EventDataBatch is the SAME rmp-serde "compact" MessagePack as a `.delila` data
//   block: a positional array in field-declaration order. Unlike `.delila` v3 files,
//   the ZMQ stream carries NO embedded schema header, so the EventData field order
//   is hardcoded here and MUST mirror src/common/delila_schema.rs (EVENT_DATA) and
//   the Rust `struct EventData` in src/common/mod.rs.
//
// License: BSD-3-Clause (same as delila-rs).

#ifndef ROOTSINK_SINK_CORE_HPP
#define ROOTSINK_SINK_CORE_HPP

#include <cstdint>
#include <cstdlib>
#include <deque>
#include <limits>
#include <set>
#include <string>
#include <vector>

#include "../delila2root/TDelila.hpp"  // tdelila::mp::Reader (header-only, no ROOT)

namespace rootsink {

using tdelila::mp::Reader;

// ---------------------------------------------------------------------------
// 1. Envelope parser
// ---------------------------------------------------------------------------

enum class MsgKind { Data, EndOfStream, Heartbeat, Unknown };

// Result of splitting one ZMQ message. For Data, `payload`/`payload_size` point
// at the EventDataBatch bytes (a slice of the caller's buffer — valid only while
// that buffer lives). For EndOfStream, `source_id`/`run_number` are filled.
// For Unknown, `variant` holds the offending name (or "<malformed>") so the
// caller can emit a one-shot warning.
struct Envelope {
  MsgKind kind = MsgKind::Unknown;
  std::string variant;                 // decoded variant name (Unknown/logging)
  const uint8_t* payload = nullptr;    // Data: EventDataBatch array header
  std::size_t payload_size = 0;        // Data: bytes remaining after the key
  uint32_t source_id = 0;              // EndOfStream
  uint32_t run_number = 0;             // EndOfStream
};

// Parse the fixmap(1) envelope. Never throws: malformed input yields
// {kind=Unknown, variant="<malformed>"} so the hot loop can log-and-continue.
inline Envelope parse_envelope(const uint8_t* data, std::size_t size) {
  Envelope env;
  try {
    Reader r(data, size);
    uint32_t m = r.read_map_len();  // Message enum = fixmap(1)
    if (m != 1) {
      env.kind = MsgKind::Unknown;
      env.variant = "<not-fixmap1>";
      return env;
    }
    std::string key = r.read_str();
    env.variant = key;
    if (key == "Data") {
      env.kind = MsgKind::Data;
      env.payload = r.ptr();
      env.payload_size = static_cast<std::size_t>((data + size) - r.ptr());
    } else if (key == "EndOfStream") {
      env.kind = MsgKind::EndOfStream;
      // EndOfStream { source_id: u32, run_number: u32 } -> positional array(2).
      // (The stale comment in src/common/mod.rs mentions only source_id; the
      //  struct actually has two fields and rmp-serde emits both.)
      uint32_t an = r.read_array_len();
      if (an > 0) env.source_id = static_cast<uint32_t>(r.read_int());
      if (an > 1) env.run_number = static_cast<uint32_t>(r.read_int());
    } else if (key == "Heartbeat") {
      env.kind = MsgKind::Heartbeat;  // payload skipped entirely by the caller
    } else {
      env.kind = MsgKind::Unknown;    // future/unknown variant
    }
  } catch (const std::exception&) {
    env.kind = MsgKind::Unknown;
    env.variant = "<malformed>";
  }
  return env;
}

// ---------------------------------------------------------------------------
// 2. Batch decoder
// ---------------------------------------------------------------------------

// One decoded scalar event: exactly the 5 fields the ROOT scalar tree stores.
struct ScalarHit {
  uint8_t module = 0;
  uint8_t channel = 0;
  uint16_t energy = 0;
  uint16_t energy_short = 0;
  double timestamp_ns = 0.0;
};

// Decode ONE EventData from the cursor into `h`.
//
// The field order below MUST mirror src/common/delila_schema.rs `EVENT_DATA`
// (and the Rust `struct EventData` in src/common/mod.rs), positionally:
//   0 module u8, 1 channel u8, 2 energy u16, 3 energy_short u16,
//   4 timestamp_ns f64, 5 flags u64, 6 user_info [u64;4], 7 waveform ?Waveform
// We decode 0..4 with the typed readers and skip_value() the rest (flags,
// user_info, waveform, and ANY future trailing field) so a schema addition on
// the Rust side never shifts the fields we care about. `user_info` and
// `waveform` both carry `#[serde(default)]`; waveform is always present on the
// wire (MessagePack nil when absent) since format v3. This mirrors how
// TDelila.hpp's Schema::build_default() lays out EventData — keep the two in
// sync if that fallback layout ever changes.
inline void decode_event(Reader& r, ScalarHit& h) {
  uint32_t n = r.read_array_len();
  if (n > 0) h.module = static_cast<uint8_t>(r.read_int());
  if (n > 1) h.channel = static_cast<uint8_t>(r.read_int());
  if (n > 2) h.energy = static_cast<uint16_t>(r.read_int());
  if (n > 3) h.energy_short = static_cast<uint16_t>(r.read_int());
  if (n > 4) h.timestamp_ns = r.read_f64();
  for (uint32_t i = 5; i < n; ++i) r.skip_value();  // flags, user_info, waveform, ...
}

// Decode an EventDataBatch payload. Calls `on_start(source_id)` once (before any
// event, so the caller can open its run file) then `on_hit(const ScalarHit&)`
// per event. Returns the event count, or -1 on malformed input.
//
// EventDataBatch positional array(4): [source_id u32, sequence_number u64,
// timestamp u64, events [EventData]] — mirror of src/common/delila_schema.rs
// EVENT_DATA_BATCH. sequence_number and timestamp are skipped.
template <class StartF, class HitF>
inline long decode_batch(const uint8_t* payload, std::size_t size, StartF on_start,
                         HitF on_hit) {
  try {
    Reader r(payload, size);
    uint32_t bn = r.read_array_len();
    uint32_t source_id = 0;
    if (bn > 0) source_id = static_cast<uint32_t>(r.read_int());
    on_start(source_id);
    if (bn > 1) r.skip_value();  // sequence_number
    if (bn > 2) r.skip_value();  // timestamp (batch creation, unix ns)
    if (bn <= 3) return 0;       // no events field present
    uint32_t nev = r.read_array_len();
    for (uint32_t i = 0; i < nev; ++i) {
      ScalarHit h;
      decode_event(r, h);
      on_hit(h);
    }
    return static_cast<long>(nev);
  } catch (const std::exception&) {
    return -1;
  }
}

// Convenience: decode a batch into a vector, returning source_id via out-param.
inline long decode_batch_into(const uint8_t* payload, std::size_t size,
                              uint32_t& source_id_out, std::vector<ScalarHit>& out) {
  return decode_batch(
      payload, size, [&](uint32_t sid) { source_id_out = sid; },
      [&](const ScalarHit& h) { out.push_back(h); });
}

// ---------------------------------------------------------------------------
// 3. Coincidence matcher (pure logic — no ROOT)
// ---------------------------------------------------------------------------

// A buffered hit inside the matcher: timestamp plus the energy we carry along so
// coincidence histograms can gate/plot on the partner's energy (see hist_config).
struct TimedHit {
  double t = 0.0;
  uint16_t energy = 0;
};

// One emitted coincidence result for a ripe gamma hit. The three energies let a
// declarative histogram gate Δt on gamma energy (the headline use case) or plot a
// partner energy — each partner energy is only meaningful when its has_dtN is set.
struct CoincResult {
  double gamma_t = 0.0;        // the gamma timestamp this result is about
  uint16_t gamma_energy = 0;   // always valid for the ripened gamma
  bool has_dt1 = false;        // a ThGEM1 partner was found within ±window
  double dt1 = 0.0;            // t(ThGEM1) - t(gamma)
  uint16_t thgem1_energy = 0;  // valid iff has_dt1
  bool has_dt2 = false;        // a ThGEM2 partner was found within ±window
  double dt2 = 0.0;            // t(ThGEM2) - t(gamma)
  uint16_t thgem2_energy = 0;  // valid iff has_dt2
};

// Streaming nearest-partner matcher. Hits are fed roughly in time order (arrival
// order); a `margin_ns` slack absorbs the small disorder a multi-source merge can
// introduce. A gamma is only "ripened" once no future hit could still be its best
// partner: watermark = (max timestamp seen) - margin_ns, and gammas whose
// timestamp is <= watermark are matched against every ThGEM1/ThGEM2 hit currently
// buffered, picking the CLOSEST within ±window_ns.
//
// Channel identity: the ThGEM test uses a SINGLE digitizer, so hits are matched
// on `channel` alone (module ignored). If this is ever reused across digitizers
// with overlapping channel numbers, extend the identity to (module, channel).
//
// Cost: each hit is pushed once and pruned once; find_closest scans only the
// partners still inside the active window, which is bounded by rate*margin. So
// this is O(1) amortized per hit for the intended rates.
class CoincidenceMatcher {
 public:
  struct Config {
    int gamma_ch = -1;    // -1 => disabled (channel 0..255 never equals -1)
    int thgem1_ch = -1;
    int thgem2_ch = -1;
    double window_ns = 1000.0;
    double margin_ns = 10000.0;
  };

  explicit CoincidenceMatcher(Config cfg) : cfg_(cfg) {}

  // Feed one hit (call in ~arrival order). `on_result(const CoincResult&)` is
  // invoked for every gamma that ripens as a consequence.
  template <class ResultF>
  void push(const ScalarHit& h, ResultF on_result) {
    double t = h.timestamp_ns;
    if (t > max_ts_) max_ts_ = t;
    int ch = static_cast<int>(h.channel);
    if (ch == cfg_.gamma_ch) {
      gamma_.push_back({t, h.energy});
    } else if (ch == cfg_.thgem1_ch) {
      t1_.push_back({t, h.energy});
    } else if (ch == cfg_.thgem2_ch) {
      t2_.push_back({t, h.energy});
    } else {
      return;  // not a monitored channel — nothing to buffer or ripen
    }
    double watermark = max_ts_ - cfg_.margin_ns;
    ripen(watermark, on_result);

    // Safety prune (bounds memory if the gamma channel goes silent): a ThGEM hit
    // at pt can only still pair with a NOT-yet-ripe gamma, whose timestamp is
    // > watermark, whose window lower bound is > watermark - window. So partners
    // at or below (watermark - window) can never match a future gamma -> drop.
    // (Ripening already ran, so no remaining gamma needs them.)
    double safe = watermark - cfg_.window_ns;
    prune(t1_, safe);
    prune(t2_, safe);
  }

  // Ripen every buffered gamma regardless of watermark. Call once at EOS so the
  // final partial window is not lost.
  template <class ResultF>
  void flush(ResultF on_result) {
    ripen(std::numeric_limits<double>::infinity(), on_result);
  }

  // Drop all buffered hits and forget the high-water timestamp. Call at a run
  // boundary: the digitizer clock can restart between runs, and a stale (huge)
  // max_ts_ would make the next run's gammas look instantly "ripe" and match
  // before their partners arrive. Monitor histograms persist across runs; this
  // per-run timing state does not.
  void reset() {
    gamma_.clear();
    t1_.clear();
    t2_.clear();
    max_ts_ = -std::numeric_limits<double>::infinity();
  }

 private:
  template <class ResultF>
  void ripen(double watermark, ResultF& on_result) {
    while (!gamma_.empty() && gamma_.front().t <= watermark) {
      TimedHit g = gamma_.front();
      gamma_.pop_front();
      CoincResult res;
      res.gamma_t = g.t;
      res.gamma_energy = g.energy;
      find_closest(t1_, g.t, res.has_dt1, res.dt1, res.thgem1_energy);
      find_closest(t2_, g.t, res.has_dt2, res.dt2, res.thgem2_energy);
      on_result(res);
      // Partners older than (gt - window) can never match this-or-later gammas.
      prune(t1_, g.t - cfg_.window_ns);
      prune(t2_, g.t - cfg_.window_ns);
    }
  }

  // Nearest partner to `gt` within ±window_ns; dt = partner - gt (signed), and
  // `energy` reports the chosen partner's energy (untouched when none matches).
  void find_closest(const std::deque<TimedHit>& partners, double gt, bool& has,
                    double& dt, uint16_t& energy) const {
    has = false;
    dt = 0.0;
    energy = 0;
    double best = 0.0;
    for (const TimedHit& p : partners) {
      double d = p.t - gt;
      double ad = d < 0 ? -d : d;
      if (ad <= cfg_.window_ns && (!has || ad < best)) {
        has = true;
        best = ad;
        dt = d;
        energy = p.energy;
      }
    }
  }

  static void prune(std::deque<TimedHit>& dq, double below) {
    while (!dq.empty() && dq.front().t < below) dq.pop_front();
  }

  Config cfg_;
  double max_ts_ = -std::numeric_limits<double>::infinity();
  std::deque<TimedHit> gamma_, t1_, t2_;
};

// ---------------------------------------------------------------------------
// 3b. Position matcher (delay-line XY — pure logic, no ROOT)
// ---------------------------------------------------------------------------

// One emitted XY result for a ripe delay-line trigger. dt_* = t(arm) - t(trig),
// signed, valid only when its has_* is set. x/y are NOT stored here: hist_config's
// value_of derives x = dt_xr - dt_xl (needs BOTH X arms) and y = dt_yu - dt_yd
// (needs BOTH Y arms), so a partial event can still fill 1D projections.
struct PosResult {
  double trig_t = 0.0;         // the trigger timestamp this result is about
  uint16_t trig_energy = 0;    // always valid for the ripened trigger
  bool has_xl = false; double dt_xl = 0.0;
  bool has_xr = false; double dt_xr = 0.0;
  bool has_yu = false; double dt_yu = 0.0;
  bool has_yd = false; double dt_yd = 0.0;
};

// Streaming nearest-arm matcher for a delay-line detector: one trigger channel
// plus four arm channels (XL/XR/YU/YD). Same watermark scheme as
// CoincidenceMatcher above: hits arrive roughly in time order, `margin_ns` slack
// absorbs the cross-batch disorder of the merged stream (measured up to ~8 ms on
// side3 run0023 — the per-channel-pair aggregates flush independently), and a
// trigger ripens once watermark = (max timestamp seen) - margin_ns passes it.
// Each ripe trigger is matched against every buffered arm hit, picking the
// CLOSEST within ±window_ns per arm.
//
// Two deliberate differences from CoincidenceMatcher:
//   * find_closest drops the energy out-param — nothing consumes arm energies.
//   * ripen runs on EVERY pushed hit, monitored or not. The XY channels can all
//     be low-rate, so waiting for the next monitored hit to ripen would add
//     seconds of display latency; the empty-queue ripen test is O(1) anyway.
//
// Channel identity is `channel` alone (module ignored) — same caveat as above.
class PositionMatcher {
 public:
  struct Config {
    int trig_ch = -1;  // -1 => never matches (a u8 channel can't be negative)
    int xl_ch = -1;
    int xr_ch = -1;
    int yu_ch = -1;
    int yd_ch = -1;
    double window_ns = 1000.0;
    double margin_ns = 10000.0;
  };

  explicit PositionMatcher(Config cfg) : cfg_(cfg) {}

  // Feed one hit (call in ~arrival order). `on_result(const PosResult&)` is
  // invoked for every trigger that ripens as a consequence.
  template <class ResultF>
  void push(const ScalarHit& h, ResultF on_result) {
    double t = h.timestamp_ns;
    if (t > max_ts_) max_ts_ = t;  // before the channel test: every hit advances
    int ch = static_cast<int>(h.channel);
    if (ch == cfg_.trig_ch) {
      trig_.push_back({t, h.energy});
    } else if (ch == cfg_.xl_ch) {
      xl_.push_back({t, 0});
    } else if (ch == cfg_.xr_ch) {
      xr_.push_back({t, 0});
    } else if (ch == cfg_.yu_ch) {
      yu_.push_back({t, 0});
    } else if (ch == cfg_.yd_ch) {
      yd_.push_back({t, 0});
    }
    double watermark = max_ts_ - cfg_.margin_ns;
    ripen(watermark, on_result);

    // Safety prune (bounds memory if the trigger channel goes silent): an arm
    // hit at or below (watermark - window) can never match a not-yet-ripe
    // trigger, whose timestamp is > watermark.
    double safe = watermark - cfg_.window_ns;
    prune(xl_, safe);
    prune(xr_, safe);
    prune(yu_, safe);
    prune(yd_, safe);
  }

  // Ripen every buffered trigger regardless of watermark. Call once at EOS so
  // the final partial window is not lost.
  template <class ResultF>
  void flush(ResultF on_result) {
    ripen(std::numeric_limits<double>::infinity(), on_result);
  }

  // Drop all buffered hits and forget the high-water timestamp. Call at a run
  // boundary — the digitizer clock restarts between runs, and a stale (huge)
  // max_ts_ would make the next run's triggers look instantly "ripe" and emit
  // before their arm hits arrive.
  void reset() {
    trig_.clear();
    xl_.clear();
    xr_.clear();
    yu_.clear();
    yd_.clear();
    max_ts_ = -std::numeric_limits<double>::infinity();
  }

 private:
  template <class ResultF>
  void ripen(double watermark, ResultF& on_result) {
    while (!trig_.empty() && trig_.front().t <= watermark) {
      TimedHit g = trig_.front();
      trig_.pop_front();
      PosResult res;
      res.trig_t = g.t;
      res.trig_energy = g.energy;
      find_closest(xl_, g.t, res.has_xl, res.dt_xl);
      find_closest(xr_, g.t, res.has_xr, res.dt_xr);
      find_closest(yu_, g.t, res.has_yu, res.dt_yu);
      find_closest(yd_, g.t, res.has_yd, res.dt_yd);
      on_result(res);
      // Arm hits older than (trig_t - window) can never match this-or-later
      // triggers.
      double below = g.t - cfg_.window_ns;
      prune(xl_, below);
      prune(xr_, below);
      prune(yu_, below);
      prune(yd_, below);
    }
  }

  // Nearest arm hit to `tt` within ±window_ns; dt = arm - tt (signed).
  void find_closest(const std::deque<TimedHit>& arm, double tt, bool& has,
                    double& dt) const {
    has = false;
    dt = 0.0;
    double best = 0.0;
    for (const TimedHit& p : arm) {
      double d = p.t - tt;
      double ad = d < 0 ? -d : d;
      if (ad <= cfg_.window_ns && (!has || ad < best)) {
        has = true;
        best = ad;
        dt = d;
      }
    }
  }

  static void prune(std::deque<TimedHit>& dq, double below) {
    while (!dq.empty() && dq.front().t < below) dq.pop_front();
  }

  Config cfg_;
  double max_ts_ = -std::numeric_limits<double>::infinity();
  std::deque<TimedHit> trig_, xl_, xr_, yu_, yd_;
};

// Parse a "T,XL,XR,YU,YD"-style list into exactly `expect` DISTINCT ints in
// [0,255]. Backs the --xy1-ch/--xy2-ch flags. Returns false on wrong count,
// junk, negatives, out-of-range, or duplicates — a duplicated channel would
// otherwise be claimed silently by whichever deque tests first in push().
inline bool parse_ch_list(const std::string& s, std::size_t expect,
                          std::vector<int>& out) {
  out.clear();
  const char* p = s.c_str();
  while (*p) {
    char* end = nullptr;
    long v = std::strtol(p, &end, 10);
    if (end == p || v < 0 || v > 255) return false;
    out.push_back(static_cast<int>(v));
    p = end;
    if (*p == ',') {
      ++p;
      if (!*p) return false;  // trailing comma
      continue;
    }
    if (*p) return false;  // junk after a number
  }
  if (out.size() != expect) return false;
  for (std::size_t i = 0; i < out.size(); ++i)
    for (std::size_t j = i + 1; j < out.size(); ++j)
      if (out[i] == out[j]) return false;
  return true;
}

// ---------------------------------------------------------------------------
// 4. Run / file state machine
// ---------------------------------------------------------------------------

// Idle -> Writing on the first Data message; back to Idle once EVERY source seen
// in Data since run start has sent its EndOfStream. Single source => one EOS
// closes. This set-based rule deliberately avoids the "first-EOS latch" trap that
// bites naive multi-source consumers (see MEMORY: zmq_hit_source_first_eos_latch).
class RunState {
 public:
  bool is_writing() const { return writing_; }

  // Handle a Data message's source_id. `open()` is invoked exactly once, when a
  // new run starts (the transition Idle -> Writing). Returns true if it opened.
  template <class OpenF>
  bool on_data(uint32_t source_id, OpenF open) {
    bool opened = false;
    if (!writing_) {
      writing_ = true;
      seen_.clear();
      eos_.clear();
      open();
      opened = true;
    }
    seen_.insert(source_id);
    return opened;
  }

  // Handle an EndOfStream. While Writing: record the EOS; if all seen sources
  // have now reported, call `finalize(run_number)` and return to Idle (returns
  // true). While Idle: a stale EOS — call `stale(source_id, run_number)` and
  // ignore it (returns false).
  template <class FinalizeF, class StaleF>
  bool on_eos(uint32_t source_id, uint32_t run_number, FinalizeF finalize,
              StaleF stale) {
    if (!writing_) {
      stale(source_id, run_number);
      return false;
    }
    eos_.insert(source_id);
    if (all_seen_have_eos()) {
      finalize(run_number);
      writing_ = false;
      seen_.clear();
      eos_.clear();
      return true;
    }
    return false;
  }

 private:
  bool all_seen_have_eos() const {
    if (seen_.empty()) return false;
    for (uint32_t s : seen_)
      if (eos_.find(s) == eos_.end()) return false;
    return true;
  }

  bool writing_ = false;
  std::set<uint32_t> seen_;  // source_ids that sent Data this run
  std::set<uint32_t> eos_;   // source_ids that sent EOS this run
};

// ---------------------------------------------------------------------------
// 5. HTTP helpers (pure string/JSON — the socket code lives in root_sink.cxx)
// ---------------------------------------------------------------------------
//
// These back the `--operator URL` experiment-name lookup: root_sink.cxx does the
// raw-socket GET of `<URL>/api/status`, then hands the response text here so the
// parse is unit-testable without a network. Both are total (never throw).

// Split a raw HTTP response at the first "\r\n\r\n" (the header/body boundary).
// Returns true and fills header_out/body_out on success; on a missing boundary
// returns false with the whole input in header_out and an empty body.
inline bool split_http_response(const std::string& raw, std::string& header_out,
                                std::string& body_out) {
  static const std::string sep = "\r\n\r\n";
  std::size_t pos = raw.find(sep);
  if (pos == std::string::npos) {
    header_out = raw;
    body_out.clear();
    return false;
  }
  header_out = raw.substr(0, pos);
  body_out = raw.substr(pos + sep.size());
  return true;
}

// Extract the top-level "experiment_name" string from an operator /api/status
// JSON body (see src/operator/routes/status.rs SystemStatus). Returns "" on any
// parse failure or a missing/non-string key — the caller treats "" as "fall back
// to the default name" and warns, so this never fails silently upstream.
inline std::string extract_experiment_name(const std::string& json_body) {
  try {
    tdelila::json::Value root = tdelila::json::Parser(json_body).parse();
    const tdelila::json::Value* v = root.find("experiment_name");
    if (v && v->t == tdelila::json::Value::T::Str) return v->str;
  } catch (const std::exception&) {
    // fall through to "" — malformed body
  }
  return "";
}

}  // namespace rootsink

#endif  // ROOTSINK_SINK_CORE_HPP
