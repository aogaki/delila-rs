import { Component, OnInit, OnDestroy, inject, signal, computed } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MatCardModule } from '@angular/material/card';
import { MatSelectModule } from '@angular/material/select';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { NgxEchartsDirective } from 'ngx-echarts';
import type { EChartsCoreOption } from 'echarts/core';
import { Subject, interval, takeUntil, switchMap, forkJoin, of } from 'rxjs';
import { HistogramService } from '../../services/histogram.service';
import { WaveformChannelInfo, LatestWaveform } from '../../models/histogram.types';

interface ProbeConfig {
  analog1: boolean;
  analog2: boolean;
}

@Component({
  selector: 'app-waveform-page',
  standalone: true,
  imports: [
    CommonModule,
    FormsModule,
    MatCardModule,
    MatSelectModule,
    MatFormFieldModule,
    MatButtonModule,
    MatIconModule,
    MatCheckboxModule,
    NgxEchartsDirective,
  ],
  template: `
    <div class="waveform-page">
      <!-- Toolbar -->
      <div class="toolbar">
        <mat-form-field appearance="outline" class="channel-select">
          <mat-label>Select Channels</mat-label>
          <mat-select
            [value]="selectedChannels()"
            (selectionChange)="onChannelSelectionChange($event.value)"
            multiple
          >
            @for (ch of availableChannels(); track ch.module_id + ':' + ch.channel_id) {
              <mat-option [value]="ch.module_id + ':' + ch.channel_id">
                Src{{ ch.module_id }}/Ch{{ ch.channel_id }}
              </mat-option>
            }
          </mat-select>
        </mat-form-field>

        <div class="probe-toggles">
          <mat-checkbox
            [checked]="probeConfig().analog1"
            (change)="toggleProbe('analog1')"
            color="primary"
          >
            Analog 1
          </mat-checkbox>
          <mat-checkbox
            [checked]="probeConfig().analog2"
            (change)="toggleProbe('analog2')"
            color="accent"
          >
            Analog 2
          </mat-checkbox>
        </div>

        <button mat-stroked-button (click)="onRefresh()" [disabled]="isLoading()">
          <mat-icon>refresh</mat-icon>
          Refresh
        </button>

        <span class="spacer"></span>

        <span class="status-text">
          @if (waveforms().length > 0) {
            {{ waveforms().length }} waveform(s) loaded
          } @else {
            No waveforms available
          }
        </span>
      </div>

      <!-- Chart -->
      <div class="chart-container">
        @if (waveforms().length > 0) {
          <div
            echarts
            [options]="chartOptions()"
            [merge]="mergeOptions()"
            class="waveform-chart"
          ></div>
        } @else {
          <div class="no-data">
            <mat-icon>show_chart</mat-icon>
            <p>No waveform data available</p>
            <p class="hint">
              Make sure the DAQ is running with waveform enabled
              and select channels from the dropdown above.
            </p>
          </div>
        }
      </div>

      <!-- Info Panel -->
      @if (waveforms().length > 0) {
        <div class="info-panel">
          @for (wf of waveforms(); track wf.module_id + ':' + wf.channel_id) {
            <div class="waveform-info">
              <span class="channel-label">Src{{ wf.module_id }}/Ch{{ wf.channel_id }}</span>
              <span class="energy">Energy: {{ wf.energy }}</span>
              <span class="samples">Samples: {{ wf.waveform.analog_probe1.length || wf.waveform.analog_probe2.length }}</span>
            </div>
          }
        </div>
      }
    </div>
  `,
  styles: `
    :host {
      display: block;
      height: 100%;
    }

    .waveform-page {
      display: flex;
      flex-direction: column;
      height: 100%;
      padding: 16px;
      gap: 16px;
    }

    .toolbar {
      display: flex;
      align-items: center;
      gap: 16px;
      flex-wrap: wrap;
    }

    .channel-select {
      min-width: 250px;
    }

    .probe-toggles {
      display: flex;
      gap: 16px;
    }

    .spacer {
      flex: 1;
    }

    .status-text {
      color: #666;
      font-size: 14px;
    }

    .chart-container {
      flex: 1;
      min-height: 300px;
      background: white;
      border: 1px solid #e0e0e0;
      border-radius: 4px;
      overflow: hidden;
    }

    .waveform-chart {
      width: 100%;
      height: 100%;
    }

    .no-data {
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      height: 100%;
      color: #999;

      mat-icon {
        font-size: 64px;
        width: 64px;
        height: 64px;
        margin-bottom: 16px;
      }

      p {
        margin: 4px 0;
      }

      .hint {
        font-size: 12px;
        color: #bbb;
      }
    }

    .info-panel {
      display: flex;
      gap: 24px;
      flex-wrap: wrap;
      padding: 8px 0;
    }

    .waveform-info {
      display: flex;
      gap: 16px;
      align-items: center;
      font-size: 13px;
    }

    .channel-label {
      font-weight: 500;
    }

    .energy,
    .samples {
      color: #666;
    }
  `,
})
export class WaveformPageComponent implements OnInit, OnDestroy {
  private readonly histogramService = inject(HistogramService);
  private readonly destroy$ = new Subject<void>();
  private readonly refreshInterval = 500; // 500ms for waveform

