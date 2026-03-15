import { useEffect, useState } from "react";
import { RefreshCw, Plus, Trash2, Usb, Wifi, Cpu, ChevronDown, ChevronRight, Wrench } from "lucide-react";
import { useNodesStore } from "../stores/appStore";
import { listNodes, scanUsbDevices, removeNode } from "../hooks/useTauri";
import type { PeripheralNode, NodeStatus } from "../types";

// ── Status Badge ──────────────────────────────────────────────────────────────

function StatusBadge({ status }: { status: NodeStatus }) {
  const map: Record<NodeStatus, { label: string; cls: string }> = {
    online:      { label: "Online",      cls: "bg-emerald-500/20 text-emerald-400 border-emerald-500/30" },
    offline:     { label: "Offline",     cls: "bg-slate-500/20 text-slate-400 border-slate-500/30" },
    error:       { label: "Error",       cls: "bg-red-500/20 text-red-400 border-red-500/30" },
    paired:      { label: "Paired",      cls: "bg-obc-500/20 text-obc-400 border-obc-500/30" },
    quarantined: { label: "Quarantined", cls: "bg-amber-500/20 text-amber-400 border-amber-500/30" },
  };
  const { label, cls } = map[status];
  return (
    <span className={`text-xs px-2 py-0.5 rounded-full border font-medium ${cls}`}>
      {label}
    </span>
  );
}

// ── Transport Icon ────────────────────────────────────────────────────────────

function TransportIcon({ transport }: { transport: PeripheralNode["transport"] }) {
  if (transport === "serial") return <Usb size={14} className="text-slate-400" />;
  if (transport === "mqtt")   return <Wifi size={14} className="text-slate-400" />;
  return <Cpu size={14} className="text-slate-400" />;
}

// ── Node Card ─────────────────────────────────────────────────────────────────

