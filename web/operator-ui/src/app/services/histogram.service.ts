import { Injectable, inject, signal, computed } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable, interval, switchMap, catchError, of, tap, Subject, takeUntil } from 'rxjs';
import {
  Histogram1D,
  HistogramListResponse,
  MonitorStatusResponse,
  ChannelSummary,
  channelKeyString,
  WaveformListResponse,
  LatestWaveform,
} from '../models/histogram.types';

@Injectable({
  providedIn: 'root',
})
export class HistogramService {
  private readonly baseUrl = 'http://localhost:8081/api';
  private readonly refreshInterval = 1000; // 1 second

  private readonly http = inject(HttpClient);
  private stopPolling$ = new Subject<void>();

  // Signals for reactive state
  readonly status = signal<MonitorStatusResponse | null>(null);
  readonly channelList = signal<ChannelSummary[]>([]);
  readonly histogramCache = signal<Map<string, Histogram1D>>(new Map());
  readonly isPolling = signal(false);
  readonly error = signal<string | null>(null);

  // Computed values
  readonly totalEvents = computed(() => this.status()?.total_events ?? 0);
  readonly eventRate = computed(() => this.status()?.event_rate ?? 0);
  readonly elapsedSecs = computed(() => this.status()?.elapsed_secs ?? 0);
  readonly numChannels = computed(() => this.status()?.num_channels ?? 0);

  // Get histogram from cache
  getHistogram(moduleId: number, channelId: number): Histogram1D | undefined {
    return this.histogramCache().get(channelKeyString(moduleId, channelId));
  }

  // Start polling for status and histogram list
  startPolling(): void {
    if (this.isPolling()) return;
    this.isPolling.set(true);

    // Poll status
    interval(this.refreshInterval)
      .pipe(
        takeUntil(this.stopPolling$),
        switchMap(() => this.fetchStatus()),
        tap((status) => {
          if (status) {
            this.status.set(status);
            this.error.set(null);
          }
        }),
        catchError(() => {
          this.error.set('Failed to connect to Monitor');
          return of(null);
        })
      )
      .subscribe();

    // Poll histogram list
    interval(this.refreshInterval)
      .pipe(
        takeUntil(this.stopPolling$),
        switchMap(() => this.fetchHistogramList()),
        tap((list) => {
          if (list) {
            this.channelList.set(list.channels);
          }
        }),
        catchError(() => of(null))
      )
      .subscribe();

    // Initial fetch
    this.fetchStatus().subscribe((status) => {
      if (status) this.status.set(status);
    });
    this.fetchHistogramList().subscribe((list) => {
      if (list) this.channelList.set(list.channels);
    });
  }

  stopPolling(): void {
    this.stopPolling$.next();
    this.isPolling.set(false);
  }

  // Fetch specific histogram and update cache
  fetchAndCacheHistogram(moduleId: number, channelId: number): Observable<Histogram1D | null> {
    return this.fetchHistogram(moduleId, channelId).pipe(
      tap((histogram) => {
        if (histogram) {
          const key = channelKeyString(moduleId, channelId);
          const cache = new Map(this.histogramCache());
          cache.set(key, histogram);
          this.histogramCache.set(cache);
        }
      }),
      catchError(() => of(null))
    );
  }

  // API calls
  fetchStatus(): Observable<MonitorStatusResponse | null> {
    return this.http.get<MonitorStatusResponse>(`${this.baseUrl}/status`).pipe(catchError(() => of(null)));
  }

  fetchHistogramList(): Observable<HistogramListResponse | null> {
    return this.http.get<HistogramListResponse>(`${this.baseUrl}/histograms`).pipe(catchError(() => of(null)));
  }

  fetchHistogram(moduleId: number, channelId: number): Observable<Histogram1D | null> {
    return this.http
      .get<Histogram1D>(`${this.baseUrl}/histograms/${moduleId}/${channelId}`)
      .pipe(catchError(() => of(null)));
  }

  clearHistograms(): Observable<void> {
    return this.http.post<void>(`${this.baseUrl}/histograms/clear`, {});
  }

  // Waveform API calls
  fetchWaveformList(): Observable<WaveformListResponse | null> {
    return this.http.get<WaveformListResponse>(`${this.baseUrl}/waveforms`).pipe(catchError(() => of(null)));
  }

  fetchWaveform(moduleId: number, channelId: number): Observable<LatestWaveform | null> {
    return this.http
      .get<LatestWaveform>(`${this.baseUrl}/waveforms/${moduleId}/${channelId}`)
      .pipe(catchError(() => of(null)));
  }
}
