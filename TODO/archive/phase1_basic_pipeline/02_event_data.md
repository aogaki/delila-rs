# 02: EventData Structures

## Goal
Define both EventData (full) and MinimalEventData structures compatible with C++ version.

## C++ Reference (DELILA2)

### EventData (Full - with waveforms)
```cpp
class EventData {
    double timeStampNs;                    // 8 bytes
    size_t waveformSize;                   // 8 bytes
    std::vector<int32_t> analogProbe1;     // variable
    std::vector<int32_t> analogProbe2;     // variable
    std::vector<uint8_t> digitalProbe1;    // variable
    std::vector<uint8_t> digitalProbe2;    // variable
    std::vector<uint8_t> digitalProbe3;    // variable
    std::vector<uint8_t> digitalProbe4;    // variable
    uint16_t energy;
    uint16_t energyShort;
    uint8_t module;
    uint8_t channel;
    uint8_t timeResolution;
    uint8_t analogProbe1Type;
    uint8_t analogProbe2Type;
    uint8_t digitalProbe1Type;
    uint8_t digitalProbe2Type;
    uint8_t digitalProbe3Type;
    uint8_t digitalProbe4Type;
    uint8_t downSampleFactor;
    uint64_t flags;
    uint64_t aMax;
};
```

### MinimalEventData (No waveforms - 22 bytes packed)
```cpp
class MinimalEventData {
    uint8_t module;          // 1 byte
    uint8_t channel;         // 1 byte
    uint16_t energy;         // 2 bytes
    uint16_t energyShort;    // 2 bytes
    double timeStampNs;      // 8 bytes
    uint64_t flags;          // 8 bytes
} __attribute__((packed));   // Total: 22 bytes
```

### Flag Definitions
```cpp
FLAG_PILEUP        = 0x01  // Pileup detected
FLAG_TRIGGER_LOST  = 0x02  // Trigger lost
FLAG_OVER_RANGE    = 0x04  // Signal saturation
FLAG_1024_TRIGGER  = 0x08  // 1024 trigger count
FLAG_N_LOST_TRIGGER= 0x10  // N lost triggers
```

## Tasks
- [x] Create `src/common/mod.rs`
- [x] Define `MinimalEventData` struct (packed, 22 bytes)
- [ ] Define `EventData` struct (full, with waveforms) - *Deferred to Phase 2*
- [x] Define `MinimalEventDataBatch` for batched transfer
- [x] Implement flag constants and helper methods
- [x] Write unit tests for serialization roundtrip
- [ ] Verify MessagePack binary compatibility with C++ - *Deferred*

## Implemented
- `MinimalEventData` - 22 bytes packed struct with serde
- `MinimalEventDataBatch` - batch container with source_id, sequence_number
- `flags` module - FLAG_PILEUP, FLAG_TRIGGER_LOST, etc.
- Helper methods: `has_pileup()`, `has_trigger_lost()`, `has_over_range()`
- Unit tests: size check, roundtrip serialization, flag helpers

## Test Results
```
test common::tests::flag_helpers ... ok
test common::tests::minimal_event_data_size ... ok
test common::tests::minimal_event_data_roundtrip ... ok
test common::tests::batch_roundtrip ... ok
```

## Status: COMPLETED (Phase 1 scope)
EventData (full) deferred to Phase 2 (CAEN driver integration).
