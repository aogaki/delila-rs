import { Component, Input, Output, EventEmitter, inject, signal } from '@angular/core';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatCardModule } from '@angular/material/card';
import { MatDividerModule } from '@angular/material/divider';
import { MatExpansionModule } from '@angular/material/expansion';
import { FormsModule } from '@angular/forms';
import { HistogramService } from '../../services/histogram.service';
import { DigitizerService } from '../../services/digitizer.service';
import {
  SetupConfig,
  SetupCell,
  ChannelSummary,
  XAxisLabel,
  HistogramType,
  AxisSource,
  AxisView,
  AXIS_SOURCE_LABEL,
  DEFAULT_AXIS_VIEW,
  axisSourceOptions,
  createDefaultSetupCell,
  defaultYAxisSource,
} from '../../models/histogram.types';

@Component({
  selector: 'app-setup-tab',
  standalone: true,
  imports: [
    MatFormFieldModule,
    MatInputModule,
    MatSelectModule,
    MatButtonModule,
    MatIconModule,
    MatCardModule,
    MatDividerModule,
    MatExpansionModule,
    FormsModule,
  ],
  template: `
    <div class="setup-container">
      <!-- Quick Create section -->
      <div class="quick-create">
        <span class="quick-create-label">Quick Create</span>
        <mat-form-field appearance="outline" class="digitizer-select">
          <mat-label>Digitizer</mat-label>
          <mat-select
            [value]="selectedDigitizerId()"
            (selectionChange)="selectedDigitizerId.set($event.value)"
          >
            @for (d of digitizerService.digitizers(); track d.digitizer_id) {
              <mat-option [value]="d.digitizer_id">
                #{{ d.digitizer_id }} {{ d.name }} ({{ d.num_channels }}ch)
              </mat-option>
            }
          </mat-select>
        </mat-form-field>
        <button
          mat-raised-button
          color="accent"
          (click)="onQuickCreate()"
          [disabled]="selectedDigitizerId() === null"
        >
          <mat-icon>auto_awesome</mat-icon>
          Quick Create
        </button>
      </div>

      <!-- Manual setup (collapsed by default — Quick Create handles 90% of cases) -->
      <mat-expansion-panel class="manual-setup-panel">
        <mat-expansion-panel-header>
          <mat-panel-title>Manual setup</mat-panel-title>
          <mat-panel-description>
            Custom grid + per-cell channel mapping
          </mat-panel-description>
        </mat-expansion-panel-header>

      <div class="setup-header">
        <mat-form-field appearance="outline" class="name-input">
          <mat-label>View Name</mat-label>
          <input
            matInput
            [value]="config.name"
            (input)="onNameChange($event)"
            placeholder="e.g., CRIB, LaBr3, Silicon"
          />
        </mat-form-field>

        <mat-form-field appearance="outline" class="size-input">
          <mat-label>Rows</mat-label>
          <input
            matInput
            type="number"
            [value]="config.gridRows"
            (change)="onRowsChange($event)"
            min="1"
            max="5"
          />
        </mat-form-field>

        <span class="size-separator">x</span>

        <mat-form-field appearance="outline" class="size-input">
          <mat-label>Cols</mat-label>
          <input
            matInput
            type="number"
            [value]="config.gridCols"
            (change)="onColsChange($event)"
            min="1"
            max="5"
          />
        </mat-form-field>

        <mat-form-field appearance="outline" class="histogram-type-select">
          <mat-label>Type</mat-label>
          <mat-select
            [value]="config.histogramType"
            (selectionChange)="onHistogramTypeChange($event.value)"
          >
            <mat-option value="energy">Energy</mat-option>
            @if (!digitizerService.allAMax()) {
              <mat-option value="psd">PSD</mat-option>
            }
            <mat-option value="user_info0">UserInfo[0]</mat-option>
            <mat-option value="user_info1">UserInfo[1]</mat-option>
            <mat-option value="user_info2">UserInfo[2]</mat-option>
            <mat-option value="user_info3">UserInfo[3]</mat-option>
            <mat-option value="2d">2D Heatmap</mat-option>
          </mat-select>
        </mat-form-field>

        @if (config.histogramType === '2d') {
          <mat-form-field appearance="outline" class="axis-source-select">
            <mat-label>X Axis</mat-label>
            <mat-select
              [value]="config.xAxis ?? 'energy'"
              (selectionChange)="onXAxisChange($event.value)"
            >
              @for (opt of axisOptions(); track opt) {
                <mat-option [value]="opt">{{ axisLabel(opt) }}</mat-option>
              }
            </mat-select>
          </mat-form-field>
          <mat-form-field appearance="outline" class="axis-source-select">
            <mat-label>Y Axis</mat-label>
            <mat-select
              [value]="yAxisOrDefault()"
              (selectionChange)="onYAxisChange($event.value)"
            >
              @for (opt of axisOptions(); track opt) {
                <mat-option [value]="opt">{{ axisLabel(opt) }}</mat-option>
              }
            </mat-select>
          </mat-form-field>
        } @else {
          <mat-form-field appearance="outline" class="axis-label-select">
            <mat-label>X-Axis</mat-label>
            <mat-select
              [value]="config.xAxisLabel"
              (selectionChange)="onXAxisLabelChange($event.value)"
            >
              <mat-option value="Channel">Channel</mat-option>
              <mat-option value="keV">keV</mat-option>
              <mat-option value="MeV">MeV</mat-option>
            </mat-select>
          </mat-form-field>
        }

        <button
          mat-raised-button
          color="primary"
          (click)="onCreateView()"
          [disabled]="!canCreateView()"
        >
          <mat-icon>add</mat-icon>
          Create View
        </button>
      </div>

      <!-- Binning controls (per-tab range + bin count). The chart components
           rebin client-side; the View tab gets a live slider on top of these
           initial values. -->
      <div class="binning-rows">
        <div class="binning-row">
          <span class="binning-section">{{ is2d() ? 'X (' + axisLabel(xAxisFor1dOr2d()) + '):' : 'Range:' }}</span>
          <mat-form-field appearance="outline" class="binning-input">
            <mat-label>Min</mat-label>
            <input matInput type="number" [value]="xMin()" (change)="onXMinChange($any($event.target).value)" />
          </mat-form-field>
          <mat-form-field appearance="outline" class="binning-input">
            <mat-label>Max</mat-label>
            <input matInput type="number" [value]="xMax()" (change)="onXMaxChange($any($event.target).value)" />
          </mat-form-field>
          <mat-form-field appearance="outline" class="binning-input">
            <mat-label>Bins</mat-label>
            <input matInput type="number" [value]="xBins()" (change)="onXBinsChange($any($event.target).value)" />
          </mat-form-field>
          @if (!is2d()) {
            <button mat-button (click)="onResetBinning()" title="Reset to axis defaults">
              <mat-icon>restart_alt</mat-icon> Defaults
            </button>
          }
        </div>

        @if (is2d()) {
          <div class="binning-row">
            <span class="binning-section">Y ({{ axisLabel(yAxisOrDefault()) }}):</span>
            <mat-form-field appearance="outline" class="binning-input">
              <mat-label>Min</mat-label>
              <input matInput type="number" [value]="yMin()" (change)="onYMinChange($any($event.target).value)" />
            </mat-form-field>
            <mat-form-field appearance="outline" class="binning-input">
              <mat-label>Max</mat-label>
              <input matInput type="number" [value]="yMax()" (change)="onYMaxChange($any($event.target).value)" />
            </mat-form-field>
            <mat-form-field appearance="outline" class="binning-input">
              <mat-label>Bins</mat-label>
              <input matInput type="number" [value]="yBins()" (change)="onYBinsChange($any($event.target).value)" />
            </mat-form-field>
            <button mat-button (click)="onResetBinning()" title="Reset to axis defaults">
              <mat-icon>restart_alt</mat-icon> Defaults
            </button>
          </div>
        }
      </div>

      <div
        class="setup-grid"
        [style.grid-template-rows]="'repeat(' + config.gridRows + ', 1fr)'"
        [style.grid-template-columns]="'repeat(' + config.gridCols + ', 1fr)'"
      >
        @for (cell of config.cells; track $index; let i = $index) {
          @if (i < config.gridRows * config.gridCols) {
            <mat-card class="setup-cell" [class.filled]="cell.sourceId !== null">
              <mat-form-field appearance="outline" class="channel-select">
                <mat-label>Channel</mat-label>
                <mat-select
                  [value]="cellKey(cell)"
                  (selectionChange)="onCellChange(i, $event.value)"
                >
                  <mat-option value="">-- Empty --</mat-option>
                  @for (channel of availableChannels(); track channelKey(channel)) {
                    <mat-option [value]="channelKey(channel)">
                      Src {{ channel.module_id }} / Ch {{ channel.channel_id }}
                    </mat-option>
                  }
                </mat-select>
              </mat-form-field>
            </mat-card>
          }
        }
      </div>

      <div class="setup-footer">
        <span class="hint">
          Select channels for each cell, then click "Create View" to generate a histogram view.
        </span>
      </div>
      </mat-expansion-panel>
    </div>
  `,
  styles: `
    .setup-container {
      display: flex;
      flex-direction: column;
      height: 100%;
      gap: 16px;
    }

    .quick-create {
      display: flex;
      align-items: center;
      gap: 12px;
      padding: 8px 12px;
      background: #fff8e1;
      border-radius: 4px;
      border-left: 3px solid #ffb300;
    }
    .quick-create-label {
      font-weight: 600;
      color: #5d4037;
    }

    .manual-setup-panel {
      box-shadow: none !important;
      border: 1px solid rgba(0, 0, 0, 0.12);
      border-radius: 4px;
    }

    .binning-rows {
      display: flex;
      flex-direction: column;
      gap: 4px;
    }

    .binning-row {
      display: flex;
      align-items: center;
      gap: 6px;
      flex-wrap: wrap;
      padding: 4px 8px;
      background: #f5f5f5;
      border-radius: 4px;
      font-size: 13px;
    }

    .binning-row .binning-section {
      font-weight: 500;
      color: #666;
      margin: 0 4px;
      white-space: nowrap;
    }

    .binning-row .binning-input {
      width: 110px;
    }

    /* Keep the form-field heights compact in this dense row */
    .binning-row .mat-mdc-form-field {
      font-size: 12px;
    }

    .quick-create-label {
      font-weight: 500;
      font-size: 14px;
      color: #666;
      white-space: nowrap;
    }

    .digitizer-select {
      width: 280px;
    }

    .setup-header {
      display: flex;
      align-items: center;
      gap: 12px;
      flex-wrap: wrap;
    }

    .name-input {
      width: 200px;
    }

    .size-input {
      width: 80px;
    }

    .histogram-type-select {
      width: 120px;
    }

    .axis-label-select {
      width: 120px;
    }

    .size-separator {
      font-size: 18px;
      color: #666;
    }

    .setup-grid {
      display: grid;
      gap: 12px;
      flex: 1;
      min-height: 0;
    }

    .setup-cell {
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 16px;
      background-color: #fafafa;
      border: 2px dashed #ddd;
    }

    .setup-cell.filled {
      background-color: #e3f2fd;
      border: 2px solid #2196f3;
    }

    .channel-select {
      width: 100%;
      max-width: 200px;
    }

    .setup-footer {
      padding: 8px 0;
    }

    .hint {
      font-size: 12px;
      color: #666;
    }

    ::ng-deep .setup-cell .mat-mdc-form-field-infix {
      padding: 8px 0 !important;
      min-height: 36px;
    }
  `,
})
export class SetupTabComponent {
  @Input() config!: SetupConfig;
  @Output() configChange = new EventEmitter<SetupConfig>();
  @Output() createView = new EventEmitter<SetupConfig>();
  @Output() quickCreate = new EventEmitter<SetupConfig[]>();

