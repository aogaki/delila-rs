import { Routes } from '@angular/router';

export const routes: Routes = [
  { path: '', redirectTo: 'control', pathMatch: 'full' },
  {
    path: 'control',
    loadComponent: () =>
      import('./pages/control/control.component').then((m) => m.ControlPageComponent),
  },
  {
    path: 'monitor',
    loadComponent: () =>
      import('./pages/monitor/monitor.component').then((m) => m.MonitorPageComponent),
  },
  {
    path: 'waveform',
    loadComponent: () =>
      import('./pages/waveform/waveform.component').then((m) => m.WaveformPageComponent),
  },
  {
    path: 'settings',
    loadComponent: () =>
      import('./pages/settings/settings.component').then((m) => m.SettingsPageComponent),
  },
];
