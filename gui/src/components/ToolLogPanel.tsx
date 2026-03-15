import { useEffect, useState } from "react";
import { RefreshCw, Trash2, CheckCircle, XCircle, Clock, ShieldOff, ChevronDown, ChevronRight } from "lucide-react";
import { useToolLogStore } from "../stores/appStore";
import { getToolLog, clearToolLog } from "../hooks/useTauri";
import type { ToolCallEntry, ToolCallStatus } from "../types";

// ── Status Icon ───────────────────────────────────────────────────────────────

function StatusIcon({ status }: { status: ToolCallStatus }) {
  switch (status) {
    case "success": return <CheckCircle size={14} className="text-emerald-400 flex-shrink-0" />;
    case "error":   return <XCircle size={14} className="text-red-400 flex-shrink-0" />;
    case "denied":  return <ShieldOff size={14} className="text-amber-400 flex-shrink-0" />;
    case "pending": return <Clock size={14} className="text-slate-400 flex-shrink-0 animate-pulse" />;
  }
}

// ── Tool Call Row ─────────────────────────────────────────────────────────────

function ToolCallRow({ entry }: { entry: ToolCallEntry }) {
  const [expanded, setExpanded] = useState(false);
  const time = new Date(entry.timestamp).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });

  const statusCls: Record<ToolCallStatus, string> = {
    success: "border-l-emerald-500/50",
    error:   "border-l-red-500/50",
    denied:  "border-l-amber-500/50",
    pending: "border-l-slate-500/50",
  };

  return (
    <div className={`bg-surface-raised border border-surface-border border-l-2 ${statusCls[entry.status]} rounded-xl overflow-hidden`}>
      <button
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-3 px-3 py-2.5 text-left hover:bg-surface-overlay transition-colors"
      >
        <StatusIcon status={entry.status} />
        <span className="font-mono text-xs text-obc-300 font-medium flex-shrink-0">{entry.toolName}</span>
        <span className="text-xs text-slate-500 flex-1 truncate">{entry.args}</span>
        <div className="flex items-center gap-2 flex-shrink-0">
          {entry.durationMs !== undefined && (
            <span className="text-xs text-slate-600">{entry.durationMs}ms</span>
          )}
          <span className="text-xs text-slate-600">{time}</span>
          {expanded ? <ChevronDown size={12} className="text-slate-500" /> : <ChevronRight size={12} className="text-slate-500" />}
        </div>
      </button>

      {expanded && (
        <div className="px-3 pb-3 space-y-2 border-t border-surface-border">
          <div>
            <div className="text-xs text-slate-500 mb-1 pt-2">Arguments</div>
            <pre className="text-xs font-mono text-slate-300 bg-surface-overlay rounded-lg p-2 overflow-x-auto selectable whitespace-pre-wrap break-all">
              {(() => {
                try { return JSON.stringify(JSON.parse(entry.args), null, 2); }
                catch { return entry.args; }
              })()}
            </pre>
          </div>
          {entry.result && (
            <div>
              <div className="text-xs text-slate-500 mb-1">Result</div>
              <pre className="text-xs font-mono text-slate-300 bg-surface-overlay rounded-lg p-2 overflow-x-auto selectable whitespace-pre-wrap break-all max-h-40">
                {entry.result}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ── Tool Log Panel ────────────────────────────────────────────────────────────

export default function ToolLogPanel() {
  const { entries, addEntry, clearLog } = useToolLogStore();
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState<ToolCallStatus | "all">("all");

  const refresh = async () => {
    setLoading(true);
    try {
      const log = await getToolLog(200);
      // Populate store
      clearLog();
      log.forEach((e) => addEntry(e));
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  const handleClear = async () => {
    try {
      await clearToolLog();
    } catch {
      // ignore
    }
    clearLog();
  };

  useEffect(() => {
    refresh();
  }, []);

  const filtered = filter === "all"
    ? entries
    : entries.filter((e) => e.status === filter);

  const counts = {
    all:     entries.length,
    success: entries.filter((e) => e.status === "success").length,
    error:   entries.filter((e) => e.status === "error").length,
    denied:  entries.filter((e) => e.status === "denied").length,
    pending: entries.filter((e) => e.status === "pending").length,
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-surface-border flex-shrink-0">
        <div className="flex items-center gap-1">
          {(["all", "success", "error", "denied"] as const).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`
                text-xs px-2.5 py-1 rounded-lg transition-colors capitalize
                ${filter === f
                  ? "bg-obc-500/20 text-obc-300 border border-obc-500/30"
                  : "text-slate-400 hover:text-slate-200 hover:bg-surface-overlay"
                }
              `}
            >
              {f} {counts[f] > 0 && <span className="opacity-60">({counts[f]})</span>}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <button onClick={refresh} className="btn-ghost p-1.5" disabled={loading}>
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
          </button>
          <button onClick={handleClear} className="btn-ghost p-1.5 text-red-400 hover:text-red-300">
            <Trash2 size={14} />
          </button>
        </div>
      </div>

      {/* Log entries */}
      <div className="flex-1 overflow-y-auto p-4 space-y-2">
        {filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-center gap-3 opacity-50">
            <Clock size={32} className="text-slate-500" />
            <div>
              <div className="text-sm font-medium text-slate-300">No tool calls yet</div>
              <div className="text-xs text-slate-500 mt-1">
                Tool calls will appear here as the agent works.
              </div>
            </div>
          </div>
        ) : (
          filtered.map((entry) => <ToolCallRow key={entry.id} entry={entry} />)
        )}
      </div>
    </div>
  );
}
