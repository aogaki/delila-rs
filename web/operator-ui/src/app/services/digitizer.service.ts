import { Injectable, computed, inject, signal } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import {
  DigitizerConfig,
  ChannelConfig,
  ApiResponse,
  DetectResponse,
} from '../models/types';
import {
  AMAX_BOARD_DEFAULTS,
  AMAX_BOARD_DOTTED_KEYS,
  AMAX_DOTTED_KEYS,
  AMaxBoardConfig,
} from '../models/amax-generated';
import { firstValueFrom } from 'rxjs';

/**
 * Keys in `ChannelConfig` that the expand/compress round-trip iterates
 * over. **MUST** stay in sync with the `ChannelConfig` interface in
 * `types.ts` for every flat (non-`amax.*`) channel parameter — any key
 * present in the type but missing here is silently dropped on Apply
 * (regression caught 2026-05-05: PHA2 trap-filter fields shipped in
 * `types.ts` via b258ab0 but were missed here, so typing
 * `energy_filter_rise_time_ns` in Tune Up never reached the digitizer
 * and re-entering Tune Up "lost" every typed value when the round-trip
 * wrote a JSON without those keys).
 *
 * The dotted `amax.*` keys are codegen-driven via `AMAX_DOTTED_KEYS`
 * and intentionally NOT listed here.
 */
const CHANNEL_PARAM_KEYS: (keyof ChannelConfig)[] = [
  // --- Input ---
  'enabled',
  'polarity',
  'dc_offset',
  'vga_gain',
  'baseline_avg',
  'fixed_baseline',
  'record_length_ns',
  'pre_trigger_ns',
  'wave_downsampling',
  'input_dynamic',
  'coarse_gain',
  // --- Trigger ---
  'discriminator_mode',
  'trigger_threshold',
  'trigger_threshold_v',
  'trigger_edge',
  'cfd_delay_ns',
  'cfd_fraction',
  'cfd_interpolation_point',
  'trigger_holdoff_ns',
  'smoothing_factor',
  'time_filter_smoothing',
  'input_smoothing',
  'fast_discr_smoothing',
  'input_rise_time_ns',
  'event_trigger_source',
  'wave_trigger_source',
  'self_trigger',
  'global_trigger_gen',
  'trigger_out_propagate',
  // --- Energy ---
  'energy_coarse_gain',
  'gate_long_ns',
  'gate_short_ns',
  'gate_pre_ns',
  'charge_pedestal',
  'short_charge_pedestal',
  'charge_smoothing',
  'charge_pedestal_en',
  'trap_rise_time_ns',
  'trap_flat_top_ns',
  'trap_pole_zero_ns',
  'peaking_time',
  'peak_nsmean',
  'peak_holdoff_ns',
  'energy_fine_gain',
  // --- PHA2 Time Filter ---
  'time_filter_rise_time_ns',
  'time_filter_retrigger_guard_ns',
  // --- PHA2 Energy Filter (trapezoidal) ---
  'energy_filter_rise_time_ns',
  'energy_filter_flat_top_ns',
  'energy_filter_pole_zero_ns',
  'energy_filter_peaking_position',
  'energy_filter_peaking_avg',
  'energy_filter_baseline_avg',
  'energy_filter_baseline_guard_ns',
  'energy_filter_pileup_guard_ns',
  'energy_filter_fine_gain',
  'energy_filter_lf_limitation',
  // --- PHA2 per-channel S_IN/GPI ---
  'sin_function',
  'gpi_function',
  // --- Coincidence ---
  'ch_trigger_mask',
  'coincidence_mask',
  'anti_coincidence_mask',
  'coincidence_window_ns',
  'coincidence_mode',
  'ch_veto_source',
  'ch_veto_width_ns',
  'event_selector',
  'pileup_rejection',
  // --- PSD1/PHA1 Extended Coincidence ---
  'trigger_latency',
  'coinc_mask',
  'coinc_operation',
  'coinc_majority_level',
  'coinc_trgext',
  'coinc_trgsw',
  'pileup_gap',
  'pileup_counting_en',
  'pileup_flag_en',
  // --- Waveform ---
  'wave_saving',
  'analog_probe_0',
  'analog_probe_1',
  'digital_probe_0',
  'digital_probe_1',
  'digital_probe_2',
  'digital_probe_3',
];

