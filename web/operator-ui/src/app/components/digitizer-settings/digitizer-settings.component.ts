import { Component, inject, signal, computed, effect } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MatCardModule } from '@angular/material/card';
import { MatSelectModule } from '@angular/material/select';
import { MatInputModule } from '@angular/material/input';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatButtonModule } from '@angular/material/button';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { MatIconModule } from '@angular/material/icon';
import { MatSnackBar, MatSnackBarModule } from '@angular/material/snack-bar';
import { MatDividerModule } from '@angular/material/divider';
import { MatTabsModule } from '@angular/material/tabs';
import { MatProgressSpinnerModule } from '@angular/material/progress-spinner';
import { DigitizerService } from '../../services/digitizer.service';
import { FirmwareType } from '../../models/types';
import {
  ChannelTableComponent,
  ChannelParamDef,
  DefaultValueChange,
  ChannelValueChange,
} from '../channel-table/channel-table.component';

// =============================================================================
// Parameter Definitions (FW-specific)
// =============================================================================

/** Frequent channel parameters for PSD2 (VX2730) */
const PSD2_FREQUENT_PARAMS: ChannelParamDef[] = [
  { key: 'enabled', label: 'Enable', type: 'boolean' },
  { key: 'dc_offset', label: 'DC Offset', type: 'number', unit: '%', min: 0, max: 100 },
  { key: 'polarity', label: 'Polarity', type: 'enum', options: ['Positive', 'Negative'] },
  { key: 'trigger_threshold', label: 'Threshold', type: 'number', unit: 'ADC' },
  { key: 'gate_long_ns', label: 'Gate Long', type: 'number', unit: 'ns' },
  { key: 'gate_short_ns', label: 'Gate Short', type: 'number', unit: 'ns' },
  {
    key: 'event_trigger_source',
    label: 'Evt Trigger',
    type: 'enum',
    options: ['GlobalTriggerSource', 'ChSelfTrigger', 'Disabled'],
  },
];

/** Advanced channel parameters for PSD2 (VX2730) */
const PSD2_ADVANCED_PARAMS: ChannelParamDef[] = [
  {
    key: 'wave_trigger_source',
    label: 'Wave Trigger',
    type: 'enum',
    options: ['Disabled', 'ChSelfTrigger', 'GlobalTriggerSource'],
  },
  { key: 'cfd_delay_ns', label: 'CFD Delay', type: 'number', unit: 'ns' },
];

/** Frequent channel parameters for PSD1 (DT5730B / x725 / x730) */
const PSD1_FREQUENT_PARAMS: ChannelParamDef[] = [
  { key: 'enabled', label: 'Enable', type: 'boolean' },
  { key: 'dc_offset', label: 'DC Offset', type: 'number', unit: '%', min: 0, max: 100 },
  { key: 'polarity', label: 'Polarity', type: 'enum', options: ['Positive', 'Negative'] },
  { key: 'trigger_threshold', label: 'Threshold', type: 'number', unit: 'ADC' },
  { key: 'gate_long_ns', label: 'Gate Long', type: 'number', unit: 'samples' },
  { key: 'gate_short_ns', label: 'Gate Short', type: 'number', unit: 'samples' },
  { key: 'gate_pre_ns', label: 'Gate Pre', type: 'number', unit: 'samples' },
  {
    key: 'event_trigger_source',
    label: 'Self Trigger',
    type: 'enum',
    options: ['GlobalTriggerSource', 'ChSelfTrigger', 'Disabled'],
  },
];

/** Advanced channel parameters for PSD1 */
const PSD1_ADVANCED_PARAMS: ChannelParamDef[] = [
  { key: 'cfd_delay_ns', label: 'CFD Delay', type: 'number' },
];

/** PHA uses same as PSD2 for now (subset) */
const PHA_FREQUENT_PARAMS: ChannelParamDef[] = [
  { key: 'enabled', label: 'Enable', type: 'boolean' },
  { key: 'dc_offset', label: 'DC Offset', type: 'number', unit: '%', min: 0, max: 100 },
  { key: 'polarity', label: 'Polarity', type: 'enum', options: ['Positive', 'Negative'] },
  { key: 'trigger_threshold', label: 'Threshold', type: 'number', unit: 'ADC' },
  {
    key: 'event_trigger_source',
    label: 'Evt Trigger',
    type: 'enum',
    options: ['GlobalTriggerSource', 'ChSelfTrigger', 'Disabled'],
  },
];

