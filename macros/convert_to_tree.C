// DELILA File to ROOT TTree Converter
// Convert .delila format files to ROOT TTree format
//
// Usage:
//   root -l 'convert_to_tree.C("data/run0010_0000_data.delila")'
//   root -l 'convert_to_tree.C("data/run0010_0000_data.delila", "output.root")'
//   root -l 'convert_to_tree.C("data/run0010_0000_data.delila", "", 10000)'  // First 10000 events
//
// Output: Creates a ROOT file with TTree "events" containing all event data
//
// File format (v2):
//   Header: "DELILA02" + u32_le(len) + msgpack(metadata)
//   Data blocks: [u32_le(len) + msgpack(batch)]...
//   Footer: "DLEND002" + 56 bytes metadata (64 bytes total)

#include <TFile.h>
#include <TTree.h>
#include <TString.h>
#include <iostream>
#include <fstream>
#include <vector>
#include <cstring>
#include <cstdint>

// File format constants
const char* FILE_MAGIC = "DELILA02";
const char* FOOTER_MAGIC = "DLEND002";
const size_t FOOTER_SIZE = 64;

// Maximum waveform samples (for fixed-size arrays in TTree)
const int MAX_WAVEFORM_SAMPLES = 16384;

// Read little-endian uint32
uint32_t read_u32_le(std::ifstream& f) {
    uint8_t buf[4];
    f.read(reinterpret_cast<char*>(buf), 4);
    return buf[0] | (buf[1] << 8) | (buf[2] << 16) | (buf[3] << 24);
}

// Read little-endian uint64
uint64_t read_u64_le(std::ifstream& f) {
    uint8_t buf[8];
    f.read(reinterpret_cast<char*>(buf), 8);
    return static_cast<uint64_t>(buf[0]) |
           (static_cast<uint64_t>(buf[1]) << 8) |
           (static_cast<uint64_t>(buf[2]) << 16) |
           (static_cast<uint64_t>(buf[3]) << 24) |
           (static_cast<uint64_t>(buf[4]) << 32) |
           (static_cast<uint64_t>(buf[5]) << 40) |
           (static_cast<uint64_t>(buf[6]) << 48) |
           (static_cast<uint64_t>(buf[7]) << 56);
}

// Read little-endian double
double read_f64_le(std::ifstream& f) {
    uint64_t bits = read_u64_le(f);
    double result;
    std::memcpy(&result, &bits, sizeof(double));
    return result;
}

// Simple MessagePack parser for EventDataBatch
class MsgPackParser {
public:
    MsgPackParser(const std::vector<uint8_t>& data) : data_(data), pos_(0) {}

    // Parse a single event into the output variables
    // Returns false when no more events
    bool parse_batch_header(uint32_t& source_id, size_t& num_events) {
        // Batch is array of 4 elements
        size_t batch_size;
        if (!read_array_header(batch_size) || batch_size != 4) {
            return false;
        }

        // source_id (u32)
        uint64_t src;
        if (!read_uint(src)) return false;
        source_id = static_cast<uint32_t>(src);

        // sequence_number (u64) - skip
        uint64_t seq;
        if (!read_uint(seq)) return false;

        // timestamp (u64) - skip
        uint64_t ts;
        if (!read_uint(ts)) return false;

        // events array header
        if (!read_array_header(num_events)) return false;

        return true;
    }

