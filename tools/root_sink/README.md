# root_sink — parallel ROOT recorder + live Δt monitor

A single C++ process that subscribes to the DELILA **merger's ZMQ PUB** as an
*additional* consumer and does two jobs at once:

1. **Recorder** — decode every event to five scalar fields and write a flat,
   ZSTD-compressed ROOT `TTree` (one file per run). Cheaper than the two-step
   `.delila` → `delila2root` path when only scalars are needed.
2. **Monitor** — a coincidence Δt monitor (gamma vs ThGEM1/ThGEM2 on configurable
   channels) served live over `THttpServer`. Browse it with JSROOT — zero
   frontend code.

The Rust **Recorder that writes `.delila` stays the authoritative recorder**.
`root_sink` is a parallel best-effort sink: it never gates, throttles, or blocks
the pipeline, so it cannot cause data loss upstream (see *Design notes*).

Japanese operator manual: [docs/root_sink_manual.md](../../docs/root_sink_manual.md)
(運用手順・ヒストグラム定義・トラブルシューティング).

## Requirements

- **ROOT** 6.20+ (for ZSTD). Provides `TFile`/`TTree`/`THttpServer`.
- **libzmq** 4.3.x (the C API only — no `cppzmq`).
- A C++17 compiler.

`sink_core.hpp` (all the logic: envelope parse, decode, matcher, run state) needs
**neither ROOT nor ZMQ** — only `../delila2root/TDelila.hpp` and a C++17
compiler — so it is unit-testable on any box.

## Build

Standard (system libzmq, e.g. `apt install libzmq3-dev`):

```sh
g++ -O2 -std=c++17 root_sink.cxx $(root-config --cflags --libs) -lRHTTP -lzmq -o root_sink
```

No-sudo variant used on **gant / side3** (no `-dev` package): drop a matching
`zmq.h` (libzmq 4.3.x) into `~/.local/include` and link the runtime `.so`
directly:

```sh
g++ -O2 -std=c++17 -I$HOME/.local/include root_sink.cxx \
    $(root-config --cflags --libs) -lRHTTP \
    /lib/x86_64-linux-gnu/libzmq.so.5 -o root_sink
```

On **side3** ROOT is `/opt/ROOT` and the binary goes in `~daq/.local/bin` (same
convention as `delila2root`); on **gant** source `thisroot.sh` first
(`.bashrc` early-returns for non-interactive shells).

## Usage

```
root_sink [options]
  --zmq ADDR          merger PUB endpoint          (default tcp://localhost:5557)
  --out-dir DIR       output directory             (default .)
  --tree NAME         TTree name                   (default delila, same as delila2root)
  --exp-name NAME     experiment name for the output filename (override)
  --operator URL      operator base URL, e.g. http://localhost:9092; experiment
                      name is read from <URL>/api/status at run start (only when
                      --exp-name is not given)
  --hists FILE        histogram definition JSON (see below); replaces the
                      built-in dt1/dt2/dt2_vs_dt1/channels set
  --gamma-ch N        gamma detector channel       (enables the Δt monitor)
  --thgem1-ch N       ThGEM1 channel
  --thgem2-ch N       ThGEM2 channel
                      all three required; if any is omitted -> recorder only
  --xy1-ch T,XL,XR,YU,YD  delay-line XY monitor 1: trigger + 4 arm channels
  --xy2-ch T,XL,XR,YU,YD  delay-line XY monitor 2
                      5 distinct ints 0..255 each; fills pos1/pos2-scope
                      histograms, so --hists is required (no built-ins)
  --window-ns X       coincidence half-window      (default 1000)
  --margin-ns X       out-of-order tolerance       (default 10000)
  --http-port N       THttpServer port, 0 disables (default 8090)
  --dt-bins N         Δt histogram bins            (default 2000)
  --dt-min X          Δt axis minimum, ns          (default -1000)
  --dt-max X          Δt axis maximum, ns          (default 1000)
  --autosave-sec N    TTree AutoSave interval      (default 30)
  --help
```

