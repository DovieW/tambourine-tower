import {
  Accordion,
  ActionIcon,
  Button,
  Switch,
  Text,
  Textarea,
  Tooltip,
} from "@mantine/core";
import { Info, RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";

export interface PromptSectionEditorProps {
  sectionKey: string;
  title: string;
  description: string;
  enabled: boolean;
  initialContent: string;
  defaultContent: string;
  hasCustom: boolean;
  inheritMode?: "inheriting" | "overriding" | null;
  onDisableOverride?: () => void;
  inheritTooltip?: string;
  disableOverrideTooltip?: string;
  helpText?: string;
  placeholder?: string;
  resetLabel?: string;
  minRows?: number;
  maxRows?: number;
  hideToggle?: boolean;
  onToggle: (enabled: boolean) => void;
  onSave: (content: string) => void;
  onReset: () => void;
  isSaving: boolean;
}

export function PromptSectionEditor({
  sectionKey,
  title,
  description,
  enabled,
  initialContent,
  defaultContent,
  hasCustom,
  inheritMode = null,
  onDisableOverride,
  inheritTooltip = "Inheriting from Default profile",
  disableOverrideTooltip = "Disable override (inherit from Default)",
  helpText,
  placeholder,
  resetLabel = "Reset to Default",
  minRows = 6,
  maxRows = 15,
  hideToggle = false,
  onToggle,
  onSave,
  onReset,
  isSaving,
}: PromptSectionEditorProps) {
  const [content, setContent] = useState(initialContent);
  const [hasChanges, setHasChanges] = useState(false);

  const showInheritInfo = inheritMode === "inheriting";
  const showDisableOverride =
    inheritMode === "overriding" && !!onDisableOverride;

  // Sync local content when initialContent changes (e.g., after reset)
  useEffect(() => {
    setContent(initialContent);
    setHasChanges(false);
  }, [initialContent]);

  const handleContentChange = (value: string) => {
    setContent(value);
    setHasChanges(true);
  };

  const handleSave = () => {
    onSave(content);
    setHasChanges(false);
  };

  const handleReset = () => {
    setContent(defaultContent);
    onReset();
    setHasChanges(false);
  };

  return (
    <Accordion.Item value={sectionKey}>
      <Accordion.Control>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            width: "100%",
            paddingRight: 12,
          }}
        >
          <div>
            <p className="settings-label">{title}</p>
            <p className="settings-description">{description}</p>
          </div>
          {(showInheritInfo || showDisableOverride || !hideToggle) && (
            <div
              style={{ display: "flex", alignItems: "center", gap: 8 }}
              // NOTE: Do NOT stop propagation in capture phase here.
              // If we stop in capture phase on the wrapper, events will never reach
              // the inner controls (reload icon/switch). Instead, each control stops
              // propagation on its own capture handlers.
            >
              {showInheritInfo && (
                <Tooltip label={inheritTooltip} withArrow>
                  <Info size={14} style={{ opacity: 0.5, flexShrink: 0 }} />
                </Tooltip>
              )}

              {showDisableOverride && (
                <Tooltip label={disableOverrideTooltip} withArrow>
                  <ActionIcon
                    component="span"
                    role="button"
                    tabIndex={0}
                    variant="subtle"
                    color="gray"
                    size="sm"
                    onPointerDownCapture={(e) => {
                      // Trigger in capture phase: the Accordion control is a button and
                      // React stopPropagation in capture can prevent bubble handlers
                      // from firing on this element.
                      e.preventDefault();
                      e.stopPropagation();
                      onDisableOverride?.();
                    }}
                    onMouseDownCapture={(e) => {
                      // Fallback for environments without pointer events.
                      e.preventDefault();
                      e.stopPropagation();
                      onDisableOverride?.();
                    }}
                    onClickCapture={(e) => {
                      // Defensive: avoid accordion toggle on click.
                      e.preventDefault();
                      e.stopPropagation();
                    }}
                    onKeyDownCapture={(e) => {
                      if (e.key !== "Enter" && e.key !== " ") return;
                      e.preventDefault();
                      e.stopPropagation();
                      onDisableOverride?.();
                    }}
                  >
                    <RotateCcw size={14} style={{ opacity: 0.65 }} />
                  </ActionIcon>
                </Tooltip>
              )}

              {!hideToggle && (
                <span
                  // IMPORTANT: Don't stop propagation in *capture* phase here.
                  // Doing so can prevent the Switch's underlying input from
                  // receiving pointer events, which makes toggles flaky.
                  onPointerDown={(e) => {
                    e.stopPropagation();
                  }}
                  onMouseDown={(e) => {
                    e.stopPropagation();
                  }}
                  onClick={(e) => {
                    e.stopPropagation();
                  }}
                >
                  <Switch
                    checked={enabled}
                    onChange={(e) => {
                      e.stopPropagation();
                      onToggle(e.currentTarget.checked);
                    }}
                    color="gray"
                    size="md"
                  />
                </span>
              )}
            </div>
          )}
        </div>
      </Accordion.Control>
      <Accordion.Panel>
        {helpText && (
          <Text size="xs" c="dimmed" mb="sm">
            {helpText}
          </Text>
        )}
        <Textarea
          value={content}
          onChange={(e) => handleContentChange(e.currentTarget.value)}
          placeholder={placeholder}
          minRows={minRows}
          maxRows={maxRows}
          autosize
          styles={{
            input: {
              backgroundColor: "var(--bg-elevated)",
              borderColor: "var(--border-default)",
              color: "var(--text-primary)",
              fontFamily: "monospace",
              fontSize: "13px",
            },
          }}
        />
        <div
          style={{
            display: "flex",
            gap: 12,
            marginTop: 16,
            justifyContent: "flex-end",
          }}
        >
          <Button
            variant="subtle"
            color="gray"
            onClick={handleReset}
            disabled={!hasCustom}
          >
            {resetLabel}
          </Button>
          <Button
            color="gray"
            onClick={handleSave}
            disabled={!hasChanges}
            loading={isSaving}
          >
            Save
          </Button>
        </div>
      </Accordion.Panel>
    </Accordion.Item>
  );
}