    bool parse_event(uint8_t& module, uint8_t& channel, uint16_t& energy, uint16_t& energy_short,
                     double& timestamp_ns, uint64_t& flags, bool& has_waveform,
                     int& n_analog1, int16_t* analog1, int& n_analog2, int16_t* analog2,
                     int& n_digital1, uint8_t* digital1, int& n_digital2, uint8_t* digital2,
                     int& n_digital3, uint8_t* digital3, int& n_digital4, uint8_t* digital4,
                     uint8_t& time_resolution, uint16_t& trigger_threshold) {
        // Event is array of 6 or 7 elements (7 if has waveform)
        size_t ev_size;
        if (!read_array_header(ev_size)) {
            return false;
        }
        if (ev_size != 6 && ev_size != 7) {
            std::cerr << "Warning: Unexpected event array size: " << ev_size << std::endl;
            return false;
        }

        uint64_t tmp;

        // module (u8)
        if (!read_uint(tmp)) return false;
        module = static_cast<uint8_t>(tmp);

        // channel (u8)
        if (!read_uint(tmp)) return false;
        channel = static_cast<uint8_t>(tmp);

        // energy (u16)
        if (!read_uint(tmp)) return false;
        energy = static_cast<uint16_t>(tmp);

        // energy_short (u16)
        if (!read_uint(tmp)) return false;
        energy_short = static_cast<uint16_t>(tmp);

        // timestamp_ns (f64)
        if (!read_float64(timestamp_ns)) return false;

        // flags (u64)
        if (!read_uint(flags)) return false;

        // waveform (optional)
        has_waveform = (ev_size == 7);
        n_analog1 = 0;
        n_analog2 = 0;
        n_digital1 = 0;
        n_digital2 = 0;
        n_digital3 = 0;
        n_digital4 = 0;
        time_resolution = 0;
        trigger_threshold = 0;

        if (has_waveform) {
            if (!parse_waveform(n_analog1, analog1, n_analog2, analog2,
                               n_digital1, digital1, n_digital2, digital2,
                               n_digital3, digital3, n_digital4, digital4,
                               time_resolution, trigger_threshold)) {
                return false;
            }
        }

        return true;
    }

private:
    bool parse_waveform(int& n_analog1, int16_t* analog1, int& n_analog2, int16_t* analog2,
                       int& n_digital1, uint8_t* digital1, int& n_digital2, uint8_t* digital2,
                       int& n_digital3, uint8_t* digital3, int& n_digital4, uint8_t* digital4,
                       uint8_t& time_resolution, uint16_t& trigger_threshold) {
        // Waveform is array of 8 elements
        size_t wf_size;
        if (!read_array_header(wf_size) || wf_size != 8) {
            std::cerr << "Warning: Unexpected waveform array size: " << wf_size << std::endl;
            return false;
        }

        // analog_probe1 (Vec<i16>)
        if (!read_i16_array(n_analog1, analog1, MAX_WAVEFORM_SAMPLES)) return false;

        // analog_probe2 (Vec<i16>)
        if (!read_i16_array(n_analog2, analog2, MAX_WAVEFORM_SAMPLES)) return false;

        // digital_probe1 (Vec<u8>)
        if (!read_u8_array(n_digital1, digital1, MAX_WAVEFORM_SAMPLES)) return false;

        // digital_probe2 (Vec<u8>)
        if (!read_u8_array(n_digital2, digital2, MAX_WAVEFORM_SAMPLES)) return false;

        // digital_probe3 (Vec<u8>)
        if (!read_u8_array(n_digital3, digital3, MAX_WAVEFORM_SAMPLES)) return false;

        // digital_probe4 (Vec<u8>)
        if (!read_u8_array(n_digital4, digital4, MAX_WAVEFORM_SAMPLES)) return false;

        // time_resolution (u8)
        uint64_t tmp;
        if (!read_uint(tmp)) return false;
        time_resolution = static_cast<uint8_t>(tmp);

        // trigger_threshold (u16)
        if (!read_uint(tmp)) return false;
        trigger_threshold = static_cast<uint16_t>(tmp);

        return true;
    }

    bool read_i16_array(int& n, int16_t* arr, int max_size) {
        size_t size;
        if (!read_array_header(size)) return false;

        n = std::min(static_cast<int>(size), max_size);
        for (size_t i = 0; i < size; i++) {
            int64_t val;
            if (!read_int(val)) return false;
            if (static_cast<int>(i) < max_size) {
                arr[i] = static_cast<int16_t>(val);
            }
        }
        return true;
    }

