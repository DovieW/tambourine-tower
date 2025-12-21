use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

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

    fn path_for_id(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.wav", id))
    }

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

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn directory(&self) -> &Path {
        &self.dir
    }
}
