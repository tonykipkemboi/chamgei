import { useState } from "react";

type Tab = "general" | "inference" | "audio" | "hotkeys";

interface Settings {
  activationMode: "push_to_talk" | "toggle";
  llmProvider: "cerebras" | "groq" | "local";
  groqApiKey: string;
  cerebrasApiKey: string;
  whisperModel: "tiny" | "small" | "medium" | "large";
  vadThreshold: number;
  injectionMethod: "clipboard" | "native";
}

const defaultSettings: Settings = {
  activationMode: "push_to_talk",
  llmProvider: "cerebras",
  groqApiKey: "",
  cerebrasApiKey: "",
  whisperModel: "small",
  vadThreshold: 0.5,
  injectionMethod: "clipboard",
};

function App() {
  const [activeTab, setActiveTab] = useState<Tab>("general");
  const [settings, setSettings] = useState<Settings>(defaultSettings);

  const tabs: { id: Tab; label: string }[] = [
    { id: "general", label: "General" },
    { id: "inference", label: "Inference" },
    { id: "audio", label: "Audio" },
    { id: "hotkeys", label: "Hotkeys" },
  ];

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSettings((prev) => ({ ...prev, [key]: value }));
  };

  return (
    <div className="flex flex-col h-screen select-none">
      {/* Header */}
      <header className="flex items-center gap-2 px-5 py-3 bg-[var(--bg-secondary)] border-b border-[var(--border)]">
        <span className="text-lg font-bold tracking-wide text-[var(--accent)]">
          Chamgei
        </span>
        <span className="text-xs text-[var(--text-secondary)]">v0.1.0</span>
      </header>

      {/* Tabs */}
      <nav className="flex gap-0 bg-[var(--bg-secondary)] border-b border-[var(--border)]">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`px-5 py-2.5 text-sm font-medium transition-colors cursor-pointer border-b-2 ${
              activeTab === tab.id
                ? "border-[var(--accent)] text-[var(--accent)]"
                : "border-transparent text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            }`}
          >
            {tab.label}
          </button>
        ))}
      </nav>

      {/* Content */}
      <main className="flex-1 overflow-y-auto p-5">
        {activeTab === "general" && (
          <GeneralTab settings={settings} update={update} />
        )}
        {activeTab === "inference" && (
          <InferenceTab settings={settings} update={update} />
        )}
        {activeTab === "audio" && (
          <AudioTab settings={settings} update={update} />
        )}
        {activeTab === "hotkeys" && <HotkeysTab settings={settings} />}
      </main>
    </div>
  );
}

// --- Tab Components ---

interface TabProps {
  settings: Settings;
  update: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
}

function GeneralTab({ settings, update }: TabProps) {
  return (
    <div className="space-y-5">
      <Section title="Activation Mode">
        <div className="flex gap-3">
          <ToggleButton
            active={settings.activationMode === "push_to_talk"}
            onClick={() => update("activationMode", "push_to_talk")}
          >
            Push to Talk
          </ToggleButton>
          <ToggleButton
            active={settings.activationMode === "toggle"}
            onClick={() => update("activationMode", "toggle")}
          >
            Toggle
          </ToggleButton>
        </div>
        <p className="mt-2 text-xs text-[var(--text-secondary)]">
          {settings.activationMode === "push_to_talk"
            ? "Hold the hotkey to record, release to stop."
            : "Press once to start recording, press again to stop."}
        </p>
      </Section>

      <Section title="Text Injection">
        <Select
          value={settings.injectionMethod}
          onChange={(v) =>
            update("injectionMethod", v as Settings["injectionMethod"])
          }
          options={[
            { value: "clipboard", label: "Clipboard (paste)" },
            { value: "native", label: "Native (keystroke sim)" },
          ]}
        />
      </Section>
    </div>
  );
}

