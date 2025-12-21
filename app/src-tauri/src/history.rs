use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;
use uuid::Uuid;

/// Status of a transcription attempt in history.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    InProgress,
    Success,
    Error,
}

impl Default for HistoryStatus {
    fn default() -> Self {
        // Existing history.json entries (pre-status) should be treated as success.
        HistoryStatus::Success
    }
}

/// A single dictation history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub text: String,
    #[serde(default)]
    pub status: HistoryStatus,
    #[serde(default)]
    pub error_message: Option<String>,
}

impl HistoryEntry {
    pub fn new(text: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            text,
            status: HistoryStatus::Success,
            error_message: None,
        }
    }

    pub fn new_request_in_progress(id: String) -> Self {
        Self {
            id,
            timestamp: Utc::now(),
            text: String::new(),
            status: HistoryStatus::InProgress,
            error_message: None,
        }
    }
}

/// Storage for dictation history entries
#[derive(Debug, Serialize, Deserialize, Default)]
struct HistoryData {
    entries: Vec<HistoryEntry>,
}

/// Manages loading and saving of dictation history
pub struct HistoryStorage {
    data: RwLock<HistoryData>,
    file_path: PathBuf,
}

impl HistoryStorage {
    /// Create a new history storage with the given app data directory
    pub fn new(app_data_dir: PathBuf) -> Self {
        let file_path = app_data_dir.join("history.json");

        // Ensure the directory exists
        if let Some(parent) = file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Load existing history or use empty
        let data = Self::load_from_file(&file_path).unwrap_or_default();

        Self {
            data: RwLock::new(data),
            file_path,
        }
    }

    /// Load history from the JSON file
    fn load_from_file(file_path: &PathBuf) -> Option<HistoryData> {
        let content = fs::read_to_string(file_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Save current history to disk
    fn save(&self) -> Result<(), String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;

        let content = serde_json::to_string_pretty(&*data)
            .map_err(|e| format!("Failed to serialize history: {}", e))?;

        fs::write(&self.file_path, content)
            .map_err(|e| format!("Failed to write history file: {}", e))?;

        Ok(())
    }

    /// Add a new entry to the history
    pub fn add_entry(&self, text: String) -> Result<HistoryEntry, String> {
        let entry = HistoryEntry::new(text);
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            // Add to the beginning (newest first)
            data.entries.insert(0, entry.clone());

            // Limit to 500 entries
            if data.entries.len() > 500 {
                data.entries.truncate(500);
            }
        }
        self.save()?;
        Ok(entry)
    }

    /// Add a new in-progress request entry with a predetermined id.
    ///
    /// This is used to show a placeholder in the History view while a transcription
    /// is running, and to keep a failed attempt visible with a retry button.
    pub fn add_request_entry(&self, request_id: String) -> Result<HistoryEntry, String> {
        let entry = HistoryEntry::new_request_in_progress(request_id);
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            // Add to the beginning (newest first)
            data.entries.insert(0, entry.clone());

            // Limit to 500 entries
            if data.entries.len() > 500 {
                data.entries.truncate(500);
            }
        }
        self.save()?;
        Ok(entry)
    }

    /// Mark an existing request entry as successful and set the final text.
    pub fn complete_request_success(&self, request_id: &str, text: String) -> Result<(), String> {
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                entry.text = text;
                entry.status = HistoryStatus::Success;
                entry.error_message = None;
            } else {
                // If we somehow missed creating an in-progress entry, fall back to inserting.
                data.entries.insert(0, HistoryEntry::new_request_in_progress(request_id.to_string()));
                if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                    entry.text = text;
                    entry.status = HistoryStatus::Success;
                    entry.error_message = None;
                }
            }
        }
        self.save()
    }

    /// Mark an existing request entry as failed with an error message.
    pub fn complete_request_error(&self, request_id: &str, error_message: String) -> Result<(), String> {
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                entry.status = HistoryStatus::Error;
                entry.error_message = Some(error_message);
                // Keep text as-is (likely empty). We intentionally do not delete the entry.
            } else {
                let mut entry = HistoryEntry::new_request_in_progress(request_id.to_string());
                entry.status = HistoryStatus::Error;
                entry.error_message = Some(error_message);
                data.entries.insert(0, entry);
            }
        }
        self.save()
    }

    /// Get all history entries (newest first), optionally limited
    pub fn get_all(&self, limit: Option<usize>) -> Result<Vec<HistoryEntry>, String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;

        let entries = match limit {
            Some(n) => data.entries.iter().take(n).cloned().collect(),
            None => data.entries.clone(),
        };

        Ok(entries)
    }

    /// Delete an entry by ID
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let deleted = {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            let initial_len = data.entries.len();
            data.entries.retain(|e| e.id != id);
            data.entries.len() < initial_len
        };

        if deleted {
            self.save()?;
        }

        Ok(deleted)
    }

    /// Clear all history
    pub fn clear(&self) -> Result<(), String> {
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;
            data.entries.clear();
        }
        self.save()
    }
}