/**
 * AMax dotted-path keys are codegen-driven — the canonical list lives in
 * `amax-generated.ts`, regenerated alongside the Rust struct + register
 * map by `cargo run --bin amax_codegen`. The expand/compress logic below
 * iterates that list so a new firmware register only needs to land in
 * `fw_params.json`, not in this file.
 */

/** Read a possibly-dotted key from a ChannelConfig, returning undefined when
 *  any segment is missing. Only `amax.<field>` is supported today. */
function readDottedFromChannel(cfg: ChannelConfig | undefined, dotted: string): unknown {
  if (!cfg) return undefined;
  const parts = dotted.split('.');
  let cur: unknown = cfg;
  for (const p of parts) {
    if (cur && typeof cur === 'object' && p in (cur as Record<string, unknown>)) {
      cur = (cur as Record<string, unknown>)[p];
    } else {
      return undefined;
    }
  }
  return cur;
}

/** Write a value at a dotted path inside a ChannelConfig, creating
 *  intermediate `amax: {}` objects on demand. Mirror of `readDottedFromChannel`. */
function writeDottedToChannel(cfg: ChannelConfig, dotted: string, value: unknown): void {
  const parts = dotted.split('.');
  let obj: Record<string, unknown> = cfg as unknown as Record<string, unknown>;
  for (let i = 0; i < parts.length - 1; i++) {
    const p = parts[i];
    if (!(p in obj) || typeof obj[p] !== 'object' || obj[p] === null) {
      obj[p] = {};
    }
    obj = obj[p] as Record<string, unknown>;
  }
  obj[parts[parts.length - 1]] = value;
}

/** Strip the `amax.board.` prefix from a dotted key — board params live in
 *  `DigitizerConfig.amax_board.<field>`, not nested under `amax.board`. */
function boardField(dotted: string): string {
  return dotted.replace(/^amax\.board\./, '');
}

@Injectable({
  providedIn: 'root',
})
export class DigitizerService {
  private readonly apiUrl = '/api/digitizers';

  // Signal holding the list of digitizer configurations
  readonly digitizers = signal<DigitizerConfig[]>([]);

  /** True when every configured digitizer runs the AMax custom FW — the
   * histogram UI then defaults 2D plots to E vs UserInfo[0] and hides the
   * psd/energy_short axes, which AMax cannot produce (issue #25). Empty
   * list counts as false so the pre-load UI keeps the historical defaults. */
  readonly allAMax = computed(() => {
    const ds = this.digitizers();
    return ds.length > 0 && ds.every((d) => d.firmware === 'AMax');
  });

  // Selected digitizer ID — survives navigation between pages
  readonly selectedDigitizerId = signal<number | null>(null);

  // Selected tab index — survives navigation between pages
  readonly selectedTabIndex = signal(0);

  // Selected waveform channels — survives navigation between pages
  readonly selectedWaveformChannels = signal<string[]>([]);

  // Flag to use mock data when API is unavailable
  private useMock = false;

  private readonly http = inject(HttpClient);

  // ===========================================================================
  // API Methods
  // ===========================================================================

  /**
   * Load all digitizer configurations from the API.
   * Falls back to mock data if API is unavailable.
   */
  async loadDigitizers(): Promise<void> {
    try {
      const configs = await firstValueFrom(
        this.http.get<DigitizerConfig[]>(this.apiUrl)
      );
      this.digitizers.set(configs);
      this.useMock = false;
    } catch {
      console.warn('Failed to load digitizers from API, using mock data');
      this.digitizers.set(this.getMockDigitizers());
      this.useMock = true;
    }
  }

