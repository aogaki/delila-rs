// Component states matching Rust enum
export type ComponentState =
  | 'Idle'
  | 'Configuring'
  | 'Configured'
  | 'Arming'
  | 'Armed'
  | 'Starting'
  | 'Running'
  | 'Stopping'
  | 'Error';

// System-wide state
export type SystemState =
  | 'Idle'
  | 'Configuring'
  | 'Configured'
  | 'Arming'
  | 'Armed'
  | 'Starting'
  | 'Running'
  | 'Stopping'
  | 'Error'
  | 'Mixed'
  | 'Offline';

// Metrics for a component
export interface ComponentMetrics {
  events_processed: number;
  bytes_transferred: number;
  queue_size: number;
  queue_max: number;
  event_rate: number;
  trigger_loss_count?: number;
  trigger_loss_rate?: number;
  // Backlog gauge for Merger/Recorder unbounded channels (TODO 68)
  queue_bytes?: number;
  queue_bytes_peak?: number;
  /** 0 = ok, 1 = soft watermark exceeded, 2 = hard (drain-first stop) */
  backlog_level?: number;
}

// Status of a single component
export interface ComponentStatus {
  name: string;
  address: string;
  state: ComponentState;
  run_number?: number;
  metrics?: ComponentMetrics;
  error?: string;
  online: boolean;
  role: string;
}

// Run status
export type RunStatus = 'running' | 'completed' | 'error' | 'aborted';

// Run statistics
export interface RunStats {
  total_events: number;
  total_bytes: number;
  average_rate: number;
}

// Run note entry (append-only logbook style)
export interface RunNote {
  time: number; // UNIX timestamp in milliseconds
  text: string;
}

// Current run information
export interface CurrentRunInfo {
  run_number: number;
  exp_name: string;
  comment: string;
  start_time: number; // UNIX timestamp (ms)
  elapsed_secs: number;
  status: RunStatus;
  stats: RunStats;
  notes: RunNote[];
}

// Last run info for pre-filling comment field
export interface LastRunInfo {
  run_number: number;
  comment: string;
  notes: RunNote[];
}

// System-wide status
export interface SystemStatus {
  components: ComponentStatus[];
  system_state: SystemState;
  run_info?: CurrentRunInfo;
  /** Experiment name (server-authoritative, from config file) */
  experiment_name: string;
  /** Next run number (from MongoDB, for multi-client sync) */
  next_run_number?: number;
  /** Last run info for pre-filling comment (comment + notes from previous run) */
  last_run_info?: LastRunInfo;
  /** Whether Tune Up mode is active */
  tuneup_mode?: boolean;
  /** Digitizer ID being tuned (when tuneup_mode is true) */
  tuneup_digitizer_id?: number;
  /** Monitor HTTP port (for constructing Monitor API URL) */
  monitor_http_port?: number;
}

// Configure request
export interface ConfigureRequest {
  run_number: number;
  exp_name: string;
}

// API response
export interface ApiResponse {
  success: boolean;
  message: string;
}

// Button enable states based on system state
// Note: arm is removed from UI - backend auto-arms on start
export interface ButtonStates {
  configure: boolean;
  start: boolean;
  stop: boolean;
  reset: boolean;
}

// Firmware types for digitizer
// 'AMax' = DELILA custom MCA + amplitude-maximum FW for VX2730 (DPP_OPEN endpoint).
// 'PHA2' = CAEN DPP-PHA on VX2730 (RAW endpoint, trapezoidal-filter MCA).
export type FirmwareType =
  | 'PSD1'
  | 'PSD2'
  | 'PHA1'
  | 'PHA2'
  | 'X743Std'
  | 'AMax';

