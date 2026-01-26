import { Component, inject, signal, computed } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MatCardModule } from '@angular/material/card';
import { MatSelectModule } from '@angular/material/select';
import { MatInputModule } from '@angular/material/input';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatButtonModule } from '@angular/material/button';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { MatExpansionModule } from '@angular/material/expansion';
import { MatIconModule } from '@angular/material/icon';
import { MatSnackBar, MatSnackBarModule } from '@angular/material/snack-bar';
import { MatDividerModule } from '@angular/material/divider';
import { MatChipsModule } from '@angular/material/chips';
import { DigitizerService } from '../../services/digitizer.service';

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
    MatExpansionModule,
    MatIconModule,
    MatSnackBarModule,
    MatDividerModule,
    MatChipsModule,
  ],
  template: `
    <div class="digitizer-settings">
      <div class="digitizer-selector">
        <mat-form-field appearance="outline">
          <mat-label>Select Digitizer</mat-label>
          <mat-select [value]="selectedId()" (selectionChange)="onDigitizerChange($event.value)">
            @for (dig of digitizers(); track dig.digitizer_id) {
              <mat-option [value]="dig.digitizer_id">
                {{ dig.name }} (ID: {{ dig.digitizer_id }})
              </mat-option>
            }
          </mat-select>
        </mat-form-field>
        <span class="firmware-badge" [class]="selectedConfig()?.firmware?.toLowerCase() ?? ''">
          {{ selectedConfig()?.firmware ?? 'N/A' }}
        </span>
      </div>

      @if (selectedConfig(); as config) {
        <div class="config-sections">
          <!-- Board Settings -->
          <mat-card class="config-card">
            <mat-card-header>
              <mat-card-title>Board Settings</mat-card-title>
            </mat-card-header>
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
            </mat-card-content>
          </mat-card>

          <!-- Channel Defaults -->
          <mat-card class="config-card">
            <mat-card-header>
              <mat-card-title>Channel Defaults</mat-card-title>
              <mat-card-subtitle>Applied to all {{ config.num_channels }} channels</mat-card-subtitle>
            </mat-card-header>
            <mat-card-content>
              <div class="form-grid">
                <mat-form-field appearance="outline">
                  <mat-label>DC Offset (%)</mat-label>
                  <input
                    matInput
                    type="number"
                    [(ngModel)]="config.channel_defaults.dc_offset"
                    min="0"
                    max="100"
                  />
                </mat-form-field>

                <mat-form-field appearance="outline">
                  <mat-label>Polarity</mat-label>
                  <mat-select [(value)]="config.channel_defaults.polarity">
                    <mat-option value="Positive">Positive</mat-option>
                    <mat-option value="Negative">Negative</mat-option>
                  </mat-select>
                </mat-form-field>

                <mat-form-field appearance="outline">
                  <mat-label>Trigger Threshold (ADC)</mat-label>
                  <input matInput type="number" [(ngModel)]="config.channel_defaults.trigger_threshold" />
                </mat-form-field>

                <mat-form-field appearance="outline">
                  <mat-label>Gate Long (ns)</mat-label>
                  <input matInput type="number" [(ngModel)]="config.channel_defaults.gate_long_ns" />
                </mat-form-field>

                <mat-form-field appearance="outline">
                  <mat-label>Gate Short (ns)</mat-label>
                  <input matInput type="number" [(ngModel)]="config.channel_defaults.gate_short_ns" />
                </mat-form-field>

                <mat-form-field appearance="outline">
                  <mat-label>Event Trigger Source</mat-label>
                  <mat-select [(value)]="config.channel_defaults.event_trigger_source">
                    <mat-option value="GlobalTriggerSource">Global Trigger</mat-option>
                    <mat-option value="ChSelfTrigger">Self Trigger</mat-option>
                  </mat-select>
                </mat-form-field>
              </div>
            </mat-card-content>
          </mat-card>

          <!-- Channel Overrides -->
          <mat-card class="config-card">
            <mat-card-header>
              <mat-card-title>Channel Overrides</mat-card-title>
              <mat-card-subtitle>Per-channel settings that differ from defaults</mat-card-subtitle>
            </mat-card-header>
            <mat-card-content>
              <div class="channel-chips">
                @for (ch of channelNumbers(); track ch) {
                  <mat-chip-option
                    [selected]="hasOverride(ch)"
                    (click)="toggleChannelOverride(ch)"
                    [class.has-override]="hasOverride(ch)"
                  >
                    Ch {{ ch }}
                  </mat-chip-option>
                }
              </div>

              <mat-accordion>
                @for (ch of overrideChannels(); track ch) {
                  <mat-expansion-panel>
                    <mat-expansion-panel-header>
                      <mat-panel-title>Channel {{ ch }}</mat-panel-title>
                      <mat-panel-description>
                        {{ getOverrideSummary(ch) }}
                      </mat-panel-description>
                    </mat-expansion-panel-header>

                    <div class="form-grid">
                      <mat-form-field appearance="outline">
                        <mat-label>Enabled</mat-label>
                        <mat-select [(value)]="config.channel_overrides![ch].enabled">
                          <mat-option [value]="undefined">Use Default</mat-option>
                          <mat-option value="True">Enabled</mat-option>
                          <mat-option value="False">Disabled</mat-option>
                        </mat-select>
                      </mat-form-field>

                      <mat-form-field appearance="outline">
                        <mat-label>Trigger Threshold (ADC)</mat-label>
                        <input
                          matInput
                          type="number"
                          [(ngModel)]="config.channel_overrides![ch].trigger_threshold"
                          placeholder="Default: {{ config.channel_defaults.trigger_threshold }}"
                        />
                      </mat-form-field>

                      <mat-form-field appearance="outline">
                        <mat-label>DC Offset (%)</mat-label>
                        <input
                          matInput
                          type="number"
                          [(ngModel)]="config.channel_overrides![ch].dc_offset"
                          placeholder="Default: {{ config.channel_defaults.dc_offset }}"
                        />
                      </mat-form-field>
                    </div>

                    <button mat-button color="warn" (click)="removeOverride(ch)">
                      <mat-icon>delete</mat-icon>
                      Remove Override
                    </button>
                  </mat-expansion-panel>
                }
              </mat-accordion>
            </mat-card-content>
          </mat-card>

          <!-- Action Buttons -->
          <div class="action-buttons">
            <button mat-button (click)="resetConfig()">
              <mat-icon>refresh</mat-icon>
              Reset
            </button>
            <button mat-raised-button color="primary" (click)="applyConfig()">
              <mat-icon>check</mat-icon>
              Apply
            </button>
            <button mat-raised-button color="accent" (click)="saveConfig()">
              <mat-icon>save</mat-icon>
              Save to Disk
            </button>
          </div>
        </div>
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

    .digitizer-selector {
      display: flex;
      align-items: center;
      gap: 16px;
      margin-bottom: 16px;
    }

    .digitizer-selector mat-form-field {
      width: 300px;
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

    .config-sections {
      display: flex;
      flex-direction: column;
      gap: 16px;
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

    .channel-chips {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-bottom: 16px;
    }

    .channel-chips mat-chip-option {
      cursor: pointer;
    }

    .channel-chips mat-chip-option.has-override {
      background-color: #1976d2;
      color: white;
    }

    .action-buttons {
      display: flex;
      gap: 8px;
      justify-content: flex-end;
      max-width: 800px;
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

  readonly selectedConfig = computed(() => {
    const id = this.selectedId();
    if (id === null) return null;
    return this.digitizers().find((d) => d.digitizer_id === id) ?? null;
  });

  readonly channelNumbers = computed(() => {
    const config = this.selectedConfig();
    if (!config) return [];
    return Array.from({ length: config.num_channels }, (_, i) => i);
  });

  readonly overrideChannels = computed(() => {
    const config = this.selectedConfig();
    if (!config?.channel_overrides) return [];
    return Object.keys(config.channel_overrides)
      .map(Number)
      .sort((a, b) => a - b);
  });

  constructor() {
    // Load digitizers on init
    this.digitizerService.loadDigitizers();
  }

  onDigitizerChange(value: number): void {
    this.selectedId.set(value);
  }

  hasOverride(channel: number): boolean {
    const config = this.selectedConfig();
    return config?.channel_overrides?.[channel] !== undefined;
  }

  toggleChannelOverride(channel: number): void {
    const config = this.selectedConfig();
    if (!config) return;

    if (!config.channel_overrides) {
      config.channel_overrides = {};
    }

    if (this.hasOverride(channel)) {
      delete config.channel_overrides[channel];
    } else {
      config.channel_overrides[channel] = {};
    }
  }

  removeOverride(channel: number): void {
    const config = this.selectedConfig();
    if (config?.channel_overrides) {
      delete config.channel_overrides[channel];
    }
  }

  getOverrideSummary(channel: number): string {
    const config = this.selectedConfig();
    const override = config?.channel_overrides?.[channel];
    if (!override) return '';

    const parts: string[] = [];
    if (override.enabled !== undefined) parts.push(`Enabled: ${override.enabled}`);
    if (override.trigger_threshold !== undefined) parts.push(`Thr: ${override.trigger_threshold}`);
    if (override.dc_offset !== undefined) parts.push(`Offset: ${override.dc_offset}%`);
    return parts.join(', ') || 'No changes';
  }

  async applyConfig(): Promise<void> {
    const config = this.selectedConfig();
    if (!config) return;

    try {
      await this.digitizerService.updateDigitizer(config);
      this.snackBar.open('Configuration applied (in memory)', 'OK', { duration: 3000 });
    } catch (e) {
      this.snackBar.open('Failed to apply configuration', 'Close', { duration: 5000 });
    }
  }

  async saveConfig(): Promise<void> {
    const config = this.selectedConfig();
    if (!config) return;

    try {
      await this.digitizerService.saveDigitizer(config.digitizer_id);
      this.snackBar.open('Configuration saved to disk', 'OK', { duration: 3000 });
    } catch (e) {
      this.snackBar.open('Failed to save configuration', 'Close', { duration: 5000 });
    }
  }

  resetConfig(): void {
    if (this.selectedId() !== null) {
      this.digitizerService.loadDigitizers();
      this.snackBar.open('Configuration reset', 'OK', { duration: 2000 });
    }
  }
}
