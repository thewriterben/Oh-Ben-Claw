import { useEffect, useState } from 'react';
import './index.css';
import { WizardProvider } from './lib/wizard-context';
import { Wizard } from './components/Wizard';
import { fetchLiveData } from './lib/github-api';
import type { LiveData } from './lib/github-api';

export default function App() {
  const [liveData, setLiveData] = useState<LiveData | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    fetchLiveData()
      .then(data => setLiveData(data))
      .catch(() => setLiveData(null))
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-screen bg-[#0d1117]">
        <div className="text-center">
          <div className="w-10 h-10 rounded-lg bg-[#1f6feb] flex items-center justify-center mx-auto mb-4">
            <span className="text-white font-bold text-sm">OBC</span>
          </div>
          <p className="text-[#8b949e] text-sm">Loading latest repo data…</p>
          <div className="flex justify-center gap-1 mt-3">
            <span className="w-2 h-2 rounded-full bg-[#58a6ff] typing-dot" style={{ animationDelay: '0s' }} />
            <span className="w-2 h-2 rounded-full bg-[#58a6ff] typing-dot" style={{ animationDelay: '0.2s' }} />
            <span className="w-2 h-2 rounded-full bg-[#58a6ff] typing-dot" style={{ animationDelay: '0.4s' }} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <WizardProvider>
      <Wizard liveData={liveData} />
    </WizardProvider>
  );
}