// Board-level configuration
export interface BoardConfig {
  start_source?: string;
  gpio_mode?: string;
  test_pulse_period?: number;
  test_pulse_width?: number;
  global_trigger_source?: string;
  record_length?: number;
  waveforms_enabled?: boolean;
  // Virtual Probes (PSD1/PHA1 only)
  vtrace_probe_0?: string; // Analog Probe 1
  vtrace_probe_1?: string; // Analog Probe 2
  vtrace_probe_2?: string; // Digital Probe 1
  vtrace_probe_3?: string; // Digital Probe 2
  // PSD1/PHA1 specific
  ext_trigger_enable?: string;
  sw_trigger_enable?: string;
  io_level?: string;
  ext_clock?: string;
  start_delay?: number;
  extras_enabled?: string;
  event_aggregation?: number;
  coinc_trgout?: number;
  fine_ts_mode?: string; // DIG1 only: "hardware" | "software"
  extra?: Record<string, unknown>;
}

// Channel configuration
export interface ChannelConfig {
  // --- Input ---
  enabled?: string;
  /**
   * Pulse polarity ("Positive" / "Negative" / "POLARITY_*"). NOT trigger edge.
   * X743Std: drives software-side waveform inversion in the decoder, and (when
   * trigger_edge is unset) is also used as a fallback to derive the trigger edge.
   */
  polarity?: string;
  /** Trigger edge ("Rising" / "Falling"). X743Std only — independent of polarity. */
  trigger_edge?: string;
  dc_offset?: number;
  vga_gain?: number;
  baseline_avg?: string;
  fixed_baseline?: number;
  record_length_ns?: number;
  pre_trigger_ns?: number;
  wave_downsampling?: string;
  input_dynamic?: string;
  coarse_gain?: string;
  // --- Trigger ---
  discriminator_mode?: string;
  trigger_threshold?: number;
  /**
   * X743Std only: trigger threshold in **input-referred volts** (-1.25..+1.25).
   * Backend accounts for DC offset when converting to the threshold DAC, so
   * users specify the threshold as it appears at the input — not the ADC.
   * Takes priority over `trigger_threshold` (DAC) when set.
   */
  trigger_threshold_v?: number;
  cfd_delay_ns?: number;
  cfd_fraction?: string;
  cfd_interpolation_point?: number; // DIG1 only: 0-3
  trigger_holdoff_ns?: number;
  smoothing_factor?: string;
  time_filter_smoothing?: string;
  input_smoothing?: string;
  fast_discr_smoothing?: string;
  input_rise_time_ns?: number;
  event_trigger_source?: string;
  wave_trigger_source?: string;
  self_trigger?: string;
  global_trigger_gen?: string;
  trigger_out_propagate?: string;
  // --- Energy ---
  energy_coarse_gain?: string;
  gate_long_ns?: number;
  gate_short_ns?: number;
  gate_pre_ns?: number;
  charge_pedestal?: number;
  short_charge_pedestal?: number;
  charge_smoothing?: string;
  charge_pedestal_en?: string;
  trap_rise_time_ns?: number;
  trap_flat_top_ns?: number;
  trap_pole_zero_ns?: number;
  peaking_time?: number;
  peak_nsmean?: string;
  peak_holdoff_ns?: number;
  energy_fine_gain?: number;
  // --- Coincidence ---
  ch_trigger_mask?: string;
  coincidence_mask?: string;
  anti_coincidence_mask?: string;
  coincidence_window_ns?: number;
  coincidence_mode?: string;
  ch_veto_source?: string;
  ch_veto_width_ns?: number;
  event_selector?: string;
  pileup_rejection?: string;
  // --- PSD1/PHA1 Extended Coincidence ---
  trigger_latency?: string;
  coinc_mask?: number;
  coinc_operation?: string;
  coinc_majority_level?: number;
  coinc_trgext?: string;
  coinc_trgsw?: string;
  pileup_gap?: number;
  pileup_counting_en?: string;
  pileup_flag_en?: string;
  // --- Waveform ---
  wave_saving?: string;
  analog_probe_0?: string;
  analog_probe_1?: string;
  digital_probe_0?: string;
  digital_probe_1?: string;
  digital_probe_2?: string;
  digital_probe_3?: string;
  // --- FirmwareType.PHA2 only: trapezoidal filter (separate from PHA1 trap_*) ---
  time_filter_rise_time_ns?: number;
  time_filter_retrigger_guard_ns?: number;
  energy_filter_rise_time_ns?: number;
  energy_filter_flat_top_ns?: number;
  energy_filter_pole_zero_ns?: number;
  energy_filter_peaking_position?: number; // 10-90 %
  energy_filter_peaking_avg?: string; // "LowAVG" | "MediumAVG" | "HighAVG"
  energy_filter_baseline_avg?: string; // "Fixed" | "VeryLow" | ... | "High"
  energy_filter_baseline_guard_ns?: number;
  energy_filter_pileup_guard_ns?: number;
  energy_filter_fine_gain?: number; // 1.000-10.000
  energy_filter_lf_limitation?: string; // "On" | "Off"
  /**
   * PHA2 only: per-channel S_IN behaviour. "None" (default) | "ResetTimestamp".
   * Avoid "ResetTimestamp" mid-run — it injects Run-to-Run timestamp variability
   * (see TODO/51 注意点 §Multi-board sync).
   */
  sin_function?: string;
  /** PHA2 only: per-channel GPI behaviour. Same allowed values as sin_function. */
  gpi_function?: string;
  // --- FirmwareType.AMax only: nested 24-field register block ---
  amax?: AMaxChannelConfig;
  // --- FW-specific overflow ---
  extra?: Record<string, unknown>;
}

