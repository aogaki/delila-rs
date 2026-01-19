import { Component, inject, computed } from '@angular/core';
import { CommonModule, DecimalPipe } from '@angular/common';
import { MatCardModule } from '@angular/material/card';
import { OperatorService } from '../../services/operator.service';

@Component({
  selector: 'app-run-info',
  standalone: true,
  imports: [CommonModule, MatCardModule, DecimalPipe],
  template: `
    <mat-card>
      <mat-card-header>
        <mat-card-title>Run Information</mat-card-title>
      </mat-card-header>
      <mat-card-content>
        <div class="info-grid">
          <div class="info-item">
            <span class="label">Run #</span>
            <span class="value">{{ runNumber() }}</span>
          </div>
          <div class="info-item">
            <span class="label">Started</span>
            <span class="value">{{ startTimeDisplay() }}</span>
          </div>
          <div class="info-item">
            <span class="label">Elapsed</span>
            <span class="value">{{ elapsedDisplay() }}</span>
          </div>
          <div class="info-item">
            <span class="label">Events</span>
            <span class="value">{{ totalEvents() | number }}</span>
          </div>
          <div class="info-item">
            <span class="label">Rate</span>
            <span class="value">{{ formatRate(totalRate()) }}</span>
          </div>
        </div>
      </mat-card-content>
    </mat-card>
  `,
  styles: `
    mat-card {
      height: 100%;
    }
    .info-grid {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 16px;
    }
    .info-item {
      display: flex;
      flex-direction: column;
    }
    .label {
      font-size: 12px;
      color: #666;
      text-transform: uppercase;
    }
    .value {
      font-size: 20px;
      font-weight: 500;
    }
  `,
})
export class RunInfoComponent {
  private readonly operator = inject(OperatorService);

  // Get run info from backend (via OperatorService)
  readonly runInfo = this.operator.runInfo;

  // Computed values derived from backend run_info
  readonly runNumber = computed(() => {
    const info = this.runInfo();
    return info ? info.run_number : '-';
  });

  readonly startTimeDisplay = computed(() => {
    const info = this.runInfo();
    if (!info) return '--:--:--';
    const date = new Date(info.start_time);
    return date.toLocaleTimeString();
  });

  readonly elapsedDisplay = computed(() => {
    const info = this.runInfo();
    if (!info) return '00:00:00';

    const total = info.elapsed_secs;
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const seconds = total % 60;

    return `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
  });

  // Use stats from run_info if available, otherwise fall back to aggregated component metrics
  readonly totalEvents = computed(() => {
    const info = this.runInfo();
    if (info && info.stats.total_events > 0) {
      return info.stats.total_events;
    }
    // Fallback to component aggregation
    return this.operator.totalEvents();
  });

  readonly totalRate = computed(() => {
    const info = this.runInfo();
    if (info && info.stats.average_rate > 0) {
      return info.stats.average_rate;
    }
    // Fallback to component aggregation
    return this.operator.totalRate();
  });

  formatRate(rate: number): string {
    if (rate >= 1_000_000) {
      return `${(rate / 1_000_000).toFixed(2)} Mevt/s`;
    } else if (rate >= 1_000) {
      return `${(rate / 1_000).toFixed(2)} kevt/s`;
    }
    return `${rate.toFixed(0)} evt/s`;
  }
}
