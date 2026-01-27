//! Digitizer Config Repository - MongoDB storage for digitizer configurations
//!
//! Stores digitizer configurations with version history and run snapshots.
//! Supports both current working configs and historical snapshots.

use chrono::{DateTime, Utc};
use mongodb::{
    bson::{doc, oid::ObjectId},
    Client, Collection,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::config::DigitizerConfig;

/// Digitizer configuration document stored in MongoDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigitizerConfigDocument {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    /// Digitizer identifier (matches digitizer_id in config)
    pub digitizer_id: u32,

    /// Version number (incremented on each save)
    pub version: u32,

    /// When this config was created/updated
    pub created_at: DateTime<Utc>,

    /// Who/what created this config (e.g., "api", "import", "snapshot")
    pub created_by: String,

    /// Optional description of changes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether this is the current active config (false for historical versions)
    pub is_current: bool,

    /// Hardware serial number (top-level for MongoDB indexing)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,

    /// The actual configuration data
    pub config: DigitizerConfig,
}

/// Run snapshot document - captures all digitizer configs at run start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfigSnapshot {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,

    /// Run number this snapshot belongs to
    pub run_number: i32,

    /// Experiment name
    pub exp_name: String,

    /// When the run started
    pub run_start_time: DateTime<Utc>,

    /// All digitizer configurations at run start
    pub digitizer_configs: Vec<DigitizerConfig>,
}

/// Repository errors
#[derive(Error, Debug)]
pub enum DigitizerRepoError {
    #[error("MongoDB error: {0}")]
    Mongo(#[from] mongodb::error::Error),

    #[error("Digitizer not found: {0}")]
    NotFound(u32),

    #[error("Configuration error: {0}")]
    Config(String),
}

/// MongoDB repository for digitizer configurations
#[derive(Clone)]
pub struct DigitizerConfigRepository {
    configs: Collection<DigitizerConfigDocument>,
    snapshots: Collection<RunConfigSnapshot>,
}

impl DigitizerConfigRepository {
    /// Create a new repository using an existing MongoDB client
    pub fn new(client: &Client, database: &str) -> Self {
        let db = client.database(database);
        Self {
            configs: db.collection::<DigitizerConfigDocument>("digitizer_configs"),
            snapshots: db.collection::<RunConfigSnapshot>("run_config_snapshots"),
        }
    }

    /// Save a digitizer configuration (creates new version)
    ///
    /// This marks any existing current config as non-current and inserts a new one.
    pub async fn save_config(
        &self,
        config: DigitizerConfig,
        created_by: &str,
        description: Option<String>,
    ) -> Result<DigitizerConfigDocument, DigitizerRepoError> {
        let digitizer_id = config.digitizer_id;

        // Get current version number
        let current = self.get_current_config(digitizer_id).await?;
        let next_version = current.map(|c| c.version + 1).unwrap_or(1);

        // Mark existing current as non-current
        self.configs
            .update_many(
                doc! { "digitizer_id": digitizer_id, "is_current": true },
                doc! { "$set": { "is_current": false } },
            )
            .await?;

        // Insert new config as current
        let serial_number = config.serial_number.clone();
        let doc = DigitizerConfigDocument {
            id: None,
            digitizer_id,
            version: next_version,
            created_at: Utc::now(),
            created_by: created_by.to_string(),
            description,
            is_current: true,
            serial_number,
            config,
        };

        self.configs.insert_one(&doc).await?;

        info!(
            digitizer_id = digitizer_id,
            version = next_version,
            "Saved digitizer config"
        );

        Ok(doc)
    }

    /// Get the current (active) configuration for a digitizer
    pub async fn get_current_config(
        &self,
        digitizer_id: u32,
    ) -> Result<Option<DigitizerConfigDocument>, DigitizerRepoError> {
        let doc = self
            .configs
            .find_one(doc! { "digitizer_id": digitizer_id, "is_current": true })
            .await?;
        Ok(doc)
    }

    /// Get the current configuration for a digitizer by its hardware serial number
    ///
    /// Used by the Detect flow to restore settings for a previously-seen digitizer.
    pub async fn get_config_by_serial(
        &self,
        serial_number: &str,
    ) -> Result<Option<DigitizerConfigDocument>, DigitizerRepoError> {
        let doc = self
            .configs
            .find_one(doc! { "serial_number": serial_number, "is_current": true })
            .await?;
        Ok(doc)
    }

    /// Get a specific version of a digitizer configuration
    pub async fn get_config_version(
        &self,
        digitizer_id: u32,
        version: u32,
    ) -> Result<Option<DigitizerConfigDocument>, DigitizerRepoError> {
        let doc = self
            .configs
            .find_one(doc! { "digitizer_id": digitizer_id, "version": version })
            .await?;
        Ok(doc)
    }

    /// List all current digitizer configurations
    pub async fn list_current_configs(
        &self,
    ) -> Result<Vec<DigitizerConfigDocument>, DigitizerRepoError> {
        use futures::TryStreamExt;

        let cursor = self
            .configs
            .find(doc! { "is_current": true })
            .sort(doc! { "digitizer_id": 1 })
            .await?;

        let configs: Vec<DigitizerConfigDocument> = cursor.try_collect().await?;
        Ok(configs)
    }

