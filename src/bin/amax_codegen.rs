//! AMax FW codegen — read RegisterFile.json (FW dev output) + fw_params.json (UI metadata)
//! and emit:
//!   - `src/config/amax_generated.rs`                     (AMaxChannelConfig struct)
//!   - `src/reader/caen/amax_registers_generated.rs`      (REG_* + channel_register_byte_addr)
//!   - `web/operator-ui/src/app/models/amax-generated.ts` (interface + AMAX_*_PARAMS arrays)
//!
//! Run when FW is updated:
//!   cargo run --bin amax_codegen -- AMAX_firmware32_channel_4input_caenlist/output/output/RegisterFile.json
//!
//! Default `fw_params.json` path is `tools/amax_viewer/fw_params.json`.
//!
//! After running, `cargo build` (Rust) and `cd web/operator-ui && npm run build` (TS).

use clap::Parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Generate AMax FW Rust + TypeScript bindings")]
struct Cli {
    /// Path to RegisterFile.json (from the FW developer)
    register_file: PathBuf,

    /// Path to fw_params.json (UI metadata: label/category/type/options/...)
    #[arg(short = 'p', long, default_value = "tools/amax_viewer/fw_params.json")]
    params_file: PathBuf,

    /// Output directory for Rust files (defaults to crate src/)
    #[arg(long, default_value = "src")]
    rust_out: PathBuf,

    /// Output path for the generated TypeScript file
    #[arg(
        long,
        default_value = "web/operator-ui/src/app/models/amax-generated.ts"
    )]
    ts_out: PathBuf,

    /// Override the auto-derived channel-page base (word units, ch0). Normally
    /// left unset — `derive_layout` computes it as the minimum address of the
    /// canonical register group. Accepts decimal or `0x`-prefixed hex. Use
    /// only to force a base the auto-derivation can't see (e.g. a stripped
    /// RegisterFile.json).
    #[arg(long, value_parser = parse_u32_auto)]
    page_base: Option<u32>,

    /// Override the auto-derived word stride between consecutive channel
    /// pages. Normally derived from `addr(ch1) - addr(ch0)`. Accepts decimal
    /// or hex. `0` means a single broadcast page (write fans out in hardware).
    #[arg(long, value_parser = parse_u32_auto)]
    page_stride: Option<u32>,

    /// Override the auto-derived broadcast-page base (word units). Normally the
    /// minimum address of the `page_amax_energy/<NAME>` broadcast group, or the
    /// per-channel base when no broadcast group exists. Accepts decimal or hex.
    #[arg(long, value_parser = parse_u32_auto)]
    broadcast_base: Option<u32>,

    /// Override the auto-derived canonical-group choice. When unset,
    /// `derive_layout` prefers the per-channel ch0 page whenever one exists
    /// (this reproduces the hardware-verified 17June output). Pass
    /// `--prefer-per-channel false` to force the broadcast page as the
    /// canonical `REG_*` source instead.
    #[arg(long)]
    prefer_per_channel: Option<bool>,
}

/// Parse a `u32` accepting either decimal or a `0x`/`0X`-prefixed hex literal.
/// Register-map addresses are universally written in hex in CAEN tooling, so
/// the historical `--page-base 0x8000` invocations need this.
fn parse_u32_auto(s: &str) -> Result<u32, String> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).map_err(|e| e.to_string()),
        None => s.parse::<u32>().map_err(|e| e.to_string()),
    }
}

// ---- RegisterFile.json schema (subset) ----
// Two FW eras coexist:
//   - 2026-03 era: per-channel paths like `page_amax_energy_0/POLARITY`
//     served via the `Path` field, page base `0x800000`, ch1 stride `0x40000`.
//   - 2026-04 era (caenlist firmware32_4input): one set of `Name` registers
//     like `page_amax_energy_POLARITY` at page base `0x100000`, ch0 only.
// We accept either by treating `Path` and `Name` as alternates and trimming
// any `_<digit>/` channel infix when present.

#[derive(Deserialize)]
struct RegisterFile {
    #[serde(rename = "Registers")]
    registers: Vec<Register>,
}

#[derive(Deserialize)]
struct Register {
    #[serde(rename = "Address")]
    address: u32,
    #[serde(rename = "Path", default)]
    path: Option<String>,
    #[serde(rename = "Name", default)]
    name: Option<String>,
}

/// Consume leading `<digits>_` tokens from an underscore-form page body and
/// return `(channel, register_name)`. The channel is the LAST leading numeric
/// token, which covers every underscore naming era at once:
///   - bare:            `POLARITY`       → `(None, "POLARITY")`     broadcast page
///   - 2026-05 16-ch:   `15_THRS`        → `(Some(15), "THRS")`     `<ch>_<NAME>`
///   - 2026-06 custom:  `1_0_POLARITY`   → `(Some(0), "POLARITY")`  `<group>_<ch>_<NAME>`
///
/// Register names never start with a digit, so a leading `<digits>_` token is
/// always a page-index component (group or channel), never part of the name.
/// The custom-width FW (`page_amax_energy_1_<ch>_<NAME>`) prepends a group
/// index before the channel; taking the *last* leading number yields the real
/// channel in both the one-number and two-number forms.
fn split_underscore_body(body: &str) -> (Option<u32>, &str) {
    let mut rest = body;
    let mut channel = None;
    while let Some(us) = rest.find('_') {
        let head = &rest[..us];
        if head.is_empty() || !head.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        channel = head.parse().ok();
        rest = &rest[us + 1..];
    }
    (channel, rest)
}

impl Register {
    /// Identifier used for matching against `fw_params.json`. Strips the page
    /// prefix and any channel/group index so every era's per-channel name lands
    /// on the same key (e.g. `THRS`).
    ///
    /// FW naming eras handled:
    ///   - 2026-03:        `page_amax_energy_0/THRS`        (slash, channel)
    ///   - 2026-04 32-ch:  `page_amax_energy_THRS`          (bare, no index)
    ///   - 2026-05 16-ch:  `page_amax_energy_15_THRS`       (underscore, channel)
    ///   - 2026-05 13may:  `page_amax_energy/THRS` (broadcast) +
    ///     `page_amax_energy_4_<N>/THRS` (slash, group+channel)
    ///   - 2026-06 custom: `page_amax_energy_1_<N>_THRS`    (underscore, group+channel)
    fn fw_key(&self) -> Option<String> {
        let raw = self.path.as_deref().or(self.name.as_deref())?;
        // 13may broadcast form: `page_amax_energy/<NAME>`.
        if let Some(rest) = raw.strip_prefix("page_amax_energy/") {
            return Some(rest.to_string());
        }
        let Some(rest) = raw.strip_prefix("page_amax_energy_") else {
            // Non-page (board-level) register — use the raw name as-is.
            return Some(raw.to_string());
        };
        // Slash forms (`<N>/<NAME>` 2026-03, `4_<N>/<NAME>` 13may): the name is
        // everything after the slash.
        if let Some(slash) = rest.find('/') {
            return Some(rest[slash + 1..].to_string());
        }
        // Underscore forms: drop the leading group/channel numeric tokens.
        let (_, name) = split_underscore_body(rest);
        Some(name.to_string())
    }

    /// True iff this register belongs to the per-channel `page_amax_energy_*`
    /// family (the only set we surface in `AMaxChannelConfig`). Also true for
    /// the 13may era broadcast slash form `page_amax_energy/<NAME>` so the
    /// canonical-filter and codegen treat broadcast + per-channel uniformly.
    fn is_per_channel(&self) -> bool {
        let raw = self.path.as_deref().or(self.name.as_deref()).unwrap_or("");
        raw.starts_with("page_amax_energy_") || raw.starts_with("page_amax_energy/")
    }

    /// Returns the channel index when this register is one of the per-channel
    /// pages (`page_amax_energy_<N>_<NAME>` underscore-form or
    /// `page_amax_energy_<N>/<NAME>` slash-form). Returns `None` for the
    /// broadcast page (`page_amax_energy_<NAME>` with no index — write fans
    /// out to all channels) and for any non-`page_amax_energy_*` register.
    ///
    /// The codegen filter uses this to keep only the broadcast (canonical)
    /// register set; per-channel pages would otherwise emit duplicate
    /// `REG_*` constants. Live AMax operation goes through the broadcast
    /// page anyway (single write hits every channel — see
    /// `apply_amax_channel_config` in `src/reader/caen/handle.rs`).
    fn channel_index(&self) -> Option<u32> {
        let raw = self.path.as_deref().or(self.name.as_deref())?;
        // 13may broadcast (`page_amax_energy/<NAME>`) = canonical, no index.
        if raw.starts_with("page_amax_energy/") {
            return None;
        }
        let rest = raw.strip_prefix("page_amax_energy_")?;
        // Slash forms (`<N>/<NAME>` 2026-03, `4_<N>/<NAME>` 13may): the channel
        // is the last numeric token before the slash (e.g. `4_0` → 0).
        if let Some(slash) = rest.find('/') {
            return rest[..slash].rsplit('_').next()?.parse().ok();
        }
        // Underscore forms: channel = last leading numeric token. Covers the
        // 16-ch `<ch>_<NAME>` form and the custom-width `<group>_<ch>_<NAME>`
        // form. Bare `<NAME>` (broadcast page) yields None.
        split_underscore_body(rest).0
    }

