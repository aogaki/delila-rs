// root_sink.cxx — DELILA parallel ROOT sink + live Δt monitor.
//
// One process, two roles, subscribing to the DELILA merger's ZMQ PUB as an
// ADDITIONAL consumer (the Rust Recorder that writes `.delila` stays the
// authoritative recorder — this tool never gates or throttles the pipeline):
//
//   Role 1 (recorder): decode every event to 5 scalar fields and Fill a flat,
//     ZSTD-compressed ROOT TTree. One file per run, named on the EOS-carried
//     run number. Cheaper than the two-step `.delila` -> delila2root path when
//     only scalars are needed.
//
//   Role 2 (monitor): a coincidence Δt monitor for the ThGEM test (single
//     digitizer, gamma + ThGEM1 + ThGEM2 on configurable channels). Three
//     histograms (dt1, dt2, dt2-vs-dt1) plus channel occupancy, served live over
//     THttpServer (browse with a JSROOT-capable browser — zero frontend code).
//     With --hists, up to two delay-line XY monitors (--xy1-ch/--xy2-ch: a
//     trigger plus XL/XR/YU/YD arms each) additionally fill pos1/pos2-scope
//     histograms (x = t_XR - t_XL, y = t_YU - t_YD).
//
// All logic (envelope parse, decode, matcher, run state) is in sink_core.hpp and
// is unit-tested without ROOT/ZMQ. This file is only the wiring.
//
// Build (see README.md):
//   g++ -O2 -std=c++17 root_sink.cxx $(root-config --cflags --libs) -lRHTTP -lzmq -o root_sink
//
// License: BSD-3-Clause (same as delila-rs).

#include <zmq.h>

#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <fstream>
#include <memory>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#include "eb_core.hpp"
#include "hist_config.hpp"
#include "sink_core.hpp"

#include "TApplication.h"
#include "TFile.h"
#include "TH1.h"
#include "TH2.h"
#include "THttpServer.h"
#include "TROOT.h"
#include "TRootSniffer.h"
#include "TString.h"
#include "TSystem.h"
#include "TTree.h"

using namespace rootsink;
using Clock = std::chrono::steady_clock;

// ROOT compression = algorithm*100 + level. 505 = ZSTD level 5, the same
// constant delila2root uses (kDelilaCompression). Kept as an int to avoid the
// ROOT::kZSTD enum, whose namespace moved across versions.
static const int kDelilaCompression = 505;

// ---------------------------------------------------------------------------
// Signals: SIGINT (Ctrl-C) + SIGTERM (pkill) flip a flag; the main loop drains
// and closes the file cleanly. (See MEMORY sigterm_handler_required: ctrl_c only
// covers SIGINT; a plain SIGTERM would otherwise leave the file unfinalized.)
// ---------------------------------------------------------------------------
static volatile sig_atomic_t g_stop = 0;
static void on_signal(int) { g_stop = 1; }

// ---------------------------------------------------------------------------
// HTTP-command request flags. The /Reset and /ReloadHists buttons are executed
// by Cling on the main thread; each command bakes a COMPILED function pointer
// (see main) that flips one of these flags. The ACTUAL work runs in the main
// loop right after ProcessEvents — so a reload can safely Unregister/delete the
// live histograms without doing it from inside a Cling call frame, and neither
// command bakes histogram pointers that would dangle after a reload.
// ---------------------------------------------------------------------------
static volatile sig_atomic_t g_reset = 0;
static volatile sig_atomic_t g_reload = 0;
static void request_reset() { g_reset = 1; }
static void request_reload() { g_reload = 1; }

// ---------------------------------------------------------------------------
// Built-in monitor histograms — file-scope so the main-loop /Reset handler can
// reach them without a --hists file. In --hists mode these stay null and the
// dynamic LiveHist vector owns the displayed set instead.
// ---------------------------------------------------------------------------
static TH1D* g_h_dt1 = nullptr;
static TH1D* g_h_dt2 = nullptr;
static TH2D* g_h_dt2_vs_dt1 = nullptr;
static TH1I* g_h_channels = nullptr;

// ---------------------------------------------------------------------------
// CLI options
// ---------------------------------------------------------------------------
struct Options {
  std::string zmq = "tcp://localhost:5557";
  std::string out_dir = ".";
  std::string tree = "delila";  // same default as delila2root — one macro works on both
  std::string exp_name;       // --exp-name: explicit override (wins always)
  std::string operator_url;   // --operator: fetch experiment_name from /api/status
  std::string hists_file;     // --hists: declarative histogram set
  int gamma_ch = -1;
  int thgem1_ch = -1;
  int thgem2_ch = -1;
  std::vector<int> xy1_ch;  // --xy1-ch T,XL,XR,YU,YD (empty = disabled)
  std::vector<int> xy2_ch;  // --xy2-ch likewise
  double window_ns = 1000.0;
  double margin_ns = 10000.0;
  int http_port = 8090;
  int dt_bins = 2000;
  double dt_min = -1000.0;
  double dt_max = 1000.0;
  int autosave_sec = 30;
  // Pipeline (TODO 66 §5): worker pool size and the Sorter's data-time knobs.
  int workers = 3;
  double chunk_span_ms = 100.0;    // emit a chunk per this much DATA time
  double safe_horizon_ms = 50.0;   // arrival-disorder absorption (>= max disorder)
  // --built-tree NAME: write the integrated built-event TTree (empty = off).
  // Channels come from --xy1-ch (required), --xy2-ch (optional 2nd plane) and
  // --gamma-ch (optional LaBr3); the window is the shared --window-ns.
  std::string built_tree;
};

static void print_usage(const char* argv0) {
  std::printf(
      "usage: %s [options]\n"
      "  --zmq ADDR          merger PUB endpoint (default tcp://localhost:5557)\n"
      "  --out-dir DIR       output directory for run*.root (default .)\n"
      "  --tree NAME         TTree name (default delila)\n"
      "  --exp-name NAME     experiment name for the output filename (override)\n"
      "  --operator URL      operator base URL, e.g. http://localhost:9092; the\n"
      "                      experiment name is read from <URL>/api/status at run\n"
      "                      start (used only when --exp-name is not given)\n"
      "  --hists FILE        histogram definition JSON (see README); replaces the\n"
      "                      built-in dt1/dt2/dt2_vs_dt1/channels set\n"
      "  --gamma-ch N        gamma detector channel (enables Δt monitor)\n"
      "  --thgem1-ch N       ThGEM1 channel\n"
      "  --thgem2-ch N       ThGEM2 channel\n"
      "                      (all three required; if any omitted -> recorder only)\n"
      "  --xy1-ch T,XL,XR,YU,YD  delay-line XY monitor 1: trigger + 4 arm channels\n"
      "  --xy2-ch T,XL,XR,YU,YD  delay-line XY monitor 2\n"
      "                      (5 distinct ints 0..255; fills pos1/pos2-scope\n"
      "                       histograms, so --hists is required)\n"
      "  --window-ns X       coincidence half-window (default 1000)\n"
      "  --margin-ns X       out-of-order tolerance / ripen delay (default 10000)\n"
      "  --http-port N       THttpServer port, 0 disables (default 8090)\n"
      "  --dt-bins N         Δt histogram bins (default 2000)\n"
      "  --dt-min X          Δt axis min ns (default -1000)\n"
      "  --dt-max X          Δt axis max ns (default 1000)\n"
      "  --autosave-sec N    TTree AutoSave interval (default 30)\n"
      "  --workers N         event-build worker threads (default 3)\n"
      "  --chunk-span-ms X   sorter chunk span in data time (default 100)\n"
      "  --safe-horizon-ms X sorter disorder absorption (default 50; must\n"
      "                      exceed the stream's max arrival disorder)\n"
      "  --built-tree NAME   also write an integrated built-event TTree NAME\n"
      "                      (requires --xy1-ch; --xy2-ch adds the 2nd plane,\n"
      "                       --gamma-ch adds LaBr3/TOF; window = --window-ns)\n"
      "  --help              this message\n",
      argv0);
}

