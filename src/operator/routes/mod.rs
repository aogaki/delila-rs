//! REST API routes for DAQ control

mod digitizer;
mod emulator;
mod event_builder;
mod monitor_layout;
mod run;
pub(crate) mod status;
mod tuneup;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    routing::{get, post, put},
    Router,
};
use dashmap::DashMap;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::common::{ComponentMetrics, ComponentState};
use crate::config::{DigitizerConfig, Settings as ConfigSettings};

use super::{
    ApiResponse, CommandResult, ComponentClient, ComponentConfig, ComponentStatus,
    ConfigureRequest, CurrentRunInfo, DigitizerConfigRepository, EventBuilderRepository,
    LastRunInfo, OperatorConfig, RunNote, RunRepository, RunStats, RunStatus, StartRequest,
    SystemState, SystemStatus,
};

// Re-export public types from sub-modules (used in OpenAPI schemas)
pub use digitizer::{
    DetectResponse, DetectedDigitizer, DigitizerConfigHistoryItem, RestoreVersionRequest,
};
pub use event_builder::{
    ConfigHistoryItem as EventBuilderHistoryItem, CreateConfigRequest, UpdateChSettingsRequest,
    UpdateL2SettingsRequest, UpdateTimeSettingsRequest,
};
pub use run::{AddNoteRequest, NextRunNumberResponse};

// Import handler functions from sub-modules (used in router and ApiDoc)
use digitizer::{
    apply_digitizer_config, detect_digitizers, get_digitizer, get_digitizer_by_serial,
    get_digitizer_history, list_digitizers, read_amax_board_registers, restore_digitizer_version,
    save_all_digitizers, save_digitizer, save_digitizer_to_mongodb, update_digitizer,
};
use emulator::{get_emulator_settings, update_emulator_settings};
use event_builder::{
    create_config as eb_create_config, delete_config as eb_delete_config,
    get_config as eb_get_config, get_config_history as eb_get_history,
    list_configs as eb_list_configs, list_experiments as eb_list_experiments,
    restore_version as eb_restore_version, update_ch_settings as eb_update_ch_settings,
    update_l2_settings as eb_update_l2_settings, update_time_settings as eb_update_time_settings,
};
use monitor_layout::{get_monitor_layout, save_monitor_layout};
use run::{
    add_run_note, export_runs, get_next_run_number, get_run, get_run_config_snapshot,
    get_run_history,
};
use status::{arm, configure, get_status, reset, run_start, start, stop, stop_drain};
use tuneup::{tuneup_apply, tuneup_start, tuneup_stop};

/// Tune Up runtime state, packing the previous `tuneup_mode: RwLock<bool>` +
/// `tuneup_digitizer_id: RwLock<Option<u32>>` pair into a single `AtomicU64`.
///
/// **Why one atomic instead of two locks**: the two fields were always written
/// together (`enter` sets `(true, Some(id))`, `exit` sets `(false, None)`),
/// but the writes were not atomic — between the two `RwLock::write().await`
/// calls a concurrent reader could observe `tuneup_mode = true,
/// tuneup_digitizer_id = None` (or vice versa). Packing both into one u64
/// removes that race for free, and turns the GET /api/status hot path
/// (polled every ~1s by the frontend) into a lock-free atomic load.
///
/// Encoding: `u64::MAX` = inactive, anything else = `Some(value as u32)`
/// (valid digitizer ids fit in u32, so the discriminant never collides).
pub struct TuneupState(AtomicU64);

const TUNEUP_INACTIVE: u64 = u64::MAX;

impl TuneupState {
    pub const fn new() -> Self {
        Self(AtomicU64::new(TUNEUP_INACTIVE))
    }

    /// Mark Tune Up active for `digitizer_id`. Idempotent re-entry just
    /// rewrites the digitizer id.
    pub fn enter(&self, digitizer_id: u32) {
        self.0.store(digitizer_id as u64, Ordering::Release);
    }

    /// Clear Tune Up state.
    pub fn exit(&self) {
        self.0.store(TUNEUP_INACTIVE, Ordering::Release);
    }

