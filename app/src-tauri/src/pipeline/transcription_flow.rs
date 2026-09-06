//! Transcription flow orchestration.
//!
//! This module extracts the shared transcription logic used by:
//! - `stop_and_transcribe_detailed` (after stopping recording)
//! - `transcribe_wav_bytes_detailed_for_profile` (for retry/replay)
//!
//! The flow consists of:
//! 1. Preset routing (embeddings or LLM-based)
//! 2. LLM rewrite with fallback to raw transcript
//!
//! STT execution itself is centralized in `stt_flow.rs` so retry telemetry,
//! optional timeout behavior, cancellation priority, and log context stay in one place.

use crate::llm::{format_text, LlmProvider, ProgramPromptProfile};
use crate::request_log::RequestLogStore;
use crate::settings::{IntentRouterStrategy, ProxySettings};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use super::llm_provider::LlmProviderParams;
use super::profile_resolution::{find_preset_by_id, router_enabled};
use super::routing::{route_preset_id_with_embeddings, route_preset_id_with_llm, RoutingDecision};
use super::types::{LlmNotAttemptedReason, LlmOutcome, PipelineError, TranscriptionResult};

/// Context needed for transcription flow.
///
/// This bundles together all the dependencies and configuration needed for
/// the transcription pipeline without requiring access to the full SharedPipeline.
pub(super) struct TranscriptionContext<'a> {
    /// Active profile for this transcription (already resolved from foreground app or override).
    pub active_profile: Option<ProgramPromptProfile>,
    /// Optional OCR context from the active window.
    pub active_window_ocr_text: Option<String>,
    /// Whether LLM rewrite is enabled globally (used as fallback for Default profile).
    pub llm_enabled_global: bool,
    /// Default profile's include_clipboard_context setting.
    pub default_rewrite_include_clipboard_context: bool,
    /// Session preset lock (if any).
    pub session_lock: Option<SessionPresetLock>,
    /// Proxy settings for network requests.
    pub proxy_settings: ProxySettings,
    /// LLM API keys by provider.
    pub llm_api_keys: HashMap<String, String>,
    /// Request log store for diagnostics.
    pub request_log_store: Option<RequestLogStore>,
    /// Embedding cache for intent routing.
    pub embedding_cache: &'a Arc<Mutex<HashMap<String, Vec<f32>>>>,
    /// App handle for persisting embedding cache.
    pub persist_app: Option<AppHandle>,
    /// Cancellation token.
    pub cancel_token: CancellationToken,
    /// Injected embeddings provider for testing (bypasses real API calls).
    pub injected_embeddings_provider: Option<Arc<dyn crate::embeddings::EmbeddingsProvider>>,

    /// Optional CLI/debug override: force the LLM rewrite step on for this transcription,
    /// ignoring the usual profile/preset gates.
    pub force_llm_rewrite: bool,
    /// Optional CLI/debug override: force a specific LLM provider for this transcription.
    /// When set, this takes precedence over preset/profile/global selection.
    pub forced_llm_provider: Option<String>,
    /// Optional CLI/debug override: force a specific LLM model for this transcription.
    /// When set, this takes precedence over preset/profile/global selection.
    pub forced_llm_model: Option<String>,
}

/// Session preset lock state.
#[derive(Debug, Clone)]
pub(super) struct SessionPresetLock {
    pub profile_id: Option<String>,
    pub preset_id: String,
}

/// Callbacks for state transitions during transcription.
///
/// These allow the caller to update pipeline state and log provider creation.
pub(super) trait TranscriptionCallbacks: Send + Sync {
    /// Called when the pipeline should transition to routing state.
    fn transition_to_routing(&self);
    /// Called when the pipeline should transition back from routing.
    fn transition_from_routing(&self);
    /// Called when the pipeline should transition to rewriting state.
    fn transition_to_rewriting(&self);
    /// Get or create an LLM provider.
    fn get_or_create_llm_provider(
        &self,
        provider_id: &str,
        params: LlmProviderParams,
    ) -> Result<Arc<dyn LlmProvider>, PipelineError>;
}

/// Result of the routing phase.
struct RoutingResult {
    routed_preset_id: Option<String>,
}

/// Result of resolving the LLM provider for rewrite.
pub(super) struct LlmResolution {
    provider: Option<Arc<dyn LlmProvider>>,
    prompts: crate::llm::PromptSections,
    timeout: Duration,
    not_attempted_reason: Option<LlmNotAttemptedReason>,
}

