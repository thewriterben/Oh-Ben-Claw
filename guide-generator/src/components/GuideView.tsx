import { useRef } from 'react';
import { guideToMarkdown } from '../lib/guide-engine';
import type { GeneratedGuide } from '../lib/guide-engine';
import { CodeBlock } from './CodeBlock';

interface Props {
  guide: GeneratedGuide;
  onReset: () => void;
}

const DIFFICULTY_COLORS = {
  beginner: 'text-[#3fb950] bg-[#3fb950]/10 border-[#3fb950]/30',
  intermediate: 'text-[#d29922] bg-[#d29922]/10 border-[#d29922]/30',
  advanced: 'text-[#f85149] bg-[#f85149]/10 border-[#f85149]/30',
};

export function GuideView({ guide, onReset }: Props) {
  const guideRef = useRef<HTMLDivElement>(null);

  const handleDownloadMarkdown = () => {
    const md = guideToMarkdown(guide);
    const blob = new Blob([md], { type: 'text/markdown' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `obc-deployment-guide-${Date.now()}.md`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleDownloadPDF = async () => {
    const { jsPDF } = await import('jspdf');
    const { default: html2canvas } = await import('html2canvas');

    if (!guideRef.current) return;

    const canvas = await html2canvas(guideRef.current, {
      scale: 1.5,
      backgroundColor: '#0d1117',
      useCORS: true,
      logging: false,
    });

    const imgData = canvas.toDataURL('image/png');
    const pdf = new jsPDF({
      orientation: 'portrait',
      unit: 'px',
      format: [canvas.width / 1.5, canvas.height / 1.5],
    });

    pdf.addImage(imgData, 'PNG', 0, 0, canvas.width / 1.5, canvas.height / 1.5);
    pdf.save(`obc-deployment-guide-${Date.now()}.pdf`);
  };

  return (
    <div className="max-w-4xl mx-auto px-4 py-8">
      {/* Header */}
      <div className="mb-8 fade-in">
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div>
            <h1 className="text-2xl font-bold text-[#e6edf3] mb-2">{guide.title}</h1>
            <p className="text-[#8b949e] text-sm leading-relaxed max-w-2xl">{guide.summary}</p>
          </div>
          <button
            onClick={onReset}
            className="text-sm px-3 py-1.5 rounded-lg border border-[#30363d] text-[#8b949e] hover:text-[#e6edf3] hover:border-[#58a6ff] transition-colors shrink-0"
          >
            ← Start Over
          </button>
        </div>

        {/* Meta badges */}
        <div className="flex items-center gap-3 mt-4 flex-wrap">
          <span className="text-xs px-2 py-1 rounded-full border bg-[#161b22] border-[#30363d] text-[#8b949e]">
            ⏱ {guide.estimatedTime}
          </span>
          <span className={`text-xs px-2 py-1 rounded-full border capitalize ${DIFFICULTY_COLORS[guide.difficulty]}`}>
            {guide.difficulty}
          </span>
          <span className="text-xs px-2 py-1 rounded-full border bg-[#161b22] border-[#30363d] text-[#8b949e]">
            {guide.steps.length} steps
          </span>
        </div>

        {/* Export buttons */}
        <div className="flex items-center gap-3 mt-4">
          <button
            onClick={handleDownloadMarkdown}
            className="flex items-center gap-2 text-sm px-4 py-2 rounded-lg bg-[#21262d] border border-[#30363d] text-[#e6edf3] hover:bg-[#30363d] transition-colors"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
            Download Markdown
          </button>
          <button
            onClick={handleDownloadPDF}
            className="flex items-center gap-2 text-sm px-4 py-2 rounded-lg bg-[#1f6feb] border border-[#58a6ff]/30 text-white hover:bg-[#388bfd] transition-colors"
          >
            <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
            Download PDF
          </button>
        </div>
      </div>

      {/* Steps */}
      <div ref={guideRef} className="space-y-6">
        {guide.steps.map((step, index) => (
          <div
            key={step.id}
            className="rounded-xl border border-[#30363d] bg-[#161b22] overflow-hidden fade-in"
            style={{ animationDelay: `${index * 0.05}s` }}
          >
            {/* Step header */}
            <div className="flex items-center gap-3 px-5 py-4 border-b border-[#30363d] bg-[#21262d]">
              <div className="w-7 h-7 rounded-full bg-[#1f6feb] flex items-center justify-center shrink-0">
                <span className="text-xs font-bold text-white">{index + 1}</span>
              </div>
              <h2 className="text-base font-semibold text-[#e6edf3]">{step.title}</h2>
            </div>

            {/* Step body */}
            <div className="px-5 py-4">
              <p className="text-sm text-[#c9d1d9] leading-relaxed mb-4">{step.description}</p>

              {/* Warning */}
              {step.warning && (
                <div className="flex gap-3 p-3 rounded-lg bg-[#f85149]/10 border border-[#f85149]/30 mb-4">
                  <span className="text-[#f85149] shrink-0 mt-0.5">⚠️</span>
                  <p className="text-sm text-[#f85149]">{step.warning}</p>
                </div>
              )}

              {/* Commands */}
              {step.commands?.map((cmd, cmdIdx) => (
                <CodeBlock key={cmdIdx} block={cmd} />
              ))}

              {/* Tip */}
              {step.tip && (
                <div className="flex gap-3 p-3 rounded-lg bg-[#58a6ff]/10 border border-[#58a6ff]/30 mt-4">
                  <span className="text-[#58a6ff] shrink-0 mt-0.5">💡</span>
                  <p className="text-sm text-[#58a6ff]">{step.tip}</p>
                </div>
              )}
            </div>
          </div>
        ))}

        {/* Config TOML */}
        {guide.configToml && (
          <div className="rounded-xl border border-[#30363d] bg-[#161b22] overflow-hidden fade-in">
            <div className="flex items-center gap-3 px-5 py-4 border-b border-[#30363d] bg-[#21262d]">
              <div className="w-7 h-7 rounded-full bg-[#3fb950] flex items-center justify-center shrink-0">
                <span className="text-xs">⚙️</span>
              </div>
              <h2 className="text-base font-semibold text-[#e6edf3]">Your Generated config.toml</h2>
            </div>
            <div className="px-5 py-4">
              <p className="text-sm text-[#c9d1d9] leading-relaxed mb-4">
                This is your complete configuration file. Save it to <code className="bg-[#21262d] px-1 py-0.5 rounded text-[#58a6ff] text-xs">~/.oh-ben-claw/config.toml</code> on your host machine.
              </p>
              <CodeBlock block={{
                label: 'config.toml',
                language: 'toml',
                code: guide.configToml,
                copyable: true,
              }} />
            </div>
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="mt-10 pt-6 border-t border-[#30363d] text-center">
        <p className="text-xs text-[#8b949e]">
          Generated by the{' '}
          <a href="https://github.com/thewriterben/Oh-Ben-Claw" target="_blank" rel="noopener noreferrer" className="text-[#58a6ff] hover:underline">
            Oh-Ben-Claw
          </a>{' '}
          Deployment Guide Generator. If you run into issues, open an issue on GitHub.
        </p>
        <button
          onClick={onReset}
          className="mt-4 text-sm text-[#58a6ff] hover:underline"
        >
          Generate a new guide →
        </button>
      </div>
    </div>
  );
}