  private readonly histogramService = inject(HistogramService);
  readonly digitizerService = inject(DigitizerService);

  readonly availableChannels = this.histogramService.channelList;
  readonly selectedDigitizerId = signal<number | null>(null);

  channelKey(channel: ChannelSummary): string {
    return `${channel.module_id}:${channel.channel_id}`;
  }

  cellKey(cell: SetupCell): string {
    if (cell.sourceId === null || cell.channelId === null) {
      return '';
    }
    return `${cell.sourceId}:${cell.channelId}`;
  }

  onNameChange(event: Event): void {
    const input = event.target as HTMLInputElement;
    this.emitConfigChange({ name: input.value });
  }

  onRowsChange(event: Event): void {
    const input = event.target as HTMLInputElement;
    const rows = Math.min(5, Math.max(1, parseInt(input.value, 10) || 1));
    this.updateGridSize(rows, this.config.gridCols);
  }

  onColsChange(event: Event): void {
    const input = event.target as HTMLInputElement;
    const cols = Math.min(5, Math.max(1, parseInt(input.value, 10) || 1));
    this.updateGridSize(this.config.gridRows, cols);
  }

  /** AxisSource options shown in the X / Y dropdowns when type === '2d'.
   *  FW-aware: all-AMax systems drop psd/energy_short (issue #25). */
  axisOptions(): readonly AxisSource[] {
    return axisSourceOptions(this.digitizerService.allAMax());
  }