// Parse argv into opt. Returns false to stop (help printed, or a bad flag).
static bool parse_args(int argc, char** argv, Options& opt, bool& help) {
  help = false;
  auto need = [&](int& i) -> const char* {
    if (i + 1 >= argc) {
      std::fprintf(stderr, "root_sink: missing value for %s\n", argv[i]);
      return nullptr;
    }
    return argv[++i];
  };
  for (int i = 1; i < argc; ++i) {
    std::string a = argv[i];
    const char* v = nullptr;
    if (a == "--help" || a == "-h") {
      print_usage(argv[0]);
      help = true;
      return false;
    } else if (a == "--zmq") {
      if (!(v = need(i))) return false;
      opt.zmq = v;
    } else if (a == "--out-dir") {
      if (!(v = need(i))) return false;
      opt.out_dir = v;
    } else if (a == "--tree") {
      if (!(v = need(i))) return false;
      opt.tree = v;
    } else if (a == "--exp-name") {
      if (!(v = need(i))) return false;
      opt.exp_name = v;
    } else if (a == "--operator") {
      if (!(v = need(i))) return false;
      opt.operator_url = v;
    } else if (a == "--hists") {
      if (!(v = need(i))) return false;
      opt.hists_file = v;
    } else if (a == "--gamma-ch") {
      if (!(v = need(i))) return false;
      opt.gamma_ch = std::atoi(v);
    } else if (a == "--thgem1-ch") {
      if (!(v = need(i))) return false;
      opt.thgem1_ch = std::atoi(v);
    } else if (a == "--thgem2-ch") {
      if (!(v = need(i))) return false;
      opt.thgem2_ch = std::atoi(v);
    } else if (a == "--xy1-ch") {
      if (!(v = need(i))) return false;
      if (!parse_ch_list(v, 5, opt.xy1_ch)) {
        std::fprintf(stderr,
                     "root_sink: --xy1-ch wants 5 distinct ints 'T,XL,XR,YU,YD' "
                     "in 0..255 (got '%s')\n", v);
        return false;
      }
    } else if (a == "--xy2-ch") {
      if (!(v = need(i))) return false;
      if (!parse_ch_list(v, 5, opt.xy2_ch)) {
        std::fprintf(stderr,
                     "root_sink: --xy2-ch wants 5 distinct ints 'T,XL,XR,YU,YD' "
                     "in 0..255 (got '%s')\n", v);
        return false;
      }
    } else if (a == "--window-ns") {
      if (!(v = need(i))) return false;
      opt.window_ns = std::atof(v);
    } else if (a == "--margin-ns") {
      if (!(v = need(i))) return false;
      opt.margin_ns = std::atof(v);
    } else if (a == "--http-port") {
      if (!(v = need(i))) return false;
      opt.http_port = std::atoi(v);
    } else if (a == "--dt-bins") {
      if (!(v = need(i))) return false;
      opt.dt_bins = std::atoi(v);
    } else if (a == "--dt-min") {
      if (!(v = need(i))) return false;
      opt.dt_min = std::atof(v);
    } else if (a == "--dt-max") {
      if (!(v = need(i))) return false;
      opt.dt_max = std::atof(v);
    } else if (a == "--autosave-sec") {
      if (!(v = need(i))) return false;
      opt.autosave_sec = std::atoi(v);
    } else if (a == "--workers") {
      if (!(v = need(i))) return false;
      opt.workers = std::atoi(v);
      if (opt.workers < 1) opt.workers = 1;
    } else if (a == "--chunk-span-ms") {
      if (!(v = need(i))) return false;
      opt.chunk_span_ms = std::atof(v);
    } else if (a == "--safe-horizon-ms") {
      if (!(v = need(i))) return false;
      opt.safe_horizon_ms = std::atof(v);
    } else if (a == "--built-tree") {
      if (!(v = need(i))) return false;
      opt.built_tree = v;
    } else {
      std::fprintf(stderr, "root_sink: unknown argument '%s' (try --help)\n", a.c_str());
      return false;
    }
  }
  return true;
}

// ---------------------------------------------------------------------------
// Minimal HTTP/1.0 GET (experiment-name lookup via --operator)
// ---------------------------------------------------------------------------
//
// A dependency-free, TIME-BOUNDED GET. It fetches `<base>/api/status` from the
// operator so the output filename can mirror the Rust Recorder's exp_name. It
// must NEVER hang the sink: connect is non-blocking with a select() deadline and
// the socket carries SO_RCVTIMEO/SO_SNDTIMEO, all inside a ~2 s total budget. A
// stall here is acceptable — ZMQ (HWM=0) buffers batches while we wait. Only
// http:// is supported; https:// is rejected at startup with a clear error.

// Parse `http://host[:port][/base/path]`. Fills host/port and the request path
// (base path with any trailing slash trimmed, then "/api/status" appended).
// Returns false with `err` set for a non-http scheme or a missing host.
static bool parse_operator_url(const std::string& url, std::string& host,
                              std::string& port, std::string& request_path,
                              std::string& err) {
  const std::string http = "http://";
  const std::string https = "https://";
  if (url.rfind(https, 0) == 0) {
    err = "https:// is not supported (use a plain http:// operator URL)";
    return false;
  }
  if (url.rfind(http, 0) != 0) {
    err = "operator URL must start with http://";
    return false;
  }
  std::string rest = url.substr(http.size());
  std::string authority = rest;
  std::string base_path;
  std::size_t slash = rest.find('/');
  if (slash != std::string::npos) {
    authority = rest.substr(0, slash);
    base_path = rest.substr(slash);  // includes the leading '/'
  }
  if (authority.empty()) {
    err = "operator URL has no host";
    return false;
  }
  std::size_t colon = authority.find(':');
  if (colon != std::string::npos) {
    host = authority.substr(0, colon);
    port = authority.substr(colon + 1);
  } else {
    host = authority;
    port = "80";
  }
  if (host.empty()) {
    err = "operator URL has no host";
    return false;
  }
  // Trim a trailing '/' from the base path so we don't produce "//api/status".
  while (!base_path.empty() && base_path.back() == '/') base_path.pop_back();
  request_path = base_path + "/api/status";
  return true;
}

// GET the operator status and return the raw response text in `out`. Returns
// false with `err` set on any failure (unresolvable host, connect timeout, send
// or recv error). Bounded to `budget_ms` total wall time.
static bool http_get_status(const std::string& url, int budget_ms, std::string& out,
                            std::string& err) {
  std::string host, port, path;
  if (!parse_operator_url(url, host, port, path, err)) return false;

  using Clock = std::chrono::steady_clock;
  auto deadline = Clock::now() + std::chrono::milliseconds(budget_ms);
  auto remaining_ms = [&]() -> long {
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(deadline - Clock::now()).count();
    return ms < 0 ? 0 : static_cast<long>(ms);
  };

  struct addrinfo hints;
  std::memset(&hints, 0, sizeof(hints));
  hints.ai_family = AF_UNSPEC;
  hints.ai_socktype = SOCK_STREAM;
  struct addrinfo* ai = nullptr;
  int gr = ::getaddrinfo(host.c_str(), port.c_str(), &hints, &ai);
  if (gr != 0 || !ai) {
    err = "getaddrinfo(" + host + ":" + port + ") failed: " + gai_strerror(gr);
    return false;
  }

  int fd = -1;
  bool connected = false;
  for (struct addrinfo* p = ai; p && !connected; p = p->ai_next) {
    fd = ::socket(p->ai_family, p->ai_socktype, p->ai_protocol);
    if (fd < 0) continue;
    // Non-blocking connect so a dead host cannot block past our deadline.
    int flags = ::fcntl(fd, F_GETFL, 0);
    ::fcntl(fd, F_SETFL, flags | O_NONBLOCK);
    int rc = ::connect(fd, p->ai_addr, p->ai_addrlen);
    if (rc == 0) {
      connected = true;
    } else if (errno == EINPROGRESS) {
      fd_set wset;
      FD_ZERO(&wset);
      FD_SET(fd, &wset);
      struct timeval tv;
      long ms = remaining_ms();
      tv.tv_sec = ms / 1000;
      tv.tv_usec = (ms % 1000) * 1000;
      int sr = ::select(fd + 1, nullptr, &wset, nullptr, &tv);
      if (sr > 0) {
        int soerr = 0;
        socklen_t len = sizeof(soerr);
        if (::getsockopt(fd, SOL_SOCKET, SO_ERROR, &soerr, &len) == 0 && soerr == 0)
          connected = true;
      }
    }
    if (!connected) {
      ::close(fd);
      fd = -1;
    }
  }
  ::freeaddrinfo(ai);
  if (!connected) {
    err = "connect to " + host + ":" + port + " timed out or failed";
    if (fd >= 0) ::close(fd);
    return false;
  }

  // Back to blocking + apply per-op timeouts for the remainder of the budget.
  int flags = ::fcntl(fd, F_GETFL, 0);
  ::fcntl(fd, F_SETFL, flags & ~O_NONBLOCK);
  auto set_timeout = [&](int opt) {
    long ms = remaining_ms();
    if (ms <= 0) ms = 1;
    struct timeval tv;
    tv.tv_sec = ms / 1000;
    tv.tv_usec = (ms % 1000) * 1000;
    ::setsockopt(fd, SOL_SOCKET, opt, &tv, sizeof(tv));
  };
  set_timeout(SO_SNDTIMEO);
  set_timeout(SO_RCVTIMEO);

  std::string req = "GET " + path + " HTTP/1.0\r\nHost: " + host +
                    "\r\nConnection: close\r\n\r\n";
  std::size_t sent = 0;
  while (sent < req.size()) {
    ssize_t w = ::send(fd, req.data() + sent, req.size() - sent, 0);
    if (w <= 0) {
      err = "send to operator failed";
      ::close(fd);
      return false;
    }
    sent += static_cast<std::size_t>(w);
    if (remaining_ms() == 0) break;
  }

  out.clear();
  char buf[4096];
  while (remaining_ms() > 0) {
    ssize_t n = ::recv(fd, buf, sizeof(buf), 0);
    if (n > 0) {
      out.append(buf, static_cast<std::size_t>(n));
    } else if (n == 0) {
      break;  // peer closed (Connection: close) — full response received
    } else {
      break;  // timeout / error — return whatever we have (may be empty)
    }
  }
  ::close(fd);
  if (out.empty()) {
    err = "empty response from operator";
    return false;
  }
  return true;
}

