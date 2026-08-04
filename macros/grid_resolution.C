// grid_resolution.C — delay-line position resolution, decomposed into the
// instrumental (timing) and physical (charge spread etc.) contributions.
//
// Method:
//   1. Build 4-fold events: for each ch1 trigger take the nearest hit on each
//      arm (XL=ch2, XR=ch3, YU=ch4, YD=ch5) within +-WINDOW ns.
//
//   2. GRID-BAR method (measured imaging resolution): project x = tXR - tXL
//      and y = tYU - tYD, auto-detect the support-grid shadow dips, fit each
//      with a top-hat bar convolved with a Gaussian response:
//        N(u) = A * (1 - D * 0.5*(erf((u-c+w)/(sqrt2*s)) - erf((u-c-w)/(sqrt2*s))))
//      The fitted s is the LOCAL measured resolution (timing (+) physics).
//
//   3. CHECKSUM method (timing-only resolution): with A = tXL + tXR and
//      B = tYU + tYD (each = 2*t_event + fixed delay + noise), fit the
//      Gaussian cores of the three combinations
//        V1 = Var(A - 2*t_trig) = sigma_A^2 + 4*sigma_trig^2
//        V2 = Var(B - 2*t_trig) = sigma_B^2 + 4*sigma_trig^2
//        V3 = Var(A - B)        = sigma_A^2 + sigma_B^2
//      and solve: sigma_A^2 = (V1 - V2 + V3)/2, sigma_B^2 = (V3 - V1 + V2)/2,
//      sigma_trig^2 = (V1 + V2 - V3)/8. For uncorrelated arms
//      Var(sum) = Var(diff), so sigma_A IS the X timing resolution
//      (and sigma_B the Y one) — the event time cancels in each combination.
//
//   4. DECOMPOSITION: per axis, sigma_phys = sqrt(sigma_grid^2 - sigma_timing^2)
//      using the weighted mean of the grid-bar sigmas.
//
// Output: fit tables + summary on stdout, grid_resolution.png with the fits.
// Usage:  root -l -b -q grid_resolution.C
#include <TCanvas.h>
#include <TF1.h>
#include <TFile.h>
#include <TGraphErrors.h>
#include <TH1D.h>
#include <TLegend.h>
#include <TLine.h>
#include <TTree.h>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <memory>
#include <utility>
#include <vector>