  /** FW-aware Y-axis default: UserInfo[0] on all-AMax systems, else PSD. */
  defaultYAxis(): AxisSource {
    return defaultYAxisSource(this.digitizerService.allAMax());
  }

  /** Pretty label for an `AxisSource` value (used in dropdown options). */
  axisLabel(src: AxisSource): string {
    return AXIS_SOURCE_LABEL[src];
  }

  onHistogramTypeChange(value: HistogramType): void {
    // When switching to 2D, populate sensible default axes if the user hasn't
    // chosen any yet — otherwise the heatmap would have no axis info to send.
    const patch: Partial<SetupConfig> = { histogramType: value };
    if (value === '2d') {
      if (!this.config.xAxis) patch.xAxis = 'energy';
      if (!this.config.yAxis) patch.yAxis = this.defaultYAxis();
    }
    this.emitConfigChange(patch);
  }

  onXAxisLabelChange(value: XAxisLabel): void {
    this.emitConfigChange({ xAxisLabel: value });
  }

  onXAxisChange(value: AxisSource): void {
    this.emitConfigChange({ xAxis: value });
  }

  onYAxisChange(value: AxisSource): void {
    this.emitConfigChange({ yAxis: value });
  }

  // ---------------------------------------------------------------------
  // Per-tab binning controls (range + bin count)
  //
  // Defaults come from `DEFAULT_AXIS_VIEW[axis]` so 1D Energy starts at
  // [0, 65536] / 512 bins, AMax UserInfo at [0, 16384] / 512, etc. The
  // chart components rebin client-side, so changes here apply on the very
  // next poll without any backend round trip.
  // ---------------------------------------------------------------------