  // State
  readonly availableChannels = signal<WaveformChannelInfo[]>([]);
  readonly selectedChannels = signal<string[]>([]);
  readonly waveforms = signal<LatestWaveform[]>([]);
  readonly isLoading = signal(false);
  readonly probeConfig = signal<ProbeConfig>({ analog1: true, analog2: true });

  // Chart options
  readonly chartOptions = computed<EChartsCoreOption>(() => this.buildChartOptions());
  readonly mergeOptions = signal<EChartsCoreOption>({});

  // Colors for different channels
  private readonly colors = [
    '#1976d2', // Blue
    '#d32f2f', // Red
    '#388e3c', // Green
    '#7b1fa2', // Purple
    '#f57c00', // Orange
    '#0097a7', // Cyan
    '#c2185b', // Pink
    '#5d4037', // Brown
  ];

  ngOnInit(): void {
    // Start polling for available channels
    this.fetchChannelList();

    interval(this.refreshInterval)
      .pipe(
        takeUntil(this.destroy$),
        switchMap(() => this.fetchWaveforms())
      )
      .subscribe();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  onChannelSelectionChange(selected: string[]): void {
    this.selectedChannels.set(selected);
    this.fetchWaveforms().subscribe();
  }

  toggleProbe(probe: 'analog1' | 'analog2'): void {
    const config = this.probeConfig();
    this.probeConfig.set({
      ...config,
      [probe]: !config[probe],
    });
    this.updateChart();
  }

  onRefresh(): void {
    this.fetchChannelList();
    this.fetchWaveforms().subscribe();
  }

  private fetchChannelList(): void {
    this.histogramService.fetchWaveformList().subscribe((response) => {
      if (response) {
        this.availableChannels.set(response.channels);

        // Auto-select first channel if none selected
        if (this.selectedChannels().length === 0 && response.channels.length > 0) {
          const first = response.channels[0];
          this.selectedChannels.set([`${first.module_id}:${first.channel_id}`]);
        }
      }
    });
  }

  private fetchWaveforms() {
    const selected = this.selectedChannels();
    if (selected.length === 0) {
      this.waveforms.set([]);
      return of(null);
    }

    this.isLoading.set(true);

    const requests = selected.map((key) => {
      const [moduleId, channelId] = key.split(':').map(Number);
      return this.histogramService.fetchWaveform(moduleId, channelId);
    });

    return forkJoin(requests).pipe(
      switchMap((results) => {
        const waveforms = results.filter((wf): wf is LatestWaveform => wf !== null);
        this.waveforms.set(waveforms);
        this.updateChart();
        this.isLoading.set(false);
        return of(null);
      })
    );
  }

  private updateChart(): void {
    const waveforms = this.waveforms();
    const config = this.probeConfig();

    if (waveforms.length === 0) {
      this.mergeOptions.set({});
      return;
    }

    const series: unknown[] = [];
    let colorIndex = 0;

    for (const wf of waveforms) {
      const baseColor = this.colors[colorIndex % this.colors.length];
      const label = `Src${wf.module_id}/Ch${wf.channel_id}`;

      // Analog Probe 1
      if (config.analog1 && wf.waveform.analog_probe1.length > 0) {
        series.push({
          name: `${label} - Analog1`,
          type: 'line',
          data: wf.waveform.analog_probe1.map((v, i) => [i, v]),
          symbol: 'none',
          lineStyle: {
            width: 1,
            color: baseColor,
          },
          itemStyle: {
            color: baseColor,
          },
        });
      }

      // Analog Probe 2
      if (config.analog2 && wf.waveform.analog_probe2.length > 0) {
        // Use a lighter/different shade for probe 2
        const probe2Color = this.lightenColor(baseColor, 0.3);
        series.push({
          name: `${label} - Analog2`,
          type: 'line',
          data: wf.waveform.analog_probe2.map((v, i) => [i, v]),
          symbol: 'none',
          lineStyle: {
            width: 1,
            color: probe2Color,
            type: 'dashed',
          },
          itemStyle: {
            color: probe2Color,
          },
        });
      }

      colorIndex++;
    }

    this.mergeOptions.set({
      series,
      legend: {
        data: series.map((s) => (s as { name: string }).name),
      },
    });
  }

  private lightenColor(hex: string, factor: number): string {
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);

    const newR = Math.min(255, Math.round(r + (255 - r) * factor));
    const newG = Math.min(255, Math.round(g + (255 - g) * factor));
    const newB = Math.min(255, Math.round(b + (255 - b) * factor));

    return `#${newR.toString(16).padStart(2, '0')}${newG.toString(16).padStart(2, '0')}${newB.toString(16).padStart(2, '0')}`;
  }

