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
  CHECK(!ch.pop_for(out, 30));  // nothing arrives -> timeout -> false
  auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
                std::chrono::steady_clock::now() - t0)
                .count();
  CHECK(ms >= 25);  // actually waited (allow scheduler slop)
  int v = 5;
  ch.push(std::move(v));
  CHECK(ch.pop_for(out, 30) && out == 5);
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

int main() {
  test_channel_fifo();
  test_channel_blocking_pop();
  test_channel_close_drains_then_false();
  test_channel_bounded_try_push();
  test_channel_pop_for_timeout();
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
  std::printf("test_eb_core: %d passed, %d failed\n", g_pass, g_fail);
  return g_fail == 0 ? 0 : 1;
}
