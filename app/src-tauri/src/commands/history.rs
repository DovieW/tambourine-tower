use crate::history::{HistoryEntry, HistoryPageQuery, HistoryPageResult, HistoryStorage};
use crate::recordings::RecordingStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tauri::Manager;
use tauri::{AppHandle, State};

use crate::commands::{CommandError, CommandResult};
use crate::settings::store::{get_fresh_settings_store, store_get_u64_clamped};

fn history_error(message: impl Into<String>) -> CommandError {
    CommandError::new(message, "history")
}

pub(crate) fn get_max_saved_recordings(app: &AppHandle) -> usize {
    #[cfg(desktop)]
    {
        let default: u64 = 1000;
        let store = get_fresh_settings_store(app);
        let raw = store
            .as_ref()
            .map(|s| store_get_u64_clamped(s, "max_saved_recordings", default, 1, 100_000))
            .unwrap_or(default);
        raw as usize
    }

    #[cfg(not(desktop))]
    {
        1000
    }
}

/// Optional max entries for transcription history.
///
/// This is intentionally decoupled from recordings retention.
/// - If retention mode is "amount", we cap history to that amount.
/// - If retention mode is "time" (or missing), history is not capped by settings
///   (but will still be bounded by a hard safety cap in `HistoryStorage`).
pub(crate) fn get_history_max_entries(app: &AppHandle) -> Option<usize> {
    #[cfg(desktop)]
    {
        let store = get_fresh_settings_store(app);

        let mode: String = store
            .as_ref()
            .and_then(|s| s.get("transcription_retention_mode"))
            .and_then(|v| match v {
                serde_json::Value::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| "time".to_string());

        if mode != "amount" {
            return None;
        }

        let default_amount: u64 = 1000;
        let raw = store
            .as_ref()
            .map(|s| {
                store_get_u64_clamped(
                    s,
                    "transcription_retention_amount",
                    default_amount,
                    1,
                    100_000,
                )
            })
            .unwrap_or(default_amount);
        Some(raw as usize)
    }

    #[cfg(not(desktop))]
    {
        None
    }
}

/// Add a new entry to the dictation history
#[tauri::command]
pub async fn add_history_entry(
    app: AppHandle,
    text: String,
    history: State<'_, HistoryStorage>,
) -> CommandResult<HistoryEntry> {
    let max = get_history_max_entries(&app);
    history.add_entry(text, max).map_err(CommandError::from)
}

/// Get dictation history entries
#[tauri::command]
pub async fn get_history(
    limit: Option<usize>,
    history: State<'_, HistoryStorage>,
) -> CommandResult<Vec<HistoryEntry>> {
    history.get_all(limit).map_err(CommandError::from)
}

/// Get a filtered/paginated slice of history entries.
///
/// Returns both the items for the requested page and the total count of filtered
/// entries so the UI can render correct pagination.
#[tauri::command]
pub async fn get_history_page(
    params: HistoryPageQuery,
    history: State<'_, HistoryStorage>,
) -> CommandResult<HistoryPageResult> {
    history.query_page(params).map_err(CommandError::from)
}

#[tauri::command]
pub async fn get_history_detail(
    id: String,
    history: State<'_, HistoryStorage>,
) -> CommandResult<Option<crate::history::HistoryDetail>> {
    history.detail(&id).map_err(history_error)
}

#[tauri::command]
pub async fn save_history_edit(
    app: AppHandle,
    input: crate::history::HistoryEditInput,
) -> CommandResult<crate::history::HistoryEdit> {
    // Large corrections and fsync must not occupy the async command executor.
    let result = tauri::async_runtime::spawn_blocking(move || {
        app.state::<HistoryStorage>().save_edit(input)
    })
    .await
    .map_err(|_| history_error("Could not finish saving this correction"))?;
    result.map_err(|message| {
        if message.starts_with("HISTORY_EDIT_CONFLICT") {
            history_error(message).with_code("HISTORY_EDIT_CONFLICT")
        } else {
            history_error(message)
        }
    })
}

/// Delete a history entry by ID
#[tauri::command]
pub async fn delete_history_entry(
    id: String,
    history: State<'_, HistoryStorage>,
) -> CommandResult<bool> {
    history.delete(&id).map_err(CommandError::from)
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HistoryDeleteOptions {
    pub recording_id: Option<String>,
    pub recording_exists: bool,
    pub recording_ref_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDeleteMode {
    /// Delete only this history entry.
    EntryOnly,
    /// Delete this entry and delete the underlying recording.
    ///
    /// Any other history entries that referenced this recording will have their
    /// `recording_request_id` cleared (so Play/Rerun disappear).
    EntryAndRecording,
    /// Delete the recording AND all history entries that reference it.
    RecordingAndAllEntries,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HistoryDeleteResult {
    pub deleted_entries: u64,
    pub deleted_recording: bool,
}

fn resolve_recording_source_id(
    app: &AppHandle,
    history: &HistoryStorage,
    entry: &HistoryEntry,
) -> Result<Option<String>, CommandError> {
    if let Some(rid) = entry.recording_request_id.as_ref() {
        let r = rid.trim();
        if !r.is_empty() {
            return Ok(Some(r.to_string()));
        }
    }

    // Best-effort backfill: if a WAV exists under this entry id, treat it as the recording source.
    if let Some(store) = app.try_state::<RecordingStore>() {
        if store.wav_path_if_exists(&entry.id).ok().flatten().is_some() {
            let _ = history.set_request_recording_id(&entry.id, Some(entry.id.clone()));
            return Ok(Some(entry.id.clone()));
        }
    }

    Ok(None)
}

fn compute_recording_ref_count(
    app: &AppHandle,
    history: &HistoryStorage,
    recording_id: &str,
) -> Result<u64, CommandError> {
    let entries = history.get_all(None)?;

    let mut count = 0u64;
    for e in entries.iter() {
        let source = if let Some(rid) = e.recording_request_id.as_ref() {
            let r = rid.trim();
            if r.is_empty() {
                None
            } else {
                Some(r.to_string())
            }
        } else if let Some(store) = app.try_state::<RecordingStore>() {
            if store.wav_path_if_exists(&e.id).ok().flatten().is_some() {
                Some(e.id.clone())
            } else {
                None
            }
        } else {
            None
        };

        if source.as_deref() == Some(recording_id) {
            count += 1;
        }
    }

    Ok(count)
}

/// Determine whether deleting an entry should also delete a recording, and whether the
/// recording is shared by other history entries.
#[tauri::command]
pub async fn get_history_delete_options(
    app: AppHandle,
    id: String,
    history: State<'_, HistoryStorage>,
) -> CommandResult<HistoryDeleteOptions> {
    let Some(entry) = history.get_by_id(&id)? else {
        return Err(history_error("History entry not found"));
    };

    let recording_id = resolve_recording_source_id(&app, &history, &entry)?;
    let recording_exists = recording_id
        .as_deref()
        .map(|rid| {
            app.try_state::<RecordingStore>()
                .map(|store| store.wav_path_if_exists(rid).ok().flatten().is_some())
                .unwrap_or(false)
        })
        .unwrap_or(false);

    let recording_ref_count = if let Some(rid) = recording_id.as_deref() {
        compute_recording_ref_count(&app, &history, rid)?
    } else {
        0
    };

    Ok(HistoryDeleteOptions {
        recording_id,
        recording_exists,
        recording_ref_count,
    })
}

/// Delete a history entry with an explicit mode, optionally deleting the underlying recording.
#[tauri::command]
pub async fn delete_history_entry_ex(
    app: AppHandle,
    id: String,
    mode: HistoryDeleteMode,
    history: State<'_, HistoryStorage>,
) -> CommandResult<HistoryDeleteResult> {
    let entry = history
        .get_by_id(&id)?
        .ok_or_else(|| history_error("History entry not found"))?;

    let recording_id = resolve_recording_source_id(&app, &history, &entry)?;

    match mode {
        HistoryDeleteMode::EntryOnly => {
            let deleted = history.delete(&id)?;
            Ok(HistoryDeleteResult {
                deleted_entries: if deleted { 1 } else { 0 },
                deleted_recording: false,
            })
        }
        HistoryDeleteMode::EntryAndRecording => {
            let deleted_entry = history.delete(&id)?;

            let mut deleted_recording = false;
            if let Some(rid) = recording_id.as_deref() {
                if let Some(store) = app.try_state::<RecordingStore>() {
                    deleted_recording = store.delete_wav_if_exists(rid)?;
                }
                // Ensure any other entries pointing at this recording no longer claim a recording.
                let _ = history.clear_recording_request_id_for_source(rid);
            }

            Ok(HistoryDeleteResult {
                deleted_entries: if deleted_entry { 1 } else { 0 },
                deleted_recording,
            })
        }
        HistoryDeleteMode::RecordingAndAllEntries => {
            let Some(rid) = recording_id.as_deref() else {
                // Nothing to delete other than the requested entry.
                let deleted = history.delete(&id)?;
                return Ok(HistoryDeleteResult {
                    deleted_entries: if deleted { 1 } else { 0 },
                    deleted_recording: false,
                });
            };

            let entries = history.get_all(None)?;
            let mut ids: HashSet<String> = HashSet::new();
            for e in entries.iter() {
                let source = if let Some(x) = e.recording_request_id.as_ref() {
                    let t = x.trim();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t.to_string())
                    }
                } else if let Some(store) = app.try_state::<RecordingStore>() {
                    if store.wav_path_if_exists(&e.id).ok().flatten().is_some() {
                        Some(e.id.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if source.as_deref() == Some(rid) {
                    ids.insert(e.id.clone());
                }
            }

            let deleted_entries = history.delete_many(&ids)? as u64;

            let deleted_recording = if let Some(store) = app.try_state::<RecordingStore>() {
                store.delete_wav_if_exists(rid)?
            } else {
                false
            };

            Ok(HistoryDeleteResult {
                deleted_entries,
                deleted_recording,
            })
        }
    }
}

/// Clear all history entries
#[tauri::command]
pub async fn clear_history(history: State<'_, HistoryStorage>) -> CommandResult<()> {
    history.clear().map_err(CommandError::from)
}
