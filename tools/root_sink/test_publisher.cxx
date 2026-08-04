// test_publisher — replay a root_sink hits tree over ZMQ in the merger wire
// format. Permanent E2E infrastructure for root_sink development (the TODO 65
// feeder was ad-hoc and never committed; this one is the committed answer).
//
// It plays the MERGER's role: binds a PUB socket and emits
//   Data        fixmap(1){"Data": EventDataBatch array(4)}
//   EndOfStream fixmap(1){"EndOfStream": [source_id, run_number]}
// with EventData as the 5 scalar fields root_sink records — enough for the
// scalar decoder, which skips any trailing fields anyway.
//
// Entries are replayed in STORED order, so a file recorded by the current
// single-threaded root_sink reproduces the real cross-batch arrival disorder
// (measured up to ~8 ms on side3 run0023) — exactly what the Sorter's safe
// horizon must absorb.
//
// Test hooks:
//   --repeat K   replay the file K times as runs N, N+1, ... with the SAME
//                timestamps — a faithful model of the per-run digitizer clock
//                restart (kills the stale-Sorter-floor bug, R11).
//   --no-eos     leave the run open (mid-run SIGTERM / provisional-file test).
//   --rate H     throttle to H hits/s (0 = unthrottled).
//
// Build (ROOT for the tree, zmq for the socket):
//   g++ -O2 -std=c++17 test_publisher.cxx $(root-config --cflags --libs) \
//       -lzmq -o test_publisher
//
// Example (V0/V2 gates):
//   ./test_publisher --file tmp/run0023_0000_X730_ThGEM_Test.root --run 23
//
// License: BSD-3-Clause (same as delila-rs).

#include <zmq.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

#include "TFile.h"
#include "TTree.h"

// ---------------------------------------------------------------------------
// Minimal MessagePack builder — deliberate copy of the MB helper in
// test_sink_core.cpp (kept local so the unit-test file stays untouched by
// tool builds), extended with width-aware unsigned encoders.
// ---------------------------------------------------------------------------
struct MB {
  std::vector<uint8_t> b;
  void u8(uint8_t x) { b.push_back(x); }
  void fixmap1() { u8(0x81); }
  void fixstr(const std::string& s) {
    u8(0xA0 | static_cast<uint8_t>(s.size()));  // len <= 31 here
    for (char c : s) u8(static_cast<uint8_t>(c));
  }
  void array(uint32_t n) {
    if (n <= 15) {
      u8(0x90 | static_cast<uint8_t>(n));
    } else {
      u8(0xDC);  // array16 — batches stay well under 65536 events
      u8(static_cast<uint8_t>((n >> 8) & 0xFF));
      u8(static_cast<uint8_t>(n & 0xFF));
    }
  }
  void uint8_full(uint8_t x) {  // 0..255 (posint or uint8)
    if (x <= 127) {
      u8(x);
    } else {
      u8(0xCC);
      u8(x);
    }
  }
  void uint16(uint16_t x) {
    u8(0xCD);
    u8(static_cast<uint8_t>((x >> 8) & 0xFF));
    u8(static_cast<uint8_t>(x & 0xFF));
  }
  void uint32(uint32_t x) {
    u8(0xCE);
    for (int i = 3; i >= 0; --i) u8(static_cast<uint8_t>((x >> (i * 8)) & 0xFF));
  }
  void uint64(uint64_t x) {
    u8(0xCF);
    for (int i = 7; i >= 0; --i) u8(static_cast<uint8_t>((x >> (i * 8)) & 0xFF));
  }
  void f64(double x) {
    uint64_t r;
    std::memcpy(&r, &x, 8);
    u8(0xCB);
    for (int i = 7; i >= 0; --i) u8(static_cast<uint8_t>((r >> (i * 8)) & 0xFF));
  }
};

struct Options {
  std::string file;
  std::string tree = "delila";
  std::string zmq = "tcp://127.0.0.1:5560";
  uint32_t run = 1;
  uint32_t source_id = 0;
  int batch_size = 1000;
  double rate = 0.0;  // hits/s, 0 = unthrottled
  int repeat = 1;
  bool no_eos = false;
};

static void print_usage() {
  std::printf(
      "test_publisher — replay a root_sink hits tree in the merger wire format\n"
      "\n"
      "Usage: test_publisher --file F.root [options]\n"
      "  --file PATH        hits-tree ROOT file to replay (required)\n"
      "  --tree NAME        tree name (default delila)\n"
      "  --zmq EP           PUB bind endpoint (default tcp://127.0.0.1:5560)\n"
      "  --run N            run number for EndOfStream (default 1)\n"
      "  --source-id N      batch source_id (default 0)\n"
      "  --batch-size N     events per Data message (default 1000)\n"
      "  --rate H           throttle to H hits/s (default 0 = unthrottled)\n"
      "  --repeat K         replay K times as runs N,N+1,... (default 1)\n"
      "  --no-eos           do not send EndOfStream (mid-run shutdown tests)\n");
}