const PHA_ADVANCED_PARAMS: ChannelParamDef[] = [
  {
    key: 'wave_trigger_source',
    label: 'Wave Trigger',
    type: 'enum',
    options: ['Disabled', 'ChSelfTrigger', 'GlobalTriggerSource'],
  },
  { key: 'cfd_delay_ns', label: 'CFD Delay', type: 'number', unit: 'ns' },
];

function getFrequentParams(fw: FirmwareType): ChannelParamDef[] {
  switch (fw) {
    case 'PSD2': return PSD2_FREQUENT_PARAMS;
    case 'PSD1': return PSD1_FREQUENT_PARAMS;
    case 'PHA': return PHA_FREQUENT_PARAMS;
  }
}

function getAdvancedParams(fw: FirmwareType): ChannelParamDef[] {
  switch (fw) {
    case 'PSD2': return PSD2_ADVANCED_PARAMS;
    case 'PSD1': return PSD1_ADVANCED_PARAMS;
    case 'PHA': return PHA_ADVANCED_PARAMS;
  }
}

@Component({
  selector: 'app-digitizer-settings',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    MatCardModule,
    MatSelectModule,
    MatInputModule,
    MatFormFieldModule,
    MatButtonModule,
    MatSlideToggleModule,
    MatIconModule,
    MatSnackBarModule,
    MatDividerModule,
    MatTabsModule,
    MatProgressSpinnerModule,
    ChannelTableComponent,
  ],
  template: `
    <div class="digitizer-settings">
      <!-- Header: Digitizer selector + firmware badge + action buttons -->
      <div class="header-row">
        <mat-form-field appearance="outline" class="digitizer-select">
          <mat-label>Select Digitizer</mat-label>
          <mat-select [value]="selectedId()" (selectionChange)="onDigitizerChange($event.value)">
            @for (dig of digitizers(); track dig.digitizer_id) {
              <mat-option [value]="dig.digitizer_id">
                {{ dig.name }} (ID: {{ dig.digitizer_id }})
              </mat-option>
            }
          </mat-select>
        </mat-form-field>

        @if (selectedConfig(); as config) {
          <span class="firmware-badge" [class]="config.firmware.toLowerCase()">
            {{ config.firmware }}
          </span>
          @if (config.serial_number) {
            <span class="serial-info">S/N: {{ config.serial_number }}</span>
          }
        }

        <span class="spacer"></span>

        <button mat-button (click)="onDetect()" [disabled]="detecting()">
          @if (detecting()) {
            <mat-spinner diameter="18" class="inline-spinner"></mat-spinner>
          } @else {
            <mat-icon>search</mat-icon>
          }
          Detect
        </button>
        <button mat-button (click)="resetConfig()" [disabled]="!selectedConfig()">
          <mat-icon>refresh</mat-icon>
          Reset
        </button>
        <button
          mat-raised-button
          color="primary"
          (click)="applyConfig()"
          [disabled]="!selectedConfig()"
        >
          <mat-icon>check</mat-icon>
          Apply
        </button>
        <button
          mat-raised-button
          color="accent"
          (click)="saveConfig()"
          [disabled]="!selectedConfig()"
        >
          <mat-icon>save</mat-icon>
          Save
        </button>
      </div>

      @if (selectedConfig(); as config) {
        <!-- 3-tab layout -->
        <mat-tab-group animationDuration="0ms">
          <!-- Tab 1: Board Settings -->
          <mat-tab label="Board">
            <div class="tab-content">
              <mat-card class="config-card">
                <mat-card-content>
                  <div class="form-grid">
                    <mat-form-field appearance="outline">
                      <mat-label>Start Source</mat-label>
                      <mat-select [(value)]="config.board.start_source">
                        <mat-option value="SWcmd">Software Command</mat-option>
                        <mat-option value="ITLA">Internal Trigger</mat-option>
                        <mat-option value="GPIO">GPIO</mat-option>
                      </mat-select>
                    </mat-form-field>

                    <mat-form-field appearance="outline">
                      <mat-label>Global Trigger Source</mat-label>
                      <mat-select [(value)]="config.board.global_trigger_source">
                        <mat-option value="SwTrg">Software Trigger</mat-option>
                        <mat-option value="TestPulse">Test Pulse</mat-option>
                        <mat-option value="ITLA">Internal Trigger</mat-option>
                      </mat-select>
                    </mat-form-field>

                    <mat-form-field appearance="outline">
                      <mat-label>Test Pulse Period (ns)</mat-label>
                      <input matInput type="number" [(ngModel)]="config.board.test_pulse_period" />
                    </mat-form-field>

                    <mat-form-field appearance="outline">
                      <mat-label>Test Pulse Width (ns)</mat-label>
                      <input matInput type="number" [(ngModel)]="config.board.test_pulse_width" />
                    </mat-form-field>

                    <mat-form-field appearance="outline">
                      <mat-label>Record Length (samples)</mat-label>
                      <input matInput type="number" [(ngModel)]="config.board.record_length" />
                    </mat-form-field>

                    <mat-slide-toggle [(ngModel)]="config.board.waveforms_enabled">
                      Enable Waveforms
                    </mat-slide-toggle>
                  </div>

                  @if (config.board.waveforms_enabled) {
                    <mat-divider></mat-divider>
                    <h3 class="section-title">Waveform Probes</h3>
                    <div class="form-grid">
                      <mat-form-field appearance="outline">
                        <mat-label>Analog Probe 1</mat-label>
                        <mat-select [(value)]="config.board.extra!['analog_probe_0']">
                          @for (opt of analogProbeOptions(config.firmware); track opt) {
                            <mat-option [value]="opt">{{ opt }}</mat-option>
                          }
                        </mat-select>
                      </mat-form-field>

                      <mat-form-field appearance="outline">
                        <mat-label>Analog Probe 2</mat-label>
                        <mat-select [(value)]="config.board.extra!['analog_probe_1']">
                          @for (opt of analogProbeOptions(config.firmware); track opt) {
                            <mat-option [value]="opt">{{ opt }}</mat-option>
                          }
                        </mat-select>
                      </mat-form-field>

                      <mat-form-field appearance="outline">
                        <mat-label>Digital Probe 1</mat-label>
                        <mat-select [(value)]="config.board.extra!['digital_probe_0']">
                          @for (opt of digitalProbeOptions(config.firmware); track opt) {
                            <mat-option [value]="opt">{{ opt }}</mat-option>
                          }
                        </mat-select>
                      </mat-form-field>

                      <mat-form-field appearance="outline">
                        <mat-label>Digital Probe 2</mat-label>
                        <mat-select [(value)]="config.board.extra!['digital_probe_1']">
                          @for (opt of digitalProbeOptions(config.firmware); track opt) {
                            <mat-option [value]="opt">{{ opt }}</mat-option>
                          }
                        </mat-select>
                      </mat-form-field>
                    </div>
                  }

                  @if (config.firmware === 'PSD1') {
                    <mat-divider></mat-divider>
                    <h3 class="section-title">PSD1 Settings</h3>
                    <div class="form-grid">
                      <mat-form-field appearance="outline">
                        <mat-label>Start Mode</mat-label>
                        <mat-select [(value)]="config.board.extra!['start_mode']">
                          <mat-option value="START_MODE_SW">Software</mat-option>
                          <mat-option value="START_MODE_S_IN">S-IN</mat-option>
                          <mat-option value="START_MODE_TRGIN">TRGIN</mat-option>
                        </mat-select>
                      </mat-form-field>

                      <mat-form-field appearance="outline">
                        <mat-label>Extras</mat-label>
                        <mat-select [(value)]="config.board.extra!['extras']">
                          <mat-option value="TRUE">Enabled</mat-option>
                          <mat-option value="FALSE">Disabled</mat-option>
                        </mat-select>
                      </mat-form-field>
                    </div>
                  }
                </mat-card-content>
              </mat-card>
            </div>
          </mat-tab>

          <!-- Tab 2: Frequent Channel Parameters -->
          <mat-tab label="Frequent">
            <div class="tab-content">
              <app-channel-table
                [params]="frequentParams()"
                [numChannels]="config.num_channels"
                [defaultValues]="defaultValues()"
                [channelValues]="channelValues()"
                (defaultChange)="onDefaultChange($event)"
                (channelChange)="onChannelChange($event)"
              />
            </div>
          </mat-tab>

          <!-- Tab 3: Advanced Channel Parameters -->
          <mat-tab label="Advanced">
            <div class="tab-content">
              @if (advancedParams().length > 0) {
                <app-channel-table
                  [params]="advancedParams()"
                  [numChannels]="config.num_channels"
                  [defaultValues]="defaultValues()"
                  [channelValues]="channelValues()"
                  (defaultChange)="onDefaultChange($event)"
                  (channelChange)="onChannelChange($event)"
                />
              } @else {
                <p class="no-params-msg">No advanced parameters for {{ config.firmware }}.</p>
              }
            </div>
          </mat-tab>
        </mat-tab-group>
      } @else {
        <mat-card class="no-selection">
          <mat-card-content>
            <mat-icon>memory</mat-icon>
            <p>Select a digitizer to configure</p>
          </mat-card-content>
        </mat-card>
      }
    </div>
  `,
  styles: `
    .digitizer-settings {
      padding: 16px;
    }

    .header-row {
      display: flex;
      align-items: center;
      gap: 12px;
      margin-bottom: 8px;
      flex-wrap: wrap;
    }

    .digitizer-select {
      width: 280px;
    }

    .firmware-badge {
      padding: 4px 12px;
      border-radius: 12px;
      font-size: 12px;
      font-weight: 500;
      text-transform: uppercase;
    }

    .firmware-badge.psd2 {
      background-color: #e3f2fd;
      color: #1976d2;
    }

    .firmware-badge.psd1 {
      background-color: #fff3e0;
      color: #f57c00;
    }

    .firmware-badge.pha {
      background-color: #e8f5e9;
      color: #388e3c;
    }

    .serial-info {
      font-size: 12px;
      color: #666;
      font-family: monospace;
    }

    .spacer {
      flex: 1;
    }

    .inline-spinner {
      display: inline-block;
      margin-right: 4px;
    }

    .tab-content {
      padding: 16px 0;
    }

    .config-card {
      max-width: 800px;
    }

    .form-grid {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
      gap: 16px;
      padding: 16px 0;
    }

    .section-title {
      margin: 16px 0 0;
      font-size: 14px;
      font-weight: 500;
      color: #666;
    }

    .no-params-msg {
      color: #999;
      font-style: italic;
      padding: 24px;
    }

    .no-selection {
      max-width: 400px;
      text-align: center;
      padding: 48px;
    }

    .no-selection mat-icon {
      font-size: 48px;
      width: 48px;
      height: 48px;
      opacity: 0.5;
    }

    .no-selection p {
      margin-top: 16px;
      color: rgba(0, 0, 0, 0.54);
    }
  `,
})
export class DigitizerSettingsComponent {
  private readonly digitizerService = inject(DigitizerService);
  private readonly snackBar = inject(MatSnackBar);

