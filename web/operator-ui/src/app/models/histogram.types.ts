function generateUUID(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  // Fallback for non-secure contexts (HTTP)
  return '10000000-1000-4000-8000-100000000000'.replace(/[018]/g, (c) =>
    (
      +c ^
      (crypto.getRandomValues(new Uint8Array(1))[0] & (15 >> (+c / 4)))
    ).toString(16),
  );
}

// Histogram configuration
export interface HistogramConfig {
  num_bins: number;
  min_value: number;
  max_value: number;
}

// Single histogram data
export interface Histogram1D {
  module_id: number;
  channel_id: number;
  config: HistogramConfig;
  bins: number[];
  total_counts: number;
  overflow: number;
  underflow: number;
}

// 2D Histogram data (Energy vs PSD)
export interface Histogram2D {
  module_id: number;
  channel_id: number;
  x_config: HistogramConfig;
  y_config: HistogramConfig;
  bins: number[]; // flat array: bins[y * x_bins + x]
  total_counts: number;
  overflow: number;
}

// Channel summary for list response
export interface ChannelSummary {
  module_id: number;
  channel_id: number;
  total_counts: number;
  name?: string;
}

// Response from GET /api/histograms
export interface HistogramListResponse {
  total_events: number;
  elapsed_secs: number;
  event_rate: number;
  channels: ChannelSummary[];
}

// Response from GET /api/status
export interface MonitorStatusResponse {
  state: string;
  total_events: number;
  num_channels: number;
  elapsed_secs: number;
  event_rate: number;
}

// Channel identifier
export interface ChannelKey {
  moduleId: number;
  channelId: number;
}

// Helper to create channel key string for Maps
export function channelKeyString(moduleId: number, channelId: number): string {
  return `${moduleId}:${channelId}`;
}

// Helper to parse channel key string
export function parseChannelKey(key: string): ChannelKey {
  const [moduleId, channelId] = key.split(':').map(Number);
  return { moduleId, channelId };
}

// Monitor state for localStorage persistence
export interface MonitorState {
  setupConfig: SetupConfig;
  viewTabs: ViewTab[];
  activeTabId: string | null; // null = Setup tab is active
}

// X-axis label type
export type XAxisLabel = 'Channel' | 'keV' | 'MeV';

/**
 * Source of a 2D plot axis. Mirrors the backend `AxisSource` enum 1:1
 * (snake_case wire format). Used in REST query params and in `SetupConfig` /
 * `ViewTab` to record which X/Y axes the user picked for a 2D plot.
 */
export type AxisSource =
  | 'energy'
  | 'energy_short'
  | 'user_info0'
  | 'user_info1'
  | 'user_info2'
  | 'user_info3'
  | 'psd';

/**
 * Human-readable label for an axis (used as chart titles and in dropdowns).
 */
export const AXIS_SOURCE_LABEL: Record<AxisSource, string> = {
  energy: 'Energy',
  energy_short: 'Energy Short',
  user_info0: 'UserInfo[0]',
  user_info1: 'UserInfo[1]',
  user_info2: 'UserInfo[2]',
  user_info3: 'UserInfo[3]',
  psd: 'PSD',
};

/** Order in which axes appear in dropdowns. */
export const AXIS_SOURCE_OPTIONS: readonly AxisSource[] = [
  'energy',
  'energy_short',
  'user_info0',
  'user_info1',
  'user_info2',
  'user_info3',
  'psd',
];

// FW-aware axis policy (issue #25). The AMax custom FW carries its per-event
// extras in user_info[0..3] and has no energy_short, so psd (= (E-Es)/E)
// degenerates to a constant. On an all-AMax system the 2D default the
// operator wants is E vs UserInfo[0], and psd/energy_short only clutter the
// menus. `allAMax` comes from DigitizerService.allAMax() (mixed or non-AMax
// systems keep the historical psd behavior untouched).

/** FW-aware default for a 2D plot's Y axis. */
export function defaultYAxisSource(allAMax: boolean): AxisSource {
  return allAMax ? 'user_info0' : 'psd';
}