  /** True when the active histogram type renders as a 2D heatmap (= the
   *  Y axis controls are shown). */
  is2d(): boolean {
    return this.config.histogramType === '2d';
  }

  /** Axis backing the X-axis defaults: the explicit `xAxis` for 2D, otherwise
   *  derived from the 1D `histogramType`. */
  xAxisFor1dOr2d(): AxisSource {
    if (this.is2d()) return this.config.xAxis ?? 'energy';
    const t = this.config.histogramType;
    if (t === 'psd') return 'psd';
    if (t === 'user_info0') return 'user_info0';
    if (t === 'user_info1') return 'user_info1';
    if (t === 'user_info2') return 'user_info2';
    if (t === 'user_info3') return 'user_info3';
    return 'energy';
  }

  yAxisOrDefault(): AxisSource {
    return this.config.yAxis ?? this.defaultYAxis();
  }

  /** Resolve a saved view (or undefined) against the axis defaults so the
   *  inputs always show *something* concrete. */
  private resolveView(view: AxisView | undefined, axis: AxisSource): AxisView {
    const def = DEFAULT_AXIS_VIEW[axis];
    return {
      min: view?.min ?? def.min,
      max: view?.max ?? def.max,
      bins: view?.bins ?? def.bins,
    };
  }

  xMin(): number { return this.resolveView(this.config.xView, this.xAxisFor1dOr2d()).min!; }
  xMax(): number { return this.resolveView(this.config.xView, this.xAxisFor1dOr2d()).max!; }
  xBins(): number { return this.resolveView(this.config.xView, this.xAxisFor1dOr2d()).bins!; }
  yMin(): number { return this.resolveView(this.config.yView, this.yAxisOrDefault()).min!; }
  yMax(): number { return this.resolveView(this.config.yView, this.yAxisOrDefault()).max!; }
  yBins(): number { return this.resolveView(this.config.yView, this.yAxisOrDefault()).bins!; }