Typical ThGEM run (single V1730, gamma on ch0, ThGEMs on ch1/ch2):

```sh
root_sink --zmq tcp://localhost:5557 --out-dir /data/thgem \
          --gamma-ch 0 --thgem1-ch 1 --thgem2-ch 2 \
          --window-ns 500 --http-port 8090
```

Recorder-only (no monitor), just the scalar tree:

```sh
root_sink --out-dir /data --http-port 0
```

### Launched by `start_daq.sh`

When the DAQ config has a `[network.root_sink]` section, `scripts/start_daq.sh`
launches root_sink automatically alongside the other components. The section keys
are the CLI flag names with hyphens turned into underscores (`--out-dir` →
`output_dir`); only present keys are passed, and `--operator` is derived from the
`[operator]` port. The binary is resolved in the order `ROOT_SINK_BIN` (env) >
`PATH` > `~/.local/bin/root_sink` > `tools/root_sink/root_sink`; if none is found
it warns and skips (not fatal). It is managed symmetrically: both
`start_daq.sh` and `stop_daq.sh` now `pkill -x root_sink`, so a stale sink is
cleaned up on restart and stopped on shutdown. See
[docs/root_sink_manual.md](../../docs/root_sink_manual.md) for the section
example and operator notes.

## Output files

While a run is in progress the tree is written to
`<out-dir>/run_inprogress_<unixtime>.root` and `AutoSave`d every `--autosave-sec`
so it is already openable in ROOT. On the run's `EndOfStream` the file is closed
and renamed to

```
run%04u_0000_<exp>.root
```

using the EOS-carried run number. This is **identical to the Rust Recorder's
`.delila` filename** (`run{run:04}_{seq:04}_{exp}.delila`) but for the extension:
`root_sink` does not split runs itself, so the sequence field is normally the
literal `0000`. On a name collision it appends `_<unix_ns>` (nanoseconds since the
epoch) before the extension — the same collision scheme the Rust Recorder uses.

One caveat: ROOT itself auto-splits a `TTree`'s file when it crosses
`TTree::GetMaxTreeSize`. root_sink raises that limit to **2 TB** (unreachable
for scalar data — ~10^11 events), and if it is ever crossed anyway, the
ROOT-made continuation files (`<stem>_1.root`, …) are renamed at finalize with
Recorder-style sequence numbers (`run%04u_0001_<exp>.root`, …) plus a WARNING —
verified end-to-end with a tiny test threshold (see *Testing*). The rollover
also invalidates the original `TFile*` (ROOT deletes it in `ChangeFile`), which
is why all closing goes through `TTree::GetCurrentFile()` and rollover detection
compares file NAMES, never pointers (the allocator can reuse the address).

`<exp>` (the experiment name) is resolved once at each run start, in priority
order — the resolved name and its source are logged, and any fallback is warned
(never silent):

1. `--exp-name NAME` — explicit override, wins always.
2. `--operator URL` — HTTP `GET <URL>/api/status` and take the top-level
   `experiment_name` string. In normal UI-driven operation this matches the Rust
   Recorder's filename by construction. The fetch is bounded to ~2 s (a
   non-blocking connect + socket timeouts); ZMQ (HWM=0) buffers batches while it
   runs, so it never hangs the sink. Only `http://` is supported — `https://` is
   rejected at startup.
3. Neither given, or the fetch failed / returned no name → `"data"` (with a
   warning to stderr). This mirrors the Rust Recorder's empty-name fallback.

The tree has exactly:

| branch         | leaf | type      |
|----------------|------|-----------|
| `module`       | `/b` | `UChar_t` |
| `channel`      | `/b` | `UChar_t` |
| `energy`       | `/s` | `UShort_t`|
| `energy_short` | `/s` | `UShort_t`|
| `timestamp_ns` | `/D` | `Double_t`|

If the process is stopped mid-run (Ctrl-C / SIGTERM), the file is written and
closed but **keeps its `run_inprogress_*` name** (it was never finalized) — a log
line reports it.

