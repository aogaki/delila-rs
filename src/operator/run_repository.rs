//! Run Repository - MongoDB storage for run history
//!
//! Stores run information, statistics, config snapshots, and error logs.

use chrono::Utc;
use mongodb::{
    bson::{doc, oid::ObjectId},
    options::ClientOptions,
    Client, Collection,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info};
use utoipa::ToSchema;

/// Run status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Running,
    Completed,
    Error,
    Aborted,
}

/// Run statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct RunStats {
    pub total_events: i64,
    pub total_bytes: i64,
    pub average_rate: f64,
    #[serde(default)]
    pub trigger_loss_count: i64,
}

/// Error log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorLogEntry {
    /// UNIX timestamp in milliseconds
    pub time: i64,
    pub component: String,
    pub message: String,
}

/// Run note entry (append-only logbook style)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RunNote {
    /// UNIX timestamp in milliseconds
    pub time: i64,
    pub text: String,
}

/// Last run info for pre-filling comment field
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LastRunInfo {
    pub run_number: i32,
    pub comment: String,
    pub notes: Vec<RunNote>,
}

/// Run document stored in MongoDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDocument {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub run_number: i32,
    pub exp_name: String,
    #[serde(default)]
    pub comment: String,
    /// UNIX timestamp in milliseconds
    pub start_time: i64,
    /// UNIX timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<i32>,
    pub status: RunStatus,
    #[serde(default)]
    pub stats: RunStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<ErrorLogEntry>,
    /// Append-only notes (logbook style)
    #[serde(default)]
    pub notes: Vec<RunNote>,
    /// Why the run ended, when it ended abnormally (TODO 68 drain-first stop:
    /// backlog autostop reason and/or drain residual). None for a plain stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// Current run info (in-memory, for API responses)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CurrentRunInfo {
    pub run_number: i32,
    pub exp_name: String,
    pub comment: String,
    /// UNIX timestamp in milliseconds
    pub start_time: i64,
    pub elapsed_secs: i64,
    pub status: RunStatus,
    pub stats: RunStats,
    /// Append-only notes (logbook style)
    #[serde(default)]
    pub notes: Vec<RunNote>,
}

impl CurrentRunInfo {
    /// Create from a running RunDocument
    pub fn from_document(doc: &RunDocument) -> Self {
        let now_ms = Utc::now().timestamp_millis();
        let elapsed = (now_ms - doc.start_time) / 1000;
        Self {
            run_number: doc.run_number,
            exp_name: doc.exp_name.clone(),
            comment: doc.comment.clone(),
            start_time: doc.start_time,
            elapsed_secs: elapsed,
            status: doc.status,
            stats: doc.stats.clone(),
            notes: doc.notes.clone(),
        }
    }
}

/// Repository errors
#[derive(Error, Debug)]
pub enum RepositoryError {
    #[error("MongoDB connection error: {0}")]
    Connection(#[from] mongodb::error::Error),

    #[error("Run not found: {0}")]
    NotFound(i32),

    #[error("Run already exists: {0}")]
    AlreadyExists(i32),
}

/// MongoDB repository for run history
#[derive(Clone)]
pub struct RunRepository {
    collection: Collection<RunDocument>,
}

impl RunRepository {
    /// Create a new repository using an existing MongoDB client
    pub fn new(client: &Client, database: &str) -> Self {
        let db = client.database(database);
        let collection = db.collection::<RunDocument>("runs");
        Self { collection }
    }

    /// Connect to MongoDB and return a repository instance
    pub async fn connect(uri: &str, database: &str) -> Result<Self, RepositoryError> {
        let options = ClientOptions::parse(uri).await?;
        let client = Client::with_options(options)?;

        // Test connection
        client
            .database("admin")
            .run_command(doc! { "ping": 1 })
            .await?;

        info!(uri = uri, database = database, "Connected to MongoDB");

        let db = client.database(database);
        let collection = db.collection::<RunDocument>("runs");

        Ok(Self { collection })
    }

    /// Start a new run
    /// Note: Same run_number can be reused (e.g., for retakes). Each start creates a new document.
    pub async fn start_run(
        &self,
        run_number: i32,
        exp_name: &str,
        comment: &str,
        config_snapshot: Option<serde_json::Value>,
    ) -> Result<RunDocument, RepositoryError> {
        // No duplicate check - same run_number can be reused for retakes
        // Each start creates a new document with unique start_time

        let doc = RunDocument {
            id: None,
            run_number,
            exp_name: exp_name.to_string(),
            comment: comment.to_string(),
            start_time: Utc::now().timestamp_millis(),
            end_time: None,
            duration_secs: None,
            status: RunStatus::Running,
            stats: RunStats::default(),
            config_snapshot,
            errors: Vec::new(),
            notes: Vec::new(),
            stop_reason: None,
        };

        self.collection.insert_one(&doc).await?;

        info!(run_number = run_number, exp_name = exp_name, "Run started");

        Ok(doc)
    }