namespace
{

constexpr double WINDOW = 500.0;  // ns, arm-matching half-window
constexpr double MM_PER_NS = 0.188696;
constexpr double PLATEAU_LIMIT = 190;  // |u| < this avoids the aperture edges
constexpr double DIP_THRESHOLD =
    0.80;  // smoothed bin < threshold*median => dip

struct BarFit {
  double center, halfwidth, sigma, sigma_err, depth;
  bool ok;
};

double plateau_median(TH1D *h)
{
  std::vector<double> v;
  for (int b = 1; b <= h->GetNbinsX(); ++b) {
    double u = h->GetBinCenter(b);
    if (std::fabs(u) < PLATEAU_LIMIT) v.push_back(h->GetBinContent(b));
  }
  std::nth_element(v.begin(), v.begin() + v.size() / 2, v.end());
  return v[v.size() / 2];
}

// Dip detection on a SMOOTHED copy (kills single-bin statistical dips), then
// merge candidates closer than 15 ns. Fits still run on the raw histogram.
std::vector<double> find_dips(TH1D *h, double median)
{
  std::unique_ptr<TH1D> sm(static_cast<TH1D *>(h->Clone("sm_tmp")));
  sm->SetDirectory(nullptr);
  sm->Smooth(2);

  std::vector<double> centers;
  int start = 0;
  double minval = 1e30, minpos = 0;
  auto flush = [&]() {
    if (start) centers.push_back(minpos);
    start = 0;
    minval = 1e30;
  };
  for (int b = 1; b <= sm->GetNbinsX(); ++b) {
    double u = sm->GetBinCenter(b);
    if (std::fabs(u) >= PLATEAU_LIMIT) {
      flush();
      continue;
    }
    if (sm->GetBinContent(b) < DIP_THRESHOLD * median) {
      if (!start) start = b;
      if (sm->GetBinContent(b) < minval) {
        minval = sm->GetBinContent(b);
        minpos = u;
      }
    } else if (start) {
      flush();
    }
  }
  flush();

  // merge near-duplicates (keep the first of each cluster)
  std::vector<double> merged;
  for (double c : centers)
    if (merged.empty() || c - merged.back() > 15.0) merged.push_back(c);
  return merged;
}

BarFit fit_bar(TH1D *h, double center, double median)
{
  TF1 fd("fd",
         "[0]*(1-[1]*0.5*(TMath::Erf((x-[2]+[3])/(sqrt(2.)*[4]))"
         "-TMath::Erf((x-[2]-[3])/(sqrt(2.)*[4]))))",
         center - 20, center + 20);
  fd.SetParameters(median, 0.4, center, 3.0, 1.5);
  fd.SetParLimits(1, 0.05, 1.0);
  fd.SetParLimits(2, center - 8, center + 8);
  fd.SetParLimits(3, 0.5, 12.0);
  fd.SetParLimits(4, 0.3, 8.0);
  int st = h->Fit(&fd, "Q0R+");
  BarFit r;
  r.center = fd.GetParameter(2);
  r.halfwidth = fd.GetParameter(3);
  r.sigma = fd.GetParameter(4);
  r.sigma_err = fd.GetParError(4);
  r.depth = fd.GetParameter(1);
  // at-limit sigma or failed status => don't trust
  r.ok = (st == 0) && r.sigma > 0.31 && r.sigma < 7.9;
  return r;
}

// Weighted mean of the accepted bar sigmas: the "measured" resolution.
// `bars` keeps every accepted fit for the per-position breakdown.
struct AxisResult {
  double sigma = 0, err = 0;
  int nbars = 0;
  std::vector<BarFit> bars;
};

AxisResult analyze_axis(const char *name, TH1D *h)
{
  double med = plateau_median(h);
  auto dips = find_dips(h, med);
  std::printf(
      "\n%s axis: plateau median %.0f counts/bin, %zu bar candidate(s)\n", name,
      med, dips.size());
  std::printf("  %-10s %-12s %-22s %s\n", "center", "bar width",
              "sigma (resolution)", "depth");
  double sw = 0, swx = 0;
  AxisResult res;
  for (double c : dips) {
    BarFit r = fit_bar(h, c, med);
    if (!r.ok) {
      std::printf("  %7.1f ns  -- fit rejected --\n", c);
      continue;
    }
    std::printf("  %7.1f ns  %5.2f ns     %.2f +- %.2f ns = %.3f mm  %4.0f%%\n",
                r.center, 2 * r.halfwidth, r.sigma, r.sigma_err,
                r.sigma * MM_PER_NS, 100 * r.depth);
    if (r.sigma_err > 1e-6) {
      double w = 1.0 / (r.sigma_err * r.sigma_err);
      sw += w;
      swx += w * r.sigma;
      res.nbars++;
      res.bars.push_back(r);
    }
  }
  if (sw > 0) {
    res.sigma = swx / sw;
    res.err = std::sqrt(1.0 / sw);
    std::printf("  weighted mean: %.2f +- %.2f ns = %.3f mm (%d bars)\n",
                res.sigma, res.err, res.sigma * MM_PER_NS, res.nbars);
  }
  return res;
}

// Aperture-edge fit: erfc step at the active-area boundary. Extends the
// positional coverage beyond the outermost grid bar (out to ~+-209 ns). Note
// the extra systematics vs the bars: an illumination gradient at the boundary
// and the source-grid penumbra both inflate the apparent sigma, so treat edge
// points as upper limits.
struct EdgeFit {
  double pos, sigma, sigma_err;
  bool ok;
};

EdgeFit fit_edge(TH1D *h, double lo, double hi, int sign, double plateau)
{
  TF1 fe("fe", "[0]*0.5*TMath::Erfc([3]*(x-[1])/(sqrt(2.)*[2]))+[4]", lo, hi);
  fe.SetParameters(plateau, sign > 0 ? 208.0 : -208.0, 3.0, (double)sign, 0.0);
  fe.FixParameter(3, (double)sign);
  fe.SetParLimits(1, sign > 0 ? lo + 10 : lo, sign > 0 ? hi : hi - 10);
  fe.SetParLimits(2, 0.3, 30.0);
  int st = h->Fit(&fe, "Q0R+");
  EdgeFit r;
  r.pos = fe.GetParameter(1);
  r.sigma = fe.GetParameter(2);
  r.sigma_err = fe.GetParError(2);
  r.ok = (st == 0) && r.sigma > 0.31 && r.sigma < 29.0;
  return r;
}

// Iterative gaussian core fit (+-2 sigma, 3 passes). Returns (sigma, sigma_err).
std::pair<double, double> gauss_core(TH1D *h)
{
  double m = h->GetMean(), s = h->GetRMS();
  TF1 g("gc", "gaus");
  for (int it = 0; it < 3; ++it) {
    h->Fit(&g, "Q0", "", m - 2 * s, m + 2 * s);
    m = g.GetParameter(1);
    s = g.GetParameter(2);
  }
  return {s, g.GetParError(2)};
}

// sqrt(a^2 - b^2) with errors; flags the "consistent with zero" case.
struct Sub {
  double val = 0, err = 0;
  bool below = false;  // a^2 - b^2 <= 0 within the numbers
};

Sub quad_subtract(double a, double ea, double b, double eb)
{
  Sub r;
  double d = a * a - b * b;
  if (d <= 0) {
    r.below = true;
    return r;
  }
  r.val = std::sqrt(d);
  r.err = std::sqrt(a * a * ea * ea + b * b * eb * eb) / r.val;
  return r;
}

}  // namespace

