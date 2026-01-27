import {
  Component,
  input,
  output,
  computed,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';

/**
 * Definition of a channel parameter (one row in the table)
 */
export interface ChannelParamDef {
  /** Key in ChannelConfig (e.g., 'trigger_threshold') */
  key: string;
  /** Display label (e.g., 'Threshold') */
  label: string;
  /** Input type */
  type: 'number' | 'enum' | 'boolean';
  /** Options for enum type (e.g., ['Positive', 'Negative']) */
  options?: string[];
  /** Unit label (e.g., 'ns', '%', 'ADC') */
  unit?: string;
  /** Min value for number type */
  min?: number;
  /** Max value for number type */
  max?: number;
}

/**
 * Emitted when the "All" (default) column value changes
 */
export interface DefaultValueChange {
  key: string;
  value: unknown;
}

/**
 * Emitted when a specific channel's value changes
 */
export interface ChannelValueChange {
  channel: number;
  key: string;
  value: unknown;
}

/**
 * Reusable channel parameter table component.
 *
 * Displays parameters as rows, channels as columns.
 * The leftmost columns (Parameter name + All) are sticky.
 * Cells that differ from the "All" column are highlighted.
 *
 * Usage:
 * ```html
 * <app-channel-table
 *   [params]="frequentParams"
 *   [numChannels]="32"
 *   [defaultValues]="channelDefaults"
 *   [channelValues]="expandedChannelValues"
 *   (defaultChange)="onDefaultChange($event)"
 *   (channelChange)="onChannelChange($event)"
 * />
 * ```
 */
@Component({
  selector: 'app-channel-table',
  standalone: true,
  imports: [CommonModule, FormsModule],
  template: `
    <div class="channel-table-wrapper">
      <table class="channel-table">
        <thead>
          <tr>
            <th class="sticky-col param-header">Parameter</th>
            <th class="sticky-col all-header">All</th>
            @for (ch of channelIndices(); track ch) {
              <th class="ch-header">{{ ch }}</th>
            }
          </tr>
        </thead>
        <tbody>
          @for (param of params(); track param.key) {
            <tr>
              <td class="sticky-col param-cell">
                {{ param.label }}
                @if (param.unit) {
                  <span class="unit">({{ param.unit }})</span>
                }
              </td>
              <td class="sticky-col all-cell">
                @switch (param.type) {
                  @case ('number') {
                    <input
                      type="number"
                      class="cell-input"
                      [value]="getDefault(param.key)"
                      [min]="param.min"
                      [max]="param.max"
                      (change)="onDefaultInput(param.key, $event)"
                    />
                  }
                  @case ('enum') {
                    <select
                      class="cell-select"
                      [value]="getDefault(param.key) ?? ''"
                      (change)="onDefaultSelect(param.key, $event)"
                    >
                      @for (opt of param.options ?? []; track opt) {
                        <option [value]="opt">{{ opt }}</option>
                      }
                    </select>
                  }
                  @case ('boolean') {
                    <select
                      class="cell-select"
                      [value]="getDefault(param.key) ?? 'True'"
                      (change)="onDefaultSelect(param.key, $event)"
                    >
                      <option value="True">ON</option>
                      <option value="False">OFF</option>
                    </select>
                  }
                }
              </td>
              @for (ch of channelIndices(); track ch) {
                <td
                  class="ch-cell"
                  [class.override]="isOverride(ch, param.key)"
                >
                  @switch (param.type) {
                    @case ('number') {
                      <input
                        type="number"
                        class="cell-input"
                        [value]="getChannel(ch, param.key)"
                        [min]="param.min"
                        [max]="param.max"
                        (change)="onChannelInput(ch, param.key, $event)"
                      />
                    }
                    @case ('enum') {
                      <select
                        class="cell-select"
                        [value]="getChannel(ch, param.key) ?? ''"
                        (change)="onChannelSelect(ch, param.key, $event)"
                      >
                        @for (opt of param.options ?? []; track opt) {
                          <option [value]="opt">{{ opt }}</option>
                        }
                      </select>
                    }
                    @case ('boolean') {
                      <select
                        class="cell-select"
                        [value]="getChannel(ch, param.key) ?? 'True'"
                        (change)="onChannelSelect(ch, param.key, $event)"
                      >
                        <option value="True">ON</option>
                        <option value="False">OFF</option>
                      </select>
                    }
                  }
                </td>
              }
            </tr>
          }
        </tbody>
      </table>
    </div>
  `,
  styles: `
    .channel-table-wrapper {
      overflow-x: auto;
      max-width: 100%;
      border: 1px solid #e0e0e0;
      border-radius: 4px;
    }

    .channel-table {
      border-collapse: separate;
      border-spacing: 0;
      font-size: 13px;
      white-space: nowrap;
    }

    th, td {
      padding: 4px 6px;
      border-bottom: 1px solid #e0e0e0;
      border-right: 1px solid #f0f0f0;
    }

    thead th {
      background: #fafafa;
      font-weight: 500;
      text-align: center;
      position: sticky;
      top: 0;
      z-index: 1;
    }

    /* Sticky columns: Parameter name + All */
    .sticky-col {
      position: sticky;
      z-index: 2;
      background: #fff;
    }

    thead .sticky-col {
      z-index: 3;
      background: #fafafa;
    }

    .param-header, .param-cell {
      left: 0;
      min-width: 120px;
      max-width: 160px;
      font-weight: 500;
      border-right: 2px solid #e0e0e0;
    }

    .all-header, .all-cell {
      left: 120px;
      min-width: 80px;
      border-right: 2px solid #1976d2;
      background: #e3f2fd;
    }

    thead .all-header {
      background: #bbdefb;
      font-weight: 600;
      color: #1565c0;
    }

    .ch-header {
      min-width: 72px;
      text-align: center;
    }

    .ch-cell {
      text-align: center;
    }

    /* Highlight overridden cells */
    .ch-cell.override {
      background-color: #fff3e0;
    }

    .unit {
      font-size: 11px;
      color: #999;
      margin-left: 2px;
    }

    /* Compact inputs */
    .cell-input {
      width: 64px;
      padding: 2px 4px;
      border: 1px solid #ccc;
      border-radius: 3px;
      font-size: 13px;
      text-align: center;
      background: transparent;
    }

    .cell-input:focus {
      outline: none;
      border-color: #1976d2;
    }

    .cell-select {
      width: 72px;
      padding: 2px;
      border: 1px solid #ccc;
      border-radius: 3px;
      font-size: 12px;
      background: transparent;
      cursor: pointer;
    }

    .cell-select:focus {
      outline: none;
      border-color: #1976d2;
    }

    /* Zebra striping */
    tbody tr:nth-child(even) td {
      background-color: inherit;
    }

    tbody tr:hover td {
      background-color: #f5f5f5;
    }

    tbody tr:hover td.sticky-col {
      background-color: #f5f5f5;
    }

    tbody tr:hover td.all-cell {
      background-color: #e3f2fd;
    }

    tbody tr:hover td.override {
      background-color: #ffe0b2;
    }
  `,
})
export class ChannelTableComponent {
  /** Parameter definitions (one per row) */
  readonly params = input.required<ChannelParamDef[]>();
  /** Number of channels */
  readonly numChannels = input.required<number>();
  /** Default values (the "All" column) — keyed by param.key */
  readonly defaultValues = input.required<Record<string, unknown>>();
  /** Per-channel values — array of length numChannels, each keyed by param.key */
  readonly channelValues = input.required<Record<string, unknown>[]>();

  /** Emitted when a value in the "All" column changes */
  readonly defaultChange = output<DefaultValueChange>();
  /** Emitted when a specific channel value changes */
  readonly channelChange = output<ChannelValueChange>();

  /** Array [0, 1, 2, ..., numChannels-1] for iteration */
  readonly channelIndices = computed(() =>
    Array.from({ length: this.numChannels() }, (_, i) => i)
  );

  /** Get the default (All column) value for a parameter */
  getDefault(key: string): unknown {
    return this.defaultValues()[key];
  }

  /** Get a channel's value for a parameter */
  getChannel(ch: number, key: string): unknown {
    const values = this.channelValues();
    return values[ch]?.[key];
  }

  /** Check if a channel value differs from the All column */
  isOverride(ch: number, key: string): boolean {
    const defaultVal = this.defaultValues()[key];
    const chVal = this.channelValues()[ch]?.[key];
    // Both undefined/null → not override
    if (defaultVal == null && chVal == null) return false;
    return defaultVal !== chVal;
  }

  /** Handle number input change in the All column */
  onDefaultInput(key: string, event: Event): void {
    const input = event.target as HTMLInputElement;
    const value = input.value === '' ? undefined : Number(input.value);
    this.defaultChange.emit({ key, value });
  }

  /** Handle select change in the All column */
  onDefaultSelect(key: string, event: Event): void {
    const select = event.target as HTMLSelectElement;
    this.defaultChange.emit({ key, value: select.value });
  }

  /** Handle number input change in a channel column */
  onChannelInput(ch: number, key: string, event: Event): void {
    const input = event.target as HTMLInputElement;
    const value = input.value === '' ? undefined : Number(input.value);
    this.channelChange.emit({ channel: ch, key, value });
  }

  /** Handle select change in a channel column */
  onChannelSelect(ch: number, key: string, event: Event): void {
    const select = event.target as HTMLSelectElement;
    this.channelChange.emit({ channel: ch, key, value: select.value });
  }
}
