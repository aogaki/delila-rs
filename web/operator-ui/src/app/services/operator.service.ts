import { Injectable, signal, computed, inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable, interval, switchMap, catchError, of, tap } from 'rxjs';
import {
  SystemStatus,
  ConfigureRequest,
  ApiResponse,
  getButtonStates,
  ButtonStates,
  CurrentRunInfo,
} from '../models/types';

@Injectable({
  providedIn: 'root',
})
export class OperatorService {
  private readonly baseUrl = 'http://localhost:8080/api';
  private readonly pollingInterval = 1000; // 1 second

  // Signals for reactive state
  readonly status = signal<SystemStatus | null>(null);
  readonly error = signal<string | null>(null);
  readonly isPolling = signal(false);

  // Computed values
  readonly systemState = computed(() => this.status()?.system_state ?? 'Offline');
  readonly components = computed(() => this.status()?.components ?? []);
  readonly buttonStates = computed<ButtonStates>(() => getButtonStates(this.systemState()));
  readonly runInfo = computed<CurrentRunInfo | null>(() => this.status()?.run_info ?? null);

  // Aggregate metrics
  readonly totalEvents = computed(() => {
    const comps = this.components();
    return comps.reduce((sum, c) => sum + (c.metrics?.events_processed ?? 0), 0);
  });

  readonly totalRate = computed(() => {
    const comps = this.components();
    return comps.reduce((sum, c) => sum + (c.metrics?.event_rate ?? 0), 0);
  });

  private readonly http = inject(HttpClient);

  // Start polling for status
  startPolling(): void {
    if (this.isPolling()) return;
    this.isPolling.set(true);

    interval(this.pollingInterval)
      .pipe(
        switchMap(() => this.getStatus()),
        tap((status) => {
          this.status.set(status);
          this.error.set(null);
        }),
        catchError(() => {
          this.error.set('Failed to connect to Operator');
          this.status.set(null);
          return of(null);
        })
      )
      .subscribe();

    // Initial fetch
    this.getStatus().subscribe({
      next: (status) => {
        this.status.set(status);
        this.error.set(null);
      },
      error: () => {
        this.error.set('Failed to connect to Operator');
        this.status.set(null);
      },
    });
  }

  stopPolling(): void {
    this.isPolling.set(false);
  }

  // API calls
  getStatus(): Observable<SystemStatus> {
    return this.http.get<SystemStatus>(`${this.baseUrl}/status`);
  }

  configure(request: ConfigureRequest): Observable<ApiResponse> {
    return this.http.post<ApiResponse>(`${this.baseUrl}/configure`, request);
  }

  // Note: arm() removed - backend auto-arms on start()
  // run_number is passed at start time to allow changing it without re-configure
  start(runNumber: number): Observable<ApiResponse> {
    return this.http.post<ApiResponse>(`${this.baseUrl}/start`, { run_number: runNumber });
  }

  stop(): Observable<ApiResponse> {
    return this.http.post<ApiResponse>(`${this.baseUrl}/stop`, {});
  }

  reset(): Observable<ApiResponse> {
    return this.http.post<ApiResponse>(`${this.baseUrl}/reset`, {});
  }
}
