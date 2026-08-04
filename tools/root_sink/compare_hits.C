// compare_hits.C — order-INDEPENDENT equality check between two root_sink
// hits trees (V2/V3 gates of the TODO 66 thread restructure).
//
// The multithreaded sink writes the same hit SET as the old one but globally
// time-sorted, so entry-by-entry diffing is useless. Instead compare, per
// channel: entry count, Σenergy, Σenergy_short (exact integer sums), and the
// XOR of every timestamp's 64-bit pattern (exact, order-independent — unlike
// a floating sum). Also verifies the second file is monotonically time-sorted.
//
//   root -l -b -q 'compare_hits.C("baseline.root","candidate.root")'
#include <cstdint>
#include <cstring>
#include <cstdio>

#include "TFile.h"
#include "TTree.h"

void compare_hits(const char* a_path, const char* b_path,
                  const char* tree_name = "delila") {
  struct Stat {
    Long64_t n[256] = {};
    ULong64_t esum[256] = {};
    ULong64_t essum[256] = {};
    ULong64_t tsxor[256] = {};
    Long64_t total = 0;
    bool monotone = true;
  };
  auto scan = [&](const char* path, Stat& s) -> bool {
    TFile* f = TFile::Open(path, "READ");
    if (!f || f->IsZombie()) {
      printf("compare_hits: cannot open %s\n", path);
      return false;
    }
    TTree* t = dynamic_cast<TTree*>(f->Get(tree_name));
    if (!t) {
      printf("compare_hits: no tree '%s' in %s\n", tree_name, path);
      return false;
    }
    UChar_t mod = 0, ch = 0;
    UShort_t e = 0, es = 0;
    Double_t ts = 0;
    t->SetBranchAddress("module", &mod);
    t->SetBranchAddress("channel", &ch);
    t->SetBranchAddress("energy", &e);
    t->SetBranchAddress("energy_short", &es);
    t->SetBranchAddress("timestamp_ns", &ts);
    Double_t prev = -1e300;
    const Long64_t n = t->GetEntries();
    for (Long64_t i = 0; i < n; ++i) {
      t->GetEntry(i);
      s.n[ch]++;
      s.esum[ch] += e;
      s.essum[ch] += es;
      ULong64_t bits;
      std::memcpy(&bits, &ts, 8);
      s.tsxor[ch] ^= bits;
      if (ts < prev) s.monotone = false;
      prev = ts;
    }
    s.total = n;
    f->Close();
    return true;
  };

  Stat A, B;
  if (!scan(a_path, A) || !scan(b_path, B)) return;

  bool same = A.total == B.total;
  for (int c = 0; c < 256; ++c) {
    if (A.n[c] != B.n[c] || A.esum[c] != B.esum[c] || A.essum[c] != B.essum[c] ||
        A.tsxor[c] != B.tsxor[c]) {
      printf("compare_hits: MISMATCH ch%d  n %lld/%lld  esum %llu/%llu  tsxor %llx/%llx\n",
             c, A.n[c], B.n[c], A.esum[c], B.esum[c], A.tsxor[c], B.tsxor[c]);
      same = false;
    }
  }
  printf("compare_hits: A=%lld entries, B=%lld entries\n", A.total, B.total);
  printf("compare_hits: B monotone in timestamp_ns: %s\n", B.monotone ? "YES" : "NO");
  printf("compare_hits: content %s\n", same ? "IDENTICAL" : "DIFFERS");
}
