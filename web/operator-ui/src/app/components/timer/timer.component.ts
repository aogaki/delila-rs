import { Component, inject, output, signal } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MatCardModule } from '@angular/material/card';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatDialog, MatDialogModule } from '@angular/material/dialog';
import { TimerService } from '../../services/timer.service';
import { TimerAlarmDialogComponent } from './timer-alarm-dialog.component';

@Component({
  selector: 'app-timer',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    MatCardModule,
    MatButtonModule,
    MatFormFieldModule,
    MatInputModule,
    MatCheckboxModule,
    MatProgressBarModule,
    MatDialogModule,
  ],
  template: `
    <mat-card [class.flashing]="isFlashing()">
      <mat-card-header>
        <mat-card-title>Timer</mat-card-title>
      </mat-card-header>
      <mat-card-content>
        <div class="timer-form">
          <mat-form-field appearance="outline">
            <mat-label>Duration (minutes)</mat-label>
            <input
              matInput
              type="number"
              [ngModel]="timer.durationMinutes()"
              (ngModelChange)="timer.durationMinutes.set($event)"
              [disabled]="timer.isRunning()"
              min="1"
              max="180"
            />
          </mat-form-field>

          <mat-checkbox
            [ngModel]="startWithTimer()"
            (ngModelChange)="startWithTimer.set($event)"
            [disabled]="timer.isRunning()"
          >
            Start with Timer
          </mat-checkbox>

          <mat-checkbox
            [ngModel]="timer.autoStop()"
            (ngModelChange)="timer.autoStop.set($event)"
            [disabled]="timer.isRunning()"
          >
            Stop when timer expires
          </mat-checkbox>
        </div>

        @if (timer.isRunning()) {
          <div class="timer-display">
            <div class="remaining">{{ timer.remainingDisplay() }}</div>
            <mat-progress-bar mode="determinate" [value]="timer.progress()"></mat-progress-bar>
          </div>
        }

        <div class="timer-buttons">
          @if (!timer.isRunning()) {
            <button mat-raised-button color="primary" (click)="onStartTimer()">Start Timer</button>
          } @else {
            <button mat-raised-button color="warn" (click)="onStopTimer()">Stop Timer</button>
          }
        </div>
      </mat-card-content>
    </mat-card>
  `,
  styles: `
    mat-card {
      height: 100%;
      transition: background-color 0.2s;
    }
    mat-card.flashing {
      animation: flash 0.5s infinite;
    }
    @keyframes flash {
      0%, 100% { background-color: white; }
      50% { background-color: #ffcdd2; }
    }
    .timer-form {
      display: flex;
      flex-direction: column;
      gap: 8px;
      margin-bottom: 16px;
    }
    .timer-display {
      text-align: center;
      margin-bottom: 16px;
    }
    .remaining {
      font-size: 32px;
      font-weight: 500;
      margin-bottom: 8px;
    }
    .timer-buttons {
      display: flex;
      justify-content: center;
    }
    .timer-buttons button {
      min-width: 120px;
    }
  `,
})
export class TimerComponent {
  readonly timer = inject(TimerService);
  private readonly dialog = inject(MatDialog);

  // Options - both enabled by default
  readonly startWithTimer = signal(true);
  readonly isFlashing = signal(false);

  // Events
  readonly timerStarted = output<void>();
  readonly timerExpired = output<void>();

  constructor() {
    // Enable auto stop by default
    this.timer.autoStop.set(true);

    // Set up timer completion callback
    this.timer.onTimerComplete = () => {
      this.showAlarmDialog();
      if (this.timer.autoStop()) {
        this.timerExpired.emit();
      }
    };
  }

  onStartTimer(): void {
    this.timer.startTimer();
    if (this.startWithTimer()) {
      this.timerStarted.emit();
    }
  }

  onStopTimer(): void {
    this.timer.stopTimer();
    this.isFlashing.set(false);
  }

  private showAlarmDialog(): void {
    this.isFlashing.set(true);

    const dialogRef = this.dialog.open(TimerAlarmDialogComponent, {
      disableClose: true,
      width: '400px',
    });

    dialogRef.afterClosed().subscribe(() => {
      this.isFlashing.set(false);
      this.timer.stopAlarm();
    });
  }
}
