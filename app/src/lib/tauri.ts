import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Store } from "@tauri-apps/plugin-store";
import { z } from "zod";
import { normalizeHexColor } from "./accentColor";

/**
 * Connection state for UI display (maps from pipeline state)
 */
export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "idle"
  | "recording"
  | "processing";

interface TypeTextResult {
  success: boolean;
  error?: string;
}

export interface HotkeyConfig {
  modifiers: string[];
  key: string;
}

// Zod schema for HotkeyConfig validation
export const HotkeyConfigSchema = z.object({
  modifiers: z.array(z.string()),
  key: z.string().min(1, "Key is required"),
});

interface HistoryEntry {
  id: string;
  timestamp: string;
  text: string;
  status?: "in_progress" | "success" | "error";
  error_message?: string | null;
  stt_provider?: string | null;
  stt_model?: string | null;
  llm_provider?: string | null;
  llm_model?: string | null;
}

export interface PromptSection {
  enabled: boolean;
  content: string | null;
}

export interface CleanupPromptSections {
  main: PromptSection;
  advanced: PromptSection;
  dictionary: PromptSection;
}

// Per-profile prompt overrides: each section can be omitted/null to inherit from Default.
export interface CleanupPromptSectionsOverride {
  main?: PromptSection | null;
  advanced?: PromptSection | null;
  dictionary?: PromptSection | null;
}

export interface RewriteProgramPromptProfile {
  id: string;
  name: string;
  program_paths: string[];
  cleanup_prompt_sections: CleanupPromptSectionsOverride | null;

  // Per-profile gate for the optional LLM rewrite step (falls back to AppSettings.rewrite_llm_enabled)
  rewrite_llm_enabled?: boolean | null;

  // Per-profile overrides for the pipeline
  stt_provider?: string | null;
  stt_model?: string | null;
  stt_timeout_seconds?: number | null;
  llm_provider?: string | null;
  llm_model?: string | null;

  // Per-profile overrides for UI (Option 1: override-or-inherit)
  // NOTE: These are persisted in settings.json as part of the profile object.
  // The backend may ignore them until it is updated to apply them at runtime.
  sound_enabled?: boolean | null;
  playing_audio_handling?: PlayingAudioHandling | null;
  overlay_mode?: OverlayMode | null;
  widget_position?: WidgetPosition | null;
  output_mode?: OutputMode | null;

  // After paste, optionally press Enter.
  // (May be ignored by backend until runtime/profile routing supports it.)
  output_hit_enter?: boolean | null;
}

export type PlayingAudioHandling = "none" | "mute" | "pause" | "mute_and_pause";

export type AudioCue = "tangerine" | "maraca" | "clave" | "tambourine";

export type OverlayMode = "always" | "never" | "recording_only";

export type WidgetPosition =
  | "center"
  | "top-left"
  | "top-center"
  | "top-right"
  | "bottom-left"
  | "bottom-center"
  | "bottom-right";

export type OutputMode = "paste" | "paste_and_clipboard" | "clipboard";

function normalizeOutputMode(value: unknown): OutputMode {
  if (
    value === "paste" ||
    value === "paste_and_clipboard" ||
    value === "clipboard"
  ) {
    return value;
  }

  // Legacy/disabled values:
  // - "keystrokes"
  // - "keystrokes_and_clipboard"
  // - "auto_paste"
  return "paste";
}

export interface AppSettings {
  toggle_hotkey: HotkeyConfig;
  hold_hotkey: HotkeyConfig;
  paste_last_hotkey: HotkeyConfig;
  selected_mic_id: string | null;
  sound_enabled: boolean;
  audio_cue: AudioCue;
  /** Optional user override; null/undefined means use default Tangerine accent */
  accent_color: string | null;
  // Global gate for the optional LLM rewrite step
  rewrite_llm_enabled: boolean;
  cleanup_prompt_sections: CleanupPromptSections | null;
  rewrite_program_prompt_profiles: RewriteProgramPromptProfile[];
  stt_provider: string | null;
  stt_model: string | null;
  // Global STT prompt (applies to all transcriptions when supported by the selected provider/model)
  stt_transcription_prompt: string | null;
  llm_provider: string | null;
  llm_model: string | null;
  playing_audio_handling: PlayingAudioHandling;
  stt_timeout_seconds: number | null;
  overlay_mode: OverlayMode;
  widget_position: WidgetPosition;
  output_mode: OutputMode;
  output_hit_enter: boolean;

