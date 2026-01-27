import { Injectable, inject, signal } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import {
  DigitizerConfig,
  ChannelConfig,
  ApiResponse,
  DetectResponse,
} from '../models/types';
import { firstValueFrom } from 'rxjs';

/** Keys in ChannelConfig that are channel parameters (not extra) */
const CHANNEL_PARAM_KEYS: (keyof ChannelConfig)[] = [
  'enabled',
  'dc_offset',
  'polarity',
  'trigger_threshold',
  'gate_long_ns',
  'gate_short_ns',
  'gate_pre_ns',
  'event_trigger_source',
  'wave_trigger_source',
  'cfd_delay_ns',
];

@Injectable({
  providedIn: 'root',
})
export class DigitizerService {
  private readonly apiUrl = 'http://localhost:8080/api/digitizers';

  // Signal holding the list of digitizer configurations
  readonly digitizers = signal<DigitizerConfig[]>([]);

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

      if (hasOverride) {
        channel_overrides[ch] = overrideConfig;
      }
    }

    return { channel_defaults, channel_overrides };
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
        firmware: 'PHA',
        num_channels: 16,
        board: {
          start_source: 'SWcmd',
          global_trigger_source: 'SwTrg',
          record_length: 4000,
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
          start_source: 'ITLA',
          global_trigger_source: 'ITLA',
          record_length: 1024,
          waveforms_enabled: true,
        },
        channel_defaults: {
          enabled: 'True',
          dc_offset: 50,
          polarity: 'Negative',
          trigger_threshold: 100,
          gate_long_ns: 200,
          gate_short_ns: 50,
          gate_pre_ns: 20,
          event_trigger_source: 'ChSelfTrigger',
        },
        channel_overrides: {
          7: { enabled: 'False' },
        },
      },
    ];
  }
}
