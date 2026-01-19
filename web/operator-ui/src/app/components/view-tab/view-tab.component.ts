import {
  Component,
  Input,
  Output,
  EventEmitter,
  OnInit,
  OnDestroy,
  inject,
  signal,
  ViewChildren,
  QueryList,
} from '@angular/core';
import { interval, Subject, takeUntil, switchMap, forkJoin, of } from 'rxjs';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { HistogramChartComponent, RangeChangeEvent } from '../histogram-chart/histogram-chart.component';
import { HistogramService } from '../../services/histogram.service';
import { ViewTab, Histogram1D } from '../../models/histogram.types';

@Component({
  selector: 'app-view-tab',
  standalone: true,
  imports: [HistogramChartComponent, MatButtonModule, MatIconModule],
  template: `
    <div class="view-container">
      <!-- Toolbar -->
      <div class="view-toolbar">
        <button
          mat-stroked-button
          (click)="onApplyRangeToAll()"
          [disabled]="!hasLockedCell()"
          title="Apply the range from the first locked cell to all cells"
        >
          <mat-icon>content_copy</mat-icon>
          Apply Range to All
        </button>
        <button
          mat-stroked-button
          (click)="onResetAllRanges()"
          [disabled]="!hasLockedCell()"
        >
          <mat-icon>restart_alt</mat-icon>
          Reset All
        </button>
        <button
          mat-stroked-button
          (click)="onToggleLogScale()"
          [class.active]="isAllLogScale()"
          title="Toggle logarithmic Y-axis scale"
        >
          <mat-icon>{{ isAllLogScale() ? 'linear_scale' : 'show_chart' }}</mat-icon>
          {{ isAllLogScale() ? 'Linear' : 'Log' }}
        </button>
        <button
          mat-stroked-button
          (click)="onSaveImage()"
          [disabled]="isSaving()"
          title="Save grid as PNG image"
        >
          <mat-icon>save_alt</mat-icon>
          {{ isSaving() ? 'Saving...' : 'Save Image' }}
        </button>
        <span class="toolbar-hint">
          Drag to select range, Ctrl+Scroll for X-axis zoom
        </span>
      </div>

      <!-- Grid -->
      <div
        class="view-grid"
        [style.grid-template-rows]="'repeat(' + tab.gridRows + ', 1fr)'"
        [style.grid-template-columns]="'repeat(' + tab.gridCols + ', 1fr)'"
      >
        @for (cell of tab.cells; track $index; let i = $index) {
          @if (i < tab.gridRows * tab.gridCols && !cell.isEmpty) {
            <div
              class="view-cell"
              [class.locked]="cell.isLocked"
              (dblclick)="onCellDoubleClick(i)"
            >
              <div class="cell-label">
                Src{{ cell.sourceId }}/Ch{{ cell.channelId }}
                @if (cell.isLocked) {
                  <span class="lock-icon">🔒</span>
                }
              </div>
              <div class="chart-wrapper">
                <app-histogram-chart
                  #chartRef
                  [histogram]="histograms()[i] ?? null"
                  [xRange]="cell.xRange"
                  [yRange]="cell.yRange"
                  [showDataZoom]="true"
                  [logScale]="cell.logScale ?? false"
                  [xAxisLabel]="tab.xAxisLabel"
                  (rangeChange)="onRangeChange(i, $event)"
                ></app-histogram-chart>
              </div>
              <button class="expand-button" (click)="onExpandClick(i)" title="Expand">
                <span class="expand-icon">&#x26F6;</span>
              </button>
            </div>
          } @else if (i < tab.gridRows * tab.gridCols) {
            <div class="view-cell empty"></div>
          }
        }
      </div>
    </div>
  `,
  styles: `
    :host {
      display: block;
      height: 100%;
    }

    .view-container {
      display: flex;
      flex-direction: column;
      height: 100%;
      gap: 8px;
    }

    .view-toolbar {
      display: flex;
      align-items: center;
      gap: 12px;
      flex-shrink: 0;
    }

    .toolbar-hint {
      font-size: 12px;
      color: #666;
      margin-left: auto;
    }

    .view-grid {
      display: grid;
      gap: 4px;
      flex: 1;
      min-height: 0;
    }

    .view-cell {
      position: relative;
      background: white;
      border: 1px solid #e0e0e0;
      border-radius: 4px;
      overflow: hidden;
      display: flex;
      flex-direction: column;
    }

    .view-cell.empty {
      background: #fafafa;
    }

    .view-cell.locked {
      border-color: #1976d2;
      border-width: 2px;
    }

    .cell-label {
      position: absolute;
      top: 2px;
      left: 4px;
      font-size: 10px;
      color: #666;
      z-index: 1;
      background: rgba(255, 255, 255, 0.8);
      padding: 0 4px;
      border-radius: 2px;
    }

    .lock-icon {
      font-size: 8px;
      margin-left: 2px;
    }

    .chart-wrapper {
      flex: 1;
      min-height: 0;
    }

    .expand-button {
      position: absolute;
      top: 2px;
      right: 2px;
      width: 24px;
      height: 24px;
      border: none;
      background: rgba(255, 255, 255, 0.8);
      border-radius: 4px;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      opacity: 0;
      transition: opacity 0.2s;
      z-index: 1;
    }

    .view-cell:hover .expand-button {
      opacity: 1;
    }

    .expand-button:hover {
      background: #e0e0e0;
    }

    .expand-icon {
      font-size: 14px;
    }

    button.active {
      background-color: #1976d2;
      color: white;
    }
  `,
})
export class ViewTabComponent implements OnInit, OnDestroy {
  @Input() tab!: ViewTab;
  @Output() tabChange = new EventEmitter<ViewTab>();
  @Output() cellExpand = new EventEmitter<number>();