void grid_resolution()
{
  const char *fileName = "run0045_0000_X730_ThGEM_Test.root";
  auto file = TFile::Open(fileName, "READ");
  if (!file || file->IsZombie()) {
    std::fprintf(stderr, "cannot open %s\n", fileName);
    return;
  }
  auto tree = dynamic_cast<TTree *>(file->Get("delila"));
  tree->SetBranchStatus("*", 0);
  UChar_t ch;
  Double_t ts;
  tree->SetBranchStatus("channel", 1);
  tree->SetBranchAddress("channel", &ch);
  tree->SetBranchStatus("timestamp_ns", 1);
  tree->SetBranchAddress("timestamp_ns", &ts);

  std::vector<Double_t> byCh[6];
  const Long64_t n = tree->GetEntries();
  for (Long64_t i = 0; i < n; ++i) {
    tree->GetEntry(i);
    if (ch < 6) byCh[ch].push_back(ts);
  }
  for (auto &v : byCh) std::sort(v.begin(), v.end());

  auto nearest = [&](int c, Double_t t0) {
    const auto &v = byCh[c];
    auto it = std::lower_bound(v.begin(), v.end(), t0);
    Double_t best = NAN;
    if (it != v.end()) best = *it;
    if (it != v.begin() && (std::isnan(best) || t0 - *(it - 1) < best - t0))
      best = *(it - 1);
    return (std::fabs(best - t0) <= WINDOW) ? best : NAN;
  };

  auto hX = new TH1D("hX", "x projection;x = t_{XR} - t_{XL} [ns];counts/ns",
                     500, -250, 250);
  auto hY = new TH1D("hY", "y projection;y = t_{YU} - t_{YD} [ns];counts/ns",
                     500, -250, 250);
  // checksum combinations (fixed delay offsets land somewhere in +-1000)
  auto hAB = new TH1D("hAB", "A-B", 2000, -1000, 1000);
  auto hA2 = new TH1D("hA2", "A-2t0", 2000, -1000, 1000);
  auto hB2 = new TH1D("hB2", "B-2t0", 2000, -1000, 1000);
  long fourfold = 0;
  for (auto t0 : byCh[1]) {
    Double_t xl = nearest(2, t0), xr = nearest(3, t0);
    Double_t yu = nearest(4, t0), yd = nearest(5, t0);
    if (std::isnan(xl) || std::isnan(xr) || std::isnan(yu) || std::isnan(yd))
      continue;
    ++fourfold;
    hX->Fill(xr - xl);
    hY->Fill(yu - yd);
    double A = xl + xr, B = yu + yd;
    hAB->Fill(A - B);
    hA2->Fill(A - 2 * t0);
    hB2->Fill(B - 2 * t0);
  }
  std::printf("%s: %lld entries, %ld 4-fold events\n", fileName, n, fourfold);

  AxisResult gx = analyze_axis("X", hX);
  AxisResult gy = analyze_axis("Y", hY);

  // --- Checksum: solve the three-variance system for the timing sigmas ---
  auto [sAB, eAB] = gauss_core(hAB);
  auto [sA2, eA2] = gauss_core(hA2);
  auto [sB2, eB2] = gauss_core(hB2);
  double V3 = sAB * sAB, dV3 = 2 * sAB * eAB;
  double V1 = sA2 * sA2, dV1 = 2 * sA2 * eA2;
  double V2 = sB2 * sB2, dV2 = 2 * sB2 * eB2;
  std::printf(
      "\nchecksum cores: sigma(A-B)=%.2f  sigma(A-2t0)=%.2f  sigma(B-2t0)=%.2f "
      "ns\n",
      sAB, sA2, sB2);

  auto solve = [](double v, double dv2) {
    Sub r;
    if (v <= 0) {
      r.below = true;
      return r;
    }
    r.val = std::sqrt(v);
    r.err = 0.5 * dv2 / r.val;
    return r;
  };
  double dq = std::sqrt(dV1 * dV1 + dV2 * dV2 + dV3 * dV3);
  Sub tx = solve((V1 - V2 + V3) / 2, dq / 2);  // sigma_A = X timing sigma
  Sub ty = solve((V3 - V1 + V2) / 2, dq / 2);  // sigma_B = Y timing sigma
  Sub tt = solve((V1 + V2 - V3) / 8, dq / 8);  // trigger-anode spread

  // The 3-variance closure assumes the trigger enters A-2t0 and B-2t0 with an
  // uncorrelated, axis-independent term. When the trigger-vs-anode spread
  // (drift-time variation + walk) dominates — sigma(A-2t0) >> sigma(A-B) — the
  // solution can go negative. In that case fall back to the clean A-B checksum
  // with an equal X/Y split: sigma_x = sigma_y = sigma(A-B)/sqrt(2).
  const bool fallback = tx.below || ty.below;
  if (fallback) {
    std::printf(
        "NOTE: trigger-referenced system inconsistent (trigger-vs-anode spread"
        " ~%.1f ns dominates) — using equal-split of sigma(A-B) for timing.\n",
        tt.below ? 0.0 : tt.val);
    tx.val = ty.val = sAB / std::sqrt(2.0);
    tx.err = ty.err = eAB / std::sqrt(2.0);
    tx.below = ty.below = false;
  }

  // --- Decomposition: physics = grid (-) timing, in quadrature ---
  std::printf("\n=== Resolution decomposition (%.6f mm/ns) ===\n", MM_PER_NS);
  std::printf("%-28s %-24s %s\n", "", "X", "Y");
  auto row = [](const char *what, double v, double e, double v2, double e2) {
    std::printf(
        "%-28s %5.2f +- %4.2f ns (%.3f mm)  %5.2f +- %4.2f ns (%.3f mm)\n",
        what, v, e, v * MM_PER_NS, v2, e2, v2 * MM_PER_NS);
  };
  row(fallback ? "timing (A-B, equal split)" : "timing (checksum)", tx.val,
      tx.err, ty.val, ty.err);
  row("measured (grid bars)", gx.sigma, gx.err, gy.sigma, gy.err);
  Sub px = quad_subtract(gx.sigma, gx.err, tx.val, tx.err);
  Sub py = quad_subtract(gy.sigma, gy.err, ty.val, ty.err);
  std::printf("%-28s ", "physics (quad. difference)");
  if (px.below)
    std::printf("consistent with 0         ");
  else
    std::printf("%5.2f +- %4.2f ns (%.3f mm)  ", px.val, px.err,
                px.val * MM_PER_NS);
  if (py.below)
    std::printf("consistent with 0\n");
  else
    std::printf("%5.2f +- %4.2f ns (%.3f mm)\n", py.val, py.err,
                py.val * MM_PER_NS);
  if (!tt.below)
    std::printf(
        "(bonus) trigger-vs-anode spread (drift-time variation + walk, from\n"
        "        sigma(A-2t0)/sigma(B-2t0)): %5.2f +- %4.2f ns\n",
        tt.val, tt.err);

  // --- Positional dependence: decompose each bar separately ---
  // The timing sigma is one number per axis (the checksum has no position
  // information), so any bar-to-bar variation shows up in the physics term.
  auto pos_table = [&](const char *name, const AxisResult &g, const Sub &t) {
    std::printf("\n--- %s: per-bar positional dependence ---\n", name);
    std::printf("  %-9s %-9s %-24s %s\n", "pos [ns]", "pos [mm]",
                "measured sigma", "physics sigma");
    for (const auto &b : g.bars) {
      Sub p = quad_subtract(b.sigma, b.sigma_err, t.val, t.err);
      std::printf("  %7.1f   %7.2f   %4.2f +- %4.2f (%.3f mm)   ", b.center,
                  b.center * MM_PER_NS, b.sigma, b.sigma_err,
                  b.sigma * MM_PER_NS);
      if (p.below)
        std::printf("consistent with 0\n");
      else
        std::printf("%4.2f +- %4.2f (%.3f mm)\n", p.val, p.err,
                    p.val * MM_PER_NS);
    }
  };
  pos_table("X", gx, tx);
  pos_table("Y", gy, ty);

  // --- Aperture edges: the outermost sample points (~+-209 ns = +-39 mm) ---
  // x = tXR - tXL < 0 (left edge) means tXL is LATE: the XL signal travelled
  // the full line, so the left-edge sigma probes XL's far-end degradation
  // (attenuation/dispersion); the near arm (XR) contributes almost nothing
  // there. Right edge likewise probes XR's far end. Same logic for Y.
  double medX = plateau_median(hX), medY = plateau_median(hY);
  struct EdgeRow {
    const char *what;
    EdgeFit e;
  };
  EdgeRow erows[4] = {
      {"X left  (probes XL far end)", fit_edge(hX, -250, -170, -1, medX)},
      {"X right (probes XR far end)", fit_edge(hX, 170, 250, +1, medX)},
      {"Y bottom(probes YD far end)", fit_edge(hY, -250, -170, -1, medY)},
      {"Y top   (probes YU far end)", fit_edge(hY, 170, 250, +1, medY)}};
  std::printf(
      "\n--- aperture edges (upper limits: penumbra/illumination add in) ---\n");
  std::printf("  %-28s %-9s %-24s %s\n", "", "pos [mm]", "measured sigma",
              "physics sigma");
  for (auto &er : erows) {
    if (!er.e.ok) {
      std::printf("  %-28s -- fit rejected --\n", er.what);
      continue;
    }
    const Sub &t = (er.what[0] == 'X') ? tx : ty;
    Sub p = quad_subtract(er.e.sigma, er.e.sigma_err, t.val, t.err);
    std::printf("  %-28s %7.2f   %4.2f +- %4.2f (%.3f mm)   ", er.what,
                er.e.pos * MM_PER_NS, er.e.sigma, er.e.sigma_err,
                er.e.sigma * MM_PER_NS);
    if (p.below)
      std::printf("consistent with 0\n");
    else
      std::printf("%4.2f +- %4.2f (%.3f mm)\n", p.val, p.err,
                  p.val * MM_PER_NS);
  }

  // sigma-vs-position graphs for the bottom pad (and the output file)
  auto make_graph = [](const AxisResult &g, const char *gname) {
    auto *gr = new TGraphErrors();
    gr->SetName(gname);
    for (const auto &b : g.bars) {
      int i = gr->GetN();
      gr->SetPoint(i, b.center, b.sigma);
      gr->SetPointError(i, 0.0, b.sigma_err);
    }
    return gr;
  };
  auto *grX = make_graph(gx, "gResX");
  auto *grY = make_graph(gy, "gResY");
  grX->SetMarkerStyle(kFullCircle);
  grX->SetMarkerColor(kRed + 1);
  grX->SetLineColor(kRed + 1);
  grY->SetMarkerStyle(kFullSquare);
  grY->SetMarkerColor(kBlue + 1);
  grY->SetLineColor(kBlue + 1);

  // aperture-edge points: open markers (upper limits, extra systematics)
  auto *geX = new TGraphErrors();
  geX->SetName("gEdgeX");
  auto *geY = new TGraphErrors();
  geY->SetName("gEdgeY");
  for (auto &er : erows) {
    if (!er.e.ok) continue;
    auto *g = (er.what[0] == 'X') ? geX : geY;
    int i = g->GetN();
    g->SetPoint(i, er.e.pos, er.e.sigma);
    g->SetPointError(i, 0.0, er.e.sigma_err);
  }
  geX->SetMarkerStyle(kOpenCircle);
  geX->SetMarkerColor(kRed + 1);
  geX->SetLineColor(kRed + 1);
  geY->SetMarkerStyle(kOpenSquare);
  geY->SetMarkerColor(kBlue + 1);
  geY->SetLineColor(kBlue + 1);

  auto c1 = new TCanvas("c1", "grid resolution", 1400, 1350);
  c1->Divide(1, 3);
  c1->cd(1);
  hX->SetMinimum(0);
  hX->Draw("hist");
  for (auto *obj : *hX->GetListOfFunctions())
    if (auto *fn = dynamic_cast<TF1 *>(obj)) {
      fn->SetLineColor(kRed);
      fn->Draw("same");
    }
  c1->cd(2);
  hY->SetMinimum(0);
  hY->Draw("hist");
  for (auto *obj : *hY->GetListOfFunctions())
    if (auto *fn = dynamic_cast<TF1 *>(obj)) {
      fn->SetLineColor(kRed);
      fn->Draw("same");
    }
  c1->cd(3);
  gPad->DrawFrame(-250, 0, 250, 3,
                  "measured #sigma vs position;position [ns];#sigma [ns]");
  auto *tl = new TLine(-250, tx.val, 250, tx.val);
  tl->SetLineStyle(kDashed);
  tl->SetLineColor(kGray + 2);
  tl->Draw();
  if (grX->GetN()) grX->Draw("P same");
  if (grY->GetN()) grY->Draw("P same");
  if (geX->GetN()) geX->Draw("P same");
  if (geY->GetN()) geY->Draw("P same");
  auto *leg = new TLegend(0.70, 0.60, 0.89, 0.89);
  leg->AddEntry(grX, "X bars", "pe");
  leg->AddEntry(grY, "Y bars", "pe");
  leg->AddEntry(geX, "X aperture edge", "pe");
  leg->AddEntry(geY, "Y aperture edge", "pe");
  leg->AddEntry(tl, "timing floor", "l");
  leg->Draw();
  c1->SaveAs("grid_resolution.png");

  // Write the results to a ROOT file for later use
  TFile *out = TFile::Open("grid_resolution.root", "RECREATE");
  out->cd();
  hX->Write();
  hY->Write();
  hAB->Write();
  hA2->Write();
  hB2->Write();
  grX->Write();
  grY->Write();
  geX->Write();
  geY->Write();
  out->Write();
  out->Close();
}