/** FW-aware dropdown option list for 2D plot axes. */
export function axisSourceOptions(allAMax: boolean): readonly AxisSource[] {
  return allAMax
    ? AXIS_SOURCE_OPTIONS.filter((a) => a !== 'psd' && a !== 'energy_short')
    : AXIS_SOURCE_OPTIONS;
}

/**
 * Client-side fallback defaults per axis — used to seed Setup tab inputs
 * and as a fallback when a `ViewTab.xView` / `yView` field is missing **and**
 * no histogram payload has arrived yet. Once a payload arrives, the actual
 * server bin count from `Histogram1D.config` / `Histogram2D.x_config` is
 * authoritative.
 *
 * `bins` here mirrors the Rust `AxisSource::default_axis()` 2D fallback
 * (1024 native bins for integer axes, 200 for PSD). 1D histograms have
 * higher native resolution on the server (Energy = 65536, UserInfo = 16384,
 * PSD = 200) — `view-tab` reads those directly from the payload.
 */
export const DEFAULT_AXIS_VIEW: Record<AxisSource, { min: number; max: number; bins: number }> = {
  energy: { min: 0, max: 65536, bins: 1024 },
  energy_short: { min: 0, max: 65536, bins: 1024 },
  user_info0: { min: 0, max: 16384, bins: 1024 },
  user_info1: { min: 0, max: 16384, bins: 1024 },
  user_info2: { min: 0, max: 16384, bins: 1024 },
  user_info3: { min: 0, max: 16384, bins: 1024 },
  psd: { min: -0.2, max: 1.2, bins: 200 },
};

/**
 * Histogram type for tab-level selection.
 *
 * - `'energy'` / `'psd'`: 1D plot of the named axis.
 * - `'user_info0'..'user_info3'`: 1D plot of the AMax-style 63-bit user-info
 *   slot. Available on every channel; non-AMax FW just leaves the slot at 0.
 * - `'2d'`: 2D heatmap whose axes are taken from `SetupConfig.xAxis` /
 *   `SetupConfig.yAxis` (or the same fields on `ViewTab`).
 *
 * The legacy `'psd2d'` and `'amax2d'` literals are migrated to `'2d'` with
 * `(xAxis, yAxis) = ('energy', 'psd')` and `('energy', 'user_info0')`
 * respectively — see `migrateHistogramType`.
 */
export type HistogramType =
  | 'energy'
  | 'psd'
  | 'user_info0'
  | 'user_info1'
  | 'user_info2'
  | 'user_info3'
  | '2d';

/** Returns true if the type is a 2D heatmap (needs `xAxis` / `yAxis`). */
export function is2dHistogramType(t: HistogramType | undefined): boolean {
  return t === '2d';
}

/**
 * Migrate legacy localStorage / monitor_layout.json values
 * (`'psd2d'`, `'amax2d'`) to the unified `'2d'` representation. Returns
 * `null` if the value is not a known legacy alias (caller falls back to
 * pre-existing fields).
 */
export function migrateLegacyHistType(legacy: string): {
  histogramType: HistogramType;
  xAxis: AxisSource;
  yAxis: AxisSource;
} | null {
  if (legacy === 'psd2d') {
    return { histogramType: '2d', xAxis: 'energy', yAxis: 'psd' };
  }
  if (legacy === 'amax2d') {
    return { histogramType: '2d', xAxis: 'energy', yAxis: 'user_info0' };
  }
  return null;
}

/**
 * Per-axis viewing range and binning, applied client-side.
 *
 * The backend serves at its native resolution (1D Energy = 65536, 1D
 * UserInfo = 16384, 1D PSD = 200; 2D = 1024×1024 by default, configurable
 * via `histogram2d_overrides`). These fields tell the chart how to
 * *display* that data: zoom into `[min, max]` and merge adjacent native
 * bins until `bins` slots remain.
 *
 * `bins` is the *displayed* bin count, derived from the user-chosen rebin
 * factor: factor 1 → bins = native, factor N → bins = native / N. The
 * view-tab slider drives this. Going below 1 (= more bins than native) is
 * silently clamped by `rebin2d` / `rebin1d`.
 *
 * Optional everywhere; missing means "use the server's native config" (=
 * rebin factor 1).
 */
