//! AMax custom-firmware register map (CAEN VX2730 + DELILA AMax FW).
//!
//! The actual constants (`PAGE_BASE`, `PAGE_STRIDE`, `REG_*`) and the
//! `channel_register_byte_addr()` helper are auto-generated from the FW
//! developer's `RegisterFile.json` + `tools/amax_viewer/fw_params.json`.
//!
//! Run `cargo run --bin amax_codegen -- <RegisterFile.json>` to regenerate.

pub use super::amax_registers_generated::*;

use crate::config::digitizer::FirmwareType;

/// How many AMax channel pages `apply_amax_channel_config` may sweep.
///
/// `num_channels` comes from the digitizer JSON and is hand-maintained, while
/// [`CHANNEL_PAGES`] is auto-derived from the RegisterFile the current bindings
/// were generated from. When the config is wider than the firmware, the extra
/// channels' registers would land on addresses the firmware does not implement
/// — for the 2-channel 12august FW, channel 30 of a stale 32-channel config
/// resolves to word `0x800000`, which is the *old* 32-channel map's ch0 page.
/// Writing outside the firmware's own map is how AMax firmware gets corrupted,
/// so the sweep is clamped instead.
///
/// `fw_channel_pages == 0` means the RegisterFile carries no per-channel pages
/// at all (broadcast-only firmware); there is no page span to clamp against,
/// so the config is trusted as-is.
pub fn amax_channel_span(num_channels: u8, fw_channel_pages: u32) -> u8 {
    if fw_channel_pages == 0 {
        return num_channels;
    }
    num_channels.min(u8::try_from(fw_channel_pages).unwrap_or(u8::MAX))
}

/// Operator-facing note when an AMax config asks for more channels than the
/// firmware bindings define, `None` when it fits (or the config isn't AMax).
///
/// The clamp itself protects the hardware, but it happens inside the Reader and
/// only shows up as a smaller `params_applied` count — from the operator's seat
/// that is a silent failure. Both the Apply and Configure routes append this to
/// the HTTP response so the skipped channels are visible where the action was
/// taken (CLAUDE.md: silent failure を作らない).
pub fn channel_clamp_note(firmware: FirmwareType, num_channels: u8) -> Option<String> {
    if firmware != FirmwareType::AMax {
        return None;
    }
    let span = amax_channel_span(num_channels, CHANNEL_PAGES);
    if span >= num_channels {
        return None;
    }
    let last = num_channels - 1;
    let which = if span == last {
        format!("channel {}", span)
    } else {
        format!("channels {}-{}", span, last)
    };
    Some(format!(
        "AMax: config num_channels={} exceeds the {} channel page(s) this firmware defines; \
         {} skipped (fix num_channels in the digitizer JSON)",
        num_channels, CHANNEL_PAGES, which
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amax_channel_span_clamps_config_wider_than_firmware() {
        // The 2026-08 case: 32-channel digitizer JSON, 2-channel AMax FW.
        assert_eq!(amax_channel_span(32, 2), 2);
    }

    #[test]
    fn amax_channel_span_keeps_config_narrower_than_firmware() {
        // A config may deliberately drive fewer channels than the FW offers.
        assert_eq!(amax_channel_span(4, 32), 4);
        assert_eq!(amax_channel_span(32, 32), 32);
    }

    #[test]
    fn amax_channel_span_trusts_config_when_firmware_has_no_channel_pages() {
        // Broadcast-only firmware: CHANNEL_PAGES is 0 and must NOT be read as
        // "write nothing" — that would silently stop configuring the board.
        assert_eq!(amax_channel_span(32, 0), 32);
    }

    #[test]
    fn amax_channel_span_saturates_absurd_firmware_page_counts() {
        // CHANNEL_PAGES is a u32 span; a malformed RegisterFile must not make
        // the clamp truncate to some small u8.
        assert_eq!(amax_channel_span(32, 4096), 32);
    }

    #[test]
    fn clamp_note_names_the_skipped_channels() {
        let note = channel_clamp_note(FirmwareType::AMax, u8::MAX).unwrap_or_default();
        // Whatever RegisterFile the committed bindings came from, the note must
        // name the real span rather than a hardcoded guess.
        assert!(
            note.contains(&format!("{} channel page(s)", CHANNEL_PAGES)),
            "note should quote CHANNEL_PAGES: {note}"
        );
        assert!(note.contains("skipped"), "note should say skipped: {note}");
    }

    #[test]
    fn clamp_note_is_silent_when_the_config_fits() {
        let fits = u8::try_from(CHANNEL_PAGES).unwrap_or(u8::MAX);
        assert_eq!(channel_clamp_note(FirmwareType::AMax, fits), None);
        assert_eq!(channel_clamp_note(FirmwareType::AMax, 1), None);
    }

    #[test]
    fn clamp_note_ignores_non_amax_firmware() {
        // Every other firmware addresses channels through FELib, not through
        // the AMax user-register page map, so CHANNEL_PAGES says nothing.
        assert_eq!(channel_clamp_note(FirmwareType::PSD1, u8::MAX), None);
    }
}