function NodeCard({ node }: { node: PeripheralNode }) {
  const [expanded, setExpanded] = useState(false);
  const { setNodes } = useNodesStore();

  const handleRemove = async () => {
    try {
      await removeNode(node.id);
      const updated = await listNodes();
      setNodes(updated);
    } catch {
      // ignore
    }
  };

  const lastSeen = node.lastSeen
    ? new Date(node.lastSeen).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : null;

  return (
    <div className="glass p-4 space-y-3">
      {/* Header row */}
      <div className="flex items-start gap-3">
        <div className="w-9 h-9 rounded-lg bg-surface-overlay border border-surface-border flex items-center justify-center flex-shrink-0">
          <TransportIcon transport={node.transport} />
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-sm font-semibold text-slate-100 truncate">{node.board}</span>
            <StatusBadge status={node.status} />
          </div>
          <div className="text-xs text-slate-500 mt-0.5 flex items-center gap-2">
            <span className="font-mono">{node.id}</span>
            {node.address && <span>· {node.address}</span>}
            {lastSeen && <span>· {lastSeen}</span>}
          </div>
        </div>
        <button onClick={handleRemove} className="btn-ghost p-1.5 text-red-400 hover:text-red-300 flex-shrink-0">
          <Trash2 size={14} />
        </button>
      </div>

      {/* Tools section */}
      {node.tools.length > 0 && (
        <div>
          <button
            onClick={() => setExpanded(!expanded)}
            className="flex items-center gap-1.5 text-xs text-slate-400 hover:text-slate-200 transition-colors"
          >
            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            <Wrench size={11} />
            <span>{node.tools.length} tool{node.tools.length !== 1 ? "s" : ""}</span>
          </button>

          {expanded && (
            <div className="mt-2 space-y-1.5 pl-4 border-l border-surface-border">
              {node.tools.map((tool) => (
                <div key={tool.name} className="text-xs">
                  <span className="font-mono text-obc-300">{tool.name}</span>
                  <span className="text-slate-500 ml-2">{tool.description}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ── Add Node Modal ────────────────────────────────────────────────────────────

function AddNodeModal({ onClose }: { onClose: () => void }) {
  const [board, setBoard] = useState("waveshare-esp32-s3-touch-lcd-2.1");
  const [transport, setTransport] = useState<"serial" | "mqtt" | "native">("serial");
  const [path, setPath] = useState("/dev/ttyUSB0");
  const { setNodes } = useNodesStore();

  const handleAdd = async () => {
    try {
      const { addNode } = await import("../hooks/useTauri");
      await addNode(board, transport, path || undefined);
      const updated = await listNodes();
      setNodes(updated);
      onClose();
    } catch {
      // ignore for now
      onClose();
    }
  };

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 animate-fade-in">
      <div className="glass p-6 w-96 space-y-4 animate-slide-up">
        <h2 className="text-base font-semibold text-slate-100">Add Peripheral Node</h2>

        <div className="space-y-3">
          <div>
            <label className="text-xs text-slate-400 mb-1 block">Board</label>
            <select
              value={board}
              onChange={(e) => setBoard(e.target.value)}
              className="obc-input w-full"
            >
              <option value="waveshare-esp32-s3-touch-lcd-2.1">Waveshare ESP32-S3 Touch LCD 2.1</option>
              <option value="nanopi-neo3">NanoPi Neo3</option>
              <option value="rpi-gpio">Raspberry Pi (GPIO)</option>
              <option value="arduino-uno">Arduino Uno</option>
              <option value="stm32-nucleo">STM32 Nucleo</option>
              <option value="custom">Custom</option>
            </select>
          </div>

          <div>
            <label className="text-xs text-slate-400 mb-1 block">Transport</label>
            <select
              value={transport}
              onChange={(e) => setTransport(e.target.value as typeof transport)}
              className="obc-input w-full"
            >
              <option value="serial">USB Serial</option>
              <option value="mqtt">MQTT (Wireless)</option>
              <option value="native">Native (same host)</option>
            </select>
          </div>

          {transport === "serial" && (
            <div>
              <label className="text-xs text-slate-400 mb-1 block">Serial Port</label>
              <input
                type="text"
                value={path}
                onChange={(e) => setPath(e.target.value)}
                placeholder="/dev/ttyUSB0"
                className="obc-input w-full selectable"
              />
            </div>
          )}
        </div>

        <div className="flex gap-2 justify-end pt-2">
          <button onClick={onClose} className="btn-ghost">Cancel</button>
          <button onClick={handleAdd} className="btn-primary">Add Node</button>
        </div>
      </div>
    </div>
  );
}

// ── Nodes Panel ───────────────────────────────────────────────────────────────

export default function NodesPanel() {
  const { nodes, setNodes } = useNodesStore();
  const [loading, setLoading] = useState(false);
  const [showAdd, setShowAdd] = useState(false);

  const refresh = async () => {
    setLoading(true);
    try {
      const updated = await listNodes();
      setNodes(updated);
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  const scan = async () => {
    setLoading(true);
    try {
      const found = await scanUsbDevices();
      setNodes(found);
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const online = nodes.filter((n) => n.status === "online" || n.status === "paired").length;
  const total = nodes.length;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-surface-border flex-shrink-0">
        <div className="text-sm text-slate-400">
          <span className="text-slate-100 font-medium">{online}</span>/{total} online
        </div>
        <div className="flex items-center gap-2">
          <button onClick={scan} className="btn-ghost text-xs flex items-center gap-1.5">
            <Usb size={13} />
            Scan USB
          </button>
          <button
            onClick={refresh}
            className="btn-ghost p-1.5"
            disabled={loading}
          >
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
          </button>
          <button onClick={() => setShowAdd(true)} className="btn-primary flex items-center gap-1.5 text-sm">
            <Plus size={14} />
            Add
          </button>
        </div>
      </div>

      {/* Node list */}
      <div className="flex-1 overflow-y-auto p-4 space-y-3">
        {nodes.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-center gap-3 opacity-50">
            <Cpu size={32} className="text-slate-500" />
            <div>
              <div className="text-sm font-medium text-slate-300">No devices connected</div>
              <div className="text-xs text-slate-500 mt-1">
                Click "Scan USB" to detect connected boards, or "Add" to configure manually.
              </div>
            </div>
          </div>
        ) : (
          nodes.map((node) => <NodeCard key={node.id} node={node} />)
        )}
      </div>

      {showAdd && <AddNodeModal onClose={() => setShowAdd(false)} />}
    </div>
  );
}
