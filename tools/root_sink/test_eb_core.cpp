// test_eb_core.cpp — unit tests for eb_core.hpp (no ROOT, no ZMQ, no network).
//
// Covers the multithreaded event-builder core: Channel, SeqReorder, the
// SortedChunk splitter (including the TODO 58 H10 chunk-boundary rules ported
// from src/event_builder/chunk_builder.rs), and the data-time-driven Sorter.
//
// Build & run (Channel tests spawn threads, hence -pthread):
//   g++ -std=c++17 -O0 -g -pthread test_eb_core.cpp -o /tmp/teb && /tmp/teb
//
// Uses the same tiny CHECK harness as test_sink_core.cpp (deliberate copy —
// the two binaries stay independent so the 215 existing checks are never
// touched by eb work).

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <random>
#include <thread>
#include <vector>

#include "eb_core.hpp"

using namespace rootsink;
using namespace rootsink::eb;

static int g_pass = 0;
static int g_fail = 0;

#define CHECK(cond)                                                     \
  do {                                                                  \
    if (cond) {                                                         \
      ++g_pass;                                                         \
    } else {                                                            \
      ++g_fail;                                                         \
      std::printf("FAIL %s:%d  CHECK(%s)\n", __FILE__, __LINE__, #cond); \
    }                                                                   \
  } while (0)

static bool near(double a, double b, double eps = 1e-9) {
  double d = a - b;
  if (d < 0) d = -d;
  return d <= eps;
}

// Convenience: a hit with just channel + timestamp (energies default 0).
static ScalarHit hit(int ch, double t, uint16_t e = 0) {
  ScalarHit h;
  h.channel = static_cast<uint8_t>(ch);
  h.timestamp_ns = t;
  h.energy = e;
  return h;
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

static void test_channel_fifo() {
  Channel<int> ch;
  int a = 1, b = 2, c = 3;
  ch.push(std::move(a));
  ch.push(std::move(b));
  ch.push(std::move(c));
  CHECK(ch.size() == 3);
  int out = 0;
  CHECK(ch.pop(out) && out == 1);
  CHECK(ch.pop(out) && out == 2);
  CHECK(ch.pop(out) && out == 3);
  CHECK(ch.size() == 0);
}

static void test_channel_blocking_pop() {
  Channel<int> ch;
  int got = 0;
  std::thread consumer([&] {
    int v = 0;
    if (ch.pop(v)) got = v;  // blocks until the push below
  });
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  int v = 42;
  ch.push(std::move(v));
  consumer.join();
  CHECK(got == 42);
}

static void test_channel_close_drains_then_false() {
  Channel<int> ch;
  int a = 7, b = 8;
  ch.push(std::move(a));
  ch.push(std::move(b));
  ch.close();
  int out = 0;
  CHECK(ch.pop(out) && out == 7);  // pending items still poppable after close
  CHECK(ch.pop(out) && out == 8);
  CHECK(!ch.pop(out));  // closed AND drained -> false

  // A pop blocked on an empty channel must wake on close and return false.
  Channel<int> ch2;
  std::atomic<bool> returned_false{false};
  std::thread blocked([&] {
    int v = 0;
    if (!ch2.pop(v)) returned_false = true;
  });
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  ch2.close();
  blocked.join();
  CHECK(returned_false);
}

static void test_channel_bounded_try_push() {
  Channel<int> ch(2);  // bounded(2) — the display-tee shape
  int a = 1, b = 2, c = 3;
  CHECK(ch.try_push(std::move(a)));
  CHECK(ch.try_push(std::move(b)));
  CHECK(!ch.try_push(std::move(c)));  // full -> false...
  CHECK(c == 3);                      // ...and the value was NOT consumed
  int out = 0;
  CHECK(ch.pop(out) && out == 1);
  CHECK(ch.try_push(std::move(c)));  // space again -> accepted
  CHECK(ch.size() == 2);
}

static void test_channel_pop_for_timeout() {
  Channel<int> ch;
  int out = 0;
  auto t0 = std::chrono::steady_clock::now();
  CHECK(ch.pop_for(out, 30) == PopResult::Timeout);  // open + empty -> Timeout
  auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::steady_clock::now() - t0)
                .count();
  CHECK(ms >= 25);  // actually waited (allow scheduler slop)
  int v = 5;
  ch.push(std::move(v));
  CHECK(ch.pop_for(out, 30) == PopResult::Value && out == 5);
}

// Issue #26: pop_for must distinguish Timeout from Closed, and must only
// report Closed once the queue is drained — otherwise a consumer that exits
// on an external flag can race a producer's final push(msg); close() and
// lose the last message.
static void test_channel_pop_for_three_state() {
  // Drain-then-Closed: pending items outlive close(), Closed comes last.
  Channel<int> ch;
  int a = 7, b = 8;
  ch.push(std::move(a));
  ch.push(std::move(b));
  ch.close();
  int out = 0;
  CHECK(ch.pop_for(out, 30) == PopResult::Value && out == 7);
  CHECK(ch.pop_for(out, 30) == PopResult::Value && out == 8);
  CHECK(ch.pop_for(out, 30) == PopResult::Closed);
  CHECK(ch.pop_for(out, 0) == PopResult::Closed);  // stays Closed, 0-timeout too

  // A pop_for blocked on an empty channel must wake on close() -> Closed
  // (not sit out its full timeout).
  Channel<int> ch2;
  std::atomic<bool> got_closed{false};
  std::thread blocked([&] {
    int v = 0;
    if (ch2.pop_for(v, 10000) == PopResult::Closed) got_closed = true;
  });
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  ch2.close();
  blocked.join();
  CHECK(got_closed);
}

// The issue #26 race shape, end to end: a consumer that exits ONLY on Closed
// receives every message even when the producer's last push and close() land
// mid-poll. With the old bool contract + external stop flag this
// intermittently lost the final message.
static void test_channel_pop_for_close_race() {
  const int kRounds = 50;  // the original race was intermittent — hammer it
  for (int round = 0; round < kRounds; ++round) {
    Channel<int> ch;
    const int kMsgs = 3;
    std::thread producer([&] {
      for (int i = 0; i < kMsgs; ++i) {
        int v = i;
        ch.push(std::move(v));
      }
      ch.close();  // close() IS the termination signal here — no pill
    });
    int received = 0;
    for (;;) {
      int v = 0;
      PopResult r = ch.pop_for(v, 1);
      if (r == PopResult::Closed) break;
      if (r == PopResult::Value) ++received;
    }
    producer.join();
    if (received != kMsgs) {
      CHECK(received == kMsgs);  // report the losing round
      return;
    }
  }
  CHECK(true);  // all rounds delivered every message
}

static void test_channel_move_only() {
  // The zero-copy contract: payloads move through, never copy.
  Channel<std::unique_ptr<int>> ch;
  ch.push(std::make_unique<int>(99));
  std::unique_ptr<int> out;
  CHECK(ch.pop(out) && out && *out == 99);
}

static void test_channel_multi_producer() {
  Channel<int> ch;
  const int kProducers = 4, kEach = 250;
  std::vector<std::thread> producers;
  for (int p = 0; p < kProducers; ++p) {
    producers.emplace_back([&ch, p] {
      for (int i = 0; i < kEach; ++i) {
        int v = p * kEach + i;
        ch.push(std::move(v));
      }
    });
  }
  long long sum = 0;
  int count = 0;
  std::thread consumer([&] {
    int v = 0;
    while (count < kProducers * kEach && ch.pop(v)) {
      sum += v;
      ++count;
    }
  });
  for (auto& t : producers) t.join();
  consumer.join();
  CHECK(count == kProducers * kEach);  // conservation: nothing lost
  CHECK(sum == 1000LL * 999 / 2);      // sum of 0..999
}

// ---------------------------------------------------------------------------
// SeqReorder
// ---------------------------------------------------------------------------

static void test_seq_reorder_in_order() {
  SeqReorder<int> r;
  int out = 0;
  r.push(0, 10);
  CHECK(r.pop_ready(out) && out == 10);
  r.push(1, 11);
  CHECK(r.pop_ready(out) && out == 11);
  CHECK(!r.pop_ready(out));
}

static void test_seq_reorder_out_of_order() {
  SeqReorder<int> r;
  int out = 0;
  r.push(2, 12);
  r.push(0, 10);
  r.push(1, 11);
  CHECK(r.pop_ready(out) && out == 10);
  CHECK(r.pop_ready(out) && out == 11);
  CHECK(r.pop_ready(out) && out == 12);
  CHECK(!r.pop_ready(out));
}

static void test_seq_reorder_gap_holds() {
  SeqReorder<int> r;
  int out = 0;
  r.push(1, 11);  // seq 0 missing
  CHECK(!r.pop_ready(out));
  CHECK(r.pending() == 1);
  r.push(0, 10);
  CHECK(r.pop_ready(out) && out == 10);
  CHECK(r.pop_ready(out) && out == 11);
  CHECK(r.pending() == 0);
}

// ---------------------------------------------------------------------------
// sort_and_split / sort_and_flush / core_range
// ---------------------------------------------------------------------------

static const double kInf = std::numeric_limits<double>::infinity();

static void test_split_empty() {
  std::vector<ScalarHit> buf;
  SortedChunk c;
  CHECK(!sort_and_split(buf, 50000.0, 500.0, -kInf, c));
}

static void test_split_insufficient_span() {
  // core_end = latest - horizon must clear the earliest hit, else no split.
  std::vector<ScalarHit> buf = {hit(0, 3000.0), hit(0, 1000.0), hit(0, 2000.0)};
  SortedChunk c;
  CHECK(!sort_and_split(buf, 50000.0, 500.0, -kInf, c));
  CHECK(buf.size() == 3);  // buffer kept...
  CHECK(near(buf[0].timestamp_ns, 1000.0) && near(buf[2].timestamp_ns, 3000.0));
  // ...and sorted (documented side effect)
}

static void test_split_retained_from_lookback() {
  // latest=51100, horizon=50000 -> core_end=1100; lookback=500 -> retained
  // begins at 600 (the H10 fix: context BEFORE core_end is carried too).
  std::vector<ScalarHit> buf = {hit(0, 100.0), hit(1, 700.0), hit(2, 1000.0),
                                hit(3, 51100.0)};
  SortedChunk c;
  CHECK(sort_and_split(buf, 50000.0, 500.0, -kInf, c));
  CHECK(near(c.core_end, 1100.0));
  CHECK(c.core_start == -kInf);
  CHECK(c.hits.size() == 4);  // chunk carries ALL hits incl. the overlap
  CHECK(buf.size() == 3);     // retained: 700, 1000 (>= 600) and 51100
  CHECK(near(buf[0].timestamp_ns, 700.0));
  CHECK(near(buf[1].timestamp_ns, 1000.0));
  CHECK(near(buf[2].timestamp_ns, 51100.0));
}

static void test_split_chain_invariant() {
  // chunk[N+1].core_start == chunk[N].core_end, maintained by the caller
  // passing prev core_end in — exactly what Sorter does.
  std::vector<ScalarHit> buf = {hit(0, 0.0), hit(0, 60000.0)};
  SortedChunk c1;
  CHECK(sort_and_split(buf, 50000.0, 500.0, -kInf, c1));
  CHECK(near(c1.core_end, 10000.0));
  buf.push_back(hit(0, 130000.0));
  SortedChunk c2;
  CHECK(sort_and_split(buf, 50000.0, 500.0, c1.core_end, c2));
  CHECK(near(c2.core_start, c1.core_end));
  CHECK(near(c2.core_end, 80000.0));
}

static void test_flush() {
  std::vector<ScalarHit> buf = {hit(0, 2000.0), hit(0, 1000.0)};
  SortedChunk c;
  CHECK(sort_and_flush(buf, 1100.0, c));
  CHECK(buf.empty());
  CHECK(c.hits.size() == 2 && near(c.hits[0].timestamp_ns, 1000.0));
  CHECK(near(c.core_start, 1100.0));  // floor honored: 1000 was emitted before
  CHECK(c.core_end == kInf);
  std::vector<ScalarHit> empty;
  SortedChunk c2;
  CHECK(!sort_and_flush(empty, -kInf, c2));
}

static void test_split_shuffled_sorts() {
  std::vector<ScalarHit> buf;
  for (int i = 0; i < 200; ++i) buf.push_back(hit(0, (i * 37) % 200 * 10.0));
  buf.push_back(hit(0, 100000.0));
  SortedChunk c;
  CHECK(sort_and_split(buf, 50000.0, 500.0, -kInf, c));
  bool sorted = true;
  for (std::size_t i = 1; i < c.hits.size(); ++i)
    if (c.hits[i - 1].timestamp_ns > c.hits[i].timestamp_ns) sorted = false;
  CHECK(sorted);
}

static void test_core_range_conservation() {
  // Over consecutive chunks + final flush, the core ranges partition the full
  // hit set: every hit written exactly once (guards the hits tree).
  std::mt19937 rng(12345);
  std::uniform_real_distribution<double> jitter(0.0, 5000.0);
  std::vector<double> all_ts;
  for (int i = 0; i < 500; ++i) all_ts.push_back(i * 1000.0 + jitter(rng));

  std::vector<ScalarHit> buf;
  double prev_core_end = -kInf;
  std::size_t written = 0;
  std::size_t feed = 0;
  double max_written_ts = -kInf;
  bool monotone = true;
  auto write_core = [&](const SortedChunk& c) {
    auto range = core_range(c);
    for (std::size_t i = range.first; i < range.second; ++i) {
      if (c.hits[i].timestamp_ns < max_written_ts) monotone = false;
      max_written_ts = c.hits[i].timestamp_ns;
      ++written;
    }
  };
  // Feed in batches of 50, splitting whenever possible (tiny horizon/span so
  // several chunks happen).
  while (feed < all_ts.size()) {
    for (int i = 0; i < 50 && feed < all_ts.size(); ++i, ++feed)
      buf.push_back(hit(0, all_ts[feed]));
    SortedChunk c;
    while (sort_and_split(buf, 20000.0, 1000.0, prev_core_end, c)) {
      prev_core_end = c.core_end;
      write_core(c);
      c = SortedChunk{};
    }
  }
  SortedChunk fin;
  if (sort_and_flush(buf, prev_core_end, fin)) write_core(fin);
  CHECK(written == all_ts.size());  // no loss, no duplication
  CHECK(monotone);                  // and globally time-sorted across chunks
}

static void test_core_range_bounds() {
  SortedChunk c;
  c.hits = {hit(0, 100.0), hit(0, 600.0), hit(0, 1000.0), hit(0, 1100.0),
            hit(0, 51100.0)};
  c.core_start = 700.0;
  c.core_end = 1100.0;
  auto r = core_range(c);
  // [700, 1100): excludes the lookback head (100, 600) and the boundary hit
  // at exactly core_end (owned by the NEXT chunk) and the tail.
  CHECK(r.first == 2 && r.second == 3);
  CHECK(near(c.hits[r.first].timestamp_ns, 1000.0));
}

// ---------------------------------------------------------------------------
// Sorter
// ---------------------------------------------------------------------------

static Sorter::Config small_cfg() {
  Sorter::Config cfg;
  cfg.chunk_span_ns = 10000.0;   // 10 us span
  cfg.safe_horizon_ns = 5000.0;  // 5 us horizon
  cfg.lookback_ns = 1000.0;
  return cfg;
}

static void test_sorter_data_time_only() {
  // Many hits inside a tiny time span: count alone must NOT trigger a chunk.
  Sorter s(small_cfg());
  int chunks = 0;
  std::vector<ScalarHit> batch;
  for (int i = 0; i < 10000; ++i) batch.push_back(hit(0, 100.0 + i * 0.01));
  s.push_batch(std::move(batch), [&](SortedChunk&&) { ++chunks; });
  CHECK(chunks == 0);
  CHECK(s.buffered() == 10000);
}

static void test_sorter_first_chunk_floor() {
  // First chunk: floor = earliest hit. Emit once (max - horizon) - earliest
  // >= span, i.e. max >= earliest + 15000 with the small config.
  Sorter s(small_cfg());
  int chunks = 0;
  SortedChunk last;
  std::vector<ScalarHit> b1 = {hit(0, 1000.0), hit(0, 15999.0)};
  s.push_batch(std::move(b1), [&](SortedChunk&&) { ++chunks; });
  CHECK(chunks == 0);  // candidate 10999 - 1000 = 9999 < 10000
  std::vector<ScalarHit> b2 = {hit(0, 16001.0)};
  s.push_batch(std::move(b2), [&](SortedChunk&& c) {
    ++chunks;
    last = std::move(c);
  });
  CHECK(chunks == 1);  // candidate 11001 - 1000 = 10001 >= 10000
  CHECK(near(last.core_end, 11001.0));
  CHECK(last.core_start == -kInf);
}

static void test_sorter_chain() {
  Sorter s(small_cfg());
  std::vector<SortedChunk> chunks;
  auto grab = [&](SortedChunk&& c) { chunks.push_back(std::move(c)); };
  std::vector<ScalarHit> b1 = {hit(0, 0.0), hit(0, 20000.0)};
  s.push_batch(std::move(b1), grab);
  CHECK(chunks.size() == 1);
  std::vector<ScalarHit> b2 = {hit(0, 40000.0)};
  s.push_batch(std::move(b2), grab);
  CHECK(chunks.size() == 2);
  CHECK(near(chunks[1].core_start, chunks[0].core_end));  // chain invariant
  // The hit at 20000 sat in chunk 0's tail (past core_end=15000, not emitted);
  // it must reappear in chunk 1 AND fall inside chunk 1's core range.
  CHECK(near(chunks[1].hits.front().timestamp_ns, 20000.0));
  auto r = core_range(chunks[1]);
  CHECK(r.second > r.first && near(chunks[1].hits[r.first].timestamp_ns, 20000.0));
}

static void test_sorter_flush_remainder() {
  Sorter s(small_cfg());
  std::vector<SortedChunk> chunks;
  auto grab = [&](SortedChunk&& c) { chunks.push_back(std::move(c)); };
  std::vector<ScalarHit> b = {hit(0, 0.0), hit(0, 20000.0)};
  s.push_batch(std::move(b), grab);
  CHECK(chunks.size() == 1);
  s.flush(grab);
  CHECK(chunks.size() == 2);
  CHECK(chunks[1].core_end == kInf);
  CHECK(near(chunks[1].core_start, chunks[0].core_end));
  CHECK(s.buffered() == 0);
  s.flush(grab);  // nothing left -> no chunk
  CHECK(chunks.size() == 2);
}

static void test_sorter_reset_between_runs() {
  // R11: after reset(), a new run with SMALL timestamps must emit normally.
  // A stale prev_core_end floor from run 1 would silently swallow run 2.
  Sorter s(small_cfg());
  int chunks = 0;
  auto count = [&](SortedChunk&&) { ++chunks; };
  std::vector<ScalarHit> run1 = {hit(0, 1e9), hit(0, 1e9 + 20000.0)};
  s.push_batch(std::move(run1), count);
  CHECK(chunks == 1);
  s.flush(count);
  CHECK(chunks == 2);

  s.reset();  // RunOpen of the next run — digitizer clock restarted
  SortedChunk c;
  std::vector<ScalarHit> run2 = {hit(0, 100.0), hit(0, 20100.0)};
  s.push_batch(std::move(run2), [&](SortedChunk&& cc) {
    ++chunks;
    c = std::move(cc);
  });
  CHECK(chunks == 3);            // run 2 emits despite tiny timestamps
  CHECK(c.core_start == -kInf);  // floor was reset
  auto r = core_range(c);
  CHECK(r.second > r.first);  // and its core actually contains hits
}

static void test_sorter_late_hit_lands_sorted() {
  // A hit arriving late (but within the safe horizon) must appear in the
  // correct sorted position of the next chunk, not be lost or misplaced.
  Sorter s(small_cfg());
  std::vector<SortedChunk> chunks;
  auto grab = [&](SortedChunk&& c) { chunks.push_back(std::move(c)); };
  std::vector<ScalarHit> b1 = {hit(0, 0.0), hit(0, 12000.0), hit(0, 20000.0)};
  s.push_batch(std::move(b1), grab);
  CHECK(chunks.size() == 1);
  CHECK(near(chunks[0].core_end, 15000.0));
  // 13000 is OLDER than max seen (20000) but younger than core_end - lookback?
  // No: 13000 < 15000 (already-emitted core_end) but >= 14000 lookback? It is
  // 13000 < 14000 — deliberately choose 14500: inside the retained lookback.
  std::vector<ScalarHit> b2 = {hit(0, 14500.0), hit(0, 40000.0)};
  s.push_batch(std::move(b2), grab);
  CHECK(chunks.size() == 2);
  // 14500 must sit sorted between the retained 12000-tail context and 20000.
  bool found = false, sorted = true;
  const auto& h2 = chunks[1].hits;
  for (std::size_t i = 0; i < h2.size(); ++i) {
    if (near(h2[i].timestamp_ns, 14500.0)) found = true;
    if (i > 0 && h2[i - 1].timestamp_ns > h2[i].timestamp_ns) sorted = false;
  }
  CHECK(found && sorted);
  // And it belongs to chunk 2's core: core_start = 15000 ... no — 14500 is
  // BELOW core_start (15000): it was late enough to miss chunk 1's core, and
  // the H10 floor excludes it from chunk 2's core too. That is exactly the
  // safe-horizon contract: 14500 arrived AFTER max_ts had reached 20000, i.e.
  // disorder of 5500 ns > safe_horizon 5000 — out of contract, context-only.
  auto r = core_range(chunks[1]);
  CHECK(h2[r.first].timestamp_ns >= 15000.0);
}

// ---------------------------------------------------------------------------
// build_events_from_chunk (integrated-event builder)
// ---------------------------------------------------------------------------

// Reorder `hits` into a realistic arrival order: true time plus a bounded
// per-hit displacement. The Sorter's contract only covers disorder up to
// safe_horizon_ns — a full shuffle would exceed it by construction (hits
// arriving later than their chunk's core are context-only, by design), so
// integration tests must model the real, BOUNDED network disorder.
static void jitter_arrival(std::vector<ScalarHit>& hits, double max_disp,
                           std::mt19937& rng) {
  std::uniform_real_distribution<double> disp(-max_disp, max_disp);
  std::vector<std::pair<double, std::size_t>> order;
  order.reserve(hits.size());
  for (std::size_t i = 0; i < hits.size(); ++i)
    order.push_back({hits[i].timestamp_ns + disp(rng), i});
  std::sort(order.begin(), order.end());
  std::vector<ScalarHit> arrival;
  arrival.reserve(hits.size());
  for (const auto& p : order) arrival.push_back(hits[p.second]);
  hits = std::move(arrival);
}

// The side3 channel map: det1 = trig ch1 + arms ch2-5, det2 = trig ch6 +
// arms ch7-10, LaBr3 = ch0.
static BuilderConfig side3_cfg(double window = 500.0) {
  BuilderConfig c;
  c.trig1 = 1; c.xl1 = 2; c.xr1 = 3; c.yu1 = 4; c.yd1 = 5;
  c.trig2 = 6; c.xl2 = 7; c.xr2 = 8; c.yu2 = 9; c.yd2 = 10;
  c.labr_ch = 0;
  c.window_ns = window;
  return c;
}

static SortedChunk mkchunk(std::vector<ScalarHit> hits, double cs = -kInf,
                           double ce = kInf) {
  std::sort(hits.begin(), hits.end(), [](const ScalarHit& a, const ScalarHit& b) {
    return a.timestamp_ns < b.timestamp_ns;
  });
  SortedChunk c;
  c.hits = std::move(hits);
  c.core_start = cs;
  c.core_end = ce;
  return c;
}

static void test_builder_full_event() {
  auto ev = build_events_from_chunk(
      mkchunk({hit(0, 990.0, 50), hit(1, 1000.0, 100), hit(2, 1010.0, 10),
               hit(3, 1020.0, 11), hit(4, 1030.0, 12), hit(5, 1040.0, 13)}),
      side3_cfg());
  CHECK(ev.size() == 1);
  const BuiltEvent& e = ev[0];
  CHECK(near(e.trig1_t, 1000.0) && e.e_trig1 == 100);
  CHECK(near(e.dt_xl1, 10.0) && near(e.dt_xr1, 20.0));
  CHECK(near(e.dt_yu1, 30.0) && near(e.dt_yd1, 40.0));
  CHECK(near(e.x1, 10.0));   // dt_xr - dt_xl = 20 - 10
  CHECK(near(e.y1, -10.0));  // dt_yu - dt_yd = 30 - 40
  CHECK(e.e_xl1 == 10 && e.e_xr1 == 11 && e.e_yu1 == 12 && e.e_yd1 == 13);
  CHECK(e.e_sum1 == 46 && e.n_arms1 == 4);
  CHECK(near(e.labr_t, 990.0) && e.e_labr == 50);
  CHECK(near(e.tof1, 10.0));  // trig - labr
  // Detector 2 never fired: everything NaN/0.
  CHECK(std::isnan(e.trig2_t) && std::isnan(e.x2) && std::isnan(e.dt_trig));
  CHECK(std::isnan(e.tof2) && e.n_arms2 == 0 && e.e_sum2 == 0);
}

static void test_builder_partial_arm() {
  // XR missing: x1 NaN but y1 still valid — completeness is per-axis.
  auto ev = build_events_from_chunk(
      mkchunk({hit(1, 1000.0, 100), hit(2, 1010.0, 10), hit(4, 1030.0, 12),
               hit(5, 1040.0, 13)}),
      side3_cfg());
  CHECK(ev.size() == 1);
  const BuiltEvent& e = ev[0];
  CHECK(std::isnan(e.dt_xr1) && std::isnan(e.x1));
  CHECK(near(e.y1, -10.0));
  CHECK(e.n_arms1 == 3);
  CHECK(e.e_sum1 == 35);  // 10 + 12 + 13, XR's energy excluded
}

static void test_builder_trigger_only() {
  // A lone trigger is EMITTED (arm-efficiency measurement), not dropped.
  auto ev = build_events_from_chunk(mkchunk({hit(1, 1000.0, 100)}), side3_cfg());
  CHECK(ev.size() == 1);
  CHECK(near(ev[0].trig1_t, 1000.0) && ev[0].n_arms1 == 0 && ev[0].e_sum1 == 0);
  CHECK(std::isnan(ev[0].x1) && std::isnan(ev[0].y1) && std::isnan(ev[0].labr_t));
}

static void test_builder_nearest_not_first() {
  // Two XL candidates: 700 (backward, |300|) and 1100 (forward, |100|).
  // Nearest matching picks 1100; LiveEventBuilder's backward-first rule would
  // have picked 700 — this test pins the divergence.
  auto ev = build_events_from_chunk(
      mkchunk({hit(2, 700.0, 7), hit(1, 1000.0, 100), hit(2, 1100.0, 8)}),
      side3_cfg());
  CHECK(ev.size() == 1);
  CHECK(near(ev[0].dt_xl1, 100.0));
  CHECK(ev[0].e_xl1 == 8);
}

static void test_builder_window_inclusive() {
  // |dt| == window is accepted (closed interval, matching tmp/test.cpp's <=).
  auto ev = build_events_from_chunk(
      mkchunk({hit(1, 1000.0, 100), hit(2, 1500.0, 9)}), side3_cfg(500.0));
  CHECK(ev.size() == 1 && near(ev[0].dt_xl1, 500.0));
  auto ev2 = build_events_from_chunk(
      mkchunk({hit(1, 1000.0, 100), hit(2, 1501.0, 9)}), side3_cfg(500.0));
  CHECK(ev2.size() == 1 && std::isnan(ev2[0].dt_xl1));
}

static void test_builder_core_gates() {
  // Triggers outside [core_start, core_end) are context only; arm hits from
  // the context region still match an in-core trigger.
  auto ev = build_events_from_chunk(
      mkchunk({hit(1, 800.0, 1), hit(1, 1000.0, 2), hit(2, 1150.0, 9),
               hit(1, 1100.0, 3)},
              900.0, 1100.0),
      side3_cfg());
  CHECK(ev.size() == 1);            // 800 (< core_start) and 1100 (>= core_end) gated
  CHECK(near(ev[0].trig1_t, 1000.0));
  CHECK(near(ev[0].dt_xl1, 150.0));  // arm at 1150 is past core_end but usable
}

static void test_builder_boundary_backward_partner() {
  // Port of the Rust H10 content test: an arm hit BEFORE core_end must remain
  // available (via lookback) to a trigger emitted by the NEXT chunk, with the
  // correct negative dt.
  std::vector<ScalarHit> buf = {hit(2, 1000.0, 5), hit(1, 1300.0, 100),
                                hit(0, 51100.0, 0)};
  SortedChunk c1;
  CHECK(sort_and_split(buf, 50000.0, 500.0, -kInf, c1));
  CHECK(near(c1.core_end, 1100.0));
  auto ev1 = build_events_from_chunk(c1, side3_cfg());
  CHECK(ev1.empty());  // trigger 1300 >= core_end: deferred
  SortedChunk c2;
  CHECK(sort_and_flush(buf, c1.core_end, c2));
  auto ev2 = build_events_from_chunk(c2, side3_cfg());
  CHECK(ev2.size() == 1);
  CHECK(near(ev2[0].trig1_t, 1300.0));
  CHECK(near(ev2[0].dt_xl1, -300.0));  // the carried-back partner
}

static void test_builder_no_double_emission() {
  // Port of the Rust no-double-emit test: a trigger emitted by chunk N rides
  // into chunk N+1 as lookback context and must NOT fire again.
  std::vector<ScalarHit> buf = {hit(1, 1000.0, 100), hit(2, 1200.0, 5),
                                hit(0, 51050.0, 0)};
  SortedChunk c1;
  CHECK(sort_and_split(buf, 50000.0, 500.0, -kInf, c1));
  CHECK(near(c1.core_end, 1050.0));
  auto ev1 = build_events_from_chunk(c1, side3_cfg());
  CHECK(ev1.size() == 1);              // emitted here (1000 < 1050)...
  CHECK(near(ev1[0].dt_xl1, 200.0));   // ...with its forward partner from context
  SortedChunk c2;
  CHECK(sort_and_flush(buf, c1.core_end, c2));
  bool trig_in_c2 = false;
  for (const auto& h : c2.hits)
    if (h.channel == 1) trig_in_c2 = true;
  CHECK(trig_in_c2);  // the trigger IS present in chunk 2 (lookback)...
  auto ev2 = build_events_from_chunk(c2, side3_cfg());
  CHECK(ev2.empty());  // ...but the core_start floor silences it
}

static void test_builder_labr_disabled() {
  BuilderConfig cfg = side3_cfg();
  cfg.labr_ch = -1;
  auto ev = build_events_from_chunk(
      mkchunk({hit(0, 990.0, 50), hit(1, 1000.0, 100)}), cfg);
  CHECK(ev.size() == 1);
  CHECK(std::isnan(ev[0].labr_t) && std::isnan(ev[0].tof1));
}

static void test_builder_same_det_no_merge() {
  auto ev = build_events_from_chunk(
      mkchunk({hit(1, 1000.0, 1), hit(1, 1100.0, 2)}), side3_cfg());
  CHECK(ev.size() == 2);  // same-detector triggers never merge
  CHECK(near(ev[0].trig1_t, 1000.0) && near(ev[1].trig1_t, 1100.0));
}

static void test_builder_pairing() {
  // det1 then det2 within the window -> ONE integrated event.
  auto ev = build_events_from_chunk(
      mkchunk({hit(1, 1000.0, 100), hit(6, 1200.0, 200), hit(7, 1210.0, 20)}),
      side3_cfg());
  CHECK(ev.size() == 1);
  const BuiltEvent& e = ev[0];
  CHECK(near(e.trig1_t, 1000.0) && near(e.trig2_t, 1200.0));
  CHECK(near(e.dt_trig, 200.0));
  CHECK(near(e.dt_xl2, 10.0));  // det2 arms match det2's OWN trigger time
  CHECK(e.e_trig2 == 200);

  // det2 first: dt_trig is STILL t(det2) - t(det1), i.e. negative here.
  auto ev2 = build_events_from_chunk(
      mkchunk({hit(6, 1000.0, 200), hit(1, 1200.0, 100)}), side3_cfg());
  CHECK(ev2.size() == 1);
  CHECK(near(ev2[0].dt_trig, -200.0));
  CHECK(near(ev2[0].trig1_t, 1200.0) && near(ev2[0].trig2_t, 1000.0));
}

static void test_builder_chain() {
  // trig1@1000 pairs trig2@1400; trig1@1800 is on its own: 3 triggers -> 2
  // events (a pair is closed once formed — no chained mega-merge).
  auto ev = build_events_from_chunk(
      mkchunk({hit(1, 1000.0, 1), hit(6, 1400.0, 2), hit(1, 1800.0, 3)}),
      side3_cfg());
  CHECK(ev.size() == 2);
  CHECK(!std::isnan(ev[0].dt_trig));
  CHECK(std::isnan(ev[1].dt_trig) && near(ev[1].trig1_t, 1800.0));
}

static void test_builder_boundary_pair() {
  // A pair spanning core_end is emitted exactly once — by the chunk that owns
  // the ANCHOR. In the next chunk the anchor is context (< core_start), pairs
  // again, and gates out, consuming the partner with it.
  auto c1 = mkchunk({hit(1, 1090.0, 1), hit(6, 1150.0, 2)}, -kInf, 1100.0);
  auto ev1 = build_events_from_chunk(c1, side3_cfg());
  CHECK(ev1.size() == 1);
  CHECK(!std::isnan(ev1[0].dt_trig));
  auto c2 = mkchunk({hit(1, 1090.0, 1), hit(6, 1150.0, 2)}, 1100.0, kInf);
  auto ev2 = build_events_from_chunk(c2, side3_cfg());
  CHECK(ev2.empty());  // no double emission, no orphaned partner event
}

static void test_builder_through_sorter() {
  // 20 trigger+arm pairs, fed shuffled through the Sorter in small batches:
  // chunk-by-chunk building must recover exactly 20 events, each with its arm.
  BuilderConfig bcfg = side3_cfg(500.0);
  Sorter::Config scfg;
  scfg.chunk_span_ns = 10000.0;
  scfg.safe_horizon_ns = 5000.0;
  scfg.lookback_ns = bcfg.window_ns;
  Sorter s(scfg);

  std::vector<ScalarHit> all;
  for (int i = 0; i < 20; ++i) {
    double t = 1000.0 + i * 3000.0;  // spacing >> window: no pairing effects
    all.push_back(hit(1, t, static_cast<uint16_t>(i)));
    all.push_back(hit(2, t + 50.0, 1));
  }
  std::mt19937 rng(777);
  jitter_arrival(all, 2000.0, rng);  // bounded disorder < safe_horizon (5000)

  std::vector<BuiltEvent> events;
  auto build = [&](SortedChunk&& c) {
    auto ev = build_events_from_chunk(c, bcfg);
    events.insert(events.end(), ev.begin(), ev.end());
  };
  std::size_t fed = 0;
  while (fed < all.size()) {
    std::vector<ScalarHit> batch;
    for (int i = 0; i < 7 && fed < all.size(); ++i, ++fed)
      batch.push_back(all[fed]);
    s.push_batch(std::move(batch), build);
  }
  s.flush(build);
  CHECK(events.size() == 20);
  bool all_armed = true, ordered = true;
  for (std::size_t i = 0; i < events.size(); ++i) {
    if (std::isnan(events[i].dt_xl1) || !near(events[i].dt_xl1, 50.0))
      all_armed = false;
    if (i > 0 && events[i].trig1_t <= events[i - 1].trig1_t) ordered = false;
  }
  CHECK(all_armed);
  CHECK(ordered);  // chunked building preserves global event time order
}

// ---------------------------------------------------------------------------
// Worker-pool determinism (Channel + SeqReorder + builder end-to-end)
// ---------------------------------------------------------------------------

static std::vector<double> run_pool(int nworkers,
                                    const std::vector<SortedChunk>& chunks,
                                    const BuilderConfig& cfg) {
  Channel<WorkMsg> work, done;
  std::vector<std::thread> ws;
  for (int i = 0; i < nworkers; ++i) {
    ws.emplace_back([&] {
      WorkMsg m;
      while (work.pop(m)) {
        if (m.ctrl == Ctrl::Shutdown) {
          done.push(std::move(m));  // forward the pill, then exit
          break;
        }
        m.built = build_events_from_chunk(m.chunk, cfg);
        done.push(std::move(m));
      }
    });
  }
  uint64_t seq = 0;
  for (const auto& c : chunks) {
    WorkMsg m;
    m.chunk = c;
    m.seq = seq++;
    work.push(std::move(m));
  }
  for (int i = 0; i < nworkers; ++i) {
    WorkMsg m;
    m.ctrl = Ctrl::Shutdown;
    m.seq = seq++;
    work.push(std::move(m));
  }
  // "Writer": reassemble in seq order; done after all pills drained.
  SeqReorder<WorkMsg> reorder;
  std::vector<double> trig_times;
  int pills = 0;
  WorkMsg m;
  while (pills < nworkers && done.pop(m)) {
    reorder.push(m.seq, std::move(m));
    WorkMsg r;
    while (reorder.pop_ready(r)) {
      if (r.ctrl == Ctrl::Shutdown) {
        ++pills;
      } else {
        for (const auto& ev : r.built) trig_times.push_back(ev.trig1_t);
      }
    }
  }
  for (auto& t : ws) t.join();
  return trig_times;
}

static void test_pool_determinism() {
  // Same chunks through 1 worker and 4 workers -> IDENTICAL event sequence.
  BuilderConfig bcfg = side3_cfg(500.0);
  Sorter::Config scfg;
  scfg.chunk_span_ns = 10000.0;
  scfg.safe_horizon_ns = 5000.0;
  scfg.lookback_ns = bcfg.window_ns;
  Sorter s(scfg);

  std::mt19937 rng(4242);
  std::uniform_real_distribution<double> jit(0.0, 200.0);
  std::vector<ScalarHit> all;
  for (int i = 0; i < 300; ++i) {
    double t = 1000.0 + i * 1500.0 + jit(rng);
    all.push_back(hit(1, t, static_cast<uint16_t>(i & 0xffff)));
    all.push_back(hit(2, t + 30.0, 1));
    all.push_back(hit(3, t + 60.0, 2));
  }
  jitter_arrival(all, 2000.0, rng);  // bounded disorder < safe_horizon (5000)

  std::vector<SortedChunk> chunks;
  auto grab = [&](SortedChunk&& c) { chunks.push_back(std::move(c)); };
  std::size_t fed = 0;
  while (fed < all.size()) {
    std::vector<ScalarHit> batch;
    for (int i = 0; i < 17 && fed < all.size(); ++i, ++fed)
      batch.push_back(all[fed]);
    s.push_batch(std::move(batch), grab);
  }
  s.flush(grab);
  CHECK(chunks.size() >= 3);  // the scenario actually exercises chunking

  auto seq1 = run_pool(1, chunks, bcfg);
  auto seq4 = run_pool(4, chunks, bcfg);
  CHECK(seq1.size() == 300);
  CHECK(seq1.size() == seq4.size());
  bool identical = seq1.size() == seq4.size();
  for (std::size_t i = 0; identical && i < seq1.size(); ++i)
    if (!near(seq1[i], seq4[i])) identical = false;
  CHECK(identical);
}

// ---------------------------------------------------------------------------

int main() {
  test_channel_fifo();
  test_channel_blocking_pop();
  test_channel_close_drains_then_false();
  test_channel_bounded_try_push();
  test_channel_pop_for_timeout();
  test_channel_pop_for_three_state();
  test_channel_pop_for_close_race();
  test_channel_move_only();
  test_channel_multi_producer();
  test_seq_reorder_in_order();
  test_seq_reorder_out_of_order();
  test_seq_reorder_gap_holds();
  test_split_empty();
  test_split_insufficient_span();
  test_split_retained_from_lookback();
  test_split_chain_invariant();
  test_flush();
  test_split_shuffled_sorts();
  test_core_range_conservation();
  test_core_range_bounds();
  test_sorter_data_time_only();
  test_sorter_first_chunk_floor();
  test_sorter_chain();
  test_sorter_reset_between_runs();
  test_sorter_flush_remainder();
  test_sorter_late_hit_lands_sorted();
  test_builder_full_event();
  test_builder_partial_arm();
  test_builder_trigger_only();
  test_builder_nearest_not_first();
  test_builder_window_inclusive();
  test_builder_core_gates();
  test_builder_boundary_backward_partner();
  test_builder_no_double_emission();
  test_builder_labr_disabled();
  test_builder_same_det_no_merge();
  test_builder_pairing();
  test_builder_chain();
  test_builder_boundary_pair();
  test_builder_through_sorter();
  test_pool_determinism();
  std::printf("test_eb_core: %d passed, %d failed\n", g_pass, g_fail);
  return g_fail == 0 ? 0 : 1;
}