  // Hallucination protection (quiet-audio gate)
  quiet_audio_gate_enabled: boolean;
  quiet_audio_min_duration_secs: number;
  quiet_audio_rms_dbfs_threshold: number;
  quiet_audio_peak_dbfs_threshold: number;

  // Experimental: noise gate (0 = off, 100 = strongest)
  noise_gate_strength: number;
}

function normalizePlayingAudioHandling(value: unknown): PlayingAudioHandling {
  if (
    value === "none" ||
    value === "mute" ||
    value === "pause" ||
    value === "mute_and_pause"
  ) {
    return value;
  }

  // Legacy boolean (auto_mute_audio) migration:
  // - true  => mute
  // - false => none
  if (typeof value === "boolean") {
    return value ? "mute" : "none";
  }

  // Default for fresh installs / missing setting
  return "mute";
}

function normalizeAudioCue(value: unknown): AudioCue {
  if (
    value === "tangerine" ||
    value === "maraca" ||
    value === "clave" ||
    value === "tambourine"
  ) {
    return value;
  }

  // Default for fresh installs / missing setting
  return "tangerine";
}

function normalizeNoiseGateStrength(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 0;
  const rounded = Math.round(value);
  return Math.min(100, Math.max(0, rounded));
}

// ============================================================================
// Default values - must match Rust defaults
// ============================================================================

const DEFAULT_HOTKEY_MODIFIERS = ["ctrl", "alt"];

export const defaultToggleHotkey: HotkeyConfig = {
  modifiers: DEFAULT_HOTKEY_MODIFIERS,
  key: "Space",
};

export const defaultHoldHotkey: HotkeyConfig = {
  modifiers: DEFAULT_HOTKEY_MODIFIERS,
  key: "Backquote",
};

export const defaultPasteLastHotkey: HotkeyConfig = {
  modifiers: DEFAULT_HOTKEY_MODIFIERS,
  key: "Period",
};

// ============================================================================
// Store helpers
// ============================================================================

let storeInstance: Store | null = null;

async function getStore(): Promise<Store> {
  if (!storeInstance) {
    storeInstance = await Store.load("settings.json");
  }
  return storeInstance;
}

// ============================================================================
// Hotkey validation helpers (Zod-based)
// ============================================================================

/**
 * Check if two hotkey configs are equivalent (case-insensitive comparison)
 */
export function hotkeyIsSameAs(a: HotkeyConfig, b: HotkeyConfig): boolean {
  if (a.key.toLowerCase() !== b.key.toLowerCase()) return false;
  if (a.modifiers.length !== b.modifiers.length) return false;
  return a.modifiers.every((mod) =>
    b.modifiers.some((other) => mod.toLowerCase() === other.toLowerCase())
  );
}

type HotkeyType = "toggle" | "hold" | "paste_last";

const HOTKEY_LABELS: Record<HotkeyType, string> = {
  toggle: "toggle",
  hold: "hold",
  paste_last: "paste last",
};

/**
 * Create a Zod schema for validating a hotkey doesn't conflict with existing hotkeys
 */
export function createHotkeyDuplicateSchema(
  allHotkeys: Record<HotkeyType, HotkeyConfig>,
  excludeType: HotkeyType
) {
  return HotkeyConfigSchema.superRefine((hotkey, ctx) => {
    for (const [type, existing] of Object.entries(allHotkeys)) {
      if (type !== excludeType && hotkeyIsSameAs(hotkey, existing)) {
        ctx.addIssue({
          code: "custom",
          message: `This shortcut is already used for the ${
            HOTKEY_LABELS[type as HotkeyType]
          } hotkey`,
        });
        return;
      }
    }
  });
}

/**
 * Validate that a hotkey doesn't conflict with other hotkeys
 * Returns error message if invalid, null if valid
 */
export function validateHotkeyNotDuplicate(
  newHotkey: HotkeyConfig,
  allHotkeys: {
    toggle: HotkeyConfig;
    hold: HotkeyConfig;
    paste_last: HotkeyConfig;
  },
  excludeType: HotkeyType
): string | null {
  const schema = createHotkeyDuplicateSchema(allHotkeys, excludeType);
  const result = schema.safeParse(newHotkey);
  if (!result.success) {
    return result.error.issues[0]?.message ?? "Invalid hotkey";
  }
  return null;
}