/// Route to a preset based on the transcript.
async fn route_preset<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    stt_text: &str,
) -> RoutingResult {
    let mut routed_preset_id: Option<String> = None;

    let profile_rewrite_enabled = ctx
        .active_profile
        .as_ref()
        .and_then(|p| p.rewrite_llm_enabled)
        .unwrap_or(ctx.llm_enabled_global);

    if !profile_rewrite_enabled {
        return RoutingResult { routed_preset_id };
    }

    let Some(profile) = ctx.active_profile.as_ref() else {
        return RoutingResult { routed_preset_id };
    };

    // Session override wins over everything else.
    if let Some(lock) = ctx.session_lock.as_ref() {
        let profile_ok = lock
            .profile_id
            .as_deref()
            .map(|pid| pid == profile.id)
            .unwrap_or(true);

        if profile_ok && find_preset_by_id(profile, lock.preset_id.as_str()).is_some() {
            routed_preset_id = Some(lock.preset_id.clone());
        }
    }

    // Persisted manual override wins over router/default.
    if routed_preset_id.is_none() {
        if let Some(id) = profile.active_preset_id.as_deref() {
            routed_preset_id = Some(id.to_string());
        }
    }

    // Run router if enabled and no override is set.
    if routed_preset_id.is_none() && router_enabled(profile) {
        routed_preset_id = run_intent_router(ctx, callbacks, profile, stt_text).await;
    }

    // Default preset is the fallback when routing is off/undecided.
    if routed_preset_id.is_none() {
        routed_preset_id = profile.default_preset_id.clone();
    }

    RoutingResult { routed_preset_id }
}

/// Run intent router (embeddings or LLM-based).
async fn run_intent_router<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    profile: &ProgramPromptProfile,
    stt_text: &str,
) -> Option<String> {
    let router = profile.router.as_ref()?;

    if router.strategy == IntentRouterStrategy::Llm {
        run_llm_router(ctx, callbacks, profile, stt_text).await
    } else {
        run_embeddings_router(ctx, callbacks, profile, stt_text).await
    }
}

/// Run LLM-based intent router.
async fn run_llm_router<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    profile: &ProgramPromptProfile,
    stt_text: &str,
) -> Option<String> {
    // If there's no active profile, routing should be disabled.
    let _active_profile = ctx.active_profile.as_ref()?;

    // Build provider params from router config or global defaults.
    let router_cfg = profile.router.as_ref();
    let desired_provider = router_cfg
        .and_then(|r| r.llm_provider.clone())
        .unwrap_or_else(|| "openai".to_string());
    let desired_model = router_cfg.and_then(|r| r.llm_model.clone());
    let desired_openai_effort = router_cfg.and_then(|r| r.openai_reasoning_effort.clone());
    let desired_gemini_budget = router_cfg.and_then(|r| r.gemini_thinking_budget);
    let desired_gemini_level = router_cfg.and_then(|r| r.gemini_thinking_level.clone());
    let desired_anthropic_budget = router_cfg.and_then(|r| r.anthropic_thinking_budget);

    let maybe_provider = callbacks
        .get_or_create_llm_provider(
            desired_provider.as_str(),
            LlmProviderParams {
                model: desired_model,
                timeout: Duration::from_secs(30),
                ollama_url: None,
                openai_reasoning_effort: desired_openai_effort,
                gemini_thinking_budget: desired_gemini_budget,
                gemini_thinking_level: desired_gemini_level,
                anthropic_thinking_budget: desired_anthropic_budget,
            },
        )
        .ok();

    let provider = maybe_provider?;

    // Expose routing as a distinct UI phase.
    callbacks.transition_to_routing();

    let router_start = std::time::Instant::now();
    let llm_out = route_preset_id_with_llm(profile, stt_text, provider.as_ref()).await;

    let routed_preset_id = if let Some(decision) = llm_out {
        let router_duration_ms = router_start.elapsed().as_millis() as u64;
        record_routing_decision(ctx, profile, "llm", router_duration_ms, decision)
    } else {
        None
    };

    // Restore the phase from routing.
    callbacks.transition_from_routing();

    routed_preset_id
}

/// Run embeddings-based intent router.
async fn run_embeddings_router<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    profile: &ProgramPromptProfile,
    stt_text: &str,
) -> Option<String> {
    // Expose routing as a distinct UI phase.
    callbacks.transition_to_routing();

    let router_start = std::time::Instant::now();
    let embeddings_out = route_preset_id_with_embeddings(
        profile,
        stt_text,
        &ctx.proxy_settings,
        &ctx.llm_api_keys,
        ctx.embedding_cache,
        ctx.persist_app.clone(),
        ctx.injected_embeddings_provider.clone(),
    )
    .await;

    let routed_preset_id = if let Some(decision) = embeddings_out {
        let router_duration_ms = router_start.elapsed().as_millis() as u64;
        record_routing_decision(ctx, profile, "embeddings", router_duration_ms, decision)
    } else {
        None
    };

    // Restore the phase from routing.
    callbacks.transition_from_routing();

    routed_preset_id
}

