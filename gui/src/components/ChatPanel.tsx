import { useState, useRef, useEffect, useCallback } from "react";
import { Send, Loader2, Plus, Trash2, ChevronDown, Wrench } from "lucide-react";
import { useChatStore, useAgentStore } from "../stores/appStore";
import { sendMessage, listSessions, loadSessionHistory, createSession, clearSession, onAssistantToken, onToolCallEvent } from "../hooks/useTauri";
import type { ChatMessage } from "../types";

// ── Message Bubble ────────────────────────────────────────────────────────────

function MessageBubble({ msg }: { msg: ChatMessage }) {
  const time = new Date(msg.timestamp).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });

  if (msg.role === "tool_call") {
    return (
      <div className="flex gap-2 items-start animate-slide-up">
        <div className="w-5 h-5 rounded bg-amber-500/20 border border-amber-500/30 flex items-center justify-center flex-shrink-0 mt-0.5">
          <Wrench size={10} className="text-amber-400" />
        </div>
        <div className="flex-1 min-w-0">
          <div className="text-xs text-amber-400 font-mono mb-1">
            {msg.toolName} <span className="text-slate-500">called</span>
          </div>
          <div className="chat-bubble-tool">{msg.toolArgs || msg.content}</div>
        </div>
      </div>
    );
  }

  if (msg.role === "tool_result") {
    return (
      <div className="flex gap-2 items-start animate-slide-up">
        <div className="w-5 h-5 rounded bg-emerald-500/20 border border-emerald-500/30 flex items-center justify-center flex-shrink-0 mt-0.5">
          <Wrench size={10} className="text-emerald-400" />
        </div>
        <div className="flex-1 min-w-0">
          <div className="text-xs text-emerald-400 font-mono mb-1">
            {msg.toolName} <span className="text-slate-500">result</span>
          </div>
          <div className="chat-bubble-tool">{msg.content}</div>
        </div>
      </div>
    );
  }

  if (msg.role === "user") {
    return (
      <div className="flex justify-end animate-slide-up">
        <div>
          <div className="chat-bubble-user">{msg.content}</div>
          <div className="text-xs text-slate-600 text-right mt-1 pr-1">{time}</div>
        </div>
      </div>
    );
  }

  // assistant
  const isStreaming = msg.streaming === true;
  return (
    <div className="flex gap-2 items-start animate-slide-up">
      <div className="w-6 h-6 rounded-lg bg-obc-500/20 border border-obc-500/30 flex items-center justify-center flex-shrink-0 mt-0.5 text-xs font-bold text-obc-400">
        O
      </div>
      <div className="flex-1 min-w-0">
        <div className="chat-bubble-assistant whitespace-pre-wrap">
          {msg.content}
          {isStreaming && (
            <span className="inline-block w-0.5 h-3.5 bg-obc-400 ml-0.5 align-middle animate-pulse" />
          )}
        </div>
        <div className="text-xs text-slate-600 mt-1 pl-1">{time}</div>
      </div>
    </div>
  );
}

// ── Thinking Indicator ────────────────────────────────────────────────────────

function ThinkingIndicator() {
  return (
    <div className="flex gap-2 items-start animate-fade-in">
      <div className="w-6 h-6 rounded-lg bg-obc-500/20 border border-obc-500/30 flex items-center justify-center flex-shrink-0 mt-0.5">
        <Loader2 size={12} className="text-obc-400 animate-spin" />
      </div>
      <div className="chat-bubble-assistant flex gap-1 items-center py-3">
        <span className="w-1.5 h-1.5 bg-slate-400 rounded-full animate-bounce" style={{ animationDelay: "0ms" }} />
        <span className="w-1.5 h-1.5 bg-slate-400 rounded-full animate-bounce" style={{ animationDelay: "150ms" }} />
        <span className="w-1.5 h-1.5 bg-slate-400 rounded-full animate-bounce" style={{ animationDelay: "300ms" }} />
      </div>
    </div>
  );
}

// ── Session Selector ──────────────────────────────────────────────────────────

