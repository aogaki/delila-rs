import { Injectable, signal } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { DigitizerConfig, ApiResponse } from '../models/types';
import { firstValueFrom } from 'rxjs';

@Injectable({
  providedIn: 'root',
})
export class DigitizerService {
  private readonly apiUrl = 'http://localhost:8080/api/digitizers';

  // Signal holding the list of digitizer configurations
  readonly digitizers = signal<DigitizerConfig[]>([]);

  // Flag to use mock data when API is unavailable
  private useMock = false;

  constructor(private http: HttpClient) {}

  /**
   * Load all digitizer configurations from the API
   * Falls back to mock data if API is unavailable
   */
  async loadDigitizers(): Promise<void> {
    try {
      const configs = await firstValueFrom(this.http.get<DigitizerConfig[]>(this.apiUrl));
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
      return await firstValueFrom(this.http.get<DigitizerConfig>(`${this.apiUrl}/${id}`));
    } catch {
      return null;
    }
  }

  /**
   * Update a digitizer configuration (in memory)
   */
  async updateDigitizer(config: DigitizerConfig): Promise<void> {
    if (this.useMock) {
      // Update local mock data
      const current = this.digitizers();
      const index = current.findIndex((d) => d.digitizer_id === config.digitizer_id);
      if (index >= 0) {
        const updated = [...current];
        updated[index] = config;
        this.digitizers.set(updated);
      }
      return;
    }

    await firstValueFrom(
      this.http.put<ApiResponse>(`${this.apiUrl}/${config.digitizer_id}`, config)
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

    await firstValueFrom(this.http.post<ApiResponse>(`${this.apiUrl}/${id}/save`, {}));
  }

  /**
   * Generate mock digitizer configurations for testing without hardware
   */
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
