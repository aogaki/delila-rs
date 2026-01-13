/**
 * C wrapper for CAEN_FELib variadic functions
 *
 * Rust cannot directly call C variadic functions correctly on all platforms
 * (especially macOS ARM64). This wrapper provides non-variadic functions
 * that Rust can safely call.
 */

#include <stddef.h>
#include <stdint.h>
#include <CAEN_FELib.h>

/**
 * Wrapper for CAEN_FELib_ReadData with RAW format:
 * - DATA: uint8_t* (pointer to buffer)
 * - SIZE: size_t* (pointer to receive actual size)
 * - N_EVENTS: uint32_t* (pointer to receive event count)
 */
int caen_read_data_raw(
    uint64_t handle,
    int timeout,
    uint8_t* data,
    size_t* size,
    uint32_t* n_events
) {
    return CAEN_FELib_ReadData(handle, timeout, data, size, n_events);
}