export interface AxisView {
  /** Lower edge of the visible range (in axis units, e.g. ADC). */
  min?: number;
  /** Upper edge of the visible range. */
  max?: number;
  /** Number of bins to show after client-side rebinning. */
  bins?: number;
}

// Setup tab configuration (template for creating views)
export interface SetupConfig {
  name: string;
  gridRows: number;
  gridCols: number;
  xAxisLabel: XAxisLabel;
  histogramType: HistogramType;
  /** Required when `histogramType === '2d'`. Default: `'energy'`. */
  xAxis?: AxisSource;
  /** Required when `histogramType === '2d'`. Default: `'psd'`. */
  yAxis?: AxisSource;
  /** Client-side X axis view: zoom range + bin count (1D and 2D). */
  xView?: AxisView;
  /** Client-side Y axis view: zoom range + bin count (2D only). */
  yView?: AxisView;
  cells: SetupCell[];
}

// Setup cell - only channel assignment, no runtime state
export interface SetupCell {
  sourceId: number | null;
  channelId: number | null;
}

// View tab - created from setup, read-only layout
export interface ViewTab {
  id: string;
  name: string;
  gridRows: number;
  gridCols: number;
  xAxisLabel: XAxisLabel;
  histogramType?: HistogramType; // optional for backward compat (default: 'energy')
  /** 2D X axis (only meaningful when `histogramType === '2d'`). */
  xAxis?: AxisSource;
  /** 2D Y axis (only meaningful when `histogramType === '2d'`). */
  yAxis?: AxisSource;
  /** Client-side X axis view: zoom range + bin count. */
  xView?: AxisView;
  /** Client-side Y axis view (2D only). */
  yView?: AxisView;
  cells: ViewCell[];
  lastModifiedCellIndex?: number;
}

// View cell - has runtime state for display
export interface ViewCell {
  sourceId: number;
  channelId: number;
  xRange: { min: number; max: number } | 'auto';
  yRange: { min: number; max: number } | 'auto';
  isLocked: boolean;
  isEmpty: boolean; // true for placeholder cells in grid
  logScale?: boolean; // Y-axis log scale
  fitResult?: ViewCellFitResult; // Gaussian fit result
  fitRange?: { min: number; max: number }; // Range used for fitting
}

// Simplified fit result for ViewCell (serializable to localStorage)
export interface ViewCellFitResult {
  center: number;
  centerError: number;
  sigma: number;
  sigmaError: number;
  fwhm: number;
  netArea: number;
  netAreaError: number;
  chi2: number;
  ndf: number;
  // Background lines for drawing
  leftLine: { slope: number; intercept: number };
  rightLine: { slope: number; intercept: number };
  bgLine: { slope: number; intercept: number };
  // Gaussian parameters for drawing
  amplitude: number;
}

// Legacy type for backward compatibility during migration
export interface HistogramCell {
  sourceId: number | null;
  channelId: number | null;
  xRange: { min: number; max: number } | 'auto';
  yRange: { min: number; max: number } | 'auto';
  isLocked: boolean;
}

// Legacy type
export interface MonitorTab {
  id: string;
  name: string;
  gridRows: number;
  gridCols: number;
  cells: HistogramCell[];
}

// Gaussian fit result (for Phase 6)
export interface GaussianFitResult {
  amplitude: number;
  center: number;
  sigma: number;
  leftLine: { slope: number; intercept: number };
  rightLine: { slope: number; intercept: number };
  bgLine: { slope: number; intercept: number };
  fwhm: number;
  area: number;
  chi2: number;
}

// Create default setup cell
export function createDefaultSetupCell(): SetupCell {
  return {
    sourceId: null,
    channelId: null,
  };
}

// Create default setup config
export function createDefaultSetupConfig(): SetupConfig {
  return {
    name: '',
    gridRows: 2,
    gridCols: 2,
    xAxisLabel: 'Channel',
    histogramType: 'energy',
    xAxis: 'energy',
    yAxis: 'psd',
    cells: Array(4)
      .fill(null)
      .map(() => createDefaultSetupCell()),
  };
}