/// Persist one strategy-independent routing decision into request logs and legacy score fields.
fn record_routing_decision(
    ctx: &TranscriptionContext<'_>,
    profile: &ProgramPromptProfile,
    strategy: &str,
    router_duration_ms: u64,
    decision: RoutingDecision,
) -> Option<String> {
    let RoutingDecision {
        selected_preset_id,
        scores,
        threshold,
        margin,
        request_json,
        response_json,
        ..
    } = decision;

    if let Some(store) = ctx.request_log_store.as_ref() {
        store.with_current(|log| {
            log.router_request_json = Some(request_json);
            log.router_response_json = Some(response_json);
        });
    }

    if strategy == "embeddings" {
        log_embeddings_router_scores(
            ctx,
            profile,
            &selected_preset_id,
            router_duration_ms,
            &scores,
            threshold.unwrap_or(0.0),
            margin.unwrap_or(0.0),
        );
    } else {
        log_router_scores(
            ctx,
            profile,
            &selected_preset_id,
            router_duration_ms,
            strategy,
            &[],
        );
    }

    selected_preset_id
}

/// Log router scores for LLM-based routing.
fn log_router_scores(
    ctx: &TranscriptionContext<'_>,
    profile: &ProgramPromptProfile,
    selected: &Option<String>,
    router_duration_ms: u64,
    strategy: &str,
    _scores_raw: &[(String, f32)],
) {
    if let Some(store) = ctx.request_log_store.as_ref() {
        let mut scores: Vec<crate::request_log::RouterPresetScore> = profile
            .presets
            .iter()
            .map(|preset| crate::request_log::RouterPresetScore {
                preset_id: preset.id.clone(),
                preset_name: preset.name.clone(),
                score: None,
                selected: selected
                    .as_deref()
                    .map(|id| id == preset.id)
                    .unwrap_or(false),
            })
            .collect();

        // Include the implicit Default (no preset) target when configured.
        if profile
            .default_preset_description
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            scores.push(crate::request_log::RouterPresetScore {
                preset_id: "__default__".to_string(),
                preset_name: "Default (no preset)".to_string(),
                score: None,
                selected: selected.is_none(),
            });
        }

        store.with_current(|log| {
            log.router_duration_ms = Some(router_duration_ms);
            log.router_strategy = Some(strategy.to_string());
            log.router_scores = Some(scores);
            log.info(format!(
                "Intent router ({}) completed in {}ms",
                strategy, router_duration_ms
            ));
        });
    }
}

