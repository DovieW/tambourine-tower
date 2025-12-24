use crate::audio::{self, AudioCue, SoundType};
use crate::audio_capture;
use std::thread;
use std::time::Duration;

/// Play the selected cue once as a short preview.
///
/// Frontend passes the cue string (e.g. "tangerine"). Unknown values fall back to Tangerine.
#[tauri::command]
pub async fn play_audio_cue_preview(cue: String) -> Result<(), String> {
    let cue = AudioCue::from_str(&cue);

    // Preview both sounds so it's obvious which pair will be used during real recording.
    log::info!("Previewing audio cue: {:?} (start then stop)", cue);

    // Run the preview sequence off-thread so we don't block the command handler.
    thread::spawn(move || {
        if let Err(e) = audio::play_sound_blocking(SoundType::RecordingStart, cue) {
            log::warn!("Failed to play preview start sound: {}", e);
            return;
        }

        // A small deliberate gap so users can clearly distinguish start vs stop.
        thread::sleep(Duration::from_millis(140));

        if let Err(e) = audio::play_sound_blocking(SoundType::RecordingStop, cue) {
            log::warn!("Failed to play preview stop sound: {}", e);
        }
    });

    Ok(())
}

/// List available audio input devices as seen by the backend (CPAL).
///
/// This is the authoritative device list for recording and the backend-driven overlay waveform.
#[tauri::command]
pub fn list_audio_input_devices() -> Vec<String> {
    audio_capture::list_input_devices()
}

/// Get the backend default audio input device name (CPAL default), if available.
#[tauri::command]
pub fn get_default_audio_input_device_name() -> Option<String> {
    audio_capture::get_default_input_device_info().map(|(name, _sr, _ch)| name)
}
