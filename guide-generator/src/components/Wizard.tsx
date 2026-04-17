import { useState, useEffect, useRef } from 'react';
import { useWizard } from '../lib/wizard-context';
import { BOARDS, FEATURE_DESIRES } from '../data/hardware';
import type { BoardInfo, FeatureDesireInfo } from '../data/hardware';
import { generateGuide } from '../lib/guide-engine';
import { GuideView } from './GuideView';
import { RoleAssignmentStep } from './RoleAssignmentStep';
import type { LiveData } from '../lib/github-api';

interface Props {
  liveData: LiveData | null;
}

type WizardStep =
  | 'welcome'
  | 'goal'
  | 'host-os'
  | 'host-board'
  | 'features'
  | 'peripheral-boards'
  | 'role-assignment'
  | 'toolchain'
  | 'llm-provider'
  | 'llm-model'
  | 'wifi-config'
  | 'review'
  | 'guide';

interface ChatMessage {
  id: string;
  role: 'assistant' | 'user';
  content: React.ReactNode;
}

const HOST_BOARDS = BOARDS.filter(b => b.category === 'host');
const PERIPHERAL_BOARDS = BOARDS.filter(b => b.category !== 'host');

function BoardCard({ board, selected, onSelect }: { board: BoardInfo; selected: boolean; onSelect: () => void }) {
  return (
    <button
      onClick={onSelect}
      className={`text-left p-3 rounded-lg border transition-all ${
        selected
          ? 'border-[#58a6ff] bg-[#1f6feb]/10'
          : 'border-[#30363d] bg-[#161b22] hover:border-[#58a6ff]/50'
      }`}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className={`text-sm font-medium truncate ${selected ? 'text-[#58a6ff]' : 'text-[#e6edf3]'}`}>
            {board.displayName}
          </p>
          <p className="text-xs text-[#8b949e] mt-0.5 truncate">{board.architecture}</p>
        </div>
        {selected && (
          <svg className="w-4 h-4 text-[#58a6ff] shrink-0 mt-0.5" fill="currentColor" viewBox="0 0 20 20">
            <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clipRule="evenodd" />
          </svg>
        )}
      </div>
      <p className="text-xs text-[#8b949e] mt-1 line-clamp-2">{board.description}</p>
      <div className="flex flex-wrap gap-1 mt-2">
        {board.capabilities.slice(0, 4).map(cap => (
          <span key={cap} className="text-xs px-1.5 py-0.5 rounded bg-[#21262d] text-[#8b949e]">{cap}</span>
        ))}
      </div>
    </button>
  );
}

function FeatureCard({ feature, selected, onToggle }: { feature: FeatureDesireInfo; selected: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className={`text-left p-3 rounded-lg border transition-all ${
        selected
          ? 'border-[#58a6ff] bg-[#1f6feb]/10'
          : 'border-[#30363d] bg-[#161b22] hover:border-[#58a6ff]/50'
      }`}
    >
      <div className="flex items-center gap-2">
        <span className="text-lg">{feature.icon}</span>
        <div>
          <p className={`text-sm font-medium ${selected ? 'text-[#58a6ff]' : 'text-[#e6edf3]'}`}>
            {feature.label}
          </p>
          <p className="text-xs text-[#8b949e] mt-0.5">{feature.description}</p>
        </div>
        {selected && (
          <svg className="w-4 h-4 text-[#58a6ff] shrink-0 ml-auto" fill="currentColor" viewBox="0 0 20 20">
            <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clipRule="evenodd" />
          </svg>
        )}
      </div>
    </button>
  );
}

