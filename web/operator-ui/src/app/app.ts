import { Component, inject, OnInit, ViewChild } from '@angular/core';
import { CommonModule } from '@angular/common';
import { MatToolbarModule } from '@angular/material/toolbar';
import { MatGridListModule } from '@angular/material/grid-list';
import { MatSnackBar, MatSnackBarModule } from '@angular/material/snack-bar';
import { StatusPanelComponent } from './components/status-panel/status-panel.component';
import { ControlPanelComponent } from './components/control-panel/control-panel.component';
import { RunInfoComponent } from './components/run-info/run-info.component';
import { TimerComponent } from './components/timer/timer.component';
import { OperatorService } from './services/operator.service';

@Component({
  selector: 'app-root',
  standalone: true,
  imports: [
    CommonModule,
    MatToolbarModule,
    MatGridListModule,
    MatSnackBarModule,
    StatusPanelComponent,
    ControlPanelComponent,
    RunInfoComponent,
    TimerComponent,
  ],
  template: `
    <mat-toolbar color="primary">
      <span>DELILA DAQ Control</span>
      <span class="spacer"></span>
      <span class="status-indicator" [class.online]="operator.status()" [class.offline]="!operator.status()">
        {{ operator.status() ? 'Online' : 'Offline' }}
      </span>
    </mat-toolbar>

    <div class="main-content">
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
    :host {
      display: flex;
      flex-direction: column;
      height: 100vh;
    }

    mat-toolbar {
      position: sticky;
      top: 0;
      z-index: 100;
    }

    .spacer {
      flex: 1 1 auto;
    }

    .status-indicator {
      padding: 4px 12px;
      border-radius: 12px;
      font-size: 12px;
      font-weight: 500;
    }

    .status-indicator.online {
      background-color: #4caf50;
      color: white;
    }

    .status-indicator.offline {
      background-color: #f44336;
      color: white;
    }

    .main-content {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 16px;
      padding: 16px;
      flex: 1;
      overflow: auto;
    }

    .left-column,
    .right-column {
      display: flex;
      flex-direction: column;
      gap: 16px;
    }

    @media (max-width: 800px) {
      .main-content {
        grid-template-columns: 1fr;
      }
    }
  `,
})
export class App implements OnInit {
  readonly operator = inject(OperatorService);
  private readonly snackBar = inject(MatSnackBar);

  @ViewChild('runInfo') runInfo!: RunInfoComponent;
  @ViewChild('controlPanel') controlPanel!: ControlPanelComponent;

  ngOnInit(): void {
    this.operator.startPolling();
  }

  onRunStarted(event: { runNumber: number; expName: string }): void {
    this.runInfo.startRun(event.runNumber);
  }

  onRunStopped(): void {
    this.runInfo.stopRun();
  }

  // Called when timer starts with "Start with Timer" enabled
  onTimerStarted(): void {
    // Trigger the control panel's start action with the current run number
    const runNumber = this.controlPanel.runNumber;
    this.operator.start(runNumber).subscribe({
      next: (res) => {
        if (res.success) {
          this.snackBar.open(`Started run ${runNumber} with timer`, 'Close', { duration: 3000 });
          this.runInfo.startRun(runNumber);
        } else {
          this.snackBar.open(`Start failed: ${res.message}`, 'Close', { duration: 3000 });
        }
      },
      error: () => {
        this.snackBar.open('Start failed: Network error', 'Close', { duration: 3000 });
      },
    });
  }

  onTimerExpired(): void {
    // Auto stop when timer expires
    this.operator.stop().subscribe({
      next: (res) => {
        if (res.success) {
          this.runInfo.stopRun();
          // Auto increment run number
          if (this.controlPanel.autoIncrement()) {
            this.controlPanel.runNumber++;
          }
        }
      },
    });
  }
}