## Live monitor (THttpServer / JSROOT)

Point a browser at `http://<host>:8090/`. ROOT serves the built-in JSROOT UI;
the whole tree auto-refreshes every 2 s (`_monitoring`), and each 2D histogram
draws with `colz` by default (`_drawopt`). The default (no `--hists`) objects:

- **`dt1`**  — `t(ThGEM1) − t(gamma)` [ns]
- **`dt2`**  — `t(ThGEM2) − t(gamma)` [ns]
- **`dt2_vs_dt1`** — 2D, X = Δt₁, Y = Δt₂ (each axis capped at 500 bins)
- **`channels`** — channel occupancy 0..63 (a freebie; filled for every event)
- **`/Reset`** — a command button that zeroes every currently live histogram

Histograms are **never auto-cleared on a run boundary** — they accumulate physics
across runs until you hit `/Reset` (a log line marks each boundary). The
coincidence *timing* state, on the other hand, is reset per run, because the
digitizer clock can restart between runs.

## Custom histograms (`--hists FILE`)

By default the four objects above are hard-coded. Pass `--hists FILE` and that
JSON file **fully defines** the displayed set instead (the built-ins are not
created). Without `--hists` behaviour is exactly as before — full backward
compatibility. `channels` is expressible as `x = channel`, so there is no special
case. If `--hists` is given while the HTTP server is disabled (`--http-port 0`)
the file is warned about and ignored (histograms only live on the server). If the
file has coincidence histograms but the Δt matcher is off (no `--gamma-ch /
--thgem1-ch / --thgem2-ch`), a prominent startup warning notes they will stay
empty (not fatal).

Any parse error at startup prints **all** problems and exits with code 2. On
finalize the current histogram file is copied next to the run's ROOT file as
`<final root path minus ".root">_hists.json`, so each run keeps a snapshot of the
config that produced it (it inherits any collision suffix and stays paired).

### File format

Top-level object with a `histograms` array. Each entry is one histogram:

```json
{ "histograms": [
  { "name": "dt1", "type": "TH1D", "fill": "dt1", "bins": 2000, "min": -1000, "max": 1000 }
] }
```

Common keys: `name` (required, ROOT-safe: non-empty, no `/`, unique), `title`
(optional, defaults to `name`; ROOT parses an embedded `";xaxis;yaxis"`), `type`
(`"TH1D"` or `"TH2D"`), `drawopt` (optional; empty means `colz` for 2D, nothing
for 1D).

- **1D (`TH1D`)** — `x` (the fill variable) plus `xbins` / `xmin` / `xmax`.
  Aliases: `fill` for `x`, and `bins` / `min` / `max` for the axis keys.
- **2D (`TH2D`)** — `x`, `y`, and `xbins/xmin/xmax` + `ybins/ymin/ymax`
  (no aliases).

Validation collects every error at once: unknown JSON keys (typo protection),
unknown type/variable, duplicate or unsafe names, `bins ≤ 0`, `min ≥ max`, and
**scope mixing** (see below).

### Vocabulary

Every histogram lives entirely in one **scope**; `x`, `y`, and the cut must all
belong to the same scope (mixing is an error). This is what makes "gate Δt on
gamma energy" expressible.

**hit scope** — one row per decoded event:

| kind     | names |
|----------|-------|
| variable | `energy`, `energy_short`, `channel`, `module` |
| cut      | `channel` (int: keep events on that channel), `energy_range` (`[min,max]` inclusive) |

**coinc scope** — one row per *ripe gamma* coincidence result (needs the Δt
matcher enabled):

| kind     | names |
|----------|-------|
| variable | `dt1`, `dt2`, `gamma_energy`, `thgem1_energy`, `thgem2_energy` |
| cut      | `gamma_energy_range`, `thgem1_energy_range`, `thgem2_energy_range` (each `[min,max]` inclusive) |

Coinc semantics: `gamma_energy` is always present for a ripe gamma; `dt1` /
`thgem1_energy` only fill when a ThGEM1 partner was found (likewise dt2/ThGEM2). A
`thgem1_energy_range` cut therefore fails when that partner is absent. At most one
cut key per histogram.