function OptionButton({ label, description, icon, selected, onClick }: {
  label: string;
  description?: string;
  icon?: string;
  selected?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`text-left w-full p-4 rounded-xl border transition-all ${
        selected
          ? 'border-[#58a6ff] bg-[#1f6feb]/10'
          : 'border-[#30363d] bg-[#161b22] hover:border-[#58a6ff]/50 hover:bg-[#21262d]'
      }`}
    >
      <div className="flex items-center gap-3">
        {icon && <span className="text-2xl">{icon}</span>}
        <div>
          <p className={`font-medium ${selected ? 'text-[#58a6ff]' : 'text-[#e6edf3]'}`}>{label}</p>
          {description && <p className="text-sm text-[#8b949e] mt-0.5">{description}</p>}
        </div>
        {selected && (
          <svg className="w-5 h-5 text-[#58a6ff] shrink-0 ml-auto" fill="currentColor" viewBox="0 0 20 20">
            <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.707-9.293a1 1 0 00-1.414-1.414L9 10.586 7.707 9.293a1 1 0 00-1.414 1.414l2 2a1 1 0 001.414 0l4-4z" clipRule="evenodd" />
          </svg>
        )}
      </div>
    </button>
  );
}

export function Wizard({ liveData }: Props) {
  const { state, update, reset } = useWizard();
  const [currentStep, setCurrentStep] = useState<WizardStep>('welcome');
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [guide, setGuide] = useState<ReturnType<typeof generateGuide> | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  const scrollToBottom = () => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  };

  useEffect(() => {
    scrollToBottom();
  }, [messages]);

  const addMessage = (msg: Omit<ChatMessage, 'id'>) => {
    setMessages(prev => [...prev, { ...msg, id: Math.random().toString(36).slice(2) }]);
  };

  const handleGoal = (goal: 'host' | 'peripheral' | 'full') => {
    update({ goal });
    addMessage({
      role: 'user',
      content: goal === 'host' ? 'Set up the host agent' : goal === 'peripheral' ? 'Set up a peripheral node' : 'Full deployment (host + peripherals)',
    });

    const nextMsg = goal === 'peripheral'
      ? "Great! Let's set up your peripheral node. What operating system is your build machine running? (This is the computer you'll use to compile and flash the firmware.)"
      : "Perfect. What operating system is your host machine running?";

    setTimeout(() => {
      addMessage({ role: 'assistant', content: nextMsg });
      setCurrentStep('host-os');
    }, 300);
  };

  const handleOS = (os: 'windows' | 'macos' | 'linux') => {
    update({ hostOS: os });
    addMessage({ role: 'user', content: `${os.charAt(0).toUpperCase() + os.slice(1)}` });

    const nextMsg = state.goal === 'peripheral'
      ? "Which peripheral board(s) are you setting up?"
      : "Which board will be running the Oh-Ben-Claw brain agent?";

    setTimeout(() => {
      addMessage({ role: 'assistant', content: nextMsg });
      setCurrentStep(state.goal === 'peripheral' ? 'peripheral-boards' : 'host-board');
    }, 300);
  };

  const handleHostBoard = (boardId: string) => {
    const board = BOARDS.find(b => b.id === boardId);
    update({ hostBoard: boardId });
    addMessage({ role: 'user', content: board?.displayName || boardId });

    setTimeout(() => {
      addMessage({
        role: 'assistant',
        content: "What features do you want your Oh-Ben-Claw deployment to have? Select all that apply.",
      });
      setCurrentStep('features');
    }, 300);
  };

  const handleFeaturesNext = () => {
    const featureNames = state.featureDesires.join(', ') || 'None selected';
    addMessage({ role: 'user', content: `Features: ${featureNames}` });

    if (state.goal === 'full' || state.goal === 'peripheral') {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Which peripheral boards are you connecting? You can select multiple.",
        });
        setCurrentStep('peripheral-boards');
      }, 300);
    } else {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Which LLM provider would you like to use for the AI brain?",
        });
        setCurrentStep('llm-provider');
      }, 300);
    }
  };

  const handlePeripheralBoardsNext = () => {
    const boardNames = state.peripheralBoards.map(id => BOARDS.find(b => b.id === id)?.displayName || id).join(', ');
    addMessage({ role: 'user', content: `Peripheral boards: ${boardNames || 'None'}` });
    setTimeout(() => {
      addMessage({
        role: 'assistant',
        content: "Now let's assign roles to each component — what does each board do in your deployment?",
      });
      setCurrentStep('role-assignment');
    }, 300);
  };

  const handleRoleAssignmentNext = () => {
    const totalRoles = state.roleConfigs.reduce((sum, rc) => sum + rc.assignments.length, 0);
    addMessage({
      role: 'user',
      content: totalRoles > 0
        ? `Roles assigned: ${totalRoles} role${totalRoles !== 1 ? 's' : ''} across ${state.roleConfigs.length} board${state.roleConfigs.length !== 1 ? 's' : ''}`
        : 'Skipped role assignment',
    });

    const hasESP32 = state.peripheralBoards.some(id => BOARDS.find(b => b.id === id)?.category === 'esp32');
    const hasArduino = state.peripheralBoards.some(id => BOARDS.find(b => b.id === id)?.category === 'arduino');

    if (hasESP32 || hasArduino) {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Which toolchain would you like to use for flashing firmware to your boards?",
        });
        setCurrentStep('toolchain');
      }, 300);
    } else {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Which LLM provider would you like to use?",
        });
        setCurrentStep('llm-provider');
      }, 300);
    }
  };

  const handleToolchain = (toolchain: 'rust-cargo' | 'arduino-ide' | 'vscode-platformio' | 'esp-idf' | 'probe-rs') => {
    update({ toolchain });
    const labels: Record<string, string> = {
      'rust-cargo': 'Rust / Cargo (Recommended)',
      'arduino-ide': 'Arduino IDE',
      'vscode-platformio': 'VS Code + PlatformIO',
      'esp-idf': 'ESP-IDF',
      'probe-rs': 'probe-rs',
    };
    addMessage({ role: 'user', content: labels[toolchain] || toolchain });

    setTimeout(() => {
      addMessage({
        role: 'assistant',
        content: "Which LLM provider would you like to use for the AI brain?",
      });
      setCurrentStep('llm-provider');
    }, 300);
  };

  const handleLLMProvider = (provider: 'openai' | 'anthropic' | 'ollama') => {
    update({ llmProvider: provider });
    const labels = { openai: 'OpenAI (GPT-4o)', anthropic: 'Anthropic (Claude)', ollama: 'Ollama (Local, Free)' };
    addMessage({ role: 'user', content: labels[provider] });

    const defaultModels = { openai: 'gpt-4o', anthropic: 'claude-3-5-sonnet-20241022', ollama: 'llama3.2' };
    update({ llmModel: defaultModels[provider] });

    const hasWifi = state.peripheralBoards.some(id => BOARDS.find(b => b.id === id)?.capabilities.includes('wifi'));

    if (provider !== 'ollama') {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: provider === 'openai'
            ? "You'll need an OpenAI API key. Get one at https://platform.openai.com/api-keys. You can enter it now (it will only be stored in your browser session) or leave it blank and set it as an environment variable later."
            : "You'll need an Anthropic API key. Get one at https://console.anthropic.com. You can enter it now or leave it blank.",
        });
        setCurrentStep('llm-model');
      }, 300);
    } else {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Great choice — Ollama runs locally and is completely free. Make sure you have Ollama installed (https://ollama.ai) and have pulled a model: `ollama pull llama3.2`",
        });
        if (hasWifi) {
          setCurrentStep('wifi-config');
          addMessage({
            role: 'assistant',
            content: "Your ESP32 boards need Wi-Fi credentials to connect to the MQTT spine. Enter your Wi-Fi network name and password:",
          });
        } else {
          setCurrentStep('review');
          addMessage({
            role: 'assistant',
            content: "Almost there! Here's a summary of your deployment. Does everything look correct?",
          });
        }
      }, 300);
    }
  };

  const handleLLMModelNext = () => {
    const hasWifi = state.peripheralBoards.some(id => BOARDS.find(b => b.id === id)?.capabilities.includes('wifi'));

    addMessage({ role: 'user', content: `API key: ${state.llmApiKey ? '••••••••' : '(will set later)'}, Model: ${state.llmModel}` });

    if (hasWifi) {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Your ESP32 boards need Wi-Fi credentials to connect to the MQTT spine. Enter your Wi-Fi network name and password:",
        });
        setCurrentStep('wifi-config');
      }, 300);
    } else {
      setTimeout(() => {
        addMessage({
          role: 'assistant',
          content: "Almost there! Here's a summary of your deployment. Does everything look correct?",
        });
        setCurrentStep('review');
      }, 300);
    }
  };

  const handleWifiNext = () => {
    addMessage({ role: 'user', content: `Wi-Fi: ${state.wifiSsid || '(not set)'}, MQTT Host: ${state.mqttHost}` });
    setTimeout(() => {
      addMessage({
        role: 'assistant',
        content: "Almost there! Here's a summary of your deployment. Does everything look correct?",
      });
      setCurrentStep('review');
    }, 300);
  };

  const handleGenerateGuide = () => {
    addMessage({ role: 'user', content: "Yes, generate my guide!" });
    const generatedGuide = generateGuide(state);
    setGuide(generatedGuide);
    setTimeout(() => {
      setCurrentStep('guide');
    }, 300);
  };

  const handleReset = () => {
    reset();
    setMessages([]);
    setGuide(null);
    setCurrentStep('welcome');
  };

  if (currentStep === 'guide' && guide) {
    return <GuideView guide={guide} onReset={handleReset} />;
  }

  return (
    <div className="flex flex-col h-screen max-h-screen">
      {/* Header */}
      <header className="shrink-0 border-b border-[#30363d] bg-[#161b22] px-6 py-4">
        <div className="max-w-3xl mx-auto flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="w-8 h-8 rounded-lg bg-[#1f6feb] flex items-center justify-center">
              <span className="text-white text-sm font-bold">OBC</span>
            </div>
            <div>
              <h1 className="text-sm font-semibold text-[#e6edf3]">Oh-Ben-Claw</h1>
              <p className="text-xs text-[#8b949e]">Deployment Guide Generator</p>
            </div>
          </div>
          {liveData && (
            <div className="hidden sm:flex items-center gap-4 text-xs text-[#8b949e]">
              <a
                href="https://github.com/thewriterben/Oh-Ben-Claw"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1 hover:text-[#58a6ff] transition-colors"
              >
                <svg className="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 20 20">
                  <path fillRule="evenodd" d="M10 0C4.477 0 0 4.484 0 10.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0110 4.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.203 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0020 10.017C20 4.484 15.522 0 10 0z" clipRule="evenodd" />
                </svg>
                ★ {liveData.obcRepo.stars}
              </a>
              {liveData.obcRepo.latestTag && (
                <span className="px-1.5 py-0.5 rounded bg-[#3fb950]/10 text-[#3fb950] border border-[#3fb950]/30">
                  {liveData.obcRepo.latestTag}
                </span>
              )}
            </div>
          )}
        </div>
      </header>

      {/* Chat area */}
      <div className="flex-1 overflow-y-auto px-4 py-6">
        <div className="max-w-3xl mx-auto space-y-4">

          {/* Welcome / initial message */}
          {currentStep === 'welcome' && (
            <div className="fade-in">
              <div className="flex gap-3 mb-6">
                <div className="w-8 h-8 rounded-full bg-[#1f6feb] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-white text-xs font-bold">OBC</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg">
                  <p className="text-sm text-[#e6edf3] leading-relaxed">
                    👋 Welcome to the <strong>Oh-Ben-Claw Deployment Guide Generator</strong>!
                  </p>
                  <p className="text-sm text-[#c9d1d9] leading-relaxed mt-2">
                    I'll walk you through setting up your Oh-Ben-Claw deployment step by step — from installing prerequisites to flashing firmware. Every command is included, so no prior experience is needed.
                  </p>
                  {liveData?.obcRepo.latestTag && (
                    <p className="text-xs text-[#8b949e] mt-2">
                      Latest release: <span className="text-[#3fb950]">{liveData.obcRepo.latestTag}</span>
                      {liveData.obcRepo.latestCommitDate && (
                        <> · Updated {new Date(liveData.obcRepo.latestCommitDate).toLocaleDateString()}</>
                      )}
                    </p>
                  )}
                </div>
              </div>

              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#1f6feb] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-white text-xs font-bold">OBC</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg">
                  <p className="text-sm text-[#e6edf3] font-medium mb-3">What would you like to set up today?</p>
                  <div className="space-y-2">
                    <OptionButton
                      icon="🧠"
                      label="Host Agent (Brain)"
                      description="Set up the core Oh-Ben-Claw AI agent on your PC, Mac, or Linux machine."
                      onClick={() => handleGoal('host')}
                    />
                    <OptionButton
                      icon="🔌"
                      label="Peripheral Node"
                      description="Flash firmware to a microcontroller (ESP32, Arduino, STM32) or set up a Raspberry Pi as a node."
                      onClick={() => handleGoal('peripheral')}
                    />
                    <OptionButton
                      icon="🚀"
                      label="Full Deployment"
                      description="Set up everything: the host brain agent AND one or more peripheral nodes."
                      onClick={() => handleGoal('full')}
                    />
                  </div>
                </div>
              </div>
            </div>
          )}

          {/* Chat messages */}
          {messages.map((msg) => (
            <div key={msg.id} className={`flex gap-3 fade-in ${msg.role === 'user' ? 'justify-end' : ''}`}>
              {msg.role === 'assistant' && (
                <div className="w-8 h-8 rounded-full bg-[#1f6feb] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-white text-xs font-bold">OBC</span>
                </div>
              )}
              <div className={`rounded-2xl px-4 py-3 max-w-lg text-sm leading-relaxed ${
                msg.role === 'assistant'
                  ? 'bg-[#161b22] border border-[#30363d] rounded-tl-none text-[#e6edf3]'
                  : 'bg-[#1f6feb] text-white rounded-tr-none'
              }`}>
                {msg.content}
              </div>
            </div>
          ))}

          {/* Current step input */}
          <div className="fade-in">
            {currentStep === 'host-os' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg w-full">
                  <p className="text-sm text-[#8b949e] mb-3">Select your operating system:</p>
                  <div className="space-y-2">
                    <OptionButton icon="🐧" label="Linux" description="Ubuntu, Debian, Fedora, Arch, etc." onClick={() => handleOS('linux')} />
                    <OptionButton icon="🍎" label="macOS" description="macOS 12 Monterey or later (Apple Silicon or Intel)" onClick={() => handleOS('macos')} />
                    <OptionButton icon="🪟" label="Windows" description="Windows 10 or 11 (WSL2 recommended)" onClick={() => handleOS('windows')} />
                  </div>
                </div>
              </div>
            )}

            {currentStep === 'host-board' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-2xl w-full">
                  <p className="text-sm text-[#8b949e] mb-3">Select your host board:</p>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                    {HOST_BOARDS.map(board => (
                      <BoardCard
                        key={board.id}
                        board={board}
                        selected={state.hostBoard === board.id}
                        onSelect={() => handleHostBoard(board.id)}
                      />
                    ))}
                  </div>
                </div>
              </div>
            )}

            {currentStep === 'features' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-2xl w-full">
                  <p className="text-sm text-[#8b949e] mb-3">Select desired features (choose all that apply):</p>
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 mb-4">
                    {FEATURE_DESIRES.map(feature => (
                      <FeatureCard
                        key={feature.id}
                        feature={feature}
                        selected={state.featureDesires.includes(feature.id)}
                        onToggle={() => {
                          const current = state.featureDesires;
                          update({
                            featureDesires: current.includes(feature.id)
                              ? current.filter(f => f !== feature.id)
                              : [...current, feature.id],
                          });
                        }}
                      />
                    ))}
                  </div>
                  <button
                    onClick={handleFeaturesNext}
                    className="w-full py-2.5 rounded-lg bg-[#1f6feb] text-white text-sm font-medium hover:bg-[#388bfd] transition-colors"
                  >
                    Continue →
                  </button>
                </div>
              </div>
            )}

            {currentStep === 'peripheral-boards' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-2xl w-full">
                  <p className="text-sm text-[#8b949e] mb-3">Select peripheral boards (choose all you have):</p>

                  {/* Group by category */}
                  {(['esp32', 'rpi', 'arduino', 'stm32', 'other'] as const).map(cat => {
                    const catBoards = PERIPHERAL_BOARDS.filter(b => b.category === cat);
                    if (catBoards.length === 0) return null;
                    const catLabels: Record<string, string> = {
                      esp32: '🔵 ESP32 Family',
                      rpi: '🟣 Raspberry Pi Pico',
                      arduino: '🟢 Arduino',
                      stm32: '🔴 STM32',
                      other: '⚪ Other',
                    };
                    return (
                      <div key={cat} className="mb-4">
                        <p className="text-xs font-medium text-[#8b949e] mb-2">{catLabels[cat]}</p>
                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                          {catBoards.map(board => (
                            <BoardCard
                              key={board.id}
                              board={board}
                              selected={state.peripheralBoards.includes(board.id)}
                              onSelect={() => {
                                const current = state.peripheralBoards;
                                update({
                                  peripheralBoards: current.includes(board.id)
                                    ? current.filter(id => id !== board.id)
                                    : [...current, board.id],
                                });
                              }}
                            />
                          ))}
                        </div>
                      </div>
                    );
                  })}

                  <button
                    onClick={handlePeripheralBoardsNext}
                    className="w-full py-2.5 rounded-lg bg-[#1f6feb] text-white text-sm font-medium hover:bg-[#388bfd] transition-colors mt-2"
                  >
                    Continue →
                  </button>
                </div>
              </div>
            )}

            {currentStep === 'role-assignment' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-2xl w-full">
                  <RoleAssignmentStep
                    onNext={handleRoleAssignmentNext}
                    onBack={() => setCurrentStep('peripheral-boards')}
                  />
                </div>
              </div>
            )}
            {currentStep === 'toolchain' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg w-full">
                  <p className="text-sm text-[#8b949e] mb-3">Select your preferred firmware toolchain:</p>
                  <div className="space-y-2">
                    <OptionButton
                      icon="🦀"
                      label="Rust / Cargo (Recommended)"
                      description="Best performance and full OBC firmware support. Slightly more setup required."
                      selected={state.toolchain === 'rust-cargo'}
                      onClick={() => handleToolchain('rust-cargo')}
                    />
                    <OptionButton
                      icon="🔧"
                      label="VS Code + PlatformIO"
                      description="Powerful IDE with automatic toolchain management. Great for beginners."
                      selected={state.toolchain === 'vscode-platformio'}
                      onClick={() => handleToolchain('vscode-platformio')}
                    />
                    <OptionButton
                      icon="🎨"
                      label="Arduino IDE"
                      description="The simplest option. Best for Arduino boards and basic ESP32 sketches."
                      selected={state.toolchain === 'arduino-ide'}
                      onClick={() => handleToolchain('arduino-ide')}
                    />
                    <OptionButton
                      icon="⚡"
                      label="ESP-IDF"
                      description="Espressif's official framework. Most control, steepest learning curve."
                      selected={state.toolchain === 'esp-idf'}
                      onClick={() => handleToolchain('esp-idf')}
                    />
                  </div>
                </div>
              </div>
            )}

            {currentStep === 'llm-provider' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg w-full">
                  <p className="text-sm text-[#8b949e] mb-3">Select your LLM provider:</p>
                  <div className="space-y-2">
                    <OptionButton
                      icon="🤖"
                      label="OpenAI (GPT-4o)"
                      description="Best quality. Requires an API key from platform.openai.com. Pay-per-use."
                      selected={state.llmProvider === 'openai'}
                      onClick={() => handleLLMProvider('openai')}
                    />
                    <OptionButton
                      icon="🧬"
                      label="Anthropic (Claude)"
                      description="Excellent quality. Requires an API key from console.anthropic.com. Pay-per-use."
                      selected={state.llmProvider === 'anthropic'}
                      onClick={() => handleLLMProvider('anthropic')}
                    />
                    <OptionButton
                      icon="🏠"
                      label="Ollama (Local, Free)"
                      description="Runs entirely on your machine. No API key needed. Requires a capable GPU."
                      selected={state.llmProvider === 'ollama'}
                      onClick={() => handleLLMProvider('ollama')}
                    />
                  </div>
                </div>
              </div>
            )}

            {currentStep === 'llm-model' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg w-full">
                  <div className="space-y-3">
                    <div>
                      <label className="text-xs text-[#8b949e] block mb-1">
                        API Key {state.llmProvider === 'openai' ? '(from platform.openai.com)' : '(from console.anthropic.com)'}
                      </label>
                      <input
                        type="password"
                        placeholder="sk-... (optional, can set later)"
                        value={state.llmApiKey}
                        onChange={e => update({ llmApiKey: e.target.value })}
                        className="w-full bg-[#0d1117] border border-[#30363d] rounded-lg px-3 py-2 text-sm text-[#e6edf3] placeholder-[#8b949e] focus:outline-none focus:border-[#58a6ff]"
                      />
                    </div>
                    <div>
                      <label className="text-xs text-[#8b949e] block mb-1">Model</label>
                      <input
                        type="text"
                        placeholder={state.llmProvider === 'openai' ? 'gpt-4o' : 'claude-3-5-sonnet-20241022'}
                        value={state.llmModel}
                        onChange={e => update({ llmModel: e.target.value })}
                        className="w-full bg-[#0d1117] border border-[#30363d] rounded-lg px-3 py-2 text-sm text-[#e6edf3] placeholder-[#8b949e] focus:outline-none focus:border-[#58a6ff]"
                      />
                    </div>
                    <button
                      onClick={handleLLMModelNext}
                      className="w-full py-2.5 rounded-lg bg-[#1f6feb] text-white text-sm font-medium hover:bg-[#388bfd] transition-colors"
                    >
                      Continue →
                    </button>
                  </div>
                </div>
              </div>
            )}

            {currentStep === 'wifi-config' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg w-full">
                  <p className="text-xs text-[#8b949e] mb-3">These will be embedded in the firmware config. You can change them later.</p>
                  <div className="space-y-3">
                    <div>
                      <label className="text-xs text-[#8b949e] block mb-1">Wi-Fi Network Name (SSID)</label>
                      <input
                        type="text"
                        placeholder="MyHomeNetwork"
                        value={state.wifiSsid}
                        onChange={e => update({ wifiSsid: e.target.value })}
                        className="w-full bg-[#0d1117] border border-[#30363d] rounded-lg px-3 py-2 text-sm text-[#e6edf3] placeholder-[#8b949e] focus:outline-none focus:border-[#58a6ff]"
                      />
                    </div>
                    <div>
                      <label className="text-xs text-[#8b949e] block mb-1">Wi-Fi Password</label>
                      <input
                        type="password"
                        placeholder="(leave blank to set later)"
                        value={state.wifiPassword}
                        onChange={e => update({ wifiPassword: e.target.value })}
                        className="w-full bg-[#0d1117] border border-[#30363d] rounded-lg px-3 py-2 text-sm text-[#e6edf3] placeholder-[#8b949e] focus:outline-none focus:border-[#58a6ff]"
                      />
                    </div>
                    <div>
                      <label className="text-xs text-[#8b949e] block mb-1">MQTT Broker Host</label>
                      <input
                        type="text"
                        placeholder="localhost or 192.168.x.x"
                        value={state.mqttHost}
                        onChange={e => update({ mqttHost: e.target.value })}
                        className="w-full bg-[#0d1117] border border-[#30363d] rounded-lg px-3 py-2 text-sm text-[#e6edf3] placeholder-[#8b949e] focus:outline-none focus:border-[#58a6ff]"
                      />
                    </div>
                    <button
                      onClick={handleWifiNext}
                      className="w-full py-2.5 rounded-lg bg-[#1f6feb] text-white text-sm font-medium hover:bg-[#388bfd] transition-colors"
                    >
                      Continue →
                    </button>
                  </div>
                </div>
              </div>
            )}

            {currentStep === 'review' && (
              <div className="flex gap-3">
                <div className="w-8 h-8 rounded-full bg-[#21262d] border border-[#30363d] flex items-center justify-center shrink-0 mt-1">
                  <span className="text-[#8b949e] text-xs">You</span>
                </div>
                <div className="bg-[#161b22] border border-[#30363d] rounded-2xl rounded-tl-none px-4 py-3 max-w-lg w-full">
                  <p className="text-sm font-medium text-[#e6edf3] mb-3">Deployment Summary</p>
                  <div className="space-y-2 text-sm">
                    <div className="flex justify-between">
                      <span className="text-[#8b949e]">Goal</span>
                      <span className="text-[#e6edf3] capitalize">{state.goal}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-[#8b949e]">OS</span>
                      <span className="text-[#e6edf3] capitalize">{state.hostOS}</span>
                    </div>
                    {state.hostBoard && (
                      <div className="flex justify-between">
                        <span className="text-[#8b949e]">Host Board</span>
                        <span className="text-[#e6edf3]">{BOARDS.find(b => b.id === state.hostBoard)?.displayName}</span>
                      </div>
                    )}
                    {state.peripheralBoards.length > 0 && (
                      <div className="flex justify-between gap-4">
                        <span className="text-[#8b949e] shrink-0">Peripherals</span>
                        <span className="text-[#e6edf3] text-right text-xs">
                          {state.peripheralBoards.map(id => BOARDS.find(b => b.id === id)?.displayName).join(', ')}
                        </span>
                      </div>
                    )}
                    {state.toolchain && (
                      <div className="flex justify-between">
                        <span className="text-[#8b949e]">Toolchain</span>
                        <span className="text-[#e6edf3] capitalize">{state.toolchain}</span>
                      </div>
                    )}
                    {state.llmProvider && (
                      <div className="flex justify-between">
                        <span className="text-[#8b949e]">LLM Provider</span>
                        <span className="text-[#e6edf3] capitalize">{state.llmProvider}</span>
                      </div>
                    )}
                    {state.featureDesires.length > 0 && (
                      <div className="flex justify-between gap-4">
                        <span className="text-[#8b949e] shrink-0">Features</span>
                        <span className="text-[#e6edf3] text-right text-xs">{state.featureDesires.join(', ')}</span>
                      </div>
                    )}
                    {state.roleConfigs.length > 0 && (
                      <div className="flex flex-col gap-1 pt-1 border-t border-[#30363d]">
                        <span className="text-[#8b949e] text-xs">Role Assignments</span>
                        {state.roleConfigs.map(rc => {
                          const board = BOARDS.find(b => b.id === rc.boardId);
                          return (
                            <div key={rc.boardId} className="text-xs text-[#e6edf3]">
                              <span className="text-[#58a6ff]">{board?.displayName || rc.boardId}:</span>{' '}
                              {rc.assignments.map(a => a.roleId).join(', ')}
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                  <div className="flex gap-2 mt-4">
                    <button
                      onClick={handleReset}
                      className="flex-1 py-2.5 rounded-lg border border-[#30363d] text-[#8b949e] text-sm hover:text-[#e6edf3] hover:border-[#58a6ff] transition-colors"
                    >
                      Start Over
                    </button>
                    <button
                      onClick={handleGenerateGuide}
                      className="flex-1 py-2.5 rounded-lg bg-[#1f6feb] text-white text-sm font-medium hover:bg-[#388bfd] transition-colors"
                    >
                      Generate Guide 🚀
                    </button>
                  </div>
                </div>
              </div>
            )}
          </div>

          <div ref={messagesEndRef} />
        </div>
      </div>
    </div>
  );
}
