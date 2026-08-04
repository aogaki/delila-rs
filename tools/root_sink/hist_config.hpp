// hist_config.hpp — declarative histogram definitions for `root_sink`.
//
// A small, ROOT-free vocabulary that lets a JSON file (loaded via `--hists`)
// fully describe the monitor histograms without recompiling. Like sink_core.hpp
// this header depends ONLY on the C++ standard library plus `TDelila.hpp`'s JSON
// parser and `sink_core.hpp` (for ScalarHit / CoincResult), so the whole
// parser + fill-time logic is unit-testable with a plain `g++ -std=c++17` build.
// The ROOT wiring (creating TH1D/TH2D, Register, Fill) stays in root_sink.cxx.
//
// KISS: this is a FIXED vocabulary, not an expression engine. Four scopes:
//   * hit   — per-event scalars: variables energy, energy_short, channel, module;
//             cuts `channel` (int equals) and `energy_range` [min,max].
//   * coinc — per-ripe-gamma coincidence results: variables dt1, dt2,
//             gamma_energy, thgem1_energy, thgem2_energy; cuts
//             gamma/thgem1/thgem2_energy_range [min,max].
//   * pos1 / pos2 — per-ripe-trigger delay-line XY results (the two
//             PositionMatcher instances): variables xy1_x/xy1_y (pos1) and
//             xy2_x/xy2_y (pos2). x = t_XR - t_XL fills only when BOTH X arms
//             matched; y = t_YU - t_YD likewise. No pos cuts exist yet.
// Every histogram lives entirely in one scope: its x, its y (2D), and its cut
// must all belong to the same scope (no scope mixing). This is what makes the
// headline use case — "gate Δt on gamma energy" (x=dt1, cut=gamma_energy_range)
// — expressible while keeping fill dispatch a single scope test per histogram.
//
// License: BSD-3-Clause (same as delila-rs).

#ifndef ROOTSINK_HIST_CONFIG_HPP
#define ROOTSINK_HIST_CONFIG_HPP

#include <set>
#include <string>
#include <vector>

#include "sink_core.hpp"  // ScalarHit, CoincResult (+ tdelila::json parser)