**pos1 / pos2 scopes** — one row per *ripe trigger* of the corresponding
delay-line XY monitor (pos1 needs `--xy1-ch`, pos2 needs `--xy2-ch`):

| kind     | names |
|----------|-------|
| variable | pos1: `xy1_x`, `xy1_y` — pos2: `xy2_x`, `xy2_y` |
| cut      | (none yet) |

Pos semantics: `xy1_x = t(XR) − t(XL)` fills only when **both** X arms matched
the trigger within ±window; `xy1_y = t(YU) − t(YD)` likewise needs both Y arms.
A 2D `x:"xy1_x", y:"xy1_y"` image therefore requires the full 4-arm coincidence,
while a 1D projection fills on its own arm pair. There are no built-in fallback
histograms for these scopes — the XY monitors only display through `--hists`.

### Standard config (reproduces the built-ins)

The repo ships `histograms.json` that reproduces the four built-in histograms
exactly — `--hists histograms.json` renders the same display as the defaults:

```json
{ "histograms": [
  { "name": "dt1",        "type": "TH1D",
    "title": "t(ThGEM1) - t(gamma) [ns];#Deltat_{1} [ns];counts",
    "fill": "dt1",     "bins": 2000, "min": -1000, "max": 1000 },
  { "name": "dt2",        "type": "TH1D",
    "title": "t(ThGEM2) - t(gamma) [ns];#Deltat_{2} [ns];counts",
    "fill": "dt2",     "bins": 2000, "min": -1000, "max": 1000 },
  { "name": "dt2_vs_dt1", "type": "TH2D",
    "title": "#Deltat_{2} vs #Deltat_{1};#Deltat_{1} [ns];#Deltat_{2} [ns]",
    "x": "dt1", "y": "dt2",
    "xbins": 500, "xmin": -1000, "xmax": 1000,
    "ybins": 500, "ymin": -1000, "ymax": 1000 },
  { "name": "channels",   "type": "TH1D",
    "title": "channel occupancy;channel;counts",
    "fill": "channel", "bins": 64, "min": 0, "max": 64 }
]}
```

### Advanced examples

Add these to your own `--hists` file (they are examples, not part of the standard
config):

Gamma energy spectrum, only for events on channel 0 (hit scope, `channel` cut):

```json
{ "name": "E_gamma", "type": "TH1D", "x": "energy", "channel": 0,
  "xbins": 4096, "xmin": 0, "xmax": 16384 }
```

Gamma energy vs Δt₁ — both axes are coinc-scope, so this is allowed:

```json
{ "name": "E_vs_dt1", "type": "TH2D", "x": "dt1", "y": "gamma_energy",
  "xbins": 400, "xmin": -1000, "xmax": 1000,
  "ybins": 512, "ymin": 0, "ymax": 16384 }
```

Δt₁ gated by gamma energy (the headline use case):

```json
{ "name": "dt1_gated", "type": "TH1D", "fill": "dt1",
  "bins": 400, "min": -200, "max": 200,
  "gamma_energy_range": [800, 1200] }
```

Delay-line XY image (pos1 scope; needs `--xy1-ch T,XL,XR,YU,YD`). Axis ranges
follow the delay line: ±window covers any arm, the physical spread is what
matters (the ThGEM measured ±250 ns):

```json
{ "name": "xy1_pos", "type": "TH2D",
  "title": "ThGEM1 XY;x = t_{XR} - t_{XL} [ns];y = t_{YU} - t_{YD} [ns]",
  "x": "xy1_x", "y": "xy1_y",
  "xbins": 500, "xmin": -250, "xmax": 250,
  "ybins": 500, "ymin": -250, "ymax": 250 }
```

### Reloading live (`/ReloadHists`)

With `--hists`, a **`/ReloadHists`** command button appears (and can be triggered
without the UI):

```sh
curl 'http://<host>:8090/ReloadHists/cmd.json'
```

