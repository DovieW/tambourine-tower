use crate::history::{HistoryEntry, HistoryStorage};
use tauri::{AppHandle, State};

#[cfg(desktop)]
use tauri_plugin_store::StoreExt;

fn get_max_saved_recordings(app: &AppHandle) -> usize {
    #[cfg(desktop)]
    {
        let default: u64 = 1000;
        let raw = app
            .store("settings.json")
            .ok()
            .and_then(|store| store.get("max_saved_recordings"))
            .and_then(|v| v.as_u64())
            .unwrap_or(default);

        // Be defensive: avoid runaway values if settings.json was edited.
        return (raw.clamp(1, 100_000)) as usize;
    }

    #[cfg(not(desktop))]
    {
        1000
    }
}

/// Add a new entry to the dictation history
#[tauri::command]
pub async fn add_history_entry(
    app: AppHandle,
    text: String,
    history: State<'_, HistoryStorage>,
) -> Result<HistoryEntry, String> {
    let max = get_max_saved_recordings(&app);
    history.add_entry(text, max)
}

/// Get dictation history entries
#[tauri::command]
pub async fn get_history(
    limit: Option<usize>,
    history: State<'_, HistoryStorage>,
) -> Result<Vec<HistoryEntry>, String> {
    history.get_all(limit)
}

/// Delete a history entry by ID
#[tauri::command]
pub async fn delete_history_entry(
    id: String,
    history: State<'_, HistoryStorage>,
) -> Result<bool, String> {
    history.delete(&id)
}

/// Clear all history entries
#[tauri::command]
pub async fn clear_history(history: State<'_, HistoryStorage>) -> Result<(), String> {
    history.clear()
}