    /// True iff this register's name matches a key in `fw_params.board_params`
    /// — i.e. a global / board-level register (e.g. `ENABLE_ACQ` for the AMax
    /// debug FW). Per-channel registers always take priority over board-level
    /// matches so we don't accidentally promote a channel reg to global.
    fn is_board_level(&self, board_param_keys: &[String]) -> bool {
        if self.is_per_channel() {
            return false;
        }
        let raw = self.path.as_deref().or(self.name.as_deref()).unwrap_or("");
        // Strip leading slash so JSON `"Path": "/ENABLE_ACQ"` matches
        // `"ENABLE_ACQ"`.
        let trimmed = raw.trim_start_matches('/');
        board_param_keys
            .iter()
            .any(|k| trimmed == k || trimmed.contains(k))
    }
}

/// Pick the canonical per-channel register entries from a `Register` list.
/// Across FW eras the canonical entry is the one without a channel index:
///   - 2026-03 era: only per-channel pages exist (`page_amax_energy_0/...`,
///     `page_amax_energy_1/...`, ...) — we fall back to ch0 (lowest index)
///     so the page is represented at all.
///   - 2026-04 32-ch era: `page_amax_energy_<NAME>` only (no index, single
///     broadcast page) — picked directly.
///   - 2026-05 16-ch era: broadcast `page_amax_energy_<NAME>` (no index)
///     plus 16 per-channel pages `page_amax_energy_<N>_<NAME>` — broadcast
///     wins, per-channel duplicates dropped (single write fans out to all
///     channels via hardware; see `apply_amax_channel_config`).
#[allow(dead_code)] // retained for unit-test ergonomics; production path uses `_with`
fn select_canonical_per_channel(registers: &[Register]) -> Vec<&Register> {
    select_canonical_per_channel_with(registers, false)
}

/// Same as `select_canonical_per_channel` but switchable: when
/// `prefer_per_channel` is true the ch0 per-channel page wins over the
/// broadcast page. Use when the broadcast page does not fan out to all
/// channels in hardware (observed on the 13may caenlist FW).
fn select_canonical_per_channel_with(
    registers: &[Register],
    prefer_per_channel: bool,
) -> Vec<&Register> {
    let has_broadcast = registers
        .iter()
        .any(|r| r.is_per_channel() && r.channel_index().is_none());
    let has_ch0 = registers
        .iter()
        .any(|r| r.is_per_channel() && r.channel_index() == Some(0));
    let use_per_channel = prefer_per_channel && has_ch0;
    registers
        .iter()
        .filter(|r| {
            if !r.is_per_channel() {
                return false;
            }
            match r.channel_index() {
                None => !use_per_channel, // broadcast — preferred unless we asked for per-channel
                Some(0) => use_per_channel || !has_broadcast, // ch0 wins when asked or as 2026-03 fallback
                Some(_) => false,                             // skip ch1+ duplicates
            }
        })
        .collect()
}

// ---- Address-map auto-derivation ----
//
// Every AMax FW rebuild shifts the register page addresses. Rather than make a
// human hand-pick `--page-base` / `--page-stride` / `--prefer-per-channel`
// (and separately hand-edit `BROADCAST_BASE` in handle.rs), we derive the whole
// layout from RegisterFile.json. The three groups are classified by
// `Register::channel_index()`:
//   - `None`     → broadcast page (`page_amax_energy/<NAME>`)
//   - `Some(0)`  → per-channel ch0 page (`page_amax_energy_4_0/<NAME>`)
//   - `Some(n)`  → per-channel ch-n page (used only to measure the stride)

/// Auto-derived register-page layout. All values are word units (the FELib
/// byte address is `word * 4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DerivedLayout {
    /// Base of the canonical (per-channel ch0, or broadcast fallback) page.
    page_base: u32,
    /// Word stride between consecutive per-channel pages (`0` if none).
    page_stride: u32,
    /// Base of the broadcast page (`page_base` when no broadcast group exists).
    broadcast_base: u32,
    /// Number of per-channel pages the FW defines: the span `0..=max index`
    /// (`0` when the RegisterFile has no per-channel pages at all). Emitted as
    /// `CHANNEL_PAGES` so `apply_amax_channel_config` can clamp its write loop
    /// to the pages that actually exist — the digitizer JSON's `num_channels`
    /// is hand-maintained and does not track FW rebuilds.
    channel_pages: u32,
    /// True when the per-channel ch0 page is the canonical `REG_*` source.
    prefer_per_channel: bool,
}

/// Derive the page layout from the register list. `prefer_override` forces the
/// canonical-group choice; when `None`, the per-channel ch0 page wins whenever
/// one exists (this reproduces the hardware-verified 17June output).
fn derive_layout(registers: &[Register], prefer_override: Option<bool>) -> DerivedLayout {
    use std::collections::BTreeMap;

    // Minimum address seen per per-channel index, and for the broadcast group.
    let mut min_by_idx: BTreeMap<u32, u32> = BTreeMap::new();
    let mut broadcast_min: Option<u32> = None;
    for r in registers {
        if !r.is_per_channel() {
            continue;
        }
        match r.channel_index() {
            None => {
                broadcast_min = Some(broadcast_min.map_or(r.address, |m| m.min(r.address)));
            }
            Some(idx) => {
                let e = min_by_idx.entry(idx).or_insert(r.address);
                *e = (*e).min(r.address);
            }
        }
    }

    let has_ch0 = min_by_idx.contains_key(&0);
    let prefer_per_channel = prefer_override.unwrap_or(has_ch0);

    // `page_base` is the base of whichever group is canonical.
    let page_base = if prefer_per_channel && has_ch0 {
        min_by_idx[&0]
    } else if let Some(bc) = broadcast_min {
        bc
    } else if has_ch0 {
        min_by_idx[&0]
    } else {
        0
    };

    // Stride = (addr(ch-k) - addr(ch0)) / k for the smallest k > 0, so a sparse
    // index set (e.g. only ch0 + ch4 present) still yields the per-page stride.
    let page_stride = match (min_by_idx.get(&0), min_by_idx.iter().find(|(k, _)| **k > 0)) {
        (Some(&a0), Some((&k, &ak))) if k > 0 => {
            let delta = ak.saturating_sub(a0);
            if delta % k == 0 {
                delta / k
            } else {
                delta // non-uniform spacing — fall back to the raw first step
            }
        }
        _ => 0,
    };

    let broadcast_base = broadcast_min.unwrap_or(page_base);

    // Span rather than count: a sparse index set (ch0 + ch4 only) reports 5, so
    // clamping to it can never hide a page that does exist. `main` warns when
    // the set is sparse instead of deciding silently.
    let channel_pages = min_by_idx.keys().next_back().map_or(0, |&max| max + 1);

    DerivedLayout {
        page_base,
        page_stride,
        broadcast_base,
        channel_pages,
        prefer_per_channel,
    }
}

/// FW-corruption guard: the broadcast page and the per-channel ch0 page must
/// share an identical in-page offset layout, because `apply_amax_channel_config`
/// reuses one `channel_writes()` offset list to drive BOTH pages. If a register
/// sits at a different relative offset on the two pages, a write would land on
/// the wrong register — and writing a stale/wrong AMax address can brick the
/// firmware. We verify the invariant and fail codegen loudly if it breaks.
fn assert_broadcast_layout_matches(
    registers: &[Register],
    page_base: u32,
    broadcast_base: u32,
) -> Result<(), String> {
    use std::collections::HashMap;

    // fw_key → ch0 address (first seen).
    let mut ch0: HashMap<String, u32> = HashMap::new();
    for r in registers {
        if r.is_per_channel() && r.channel_index() == Some(0) {
            if let Some(k) = r.fw_key() {
                ch0.entry(k).or_insert(r.address);
            }
        }
    }

    for r in registers {
        if !(r.is_per_channel() && r.channel_index().is_none()) {
            continue;
        }
        let Some(k) = r.fw_key() else { continue };
        let Some(&a0) = ch0.get(&k) else { continue };
        let off_bc = i64::from(r.address) - i64::from(broadcast_base);
        let off_ch0 = i64::from(a0) - i64::from(page_base);
        if off_bc != off_ch0 {
            return Err(format!(
                "broadcast/per-channel layout mismatch for `{k}`: \
                 broadcast offset 0x{off_bc:X} (addr 0x{:X} - base 0x{broadcast_base:X}) \
                 != ch0 offset 0x{off_ch0:X} (addr 0x{a0:X} - base 0x{page_base:X}). \
                 Writing this FW would corrupt registers — codegen aborted.",
                r.address,
            ));
        }
    }
    Ok(())
}

/// Scrape the `REG_<NAME>` constant names from an existing generated registers
/// file so the change-summary can diff against the previous FW. Returns an
/// empty list if the file is absent (first run).
fn existing_reg_names(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("pub const REG_")?;
            Some(rest.split(':').next()?.trim().to_string())
        })
        .collect()
}

// ---- fw_params.json schema (extended with UI metadata) ----

#[derive(Deserialize)]
struct FwParams {
    params: HashMap<String, FwParam>,
    /// Board-level (global, non-per-channel) registers — e.g. `ENABLE_ACQ`
    /// for the AMax debug FW. Same `FwParam` shape as `params` but routed to
    /// a separate `AMaxBoardConfig` struct in the generated code.
    #[serde(default)]
    board_params: HashMap<String, FwParam>,
    #[serde(default)]
    readonly_patterns: Vec<String>,
}

