// DELILA File Reader - ROOT Macro
// Read .delila format files written by delila-rs recorder
//
// Usage:
//   root -l 'read_delila.C("data/run0010_0000_data.delila")'
//   root -l 'read_delila.C("data/run0010_0000_data.delila", 100)'  // First 100 events
//
// File format (v2):
//   Header: "DELILA02" + u32_le(len) + msgpack(metadata)
//   Data blocks: [u32_le(len) + msgpack(batch)]...
//   Footer: "DLEND002" + 56 bytes metadata (64 bytes total)

#include <TFile.h>
#include <TTree.h>
#include <TH1F.h>
#include <TCanvas.h>
#include <iostream>
#include <fstream>
#include <vector>
#include <cstring>
#include <cstdint>

// File format constants
const char* FILE_MAGIC = "DELILA02";
const char* FOOTER_MAGIC = "DLEND002";
const size_t FOOTER_SIZE = 64;

// Event data structure (matches Rust MinimalEventData)
struct Event {
    uint8_t module;
    uint8_t channel;
    uint16_t energy;
    uint16_t energy_short;
    double timestamp_ns;
    uint64_t flags;
};

// Footer structure
struct Footer {
    char magic[8];
    uint64_t data_checksum;
    uint64_t total_events;
    uint64_t data_bytes;
    double first_event_time_ns;
    double last_event_time_ns;
    uint64_t file_end_time_ns;
    uint8_t write_complete;
    uint8_t reserved[7];
};

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

// Simple MessagePack parser for MinimalEventDataBatch
// MessagePack format:
//   - Array of 4 elements: [source_id, sequence_number, timestamp, events]
//   - events is an array of arrays: [[module, channel, energy, energy_short, timestamp_ns, flags], ...]
class MsgPackParser {
public:
    MsgPackParser(const std::vector<uint8_t>& data) : data_(data), pos_(0) {}