    bool read_u8_array(int& n, uint8_t* arr, int max_size) {
        // Can be either an array or binary data (bin8/bin16/bin32)
        if (pos_ >= data_.size()) return false;
        uint8_t b = data_[pos_];

        // Check for binary format first
        if (b == 0xc4 || b == 0xc5 || b == 0xc6) {
            return read_bin(n, arr, max_size);
        }

        // Otherwise, it's an array
        size_t size;
        if (!read_array_header(size)) return false;

        n = std::min(static_cast<int>(size), max_size);
        for (size_t i = 0; i < size; i++) {
            uint64_t val;
            if (!read_uint(val)) return false;
            if (static_cast<int>(i) < max_size) {
                arr[i] = static_cast<uint8_t>(val);
            }
        }
        return true;
    }

    bool read_bin(int& n, uint8_t* arr, int max_size) {
        if (pos_ >= data_.size()) return false;
        uint8_t b = data_[pos_++];
        size_t size = 0;

        if (b == 0xc4 && pos_ + 1 <= data_.size()) {
            // bin8
            size = data_[pos_++];
        } else if (b == 0xc5 && pos_ + 2 <= data_.size()) {
            // bin16
            size = (data_[pos_] << 8) | data_[pos_ + 1];
            pos_ += 2;
        } else if (b == 0xc6 && pos_ + 4 <= data_.size()) {
            // bin32
            size = (static_cast<uint32_t>(data_[pos_]) << 24) |
                   (static_cast<uint32_t>(data_[pos_ + 1]) << 16) |
                   (static_cast<uint32_t>(data_[pos_ + 2]) << 8) |
                   static_cast<uint32_t>(data_[pos_ + 3]);
            pos_ += 4;
        } else {
            return false;
        }

        n = std::min(static_cast<int>(size), max_size);
        if (pos_ + size > data_.size()) return false;
        std::memcpy(arr, &data_[pos_], n);
        pos_ += size;
        return true;
    }

    bool read_array_header(size_t& size) {
        if (pos_ >= data_.size()) return false;
        uint8_t b = data_[pos_++];

        // fixarray (0x90 - 0x9f)
        if ((b & 0xf0) == 0x90) {
            size = b & 0x0f;
            return true;
        }
        // array16 (0xdc)
        if (b == 0xdc && pos_ + 2 <= data_.size()) {
            size = (data_[pos_] << 8) | data_[pos_ + 1];
            pos_ += 2;
            return true;
        }
        // array32 (0xdd)
        if (b == 0xdd && pos_ + 4 <= data_.size()) {
            size = (static_cast<uint32_t>(data_[pos_]) << 24) |
                   (static_cast<uint32_t>(data_[pos_ + 1]) << 16) |
                   (static_cast<uint32_t>(data_[pos_ + 2]) << 8) |
                   static_cast<uint32_t>(data_[pos_ + 3]);
            pos_ += 4;
            return true;
        }
        return false;
    }

    bool read_uint(uint64_t& val) {
        if (pos_ >= data_.size()) return false;
        uint8_t b = data_[pos_++];

        // positive fixint (0x00 - 0x7f)
        if (b <= 0x7f) {
            val = b;
            return true;
        }
        // uint8 (0xcc)
        if (b == 0xcc && pos_ + 1 <= data_.size()) {
            val = data_[pos_++];
            return true;
        }
        // uint16 (0xcd)
        if (b == 0xcd && pos_ + 2 <= data_.size()) {
            val = (data_[pos_] << 8) | data_[pos_ + 1];
            pos_ += 2;
            return true;
        }
        // uint32 (0xce)
        if (b == 0xce && pos_ + 4 <= data_.size()) {
            val = (static_cast<uint32_t>(data_[pos_]) << 24) |
                  (static_cast<uint32_t>(data_[pos_ + 1]) << 16) |
                  (static_cast<uint32_t>(data_[pos_ + 2]) << 8) |
                  static_cast<uint32_t>(data_[pos_ + 3]);
            pos_ += 4;
            return true;
        }
        // uint64 (0xcf)
        if (b == 0xcf && pos_ + 8 <= data_.size()) {
            val = (static_cast<uint64_t>(data_[pos_]) << 56) |
                  (static_cast<uint64_t>(data_[pos_ + 1]) << 48) |
                  (static_cast<uint64_t>(data_[pos_ + 2]) << 40) |
                  (static_cast<uint64_t>(data_[pos_ + 3]) << 32) |
                  (static_cast<uint64_t>(data_[pos_ + 4]) << 24) |
                  (static_cast<uint64_t>(data_[pos_ + 5]) << 16) |
                  (static_cast<uint64_t>(data_[pos_ + 6]) << 8) |
                  static_cast<uint64_t>(data_[pos_ + 7]);
            pos_ += 8;
            return true;
        }
        return false;
    }