    /// End a run (completed, error, or aborted)
    pub async fn end_run(
        &self,
        run_number: i32,
        exp_name: &str,
        status: RunStatus,
        stats: RunStats,
        stop_reason: Option<String>,
    ) -> Result<(), RepositoryError> {
        let now_ms = Utc::now().timestamp_millis();

        // Get start time to calculate duration. Filter on status:"running" so a
        // re-used run number (retake) can never clobber an already-completed
        // run's document (TODO 58 H11) — without it, find_one may match the OLD
        // completed doc and overwrite its end_time/duration, while the new doc
        // stays "running" forever.
        let run_doc = self
            .collection
            .find_one(doc! { "run_number": run_number, "exp_name": exp_name, "status": "running" })
            .await?
            .ok_or(RepositoryError::NotFound(run_number))?;

        let duration = ((now_ms - run_doc.start_time) / 1000) as i32;

        let mut set_doc = doc! {
            "end_time": now_ms,
            "duration_secs": duration,
            "status": mongodb::bson::to_bson(&status).expect("RunStatus serializes to BSON"),
            "stats": mongodb::bson::to_bson(&stats).expect("RunStats serializes to BSON"),
        };
        if let Some(reason) = stop_reason {
            set_doc.insert("stop_reason", reason);
        }
        self.collection
            .update_one(
                doc! { "run_number": run_number, "exp_name": exp_name, "status": "running" },
                doc! { "$set": set_doc },
            )
            .await?;

        info!(
            run_number = run_number,
            exp_name = exp_name,
            status = ?status,
            duration_secs = duration,
            "Run ended"
        );

        Ok(())
    }

    /// Update run statistics (while running)
    pub async fn update_stats(
        &self,
        run_number: i32,
        stats: &RunStats,
    ) -> Result<(), RepositoryError> {
        self.collection
            .update_one(
                doc! { "run_number": run_number, "status": "running" },
                doc! {
                    "$set": {
                        "stats": mongodb::bson::to_bson(stats).expect("RunStats serializes to BSON"),
                    }
                },
            )
            .await?;

        Ok(())
    }

    /// Add an error log entry
    pub async fn add_error(
        &self,
        run_number: i32,
        component: &str,
        message: &str,
    ) -> Result<(), RepositoryError> {
        let entry = ErrorLogEntry {
            time: Utc::now().timestamp_millis(),
            component: component.to_string(),
            message: message.to_string(),
        };

        // status:"running" keeps errors attached to the ACTIVE run — a re-used
        // run number must not append errors to an old completed doc (TODO 58 H11).
        self.collection
            .update_one(
                doc! { "run_number": run_number, "status": "running" },
                doc! {
                    "$push": {
                        "errors": mongodb::bson::to_bson(&entry).expect("ErrorLogEntry serializes to BSON"),
                    }
                },
            )
            .await?;

        error!(
            run_number = run_number,
            component = component,
            message = message,
            "Error logged"
        );

        Ok(())
    }

    /// Add a note to the current run (append-only)
    pub async fn add_note(&self, run_number: i32, text: &str) -> Result<RunNote, RepositoryError> {
        let time_ms = Utc::now().timestamp_millis();

        let result = self
            .collection
            .update_one(
                doc! { "run_number": run_number, "status": "running" },
                doc! {
                    "$push": {
                        "notes": {
                            "time": time_ms,
                            "text": text,
                        }
                    }
                },
            )
            .await?;

        if result.matched_count == 0 {
            return Err(RepositoryError::NotFound(run_number));
        }

        info!(run_number = run_number, "Note added");

        Ok(RunNote {
            time: time_ms,
            text: text.to_string(),
        })
    }

    /// Get current running run (if any)
    pub async fn get_current_run(&self) -> Result<Option<RunDocument>, RepositoryError> {
        let doc = self
            .collection
            .find_one(doc! { "status": "running" })
            .await?;

        Ok(doc)
    }

    /// Get run by number
    pub async fn get_run(&self, run_number: i32) -> Result<Option<RunDocument>, RepositoryError> {
        let doc = self
            .collection
            .find_one(doc! { "run_number": run_number })
            .await?;

        Ok(doc)
    }