  readonly digitizers = this.digitizerService.digitizers;
  readonly selectedId = signal<number | null>(null);
  readonly detecting = signal(false);

  // Expanded channel data (mutable working copy)
  readonly defaultValues = signal<Record<string, unknown>>({});
  readonly channelValues = signal<Record<string, unknown>[]>([]);

  readonly selectedConfig = computed(() => {
    const id = this.selectedId();
    if (id === null) return null;
    return this.digitizers().find((d) => d.digitizer_id === id) ?? null;
  });

  readonly frequentParams = computed(() => {
    const config = this.selectedConfig();
    if (!config) return [];
    return getFrequentParams(config.firmware);
  });

  readonly advancedParams = computed(() => {
    const config = this.selectedConfig();
    if (!config) return [];
    return getAdvancedParams(config.firmware);
  });

  constructor() {
    // Load digitizers on init
    this.digitizerService.loadDigitizers();

    // When selected config changes, expand it into flat channel arrays
    effect(() => {
      const config = this.selectedConfig();
      if (config) {
        // Ensure board.extra exists for waveform probe settings
        if (!config.board.extra) {
          config.board.extra = {};
        }
        this.defaultValues.set(this.digitizerService.extractDefaults(config));
        this.channelValues.set(this.digitizerService.expandConfig(config));
      } else {
        this.defaultValues.set({});
        this.channelValues.set([]);
      }
    });
  }

