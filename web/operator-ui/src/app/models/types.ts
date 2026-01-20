// Component states matching Rust enum
export type ComponentState =
  | 'Idle'
  | 'Configuring'
  | 'Configured'
  | 'Arming'
  | 'Armed'
  | 'Starting'
  | 'Running'
  | 'Stopping'
  | 'Error';

// System-wide state
export type SystemState =
  | 'Idle'
  | 'Configuring'
  | 'Configured'
  | 'Arming'
  | 'Armed'
  | 'Starting'
  | 'Running'
  | 'Stopping'
  | 'Error'
  | 'Mixed'
  | 'Offline';

// Metrics for a component
export interface ComponentMetrics {
  events_processed: number;
  bytes_transferred: number;
  queue_size: number;
  queue_max: number;
  event_rate: number;
}

// Status of a single component
export interface ComponentStatus {
  name: string;
  address: string;
  state: ComponentState;
  run_number?: number;
  metrics?: ComponentMetrics;
  error?: string;
  online: boolean;
}

// Run status
export type RunStatus = 'running' | 'completed' | 'error' | 'aborted';

// Run statistics
export interface RunStats {
  total_events: number;
  total_bytes: number;
  average_rate: number;
}

// Run note entry (append-only logbook style)
export interface RunNote {
  time: number; // UNIX timestamp in milliseconds
  text: string;
}

// Current run information
export interface CurrentRunInfo {
  run_number: number;
  exp_name: string;
  comment: string;
  start_time: string; // ISO date string
  elapsed_secs: number;
  status: RunStatus;
  stats: RunStats;
  notes: RunNote[];
}

// Last run info for pre-filling comment field
export interface LastRunInfo {
  run_number: number;
  comment: string;
  notes: RunNote[];
}

// System-wide status
export interface SystemStatus {
  components: ComponentStatus[];
  system_state: SystemState;
  run_info?: CurrentRunInfo;
  /** Experiment name (server-authoritative, from config file) */
  experiment_name: string;
  /** Next run number (from MongoDB, for multi-client sync) */
  next_run_number?: number;
  /** Last run info for pre-filling comment (comment + notes from previous run) */
  last_run_info?: LastRunInfo;
}

// Configure request
export interface ConfigureRequest {
  run_number: number;
  exp_name: string;
}

// API response
export interface ApiResponse {
  success: boolean;
  message: string;
}

// Button enable states based on system state
// Note: arm is removed from UI - backend auto-arms on start
export interface ButtonStates {
  configure: boolean;
  start: boolean;
  stop: boolean;
  reset: boolean;
}

// Get button states based on system state
export function getButtonStates(state: SystemState): ButtonStates {
  switch (state) {
    case 'Idle':
      return { configure: true, start: false, stop: false, reset: false };
    case 'Configured':
      // Start is enabled - backend will auto-arm
      return { configure: false, start: true, stop: false, reset: true };
    case 'Armed':
      return { configure: false, start: true, stop: false, reset: true };
    case 'Running':
      return { configure: false, start: false, stop: true, reset: false };
    case 'Error':
      return { configure: false, start: false, stop: false, reset: true };
    default:
      // Transitional states - all disabled
      return { configure: false, start: false, stop: false, reset: false };
  }
}
