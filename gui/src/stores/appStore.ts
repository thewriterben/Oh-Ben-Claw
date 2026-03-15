import { create } from "zustand";
import type {
  AgentStatus,
  AppSettings,
  ChatMessage,
  PeripheralNode,
  Session,
  ToolCallEntry,
  VaultStatus,
} from "../types";

// ── Chat Store ────────────────────────────────────────────────────────────────

interface ChatState {
  messages: ChatMessage[];
  sessions: Session[];
  activeSessionId: string;
  isThinking: boolean;
  addMessage: (msg: ChatMessage) => void;
  setMessages: (msgs: ChatMessage[]) => void;
  setSessions: (sessions: Session[]) => void;
  setActiveSession: (id: string) => void;
  setThinking: (v: boolean) => void;
  clearMessages: () => void;
}

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  sessions: [],
  activeSessionId: "default",
  isThinking: false,
  addMessage: (msg) =>
    set((s) => ({ messages: [...s.messages, msg] })),
  setMessages: (messages) => set({ messages }),
  setSessions: (sessions) => set({ sessions }),
  setActiveSession: (activeSessionId) => set({ activeSessionId }),
  setThinking: (isThinking) => set({ isThinking }),
  clearMessages: () => set({ messages: [] }),
}));

// ── Peripheral Nodes Store ────────────────────────────────────────────────────

interface NodesState {
  nodes: PeripheralNode[];
  setNodes: (nodes: PeripheralNode[]) => void;
  updateNode: (id: string, patch: Partial<PeripheralNode>) => void;
}

export const useNodesStore = create<NodesState>((set) => ({
  nodes: [],
  setNodes: (nodes) => set({ nodes }),
  updateNode: (id, patch) =>
    set((s) => ({
      nodes: s.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)),
    })),
}));

// ── Tool Call Log Store ───────────────────────────────────────────────────────

interface ToolLogState {
  entries: ToolCallEntry[];
  addEntry: (entry: ToolCallEntry) => void;
  updateEntry: (id: string, patch: Partial<ToolCallEntry>) => void;
  clearLog: () => void;
}

export const useToolLogStore = create<ToolLogState>((set) => ({
  entries: [],
  addEntry: (entry) =>
    set((s) => ({ entries: [entry, ...s.entries].slice(0, 200) })),
  updateEntry: (id, patch) =>
    set((s) => ({
      entries: s.entries.map((e) => (e.id === id ? { ...e, ...patch } : e)),
    })),
  clearLog: () => set({ entries: [] }),
}));

// ── Agent Status Store ────────────────────────────────────────────────────────

interface AgentState {
  status: AgentStatus | null;
  setStatus: (status: AgentStatus | null) => void;
}

export const useAgentStore = create<AgentState>((set) => ({
  status: null,
  setStatus: (status) => set({ status }),
}));

// ── Vault Store ───────────────────────────────────────────────────────────────

interface VaultState {
  vaultStatus: VaultStatus;
  secretNames: string[];
  setVaultStatus: (s: VaultStatus) => void;
  setSecretNames: (names: string[]) => void;
}

export const useVaultStore = create<VaultState>((set) => ({
  vaultStatus: "locked",
  secretNames: [],
  setVaultStatus: (vaultStatus) => set({ vaultStatus }),
  setSecretNames: (secretNames) => set({ secretNames }),
}));

// ── Settings Store ────────────────────────────────────────────────────────────

const defaultSettings: AppSettings = {
  provider: "openai",
  model: "gpt-4o",
  autostart: false,
  minimizeToTray: true,
  spineHost: "localhost",
  spinePort: 1883,
  requirePairing: false,
  vaultEnabled: false,
};

interface SettingsState {
  settings: AppSettings;
  setSettings: (s: AppSettings) => void;
  patchSettings: (patch: Partial<AppSettings>) => void;
}

export const useSettingsStore = create<SettingsState>((set) => ({
  settings: defaultSettings,
  setSettings: (settings) => set({ settings }),
  patchSettings: (patch) =>
    set((s) => ({ settings: { ...s.settings, ...patch } })),
}));

// ── UI Store ──────────────────────────────────────────────────────────────────

export type ActivePanel = "chat" | "nodes" | "toollog" | "vault" | "settings";

interface UIState {
  activePanel: ActivePanel;
  sidebarOpen: boolean;
  setActivePanel: (p: ActivePanel) => void;
  setSidebarOpen: (v: boolean) => void;
}

export const useUIStore = create<UIState>((set) => ({
  activePanel: "chat",
  sidebarOpen: true,
  setActivePanel: (activePanel) => set({ activePanel }),
  setSidebarOpen: (sidebarOpen) => set({ sidebarOpen }),
}));
