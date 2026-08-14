import { Component, signal, OnInit, OnDestroy, inject, computed } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { MAT_DIALOG_DATA, MatDialogRef, MatDialogModule } from '@angular/material/dialog';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { interval, Subject, takeUntil, switchMap } from 'rxjs';
import { HistogramChartComponent, RangeChangeEvent } from '../histogram-chart/histogram-chart.component';
import { HeatmapChartComponent } from '../heatmap-chart/heatmap-chart.component';
import { HistogramService } from '../../services/histogram.service';
import { FittingService } from '../../services/fitting.service';
import { DigitizerService } from '../../services/digitizer.service';
import { ViewCell, ViewCellFitResult, Histogram1D, Histogram2D, XAxisLabel, HistogramType, AxisSource, AxisView, AXIS_SOURCE_LABEL, defaultYAxisSource } from '../../models/histogram.types';

export interface ExpandDialogData {
  cell: ViewCell;
  cellIndex: number;
  xAxisLabel: XAxisLabel;
  histogramType: HistogramType;
  /** Required when `histogramType === '2d'`. Default: `'energy'`. */
  xAxis?: AxisSource;
  /** Required when `histogramType === '2d'`. Default: `'psd'`. */
  yAxis?: AxisSource;
  /** Client-side X view (range + bin count). Inherited from the source tab. */
  xView?: AxisView;
  /** Client-side Y view (2D only). */
  yView?: AxisView;
}

export interface ExpandDialogResult {
  cell: ViewCell;
}

@Component({
  selector: 'app-histogram-expand-dialog',
  standalone: true,
  imports: [
    DecimalPipe,
    MatDialogModule,
    MatButtonModule,
    MatIconModule,
    HistogramChartComponent,
    HeatmapChartComponent,
  ],
  template: `
    <div class="expand-dialog">
      <div class="dialog-header">
        <span class="title">
          Source {{ data.cell.sourceId }} / Channel {{ data.cell.channelId }}
        </span>
        <div class="header-actions">
          @if (data.histogramType !== '2d') {
            <button
              mat-stroked-button
              (click)="onFit()"
              [disabled]="!canFit()"
              title="Fit Gaussian to selected range"
            >
              <mat-icon>ssid_chart</mat-icon>
              Fit
            </button>
            <button
              mat-stroked-button
              (click)="onClearFit()"
              [disabled]="!hasFitResult()"
            >
              <mat-icon>clear</mat-icon>
              Clear Fit
            </button>
            <span class="separator"></span>
            <button
              mat-stroked-button
              (click)="onResetRange()"
              [disabled]="!isLocked()"
            >
              <mat-icon>restart_alt</mat-icon>
              Reset Range
            </button>
          }
          <button
            mat-stroked-button
            (click)="onToggleLogScale()"
            [class.active]="cell().logScale"
          >
            <mat-icon>{{ cell().logScale ? 'linear_scale' : 'show_chart' }}</mat-icon>
            {{ cell().logScale ? 'Linear' : 'Log' }}
          </button>
          <button mat-icon-button (click)="onClose()">
            <mat-icon>close</mat-icon>
          </button>
        </div>
      </div>

      <div class="main-content">
        <div class="chart-container">
          @if (data.histogramType !== '2d') {
            <app-histogram-chart
              [histogram]="histogram()"
              [xRange]="cell().xRange"
              [yRange]="cell().yRange"
              [showDataZoom]="true"
              [logScale]="cell().logScale ?? false"
              [xAxisLabel]="data.xAxisLabel"
              [fitResult]="cell().fitResult ?? null"
              [xView]="data.xView ?? null"
              (rangeChange)="onRangeChange($event)"
            ></app-histogram-chart>
          } @else {
            <app-heatmap-chart
              [histogram]="histogram2d()"
              [logScale]="cell().logScale ?? true"
              [xAxisLabel]="axisLabel(xAxis())"
              [yAxisLabel]="axisLabel(yAxis())"
              [xView]="data.xView ?? null"
              [yView]="data.yView ?? null"
            ></app-heatmap-chart>
          }
        </div>
      </div>

      <div class="dialog-footer">
        <div class="stats">
          @if (data.histogramType !== '2d') {
            @if (histogram(); as hist) {
              <span>Total: {{ hist.total_counts | number }}</span>
              <span>Underflow: {{ hist.underflow | number }}</span>
              <span>Overflow: {{ hist.overflow | number }}</span>
            }
          } @else {
            @if (histogram2d(); as hist) {
              <span>Total: {{ hist.total_counts | number }}</span>
              <span>Overflow: {{ hist.overflow | number }}</span>
            }
          }
        </div>
        <div class="hint">
          @if (data.histogramType === '2d') {
            {{ axisLabel(xAxis()) }} vs {{ axisLabel(yAxis()) }} 2D heatmap
          } @else if (!isLocked()) {
            Drag to select fit range, Ctrl+Scroll for X-axis zoom
          } @else {
            Range selected. Click "Fit" to perform Gaussian fit.
          }
        </div>
      </div>
    </div>
  `,
  styles: `
    .expand-dialog {
      display: flex;
      flex-direction: column;
      width: 90vw;
      height: 80vh;
      min-width: 800px;
      max-width: 1600px;
      max-height: 900px;
    }

    .dialog-header {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 8px 16px;
      border-bottom: 1px solid #e0e0e0;
      flex-shrink: 0;
    }

    .title {
      font-size: 16px;
      font-weight: 500;
    }

    .header-actions {
      display: flex;
      align-items: center;
      gap: 8px;
    }

    .separator {
      width: 1px;
      height: 24px;
      background-color: #e0e0e0;
      margin: 0 4px;
    }

    .header-actions button.active {
      background-color: #1976d2;
      color: white;
    }

    .main-content {
      flex: 1;
      display: flex;
      min-height: 0;
    }

    .chart-container {
      flex: 1;
      min-height: 0;
      padding: 16px;
    }

    .dialog-footer {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 8px 16px;
      border-top: 1px solid #e0e0e0;
      flex-shrink: 0;
    }

    .stats {
      display: flex;
      gap: 16px;
      font-size: 13px;
      color: #666;
    }

    .hint {
      font-size: 12px;
      color: #999;
    }
  `,
})
export class HistogramExpandDialogComponent implements OnInit, OnDestroy {
  private readonly histogramService = inject(HistogramService);
  private readonly fittingService = inject(FittingService);
  private readonly digitizerService = inject(DigitizerService);
  private readonly destroy$ = new Subject<void>();
  private readonly refreshInterval = 1000;