    bool parse_batch(uint32_t& source_id, std::vector<Event>& events) {
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

        // events array
        size_t num_events;
        if (!read_array_header(num_events)) return false;

        events.reserve(num_events);
        for (size_t i = 0; i < num_events; i++) {
            Event ev;
            if (!parse_event(ev)) return false;
            events.push_back(ev);
        }

        return true;
    }

private:
    bool parse_event(Event& ev) {
        // Event is array of 6 elements
        size_t ev_size;
        if (!read_array_header(ev_size) || ev_size != 6) {
            return false;
        }

        uint64_t tmp;

        // module (u8)
        if (!read_uint(tmp)) return false;
        ev.module = static_cast<uint8_t>(tmp);

        // channel (u8)
        if (!read_uint(tmp)) return false;
        ev.channel = static_cast<uint8_t>(tmp);

        // energy (u16)
        if (!read_uint(tmp)) return false;
        ev.energy = static_cast<uint16_t>(tmp);

        // energy_short (u16)
        if (!read_uint(tmp)) return false;
        ev.energy_short = static_cast<uint16_t>(tmp);

        // timestamp_ns (f64)
        if (!read_float64(ev.timestamp_ns)) return false;

        // flags (u64)
        if (!read_uint(ev.flags)) return false;

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

// Read and print file header info
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
    std::cout << "Header length: " << header_len << " bytes" << std::endl;

    // Skip header content (MessagePack metadata)
    // For now, we just skip it. Could parse for run_number, exp_name, etc.
    f.seekg(header_len, std::ios::cur);

    header_end_pos = f.tellg();
    std::cout << "Data starts at offset: " << header_end_pos << std::endl;

    return true;
}

// Read and print footer info
bool read_footer(std::ifstream& f, size_t file_size) {
    if (file_size < FOOTER_SIZE) {
        std::cerr << "Warning: File too small for footer" << std::endl;
        return false;
    }

    f.seekg(file_size - FOOTER_SIZE, std::ios::beg);

    Footer footer;
    f.read(footer.magic, 8);

    if (std::memcmp(footer.magic, FOOTER_MAGIC, 8) != 0) {
        std::cerr << "Warning: Invalid footer magic" << std::endl;
        return false;
    }

    footer.data_checksum = read_u64_le(f);
    footer.total_events = read_u64_le(f);
    footer.data_bytes = read_u64_le(f);
    footer.first_event_time_ns = read_f64_le(f);
    footer.last_event_time_ns = read_f64_le(f);
    footer.file_end_time_ns = read_u64_le(f);
    f.read(reinterpret_cast<char*>(&footer.write_complete), 1);

    std::cout << "\n=== Footer ===" << std::endl;
    std::cout << "Total events:    " << footer.total_events << std::endl;
    std::cout << "Data bytes:      " << footer.data_bytes << std::endl;
    std::cout << "First timestamp: " << footer.first_event_time_ns << " ns" << std::endl;
    std::cout << "Last timestamp:  " << footer.last_event_time_ns << " ns" << std::endl;
    std::cout << "Write complete:  " << (footer.write_complete ? "Yes" : "No") << std::endl;

    return true;
}

// Main function
void read_delila(const char* filename, int max_events = -1) {
    std::cout << "Reading DELILA file: " << filename << std::endl;

    std::ifstream f(filename, std::ios::binary);
    if (!f.is_open()) {
        std::cerr << "Error: Cannot open file" << std::endl;
        return;
    }

    // Get file size
    f.seekg(0, std::ios::end);
    size_t file_size = f.tellg();
    f.seekg(0, std::ios::beg);
    std::cout << "File size: " << file_size << " bytes" << std::endl;

    // Read header
    size_t header_end_pos;
    if (!read_header(f, header_end_pos)) {
        return;
    }

    // Read footer
    read_footer(f, file_size);

    // Calculate data region
    size_t data_end = file_size - FOOTER_SIZE;
    std::cout << "\nData region: " << header_end_pos << " - " << data_end << std::endl;

    // Read data blocks
    f.seekg(header_end_pos, std::ios::beg);

    std::vector<Event> all_events;
    int block_count = 0;

    while (f.tellg() < static_cast<std::streampos>(data_end)) {
        // Read block length
        uint32_t block_len = read_u32_le(f);
        if (block_len == 0 || block_len > 100000000) {
            std::cerr << "Warning: Invalid block length " << block_len << std::endl;
            break;
        }

        // Read block data
        std::vector<uint8_t> block_data(block_len);
        f.read(reinterpret_cast<char*>(block_data.data()), block_len);

        if (!f.good()) {
            std::cerr << "Warning: Read error at block " << block_count << std::endl;
            break;
        }

        // Parse MessagePack
        MsgPackParser parser(block_data);
        uint32_t source_id;
        std::vector<Event> events;

        if (!parser.parse_batch(source_id, events)) {
            std::cerr << "Warning: Failed to parse block " << block_count << std::endl;
            break;
        }

        // Add events
        for (const auto& ev : events) {
            all_events.push_back(ev);
            if (max_events > 0 && static_cast<int>(all_events.size()) >= max_events) {
                break;
            }
        }

        block_count++;

        if (max_events > 0 && static_cast<int>(all_events.size()) >= max_events) {
            break;
        }
    }

    std::cout << "\nParsed " << block_count << " blocks, " << all_events.size() << " events" << std::endl;

    if (all_events.empty()) {
        std::cout << "No events to display" << std::endl;
        return;
    }

    // Print first 10 events
    std::cout << "\n=== First " << std::min(10, static_cast<int>(all_events.size())) << " events ===" << std::endl;
    std::cout << "Module  Ch  Energy  EShort  Timestamp(ns)      Flags" << std::endl;
    std::cout << "------  --  ------  ------  -----------------  -----" << std::endl;
    for (size_t i = 0; i < std::min(static_cast<size_t>(10), all_events.size()); i++) {
        const Event& ev = all_events[i];
        printf("%6d  %2d  %6d  %6d  %17.1f  0x%llx\n",
               ev.module, ev.channel, ev.energy, ev.energy_short,
               ev.timestamp_ns, static_cast<unsigned long long>(ev.flags));
    }

    // Create histograms
    TCanvas* c1 = new TCanvas("c1", "DELILA Data", 1200, 800);
    c1->Divide(2, 2);

    // Energy histogram
    c1->cd(1);
    TH1F* h_energy = new TH1F("h_energy", "Energy Distribution;Energy;Counts", 4096, 0, 65536);
    for (const auto& ev : all_events) {
        h_energy->Fill(ev.energy);
    }
    h_energy->Draw();

    // Energy short histogram
    c1->cd(2);
    TH1F* h_eshort = new TH1F("h_eshort", "Energy Short Distribution;Energy Short;Counts", 4096, 0, 65536);
    for (const auto& ev : all_events) {
        h_eshort->Fill(ev.energy_short);
    }
    h_eshort->Draw();

    // Channel distribution
    c1->cd(3);
    TH1F* h_ch = new TH1F("h_ch", "Channel Distribution;Channel;Counts", 64, 0, 64);
    for (const auto& ev : all_events) {
        h_ch->Fill(ev.channel);
    }
    h_ch->Draw();

    // Module distribution
    c1->cd(4);
    TH1F* h_mod = new TH1F("h_mod", "Module Distribution;Module;Counts", 32, 0, 32);
    for (const auto& ev : all_events) {
        h_mod->Fill(ev.module);
    }
    h_mod->Draw();

    c1->Update();

    std::cout << "\nHistograms created. Use ROOT interactive mode to explore." << std::endl;
}