#[derive(Deserialize, Clone)]
struct FwParam {
    bits: u32,
    #[serde(default)]
    default: u32,
    label: Option<String>,
    category: Option<String>,
    #[serde(rename = "type")]
    ty: Option<String>,
    options: Option<Vec<String>>,
    ui_max: Option<u64>,
    unit: Option<String>,
    /// Optional UI display-order key within a category. When set, overrides
    /// the default address-order sort for the TypeScript `AMAX_<CAT>_PARAMS`
    /// arrays only (Rust emission stays address-ordered). Registers without
    /// `ui_order` keep sorting by `word_offset`, so a value just needs to be
    /// chosen relative to the neighbours it should sit among.
    ui_order: Option<i64>,
}

// ---- Resolved per-register info used by all three emitters ----

struct ResolvedReg {
    /// Original FW name (from RegisterFile.json, e.g. "POLARITY", "AMAX_window").
    fw_name: String,
    /// snake_case Rust/TS field name (e.g. "polarity", "amax_window").
    field: String,
    /// Word offset within a channel page.
    word_offset: u32,
    bits: u32,
    default: u32,
    label: String,
    category: String,
    ty: String,           // "number" or "enum"
    options: Vec<String>, // empty unless ty == "enum"
    ui_max: u64,
    unit: Option<String>,
    /// Optional UI display-order override (TS arrays only); see `FwParam`.
    ui_order: Option<i64>,
}

fn snake_case(name: &str) -> String {
    name.to_lowercase()
}