  readonly cell: ReturnType<typeof signal<ViewCell>>;
  readonly histogram = signal<Histogram1D | null>(null);
  readonly histogram2d = signal<Histogram2D | null>(null);

  readonly fitResult = computed(() => this.cell().fitResult);
  readonly chi2PerNdf = computed(() => {
    const fit = this.fitResult();
    if (!fit || fit.ndf === 0) return 0;
    return fit.chi2 / fit.ndf;
  });

  readonly data = inject<ExpandDialogData>(MAT_DIALOG_DATA);
  private readonly dialogRef = inject(MatDialogRef<HistogramExpandDialogComponent, ExpandDialogResult>);

  constructor() {
    this.cell = signal<ViewCell>(this.data.cell);
  }

  ngOnInit(): void {
    const { sourceId, channelId } = this.data.cell;
    const type = this.data.histogramType;

    if (type === '2d') {
      // Poll 2D histogram with the (X, Y) axes the dialog was opened with.
      const x = this.xAxis();
      const y = this.yAxis();
      interval(this.refreshInterval)
        .pipe(
          takeUntil(this.destroy$),
          switchMap(() => this.histogramService.fetchHistogram2d(sourceId, channelId, x, y))
        )
        .subscribe((hist) => {
          if (hist) this.histogram2d.set(hist);
        });
      this.histogramService.fetchHistogram2d(sourceId, channelId, x, y)
        .subscribe((hist) => {
          if (hist) this.histogram2d.set(hist);
        });
    } else {
      // Poll 1D histogram (energy / psd / user_info[0..3])
      const slot = this.userInfoSlot(type);
      const fetch$ = type === 'psd'
        ? () => this.histogramService.fetchPsdHistogram(sourceId, channelId)
        : slot !== null
          ? () => this.histogramService.fetchUserInfoHistogram(sourceId, channelId, slot)
          : () => this.histogramService.fetchAndCacheHistogram(sourceId, channelId);

      interval(this.refreshInterval)
        .pipe(
          takeUntil(this.destroy$),
          switchMap(() => fetch$())
        )
        .subscribe((hist) => {
          if (hist) this.histogram.set(hist);
        });
      fetch$().subscribe((hist) => {
        if (hist) this.histogram.set(hist);
      });
    }
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  isLocked(): boolean {
    return this.cell().isLocked;
  }

  /** 2D X axis source (defaults to `'energy'` for legacy data). */
  xAxis(): AxisSource {
    return this.data.xAxis ?? 'energy';
  }

  /** 2D Y axis source (legacy data falls back to the FW default: UserInfo[0]
   *  on all-AMax systems, psd otherwise — issue #25). */
  yAxis(): AxisSource {
    return this.data.yAxis ?? defaultYAxisSource(this.digitizerService.allAMax());
  }

  /** Pretty label for an `AxisSource` used in chart titles + tooltips. */
  axisLabel(src: AxisSource): string {
    return AXIS_SOURCE_LABEL[src];
  }

  /** Map a `'user_infoN'` HistogramType to its 0..3 slot, otherwise null. */
  private userInfoSlot(t: HistogramType): 0 | 1 | 2 | 3 | null {
    switch (t) {
      case 'user_info0': return 0;
      case 'user_info1': return 1;
      case 'user_info2': return 2;
      case 'user_info3': return 3;
      default: return null;
    }
  }

  canFit(): boolean {
    const c = this.cell();
    const hist = this.histogram();
    return c.isLocked && c.xRange !== 'auto' && hist !== null;
  }

  hasFitResult(): boolean {
    return this.cell().fitResult !== undefined;
  }

  onRangeChange(event: RangeChangeEvent): void {
    this.cell.update((c) => ({
      ...c,
      xRange: event.xRange,
      yRange: event.yRange,
      isLocked: true,
    }));
  }

  onResetRange(): void {
    this.cell.update((c) => ({
      ...c,
      xRange: 'auto',
      yRange: 'auto',
      isLocked: false,
    }));
  }

  onToggleLogScale(): void {
    this.cell.update((c) => ({
      ...c,
      logScale: !c.logScale,
    }));
  }

  onFit(): void {
    const hist = this.histogram();
    const c = this.cell();

    if (!hist || c.xRange === 'auto') return;

    const result = this.fittingService.fitGaussian({
      bins: hist.bins,
      binWidth: (hist.config.max_value - hist.config.min_value) / hist.config.num_bins,
      minValue: hist.config.min_value,
      fitRangeMin: c.xRange.min,
      fitRangeMax: c.xRange.max,
    });

    if (result) {
      const fitResult: ViewCellFitResult = {
        center: result.center,
        centerError: result.centerError,
        sigma: result.sigma,
        sigmaError: result.sigmaError,
        fwhm: result.fwhm,
        netArea: result.netArea,
        netAreaError: result.netAreaError,
        chi2: result.chi2,
        ndf: result.ndf,
        leftLine: result.leftLine,
        rightLine: result.rightLine,
        bgLine: result.bgLine,
        amplitude: result.amplitude,
      };

      const xRange = c.xRange as { min: number; max: number };
      this.cell.update((cell) => ({
        ...cell,
        fitResult,
        fitRange: { min: xRange.min, max: xRange.max },
      }));
    } else {
      // Fit failed - could show a notification here
      console.warn('Fit failed');
    }
  }

  onClearFit(): void {
    this.cell.update((c) => ({
      ...c,
      fitResult: undefined,
      fitRange: undefined,
    }));
  }

  onClose(): void {
    this.dialogRef.close({ cell: this.cell() });
  }
}
