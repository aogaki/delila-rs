// Read flat binary from "delila-recover dump" into a ROOT TTree
// TTree format matches legacy DELILA Recorder (DELILA_Tree)
//
// Usage:
//   root -l 'read_dump.C("events.bin")'
//   root -l 'read_dump.C("events.bin", "output.root")'
//
// Dump format (Little-Endian, 22 bytes/event):
//   Header:  "DLDUMP01" (8 bytes) + n_events (u64, 8 bytes)
//   Event:   module(u8) channel(u8) energy(u16) energy_short(u16) flags(u64) timestamp_ns(f64)
//
// Branch mapping (legacy compatible):
//   Mod/b  Ch/b  TimeStamp/l  FineTS/D  ChargeLong/s  ChargeShort/s  RecordLength/i

#include <TFile.h>
#include <TTree.h>
#include <iostream>
#include <fstream>
#include <cstring>
#include <cstdint>

void read_dump(const char* input, const char* output = "") {
    std::ifstream f(input, std::ios::binary);
    if (!f.is_open()) {
        std::cerr << "Error: Cannot open " << input << std::endl;
        return;
    }

    // Read header
    char magic[8];
    f.read(magic, 8);
    if (std::memcmp(magic, "DLDUMP01", 8) != 0) {
        std::cerr << "Error: Invalid magic (expected DLDUMP01)" << std::endl;
        return;
    }

    uint64_t n_events;
    f.read(reinterpret_cast<char*>(&n_events), 8);
    std::cout << "Events in file: " << n_events << std::endl;

    // Output ROOT file
    TString out_name = (strlen(output) > 0) ? output : TString(input).ReplaceAll(".bin", ".root");
    TFile* outFile = new TFile(out_name, "RECREATE");

    // TTree with legacy-compatible branch names
    TTree* tree = new TTree("DELILA_Tree", "DELILA data");

    UChar_t   Mod;
    UChar_t   Ch;
    ULong64_t TimeStamp;
    Double_t  FineTS;
    UShort_t  ChargeLong;
    UShort_t  ChargeShort;
    UInt_t    RecordLength = 0;

    tree->Branch("Mod",          &Mod,          "Mod/b");
    tree->Branch("Ch",           &Ch,           "Ch/b");
    tree->Branch("TimeStamp",    &TimeStamp,    "TimeStamp/l");
    tree->Branch("FineTS",       &FineTS,       "FineTS/D");
    tree->Branch("ChargeLong",   &ChargeLong,   "ChargeLong/s");
    tree->Branch("ChargeShort",  &ChargeShort,  "ChargeShort/s");
    tree->Branch("RecordLength", &RecordLength,  "RecordLength/i");

    // Read events (22 bytes each)
    uint64_t count = 0;
    uint8_t buf[22];
    uint64_t flags_tmp;
    double   ts_tmp;

    while (f.read(reinterpret_cast<char*>(buf), 22)) {
        Mod = buf[0];
        Ch  = buf[1];
        std::memcpy(&ChargeLong,  &buf[2],  2);
        std::memcpy(&ChargeShort, &buf[4],  2);
        std::memcpy(&flags_tmp,   &buf[6],  8);
        std::memcpy(&ts_tmp,      &buf[14], 8);

        FineTS    = ts_tmp;
        TimeStamp = static_cast<ULong64_t>(ts_tmp);
        RecordLength = 0;

        tree->Fill();
        count++;
    }

    tree->Write();
    outFile->Close();

    std::cout << "Converted " << count << " events -> " << out_name << std::endl;
    std::cout << "  tree->Draw(\"ChargeLong\");" << std::endl;
    std::cout << "  tree->Draw(\"ChargeLong:Ch\",\"\",\"colz\");" << std::endl;
}