    /// Whether Tune Up mode is currently active.
    pub fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire) != TUNEUP_INACTIVE
    }

    /// Snapshot `(active, digitizer_id)` from a single atomic load — both
    /// fields agree by construction (no torn read across two locks).
    pub fn snapshot(&self) -> (bool, Option<u32>) {
        let v = self.0.load(Ordering::Acquire);
        if v == TUNEUP_INACTIVE {
            (false, None)
        } else {
            (true, Some(v as u32))
        }
    }
}

impl Default for TuneupState {
    fn default() -> Self {
        Self::new()
    }
}

/// Application state shared across handlers
pub struct AppState {
    pub client: ComponentClient,
    pub components: Vec<ComponentConfig>,
    pub config: OperatorConfig,
    /// Digitizer configurations (keyed by digitizer_id).
    ///
    /// `DashMap` instead of `RwLock<HashMap>` — per-shard locking keeps
    /// individual `get(id)` / `insert(id, c)` operations from blocking
    /// each other across digitizer ids, removes the `.read().await` /
    /// `.write().await` boilerplate from every digitizer route handler,
    /// and is a strict-superset replacement (same `get` / `insert` /
    /// `iter` / `len` API surface).
    pub digitizer_configs: DashMap<u32, DigitizerConfig>,
    /// Directory for storing digitizer config files
    pub config_dir: PathBuf,
    /// Source-specific config file paths (for reloading from disk at run start)
    pub config_files: Vec<(u32, PathBuf)>,
    /// Run repository for MongoDB storage (optional)
    pub run_repo: Option<RunRepository>,
    /// Digitizer config repository for MongoDB (optional)
    pub digitizer_repo: Option<DigitizerConfigRepository>,
    /// Event Builder config repository for MongoDB (optional)
    pub event_builder_repo: Option<EventBuilderRepository>,
    /// Current run info (cached in memory for fast access)
    pub current_run: RwLock<Option<CurrentRunInfo>>,
    /// Emulator settings (runtime-configurable)
    pub emulator_settings: RwLock<EmulatorSettings>,
    /// Tune Up mode state (atomic — see `TuneupState` doc).
    pub tuneup: TuneupState,
    /// Monitor layout state (opaque JSON, persisted to file)
    pub monitor_layout: RwLock<serde_json::Value>,
    /// Serializes tuneup_apply calls to prevent concurrent Stop→Apply→Start races
    pub tuneup_apply_lock: tokio::sync::Mutex<()>,
    /// Serializes run-control handlers (configure/arm/start/stop/reset/run_start).
    /// Every guard in those handlers is check-then-act; without this two
    /// concurrent requests both pass their guards and interleave
    /// Reset/Configure/Arm/Start across components (TODO 58 H14). Multi-client
    /// operation is an explicit feature, so the race is realistic.
    pub run_control_lock: tokio::sync::Mutex<()>,
}

/// Emulator runtime settings (API model)
///
/// These settings can be changed via the API and will be applied
/// when the emulator is next started.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EmulatorSettings {
    /// Events per batch
    pub events_per_batch: u32,
    /// Batch interval in milliseconds (0 = maximum speed)
    pub batch_interval_ms: u64,
    /// Number of simulated modules
    pub num_modules: u32,
    /// Channels per module
    pub channels_per_module: u32,
    /// Enable waveform generation
    pub enable_waveform: bool,
    /// Waveform probe bitmask (1=analog1, 2=analog2, 3=both analog, 63=all)
    pub waveform_probes: u8,
    /// Number of waveform samples
    pub waveform_samples: u32,
}

impl Default for EmulatorSettings {
    fn default() -> Self {
        Self {
            events_per_batch: 5000,
            batch_interval_ms: 0,
            num_modules: 2,
            channels_per_module: 16,
            enable_waveform: false,
            waveform_probes: 3, // Both analog probes
            waveform_samples: 512,
        }
    }
}

impl From<&ConfigSettings> for EmulatorSettings {
    fn from(settings: &ConfigSettings) -> Self {
        Self {
            events_per_batch: settings.events_per_batch,
            batch_interval_ms: settings.batch_interval_ms,
            num_modules: settings.num_modules,
            channels_per_module: settings.channels_per_module,
            enable_waveform: settings.enable_waveform,
            waveform_probes: settings.waveform_probes,
            waveform_samples: settings.waveform_samples as u32,
        }
    }
}

