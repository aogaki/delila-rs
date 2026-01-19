//! Run Repository - MongoDB storage for run history
//!
//! Stores run information, statistics, config snapshots, and error logs.

use chrono::{DateTime, Utc};
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
}

/// Error log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorLogEntry {
    pub time: DateTime<Utc>,
    pub component: String,
    pub message: String,
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
    pub start_time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<i32>,
    pub status: RunStatus,
    #[serde(default)]
    pub stats: RunStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<ErrorLogEntry>,
}

/// Current run info (in-memory, for API responses)
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CurrentRunInfo {
    pub run_number: i32,
    pub exp_name: String,
    pub comment: String,
    #[schema(value_type = String, format = "date-time")]
    pub start_time: DateTime<Utc>,
    pub elapsed_secs: i64,
    pub status: RunStatus,
    pub stats: RunStats,
}

impl CurrentRunInfo {
    /// Create from a running RunDocument
    pub fn from_document(doc: &RunDocument) -> Self {
        let elapsed = Utc::now()
            .signed_duration_since(doc.start_time)
            .num_seconds();
        Self {
            run_number: doc.run_number,
            exp_name: doc.exp_name.clone(),
            comment: doc.comment.clone(),
            start_time: doc.start_time,
            elapsed_secs: elapsed,
            status: doc.status,
            stats: doc.stats.clone(),
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
    pub async fn start_run(
        &self,
        run_number: i32,
        exp_name: &str,
        comment: &str,
        config_snapshot: Option<serde_json::Value>,
    ) -> Result<RunDocument, RepositoryError> {
        // Check if run already exists
        if let Some(_existing) = self
            .collection
            .find_one(doc! { "run_number": run_number })
            .await?
        {
            return Err(RepositoryError::AlreadyExists(run_number));
        }

        let doc = RunDocument {
            id: None,
            run_number,
            exp_name: exp_name.to_string(),
            comment: comment.to_string(),
            start_time: Utc::now(),
            end_time: None,
            duration_secs: None,
            status: RunStatus::Running,
            stats: RunStats::default(),
            config_snapshot,
            errors: Vec::new(),
        };

        self.collection.insert_one(&doc).await?;

        info!(run_number = run_number, exp_name = exp_name, "Run started");

        Ok(doc)
    }

    /// End a run (completed, error, or aborted)
    pub async fn end_run(
        &self,
        run_number: i32,
        status: RunStatus,
        stats: RunStats,
    ) -> Result<(), RepositoryError> {
        let now = Utc::now();

        // Get start time to calculate duration
        let doc = self
            .collection
            .find_one(doc! { "run_number": run_number })
            .await?
            .ok_or(RepositoryError::NotFound(run_number))?;

        let duration = now.signed_duration_since(doc.start_time).num_seconds() as i32;

        self.collection
            .update_one(
                doc! { "run_number": run_number },
                doc! {
                    "$set": {
                        "end_time": mongodb::bson::DateTime::from_millis(now.timestamp_millis()),
                        "duration_secs": duration,
                        "status": mongodb::bson::to_bson(&status).unwrap(),
                        "stats": mongodb::bson::to_bson(&stats).unwrap(),
                    }
                },
            )
            .await?;

        info!(
            run_number = run_number,
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
                        "stats": mongodb::bson::to_bson(stats).unwrap(),
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
            time: Utc::now(),
            component: component.to_string(),
            message: message.to_string(),
        };

        self.collection
            .update_one(
                doc! { "run_number": run_number },
                doc! {
                    "$push": {
                        "errors": mongodb::bson::to_bson(&entry).unwrap(),
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
    ) -> Result<Vec<RunDocument>, RepositoryError> {
        use futures::TryStreamExt;

        let cursor = self
            .collection
            .find(doc! { "exp_name": exp_name })
            .sort(doc! { "start_time": -1 })
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

        let doc = self.collection.find_one(doc! {}).with_options(options).await?;

        Ok(doc.map(|d| d.run_number + 1).unwrap_or(1))
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
        let doc = RunDocument {
            id: None,
            run_number: 1,
            exp_name: "test".to_string(),
            comment: String::new(),
            start_time: Utc::now() - chrono::Duration::seconds(60),
            end_time: None,
            duration_secs: None,
            status: RunStatus::Running,
            stats: RunStats::default(),
            config_snapshot: None,
            errors: Vec::new(),
        };

        let info = CurrentRunInfo::from_document(&doc);
        // Allow 1 second tolerance
        assert!(info.elapsed_secs >= 59 && info.elapsed_secs <= 61);
    }
}