    /// Get recent runs (newest first)
    pub async fn get_recent_runs(&self, limit: i64) -> Result<Vec<RunDocument>, RepositoryError> {
        use futures::TryStreamExt;

        let cursor = self
            .collection
            .find(doc! {})
            .sort(doc! { "start_time": -1 })
            .limit(limit)
            .await?;

        let runs: Vec<RunDocument> = cursor.try_collect().await?;

        Ok(runs)
    }

    /// Get runs by experiment name
    pub async fn get_runs_by_experiment(
        &self,
        exp_name: &str,
        limit: i64,
    ) -> Result<Vec<RunDocument>, RepositoryError> {
        use futures::TryStreamExt;

        let cursor = self
            .collection
            .find(doc! { "exp_name": exp_name })
            .sort(doc! { "start_time": -1 })
            .limit(limit)
            .await?;

        let runs: Vec<RunDocument> = cursor.try_collect().await?;

        Ok(runs)
    }

    /// Get next run number (max + 1)
    pub async fn get_next_run_number(&self) -> Result<i32, RepositoryError> {
        use mongodb::options::FindOneOptions;

        let options = FindOneOptions::builder()
            .sort(doc! { "run_number": -1 })
            .projection(doc! { "run_number": 1 })
            .build();

        let doc = self
            .collection
            .find_one(doc! {})
            .with_options(options)
            .await?;

        Ok(doc.map(|d| d.run_number + 1).unwrap_or(1))
    }

    /// Get next run number for a specific experiment
    /// Returns the run_number + 1 of the most recent run (by start_time), not the max run_number.
    /// This allows re-running the same run number (e.g., for retakes).
    pub async fn get_next_run_number_for_experiment(
        &self,
        exp_name: &str,
    ) -> Result<i32, RepositoryError> {
        use mongodb::bson::Document;
        use mongodb::options::FindOneOptions;

        // Use raw Document to avoid deserialization issues with projection
        let collection = self.collection.clone_with_type::<Document>();

        // Sort by start_time descending to get the most recent run
        let options = FindOneOptions::builder()
            .sort(doc! { "start_time": -1 })
            .projection(doc! { "run_number": 1 })
            .build();

        let doc = collection
            .find_one(doc! { "exp_name": exp_name })
            .with_options(options)
            .await?;

        Ok(doc
            .and_then(|d| d.get_i32("run_number").ok())
            .map(|n| n + 1)
            .unwrap_or(1))
    }

    /// Get the most recent run info for a specific experiment (for pre-filling comment)
    /// Returns the comment and notes from the last run.
    pub async fn get_last_run_info_for_experiment(
        &self,
        exp_name: &str,
    ) -> Result<Option<LastRunInfo>, RepositoryError> {
        use mongodb::options::FindOneOptions;

        // Use projection to only fetch needed fields, avoiding DateTime deserialization issues
        let options = FindOneOptions::builder()
            .sort(doc! { "start_time": -1 })
            .projection(doc! {
                "run_number": 1,
                "comment": 1,
                "notes": 1,
            })
            .build();

        // Use raw BSON document to avoid type issues
        let raw_collection = self.collection.clone_with_type::<mongodb::bson::Document>();

        let doc = raw_collection
            .find_one(doc! { "exp_name": exp_name })
            .with_options(options)
            .await?;

        Ok(doc.map(|d| {
            let run_number = d.get_i32("run_number").unwrap_or(0);
            let comment = d.get_str("comment").unwrap_or("").to_string();
            let notes: Vec<RunNote> = d
                .get_array("notes")
                .ok()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            let doc = v.as_document()?;
                            Some(RunNote {
                                time: doc.get_i64("time").ok()?,
                                text: doc.get_str("text").ok()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            LastRunInfo {
                run_number,
                comment,
                notes,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_serialization() {
        let status = RunStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let status = RunStatus::Completed;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"completed\"");
    }

    #[test]
    fn test_run_stats_default() {
        let stats = RunStats::default();
        assert_eq!(stats.total_events, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.average_rate, 0.0);
    }

    #[test]
    fn test_current_run_info_elapsed() {
        let start_ms = Utc::now().timestamp_millis() - 60_000;
        let doc = RunDocument {
            id: None,
            run_number: 1,
            exp_name: "test".to_string(),
            comment: String::new(),
            start_time: start_ms,
            end_time: None,
            duration_secs: None,
            status: RunStatus::Running,
            stats: RunStats::default(),
            config_snapshot: None,
            errors: Vec::new(),
            notes: Vec::new(),
            stop_reason: None,
        };

        let info = CurrentRunInfo::from_document(&doc);
        // Allow 1 second tolerance
        assert!(info.elapsed_secs >= 59 && info.elapsed_secs <= 61);
    }
}
