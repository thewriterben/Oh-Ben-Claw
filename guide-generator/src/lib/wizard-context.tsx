import { createContext, useContext, useState } from 'react';
import type { ReactNode } from 'react';
import type { WizardState } from './guide-engine';

const defaultState: WizardState = {
  goal: null,
  hostOS: null,
  hostBoard: null,
  peripheralBoards: [],
  roleConfigs: [],
  toolchain: null,
  featureDesires: [],
  wifiSsid: '',
  wifiPassword: '',
  mqttHost: 'localhost',
  llmProvider: null,
  llmApiKey: '',
  llmModel: '',
  nodeId: '',
};

interface WizardContextValue {
  state: WizardState;
  update: (partial: Partial<WizardState>) => void;
  reset: () => void;
}

const WizardContext = createContext<WizardContextValue | null>(null);

export function WizardProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<WizardState>(defaultState);

  const update = (partial: Partial<WizardState>) => {
    setState(prev => ({ ...prev, ...partial }));
  };

  const reset = () => setState(defaultState);

  return (
    <WizardContext.Provider value={{ state, update, reset }}>
      {children}
    </WizardContext.Provider>
  );
}

export function useWizard() {
  const ctx = useContext(WizardContext);
  if (!ctx) throw new Error('useWizard must be used inside WizardProvider');
  return ctx;
}
