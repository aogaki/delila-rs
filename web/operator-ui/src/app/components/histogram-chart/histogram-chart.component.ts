import {
  Component,
  Input,
  Output,
  EventEmitter,
  OnChanges,
  SimpleChanges,
  signal,
  computed,
} from '@angular/core';
import { NgxEchartsDirective } from 'ngx-echarts';
import type { EChartsCoreOption } from 'echarts/core';
import type { ECharts } from 'echarts/core';
import { Histogram1D, XAxisLabel, ViewCellFitResult } from '../../models/histogram.types';

export interface RangeChangeEvent {
  xRange: { min: number; max: number };
  yRange: { min: number; max: number } | 'auto';
}

@Component({
  selector: 'app-histogram-chart',
  standalone: true,
  imports: [NgxEchartsDirective],
  template: `
    <div
      echarts
      [options]="chartOptions()"
      [merge]="mergeOptions()"
      (chartInit)="onChartInit($event)"
      (chartDataZoom)="onDataZoom($event)"
      (chartBrushEnd)="onBrushEnd($event)"
      class="histogram-chart"
    ></div>
  `,
  styles: `
    :host {
      display: block;
      width: 100%;
      height: 100%;
    }

    .histogram-chart {
      width: 100%;
      height: 100%;
      min-height: 200px;
    }
  `,
})
export class HistogramChartComponent implements OnChanges {
  @Input() histogram: Histogram1D | null = null;
  @Input() xRange: { min: number; max: number } | 'auto' = 'auto';
  @Input() yRange: { min: number; max: number } | 'auto' = 'auto';
  @Input() showDataZoom = true;
  @Input() logScale = false;
  @Input() xAxisLabel: XAxisLabel = 'Channel';
  @Input() fitResult: ViewCellFitResult | null = null;

  @Output() rangeChange = new EventEmitter<RangeChangeEvent>();
  @Output() logScaleChange = new EventEmitter<boolean>();

  private chartInstance: ECharts | null = null;
  private data = signal<number[][]>([]);

  readonly chartOptions = computed<EChartsCoreOption>(() => this.buildChartOptions());
  readonly mergeOptions = signal<EChartsCoreOption>({});

  ngOnChanges(changes: SimpleChanges): void {
    if (changes['histogram'] && this.histogram) {
      this.updateData();
    }
    if (changes['xRange'] || changes['yRange'] || changes['logScale'] || changes['fitResult']) {
      this.updateMergeOptions();
    }
  }

  onChartInit(chart: ECharts): void {
    this.chartInstance = chart;

    // Enable brush (Select Range) mode after chart finishes rendering
    if (this.showDataZoom) {
      chart.on('finished', () => {
        chart.off('finished'); // Only run once
        chart.dispatchAction({
          type: 'takeGlobalCursor',
          key: 'brush',
          brushOption: {
            brushType: 'lineX',
          },
        });
      });
    }
  }

  /** Get chart as data URL (PNG) for image export */
  getDataURL(pixelRatio = 2): string | null {
    if (!this.chartInstance) return null;
    return this.chartInstance.getDataURL({
      type: 'png',
      pixelRatio,
      backgroundColor: '#fff',
    });
  }

  /** Get chart instance for external access */
  getChartInstance(): ECharts | null {
    return this.chartInstance;
  }

  onBrushEnd(event: unknown): void {
    if (!this.chartInstance) return;

    const brushEvent = event as {
      areas?: Array<{
        coordRange?: [number, number];
      }>;
    };

    if (brushEvent.areas && brushEvent.areas.length > 0) {
      const area = brushEvent.areas[0];
      if (area.coordRange) {
        const [xMin, xMax] = area.coordRange;
        // Only lock X range, keep Y range auto
        this.rangeChange.emit({
          xRange: { min: xMin, max: xMax },
          yRange: 'auto',
        });

        // Clear the brush selection after zooming
        this.chartInstance.dispatchAction({
          type: 'brush',
          areas: [],
        });
      }
    }
  }

  onDataZoom(event: unknown): void {
    if (!this.chartInstance) return;

    const option = this.chartInstance.getOption() as { dataZoom?: unknown[] };
    const dataZoom = option.dataZoom;
    if (dataZoom && dataZoom.length > 0) {
      const xZoom = dataZoom[0] as { startValue?: number; endValue?: number };

      if (xZoom.startValue !== undefined && xZoom.endValue !== undefined) {
        // Only lock X range, keep Y range auto
        this.rangeChange.emit({
          xRange: { min: xZoom.startValue, max: xZoom.endValue },
          yRange: 'auto',
        });
      }
    }
  }