    bool read_int(int64_t& val) {
        if (pos_ >= data_.size()) return false;
        uint8_t b = data_[pos_];

        // positive fixint (0x00 - 0x7f)
        if (b <= 0x7f) {
            pos_++;
            val = b;
            return true;
        }
        // negative fixint (0xe0 - 0xff)
        if (b >= 0xe0) {
            pos_++;
            val = static_cast<int8_t>(b);
            return true;
        }
        // int8 (0xd0)
        if (b == 0xd0 && pos_ + 2 <= data_.size()) {
            pos_++;
            val = static_cast<int8_t>(data_[pos_++]);
            return true;
        }
        // int16 (0xd1)
        if (b == 0xd1 && pos_ + 3 <= data_.size()) {
            pos_++;
            val = static_cast<int16_t>((data_[pos_] << 8) | data_[pos_ + 1]);
            pos_ += 2;
            return true;
        }
        // int32 (0xd2)
        if (b == 0xd2 && pos_ + 5 <= data_.size()) {
            pos_++;
            val = static_cast<int32_t>(
                (static_cast<uint32_t>(data_[pos_]) << 24) |
                (static_cast<uint32_t>(data_[pos_ + 1]) << 16) |
                (static_cast<uint32_t>(data_[pos_ + 2]) << 8) |
                static_cast<uint32_t>(data_[pos_ + 3]));
            pos_ += 4;
            return true;
        }
        // uint types (for positive values stored as unsigned)
        uint64_t uval;
        if (read_uint(uval)) {
            val = static_cast<int64_t>(uval);
            return true;
        }
        return false;
    }

    bool read_float64(double& val) {
        if (pos_ >= data_.size()) return false;
        uint8_t b = data_[pos_++];

        // float64 (0xcb)
        if (b == 0xcb && pos_ + 8 <= data_.size()) {
            // Big-endian IEEE 754
            uint64_t bits = (static_cast<uint64_t>(data_[pos_]) << 56) |
                           (static_cast<uint64_t>(data_[pos_ + 1]) << 48) |
                           (static_cast<uint64_t>(data_[pos_ + 2]) << 40) |
                           (static_cast<uint64_t>(data_[pos_ + 3]) << 32) |
                           (static_cast<uint64_t>(data_[pos_ + 4]) << 24) |
                           (static_cast<uint64_t>(data_[pos_ + 5]) << 16) |
                           (static_cast<uint64_t>(data_[pos_ + 6]) << 8) |
                           static_cast<uint64_t>(data_[pos_ + 7]);
            pos_ += 8;
            std::memcpy(&val, &bits, sizeof(double));
            return true;
        }
        return false;
    }

    const std::vector<uint8_t>& data_;
    size_t pos_;
};

