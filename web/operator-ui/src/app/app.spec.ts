import { ComponentFixture, TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { provideHttpClientTesting } from '@angular/common/http/testing';
import { NoopAnimationsModule } from '@angular/platform-browser/animations';
import { App } from './app';
import { OperatorService } from './services/operator.service';
import { SystemStatus } from './models/types';

describe('App', () => {
  let component: App;
  let fixture: ComponentFixture<App>;
  let operatorService: OperatorService;

  const mockSystemStatus: SystemStatus = {
    components: [
      {
        name: 'Reader-0',
        address: 'tcp://localhost:5555',
        state: 'Running',
        run_number: 1,
        metrics: {
          events_processed: 1000,
          bytes_transferred: 50000,
          queue_size: 10,
          queue_max: 100,
          event_rate: 100.5,
        },
        online: true,
      },
    ],
    system_state: 'Running',
  };

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [App, NoopAnimationsModule],
      providers: [OperatorService, provideHttpClient(), provideHttpClientTesting()],
    }).compileComponents();

    operatorService = TestBed.inject(OperatorService);
    // Mock startPolling to prevent actual HTTP calls
    spyOn(operatorService, 'startPolling').and.stub();

    fixture = TestBed.createComponent(App);
    component = fixture.componentInstance;
    fixture.detectChanges();
  });

  it('should create the app', () => {
    expect(component).toBeTruthy();
  });

  it('should have correct title', () => {
    const toolbar = fixture.nativeElement.querySelector('mat-toolbar');
    expect(toolbar.textContent).toContain('DELILA DAQ Control');
  });

  it('should call startPolling on init', () => {
    expect(operatorService.startPolling).toHaveBeenCalled();
  });

  it('should show Online when status is available', () => {
    operatorService.status.set(mockSystemStatus);
    fixture.detectChanges();

    const statusIndicator = fixture.nativeElement.querySelector('.status-indicator');
    expect(statusIndicator.textContent.trim()).toBe('Online');
    expect(statusIndicator.classList.contains('online')).toBeTrue();
  });

  it('should show Offline when status is null', () => {
    operatorService.status.set(null);
    fixture.detectChanges();

    const statusIndicator = fixture.nativeElement.querySelector('.status-indicator');
    expect(statusIndicator.textContent.trim()).toBe('Offline');
    expect(statusIndicator.classList.contains('offline')).toBeTrue();
  });

  it('should contain status panel component', () => {
    const statusPanel = fixture.nativeElement.querySelector('app-status-panel');
    expect(statusPanel).toBeTruthy();
  });

  it('should contain control panel component', () => {
    const controlPanel = fixture.nativeElement.querySelector('app-control-panel');
    expect(controlPanel).toBeTruthy();
  });

  it('should contain run info component', () => {
    const runInfo = fixture.nativeElement.querySelector('app-run-info');
    expect(runInfo).toBeTruthy();
  });

  it('should contain timer component', () => {
    const timer = fixture.nativeElement.querySelector('app-timer');
    expect(timer).toBeTruthy();
  });
});
