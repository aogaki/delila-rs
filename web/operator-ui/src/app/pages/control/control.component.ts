import { Component, inject, OnInit, ViewChild } from '@angular/core';
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
        <app-run-info #runInfo></app-run-info>
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
export class ControlPageComponent implements OnInit {
  private readonly operator = inject(OperatorService);
  private readonly notification = inject(NotificationService);

  @ViewChild('runInfo') runInfo!: RunInfoComponent;
  @ViewChild('controlPanel') controlPanel!: ControlPanelComponent;

  ngOnInit(): void {
    // Polling is started in App component
  }

  onRunStarted(event: { runNumber: number; expName: string }): void {
    this.runInfo.startRun(event.runNumber);
  }

  onRunStopped(): void {
    this.runInfo.stopRun();
  }

  onTimerStarted(): void {
    const runNumber = this.controlPanel.runNumber;
    this.operator.start(runNumber).subscribe({
      next: (res) => {
        if (res.success) {
          this.notification.success(`Started run ${runNumber} with timer`);
          this.runInfo.startRun(runNumber);
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
          this.runInfo.stopRun();
          if (this.controlPanel.autoIncrement()) {
            this.controlPanel.runNumber++;
          }
        }
      },
    });
  }
}
