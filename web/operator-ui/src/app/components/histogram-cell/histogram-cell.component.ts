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
} from '@angular/core';
import { DecimalPipe } from '@angular/common';
import { MatCardModule } from '@angular/material/card';
import { MatSelectModule } from '@angular/material/select';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatButtonModule } from '@angular/material/button';
import { FormsModule } from '@angular/forms';
import { interval, Subject, takeUntil, switchMap, filter } from 'rxjs';
import { HistogramChartComponent, RangeChangeEvent } from '../histogram-chart/histogram-chart.component';
import { HistogramService } from '../../services/histogram.service';
import { HistogramCell, ChannelSummary, Histogram1D } from '../../models/histogram.types';

@Component({
  selector: 'app-histogram-cell',
  standalone: true,
  imports: [
    DecimalPipe,
    MatCardModule,
    MatSelectModule,
    MatFormFieldModule,
    MatIconModule,
    MatButtonModule,
    FormsModule,
    HistogramChartComponent,
  ],
  template: `
    <mat-card class="histogram-cell" [class.empty]="!hasChannel()">
      <div class="cell-header">
        <mat-form-field appearance="outline" class="channel-select">
          <mat-select
            [value]="selectedChannelKey()"
            (selectionChange)="onChannelChange($event.value)"
            placeholder="Select channel"
          >
            <mat-option value="">-- Empty --</mat-option>
            @for (channel of availableChannels(); track channelKey(channel)) {
              <mat-option [value]="channelKey(channel)">
                Src {{ channel.module_id }} / Ch {{ channel.channel_id }}
              </mat-option>
            }
          </mat-select>
        </mat-form-field>
        <button mat-icon-button (click)="onExpandClick()" [disabled]="!hasChannel()">
          <mat-icon>open_in_full</mat-icon>
        </button>
      </div>

      @if (hasChannel()) {
        <div class="chart-container">
          <app-histogram-chart
            [histogram]="histogram()"
            [xRange]="cell.xRange"
            [yRange]="cell.yRange"
            [showDataZoom]="false"
            (rangeChange)="onRangeChange($event)"
          ></app-histogram-chart>
        </div>
        <div class="cell-footer">
          <span class="stats">
            Total: {{ histogram()?.total_counts ?? 0 | number }}
          </span>
          @if (cell.isLocked) {
            <mat-icon class="lock-icon" matTooltip="Range locked">lock</mat-icon>
          }
        </div>
      } @else {
        <div class="empty-message">
          <mat-icon>bar_chart</mat-icon>
          <span>Select a channel</span>
        </div>
      }
    </mat-card>
  `,
  styles: `
    :host {
      display: block;
      height: 100%;
    }

    .histogram-cell {
      height: 100%;
      display: flex;
      flex-direction: column;
      padding: 8px;
    }

    .histogram-cell.empty {
      background-color: #fafafa;
    }

    .cell-header {
      display: flex;
      align-items: center;
      gap: 4px;
      margin-bottom: 4px;
    }

    .channel-select {
      flex: 1;
    }

    .channel-select ::ng-deep .mat-mdc-form-field-infix {
      padding: 8px 0 !important;
      min-height: 36px;
    }

    .channel-select ::ng-deep .mat-mdc-select-value {
      font-size: 12px;
    }

    .chart-container {
      flex: 1;
      min-height: 150px;
    }

    .cell-footer {
      display: flex;
      align-items: center;
      justify-content: space-between;
      font-size: 11px;
      color: #666;
      margin-top: 4px;
    }

    .lock-icon {
      font-size: 14px;
      width: 14px;
      height: 14px;
    }

    .empty-message {
      flex: 1;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      color: #999;
      gap: 8px;
    }

    .empty-message mat-icon {
      font-size: 48px;
      width: 48px;
      height: 48px;
      opacity: 0.3;
    }
  `,
})
export class HistogramCellComponent implements OnInit, OnDestroy {
  @Input() cell!: HistogramCell;
  @Input() cellIndex = 0;

  @Output() cellChange = new EventEmitter<HistogramCell>();
  @Output() expand = new EventEmitter<number>();

  private readonly histogramService = inject(HistogramService);
  private readonly destroy$ = new Subject<void>();
  private readonly refreshInterval = 1000;

  readonly histogram = signal<Histogram1D | null>(null);

  readonly availableChannels = computed(() => this.histogramService.channelList());

  readonly hasChannel = computed(() => this.cell.sourceId !== null && this.cell.channelId !== null);

  readonly selectedChannelKey = computed(() => {
    if (this.cell.sourceId === null || this.cell.channelId === null) {
      return '';
    }
    return `${this.cell.sourceId}:${this.cell.channelId}`;
  });

  ngOnInit(): void {
    // Start auto-refresh for this cell
    interval(this.refreshInterval)
      .pipe(
        takeUntil(this.destroy$),
        filter(() => this.hasChannel()),
        switchMap(() =>
          this.histogramService.fetchAndCacheHistogram(this.cell.sourceId!, this.cell.channelId!)
        )
      )
      .subscribe((hist) => {
        if (hist) {
          this.histogram.set(hist);
        }
      });

    // Initial fetch
    if (this.hasChannel()) {
      this.histogramService
        .fetchAndCacheHistogram(this.cell.sourceId!, this.cell.channelId!)
        .subscribe((hist) => {
          if (hist) this.histogram.set(hist);
        });
    }
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  channelKey(channel: ChannelSummary): string {
    return `${channel.module_id}:${channel.channel_id}`;
  }

  onChannelChange(value: string): void {
    if (!value) {
      this.emitCellChange({ sourceId: null, channelId: null });
      this.histogram.set(null);
      return;
    }

    const [moduleId, channelId] = value.split(':').map(Number);
    this.emitCellChange({ sourceId: moduleId, channelId });

    // Fetch immediately
    this.histogramService.fetchAndCacheHistogram(moduleId, channelId).subscribe((hist) => {
      if (hist) this.histogram.set(hist);
    });
  }

  onRangeChange(event: RangeChangeEvent): void {
    this.emitCellChange({
      xRange: event.xRange,
      yRange: event.yRange,
      isLocked: true,
    });
  }

  onExpandClick(): void {
    this.expand.emit(this.cellIndex);
  }

  private emitCellChange(changes: Partial<HistogramCell>): void {
    this.cellChange.emit({ ...this.cell, ...changes });
  }
}
