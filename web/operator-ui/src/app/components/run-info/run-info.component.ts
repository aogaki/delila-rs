import { Component, inject, signal, computed, OnDestroy } from '@angular/core';
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
            <span class="value">{{ currentRunNumber() ?? '-' }}</span>
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
            <span class="value">{{ operator.totalEvents() | number }}</span>
          </div>
          <div class="info-item">
            <span class="label">Rate</span>
            <span class="value">{{ formatRate(operator.totalRate()) }}</span>
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
export class RunInfoComponent implements OnDestroy {
  readonly operator = inject(OperatorService);

  readonly currentRunNumber = signal<number | null>(null);
  readonly startTime = signal<Date | null>(null);
  readonly elapsedSeconds = signal(0);

  private intervalId: ReturnType<typeof setInterval> | null = null;

  readonly startTimeDisplay = computed(() => {
    const time = this.startTime();
    if (!time) return '--:--:--';
    return time.toLocaleTimeString();
  });

  readonly elapsedDisplay = computed(() => {
    const total = this.elapsedSeconds();
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const seconds = total % 60;

    return `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
  });

  startRun(runNumber: number): void {
    this.currentRunNumber.set(runNumber);
    this.startTime.set(new Date());
    this.elapsedSeconds.set(0);

    // Start elapsed time counter
    this.intervalId = setInterval(() => {
      this.elapsedSeconds.update((v) => v + 1);
    }, 1000);
  }

  stopRun(): void {
    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }

  resetRun(): void {
    this.stopRun();
    this.currentRunNumber.set(null);
    this.startTime.set(null);
    this.elapsedSeconds.set(0);
  }

  formatRate(rate: number): string {
    if (rate >= 1_000_000) {
      return `${(rate / 1_000_000).toFixed(2)} Mevt/s`;
    } else if (rate >= 1_000) {
      return `${(rate / 1_000).toFixed(2)} kevt/s`;
    }
    return `${rate.toFixed(0)} evt/s`;
  }

  ngOnDestroy(): void {
    this.stopRun();
  }
}