// =============================================================================
// Channel Registration helpers
// =============================================================================

use crate::common::{ChannelRegistration, Command};

impl AppState {
    /// Build channel registrations from loaded digitizer configs.
    /// If `filter_id` is Some, only include channels for that digitizer.
    pub(super) fn build_channel_registrations_from(
        configs: &DashMap<u32, DigitizerConfig>,
        filter_id: Option<u32>,
    ) -> Vec<ChannelRegistration> {
        let mut registrations = Vec::new();
        for entry in configs.iter() {
            let id = *entry.key();
            let config = entry.value();
            if let Some(filter) = filter_id {
                if id != filter {
                    continue;
                }
            }
            for ch in 0..config.num_channels as u32 {
                let name = config
                    .channel_names
                    .as_ref()
                    .and_then(|names| names.get(&ch).cloned())
                    .unwrap_or_else(|| format!("{}/Ch{}", config.name, ch));
                registrations.push(ChannelRegistration {
                    module_id: config.digitizer_id,
                    channel_id: ch,
                    name,
                });
            }
        }
        registrations
    }

    /// Reload digitizer configs from disk (JSON files).
    /// Called at run_start to ensure Phase 1.5 uses fresh configs.
    pub(super) fn reload_digitizer_configs(&self) {
        let fresh = if self.config_files.is_empty() {
            load_digitizer_configs_from_dir(&self.config_dir).unwrap_or_default()
        } else {
            load_digitizer_configs_from_files(&self.config_files)
        };
        let count = fresh.len();
        // DashMap doesn't expose a single-shot replace; clear+insert is the
        // semantic equivalent. Existing readers see a brief empty window
        // (~µs), the same race the previous `*write().await = fresh` had.
        self.digitizer_configs.clear();
        for (k, v) in fresh {
            self.digitizer_configs.insert(k, v);
        }
        tracing::info!(count, "Reloaded digitizer configs from disk");
    }

    /// Find the Monitor component address from the component list.
    pub(super) fn find_monitor_address(&self) -> Option<String> {
        self.components
            .iter()
            .find(|c| !c.is_digitizer && c.source_id.is_none() && c.name.contains("Monitor"))
            .map(|c| c.address.clone())
    }

    /// Send RegisterChannels command to Monitor (best-effort, logs on failure).
    pub(super) async fn send_register_channels(&self, registrations: Vec<ChannelRegistration>) {
        if registrations.is_empty() {
            return;
        }
        if let Some(monitor_addr) = self.find_monitor_address() {
            let cmd = Command::RegisterChannels(registrations);
            match self.client.send_command(&monitor_addr, &cmd).await {
                Ok(resp) if resp.success => {
                    tracing::info!("RegisterChannels sent to Monitor: {}", resp.message);
                }
                Ok(resp) => {
                    tracing::warn!("Monitor rejected RegisterChannels: {}", resp.message);
                }
                Err(e) => {
                    tracing::warn!("Failed to send RegisterChannels to Monitor: {}", e);
                }
            }
        } else {
            tracing::warn!("No Monitor component found for RegisterChannels");
        }
    }
}

/// OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    paths(
        status::get_status,
        status::configure,
        status::arm,
        status::start,
        status::stop,
        status::stop_drain,
        status::reset,
        status::run_start,
        digitizer::list_digitizers,
        digitizer::detect_digitizers,
        digitizer::get_digitizer_by_serial,
        digitizer::get_digitizer,
        digitizer::update_digitizer,
        digitizer::apply_digitizer_config,
        digitizer::save_digitizer,
        digitizer::save_all_digitizers,
        digitizer::save_digitizer_to_mongodb,
        digitizer::get_digitizer_history,
        digitizer::restore_digitizer_version,
        run::get_run_config_snapshot,
        run::get_run_history,
        run::export_runs,
        run::get_run,
        run::get_next_run_number,
        run::add_run_note,
        emulator::get_emulator_settings,
        emulator::update_emulator_settings,
        event_builder::list_experiments,
        event_builder::list_configs,
        event_builder::get_config,
        event_builder::create_config,
        event_builder::update_ch_settings,
        event_builder::update_time_settings,
        event_builder::update_l2_settings,
        event_builder::get_config_history,
        event_builder::restore_version,
        event_builder::delete_config,
        tuneup::tuneup_start,
        tuneup::tuneup_stop,
        tuneup::tuneup_apply,
    ),
    components(schemas(
        SystemStatus,
        ComponentStatus,
        SystemState,
        ComponentState,
        ComponentMetrics,
        ConfigureRequest,
        StartRequest,
        ApiResponse,
        CommandResult,
        DigitizerConfig,
        DetectedDigitizer,
        DetectResponse,
        CurrentRunInfo,
        RunStats,
        RunStatus,
        NextRunNumberResponse,
        AddNoteRequest,
        RunNote,
        LastRunInfo,
        EmulatorSettings,
        DigitizerConfigHistoryItem,
        RestoreVersionRequest,
        EventBuilderHistoryItem,
        CreateConfigRequest,
        UpdateChSettingsRequest,
        UpdateTimeSettingsRequest,
        UpdateL2SettingsRequest,
        tuneup::TuneUpStartRequest,
    )),
    tags(
        (name = "DAQ Control", description = "DAQ system control endpoints"),
        (name = "Digitizer Config", description = "Digitizer configuration endpoints"),
        (name = "Run History", description = "Run history and statistics"),
        (name = "Emulator Settings", description = "Emulator runtime configuration"),
        (name = "Event Builder", description = "Event Builder configuration (chSettings, timeSettings, L2Settings)")
    ),
    info(
        title = "DELILA DAQ Operator API",
        version = "1.0.0",
        description = "REST API for controlling the DELILA DAQ system"
    )
)]
struct ApiDoc;

/// Builder for creating the Operator API router.
///
/// All fields have sensible defaults; only `components` is required.
///
/// ```ignore
/// let app = RouterBuilder::new(components)
///     .config(operator_config)
///     .emulator_settings(settings)
///     .build();
/// ```
pub struct RouterBuilder {
    components: Vec<ComponentConfig>,
    config: OperatorConfig,
    config_dir: PathBuf,
    /// (source_id, config_file_path) pairs from TOML source configs.
    /// If empty, falls back to loading all JSON files in config_dir.
    config_files: Vec<(u32, PathBuf)>,
    run_repo: Option<RunRepository>,
    digitizer_repo: Option<DigitizerConfigRepository>,
    event_builder_repo: Option<EventBuilderRepository>,
    emulator_settings: EmulatorSettings,
}

impl RouterBuilder {
    pub fn new(components: Vec<ComponentConfig>) -> Self {
        Self {
            components,
            config: OperatorConfig::default(),
            config_dir: PathBuf::from("./config/digitizers"),
            config_files: Vec::new(),
            run_repo: None,
            digitizer_repo: None,
            event_builder_repo: None,
            emulator_settings: EmulatorSettings::default(),
        }
    }

    pub fn config(mut self, config: OperatorConfig) -> Self {
        self.config = config;
        self
    }

    pub fn config_dir(mut self, path: PathBuf) -> Self {
        self.config_dir = path;
        self
    }

    pub fn run_repo(mut self, repo: Option<RunRepository>) -> Self {
        self.run_repo = repo;
        self
    }

    pub fn digitizer_repo(mut self, repo: Option<DigitizerConfigRepository>) -> Self {
        self.digitizer_repo = repo;
        self
    }

    pub fn event_builder_repo(mut self, repo: Option<EventBuilderRepository>) -> Self {
        self.event_builder_repo = repo;
        self
    }

    /// Set specific config files to load (from TOML source config_file fields).
    /// Each entry pairs a TOML source_id with the JSON file path.
    /// If not set, falls back to loading all JSON files in config_dir.
    pub fn config_files(mut self, files: Vec<(u32, PathBuf)>) -> Self {
        self.config_files = files;
        self
    }

    pub fn emulator_settings(mut self, settings: EmulatorSettings) -> Self {
        self.emulator_settings = settings;
        self
    }