// Read file header and return data start position
bool read_header(std::ifstream& f, size_t& header_end_pos) {
    // Check magic
    char magic[8];
    f.read(magic, 8);
    if (std::memcmp(magic, FILE_MAGIC, 8) != 0) {
        std::cerr << "Error: Invalid file magic. Expected DELILA02" << std::endl;
        return false;
    }

    // Read header length
    uint32_t header_len = read_u32_le(f);

    // Skip header content (MessagePack metadata)
    f.seekg(header_len, std::ios::cur);

    header_end_pos = f.tellg();
    return true;
}

// Main function
void convert_to_tree(const char* input_file, const char* output_file = "", int max_events = -1) {
    std::cout << "Converting DELILA file to ROOT TTree: " << input_file << std::endl;

    // Open input file
    std::ifstream f(input_file, std::ios::binary);
    if (!f.is_open()) {
        std::cerr << "Error: Cannot open input file" << std::endl;
        return;
    }

    // Get file size
    f.seekg(0, std::ios::end);
    size_t file_size = f.tellg();
    f.seekg(0, std::ios::beg);

    // Read header
    size_t header_end_pos;
    if (!read_header(f, header_end_pos)) {
        return;
    }

    // Generate output filename if not specified
    TString out_name;
    if (strlen(output_file) == 0) {
        out_name = input_file;
        out_name.ReplaceAll(".delila", ".root");
    } else {
        out_name = output_file;
    }

    std::cout << "Output file: " << out_name << std::endl;

    // Create output ROOT file
    TFile* outFile = new TFile(out_name, "RECREATE");
    if (!outFile->IsOpen()) {
        std::cerr << "Error: Cannot create output file" << std::endl;
        return;
    }

    // Create TTree
    TTree* tree = new TTree("events", "DELILA Event Data");

    // Branch variables
    UChar_t b_module;
    UChar_t b_channel;
    UShort_t b_energy;
    UShort_t b_energy_short;
    Double_t b_timestamp_ns;
    ULong64_t b_flags;
    Bool_t b_has_waveform;

    // Waveform branches
    Int_t b_n_analog1;
    Int_t b_n_analog2;
    Int_t b_n_digital1;
    Int_t b_n_digital2;
    Int_t b_n_digital3;
    Int_t b_n_digital4;
    Short_t* b_analog1 = new Short_t[MAX_WAVEFORM_SAMPLES];
    Short_t* b_analog2 = new Short_t[MAX_WAVEFORM_SAMPLES];
    UChar_t* b_digital1 = new UChar_t[MAX_WAVEFORM_SAMPLES];
    UChar_t* b_digital2 = new UChar_t[MAX_WAVEFORM_SAMPLES];
    UChar_t* b_digital3 = new UChar_t[MAX_WAVEFORM_SAMPLES];
    UChar_t* b_digital4 = new UChar_t[MAX_WAVEFORM_SAMPLES];
    UChar_t b_time_resolution;
    UShort_t b_trigger_threshold;

    // Create branches
    tree->Branch("module", &b_module, "module/b");
    tree->Branch("channel", &b_channel, "channel/b");
    tree->Branch("energy", &b_energy, "energy/s");
    tree->Branch("energy_short", &b_energy_short, "energy_short/s");
    tree->Branch("timestamp_ns", &b_timestamp_ns, "timestamp_ns/D");
    tree->Branch("flags", &b_flags, "flags/l");
    tree->Branch("has_waveform", &b_has_waveform, "has_waveform/O");

    // Waveform branches (variable-length arrays)
    tree->Branch("n_analog1", &b_n_analog1, "n_analog1/I");
    tree->Branch("n_analog2", &b_n_analog2, "n_analog2/I");
    tree->Branch("analog1", b_analog1, "analog1[n_analog1]/S");
    tree->Branch("analog2", b_analog2, "analog2[n_analog2]/S");
    tree->Branch("n_digital1", &b_n_digital1, "n_digital1/I");
    tree->Branch("n_digital2", &b_n_digital2, "n_digital2/I");
    tree->Branch("n_digital3", &b_n_digital3, "n_digital3/I");
    tree->Branch("n_digital4", &b_n_digital4, "n_digital4/I");
    tree->Branch("digital1", b_digital1, "digital1[n_digital1]/b");
    tree->Branch("digital2", b_digital2, "digital2[n_digital2]/b");
    tree->Branch("digital3", b_digital3, "digital3[n_digital3]/b");
    tree->Branch("digital4", b_digital4, "digital4[n_digital4]/b");
    tree->Branch("time_resolution", &b_time_resolution, "time_resolution/b");
    tree->Branch("trigger_threshold", &b_trigger_threshold, "trigger_threshold/s");

    // Calculate data region
    size_t data_end = file_size - FOOTER_SIZE;

    // Read data blocks
    f.seekg(header_end_pos, std::ios::beg);

    int block_count = 0;
    Long64_t event_count = 0;
    int waveform_count = 0;

    std::cout << "Processing..." << std::flush;

    while (f.tellg() < static_cast<std::streampos>(data_end)) {
        // Read block length
        uint32_t block_len = read_u32_le(f);
        if (block_len == 0 || block_len > 100000000) {
            std::cerr << "\nWarning: Invalid block length " << block_len << std::endl;
            break;
        }

        // Read block data
        std::vector<uint8_t> block_data(block_len);
        f.read(reinterpret_cast<char*>(block_data.data()), block_len);

        if (!f.good()) {
            std::cerr << "\nWarning: Read error at block " << block_count << std::endl;
            break;
        }

        // Parse MessagePack
        MsgPackParser parser(block_data);
        uint32_t source_id;
        size_t num_events;

        if (!parser.parse_batch_header(source_id, num_events)) {
            std::cerr << "\nWarning: Failed to parse block " << block_count << std::endl;
            break;
        }

        // Process events
        for (size_t i = 0; i < num_events; i++) {
            if (!parser.parse_event(b_module, b_channel, b_energy, b_energy_short,
                                   b_timestamp_ns, b_flags, b_has_waveform,
                                   b_n_analog1, b_analog1, b_n_analog2, b_analog2,
                                   b_n_digital1, b_digital1, b_n_digital2, b_digital2,
                                   b_n_digital3, b_digital3, b_n_digital4, b_digital4,
                                   b_time_resolution, b_trigger_threshold)) {
                std::cerr << "\nWarning: Failed to parse event " << event_count << std::endl;
                break;
            }

            tree->Fill();
            event_count++;
            if (b_has_waveform) waveform_count++;

            if (max_events > 0 && event_count >= max_events) {
                break;
            }
        }

        block_count++;

        // Progress indicator
        if (block_count % 100 == 0) {
            std::cout << "." << std::flush;
        }

        if (max_events > 0 && event_count >= max_events) {
            break;
        }
    }

    std::cout << " done!" << std::endl;

    // Write and close
    tree->Write();
    outFile->Close();

    // Cleanup
    delete[] b_analog1;
    delete[] b_analog2;
    delete[] b_digital1;
    delete[] b_digital2;
    delete[] b_digital3;
    delete[] b_digital4;

    std::cout << "\n=== Conversion Summary ===" << std::endl;
    std::cout << "Blocks processed:      " << block_count << std::endl;
    std::cout << "Events converted:      " << event_count << std::endl;
    std::cout << "Events with waveform:  " << waveform_count << std::endl;
    std::cout << "Output file:           " << out_name << std::endl;

    std::cout << "\nTo use the TTree:" << std::endl;
    std::cout << "  TFile* f = TFile::Open(\"" << out_name << "\");" << std::endl;
    std::cout << "  TTree* t = (TTree*)f->Get(\"events\");" << std::endl;
    std::cout << "  t->Draw(\"energy\");                    // Energy histogram" << std::endl;
    std::cout << "  t->Draw(\"energy:channel\", \"\", \"colz\"); // 2D: Energy vs Channel" << std::endl;
    std::cout << "  t->Draw(\"analog1\", \"Entry$==0\");       // First waveform" << std::endl;
}
