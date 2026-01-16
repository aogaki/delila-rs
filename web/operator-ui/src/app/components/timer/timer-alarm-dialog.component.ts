import { Component, inject, OnInit, OnDestroy } from '@angular/core';
import { MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatButtonModule } from '@angular/material/button';
import { TimerService } from '../../services/timer.service';

@Component({
  selector: 'app-timer-alarm-dialog',
  standalone: true,
  imports: [MatDialogModule, MatButtonModule],
  template: `
    <h2 mat-dialog-title>Timer Complete!</h2>
    <mat-dialog-content>
      <p>The timer has expired.</p>
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button mat-raised-button color="primary" (click)="onClose()">Dismiss</button>
    </mat-dialog-actions>
  `,
  styles: `
    :host {
      display: block;
      text-align: center;
    }
    h2 {
      color: #f44336;
    }
    p {
      font-size: 18px;
      margin: 16px 0;
    }
  `,
})
export class TimerAlarmDialogComponent implements OnInit, OnDestroy {
  private readonly dialogRef = inject(MatDialogRef<TimerAlarmDialogComponent>);
  private readonly timer = inject(TimerService);

  ngOnInit(): void {
    this.timer.startAlarm();
  }

  ngOnDestroy(): void {
    this.timer.stopAlarm();
  }

  onClose(): void {
    this.dialogRef.close();
  }
}
