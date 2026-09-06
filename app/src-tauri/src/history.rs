use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use uuid::Uuid;

use crate::app_paths::ensure_dir;
use crate::fs::{Fs, RealFs};

mod edits;
pub use edits::{HistoryDetail, HistoryEdit, HistoryEditInput};

/// Hard safety cap to prevent unbounded growth of `history.json`.
///
/// This is intentionally high so users can effectively keep history forever,
/// while still bounding worst-case disk/memory usage.
const HARD_MAX_HISTORY_ENTRIES: usize = 100_000;

fn effective_history_max(max_entries: Option<usize>) -> usize {
    let hard = HARD_MAX_HISTORY_ENTRIES.max(1);
    match max_entries {
        Some(n) => n.max(1).min(hard),
        None => hard,
    }
}

/// Status of a transcription attempt in history.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    InProgress,
    #[default]
    Success,
    Error,
}

/// A single dictation history entry
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub text: String,
    #[serde(default)]
    pub status: HistoryStatus,
    #[serde(default)]
    pub error_message: Option<String>,
    /// Prompt profile id used for this transcription.
    ///
    /// "default" means no per-program profile matched.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Prompt profile display name used for this transcription.
    #[serde(default)]
    pub profile_name: Option<String>,

    /// Preset id selected for this transcription (if any).
    ///
    /// When None, the request used the profile/global defaults ("Default" in UI).
    #[serde(default)]
    pub preset_id: Option<String>,
    /// Preset display name selected for this transcription (if any).
    #[serde(default)]
    pub preset_name: Option<String>,
    /// STT provider used for this transcription (e.g., "groq", "openai").
    #[serde(default)]
    pub stt_provider: Option<String>,
    /// STT model used for this transcription.
    #[serde(default)]
    pub stt_model: Option<String>,
    /// LLM provider used for rewriting (if enabled).
    #[serde(default)]
    pub llm_provider: Option<String>,
    /// LLM model used for rewriting (if enabled).
    #[serde(default)]
    pub llm_model: Option<String>,

    /// Request id of the WAV recording to use for playback/rerun.
    ///
    /// - For "normal" requests this will typically equal `id` (when a recording was saved).
    /// - For reruns/retries, this should point to the original request id that owns the WAV.
    ///
    /// When `None`, no recording is known/available for this entry.
    #[serde(default)]
    pub recording_request_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    /// Optional original recording metadata, never realigned to manual edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_mode: Option<crate::recordings::options::RecordingMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_segments: Vec<crate::stt::SpeakerSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_stt_text: Option<String>,
}

/// List contract: text is a bounded preview, never the full document. Original
/// STT and speaker metadata are intentionally only available through detail.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistorySummary {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub text: String,
    pub status: HistoryStatus,
    pub error_message: Option<String>,
    pub title: Option<String>,
    pub duration_seconds: Option<f64>,
    pub recording_mode: Option<crate::recordings::options::RecordingMode>,
    pub recording_request_id: Option<String>,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub preset_id: Option<String>,
    pub preset_name: Option<String>,
    pub stt_provider: Option<String>,
    pub stt_model: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
}

/// Metadata about which models were used for a transcription request.
#[derive(Debug, Clone, Default)]
pub struct RequestModelInfo {
    pub stt_provider: Option<String>,
    pub stt_model: Option<String>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub profile_id: Option<String>,
    pub profile_name: Option<String>,
    pub preset_id: Option<String>,
    pub preset_name: Option<String>,
}

/// Request-history lifecycle updates applied on top of the low-level storage helpers.
///
/// `HistoryStorage` remains responsible for persistence/corruption recovery. This enum exists so
/// higher-level flows can describe *what* happened to a request row without re-remembering which
/// individual setter/terminal method to call next.
#[derive(Debug, Clone)]
pub enum RequestHistoryUpdate {
    CreateInProgress {
        request_id: String,
        model_info: RequestModelInfo,
        max_entries: Option<usize>,
    },
    SetProfile {
        request_id: String,
        profile_id: Option<String>,
        profile_name: Option<String>,
    },
    SetPreset {
        request_id: String,
        preset_id: Option<String>,
        preset_name: Option<String>,
    },
    SetRecordingSource {
        request_id: String,
        recording_request_id: Option<String>,
    },
    SetLlmModel {
        request_id: String,
        llm_provider: Option<String>,
        llm_model: Option<String>,
    },
    CompleteSuccess {
        request_id: String,
        text: String,
    },
    CompleteError {
        request_id: String,
        error_message: String,
    },
    Delete {
        request_id: String,
    },
}