// Resolve the experiment name for the current run, honouring the priority order
// --exp-name > --operator (/api/status) > "data". Always logs the resolved name
// and its source; warns visibly on the "data" fallback (never silent).
static std::string resolve_exp_name(const Options& opt) {
  if (!opt.exp_name.empty()) {
    std::printf("root_sink: experiment_name = \"%s\" (source: --exp-name)\n",
                opt.exp_name.c_str());
    return opt.exp_name;
  }
  if (!opt.operator_url.empty()) {
    std::string raw, err;
    if (http_get_status(opt.operator_url, 2000, raw, err)) {
      std::string hdr, body;
      split_http_response(raw, hdr, body);
      std::string name = extract_experiment_name(body);
      if (!name.empty()) {
        std::printf("root_sink: experiment_name = \"%s\" (source: %s/api/status)\n",
                    name.c_str(), opt.operator_url.c_str());
        return name;
      }
      std::fprintf(stderr,
                   "root_sink: WARNING operator /api/status had no experiment_name; "
                   "using \"data\"\n");
      return "data";
    }
    std::fprintf(stderr,
                 "root_sink: WARNING could not fetch experiment_name from %s (%s); "
                 "using \"data\"\n",
                 opt.operator_url.c_str(), err.c_str());
    return "data";
  }
  std::fprintf(stderr,
               "root_sink: WARNING no --exp-name/--operator given; output uses "
               "experiment_name \"data\"\n");
  return "data";
}

// ---------------------------------------------------------------------------
// ROOT scalar recorder
// ---------------------------------------------------------------------------
static bool path_exists(const std::string& p) {
  struct stat st;
  return ::stat(p.c_str(), &st) == 0;
}

// Nanoseconds since the Unix epoch — the collision suffix, matching the Rust
// Recorder's `_{unix_ns}` scheme so a clash appends the same kind of tag.
static long long unix_nanos() {
  return std::chrono::duration_cast<std::chrono::nanoseconds>(
             std::chrono::system_clock::now().time_since_epoch())
      .count();
}

class Recorder {
 public:
  bool is_open() const { return file_ != nullptr; }
  const std::string& provisional() const { return provisional_; }
  long long entries() const { return entries_; }
  // The path chosen by the last finalize() (for pairing the hists-config copy).
  const std::string& final_path() const { return final_path_; }

  // Open a provisional file `<dir>/run_inprogress_<unixtime>.root` + empty tree.
  // `exp_name` is remembered for the final filename (resolved at run start so it
  // matches the Rust Recorder even though EOS/finalize come later). A non-empty
  // `built_tree_name` adds the integrated built-event tree in the SAME file.
  bool open_run(const std::string& dir, const std::string& tree_name,
                const std::string& exp_name, const std::string& built_tree_name = "") {
    out_dir_ = dir;
    exp_name_ = exp_name.empty() ? "data" : exp_name;  // mirror the Rust fallback
    entries_ = 0;
    built_entries_ = 0;
    final_path_.clear();
    provisional_ = dir + "/run_inprogress_" + std::to_string((long long)::time(nullptr)) + ".root";
    file_ = TFile::Open(provisional_.c_str(), "RECREATE", "", kDelilaCompression);
    if (!file_ || file_->IsZombie()) {
      std::fprintf(stderr, "root_sink: ERROR cannot create %s\n", provisional_.c_str());
      delete file_;
      file_ = nullptr;
      return false;
    }
    tree_ = new TTree(tree_name.c_str(), "DELILA scalar events");
    tree_->Branch("module", &module_, "module/b");
    tree_->Branch("channel", &channel_, "channel/b");
    tree_->Branch("energy", &energy_, "energy/s");
    tree_->Branch("energy_short", &energy_short_, "energy_short/s");
    tree_->Branch("timestamp_ns", &timestamp_ns_, "timestamp_ns/D");
    built_ = nullptr;
    if (!built_tree_name.empty()) {
      // One row per trigger-anchored event; missing pieces are NaN (floats) or
      // 0 (energies) — completeness is an OFFLINE cut (TODO 66 §5.2), e.g.
      // 4-fold on plane 1 = "!TMath::IsNaN(x1) && !TMath::IsNaN(y1)". Branch
      // buffers point straight into bev_ (an eb::BuiltEvent), so fill_built is
      // one struct copy.
      built_ = new TTree(built_tree_name.c_str(),
                         "DELILA built events (ThGEM planes + LaBr3, NaN = absent)");
      built_->Branch("trig1_t", &bev_.trig1_t, "trig1_t/D");
      built_->Branch("trig2_t", &bev_.trig2_t, "trig2_t/D");
      built_->Branch("labr_t", &bev_.labr_t, "labr_t/D");
      built_->Branch("x1", &bev_.x1, "x1/F");
      built_->Branch("y1", &bev_.y1, "y1/F");
      built_->Branch("x2", &bev_.x2, "x2/F");
      built_->Branch("y2", &bev_.y2, "y2/F");
      built_->Branch("dt_xl1", &bev_.dt_xl1, "dt_xl1/F");
      built_->Branch("dt_xr1", &bev_.dt_xr1, "dt_xr1/F");
      built_->Branch("dt_yu1", &bev_.dt_yu1, "dt_yu1/F");
      built_->Branch("dt_yd1", &bev_.dt_yd1, "dt_yd1/F");
      built_->Branch("dt_xl2", &bev_.dt_xl2, "dt_xl2/F");
      built_->Branch("dt_xr2", &bev_.dt_xr2, "dt_xr2/F");
      built_->Branch("dt_yu2", &bev_.dt_yu2, "dt_yu2/F");
      built_->Branch("dt_yd2", &bev_.dt_yd2, "dt_yd2/F");
      built_->Branch("dt_trig", &bev_.dt_trig, "dt_trig/F");
      built_->Branch("tof1", &bev_.tof1, "tof1/F");
      built_->Branch("tof2", &bev_.tof2, "tof2/F");
      built_->Branch("e_trig1", &bev_.e_trig1, "e_trig1/s");
      built_->Branch("e_trig2", &bev_.e_trig2, "e_trig2/s");
      built_->Branch("e_labr", &bev_.e_labr, "e_labr/s");
      built_->Branch("e_xl1", &bev_.e_xl1, "e_xl1/s");
      built_->Branch("e_xr1", &bev_.e_xr1, "e_xr1/s");
      built_->Branch("e_yu1", &bev_.e_yu1, "e_yu1/s");
      built_->Branch("e_yd1", &bev_.e_yd1, "e_yd1/s");
      built_->Branch("e_xl2", &bev_.e_xl2, "e_xl2/s");
      built_->Branch("e_xr2", &bev_.e_xr2, "e_xr2/s");
      built_->Branch("e_yu2", &bev_.e_yu2, "e_yu2/s");
      built_->Branch("e_yd2", &bev_.e_yd2, "e_yd2/s");
      built_->Branch("e_sum1", &bev_.e_sum1, "e_sum1/i");
      built_->Branch("e_sum2", &bev_.e_sum2, "e_sum2/i");
      built_->Branch("n_arms1", &bev_.n_arms1, "n_arms1/b");
      built_->Branch("n_arms2", &bev_.n_arms2, "n_arms2/b");
    }
    return true;
  }

  void fill(const ScalarHit& h) {
    if (!tree_) return;
    module_ = h.module;
    channel_ = h.channel;
    energy_ = h.energy;
    energy_short_ = h.energy_short;
    timestamp_ns_ = h.timestamp_ns;
    tree_->Fill();
    ++entries_;
  }

  void fill_built(const eb::BuiltEvent& e) {
    if (!built_) return;
    bev_ = e;
    built_->Fill();
    ++built_entries_;
  }

  long long built_entries() const { return built_entries_; }
  bool has_built() const { return built_ != nullptr; }

  // Flush the current tree(s) so the in-progress file is openable in ROOT while
  // the run is still going (SaveSelf writes tree + keys without closing).
  void autosave() {
    if (tree_) tree_->AutoSave("SaveSelf");
    if (built_) built_->AutoSave("SaveSelf");
  }