It re-reads the file: on success the old histograms are unregistered and the new
set is built + registered (drawopt applied); on **any** parse error it prints them
all and **keeps the current set** unchanged. A one-line summary is logged either
way. Edit the file, hit reload — no restart, no lost data.

`/Reset` is unchanged: it zeroes every currently live histogram (built-in or
dynamic). Both commands run on the main loop (a flag flipped by the button), so a
reload can safely delete and rebuild the live objects.

## Design notes

- **Parallel sink, `.delila` untouched.** `root_sink` is one more ZMQ subscriber.
  It uses `ZMQ_RCVHWM = 0` (the project's HWM=0 rule) so ZMQ never drops on its
  socket; if it ever falls behind, the merger's PUB simply buffers for it and the
  authoritative `.delila` Recorder is unaffected. Losing the monitor never loses
  data.
- **Wire format.** Each ZMQ message is one Rust `Message` enum value, encoded by
  `rmp-serde` as `fixmap(1){ variant : payload }`:
  - `Data` → `EventDataBatch` = positional array(4) `[source_id, sequence_number,
    timestamp, events]`; each `EventData` is a positional array whose first five
    fields are `module, channel, energy, energy_short, timestamp_ns` (the rest —
    `flags`, `user_info`, `waveform` — are skipped). This mirrors
    `src/common/delila_schema.rs`; unlike a `.delila` file the ZMQ stream carries
    no schema header, so the order is hardcoded in `sink_core.hpp`.
  - `EndOfStream` → array(2) `[source_id, run_number]`.
  - `Heartbeat` → skipped. Unknown variants → one-shot warning, then skipped.
- **Run-boundary semantics.** First `Data` opens a file; the run closes once
  **every** `source_id` seen in `Data` has sent its `EndOfStream`. Single source
  ⇒ one EOS closes. This set-based rule deliberately avoids the first-EOS-latch
  trap of naive multi-source consumers. A stale EOS received while idle is logged
  and ignored.
- **Monitor disabled without channel flags.** Omit any of `--gamma-ch /
  --thgem1-ch / --thgem2-ch` and the Δt matcher is off (recorder-only). The
  `THttpServer` (if `--http-port` ≠ 0) still shows channel occupancy.
- **Threading.** HTTP requests are serviced on the main thread via
  `gSystem->ProcessEvents()`, the same thread that `Fill`s the histograms — so
  there is no lock needed between serving and filling.
- **Reuses `TDelila.hpp`.** The MessagePack reader (`tdelila::mp::Reader`, typed
  fast-path decode) is shared by `#include`, not copied. `sink_core.hpp`'s event
  decoder must stay in sync with `TDelila`'s `Schema::build_default()` and with
  `src/common/delila_schema.rs`.

## Testing

`sink_core.hpp` and `hist_config.hpp` are pure logic and compile with plain
`g++ -std=c++17` — no ROOT, no ZMQ. `test_sink_core.cpp` covers envelope parse,
batch decode, the coincidence matcher (with energies), run state, the HTTP
helpers (`split_http_response` / `extract_experiment_name`), and the histogram
config parser (all error classes + `value_of` / `pass_cut` for both scopes):

```sh
cd tools/root_sink
g++ -std=c++17 -O0 -g test_sink_core.cpp -o /tmp/ts && /tmp/ts
```

It prints `N passed, 0 failed` and exits non-zero on any failure.

The `--operator` HTTP client and all the ROOT wiring in `root_sink.cxx` are not
in the unit test (they need sockets / ROOT); they are covered by the live/E2E
runs on gant and side3.

The MaxTreeSize rollover path can be E2E-tested by compiling with a tiny
threshold and running any emulator stack:

```sh
g++ ... -DROOTSINK_TEST_MAX_TREE_SIZE=300000 root_sink.cxx ... -o root_sink_rolltest
```

A short run must then finalize into `run%04u_0000_…`, `run%04u_0001_…`, … with
the part entries summing to the Recorder's event count.
