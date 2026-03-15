import { useEffect } from "react";
import {
  MessageSquare,
  Cpu,
  Terminal,
  KeyRound,
  Settings,
  Menu,
  X,
  Radio,
} from "lucide-react";
import { useUIStore, useAgentStore, useNodesStore, useToolLogStore } from "./stores/appStore";
import { onAgentStatusChange, onNodeStatusChange, onToolCallEvent } from "./hooks/useTauri";
import ChatPanel from "./components/ChatPanel";
import NodesPanel from "./components/NodesPanel";
import ToolLogPanel from "./components/ToolLogPanel";
import VaultPanel from "./components/VaultPanel";
import SettingsPanel from "./components/SettingsPanel";
import type { ActivePanel } from "./stores/appStore";

// ── Sidebar Nav Item ──────────────────────────────────────────────────────────

interface NavItemProps {
  icon: React.ReactNode;
  label: string;
  panel: ActivePanel;
  badge?: number;
}

function NavItem({ icon, label, panel, badge }: NavItemProps) {
  const { activePanel, setActivePanel } = useUIStore();
  const isActive = activePanel === panel;

  return (
    <button
      onClick={() => setActivePanel(panel)}
      className={`
        w-full flex items-center gap-3 px-3 py-2.5 rounded-xl text-sm font-medium
        transition-all duration-150 no-drag relative
        ${isActive
          ? "bg-obc-500/20 text-obc-300 border border-obc-500/30"
          : "text-slate-400 hover:text-slate-200 hover:bg-surface-overlay"
        }
      `}
    >
      <span className={`flex-shrink-0 ${isActive ? "text-obc-400" : ""}`}>
        {icon}
      </span>
      <span className="flex-1 text-left">{label}</span>
      {badge !== undefined && badge > 0 && (
        <span className="bg-obc-500 text-white text-xs rounded-full w-5 h-5 flex items-center justify-center flex-shrink-0">
          {badge > 99 ? "99+" : badge}
        </span>
      )}
    </button>
  );
}

// ── Main App ──────────────────────────────────────────────────────────────────

export default function App() {
  const { activePanel, sidebarOpen, setSidebarOpen } = useUIStore();
  const { setStatus } = useAgentStore();
  const { updateNode } = useNodesStore();
  const { addEntry, updateEntry } = useToolLogStore();

  // Subscribe to Tauri events
  useEffect(() => {
    const unsubs = [
      onAgentStatusChange(setStatus),
      onNodeStatusChange((node) => updateNode(node.id, node)),
      onToolCallEvent((entry) => {
        if (entry.status === "pending") {
          addEntry(entry);
        } else {
          updateEntry(entry.id, entry);
        }
      }),
    ];
    return () => unsubs.forEach((u) => u());
  }, [setStatus, updateNode, addEntry, updateEntry]);

  const panels: Record<ActivePanel, React.ReactNode> = {
    chat: <ChatPanel />,
    nodes: <NodesPanel />,
    toollog: <ToolLogPanel />,
    vault: <VaultPanel />,
    settings: <SettingsPanel />,
  };

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-surface">
      {/* Sidebar */}
      <aside
        className={`
          flex flex-col bg-surface-raised border-r border-surface-border
          transition-all duration-200 flex-shrink-0
          ${sidebarOpen ? "w-52" : "w-0 overflow-hidden"}
        `}
      >
        {/* Logo / Title bar */}
        <div className="drag-region flex items-center gap-2.5 px-4 py-4 border-b border-surface-border">
          <div className="w-7 h-7 rounded-lg bg-obc-500/20 border border-obc-500/40 flex items-center justify-center flex-shrink-0">
            <Radio size={14} className="text-obc-400" />
          </div>
          <div className="flex-1 min-w-0">
            <div className="text-sm font-semibold text-slate-100 truncate">Oh-Ben-Claw</div>
            <div className="text-xs text-slate-500 truncate">v0.1.0</div>
          </div>
        </div>

        {/* Navigation */}
        <nav className="flex-1 p-3 space-y-1 overflow-y-auto">
          <NavItem icon={<MessageSquare size={16} />} label="Chat" panel="chat" />
          <NavItem icon={<Cpu size={16} />} label="Devices" panel="nodes" />
          <NavItem icon={<Terminal size={16} />} label="Tool Log" panel="toollog" />
          <NavItem icon={<KeyRound size={16} />} label="Vault" panel="vault" />
          <div className="pt-2 mt-2 border-t border-surface-border">
            <NavItem icon={<Settings size={16} />} label="Settings" panel="settings" />
          </div>
        </nav>
      </aside>

      {/* Main content area */}
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Top bar */}
        <header className="drag-region flex items-center gap-2 px-3 py-2 border-b border-surface-border bg-surface-raised/50 flex-shrink-0">
          <button
            onClick={() => setSidebarOpen(!sidebarOpen)}
            className="btn-ghost p-1.5 no-drag"
          >
            {sidebarOpen ? <X size={16} /> : <Menu size={16} />}
          </button>
          <span className="text-sm text-slate-400 font-medium capitalize">
            {activePanel === "toollog" ? "Tool Log" : activePanel}
          </span>
        </header>

        {/* Active panel */}
        <div className="flex-1 overflow-hidden animate-fade-in">
          {panels[activePanel]}
        </div>
      </main>
    </div>
  );
}
