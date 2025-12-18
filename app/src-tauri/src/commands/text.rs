use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

/// Delay after clipboard operations to ensure system stability
const CLIPBOARD_STABILIZATION_DELAY_MS: u64 = 50;

/// Delay between keyboard key press and release events
const KEY_EVENT_DELAY_MS: u64 = 50;

/// Delay before restoring previous clipboard content
const CLIPBOARD_RESTORE_DELAY_MS: u64 = 100;

/// Delay between keystrokes when typing character by character
const KEYSTROKE_DELAY_MS: u64 = 10;

const SERVER_URL: &str = "http://127.0.0.1:8765";

/// Output mode for transcribed text
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum OutputMode {
    /// Copy to clipboard and simulate Ctrl+V/Cmd+V, then restore clipboard
    #[default]
    AutoPaste,
    /// Just copy to clipboard (no paste)
    Clipboard,
    /// Type each character as keystrokes
    Keystrokes,
}

impl OutputMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "clipboard" => OutputMode::Clipboard,
            "keystrokes" => OutputMode::Keystrokes,
            _ => OutputMode::AutoPaste,
        }
    }
}

#[tauri::command]
pub async fn get_server_url() -> String {
    SERVER_URL.to_string()
}

#[tauri::command]
pub async fn type_text(app: AppHandle, text: String) -> Result<(), String> {
    // macOS HIToolbox APIs (used by enigo) must run on the main thread
    // Use a channel to get the result back from the main thread
    let (tx, rx) = mpsc::channel::<Result<(), String>>();

    app.run_on_main_thread(move || {
        let result = type_text_blocking(&text);
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;

    // Wait for result from main thread
    rx.recv().map_err(|e| e.to_string())?
}

/// Output text based on the specified mode
pub fn output_text_with_mode(text: &str, mode: OutputMode) -> Result<(), String> {
    match mode {
        OutputMode::AutoPaste => type_text_blocking(text),
        OutputMode::Clipboard => copy_to_clipboard(text),
        OutputMode::Keystrokes => type_as_keystrokes(text),
    }
}

/// Copy text to clipboard only (no paste)
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())?;
    log::info!("Copied {} chars to clipboard", text.len());
    Ok(())
}

/// Type text character by character as keystrokes
pub fn type_as_keystrokes(text: &str) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;

    for c in text.chars() {
        // Handle special characters
        match c {
            '\n' => {
                enigo.key(Key::Return, Direction::Click).map_err(|e| e.to_string())?;
            }
            '\t' => {
                enigo.key(Key::Tab, Direction::Click).map_err(|e| e.to_string())?;
            }
            _ => {
                enigo.key(Key::Unicode(c), Direction::Click).map_err(|e| e.to_string())?;
            }
        }
        thread::sleep(Duration::from_millis(KEYSTROKE_DELAY_MS));
    }

    log::info!("Typed {} chars as keystrokes", text.len());
    Ok(())
}

/// Type text using clipboard and paste. Used internally by shortcut handlers.
pub fn type_text_blocking(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;

    // Save previous clipboard content
    let previous = clipboard.get_text().unwrap_or_default();

    // Set new text
    clipboard.set_text(text).map_err(|e| e.to_string())?;

    // Small delay for clipboard to stabilize
    thread::sleep(Duration::from_millis(CLIPBOARD_STABILIZATION_DELAY_MS));

    // Simulate Ctrl+V / Cmd+V
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    let modifier = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| e.to_string())?;

    // Restore previous clipboard after a delay
    thread::sleep(Duration::from_millis(CLIPBOARD_RESTORE_DELAY_MS));
    let _ = clipboard.set_text(&previous);

    Ok(())
}
