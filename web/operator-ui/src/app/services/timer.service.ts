import { Injectable, signal, computed } from '@angular/core';

@Injectable({
  providedIn: 'root',
})
export class TimerService {
  // Timer state
  readonly durationMinutes = signal(10); // Default 10 minutes
  readonly autoStop = signal(true); // Default enabled
  readonly isRunning = signal(false);
  readonly remainingSeconds = signal(0);

  // Computed values
  readonly remainingDisplay = computed(() => {
    const total = this.remainingSeconds();
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const seconds = total % 60;

    if (hours > 0) {
      return `${hours}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`;
    }
    return `${minutes}:${seconds.toString().padStart(2, '0')}`;
  });

  readonly progress = computed(() => {
    const total = this.durationMinutes() * 60;
    const remaining = this.remainingSeconds();
    if (total === 0) return 0;
    return ((total - remaining) / total) * 100;
  });

  private intervalId: ReturnType<typeof setInterval> | null = null;
  private audioContext: AudioContext | null = null;
  private alarmIntervalId: ReturnType<typeof setInterval> | null = null;

  // Callbacks
  onTimerComplete: (() => void) | null = null;

  startTimer(): void {
    if (this.isRunning()) return;

    this.remainingSeconds.set(this.durationMinutes() * 60);
    this.isRunning.set(true);

    this.intervalId = setInterval(() => {
      const current = this.remainingSeconds();
      if (current <= 0) {
        this.stopTimer();
        if (this.onTimerComplete) {
          this.onTimerComplete();
        }
      } else {
        this.remainingSeconds.set(current - 1);
      }
    }, 1000);
  }

  stopTimer(): void {
    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }
    this.isRunning.set(false);
  }

  resetTimer(): void {
    this.stopTimer();
    this.remainingSeconds.set(0);
  }

  // Start continuous alarm sound
  startAlarm(): void {
    this.playBeep();
    // Continue playing every 2 seconds
    this.alarmIntervalId = setInterval(() => {
      this.playBeep();
    }, 2000);
  }

  // Stop alarm sound
  stopAlarm(): void {
    if (this.alarmIntervalId) {
      clearInterval(this.alarmIntervalId);
      this.alarmIntervalId = null;
    }
    if (this.audioContext) {
      this.audioContext.close();
      this.audioContext = null;
    }
  }

  private playBeep(): void {
    try {
      // Create new context if needed
      if (!this.audioContext || this.audioContext.state === 'closed') {
        this.audioContext = new AudioContext();
      }

      const ctx = this.audioContext;

      // Play a series of beeps
      for (let i = 0; i < 3; i++) {
        const oscillator = ctx.createOscillator();
        const gainNode = ctx.createGain();

        oscillator.connect(gainNode);
        gainNode.connect(ctx.destination);

        oscillator.frequency.value = 880; // A5 note
        oscillator.type = 'sine';

        const startTime = ctx.currentTime + i * 0.3;
        gainNode.gain.setValueAtTime(0.5, startTime);
        gainNode.gain.exponentialRampToValueAtTime(0.01, startTime + 0.2);

        oscillator.start(startTime);
        oscillator.stop(startTime + 0.2);
      }
    } catch (e) {
      console.error('Failed to play alarm:', e);
    }
  }
}
