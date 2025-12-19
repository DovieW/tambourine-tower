use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::llm::PromptSections;

#[cfg(desktop)]
use tauri_plugin_global_shortcut::Shortcut;

// ============================================================================
// DEFAULT HOTKEY CONSTANTS - Single source of truth for all default hotkeys
// ============================================================================

/// Default modifiers for all hotkeys
pub const DEFAULT_HOTKEY_MODIFIERS: &[&str] = &["ctrl", "alt"];

/// Default key for toggle recording (Ctrl+Alt+Space)
pub const DEFAULT_TOGGLE_KEY: &str = "Space";

/// Default key for hold-to-record (Ctrl+Alt+`)
pub const DEFAULT_HOLD_KEY: &str = "Backquote";

/// Default key for paste last transcription (Ctrl+Alt+.)
pub const DEFAULT_PASTE_LAST_KEY: &str = "Period";

// ============================================================================
// DEFAULT VAD SETTINGS - Voice Activity Detection
// ============================================================================

/// Default VAD enabled state
pub const DEFAULT_VAD_ENABLED: bool = false;

/// Default VAD auto-stop enabled state
pub const DEFAULT_VAD_AUTO_STOP: bool = false;

/// Default VAD aggressiveness level (0-3, higher = more aggressive filtering)
pub const DEFAULT_VAD_AGGRESSIVENESS: u8 = 2;

/// Default speech frames threshold before triggering speech start
pub const DEFAULT_VAD_SPEECH_FRAMES_THRESHOLD: u32 = 3;

/// Default hangover frames (silence frames before triggering speech end)
pub const DEFAULT_VAD_HANGOVER_FRAMES: u32 = 30;

/// Default pre-roll milliseconds to capture before speech is detected
pub const DEFAULT_VAD_PRE_ROLL_MS: u32 = 300;

// ============================================================================

/// Configuration for a hotkey combination
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotkeyConfig {
    /// Modifier keys (e.g., ["ctrl", "alt"])
    pub modifiers: Vec<String>,
    /// The main key (e.g., "Space")
    pub key: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            modifiers: DEFAULT_HOTKEY_MODIFIERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            key: DEFAULT_TOGGLE_KEY.to_string(),
        }
    }
}

impl HotkeyConfig {
    /// Create default toggle hotkey config
    pub fn default_toggle() -> Self {
        Self {
            modifiers: DEFAULT_HOTKEY_MODIFIERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            key: DEFAULT_TOGGLE_KEY.to_string(),
        }
    }

    /// Create default hold hotkey config
    pub fn default_hold() -> Self {
        Self {
            modifiers: DEFAULT_HOTKEY_MODIFIERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            key: DEFAULT_HOLD_KEY.to_string(),
        }
    }

    /// Create default paste-last hotkey config
    pub fn default_paste_last() -> Self {
        Self {
            modifiers: DEFAULT_HOTKEY_MODIFIERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            key: DEFAULT_PASTE_LAST_KEY.to_string(),
        }
    }

    /// Convert to shortcut string format like "ctrl+alt+Space"
    /// Note: modifiers must be lowercase for the parser to recognize them
    pub fn to_shortcut_string(&self) -> String {
        let mut parts: Vec<String> = self.modifiers.iter().map(|m| m.to_lowercase()).collect();
        parts.push(self.key.clone());
        parts.join("+")
    }

    /// Convert to a tauri Shortcut using FromStr parsing
    #[cfg(desktop)]
    pub fn to_shortcut(&self) -> Result<Shortcut, String> {
        let shortcut_str = self.to_shortcut_string();
        Shortcut::from_str(&shortcut_str)
            .map_err(|e| format!("Failed to parse shortcut '{}': {:?}", shortcut_str, e))
    }

    /// Convert to a tauri Shortcut, falling back to a default if parsing fails
    #[cfg(desktop)]
    pub fn to_shortcut_or_default(&self, default_fn: fn() -> Self) -> Shortcut {
        self.to_shortcut().unwrap_or_else(|_| {
            default_fn()
                .to_shortcut()
                .expect("Default hotkey must be valid")
        })
    }
}