// Create view tab from setup config
export function createViewTabFromSetup(setup: SetupConfig): ViewTab | null {
  // Filter out empty cells
  const validCells = setup.cells.filter(
    (cell): cell is { sourceId: number; channelId: number } =>
      cell.sourceId !== null && cell.channelId !== null
  );

  if (validCells.length === 0) {
    return null; // No valid cells to display
  }

  const rows = setup.gridRows;
  const cols = setup.gridCols;

  return {
    id: generateUUID(),
    name: setup.name || `View ${Date.now()}`,
    gridRows: rows,
    gridCols: cols,
    xAxisLabel: setup.xAxisLabel,
    histogramType: setup.histogramType,
    xAxis: setup.xAxis,
    yAxis: setup.yAxis,
    xView: setup.xView,
    yView: setup.yView,
    cells: setup.cells.map((cell) => ({
      sourceId: cell.sourceId ?? 0,
      channelId: cell.channelId ?? 0,
      xRange: 'auto' as const,
      yRange: 'auto' as const,
      isLocked: false,
      isEmpty: cell.sourceId === null,
    })),
  };
}

// Create default monitor state
export function createDefaultMonitorState(): MonitorState {
  return {
    setupConfig: createDefaultSetupConfig(),
    viewTabs: [],
    activeTabId: null,
  };
}

// =============================================================================
// Waveform Types
// =============================================================================

// Waveform data from backend
export interface Waveform {
  analog_probe1: number[];
  analog_probe2: number[];
  /** AMax debug FW: triangle filter wave (signed 16-bit). Empty `[]` on
   *  legacy AMax / PSD2 / PHA2 / PHA1 / PSD1 paths. Optional for BC with
   *  old `.delila` payloads. */
  analog_probe3?: number[];
  digital_probe1: number[]; // Packed bits
  digital_probe2: number[];
  digital_probe3: number[];
  digital_probe4: number[];
  /** AMax debug FW: shaping_track (1-bit per sample). Empty `[]` on
   *  legacy paths. Optional for BC with old `.delila` payloads. */
  digital_probe5?: number[];
  /** Digital probes 6..16 — reserved slots for future AMax debug FW
   *  digital lane bit assignments (currently bits 10..0 are constant 0
   *  in hardware, so all empty today). Data-driven UI (`activeDigitalProbes()`)
   *  filters them by `digital_probe_type[i] !== UNKNOWN_PROBE_TYPE`, so
   *  empty slots never render. */
  digital_probe6?: number[];
  digital_probe7?: number[];
  digital_probe8?: number[];
  digital_probe9?: number[];
  digital_probe10?: number[];
  digital_probe11?: number[];
  digital_probe12?: number[];
  digital_probe13?: number[];
  digital_probe14?: number[];
  digital_probe15?: number[];
  digital_probe16?: number[];
  time_resolution: number;
  trigger_threshold: number;
  ns_per_sample?: number;
  /** True when `analog_probe1` is signed 14-bit data (PHA1 trapezoid /
   *  Delta probe). The waveform UI applies the +8191 centering offset
   *  only when this flag is true so unsigned ADC traces (PSD1/PSD2/AMax)
   *  render at their natural scale. Optional for backward compatibility
   *  with older event payloads. */
  analog_probe1_is_signed?: boolean;
  /** Same as `analog_probe1_is_signed` for the second analog probe. */
  analog_probe2_is_signed?: boolean;
  /** Same as `analog_probe1_is_signed` for the third analog probe (AMax
   *  debug FW). */
  analog_probe3_is_signed?: boolean;
  /** Probe-type identifier per analog probe — PHA2 canonical encoding:
   *  0=ADCInput / 1=TimeFilter / 2=EnergyFilter / 3=EnergyFilterBaseline
   *  / 4=EnergyFilterMinusBaseline / 0xFF=Unknown (FW that doesn't carry
   *  typed probe info on the wire). AMax debug FW uses 0x40+ codes
   *  (0x40=Raw / 0x41=Trapezoidal / 0x42=Triangle). The waveform UI maps
   *  these to display labels like "A0: TimeFilter". Optional for BC with
   *  old `.delila`. */
  analog_probe_type?: [number, number, number];
  /** Probe-type identifier per digital probe — PHA2 canonical encoding:
   *  0=Trigger / 1=TimeFilterArmed / 2=ReTriggerGuard /
   *  3=EnergyFilterBaselineFreeze / 4=EnergyFilterPeaking /
   *  5=EnergyFilterPeakReady / 6=EnergyFilterPileUpGuard / 7=EventPileUp
   *  / 8=ADCSaturation / 9=ADCSaturationProtection / A=PostSaturationEvent
   *  / B=EnergyFilterSaturation / C=SignalInhibit / 0xFF=Unknown. AMax
   *  debug FW uses 0x40+ codes (0x40=Trigger Out / 0x41=BL Hold /
   *  0x42=Energy Ready / 0x43=Shaping Ready / 0x44=Shaping Tracking). */
  digital_probe_type?: [
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
    number,
  ];
}

