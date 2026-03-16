import { useState } from "react";
import { KeyRound, Lock, Plus, Trash2, Eye, EyeOff, ShieldCheck } from "lucide-react";
import { useVaultStore } from "../stores/appStore";
import {
  unlockVault,
  lockVault,
  listVaultSecrets,
  setVaultSecret,
  deleteVaultSecret,
} from "../hooks/useTauri";

// ── Unlock Form ───────────────────────────────────────────────────────────────

function UnlockForm() {
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const { setVaultStatus, setSecretNames } = useVaultStore();

  const handleUnlock = async () => {
    if (!password) return;
    setLoading(true);
    setError("");
    try {
      await unlockVault(password);
      setVaultStatus("unlocked");
      const names = await listVaultSecrets();
      setSecretNames(names);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Incorrect password");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex flex-col items-center justify-center h-full gap-6">
      <div className="text-center">
        <div className="w-14 h-14 rounded-2xl bg-obc-500/10 border border-obc-500/20 flex items-center justify-center mx-auto mb-4">
          <Lock size={24} className="text-obc-400" />
        </div>
        <div className="text-base font-semibold text-slate-100">Vault Locked</div>
        <div className="text-xs text-slate-500 mt-1 max-w-xs">
          Enter your master password to access the encrypted secrets vault.
        </div>
      </div>

      <div className="w-72 space-y-3">
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleUnlock()}
          placeholder="Master password"
          className="obc-input w-full selectable"
          autoFocus
        />
        {error && (
          <div className="text-xs text-red-400 text-center">{error}</div>
        )}
        <button
          onClick={handleUnlock}
          disabled={!password || loading}
          className="btn-primary w-full disabled:opacity-40"
        >
          {loading ? "Unlocking…" : "Unlock Vault"}
        </button>
      </div>

      <div className="text-xs text-slate-600 text-center max-w-xs">
        Secrets are encrypted with AES-256-GCM. The master password is never stored.
      </div>
    </div>
  );
}

// ── Add Secret Modal ──────────────────────────────────────────────────────────

function AddSecretModal({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [showValue, setShowValue] = useState(false);
  const [loading, setLoading] = useState(false);

  const handleAdd = async () => {
    if (!name.trim() || !value.trim()) return;
    setLoading(true);
    try {
      await setVaultSecret(name.trim(), value.trim());
      onAdded();
      onClose();
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 animate-fade-in">
      <div className="glass p-6 w-96 space-y-4 animate-slide-up">
        <h2 className="text-base font-semibold text-slate-100">Add Secret</h2>

        <div className="space-y-3">
          <div>
            <label className="text-xs text-slate-400 mb-1 block">Name</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. OPENAI_API_KEY"
              className="obc-input w-full selectable font-mono"
              autoFocus
            />
          </div>
          <div>
            <label className="text-xs text-slate-400 mb-1 block">Value</label>
            <div className="relative">
              <input
                type={showValue ? "text" : "password"}
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder="sk-…"
                className="obc-input w-full selectable font-mono pr-10"
              />
              <button
                onClick={() => setShowValue(!showValue)}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-slate-500 hover:text-slate-300"
              >
                {showValue ? <EyeOff size={14} /> : <Eye size={14} />}
              </button>
            </div>
          </div>
        </div>

        <div className="flex gap-2 justify-end pt-2">
          <button onClick={onClose} className="btn-ghost">Cancel</button>
          <button
            onClick={handleAdd}
            disabled={!name.trim() || !value.trim() || loading}
            className="btn-primary disabled:opacity-40"
          >
            {loading ? "Saving…" : "Save Secret"}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Vault Panel ───────────────────────────────────────────────────────────────

export default function VaultPanel() {
  const { vaultStatus, secretNames, setVaultStatus, setSecretNames } = useVaultStore();
  const [showAdd, setShowAdd] = useState(false);

  const handleLock = async () => {
    try {
      await lockVault();
    } catch {
      // ignore
    }
    setVaultStatus("locked");
    setSecretNames([]);
  };

  const handleDelete = async (name: string) => {
    try {
      await deleteVaultSecret(name);
      setSecretNames(secretNames.filter((n) => n !== name));
    } catch {
      // ignore
    }
  };

  const refreshSecrets = async () => {
    try {
      const names = await listVaultSecrets();
      setSecretNames(names);
    } catch {
      // ignore
    }
  };

  if (vaultStatus === "locked") {
    return <UnlockForm />;
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-surface-border flex-shrink-0">
        <div className="flex items-center gap-2 text-sm text-emerald-400">
          <ShieldCheck size={15} />
          <span className="font-medium">Vault Unlocked</span>
          <span className="text-slate-500 text-xs">· {secretNames.length} secret{secretNames.length !== 1 ? "s" : ""}</span>
        </div>
        <div className="flex items-center gap-2">
          <button onClick={() => setShowAdd(true)} className="btn-primary flex items-center gap-1.5 text-sm">
            <Plus size={14} />
            Add Secret
          </button>
          <button onClick={handleLock} className="btn-ghost flex items-center gap-1.5 text-sm">
            <Lock size={14} />
            Lock
          </button>
        </div>
      </div>

      {/* Secrets list */}
      <div className="flex-1 overflow-y-auto p-4 space-y-2">
        {secretNames.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-center gap-3 opacity-50">
            <KeyRound size={32} className="text-slate-500" />
            <div>
              <div className="text-sm font-medium text-slate-300">No secrets stored</div>
              <div className="text-xs text-slate-500 mt-1">
                Add API keys and credentials. They take precedence over environment variables.
              </div>
            </div>
          </div>
        ) : (
          secretNames.map((name) => (
            <div
              key={name}
              className="glass flex items-center gap-3 px-4 py-3"
            >
              <div className="w-7 h-7 rounded-lg bg-obc-500/10 border border-obc-500/20 flex items-center justify-center flex-shrink-0">
                <KeyRound size={13} className="text-obc-400" />
              </div>
              <div className="flex-1 min-w-0">
                <div className="font-mono text-sm text-slate-200">{name}</div>
                <div className="text-xs text-slate-500">••••••••••••••••</div>
              </div>
              <div className="flex items-center gap-1">
                <button
                  onClick={() => handleDelete(name)}
                  className="btn-ghost p-1.5 text-red-400 hover:text-red-300"
                  title="Delete secret"
                >
                  <Trash2 size={13} />
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      <div className="px-4 py-3 border-t border-surface-border flex-shrink-0">
        <div className="flex items-center gap-2 text-xs text-slate-600">
          <Lock size={11} />
          <span>AES-256-GCM · Argon2id key derivation · SQLite WAL backend</span>
        </div>
      </div>

      {showAdd && (
        <AddSecretModal
          onClose={() => setShowAdd(false)}
          onAdded={refreshSecrets}
        />
      )}
    </div>
  );
}