    pub fn build(self) -> (Router, Arc<AppState>) {
        let digitizer_configs = if self.config_files.is_empty() {
            load_digitizer_configs_from_dir(&self.config_dir).unwrap_or_default()
        } else {
            load_digitizer_configs_from_files(&self.config_files)
        };

        tracing::info!(
            "Loaded {} digitizer config(s) at startup",
            digitizer_configs.len()
        );
        if digitizer_configs.is_empty() {
            tracing::warn!(
                "No digitizer configs loaded — config snapshots will not be created on run start"
            );
        }

        // Extract web_ui_dir before moving config into AppState
        let web_ui_dir = self.config.web_ui_dir.clone();

        let monitor_layout = monitor_layout::load_monitor_layout(&self.config_dir);

        let state = Arc::new(AppState {
            client: ComponentClient::new(),
            components: self.components,
            config: self.config,
            digitizer_configs: digitizer_configs.into_iter().collect(),
            config_dir: self.config_dir.clone(),
            config_files: self.config_files,
            run_repo: self.run_repo,
            digitizer_repo: self.digitizer_repo,
            event_builder_repo: self.event_builder_repo,
            current_run: RwLock::new(None),
            emulator_settings: RwLock::new(self.emulator_settings),
            tuneup: TuneupState::new(),
            monitor_layout: RwLock::new(monitor_layout),
            tuneup_apply_lock: tokio::sync::Mutex::new(()),
            run_control_lock: tokio::sync::Mutex::new(()),
        });

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let mut router = Router::new()
            // DAQ Control API routes
            .route("/api/status", get(get_status))
            .route("/api/configure", post(configure))
            .route("/api/arm", post(arm))
            .route("/api/start", post(start))
            .route("/api/stop", post(stop))
            .route("/api/stop_drain", post(stop_drain))
            .route("/api/reset", post(reset))
            // Two-phase synchronized run control
            .route("/api/run/start", post(run_start))
            // Tune Up mode
            .route("/api/tuneup/start", post(tuneup_start))
            .route("/api/tuneup/stop", post(tuneup_stop))
            .route("/api/tuneup/apply/:id", post(tuneup_apply))
            // Run history routes
            .route("/api/runs", get(get_run_history))
            .route("/api/runs/export", get(export_runs))
            .route("/api/runs/next", get(get_next_run_number))
            .route("/api/runs/current/note", post(add_run_note))
            .route("/api/runs/:run_number", get(get_run))
            // Digitizer configuration routes
            .route("/api/digitizers", get(list_digitizers))
            .route("/api/digitizers/detect", post(detect_digitizers))
            .route("/api/digitizers/save-all", post(save_all_digitizers))
            .route(
                "/api/digitizers/by-serial/:serial",
                get(get_digitizer_by_serial),
            )
            .route("/api/digitizers/:id", get(get_digitizer))
            .route("/api/digitizers/:id", put(update_digitizer))
            .route("/api/digitizers/:id/apply", post(apply_digitizer_config))
            .route(
                "/api/digitizers/:id/amax-board-registers",
                get(read_amax_board_registers),
            )
            .route("/api/digitizers/:id/save", post(save_digitizer))
            .route(
                "/api/digitizers/:id/save-to-db",
                post(save_digitizer_to_mongodb),
            )
            .route("/api/digitizers/:id/history", get(get_digitizer_history))
            .route(
                "/api/digitizers/:id/restore",
                post(restore_digitizer_version),
            )
            // Run config snapshots
            .route("/api/runs/:run_number/config", get(get_run_config_snapshot))
            // Monitor layout persistence
            .route(
                "/api/monitor/layout",
                get(get_monitor_layout).put(save_monitor_layout),
            )
            // Emulator settings routes
            .route("/api/emulator", get(get_emulator_settings))
            .route("/api/emulator", put(update_emulator_settings))
            // Event Builder configuration routes
            .route("/api/event-builder/experiments", get(eb_list_experiments))
            .route("/api/event-builder/configs", get(eb_list_configs))
            .route("/api/event-builder/configs", post(eb_create_config))
            .route(
                "/api/event-builder/configs/:exp_name/:name",
                get(eb_get_config),
            )
            .route(
                "/api/event-builder/configs/:exp_name/:name",
                axum::routing::delete(eb_delete_config),
            )
            .route(
                "/api/event-builder/configs/:exp_name/:name/ch-settings",
                put(eb_update_ch_settings),
            )
            .route(
                "/api/event-builder/configs/:exp_name/:name/time-settings",
                put(eb_update_time_settings),
            )
            .route(
                "/api/event-builder/configs/:exp_name/:name/l2-settings",
                put(eb_update_l2_settings),
            )
            .route(
                "/api/event-builder/configs/:exp_name/:name/history",
                get(eb_get_history),
            )
            .route(
                "/api/event-builder/configs/:exp_name/:name/restore",
                post(eb_restore_version),
            )
            // Swagger UI
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
            .layer(cors)
            .with_state(state.clone());

        // Serve Angular SPA from web_ui_dir (if configured)
        if let Some(ref ui_dir) = web_ui_dir {
            if ui_dir.exists() {
                let index = ui_dir.join("index.html");
                let spa_service = ServeDir::new(ui_dir).fallback(ServeFile::new(index));
                router = router.fallback_service(spa_service);
                tracing::info!("Serving Web UI from {}", ui_dir.display());
            } else {
                tracing::warn!(
                    "web_ui_dir {} does not exist, Web UI disabled",
                    ui_dir.display()
                );
            }
        }

        (router, state)
    }
}

