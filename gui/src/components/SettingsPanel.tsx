import { useEffect, useState } from "react";
import { Save, Play, Square, RefreshCw } from "lucide-react";
import { useSettingsStore, useAgentStore } from "../stores/appStore";
import { getSettings, saveSettings, getAgentStatus, startAgent, stopAgent } from "../hooks/useTauri";
import type { AppSettings } from "../types";

// ── Section Wrapper ───────────────────────────────────────────────────────────

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="space-y-3">
      <h3 className="text-xs font-semibold text-slate-400 uppercase tracking-wider">{title}</h3>
      <div className="glass p-4 space-y-4">{children}</div>
    </div>
  );
}

// ── Field ─────────────────────────────────────────────────────────────────────

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="flex-1 min-w-0">
        <div className="text-sm text-slate-200">{label}</div>
        {hint && <div className="text-xs text-slate-500 mt-0.5">{hint}</div>}
      </div>
      <div className="flex-shrink-0">{children}</div>
    </div>
  );
}

// ── Toggle ────────────────────────────────────────────────────────────────────

function Toggle({ value, onChange }: { value: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      onClick={() => onChange(!value)}
      className={`
        relative w-10 h-5 rounded-full transition-colors duration-200
        ${value ? "bg-obc-500" : "bg-surface-border"}
      `}
    >
      <span
        className={`
          absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-transform duration-200
          ${value ? "translate-x-5" : "translate-x-0.5"}
        `}
      />
    </button>
  );
}

// ── Settings Panel ────────────────────────────────────────────────────────────

export default function SettingsPanel() {
  const { settings, setSettings } = useSettingsStore();
  const { status, setStatus } = useAgentStore();
  const [draft, setDraft] = useState<AppSettings>(settings);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [agentLoading, setAgentLoading] = useState(false);

  const patch = (p: Partial<AppSettings>) => setDraft((d) => ({ ...d, ...p }));

  // Load settings from backend on mount
  useEffect(() => {
    getSettings()
      .then((s) => { setSettings(s); setDraft(s); })
      .catch(() => {});

    getAgentStatus()
      .then(setStatus)
      .catch(() => {});
  }, [setSettings, setStatus]);

  const handleSave = async () => {
    setSaving(true);
    try {
      await saveSettings(draft);
      setSettings(draft);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch {
      // ignore
    } finally {
      setSaving(false);
    }
  };

  const handleToggleAgent = async () => {
    setAgentLoading(true);
    try {
      if (status?.running) {
        await stopAgent();
        setStatus(null);
      } else {
        await startAgent(draft.provider, draft.model);
        const s = await getAgentStatus();
        setStatus(s);
      }
    } catch {
      // ignore
    } finally {
      setAgentLoading(false);
    }
  };

  const agentRunning = status?.running ?? false;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-surface-border flex-shrink-0">
        <div className="text-sm text-slate-400">
          Agent: {" "}
          <span className={agentRunning ? "text-emerald-400" : "text-slate-500"}>
            {agentRunning ? `Running · ${status?.provider}/${status?.model}` : "Stopped"}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleToggleAgent}
            disabled={agentLoading}
            className={agentRunning ? "btn-danger flex items-center gap-1.5 text-sm" : "btn-primary flex items-center gap-1.5 text-sm"}
          >
            {agentLoading ? (
              <RefreshCw size={14} className="animate-spin" />
            ) : agentRunning ? (
              <><Square size={14} /> Stop</>
            ) : (
              <><Play size={14} /> Start</>
            )}
          </button>
          <button
            onClick={handleSave}
            disabled={saving}
            className="btn-primary flex items-center gap-1.5 text-sm"
          >
            <Save size={14} />
            {saved ? "Saved!" : saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>

      {/* Settings form */}
      <div className="flex-1 overflow-y-auto p-4 space-y-6">

        {/* LLM Provider */}
        <Section title="LLM Provider">
          <Field label="Provider">
            <select
              value={draft.provider}
              onChange={(e) => patch({ provider: e.target.value })}
              className="obc-input text-sm w-44"
            >
              <option value="openai">OpenAI</option>
              <option value="anthropic">Anthropic</option>
              <option value="ollama">Ollama (local)</option>
              <option value="openrouter">OpenRouter</option>
              <option value="compatible">OpenAI-compatible</option>
            </select>
          </Field>
          <Field label="Model">
            <input
              type="text"
              value={draft.model}
              onChange={(e) => patch({ model: e.target.value })}
              placeholder="gpt-4o"
              className="obc-input text-sm w-44 selectable font-mono"
            />
          </Field>
          {draft.provider !== "ollama" && (
            <Field label="API Key" hint="Stored in memory only — use the Vault for persistence">
              <input
                type="password"
                value={draft.apiKey ?? ""}
                onChange={(e) => patch({ apiKey: e.target.value })}
                placeholder="sk-… or use Vault"
                className="obc-input text-sm w-44 selectable font-mono"
              />
            </Field>
          )}
          {draft.provider === "ollama" && (
            <Field label="Ollama URL">
              <input
                type="text"
                value={draft.ollamaUrl ?? "http://localhost:11434"}
                onChange={(e) => patch({ ollamaUrl: e.target.value })}
                className="obc-input text-sm w-44 selectable font-mono"
              />
            </Field>
          )}
        </Section>

        {/* Spine (MQTT) */}
        <Section title="Spine (MQTT Bus)">
          <Field label="Broker Host">
            <input
              type="text"
              value={draft.spineHost}
              onChange={(e) => patch({ spineHost: e.target.value })}
              className="obc-input text-sm w-44 selectable font-mono"
            />
          </Field>
          <Field label="Broker Port">
            <input
              type="number"
              value={draft.spinePort}
              onChange={(e) => patch({ spinePort: Number(e.target.value) })}
              className="obc-input text-sm w-24 selectable"
              min={1}
              max={65535}
            />
          </Field>
          <Field label="Require Node Pairing" hint="Verify HMAC-SHA256 tokens from peripheral nodes">
            <Toggle value={draft.requirePairing} onChange={(v) => patch({ requirePairing: v })} />
          </Field>
        </Section>

        {/* Security */}
        <Section title="Security">
          <Field label="Encrypted Vault" hint="Store API keys with AES-256-GCM encryption">
            <Toggle value={draft.vaultEnabled} onChange={(v) => patch({ vaultEnabled: v })} />
          </Field>
        </Section>

        {/* Application */}
        <Section title="Application">
          <Field label="Launch at Login" hint="Start Oh-Ben-Claw automatically on system startup">
            <Toggle value={draft.autostart} onChange={(v) => patch({ autostart: v })} />
          </Field>
          <Field label="Minimize to Tray" hint="Keep running in the system tray when window is closed">
            <Toggle value={draft.minimizeToTray} onChange={(v) => patch({ minimizeToTray: v })} />
          </Field>
        </Section>

      </div>
    </div>
  );
}