// ============================================================================
// Tauri API
// ============================================================================

export const tauriAPI = {
  async typeText(text: string): Promise<TypeTextResult> {
    try {
      await invoke("type_text", { text });
      return { success: true };
    } catch (error) {
      return { success: false, error: String(error) };
    }
  },

  async onStartRecording(callback: () => void): Promise<UnlistenFn> {
    return listen("recording-start", callback);
  },

  async onStopRecording(callback: () => void): Promise<UnlistenFn> {
    return listen("recording-stop", callback);
  },

  // Settings API - using store plugin directly
  async getSettings(): Promise<AppSettings> {
    const store = await getStore();

    const normalizePromptSection = (value: any): PromptSection | null => {
      if (value === null) return null;
      if (!value || typeof value !== "object") return null;

      const enabled =
        typeof (value as any).enabled === "boolean"
          ? (value as any).enabled
          : true;
      const content =
        typeof (value as any).content === "string"
          ? (value as any).content
          : null;

      return { enabled, content };
    };

    const normalizeCleanupPromptSectionsOverride = (
      value: any
    ): CleanupPromptSectionsOverride | null => {
      if (value === null || value === undefined) return null;
      if (!value || typeof value !== "object") return null;

      const v = value as any;
      const out: CleanupPromptSectionsOverride = {};

      if (Object.prototype.hasOwnProperty.call(v, "main")) {
        out.main = normalizePromptSection(v.main);
      }
      if (Object.prototype.hasOwnProperty.call(v, "advanced")) {
        out.advanced = normalizePromptSection(v.advanced);
      }
      if (Object.prototype.hasOwnProperty.call(v, "dictionary")) {
        out.dictionary = normalizePromptSection(v.dictionary);
      }

      return out;
    };

    const normalizeRewriteProfile = (
      p: any
    ): RewriteProgramPromptProfile | null => {
      if (!p || typeof p !== "object") return null;
      const id = typeof p.id === "string" ? p.id : "";
      const name = typeof p.name === "string" ? p.name : "";

      const program_paths_raw = (p as any).program_paths;
      const legacy_program_path = (p as any).program_path;

      const program_paths = Array.isArray(program_paths_raw)
        ? program_paths_raw.filter((x) => typeof x === "string")
        : typeof legacy_program_path === "string" &&
          legacy_program_path.length > 0
        ? [legacy_program_path]
        : [];

      const cleanup_prompt_sections = normalizeCleanupPromptSectionsOverride(
        (p as any).cleanup_prompt_sections
      );
      const stt_provider =
        typeof (p as any).stt_provider === "string"
          ? (p as any).stt_provider
          : null;
      const stt_model =
        typeof (p as any).stt_model === "string" ? (p as any).stt_model : null;
      const stt_timeout_seconds =
        typeof (p as any).stt_timeout_seconds === "number"
          ? (p as any).stt_timeout_seconds
          : null;
      const llm_provider =
        typeof (p as any).llm_provider === "string"
          ? (p as any).llm_provider
          : null;
      const llm_model =
        typeof (p as any).llm_model === "string" ? (p as any).llm_model : null;
      const rewrite_llm_enabled =
        typeof (p as any).rewrite_llm_enabled === "boolean"
          ? (p as any).rewrite_llm_enabled
          : null;

      const sound_enabled =
        typeof (p as any).sound_enabled === "boolean"
          ? (p as any).sound_enabled
          : null;
      const playing_audio_handling_raw = (p as any).playing_audio_handling;
      const legacy_auto_mute_audio = (p as any).auto_mute_audio;

      const playing_audio_handling: PlayingAudioHandling | null =
        typeof playing_audio_handling_raw === "string"
          ? normalizePlayingAudioHandling(playing_audio_handling_raw)
          : typeof legacy_auto_mute_audio === "boolean"
          ? legacy_auto_mute_audio
            ? "mute"
            : "none"
          : null;

      const overlay_mode =
        (p as any).overlay_mode === "always" ||
        (p as any).overlay_mode === "never" ||
        (p as any).overlay_mode === "recording_only"
          ? ((p as any).overlay_mode as OverlayMode)
          : null;

      const widget_position =
        (p as any).widget_position === "center" ||
        (p as any).widget_position === "top-left" ||
        (p as any).widget_position === "top-center" ||
        (p as any).widget_position === "top-right" ||
        (p as any).widget_position === "bottom-left" ||
        (p as any).widget_position === "bottom-center" ||
        (p as any).widget_position === "bottom-right"
          ? ((p as any).widget_position as WidgetPosition)
          : null;

      const output_mode =
        typeof (p as any).output_mode === "string"
          ? normalizeOutputMode((p as any).output_mode)
          : null;

      const output_hit_enter =
        typeof (p as any).output_hit_enter === "boolean"
          ? (p as any).output_hit_enter
          : null;

      if (!id) return null;

      return {
        id,
        name,
        program_paths,
        cleanup_prompt_sections,
        rewrite_llm_enabled,
        stt_provider,
        stt_model,
        stt_timeout_seconds,
        llm_provider,
        llm_model,
        sound_enabled,
        playing_audio_handling,
        overlay_mode,
        widget_position,
        output_mode,
        output_hit_enter,
      };
    };

    const rawProfiles =
      (await store.get<any>("rewrite_program_prompt_profiles")) ?? [];
    const rewrite_program_prompt_profiles: RewriteProgramPromptProfile[] =
      Array.isArray(rawProfiles)
        ? rawProfiles
            .map(normalizeRewriteProfile)
            .filter((p): p is RewriteProgramPromptProfile => p !== null)
        : [];

    return {
      toggle_hotkey:
        (await store.get<HotkeyConfig>("toggle_hotkey")) ?? defaultToggleHotkey,
      hold_hotkey:
        (await store.get<HotkeyConfig>("hold_hotkey")) ?? defaultHoldHotkey,
      paste_last_hotkey:
        (await store.get<HotkeyConfig>("paste_last_hotkey")) ??
        defaultPasteLastHotkey,
      selected_mic_id:
        (await store.get<string | null>("selected_mic_id")) ?? null,
      sound_enabled: (await store.get<boolean>("sound_enabled")) ?? true,
      audio_cue: normalizeAudioCue(await store.get("audio_cue")),
      accent_color: normalizeHexColor(
        (await store.get<string | null>("accent_color")) ?? null
      ),
      rewrite_llm_enabled:
        (await store.get<boolean>("rewrite_llm_enabled")) ?? false,
      cleanup_prompt_sections:
        (await store.get<CleanupPromptSections | null>(
          "cleanup_prompt_sections"
        )) ?? null,
      rewrite_program_prompt_profiles,
      stt_provider: (await store.get<string | null>("stt_provider")) ?? null,
      stt_model: (await store.get<string | null>("stt_model")) ?? null,
      stt_transcription_prompt:
        (await store.get<string | null>("stt_transcription_prompt")) ?? null,
      llm_provider: (await store.get<string | null>("llm_provider")) ?? null,
      llm_model: (await store.get<string | null>("llm_model")) ?? null,
      playing_audio_handling: normalizePlayingAudioHandling(
        (await store.get("playing_audio_handling")) ??
          // Legacy key for migration:
          (await store.get<boolean>("auto_mute_audio")) ??
          // If neither exists, default to mute
          "mute"
      ),
      stt_timeout_seconds:
        (await store.get<number | null>("stt_timeout_seconds")) ?? null,
      overlay_mode:
        (await store.get<OverlayMode>("overlay_mode")) ?? "recording_only",
      widget_position:
        (await store.get<WidgetPosition>("widget_position")) ?? "bottom-center",
      output_mode: normalizeOutputMode(await store.get("output_mode")),
      output_hit_enter: (await store.get<boolean>("output_hit_enter")) ?? false,

      quiet_audio_gate_enabled:
        (await store.get<boolean>("quiet_audio_gate_enabled")) ?? true,
      quiet_audio_min_duration_secs:
        (await store.get<number>("quiet_audio_min_duration_secs")) ?? 0.15,
      quiet_audio_rms_dbfs_threshold:
        (await store.get<number>("quiet_audio_rms_dbfs_threshold")) ?? -60,
      quiet_audio_peak_dbfs_threshold:
        (await store.get<number>("quiet_audio_peak_dbfs_threshold")) ?? -50,

      noise_gate_strength: normalizeNoiseGateStrength(
        await store.get("noise_gate_strength")
      ),
    };
  },

  /**
   * Force the settings store to reload from disk.
   * Useful for secondary windows (overlay) when another window updates settings.json.
   */
  async reloadSettingsFromDisk(): Promise<void> {
    const store = await getStore();
    await store.load();
  },

  async updateAccentColor(color: string | null): Promise<void> {
    const store = await getStore();
    const normalized = normalizeHexColor(color);

    if (!normalized) {
      await store.delete("accent_color");
    } else {
      await store.set("accent_color", normalized);
    }

    await store.save();

    // Notify other windows (overlay) to refresh cached settings.
    // Include the new accent in the payload so the overlay can update immediately
    // without waiting for a disk reload.
    await emit("settings-changed", { accent_color: normalized ?? null });
  },

  async updateToggleHotkey(hotkey: HotkeyConfig): Promise<void> {
    const store = await getStore();
    await store.set("toggle_hotkey", hotkey);
    await store.save();
  },

  async updateHoldHotkey(hotkey: HotkeyConfig): Promise<void> {
    const store = await getStore();
    await store.set("hold_hotkey", hotkey);
    await store.save();
  },

  async updatePasteLastHotkey(hotkey: HotkeyConfig): Promise<void> {
    const store = await getStore();
    await store.set("paste_last_hotkey", hotkey);
    await store.save();
  },

  async updateSelectedMic(micId: string | null): Promise<void> {
    const store = await getStore();
    await store.set("selected_mic_id", micId);
    await store.save();

    // Notify other windows (overlay) to refresh cached settings.
    await emit("settings-changed", {});
  },

  async updateSoundEnabled(enabled: boolean): Promise<void> {
    const store = await getStore();
    await store.set("sound_enabled", enabled);
    await store.save();
  },

  async updateAudioCue(cue: AudioCue): Promise<void> {
    const store = await getStore();
    await store.set("audio_cue", normalizeAudioCue(cue));
    await store.save();

    // Notify other windows (overlay) to refresh cached settings.
    await emit("settings-changed", {});
  },

  async updateRewriteLlmEnabled(enabled: boolean): Promise<void> {
    const store = await getStore();
    await store.set("rewrite_llm_enabled", enabled);
    await store.save();
  },

  async updateCleanupPromptSections(
    sections: CleanupPromptSections | null
  ): Promise<void> {
    const store = await getStore();
    await store.set("cleanup_prompt_sections", sections);
    await store.save();
  },

  async updateRewriteProgramPromptProfiles(
    profiles: RewriteProgramPromptProfile[]
  ): Promise<void> {
    const store = await getStore();
    await store.set("rewrite_program_prompt_profiles", profiles);
    await store.save();
  },

  async listOpenWindows(): Promise<OpenWindowInfo[]> {
    return invoke("list_open_windows");
  },

  async getForegroundProcessPath(): Promise<string | null> {
    return invoke("get_foreground_process_path");
  },

  async updateSTTProvider(provider: string | null): Promise<void> {
    const store = await getStore();
    await store.set("stt_provider", provider);
    await store.save();
  },

  async updateSTTModel(model: string | null): Promise<void> {
    const store = await getStore();
    await store.set("stt_model", model);
    await store.save();
  },

  async updateSTTTranscriptionPrompt(prompt: string | null): Promise<void> {
    const store = await getStore();
    await store.set("stt_transcription_prompt", prompt);
    await store.save();
  },

  async updateLLMProvider(provider: string | null): Promise<void> {
    const store = await getStore();
    await store.set("llm_provider", provider);
    await store.save();
  },

  async updateLLMModel(model: string | null): Promise<void> {
    const store = await getStore();
    await store.set("llm_model", model);
    await store.save();
  },

  async updatePlayingAudioHandling(
    handling: PlayingAudioHandling
  ): Promise<void> {
    const store = await getStore();
    await store.set("playing_audio_handling", handling);
    await store.save();
  },

  async updateSTTTimeout(timeoutSeconds: number | null): Promise<void> {
    const store = await getStore();
    await store.set("stt_timeout_seconds", timeoutSeconds);
    await store.save();
  },

  async updateOverlayMode(mode: OverlayMode): Promise<void> {
    const store = await getStore();
    await store.set("overlay_mode", mode);
    await store.save();
    // Apply the mode immediately
    await invoke("set_overlay_mode", { mode });

    // Notify other windows (overlay) to refresh cached settings.
    await emit("settings-changed", {});
  },

  async updateWidgetPosition(position: WidgetPosition): Promise<void> {
    const store = await getStore();
    await store.set("widget_position", position);
    await store.save();
    // Apply the position immediately
    await invoke("set_widget_position", { position });

    // Notify other windows (overlay) to refresh cached settings.
    await emit("settings-changed", {});
  },

  async updateOutputMode(mode: OutputMode): Promise<void> {
    const store = await getStore();
    await store.set("output_mode", mode);
    await store.save();
  },

  async updateOutputHitEnter(enabled: boolean): Promise<void> {
    const store = await getStore();
    await store.set("output_hit_enter", enabled);
    await store.save();
  },

  async updateQuietAudioGateEnabled(enabled: boolean): Promise<void> {
    const store = await getStore();
    await store.set("quiet_audio_gate_enabled", enabled);
    await store.save();
  },

  async updateQuietAudioMinDurationSecs(seconds: number): Promise<void> {
    const store = await getStore();
    await store.set("quiet_audio_min_duration_secs", seconds);
    await store.save();
  },

  async updateQuietAudioRmsDbfsThreshold(dbfs: number): Promise<void> {
    const store = await getStore();
    await store.set("quiet_audio_rms_dbfs_threshold", dbfs);
    await store.save();
  },

  async updateQuietAudioPeakDbfsThreshold(dbfs: number): Promise<void> {
    const store = await getStore();
    await store.set("quiet_audio_peak_dbfs_threshold", dbfs);
    await store.save();
  },

  async updateNoiseGateStrength(strength: number): Promise<void> {
    const store = await getStore();
    await store.set(
      "noise_gate_strength",
      normalizeNoiseGateStrength(strength)
    );
    await store.save();
  },

  async isAudioMuteSupported(): Promise<boolean> {
    return invoke("is_audio_mute_supported");
  },

  // API Key management
  async hasApiKey(storeKey: string): Promise<boolean> {
    const store = await getStore();
    const value = await store.get<string>(storeKey);
    return value !== null && value !== undefined && value.length > 0;
  },

  async getApiKey(storeKey: string): Promise<string | null> {
    const store = await getStore();
    const value = await store.get<string>(storeKey);
    return value ?? null;
  },

  async setApiKey(storeKey: string, apiKey: string): Promise<void> {
    const store = await getStore();
    await store.set(storeKey, apiKey);
    await store.save();
  },

  async clearApiKey(storeKey: string): Promise<void> {
    const store = await getStore();
    await store.delete(storeKey);
    await store.save();
  },

  async resetHotkeysToDefaults(): Promise<void> {
    const store = await getStore();
    await store.set("toggle_hotkey", defaultToggleHotkey);
    await store.set("hold_hotkey", defaultHoldHotkey);
    await store.set("paste_last_hotkey", defaultPasteLastHotkey);
    await store.save();
  },

  async registerShortcuts(): Promise<void> {
    return invoke("register_shortcuts");
  },

  async unregisterShortcuts(): Promise<void> {
    return invoke("unregister_shortcuts");
  },

  // History API
  async addHistoryEntry(text: string): Promise<HistoryEntry> {
    return invoke("add_history_entry", { text });
  },

  async getHistory(limit?: number): Promise<HistoryEntry[]> {
    return invoke("get_history", { limit });
  },

  async deleteHistoryEntry(id: string): Promise<boolean> {
    return invoke("delete_history_entry", { id });
  },

  async clearHistory(): Promise<void> {
    return invoke("clear_history");
  },

  // Overlay API
  async resizeOverlay(width: number, height: number): Promise<void> {
    return invoke("resize_overlay", { width, height });
  },

  async startDragging(): Promise<void> {
    const window = getCurrentWindow();
    return window.startDragging();
  },

  // Connection state sync between windows
  async emitConnectionState(state: ConnectionState): Promise<void> {
    return emit("connection-state-changed", { state });
  },

  async onConnectionStateChanged(
    callback: (state: ConnectionState) => void
  ): Promise<UnlistenFn> {
    return listen<{ state: ConnectionState }>(
      "connection-state-changed",
      (event) => {
        callback(event.payload.state);
      }
    );
  },

  // History sync between windows
  async emitHistoryChanged(): Promise<void> {
    return emit("history-changed", {});
  },

  async onHistoryChanged(callback: () => void): Promise<UnlistenFn> {
    return listen("history-changed", () => {
      callback();
    });
  },

  // Settings sync between windows (main -> overlay)
  async emitSettingsChanged(
    payload: Record<string, unknown> = {}
  ): Promise<void> {
    return emit("settings-changed", payload);
  },

  async onSettingsChanged(
    callback: (payload: unknown) => void
  ): Promise<UnlistenFn> {
    return listen("settings-changed", (event) => {
      callback(event.payload);
    });
  },
};

