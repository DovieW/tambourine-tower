use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::sync::{Mutex, OnceLock};
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

const SERVER_URL: &str = "http://127.0.0.1:8765";

/// Global lock to ensure we never run multiple output injections concurrently.
///
/// Without this, two overlapping "type/paste" operations can interleave key events and
/// produce dropped/mangled text in target applications.
static OUTPUT_INJECTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn output_injection_lock() -> &'static Mutex<()> {
    OUTPUT_INJECTION_LOCK.get_or_init(|| Mutex::new(()))
}

fn maybe_hit_enter(enigo: &mut Enigo, hit_enter: bool) -> Result<(), String> {
    if !hit_enter {
        return Ok(());
    }

    // Small delay to avoid racing the paste keystroke.
    thread::sleep(Duration::from_millis(KEY_EVENT_DELAY_MS));

    enigo
        .key(Key::Return, Direction::Click)
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Output mode for transcribed text
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum OutputMode {
    /// Copy to clipboard and simulate Ctrl+V/Cmd+V, then restore clipboard
    #[default]
    Paste,
    /// Paste and keep in clipboard (no restore)
    PasteAndClipboard,
    /// Just copy to clipboard (no paste)
    Clipboard,
    // NOTE: Keystrokes mode was removed/disabled due to reliability issues across targets.
}

impl OutputMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "paste" => OutputMode::Paste,
            "paste_and_clipboard" => OutputMode::PasteAndClipboard,
            "clipboard" => OutputMode::Clipboard,
            // Legacy/disabled values: map to paste so existing settings.json doesn't break.
            "keystrokes" => OutputMode::Paste,
            "keystrokes_and_clipboard" => OutputMode::Paste,
            // Handle legacy value
            "auto_paste" => OutputMode::Paste,
            _ => OutputMode::Paste,
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
        // Serialize output across all modes to avoid interleaving key events.
        let _guard = match output_injection_lock().lock() {
            Ok(g) => g,
            Err(_) => {
                let _ = tx.send(Err("Output lock poisoned".to_string()));
                return;
            }
        };

        let result = type_text_blocking(&text, false);
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;

    // Wait for result from main thread
    rx.recv().map_err(|e| e.to_string())?
}

/// Output text based on the specified mode
pub fn output_text_with_mode(text: &str, mode: OutputMode, hit_enter: bool) -> Result<(), String> {
    let _guard = output_injection_lock()
        .lock()
        .map_err(|_| "Output lock poisoned".to_string())?;

    match mode {
        OutputMode::Paste => type_text_blocking(text, hit_enter),
        OutputMode::PasteAndClipboard => paste_and_keep_clipboard(text, hit_enter),
        OutputMode::Clipboard => copy_to_clipboard(text),
    }
}

/// Copy text to clipboard and paste, keeping text in clipboard (no restore)
pub fn paste_and_keep_clipboard(text: &str, hit_enter: bool) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;

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

    maybe_hit_enter(&mut enigo, hit_enter)?;

    // Don't restore clipboard - keep the text there
    log::info!("Pasted {} chars (kept in clipboard)", text.len());
    Ok(())
}

/// Copy text to clipboard only (no paste)
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())?;
    log::info!("Copied {} chars to clipboard", text.len());
    Ok(())
}

// Keystrokes mode intentionally disabled.
// (Kept as a stub in case any legacy call sites remain in downstream forks.)
#[allow(dead_code)]
pub fn type_as_keystrokes(_text: &str) -> Result<(), String> {
    Err("Keystrokes output mode is disabled".to_string())
}

/// Type text using clipboard and paste. Used internally by shortcut handlers.
pub fn type_text_blocking(text: &str, hit_enter: bool) -> Result<(), String> {
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

    maybe_hit_enter(&mut enigo, hit_enter)?;

    // Restore previous clipboard after a delay
    thread::sleep(Duration::from_millis(CLIPBOARD_RESTORE_DELAY_MS));
    let _ = clipboard.set_text(&previous);

    Ok(())
}