  private updateData(): void {
    if (!this.histogram) return;

    const { bins, config } = this.histogram;
    const binWidth = (config.max_value - config.min_value) / config.num_bins;

    // Convert to [x, y] pairs for bar chart
    const chartData: number[][] = bins.map((count, i) => {
      const x = config.min_value + (i + 0.5) * binWidth;
      return [x, count];
    });

    this.data.set(chartData);
    this.updateMergeOptions();
  }

  private updateMergeOptions(): void {
    const data = this.data();
    if (data.length === 0) return;

    // Always use full histogram range for axis limits (allows zoom out)
    const config = this.histogram?.config;
    const fullXMin = config?.min_value ?? 0;
    const fullXMax = config?.max_value ?? 65535;

    // Determine visible X range
    const xMin = this.xRange === 'auto' ? fullXMin : this.xRange.min;
    const xMax = this.xRange === 'auto' ? fullXMax : this.xRange.max;

    // Y axis: calculate max within visible X range
    const maxInRange = this.getMaxCountInRange(xMin, xMax);
    const yMax = this.yRange === 'auto' ? maxInRange * 1.1 : this.yRange.max;

    // Y axis label formatter - different for log vs linear
    const yAxisFormatter = this.logScale
      ? (value: number) => {
          if (value === 0) return '0';
          if (value >= 1e9) return (value / 1e9).toFixed(0) + 'G';
          if (value >= 1e6) return (value / 1e6).toFixed(0) + 'M';
          if (value >= 1e3) return (value / 1e3).toFixed(0) + 'k';
          return value.toString();
        }
      : (value: number) => {
          if (value === 0) return '0';
          if (Math.abs(value) >= 1e9) return (value / 1e9).toFixed(1) + 'G';
          if (Math.abs(value) >= 1e6) return (value / 1e6).toFixed(1) + 'M';
          if (Math.abs(value) >= 1e3) return (value / 1e3).toFixed(0) + 'k';
          return Math.floor(value).toString();
        };

    // Generate fit curve data if fitResult exists
    const fitCurveData = this.generateFitCurve(xMin, xMax);

    const mergeOpts: EChartsCoreOption = {
      series: [
        {
          data: data,
        },
        // Fit curve series (empty if no fit)
        {
          type: 'line',
          data: fitCurveData,
          smooth: true,
          symbol: 'none',
          lineStyle: {
            color: '#e53935',
            width: 2,
          },
          z: 10,
        },
      ],
      // Always set axis to full range - dataZoom controls the visible range
      xAxis: {
        name: this.xAxisLabel,
        min: fullXMin,
        max: fullXMax,
      },
      yAxis: {
        type: this.logScale ? 'log' : 'value',
        name: 'Counts',
        min: this.logScale ? 1 : 0,
        max: this.yRange === 'auto' ? yMax : undefined,
        axisLabel: {
          formatter: yAxisFormatter,
        },
      },
    };

    // Set dataZoom to control the visible X range only
    if (this.showDataZoom) {
      mergeOpts['dataZoom'] = [
        {
          type: 'inside',
          xAxisIndex: 0,
          startValue: xMin,
          endValue: xMax,
          zoomOnMouseWheel: 'ctrl',
          moveOnMouseMove: false,
          moveOnMouseWheel: false,
          filterMode: 'none',
        },
      ];
    }

    // Add fit result text overlay
    if (this.fitResult) {
      const fit = this.fitResult;
      const chi2Ndf = fit.ndf > 0 ? (fit.chi2 / fit.ndf).toFixed(2) : '---';
      const fitText = [
        `Center: ${fit.center.toFixed(1)} ± ${fit.centerError.toFixed(1)}`,
        `Sigma: ${fit.sigma.toFixed(1)} ± ${fit.sigmaError.toFixed(1)}`,
        `FWHM: ${fit.fwhm.toFixed(1)}`,
        `Area: ${fit.netArea.toFixed(0)} ± ${fit.netAreaError.toFixed(0)}`,
        `χ²/ndf: ${chi2Ndf}`,
      ].join('\n');

      mergeOpts['graphic'] = [
        {
          type: 'text',
          right: 40,
          top: 20,
          style: {
            text: fitText,
            fontSize: 14,
            fontFamily: 'monospace',
            fill: '#333',
            backgroundColor: 'rgba(255, 255, 255, 0.9)',
            padding: [10, 14],
            borderColor: '#1976d2',
            borderWidth: 1,
            borderRadius: 4,
            lineHeight: 22,
          },
          z: 100,
        },
      ];
    } else {
      // Clear graphic when no fit result
      mergeOpts['graphic'] = [];
    }

    this.mergeOptions.set(mergeOpts);
  }