// V1743 Standard Mode configuration (board-level, excluding connection params)
// Connection params (link_type/link_num/conet_node/vme_base_address) are JSON-only.
export interface X743Config {
  // Connection (read-only in UI; set via JSON)
  link_type?: string;
  link_num?: number;
  conet_node?: number;
  vme_base_address?: number;
  // SAM
  sampling_frequency?: string; // "3.2ghz" | "1.6ghz" | "800mhz" | "400mhz"
  correction_level?: string; // "all" | "pedestal_only" | "inl" | "disabled"
  // Acquisition
  record_length?: number;
  post_trigger_size?: number;
  max_num_events_blt?: number;
  // I/O
  io_level?: string; // "nim" | "ttl"
  trigger_source?: string; // "software" | "external" | "self"
  // Group enable (bitmask, bit i = group i, 2 ch/group)
  group_enable_mask?: number;
  // Test pulse
  pulse_gen_enabled?: boolean;
  pulse_pattern?: number;
  pulse_source?: string; // "software" | "continuous"
  // Decode (post-processing)
  fine_time_source?: string; // kept in JSON but hidden in UI (currently "cfd_soft")
  energy_source?: string; // "amplitude" (others not usable in Standard mode)
  energy_scale?: number;
  energy_offset?: number;
  save_waveform?: boolean;
  baseline_samples?: number;
  cfd_delay_samples?: number;
  cfd_fraction?: number;
  /**
   * TTF (Trigger and Timing Filter) smoothing — N-tap moving average applied
   * before baseline / software CFD. Mirrors WaveDemo's TTF_SMOOTHING.
   * Backend serializes this as lowercase.
   */
  ttf_smoothing?: 'off' | 'n2' | 'n4' | 'n8' | 'n16';
  /**
   * Arbitrary register writes applied at the end of `apply_config_standard`.
   * Order is preserved; later entries override earlier writes to the same address.
   * Mirrors WaveDemo's WRITE_REGISTER escape hatch.
   */
  extra_registers?: RegisterWrite[];
}

/** Single arbitrary register write entry for X743Config.extra_registers. */
export interface RegisterWrite {
  /** 32-bit register address. UI displays/edits as hex; serialized as number or "0x..." string. */
  addr: number;
  /** 32-bit data word. Same encoding as `addr`. */
  data: number;
  /** Optional human-readable note (logged when applied). */
  comment?: string;
}

// AMax custom-firmware per-channel writable register set.
// The interface is auto-generated from RegisterFile.json + fw_params.json
// — see `cargo run --bin amax_codegen`. Re-exported here so existing
// imports (`./types`) keep working.
import type { AMaxBoardConfig, AMaxChannelConfig } from './amax-generated';
export type { AMaxBoardConfig, AMaxChannelConfig };

