import {
  Anchor,
  Button,
  Group,
  Kbd,
  PasswordInput,
  Text,
  Textarea,
  Title,
} from "@mantine/core";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Logo } from "../Logo";
import { configAPI, type HotkeyConfig, tauriAPI } from "../../lib/tauri";

type Phase = "welcome" | "guide";

type Step = "groq" | "dictation" | "wrapup";

type NavStep = "welcome" | Step;

const GUIDE_STEPS: Step[] = ["groq", "dictation", "wrapup"];
const NAV_STEPS: NavStep[] = ["welcome", ...GUIDE_STEPS];

function HotkeyCombo({ config }: { config: HotkeyConfig }) {
  const parts = useMemo(() => {
    const mods = config.modifiers.map(
      (m) => m.charAt(0).toUpperCase() + m.slice(1)
    );
    return [...mods, config.key];
  }, [config]);

  return (
    <span className="tang-guide-kbd-combo">
      {parts.map((part, idx) => (
        <span key={`${part}-${idx}`}>
          <Kbd>{part}</Kbd>
          {idx < parts.length - 1 && <span className="kbd-plus">+</span>}
        </span>
      ))}
    </span>
  );
}

export function SettingsGuideOverlay({
  opened,
  holdHotkey,
  onSkip,
  onFinished,
  onGoHome,
}: {
  opened: boolean;
  holdHotkey: HotkeyConfig | null;
  onSkip: () => void;
  onFinished: () => void;
  onGoHome: () => void;
}) {
  const queryClient = useQueryClient();

  const welcomeTimersRef = useRef<number[]>([]);

  const [phase, setPhase] = useState<Phase>("welcome");
  const [step, setStep] = useState<Step>("groq");

  const [welcomeIconVisible, setWelcomeIconVisible] = useState(false);
  const [welcomeTextVisible, setWelcomeTextVisible] = useState(false);
  const [welcomeFadingOut, setWelcomeFadingOut] = useState(false);
  const [welcomeContinueVisible, setWelcomeContinueVisible] = useState(false);
  const [welcomeContinueSeen, setWelcomeContinueSeen] = useState(false);

  const [skipVisible, setSkipVisible] = useState(false);

  const { data: groqApiKeyValue } = useQuery({
    queryKey: ["apiKeyValue", "groq_api_key"],
    enabled: opened,
    queryFn: () => tauriAPI.getApiKey("groq_api_key"),
    staleTime: 0,
  });

  const [groqKeyValue, setGroqKeyValue] = useState("");
  const [isSavingGroqKey, setIsSavingGroqKey] = useState(false);

  const trimmedGroqKeyValue = groqKeyValue.trim();
  const savedGroqKeyValue = (groqApiKeyValue ?? "").trim();
  const isGroqKeyUnchanged =
    savedGroqKeyValue.length > 0 && trimmedGroqKeyValue === savedGroqKeyValue;

  const hasHydratedGroqKeyRef = useRef(false);

  const [finishVisible, setFinishVisible] = useState(false);
  const [finishSeen, setFinishSeen] = useState(false);

  const [dictationText, setDictationText] = useState("");
  const dictationInputRef = useRef<HTMLTextAreaElement | null>(null);

  const sampleText =
    "Tangerine is ready. I can dictate with my voice, rewrite text, and tune settings per app.";

  const clearWelcomeTimers = () => {
    for (const t of welcomeTimersRef.current) window.clearTimeout(t);
    welcomeTimersRef.current = [];
  };

  const enterGuideAt = (nextStep: Step) => {
    clearWelcomeTimers();
    setPhase("guide");
    setStep(nextStep);
    setSkipVisible(true);
  };

  const restartWelcomeSequence = () => {
    clearWelcomeTimers();

    // Always restart the intro from scratch.
    setPhase("welcome");
    setStep("groq");
    setSkipVisible(false);

    setWelcomeIconVisible(false);
    setWelcomeTextVisible(false);
    setWelcomeFadingOut(false);
    setWelcomeContinueVisible(welcomeContinueSeen);

    const timers: Array<number> = [];
    timers.push(window.setTimeout(() => setWelcomeIconVisible(true), 150));
    timers.push(window.setTimeout(() => setWelcomeTextVisible(true), 650));
    // Reveal the Continue button after the title/subtext have faded in,
    // then held on-screen for ~2s.
    if (!welcomeContinueSeen) {
      timers.push(
        window.setTimeout(() => {
          setWelcomeContinueSeen(true);
          setWelcomeContinueVisible(true);
        }, 2930)
      );
    }

    welcomeTimersRef.current = timers;
  };

  useEffect(() => {
    if (!opened) return;

    // Reset guide state on open.
    restartWelcomeSequence();

    setGroqKeyValue("");
    hasHydratedGroqKeyRef.current = false;

    setFinishVisible(false);
    setFinishSeen(false);
    setWelcomeContinueSeen(false);
    setDictationText("");
    return () => {
      clearWelcomeTimers();
    };
  }, [opened]);

  useEffect(() => {
    if (!opened) return;
    if (hasHydratedGroqKeyRef.current) return;
    if (!groqApiKeyValue) return;

    // If a key already exists, show it in the PasswordInput (hidden by default).
    setGroqKeyValue(groqApiKeyValue);
    hasHydratedGroqKeyRef.current = true;
  }, [opened, groqApiKeyValue]);

  useEffect(() => {
    if (!opened) return;

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      // Escape exits the setup guide entirely.
      onSkip();
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [opened, onSkip]);

  useEffect(() => {
    if (!opened) return;
    if (phase !== "guide") return;
    if (step !== "dictation") return;

    // Give the user a target to dictate into.
    dictationInputRef.current?.focus();
  }, [opened, phase, step]);

  useEffect(() => {
    if (!opened) return;
    if (step !== "wrapup") {
      setFinishVisible(false);
      return;
    }

    if (finishSeen) {
      setFinishVisible(true);
      return;
    }

    // Match the welcome slide timing: wait for content to be fully faded in (~280ms),
    // then hold for ~2s before revealing the action.
    const t = window.setTimeout(() => {
      setFinishSeen(true);
      setFinishVisible(true);
    }, 2280);
    return () => window.clearTimeout(t);
  }, [opened, step, finishSeen]);

  const nextStep = () => {
    // Bottom-right action advances to the next page.
    goForward();
  };

  const navStep: NavStep = phase === "welcome" ? "welcome" : step;
  const navIndex = NAV_STEPS.indexOf(navStep);

  const canGoBack = navIndex > 0;
  const canGoForward = (() => {
    if (navIndex < 0) return false;
    if (navIndex >= NAV_STEPS.length - 1) return false;

    // From the welcome slide, always allow moving forward.
    if (navStep === "welcome") return true;

    return true;
  })();

  const goBack = () => {
    if (!canGoBack) return;

    const next = NAV_STEPS[navIndex - 1];
    if (!next) return;

    if (next === "welcome") {
      restartWelcomeSequence();
      return;
    }

    enterGuideAt(next);
  };

  const goForward = () => {
    if (!canGoForward) return;

    const next = NAV_STEPS[navIndex + 1];
    if (!next) return;

    if (next === "welcome") {
      restartWelcomeSequence();
      return;
    }

    enterGuideAt(next);
  };

  const handleSaveGroqKey = async () => {
    const trimmed = groqKeyValue.trim();
    if (!trimmed) return;

    setIsSavingGroqKey(true);
    try {
      await tauriAPI.setApiKey("groq_api_key", trimmed);
      await configAPI.syncPipelineConfig();
      await queryClient.invalidateQueries({
        queryKey: ["apiKey", "groq_api_key"],
      });
      await queryClient.invalidateQueries({
        queryKey: ["apiKeyValue", "groq_api_key"],
      });
      await queryClient.invalidateQueries({ queryKey: ["availableProviders"] });

      // Keep the value in-state so if the user navigates back in this same
      // guide session, the field stays prefilled.
      setGroqKeyValue(trimmed);
      hasHydratedGroqKeyRef.current = true;
      setStep("dictation");
    } catch (err) {
      console.error("Failed to save Groq key", err);
    } finally {
      setIsSavingGroqKey(false);
    }
  };

  if (!opened) return null;

  return (
    <div className="tang-guide-overlay" role="dialog" aria-modal="true">
      {phase === "welcome" && (
        <div
          className={
            "tang-guide-welcome" +
            (welcomeFadingOut ? " tang-guide-welcome--fade-out" : "")
          }
        >
          <div className="tang-guide-welcome-center">
            <div
              className={
                "tang-guide-welcome-logo" +
                (welcomeIconVisible
                  ? " tang-guide-fade-in tang-guide-fade-in-slow"
                  : "")
              }
            >
              <Logo size={140} />
            </div>
            <div
              className={
                "tang-guide-welcome-text" +
                (welcomeTextVisible
                  ? " tang-guide-fade-in tang-guide-fade-in-slow"
                  : "")
              }
            >
              <Title order={2} style={{ marginTop: 18 }}>
                Welcome to Tangerine
              </Title>
              <Text c="dimmed" size="sm" style={{ marginTop: 6 }}>
                Let’s get your voice dictation set up.
              </Text>
            </div>
          </div>

          <button
            type="button"
            className={
              "tang-guide-continue" +
              (welcomeContinueVisible || welcomeContinueSeen
                ? " tang-guide-fade-in"
                : "")
            }
            onClick={() => enterGuideAt("groq")}
          >
            <span>Start</span>
            <ChevronRight size={16} />
          </button>
        </div>
      )}

      {phase === "guide" && (
        <>
          {skipVisible && navIndex < NAV_STEPS.length - 1 && (
            <button
              type="button"
              className="tang-guide-skip tang-guide-fade-in"
              onClick={nextStep}
            >
              <span>Next</span>
              <ChevronRight size={16} />
            </button>
          )}

          {canGoBack && (
            <button
              type="button"
              className="tang-guide-back tang-guide-fade-in"
              onClick={goBack}
            >
              <ChevronLeft size={16} />
              <span>Back</span>
            </button>
          )}

          <div className="tang-guide-content tang-guide-fade-in">
            {step === "groq" && (
              <div className="tang-guide-step">
                <Title order={3}>Create a Groq API key</Title>
                <Text c="dimmed" size="sm" style={{ marginTop: 8 }}>
                  Groq provides free voice dictation (Whisper) and fast LLM
                  rewriting. Create an API key here:{" "}
                  <Anchor
                    href="https://console.groq.com/keys"
                    target="_blank"
                    rel="noreferrer"
                  >
                    https://console.groq.com/keys
                  </Anchor>
                </Text>

                <div style={{ marginTop: 18 }}>
                  <div style={{ marginTop: 12 }}>
                    <PasswordInput
                      value={groqKeyValue}
                      onChange={(e) => setGroqKeyValue(e.currentTarget.value)}
                      placeholder="Paste your Groq API key"
                      autoFocus
                      styles={{
                        input: {
                          backgroundColor: "var(--bg-elevated)",
                          borderColor: "var(--border-default)",
                          color: "var(--text-primary)",
                        },
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") handleSaveGroqKey();
                      }}
                    />
                    <Group justify="flex-end" mt="sm">
                      <Button
                        color="orange"
                        onClick={handleSaveGroqKey}
                        loading={isSavingGroqKey}
                        disabled={
                          !trimmedGroqKeyValue ||
                          isSavingGroqKey ||
                          isGroqKeyUnchanged
                        }
                      >
                        Set key
                      </Button>
                    </Group>
                  </div>
                </div>
              </div>
            )}

            {step === "dictation" && (
              <div className="tang-guide-step">
                <Title order={3}>Voice dictation test</Title>
                <Text c="dimmed" size="sm" style={{ marginTop: 8 }}>
                  Use your hold-to-record shortcut{" "}
                  {holdHotkey ? <HotkeyCombo config={holdHotkey} /> : null}.
                  Hold it while you speak, then release.
                </Text>

                <Text c="dimmed" size="sm" style={{ marginTop: 8 }}>
                  Click the box below so it’s focused, then dictate into it.
                </Text>

                <div className="tang-guide-copy" style={{ marginTop: 14 }}>
                  <Text size="sm" style={{ marginBottom: 6, opacity: 0.9 }}>
                    Say something like:
                  </Text>
                  <div className="tang-guide-copy-box">
                    <Text size="sm">{sampleText}</Text>
                  </div>
                </div>

                <div style={{ marginTop: 14 }}>
                  <Textarea
                    ref={dictationInputRef}
                    value={dictationText}
                    onChange={(e) => setDictationText(e.currentTarget.value)}
                    placeholder="Dictate here…"
                    minRows={4}
                    autosize
                    styles={{
                      input: {
                        backgroundColor: "var(--bg-elevated)",
                        borderColor: "var(--border-default)",
                        color: "var(--text-primary)",
                      },
                    }}
                  />
                </div>

                <Group justify="flex-end" mt="md">
                  <Button variant="default" onClick={() => setStep("wrapup")}>
                    Done
                  </Button>
                </Group>
              </div>
            )}

            {step === "wrapup" && (
              <div className="tang-guide-step">
                <Title order={3}>You’re good to go</Title>
                <Text c="dimmed" size="sm" style={{ marginTop: 8 }}>
                  Next, explore Settings to add AI providers, add an LLM rewrite
                  step with custom prompts, change settings per program, adjust
                  the accent color, and more.
                </Text>
              </div>
            )}
          </div>

          {step === "wrapup" && (
            <button
              type="button"
              className={
                "tang-guide-finish" +
                (finishSeen || finishVisible ? " tang-guide-fade-in" : "")
              }
              onClick={() => {
                onFinished();
                onGoHome();
              }}
              disabled={!finishSeen && !finishVisible}
            >
              Finish
            </button>
          )}
        </>
      )}
    </div>
  );
}
