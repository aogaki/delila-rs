import { ComponentFixture, TestBed, fakeAsync, tick, discardPeriodicTasks } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { provideHttpClientTesting, HttpTestingController } from '@angular/common/http/testing';
import { NoopAnimationsModule } from '@angular/platform-browser/animations';
import { App } from './app';
import { OperatorService } from './services/operator.service';
import { SystemStatus } from './models/types';

describe('App', () => {
  let component: App;
  let fixture: ComponentFixture<App>;
  let httpMock: HttpTestingController;
  let operatorService: OperatorService;
  const baseUrl = 'http://localhost:8080/api';

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

    fixture = TestBed.createComponent(App);
    component = fixture.componentInstance;
    httpMock = TestBed.inject(HttpTestingController);
    operatorService = TestBed.inject(OperatorService);
  });

  afterEach(() => {
    httpMock.verify();
  });

  it('should create the app', () => {
    expect(component).toBeTruthy();
  });

  it('should have correct title', fakeAsync(() => {
    fixture.detectChanges();

    // Handle the initial status fetch from ngOnInit
    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    const toolbar = fixture.nativeElement.querySelector('mat-toolbar');
    expect(toolbar.textContent).toContain('DELILA DAQ Control');

    discardPeriodicTasks();
  }));

  it('should start polling on init', fakeAsync(() => {
    expect(operatorService.isPolling()).toBeFalse();

    fixture.detectChanges(); // triggers ngOnInit

    // Handle the initial status fetch
    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    expect(operatorService.isPolling()).toBeTrue();

    discardPeriodicTasks();
  }));

  it('should show Online when status is available', fakeAsync(() => {
    fixture.detectChanges();

    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    fixture.detectChanges();

    const statusIndicator = fixture.nativeElement.querySelector('.status-indicator');
    expect(statusIndicator.textContent.trim()).toBe('Online');
    expect(statusIndicator.classList.contains('online')).toBeTrue();

    discardPeriodicTasks();
  }));

  it('should show Offline when status is null', fakeAsync(() => {
    fixture.detectChanges();

    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.error(new ProgressEvent('Network error'));
    tick();

    fixture.detectChanges();

    const statusIndicator = fixture.nativeElement.querySelector('.status-indicator');
    expect(statusIndicator.textContent.trim()).toBe('Offline');
    expect(statusIndicator.classList.contains('offline')).toBeTrue();

    discardPeriodicTasks();
  }));

  it('should contain status panel component', fakeAsync(() => {
    fixture.detectChanges();

    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    const statusPanel = fixture.nativeElement.querySelector('app-status-panel');
    expect(statusPanel).toBeTruthy();

    discardPeriodicTasks();
  }));

  it('should contain control panel component', fakeAsync(() => {
    fixture.detectChanges();

    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    const controlPanel = fixture.nativeElement.querySelector('app-control-panel');
    expect(controlPanel).toBeTruthy();

    discardPeriodicTasks();
  }));

  it('should contain run info component', fakeAsync(() => {
    fixture.detectChanges();

    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    const runInfo = fixture.nativeElement.querySelector('app-run-info');
    expect(runInfo).toBeTruthy();

    discardPeriodicTasks();
  }));

  it('should contain timer component', fakeAsync(() => {
    fixture.detectChanges();

    const req = httpMock.expectOne(`${baseUrl}/status`);
    req.flush(mockSystemStatus);
    tick();

    const timer = fixture.nativeElement.querySelector('app-timer');
    expect(timer).toBeTruthy();

    discardPeriodicTasks();
  }));
});
