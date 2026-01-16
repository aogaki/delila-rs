import { Component, inject, signal, output } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MatCardModule } from '@angular/material/card';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MatSnackBar, MatSnackBarModule } from '@angular/material/snack-bar';
import { OperatorService } from '../../services/operator.service';

@Component({
  selector: 'app-control-panel',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    MatCardModule,
    MatButtonModule,
    MatFormFieldModule,
    MatInputModule,
    MatCheckboxModule,
    MatSnackBarModule,
  ],
  template: `
    <mat-card>
      <mat-card-header>
        <mat-card-title>Control Panel</mat-card-title>
      </mat-card-header>
      <mat-card-content>
        <div class="form-fields">
          <mat-form-field appearance="outline">
            <mat-label>Experiment Name</mat-label>
            <input matInput [(ngModel)]="expName" [disabled]="!operator.buttonStates().configure" />
          </mat-form-field>

          <mat-form-field appearance="outline">
            <mat-label>Run Number</mat-label>
            <input
              matInput
              type="number"
              [(ngModel)]="runNumber"
              [disabled]="!operator.buttonStates().configure || autoIncrement()"
            />
          </mat-form-field>

          <mat-checkbox [(ngModel)]="autoIncrement" [disabled]="!operator.buttonStates().configure">
            Auto Increment
          </mat-checkbox>

          <mat-form-field appearance="outline">
            <mat-label>Comment</mat-label>
            <textarea matInput [(ngModel)]="comment" rows="2"></textarea>
          </mat-form-field>
        </div>

        <div class="button-grid">
          <button
            mat-raised-button
            color="primary"
            [disabled]="!operator.buttonStates().configure"
            (click)="onConfigure()"
          >
            Configure
          </button>
          <button mat-raised-button [disabled]="!operator.buttonStates().reset" (click)="onReset()">Reset</button>
          <button
            mat-raised-button
            color="accent"
            [disabled]="!canStart()"
            (click)="onStart()"
          >
            Start
          </button>
          <button mat-raised-button color="warn" [disabled]="!operator.buttonStates().stop" (click)="onStop()">
            Stop
          </button>
        </div>

        <div class="state-display">
          <strong>System State:</strong> {{ operator.systemState() }}
        </div>
      </mat-card-content>
    </mat-card>
  `,
  styles: `
    mat-card {
      height: 100%;
    }
    .form-fields {
      display: flex;
      flex-direction: column;
      gap: 8px;
      margin-bottom: 16px;
    }
    .button-grid {
      display: grid;
      grid-template-columns: 1fr 1fr;
      gap: 8px;
      margin-bottom: 16px;
    }
    .state-display {
      text-align: center;
      padding: 8px;
      background: #f5f5f5;
      border-radius: 4px;
    }
  `,
})
export class ControlPanelComponent {
  readonly operator = inject(OperatorService);
  private readonly snackBar = inject(MatSnackBar);

  expName = 'TestExp';
  runNumber = 1;
  comment = '';
  autoIncrement = signal(true);

  // Events for parent component
  readonly runStarted = output<{ runNumber: number; expName: string }>();
  readonly runStopped = output<void>();

  // Start is enabled for both Configured and Armed states
  // (backend will auto-arm if needed)
  canStart(): boolean {
    const state = this.operator.systemState();
    return state === 'Configured' || state === 'Armed';
  }

  onConfigure(): void {
    this.operator.configure({ run_number: this.runNumber, exp_name: this.expName }).subscribe({
      next: (res) => {
        if (res.success) {
          this.showMessage('Configured successfully');
        } else {
          this.showMessage(`Configure failed: ${res.message}`);
        }
      },
      error: () => this.showMessage('Configure failed: Network error'),
    });
  }

  onStart(): void {
    this.operator.start(this.runNumber).subscribe({
      next: (res) => {
        if (res.success) {
          this.showMessage('Started successfully');
          this.runStarted.emit({ runNumber: this.runNumber, expName: this.expName });
        } else {
          this.showMessage(`Start failed: ${res.message}`);
        }
      },
      error: () => this.showMessage('Start failed: Network error'),
    });
  }

  onStop(): void {
    this.operator.stop().subscribe({
      next: (res) => {
        if (res.success) {
          this.showMessage('Stopped successfully');
          this.runStopped.emit();
          // Auto increment run number
          if (this.autoIncrement()) {
            this.runNumber++;
          }
        } else {
          this.showMessage(`Stop failed: ${res.message}`);
        }
      },
      error: () => this.showMessage('Stop failed: Network error'),
    });
  }

  onReset(): void {
    this.operator.reset().subscribe({
      next: (res) => {
        if (res.success) {
          this.showMessage('Reset successfully');
        } else {
          this.showMessage(`Reset failed: ${res.message}`);
        }
      },
      error: () => this.showMessage('Reset failed: Network error'),
    });
  }

  private showMessage(message: string): void {
    this.snackBar.open(message, 'Close', { duration: 3000 });
  }
}
