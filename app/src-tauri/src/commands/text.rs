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

const SERVER_URL: &str = "http://127.0.0.1:8765";

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
    /// Type each character as keystrokes
    Keystrokes,
    /// Type as keystrokes and also copy to clipboard
    KeystrokesAndClipboard,
}

impl OutputMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "paste" => OutputMode::Paste,
            "paste_and_clipboard" => OutputMode::PasteAndClipboard,
            "clipboard" => OutputMode::Clipboard,
            "keystrokes" => OutputMode::Keystrokes,
            "keystrokes_and_clipboard" => OutputMode::KeystrokesAndClipboard,
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
        OutputMode::Paste => type_text_blocking(text),
        OutputMode::PasteAndClipboard => paste_and_keep_clipboard(text),
        OutputMode::Clipboard => copy_to_clipboard(text),
        OutputMode::Keystrokes => type_as_keystrokes(text),
        OutputMode::KeystrokesAndClipboard => {
            copy_to_clipboard(text)?;
            type_as_keystrokes(text)
        }
    }
}

/// Copy text to clipboard and paste, keeping text in clipboard (no restore)
pub fn paste_and_keep_clipboard(text: &str) -> Result<(), String> {
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

/// Type text character by character as keystrokes
pub fn type_as_keystrokes(text: &str) -> Result<(), String> {
    // Wait for any modifier keys from the hotkey to be fully released.
    // This prevents typed characters from combining with Ctrl/Alt/etc.
    thread::sleep(Duration::from_millis(250));

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;

    // Best-effort: explicitly release common modifiers.
    // On some platforms the global shortcut key-up may arrive slightly late; releasing here
    // avoids "shortcut chaos" where characters are treated as Ctrl+... shortcuts.
    let _ = enigo.key(Key::Control, Direction::Release);
    let _ = enigo.key(Key::Alt, Direction::Release);
    let _ = enigo.key(Key::Shift, Direction::Release);
    let _ = enigo.key(Key::Meta, Direction::Release);

    // Throttle typing to avoid dropped characters in some targets (especially when repeatedly
    // triggering Output Last Transcription).
    const CHUNK_CHARS: usize = 24;
    const CHUNK_DELAY_MS: u64 = 18;

    let mut buf = String::with_capacity(CHUNK_CHARS * 2);
    let mut count = 0usize;
    for ch in text.chars() {
        buf.push(ch);
        count += 1;

        if count >= CHUNK_CHARS {
            enigo.text(&buf).map_err(|e| e.to_string())?;
            buf.clear();
            count = 0;
            thread::sleep(Duration::from_millis(CHUNK_DELAY_MS));
        }
    }

    if !buf.is_empty() {
        enigo.text(&buf).map_err(|e| e.to_string())?;
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
