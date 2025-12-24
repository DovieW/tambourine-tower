use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct RecordingsStats {
    pub count: u64,
    pub bytes: u64,
}

/// Simple on-disk store for WAV recordings keyed by request id.
///
/// Files are stored under `<app_data_dir>/recordings/<id>.wav`.
#[derive(Debug)]
pub struct RecordingStore {
    dir: PathBuf,
    // Keep a tiny in-memory cache of existence checks to avoid repeated fs hits.
    // This is best-effort; correctness still relies on the filesystem.
    known_existing: RwLock<std::collections::HashSet<String>>,
}

impl RecordingStore {
    pub fn new(app_data_dir: PathBuf) -> Self {
        let dir = app_data_dir.join("recordings");
        let _ = fs::create_dir_all(&dir);
        Self {
            dir,
            known_existing: RwLock::new(std::collections::HashSet::new()),
        }
    }

    fn is_safe_request_id(id: &str) -> bool {
        // Request ids are expected to be UUID-like strings.
        // We keep this conservative to prevent path traversal / weird filenames.
        !id.trim().is_empty()
            && id
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_'))
    }

    fn path_for_id(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.wav", id))
    }

    /// Returns the absolute WAV path for a given request id if it exists on disk.
    ///
    /// This is intended for frontend playback via `convertFileSrc`.
    pub fn wav_path_if_exists(&self, id: &str) -> Result<Option<PathBuf>, String> {
        if !Self::is_safe_request_id(id) {
            return Err("Invalid request id".to_string());
        }

        if let Ok(known) = self.known_existing.read() {
            if known.contains(id) {
                let p = self.path_for_id(id);
                return Ok(if p.exists() { Some(p) } else { None });
            }
        }

        let path = self.path_for_id(id);
        if path.exists() {
            if let Ok(mut known) = self.known_existing.write() {
                known.insert(id.to_string());
            }
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has(&self, id: &str) -> bool {
        if let Ok(known) = self.known_existing.read() {
            if known.contains(id) {
                return true;
            }
        }
        self.path_for_id(id).exists()
    }

    pub fn save_wav(&self, id: &str, wav_bytes: &[u8]) -> Result<(), String> {
        if id.trim().is_empty() {
            return Err("Cannot save recording: empty id".to_string());
        }
        if wav_bytes.is_empty() {
            return Err("Cannot save recording: empty audio".to_string());
        }

        let path = self.path_for_id(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create recordings dir: {}", e))?;
        }

        fs::write(&path, wav_bytes).map_err(|e| format!("Failed to write recording {}: {}", path.display(), e))?;

        if let Ok(mut known) = self.known_existing.write() {
            known.insert(id.to_string());
        }

        Ok(())
    }

    pub fn load_wav(&self, id: &str) -> Result<Vec<u8>, String> {
        let path = self.path_for_id(id);
        fs::read(&path).map_err(|e| format!("Failed to read recording {}: {}", path.display(), e))
    }

    /// Returns total size (in bytes) of all files in the recordings directory.
    ///
    /// Best-effort: skips individual files it cannot stat.
    pub fn total_size_bytes(&self) -> Result<u64, String> {
        let mut total: u64 = 0;
        let entries = fs::read_dir(&self.dir)
            .map_err(|e| format!("Failed to read recordings dir {}: {}", self.dir.display(), e))?;

        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            total = total.saturating_add(meta.len());
        }

        Ok(total)
    }

    /// Returns basic stats about saved recordings.
    ///
    /// - `count`: number of `.wav` files in the recordings directory
    /// - `bytes`: total size (in bytes) of those `.wav` files
    ///
    /// Best-effort: skips files it can't stat.
    pub fn stats(&self) -> Result<RecordingsStats, String> {
        let mut count: u64 = 0;
        let mut bytes: u64 = 0;

        let entries = fs::read_dir(&self.dir)
            .map_err(|e| format!("Failed to read recordings dir {}: {}", self.dir.display(), e))?;

        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase()
                != "wav"
            {
                continue;
            }

            let Ok(meta) = entry.metadata() else {
                continue;
            };

            count = count.saturating_add(1);
            bytes = bytes.saturating_add(meta.len());
        }

        Ok(RecordingsStats { count, bytes })
    }

    /// Prune old recordings to keep at most `max_keep` files.
    ///
    /// Oldest is determined by filesystem modified time.
    /// Best-effort: skips files it can't stat, continues on individual delete errors.
    pub fn prune_to_max_files(&self, max_keep: usize) -> Result<usize, String> {
        if max_keep == 0 {
            return Ok(0);
        }

        let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
        let entries = fs::read_dir(&self.dir)
            .map_err(|e| format!("Failed to read recordings dir {}: {}", self.dir.display(), e))?;

        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Only manage .wav files (be conservative).
            if path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase() != "wav" {
                continue;
            }

            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((path, modified));
        }

        if files.len() <= max_keep {
            return Ok(0);
        }

        // Oldest first.
        files.sort_by_key(|(_, modified)| *modified);
        let delete_count = files.len() - max_keep;

        let mut deleted = 0usize;
        for (path, _) in files.into_iter().take(delete_count) {
            // Best-effort delete.
            if fs::remove_file(&path).is_ok() {
                deleted += 1;

                // Keep existence cache best-effort in sync.
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(mut known) = self.known_existing.write() {
                        known.remove(stem);
                    }
                }
            }
        }

        Ok(deleted)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn directory(&self) -> &Path {
        &self.dir
    }
}
