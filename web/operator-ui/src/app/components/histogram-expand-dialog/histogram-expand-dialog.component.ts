import { Component, signal, OnInit, OnDestroy, inject, computed } from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { MAT_DIALOG_DATA, MatDialogRef, MatDialogModule } from '@angular/material/dialog';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { interval, Subject, takeUntil, switchMap } from 'rxjs';
import { HistogramChartComponent, RangeChangeEvent } from '../histogram-chart/histogram-chart.component';
import { HistogramService } from '../../services/histogram.service';
import { FittingService } from '../../services/fitting.service';
import { ViewCell, ViewCellFitResult, Histogram1D, XAxisLabel } from '../../models/histogram.types';

export interface ExpandDialogData {
  cell: ViewCell;
  cellIndex: number;
  xAxisLabel: XAxisLabel;
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
  ],
  template: `
    <div class="expand-dialog">
      <div class="dialog-header">
        <span class="title">
          Source {{ data.cell.sourceId }} / Channel {{ data.cell.channelId }}
        </span>
        <div class="header-actions">
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
          <app-histogram-chart
            [histogram]="histogram()"
            [xRange]="cell().xRange"
            [yRange]="cell().yRange"
            [showDataZoom]="true"
            [logScale]="cell().logScale ?? false"
            [xAxisLabel]="data.xAxisLabel"
            [fitResult]="cell().fitResult ?? null"
            (rangeChange)="onRangeChange($event)"
          ></app-histogram-chart>
        </div>
      </div>

      <div class="dialog-footer">
        <div class="stats">
          @if (histogram(); as hist) {
            <span>Total: {{ hist.total_counts | number }}</span>
            <span>Underflow: {{ hist.underflow | number }}</span>
            <span>Overflow: {{ hist.overflow | number }}</span>
          }
        </div>
        <div class="hint">
          @if (!isLocked()) {
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
  private readonly destroy$ = new Subject<void>();
  private readonly refreshInterval = 1000;

  readonly cell: ReturnType<typeof signal<ViewCell>>;
  readonly histogram = signal<Histogram1D | null>(null);

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
    // Start auto-refresh
    interval(this.refreshInterval)
      .pipe(
        takeUntil(this.destroy$),
        switchMap(() =>
          this.histogramService.fetchAndCacheHistogram(
            this.data.cell.sourceId,
            this.data.cell.channelId
          )
        )
      )
      .subscribe((hist) => {
        if (hist) {
          this.histogram.set(hist);
        }
      });

    // Initial fetch
    this.histogramService
      .fetchAndCacheHistogram(this.data.cell.sourceId, this.data.cell.channelId)
      .subscribe((hist) => {
        if (hist) this.histogram.set(hist);
      });
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  isLocked(): boolean {
    return this.cell().isLocked;
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