namespace rootsink {

// Which family of records a def draws from — see the header comment.
enum class Scope { Hit, Coinc, Pos1, Pos2 };

// Plottable variables. `None` is the unset y of a 1D def.
enum class Var {
  None,
  // hit scope
  Energy,
  EnergyShort,
  Channel,
  Module,
  // coinc scope
  Dt1,
  Dt2,
  GammaEnergy,
  Thgem1Energy,
  Thgem2Energy,
  // pos1 / pos2 scopes (delay-line XY)
  Xy1X,
  Xy1Y,
  Xy2X,
  Xy2Y
};

// A single optional cut. `channel` is an equality test (ivalue); every *_range is
// an inclusive [lo,hi] test. A *_energy_range on a coinc result requires the
// matching partner to exist (see pass_cut below).
enum class CutKind {
  None,
  Channel,            // hit:   channel == ivalue
  EnergyRange,        // hit:   energy   in [lo,hi]
  GammaEnergyRange,   // coinc: gamma_energy  in [lo,hi]
  Thgem1EnergyRange,  // coinc: thgem1_energy in [lo,hi] (requires has_dt1)
  Thgem2EnergyRange   // coinc: thgem2_energy in [lo,hi] (requires has_dt2)
};

struct Cut {
  CutKind kind = CutKind::None;
  int ivalue = 0;   // Channel
  double lo = 0.0;  // *Range lower bound (inclusive)
  double hi = 0.0;  // *Range upper bound (inclusive)
};

// One parsed histogram. `title` defaults to `name`; ROOT parses any embedded
// ";x;y" axis labels. `drawopt` empty means the wiring's default (colz for 2D,
// nothing for 1D). `scope` is derived from the x variable at parse time.
struct HistDef {
  std::string name;
  std::string title;
  bool is2d = false;
  Var x = Var::None;
  Var y = Var::None;  // 2D only
  int xbins = 0;
  double xmin = 0.0, xmax = 0.0;
  int ybins = 0;
  double ymin = 0.0, ymax = 0.0;
  Cut cut;
  std::string drawopt;
  Scope scope = Scope::Hit;
};

struct ParseResult {
  std::vector<HistDef> defs;
  std::vector<std::string> errors;  // ALL problems, not just the first
};

// ---------------------------------------------------------------------------
// Vocabulary lookups
// ---------------------------------------------------------------------------

// Map a variable name to (Var, Scope). Returns false for an unknown name.
inline bool var_lookup(const std::string& n, Var& v, Scope& sc) {
  if (n == "energy") { v = Var::Energy; sc = Scope::Hit; return true; }
  if (n == "energy_short") { v = Var::EnergyShort; sc = Scope::Hit; return true; }
  if (n == "channel") { v = Var::Channel; sc = Scope::Hit; return true; }
  if (n == "module") { v = Var::Module; sc = Scope::Hit; return true; }
  if (n == "dt1") { v = Var::Dt1; sc = Scope::Coinc; return true; }
  if (n == "dt2") { v = Var::Dt2; sc = Scope::Coinc; return true; }
  if (n == "gamma_energy") { v = Var::GammaEnergy; sc = Scope::Coinc; return true; }
  if (n == "thgem1_energy") { v = Var::Thgem1Energy; sc = Scope::Coinc; return true; }
  if (n == "thgem2_energy") { v = Var::Thgem2Energy; sc = Scope::Coinc; return true; }
  if (n == "xy1_x") { v = Var::Xy1X; sc = Scope::Pos1; return true; }
  if (n == "xy1_y") { v = Var::Xy1Y; sc = Scope::Pos1; return true; }
  if (n == "xy2_x") { v = Var::Xy2X; sc = Scope::Pos2; return true; }
  if (n == "xy2_y") { v = Var::Xy2Y; sc = Scope::Pos2; return true; }
  return false;
}

// Map a cut JSON key to (CutKind, Scope). Returns false for a non-cut key.
inline bool cutkey_lookup(const std::string& key, CutKind& k, Scope& sc) {
  if (key == "channel") { k = CutKind::Channel; sc = Scope::Hit; return true; }
  if (key == "energy_range") { k = CutKind::EnergyRange; sc = Scope::Hit; return true; }
  if (key == "gamma_energy_range") { k = CutKind::GammaEnergyRange; sc = Scope::Coinc; return true; }
  if (key == "thgem1_energy_range") { k = CutKind::Thgem1EnergyRange; sc = Scope::Coinc; return true; }
  if (key == "thgem2_energy_range") { k = CutKind::Thgem2EnergyRange; sc = Scope::Coinc; return true; }
  return false;
}

// ---------------------------------------------------------------------------
// Fill-time value + cut evaluation
// ---------------------------------------------------------------------------

// Read a hit-scope variable. Always true for the four hit variables; false for a
// coinc variable (unreachable once the def passed scope validation).
inline bool value_of(Var v, const ScalarHit& h, double& out) {
  switch (v) {
    case Var::Energy: out = h.energy; return true;
    case Var::EnergyShort: out = h.energy_short; return true;
    case Var::Channel: out = h.channel; return true;
    case Var::Module: out = h.module; return true;
    default: return false;
  }
}

// Read a coinc-scope variable. False (do not fill) when the value needs a partner
// that this gamma did not get: dt1/thgem1_energy require has_dt1, dt2/thgem2_energy
// require has_dt2. gamma_energy is always valid for a ripened gamma.
inline bool value_of(Var v, const CoincResult& r, double& out) {
  switch (v) {
    case Var::Dt1:
      if (!r.has_dt1) return false;
      out = r.dt1;
      return true;
    case Var::Dt2:
      if (!r.has_dt2) return false;
      out = r.dt2;
      return true;
    case Var::GammaEnergy:
      out = r.gamma_energy;
      return true;
    case Var::Thgem1Energy:
      if (!r.has_dt1) return false;
      out = r.thgem1_energy;
      return true;
    case Var::Thgem2Energy:
      if (!r.has_dt2) return false;
      out = r.thgem2_energy;
      return true;
    default:
      return false;
  }
}

// Read a pos-scope variable. Xy1X and Xy2X (and the Ys likewise) intentionally
// compute the SAME thing: which detector a result belongs to is decided by
// fill_pos_hists' scope filter in root_sink.cxx, never here — a pos1 histogram
// only ever sees xy1 matcher results. x/y each need BOTH of their arms.
inline bool value_of(Var v, const PosResult& r, double& out) {
  switch (v) {
    case Var::Xy1X:
    case Var::Xy2X:
      if (!(r.has_xl && r.has_xr)) return false;
      out = r.dt_xr - r.dt_xl;  // = t_XR - t_XL (trig_t cancels)
      return true;
    case Var::Xy1Y:
    case Var::Xy2Y:
      if (!(r.has_yu && r.has_yd)) return false;
      out = r.dt_yu - r.dt_yd;  // = t_YU - t_YD
      return true;
    default:
      return false;  // hit/coinc var on a pos result — unreachable after
                     // scope validation
  }
}

inline bool pass_cut(const Cut& c, const ScalarHit& h) {
  switch (c.kind) {
    case CutKind::None:
      return true;
    case CutKind::Channel:
      return static_cast<int>(h.channel) == c.ivalue;
    case CutKind::EnergyRange:
      return h.energy >= c.lo && h.energy <= c.hi;
    default:
      return false;  // coinc cut on a hit — unreachable after scope validation
  }
}

inline bool pass_cut(const Cut& c, const CoincResult& r) {
  switch (c.kind) {
    case CutKind::None:
      return true;
    case CutKind::GammaEnergyRange:
      return r.gamma_energy >= c.lo && r.gamma_energy <= c.hi;
    case CutKind::Thgem1EnergyRange:
      // A partner-energy gate can only pass if that partner actually exists.
      return r.has_dt1 && r.thgem1_energy >= c.lo && r.thgem1_energy <= c.hi;
    case CutKind::Thgem2EnergyRange:
      return r.has_dt2 && r.thgem2_energy >= c.lo && r.thgem2_energy <= c.hi;
    default:
      return false;  // hit cut on a coinc — unreachable after scope validation
  }
}

// No pos-scope cuts exist yet, so only the no-cut case can pass. Every existing
// cut key is Hit/Coinc scope and parse_one's scope check already rejects it on a
// pos histogram; this overload just keeps fill dispatch total.
inline bool pass_cut(const Cut& c, const PosResult&) {
  return c.kind == CutKind::None;
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

namespace detail {

// The full set of cut keys, in a stable order for deterministic error messages.
inline const std::vector<std::string>& cut_keys() {
  static const std::vector<std::string> k = {
      "channel", "energy_range", "gamma_energy_range",
      "thgem1_energy_range", "thgem2_energy_range"};
  return k;
}

// Parse one histogram object, appending any errors and (when clean) one HistDef.
// `names` accumulates seen names for duplicate detection across the whole file.
inline void parse_one(const tdelila::json::Value& hv, std::size_t idx,
                      std::set<std::string>& names, ParseResult& pr) {
  using JV = tdelila::json::Value;
  std::string label = "histograms[" + std::to_string(idx) + "]";
  if (hv.t != JV::T::Obj) {
    pr.errors.push_back(label + ": not a JSON object");
    return;
  }
  HistDef def;
  bool ok = true;

  // --- name (also refines the error label) ---
  const JV* nv = hv.find("name");
  if (!nv || nv->t != JV::T::Str || nv->str.empty()) {
    pr.errors.push_back(label + ": missing or empty \"name\"");
    ok = false;
  } else {
    def.name = nv->str;
    label = "hist \"" + def.name + "\"";
    if (def.name.find('/') != std::string::npos) {
      pr.errors.push_back(label + ": name must not contain '/'");
      ok = false;
    }
    if (names.count(def.name)) {
      pr.errors.push_back(label + ": duplicate name");
      ok = false;
    } else {
      names.insert(def.name);
    }
  }

  // --- type (decides 1D vs 2D; without it we cannot validate axes) ---
  const JV* tv = hv.find("type");
  if (!tv || tv->t != JV::T::Str) {
    pr.errors.push_back(label + ": missing \"type\" (\"TH1D\" or \"TH2D\")");
    return;
  }
  if (tv->str == "TH1D") {
    def.is2d = false;
  } else if (tv->str == "TH2D") {
    def.is2d = true;
  } else {
    pr.errors.push_back(label + ": unknown type \"" + tv->str + "\" (want TH1D|TH2D)");
    return;
  }

  // --- title / drawopt (optional strings) ---
  if (const JV* tit = hv.find("title")) {
    if (tit->t == JV::T::Str) {
      def.title = tit->str;
    } else {
      pr.errors.push_back(label + ": \"title\" must be a string");
      ok = false;
    }
  }
  if (def.title.empty()) def.title = def.name;
  if (const JV* dp = hv.find("drawopt")) {
    if (dp->t == JV::T::Str) {
      def.drawopt = dp->str;
    } else {
      pr.errors.push_back(label + ": \"drawopt\" must be a string");
      ok = false;
    }
  }

  // --- unknown-key detection (typo protection) ---
  std::set<std::string> allowed = {"name", "title", "type", "drawopt"};
  for (const auto& ck : cut_keys()) allowed.insert(ck);
  if (def.is2d) {
    for (const char* k : {"x", "y", "xbins", "xmin", "xmax", "ybins", "ymin", "ymax"})
      allowed.insert(k);
  } else {
    // 1D accepts both the canonical x*/ names and the fill/bins/min/max aliases.
    for (const char* k : {"x", "fill", "xbins", "bins", "xmin", "min", "xmax", "max"})
      allowed.insert(k);
  }
  for (const auto& kv : hv.obj) {
    if (!allowed.count(kv.first))
      pr.errors.push_back(label + ": unknown key \"" + kv.first + "\"");
    // an unknown key is a hard error but does not block the rest of validation
    if (!allowed.count(kv.first)) ok = false;
  }

  // --- helpers for reading typed fields (first key wins over its alias) ---
  auto find2 = [&](const char* k1, const char* k2) -> const JV* {
    const JV* v = hv.find(k1);
    if (!v && k2 && *k2) v = hv.find(k2);
    return v;
  };
  auto get_num = [&](const char* k1, const char* k2, const char* what,
                     double& out) -> bool {
    const JV* v = find2(k1, k2);
    if (!v) {
      pr.errors.push_back(label + ": missing \"" + what + "\"");
      return false;
    }
    if (v->t != JV::T::Num) {
      pr.errors.push_back(label + ": \"" + what + "\" must be a number");
      return false;
    }
    out = v->num;
    return true;
  };
  auto get_var = [&](const char* k1, const char* k2, const char* what, Var& v,
                     Scope& sc) -> bool {
    const JV* jv = find2(k1, k2);
    if (!jv) {
      pr.errors.push_back(label + ": missing \"" + what + "\"");
      return false;
    }
    if (jv->t != JV::T::Str) {
      pr.errors.push_back(label + ": \"" + what + "\" must be a variable name string");
      return false;
    }
    if (!var_lookup(jv->str, v, sc)) {
      pr.errors.push_back(label + ": unknown variable \"" + jv->str + "\"");
      return false;
    }
    return true;
  };

  // --- axes + variables ---
  Scope xsc = Scope::Hit;
  bool xok = false;
  {
    Scope sc;
    xok = get_var("x", def.is2d ? "" : "fill", def.is2d ? "x" : "x/fill", def.x, sc);
    if (xok) {
      xsc = sc;
      def.scope = sc;
    } else {
      ok = false;
    }
  }
  {
    double v;
    bool b = get_num("xbins", def.is2d ? "" : "bins", def.is2d ? "xbins" : "bins", v);
    if (b) {
      def.xbins = static_cast<int>(v);
      if (def.xbins <= 0) {
        pr.errors.push_back(label + ": xbins must be > 0");
        ok = false;
      }
    } else {
      ok = false;
    }
  }
  double xmn = 0.0, xmx = 0.0;
  bool xmn_ok = get_num("xmin", def.is2d ? "" : "min", def.is2d ? "xmin" : "min", xmn);
  bool xmx_ok = get_num("xmax", def.is2d ? "" : "max", def.is2d ? "xmax" : "max", xmx);
  if (xmn_ok) def.xmin = xmn; else ok = false;
  if (xmx_ok) def.xmax = xmx; else ok = false;
  if (xmn_ok && xmx_ok && def.xmin >= def.xmax) {
    pr.errors.push_back(label + ": xmin must be < xmax");
    ok = false;
  }

  if (def.is2d) {
    Scope ysc = Scope::Hit;
    bool yok = false;
    {
      Scope sc;
      yok = get_var("y", "", "y", def.y, sc);
      if (yok) ysc = sc; else ok = false;
    }
    {
      double v;
      if (get_num("ybins", "", "ybins", v)) {
        def.ybins = static_cast<int>(v);
        if (def.ybins <= 0) {
          pr.errors.push_back(label + ": ybins must be > 0");
          ok = false;
        }
      } else {
        ok = false;
      }
    }
    double ymn = 0.0, ymx = 0.0;
    bool ymn_ok = get_num("ymin", "", "ymin", ymn);
    bool ymx_ok = get_num("ymax", "", "ymax", ymx);
    if (ymn_ok) def.ymin = ymn; else ok = false;
    if (ymx_ok) def.ymax = ymx; else ok = false;
    if (ymn_ok && ymx_ok && def.ymin >= def.ymax) {
      pr.errors.push_back(label + ": ymin must be < ymax");
      ok = false;
    }
    if (xok && yok && xsc != ysc) {
      pr.errors.push_back(label + ": scope mixing — x and y are in different scopes");
      ok = false;
    }
  }

  // --- cut (at most one key; must match the histogram's scope) ---
  int cut_count = 0;
  for (const auto& ck : cut_keys()) {
    const JV* cv = hv.find(ck);
    if (!cv) continue;
    ++cut_count;
    CutKind kind;
    Scope csc;
    cutkey_lookup(ck, kind, csc);  // always true for a cut_keys() entry
    Cut c;
    c.kind = kind;
    bool val_ok = true;
    if (kind == CutKind::Channel) {
      if (cv->t != JV::T::Num) {
        pr.errors.push_back(label + ": cut \"channel\" must be an integer");
        val_ok = false;
        ok = false;
      } else {
        c.ivalue = static_cast<int>(cv->num);
      }
    } else {
      if (cv->t != JV::T::Arr || cv->arr.size() != 2 ||
          cv->arr[0].t != JV::T::Num || cv->arr[1].t != JV::T::Num) {
        pr.errors.push_back(label + ": cut \"" + ck + "\" must be [min,max] numbers");
        val_ok = false;
        ok = false;
      } else {
        c.lo = cv->arr[0].num;
        c.hi = cv->arr[1].num;
        if (c.lo > c.hi) {
          pr.errors.push_back(label + ": cut \"" + ck + "\" has min > max");
          val_ok = false;
          ok = false;
        }
      }
    }
    if (val_ok) def.cut = c;
    if (xok && csc != def.scope) {
      pr.errors.push_back(label + ": scope mixing — cut \"" + ck +
                          "\" is not in the same scope as x");
      ok = false;
    }
  }
  if (cut_count > 1) {
    pr.errors.push_back(label + ": at most one cut key per histogram");
    ok = false;
  }

  if (ok) pr.defs.push_back(std::move(def));
}

}  // namespace detail

// Parse a `{"histograms":[ ... ]}` document. Collects ALL errors (never stops at
// the first) so a config author sees every problem in one run. On any error the
// caller must NOT use `defs` (root_sink.cxx exits, or keeps the previous set on a
// live reload). Malformed JSON yields exactly one clear error.
inline ParseResult parse_hist_config(const std::string& json_text) {
  using JV = tdelila::json::Value;
  ParseResult pr;
  JV root;
  try {
    root = tdelila::json::Parser(json_text).parse();
  } catch (const std::exception& e) {
    pr.errors.push_back(std::string("malformed JSON: ") + e.what());
    return pr;
  }
  if (root.t != JV::T::Obj) {
    pr.errors.push_back("top-level must be an object with a \"histograms\" array");
    return pr;
  }
  const JV* hists = root.find("histograms");
  if (!hists || hists->t != JV::T::Arr) {
    pr.errors.push_back("missing or non-array \"histograms\"");
    return pr;
  }
  std::set<std::string> names;
  for (std::size_t i = 0; i < hists->arr.size(); ++i)
    detail::parse_one(hists->arr[i], i, names, pr);
  return pr;
}

}  // namespace rootsink

#endif  // ROOTSINK_HIST_CONFIG_HPP