  /**
   * Get a single digitizer configuration
   */
  async getDigitizer(id: number): Promise<DigitizerConfig | null> {
    if (this.useMock) {
      return this.digitizers().find((d) => d.digitizer_id === id) ?? null;
    }

    try {
      return await firstValueFrom(
        this.http.get<DigitizerConfig>(`${this.apiUrl}/${id}`)
      );
    } catch {
      return null;
    }
  }

  /**
   * Update a digitizer configuration (in memory on the server)
   */
  async updateDigitizer(config: DigitizerConfig): Promise<void> {
    if (this.useMock) {
      const current = this.digitizers();
      const index = current.findIndex(
        (d) => d.digitizer_id === config.digitizer_id
      );
      if (index >= 0) {
        const updated = [...current];
        updated[index] = config;
        this.digitizers.set(updated);
      }
      return;
    }

    await firstValueFrom(
      this.http.put<ApiResponse>(
        `${this.apiUrl}/${config.digitizer_id}`,
        config
      )
    );
  }

  /**
   * Save a digitizer configuration to disk
   */
  async saveDigitizer(id: number): Promise<void> {
    if (this.useMock) {
      console.log('Mock: Would save digitizer', id, 'to disk');
      return;
    }

    await firstValueFrom(
      this.http.post<ApiResponse>(`${this.apiUrl}/${id}/save`, {})
    );
  }

  /**
   * Apply a digitizer configuration to hardware via Reader.
   * Updates in-memory config, saves to disk, and writes parameters to the digitizer.
   */
  async applyToHardware(config: DigitizerConfig): Promise<ApiResponse> {
    if (this.useMock) {
      return { success: true, message: 'Mock: Would apply to hardware' };
    }

    const result = await firstValueFrom(
      this.http.post<ApiResponse>(
        `${this.apiUrl}/${config.digitizer_id}/apply`,
        config
      )
    );

    // Refresh digitizers cache from backend to reflect applied config
    // (backend may also modify values via validation/clamping)
    if (result.success) {
      await this.loadDigitizers();
    }

    return result;
  }

  /**
   * Detect connected digitizer hardware via Reader.
   * Returns detected digitizers with their device info and any saved configs.
   */
  async detectDigitizers(): Promise<DetectResponse> {
    if (this.useMock) {
      return {
        success: true,
        message: 'Mock: No hardware available',
        digitizers: [],
      };
    }

    return await firstValueFrom(
      this.http.post<DetectResponse>(`${this.apiUrl}/detect`, {})
    );
  }

  // ===========================================================================
  // Config Expand / Compress
  // ===========================================================================

  /**
   * Expand a DigitizerConfig into flat per-channel value arrays.
   *
   * Each channel gets the default values merged with any overrides.
   * Returns an array of Records (one per channel), keyed by ChannelConfig fields.
   *
   * Example:
   *   defaults = { trigger_threshold: 1000 }
   *   overrides = { 4: { trigger_threshold: 500 } }
   *   → channelValues[0].trigger_threshold = 1000
   *   → channelValues[4].trigger_threshold = 500
   */
  expandConfig(config: DigitizerConfig): Record<string, unknown>[] {
    const result: Record<string, unknown>[] = [];
    const defaults = config.channel_defaults;

    for (let ch = 0; ch < config.num_channels; ch++) {
      const override = config.channel_overrides?.[ch];
      const values: Record<string, unknown> = {};

      for (const key of CHANNEL_PARAM_KEYS) {
        const defaultVal = defaults[key];
        const overrideVal = override?.[key];
        // Use override if defined, else default
        values[key] = overrideVal !== undefined ? overrideVal : defaultVal;
      }
      // Dotted (nested) keys — currently only `amax.<field>`. The flat-key
      // map above misses these because they live inside ChannelConfig.amax.
      for (const dotted of AMAX_DOTTED_KEYS) {
        const overrideVal = readDottedFromChannel(override, dotted);
        const defaultVal = readDottedFromChannel(defaults, dotted);
        values[dotted] = overrideVal !== undefined ? overrideVal : defaultVal;
      }

      result.push(values);
    }

    return result;
  }

