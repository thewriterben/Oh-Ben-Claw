import { useState } from 'react';
import type { CodeBlock as CodeBlockType } from '../lib/guide-engine';

interface Props {
  block: CodeBlockType;
}

export function CodeBlock({ block }: Props) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    await navigator.clipboard.writeText(block.code);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="my-3 rounded-lg border border-[#30363d] bg-[#010409] overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-2 border-b border-[#30363d] bg-[#161b22]">
        <div className="flex items-center gap-2">
          <span className="text-xs font-mono text-[#8b949e]">{block.language}</span>
          {block.label && (
            <span className="text-xs text-[#8b949e] truncate max-w-xs">{block.label}</span>
          )}
        </div>
        {block.copyable && (
          <button
            onClick={handleCopy}
            className="text-xs px-2 py-1 rounded text-[#8b949e] hover:text-[#e6edf3] hover:bg-[#21262d] transition-colors flex items-center gap-1"
          >
            {copied ? (
              <>
                <svg className="w-3 h-3 text-[#3fb950]" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                </svg>
                <span className="text-[#3fb950]">Copied!</span>
              </>
            ) : (
              <>
                <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                </svg>
                Copy
              </>
            )}
          </button>
        )}
      </div>
      {/* Code */}
      <div className="p-4 overflow-x-auto">
        <pre className="text-sm leading-relaxed text-[#e6edf3] whitespace-pre-wrap break-all font-mono">
          <code>{block.code}</code>
        </pre>
      </div>
    </div>
  );
}