impl HistoryEntry {
    pub fn new(text: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            text,
            status: HistoryStatus::Success,
            error_message: None,
            profile_id: None,
            profile_name: None,
            preset_id: None,
            preset_name: None,
            stt_provider: None,
            stt_model: None,
            llm_provider: None,
            llm_model: None,
            recording_request_id: None,
            title: None,
            duration_seconds: None,
            recording_mode: None,
            speaker_segments: Vec::new(),
            original_stt_text: None,
        }
    }

    pub fn new_request_in_progress(id: String, model_info: RequestModelInfo) -> Self {
        Self {
            id,
            timestamp: Utc::now(),
            text: String::new(),
            status: HistoryStatus::InProgress,
            error_message: None,
            profile_id: model_info.profile_id,
            profile_name: model_info.profile_name,
            preset_id: model_info.preset_id,
            preset_name: model_info.preset_name,
            stt_provider: model_info.stt_provider,
            stt_model: model_info.stt_model,
            llm_provider: model_info.llm_provider,
            llm_model: model_info.llm_model,
            recording_request_id: None,
            title: None,
            duration_seconds: None,
            recording_mode: None,
            speaker_segments: Vec::new(),
            original_stt_text: None,
        }
    }
}

/// Storage for dictation history entries
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HistoryData {
    entries: Vec<HistoryEntry>,
    /// Privacy clears invalidate every outstanding editor without retaining text.
    #[serde(default)]
    edit_revision_floor: u64,
}

/// Manages loading and saving of dictation history
pub struct HistoryStorage {
    data: RwLock<HistoryData>,
    file_path: PathBuf,
    fs: Arc<dyn Fs>,
    mutation: Mutex<()>,
    edits: RwLock<edits::EditStore>,
}

