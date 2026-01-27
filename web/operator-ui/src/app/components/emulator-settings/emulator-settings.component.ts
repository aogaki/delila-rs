import { Component, OnInit, computed, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MatCardModule } from '@angular/material/card';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatDividerModule } from '@angular/material/divider';
import { MatSnackBar, MatSnackBarModule } from '@angular/material/snack-bar';
import { MatTooltipModule } from '@angular/material/tooltip';
import { EmulatorService } from '../../services/emulator.service';
import { EmulatorConfig } from '../../models/types';

@Component({
  selector: 'app-emulator-settings',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    MatCardModule,
    MatFormFieldModule,
    MatInputModule,
    MatSelectModule,
    MatSlideToggleModule,
    MatButtonModule,
    MatIconModule,
    MatDividerModule,
    MatSnackBarModule,
    MatTooltipModule,
  ],
  template: `
    <div class="emulator-settings">
      @if (emulatorService.config(); as config) {
        <mat-card class="info-card">
          <mat-card-content>
            <div class="info-banner">
              <mat-icon>info</mat-icon>
              <span>
                @if (emulatorService.isUsingMock()) {
                  Using mock data (API not available). Changes are local only.
                } @else {
                  Connected to Emulator API. Changes will be applied on next run.
                }
              </span>
            </div>
          </mat-card-content>
        </mat-card>

        <mat-card class="settings-card">
          <mat-card-header>
            <mat-card-title>Data Generation Settings</mat-card-title>
          </mat-card-header>
          <mat-card-content>
            <div class="form-grid">
              <mat-form-field>
                <mat-label>Events per Batch</mat-label>
                <input
                  matInput
                  type="number"
                  [(ngModel)]="editConfig.events_per_batch"
                  min="1"
                  max="100000"
                />
                <mat-hint>Number of events in each batch</mat-hint>
              </mat-form-field>

              <mat-form-field>
                <mat-label>Batch Interval (ms)</mat-label>
                <input
                  matInput
                  type="number"
                  [(ngModel)]="editConfig.batch_interval_ms"
                  min="0"
                  max="10000"
                />
                <mat-hint>0 = maximum speed</mat-hint>
              </mat-form-field>

              <mat-form-field>
                <mat-label>Number of Modules</mat-label>
                <input
                  matInput
                  type="number"
                  [(ngModel)]="editConfig.num_modules"
                  min="1"
                  max="16"
                />
                <mat-hint>Simulated digitizer modules</mat-hint>
              </mat-form-field>

              <mat-form-field>
                <mat-label>Channels per Module</mat-label>
                <input
                  matInput
                  type="number"
                  [(ngModel)]="editConfig.channels_per_module"
                  min="1"
                  max="64"
                />
                <mat-hint>Channels per digitizer module</mat-hint>
              </mat-form-field>
            </div>
          </mat-card-content>
        </mat-card>

        <mat-card class="settings-card">
          <mat-card-header>
            <mat-card-title>Waveform Settings</mat-card-title>
          </mat-card-header>
          <mat-card-content>
            <div class="form-row">
              <mat-slide-toggle [(ngModel)]="editConfig.enable_waveform">
                Enable Waveform Generation
              </mat-slide-toggle>
              <span class="toggle-hint">Generate waveform data for all events</span>
            </div>

            <div class="form-grid" [class.disabled]="!editConfig.enable_waveform">
              <mat-form-field>
                <mat-label>Waveform Samples</mat-label>
                <input
                  matInput
                  type="number"
                  [(ngModel)]="editConfig.waveform_samples"
                  min="64"
                  max="10000"
                  [disabled]="!editConfig.enable_waveform"
                />
                <mat-hint>Samples per waveform (typical: 512-2000)</mat-hint>
              </mat-form-field>
            </div>
          </mat-card-content>
        </mat-card>

        <mat-card class="settings-card">
          <mat-card-header>
            <mat-card-title>Estimated Data Rate</mat-card-title>
          </mat-card-header>
          <mat-card-content>
            <div class="rate-display">
              <div class="rate-item">
                <span class="rate-label">Total Channels:</span>
                <span class="rate-value">{{ totalChannels() }}</span>
              </div>
              <div class="rate-item">
                <span class="rate-label">Events/sec (max):</span>
                <span class="rate-value">{{ estimatedEventsPerSec() | number:'1.0-0' }}</span>
              </div>
              <div class="rate-item">
                <span class="rate-label">Data Rate (est):</span>
                <span class="rate-value">{{ estimatedDataRate() }}</span>
              </div>
            </div>
          </mat-card-content>
        </mat-card>

        <div class="action-buttons">
          <button mat-raised-button color="primary" (click)="applyChanges()" [disabled]="!hasChanges()">
            <mat-icon>check</mat-icon>
            Apply
          </button>
          <button mat-raised-button (click)="saveToFile()" [disabled]="emulatorService.isUsingMock()">
            <mat-icon>save</mat-icon>
            Save to Config
          </button>
          <button mat-raised-button (click)="resetChanges()">
            <mat-icon>refresh</mat-icon>
            Reset
          </button>
        </div>
      } @else {
        <mat-card>
          <mat-card-content>
            <p>Loading emulator configuration...</p>
          </mat-card-content>
        </mat-card>
      }
    </div>
  `,
  styles: `
    .emulator-settings {
      padding: 16px;
      max-width: 800px;
    }

    .info-card {
      margin-bottom: 16px;
    }

    .info-banner {
      display: flex;
      align-items: center;
      gap: 8px;
      color: rgba(0, 0, 0, 0.6);
    }

    .info-banner mat-icon {
      font-size: 20px;
      width: 20px;
      height: 20px;
    }

    .settings-card {
      margin-bottom: 16px;
    }

    .form-grid {
      display: grid;
      grid-template-columns: repeat(2, 1fr);
      gap: 16px;
      padding-top: 8px;
    }

    .form-grid.disabled {
      opacity: 0.5;
    }

    .form-row {
      display: flex;
      align-items: center;
      gap: 16px;
      padding: 8px 0;
    }

    .toggle-hint {
      color: rgba(0, 0, 0, 0.6);
      font-size: 12px;
    }

    .rate-display {
      display: grid;
      grid-template-columns: repeat(3, 1fr);
      gap: 16px;
      padding: 8px 0;
    }

    .rate-item {
      display: flex;
      flex-direction: column;
      gap: 4px;
    }

    .rate-label {
      font-size: 12px;
      color: rgba(0, 0, 0, 0.6);
    }

    .rate-value {
      font-size: 18px;
      font-weight: 500;
    }

    .action-buttons {
      display: flex;
      gap: 8px;
      padding-top: 8px;
    }

    mat-form-field {
      width: 100%;
    }
  `,
})
export class EmulatorSettingsComponent implements OnInit {
  // Editable copy of the config
  editConfig: EmulatorConfig = {
    events_per_batch: 5000,
    batch_interval_ms: 0,
    enable_waveform: false,
    waveform_probes: 3,
    waveform_samples: 512,
    num_modules: 2,
    channels_per_module: 16,
  };