/// Log router scores for embeddings-based routing.
fn log_embeddings_router_scores(
    ctx: &TranscriptionContext<'_>,
    profile: &ProgramPromptProfile,
    selected: &Option<String>,
    router_duration_ms: u64,
    scores_raw: &[(String, f32)],
    threshold: f32,
    margin: f32,
) {
    let Some(store) = ctx.request_log_store.as_ref() else {
        return;
    };

    let pick_highest_score = profile
        .router
        .as_ref()
        .map(|r| r.pick_highest_score)
        .unwrap_or(false);

    let selected_default = {
        let mut best_id: Option<&str> = None;
        let mut best_score: f32 = 0.0;
        let mut second_best_score: Option<f32> = None;

        for (id, score) in scores_raw {
            let score = *score;
            match best_id {
                None => {
                    best_id = Some(id.as_str());
                    best_score = score;
                }
                Some(_) if score > best_score => {
                    second_best_score = Some(best_score);
                    best_id = Some(id.as_str());
                    best_score = score;
                }
                Some(_) => {
                    if score <= best_score && second_best_score.map(|s| score > s).unwrap_or(true) {
                        second_best_score = Some(score);
                    }
                }
            }
        }

        if pick_highest_score {
            matches!(best_id, Some("__default__"))
        } else {
            match best_id {
                Some("__default__") if best_score >= threshold => second_best_score
                    .map(|s| best_score - s >= margin)
                    .unwrap_or(true),
                _ => false,
            }
        }
    };

    // Map raw candidate score list -> per-preset scores.
    let mut score_map: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for (id, score) in scores_raw {
        score_map.insert(id.clone(), *score);
    }

    let mut scores: Vec<crate::request_log::RouterPresetScore> = profile
        .presets
        .iter()
        .map(|preset| crate::request_log::RouterPresetScore {
            preset_id: preset.id.clone(),
            preset_name: preset.name.clone(),
            score: score_map.get(&preset.id).copied(),
            selected: selected
                .as_deref()
                .map(|id| id == preset.id)
                .unwrap_or(false),
        })
        .collect();

    // Include the implicit Default (no preset) target when configured.
    if profile
        .default_preset_description
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        scores.push(crate::request_log::RouterPresetScore {
            preset_id: "__default__".to_string(),
            preset_name: "Default (no preset)".to_string(),
            score: score_map.get("__default__").copied(),
            selected: selected_default,
        });
    }

    // Sort by score desc, with None last.
    scores.sort_by(|a, b| match (a.score, b.score) {
        (Some(sa), Some(sb)) => sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    store.with_current(|log| {
        log.router_duration_ms = Some(router_duration_ms);
        log.router_strategy = Some("embeddings".to_string());
        log.router_scores = Some(scores);
        if pick_highest_score {
            log.info_with_details(
                format!(
                    "Intent router (embeddings) completed in {}ms",
                    router_duration_ms
                ),
                "pick_highest_score=true".to_string(),
            );
        } else {
            log.info_with_details(
                format!(
                    "Intent router (embeddings) completed in {}ms",
                    router_duration_ms
                ),
                format!("threshold={:.3}, margin={:.3}", threshold, margin),
            );
        }
    });
}

/// Log preset selection info.
pub(super) fn log_preset_selection(
    ctx: &TranscriptionContext<'_>,
    routed_preset_id: &Option<String>,
) {
    let Some(store) = ctx.request_log_store.as_ref() else {
        return;
    };

    // Persist the selected preset (or lack of preset) into the request log.
    let (preset_id, preset_name) = if let Some(profile) = ctx.active_profile.as_ref() {
        let preset_name = routed_preset_id
            .as_deref()
            .and_then(|id| find_preset_by_id(profile, id))
            .map(|p| p.name.clone());
        (routed_preset_id.clone(), preset_name)
    } else {
        (None, None)
    };

    store.with_current(|log| {
        log.preset_id = preset_id;
        log.preset_name = preset_name;
    });

    if let Some(profile) = ctx.active_profile.as_ref() {
        if let Some(id) = routed_preset_id.as_deref() {
            if let Some(preset) = find_preset_by_id(profile, id) {
                let reason = if ctx
                    .session_lock
                    .as_ref()
                    .map(|l| l.preset_id == id)
                    .unwrap_or(false)
                {
                    "Session override selected preset"
                } else if profile
                    .active_preset_id
                    .as_deref()
                    .map(|l| l == id)
                    .unwrap_or(false)
                {
                    "Manual (persisted) override selected preset"
                } else if router_enabled(profile) {
                    "Intent router selected preset"
                } else {
                    "Default preset selected"
                };

                store.with_current(|log| {
                    log.info_with_details(reason, format!("{} ({})", preset.name, preset.id));
                });
            }
        }
    }
}

/// Resolve LLM provider and prompts for the rewrite step.
pub(super) fn resolve_llm_for_rewrite<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    routed_preset_id: &Option<String>,
    llm_config: &crate::llm::LlmConfig,
) -> LlmResolution {
    let selected_profile = ctx.active_profile.as_ref();
    let selected_preset = selected_profile.and_then(|p| {
        routed_preset_id
            .as_deref()
            .and_then(|id| find_preset_by_id(p, id))
    });

    let llm_prompts = selected_preset
        .map(|p| p.prompts.clone())
        .or_else(|| selected_profile.map(|p| p.prompts.clone()))
        .unwrap_or_else(|| llm_config.prompts.clone());

    let llm_timeout = llm_config.timeout;

    let selected_preset_rewrite_enabled = selected_preset.map(|p| p.rewrite_llm_enabled);
    let default_target_rewrite_enabled = selected_profile
        .map(|p| p.default_target_rewrite_llm_enabled)
        .unwrap_or(true);

    // Rewrite gates
    let default_profile_enabled = ctx.llm_enabled_global;

    let profile_enabled = match selected_profile {
        Some(p) => match p.rewrite_llm_enabled {
            Some(v) => v,
            None => {
                if p.id == "default" {
                    default_profile_enabled
                } else {
                    true
                }
            }
        },
        None => default_profile_enabled,
    };

    let effective_llm_enabled = if ctx.force_llm_rewrite {
        true
    } else if let Some(preset) = selected_preset {
        profile_enabled && preset.rewrite_llm_enabled
    } else {
        profile_enabled && default_target_rewrite_enabled
    };

    let disabled_reason = if !profile_enabled {
        if selected_profile
            .as_ref()
            .map(|p| p.id == "default" && p.rewrite_llm_enabled.is_none())
            .unwrap_or(false)
            && !default_profile_enabled
        {
            Some(LlmNotAttemptedReason::DisabledByDefaultProfile)
        } else {
            Some(LlmNotAttemptedReason::DisabledByProfile)
        }
    } else if selected_preset_rewrite_enabled == Some(false) {
        Some(LlmNotAttemptedReason::DisabledByPreset)
    } else if selected_preset.is_none() && !default_target_rewrite_enabled {
        Some(LlmNotAttemptedReason::DisabledByDefaultTarget)
    } else {
        None
    };

    let (llm_provider, not_attempted_reason) = if effective_llm_enabled {
        let desired_llm_provider = ctx.forced_llm_provider.clone().unwrap_or_else(|| {
            selected_preset
                .and_then(|p| p.llm_provider.clone())
                .or_else(|| selected_profile.and_then(|p| p.llm_provider.clone()))
                .unwrap_or_else(|| llm_config.provider.clone())
        });
        let desired_llm_model = ctx.forced_llm_model.clone().or_else(|| {
            selected_preset
                .and_then(|p| p.llm_model.clone())
                .or_else(|| selected_profile.and_then(|p| p.llm_model.clone()))
                .or_else(|| llm_config.model.clone())
        });

        // Resolve effective provider-specific thinking knobs.
        let effective_openai_reasoning_effort = selected_preset
            .and_then(|p| p.openai_reasoning_effort.clone())
            .or_else(|| selected_profile.and_then(|p| p.openai_reasoning_effort.clone()))
            .or_else(|| llm_config.openai_reasoning_effort.clone());
        let effective_gemini_thinking_budget = selected_preset
            .and_then(|p| p.gemini_thinking_budget)
            .or_else(|| selected_profile.and_then(|p| p.gemini_thinking_budget))
            .or(llm_config.gemini_thinking_budget);
        let effective_gemini_thinking_level = selected_preset
            .and_then(|p| p.gemini_thinking_level.clone())
            .or_else(|| selected_profile.and_then(|p| p.gemini_thinking_level.clone()))
            .or_else(|| llm_config.gemini_thinking_level.clone());
        let effective_anthropic_thinking_budget = selected_preset
            .and_then(|p| p.anthropic_thinking_budget)
            .or_else(|| selected_profile.and_then(|p| p.anthropic_thinking_budget))
            .or(llm_config.anthropic_thinking_budget);

        callbacks
            .get_or_create_llm_provider(
                desired_llm_provider.as_str(),
                LlmProviderParams {
                    model: desired_llm_model.clone(),
                    timeout: llm_timeout,
                    ollama_url: llm_config.ollama_url.clone(),
                    openai_reasoning_effort: effective_openai_reasoning_effort,
                    gemini_thinking_budget: effective_gemini_thinking_budget,
                    gemini_thinking_level: effective_gemini_thinking_level,
                    anthropic_thinking_budget: effective_anthropic_thinking_budget,
                },
            )
            .map(|p| (Some(p), None))
            .unwrap_or_else(|e| {
                log::warn!(
                    "Pipeline: LLM provider '{}' unavailable: {}",
                    desired_llm_provider,
                    e
                );
                if let Some(store) = ctx.request_log_store.as_ref() {
                    store.with_current(|log| {
                        log.warn(format!(
                            "LLM rewrite enabled but provider '{}' was unavailable: {}",
                            desired_llm_provider, e
                        ));
                    });
                }
                (
                    None,
                    Some(LlmNotAttemptedReason::ProviderUnavailable {
                        provider: desired_llm_provider,
                        error: e.to_string(),
                    }),
                )
            })
    } else {
        (None, disabled_reason)
    };

    LlmResolution {
        provider: llm_provider,
        prompts: llm_prompts,
        timeout: llm_timeout,
        not_attempted_reason,
    }
}