impl HistoryStorage {
    /// Create a new history storage with the given app data directory
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self::with_fs(app_data_dir, Arc::new(RealFs))
    }

    /// Create a new history storage with a custom filesystem implementation.
    pub fn with_fs(app_data_dir: PathBuf, fs: Arc<dyn Fs>) -> Self {
        let file_path = app_data_dir.join("history.json");

        // Ensure the directory exists
        if let Some(parent) = file_path.parent() {
            if let Err(e) = ensure_dir(parent) {
                log::warn!(
                    "HistoryStorage: failed to create history directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }

        // Load existing history or recover safely.
        let backup = file_path.with_extension("json.bak");
        if !fs.exists(&file_path) && fs.exists(&backup) {
            // Compatibility with snapshots interrupted during the old two-rename replacement.
            let _ = fs.rename(&backup, &file_path);
        }
        let data = match Self::load_from_file(fs.as_ref(), &file_path) {
            Ok(data) => data,
            Err(LoadHistoryError::NotFound) => HistoryData::default(),
            Err(LoadHistoryError::Read(e)) => {
                log::warn!(
                    "HistoryStorage: failed to read history file {}: {}",
                    file_path.display(),
                    e
                );
                HistoryData::default()
            }
            Err(LoadHistoryError::Parse { error, content }) => {
                log::warn!(
                    "HistoryStorage: failed to parse history file {}; preserving corrupt file and starting fresh: {}",
                    file_path.display(),
                    error
                );
                Self::preserve_corrupt_history_file(fs.as_ref(), &file_path, &content);
                HistoryData::default()
            }
        };

        let edits = edits::EditStore::load(
            fs.as_ref(),
            &file_path,
            &data.entries,
            data.edit_revision_floor,
        );
        Self {
            data: RwLock::new(data),
            file_path,
            fs,
            mutation: Mutex::new(()),
            edits: RwLock::new(edits),
        }
    }

    fn preserve_corrupt_history_file(fs: &dyn Fs, file_path: &std::path::Path, content: &str) {
        let Some(parent) = file_path.parent() else {
            return;
        };

        // Keep filename stable and filesystem-friendly (Windows-safe).
        let ts = Utc::now().format("%Y%m%d_%H%M%S");
        let corrupt_path = parent.join(format!("history.corrupt.{}.{}.json", ts, Uuid::new_v4()));

        // Best-effort: rename the original file out of the way.
        // If rename fails (e.g. cross-device or permission issues), fall back
        // to copying the bytes to the corrupt path.
        match fs.rename(file_path, &corrupt_path) {
            Ok(()) => {
                log::warn!(
                    "HistoryStorage: moved corrupt history file to {}",
                    corrupt_path.display()
                );
            }
            Err(e) => {
                log::warn!(
                    "HistoryStorage: failed to rename corrupt history file {} -> {}: {} (will attempt copy)",
                    file_path.display(),
                    corrupt_path.display(),
                    e
                );
                if let Err(e2) = fs.write(&corrupt_path, content.as_bytes()) {
                    log::warn!(
                        "HistoryStorage: failed to write corrupt history copy {}: {}",
                        corrupt_path.display(),
                        e2
                    );
                }
            }
        }
    }

    /// Load history from the JSON file
    fn load_from_file(
        fs: &dyn Fs,
        file_path: &std::path::Path,
    ) -> Result<HistoryData, LoadHistoryError> {
        let content = match fs.read_to_string(file_path) {
            Ok(s) => s,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(LoadHistoryError::NotFound),
            Err(e) => return Err(LoadHistoryError::Read(e.to_string())),
        };

        serde_json::from_str(&content).map_err(|e| LoadHistoryError::Parse {
            error: e.to_string(),
            content,
        })
    }

    /// Save current history to disk
    fn save(&self) -> Result<(), String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;

        let content = serde_json::to_string_pretty(&*data)
            .map_err(|e| format!("Failed to serialize history: {}", e))?;

        self.atomic_write_history_json(content.as_bytes())?;
        self.cleanup_edits(&data.entries)?;

        Ok(())
    }

    fn atomic_write_history_json(&self, bytes: &[u8]) -> Result<(), String> {
        self.atomic_write_json(&self.file_path, bytes)
    }

    fn atomic_write_json(&self, file_path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
        let Some(parent) = file_path.parent() else {
            return Err("Failed to write history file: missing parent directory".to_string());
        };
        self.fs
            .create_dir_all(parent)
            .map_err(|e| format!("Failed to create history dir {}: {}", parent.display(), e))?;

        let file_name = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("history.json");

        let tmp_path = parent.join(format!("{}.tmp.{}", file_name, Uuid::new_v4()));
        let bak_path = parent.join(format!("{}.bak", file_name));

        // Write new content to a temp file first.
        self.fs.write_private(&tmp_path, bytes).map_err(|e| {
            let _ = self.fs.remove_file(&tmp_path);
            format!(
                "Failed to write temp history file {}: {}",
                tmp_path.display(),
                e
            )
        })?;

        // Remove legacy backups before committing, so cleanup failure cannot
        // report a failed save after the new revision has already been written.
        if self.fs.exists(&bak_path) && self.fs.remove_file(&bak_path).is_err() {
            let _ = self.fs.remove_file(&tmp_path);
            return Err("Could not remove previous document snapshot".into());
        }

        // Same-directory rename replaces a file on all supported platforms.
        // Never unlink the original first: it remains intact on rename failure.
        if let Err(e) = self.fs.rename(&tmp_path, file_path) {
            let _ = self.fs.remove_file(&tmp_path);
            return Err(format!(
                "Failed to replace history file {}: {}",
                file_path.display(),
                e
            ));
        }

        Ok(())
    }

    /// Add a new entry to the history
    pub fn add_entry(
        &self,
        text: String,
        max_entries: Option<usize>,
    ) -> Result<HistoryEntry, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let entry = HistoryEntry::new(text);
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            // Add to the beginning (newest first)
            data.entries.insert(0, entry.clone());

            let max = effective_history_max(max_entries);
            if data.entries.len() > max {
                data.entries.truncate(max);
            }
        }
        self.save()?;
        Ok(entry)
    }

    /// Add a new in-progress request entry with a predetermined id.
    ///
    /// This is used to show a placeholder in the History view while a transcription
    /// is running, and to keep a failed attempt visible with a retry button.
    pub fn add_request_entry(
        &self,
        request_id: String,
        model_info: RequestModelInfo,
        max_entries: Option<usize>,
    ) -> Result<HistoryEntry, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let entry = HistoryEntry::new_request_in_progress(request_id, model_info);
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            // Add to the beginning (newest first)
            data.entries.insert(0, entry.clone());

            let max = effective_history_max(max_entries);
            if data.entries.len() > max {
                data.entries.truncate(max);
            }
        }
        self.save()?;
        Ok(entry)
    }

    /// Truncate history to the effective configured max.
    ///
    /// When `max_entries` is None, we still apply the hard safety cap.
    pub fn trim_to_configured(&self, max_entries: Option<usize>) -> Result<(), String> {
        let max = effective_history_max(max_entries);
        self.trim_to(max)
    }

    /// Truncate history to at most `max_entries` entries.
    pub fn trim_to(&self, max_entries: usize) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let max = max_entries.max(1);
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;
            if data.entries.len() > max {
                data.entries.truncate(max);
            }
        }
        self.save()
    }

    /// Delete entries older than `cutoff` (strictly earlier than cutoff).
    ///
    /// Returns the list of removed entry IDs (useful for cleaning up recordings).
    pub fn prune_older_than(&self, cutoff: DateTime<Utc>) -> Result<Vec<String>, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let mut removed: Vec<String> = Vec::new();
        let changed = {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            let before = data.entries.len();
            data.entries.retain(|entry| {
                if entry.timestamp < cutoff {
                    removed.push(entry.id.clone());
                    false
                } else {
                    true
                }
            });

            data.entries.len() != before
        };

        if changed {
            self.save()?;
        }

        Ok(removed)
    }

    /// Convert any entries still marked as `in_progress` into `error`.
    ///
    /// This is primarily a safety net for app restarts/crashes so the UI doesn't show
    /// stale "Transcribing..." indicators for old history rows.
    ///
    /// Returns the number of entries updated.
    pub fn finalize_all_in_progress_as_error(
        &self,
        error_message: String,
    ) -> Result<usize, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let mut updated = 0usize;

        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            for entry in data.entries.iter_mut() {
                if entry.status == HistoryStatus::InProgress {
                    entry.status = HistoryStatus::Error;
                    entry.error_message = Some(error_message.clone());
                    updated += 1;
                }
            }
        }

        if updated > 0 {
            self.save()?;
        }

        Ok(updated)
    }

    /// Mark an existing request entry as successful and set the final text.
    pub fn complete_request_success(&self, request_id: &str, text: String) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let previous = self
            .data
            .read()
            .map_err(|_| "History unavailable")?
            .entries
            .iter()
            .find(|e| e.id == request_id)
            .cloned();
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
                data.entries.insert(
                    0,
                    HistoryEntry::new_request_in_progress(
                        request_id.to_string(),
                        RequestModelInfo::default(),
                    ),
                );
                if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                    entry.text = text;
                    entry.status = HistoryStatus::Success;
                    entry.error_message = None;
                }
            }
        }
        if let Err(error) = self.save() {
            // Recovery checks Success before removing its journal. A failed
            // durable save must never leave a false successful row in memory.
            let mut data = self.data.write().map_err(|_| "History unavailable")?;
            if let Some(previous) = previous {
                if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                    *entry = previous;
                }
            } else {
                data.entries.retain(|e| e.id != request_id);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Mark an existing request entry as failed with an error message.
    pub fn complete_request_error(
        &self,
        request_id: &str,
        error_message: String,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
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
                let mut entry = HistoryEntry::new_request_in_progress(
                    request_id.to_string(),
                    RequestModelInfo::default(),
                );
                entry.status = HistoryStatus::Error;
                entry.error_message = Some(error_message);
                data.entries.insert(0, entry);
            }
        }
        self.save()
    }

    /// Apply one request-row lifecycle update.
    ///
    /// Keep this as a thin orchestration layer over the storage primitives above. The goal is not
    /// to hide persistence behavior; it is to give command/session flows a single, testable
    /// request-history surface so they stop duplicating mutation ordering by hand.
    pub fn apply_request_update(&self, update: RequestHistoryUpdate) -> Result<(), String> {
        match update {
            RequestHistoryUpdate::CreateInProgress {
                request_id,
                model_info,
                max_entries,
            } => {
                self.add_request_entry(request_id, model_info, max_entries)?;
                Ok(())
            }
            RequestHistoryUpdate::SetProfile {
                request_id,
                profile_id,
                profile_name,
            } => self.set_request_profile(&request_id, profile_id, profile_name),
            RequestHistoryUpdate::SetPreset {
                request_id,
                preset_id,
                preset_name,
            } => self.set_request_preset(&request_id, preset_id, preset_name),
            RequestHistoryUpdate::SetRecordingSource {
                request_id,
                recording_request_id,
            } => self.set_request_recording_id(&request_id, recording_request_id),
            RequestHistoryUpdate::SetLlmModel {
                request_id,
                llm_provider,
                llm_model,
            } => self.set_request_llm_model(&request_id, llm_provider, llm_model),
            RequestHistoryUpdate::CompleteSuccess { request_id, text } => {
                self.complete_request_success(&request_id, text)
            }
            RequestHistoryUpdate::CompleteError {
                request_id,
                error_message,
            } => self.complete_request_error(&request_id, error_message),
            RequestHistoryUpdate::Delete { request_id } => self.delete(&request_id).map(|_| ()),
        }
    }

    /// Get all history entries (newest first), optionally limited
    pub fn get_all(&self, limit: Option<usize>) -> Result<Vec<HistoryEntry>, String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;

        let entries: Vec<HistoryEntry> = match limit {
            Some(n) => data.entries.iter().take(n).cloned().collect(),
            None => data.entries.clone(),
        };

        let edits = self.edits.read().map_err(|_| "History edits unavailable")?;
        Ok(entries
            .into_iter()
            .map(|entry| edits.apply(entry))
            .collect())
    }

    /// Get a single history entry by request id.
    pub fn get_by_id(&self, request_id: &str) -> Result<Option<HistoryEntry>, String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;

        let edits = self.edits.read().map_err(|_| "History edits unavailable")?;
        Ok(data
            .entries
            .iter()
            .find(|e| e.id == request_id)
            .cloned()
            .map(|e| edits.apply(e)))
    }

    /// Update the stored profile metadata for an existing history entry.
    ///
    /// This is useful when we create an in-progress entry early, then later
    /// learn the effective profile used once transcription actually begins.
    pub fn set_request_profile(
        &self,
        request_id: &str,
        profile_id: Option<String>,
        profile_name: Option<String>,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                entry.profile_id = profile_id;
                entry.profile_name = profile_name;
            }
        }

        self.save()
    }

    /// Update the stored preset metadata for an existing history entry.
    ///
    /// This is useful when we create an in-progress entry early, then later
    /// learn the effective preset used once routing has been resolved.
    pub fn set_request_preset(
        &self,
        request_id: &str,
        preset_id: Option<String>,
        preset_name: Option<String>,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                entry.preset_id = preset_id;
                entry.preset_name = preset_name;
            }
        }

        self.save()
    }

    /// Update the stored recording source id for an existing history entry.
    pub fn set_request_recording_id(
        &self,
        request_id: &str,
        recording_request_id: Option<String>,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                entry.recording_request_id = recording_request_id;
            }
        }

        self.save()
    }

    /// Update the stored LLM provider/model for an existing history entry.
    ///
    /// This is useful when we create an in-progress entry early (based on the
    /// configured model), but later learn whether the LLM step actually ran and
    /// which concrete model was used.
    pub fn set_request_llm_model(
        &self,
        request_id: &str,
        llm_provider: Option<String>,
        llm_model: Option<String>,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            if let Some(entry) = data.entries.iter_mut().find(|e| e.id == request_id) {
                entry.llm_provider = llm_provider;
                entry.llm_model = llm_model;
            }
        }

        self.save()
    }

    /// Query history with server-side filtering and pagination.
    ///
    /// This is primarily used by the UI to avoid transferring the entire history
    /// list across the IPC boundary on every filter keystroke.
    pub fn query_page(&self, params: HistoryPageQuery) -> Result<HistoryPageResult, String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;

        let entries = &data.entries;
        let edits = self.edits.read().map_err(|_| "History edits unavailable")?;
        let total_all = entries.len();

        // Defaults match the current UI behavior.
        let filter_text = params.filter_text.unwrap_or_default().trim().to_lowercase();
        let show_failed = params.show_failed.unwrap_or(true);
        let show_empty_transcript = params.show_empty_transcript.unwrap_or(false);
        let selected_stt_model_keys = params.selected_stt_model_keys.unwrap_or_default();
        let selected_llm_model_keys = params.selected_llm_model_keys.unwrap_or_default();

        let page_size_raw = params.page_size.unwrap_or(25);
        // Defensive clamp; keep page sizes reasonable for IPC payloads.
        let page_size = page_size_raw.clamp(1, 200);

        // Usage counts (across ALL history, independent of filters).
        let include_usage_counts = params.include_usage_counts.unwrap_or(true);
        let (stt_model_usage, llm_model_usage) = if include_usage_counts {
            let mut stt_counts: HashMap<String, usize> = HashMap::new();
            let mut llm_counts: HashMap<String, usize> = HashMap::new();

            for entry in entries.iter() {
                if let (Some(p), Some(m)) = (entry.stt_provider.as_ref(), entry.stt_model.as_ref())
                {
                    let key = format!("{}::{}", p, m);
                    *stt_counts.entry(key).or_insert(0) += 1;
                }

                if let (Some(p), Some(m)) = (entry.llm_provider.as_ref(), entry.llm_model.as_ref())
                {
                    let key = format!("{}::{}", p, m);
                    *llm_counts.entry(key).or_insert(0) += 1;
                }
            }

            let mut stt_vec: Vec<ModelUsageCount> = stt_counts
                .into_iter()
                .map(|(key, count)| ModelUsageCount { key, count })
                .collect();
            let mut llm_vec: Vec<ModelUsageCount> = llm_counts
                .into_iter()
                .map(|(key, count)| ModelUsageCount { key, count })
                .collect();

            // Sort: most-used first, stable by key for determinism.
            stt_vec.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
            llm_vec.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));

            (stt_vec, llm_vec)
        } else {
            (Vec::new(), Vec::new())
        };

        let matches_filters = |entry: &HistoryEntry| -> bool {
            // 1) Text search
            if !filter_text.is_empty() {
                let text = edits.text(entry).to_lowercase();
                let status = match entry.status {
                    HistoryStatus::InProgress => "in_progress",
                    HistoryStatus::Success => "success",
                    HistoryStatus::Error => "error",
                };
                let err = entry.error_message.as_deref().unwrap_or("").to_lowercase();

                let matches = text.contains(&filter_text)
                    || status.contains(&filter_text)
                    || err.contains(&filter_text)
                    || edits
                        .title(entry)
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&filter_text);
                if !matches {
                    return false;
                }
            }

            // 2) Show Failed
            if !show_failed && entry.status == HistoryStatus::Error {
                return false;
            }

            // 3) Show Empty transcript
            if !show_empty_transcript
                && entry.status == HistoryStatus::Success
                && edits.text(entry).trim().is_empty()
            {
                return false;
            }

            // 4) STT model filter
            if !selected_stt_model_keys.is_empty() {
                let Some(provider) = entry.stt_provider.as_ref() else {
                    return false;
                };
                let Some(model) = entry.stt_model.as_ref() else {
                    return false;
                };
                let key = format!("{}::{}", provider, model);
                if !selected_stt_model_keys.iter().any(|k| k == &key) {
                    return false;
                }
            }

            // 5) LLM model filter
            if !selected_llm_model_keys.is_empty() {
                let Some(provider) = entry.llm_provider.as_ref() else {
                    return false;
                };
                let Some(model) = entry.llm_model.as_ref() else {
                    return false;
                };
                let key = format!("{}::{}", provider, model);
                if !selected_llm_model_keys.iter().any(|k| k == &key) {
                    return false;
                }
            }

            true
        };

        // Filter into references to avoid cloning the entire dataset.
        let mut filtered: Vec<&HistoryEntry> = Vec::with_capacity(entries.len().min(2048));
        for entry in entries.iter() {
            if matches_filters(entry) {
                filtered.push(entry);
            }
        }

        let total_filtered = filtered.len();
        let total_pages = total_filtered.div_ceil(page_size).max(1);

        let mut page = params.page.unwrap_or(1);
        if page < 1 {
            page = 1;
        }
        if page > total_pages {
            page = total_pages;
        }

        let start = (page - 1) * page_size;
        let end = (start + page_size).min(total_filtered);

        let items: Vec<HistorySummary> = if start >= total_filtered {
            Vec::new()
        } else {
            filtered[start..end]
                .iter()
                .map(|e| {
                    // Only a bounded preview crosses IPC. Details are loaded on demand.
                    HistorySummary {
                        id: e.id.clone(),
                        timestamp: e.timestamp,
                        status: e.status,
                        text: edits.text(e).chars().take(320).collect(),
                        title: edits.title(e).map(str::to_owned),
                        error_message: e
                            .error_message
                            .as_ref()
                            .map(|s| s.chars().take(320).collect()),
                        duration_seconds: e.duration_seconds,
                        recording_mode: e.recording_mode,
                        recording_request_id: e.recording_request_id.clone(),
                        profile_id: e.profile_id.clone(),
                        profile_name: e.profile_name.clone(),
                        preset_id: e.preset_id.clone(),
                        preset_name: e.preset_name.clone(),
                        stt_provider: e.stt_provider.clone(),
                        stt_model: e.stt_model.clone(),
                        llm_provider: e.llm_provider.clone(),
                        llm_model: e.llm_model.clone(),
                    }
                })
                .collect()
        };

        Ok(HistoryPageResult {
            items,
            total_all,
            total_filtered,
            page,
            page_size,
            stt_model_usage,
            llm_model_usage,
        })
    }

    /// Return the number of stored history entries.
    pub fn count(&self) -> Result<usize, String> {
        let data = self
            .data
            .read()
            .map_err(|e| format!("Failed to read history: {}", e))?;
        Ok(data.entries.len())
    }

    /// Best-effort history file size on disk (bytes).
    pub fn file_size_bytes(&self) -> u64 {
        self.fs
            .metadata(&self.file_path)
            .map(|m| m.len())
            .unwrap_or(0)
            .saturating_add(self.edits_size_bytes())
    }

    /// Delete an entry by ID
    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
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

    /// Delete multiple entries by ID in a single save.
    ///
    /// Returns the number of entries removed.
    pub fn delete_many(&self, ids: &HashSet<String>) -> Result<usize, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        if ids.is_empty() {
            return Ok(0);
        }

        let deleted = {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            let initial_len = data.entries.len();
            data.entries.retain(|e| !ids.contains(&e.id));
            initial_len.saturating_sub(data.entries.len())
        };

        if deleted > 0 {
            self.save()?;
        }

        Ok(deleted)
    }

    /// Clear `recording_request_id` for any entries pointing at a given recording source.
    ///
    /// Returns the number of entries updated.
    pub fn clear_recording_request_id_for_source(
        &self,
        recording_request_id: &str,
    ) -> Result<usize, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let mut updated = 0usize;

        {
            let mut data = self
                .data
                .write()
                .map_err(|e| format!("Failed to write history: {}", e))?;

            for entry in data.entries.iter_mut() {
                if entry.recording_request_id.as_deref() == Some(recording_request_id) {
                    entry.recording_request_id = None;
                    updated += 1;
                }
            }
        }

        if updated > 0 {
            self.save()?;
        }

        Ok(updated)
    }

    /// Clear all stored transcript text while keeping history rows and recording links.
    ///
    /// This is useful for privacy: users may want to keep the audio (recordings)
    /// but remove the transcribed text from `history.json`.
    ///
    /// Returns the number of entries whose `text` was changed.
    pub fn clear_all_transcript_text_keep_recordings(&self) -> Result<usize, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let mut data = self.data.write().map_err(|_| "History unavailable")?;
        let mut edits = self
            .edits
            .write()
            .map_err(|_| "History edits unavailable")?;
        let mut cleared = data.clone();
        cleared.edit_revision_floor = edits
            .max_revision()
            .max(data.edit_revision_floor)
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or("Document revision limit reached")?;
        let mut updated = 0usize;
        for entry in cleared.entries.iter_mut() {
            if !entry.text.is_empty()
                || entry.title.is_some()
                || !entry.speaker_segments.is_empty()
                || entry.original_stt_text.is_some()
                || edits.has_correction(&entry.id)
            {
                updated += 1;
            }
            entry.title = None;
            entry.speaker_segments.clear();
            entry.original_stt_text = None;
            if !entry.text.is_empty() {
                entry.text.clear();
            }
        }
        // Commit the empty originals and revision barrier first. After a crash,
        // old sidecars cannot make deleted text reappear; delayed saves conflict.
        let bytes =
            serde_json::to_vec_pretty(&cleared).map_err(|_| "Could not prepare History clear")?;
        self.atomic_write_history_json(&bytes)?;
        *data = cleared;
        *edits = edits::EditStore::default();
        drop(edits);
        drop(data);
        self.cleanup_edits(&[])?;
        Ok(updated)
    }

    /// Clear all history
    pub fn clear(&self) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
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