  private patchXView(patch: Partial<AxisView>): void {
    const next = { ...this.resolveView(this.config.xView, this.xAxisFor1dOr2d()), ...patch };
    this.emitConfigChange({ xView: next });
  }
  private patchYView(patch: Partial<AxisView>): void {
    const next = { ...this.resolveView(this.config.yView, this.yAxisOrDefault()), ...patch };
    this.emitConfigChange({ yView: next });
  }

  onXMinChange(raw: string | number): void { this.patchXView({ min: Number(raw) }); }
  onXMaxChange(raw: string | number): void { this.patchXView({ max: Number(raw) }); }
  onXBinsChange(raw: string | number): void { this.patchXView({ bins: Math.max(1, Math.floor(Number(raw))) }); }
  onYMinChange(raw: string | number): void { this.patchYView({ min: Number(raw) }); }
  onYMaxChange(raw: string | number): void { this.patchYView({ max: Number(raw) }); }
  onYBinsChange(raw: string | number): void { this.patchYView({ bins: Math.max(1, Math.floor(Number(raw))) }); }

  /** Drop the explicit overrides so the inputs fall back to the per-axis
   *  defaults — useful after a long zoom session or when switching axes. */
  onResetBinning(): void {
    this.emitConfigChange({ xView: undefined, yView: undefined });
  }

  private updateGridSize(rows: number, cols: number): void {
    const newCellCount = rows * cols;
    const currentCells = [...this.config.cells];

    while (currentCells.length < newCellCount) {
      currentCells.push(createDefaultSetupCell());
    }

    this.configChange.emit({
      ...this.config,
      gridRows: rows,
      gridCols: cols,
      cells: currentCells.slice(0, newCellCount),
    });
  }

  onCellChange(index: number, value: string): void {
    const cells = [...this.config.cells];

    if (!value) {
      cells[index] = { sourceId: null, channelId: null };
    } else {
      const [sourceId, channelId] = value.split(':').map(Number);
      cells[index] = { sourceId, channelId };
    }

    this.emitConfigChange({ cells });
  }

  canCreateView(): boolean {
    // At least one cell must have a channel assigned
    return this.config.cells.some((cell) => cell.sourceId !== null);
  }

  onCreateView(): void {
    if (this.canCreateView()) {
      this.createView.emit(this.config);
    }
  }

  onQuickCreate(): void {
    const id = this.selectedDigitizerId();
    if (id === null) return;

    const digitizer = this.digitizerService.digitizers().find((d) => d.digitizer_id === id);
    if (!digitizer) return;

    const numCh = digitizer.num_channels;
    const configs: SetupConfig[] = [];
    const chunkSize = numCh <= 8 ? numCh : 16;

    for (let offset = 0; offset < numCh; offset += chunkSize) {
      const end = Math.min(offset + chunkSize, numCh);
      const count = end - offset;
      const rows = count <= 8 ? 2 : 4;
      const cols = 4;
      const totalCells = rows * cols;

      const cells: SetupCell[] = [];
      for (let i = 0; i < totalCells; i++) {
        const ch = offset + i;
        if (ch < end) {
          cells.push({ sourceId: id, channelId: ch });
        } else {
          cells.push({ sourceId: null, channelId: null });
        }
      }

      const name =
        numCh <= chunkSize
          ? `${digitizer.name}`
          : `${digitizer.name} Ch${offset}-${end - 1}`;

      configs.push({
        name,
        gridRows: rows,
        gridCols: cols,
        xAxisLabel: 'Channel',
        histogramType: this.config.histogramType,
        cells,
      });
    }

    this.quickCreate.emit(configs);
  }

  private emitConfigChange(changes: Partial<SetupConfig>): void {
    this.configChange.emit({ ...this.config, ...changes });
  }
}
