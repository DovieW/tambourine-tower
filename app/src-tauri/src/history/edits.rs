//! Per-entry corrections owned by HistoryStorage. Originals never change when
//! editing. A single revision-checked durable write replaces only this entry.
use super::{HistoryEntry, HistoryStatus, HistoryStorage};
use crate::fs::Fs;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEdit {
    pub revision: u64,
    pub title: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HistoryEditInput {
    pub id: String,
    pub expected_revision: u64,
    pub title: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HistoryDetail {
    pub entry: HistoryEntry,
    pub original_text: String,
    pub revision: u64,
    pub edited: bool,
    /// Preserve an unreadable correction file rather than overwriting it.
    pub edit_error: Option<String>,
}

#[derive(Default)]
pub(super) struct EditStore {
    values: HashMap<String, HistoryEdit>,
    unreadable: HashSet<String>,
}

fn filename(id: &str) -> String {
    format!("{:x}.json", Sha256::digest(id.as_bytes()))
}

fn directory(history_path: &Path) -> PathBuf {
    history_path.with_file_name("history-edits")
}

impl EditStore {
    pub fn load(fs: &dyn Fs, path: &Path, entries: &[HistoryEntry], revision_floor: u64) -> Self {
        let mut store = Self::default();
        for entry in entries {
            let path = directory(path).join(filename(&entry.id));
            let backup = path.with_extension("json.bak");
            // Recover the last durable revision if a process stopped mid-replace.
            if !fs.exists(&path) && fs.exists(&backup) {
                let _ = fs.rename(&backup, &path);
            }
            if !fs.exists(&path) {
                continue;
            }
            if !fs
                .metadata(&path)
                .is_ok_and(|m| m.len() <= 32 * 1024 * 1024)
            {
                store.unreadable.insert(entry.id.clone());
                continue;
            }
            match fs
                .read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<HistoryEdit>(&bytes).ok())
                .filter(|e| {
                    e.revision <= 9_007_199_254_740_991
                        && e.title.as_ref().is_none_or(|t| t.chars().count() <= 200)
                        && e.text.as_ref().is_none_or(|t| t.len() <= 16 * 1024 * 1024)
                }) {
                Some(edit) if edit.revision >= revision_floor => {
                    store.values.insert(entry.id.clone(), edit);
                }
                Some(_) => { /* Invalidated by a durable privacy clear. */ }
                None => {
                    store.unreadable.insert(entry.id.clone());
                }
            }
        }
        store
    }

    pub(super) fn max_revision(&self) -> u64 {
        self.values.values().map(|e| e.revision).max().unwrap_or(0)
    }

    pub(super) fn has_correction(&self, id: &str) -> bool {
        self.values
            .get(id)
            .is_some_and(|e| e.title.is_some() || e.text.is_some())
    }

    pub fn text<'a>(&'a self, entry: &'a HistoryEntry) -> &'a str {
        self.values
            .get(&entry.id)
            .and_then(|e| e.text.as_deref())
            .unwrap_or(&entry.text)
    }

    pub fn title<'a>(&'a self, entry: &'a HistoryEntry) -> Option<&'a str> {
        self.values
            .get(&entry.id)
            .and_then(|e| e.title.as_deref())
            .or(entry.title.as_deref())
    }

    pub fn apply(&self, mut entry: HistoryEntry) -> HistoryEntry {
        if let Some(edit) = self.values.get(&entry.id) {
            if let Some(text) = &edit.text {
                entry.text.clone_from(text);
            }
            if let Some(title) = &edit.title {
                entry.title = Some(title.clone());
            }
        }
        entry
    }
}

impl HistoryStorage {
    pub fn set_recording_duration(&self, id: &str, duration: f64) -> Result<(), String> {
        if !duration.is_finite() || duration < 0.0 {
            return Ok(());
        }
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        {
            let mut data = self.data.write().map_err(|_| "History unavailable")?;
            let Some(entry) = data.entries.iter_mut().find(|e| e.id == id) else {
                return Ok(());
            };
            if entry.duration_seconds == Some(duration) {
                return Ok(());
            }
            entry.duration_seconds = Some(duration);
        }
        self.save()
    }
    pub fn set_recording_details(
        &self,
        id: &str,
        mode: crate::recordings::options::RecordingMode,
        duration: Option<f64>,
        segments: Vec<crate::stt::SpeakerSegment>,
        original_stt_text: Option<String>,
    ) -> Result<(), String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        {
            let mut data = self.data.write().map_err(|_| "History unavailable")?;
            let entry = data
                .entries
                .iter_mut()
                .find(|e| e.id == id)
                .ok_or("History item missing")?;
            entry.recording_mode = Some(mode);
            entry.duration_seconds = duration.filter(|d| d.is_finite() && *d >= 0.0);
            entry.speaker_segments = segments;
            entry.original_stt_text = original_stt_text;
        }
        self.save()
    }
    pub fn detail(&self, id: &str) -> Result<Option<HistoryDetail>, String> {
        let data = self.data.read().map_err(|_| "History unavailable")?;
        let edits = self.edits.read().map_err(|_| "History edits unavailable")?;
        Ok(data.entries.iter().find(|e| e.id == id).map(|original| {
            let edit = edits.values.get(id);
            HistoryDetail {
                entry: edits.apply(original.clone()),
                original_text: original.text.clone(),
                revision: edit.map_or(data.edit_revision_floor, |e| e.revision),
                edited: edit.is_some_and(|e| e.text.is_some() || e.title.is_some()),
                edit_error: edits.unreadable.contains(id).then(|| {
                    "Saved corrections could not be read. The file has been preserved.".into()
                }),
            }
        }))
    }

    /// Null text/title restores the original value. Revision numbers are never
    /// reset (including Restore original), so delayed saves cannot undo a restore.
    pub fn save_edit(&self, input: HistoryEditInput) -> Result<HistoryEdit, String> {
        let _mutation = self.mutation.lock().map_err(|_| "History unavailable")?;
        let data = self.data.read().map_err(|_| "History unavailable")?;
        let original = data
            .entries
            .iter()
            .find(|e| e.id == input.id)
            .ok_or("This History item no longer exists")?;
        if original.status == HistoryStatus::InProgress {
            return Err("Wait for transcription to finish before editing".into());
        }
        if input
            .title
            .as_ref()
            .is_some_and(|s| s.chars().count() > 200)
            || input
                .text
                .as_ref()
                .is_some_and(|s| s.len() > 16 * 1024 * 1024)
        {
            return Err("The document exceeds the local editing limit".into());
        }
        let mut edits = self
            .edits
            .write()
            .map_err(|_| "History edits unavailable")?;
        if edits.unreadable.contains(&input.id) {
            return Err(
                "Saved corrections could not be read; they have not been overwritten".into(),
            );
        }
        let revision = edits
            .values
            .get(&input.id)
            .map_or(data.edit_revision_floor, |e| e.revision);
        if revision != input.expected_revision {
            return Err(
                "HISTORY_EDIT_CONFLICT: A newer correction was saved. Your draft has been kept."
                    .into(),
            );
        }
        let next = HistoryEdit {
            revision: revision
                .checked_add(1)
                .filter(|n| *n <= 9_007_199_254_740_991)
                .ok_or("Document revision limit reached")?,
            title: input
                .title
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty() && Some(s) != original.title.as_ref()),
            text: input.text.filter(|s| s != &original.text),
        };
        let bytes = serde_json::to_vec(&next).map_err(|_| "Could not prepare corrections")?;
        self.atomic_write_json(
            &directory(&self.file_path).join(filename(&input.id)),
            &bytes,
        )?;
        edits.values.insert(input.id, next.clone());
        Ok(next)
    }

    /// Called under the mutation lock after a durable base snapshot. Handles
    /// retention, bulk deletion, and crash leftovers without relying on the UI.
    pub(super) fn cleanup_edits(&self, entries: &[HistoryEntry]) -> Result<(), String> {
        let keep: HashSet<_> = entries.iter().map(|e| filename(&e.id)).collect();
        let dir = directory(&self.file_path);
        if self.fs.exists(&dir) {
            for path in self
                .fs
                .read_dir(&dir)
                .map_err(|_| "Could not inspect History corrections")?
            {
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Only our own hashed filenames, never arbitrary files/directories.
                if name
                    .get(..64)
                    .is_some_and(|prefix| prefix.bytes().all(|b| b.is_ascii_hexdigit()))
                    && name.get(64..).is_some_and(|suffix| {
                        suffix == ".json"
                            || suffix == ".json.bak"
                            || suffix.starts_with(".json.tmp.")
                    })
                    && !keep.contains(name)
                {
                    self.fs
                        .remove_file(&path)
                        .map_err(|_| "Could not clean up History corrections")?;
                }
            }
        }
        let ids: HashSet<_> = entries.iter().map(|e| e.id.as_str()).collect();
        let mut edits = self
            .edits
            .write()
            .map_err(|_| "History edits unavailable")?;
        edits.values.retain(|id, _| ids.contains(id.as_str()));
        edits.unreadable.retain(|id| ids.contains(id.as_str()));
        Ok(())
    }

    pub(super) fn edits_size_bytes(&self) -> u64 {
        self.fs
            .read_dir(&directory(&self.file_path))
            .unwrap_or_default()
            .iter()
            .filter_map(|p| self.fs.metadata(p).ok())
            .map(|m| m.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::HistoryPageQuery;

    #[test]
    fn revisions_restore_search_and_cleanup_do_not_rewrite_original() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStorage::new(temp.path().to_owned());
        let entry = store.add_entry("original".into(), None).unwrap();
        let original_file = std::fs::read(temp.path().join("history.json")).unwrap();
        let save = |revision, text| {
            store.save_edit(HistoryEditInput {
                id: entry.id.clone(),
                expected_revision: revision,
                title: Some("Meeting notes".into()),
                text,
            })
        };
        assert_eq!(save(0, Some("corrected".into())).unwrap().revision, 1);
        assert!(save(0, Some("stale".into()))
            .unwrap_err()
            .contains("CONFLICT"));
        assert_eq!(
            std::fs::read(temp.path().join("history.json")).unwrap(),
            original_file
        );
        assert_eq!(
            store.get_by_id(&entry.id).unwrap().unwrap().text,
            "corrected"
        );
        let detail = store.detail(&entry.id).unwrap().unwrap();
        assert_eq!(detail.original_text, "original");
        assert!(detail.edited);
        let page = store
            .query_page(
                serde_json::from_str::<HistoryPageQuery>(r#"{"filterText":"corrected"}"#).unwrap(),
            )
            .unwrap();
        assert_eq!(page.total_filtered, 1);
        assert_eq!(save(1, None).unwrap().revision, 2);
        assert_eq!(store.get_all(None).unwrap()[0].text, "original");
        let reopened = HistoryStorage::new(temp.path().to_owned());
        assert_eq!(reopened.detail(&entry.id).unwrap().unwrap().revision, 2);
        store.delete(&entry.id).unwrap();
        assert!(save(2, Some("resurrect".into())).is_err());
        assert_eq!(
            std::fs::read_dir(directory(&store.file_path))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn old_interrupted_snapshot_recovers_and_metadata_is_removed_with_text() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStorage::new(temp.path().to_owned());
        let entry = store.add_entry(String::new(), None).unwrap();
        store
            .set_recording_details(
                &entry.id,
                crate::recordings::options::RecordingMode::Meeting,
                Some(60.0),
                vec![],
                Some("Original STT".into()),
            )
            .unwrap();
        let path = temp.path().join("history.json");
        std::fs::rename(&path, path.with_extension("json.bak")).unwrap();
        let recovered = HistoryStorage::new(temp.path().to_owned());
        assert_eq!(recovered.get_all(None).unwrap().len(), 1);
        recovered
            .clear_all_transcript_text_keep_recordings()
            .unwrap();
        let reopened = HistoryStorage::new(temp.path().to_owned());
        assert!(reopened
            .get_by_id(&entry.id)
            .unwrap()
            .unwrap()
            .original_stt_text
            .is_none());
    }

    #[test]
    fn previews_are_bounded_and_failed_write_keeps_revision() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStorage::new(temp.path().to_owned());
        let entry = store.add_entry("語".repeat(10000), None).unwrap();
        let page = store
            .query_page(serde_json::from_str("{}").unwrap())
            .unwrap();
        assert_eq!(page.items[0].text.chars().count(), 320);
        std::fs::write(directory(&store.file_path), b"not a directory").unwrap();
        assert!(store
            .save_edit(HistoryEditInput {
                id: entry.id.clone(),
                expected_revision: 0,
                title: None,
                text: Some("draft".into())
            })
            .is_err());
        assert_eq!(store.detail(&entry.id).unwrap().unwrap().revision, 0);
        assert_eq!(store.get_all(None).unwrap()[0].text, entry.text);
    }

    #[test]
    fn privacy_clear_and_retention_cannot_resurrect_old_corrections() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStorage::new(temp.path().to_owned());
        let entry = store.add_entry("original".into(), None).unwrap();
        let input = |revision| HistoryEditInput {
            id: entry.id.clone(),
            expected_revision: revision,
            title: None,
            text: Some("private correction".into()),
        };
        store.save_edit(input(0)).unwrap();
        let sidecar = directory(&store.file_path).join(filename(&entry.id));
        let before_clear = std::fs::read(&sidecar).unwrap();
        store.clear_all_transcript_text_keep_recordings().unwrap();
        assert!(!sidecar.exists());
        assert!(store.save_edit(input(0)).unwrap_err().contains("CONFLICT"));
        assert!(store.save_edit(input(1)).unwrap_err().contains("CONFLICT"));
        // Simulate a crash after the clear snapshot but before sidecar cleanup.
        std::fs::write(&sidecar, before_clear).unwrap();
        let reopened = HistoryStorage::new(temp.path().to_owned());
        let detail = reopened.detail(&entry.id).unwrap().unwrap();
        assert!(detail.entry.text.is_empty());
        assert_eq!(detail.revision, 2);
        reopened.save_edit(input(2)).unwrap();
        reopened.add_entry("new".into(), Some(1)).unwrap();
        assert!(!sidecar.exists());
        assert!(reopened.save_edit(input(3)).is_err());
    }

    #[test]
    fn failed_final_save_does_not_report_success_to_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let store = HistoryStorage::new(temp.path().to_owned());
        store
            .add_request_entry("request".into(), Default::default(), None)
            .unwrap();
        std::fs::rename(&store.file_path, temp.path().join("before.json")).unwrap();
        std::fs::create_dir(&store.file_path).unwrap();
        assert!(store
            .complete_request_success("request", "transcript".into())
            .is_err());
        assert_eq!(
            store.get_by_id("request").unwrap().unwrap().status,
            HistoryStatus::InProgress
        );
    }
}