// Digitizer configuration
export interface DigitizerConfig {
  digitizer_id: number;
  name: string;
  firmware: FirmwareType;
  serial_number?: string;
  model?: string;
  num_channels: number;
  is_master?: boolean;
  sync?: unknown;
  board: BoardConfig;
  channel_defaults: ChannelConfig;
  channel_overrides?: Record<number, ChannelConfig>;
  channel_names?: Record<number, string>;
  // V1743 Standard mode only (firmware === 'X743Std')
  x743?: X743Config;
  // AMax board-level (global) registers — currently the debug-FW
  // `ENABLE_ACQ` toggle. Optional so legacy AMax configs keep working.
  amax_board?: AMaxBoardConfig;
}

// Detected digitizer from hardware probe
export interface DetectedDigitizer {
  component_name: string;
  source_id: number;
  device_info: Record<string, unknown>;
  config_found: boolean;
  config?: DigitizerConfig;
}

// Detect response from API
export interface DetectResponse {
  success: boolean;
  message: string;
  digitizers: DetectedDigitizer[];
}

// Emulator configuration (runtime settings)
export interface EmulatorConfig {
  events_per_batch: number;
  batch_interval_ms: number;
  enable_waveform: boolean;
  waveform_probes: number;
  waveform_samples: number;
  num_modules: number;
  channels_per_module: number;
}

// ============================================================================
// Event Builder Configuration Types
// ============================================================================

// Channel settings for Event Builder (ELIFANT-Event compatible)
export interface ChSettings {
  ID: number;
  Module: number;
  Channel: number;
  IsEventTrigger: boolean;
  ThresholdADC: number;
  HasAC: boolean;
  ACModule: number;
  ACChannel: number;
  DetectorType: string;
  Tags: string[];
  P0: number;
  P1: number;
  P2: number;
  P3: number;
}

// L2 Operators
export type L2Operator = '>' | '>=' | '<' | '<=' | '==' | '!=';
export type L2LogicalOperator = 'AND' | 'OR';

// L2 Setting types (Counter, Flag, Accept)
export type L2Setting =
  | { Type: 'Counter'; Name: string; Tags: string[] }
  | { Type: 'Flag'; Name: string; Monitor: string; Operator: L2Operator; Value: number }
  | { Type: 'Accept'; Name: string; Monitor: string[]; Operator: L2LogicalOperator };

// Time calibration settings
export interface TimeCalibration {
  ref_module: number;
  ref_channel: number;
  offsets: Record<string, number>;
}

// Event Builder configuration document
export interface EventBuilderConfig {
  id?: string;
  name: string;
  exp_name: string;
  version: number;
  created_at: string;
  created_by: string;
  description?: string;
  is_current: boolean;
  ch_settings: ChSettings[][];
  time_settings?: TimeCalibration;
  l2_settings?: L2Setting[];
  coincidence_window_ns: number;
  slice_duration_ns: number;
}

// Config history item
export interface EventBuilderHistoryItem {
  version: number;
  created_at: string;
  created_by: string;
  description?: string;
  is_current: boolean;
}

// Get button states based on system state
export function getButtonStates(state: SystemState): ButtonStates {
  switch (state) {
    case 'Idle':
      // Start enabled - backend does full Reset → Configure → Arm → Start
      return { configure: true, start: true, stop: false, reset: false };
    case 'Configured':
      // Start is enabled - backend will auto-arm
      return { configure: false, start: true, stop: false, reset: true };
    case 'Armed':
      return { configure: false, start: true, stop: false, reset: true };
    case 'Running':
      return { configure: false, start: false, stop: true, reset: false };
    case 'Error':
      return { configure: false, start: false, stop: false, reset: true };
    default:
      // Transitional states - all disabled
      return { configure: false, start: false, stop: false, reset: false };
  }
}
