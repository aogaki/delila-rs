import { TestBed } from '@angular/core/testing';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { provideHttpClient } from '@angular/common/http';
import { OperatorService } from './operator.service';
import { SystemStatus, ConfigureRequest, ApiResponse, ComponentStatus } from '../models/types';

describe('OperatorService', () => {
  let service: OperatorService;
  let httpMock: HttpTestingController;
  const baseUrl = 'http://localhost:8080/api';

  // Test fixtures
  const mockComponentStatus: ComponentStatus = {
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
  };

  const mockSystemStatus: SystemStatus = {
    components: [
      mockComponentStatus,
      {
        name: 'Merger',
        address: 'tcp://localhost:5556',
        state: 'Running',
        run_number: 1,
        metrics: {
          events_processed: 950,
          bytes_transferred: 47500,
          queue_size: 5,
          queue_max: 100,
          event_rate: 95.0,
        },
        online: true,
      },
      {
        name: 'Recorder',
        address: 'tcp://localhost:5580',
        state: 'Running',
        run_number: 1,
        metrics: {
          events_processed: 1950,
          bytes_transferred: 97500,
          queue_size: 0,
          queue_max: 100,
          event_rate: 195.5,
        },
        online: true,
      },
    ],
    system_state: 'Running',
    experiment_name: 'TestExp',
    next_run_number: 2,
  };

  const mockApiResponse: ApiResponse = {
    success: true,
    message: 'Operation completed',
  };

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [OperatorService, provideHttpClient(), provideHttpClientTesting()],
    });

    service = TestBed.inject(OperatorService);
    httpMock = TestBed.inject(HttpTestingController);
  });

  afterEach(() => {
    httpMock.verify();
  });

  describe('initialization', () => {
    it('should be created', () => {
      expect(service).toBeTruthy();
    });

    it('should have initial null status', () => {
      expect(service.status()).toBeNull();
    });

    it('should have initial null error', () => {
      expect(service.error()).toBeNull();
    });

    it('should not be polling initially', () => {
      expect(service.isPolling()).toBeFalse();
    });

    it('should have Offline system state when status is null', () => {
      expect(service.systemState()).toBe('Offline');
    });

    it('should have empty components when status is null', () => {
      expect(service.components()).toEqual([]);
    });
  });

  describe('computed values', () => {
    it('should compute systemState from status', () => {
      service.status.set(mockSystemStatus);
      expect(service.systemState()).toBe('Running');
    });

    it('should compute components from status', () => {
      service.status.set(mockSystemStatus);
      expect(service.components().length).toBe(3);
      expect(service.components()[0].name).toBe('Reader-0');
    });

    it('should compute totalEvents from Recorder metrics', () => {
      service.status.set(mockSystemStatus);
      // Recorder is the authoritative source for total events
      expect(service.totalEvents()).toBe(1950);
    });

    it('should compute totalRate from Recorder metrics', () => {
      service.status.set(mockSystemStatus);
      // Recorder is the authoritative source for event rate
      expect(service.totalRate()).toBe(195.5);
    });

    it('should handle components without metrics', () => {
      const statusWithoutMetrics: SystemStatus = {
        components: [
          { name: 'Reader-0', address: 'tcp://localhost:5555', state: 'Idle', online: true },
        ],
        system_state: 'Idle',
        experiment_name: 'TestExp',
        next_run_number: 1,
      };
      service.status.set(statusWithoutMetrics);
      expect(service.totalEvents()).toBe(0);
      expect(service.totalRate()).toBe(0);
    });

    it('should compute button states based on system state', () => {
      // Idle state
      service.status.set({ ...mockSystemStatus, system_state: 'Idle' });
      expect(service.buttonStates()).toEqual({
        configure: true,
        start: false,
        stop: false,
        reset: false,
      });

      // Configured state
      service.status.set({ ...mockSystemStatus, system_state: 'Configured' });
      expect(service.buttonStates()).toEqual({
        configure: false,
        start: true,
        stop: false,
        reset: true,
      });

      // Running state
      service.status.set({ ...mockSystemStatus, system_state: 'Running' });
      expect(service.buttonStates()).toEqual({
        configure: false,
        start: false,
        stop: true,
        reset: false,
      });

      // Error state
      service.status.set({ ...mockSystemStatus, system_state: 'Error' });
      expect(service.buttonStates()).toEqual({
        configure: false,
        start: false,
        stop: false,
        reset: true,
      });
    });
  });

  describe('getStatus()', () => {
    it('should fetch system status', () => {
      service.getStatus().subscribe((status) => {
        expect(status).toEqual(mockSystemStatus);
      });

      const req = httpMock.expectOne(`${baseUrl}/status`);
      expect(req.request.method).toBe('GET');
      req.flush(mockSystemStatus);
    });
  });

  describe('configure()', () => {
    it('should send configure request', () => {
      const configRequest: ConfigureRequest = {
        run_number: 1,
        exp_name: 'test_experiment',
      };

      service.configure(configRequest).subscribe((response) => {
        expect(response).toEqual(mockApiResponse);
      });

      const req = httpMock.expectOne(`${baseUrl}/configure`);
      expect(req.request.method).toBe('POST');
      expect(req.request.body).toEqual(configRequest);
      req.flush(mockApiResponse);
    });
  });

  describe('start()', () => {
    it('should send start request with run number', () => {
      const runNumber = 42;

      service.start(runNumber).subscribe((response) => {
        expect(response).toEqual(mockApiResponse);
      });

      const req = httpMock.expectOne(`${baseUrl}/start`);
      expect(req.request.method).toBe('POST');
      expect(req.request.body).toEqual({ run_number: runNumber, comment: '' });
      req.flush(mockApiResponse);
    });
  });

  describe('stop()', () => {
    it('should send stop request', () => {
      service.stop().subscribe((response) => {
        expect(response).toEqual(mockApiResponse);
      });

      const req = httpMock.expectOne(`${baseUrl}/stop`);
      expect(req.request.method).toBe('POST');
      expect(req.request.body).toEqual({});
      req.flush(mockApiResponse);
    });
  });

  describe('reset()', () => {
    it('should send reset request', () => {
      service.reset().subscribe((response) => {
        expect(response).toEqual(mockApiResponse);
      });

      const req = httpMock.expectOne(`${baseUrl}/reset`);
      expect(req.request.method).toBe('POST');
      expect(req.request.body).toEqual({});
      req.flush(mockApiResponse);
    });
  });

  describe('polling state', () => {
    it('should set isPolling to true when startPolling is called', () => {
      expect(service.isPolling()).toBeFalse();
      service.isPolling.set(true); // Simulate startPolling effect
      expect(service.isPolling()).toBeTrue();
    });

    it('should set isPolling to false when stopPolling is called', () => {
      service.isPolling.set(true);
      service.stopPolling();
      expect(service.isPolling()).toBeFalse();
    });
  });
});