/** Sentinel matching `UNKNOWN_PROBE_TYPE` in src/common/mod.rs.
 *  FW that doesn't carry typed probe info on the wire emits this so the
 *  UI falls back to a generic "A0" / "D0" label. */
export const UNKNOWN_PROBE_TYPE = 0xff;

/** PHA2 canonical analog-probe-type → label, per CAEN doxygen
 *  `legacy/PHA2_Parameters/a00108.html`. AMax debug FW codes start at
 *  0x40 (disjoint from the 0x00..0x0C PHA2 range). */
export const ANALOG_PROBE_TYPE_LABELS: Record<number, string> = {
  0x00: 'ADCInput',
  0x01: 'TimeFilter',
  0x02: 'EnergyFilter',
  0x03: 'EnergyFilterBaseline',
  0x04: 'EnergyFilterMinusBaseline',
  0x40: 'Raw',
  0x41: 'Trapezoidal',
  0x42: 'Triangle',
};

/** PHA2 canonical digital-probe-type → label. AMax debug FW codes start
 *  at 0x40 (disjoint from the 0x00..0x0C PHA2 range). */
export const DIGITAL_PROBE_TYPE_LABELS: Record<number, string> = {
  0x00: 'Trigger',
  0x01: 'TimeFilterArmed',
  0x02: 'ReTriggerGuard',
  0x03: 'EnergyFilterBaselineFreeze',
  0x04: 'EnergyFilterPeaking',
  0x05: 'EnergyFilterPeakReady',
  0x06: 'EnergyFilterPileUpGuard',
  0x07: 'EventPileUp',
  0x08: 'ADCSaturation',
  0x09: 'ADCSaturationProtection',
  0x0a: 'PostSaturationEvent',
  0x0b: 'EnergyFilterSaturation',
  0x0c: 'SignalInhibit',
  0x40: 'Trigger Out',
  0x41: 'BL Hold',
  0x42: 'Energy Ready',
  0x43: 'Shaping Ready',
  0x44: 'Shaping Tracking',
};

// Latest waveform response
export interface LatestWaveform {
  module_id: number;
  channel_id: number;
  energy: number;
  timestamp_ns: number;
  waveform: Waveform;
}

// Waveform list response
export interface WaveformListResponse {
  channels: WaveformChannelInfo[];
}

export interface WaveformChannelInfo {
  module_id: number;
  channel_id: number;
  name?: string;
}

// Legacy helpers (for backward compatibility)
export function createDefaultCell(): HistogramCell {
  return {
    sourceId: null,
    channelId: null,
    xRange: 'auto',
    yRange: 'auto',
    isLocked: false,
  };
}

export function createDefaultTab(name: string): MonitorTab {
  return {
    id: generateUUID(),
    name,
    gridRows: 2,
    gridCols: 2,
    cells: Array(4)
      .fill(null)
      .map(() => createDefaultCell()),
  };
}