  @ViewChildren('chartRef') chartRefs!: QueryList<HistogramChartComponent>;

  private readonly histogramService = inject(HistogramService);
  private readonly destroy$ = new Subject<void>();
  private readonly refreshInterval = 1000;

  readonly histograms = signal<(Histogram1D | null)[]>([]);
  readonly isSaving = signal(false);

  ngOnInit(): void {
    // Initialize histogram array
    this.histograms.set(new Array(this.tab.cells.length).fill(null));

    // Start auto-refresh
    interval(this.refreshInterval)
      .pipe(
        takeUntil(this.destroy$),
        switchMap(() => this.fetchAllHistograms())
      )
      .subscribe((results) => {
        this.histograms.set(results);
      });

    // Initial fetch
    this.fetchAllHistograms().subscribe((results) => {
      this.histograms.set(results);
    });
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  private fetchAllHistograms() {
    const requests = this.tab.cells.map((cell) => {
      if (cell.isEmpty) {
        return of(null);
      }
      return this.histogramService.fetchAndCacheHistogram(cell.sourceId, cell.channelId);
    });

    return forkJoin(requests);
  }

  onRangeChange(index: number, event: RangeChangeEvent): void {
    const cells = [...this.tab.cells];
    cells[index] = {
      ...cells[index],
      xRange: event.xRange,
      yRange: event.yRange,
      isLocked: true,
    };
    this.tabChange.emit({ ...this.tab, cells });
  }

  onCellDoubleClick(index: number): void {
    this.cellExpand.emit(index);
  }

  onExpandClick(index: number): void {
    this.cellExpand.emit(index);
  }

  hasLockedCell(): boolean {
    return this.tab.cells.some((cell) => cell.isLocked);
  }

  isAllLogScale(): boolean {
    const nonEmptyCells = this.tab.cells.filter((cell) => !cell.isEmpty);
    return nonEmptyCells.length > 0 && nonEmptyCells.every((cell) => cell.logScale);
  }

  onToggleLogScale(): void {
    const newLogScale = !this.isAllLogScale();
    const cells = this.tab.cells.map((cell) => ({
      ...cell,
      logScale: cell.isEmpty ? false : newLogScale,
    }));
    this.tabChange.emit({ ...this.tab, cells });
  }

  onApplyRangeToAll(): void {
    // Find the first locked cell
    const lockedCell = this.tab.cells.find((cell) => cell.isLocked);
    if (!lockedCell) return;

    // Apply its range to all non-empty cells
    const cells = this.tab.cells.map((cell) => {
      if (cell.isEmpty) return cell;
      return {
        ...cell,
        xRange: lockedCell.xRange,
        yRange: lockedCell.yRange,
        isLocked: true,
      };
    });

    this.tabChange.emit({ ...this.tab, cells });
  }

  onResetAllRanges(): void {
    // Reset all cells to auto range
    const cells = this.tab.cells.map((cell) => ({
      ...cell,
      xRange: 'auto' as const,
      yRange: 'auto' as const,
      isLocked: false,
    }));

    this.tabChange.emit({ ...this.tab, cells });
  }

  async onSaveImage(): Promise<void> {
    if (this.isSaving()) return;
    this.isSaving.set(true);

    try {
      const charts = this.chartRefs.toArray();
      if (charts.length === 0) {
        this.isSaving.set(false);
        return;
      }

      // Get chart images as data URLs
      const chartImages: HTMLImageElement[] = [];
      for (const chart of charts) {
        const dataUrl = chart.getDataURL(2);
        if (dataUrl) {
          const img = await this.loadImage(dataUrl);
          chartImages.push(img);
        }
      }

      if (chartImages.length === 0) {
        this.isSaving.set(false);
        return;
      }

      // Calculate canvas size
      const cellWidth = chartImages[0].width;
      const cellHeight = chartImages[0].height;
      const cols = this.tab.gridCols;
      const rows = this.tab.gridRows;
      const padding = 4;
      const labelHeight = 24;

      const canvasWidth = cols * cellWidth + (cols + 1) * padding;
      const canvasHeight = rows * (cellHeight + labelHeight) + (rows + 1) * padding;

      // Create canvas
      const canvas = document.createElement('canvas');
      canvas.width = canvasWidth;
      canvas.height = canvasHeight;
      const ctx = canvas.getContext('2d');
      if (!ctx) {
        this.isSaving.set(false);
        return;
      }

      // Fill background
      ctx.fillStyle = '#f5f5f5';
      ctx.fillRect(0, 0, canvasWidth, canvasHeight);

      // Draw each chart with label
      let chartIndex = 0;
      for (let row = 0; row < rows; row++) {
        for (let col = 0; col < cols; col++) {
          const cellIndex = row * cols + col;
          const cell = this.tab.cells[cellIndex];

          if (!cell || cell.isEmpty) continue;

          const x = padding + col * (cellWidth + padding);
          const y = padding + row * (cellHeight + labelHeight + padding);

          // Draw label background
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(x, y, cellWidth, labelHeight);

          // Draw label text
          ctx.fillStyle = '#333333';
          ctx.font = '14px sans-serif';
          ctx.textBaseline = 'middle';
          const label = `Src${cell.sourceId}/Ch${cell.channelId}`;
          ctx.fillText(label, x + 8, y + labelHeight / 2);

          // Draw chart image
          if (chartIndex < chartImages.length) {
            ctx.drawImage(chartImages[chartIndex], x, y + labelHeight);
            chartIndex++;
          }
        }
      }

      // Download as PNG
      const link = document.createElement('a');
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
      link.download = `${this.tab.name}_${timestamp}.png`;
      link.href = canvas.toDataURL('image/png');
      link.click();
    } finally {
      this.isSaving.set(false);
    }
  }

  private loadImage(src: string): Promise<HTMLImageElement> {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => resolve(img);
      img.onerror = reject;
      img.src = src;
    });
  }
}
