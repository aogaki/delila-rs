import { Component, inject, ViewChild } from '@angular/core';
import { MatSnackBarModule } from '@angular/material/snack-bar';
import { StatusPanelComponent } from '../../components/status-panel/status-panel.component';
import { ControlPanelComponent } from '../../components/control-panel/control-panel.component';
import { RunInfoComponent } from '../../components/run-info/run-info.component';
import { TimerComponent } from '../../components/timer/timer.component';
import { OperatorService } from '../../services/operator.service';
import { NotificationService } from '../../services/notification.service';

@Component({
  selector: 'app-control-page',
  standalone: true,
  imports: [
    MatSnackBarModule,
    StatusPanelComponent,
    ControlPanelComponent,
    RunInfoComponent,
    TimerComponent,
  ],
  template: `
    <div class="control-content">
      <div class="left-column">
        <app-status-panel></app-status-panel>
        <app-run-info></app-run-info>
      </div>
      <div class="right-column">
        <app-control-panel
          #controlPanel
          (runStarted)="onRunStarted($event)"
          (runStopped)="onRunStopped()"
        ></app-control-panel>
        <app-timer (timerStarted)="onTimerStarted()" (timerExpired)="onTimerExpired()"></app-timer>
      </div>
    </div>
  `,
  styles: `
    .control-content {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 16px;
      padding: 16px;
      height: 100%;
    }

    .left-column,
    .right-column {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }

    @media (max-width: 800px) {
      .control-content {
        grid-template-columns: 1fr;
      }
    }
  `,
})
export class ControlPageComponent {
  private readonly operator = inject(OperatorService);
  private readonly notification = inject(NotificationService);

  @ViewChild('controlPanel') controlPanel!: ControlPanelComponent;

  // Run info is now managed by the backend and displayed reactively
  // No need for manual startRun/stopRun calls

  onRunStarted(event: { runNumber: number; expName: string }): void {
    // Backend handles run_info update automatically
    // Just log for debugging
    console.log(`Run ${event.runNumber} started (${event.expName})`);
  }

  onRunStopped(): void {
    // Backend handles run_info update automatically
    console.log('Run stopped');
  }

  onTimerStarted(): void {
    const runNumber = this.controlPanel.displayRunNumber();
    this.operator.start(runNumber).subscribe({
      next: (res) => {
        if (res.success) {
          this.notification.success(`Started run ${runNumber} with timer`);
          // Backend handles run_info update automatically
        } else {
          this.notification.error(`Start failed: ${res.message}`);
        }
      },
      error: () => {
        this.notification.error('Start failed: Network error');
      },
    });
  }

  onTimerExpired(): void {
    this.operator.stop().subscribe({
      next: (res) => {
        if (res.success) {
          // Backend handles run_info update automatically
          // Run number will be updated via polling from server
        }
      },
    });
  }
}