    /// Get version history for a digitizer (newest first)
    pub async fn get_config_history(
        &self,
        digitizer_id: u32,
        limit: i64,
    ) -> Result<Vec<DigitizerConfigDocument>, DigitizerRepoError> {
        use futures::TryStreamExt;

        let cursor = self
            .configs
            .find(doc! { "digitizer_id": digitizer_id })
            .sort(doc! { "version": -1 })
            .limit(limit)
            .await?;

        let configs: Vec<DigitizerConfigDocument> = cursor.try_collect().await?;
        Ok(configs)
    }

    /// Restore a specific version as the current config
    pub async fn restore_version(
        &self,
        digitizer_id: u32,
        version: u32,
    ) -> Result<DigitizerConfigDocument, DigitizerRepoError> {
        let old_config = self
            .get_config_version(digitizer_id, version)
            .await?
            .ok_or(DigitizerRepoError::NotFound(digitizer_id))?;

        // Save as new version with description
        self.save_config(
            old_config.config,
            "restore",
            Some(format!("Restored from version {}", version)),
        )
        .await
    }

    /// Create a run config snapshot
    ///
    /// Captures all current digitizer configurations at run start.
    pub async fn create_run_snapshot(
        &self,
        run_number: i32,
        exp_name: &str,
        configs: Vec<DigitizerConfig>,
    ) -> Result<RunConfigSnapshot, DigitizerRepoError> {
        let snapshot = RunConfigSnapshot {
            id: None,
            run_number,
            exp_name: exp_name.to_string(),
            run_start_time: Utc::now(),
            digitizer_configs: configs,
        };

        self.snapshots.insert_one(&snapshot).await?;

        info!(
            run_number = run_number,
            exp_name = exp_name,
            num_configs = snapshot.digitizer_configs.len(),
            "Created run config snapshot"
        );

        Ok(snapshot)
    }

    /// Get the config snapshot for a specific run
    pub async fn get_run_snapshot(
        &self,
        run_number: i32,
        exp_name: &str,
    ) -> Result<Option<RunConfigSnapshot>, DigitizerRepoError> {
        let snapshot = self
            .snapshots
            .find_one(doc! { "run_number": run_number, "exp_name": exp_name })
            .await?;
        Ok(snapshot)
    }

    /// List all run snapshots for an experiment (newest first)
    pub async fn list_run_snapshots(
        &self,
        exp_name: &str,
        limit: i64,
    ) -> Result<Vec<RunConfigSnapshot>, DigitizerRepoError> {
        use futures::TryStreamExt;

        let cursor = self
            .snapshots
            .find(doc! { "exp_name": exp_name })
            .sort(doc! { "run_start_time": -1 })
            .limit(limit)
            .await?;

        let snapshots: Vec<RunConfigSnapshot> = cursor.try_collect().await?;
        Ok(snapshots)
    }

    /// Delete old config versions (keep only the last N versions per digitizer)
    pub async fn cleanup_old_versions(&self, keep_versions: u32) -> Result<u64, DigitizerRepoError> {
        // Get all digitizer IDs
        let current_configs = self.list_current_configs().await?;
        let mut total_deleted = 0u64;

        for config in current_configs {
            let digitizer_id = config.digitizer_id;
            let current_version = config.version;

            // Delete versions older than (current - keep_versions)
            if current_version > keep_versions {
                let delete_below = current_version - keep_versions;
                let result = self
                    .configs
                    .delete_many(doc! {
                        "digitizer_id": digitizer_id,
                        "version": { "$lt": delete_below }
                    })
                    .await?;
                total_deleted += result.deleted_count;
            }
        }

        if total_deleted > 0 {
            info!(deleted = total_deleted, "Cleaned up old config versions");
        }

        Ok(total_deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BoardConfig, ChannelConfig, FirmwareType};

    fn create_test_config(id: u32) -> DigitizerConfig {
        DigitizerConfig {
            digitizer_id: id,
            name: format!("Test Digitizer {}", id),
            firmware: FirmwareType::PSD2,
            serial_number: None,
            model: None,
            num_channels: 32,
            is_master: false,
            sync: None,
            board: BoardConfig::default(),
            channel_defaults: ChannelConfig::default(),
            channel_overrides: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn test_config_document_serialization() {
        let config = create_test_config(0);
        let doc = DigitizerConfigDocument {
            id: None,
            digitizer_id: 0,
            version: 1,
            created_at: Utc::now(),
            created_by: "test".to_string(),
            description: Some("Test config".to_string()),
            is_current: true,
            serial_number: None,
            config,
        };

        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"digitizer_id\":0"));
        assert!(json.contains("\"version\":1"));
        assert!(json.contains("\"is_current\":true"));
    }

    #[test]
    fn test_run_snapshot_serialization() {
        let configs = vec![create_test_config(0), create_test_config(1)];
        let snapshot = RunConfigSnapshot {
            id: None,
            run_number: 42,
            exp_name: "TestExp".to_string(),
            run_start_time: Utc::now(),
            digitizer_configs: configs,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"run_number\":42"));
        assert!(json.contains("\"exp_name\":\"TestExp\""));
        assert!(json.contains("digitizer_configs"));
    }
}
