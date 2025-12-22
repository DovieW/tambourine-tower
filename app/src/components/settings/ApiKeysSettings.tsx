import { ActionIcon, PasswordInput, Text, Tooltip } from "@mantine/core";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Pencil, X } from "lucide-react";
import { useState } from "react";
import { configAPI, tauriAPI } from "../../lib/tauri";

const GLOBAL_ONLY_TOOLTIP =
  "This setting can only be changed in the Default profile";

interface ApiKeyConfig {
  id: string;
  label: string;
  placeholder: string;
  storeKey: string;
}

const API_KEYS: ApiKeyConfig[] = [
  {
    id: "groq",
    label: "Groq",
    placeholder: "Enter API key",
    storeKey: "groq_api_key",
  },
  {
    id: "openai",
    label: "OpenAI",
    placeholder: "Enter API key",
    storeKey: "openai_api_key",
  },
  {
    id: "deepgram",
    label: "Deepgram",
    placeholder: "Enter API key",
    storeKey: "deepgram_api_key",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    placeholder: "Enter API key",
    storeKey: "anthropic_api_key",
  },
];

export const API_KEY_STORE_KEYS = API_KEYS.map((k) => k.storeKey);

function ApiKeyInput({ config }: { config: ApiKeyConfig }) {
  const queryClient = useQueryClient();
  const [value, setValue] = useState("");
  const [isEditing, setIsEditing] = useState(false);
  const [isPrefilling, setIsPrefilling] = useState(false);

  // Query to check if key is set
  const { data: hasKey } = useQuery({
    queryKey: ["apiKey", config.storeKey],
    queryFn: () => tauriAPI.hasApiKey(config.storeKey),
  });

  // Mutation to save key
  const saveKey = useMutation({
    mutationFn: async (key: string) => {
      await tauriAPI.setApiKey(config.storeKey, key);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["apiKey", config.storeKey] });
      queryClient.invalidateQueries({ queryKey: ["availableProviders"] });
      // Sync pipeline config when API keys change
      configAPI.syncPipelineConfig();
      setValue("");
      setIsEditing(false);
    },
  });

  // Mutation to clear key
  const clearKey = useMutation({
    mutationFn: async () => {
      await tauriAPI.clearApiKey(config.storeKey);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["apiKey", config.storeKey] });
      queryClient.invalidateQueries({ queryKey: ["availableProviders"] });
      configAPI.syncPipelineConfig();
      setValue("");
      setIsEditing(false);
    },
  });

  const handleSave = () => {
    if (value.trim()) {
      saveKey.mutate(value.trim());
    }
  };

  const handleCancel = () => {
    setValue("");
    setIsEditing(false);
  };

  const handleClear = () => {
    clearKey.mutate();
  };

  const startEditing = async () => {
    setIsEditing(true);

    // If a key exists, prefill the input so the built-in Show/Hide toggle
    // actually reveals something (instead of toggling an empty field).
    if (!hasKey) return;

    setIsPrefilling(true);
    try {
      const existingKey = await tauriAPI.getApiKey(config.storeKey);
      setValue(existingKey ?? "");
    } catch {
      // If reading fails for any reason, fall back to empty and let the user re-enter.
      setValue("");
    } finally {
      setIsPrefilling(false);
    }
  };

  if (!isEditing && hasKey) {
    return (
      <div className="settings-row">
        <div>
          <p className="settings-label">{config.label}</p>
          <p className="settings-description">API key configured</p>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <Text size="sm" c="teal">
            ✓ Set
          </Text>
          <Tooltip label="Change API key">
            <ActionIcon variant="subtle" color="gray" onClick={startEditing}>
              <Pencil size={16} />
            </ActionIcon>
          </Tooltip>
          <Tooltip label="Remove API key">
            <ActionIcon
              variant="subtle"
              color="red"
              onClick={handleClear}
              loading={clearKey.isPending}
            >
              <X size={16} />
            </ActionIcon>
          </Tooltip>
        </div>
      </div>
    );
  }

  return (
    <div className="settings-row">
      <div>
        <p className="settings-label">{config.label}</p>
        <p className="settings-description">
          {hasKey ? "Update your API key" : "Enter your API key"}
        </p>
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <PasswordInput
          value={value}
          onChange={(e) => setValue(e.currentTarget.value)}
          placeholder={config.placeholder}
          disabled={isPrefilling || saveKey.isPending || clearKey.isPending}
          styles={{
            input: {
              backgroundColor: "var(--bg-elevated)",
              borderColor: "var(--border-default)",
              color: "var(--text-primary)",
              width: 200,
            },
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleSave();
            if (e.key === "Escape") handleCancel();
          }}
        />
        <Tooltip label="Save">
          <ActionIcon
            variant="filled"
            color="teal"
            onClick={handleSave}
            loading={saveKey.isPending}
            disabled={!value.trim()}
          >
            <Check size={16} />
          </ActionIcon>
        </Tooltip>
        {(isEditing || hasKey) && (
          <Tooltip label="Cancel">
            <ActionIcon variant="subtle" color="gray" onClick={handleCancel}>
              <X size={16} />
            </ActionIcon>
          </Tooltip>
        )}
      </div>
    </div>
  );
}

export function ApiKeysSettings({
  editingProfileId,
}: {
  editingProfileId?: string;
}) {
  const isProfileScope = editingProfileId && editingProfileId !== "default";

  const content = (
    <>
      <Text size="sm" c="dimmed" mb="md">
        Add API keys to enable cloud providers. Keys are stored locally and
        never sent anywhere except to the provider's API.
      </Text>
      {API_KEYS.map((config) => (
        <ApiKeyInput key={config.id} config={config} />
      ))}
    </>
  );

  if (isProfileScope) {
    return (
      <Tooltip label={GLOBAL_ONLY_TOOLTIP} withArrow position="top-start">
        <div style={{ opacity: 0.5, cursor: "not-allowed" }}>
          <div style={{ pointerEvents: "none" }}>{content}</div>
        </div>
      </Tooltip>
    );
  }

  return content;
}
