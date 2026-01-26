import { Component } from '@angular/core';
import { CommonModule } from '@angular/common';
import { MatTabsModule } from '@angular/material/tabs';
import { DigitizerSettingsComponent } from '../../components/digitizer-settings/digitizer-settings.component';
import { EmulatorSettingsComponent } from '../../components/emulator-settings/emulator-settings.component';

@Component({
  selector: 'app-settings-page',
  standalone: true,
  imports: [CommonModule, MatTabsModule, DigitizerSettingsComponent, EmulatorSettingsComponent],
  template: `
    <div class="settings-container">
      <mat-tab-group>
        <mat-tab label="Digitizers">
          <app-digitizer-settings />
        </mat-tab>
        <mat-tab label="Emulator">
          <app-emulator-settings />
        </mat-tab>
      </mat-tab-group>
    </div>
  `,
  styles: `
    .settings-container {
      padding: 16px;
      height: 100%;
    }
  `,
})
export class SettingsPageComponent {}