/// Run LLM rewrite on the transcript.
pub(super) async fn run_llm_rewrite<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    stt_text: &str,
    llm_resolution: LlmResolution,
) -> (
    String,
    Option<u64>,
    LlmOutcome,
    Option<String>,
    Option<String>,
) {
    let mut llm_duration_ms: Option<u64> = None;
    let mut llm_outcome: LlmOutcome = LlmOutcome::NotAttempted(
        llm_resolution
            .not_attempted_reason
            .unwrap_or(LlmNotAttemptedReason::Unknown),
    );

    let llm_provider_used: Option<String> = llm_resolution
        .provider
        .as_ref()
        .map(|p| p.name().to_string());
    let llm_model_used: Option<String> = llm_resolution
        .provider
        .as_ref()
        .map(|p| p.model().to_string());

    let final_text = if let Some(llm) = llm_resolution.provider {
        // Expose the optional LLM step as a distinct phase for UI.
        callbacks.transition_to_rewriting();

        log::info!("Pipeline: Applying LLM formatting");

        llm_outcome = LlmOutcome::Succeeded; // may be overwritten by fallback paths
        let llm_start = std::time::Instant::now();

        let rewrite_include_clipboard_context = ctx
            .active_profile
            .as_ref()
            .and_then(|p| p.rewrite_include_clipboard_context)
            .unwrap_or(ctx.default_rewrite_include_clipboard_context);

        let clipboard_text = if rewrite_include_clipboard_context {
            crate::clipboard_context::read_clipboard_text_best_effort_async(8000).await
        } else {
            None
        };

        let rewrite_user_message = crate::prompt_builders::build_rewrite_user_message(
            stt_text,
            clipboard_text.as_deref(),
            ctx.active_window_ocr_text.as_deref(),
        );

        // Apply LLM formatting with timeout
        let llm_result = tokio::select! {
            biased;

            _ = ctx.cancel_token.cancelled() => {
                log::info!("Pipeline: LLM formatting cancelled");
                Err(PipelineError::Cancelled)
            }

            _ = tokio::time::sleep(llm_resolution.timeout) => {
                log::warn!("Pipeline: LLM formatting timed out, using raw transcript");
                llm_outcome = LlmOutcome::TimedOut;
                Ok(stt_text.to_string())
            }

            result = format_text(llm.as_ref(), rewrite_user_message.as_str(), &llm_resolution.prompts) => {
                match result {
                    Ok(formatted) => {
                        log::info!("Pipeline: LLM formatted {} -> {} chars", stt_text.len(), formatted.len());
                        Ok(formatted)
                    }
                    Err(e) => {
                        log::warn!("Pipeline: LLM formatting failed ({}), using raw transcript", e);
                        llm_outcome = LlmOutcome::Failed(e.to_string());
                        Ok(stt_text.to_string())
                    }
                }
            }
        };

        llm_duration_ms = Some(llm_start.elapsed().as_millis() as u64);

        // Persist the actual provider/model used into the request log.
        if let Some(store) = ctx.request_log_store.as_ref() {
            store.with_current(|log| {
                log.llm_provider = llm_provider_used.clone();
                log.llm_model = llm_model_used.clone();
                log.rewrite_clipboard_context = clipboard_text.clone();
                let ocr_chars = ctx
                    .active_window_ocr_text
                    .as_deref()
                    .map(|s| s.len() as u64);
                log.ocr_context_present = ocr_chars.is_some();
                log.ocr_context_chars = ocr_chars;
            });
        }

        match llm_result {
            Ok(text) => text,
            Err(PipelineError::Cancelled) => {
                return (
                    stt_text.to_string(),
                    llm_duration_ms,
                    LlmOutcome::NotAttempted(LlmNotAttemptedReason::Unknown),
                    llm_provider_used,
                    llm_model_used,
                );
            }
            Err(_) => stt_text.to_string(), // Fallback on other errors
        }
    } else {
        stt_text.to_string()
    };

    (
        final_text,
        llm_duration_ms,
        llm_outcome,
        llm_provider_used,
        llm_model_used,
    )
}