  // Close and rename to run%04u_<seq>_<exp>.root — identical to the Rust
  // Recorder filename but for the extension. Normally one file per run
  // (seq 0000); if the tree crossed TTree::GetMaxTreeSize, ROOT's automatic
  // ChangeFile has split it into <stem>_1.root, <stem>_2.root, ... — those
  // parts are renamed with the Recorder's sequence convention (0001, 0002, ...).
  // A collision appends _<unix_ns>. Returns the seq-0 final path (or the
  // provisional path if that rename failed).
  std::string finalize(uint32_t run) {
    if (!file_) return "";
    int parts = close_all_parts();
    std::string final = pick_final_name(run, 0);
    if (std::rename(provisional_.c_str(), final.c_str()) != 0) {
      std::fprintf(stderr, "root_sink: WARNING could not rename %s -> %s (%s); file kept\n",
                   provisional_.c_str(), final.c_str(), std::strerror(errno));
      final = provisional_;
    }
    // Rename any ROOT-made continuation parts (verified naming: <stem>_k.root).
    std::string stem = provisional_.substr(0, provisional_.size() - 5);  // drop ".root"
    for (int k = 1; k <= parts; ++k) {
      std::string part = stem + "_" + std::to_string(k) + ".root";
      std::string part_final = pick_final_name(run, k);
      if (std::rename(part.c_str(), part_final.c_str()) != 0)
        std::fprintf(stderr, "root_sink: WARNING could not rename %s -> %s (%s); file kept\n",
                     part.c_str(), part_final.c_str(), std::strerror(errno));
      else
        std::printf("root_sink: rollover part %d -> %s\n", k, part_final.c_str());
    }
    final_path_ = final;
    return final;
  }

  // Shutdown mid-run: write + close but keep the provisional name (the run was
  // never finalized, so it must not masquerade as a complete run%04d file).
  void close_unfinalized() {
    if (!file_) return;
    int parts = close_all_parts();
    std::printf("root_sink: shutdown mid-run — kept provisional %s (%lld events, NOT finalized)\n",
                provisional_.c_str(), entries_);
    if (parts > 0)
      std::printf("root_sink: (plus %d ROOT rollover part(s) kept as %s_*.root)\n", parts,
                  provisional_.substr(0, provisional_.size() - 5).c_str());
  }

 private:
  // Write + close the run's tree, via the CURRENT file. When the tree crossed
  // MaxTreeSize, ROOT's ChangeFile DELETED our original TFile and moved the
  // tree to <stem>_k.root — `file_` is then dangling and must not be touched
  // (empirically the allocator often reuses the same address, so a pointer
  // compare can lie; the file NAME is the only reliable rollover signal).
  // Returns the number of continuation parts (0 = the normal single-file case).
  int close_all_parts() {
    TFile* cur = tree_->GetCurrentFile();
    bool rolled = cur && provisional_ != cur->GetName();
    int parts = 0;
    if (rolled) {
      // Count <stem>_1.root ... — earlier parts were already closed by ROOT.
      std::string stem = provisional_.substr(0, provisional_.size() - 5);
      while (path_exists(stem + "_" + std::to_string(parts + 1) + ".root")) ++parts;
      std::fprintf(stderr,
                   "root_sink: WARNING tree crossed MaxTreeSize — ROOT split the run into "
                   "%d extra part(s); renaming them with Recorder-style sequence numbers\n",
                   parts);
    }
    cur->cd();
    tree_->Write();
    if (built_) built_->Write();  // both trees live in the same (current) file
    cur->Close();
    delete cur;  // == file_ only in the un-rolled case; file_ may be dangling
    file_ = nullptr;
    tree_ = nullptr;
    built_ = nullptr;
    return parts;
  }

  std::string pick_final_name(uint32_t run, int seq) const {
    char base[128];
    std::snprintf(base, sizeof(base), "run%04u_%04d_%s", run, seq, exp_name_.c_str());
    std::string cand = out_dir_ + "/" + base + ".root";
    if (!path_exists(cand)) return cand;
    // Collision -> append _<unix_ns> before the extension (Rust Recorder scheme).
    std::string c = out_dir_ + "/" + base + "_" + std::to_string(unix_nanos()) + ".root";
    return c;
  }

  TFile* file_ = nullptr;
  TTree* tree_ = nullptr;
  TTree* built_ = nullptr;  // optional built-event tree, same file
  std::string out_dir_;
  std::string exp_name_ = "data";
  std::string provisional_;
  std::string final_path_;
  long long entries_ = 0;
  long long built_entries_ = 0;
  // Branch buffers (types match the leaflist: /b UChar_t, /s UShort_t, /D Double_t).
  UChar_t module_ = 0, channel_ = 0;
  UShort_t energy_ = 0, energy_short_ = 0;
  Double_t timestamp_ns_ = 0.0;
  eb::BuiltEvent bev_;  // built-tree branch buffer (fill_built copies into it)
};

// ---------------------------------------------------------------------------
// Declarative histograms (--hists)
// ---------------------------------------------------------------------------
struct LiveHist {
  HistDef def;
  TH1* h = nullptr;  // TH1D or TH2D; not owned by any TFile (AddDirectory off)
};

// Read an entire file into `out`. Returns false on open failure.
static bool read_file(const std::string& path, std::string& out) {
  std::ifstream f(path, std::ios::binary);
  if (!f) return false;
  std::ostringstream ss;
  ss << f.rdbuf();
  out = ss.str();
  return true;
}

// The draw option to advertise for `d`: the explicit one, else "colz" for 2D,
// else empty (leave a 1D to JSROOT's default).
static std::string effective_drawopt(const HistDef& d) {
  if (!d.drawopt.empty()) return d.drawopt;
  return d.is2d ? std::string("colz") : std::string();
}

// Create + Register a TH1D/TH2D per def, set _drawopt, and collect LiveHists.
// Names are validated ROOT-safe (non-empty, no '/') by parse_hist_config.
static void build_live_hists(const std::vector<HistDef>& defs, THttpServer* server,
                             std::vector<LiveHist>& out) {
  out.clear();
  out.reserve(defs.size());
  for (const auto& d : defs) {
    TH1* h = nullptr;
    if (d.is2d) {
      h = new TH2D(d.name.c_str(), d.title.c_str(), d.xbins, d.xmin, d.xmax,
                   d.ybins, d.ymin, d.ymax);
    } else {
      h = new TH1D(d.name.c_str(), d.title.c_str(), d.xbins, d.xmin, d.xmax);
    }
    server->Register("/", h);
    std::string dopt = effective_drawopt(d);
    if (!dopt.empty())
      server->SetItemField((std::string("/") + d.name).c_str(), "_drawopt", dopt.c_str());
    out.push_back({d, h});
  }
}

// Fill every hit-scope histogram for one decoded event.
static void fill_hit_hists(std::vector<LiveHist>& live, const ScalarHit& h) {
  for (auto& lh : live) {
    if (lh.def.scope != Scope::Hit) continue;
    if (!pass_cut(lh.def.cut, h)) continue;
    double x = 0.0;
    if (!value_of(lh.def.x, h, x)) continue;
    if (lh.def.is2d) {
      double y = 0.0;
      if (!value_of(lh.def.y, h, y)) continue;
      static_cast<TH2*>(lh.h)->Fill(x, y);
    } else {
      lh.h->Fill(x);
    }
  }
}

// Fill every coinc-scope histogram for one ripe coincidence result.
static void fill_coinc_hists(std::vector<LiveHist>& live, const CoincResult& r) {
  for (auto& lh : live) {
    if (lh.def.scope != Scope::Coinc) continue;
    if (!pass_cut(lh.def.cut, r)) continue;
    double x = 0.0;
    if (!value_of(lh.def.x, r, x)) continue;
    if (lh.def.is2d) {
      double y = 0.0;
      if (!value_of(lh.def.y, r, y)) continue;
      static_cast<TH2*>(lh.h)->Fill(x, y);
    } else {
      lh.h->Fill(x);
    }
  }
}

// Fill every pos-scope histogram of `scope` for one ripe XY result. The SAME
// function serves both PositionMatcher instances: which detector `r` belongs to
// is expressed ONLY by the scope passed here (Pos1 for the xy1 matcher, Pos2 for
// xy2) — value_of computes identical math for xy1_*/xy2_* variables, so this
// filter is what keeps xy1 results out of xy2 histograms. Don't "simplify" it.
static void fill_pos_hists(std::vector<LiveHist>& live, const PosResult& r,
                           Scope scope) {
  for (auto& lh : live) {
    if (lh.def.scope != scope) continue;
    if (!pass_cut(lh.def.cut, r)) continue;
    double x = 0.0;
    if (!value_of(lh.def.x, r, x)) continue;
    if (lh.def.is2d) {
      double y = 0.0;
      if (!value_of(lh.def.y, r, y)) continue;
      static_cast<TH2*>(lh.h)->Fill(x, y);
    } else {
      lh.h->Fill(x);
    }
  }
}