  /**
   * Extract default values from a DigitizerConfig as a flat Record.
   */
  extractDefaults(config: DigitizerConfig): Record<string, unknown> {
    const result: Record<string, unknown> = {};
    for (const key of CHANNEL_PARAM_KEYS) {
      result[key] = config.channel_defaults[key];
    }
    for (const dotted of AMAX_DOTTED_KEYS) {
      result[dotted] = readDottedFromChannel(config.channel_defaults, dotted);
    }
    return result;
  }

  /**
   * Compress flat per-channel values back into defaults + overrides.
   *
   * Compares each channel's values to the defaults.
   * Only stores differences as overrides.
   */
  compressConfig(
    defaultValues: Record<string, unknown>,
    channelValues: Record<string, unknown>[]
  ): {
    channel_defaults: ChannelConfig;
    channel_overrides: Record<number, ChannelConfig>;
  } {
    // Build channel_defaults from the "All" column values
    const channel_defaults: ChannelConfig = {};
    for (const key of CHANNEL_PARAM_KEYS) {
      const val = defaultValues[key];
      if (val !== undefined && val !== null) {
        (channel_defaults as Record<string, unknown>)[key] = val;
      }
    }
    for (const dotted of AMAX_DOTTED_KEYS) {
      const val = defaultValues[dotted];
      if (val !== undefined && val !== null) {
        writeDottedToChannel(channel_defaults, dotted, val);
      }
    }

    // Build channel_overrides: only store values that differ from defaults
    const channel_overrides: Record<number, ChannelConfig> = {};
    for (let ch = 0; ch < channelValues.length; ch++) {
      const chValues = channelValues[ch];
      const overrideConfig: ChannelConfig = {};
      let hasOverride = false;

      for (const key of CHANNEL_PARAM_KEYS) {
        const chVal = chValues[key];
        const defVal = defaultValues[key];
        // If channel value differs from default, it's an override
        if (chVal !== defVal && chVal !== undefined) {
          (overrideConfig as Record<string, unknown>)[key] = chVal;
          hasOverride = true;
        }
      }
      for (const dotted of AMAX_DOTTED_KEYS) {
        const chVal = chValues[dotted];
        const defVal = defaultValues[dotted];
        if (chVal !== defVal && chVal !== undefined) {
          writeDottedToChannel(overrideConfig, dotted, chVal);
          hasOverride = true;
        }
      }

      if (hasOverride) {
        channel_overrides[ch] = overrideConfig;
      }
    }

    return { channel_defaults, channel_overrides };
  }

  /**
   * Live read-back of AMax board-level registers from the digitizer (via
   * the operator's `/api/digitizers/:id/amax-board-registers` route).
   * Returns `{ enable_acq: 0|1, ... }`. Empty object for non-AMax /
   * disconnected digitizers — backend returns `{}` in those cases instead
   * of erroring so the UI can render "no live registers" cleanly.
   */
  async readAmaxBoardRegisters(
    digitizerId: number,
  ): Promise<Record<string, number>> {
    return await firstValueFrom(
      this.http.get<Record<string, number>>(
        `${this.apiUrl}/${digitizerId}/amax-board-registers`,
      ),
    );
  }

