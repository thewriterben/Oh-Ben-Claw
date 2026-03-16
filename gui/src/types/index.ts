// ── Chat Types ────────────────────────────────────────────────────────────────

export type MessageRole = "user" | "assistant" | "tool_call" | "tool_result" | "system";

export interface ChatMessage {
  id: string;
  role: MessageRole;
  content: string;
  toolName?: string;
  toolArgs?: string;
  timestamp: number;
  /** Set to `true` while the assistant is still streaming tokens into this message. */
  streaming?: boolean;
}

export interface Session {
  id: string;
  title: string;
  messageCount: number;
  createdAt: number;
}

// ── Peripheral Node Types ─────────────────────────────────────────────────────

export type NodeStatus = "online" | "offline" | "error" | "paired" | "quarantined";
export type NodeTransport = "serial" | "mqtt" | "native";

export interface PeripheralTool {
  name: string;
  description: string;
}

export interface PeripheralNode {
  id: string;
  board: string;
  transport: NodeTransport;
  status: NodeStatus;
  tools: PeripheralTool[];
  lastSeen?: number;
  address?: string;
}

// ── Tool Call Log Types ───────────────────────────────────────────────────────

export type ToolCallStatus = "pending" | "success" | "error" | "denied";

export interface ToolCallEntry {
  id: string;
  toolName: string;
  args: string;
  result?: string;
  status: ToolCallStatus;
  durationMs?: number;
  timestamp: number;
  sessionId: string;
}

// ── Vault Types ───────────────────────────────────────────────────────────────

export interface VaultEntry {
  name: string;
  // Value is never returned from the backend — only names are listed
}

export type VaultStatus = "locked" | "unlocked" | "disabled";

// ── Agent Status Types ────────────────────────────────────────────────────────

export interface AgentStatus {
  running: boolean;
  provider: string;
  model: string;
  sessionId: string;
  toolCount: number;
  nodeCount: number;
  uptime?: number;
}

// ── Settings Types ────────────────────────────────────────────────────────────

export interface AppSettings {
  provider: string;
  model: string;
  apiKey?: string;
  ollamaUrl?: string;
  autostart: boolean;
  minimizeToTray: boolean;
  spineHost: string;
  spinePort: number;
  requirePairing: boolean;
  vaultEnabled: boolean;
}
