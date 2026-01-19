import { Component, Input, Output, EventEmitter } from '@angular/core';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { FormsModule } from '@angular/forms';
import { HistogramCellComponent } from '../histogram-cell/histogram-cell.component';
import { MonitorTab, HistogramCell, createDefaultCell } from '../../models/histogram.types';

@Component({
  selector: 'app-histogram-grid',
  standalone: true,
  imports: [
    MatFormFieldModule,
    MatInputModule,
    MatButtonModule,
    MatIconModule,
    FormsModule,
    HistogramCellComponent,
  ],
  template: `
    <div class="grid-controls">
      <mat-form-field appearance="outline" class="grid-size-input">
        <mat-label>Rows</mat-label>
        <input
          matInput
          type="number"
          [value]="tab.gridRows"
          (change)="onRowsChange($event)"
          min="1"
          max="4"
        />
      </mat-form-field>
      <span class="grid-size-separator">x</span>
      <mat-form-field appearance="outline" class="grid-size-input">
        <mat-label>Cols</mat-label>
        <input
          matInput
          type="number"
          [value]="tab.gridCols"
          (change)="onColsChange($event)"
          min="1"
          max="4"
        />
      </mat-form-field>

      <span class="spacer"></span>

      <button mat-stroked-button (click)="onApplyRangeToAll()">
        <mat-icon>sync</mat-icon>
        Apply Range to All
      </button>
      <button mat-stroked-button (click)="onResetAll()">
        <mat-icon>restart_alt</mat-icon>
        Reset All
      </button>
    </div>

    <div
      class="histogram-grid"
      [style.grid-template-rows]="'repeat(' + tab.gridRows + ', 1fr)'"
      [style.grid-template-columns]="'repeat(' + tab.gridCols + ', 1fr)'"
    >
      @for (cell of tab.cells; track $index; let i = $index) {
        @if (i < tab.gridRows * tab.gridCols) {
          <app-histogram-cell
            [cell]="cell"
            [cellIndex]="i"
            (cellChange)="onCellChange(i, $event)"
            (expand)="onCellExpand($event)"
          ></app-histogram-cell>
        }
      }
    </div>
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      height: 100%;
    }

    .grid-controls {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 8px 0;
      flex-shrink: 0;
    }

    .grid-size-input {
      width: 70px;
    }

    .grid-size-input ::ng-deep .mat-mdc-form-field-infix {
      padding: 8px 0 !important;
      min-height: 36px;
    }

    .grid-size-separator {
      font-size: 16px;
      color: #666;
    }

    .spacer {
      flex: 1;
    }

    .histogram-grid {
      display: grid;
      gap: 8px;
      flex: 1;
      min-height: 0;
    }
  `,
})
export class HistogramGridComponent {
  @Input() tab!: MonitorTab;

  @Output() tabChange = new EventEmitter<MonitorTab>();
  @Output() cellExpand = new EventEmitter<{ tabId: string; cellIndex: number }>();

  onRowsChange(event: Event): void {
    const input = event.target as HTMLInputElement;
    const rows = Math.min(4, Math.max(1, parseInt(input.value, 10) || 1));
    this.updateGridSize(rows, this.tab.gridCols);
  }

  onColsChange(event: Event): void {
    const input = event.target as HTMLInputElement;
    const cols = Math.min(4, Math.max(1, parseInt(input.value, 10) || 1));
    this.updateGridSize(this.tab.gridRows, cols);
  }

  private updateGridSize(rows: number, cols: number): void {
    const newCellCount = rows * cols;
    const currentCells = [...this.tab.cells];

    // Adjust cells array size
    while (currentCells.length < newCellCount) {
      currentCells.push(createDefaultCell());
    }

    this.tabChange.emit({
      ...this.tab,
      gridRows: rows,
      gridCols: cols,
      cells: currentCells.slice(0, newCellCount),
    });
  }

  onCellChange(index: number, cell: HistogramCell): void {
    const cells = [...this.tab.cells];
    cells[index] = cell;
    this.tabChange.emit({ ...this.tab, cells });
  }

  onCellExpand(cellIndex: number): void {
    this.cellExpand.emit({ tabId: this.tab.id, cellIndex });
  }

  onApplyRangeToAll(): void {
    // Find first cell with locked range
    const sourceCell = this.tab.cells.find((c) => c.isLocked && c.xRange !== 'auto');
    if (!sourceCell) return;

    const cells = this.tab.cells.map((cell) => ({
      ...cell,
      xRange: sourceCell.xRange,
      yRange: sourceCell.yRange,
      isLocked: true,
    }));

    this.tabChange.emit({ ...this.tab, cells });
  }

  onResetAll(): void {
    const cells = this.tab.cells.map((cell) => ({
      ...cell,
      xRange: 'auto' as const,
      yRange: 'auto' as const,
      isLocked: false,
    }));

    this.tabChange.emit({ ...this.tab, cells });
  }
}