/// Voice Activity Detection settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VadSettings {
    /// Enable VAD processing
    pub enabled: bool,
    /// Automatically stop recording when speech ends
    pub auto_stop: bool,
    /// VAD aggressiveness level (0-3)
    pub aggressiveness: u8,
    /// Consecutive speech frames before triggering speech start
    pub speech_frames_threshold: u32,
    /// Consecutive silence frames before triggering speech end
    pub hangover_frames: u32,
    /// Milliseconds of audio to capture before speech is detected
    pub pre_roll_ms: u32,
}

impl Default for VadSettings {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_VAD_ENABLED,
            auto_stop: DEFAULT_VAD_AUTO_STOP,
            aggressiveness: DEFAULT_VAD_AGGRESSIVENESS,
            speech_frames_threshold: DEFAULT_VAD_SPEECH_FRAMES_THRESHOLD,
            hangover_frames: DEFAULT_VAD_HANGOVER_FRAMES,
            pre_roll_ms: DEFAULT_VAD_PRE_ROLL_MS,
        }
    }
}

impl VadSettings {
    /// Convert to audio capture VAD config
    pub fn to_vad_auto_stop_config(&self) -> crate::audio_capture::VadAutoStopConfig {
        use crate::audio_capture::VadAutoStopConfig;
        use crate::vad::{VadAggressiveness, VadConfig};

        VadAutoStopConfig {
            enabled: self.enabled,
            auto_stop: self.auto_stop,
            vad_config: VadConfig {
                aggressiveness: match self.aggressiveness {
                    0 => VadAggressiveness::Quality,
                    1 => VadAggressiveness::LowBitrate,
                    2 => VadAggressiveness::Aggressive,
                    _ => VadAggressiveness::VeryAggressive,
                },
                speech_frames_threshold: self.speech_frames_threshold,
                hangover_frames: self.hangover_frames,
                pre_roll_ms: self.pre_roll_ms,
                frame_duration_ms: 30, // Fixed at 30ms for webrtc-vad
                sample_rate: 16000,    // Fixed at 16kHz for webrtc-vad
            },
        }
    }
}

// ============================================================================
// Rewrite prompt settings (stored in settings.json)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptSectionSetting {
    pub enabled: bool,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CleanupPromptSectionsSetting {
    /// Per-section overrides. If a section is None/missing, it inherits from the base prompts.
    #[serde(default)]
    pub main: Option<PromptSectionSetting>,
    #[serde(default)]
    pub advanced: Option<PromptSectionSetting>,
    #[serde(default)]
    pub dictionary: Option<PromptSectionSetting>,
}

impl CleanupPromptSectionsSetting {
    /// Apply these overrides on top of a base `PromptSections`.
    ///
    /// - Any missing section inherits from `base`.
    /// - `content: None` means "use built-in default prompt for that section".
    pub fn apply_to(&self, base: &PromptSections) -> PromptSections {
        let mut next = base.clone();

        if let Some(main) = &self.main {
            // Main section is always included; `enabled` is ignored.
            next.main_custom = main.content.clone();
        }

        if let Some(advanced) = &self.advanced {
            next.advanced_enabled = advanced.enabled;
            next.advanced_custom = advanced.content.clone();
        }

        if let Some(dictionary) = &self.dictionary {
            next.dictionary_enabled = dictionary.enabled;
            next.dictionary_custom = dictionary.content.clone();
        }

        next
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RewriteProgramPromptProfile {
    pub id: String,
    pub name: String,
    #[serde(
        default,
        alias = "program_path",
        deserialize_with = "deserialize_program_paths"
    )]
    pub program_paths: Vec<String>,
    pub cleanup_prompt_sections: Option<CleanupPromptSectionsSetting>,

    /// Optional per-profile gate for the rewrite step (falls back to global setting)
    #[serde(default)]
    pub rewrite_llm_enabled: Option<bool>,

    #[serde(default)]
    pub stt_provider: Option<String>,
    #[serde(default)]
    pub stt_model: Option<String>,
    #[serde(default)]
    pub stt_timeout_seconds: Option<f64>,
    #[serde(default)]
    pub llm_provider: Option<String>,
    #[serde(default)]
    pub llm_model: Option<String>,
}

fn deserialize_program_paths<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    let value = Option::<OneOrMany>::deserialize(deserializer)?;
    let paths = match value {
        None => Vec::new(),
        Some(OneOrMany::One(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![trimmed.to_string()]
            }
        }
        Some(OneOrMany::Many(v)) => v
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    };

    Ok(paths)
}