  // Original config for comparison
  private originalConfig: EmulatorConfig | null = null;

  // Computed values for rate estimation
  totalChannels = computed(() => this.editConfig.num_modules * this.editConfig.channels_per_module);

  estimatedEventsPerSec = computed(() => {
    if (this.editConfig.batch_interval_ms === 0) {
      // Maximum speed - depends on system performance
      return this.editConfig.events_per_batch * 1000; // Rough estimate
    }
    return (this.editConfig.events_per_batch / this.editConfig.batch_interval_ms) * 1000;
  });

  estimatedDataRate = computed(() => {
    const eventsPerSec = this.estimatedEventsPerSec();
    // Base event size: ~40 bytes + waveform if enabled
    let bytesPerEvent = 40;
    if (this.editConfig.enable_waveform) {
      // 2 bytes per sample for analog waveform
      bytesPerEvent += this.editConfig.waveform_samples * 2;
    }
    const bytesPerSec = eventsPerSec * bytesPerEvent;

    if (bytesPerSec >= 1e9) {
      return `${(bytesPerSec / 1e9).toFixed(2)} GB/s`;
    } else if (bytesPerSec >= 1e6) {
      return `${(bytesPerSec / 1e6).toFixed(1)} MB/s`;
    } else if (bytesPerSec >= 1e3) {
      return `${(bytesPerSec / 1e3).toFixed(1)} KB/s`;
    }
    return `${bytesPerSec.toFixed(0)} B/s`;
  });

  readonly emulatorService = inject(EmulatorService);
  private readonly snackBar = inject(MatSnackBar);

  async ngOnInit(): Promise<void> {
    await this.emulatorService.loadConfig();
    const config = this.emulatorService.config();
    if (config) {
      this.editConfig = { ...config };
      this.originalConfig = { ...config };
    }
  }

  hasChanges(): boolean {
    if (!this.originalConfig) return false;
    return JSON.stringify(this.editConfig) !== JSON.stringify(this.originalConfig);
  }

  async applyChanges(): Promise<void> {
    try {
      await this.emulatorService.updateConfig(this.editConfig);
      this.originalConfig = { ...this.editConfig };
      this.snackBar.open('Configuration applied', 'OK', { duration: 2000 });
    } catch {
      this.snackBar.open('Failed to apply configuration', 'OK', { duration: 3000 });
    }
  }

  async saveToFile(): Promise<void> {
    try {
      await this.emulatorService.saveConfig();
      this.snackBar.open('Configuration saved to file', 'OK', { duration: 2000 });
    } catch {
      this.snackBar.open('Failed to save configuration', 'OK', { duration: 3000 });
    }
  }

  resetChanges(): void {
    const config = this.emulatorService.config();
    if (config) {
      this.editConfig = { ...config };
    }
  }
}
