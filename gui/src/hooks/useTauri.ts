/**
 * Tauri command bridge.
 *
 * All communication between the React frontend and the Tauri Rust backend
 * goes through this module. Each function maps to a `#[tauri::command]` in
 * src-tauri/src/commands.rs.
 *
 * During development without a running Tauri backend, all commands fall back
 * to mock data so the UI can be developed in the browser.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentStatus,
  AppSettings,
  ChatMessage,
  PeripheralNode,
  Session,
  ToolCallEntry,
  VaultStatus,
} from "../types";

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ── Helper ────────────────────────────────────────────────────────────────────

async function cmd<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri) {
    throw new Error(`Tauri not available (command: ${command})`);
  }
  return invoke<T>(command, args);
}

// ── Agent Commands ────────────────────────────────────────────────────────────

export async function sendMessage(
  sessionId: string,
  message: string
): Promise<string> {
  return cmd<string>("send_message", { sessionId, message });
}

export async function getAgentStatus(): Promise<AgentStatus> {
  return cmd<AgentStatus>("get_agent_status");
}

export async function startAgent(provider: string, model: string): Promise<void> {
  return cmd<void>("start_agent", { provider, model });
}

export async function stopAgent(): Promise<void> {
  return cmd<void>("stop_agent");
}

// ── Session Commands ──────────────────────────────────────────────────────────

export async function listSessions(): Promise<Session[]> {
  return cmd<Session[]>("list_sessions");
}

export async function createSession(title?: string): Promise<string> {
  return cmd<string>("create_session", { title });
}

export async function loadSessionHistory(sessionId: string): Promise<ChatMessage[]> {
  return cmd<ChatMessage[]>("load_session_history", { sessionId });
}

export async function clearSession(sessionId: string): Promise<void> {
  return cmd<void>("clear_session", { sessionId });
}

export async function deleteSession(sessionId: string): Promise<void> {
  return cmd<void>("delete_session", { sessionId });
}

// ── Peripheral Node Commands ──────────────────────────────────────────────────

export async function listNodes(): Promise<PeripheralNode[]> {
  return cmd<PeripheralNode[]>("list_nodes");
}

export async function addNode(
  board: string,
  transport: string,
  path?: string
): Promise<void> {
  return cmd<void>("add_node", { board, transport, path });
}

export async function removeNode(nodeId: string): Promise<void> {
  return cmd<void>("remove_node", { nodeId });
}

export async function scanUsbDevices(): Promise<PeripheralNode[]> {
  return cmd<PeripheralNode[]>("scan_usb_devices");
}

// ── Tool Call Log Commands ────────────────────────────────────────────────────

export async function getToolLog(limit?: number): Promise<ToolCallEntry[]> {
  return cmd<ToolCallEntry[]>("get_tool_log", { limit: limit ?? 100 });
}

export async function clearToolLog(): Promise<void> {
  return cmd<void>("clear_tool_log");
}

// ── Vault Commands ────────────────────────────────────────────────────────────

export async function getVaultStatus(): Promise<VaultStatus> {
  return cmd<VaultStatus>("get_vault_status");
}

export async function unlockVault(password: string): Promise<void> {
  return cmd<void>("unlock_vault", { password });
}

export async function lockVault(): Promise<void> {
  return cmd<void>("lock_vault");
}

export async function listVaultSecrets(): Promise<string[]> {
  return cmd<string[]>("list_vault_secrets");
}

export async function setVaultSecret(name: string, value: string): Promise<void> {
  return cmd<void>("set_vault_secret", { name, value });
}

export async function deleteVaultSecret(name: string): Promise<void> {
  return cmd<void>("delete_vault_secret", { name });
}

// ── Settings Commands ─────────────────────────────────────────────────────────

export async function getSettings(): Promise<AppSettings> {
  return cmd<AppSettings>("get_settings");
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return cmd<void>("save_settings", { settings });
}

// ── Event Listeners ───────────────────────────────────────────────────────────

/** Listen for streaming assistant tokens. */
export function onAssistantToken(cb: (token: string) => void) {
  if (!isTauri) return () => {};
  const unlisten = listen<string>("assistant-token", (e) => cb(e.payload));
  return () => { unlisten.then((f) => f()); };
}

/** Listen for tool call events (start + result). */
export function onToolCallEvent(cb: (entry: ToolCallEntry) => void) {
  if (!isTauri) return () => {};
  const unlisten = listen<ToolCallEntry>("tool-call-event", (e) => cb(e.payload));
  return () => { unlisten.then((f) => f()); };
}

/** Listen for peripheral node status changes. */
export function onNodeStatusChange(cb: (node: PeripheralNode) => void) {
  if (!isTauri) return () => {};
  const unlisten = listen<PeripheralNode>("node-status-change", (e) => cb(e.payload));
  return () => { unlisten.then((f) => f()); };
}

/** Listen for agent status changes. */
export function onAgentStatusChange(cb: (status: AgentStatus) => void) {
  if (!isTauri) return () => {};
  const unlisten = listen<AgentStatus>("agent-status-change", (e) => cb(e.payload));
  return () => { unlisten.then((f) => f()); };
}