#[derive(Debug)]
enum LoadHistoryError {
    NotFound,
    Read(String),
    Parse { error: String, content: String },
}

// ============================================================================
// Server-side paging/filtering structs (UI API)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageCount {
    pub key: String,
    pub count: usize,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPageQuery {
    pub filter_text: Option<String>,
    pub show_failed: Option<bool>,
    pub show_empty_transcript: Option<bool>,
    pub selected_stt_model_keys: Option<Vec<String>>,
    pub selected_llm_model_keys: Option<Vec<String>>,

    /// 1-based page index.
    pub page: Option<usize>,
    pub page_size: Option<usize>,

    /// When true, include per-model usage counts in the response.
    pub include_usage_counts: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPageResult {
    pub items: Vec<HistorySummary>,
    pub total_all: usize,
    pub total_filtered: usize,
    pub page: usize,
    pub page_size: usize,
    pub stt_model_usage: Vec<ModelUsageCount>,
    pub llm_model_usage: Vec<ModelUsageCount>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::path::Path;
    use std::sync::Mutex;

    use crate::fs::Fs;

    #[derive(Debug, Default)]
    struct MemoryFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
    }

    impl MemoryFs {
        fn get_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
            let guard = self
                .files
                .lock()
                .map_err(|_| io::Error::other("memory fs lock poisoned"))?;
            guard
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))
        }
    }

    impl Fs for MemoryFs {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.get_bytes(path)
        }

        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            let bytes = self.get_bytes(path)?;
            String::from_utf8(bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        }

        fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
            let mut guard = self
                .files
                .lock()
                .map_err(|_| io::Error::other("memory fs lock poisoned"))?;
            guard.insert(path.to_path_buf(), contents.to_vec());
            Ok(())
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            let mut guard = self
                .files
                .lock()
                .map_err(|_| io::Error::other("memory fs lock poisoned"))?;

            let Some(bytes) = guard.remove(from) else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "file not found"));
            };
            guard.insert(to.to_path_buf(), bytes);
            Ok(())
        }

        fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }

        fn read_dir(&self, _path: &Path) -> io::Result<Vec<PathBuf>> {
            Ok(Vec::new())
        }

        fn metadata(&self, _path: &Path) -> io::Result<std::fs::Metadata> {
            Err(io::Error::new(io::ErrorKind::NotFound, "no metadata"))
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            let mut guard = self
                .files
                .lock()
                .map_err(|_| io::Error::other("memory fs lock poisoned"))?;
            guard.remove(path);
            Ok(())
        }

        fn exists(&self, path: &Path) -> bool {
            let Ok(guard) = self.files.lock() else {
                return false;
            };
            guard.contains_key(path)
        }
    }

    fn make_temp_app_dir() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("kolboo-history-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("failed to create temp test dir");
        dir
    }

    #[test]
    fn add_entry_without_configured_cap_does_not_truncate_small_history() {
        let dir = make_temp_app_dir();
        let history = HistoryStorage::new(dir);

        for i in 0..60 {
            history
                .add_entry(format!("entry {i}"), None)
                .expect("add_entry failed");
        }

        let entries = history.get_all(None).expect("get_all failed");
        assert_eq!(entries.len(), 60);
    }

    #[test]
    fn add_entry_with_amount_cap_truncates() {
        let dir = make_temp_app_dir();
        let history = HistoryStorage::new(dir);

        for i in 0..25 {
            history
                .add_entry(format!("entry {i}"), Some(10))
                .expect("add_entry failed");
        }

        let entries = history.get_all(None).expect("get_all failed");
        assert_eq!(entries.len(), 10);
        // Newest first.
        assert_eq!(entries[0].text, "entry 24");
    }

    #[test]
    fn finalize_all_in_progress_marks_entries_as_error_and_persists() {
        let dir = make_temp_app_dir();
        let history = HistoryStorage::new(dir.clone());

        let req_id = "req-123".to_string();
        history
            .add_request_entry(req_id.clone(), RequestModelInfo::default(), None)
            .expect("add_request_entry failed");

        let before = history
            .get_by_id(&req_id)
            .expect("get_by_id failed")
            .expect("missing entry");
        assert_eq!(before.status, HistoryStatus::InProgress);

        let updated = history
            .finalize_all_in_progress_as_error("Interrupted (app restarted)".to_string())
            .expect("finalize_all_in_progress_as_error failed");
        assert_eq!(updated, 1);

        // Reload from disk to ensure persistence.
        let history2 = HistoryStorage::new(dir);
        let after = history2
            .get_by_id(&req_id)
            .expect("get_by_id failed")
            .expect("missing entry");
        assert_eq!(after.status, HistoryStatus::Error);
        assert_eq!(
            after.error_message.as_deref(),
            Some("Interrupted (app restarted)")
        );
    }

    #[test]
    fn history_storage_with_custom_fs_writes_to_memory() {
        let fs = Arc::new(MemoryFs::default());
        let dir = PathBuf::from("mem://history");
        let history = HistoryStorage::with_fs(dir.clone(), fs.clone());

        let _ = history
            .add_entry("hello".to_string(), None)
            .expect("add_entry failed");

        let contents = fs
            .read_to_string(&dir.join("history.json"))
            .expect("read_to_string failed");
        assert!(contents.contains("hello"));
    }

    #[test]
    fn clear_all_transcript_text_keeps_recording_links() {
        let dir = make_temp_app_dir();
        let history = HistoryStorage::new(dir.clone());

        let req_id = "req-1".to_string();
        history
            .add_request_entry(req_id.clone(), RequestModelInfo::default(), None)
            .expect("add_request_entry failed");
        history
            .complete_request_success(&req_id, "hello world".to_string())
            .expect("complete_request_success failed");
        history
            .set_request_recording_id(&req_id, Some("rec-1".to_string()))
            .expect("set_request_recording_id failed");

        let updated = history
            .clear_all_transcript_text_keep_recordings()
            .expect("clear_all_transcript_text_keep_recordings failed");
        assert_eq!(updated, 1);

        // Reload to ensure persistence.
        let history2 = HistoryStorage::new(dir);
        let entry = history2
            .get_by_id(&req_id)
            .expect("get_by_id failed")
            .expect("missing entry");
        assert_eq!(entry.text, "");
        assert_eq!(entry.recording_request_id.as_deref(), Some("rec-1"));
        assert_eq!(entry.status, HistoryStatus::Success);
    }
}
