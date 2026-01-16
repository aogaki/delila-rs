import { Component, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { MatCardModule } from '@angular/material/card';
import { MatListModule } from '@angular/material/list';
import { MatIconModule } from '@angular/material/icon';
import { MatTooltipModule } from '@angular/material/tooltip';
import { OperatorService } from '../../services/operator.service';
import { ComponentState } from '../../models/types';

@Component({
  selector: 'app-status-panel',
  standalone: true,
  imports: [CommonModule, MatCardModule, MatListModule, MatIconModule, MatTooltipModule],
  template: `
    <mat-card>
      <mat-card-header>
        <mat-card-title>Component Status</mat-card-title>
      </mat-card-header>
      <mat-card-content>
        <mat-list>
          @for (component of operator.components(); track component.name) {
            <mat-list-item [matTooltip]="component.error ?? ''" [matTooltipDisabled]="!component.error">
              <mat-icon matListItemIcon [style.color]="getStateColor(component.state, component.online)">
                {{ getStateIcon(component.state, component.online) }}
              </mat-icon>
              <span matListItemTitle>{{ component.name }}</span>
              <span matListItemLine>{{ component.state }}</span>
            </mat-list-item>
          }
          @empty {
            <mat-list-item>
              <span matListItemTitle>No components</span>
            </mat-list-item>
          }
        </mat-list>
      </mat-card-content>
    </mat-card>
  `,
  styles: `
    mat-card {
      height: 100%;
    }
    mat-list-item {
      cursor: default;
    }
  `,
})
export class StatusPanelComponent {
  readonly operator = inject(OperatorService);

  getStateColor(state: ComponentState, online: boolean): string {
    if (!online) return '#9e9e9e'; // grey
    switch (state) {
      case 'Running':
        return '#4caf50'; // green
      case 'Error':
        return '#f44336'; // red
      case 'Configured':
      case 'Armed':
        return '#2196f3'; // blue
      case 'Configuring':
      case 'Arming':
      case 'Starting':
      case 'Stopping':
        return '#ff9800'; // orange
      default:
        return '#9e9e9e'; // grey
    }
  }

  getStateIcon(state: ComponentState, online: boolean): string {
    if (!online) return 'cloud_off';
    switch (state) {
      case 'Running':
        return 'play_circle';
      case 'Error':
        return 'error';
      case 'Configured':
      case 'Armed':
        return 'check_circle';
      case 'Idle':
        return 'radio_button_unchecked';
      default:
        return 'pending';
    }
  }
}
