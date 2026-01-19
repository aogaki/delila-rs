import { Component, OnInit, OnDestroy, inject, signal, computed } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatDialog } from '@angular/material/dialog';
import { SetupTabComponent } from '../../components/setup-tab/setup-tab.component';
import { ViewTabComponent } from '../../components/view-tab/view-tab.component';
import {
  HistogramExpandDialogComponent,
  ExpandDialogData,
  ExpandDialogResult,
} from '../../components/histogram-expand-dialog/histogram-expand-dialog.component';
import { HistogramService } from '../../services/histogram.service';
import {
  MonitorState,
  SetupConfig,
  ViewTab,
  createDefaultSetupConfig,
  createViewTabFromSetup,
} from '../../models/histogram.types';

const STORAGE_KEY = 'delila-monitor-state';

@Component({
  selector: 'app-monitor-page',
  standalone: true,
  imports: [
    MatButtonModule,
    MatIconModule,
    SetupTabComponent,
    ViewTabComponent,
  ],
  template: `
    <div class="monitor-page">
      <!-- Tab bar -->
      <div class="tab-bar">
        <div class="tabs-container">
          <!-- Setup tab (always first) -->
          <button
            class="tab-button setup-tab"
            [class.active]="activeTabId() === null"
            (click)="selectSetupTab()"
          >
            <span class="tab-icon">+</span>
            Setup
          </button>

          <!-- View tabs -->
          @for (tab of viewTabs(); track tab.id) {
            <button
              class="tab-button"
              [class.active]="activeTabId() === tab.id"
              (click)="selectViewTab(tab.id)"
              (dblclick)="renameViewTab(tab.id)"
            >
              {{ tab.name }}
              <span
                class="tab-close"
                (click)="removeViewTab(tab.id, $event)"
                title="Close"
              >×</span>
            </button>
          }
        </div>
      </div>

      <!-- Tab content -->
      <div class="tab-content">
        @if (activeTabId() === null) {
          <!-- Setup tab content -->
          <app-setup-tab
            [config]="setupConfig()"
            (configChange)="onSetupConfigChange($event)"
            (createView)="onCreateView($event)"
          ></app-setup-tab>
        } @else if (activeViewTab(); as tab) {
          <!-- View tab content -->
          <app-view-tab
            [tab]="tab"
            (tabChange)="onViewTabChange($event)"
            (cellExpand)="onCellExpand($event)"
          ></app-view-tab>
        }
      </div>
    </div>
  `,
  styles: `
    .monitor-page {
      display: flex;
      flex-direction: column;
      height: 100%;
      padding: 16px;
      gap: 8px;
    }

    .tab-bar {
      display: flex;
      align-items: center;
      background-color: #f5f5f5;
      border-radius: 4px;
      padding: 4px;
    }

    .tabs-container {
      display: flex;
      align-items: center;
      gap: 4px;
      flex: 1;
      overflow-x: auto;
    }

    .tab-button {
      position: relative;
      padding: 8px 28px 8px 12px;
      border: none;
      background: transparent;
      cursor: pointer;
      border-radius: 4px;
      font-size: 14px;
      white-space: nowrap;
      transition: background-color 0.2s;
    }

    .tab-button:hover {
      background-color: #e0e0e0;
    }

    .tab-button.active {
      background-color: white;
      box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
    }

    .tab-button.setup-tab {
      padding: 8px 16px;
      font-weight: 500;
      color: #1976d2;
    }

    .tab-icon {
      margin-right: 4px;
    }

    .tab-close {
      position: absolute;
      right: 6px;
      top: 50%;
      transform: translateY(-50%);
      width: 16px;
      height: 16px;
      display: flex;
      align-items: center;
      justify-content: center;
      border-radius: 50%;
      font-size: 14px;
      line-height: 1;
      opacity: 0.5;
    }

    .tab-close:hover {
      opacity: 1;
      background-color: rgba(0, 0, 0, 0.1);
    }

    .tab-content {
      flex: 1;
      min-height: 0;
      overflow: hidden;
    }
  `,
})
export class MonitorPageComponent implements OnInit, OnDestroy {
  private readonly dialog = inject(MatDialog);
  readonly histogramService = inject(HistogramService);