  /** Get max count within visible X range */
  private getMaxCountInRange(xMin: number, xMax: number): number {
    const data = this.data();
    if (data.length === 0) return 100;

    let maxCount = 0;
    for (const [x, count] of data) {
      if (x >= xMin && x <= xMax) {
        maxCount = Math.max(maxCount, count);
      }
    }
    return maxCount || 100;
  }

  /** Generate fit curve data points (Gaussian + linear background) */
  private generateFitCurve(xMin: number, xMax: number): number[][] {
    if (!this.fitResult) {
      return [];
    }

    const fit = this.fitResult;
    const { amplitude, center, sigma, bgLine } = fit;
    const { slope, intercept } = bgLine;

    // Generate points for smooth curve
    const numPoints = 200;
    const step = (xMax - xMin) / numPoints;
    const curveData: number[][] = [];

    for (let i = 0; i <= numPoints; i++) {
      const x = xMin + i * step;
      // Gaussian + linear background
      const gaussian = amplitude * Math.exp(-0.5 * Math.pow((x - center) / sigma, 2));
      const background = slope * x + intercept;
      const y = gaussian + background;
      curveData.push([x, y]);
    }

    return curveData;
  }

  private buildChartOptions(): EChartsCoreOption {
    return {
      animation: false,
      grid: {
        left: 50,
        right: 20,
        top: 10,
        bottom: 30,
      },
      tooltip: {
        trigger: 'axis',
        axisPointer: {
          type: 'shadow',
        },
        formatter: (params: unknown) => {
          const data = params as { data: number[] }[];
          if (data && data[0]) {
            const [x, y] = data[0].data;
            return `Channel: ${x.toFixed(0)}<br/>Counts: ${y}`;
          }
          return '';
        },
      },
      xAxis: {
        type: 'value',
        name: this.xAxisLabel,
        nameLocation: 'middle',
        nameGap: 25,
        min: 0,
        max: 65535, // Default, will be updated from histogram config
      },
      yAxis: {
        type: 'value',
        name: 'Counts',
        min: 0,
        axisLabel: {
          formatter: (value: number) => {
            if (value === 0) return '0';
            if (Math.abs(value) >= 1e9) return (value / 1e9).toFixed(1) + 'G';
            if (Math.abs(value) >= 1e6) return (value / 1e6).toFixed(1) + 'M';
            if (Math.abs(value) >= 1e3) return (value / 1e3).toFixed(0) + 'k';
            return Math.floor(value).toString();
          },
        },
      },
      series: [
        {
          type: 'bar',
          data: [],
          barWidth: '100%',
          itemStyle: {
            color: '#1976d2',
          },
          large: true,
          largeThreshold: 500,
        },
        {
          type: 'line',
          data: [],
          smooth: true,
          symbol: 'none',
          lineStyle: {
            color: '#e53935',
            width: 2,
          },
          z: 10,
        },
      ],
      toolbox: {
        show: false,
      },
      brush: this.showDataZoom
        ? {
            brushType: 'lineX',
            xAxisIndex: 0,
            brushStyle: {
              borderWidth: 1,
              color: 'rgba(25, 118, 210, 0.2)',
              borderColor: '#1976d2',
            },
            throttleType: 'debounce',
            throttleDelay: 300,
          }
        : undefined,
      dataZoom: this.showDataZoom
        ? [
            {
              type: 'inside',
              xAxisIndex: 0,
              zoomOnMouseWheel: 'ctrl', // Only zoom X with Ctrl+scroll
              moveOnMouseMove: false,
              moveOnMouseWheel: false,
              filterMode: 'none',
              zoomLock: false,
              throttle: 100,
            },
          ]
        : [],
    };
  }
}