/// Complete transcription flow after STT is done.
///
/// This orchestrates routing and LLM rewrite.
pub(super) async fn complete_transcription_flow<C: TranscriptionCallbacks>(
    ctx: &TranscriptionContext<'_>,
    callbacks: &C,
    stt_text: &str,
    stt_duration_ms: u64,
    stt_retry: Option<crate::stt::RetryTelemetry>,
    llm_config: &crate::llm::LlmConfig,
) -> TranscriptionResult {
    // Phase 3a: Route preset
    let routing_result = route_preset(ctx, callbacks, stt_text).await;
    let routed_preset_id = routing_result.routed_preset_id;

    // Log preset selection
    log_preset_selection(ctx, &routed_preset_id);

    // Phase 3b: Resolve LLM provider
    let llm_resolution = resolve_llm_for_rewrite(ctx, callbacks, &routed_preset_id, llm_config);

    // Phase 4: LLM rewrite
    let (final_text, llm_duration_ms, llm_outcome, llm_provider_used, llm_model_used) =
        run_llm_rewrite(ctx, callbacks, stt_text, llm_resolution).await;

    TranscriptionResult {
        speaker_segments: Vec::new(),
        stt_text: stt_text.to_string(),
        final_text,
        stt_duration_ms,
        stt_retry,
        llm_duration_ms,
        llm_provider_used,
        llm_model_used,
        llm_outcome,
        live_output_completed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::EmbeddingsError;
    use crate::llm::{LlmError, ProgramPromptProfile, PromptSections};
    use crate::pipeline::llm_provider::LlmProviderParams;
    use crate::settings::IntentRouterSettings;
    use async_trait::async_trait;

    struct RecordingCallbacks(std::sync::Mutex<Option<(String, Option<String>)>>);

    impl TranscriptionCallbacks for RecordingCallbacks {
        fn transition_to_routing(&self) {}
        fn transition_from_routing(&self) {}
        fn transition_to_rewriting(&self) {}

        fn get_or_create_llm_provider(
            &self,
            provider_id: &str,
            params: LlmProviderParams,
        ) -> Result<std::sync::Arc<dyn crate::llm::LlmProvider>, crate::pipeline::PipelineError>
        {
            *self.0.lock().expect("lock") = Some((provider_id.to_string(), params.model.clone()));

            // We don't need a real provider instance for this test; we only validate selection.
            Err(crate::pipeline::PipelineError::Config(
                "test: provider creation not needed".to_string(),
            ))
        }
    }

    struct RoutingCallbacks {
        transitions: std::sync::Mutex<Vec<&'static str>>,
    }

    impl RoutingCallbacks {
        fn new() -> Self {
            Self {
                transitions: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl TranscriptionCallbacks for RoutingCallbacks {
        fn transition_to_routing(&self) {
            self.transitions.lock().expect("lock").push("to_routing");
        }

        fn transition_from_routing(&self) {
            self.transitions.lock().expect("lock").push("from_routing");
        }

        fn transition_to_rewriting(&self) {}

        fn get_or_create_llm_provider(
            &self,
            _provider_id: &str,
            _params: LlmProviderParams,
        ) -> Result<std::sync::Arc<dyn crate::llm::LlmProvider>, crate::pipeline::PipelineError>
        {
            Err(crate::pipeline::PipelineError::Config(
                "test: provider creation not needed".to_string(),
            ))
        }
    }

    struct FlowEmbeddingsProvider(std::collections::HashMap<String, Vec<f32>>);

    #[async_trait]
    impl crate::embeddings::EmbeddingsProvider for FlowEmbeddingsProvider {
        async fn embed_text(
            &self,
            text: &str,
            _input_type: Option<&str>,
        ) -> Result<(Vec<f32>, serde_json::Value, serde_json::Value), EmbeddingsError> {
            let embedding = self.0.get(text).cloned().unwrap_or_else(|| vec![0.0, 0.0]);
            Ok((
                embedding.clone(),
                serde_json::json!({ "text": text }),
                serde_json::json!({ "embedding_len": embedding.len() }),
            ))
        }

        fn name(&self) -> &'static str {
            "openai"
        }

        fn model(&self) -> &str {
            "fake-embedding-model"
        }
    }

    struct PanicLlmProvider;

    #[async_trait]
    impl crate::llm::LlmProvider for PanicLlmProvider {
        async fn complete(
            &self,
            _system_prompt: &str,
            _user_message: &str,
        ) -> Result<String, LlmError> {
            panic!("cancelled rewrite should not call provider");
        }

        fn name(&self) -> &'static str {
            "panic"
        }

        fn model(&self) -> &str {
            "panic-model"
        }
    }

    fn minimal_profile_with_llm_overrides(
        provider: Option<&str>,
        model: Option<&str>,
    ) -> ProgramPromptProfile {
        ProgramPromptProfile {
            id: "test-profile".to_string(),
            name: "Test Profile".to_string(),
            program_paths: vec![],
            prompts: PromptSections::default(),
            presets: vec![],
            default_preset_id: None,
            default_preset_description: None,
            default_target_rewrite_llm_enabled: true,
            active_preset_id: None,
            router: None,
            rewrite_llm_enabled: Some(true),
            stt_provider: None,
            stt_model: None,
            stt_language: None,
            stt_timeout_seconds: None,
            llm_provider: provider.map(|s| s.to_string()),
            llm_model: model.map(|s| s.to_string()),
            openai_reasoning_effort: None,
            gemini_thinking_budget: None,
            gemini_thinking_level: None,
            anthropic_thinking_budget: None,
            quick_ask_provider: None,
            quick_ask_model: None,
            quick_ask_system_prompt: None,
            context_grab_method: None,
            rewrite_include_clipboard_context: None,
            quick_replace_include_clipboard_context: None,
            quick_ask_include_clipboard_context: None,
            rewrite_active_window_ocr_mode: None,
            quick_replace_active_window_ocr_mode: None,
            quick_ask_active_window_ocr_mode: None,
            quick_replace_enabled: None,
            quick_replace_provider: None,
            quick_replace_model: None,
            quick_replace_system_prompt: None,
            quick_ask_openai_reasoning_effort: None,
            quick_ask_gemini_thinking_budget: None,
            quick_ask_gemini_thinking_level: None,
            quick_ask_anthropic_thinking_budget: None,
        }
    }

    fn embeddings_router_profile() -> ProgramPromptProfile {
        let mut profile = minimal_profile_with_llm_overrides(None, None);
        profile.presets = vec![
            crate::llm::ProgramPreset {
                id: "email".to_string(),
                name: "Email".to_string(),
                routing_hints: vec!["email hint".to_string()],
                prompts: PromptSections::default(),
                rewrite_llm_enabled: true,
                stt_provider: None,
                stt_model: None,
                stt_language: None,
                stt_timeout_seconds: None,
                llm_provider: None,
                llm_model: None,
                openai_reasoning_effort: None,
                gemini_thinking_budget: None,
                gemini_thinking_level: None,
                anthropic_thinking_budget: None,
            },
            crate::llm::ProgramPreset {
                id: "calendar".to_string(),
                name: "Calendar".to_string(),
                routing_hints: vec!["calendar hint".to_string()],
                prompts: PromptSections::default(),
                rewrite_llm_enabled: true,
                stt_provider: None,
                stt_model: None,
                stt_language: None,
                stt_timeout_seconds: None,
                llm_provider: None,
                llm_model: None,
                openai_reasoning_effort: None,
                gemini_thinking_budget: None,
                gemini_thinking_level: None,
                anthropic_thinking_budget: None,
            },
        ];
        profile.router = Some(IntentRouterSettings {
            enabled: true,
            strategy: IntentRouterStrategy::Embeddings,
            embedding_provider: Some("openai".to_string()),
            embedding_model: Some("fake-embedding-model".to_string()),
            pick_highest_score: false,
            similarity_threshold: Some(0.75),
            similarity_margin: Some(0.10),
            llm_provider: None,
            llm_model: None,
            llm_system_prompt: None,
            openai_reasoning_effort: None,
            gemini_thinking_budget: None,
            gemini_thinking_level: None,
            anthropic_thinking_budget: None,
        });
        profile
    }

    #[test]
    fn forced_llm_provider_model_take_precedence_over_profile() {
        let profile = minimal_profile_with_llm_overrides(Some("ollama"), Some("some-model"));
        let callbacks = RecordingCallbacks(std::sync::Mutex::new(None));

        let embedding_cache: std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        let ctx = TranscriptionContext {
            active_profile: Some(profile),
            active_window_ocr_text: None,
            llm_enabled_global: true,
            default_rewrite_include_clipboard_context: false,
            session_lock: None,
            proxy_settings: crate::settings::ProxySettings::default(),
            llm_api_keys: std::collections::HashMap::new(),
            request_log_store: None,
            embedding_cache: &embedding_cache,
            persist_app: None,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            injected_embeddings_provider: None,
            force_llm_rewrite: true,
            forced_llm_provider: Some("groq".to_string()),
            forced_llm_model: Some("llama-3.1-8b-instant".to_string()),
        };

        let llm_config = crate::llm::LlmConfig::default();
        let _ = resolve_llm_for_rewrite(&ctx, &callbacks, &None, &llm_config);

        let recorded = callbacks.0.lock().expect("lock").clone();
        assert_eq!(
            recorded,
            Some(("groq".to_string(), Some("llama-3.1-8b-instant".to_string())))
        );
    }

    #[tokio::test]
    async fn route_preset_consumes_routing_decision_and_records_outcome() {
        let profile = embeddings_router_profile();
        let callbacks = RoutingCallbacks::new();
        let request_log_store = RequestLogStore::new();
        request_log_store.start_request("mock-stt".to_string(), None);
        let embedding_cache: std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let provider = std::sync::Arc::new(FlowEmbeddingsProvider(
            [
                ("send email".to_string(), vec![1.0, 0.0]),
                ("email hint".to_string(), vec![0.95, 0.05]),
                ("calendar hint".to_string(), vec![0.0, 1.0]),
            ]
            .into_iter()
            .collect(),
        ));

        let ctx = TranscriptionContext {
            active_profile: Some(profile),
            active_window_ocr_text: None,
            llm_enabled_global: true,
            default_rewrite_include_clipboard_context: false,
            session_lock: None,
            proxy_settings: crate::settings::ProxySettings::default(),
            llm_api_keys: std::collections::HashMap::new(),
            request_log_store: Some(request_log_store.clone()),
            embedding_cache: &embedding_cache,
            persist_app: None,
            cancel_token: tokio_util::sync::CancellationToken::new(),
            injected_embeddings_provider: Some(provider),
            force_llm_rewrite: false,
            forced_llm_provider: None,
            forced_llm_model: None,
        };

        let result = route_preset(&ctx, &callbacks, "send email").await;

        assert_eq!(result.routed_preset_id.as_deref(), Some("email"));
        assert_eq!(
            callbacks.transitions.lock().expect("lock").as_slice(),
            ["to_routing", "from_routing"]
        );
        let response = request_log_store
            .with_current(|log| log.router_response_json.clone())
            .flatten()
            .expect("router response should be recorded");
        assert_eq!(response["outcome"], "selected_preset");
        assert_eq!(response["type"], "embeddings");
    }

    #[tokio::test]
    async fn cancellation_outranks_provider_work_in_rewrite_step() {
        let profile = minimal_profile_with_llm_overrides(None, None);
        let callbacks = RecordingCallbacks(std::sync::Mutex::new(None));
        let embedding_cache: std::sync::Arc<
            std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();
        let ctx = TranscriptionContext {
            active_profile: Some(profile),
            active_window_ocr_text: None,
            llm_enabled_global: true,
            default_rewrite_include_clipboard_context: false,
            session_lock: None,
            proxy_settings: crate::settings::ProxySettings::default(),
            llm_api_keys: std::collections::HashMap::new(),
            request_log_store: None,
            embedding_cache: &embedding_cache,
            persist_app: None,
            cancel_token,
            injected_embeddings_provider: None,
            force_llm_rewrite: true,
            forced_llm_provider: None,
            forced_llm_model: None,
        };

        let resolution = LlmResolution {
            provider: Some(std::sync::Arc::new(PanicLlmProvider)),
            prompts: PromptSections::default(),
            timeout: Duration::from_secs(30),
            not_attempted_reason: None,
        };

        let (final_text, duration, outcome, provider, model) =
            run_llm_rewrite(&ctx, &callbacks, "raw transcript", resolution).await;

        assert_eq!(final_text, "raw transcript");
        assert!(duration.is_some());
        assert!(matches!(
            outcome,
            LlmOutcome::NotAttempted(LlmNotAttemptedReason::Unknown)
        ));
        assert_eq!(provider.as_deref(), Some("panic"));
        assert_eq!(model.as_deref(), Some("panic-model"));
    }
}