  readonly setupConfig = signal<SetupConfig>(createDefaultSetupConfig());
  readonly viewTabs = signal<ViewTab[]>([]);
  readonly activeTabId = signal<string | null>(null);

  readonly activeViewTab = computed(() => {
    const id = this.activeTabId();
    if (id === null) return null;
    return this.viewTabs().find((t) => t.id === id) ?? null;
  });

  ngOnInit(): void {
    this.loadState();
    this.histogramService.startPolling();
  }

  ngOnDestroy(): void {
    this.histogramService.stopPolling();
  }

  selectSetupTab(): void {
    this.activeTabId.set(null);
    this.saveState();
  }

  selectViewTab(id: string): void {
    this.activeTabId.set(id);
    this.saveState();
  }

  renameViewTab(id: string): void {
    const tab = this.viewTabs().find((t) => t.id === id);
    if (!tab) return;

    const name = prompt('Enter new name:', tab.name);
    if (!name) return;

    this.viewTabs.update((tabs) => tabs.map((t) => (t.id === id ? { ...t, name } : t)));
    this.saveState();
  }

  removeViewTab(id: string, event: Event): void {
    event.stopPropagation();

    const confirmed = confirm('Remove this view?');
    if (!confirmed) return;

    const currentId = this.activeTabId();
    const tabs = this.viewTabs();
    const currentIndex = tabs.findIndex((t) => t.id === id);

    this.viewTabs.update((tabs) => tabs.filter((t) => t.id !== id));

    // If removing active tab, switch to adjacent or setup
    if (currentId === id) {
      const newTabs = this.viewTabs();
      if (newTabs.length === 0) {
        this.activeTabId.set(null);
      } else {
        const newIndex = Math.min(currentIndex, newTabs.length - 1);
        this.activeTabId.set(newTabs[newIndex].id);
      }
    }

    this.saveState();
  }

  onSetupConfigChange(config: SetupConfig): void {
    this.setupConfig.set(config);
    this.saveState();
  }

  onCreateView(config: SetupConfig): void {
    const viewTab = createViewTabFromSetup(config);
    if (!viewTab) {
      alert('Please select at least one channel.');
      return;
    }

    this.viewTabs.update((tabs) => [...tabs, viewTab]);
    this.activeTabId.set(viewTab.id);

    // Reset setup for next view
    this.setupConfig.set(createDefaultSetupConfig());
    this.saveState();
  }

  onViewTabChange(tab: ViewTab): void {
    this.viewTabs.update((tabs) => tabs.map((t) => (t.id === tab.id ? tab : t)));
    this.saveState();
  }

  onCellExpand(cellIndex: number): void {
    const tab = this.activeViewTab();
    if (!tab) return;

    const cell = tab.cells[cellIndex];
    if (!cell || cell.isEmpty) return;

    const dialogData: ExpandDialogData = {
      cell,
      cellIndex,
      xAxisLabel: tab.xAxisLabel,
    };

    const dialogRef = this.dialog.open(HistogramExpandDialogComponent, {
      data: dialogData,
      panelClass: 'histogram-expand-dialog-panel',
      autoFocus: false,
      maxWidth: '95vw',
      maxHeight: '90vh',
    });

    dialogRef.afterClosed().subscribe((result: ExpandDialogResult | undefined) => {
      if (result) {
        // Update the cell with any changes from the dialog
        const updatedCells = [...tab.cells];
        updatedCells[cellIndex] = result.cell;
        this.viewTabs.update((tabs) =>
          tabs.map((t) => (t.id === tab.id ? { ...t, cells: updatedCells } : t))
        );
        this.saveState();
      }
    });
  }

  private loadState(): void {
    try {
      const stored = localStorage.getItem(STORAGE_KEY);
      if (stored) {
        const state: MonitorState = JSON.parse(stored);
        this.setupConfig.set(state.setupConfig);
        this.viewTabs.set(state.viewTabs);
        this.activeTabId.set(state.activeTabId);
        return;
      }
    } catch {
      console.warn('Failed to load monitor state');
    }

    // Default state
    this.setupConfig.set(createDefaultSetupConfig());
    this.viewTabs.set([]);
    this.activeTabId.set(null);
  }

  private saveState(): void {
    const state: MonitorState = {
      setupConfig: this.setupConfig(),
      viewTabs: this.viewTabs(),
      activeTabId: this.activeTabId(),
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  }
}