// Snapshot the current --hists file next to the finalized ROOT file, named
// "<root path minus .root>_hists.json". By deriving from the (possibly
// collision-suffixed) final path, the copy stays paired with its run file.
static void copy_hists_sidecar(const std::string& hists_file,
                               const std::string& root_path) {
  if (root_path.empty()) return;
  std::string base = root_path;
  const std::string ext = ".root";
  if (base.size() >= ext.size() &&
      base.compare(base.size() - ext.size(), ext.size(), ext) == 0)
    base.erase(base.size() - ext.size());
  std::string dst = base + "_hists.json";
  std::string text;
  if (!read_file(hists_file, text)) {
    std::fprintf(stderr, "root_sink: WARNING could not read %s to pair with %s\n",
                 hists_file.c_str(), root_path.c_str());
    return;
  }
  std::ofstream out(dst, std::ios::binary | std::ios::trunc);
  if (!out) {
    std::fprintf(stderr, "root_sink: WARNING could not write %s\n", dst.c_str());
    return;
  }
  out << text;
  std::printf("root_sink: histogram config copied -> %s\n", dst.c_str());
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
int main(int argc, char** argv) {
  Options opt;
  bool help = false;
  if (!parse_args(argc, argv, opt, help)) return help ? 0 : 2;

  const bool http_enabled = opt.http_port != 0;
  const bool coinc_enabled =
      opt.gamma_ch >= 0 && opt.thgem1_ch >= 0 && opt.thgem2_ch >= 0;
  const bool xy1_enabled = opt.xy1_ch.size() == 5;
  const bool xy2_enabled = opt.xy2_ch.size() == 5;

  // The built tree is a DATA PRODUCT, not a monitor nicety: a misconfiguration
  // must be loud and fatal, never a silent recorder-only fallback.
  const bool build_enabled = !opt.built_tree.empty();
  if (build_enabled && !xy1_enabled) {
    std::fprintf(stderr,
                 "root_sink: ERROR --built-tree requires --xy1-ch (the plane-1 "
                 "trigger + arm channels)\n");
    return 2;
  }
  eb::BuilderConfig bcfg;
  if (build_enabled) {
    bcfg.trig1 = opt.xy1_ch[0];
    bcfg.xl1 = opt.xy1_ch[1];
    bcfg.xr1 = opt.xy1_ch[2];
    bcfg.yu1 = opt.xy1_ch[3];
    bcfg.yd1 = opt.xy1_ch[4];
    if (xy2_enabled) {
      bcfg.trig2 = opt.xy2_ch[0];
      bcfg.xl2 = opt.xy2_ch[1];
      bcfg.xr2 = opt.xy2_ch[2];
      bcfg.yu2 = opt.xy2_ch[3];
      bcfg.yd2 = opt.xy2_ch[4];
    }
    bcfg.labr_ch = opt.gamma_ch;  // -1 (unset) simply leaves tof/labr_t NaN
    bcfg.window_ns = opt.window_ns;
  }

  // --hists only defines monitor histograms, so it needs the HTTP server. Warn
  // and ignore it rather than silently drop it when the server is off.
  bool use_hists = http_enabled && !opt.hists_file.empty();
  if (!opt.hists_file.empty() && !http_enabled)
    std::fprintf(stderr,
                 "root_sink: WARNING --hists %s ignored because --http-port 0 "
                 "(no monitor server)\n",
                 opt.hists_file.c_str());

  // The XY monitors have no built-in fallback histograms: their results land
  // only in pos1/pos2-scope --hists entries. A channel list without --hists
  // would run the matcher into a void — warn loudly and disable instead.
  if ((xy1_enabled || xy2_enabled) && !use_hists)
    std::fprintf(stderr,
                 "root_sink: WARNING --xy1-ch/--xy2-ch given but XY histograms "
                 "only exist via --hists (pos1/pos2 scope) — XY monitor DISABLED\n");

  // Parse the histogram definitions up front — ANY error is fatal at startup.
  std::vector<HistDef> hist_defs;
  if (use_hists) {
    std::string text;
    if (!read_file(opt.hists_file, text)) {
      std::fprintf(stderr, "root_sink: ERROR cannot read --hists file %s\n",
                   opt.hists_file.c_str());
      return 2;
    }
    ParseResult pr = parse_hist_config(text);
    if (!pr.errors.empty()) {
      std::fprintf(stderr, "root_sink: %zu error(s) in %s:\n", pr.errors.size(),
                   opt.hists_file.c_str());
      for (const auto& e : pr.errors)
        std::fprintf(stderr, "root_sink:   - %s\n", e.c_str());
      return 2;
    }
    hist_defs = std::move(pr.defs);
    bool has_coinc = false, has_pos1 = false, has_pos2 = false;
    for (const auto& d : hist_defs) {
      if (d.scope == Scope::Coinc) has_coinc = true;
      if (d.scope == Scope::Pos1) has_pos1 = true;
      if (d.scope == Scope::Pos2) has_pos2 = true;
    }
    if (has_coinc && !coinc_enabled)
      std::fprintf(stderr,
                   "root_sink: WARNING --hists has coincidence histograms but the "
                   "Δt matcher is DISABLED (need --gamma-ch/--thgem1-ch/--thgem2-ch) "
                   "— those histograms will stay empty\n");
    if (has_pos1 && !xy1_enabled)
      std::fprintf(stderr,
                   "root_sink: WARNING --hists has pos1 (xy1_*) histograms but "
                   "--xy1-ch is not set — those histograms will stay empty\n");
    if (has_pos2 && !xy2_enabled)
      std::fprintf(stderr,
                   "root_sink: WARNING --hists has pos2 (xy2_*) histograms but "
                   "--xy2-ch is not set — those histograms will stay empty\n");
  }

  std::printf("root_sink: sub=%s out-dir=%s tree=%s\n", opt.zmq.c_str(),
              opt.out_dir.c_str(), opt.tree.c_str());
  if (use_hists)
    std::printf("root_sink: histograms defined by %s (%zu hist(s))\n",
                opt.hists_file.c_str(), hist_defs.size());
  if (coinc_enabled)
    std::printf("root_sink: monitor gamma=ch%d thgem1=ch%d thgem2=ch%d window=%.0fns margin=%.0fns\n",
                opt.gamma_ch, opt.thgem1_ch, opt.thgem2_ch, opt.window_ns, opt.margin_ns);
  else
    std::printf("root_sink: Δt monitor DISABLED (need --gamma-ch/--thgem1-ch/--thgem2-ch) — recorder only\n");
  if (use_hists && xy1_enabled)
    std::printf("root_sink: xy1 monitor trig=ch%d XL=ch%d XR=ch%d YU=ch%d YD=ch%d window=%.0fns margin=%.0fns\n",
                opt.xy1_ch[0], opt.xy1_ch[1], opt.xy1_ch[2], opt.xy1_ch[3],
                opt.xy1_ch[4], opt.window_ns, opt.margin_ns);
  if (use_hists && xy2_enabled)
    std::printf("root_sink: xy2 monitor trig=ch%d XL=ch%d XR=ch%d YU=ch%d YD=ch%d window=%.0fns margin=%.0fns\n",
                opt.xy2_ch[0], opt.xy2_ch[1], opt.xy2_ch[2], opt.xy2_ch[3],
                opt.xy2_ch[4], opt.window_ns, opt.margin_ns);
  if (build_enabled)
    std::printf("root_sink: built tree \"%s\": plane1 trig=ch%d%s%s window=%.0fns\n",
                opt.built_tree.c_str(), bcfg.trig1,
                xy2_enabled ? " + plane2" : " (single plane)",
                bcfg.labr_ch >= 0 ? " + LaBr3" : "", bcfg.window_ns);

  std::signal(SIGINT, on_signal);
  std::signal(SIGTERM, on_signal);

  // ROOT auto-splits a TTree's file when it crosses TTree::GetMaxTreeSize
  // (ROOT 6 default: 100 GB). Raise it to 2 TB: at 14 B/event a scalar run
  // cannot realistically get there (~10^11 events), so a run stays one file.
  // If it IS ever crossed, Recorder::finalize renames the ROOT-made parts
  // with Recorder-style sequence numbers instead of crashing on the TFile*
  // that ChangeFile deletes (see Recorder::close_all_parts).
  // ROOTSINK_TEST_MAX_TREE_SIZE exists so the rollover path can be E2E-tested
  // with a tiny threshold (see README).
#ifdef ROOTSINK_TEST_MAX_TREE_SIZE
  TTree::SetMaxTreeSize(ROOTSINK_TEST_MAX_TREE_SIZE);
#else
  TTree::SetMaxTreeSize(2000000000000LL);  // 2 TB
#endif

  // --- ROOT monitor objects (created before any TFile; detached from gDirectory
  //     so they persist across runs and are never owned/deleted by a run file) ---
  THttpServer* server = nullptr;
  std::vector<LiveHist> live_hists;  // populated only in --hists mode
  if (http_enabled) {
    TH1::AddDirectory(kFALSE);

    // A TApplication must exist for gSystem->ProcessEvents() to drive the HTTP
    // server's timer. It is process-lifetime (never deleted) — don't store it in
    // an unused local. Pass an empty argv so it doesn't try to parse our flags.
    //
    // Batch mode is mandatory for a headless DAQ sink: without it, TApplication
    // connects to $DISPLAY when one is set (e.g. an ssh session), and ROOT's
    // X11 error handler TERMINATES the process when that X connection dies —
    // observed live on side3 (sink killed mid-run when its parent ssh session
    // closed). JSROOT renders client-side, so the HTTP monitor needs no X11.
    gROOT->SetBatch(kTRUE);
    static int fake_argc = 1;
    static char a0[] = "root_sink";
    static char* fake_argv[] = {a0, nullptr};
    new TApplication("root_sink", &fake_argc, fake_argv);

    server = new THttpServer(Form("http:%d", opt.http_port));
    // 2 s JSROOT auto-refresh for the whole tree.
    server->SetItemField("/", "_monitoring", "2000");
    // The sniffer's global-directory scan defaults ON and would expose every
    // open TFile (gROOT->GetListOfFiles()) over HTTP — including the run file
    // the Writer thread is actively filling; a browser click would then read
    // that file from the HTTP thread. Our histograms are Register()ed
    // explicitly, so the scan buys nothing: turn it off. (Verify: no /Files
    // node in the JSROOT hierarchy.)
    server->GetSniffer()->SetScanGlobalDir(kFALSE);

    if (use_hists) {
      build_live_hists(hist_defs, server, live_hists);
      // /ReloadHists re-reads the file live (handled in the main loop). Only
      // registered in --hists mode. Like /Reset it bakes a compiled function
      // pointer, not histogram pointers, so it survives a reload.
      server->RegisterCommand(
          "/ReloadHists",
          Form("((void(*)())%p)();", reinterpret_cast<void*>(&request_reload)),
          "button;Reload histogram definitions");
    } else {
      g_h_dt1 = new TH1D("dt1", "t(ThGEM1) - t(gamma) [ns];#Deltat_{1} [ns];counts",
                         opt.dt_bins, opt.dt_min, opt.dt_max);
      g_h_dt2 = new TH1D("dt2", "t(ThGEM2) - t(gamma) [ns];#Deltat_{2} [ns];counts",
                         opt.dt_bins, opt.dt_min, opt.dt_max);
      // 2D reuses the Δt axes but caps each axis at 500 bins: dt-bins defaults to
      // 2000, and a 2000x2000 (=4M-bin) 2D would be needlessly heavy to draw live.
      int b2 = std::min(opt.dt_bins, 500);
      g_h_dt2_vs_dt1 = new TH2D("dt2_vs_dt1", "#Deltat_{2} vs #Deltat_{1};#Deltat_{1} [ns];#Deltat_{2} [ns]",
                                b2, opt.dt_min, opt.dt_max, b2, opt.dt_min, opt.dt_max);
      g_h_channels = new TH1I("channels", "channel occupancy;channel;counts", 64, 0, 64);
      server->Register("/", g_h_dt1);
      server->Register("/", g_h_dt2);
      server->Register("/", g_h_dt2_vs_dt1);
      server->Register("/", g_h_channels);
      server->SetItemField("/dt2_vs_dt1", "_drawopt", "colz");
    }

    // /Reset zeroes whatever histograms are currently live (built-in or dynamic).
    // It flips g_reset, consumed by the main loop; the compiled function pointer
    // keeps the command valid across a reload (no baked histogram pointers to
    // dangle). No -rdynamic needed — the pointer is baked as a literal.
    server->RegisterCommand(
        "/Reset", Form("((void(*)())%p)();", reinterpret_cast<void*>(&request_reset)),
        "button;Reset histograms");
    std::printf("root_sink: THttpServer on http://<host>:%d/  (open in a browser; JSROOT)\n",
                opt.http_port);
  }

  std::unique_ptr<CoincidenceMatcher> matcher;
  if (http_enabled && coinc_enabled) {
    CoincidenceMatcher::Config cfg;
    cfg.gamma_ch = opt.gamma_ch;
    cfg.thgem1_ch = opt.thgem1_ch;
    cfg.thgem2_ch = opt.thgem2_ch;
    cfg.window_ns = opt.window_ns;
    cfg.margin_ns = opt.margin_ns;
    matcher.reset(new CoincidenceMatcher(cfg));
  }

  // The two delay-line XY matchers share --window-ns/--margin-ns with the Δt
  // matcher (one disorder tolerance per stream). --hists required: see the
  // startup warning above.
  auto make_xy = [&](const std::vector<int>& chs) {
    PositionMatcher::Config cfg;
    cfg.trig_ch = chs[0];
    cfg.xl_ch = chs[1];
    cfg.xr_ch = chs[2];
    cfg.yu_ch = chs[3];
    cfg.yd_ch = chs[4];
    cfg.window_ns = opt.window_ns;
    cfg.margin_ns = opt.margin_ns;
    return std::unique_ptr<PositionMatcher>(new PositionMatcher(cfg));
  };
  std::unique_ptr<PositionMatcher> xy1, xy2;
  if (use_hists && xy1_enabled) xy1 = make_xy(opt.xy1_ch);
  if (use_hists && xy2_enabled) xy2 = make_xy(opt.xy2_ch);

  // --- ZMQ SUB (HWM=0 per the data-preservation rule) ---
  // Created + connected on the main thread so a bad endpoint still exits with
  // code 3 before anything spawns, then handed to the Receiver thread. ZMQ
  // permits socket migration between threads as long as use never overlaps —
  // the std::thread constructor provides the required memory barrier, and
  // only the Receiver touches the socket afterwards (it also closes it).
  void* ctx = zmq_ctx_new();
  void* sub = zmq_socket(ctx, ZMQ_SUB);
  int zero = 0;
  zmq_setsockopt(sub, ZMQ_RCVHWM, &zero, sizeof(zero));  // unlimited buffer
  zmq_setsockopt(sub, ZMQ_SUBSCRIBE, "", 0);             // all messages
  if (zmq_connect(sub, opt.zmq.c_str()) != 0) {
    std::fprintf(stderr, "root_sink: zmq_connect(%s) failed: %s\n", opt.zmq.c_str(),
                 zmq_strerror(zmq_errno()));
    return 3;
  }

  // ROOT is about to run on two threads at once (TH1 fills on this Display
  // thread, TFile/TTree exclusively on the Writer thread) — arm the global
  // locks BEFORE any of them starts.
  ROOT::EnableThreadSafety();

  // --- Cross-thread plumbing (TODO 66 §5) ----------------------------------
  // Ownership map: Receiver owns the socket + RunState; Sorter owns the
  // disorder buffer + seq counter; Workers are stateless; Writer owns the
  // Recorder (ALL TFile/TTree access); Display (this thread) owns every TH1 +
  // the THttpServer. The atomic counters are the only shared mutable state.
  struct Counters {
    std::atomic<long long> received{0};       // decoded hits (Receiver)
    std::atomic<long long> written{0};        // hits filled to the tree (Writer)
    std::atomic<long long> built{0};          // built events (Workers -> Writer)
    std::atomic<long long> chunks{0};         // Sorter emissions
    std::atomic<long long> matcher_fills{0};  // Display
    std::atomic<long long> xy_fills{0};       // Display
    std::atomic<long long> display_drops{0};  // tee overflow (counted + logged)
    std::atomic<bool> writing{false};         // stream run state (status line)
  } counters;

  const int n_workers = opt.workers;

  eb::Channel<eb::SorterMsg> to_sorter;  // record path: unbounded, never drops
  eb::Channel<eb::WorkMsg> work_q;       // record path: unbounded
  eb::Channel<eb::WorkMsg> writer_q;     // record path: unbounded
  // Display tee: bounded; DATA may drop (counted — the CLAUDE.md Monitor
  // exception; the record path above is untouched). Control markers use the
  // BLOCKING push instead: a dropped RunOpen would leave stale matcher clocks
  // across a run boundary.
  eb::Channel<eb::DisplayMsg> display_q(1000);

  // --- Receiver: ZMQ recv + decode + run state -> in-band markers ----------
  auto receiver_loop = [&] {
    RunState run_state;
    std::set<std::string> warned;
    auto process = [&](const uint8_t* data, size_t size) {
      Envelope env = parse_envelope(data, size);
      switch (env.kind) {
        case MsgKind::Data: {
          // Decode into OWNED hits before the zmq_msg buffer dies (the
          // envelope payload borrows it) and before crossing any thread.
          uint32_t sid = 0;
          std::vector<ScalarHit> hits;
          long n = decode_batch_into(env.payload, env.payload_size, sid, hits);
          if (n < 0) {
            std::fprintf(stderr, "root_sink: WARNING malformed Data batch (skipped)\n");
            break;
          }
          run_state.on_data(sid, [&] {
            counters.writing = true;
            eb::SorterMsg m;
            m.ctrl = eb::Ctrl::RunOpen;
            to_sorter.push(std::move(m));
            eb::DisplayMsg d;
            d.ctrl = eb::Ctrl::RunOpen;
            display_q.push(std::move(d));  // blocking: control is never dropped
            std::printf("root_sink: run started (source %u)\n", sid);
            std::fflush(stdout);
          });
          counters.received += static_cast<long long>(hits.size());
          if (http_enabled) {
            eb::DisplayMsg d;
            d.hits = hits;  // tee copy; the original moves to the Sorter below
            if (!display_q.try_push(std::move(d))) ++counters.display_drops;
          }
          eb::SorterMsg m;
          m.hits = std::move(hits);
          to_sorter.push(std::move(m));
          break;
        }
        case MsgKind::EndOfStream: {
          run_state.on_eos(
              env.source_id, env.run_number,
              [&](uint32_t rn) {
                counters.writing = false;
                eb::SorterMsg m;
                m.ctrl = eb::Ctrl::RunClose;
                m.run_number = rn;
                to_sorter.push(std::move(m));
                eb::DisplayMsg d;
                d.ctrl = eb::Ctrl::RunClose;
                display_q.push(std::move(d));
              },
              [&](uint32_t sid, uint32_t rn) {
                std::printf("root_sink: ignoring stale EOS (source %u, run %u) while idle\n",
                            sid, rn);
              });
          break;
        }
        case MsgKind::Heartbeat:
          break;  // liveness only — nothing to record
        case MsgKind::Unknown:
          if (warned.insert(env.variant).second)
            std::fprintf(stderr,
                         "root_sink: WARNING unknown message variant '%s' (skipping; further ones silenced)\n",
                         env.variant.c_str());
          break;
      }
    };
    while (!g_stop) {
      zmq_pollitem_t items[1];
      items[0].socket = sub;
      items[0].fd = 0;
      items[0].events = ZMQ_POLLIN;
      items[0].revents = 0;
      int rc = zmq_poll(items, 1, 100);  // 100 ms
      if (rc < 0) {
        if (zmq_errno() == EINTR) continue;  // interrupted by our signal
        std::fprintf(stderr, "root_sink: zmq_poll error: %s\n", zmq_strerror(zmq_errno()));
        break;
      }
      if (items[0].revents & ZMQ_POLLIN) {
        // Drain everything pending this wakeup (keep up with the merger).
        while (true) {
          zmq_msg_t msg;
          zmq_msg_init(&msg);
          int nb = zmq_msg_recv(&msg, sub, ZMQ_DONTWAIT);
          if (nb < 0) {
            zmq_msg_close(&msg);
            break;  // EAGAIN (drained) or error
          }
          process(static_cast<const uint8_t*>(zmq_msg_data(&msg)), zmq_msg_size(&msg));
          zmq_msg_close(&msg);
        }
      }
    }
    // Orderly teardown: one Shutdown down each path. The Sorter fans it out
    // to the workers (after flushing); the Display loop exits on its copy.
    eb::SorterMsg m;
    m.ctrl = eb::Ctrl::Shutdown;
    to_sorter.push(std::move(m));
    eb::DisplayMsg d;
    d.ctrl = eb::Ctrl::Shutdown;
    display_q.push(std::move(d));
    zmq_close(sub);  // closed on the thread that used it
  };

  // --- Sorter: disorder buffer -> sorted chunks with sequence numbers ------
  auto sorter_loop = [&] {
    eb::Sorter::Config scfg;
    scfg.chunk_span_ns = opt.chunk_span_ms * 1e6;
    scfg.safe_horizon_ns = opt.safe_horizon_ms * 1e6;
    scfg.lookback_ns = opt.window_ns;  // context carry >= coincidence window
    eb::Sorter sorter(scfg);
    uint64_t seq = 0;
    auto emit_chunk = [&](eb::SortedChunk&& c) {
      eb::WorkMsg m;
      m.seq = seq++;
      m.chunk = std::move(c);
      ++counters.chunks;
      work_q.push(std::move(m));
    };
    eb::SorterMsg in;
    while (to_sorter.pop(in)) {
      switch (in.ctrl) {
        case eb::Ctrl::None:
          sorter.push_batch(std::move(in.hits), emit_chunk);
          break;
        case eb::Ctrl::RunOpen: {
          sorter.reset();  // R11: forget the previous run's clock, or the new
                           // run's hits all fall below core_start and vanish
          eb::WorkMsg m;
          m.ctrl = eb::Ctrl::RunOpen;
          m.seq = seq++;
          work_q.push(std::move(m));
          break;
        }
        case eb::Ctrl::RunClose: {
          sorter.flush(emit_chunk);  // the run's tail becomes the last chunk
          eb::WorkMsg m;
          m.ctrl = eb::Ctrl::RunClose;
          m.seq = seq++;
          m.run_number = in.run_number;
          work_q.push(std::move(m));
          break;
        }
        case eb::Ctrl::Shutdown: {
          // Flush BEFORE the pills: a mid-run shutdown's buffered tail must
          // still reach the file (close_unfinalized keeps it). The pills then
          // carry the highest seqs, so the Writer drains all real work first.
          sorter.flush(emit_chunk);
          for (int i = 0; i < n_workers; ++i) {
            eb::WorkMsg m;
            m.ctrl = eb::Ctrl::Shutdown;
            m.seq = seq++;
            work_q.push(std::move(m));
          }
          return;
        }
      }
    }
  };

  // --- Workers: the pure-function event build; control passes through -------
  auto worker_loop = [&] {
    eb::WorkMsg m;
    while (work_q.pop(m)) {
      if (build_enabled && m.ctrl == eb::Ctrl::None)
        m.built = eb::build_events_from_chunk(m.chunk, bcfg);
      bool shutdown = (m.ctrl == eb::Ctrl::Shutdown);
      writer_q.push(std::move(m));
      if (shutdown) return;  // exactly one pill per worker
    }
  };

  // --- Writer: seq-ordered ROOT I/O (the ONLY thread touching the run file) -
  auto writer_loop = [&] {
    Recorder recorder;
    eb::SeqReorder<eb::WorkMsg> reorder;
    int pills = 0;
    auto t_last_autosave = Clock::now();
    bool done = false;
    while (!done) {
      eb::WorkMsg m;
      // Value → process; Timeout → fall through to the autosave tick below;
      // Closed → exit. Orderly shutdown still arrives as in-band pills (the
      // queue is never close()d today), but honoring Closed keeps this loop
      // correct if a future caller terminates via close() — pop_for guarantees
      // drain-then-Closed, so exiting here can never strand queued work.
      const auto pr = writer_q.pop_for(m, 200);
      if (pr == eb::PopResult::Closed) break;
      if (pr == eb::PopResult::Value) {
        reorder.push(m.seq, std::move(m));
        eb::WorkMsg r;
        while (!done && reorder.pop_ready(r)) {
          switch (r.ctrl) {
            case eb::Ctrl::None: {
              // Core range ONLY: chunks deliberately carry duplicated context
              // (lookback head + post-core tail) for the builder; writing the
              // full chunk would duplicate hits in the tree. Built events were
              // already core-gated by the builder itself.
              auto range = eb::core_range(r.chunk);
              for (std::size_t i = range.first; i < range.second; ++i)
                recorder.fill(r.chunk.hits[i]);
              counters.written += static_cast<long long>(range.second - range.first);
              for (const eb::BuiltEvent& ev : r.built) recorder.fill_built(ev);
              counters.built += static_cast<long long>(r.built.size());
              break;
            }
            case eb::Ctrl::RunOpen: {
              // exp_name resolves at the run boundary on THIS thread — the
              // bounded (~2 s) HTTP fetch only delays file naming while the
              // unbounded queues upstream absorb the stream.
              std::string exp = resolve_exp_name(opt);
              if (!recorder.open_run(opt.out_dir, opt.tree, exp, opt.built_tree))
                std::fprintf(stderr,
                             "root_sink: run start FAILED — dropping this run's events\n");
              else
                std::printf("root_sink: run file -> %s\n", recorder.provisional().c_str());
              std::fflush(stdout);
              break;
            }
            case eb::Ctrl::RunClose: {
              // Seq order makes this a barrier for free: every chunk of the
              // run has already been written when the marker pops.
              long long ev = recorder.entries();
              long long bev = recorder.built_entries();
              bool had_built = recorder.has_built();
              std::string fn = recorder.finalize(r.run_number);
              if (had_built)
                std::printf("root_sink: run %u finalized -> %s (%lld events, %lld built)\n",
                            r.run_number, fn.c_str(), ev, bev);
              else
                std::printf("root_sink: run %u finalized -> %s (%lld events)\n",
                            r.run_number, fn.c_str(), ev);
              if (use_hists) copy_hists_sidecar(opt.hists_file, recorder.final_path());
              if (http_enabled)
                std::printf("root_sink: (monitor histograms kept across the run boundary)\n");
              std::fflush(stdout);
              break;
            }
            case eb::Ctrl::Shutdown:
              if (++pills >= n_workers) done = true;  // all real work drained
              break;
          }
        }
      }
      auto now = Clock::now();
      if (recorder.is_open() &&
          std::chrono::duration_cast<std::chrono::seconds>(now - t_last_autosave)
                  .count() >= opt.autosave_sec) {
        recorder.autosave();
        t_last_autosave = now;
      }
    }
    if (recorder.is_open()) recorder.close_unfinalized();
  };

  std::thread th_receiver(receiver_loop);
  std::thread th_sorter(sorter_loop);
  std::vector<std::thread> th_workers;
  for (int i = 0; i < n_workers; ++i) th_workers.emplace_back(worker_loop);
  std::thread th_writer(writer_loop);

  // --- Display (this thread): histograms + matchers + HTTP + status --------
  // Fed by the bounded tee in ARRIVAL order (same as the old single-threaded
  // fill path), so the watermark matchers behave exactly as before.

  // Emit any coincidence results a matcher produced into the histograms. In
  // --hists mode the declarative coinc-scope set decides what gets filled; the
  // built-in mode keeps the hard-coded dt1/dt2/dt2_vs_dt1 fills.
  auto emit = [&](const CoincResult& r) {
    ++counters.matcher_fills;
    if (use_hists) {
      fill_coinc_hists(live_hists, r);
    } else {
      if (r.has_dt1) g_h_dt1->Fill(r.dt1);
      if (r.has_dt2) g_h_dt2->Fill(r.dt2);
      if (r.has_dt1 && r.has_dt2) g_h_dt2_vs_dt1->Fill(r.dt1, r.dt2);
    }
  };

  // XY results route by scope: the xy1 matcher fills pos1 histograms, xy2 fills
  // pos2. `live_hists` is captured by reference and resolved per call, so a
  // /ReloadHists rebuild is safe (nothing bakes a TH1*). xy matchers only exist
  // in --hists mode, so no built-in branch here.
  auto emit_xy1 = [&](const PosResult& r) {
    ++counters.xy_fills;
    fill_pos_hists(live_hists, r, Scope::Pos1);
  };
  auto emit_xy2 = [&](const PosResult& r) {
    ++counters.xy_fills;
    fill_pos_hists(live_hists, r, Scope::Pos2);
  };

  bool display_done = false;
  auto handle_display = [&](eb::DisplayMsg& d) {
    switch (d.ctrl) {
      case eb::Ctrl::None:
        if (http_enabled) {
          for (const ScalarHit& h : d.hits) {
            if (use_hists)
              fill_hit_hists(live_hists, h);
            else if (h.channel < 64)
              g_h_channels->Fill(h.channel);
            if (matcher) matcher->push(h, emit);
            if (xy1) xy1->push(h, emit_xy1);
            if (xy2) xy2->push(h, emit_xy2);
          }
        }
        break;
      case eb::Ctrl::RunOpen:
        if (matcher) matcher->reset();  // clock may restart between runs
        if (xy1) xy1->reset();
        if (xy2) xy2->reset();
        break;
      case eb::Ctrl::RunClose:
        if (matcher) matcher->flush(emit);  // ripen the final partial window
        if (xy1) xy1->flush(emit_xy1);
        if (xy2) xy2->flush(emit_xy2);
        break;
      case eb::Ctrl::Shutdown:
        display_done = true;
        break;
    }
  };

  std::printf("root_sink: pipeline receiver -> sorter(span=%.0fms horizon=%.0fms) -> %d worker(s) -> writer\n",
              opt.chunk_span_ms, opt.safe_horizon_ms, n_workers);
  std::printf("root_sink: running. Ctrl-C or SIGTERM to stop.\n");
  std::fflush(stdout);

  auto t_last_status = Clock::now();
  long long last_received = 0;

  while (!display_done) {
    eb::DisplayMsg d;
    auto got = display_q.pop_for(d, 100);
    while (got == eb::PopResult::Value && !display_done) {
      handle_display(d);
      got = display_q.pop_for(d, 0);  // drain whatever queued meanwhile
    }
    // Shutdown normally arrives as an in-band pill (handled above); Closed is
    // honored too so a close()-terminated tee cannot strand this loop. pop_for
    // only reports Closed once drained, so no queued display work is lost.
    if (got == eb::PopResult::Closed) display_done = true;

    // Service HTTP requests on this thread (the histograms' owner, so the
    // no-lock invariant of the single-threaded design carries over 1:1).
    if (server) gSystem->ProcessEvents();

    // Apply any HTTP command that fired during ProcessEvents. Doing the work here
    // (not inside the Cling call) lets a reload safely delete the live objects.
    if (g_reset) {
      g_reset = 0;
      if (use_hists) {
        for (auto& lh : live_hists)
          if (lh.h) lh.h->Reset();
      } else if (http_enabled) {
        if (g_h_dt1) g_h_dt1->Reset();
        if (g_h_dt2) g_h_dt2->Reset();
        if (g_h_dt2_vs_dt1) g_h_dt2_vs_dt1->Reset();
        if (g_h_channels) g_h_channels->Reset();
      }
      std::printf("root_sink: /Reset — histograms zeroed\n");
      std::fflush(stdout);
    }
    if (g_reload) {
      g_reload = 0;  // /ReloadHists is only registered in --hists mode
      std::string text;
      if (!read_file(opt.hists_file, text)) {
        std::fprintf(stderr,
                     "root_sink: /ReloadHists could not read %s — keeping current set\n",
                     opt.hists_file.c_str());
      } else {
        ParseResult pr = parse_hist_config(text);
        if (!pr.errors.empty()) {
          std::fprintf(stderr, "root_sink: /ReloadHists — %zu error(s), keeping current set:\n",
                       pr.errors.size());
          for (const auto& e : pr.errors)
            std::fprintf(stderr, "root_sink:   - %s\n", e.c_str());
        } else {
          for (auto& lh : live_hists) {
            if (lh.h) {
              server->Unregister(lh.h);
              delete lh.h;
            }
          }
          live_hists.clear();
          build_live_hists(pr.defs, server, live_hists);
          std::printf("root_sink: /ReloadHists — %zu histogram(s) now live\n",
                      live_hists.size());
        }
      }
      std::fflush(stdout);
    }

    // (autosave moved to the Writer thread — the tree's owner.)
    auto now = Clock::now();
    using std::chrono::duration;
    using std::chrono::duration_cast;
    using std::chrono::seconds;
    if (duration_cast<seconds>(now - t_last_status).count() >= 10) {
      double dt = duration_cast<duration<double>>(now - t_last_status).count();
      long long rec = counters.received;
      double rate = dt > 0 ? (double)(rec - last_received) / dt : 0.0;
      std::printf(
          "root_sink: %s | events=%lld (written=%lld) | %.0f ev/s | chunks=%lld | "
          "built=%lld | matcher_fills=%lld | xy_fills=%lld | q[sort=%zu work=%zu "
          "write=%zu disp=%zu] | display_drops=%lld\n",
          counters.writing ? "WRITING" : "idle", rec, (long long)counters.written,
          rate, (long long)counters.chunks, (long long)counters.built,
          (long long)counters.matcher_fills, (long long)counters.xy_fills,
          to_sorter.size(), work_q.size(), writer_q.size(), display_q.size(),
          (long long)counters.display_drops);
      std::fflush(stdout);
      t_last_status = now;
      last_received = rec;
    }
  }

  // Shutdown cascade: the Receiver already sent Shutdown down both paths (that
  // is what ended the display loop above); joining in pipeline order lets each
  // stage drain into the next. The Writer's close_unfinalized runs inside its
  // own thread before it exits.
  std::printf("root_sink: stopping...\n");
  std::fflush(stdout);
  th_receiver.join();
  th_sorter.join();
  for (auto& t : th_workers) t.join();
  th_writer.join();

  zmq_ctx_term(ctx);  // the Receiver closed the socket on its way out
  // ROOT objects (histograms, server, app) are process-lifetime; let the OS
  // reclaim them on exit rather than risk teardown-order surprises.
  std::printf("root_sink: bye (%lld hits received, %lld written).\n",
              (long long)counters.received, (long long)counters.written);
  return 0;
}
