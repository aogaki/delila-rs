import { Component, inject, OnInit, computed } from '@angular/core';
import { RouterModule, RouterLink, RouterLinkActive } from '@angular/router';
import { MatToolbarModule } from '@angular/material/toolbar';
import { MatTabsModule } from '@angular/material/tabs';
import { MatSnackBarModule } from '@angular/material/snack-bar';
import { OperatorService } from './services/operator.service';
import { HistogramService } from './services/histogram.service';

@Component({
  selector: 'app-root',
  standalone: true,
  imports: [
    RouterModule,
    RouterLink,
    RouterLinkActive,
    MatToolbarModule,
    MatTabsModule,
    MatSnackBarModule,
  ],
  template: `
    <mat-toolbar color="primary" class="header">
      <span class="title">DELILA DAQ</span>
      <span class="header-stats">
        <span class="stat-item" [class.running]="operator.systemState() === 'Running'">
          {{ operator.systemState() }}
        </span>
        <span class="stat-separator">|</span>
        <span class="stat-item">{{ formatEvents(operator.totalEvents()) }} events</span>
        <span class="stat-separator">|</span>
        <span class="stat-item">{{ formatRate(operator.totalRate()) }}</span>
      </span>
      <span class="spacer"></span>
      @if (currentRunNumber()) {
        <span class="run-info">Run: {{ currentRunNumber() }}</span>
      }
      <span
        class="status-indicator"
        [class.online]="operator.status()"
        [class.offline]="!operator.status()"
      >
        {{ operator.status() ? 'Online' : 'Offline' }}
      </span>
    </mat-toolbar>

    <nav mat-tab-nav-bar [tabPanel]="tabPanel" class="nav-tabs">
      <a
        mat-tab-link
        routerLink="/control"
        routerLinkActive
        #rla1="routerLinkActive"
        [active]="rla1.isActive"
      >
        Control
      </a>
      <a
        mat-tab-link
        routerLink="/monitor"
        routerLinkActive
        #rla2="routerLinkActive"
        [active]="rla2.isActive"
      >
        Monitor
      </a>
      <a
        mat-tab-link
        routerLink="/waveform"
        routerLinkActive
        #rla3="routerLinkActive"
        [active]="rla3.isActive"
      >
        Waveform
      </a>
    </nav>

    <mat-tab-nav-panel #tabPanel class="tab-content">
      <router-outlet></router-outlet>
    </mat-tab-nav-panel>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      height: 100vh;
    }

    .header {
      position: sticky;
      top: 0;
      z-index: 100;
    }

    .title {
      font-weight: 500;
      margin-right: 24px;
    }

    .header-stats {
      display: flex;
      align-items: center;
      gap: 8px;
      font-size: 14px;
    }

    .stat-item {
      opacity: 0.9;
    }

    .stat-item.running {
      color: #4caf50;
      font-weight: 500;
    }

    .stat-separator {
      opacity: 0.5;
    }

    .spacer {
      flex: 1 1 auto;
    }

    .run-info {
      margin-right: 16px;
      padding: 4px 12px;
      background-color: rgba(255, 255, 255, 0.1);
      border-radius: 4px;
      font-size: 14px;
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

    .nav-tabs {
      background-color: white;
      border-bottom: 1px solid rgba(0, 0, 0, 0.12);
    }

    .tab-content {
      flex: 1;
      overflow: auto;
    }
  `,
})
export class App implements OnInit {
  readonly operator = inject(OperatorService);
  readonly histogram = inject(HistogramService);

  // Computed: get run number from any component that has one
  readonly currentRunNumber = computed(() => {
    const components = this.operator.components();
    for (const comp of components) {
      if (comp.run_number !== undefined) {
        return comp.run_number;
      }
    }
    return null;
  });

  ngOnInit(): void {
    this.operator.startPolling();
    this.histogram.startPolling();
  }

  formatEvents(events: number): string {
    if (events >= 1_000_000) {
      return (events / 1_000_000).toFixed(2) + 'M';
    } else if (events >= 1_000) {
      return (events / 1_000).toFixed(1) + 'K';
    }
    return events.toString();
  }

  formatRate(rate: number): string {
    if (rate >= 1_000_000) {
      return (rate / 1_000_000).toFixed(2) + 'M eve/s';
    } else if (rate >= 1_000) {
      return (rate / 1_000).toFixed(1) + 'k eve/s';
    }
    return rate.toFixed(0) + ' eve/s';
  }
}