static bool parse_args(int argc, char** argv, Options& o) {
  for (int i = 1; i < argc; ++i) {
    std::string a = argv[i];
    auto need = [&](const char* what) -> const char* {
      if (i + 1 >= argc) {
        std::fprintf(stderr, "test_publisher: %s needs a value\n", what);
        return nullptr;
      }
      return argv[++i];
    };
    if (a == "--help" || a == "-h") {
      print_usage();
      std::exit(0);
    } else if (a == "--file") {
      const char* v = need("--file");
      if (!v) return false;
      o.file = v;
    } else if (a == "--tree") {
      const char* v = need("--tree");
      if (!v) return false;
      o.tree = v;
    } else if (a == "--zmq") {
      const char* v = need("--zmq");
      if (!v) return false;
      o.zmq = v;
    } else if (a == "--run") {
      const char* v = need("--run");
      if (!v) return false;
      o.run = static_cast<uint32_t>(std::strtoul(v, nullptr, 10));
    } else if (a == "--source-id") {
      const char* v = need("--source-id");
      if (!v) return false;
      o.source_id = static_cast<uint32_t>(std::strtoul(v, nullptr, 10));
    } else if (a == "--batch-size") {
      const char* v = need("--batch-size");
      if (!v) return false;
      o.batch_size = std::atoi(v);
      if (o.batch_size < 1) o.batch_size = 1;
    } else if (a == "--rate") {
      const char* v = need("--rate");
      if (!v) return false;
      o.rate = std::atof(v);
    } else if (a == "--repeat") {
      const char* v = need("--repeat");
      if (!v) return false;
      o.repeat = std::atoi(v);
      if (o.repeat < 1) o.repeat = 1;
    } else if (a == "--no-eos") {
      o.no_eos = true;
    } else {
      std::fprintf(stderr, "test_publisher: unknown flag %s\n", a.c_str());
      return false;
    }
  }
  if (o.file.empty()) {
    std::fprintf(stderr, "test_publisher: --file is required\n");
    return false;
  }
  return true;
}

int main(int argc, char** argv) {
  Options opt;
  if (!parse_args(argc, argv, opt)) {
    print_usage();
    return 2;
  }

  TFile* f = TFile::Open(opt.file.c_str(), "READ");
  if (!f || f->IsZombie()) {
    std::fprintf(stderr, "test_publisher: cannot open %s\n", opt.file.c_str());
    return 2;
  }
  TTree* tree = dynamic_cast<TTree*>(f->Get(opt.tree.c_str()));
  if (!tree) {
    std::fprintf(stderr, "test_publisher: no tree '%s' in %s\n", opt.tree.c_str(),
                 opt.file.c_str());
    return 2;
  }
  UChar_t module = 0, channel = 0;
  UShort_t energy = 0, energy_short = 0;
  Double_t timestamp_ns = 0.0;
  tree->SetBranchAddress("module", &module);
  tree->SetBranchAddress("channel", &channel);
  tree->SetBranchAddress("energy", &energy);
  tree->SetBranchAddress("energy_short", &energy_short);
  tree->SetBranchAddress("timestamp_ns", &timestamp_ns);
  const Long64_t n = tree->GetEntries();

  void* ctx = zmq_ctx_new();
  void* pub = zmq_socket(ctx, ZMQ_PUB);
  int zero = 0;
  zmq_setsockopt(pub, ZMQ_SNDHWM, &zero, sizeof(zero));  // never drop (HWM=0)
  if (zmq_bind(pub, opt.zmq.c_str()) != 0) {
    std::fprintf(stderr, "test_publisher: zmq_bind(%s) failed: %s\n",
                 opt.zmq.c_str(), zmq_strerror(zmq_errno()));
    return 3;
  }
  // PUB slow-joiner: give the subscriber a moment to connect before the first
  // send, or the head of the run is silently dropped by ZMQ itself.
  std::this_thread::sleep_for(std::chrono::milliseconds(500));

  std::printf("test_publisher: %lld entries from %s -> %s (run %u, batch %d%s)\n",
              static_cast<long long>(n), opt.file.c_str(), opt.zmq.c_str(),
              opt.run, opt.batch_size, opt.no_eos ? ", NO EOS" : "");

  long long sent_total = 0;
  for (int rep = 0; rep < opt.repeat; ++rep) {
    const uint32_t run = opt.run + static_cast<uint32_t>(rep);
    uint64_t seq = 0;
    Long64_t i = 0;
    while (i < n) {
      const int count =
          static_cast<int>(std::min<Long64_t>(opt.batch_size, n - i));
      MB m;
      m.fixmap1();
      m.fixstr("Data");
      m.array(4);  // EventDataBatch [source_id, sequence_number, timestamp, events]
      m.uint32(opt.source_id);
      m.uint64(seq++);
      m.uint64(0);  // batch creation time — unused by root_sink
      m.array(static_cast<uint32_t>(count));
      for (int k = 0; k < count; ++k, ++i) {
        tree->GetEntry(i);
        m.array(5);  // EventData scalars; the decoder skips trailing fields
        m.uint8_full(module);
        m.uint8_full(channel);
        m.uint16(energy);
        m.uint16(energy_short);
        m.f64(timestamp_ns);
      }
      zmq_send(pub, m.b.data(), m.b.size(), 0);
      sent_total += count;
      if (opt.rate > 0.0) {
        double secs = static_cast<double>(count) / opt.rate;
        std::this_thread::sleep_for(
            std::chrono::microseconds(static_cast<long long>(secs * 1e6)));
      }
      if (sent_total % 500000 < count) {
        std::printf("test_publisher: %lld hits sent\n", sent_total);
      }
    }
    if (!opt.no_eos) {
      MB e;
      e.fixmap1();
      e.fixstr("EndOfStream");
      e.array(2);
      e.uint32(opt.source_id);
      e.uint32(run);
      zmq_send(pub, e.b.data(), e.b.size(), 0);
      std::printf("test_publisher: EOS sent (run %u)\n", run);
    }
    // Between repeats: a short gap so the sink finishes its finalize before
    // the "next run" begins (mirrors the real inter-run pause).
    if (rep + 1 < opt.repeat)
      std::this_thread::sleep_for(std::chrono::milliseconds(1500));
  }

  // Let the PUB pipe drain before tearing the context down.
  std::this_thread::sleep_for(std::chrono::milliseconds(500));
  zmq_close(pub);
  zmq_ctx_term(ctx);
  f->Close();
  std::printf("test_publisher: done, %lld hits total\n", sent_total);
  return 0;
}