export interface OpenWindowInfo {
  title: string;
  process_path: string;
}

// ============================================================================
// LLM API
// ============================================================================

export interface TestLlmRewriteResponse {
  output: string;
  provider_used: string;
  model_used: string;
}

export const llmAPI = {
  testLlmRewrite: (params: { transcript: string; profileId?: string | null }) =>
    invoke<TestLlmRewriteResponse>("test_llm_rewrite", {
      transcript: params.transcript,
      // Rust param name is `profile_id`
      profile_id: params.profileId ?? null,
    }),
};

export const sttAPI = {
  testTranscribeLastAudio: (params: { profileId?: string | null }) =>
    invoke<string>("pipeline_test_transcribe_last_audio", {
      // Rust param name is `profile_id`
      profile_id: params.profileId ?? null,
    }),

  hasLastAudio: () => invoke<boolean>("pipeline_has_last_audio"),

  // Retry a previous request using its persisted audio.
  // Returns the final text (STT + optional LLM), same as normal transcription.
  retryTranscription: (params: { requestId: string }) =>
    invoke<string>("pipeline_retry_transcription", {
      requestId: params.requestId,
    }),
};

// ============================================================================
// Config API - Using Tauri commands
// ============================================================================

export interface DefaultSectionsResponse {
  main: string;
  advanced: string;
  dictionary: string;
}