  /**
   * Read board-level (global) AMax values from a DigitizerConfig as a flat
   * Record keyed by `amax.board.<field>`. Falls back to `AMAX_BOARD_DEFAULTS`
   * for any field the config doesn't explicitly set, so the Settings UI
   * always renders a value.
   */
  extractBoardValues(config: DigitizerConfig): Record<string, unknown> {
    const result: Record<string, unknown> = {};
    const board = config.amax_board ?? {};
    for (const dotted of AMAX_BOARD_DOTTED_KEYS) {
      const field = boardField(dotted);
      const v = (board as Record<string, unknown>)[field];
      result[dotted] = v !== undefined ? v : AMAX_BOARD_DEFAULTS[field];
    }
    return result;
  }

  /**
   * Compress flat board-level values (keyed by `amax.board.<field>`) back
   * into an `AMaxBoardConfig`. Returns `undefined` when every value matches
   * the FW defaults so the wire format stays minimal (and legacy configs
   * without an `amax_board` block round-trip cleanly).
   */
  compressBoardValues(values: Record<string, unknown>): AMaxBoardConfig | undefined {
    const out: Record<string, unknown> = {};
    let hasNonDefault = false;
    for (const dotted of AMAX_BOARD_DOTTED_KEYS) {
      const field = boardField(dotted);
      const v = values[dotted];
      if (v === undefined || v === null) continue;
      out[field] = v;
      if (v !== AMAX_BOARD_DEFAULTS[field]) {
        hasNonDefault = true;
      }
    }
    return hasNonDefault ? (out as AMaxBoardConfig) : undefined;
  }

  // ===========================================================================
  // Mock Data
  // ===========================================================================

  private getMockDigitizers(): DigitizerConfig[] {
    return [
      {
        digitizer_id: 0,
        name: 'LaBr3 Detector',
        firmware: 'PSD2',
        num_channels: 32,
        board: {
          start_source: 'SWcmd',
          gpio_mode: 'Run',
          test_pulse_period: 10000,
          test_pulse_width: 100,
          global_trigger_source: 'TestPulse',
          record_length: 2000,
          waveforms_enabled: true,
        },
        channel_defaults: {
          enabled: 'True',
          dc_offset: 20,
          polarity: 'Negative',
          trigger_threshold: 500,
          gate_long_ns: 400,
          gate_short_ns: 100,
          event_trigger_source: 'GlobalTriggerSource',
        },
        channel_overrides: {
          0: { trigger_threshold: 300 },
          1: { enabled: 'False' },
          15: { trigger_threshold: 800, dc_offset: 25 },
        },
      },
      {
        digitizer_id: 1,
        name: 'HPGe Detector',
        firmware: 'PHA1',
        num_channels: 16,
        board: {
          start_source: 'SWcmd',
          global_trigger_source: 'SwTrg',
          record_length: 8000,
          waveforms_enabled: false,
        },
        channel_defaults: {
          enabled: 'True',
          dc_offset: 10,
          polarity: 'Positive',
          trigger_threshold: 200,
          event_trigger_source: 'ChSelfTrigger',
        },
        channel_overrides: {},
      },
      {
        digitizer_id: 2,
        name: 'Scintillator Array',
        firmware: 'PSD1',
        num_channels: 8,
        board: {
          start_source: 'START_MODE_SW',
          record_length: 2048,
          waveforms_enabled: true,
          vtrace_probe_0: 'VPROBE_INPUT',
          vtrace_probe_1: 'VPROBE_NONE',
          vtrace_probe_2: 'VPROBE_GATE',
          vtrace_probe_3: 'VPROBE_GATESHORT',
          gpio_mode: 'OUT_PROPAGATION_RUN',
          io_level: 'FPIOTYPE_NIM',
        },
        channel_defaults: {
          enabled: 'True',
          dc_offset: 50,
          polarity: 'POLARITY_NEGATIVE',
          trigger_threshold: 100,
          gate_long_ns: 400,
          gate_short_ns: 100,
          gate_pre_ns: 40,
          self_trigger: 'TRUE',
        },
        channel_overrides: {
          7: { enabled: 'False' },
        },
      },
    ];
  }
}