function InferenceTab({ settings, update }: TabProps) {
  return (
    <div className="space-y-5">
      <Section title="LLM Provider">
        <Select
          value={settings.llmProvider}
          onChange={(v) => update("llmProvider", v as Settings["llmProvider"])}
          options={[
            { value: "cerebras", label: "Cerebras" },
            { value: "groq", label: "Groq" },
            { value: "local", label: "Local (no LLM)" },
          ]}
        />
      </Section>

      {settings.llmProvider === "cerebras" && (
        <Section title="Cerebras API Key">
          <input
            type="password"
            value={settings.cerebrasApiKey}
            onChange={(e) => update("cerebrasApiKey", e.target.value)}
            placeholder="csk-..."
            className="w-full px-3 py-2 rounded bg-[var(--input-bg)] border border-[var(--border)] text-sm text-[var(--text-primary)] placeholder:text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)]"
          />
        </Section>
      )}

      {settings.llmProvider === "groq" && (
        <Section title="Groq API Key">
          <input
            type="password"
            value={settings.groqApiKey}
            onChange={(e) => update("groqApiKey", e.target.value)}
            placeholder="gsk_..."
            className="w-full px-3 py-2 rounded bg-[var(--input-bg)] border border-[var(--border)] text-sm text-[var(--text-primary)] placeholder:text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)]"
          />
        </Section>
      )}

      <Section title="Whisper Model">
        <Select
          value={settings.whisperModel}
          onChange={(v) =>
            update("whisperModel", v as Settings["whisperModel"])
          }
          options={[
            { value: "tiny", label: "Tiny — fastest, least accurate" },
            { value: "small", label: "Small — balanced (recommended)" },
            { value: "medium", label: "Medium — slower, more accurate" },
            { value: "large", label: "Large — slowest, most accurate" },
          ]}
        />
      </Section>
    </div>
  );
}

function AudioTab({ settings, update }: TabProps) {
  return (
    <div className="space-y-5">
      <Section title="VAD Sensitivity">
        <div className="flex items-center gap-3">
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            value={settings.vadThreshold}
            onChange={(e) => update("vadThreshold", parseFloat(e.target.value))}
            className="flex-1 accent-[var(--accent)]"
          />
          <span className="text-sm font-mono w-10 text-right">
            {settings.vadThreshold.toFixed(2)}
          </span>
        </div>
        <p className="mt-1 text-xs text-[var(--text-secondary)]">
          Lower = more sensitive (picks up quieter speech). Higher = less
          sensitive (ignores background noise).
        </p>
      </Section>
    </div>
  );
}

function HotkeysTab({ settings: _settings }: { settings: Settings }) {
  return (
    <div className="space-y-5">
      <Section title="Dictation Hotkey">
        <div className="flex items-center gap-3">
          <kbd className="px-4 py-2 rounded bg-[var(--input-bg)] border border-[var(--border)] text-sm font-mono text-[var(--accent)]">
            Ctrl + Shift + Space
          </kbd>
          <button className="px-3 py-1.5 text-xs rounded bg-[var(--bg-secondary)] border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:border-[var(--accent)] transition-colors cursor-pointer">
            Change
          </button>
        </div>
        <p className="mt-2 text-xs text-[var(--text-secondary)]">
          Hotkey customization coming soon.
        </p>
      </Section>
    </div>
  );
}

// --- Shared UI Components ---

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="p-4 rounded-lg bg-[var(--bg-card)] border border-[var(--border)]">
      <h3 className="text-sm font-semibold mb-3 text-[var(--text-primary)]">
        {title}
      </h3>
      {children}
    </div>
  );
}

function Select({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full px-3 py-2 rounded bg-[var(--input-bg)] border border-[var(--border)] text-sm text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)] cursor-pointer"
    >
      {options.map((opt) => (
        <option key={opt.value} value={opt.value}>
          {opt.label}
        </option>
      ))}
    </select>
  );
}

function ToggleButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`px-4 py-2 text-sm rounded border transition-colors cursor-pointer ${
        active
          ? "bg-[var(--accent)] border-[var(--accent)] text-white"
          : "bg-[var(--input-bg)] border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--accent)]"
      }`}
    >
      {children}
    </button>
  );
}

export default App;
