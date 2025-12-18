use tauri::{AppHandle, Manager};

#[tauri::command]
pub async fn resize_overlay(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    // Enforce minimum dimensions to prevent invisible window
    let min_size = 48.0;
    let width = width.max(min_size);
    let height = height.max(min_size);

    if let Some(window) = app.get_webview_window("overlay") {
        // Get current center point from current position and size
        // This allows the overlay to be dragged and maintain its new position
        let center = if let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) {
            let scale = window.scale_factor().unwrap_or(1.0);
            let x = pos.x as f64 / scale;
            let y = pos.y as f64 / scale;
            let w = size.width as f64 / scale;
            let h = size.height as f64 / scale;
            Some((x + w / 2.0, y + h / 2.0))
        } else {
            None
        };

        // Set the new size
        window
            .set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }))
            .map_err(|e| e.to_string())?;

        // Reposition to keep center fixed
        if let Some((cx, cy)) = center {
            let x = cx - width / 2.0;
            let y = cy - height / 2.0;
            window
                .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn show_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("overlay") {
        window.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn hide_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("overlay") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Set overlay mode: "always", "never", or "recording_only"
#[tauri::command]
pub async fn set_overlay_mode(app: AppHandle, mode: String) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("overlay") {
        match mode.as_str() {
            "always" => {
                window.show().map_err(|e| e.to_string())?;
            }
            "never" => {
                window.hide().map_err(|e| e.to_string())?;
            }
            "recording_only" => {
                // Hide initially, will be shown when recording starts
                window.hide().map_err(|e| e.to_string())?;
            }
            _ => {
                return Err(format!("Invalid overlay mode: {}", mode));
            }
        }
    }
    Ok(())
}