  private buildChartOptions(): EChartsCoreOption {
    return {
      animation: false,
      grid: {
        left: 60,
        right: 50,
        top: 40,
        bottom: 50,
      },
      legend: {
        show: true,
        top: 10,
        type: 'scroll',
      },
      tooltip: {
        trigger: 'axis',
        axisPointer: {
          type: 'cross',
        },
      },
      xAxis: {
        type: 'value',
        name: 'Sample',
        nameLocation: 'middle',
        nameGap: 25,
        min: 0,
      },
      yAxis: {
        type: 'value',
        name: 'ADC',
        min: -20000,
        max: 20000,
        axisLabel: {
          formatter: (value: number) => {
            if (Math.abs(value) >= 1000) {
              return (value / 1000).toFixed(1) + 'k';
            }
            return value.toString();
          },
        },
      },
      dataZoom: [
        // X-axis inside zoom (mouse wheel with shift)
        {
          type: 'inside',
          xAxisIndex: 0,
          yAxisIndex: [],
          zoomOnMouseWheel: 'shift',
          moveOnMouseMove: true,
        },
        // Y-axis inside zoom (mouse wheel with ctrl)
        {
          type: 'inside',
          xAxisIndex: [],
          yAxisIndex: 0,
          zoomOnMouseWheel: 'ctrl',
          moveOnMouseMove: false,
        },
        // X-axis slider (bottom)
        {
          type: 'slider',
          xAxisIndex: 0,
          height: 20,
          bottom: 5,
        },
        // Y-axis slider (right)
        {
          type: 'slider',
          yAxisIndex: 0,
          width: 20,
          right: 5,
        },
      ],
      series: [],
    };
  }
}
