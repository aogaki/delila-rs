import {
  Component,
  Input,
  Output,
  EventEmitter,
  OnInit,
  OnDestroy,
  inject,
  signal,
  computed,
  ViewChildren,
  QueryList,
} from '@angular/core';
import { interval, Subject, takeUntil, switchMap, forkJoin, of, catchError } from 'rxjs';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { HistogramChartComponent, RangeChangeEvent } from '../histogram-chart/histogram-chart.component';
import { HeatmapChartComponent } from '../heatmap-chart/heatmap-chart.component';
import { HistogramService } from '../../services/histogram.service';
import { DigitizerService } from '../../services/digitizer.service';
import {
  AxisSource,
  AxisView,
  ViewTab,
  Histogram1D,
  Histogram2D,
  AXIS_SOURCE_LABEL,
  DEFAULT_AXIS_VIEW,
  defaultYAxisSource,
} from '../../models/histogram.types';

@Component({
  selector: 'app-view-tab',
  standalone: true,
  imports: [HistogramChartComponent, HeatmapChartComponent, MatButtonModule, MatIconModule],
  template: `
    <div class="view-container">
      <!-- Toolbar -->
      <div class="view-toolbar">
        @if (!is2dHistType(histType())) {
          <button
            mat-stroked-button
            (click)="onApplyRangeToAll()"
            [disabled]="!hasLockedCell()"
            title="Apply the range from the last zoomed cell to all cells"
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
        }
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
        <!-- Live rebin sliders (1D shows X only, 2D shows X+Y). The slider
             value is a *rebin factor*: 1 = server's native resolution,
             N = combine N native bins into one displayed bin. The chart
             rebins client-side so the picture updates within one frame. -->
        <div class="binning-controls">
          <label class="binning-control">
            <span class="binning-label">{{ is2dHistType(histType()) ? 'X' : '' }} rebin:</span>
            <input
              type="range"
              [min]="1"
              [max]="maxXRebinFactor()"
              [step]="1"
              [value]="xRebinFactor()"
              (input)="onXRebinSlider($any($event.target).value)"
            />
            <span class="binning-value" [title]="xView().bins + ' bins displayed'">
              {{ xRebinFactor() }}
            </span>
          </label>
          @if (is2dHistType(histType())) {
            <label class="binning-control">
              <span class="binning-label">Y rebin:</span>
              <input
                type="range"
                [min]="1"
                [max]="maxYRebinFactor()"
                [step]="1"
                [value]="yRebinFactor()"
                (input)="onYRebinSlider($any($event.target).value)"
              />
              <span class="binning-value" [title]="yView().bins + ' bins displayed'">
                {{ yRebinFactor() }}
              </span>
            </label>
          }
        </div>
        @if (!is2dHistType(histType())) {
          <span class="toolbar-hint">
            Drag to select range, Ctrl+Scroll for X-axis zoom
          </span>
        }
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
                @if (!is2dHistType(histType())) {
                  <app-histogram-chart
                    #chartRef
                    [histogram]="histograms()[i] ?? null"
                    [xRange]="cell.xRange"
                    [yRange]="cell.yRange"
                    [showDataZoom]="true"
                    [logScale]="cell.logScale ?? false"
                    [xAxisLabel]="tab.xAxisLabel"
                    [xView]="xView()"
                    (rangeChange)="onRangeChange(i, $event)"
                  ></app-histogram-chart>
                } @else {
                  <app-heatmap-chart
                    [histogram]="histograms2d()[i] ?? null"
                    [logScale]="cell.logScale ?? true"
                    [xAxisLabel]="axisLabel(xAxis())"
                    [yAxisLabel]="axisLabel(yAxis())"
                    [xView]="xView()"
                    [yView]="yView()"
                  ></app-heatmap-chart>
                }
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

    .binning-controls {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 0 8px;
      border-left: 1px solid #ddd;
      border-right: 1px solid #ddd;
    }

    .binning-control {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      font-size: 12px;
      color: #444;
      cursor: pointer;
    }

    .binning-control input[type='range'] {
      width: 100px;
    }

    .binning-label {
      white-space: nowrap;
    }

    .binning-value {
      min-width: 32px;
      font-variant-numeric: tabular-nums;
      font-weight: 500;
      color: #1976d2;
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
  /**
   * `tab` is exposed as a normal property (preserving the existing
   * `this.tab.xxx` call-sites) but backed by a signal so `computed()`s
   * that read it (xView, xRebinFactor, etc.) invalidate when the parent
   * passes a new tab object. Without this the slider's displayed value
   * stays frozen at its initial reading because the cached computed
   * never sees the parent's update.
   */
  private readonly _tab = signal<ViewTab>(undefined as unknown as ViewTab);
  @Input({ required: true }) set tab(value: ViewTab) {
    this._tab.set(value);
  }
  get tab(): ViewTab {
    return this._tab();
  }
  @Output() tabChange = new EventEmitter<ViewTab>();
  @Output() cellExpand = new EventEmitter<number>();

  @ViewChildren('chartRef') chartRefs!: QueryList<HistogramChartComponent>;

  private readonly histogramService = inject(HistogramService);
  private readonly digitizerService = inject(DigitizerService);
  private readonly destroy$ = new Subject<void>();
  private readonly refreshInterval = 1000;

  readonly histograms = signal<(Histogram1D | null)[]>([]);
  readonly histograms2d = signal<(Histogram2D | null)[]>([]);
  readonly isSaving = signal(false);
  readonly histType = computed(() => this.tab.histogramType ?? 'energy');
  /** Effective X / Y axis for 2D plots. Falls back to (energy, FW default) so
   *  old layout files written before Phase 2 still render something sensible —
   *  UserInfo[0] on all-AMax systems, psd otherwise (issue #25). */
  readonly xAxis = computed<AxisSource>(() => this.tab.xAxis ?? 'energy');
  readonly yAxis = computed<AxisSource>(
    () => this.tab.yAxis ?? defaultYAxisSource(this.digitizerService.allAMax()),
  );

  /**
   * Live view config — the chart components apply this client-side rebin.
   * Falls back to the matching `DEFAULT_AXIS_VIEW` for the current axis when
   * the tab doesn't carry an explicit override (legacy layouts and freshly
   * created views start there). The user's slider edits in Setup / Expand
   * patch `tab.xView` / `yView` directly and these computeds re-fire.
   */
  readonly xView = computed<AxisView>(() =>
    this.resolveView(
      this.tab.xView,
      this.histType() === '2d' ? this.xAxis() : this.energy1dAxis(),
      this.serverXBins(),
    ),
  );
  readonly yView = computed<AxisView>(() =>
    this.resolveView(this.tab.yView, this.yAxis(), this.serverYBins()),
  );

  /**
   * For 1D plots the axis is implied by the `histogramType` (energy / psd /
   * user_info0..3 → matching `AxisSource` for default lookup).
   */
  private energy1dAxis(): AxisSource {
    const t = this.histType();
    if (t === 'psd') return 'psd';
    if (t === 'user_info0') return 'user_info0';
    if (t === 'user_info1') return 'user_info1';
    if (t === 'user_info2') return 'user_info2';
    if (t === 'user_info3') return 'user_info3';
    return 'energy';
  }

  private resolveView(view: AxisView | undefined, axis: AxisSource, nativeBins: number): AxisView {
    const def = DEFAULT_AXIS_VIEW[axis];
    return {
      min: view?.min ?? def.min,
      max: view?.max ?? def.max,
      // Default = server's native bin count → rebin factor 1 (no rebin).
      bins: view?.bins ?? nativeBins,
    };
  }

  /** Minimum *displayed* bin count after rebinning — below this the chart
   *  loses too much shape (single peak collapses to one bar). The slider's
   *  upper bound is `serverBins / MIN_DISPLAY_BINS`. */
  readonly MIN_DISPLAY_BINS = 16;

  /** Server's native X bin count for the current view. Read from the latest
   *  histogram payload when available — the server is the source of truth.
   *  Falls back to the axis' DEFAULT_AXIS_VIEW.bins (used as a 2D-context
   *  default) until the first payload arrives. */
  readonly serverXBins = computed<number>(() => {
    if (this.is2dHistType(this.histType())) {
      const h = this.histograms2d().find((x) => x !== null);
      if (h) return h.x_config.num_bins;
    } else {
      const h = this.histograms().find((x) => x !== null);
      if (h) return h.config.num_bins;
    }
    const axis = this.is2dHistType(this.histType()) ? this.xAxis() : this.energy1dAxis();
    return DEFAULT_AXIS_VIEW[axis].bins;
  });

  readonly serverYBins = computed<number>(() => {
    if (!this.is2dHistType(this.histType())) return 1;
    const h = this.histograms2d().find((x) => x !== null);
    if (h) return h.y_config.num_bins;
    return DEFAULT_AXIS_VIEW[this.yAxis()].bins;
  });

  /** Slider upper bound — the largest rebin factor we let the user pick
   *  before the displayed bin count would drop below `MIN_DISPLAY_BINS`. */
  readonly maxXRebinFactor = computed<number>(() =>
    Math.max(1, Math.floor(this.serverXBins() / this.MIN_DISPLAY_BINS)),
  );
  readonly maxYRebinFactor = computed<number>(() =>
    Math.max(1, Math.floor(this.serverYBins() / this.MIN_DISPLAY_BINS)),
  );

  /** Current rebin factor derived from `xView.bins` (= displayed bins).
   *  factor 1 = native (no rebin), factor N = combine N native bins. */
  readonly xRebinFactor = computed<number>(() => {
    const sb = this.serverXBins();
    const db = Math.max(1, this.xView().bins ?? sb);
    return Math.max(1, Math.round(sb / db));
  });
  readonly yRebinFactor = computed<number>(() => {
    const sb = this.serverYBins();
    const db = Math.max(1, this.yView().bins ?? sb);
    return Math.max(1, Math.round(sb / db));
  });

  onXRebinSlider(raw: string | number): void {
    const factor = Math.max(1, Math.floor(Number(raw)));
    const bins = Math.max(this.MIN_DISPLAY_BINS, Math.floor(this.serverXBins() / factor));
    this.tabChange.emit({
      ...this.tab,
      xView: { ...(this.tab.xView ?? {}), ...this.xView(), bins },
    });
  }

  onYRebinSlider(raw: string | number): void {
    const factor = Math.max(1, Math.floor(Number(raw)));
    const bins = Math.max(this.MIN_DISPLAY_BINS, Math.floor(this.serverYBins() / factor));
    this.tabChange.emit({
      ...this.tab,
      yView: { ...(this.tab.yView ?? {}), ...this.yView(), bins },
    });
  }

  /** Heatmap-rendered histogram type (single 2D variant under the new model). */
  is2dHistType(t: string): boolean {
    return t === '2d';
  }

  /** Pretty label for an `AxisSource` used in chart titles + tooltips. */
  axisLabel(src: AxisSource): string {
    return AXIS_SOURCE_LABEL[src];
  }

  ngOnInit(): void {
    const cellCount = this.tab.cells.length;
    this.histograms.set(new Array(cellCount).fill(null));
    this.histograms2d.set(new Array(cellCount).fill(null));

    if (this.is2dHistType(this.histType())) {
      // Poll 2D histograms
      interval(this.refreshInterval)
        .pipe(
          takeUntil(this.destroy$),
          switchMap(() => this.fetchAll2d())
        )
        .subscribe((results) => {
          this.histograms2d.set(results);
        });
      this.fetchAll2d().subscribe((results) => {
        this.histograms2d.set(results);
      });
    } else {
      // Poll 1D histograms (energy or psd)
      interval(this.refreshInterval)
        .pipe(
          takeUntil(this.destroy$),
          switchMap(() => this.fetchAll1d())
        )
        .subscribe((results) => {
          this.histograms.set(results);
        });
      this.fetchAll1d().subscribe((results) => {
        this.histograms.set(results);
      });
    }
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  private fetchAll1d() {
    const t = this.histType();
    const requests = this.tab.cells.map((cell) => {
      if (cell.isEmpty) return of(null);
      if (t === 'psd') {
        return this.histogramService.fetchPsdHistogram(cell.sourceId, cell.channelId);
      }
      const slot = this.userInfoSlot(t);
      if (slot !== null) {
        return this.histogramService.fetchUserInfoHistogram(cell.sourceId, cell.channelId, slot);
      }
      return this.histogramService.fetchAndCacheHistogram(cell.sourceId, cell.channelId);
    });
    return forkJoin(requests);
  }

  /** Map a `'user_infoN'` HistogramType to its 0..3 slot, otherwise null. */
  private userInfoSlot(t: string): 0 | 1 | 2 | 3 | null {
    switch (t) {
      case 'user_info0': return 0;
      case 'user_info1': return 1;
      case 'user_info2': return 2;
      case 'user_info3': return 3;
      default: return null;
    }
  }

  private fetchAll2d() {
    const x = this.xAxis();
    const y = this.yAxis();
    const requests = this.tab.cells.map((cell) => {
      if (cell.isEmpty) return of(null);
      return this.histogramService.fetchHistogram2d(cell.sourceId, cell.channelId, x, y).pipe(
        catchError(() => of(null))
      );
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
    this.tabChange.emit({ ...this.tab, cells, lastModifiedCellIndex: index });
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
    // Use the last zoomed cell as reference, fallback to first locked cell
    const refIndex = this.tab.lastModifiedCellIndex;
    const refCell = refIndex != null
      ? this.tab.cells[refIndex]
      : this.tab.cells.find((cell) => cell.isLocked);
    if (!refCell?.isLocked) return;

    // Apply its range to all non-empty cells in this tab
    const cells = this.tab.cells.map((cell) => {
      if (cell.isEmpty) return cell;
      return {
        ...cell,
        xRange: refCell.xRange,
        yRange: refCell.yRange,
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

    this.tabChange.emit({ ...this.tab, cells, lastModifiedCellIndex: undefined });
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
