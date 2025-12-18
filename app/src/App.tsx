import { Kbd, NavLink, Tabs, Text, Title, Tooltip } from "@mantine/core";
import { FileText, Home, Settings } from "lucide-react";
import { useEffect, useState } from "react";
import { HistoryFeed } from "./components/HistoryFeed";
import { Logo } from "./components/Logo";
import { LogsView } from "./components/LogsView";
import {
	ApiKeysSettings,
	AudioSettings,
	HotkeySettings,
	PromptSettings,
	ProvidersSettings,
} from "./components/settings";
import {
	DEFAULT_HOLD_HOTKEY,
	DEFAULT_PASTE_LAST_HOTKEY,
	DEFAULT_TOGGLE_HOTKEY,
} from "./lib/hotkeyDefaults";
import { useSettings } from "./lib/queries";
import { type ConnectionState, type HotkeyConfig, tauriAPI } from "./lib/tauri";
import "./styles.css";

type View = "home" | "settings" | "logs";

function ConnectionStatusIndicator() {
	const [state, setState] = useState<ConnectionState>("idle");

	// Listen for connection state changes from the overlay window
	useEffect(() => {
		let unlisten: (() => void) | undefined;

		const setup = async () => {
			unlisten = await tauriAPI.onConnectionStateChanged((newState) => {
				setState(newState);
			});
		};

		setup();

		return () => {
			unlisten?.();
		};
	}, []);

	const isReady =
		state === "idle" || state === "recording" || state === "processing";

	const statusText = isReady ? "Ready" : "Initializing...";

	return (
		<Tooltip label={statusText} position="right" withArrow>
			<div className="connection-status">
				<span
					className={`connection-status-dot ${isReady ? "connected" : "disconnected"}`}
				/>
			</div>
		</Tooltip>
	);
}

function Sidebar({
	activeView,
	onViewChange,
}: {
	activeView: View;
	onViewChange: (view: View) => void;
}) {
	return (
		<aside className="sidebar">
			<header className="sidebar-header">
				<div className="sidebar-logo">
					<Logo size={32} />
				</div>
			</header>

			<nav className="sidebar-nav">
				<Tooltip label="Home" position="right" withArrow>
					<NavLink
						leftSection={<Home size={20} />}
						active={activeView === "home"}
						onClick={() => onViewChange("home")}
						variant="filled"
						className="sidebar-nav-link"
					/>
				</Tooltip>
				<Tooltip label="Settings" position="right" withArrow>
					<NavLink
						leftSection={<Settings size={20} />}
						active={activeView === "settings"}
						onClick={() => onViewChange("settings")}
						variant="filled"
						className="sidebar-nav-link"
					/>
				</Tooltip>
				<Tooltip label="Logs" position="right" withArrow>
					<NavLink
						leftSection={<FileText size={20} />}
						active={activeView === "logs"}
						onClick={() => onViewChange("logs")}
						variant="filled"
						className="sidebar-nav-link"
					/>
				</Tooltip>
			</nav>

			<footer className="sidebar-footer">
				<ConnectionStatusIndicator />
				<p className="sidebar-footer-text">v0.1.0</p>
			</footer>
		</aside>
	);
}

function HotkeyDisplay({ config }: { config: HotkeyConfig }) {
	const parts = [
		...config.modifiers.map((m) => m.charAt(0).toUpperCase() + m.slice(1)),
		config.key,
	];

	return (
		<span className="kbd-combo">
			{parts.map((part, index) => (
				<span key={part}>
					<Kbd>{part}</Kbd>
					{index < parts.length - 1 && <span className="kbd-plus">+</span>}
				</span>
			))}
		</span>
	);
}

function InstructionsCard() {
	const { data: settings } = useSettings();

	const toggleHotkey = settings?.toggle_hotkey ?? DEFAULT_TOGGLE_HOTKEY;
	const holdHotkey = settings?.hold_hotkey ?? DEFAULT_HOLD_HOTKEY;
	const pasteLastHotkey =
		settings?.paste_last_hotkey ?? DEFAULT_PASTE_LAST_HOTKEY;

	return (
		<div className="instructions-card animate-in">
			<h2 className="instructions-card-title">Dictate with your voice</h2>
			<div className="instructions-methods">
				<div className="instruction-method">
					<span className="instruction-label">Toggle:</span>
					<HotkeyDisplay config={toggleHotkey} />
					<span className="instruction-desc">Press to start/stop</span>
				</div>
				<div className="instruction-method">
					<span className="instruction-label">Hold:</span>
					<HotkeyDisplay config={holdHotkey} />
					<span className="instruction-desc">Hold to record</span>
				</div>
				<div className="instruction-method">
					<span className="instruction-label">Paste:</span>
					<HotkeyDisplay config={pasteLastHotkey} />
					<span className="instruction-desc">Paste last result</span>
				</div>
			</div>
			<p className="instructions-card-text">
				Speak clearly and your words will be typed wherever your cursor is. The
				overlay appears in the bottom-right corner of your screen.
			</p>
		</div>
	);
}

function HomeView() {
	return (
		<div className="main-content">
			<header className="animate-in" style={{ marginBottom: 32 }}>
				<Title order={1} mb={4}>
					Welcome to Tambourine
				</Title>
				<Text c="dimmed" size="sm">
					~-~-~-~-~-~
				</Text>
			</header>

			<InstructionsCard />

			<HistoryFeed />
		</div>
	);
}

function SettingsView() {
	return (
		<div className="main-content">
			<header className="animate-in" style={{ marginBottom: 32 }}>
				<Title order={1} mb={4}>
					Settings
				</Title>
				<Text c="dimmed" size="sm">
					Configure your preferences
				</Text>
			</header>

			<Tabs
				defaultValue="api-keys"
				classNames={{ root: "settings-tabs" }}
				keepMounted={false}
			>
				<Tabs.List>
					<Tabs.Tab value="api-keys">API Keys</Tabs.Tab>
					<Tabs.Tab value="providers">Providers</Tabs.Tab>
					<Tabs.Tab value="audio">Audio &amp; Overlay</Tabs.Tab>
					<Tabs.Tab value="hotkeys">Hotkeys</Tabs.Tab>
					<Tabs.Tab value="prompts">Prompts</Tabs.Tab>
				</Tabs.List>

				<Tabs.Panel value="api-keys" pt="md">
					<div className="settings-card">
						<ApiKeysSettings />
					</div>
				</Tabs.Panel>

				<Tabs.Panel value="providers" pt="md">
					<div className="settings-card">
						<ProvidersSettings />
					</div>
				</Tabs.Panel>

				<Tabs.Panel value="audio" pt="md">
					<div className="settings-card">
						<AudioSettings />
					</div>
				</Tabs.Panel>

				<Tabs.Panel value="hotkeys" pt="md">
					<div className="settings-card">
						<HotkeySettings />
					</div>
				</Tabs.Panel>

				<Tabs.Panel value="prompts" pt="md">
					<div className="settings-card">
						<PromptSettings />
					</div>
				</Tabs.Panel>
			</Tabs>
		</div>
	);
}

export default function App() {
	const [activeView, setActiveView] = useState<View>("home");

	const renderView = () => {
		switch (activeView) {
			case "home":
				return <HomeView />;
			case "settings":
				return <SettingsView />;
			case "logs":
				return <LogsView />;
			default:
				return <HomeView />;
		}
	};

	return (
		<div className="app-layout">
			<Sidebar activeView={activeView} onViewChange={setActiveView} />
			{renderView()}
		</div>
	);
}