fn category_label(cat: &str) -> String {
    // Title-case the category name for use in TypeScript const identifiers
    // (`AMAX_<LABEL>_PARAMS`). Well-known categories get their canonical
    // capitalisation; anything else is passed through with the first letter
    // upper-cased so a future `"housekeeping"` becomes `"Housekeeping"` →
    // `AMAX_HOUSEKEEPING_PARAMS` without a codegen edit.
    match cat {
        "input" => "Input".to_string(),
        "trigger" => "Trigger".to_string(),
        "energy" => "Energy".to_string(),
        "waveform" => "Waveform".to_string(),
        "debug" => "Debug".to_string(),
        "amax" => "AMax".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Sort key for emitted category order. Well-known categories first (in
/// signal-pipeline order: input → trigger → energy → waveform → coincidence
/// → debug), then any new category alphabetically. Stable so generated
/// output diffs cleanly across re-runs.
fn category_rank(cat: &str) -> u32 {
    match cat {
        "input" => 0,
        "trigger" => 1,
        "energy" => 2,
        // AMax-specific HLS peak detector (+ its dedicated baseline filter)
        // sits right after the energy / trap-filter family so the operator
        // can see the two-stage signal path side-by-side.
        "amax" => 3,
        "waveform" => 4,
        "coincidence" => 5,
        "debug" => 6,
        _ => 1000,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let register_text = fs::read_to_string(&cli.register_file)?;
    let register_file: RegisterFile = serde_json::from_str(&register_text)?;

    let params_text = fs::read_to_string(&cli.params_file)?;
    let fw_params: FwParams = serde_json::from_str(&params_text)?;

    // Auto-derive the page layout, then apply any explicit overrides.
    let layout = derive_layout(&register_file.registers, cli.prefer_per_channel);
    let page_base = cli.page_base.unwrap_or(layout.page_base);
    let page_stride = cli.page_stride.unwrap_or(layout.page_stride);
    let broadcast_base = cli.broadcast_base.unwrap_or(layout.broadcast_base);
    let prefer = layout.prefer_per_channel;
    let channel_pages = layout.channel_pages;

    // A gap in the per-channel index set means some page inside the span does
    // not exist, so the emitted `CHANNEL_PAGES` clamp is looser than the real
    // map. Never observed in a real RegisterFile; report it rather than let it
    // pass unnoticed (CLAUDE.md: no silent failures).
    {
        let present: std::collections::BTreeSet<u32> = register_file
            .registers
            .iter()
            .filter(|r| r.is_per_channel())
            .filter_map(|r| r.channel_index())
            .collect();
        if !present.is_empty() && present.len() as u32 != channel_pages {
            eprintln!(
                "amax_codegen WARN: per-channel page indices are sparse \
                 ({} pages across the span 0..{}) — CHANNEL_PAGES reports the \
                 span, so channels in the gaps would still be written",
                present.len(),
                channel_pages.saturating_sub(1),
            );
        }
    }

    // FW-corruption guard before we emit anything (uses the effective bases).
    if let Err(msg) =
        assert_broadcast_layout_matches(&register_file.registers, page_base, broadcast_base)
    {
        return Err(msg.into());
    }

    // Snapshot the previous REG_* set so we can report what this FW adds/drops.
    let rust_reg_path = cli.rust_out.join("reader/caen/amax_registers_generated.rs");
    let prev_reg_names = existing_reg_names(&rust_reg_path);

    let mut ch0: Vec<&Register> =
        select_canonical_per_channel_with(&register_file.registers, prefer);
    ch0.sort_by_key(|r| r.address);

    // Resolve each writable register against fw_params.json.
    let mut resolved: Vec<ResolvedReg> = Vec::new();
    let mut skipped_readonly: Vec<String> = Vec::new();
    let mut skipped_no_meta: Vec<String> = Vec::new();
    for r in &ch0 {
        let Some(name) = r.fw_key() else { continue };
        if fw_params.readonly_patterns.iter().any(|p| name.contains(p)) {
            skipped_readonly.push(name);
            continue;
        }
        let Some(meta) = fw_params.params.get(&name) else {
            skipped_no_meta.push(name);
            continue;
        };
        let bits = meta.bits;
        let ui_max = meta.ui_max.unwrap_or_else(|| {
            if bits >= 32 {
                (1u64 << 32) - 1
            } else {
                (1u64 << bits) - 1
            }
        });
        let ty = meta.ty.clone().unwrap_or_else(|| "number".to_string());
        // The "word offset" we emit is whatever `set_user_register` needs —
        // for the new FW (page_base = 0x100000) we treat the offset as the
        // raw address minus that base. SHAP_TRIGG / SHAP_BL_HOLD live on
        // a separate page (0x1C0000) but the byte-address math still works
        // because we encode the full distance from page_base.
        let word_offset = r.address.saturating_sub(page_base);
        resolved.push(ResolvedReg {
            field: snake_case(&name),
            word_offset,
            bits,
            default: meta.default,
            label: meta.label.clone().unwrap_or_else(|| name.clone()),
            category: meta.category.clone().unwrap_or_else(|| "other".to_string()),
            ty,
            options: meta.options.clone().unwrap_or_default(),
            ui_max,
            unit: meta.unit.clone(),
            ui_order: meta.ui_order,
            fw_name: name,
        });
    }

    if !skipped_no_meta.is_empty() {
        eprintln!(
            "warning: {} register(s) in RegisterFile.json have no entry in fw_params.json (skipped):",
            skipped_no_meta.len()
        );
        for n in &skipped_no_meta {
            eprintln!("  - {}", n);
        }
    }

    // Second pass: pick up board-level (global) registers declared in
    // `fw_params.board_params`. Currently this is just `ENABLE_ACQ` for the
    // AMax debug FW, but the same path supports any future global toggle.
    let board_param_keys: Vec<String> = fw_params.board_params.keys().cloned().collect();
    let board_resolved: Vec<ResolvedReg> = if board_param_keys.is_empty() {
        Vec::new()
    } else {
        let mut br: Vec<&Register> = register_file
            .registers
            .iter()
            .filter(|r| r.is_board_level(&board_param_keys))
            .collect();
        br.sort_by_key(|r| r.address);

        let mut out = Vec::new();
        for r in &br {
            // Use the raw name (no `page_amax_energy_` prefix to strip).
            let raw = r.path.as_deref().or(r.name.as_deref()).unwrap_or("");
            let trimmed = raw.trim_start_matches('/');
            let name = board_param_keys
                .iter()
                .find(|k| trimmed == k.as_str() || trimmed.contains(k.as_str()))
                .cloned()
                .unwrap_or_else(|| trimmed.to_string());
            let Some(meta) = fw_params.board_params.get(&name) else {
                continue;
            };
            let bits = meta.bits;
            let ui_max = meta.ui_max.unwrap_or_else(|| {
                if bits >= 32 {
                    (1u64 << 32) - 1
                } else {
                    (1u64 << bits) - 1
                }
            });
            let ty = meta.ty.clone().unwrap_or_else(|| "number".to_string());
            // Board registers carry the *raw* address from the FW because
            // there's no per-channel page offset to factor out.
            out.push(ResolvedReg {
                field: snake_case(&name),
                word_offset: r.address,
                bits,
                default: meta.default,
                label: meta.label.clone().unwrap_or_else(|| name.clone()),
                category: meta.category.clone().unwrap_or_else(|| "debug".to_string()),
                ty,
                options: meta.options.clone().unwrap_or_default(),
                ui_max,
                unit: meta.unit.clone(),
                ui_order: meta.ui_order,
                fw_name: name,
            });
        }
        out
    };

    // ---- Change summary (stderr) ----
    let src = |o: Option<u32>| if o.is_some() { "override" } else { "auto" };
    eprintln!(
        "amax_codegen layout: PAGE_BASE=0x{:X} ({}), PAGE_STRIDE=0x{:X} ({}), \
         BROADCAST_BASE=0x{:X} ({}), CHANNEL_PAGES={}, canonical={}",
        page_base,
        src(cli.page_base),
        page_stride,
        src(cli.page_stride),
        broadcast_base,
        src(cli.broadcast_base),
        channel_pages,
        if prefer {
            "per-channel ch0"
        } else {
            "broadcast"
        },
    );

    // REG_* diff vs the previously generated file.
    let new_reg_names: std::collections::BTreeSet<String> =
        resolved.iter().map(|r| r.field.to_uppercase()).collect();
    let prev_set: std::collections::BTreeSet<String> = prev_reg_names.into_iter().collect();
    let added: Vec<&String> = new_reg_names.difference(&prev_set).collect();
    let removed: Vec<&String> = prev_set.difference(&new_reg_names).collect();
    if prev_set.is_empty() {
        eprintln!(
            "amax_codegen: first generation ({} registers)",
            new_reg_names.len()
        );
    } else if added.is_empty() && removed.is_empty() {
        eprintln!(
            "amax_codegen: register set unchanged ({} registers)",
            new_reg_names.len()
        );
    } else {
        eprintln!(
            "amax_codegen: register set changed — {} added, {} removed",
            added.len(),
            removed.len()
        );
        if !added.is_empty() {
            eprintln!(
                "  + {}",
                added
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !removed.is_empty() {
            eprintln!(
                "  - {}",
                removed
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    eprintln!(
        "amax_codegen: {} per-channel writable, {} board-level, {} read-only skipped",
        resolved.len(),
        board_resolved.len(),
        skipped_readonly.len()
    );

    let rust_struct_path = cli.rust_out.join("config/amax_generated.rs");

    fs::write(
        &rust_struct_path,
        emit_rust_struct(
            &resolved,
            &board_resolved,
            &cli.register_file,
            &cli.params_file,
        ),
    )?;
    fs::write(
        &rust_reg_path,
        emit_rust_registers(
            &resolved,
            &board_resolved,
            page_base,
            page_stride,
            broadcast_base,
            channel_pages,
            &cli.register_file,
        ),
    )?;
    fs::write(
        &cli.ts_out,
        emit_typescript(
            &resolved,
            &board_resolved,
            &cli.register_file,
            &cli.params_file,
        ),
    )?;

    eprintln!("wrote {}", rust_struct_path.display());
    eprintln!("wrote {}", rust_reg_path.display());
    eprintln!("wrote {}", cli.ts_out.display());
    Ok(())
}

// ---- Rust struct emitter ----

fn header_rust(register_file: &std::path::Path, params_file: &std::path::Path) -> String {
    format!(
        "//! AUTO-GENERATED by `cargo run --bin amax_codegen`. Do not edit by hand.\n\
         //!\n\
         //! Sources:\n\
         //! - {}\n\
         //! - {}\n",
        register_file.display(),
        params_file.display()
    )
}

fn emit_rust_struct(
    resolved: &[ResolvedReg],
    board_resolved: &[ResolvedReg],
    register_file: &std::path::Path,
    params_file: &std::path::Path,
) -> String {
    let mut out = String::new();
    out.push_str(&header_rust(register_file, params_file));
    out.push_str(
        "//!\n\
         //! AMax custom-firmware per-channel writable register set.\n\
         //! Each field maps 1:1 to one user-register write at\n\
         //! `(PAGE_BASE + channel * PAGE_STRIDE + REG_<NAME>) * 4` byte\n\
         //! address (see `amax_registers_generated::channel_register_byte_addr`).\n\
         \n\
         use serde::{Deserialize, Serialize};\n\
         use utoipa::ToSchema;\n\
         \n\
         #[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]\n\
         pub struct AMaxChannelConfig {\n",
    );
    for r in resolved {
        let unit = r
            .unit
            .as_deref()
            .map(|u| format!(", unit: {}", u))
            .unwrap_or_default();
        out.push_str(&format!(
            "    /// {}-bit {}{} (FW reg `{}`)\n",
            r.bits, r.label, unit, r.fw_name
        ));
        out.push_str("    #[serde(skip_serializing_if = \"Option::is_none\")]\n");
        out.push_str(&format!("    pub {}: Option<u32>,\n", r.field));
    }
    out.push_str("}\n");

    // Board-level (global) writable register set. Each field maps to one
    // single-address user-register write (no channel stride). Currently
    // populated for the AMax debug FW's `ENABLE_ACQ` toggle.
    out.push_str(
        "\n/// AMax board-level (global) writable register set. Used for\n\
         /// firmware-wide toggles like the debug-FW `ENABLE_ACQ` switch.\n\
         #[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]\n\
         pub struct AMaxBoardConfig {\n",
    );
    for r in board_resolved {
        let unit = r
            .unit
            .as_deref()
            .map(|u| format!(", unit: {}", u))
            .unwrap_or_default();
        out.push_str(&format!(
            "    /// {}-bit {}{} (FW reg `{}`)\n",
            r.bits, r.label, unit, r.fw_name
        ));
        out.push_str("    #[serde(skip_serializing_if = \"Option::is_none\")]\n");
        out.push_str(&format!("    pub {}: Option<u32>,\n", r.field));
    }
    out.push_str("}\n");
    out
}

// ---- Rust registers emitter ----

fn emit_rust_registers(
    resolved: &[ResolvedReg],
    board_resolved: &[ResolvedReg],
    page_base: u32,
    page_stride: u32,
    broadcast_base: u32,
    channel_pages: u32,
    register_file: &std::path::Path,
) -> String {
    let mut out = String::new();
    out.push_str(&header_rust(
        register_file,
        std::path::Path::new("(addresses only)"),
    ));
    out.push_str(
        "//!\n\
         //! AMax custom-firmware register address map. Byte address =\n\
         //! `(PAGE_BASE + channel * PAGE_STRIDE + word_offset) * 4` per the\n\
         //! FELib `SetUserRegister` ABI used by `tools/amax_viewer/`.\n\
         \n",
    );
    out.push_str(&format!(
        "/// Channel-page base in word units (channel 0).\n\
         pub const PAGE_BASE: u32 = 0x{:X};\n\
         \n\
         /// Word stride between consecutive channel pages.\n\
         pub const PAGE_STRIDE: u32 = 0x{:X};\n\
         \n\
         /// Broadcast-page base in word units. A single write here fans out to\n\
         /// every channel in hardware. Auto-derived from RegisterFile.json;\n\
         /// `apply_amax_channel_config` mirrors the per-channel writes here.\n\
         pub const BROADCAST_BASE: u32 = 0x{:X};\n\
         \n\
         /// Number of per-channel pages this firmware defines (`ch0`..=\n\
         /// `CHANNEL_PAGES - 1`); `0` when the FW has no per-channel pages.\n\
         ///\n\
         /// Auto-derived from the same RegisterFile.json as the bases above, so\n\
         /// it cannot drift from them. `apply_amax_channel_config` clamps its\n\
         /// write loop to this: the digitizer JSON's `num_channels` is\n\
         /// hand-maintained and does NOT track FW rebuilds, and writing outside\n\
         /// the FW's own address map is how the AMax firmware gets corrupted.\n\
         pub const CHANNEL_PAGES: u32 = {};\n\
         \n",
        page_base, page_stride, broadcast_base, channel_pages
    ));
    out.push_str(
        "// ---- Per-channel register offsets (word, relative to the channel page base) ----\n\n",
    );
    for r in resolved {
        out.push_str(&format!("/// {}-bit {}\n", r.bits, r.label));
        out.push_str(&format!(
            "pub const REG_{}: u32 = 0x{:X};\n",
            r.field.to_uppercase(),
            r.word_offset
        ));
    }
    out.push_str(
        "\n\
         /// Compute the FELib byte address for a per-channel register.\n\
         #[inline]\n\
         pub fn channel_register_byte_addr(channel: u8, word_offset: u32) -> u32 {\n\
         \x20\x20\x20\x20let word_addr = PAGE_BASE + (channel as u32) * PAGE_STRIDE + word_offset;\n\
         \x20\x20\x20\x20word_addr * 4\n\
         }\n\
         \n\
         /// Compute the FELib byte address for a broadcast-page register. A\n\
         /// single write here fans out to all channels in hardware.\n\
         #[inline]\n\
         pub fn broadcast_register_byte_addr(word_offset: u32) -> u32 {\n\
         \x20\x20\x20\x20(BROADCAST_BASE + word_offset) * 4\n\
         }\n",
    );

    // Codegen-driven `apply` helper — handle.rs iterates this list so the
    // hand-written field array doesn't go stale every time the FW gains or
    // drops a register. Each tuple is (REG_offset, value, field_name).
    out.push_str("\n/// All writable per-channel register fields, in stable order.\n");
    out.push_str("/// Used by `apply_amax_channel_config` to drive `set_user_register` calls.\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("pub fn channel_writes(\n");
    out.push_str("    config: &crate::config::digitizer::AMaxChannelConfig,\n");
    out.push_str(") -> Vec<(u32, u32, &'static str)> {\n");
    out.push_str("    let mut writes = Vec::new();\n");
    for r in resolved {
        out.push_str(&format!(
            "    if let Some(v) = config.{f} {{ writes.push((REG_{u}, v, \"{f}\")); }}\n",
            f = r.field,
            u = r.field.to_uppercase(),
        ));
    }
    out.push_str("    writes\n");
    out.push_str("}\n");

    // Codegen-driven override/default merge. handle.rs used to hand-enumerate
    // every field here, which broke the build whenever the FW added or dropped
    // a register (a struct literal must list exactly the current fields). Emit
    // the merge from the canonical list so `apply_amax_channel_config` calls one
    // function and never touches the field set again.
    out.push_str("\n/// Merge a per-channel `override` config over `defaults`, field by field.\n");
    out.push_str(
        "/// Codegen-driven so adding/removing an AMax register needs no handle.rs edit.\n",
    );
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("pub fn merge_amax_channel_config(\n");
    out.push_str("    override_cfg: Option<&crate::config::digitizer::AMaxChannelConfig>,\n");
    out.push_str("    defaults: Option<&crate::config::digitizer::AMaxChannelConfig>,\n");
    out.push_str(") -> crate::config::digitizer::AMaxChannelConfig {\n");
    out.push_str("    crate::config::digitizer::AMaxChannelConfig {\n");
    for r in resolved {
        out.push_str(&format!(
            "        {f}: override_cfg.and_then(|c| c.{f}).or_else(|| defaults.and_then(|c| c.{f})),\n",
            f = r.field,
        ));
    }
    out.push_str("    }\n");
    out.push_str("}\n");

    // ---- Board-level (global) register addresses + apply helper ----
    //
    // Always emit `board_writes` and `all_board_registers` (even when empty)
    // so callers can iterate unconditionally. BOARD_REG_* constants only
    // appear when `board_resolved` is non-empty.
    if !board_resolved.is_empty() {
        out.push_str("\n// ---- Board-level (global) register byte addresses ----\n//\n");
        out.push_str(
            "// These registers live outside the per-channel `page_amax_energy_*`\n\
             // family — they're firmware-wide toggles. Byte addresses are taken\n\
             // verbatim from RegisterFile.json (no PAGE_BASE math).\n\n",
        );
        for r in board_resolved {
            out.push_str(&format!(
                "/// {}-bit {} (FW reg `{}`)\n",
                r.bits, r.label, r.fw_name
            ));
            out.push_str(&format!(
                "pub const BOARD_REG_{}: u32 = 0x{:X};\n",
                r.field.to_uppercase(),
                r.word_offset
            ));
        }
    }

    out.push_str(
        "\n/// All writable board-level register fields, in stable order.\n\
         /// Mirror of `channel_writes` for `AMaxBoardConfig`. Each tuple is\n\
         /// (BOARD_REG byte address, value, field_name). Empty when no\n\
         /// board-level (global) registers are declared in fw_params.json.\n\
         #[allow(dead_code)]\n\
         pub fn board_writes(\n\
         \x20\x20\x20\x20_config: &crate::config::digitizer::AMaxBoardConfig,\n\
         ) -> Vec<(u32, u32, &'static str)> {\n\
         \x20\x20\x20\x20#[allow(unused_mut)]\n\
         \x20\x20\x20\x20let mut writes = Vec::new();\n",
    );
    for r in board_resolved {
        out.push_str(&format!(
            "    if let Some(v) = _config.{f} {{ writes.push((BOARD_REG_{u}, v, \"{f}\")); }}\n",
            f = r.field,
            u = r.field.to_uppercase(),
        ));
    }
    out.push_str("    writes\n}\n");

    out.push_str(
        "\n/// All board-level register (byte_addr, name) pairs, in stable order.\n\
         /// Used by the operator UI's Tune Up debug view to read live\n\
         /// values back from the digitizer (see\n\
         /// `ReadLoopRequest::ReadAmaxBoardRegisters` in `reader/mod.rs`).\n\
         /// Empty when no board-level registers are declared.\n\
         #[allow(dead_code)]\n\
         pub fn all_board_registers() -> Vec<(u32, &'static str)> {\n\
         \x20\x20\x20\x20vec![\n",
    );
    for r in board_resolved {
        out.push_str(&format!(
            "        (BOARD_REG_{u}, \"{f}\"),\n",
            f = r.field,
            u = r.field.to_uppercase(),
        ));
    }
    out.push_str("    ]\n}\n");

    // Lightweight test: every offset distinct, byte-addr math stays consistent.
    out.push_str(
        "\n\
         #[cfg(test)]\n\
         mod tests {\n\
         \x20\x20\x20\x20use super::*;\n\
         \n\
         \x20\x20\x20\x20#[test]\n\
         \x20\x20\x20\x20fn register_offsets_are_distinct() {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20let offsets = [\n",
    );
    for r in resolved {
        out.push_str(&format!(
            "\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20REG_{},\n",
            r.field.to_uppercase()
        ));
    }
    out.push_str(
        "\x20\x20\x20\x20\x20\x20\x20\x20];\n\
         \x20\x20\x20\x20\x20\x20\x20\x20let mut sorted = offsets.to_vec();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20sorted.sort_unstable();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20sorted.dedup();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20assert_eq!(sorted.len(), offsets.len());\n\
         \x20\x20\x20\x20}\n\
         \n\
         \x20\x20\x20\x20#[test]\n\
         \x20\x20\x20\x20fn channel_addr_math() {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20// ch0 base byte addr = PAGE_BASE * 4 (word→byte).\n\
         \x20\x20\x20\x20\x20\x20\x20\x20assert_eq!(channel_register_byte_addr(0, 0), PAGE_BASE * 4);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20// Each channel index advances by PAGE_STRIDE words (=4× bytes).\n\
         \x20\x20\x20\x20\x20\x20\x20\x20assert_eq!(\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20channel_register_byte_addr(1, 0),\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20channel_register_byte_addr(0, 0) + PAGE_STRIDE * 4\n\
         \x20\x20\x20\x20\x20\x20\x20\x20);\n\
         \x20\x20\x20\x20}\n\
         }\n",
    );
    out
}

// ---- TypeScript emitter ----

fn emit_typescript(
    resolved: &[ResolvedReg],
    board_resolved: &[ResolvedReg],
    register_file: &std::path::Path,
    params_file: &std::path::Path,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "// AUTO-GENERATED by `cargo run --bin amax_codegen`. Do not edit by hand.\n\
         //\n\
         // Sources:\n\
         // - {}\n\
         // - {}\n\
         \n\
         import type {{ ChannelParamDef }} from '../components/channel-table/channel-table.component';\n\
         \n\
         export interface AMaxChannelConfig {{\n",
        register_file.display(),
        params_file.display()
    ));
    for r in resolved {
        out.push_str(&format!("  /** {}-bit {} */\n", r.bits, r.label));
        out.push_str(&format!("  {}?: number;\n", r.field));
    }
    out.push_str("}\n\n");

    // FW defaults table for the "Reset AMax to FW defaults" UI button.
    out.push_str("/** Per-key default values from `fw_params.json` (FW developer defaults). */\n");
    out.push_str("export const AMAX_DEFAULTS: Record<string, number> = {\n");
    for r in resolved {
        out.push_str(&format!("  'amax.{}': {},\n", r.field, r.default));
    }
    out.push_str("};\n\n");

    // Dotted-key allowlist for `digitizer.service.ts` expand/compress —
    // every per-channel AMax field the UI is allowed to round-trip. Adding
    // a new register here used to require a hand edit; emitting it from the
    // same canonical list keeps Settings tab in sync with the FW.
    out.push_str("/** All AMax dotted-path keys (`amax.<field>`). Used by the\n");
    out.push_str(" *  Settings expand/compress logic in `digitizer.service.ts`. */\n");
    out.push_str("export const AMAX_DOTTED_KEYS: readonly string[] = [\n");
    for r in resolved {
        out.push_str(&format!("  'amax.{}',\n", r.field));
    }
    out.push_str("];\n\n");

    // Emit one const array per category present in `fw_params.json`.
    //
    // Auto-discovery: collect every category appearing on a resolved register,
    // sorted by `category_rank` (well-known ones first, in operator-pipeline
    // order, then any new category alphabetically). This means a new FW that
    // adds e.g. `"category": "coincidence"` to a param will start surfacing
    // in the codegen output without a manual edit here.
    let mut categories: Vec<String> = resolved
        .iter()
        .map(|r| r.category.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    categories.sort_by(|a, b| {
        category_rank(a)
            .cmp(&category_rank(b))
            .then_with(|| a.cmp(b))
    });

    // Stable list of category names so the UI can iterate the available
    // tabs without re-encoding the list on the TS side. Mirrors the
    // const-array set emitted just below.
    out.push_str("/** Channel-param categories declared by `fw_params.json`.\n");
    out.push_str(" *  Mirrors the `AMAX_<CATEGORY>_PARAMS` arrays below; UI can\n");
    out.push_str(" *  iterate this list to render any future debug / housekeeping\n");
    out.push_str(" *  tabs without an additional codegen edit. */\n");
    out.push_str("export const AMAX_PARAM_CATEGORIES: readonly string[] = [\n");
    for cat in &categories {
        out.push_str(&format!("  '{}',\n", cat));
    }
    out.push_str("];\n\n");

    for cat in &categories {
        let const_name = format!("AMAX_{}_PARAMS", category_label(cat).to_uppercase());
        out.push_str(&format!(
            "export const {}: ChannelParamDef[] = [\n",
            const_name
        ));
        // Within a category, honour an explicit `ui_order` when present,
        // otherwise fall back to address (word_offset) order. Registers
        // without `ui_order` keep their address-based position, so an
        // override value only needs to be picked relative to its neighbours.
        let mut cat_regs: Vec<&ResolvedReg> =
            resolved.iter().filter(|r| &r.category == cat).collect();
        cat_regs.sort_by_key(|r| r.ui_order.unwrap_or(i64::from(r.word_offset)));
        for r in cat_regs {
            // Hex address tooltip: original FW name + page-relative word offset
            // + canonical ch0 word address (0x800000-base, what amax_viewer uses).
            let ch0_word = 0x800000 + r.word_offset;
            let tooltip = format!(
                "FW reg {} • word 0x{:02X} (ch0 @ 0x{:06X})",
                r.fw_name, r.word_offset, ch0_word
            );
            let mut line = format!("  {{ key: 'amax.{}', label: {:?}", r.field, r.label);
            if r.ty == "enum" {
                let opts = r
                    .options
                    .iter()
                    .map(|o| format!("{:?}", o))
                    .collect::<Vec<_>>()
                    .join(", ");
                // AMax enums map 1:1 to firmware register bits, so the value
                // must reach the backend as a Number — see channel-table's
                // `coerceEnum` helper which honours `numeric: true`.
                line.push_str(&format!(
                    ", type: 'enum', options: [{}], numeric: true",
                    opts
                ));
            } else {
                line.push_str(", type: 'number'");
                if let Some(u) = &r.unit {
                    line.push_str(&format!(", unit: {:?}", u));
                }
                line.push_str(&format!(", min: 0, max: {}, step: 1", r.ui_max));
            }
            line.push_str(&format!(", tooltip: {:?}", tooltip));
            line.push_str(" },\n");
            out.push_str(&line);
        }
        out.push_str("];\n\n");
    }

    // Category → params map so the operator UI can build its AMax tab set by
    // iterating `AMAX_PARAM_CATEGORIES` instead of a hand-wired per-category
    // switch. A new fw_params category surfaces a new tab with no TS edit.
    out.push_str("/** Maps each AMax category name to its params array. The UI\n");
    out.push_str(" *  iterates `AMAX_PARAM_CATEGORIES` over this map to render tabs,\n");
    out.push_str(" *  so a new firmware category auto-appears with no hand-edit. */\n");
    out.push_str("export const AMAX_PARAMS_BY_CATEGORY: Record<string, ChannelParamDef[]> = {\n");
    for cat in &categories {
        let const_name = format!("AMAX_{}_PARAMS", category_label(cat).to_uppercase());
        out.push_str(&format!("  '{}': {},\n", cat, const_name));
    }
    out.push_str("};\n\n");

    // ---- Board-level (global) interface, defaults, and params ----
    out.push_str("/** AMax board-level (global) writable register set —\n");
    out.push_str(" *  firmware-wide toggles like the debug-FW `ENABLE_ACQ`. */\n");
    out.push_str("export interface AMaxBoardConfig {\n");
    for r in board_resolved {
        out.push_str(&format!("  /** {}-bit {} */\n", r.bits, r.label));
        out.push_str(&format!("  {}?: number;\n", r.field));
    }
    out.push_str("}\n\n");

    out.push_str("/** Per-key default values for board-level registers. */\n");
    out.push_str("export const AMAX_BOARD_DEFAULTS: Record<string, number> = {\n");
    for r in board_resolved {
        out.push_str(&format!("  '{}': {},\n", r.field, r.default));
    }
    out.push_str("};\n\n");

    out.push_str("/** Board-level dotted-path keys (`amax.board.<field>`). */\n");
    out.push_str("export const AMAX_BOARD_DOTTED_KEYS: readonly string[] = [\n");
    for r in board_resolved {
        out.push_str(&format!("  'amax.board.{}',\n", r.field));
    }
    out.push_str("];\n\n");

    out.push_str("/** Board-level parameter UI definitions. */\n");
    out.push_str("export const AMAX_BOARD_PARAMS: ChannelParamDef[] = [\n");
    for r in board_resolved {
        let tooltip = format!("FW reg {} • addr 0x{:X}", r.fw_name, r.word_offset);
        let mut line = format!("  {{ key: 'amax.board.{}', label: {:?}", r.field, r.label);
        if r.ty == "enum" {
            let opts = r
                .options
                .iter()
                .map(|o| format!("{:?}", o))
                .collect::<Vec<_>>()
                .join(", ");
            line.push_str(&format!(
                ", type: 'enum', options: [{}], numeric: true",
                opts
            ));
        } else {
            line.push_str(", type: 'number'");
            if let Some(u) = &r.unit {
                line.push_str(&format!(", unit: {:?}", u));
            }
            line.push_str(&format!(", min: 0, max: {}, step: 1", r.ui_max));
        }
        line.push_str(&format!(", tooltip: {:?}", tooltip));
        line.push_str(" },\n");
        out.push_str(&line);
    }
    out.push_str("];\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name_or_path: &str) -> Register {
        // Two of the same string lets the test cover both `Path` and `Name`
        // alternates without two helpers.
        Register {
            address: 0,
            path: Some(name_or_path.to_string()),
            name: Some(name_or_path.to_string()),
        }
    }

    #[test]
    fn fw_key_handles_thirteen_may_era() {
        // 13may broadcast slash form
        assert_eq!(
            reg("page_amax_energy/POLARITY").fw_key().as_deref(),
            Some("POLARITY")
        );
        assert_eq!(
            reg("page_amax_energy/baseline_delay").fw_key().as_deref(),
            Some("baseline_delay")
        );
        // 13may per-channel slash form with `_4_<N>/` infix
        assert_eq!(
            reg("page_amax_energy_4_0/POLARITY").fw_key().as_deref(),
            Some("POLARITY")
        );
        assert_eq!(
            reg("page_amax_energy_4_31/THRS").fw_key().as_deref(),
            Some("THRS")
        );
        assert_eq!(
            reg("page_amax_energy_4_15/baseline_delay")
                .fw_key()
                .as_deref(),
            Some("baseline_delay")
        );
    }

    #[test]
    fn fw_key_handles_three_eras() {
        // 2026-03 era — slash separator after channel index
        assert_eq!(
            reg("page_amax_energy_0/THRS").fw_key().as_deref(),
            Some("THRS")
        );
        assert_eq!(
            reg("page_amax_energy_1/POLARITY").fw_key().as_deref(),
            Some("POLARITY")
        );

        // 2026-04 32-ch era — no channel index, just bare name
        assert_eq!(
            reg("page_amax_energy_THRS").fw_key().as_deref(),
            Some("THRS")
        );
        assert_eq!(
            reg("page_amax_energy_baseline_delay").fw_key().as_deref(),
            Some("baseline_delay")
        );

        // 2026-05 16-ch era — underscore-separated channel index
        assert_eq!(
            reg("page_amax_energy_15_THRS").fw_key().as_deref(),
            Some("THRS")
        );
        assert_eq!(
            reg("page_amax_energy_0_POLARITY").fw_key().as_deref(),
            Some("POLARITY")
        );
        assert_eq!(
            reg("page_amax_energy_7_baseline_delay").fw_key().as_deref(),
            Some("baseline_delay")
        );
    }

    #[test]
    fn fw_key_preserves_unprefixed_names() {
        // Global registers (no `page_amax_energy_` prefix) come through as-is.
        assert_eq!(reg("ENABLE_ACQ").fw_key().as_deref(), Some("ENABLE_ACQ"));
        assert_eq!(reg("AMAX_gol").fw_key().as_deref(), Some("AMAX_gol"));
    }

    #[test]
    fn fw_key_does_not_misclassify_underscore_names() {
        // Names starting with a non-digit followed by underscore (e.g.
        // `baseline_delay`) must NOT have their leading word stripped —
        // only digit prefixes get treated as channel indices.
        assert_eq!(
            reg("page_amax_energy_baseline_delay").fw_key().as_deref(),
            Some("baseline_delay")
        );
    }

    #[test]
    fn channel_index_handles_thirteen_may_era() {
        // 13may broadcast slash form — canonical (no index)
        assert_eq!(reg("page_amax_energy/POLARITY").channel_index(), None);
        assert_eq!(reg("page_amax_energy/baseline_delay").channel_index(), None);
        // 13may per-channel `_4_<N>/` form
        assert_eq!(
            reg("page_amax_energy_4_0/POLARITY").channel_index(),
            Some(0)
        );
        assert_eq!(reg("page_amax_energy_4_15/THRS").channel_index(), Some(15));
        assert_eq!(reg("page_amax_energy_4_31/THRS").channel_index(), Some(31));
    }

    #[test]
    fn is_per_channel_recognises_broadcast_slash_form() {
        // 13may broadcast slash form must be considered "per-channel" so the
        // canonical filter picks it up as the broadcast (None index) entry.
        assert!(reg("page_amax_energy/POLARITY").is_per_channel());
        assert!(reg("page_amax_energy/baseline_delay").is_per_channel());
        assert!(reg("page_amax_energy_4_0/POLARITY").is_per_channel());
    }

    #[test]
    fn channel_index_distinguishes_broadcast_from_per_channel() {
        // 2026-03 slash form
        assert_eq!(reg("page_amax_energy_0/THRS").channel_index(), Some(0));
        assert_eq!(reg("page_amax_energy_15/THRS").channel_index(), Some(15));

        // 2026-05 underscore form
        assert_eq!(reg("page_amax_energy_0_THRS").channel_index(), Some(0));
        assert_eq!(reg("page_amax_energy_15_THRS").channel_index(), Some(15));

        // Broadcast page (no index) — None means "this is the canonical entry"
        assert_eq!(reg("page_amax_energy_THRS").channel_index(), None);
        assert_eq!(reg("page_amax_energy_baseline_delay").channel_index(), None);

        // Non-page_amax_energy_ registers — None
        assert_eq!(reg("ENABLE_ACQ").channel_index(), None);
        assert_eq!(reg("AMAX_gol").channel_index(), None);
    }

    #[test]
    fn is_per_channel_matches_page_amax_energy_prefix() {
        assert!(reg("page_amax_energy_THRS").is_per_channel());
        assert!(reg("page_amax_energy_0_THRS").is_per_channel());
        assert!(reg("page_amax_energy_15_baseline_delay").is_per_channel());
        assert!(reg("page_amax_energy_0/THRS").is_per_channel()); // legacy slash form
        assert!(!reg("ENABLE_ACQ").is_per_channel());
        assert!(!reg("AMAX_gol").is_per_channel());
        assert!(!reg("/ENABLE_ACQ").is_per_channel());
    }

    // -----------------------------------------------------------------------
    // Integration tests: drive `select_canonical_per_channel` against
    // synthetic register lists representative of each FW era. These pin the
    // filter behaviour so future FW revisions that introduce a new naming
    // scheme don't silently break codegen output.
    // -----------------------------------------------------------------------

    /// Helper: build a Register with a specific address (so we can assert on
    /// which entry "won" when broadcast and per-channel both exist).
    fn reg_at(addr: u32, name_or_path: &str) -> Register {
        Register {
            address: addr,
            path: Some(name_or_path.to_string()),
            name: Some(name_or_path.to_string()),
        }
    }

    /// Sort + project the result to (addr, name) tuples for stable assertions.
    fn picked(regs: &[&Register]) -> Vec<(u32, String)> {
        let mut out: Vec<(u32, String)> = regs
            .iter()
            .map(|r| {
                (
                    r.address,
                    r.path.clone().or(r.name.clone()).unwrap_or_default(),
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn canonical_filter_2026_03_era_keeps_ch0_only() {
        // 2026-03 era: per-channel pages with slash separator. NO broadcast
        // page — the filter must fall back to ch0 (lowest index) so the
        // generated AMaxChannelConfig has every register.
        let regs = vec![
            reg_at(0x800002, "page_amax_energy_0/POLARITY"),
            reg_at(0x800004, "page_amax_energy_0/THRS"),
            reg_at(0x840002, "page_amax_energy_1/POLARITY"),
            reg_at(0x840004, "page_amax_energy_1/THRS"),
            reg_at(0x880002, "page_amax_energy_2/POLARITY"),
            reg_at(0x880004, "page_amax_energy_2/THRS"),
            // Sibling globals (not per-channel) must be filtered out.
            reg_at(0x80008, "ENABLE_ACQ"),
            reg_at(0x80007, "AMAX_gol"),
        ];
        let picked = picked(&select_canonical_per_channel(&regs));
        assert_eq!(
            picked,
            vec![
                (0x800002, "page_amax_energy_0/POLARITY".to_string()),
                (0x800004, "page_amax_energy_0/THRS".to_string()),
            ],
            "2026-03 era: ch0 wins, ch1+ + globals dropped"
        );
    }

    #[test]
    fn canonical_filter_2026_04_era_keeps_bare_names() {
        // 2026-04 32-ch era: a single broadcast page with no channel index.
        // No per-channel pages exist at all — the filter keeps everything
        // that lives under `page_amax_energy_*` and isn't otherwise.
        let regs = vec![
            reg_at(0x100002, "page_amax_energy_POLARITY"),
            reg_at(0x100004, "page_amax_energy_THRS"),
            reg_at(0x100006, "page_amax_energy_baseline_delay"),
            // Globals filtered out.
            reg_at(0x80008, "ENABLE_ACQ"),
        ];
        let picked = picked(&select_canonical_per_channel(&regs));
        assert_eq!(
            picked,
            vec![
                (0x100002, "page_amax_energy_POLARITY".to_string()),
                (0x100004, "page_amax_energy_THRS".to_string()),
                (0x100006, "page_amax_energy_baseline_delay".to_string()),
            ],
            "2026-04 era: all bare-name broadcast entries kept"
        );
    }

    #[test]
    fn canonical_filter_2026_05_caenlist_13may_picks_broadcast_drops_per_channel() {
        // 13may caenlist era: broadcast page lives at `page_amax_energy/<NAME>`
        // (slash form, no channel index) and per-channel pages at
        // `page_amax_energy_4_<N>/<NAME>`. Broadcast wins; the 32 per-channel
        // duplicates must drop so codegen emits one REG_* per param.
        let mut regs = vec![
            reg_at(0x200, "page_amax_energy/POLARITY"),
            reg_at(0x203, "page_amax_energy/THRS"),
            reg_at(0x211, "page_amax_energy/baseline_delay"),
        ];
        for ch in 0..32u32 {
            regs.push(reg_at(
                0x4001 + ch * 0x200,
                &format!("page_amax_energy_4_{ch}/POLARITY"),
            ));
            regs.push(reg_at(
                0x4003 + ch * 0x200,
                &format!("page_amax_energy_4_{ch}/THRS"),
            ));
        }
        let picked = picked(&select_canonical_per_channel(&regs));
        assert_eq!(
            picked,
            vec![
                (0x200, "page_amax_energy/POLARITY".to_string()),
                (0x203, "page_amax_energy/THRS".to_string()),
                (0x211, "page_amax_energy/baseline_delay".to_string()),
            ],
            "13may era: 3 broadcast entries kept; 64 per-channel duplicates dropped"
        );
    }

    #[test]
    fn canonical_filter_2026_05_era_prefers_broadcast_drops_per_channel() {
        // 2026-05 16-ch era: broadcast page (no index) + 16 per-channel
        // pages (`_<N>_<NAME>`). Broadcast wins; per-channel duplicates
        // are dropped to avoid emitting 16 duplicate `REG_*` constants.
        let mut regs = vec![
            // The broadcast (canonical) entries.
            reg_at(0x100002, "page_amax_energy_POLARITY"),
            reg_at(0x100004, "page_amax_energy_THRS"),
            // Globals filtered out.
            reg_at(0x80008, "ENABLE_ACQ"),
        ];
        // 16 per-channel duplicates that must NOT win over the broadcast.
        for ch in 0..16u32 {
            regs.push(reg_at(
                0x200000 + ch * 0x40000,
                &format!("page_amax_energy_{ch}_POLARITY"),
            ));
            regs.push(reg_at(
                0x200004 + ch * 0x40000,
                &format!("page_amax_energy_{ch}_THRS"),
            ));
        }

        let picked = picked(&select_canonical_per_channel(&regs));
        assert_eq!(
            picked,
            vec![
                (0x100002, "page_amax_energy_POLARITY".to_string()),
                (0x100004, "page_amax_energy_THRS".to_string()),
            ],
            "2026-05 era: only the 2 broadcast entries kept; 32 per-channel duplicates dropped"
        );
    }

    #[test]
    fn canonical_filter_does_not_double_count_register_names() {
        // Cross-era invariant: after filtering, every distinct fw_key must
        // appear exactly once. If this regresses, the codegen would emit
        // duplicate `REG_*` constants and the channel-page filter would
        // be silently wrong.
        let regs = vec![
            reg_at(0x100002, "page_amax_energy_POLARITY"),
            reg_at(0x200002, "page_amax_energy_0_POLARITY"), // 2026-05 ch0 dup
            reg_at(0x300002, "page_amax_energy_1_POLARITY"), // 2026-05 ch1 dup
            reg_at(0x100004, "page_amax_energy_THRS"),
            reg_at(0x200004, "page_amax_energy_0_THRS"),
        ];
        let canonical = select_canonical_per_channel(&regs);
        let mut keys: Vec<String> = canonical.iter().filter_map(|r| r.fw_key()).collect();
        keys.sort();
        let unique = {
            let mut v = keys.clone();
            v.dedup();
            v
        };
        assert_eq!(keys, unique, "every fw_key must appear exactly once");
        assert_eq!(keys, vec!["POLARITY", "THRS"]);
    }

    #[test]
    fn canonical_filter_handles_empty_input() {
        let picked = picked(&select_canonical_per_channel(&[]));
        assert!(picked.is_empty());
    }

    #[test]
    fn canonical_filter_handles_globals_only() {
        // Boards with no `page_amax_energy_*` registers (e.g. a stripped
        // FW dump for ENABLE_ACQ probing) should yield zero per-channel
        // entries — globals are picked up by the `is_board_level` path.
        let regs = vec![reg_at(0x80008, "ENABLE_ACQ"), reg_at(0x80007, "AMAX_gol")];
        assert!(select_canonical_per_channel(&regs).is_empty());
    }

    // -----------------------------------------------------------------------
    // Address-map auto-derivation (`derive_layout` + safety guard).
    // -----------------------------------------------------------------------

    /// Build a synthetic 13may/17june-shape register list: a broadcast page at
    /// `bc_base` plus `nch` per-channel pages at `pc_base` (stride `stride`),
    /// each page carrying the same two registers at in-page offsets 0 and 3.
    fn thirteen_may_shape(bc_base: u32, pc_base: u32, stride: u32, nch: u32) -> Vec<Register> {
        let mut regs = vec![
            reg_at(bc_base, "page_amax_energy/POLARITY"),
            reg_at(bc_base + 3, "page_amax_energy/THRS"),
        ];
        for ch in 0..nch {
            regs.push(reg_at(
                pc_base + ch * stride,
                &format!("page_amax_energy_4_{ch}/POLARITY"),
            ));
            regs.push(reg_at(
                pc_base + ch * stride + 3,
                &format!("page_amax_energy_4_{ch}/THRS"),
            ));
        }
        regs
    }

    #[test]
    fn derive_layout_default_prefers_ch0_when_present() {
        // 17June shape: broadcast @ 0x200, per-channel ch0 @ 0x8000 stride 0x200.
        let regs = thirteen_may_shape(0x200, 0x8000, 0x200, 32);
        let l = derive_layout(&regs, None);
        assert_eq!(l.page_base, 0x8000, "ch0 page is canonical");
        assert_eq!(l.page_stride, 0x200);
        assert_eq!(l.broadcast_base, 0x200);
        assert_eq!(l.channel_pages, 32);
        assert!(l.prefer_per_channel);
    }

    // -----------------------------------------------------------------------
    // 2026-06 custom-width era: `page_amax_energy_<group>_<ch>_<NAME>` —
    // underscore-separated, a group index (`1`) prepended before the channel,
    // and NO broadcast page. Regression for the gant build failure where the
    // group index was mistaken for the channel, dropping every register.
    // -----------------------------------------------------------------------

    #[test]
    fn fw_key_handles_custom_width_era() {
        assert_eq!(
            reg("page_amax_energy_1_0_POLARITY").fw_key().as_deref(),
            Some("POLARITY")
        );
        assert_eq!(
            reg("page_amax_energy_1_3_THRS").fw_key().as_deref(),
            Some("THRS")
        );
        // register name with its own underscores must survive intact
        assert_eq!(
            reg("page_amax_energy_1_2_baseline_delay")
                .fw_key()
                .as_deref(),
            Some("baseline_delay")
        );
        assert_eq!(
            reg("page_amax_energy_1_0_select_width").fw_key().as_deref(),
            Some("select_width")
        );
    }

    #[test]
    fn channel_index_handles_custom_width_era() {
        // The channel is the SECOND number (the `1` is the group prefix).
        assert_eq!(
            reg("page_amax_energy_1_0_POLARITY").channel_index(),
            Some(0)
        );
        assert_eq!(
            reg("page_amax_energy_1_1_POLARITY").channel_index(),
            Some(1)
        );
        assert_eq!(reg("page_amax_energy_1_3_THRS").channel_index(), Some(3));
        assert_eq!(
            reg("page_amax_energy_1_2_baseline_delay").channel_index(),
            Some(2)
        );
    }

    /// 2026-06 custom-width shape: `page_amax_energy_1_<ch>_<NAME>`, channels
    /// 0..nch, page base 0x100000, stride 0x40000, NO broadcast page.
    fn custom_width_shape(nch: u32) -> Vec<Register> {
        let mut regs = Vec::new();
        for ch in 0..nch {
            let base = 0x100000 + ch * 0x40000;
            regs.push(reg_at(
                base,
                &format!("page_amax_energy_1_{ch}_PRETRIGGER_INPUT"),
            ));
            regs.push(reg_at(
                base + 1,
                &format!("page_amax_energy_1_{ch}_POLARITY"),
            ));
            regs.push(reg_at(base + 3, &format!("page_amax_energy_1_{ch}_THRS")));
        }
        regs
    }

    #[test]
    fn derive_layout_custom_width_era() {
        let regs = custom_width_shape(4);
        let l = derive_layout(&regs, None);
        assert!(
            l.prefer_per_channel,
            "ch0 page present → per-channel canonical"
        );
        assert_eq!(l.page_base, 0x100000, "ch0 base");
        assert_eq!(l.page_stride, 0x40000, "ch1 - ch0");
        assert_eq!(
            l.broadcast_base, 0x100000,
            "no broadcast page → falls back to page_base"
        );
        assert_eq!(l.channel_pages, 4);
    }

    #[test]
    fn derive_layout_channel_pages_tracks_the_fw_channel_count() {
        // The 2026-08 regression: the same FW family rebuilt for 2 channels.
        // `num_channels` in the digitizer JSON does not follow a FW rebuild, so
        // `CHANNEL_PAGES` is what stops a stale 32-channel config from writing
        // 30 channels' worth of registers into unmapped address space.
        assert_eq!(derive_layout(&custom_width_shape(2), None).channel_pages, 2);
        assert_eq!(
            derive_layout(&custom_width_shape(32), None).channel_pages,
            32
        );
    }

    #[test]
    fn derive_layout_channel_pages_reports_the_span_not_the_count() {
        // A sparse index set (ch0, ch1, ch4) must report 5, never 3: clamping
        // to a count would silently stop configuring a page that does exist.
        // `main` warns separately when it sees a gap.
        let mut regs = custom_width_shape(2);
        regs.push(reg_at(
            0x100000 + 4 * 0x40000,
            "page_amax_energy_1_4_POLARITY",
        ));
        assert_eq!(derive_layout(&regs, None).channel_pages, 5);
    }

    #[test]
    fn canonical_filter_custom_width_keeps_ch0_only() {
        // ch0 group kept, ch1..3 dropped, every fw_key appears exactly once.
        let regs = custom_width_shape(4);
        let canonical = select_canonical_per_channel(&regs);
        let mut keys: Vec<String> = canonical.iter().filter_map(|r| r.fw_key()).collect();
        keys.sort();
        assert_eq!(keys, vec!["POLARITY", "PRETRIGGER_INPUT", "THRS"]);
    }

    #[test]
    fn derive_layout_override_forces_broadcast() {
        let regs = thirteen_may_shape(0x200, 0x8000, 0x200, 4);
        let l = derive_layout(&regs, Some(false));
        assert!(!l.prefer_per_channel);
        assert_eq!(
            l.page_base, 0x200,
            "broadcast page is canonical under override"
        );
        // Stride is still measured from the per-channel pages.
        assert_eq!(l.page_stride, 0x200);
        assert_eq!(l.broadcast_base, 0x200);
    }

    #[test]
    fn derive_layout_broadcast_only_era() {
        // 2026-04 32-ch era: single broadcast page, no per-channel pages.
        let regs = vec![
            reg_at(0x100002, "page_amax_energy_POLARITY"),
            reg_at(0x100004, "page_amax_energy_THRS"),
        ];
        let l = derive_layout(&regs, None);
        // No per-channel page span at all. `apply_amax_channel_config` reads
        // this 0 as "nothing to clamp against", NOT as "write nothing".
        assert_eq!(l.channel_pages, 0);
        assert!(!l.prefer_per_channel, "no ch0 page → broadcast canonical");
        assert_eq!(l.page_base, 0x100002);
        assert_eq!(l.page_stride, 0, "no per-channel pages → stride 0");
        assert_eq!(l.broadcast_base, 0x100002);
    }

    #[test]
    fn safety_guard_passes_on_aligned_layout() {
        let regs = thirteen_may_shape(0x200, 0x8000, 0x200, 32);
        assert!(assert_broadcast_layout_matches(&regs, 0x8000, 0x200).is_ok());
    }

    #[test]
    fn safety_guard_rejects_divergent_offsets() {
        // THRS sits at broadcast offset 5 but ch0 offset 3 → corruption risk.
        let regs = vec![
            reg_at(0x200, "page_amax_energy/POLARITY"),
            reg_at(0x8000, "page_amax_energy_4_0/POLARITY"),
            reg_at(0x205, "page_amax_energy/THRS"),
            reg_at(0x8003, "page_amax_energy_4_0/THRS"),
        ];
        let err = assert_broadcast_layout_matches(&regs, 0x8000, 0x200);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("THRS"));
    }

    #[test]
    fn derive_layout_reproduces_17june_golden() {
        // Strongest guarantee: the real, hardware-verified 17June RegisterFile
        // must auto-derive to exactly the committed PAGE_BASE/STRIDE/BROADCAST.
        let text = include_str!("../../FW/20260617/RegisterFile_17june.json");
        let rf: RegisterFile = serde_json::from_str(text).expect("17June json parses");
        let l = derive_layout(&rf.registers, None);
        assert_eq!(l.page_base, 0x8000, "committed PAGE_BASE");
        assert_eq!(l.page_stride, 0x200, "committed PAGE_STRIDE");
        assert_eq!(l.broadcast_base, 0x200, "current handle.rs BROADCAST_BASE");
        assert!(l.prefer_per_channel);
        assert!(
            assert_broadcast_layout_matches(&rf.registers, l.page_base, l.broadcast_base).is_ok(),
            "17June broadcast/per-channel layout is consistent"
        );
    }
}