  onDigitizerChange(value: number): void {
    this.selectedId.set(value);
  }

  // ===========================================================================
  // Channel Table Event Handlers
  // ===========================================================================

  /**
   * "All" column changed — update default and propagate to all channels.
   */
  onDefaultChange(event: DefaultValueChange): void {
    const defaults = { ...this.defaultValues() };
    defaults[event.key] = event.value;
    this.defaultValues.set(defaults);

    // Propagate to all channels
    const channels = this.channelValues().map((ch) => ({
      ...ch,
      [event.key]: event.value,
    }));
    this.channelValues.set(channels);
  }

  /**
   * Individual channel changed — update only that channel.
   */
  onChannelChange(event: ChannelValueChange): void {
    const channels = [...this.channelValues()];
    channels[event.channel] = {
      ...channels[event.channel],
      [event.key]: event.value,
    };
    this.channelValues.set(channels);
  }

  // ===========================================================================
  // Waveform Probe Options (FW-specific)
  // ===========================================================================

  analogProbeOptions(fw: FirmwareType): string[] {
    if (fw === 'PSD2') {
      return [
        'ADCInput',
        'CFDOutput',
        'TimeFilter',
        'EnergyFilter',
        'EnergyFilterBaseline',
        'EnergyFilterMinusBaseline',
      ];
    }
    return ['InputSignal', 'CFDSignal'];
  }