function SessionSelector() {
  const { sessions, activeSessionId, setActiveSession, setSessions, setMessages } = useChatStore();
  const [open, setOpen] = useState(false);

  const switchSession = async (id: string) => {
    setActiveSession(id);
    setOpen(false);
    try {
      const history = await loadSessionHistory(id);
      setMessages(history);
    } catch {
      setMessages([]);
    }
  };

  const newSession = async () => {
    try {
      const id = await createSession();
      const updated = await listSessions();
      setSessions(updated);
      await switchSession(id);
    } catch {
      // fallback: just create a local session
    }
    setOpen(false);
  };

  const activeSession = sessions.find((s) => s.id === activeSessionId);

  return (
    <div className="relative">
      <button
        onClick={() => setOpen(!open)}
        className="flex items-center gap-1.5 text-xs text-slate-400 hover:text-slate-200 transition-colors no-drag"
      >
        <span className="max-w-[120px] truncate">
          {activeSession?.title || activeSessionId}
        </span>
        <ChevronDown size={12} />
      </button>

      {open && (
        <div className="absolute top-full left-0 mt-1 w-52 bg-surface-overlay border border-surface-border rounded-xl shadow-xl z-50 overflow-hidden">
          <div className="p-1">
            {sessions.map((s) => (
              <button
                key={s.id}
                onClick={() => switchSession(s.id)}
                className={`
                  w-full text-left px-3 py-2 rounded-lg text-xs transition-colors
                  ${s.id === activeSessionId
                    ? "bg-obc-500/20 text-obc-300"
                    : "text-slate-300 hover:bg-surface-border"
                  }
                `}
              >
                <div className="font-medium truncate">{s.title}</div>
                <div className="text-slate-500">{s.messageCount} messages</div>
              </button>
            ))}
          </div>
          <div className="border-t border-surface-border p-1">
            <button
              onClick={newSession}
              className="w-full flex items-center gap-2 px-3 py-2 rounded-lg text-xs text-slate-400 hover:text-slate-200 hover:bg-surface-border transition-colors"
            >
              <Plus size={12} />
              New session
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Chat Panel ────────────────────────────────────────────────────────────────

export default function ChatPanel() {
  const [input, setInput] = useState("");
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // ID of the currently-streaming assistant message (null when not streaming).
  const streamingIdRef = useRef<string | null>(null);

  const {
    messages,
    activeSessionId,
    isThinking,
    setThinking,
    addMessage,
    setSessions,
    setMessages,
    clearMessages,
  } = useChatStore();
  const { status } = useAgentStore();

  // Auto-scroll to bottom
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, isThinking]);

  // Load sessions and history on mount
  useEffect(() => {
    listSessions()
      .then((sessions) => {
        setSessions(sessions);
        if (sessions.length > 0) {
          return loadSessionHistory(sessions[0].id);
        }
        return [];
      })
      .then(setMessages)
      .catch(() => {});
  }, [setSessions, setMessages]);

  // Subscribe to streaming token events
  useEffect(() => {
    const unsub = onAssistantToken((token) => {
      const id = streamingIdRef.current;
      if (!id) return;

      if (token === "") {
        // Empty sentinel — streaming complete: clear streaming flag.
        useChatStore.setState((s) => ({
          messages: s.messages.map((m) =>
            m.id === id ? { ...m, streaming: false } : m
          ),
        }));
        streamingIdRef.current = null;
        return;
      }

      useChatStore.setState((s) => ({
        messages: s.messages.map((m) =>
          m.id === id
            ? { ...m, content: m.content + token, streaming: true }
            : m
        ),
      }));
    });

    const unsubTool = onToolCallEvent((entry) => {
      // Tool-call events from ChatPanel surface as tool_call / tool_result bubbles.
      const role = entry.status === "pending" ? "tool_call" : "tool_result";
      addMessage({
        id: entry.id,
        role,
        content: entry.result ?? entry.args,
        toolName: entry.toolName,
        toolArgs: entry.args,
        timestamp: entry.timestamp,
      });
    });

    return () => {
      unsub();
      unsubTool();
    };
  }, [addMessage]);

  const handleSend = useCallback(async () => {
    const text = input.trim();
    if (!text || isThinking) return;

    setInput("");
    setThinking(true);

    const userMsg: ChatMessage = {
      id: crypto.randomUUID(),
      role: "user",
      content: text,
      timestamp: Date.now(),
    };
    addMessage(userMsg);

    // Add an empty streaming assistant message placeholder.
    const assistantId = crypto.randomUUID();
    streamingIdRef.current = assistantId;
    addMessage({
      id: assistantId,
      role: "assistant",
      content: "",
      timestamp: Date.now(),
      streaming: true,
    });

    try {
      // `sendMessage` drives the backend process; tokens arrive via the
      // `assistant-token` Tauri event and are stitched into the message above.
      await sendMessage(activeSessionId, text);
    } catch (err) {
      // Replace streaming placeholder with an error message.
      useChatStore.setState((s) => ({
        messages: s.messages.map((m) =>
          m.id === assistantId
            ? {
                ...m,
                content: `Error: ${err instanceof Error ? err.message : String(err)}`,
                streaming: false,
              }
            : m
        ),
      }));
      streamingIdRef.current = null;
    } finally {
      setThinking(false);
      inputRef.current?.focus();
    }
  }, [input, isThinking, activeSessionId, addMessage, setThinking]);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleClear = async () => {
    try {
      await clearSession(activeSessionId);
    } catch {
      // ignore
    }
    clearMessages();
  };

  const agentReady = status?.running ?? false;

  return (
    <div className="flex flex-col h-full">
      {/* Chat header */}
      <div className="flex items-center justify-between px-4 py-2.5 border-b border-surface-border flex-shrink-0">
        <SessionSelector />
        <div className="flex items-center gap-2">
          {status && (
            <span className="text-xs text-slate-500">
              {status.provider}/{status.model}
            </span>
          )}
          <button
            onClick={handleClear}
            className="btn-ghost p-1.5"
            title="Clear conversation"
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>

      {/* Messages */}
      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-4">
        {messages.length === 0 && (
          <div className="flex flex-col items-center justify-center h-full text-center gap-3 opacity-50">
            <div className="w-12 h-12 rounded-2xl bg-obc-500/10 border border-obc-500/20 flex items-center justify-center">
              <span className="text-2xl font-bold text-obc-400">O</span>
            </div>
            <div>
              <div className="text-sm font-medium text-slate-300">Oh-Ben-Claw</div>
              <div className="text-xs text-slate-500 mt-1">
                {agentReady
                  ? "Ready. Ask me anything."
                  : "Start the agent in Settings to begin."}
              </div>
            </div>
          </div>
        )}

        {messages.map((msg) => (
          <MessageBubble key={msg.id} msg={msg} />
        ))}

        {isThinking && !messages.some((m) => m.role === "assistant" && m.streaming) && (
          <ThinkingIndicator />
        )}
        <div ref={messagesEndRef} />
      </div>

      {/* Input area */}
      <div className="px-4 py-3 border-t border-surface-border flex-shrink-0">
        <div className="flex gap-2 items-end">
          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={agentReady ? "Message Oh-Ben-Claw… (Enter to send)" : "Start the agent to chat…"}
            disabled={!agentReady || isThinking}
            rows={1}
            className="
              obc-input flex-1 resize-none selectable
              min-h-[42px] max-h-[120px] overflow-y-auto
              disabled:opacity-40 disabled:cursor-not-allowed
            "
            style={{ height: "auto" }}
            onInput={(e) => {
              const t = e.currentTarget;
              t.style.height = "auto";
              t.style.height = `${Math.min(t.scrollHeight, 120)}px`;
            }}
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || !agentReady || isThinking}
            className="btn-primary p-2.5 disabled:opacity-40 disabled:cursor-not-allowed flex-shrink-0"
          >
            {isThinking ? (
              <Loader2 size={16} className="animate-spin" />
            ) : (
              <Send size={16} />
            )}
          </button>
        </div>
        <div className="text-xs text-slate-600 mt-1.5 pl-1">
          Shift+Enter for new line
        </div>
      </div>
    </div>
  );
}