/// Load digitizer configurations from specific JSON files (referenced by TOML sources).
///
/// Each entry pairs a TOML source_id with the JSON file path.
/// The TOML source_id is used as the HashMap key (single source of truth).
/// If the JSON's digitizer_id differs, it is overridden to match the TOML id.
fn load_digitizer_configs_from_files(files: &[(u32, PathBuf)]) -> HashMap<u32, DigitizerConfig> {
    let mut configs = HashMap::new();

    for (source_id, path) in files {
        if let Ok(content) = std::fs::read_to_string(path) {
            match serde_json::from_str::<DigitizerConfig>(&content) {
                Ok(mut config) => {
                    if config.digitizer_id != *source_id {
                        tracing::warn!(
                            "Overriding digitizer_id {} -> {} for '{}' (from {})",
                            config.digitizer_id,
                            source_id,
                            config.name,
                            path.display()
                        );
                        config.digitizer_id = *source_id;
                    }
                    tracing::info!(
                        "Loaded digitizer config '{}' (id={}) from {}",
                        config.name,
                        source_id,
                        path.display()
                    );
                    configs.insert(*source_id, config);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", path.display(), e);
                }
            }
        } else {
            tracing::warn!("Failed to read config file: {}", path.display());
        }
    }

    configs
}

/// Load all digitizer configurations from JSON files in the config directory (fallback)
fn load_digitizer_configs_from_dir(
    config_dir: &PathBuf,
) -> std::io::Result<HashMap<u32, DigitizerConfig>> {
    let mut configs = HashMap::new();

    if !config_dir.exists() {
        return Ok(configs);
    }

    for entry in std::fs::read_dir(config_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<DigitizerConfig>(&content) {
                    configs.insert(config.digitizer_id, config);
                }
            }
        }
    }

    Ok(configs)
}

#[cfg(test)]
mod tuneup_state_tests {
    use super::TuneupState;

    #[test]
    fn new_is_inactive() {
        let s = TuneupState::new();
        assert!(!s.is_active());
        assert_eq!(s.snapshot(), (false, None));
    }

    #[test]
    fn enter_then_snapshot_returns_active_with_id() {
        let s = TuneupState::new();
        s.enter(7);
        assert!(s.is_active());
        assert_eq!(s.snapshot(), (true, Some(7)));
    }

    #[test]
    fn exit_returns_to_inactive() {
        let s = TuneupState::new();
        s.enter(3);
        s.exit();
        assert!(!s.is_active());
        assert_eq!(s.snapshot(), (false, None));
    }

    #[test]
    fn enter_replaces_previous_id() {
        let s = TuneupState::new();
        s.enter(1);
        s.enter(42);
        assert_eq!(s.snapshot(), (true, Some(42)));
    }

    #[test]
    fn high_digitizer_ids_round_trip() {
        // u32::MAX is encoded as a non-sentinel value (the sentinel is u64::MAX).
        let s = TuneupState::new();
        s.enter(u32::MAX);
        assert_eq!(s.snapshot(), (true, Some(u32::MAX)));
    }
}