  digitalProbeOptions(fw: FirmwareType): string[] {
    if (fw === 'PSD2') {
      return [
        'LongGate',
        'ShortGate',
        'OverThreshold',
        'ChargeReady',
        'PileUpTrigger',
        'Trigger',
      ];
    }
    return ['Gate', 'OverThreshold', 'TrgVal', 'CoincWindow'];
  }

  // ===========================================================================
  // Actions
  // ===========================================================================

  async onDetect(): Promise<void> {
    this.detecting.set(true);
    try {
      const result = await this.digitizerService.detectDigitizers();
      if (result.success && result.digitizers.length > 0) {
        this.snackBar.open(result.message, 'OK', { duration: 5000 });
        // Reload digitizers to pick up any newly created configs
        await this.digitizerService.loadDigitizers();
      } else {
        this.snackBar.open(result.message || 'No digitizers detected', 'OK', {
          duration: 5000,
        });
      }
    } catch {
      this.snackBar.open('Failed to detect hardware', 'Close', {
        duration: 5000,
      });
    } finally {
      this.detecting.set(false);
    }
  }

  async applyConfig(): Promise<void> {
    const config = this.selectedConfig();
    if (!config) return;

    // Compress flat channel values back into defaults + overrides
    const { channel_defaults, channel_overrides } =
      this.digitizerService.compressConfig(
        this.defaultValues(),
        this.channelValues()
      );

    const updatedConfig = {
      ...config,
      channel_defaults,
      channel_overrides,
    };

    try {
      await this.digitizerService.updateDigitizer(updatedConfig);
      this.snackBar.open('Configuration applied (in memory)', 'OK', {
        duration: 3000,
      });
    } catch {
      this.snackBar.open('Failed to apply configuration', 'Close', {
        duration: 5000,
      });
    }
  }

  async saveConfig(): Promise<void> {
    const config = this.selectedConfig();
    if (!config) return;

    // First apply (compress & send), then save to disk
    await this.applyConfig();

    try {
      await this.digitizerService.saveDigitizer(config.digitizer_id);
      this.snackBar.open('Configuration saved to disk', 'OK', {
        duration: 3000,
      });
    } catch {
      this.snackBar.open('Failed to save configuration', 'Close', {
        duration: 5000,
      });
    }
  }

  resetConfig(): void {
    if (this.selectedId() !== null) {
      this.digitizerService.loadDigitizers();
      this.snackBar.open('Configuration reset', 'OK', { duration: 2000 });
    }
  }
}
