import { Injectable, inject, signal } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { EmulatorConfig, ApiResponse } from '../models/types';
import { firstValueFrom } from 'rxjs';

@Injectable({
  providedIn: 'root',
})
export class EmulatorService {
  private readonly apiUrl = 'http://localhost:8080/api/emulator';

  // Signal holding the current emulator configuration
  readonly config = signal<EmulatorConfig | null>(null);

  // Flag to use mock data when API is unavailable
  private useMock = false;

  private readonly http = inject(HttpClient);

  /**
   * Load emulator configuration from the API
   * Falls back to mock data if API is unavailable
   */
  async loadConfig(): Promise<void> {
    try {
      const config = await firstValueFrom(this.http.get<EmulatorConfig>(this.apiUrl));
      this.config.set(config);
      this.useMock = false;
    } catch {
      console.warn('Failed to load emulator config from API, using mock data');
      this.config.set(this.getMockConfig());
      this.useMock = true;
    }
  }

  /**
   * Update emulator configuration (in memory or via API)
   */
  async updateConfig(config: EmulatorConfig): Promise<void> {
    if (this.useMock) {
      // Update local mock data
      this.config.set(config);
      console.log('Mock: Updated emulator config in memory');
      return;
    }

    await firstValueFrom(this.http.put<ApiResponse>(this.apiUrl, config));
    this.config.set(config);
  }

  /**
   * Save emulator configuration to disk
   */
  async saveConfig(): Promise<void> {
    if (this.useMock) {
      console.log('Mock: Would save emulator config to disk');
      return;
    }

    await firstValueFrom(this.http.post<ApiResponse>(`${this.apiUrl}/save`, {}));
  }

  /**
   * Check if using mock data
   */
  isUsingMock(): boolean {
    return this.useMock;
  }

  /**
   * Generate mock emulator configuration for testing without hardware
   */
  private getMockConfig(): EmulatorConfig {
    return {
      events_per_batch: 5000,
      batch_interval_ms: 0,
      enable_waveform: false,
      waveform_probes: 3, // Both analog probes
      waveform_samples: 512,
      num_modules: 2,
      channels_per_module: 16,
    };
  }
}