interface ProviderInfo {
  value: string;
  label: string;
  is_local: boolean;
}

interface AvailableProvidersResponse {
  stt: ProviderInfo[];
  llm: ProviderInfo[];
}

export const configAPI = {
  // Default prompt sections (from Tauri)
  getDefaultSections: () =>
    invoke<DefaultSectionsResponse>("get_default_sections"),

  // Available providers (from Tauri, based on configured API keys)
  getAvailableProviders: () =>
    invoke<AvailableProvidersResponse>("get_available_providers"),

  // Sync pipeline config when settings change
  syncPipelineConfig: () => invoke<void>("sync_pipeline_config"),
};

// ============================================================================
// Request Logs API
// ============================================================================

export type LogLevel = "debug" | "info" | "warn" | "error";
export type RequestStatus = "in_progress" | "success" | "error" | "cancelled";

export interface LogEntry {
  timestamp: string;
  level: LogLevel;
  message: string;
  details: string | null;
}

export interface RequestLog {
  id: string;
  started_at: string;
  ended_at: string | null;
  stt_provider: string;
  stt_model: string | null;
  llm_provider: string | null;
  llm_model: string | null;
  raw_transcript: string | null;
  final_text: string | null;
  stt_duration_ms: number | null;
  llm_duration_ms: number | null;
  status: RequestStatus;
  error_message: string | null;
  entries: LogEntry[];
}

export const logsAPI = {
  getRequestLogs: (limit?: number) =>
    invoke<RequestLog[]>("get_request_logs", { limit: limit ?? 100 }),

  clearRequestLogs: () => invoke<void>("clear_request_logs"),
};

// ============================================================================
// Recordings API (playback)
// ============================================================================

export const recordingsAPI = {
  // Returns a URL usable as an <audio src>, or null if no recording exists.
  getRecordingAssetUrl: async (params: { requestId: string }) => {
    const path = await invoke<string | null>("recording_get_wav_path", {
      requestId: params.requestId,
    });
    return path ? convertFileSrc(path) : null;
  },

  // Returns base64 WAV bytes, or null if no recording exists.
  getRecordingWavBase64: (params: { requestId: string }) =>
    invoke<string | null>("recording_get_wav_base64", {
      requestId: params.requestId,
    }),
};
