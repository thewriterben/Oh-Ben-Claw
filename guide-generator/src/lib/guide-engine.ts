// ── Guide Engine ──────────────────────────────────────────────────────────────
// Generates step-by-step deployment guides based on user selections.
// Every command and code block is included for zero-experience users.

import { BOARD_BY_ID, ROLE_BY_ID } from '../data/hardware';
import type { HostOS, Toolchain, BoardRoleConfig } from '../data/hardware';

export interface WizardState {
  goal: 'host' | 'peripheral' | 'full' | null;
  hostOS: HostOS | null;
  hostBoard: string | null;
  peripheralBoards: string[];
  /** Role assignments keyed by boardId */
  roleConfigs: BoardRoleConfig[];
  toolchain: Toolchain | null;
  featureDesires: string[];
  wifiSsid: string;
  wifiPassword: string;
  mqttHost: string;
  llmProvider: 'openai' | 'anthropic' | 'ollama' | null;
  llmApiKey: string;
  llmModel: string;
  nodeId: string;
}

export interface GuideStep {
  id: string;
  title: string;
  description: string;
  commands?: CodeBlock[];
  warning?: string;
  tip?: string;
  substeps?: GuideStep[];
}

export interface CodeBlock {
  label?: string;
  language: string;
  code: string;
  copyable: boolean;
  platform?: HostOS | 'all';
}

export interface GeneratedGuide {
  title: string;
  summary: string;
  estimatedTime: string;
  difficulty: 'beginner' | 'intermediate' | 'advanced';
  steps: GuideStep[];
  configToml?: string;
  firmwareCode?: string;
}

// ── OS Prerequisites ──────────────────────────────────────────────────────────

function getOSPrerequisites(os: HostOS): GuideStep {
  if (os === 'linux') {
    return {
      id: 'prereq-linux',
      title: 'Install System Prerequisites (Linux)',
      description: 'First, update your system and install the essential build tools. Open a terminal (press Ctrl+Alt+T on Ubuntu) and run these commands one by one. The `sudo` command requires your user password.',
      commands: [
        {
          label: 'Update package lists and upgrade existing packages',
          language: 'bash',
          code: `sudo apt update && sudo apt upgrade -y`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'Install build essentials, SSL libraries, and other required tools',
          language: 'bash',
          code: `sudo apt install -y build-essential pkg-config libssl-dev \\\n  libsqlite3-dev curl git wget unzip libudev-dev`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'Install Rust (the programming language Oh-Ben-Claw is written in)',
          language: 'bash',
          code: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'After the Rust installer finishes, reload your shell environment so the `cargo` and `rustc` commands are available',
          language: 'bash',
          code: `source "$HOME/.cargo/env"`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'Verify Rust is installed correctly',
          language: 'bash',
          code: `rustc --version\ncargo --version`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'Install Node.js 20+ and pnpm (needed for the GUI)',
          language: 'bash',
          code: `curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -\nsudo apt install -y nodejs\nnpm install -g pnpm`,
          copyable: true,
          platform: 'linux',
        },
      ],
      tip: 'If you are on Fedora, replace `apt` with `dnf`. If you are on Arch Linux, use `pacman -S base-devel openssl sqlite curl git wget unzip`.',
    };
  }

  if (os === 'macos') {
    return {
      id: 'prereq-macos',
      title: 'Install System Prerequisites (macOS)',
      description: 'On macOS, we use Homebrew as the package manager. If you do not have Homebrew installed, the first command will install it for you. Open the Terminal app (press Cmd+Space, type "Terminal", press Enter).',
      commands: [
        {
          label: 'Install Homebrew (the macOS package manager). If already installed, this will update it.',
          language: 'bash',
          code: `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"`,
          copyable: true,
          platform: 'macos',
        },
        {
          label: 'Install Git and other tools',
          language: 'bash',
          code: `brew install git curl wget`,
          copyable: true,
          platform: 'macos',
        },
        {
          label: 'Install Rust',
          language: 'bash',
          code: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`,
          copyable: true,
          platform: 'macos',
        },
        {
          label: 'Reload your shell environment',
          language: 'bash',
          code: `source "$HOME/.cargo/env"`,
          copyable: true,
          platform: 'macos',
        },
        {
          label: 'Verify Rust is installed',
          language: 'bash',
          code: `rustc --version\ncargo --version`,
          copyable: true,
          platform: 'macos',
        },
        {
          label: 'Install Node.js 20+ and pnpm (needed for the GUI)',
          language: 'bash',
          code: `brew install node\nnpm install -g pnpm`,
          copyable: true,
          platform: 'macos',
        },
        {
          label: 'Install Xcode Command Line Tools (required for Tauri GUI builds)',
          language: 'bash',
          code: `xcode-select --install`,
          copyable: true,
          platform: 'macos',
        },
      ],
      tip: 'On Apple Silicon (M1/M2/M3/M4), Homebrew installs to /opt/homebrew. Make sure to follow any post-install instructions Homebrew prints to add it to your PATH.',
    };
  }

  // Windows
  return {
    id: 'prereq-windows',
    title: 'Install System Prerequisites (Windows)',
    description: 'On Windows, we recommend using Windows Subsystem for Linux 2 (WSL2) for the best experience. This gives you a full Linux environment inside Windows. Alternatively, you can use native Windows tools, but WSL2 is strongly recommended for beginners.',
    commands: [
      {
        label: 'Open PowerShell as Administrator (right-click the Start menu → "Windows PowerShell (Admin)") and run this command to install WSL2 with Ubuntu',
        language: 'powershell',
        code: `wsl --install`,
        copyable: true,
        platform: 'windows',
      },
      {
        label: 'Restart your computer when prompted. After restarting, Ubuntu will finish installing and ask you to create a username and password.',
        language: 'text',
        code: `# Restart your computer now, then continue in the Ubuntu terminal that opens automatically.`,
        copyable: false,
        platform: 'windows',
      },
      {
        label: 'Once inside the Ubuntu (WSL2) terminal, follow the Linux prerequisites above. Start here:',
        language: 'bash',
        code: `sudo apt update && sudo apt upgrade -y\nsudo apt install -y build-essential pkg-config libssl-dev libsqlite3-dev curl git wget unzip libudev-dev`,
        copyable: true,
        platform: 'windows',
      },
      {
        label: 'Install Rust inside WSL2',
        language: 'bash',
        code: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh\nsource "$HOME/.cargo/env"`,
        copyable: true,
        platform: 'windows',
      },
    ],
    warning: 'If you choose NOT to use WSL2, you will need to install Visual Studio Build Tools 2022 from https://visualstudio.microsoft.com/visual-cpp-build-tools/ and Rust from https://rustup.rs. The experience is more complex and is not recommended for beginners.',
    tip: 'After installing WSL2, you can open the Ubuntu terminal at any time by searching for "Ubuntu" in the Start menu.',
  };
}

// ── MQTT Broker Setup ─────────────────────────────────────────────────────────

function getMQTTBrokerStep(os: HostOS): GuideStep {
  const linuxCommands: CodeBlock[] = [
    {
      label: 'Install Mosquitto MQTT broker',
      language: 'bash',
      code: `sudo apt install -y mosquitto mosquitto-clients`,
      copyable: true,
      platform: 'linux',
    },
    {
      label: 'Enable and start the Mosquitto service so it runs automatically on boot',
      language: 'bash',
      code: `sudo systemctl enable mosquitto\nsudo systemctl start mosquitto`,
      copyable: true,
      platform: 'linux',
    },
    {
      label: 'Verify Mosquitto is running',
      language: 'bash',
      code: `sudo systemctl status mosquitto`,
      copyable: true,
      platform: 'linux',
    },
    {
      label: 'Test the broker by subscribing to a test topic in one terminal and publishing in another',
      language: 'bash',
      code: `# Terminal 1 — subscribe:\nmosquitto_sub -h localhost -t "obc/test"\n\n# Terminal 2 — publish:\nmosquitto_pub -h localhost -t "obc/test" -m "hello"`,
      copyable: true,
      platform: 'linux',
    },
  ];

  const macCommands: CodeBlock[] = [
    {
      label: 'Install Mosquitto via Homebrew',
      language: 'bash',
      code: `brew install mosquitto`,
      copyable: true,
      platform: 'macos',
    },
    {
      label: 'Start Mosquitto as a background service',
      language: 'bash',
      code: `brew services start mosquitto`,
      copyable: true,
      platform: 'macos',
    },
    {
      label: 'Verify it is running',
      language: 'bash',
      code: `brew services list | grep mosquitto`,
      copyable: true,
      platform: 'macos',
    },
  ];

  return {
    id: 'mqtt-broker',
    title: 'Install and Configure the MQTT Broker (Mosquitto)',
    description: 'Oh-Ben-Claw uses MQTT as its communication backbone (called the "Spine"). Mosquitto is a lightweight, open-source MQTT broker that runs on your host machine. All peripheral nodes connect to it to send and receive messages.',
    commands: os === 'macos' ? macCommands : linuxCommands,
    tip: 'The MQTT broker runs on port 1883 by default. If you have a firewall, you may need to allow this port. For a home network, this is usually not necessary.',
  };
}

// ── Clone & Build Oh-Ben-Claw ─────────────────────────────────────────────────

function getCloneAndBuildStep(_os: HostOS, features: string[]): GuideStep {
  const featureFlags = buildFeatureFlags(features);

  return {
    id: 'clone-build',
    title: 'Clone and Build Oh-Ben-Claw',
    description: 'Now we will download the Oh-Ben-Claw source code from GitHub and compile it. The first build may take 5–10 minutes as Rust downloads and compiles all dependencies.',
    commands: [
      {
        label: 'Clone the repository',
        language: 'bash',
        code: `git clone https://github.com/thewriterben/Oh-Ben-Claw.git\ncd Oh-Ben-Claw`,
        copyable: true,
        platform: 'all',
      },
      {
        label: `Build the core agent with the required features: ${featureFlags || 'default'}`,
        language: 'bash',
        code: featureFlags
          ? `cargo build --release --features "${featureFlags}"`
          : `cargo build --release`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Verify the build succeeded — this should print the version number',
        language: 'bash',
        code: `./target/release/oh-ben-claw --version`,
        copyable: true,
        platform: 'all',
      },
    ],
    tip: 'The compiled binary is at `target/release/oh-ben-claw`. You can copy it to `/usr/local/bin/` to make it available system-wide: `sudo cp target/release/oh-ben-claw /usr/local/bin/`',
  };
}

function buildFeatureFlags(features: string[]): string {
  const flags: string[] = [];
  if (features.some(f => ['Vision', 'Listening', 'Speech', 'EnvironmentalSensing', 'WirelessMesh'].includes(f))) {
    flags.push('hardware');
  }
  if (features.includes('WirelessMesh')) {
    flags.push('mqtt-spine');
  }
  return flags.join(',');
}

// ── Configuration ─────────────────────────────────────────────────────────────

function getConfigStep(state: WizardState): GuideStep {
  const config = generateConfigToml(state);

  return {
    id: 'configure',
    title: 'Create the Configuration File',
    description: 'Oh-Ben-Claw is configured via a TOML file at `~/.oh-ben-claw/config.toml`. The `~` symbol means your home directory (e.g., `/home/yourname/` on Linux, `/Users/yourname/` on macOS). Run these commands to create it:',
    commands: [
      {
        label: 'Create the configuration directory',
        language: 'bash',
        code: `mkdir -p ~/.oh-ben-claw`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Create the configuration file with your settings. Copy the entire block below and paste it into your terminal.',
        language: 'bash',
        code: `cat > ~/.oh-ben-claw/config.toml << 'EOF'\n${config}\nEOF`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Verify the file was created correctly',
        language: 'bash',
        code: `cat ~/.oh-ben-claw/config.toml`,
        copyable: true,
        platform: 'all',
      },
    ],
    tip: 'You can also edit this file with any text editor: `nano ~/.oh-ben-claw/config.toml` (press Ctrl+X, then Y, then Enter to save in nano).',
  };
}

function generateConfigToml(state: WizardState): string {
  const lines: string[] = [];

  lines.push(`# Oh-Ben-Claw Configuration`);
  lines.push(`# Generated by the Oh-Ben-Claw Deployment Guide Generator`);
  lines.push(`# Edit this file to customise your deployment.`);
  lines.push(``);
  lines.push(`[agent]`);
  lines.push(`name = "Oh-Ben-Claw"`);
  lines.push(`system_prompt = """`);
  lines.push(`You are Oh-Ben-Claw, an advanced multi-device AI assistant.`);
  lines.push(`You can interact with the physical world through connected hardware nodes.`);
  lines.push(`"""`);
  lines.push(`max_tool_iterations = 15`);
  lines.push(``);

  lines.push(`[provider]`);
  if (state.llmProvider === 'openai') {
    lines.push(`name = "openai"`);
    lines.push(`model = "${state.llmModel || 'gpt-4o'}"`);
    if (state.llmApiKey) {
      lines.push(`api_key = "${state.llmApiKey}"`);
    } else {
      lines.push(`# api_key = "sk-..."  # Or set the OPENAI_API_KEY environment variable`);
    }
  } else if (state.llmProvider === 'anthropic') {
    lines.push(`name = "anthropic"`);
    lines.push(`model = "${state.llmModel || 'claude-3-5-sonnet-20241022'}"`);
    if (state.llmApiKey) {
      lines.push(`api_key = "${state.llmApiKey}"`);
    } else {
      lines.push(`# api_key = "sk-ant-..."  # Or set the ANTHROPIC_API_KEY environment variable`);
    }
  } else if (state.llmProvider === 'ollama') {
    lines.push(`name = "ollama"`);
    lines.push(`model = "${state.llmModel || 'llama3.2'}"`);
    lines.push(`base_url = "http://localhost:11434"`);
  } else {
    lines.push(`name = "openai"`);
    lines.push(`model = "gpt-4o"`);
    lines.push(`# api_key = "sk-..."  # Set your API key here or via OPENAI_API_KEY env var`);
  }
  lines.push(``);

  if (state.featureDesires.includes('WirelessMesh') || state.peripheralBoards.length > 0) {
    lines.push(`[spine]`);
    lines.push(`kind = "mqtt"`);
    lines.push(`host = "${state.mqttHost || 'localhost'}"`);
    lines.push(`port = 1883`);
    lines.push(`tool_timeout_secs = 30`);
    lines.push(``);
  }

  if (state.featureDesires.includes('PersistentMemory')) {
    lines.push(`[memory]`);
    lines.push(`backend = "sqlite"`);
    lines.push(`path = "~/.oh-ben-claw/memory.db"`);
    lines.push(``);
  }

  if (state.peripheralBoards.length > 0) {
    lines.push(`[peripherals]`);
    lines.push(`enabled = true`);
    lines.push(`datasheet_dir = "docs/datasheets"`);
    lines.push(``);

    for (const boardId of state.peripheralBoards) {
      const board = BOARD_BY_ID[boardId];
      if (!board) continue;

      lines.push(`# ${board.displayName}`);
      lines.push(`[[peripherals.boards]]`);
      // Role assignments for this board
      const roleConfig = state.roleConfigs?.find(rc => rc.boardId === boardId);
      const roleIds = roleConfig?.assignments.map(a => a.roleId) ?? [];

      lines.push(`board = "${boardId}"`);
      lines.push(`transport = "${board.transport}"`);

      if (board.transport === 'serial') {
        lines.push(`path = "/dev/ttyUSB0"   # Linux: /dev/ttyUSB0 or /dev/ttyACM0; macOS: /dev/cu.usbmodem*`);
        lines.push(`baud = 115200`);
      } else if (board.transport === 'mqtt') {
        lines.push(`node_id = "${state.nodeId || boardId + '-node'}"`);
      }

      // Emit role assignments
      if (roleIds.length > 0) {
        lines.push(`roles = [${roleIds.map(r => `"${r}"`).join(', ')}]`);
        for (const assignment of (roleConfig?.assignments ?? [])) {
          const roleDef = ROLE_BY_ID[assignment.roleId];
          if (!roleDef) continue;
          lines.push(`# ${roleDef.label} — bus: ${assignment.bus}${assignment.pinOrAddress ? ', pin/addr: ' + assignment.pinOrAddress : ''}`);
          lines.push(`[peripherals.roles.${assignment.roleId}.${boardId.replace(/-/g, '_')}]`);
          lines.push(`bus = "${assignment.bus}"`);
          if (assignment.pinOrAddress) lines.push(`pin_or_address = "${assignment.pinOrAddress}"`);
          if (assignment.notes) lines.push(`notes = "${assignment.notes}"`);
          lines.push(``);
        }
      }
      lines.push(``);
    }
  }

  return lines.join('\n');
}

// ── Run the Agent ─────────────────────────────────────────────────────────────

function getRunStep(): GuideStep {
  return {
    id: 'run',
    title: 'Run Oh-Ben-Claw',
    description: 'With the configuration in place, you can now start the agent. The first time you run it, it will validate your configuration and connect to your LLM provider.',
    commands: [
      {
        label: 'Start the agent in interactive CLI mode',
        language: 'bash',
        code: `./target/release/oh-ben-claw`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Or, if you installed it system-wide',
        language: 'bash',
        code: `oh-ben-claw`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Run the system diagnostics tool to check everything is configured correctly',
        language: 'bash',
        code: `oh-ben-claw doctor`,
        copyable: true,
        platform: 'all',
      },
    ],
    tip: 'To run Oh-Ben-Claw with the real-time terminal dashboard, add the `--features dashboard` flag when building: `cargo build --release --features dashboard`',
  };
}

// ── GUI Setup ─────────────────────────────────────────────────────────────────

function getGUIStep(os: HostOS): GuideStep {
  const linuxDeps: CodeBlock = {
    label: 'Install Tauri system dependencies (Linux only)',
    language: 'bash',
    code: `sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev \\\n  libayatana-appindicator3-dev librsvg2-dev libssl-dev`,
    copyable: true,
    platform: 'linux',
  };

  const commands: CodeBlock[] = [];
  if (os === 'linux') commands.push(linuxDeps);
  if (os === 'macos') {
    commands.push({
      label: 'Install Xcode Command Line Tools (if not already installed)',
      language: 'bash',
      code: `xcode-select --install`,
      copyable: true,
      platform: 'macos',
    });
  }

  commands.push(
    {
      label: 'Navigate to the GUI directory and install JavaScript dependencies',
      language: 'bash',
      code: `cd gui\npnpm install`,
      copyable: true,
      platform: 'all',
    },
    {
      label: 'Build the native desktop application',
      language: 'bash',
      code: `pnpm tauri build`,
      copyable: true,
      platform: 'all',
    },
    {
      label: 'The installer will be in the `gui/src-tauri/target/release/bundle/` directory. On Linux, look for a `.deb` or `.AppImage` file. On macOS, look for a `.dmg` file.',
      language: 'bash',
      code: `ls gui/src-tauri/target/release/bundle/`,
      copyable: true,
      platform: 'all',
    },
  );

  return {
    id: 'gui',
    title: 'Build the Native Desktop GUI (Optional)',
    description: 'Oh-Ben-Claw includes a native desktop application built with Tauri 2 and React. It provides a visual interface for chat, device management, tool logs, and settings. This step is optional — the CLI works perfectly well without it.',
    commands,
    tip: 'For development (with hot-reload), use `pnpm tauri dev` instead of `pnpm tauri build`.',
  };
}

// ── ESP32 Firmware Setup ──────────────────────────────────────────────────────

function getESP32FirmwareStep(boardId: string, os: HostOS): GuideStep {
  const board = BOARD_BY_ID[boardId];
  const boardLabel = board?.displayName || boardId;

  return {
    id: `firmware-${boardId}`,
    title: `Flash Oh-Ben-Claw Firmware to ${boardLabel}`,
    description: `This step flashes the Oh-Ben-Claw ESP32-S3 firmware to your ${boardLabel}. The firmware enables the board to communicate with the host agent over USB serial or MQTT Wi-Fi. We use the Rust ESP toolchain (espup) and espflash.`,
    commands: [
      {
        label: 'Install espup — the ESP Rust toolchain installer',
        language: 'bash',
        code: `cargo install espup`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Install the ESP Rust toolchain (this downloads ~1 GB of tools — takes a few minutes)',
        language: 'bash',
        code: `espup install`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Load the ESP environment variables. You must run this in every new terminal session before building ESP32 firmware.',
        language: 'bash',
        code: `. $HOME/export-esp.sh`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Add this line to your shell profile so it loads automatically',
        language: 'bash',
        code: `echo '. $HOME/export-esp.sh' >> ~/.bashrc\nsource ~/.bashrc`,
        copyable: true,
        platform: 'linux',
      },
      {
        label: 'Install espflash — the tool that flashes firmware to the board',
        language: 'bash',
        code: `cargo install espflash`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Navigate to the ESP32-S3 firmware directory',
        language: 'bash',
        code: `cd firmware/obc-esp32-s3`,
        copyable: true,
        platform: 'all',
      },
      {
        label: `Connect your ${boardLabel} to your computer via USB. Then build and flash the firmware. The --monitor flag opens a serial monitor so you can see the boot log.`,
        language: 'bash',
        code: `cargo build --release\ncargo espflash flash --monitor`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'You should see output like this in the serial monitor when the board boots successfully:',
        language: 'text',
        code: `I (XXX) obc_esp32_s3: Oh-Ben-Claw firmware starting — node_id=obc-esp32-s3-XXXX\nI (XXX) obc_esp32_s3: Board preset: ${board?.displayName || boardId}\nI (XXX) obc_esp32_s3: UART0 ready at 115200 baud — waiting for JSON commands`,
        copyable: false,
        platform: 'all',
      },
      {
        label: 'Test the firmware by sending a ping command (press Ctrl+C first to exit the monitor, then open a new terminal)',
        language: 'bash',
        code: `# Replace /dev/ttyUSB0 with your actual serial port\necho '{"id":1,"cmd":"ping","args":{}}' | nc -q1 /dev/ttyUSB0 115200\n# Expected response: {"id":1,"ok":true,"result":"pong"}`,
        copyable: true,
        platform: 'all',
      },
    ],
    warning: os === 'windows'
      ? 'On Windows (WSL2), USB devices are not automatically available in WSL. You need to install usbipd-win: https://github.com/dorssel/usbipd-win. Then run `usbipd attach --wsl --busid <busid>` in a Windows PowerShell (Admin) terminal.'
      : undefined,
    tip: 'On Linux, you may need to add your user to the `dialout` group to access serial ports without sudo: `sudo usermod -aG dialout $USER` (then log out and back in).',
  };
}

// ── USB Driver Step ───────────────────────────────────────────────────────────

function getUSBDriverStep(boardId: string, os: HostOS): GuideStep | null {
  const board = BOARD_BY_ID[boardId];
  if (!board?.usbDriverNote) return null;

  const commands: CodeBlock[] = [];

  if (os === 'linux') {
    commands.push({
      label: 'On Linux, add your user to the dialout group to access serial ports',
      language: 'bash',
      code: `sudo usermod -aG dialout $USER\n# Log out and back in for this to take effect`,
      copyable: true,
      platform: 'linux',
    });
    commands.push({
      label: 'Check that your board is detected after plugging it in',
      language: 'bash',
      code: `ls /dev/ttyUSB* /dev/ttyACM* 2>/dev/null`,
      copyable: true,
      platform: 'linux',
    });
  } else if (os === 'macos') {
    commands.push({
      label: 'Check that your board is detected after plugging it in',
      language: 'bash',
      code: `ls /dev/cu.*`,
      copyable: true,
      platform: 'macos',
    });
  } else {
    commands.push({
      label: 'On Windows, open Device Manager (Win+X → Device Manager) and look under "Ports (COM & LPT)" after plugging in the board.',
      language: 'text',
      code: `# If the board appears as an "Unknown Device", you need to install a driver.\n# See the note below for the correct driver for your board.`,
      copyable: false,
      platform: 'windows',
    });
  }

  return {
    id: `usb-driver-${boardId}`,
    title: `USB Driver Setup for ${board.displayName}`,
    description: `${board.usbDriverNote} This step ensures your computer can communicate with the board over USB.`,
    commands,
    tip: 'Common driver download links: CP210x (Silicon Labs): https://www.silabs.com/developers/usb-to-uart-bridge-vcp-drivers | CH340/CH341: https://www.wch-ic.com/downloads/CH341SER_EXE.html',
  };
}

// ── OTA Setup ─────────────────────────────────────────────────────────────────

function getOTAStep(boardId: string): GuideStep {
  const board = BOARD_BY_ID[boardId];

  if (board?.category === 'esp32') {
    return {
      id: `ota-${boardId}`,
      title: `Configure OTA (Over-The-Air) Updates for ${board?.displayName || boardId}`,
      description: 'OTA updates allow you to update the firmware on your ESP32 board wirelessly, without a USB cable. The Oh-Ben-Claw firmware includes an OTA update tool that can be triggered from the host agent.',
      commands: [
        {
          label: 'First, configure the Wi-Fi credentials on the board by sending a JSON command over serial',
          language: 'bash',
          code: `# Replace /dev/ttyUSB0 with your serial port, and fill in your Wi-Fi details\necho '{"id":1,"cmd":"agent_config","args":{"wifi_ssid":"YOUR_WIFI_NAME","wifi_password":"YOUR_WIFI_PASSWORD","llm_api_key":"sk-...","llm_base_url":"https://api.openai.com","llm_model":"gpt-4o-mini"}}' > /dev/ttyUSB0`,
          copyable: true,
          platform: 'all',
        },
        {
          label: 'Verify the board connected to Wi-Fi by checking the serial monitor output',
          language: 'bash',
          code: `# You should see: "WiFi connected — IP: 192.168.x.x"`,
          copyable: false,
          platform: 'all',
        },
        {
          label: 'To trigger an OTA update from the host agent, use the ota_update tool in the Oh-Ben-Claw CLI',
          language: 'text',
          code: `# In the Oh-Ben-Claw CLI, type:\nUpdate the firmware on my ESP32 node\n\n# Or use the tool directly:\n> ota_update node_name="esp32-sensor-1" board_type="esp32" firmware_url="https://example.com/firmware.bin"`,
          copyable: false,
          platform: 'all',
        },
      ],
      tip: 'To build a firmware binary for OTA distribution, run: `cargo build --release` and find the binary at `target/xtensa-esp32s3-espidf/release/obc-esp32-s3`.',
    };
  }

  if (board?.category === 'rpi' || board?.category === 'host') {
    return {
      id: `ota-${boardId}`,
      title: `Configure OTA Updates for ${board?.displayName || boardId}`,
      description: 'For Linux-based boards (Raspberry Pi, NanoPi), OTA updates are performed via SSH. The Oh-Ben-Claw OTA tool connects over SSH, runs system updates, and restarts the service.',
      commands: [
        {
          label: 'Enable SSH on the Raspberry Pi (if not already enabled)',
          language: 'bash',
          code: `sudo systemctl enable ssh\nsudo systemctl start ssh`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'Find the IP address of your Raspberry Pi',
          language: 'bash',
          code: `hostname -I`,
          copyable: true,
          platform: 'linux',
        },
        {
          label: 'From the host machine, trigger an OTA update via the Oh-Ben-Claw CLI',
          language: 'text',
          code: `# In the Oh-Ben-Claw CLI:\nUpdate the software on my Raspberry Pi node\n\n# Or use the tool directly:\n> ota_update node_name="rpi-living-room" board_type="rpi"`,
          copyable: false,
          platform: 'all',
        },
      ],
    };
  }

  return {
    id: `ota-${boardId}`,
    title: `OTA Updates for ${board?.displayName || boardId}`,
    description: 'OTA updates for this board type use probe-rs to flash new firmware over the ST-Link debug connection.',
    commands: [
      {
        label: 'Install probe-rs tools',
        language: 'bash',
        code: `cargo install probe-rs-tools --locked`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Flash new firmware using probe-rs',
        language: 'bash',
        code: `probe-rs flash --chip STM32F401RE target/thumbv7em-none-eabihf/release/obc-stm32`,
        copyable: true,
        platform: 'all',
      },
    ],
  };
}

// ── Arduino IDE Setup ─────────────────────────────────────────────────────────

function getArduinoIDEStep(boardId: string, os: HostOS): GuideStep {
  const board = BOARD_BY_ID[boardId];
  const isESP32 = board?.category === 'esp32';
  const isArduino = board?.category === 'arduino';

  const boardManagerURL = isESP32
    ? 'https://raw.githubusercontent.com/espressif/arduino-esp32/gh-pages/package_esp32_index.json'
    : '';

  const commands: CodeBlock[] = [];

  if (os === 'linux') {
    commands.push({
      label: 'Download and install Arduino IDE 2.x',
      language: 'bash',
      code: `# Download the latest Arduino IDE AppImage\nwget -O ~/arduino-ide.AppImage "https://downloads.arduino.cc/arduino-ide/arduino-ide_latest_Linux_64bit.AppImage"\nchmod +x ~/arduino-ide.AppImage\n# Run it:\n~/arduino-ide.AppImage`,
      copyable: true,
      platform: 'linux',
    });
  } else if (os === 'macos') {
    commands.push({
      label: 'Install Arduino IDE via Homebrew',
      language: 'bash',
      code: `brew install --cask arduino-ide`,
      copyable: true,
      platform: 'macos',
    });
  } else {
    commands.push({
      label: 'Download and install Arduino IDE 2.x from the official website',
      language: 'text',
      code: `# Download from: https://www.arduino.cc/en/software\n# Choose "Windows Win 10 and newer, 64 bits"\n# Run the installer and follow the prompts.`,
      copyable: false,
      platform: 'windows',
    });
  }

  if (isESP32) {
    commands.push(
      {
        label: 'Open Arduino IDE. Go to File → Preferences. In the "Additional boards manager URLs" field, add this URL:',
        language: 'text',
        code: boardManagerURL,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Open the Boards Manager (Tools → Board → Boards Manager). Search for "esp32" and install the "esp32 by Espressif Systems" package.',
        language: 'text',
        code: `# In Boards Manager, search for: esp32\n# Install: "esp32 by Espressif Systems" (version 3.x recommended)`,
        copyable: false,
        platform: 'all',
      },
      {
        label: `Select your board: Tools → Board → esp32 → ${board?.displayName || 'ESP32S3 Dev Module'}`,
        language: 'text',
        code: `# Board selection path:\n# Tools → Board → esp32 → "${board?.displayName || 'ESP32S3 Dev Module'}"`,
        copyable: false,
        platform: 'all',
      },
      {
        label: 'Install required libraries via Tools → Manage Libraries: search for and install "ArduinoJson"',
        language: 'text',
        code: `# Library Manager → Search: ArduinoJson\n# Install: ArduinoJson by Benoit Blanchon (version 7.x)`,
        copyable: false,
        platform: 'all',
      },
    );
  } else if (isArduino) {
    commands.push(
      {
        label: `Select your board: Tools → Board → Arduino AVR Boards → ${board?.displayName || 'Arduino Uno'}`,
        language: 'text',
        code: `# Board selection path:\n# Tools → Board → Arduino AVR Boards → "${board?.displayName || 'Arduino Uno'}"`,
        copyable: false,
        platform: 'all',
      },
      {
        label: 'Install required libraries via Tools → Manage Libraries: search for and install "ArduinoJson"',
        language: 'text',
        code: `# Library Manager → Search: ArduinoJson\n# Install: ArduinoJson by Benoit Blanchon (version 7.x)`,
        copyable: false,
        platform: 'all',
      },
    );
  }

  commands.push(
    {
      label: 'Select the correct serial port: Tools → Port → (your board\'s port)',
      language: 'text',
      code: `# Linux:   /dev/ttyUSB0 or /dev/ttyACM0\n# macOS:   /dev/cu.usbmodem* or /dev/cu.SLAB_USBtoUART\n# Windows: COM3, COM4, etc. (check Device Manager)`,
      copyable: false,
      platform: 'all',
    },
    {
      label: 'Open the Oh-Ben-Claw companion sketch and upload it to the board',
      language: 'text',
      code: `# The sketch is located in the Oh-Ben-Claw repository.\n# Open: File → Open → (path to Oh-Ben-Claw)/firmware/obc-arduino/obc-arduino.ino\n# Click the Upload button (→) or press Ctrl+U`,
      copyable: false,
      platform: 'all',
    },
  );

  return {
    id: `arduino-ide-${boardId}`,
    title: `Set Up Arduino IDE for ${board?.displayName || boardId}`,
    description: 'Arduino IDE is a beginner-friendly tool for programming microcontroller boards. We will use it to flash the Oh-Ben-Claw companion firmware onto your board.',
    commands,
    tip: 'If you prefer a more powerful editor, VS Code with the PlatformIO extension is an excellent alternative to Arduino IDE and supports all the same boards.',
  };
}

// ── VS Code + PlatformIO Setup ────────────────────────────────────────────────

function getVSCodePlatformIOStep(boardId: string, os: HostOS): GuideStep {
  const board = BOARD_BY_ID[boardId];

  const commands: CodeBlock[] = [];

  if (os === 'linux') {
    commands.push({
      label: 'Download and install VS Code',
      language: 'bash',
      code: `# Download the .deb package from https://code.visualstudio.com/\n# Or install via snap:\nsudo snap install code --classic`,
      copyable: true,
      platform: 'linux',
    });
  } else if (os === 'macos') {
    commands.push({
      label: 'Install VS Code via Homebrew',
      language: 'bash',
      code: `brew install --cask visual-studio-code`,
      copyable: true,
      platform: 'macos',
    });
  } else {
    commands.push({
      label: 'Download VS Code from https://code.visualstudio.com/ and run the installer.',
      language: 'text',
      code: `# Download from: https://code.visualstudio.com/\n# Choose "Windows" and run the installer.`,
      copyable: false,
      platform: 'windows',
    });
  }

  commands.push(
    {
      label: 'Open VS Code. Install the PlatformIO IDE extension: click the Extensions icon (Ctrl+Shift+X), search for "PlatformIO IDE", and click Install.',
      language: 'text',
      code: `# Extensions panel → Search: PlatformIO IDE\n# Publisher: PlatformIO\n# Click Install`,
      copyable: false,
      platform: 'all',
    },
    {
      label: 'Restart VS Code after PlatformIO finishes installing.',
      language: 'text',
      code: `# Click "Reload Window" when prompted, or close and reopen VS Code.`,
      copyable: false,
      platform: 'all',
    },
    {
      label: 'Open the Oh-Ben-Claw firmware project in VS Code',
      language: 'bash',
      code: `code firmware/obc-esp32-s3`,
      copyable: true,
      platform: 'all',
    },
    {
      label: 'PlatformIO will automatically detect the project and install required toolchains. Click the checkmark (✓) button in the bottom toolbar to Build, or the right-arrow (→) button to Upload.',
      language: 'text',
      code: `# Build:  Click ✓ in the bottom toolbar (or Ctrl+Alt+B)\n# Upload: Click → in the bottom toolbar (or Ctrl+Alt+U)\n# Monitor: Click the plug icon 🔌 (or Ctrl+Alt+S)`,
      copyable: false,
      platform: 'all',
    },
  );

  return {
    id: `vscode-platformio-${boardId}`,
    title: `Set Up VS Code + PlatformIO for ${board?.displayName || boardId}`,
    description: 'VS Code with the PlatformIO extension is a powerful, modern development environment that supports all Oh-Ben-Claw target boards. It is the recommended toolchain for users who want more control than Arduino IDE provides.',
    commands,
    tip: 'PlatformIO automatically manages board definitions, toolchains, and libraries. You do not need to manually install the ESP32 board package or Arduino libraries.',
  };
}

// ── Raspberry Pi OS Setup ─────────────────────────────────────────────────────

function getRaspberryPiSetupStep(): GuideStep {
  return {
    id: 'rpi-os-setup',
    title: 'Set Up Raspberry Pi OS',
    description: 'Before installing Oh-Ben-Claw on a Raspberry Pi, you need to install the operating system. We recommend Raspberry Pi OS (64-bit) for the best compatibility.',
    commands: [
      {
        label: 'Download the Raspberry Pi Imager from the official website',
        language: 'text',
        code: `# Download from: https://www.raspberrypi.com/software/\n# Available for Windows, macOS, and Linux.`,
        copyable: false,
        platform: 'all',
      },
      {
        label: 'Insert your microSD card (16 GB minimum, 32 GB recommended). Open Raspberry Pi Imager and select:\n  - Device: Your Raspberry Pi model\n  - OS: "Raspberry Pi OS (64-bit)"\n  - Storage: Your microSD card',
        language: 'text',
        code: `# Recommended OS: Raspberry Pi OS (64-bit)\n# Minimum SD card size: 16 GB\n# Click "Next" and then "Edit Settings" to pre-configure:\n#   - Hostname\n#   - Username and password\n#   - Wi-Fi SSID and password\n#   - Enable SSH`,
        copyable: false,
        platform: 'all',
      },
      {
        label: 'After flashing, insert the SD card into your Raspberry Pi and power it on. Connect via SSH:',
        language: 'bash',
        code: `ssh pi@raspberrypi.local\n# Or use the IP address: ssh pi@192.168.x.x`,
        copyable: true,
        platform: 'all',
      },
      {
        label: 'Update the system',
        language: 'bash',
        code: `sudo apt update && sudo apt upgrade -y`,
        copyable: true,
        platform: 'linux',
      },
      {
        label: 'Install Oh-Ben-Claw dependencies on the Raspberry Pi',
        language: 'bash',
        code: `sudo apt install -y build-essential pkg-config libssl-dev libsqlite3-dev curl git mosquitto mosquitto-clients`,
        copyable: true,
        platform: 'linux',
      },
      {
        label: 'Install Rust on the Raspberry Pi',
        language: 'bash',
        code: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh\nsource "$HOME/.cargo/env"`,
        copyable: true,
        platform: 'linux',
      },
      {
        label: 'Enable the camera (if using a Pi Camera module)',
        language: 'bash',
        code: `sudo raspi-config\n# Navigate to: Interface Options → Camera → Enable\n# Then reboot: sudo reboot`,
        copyable: true,
        platform: 'linux',
      },
    ],
    tip: 'For the best performance on Raspberry Pi 4 or 5, use a fast microSD card (Class 10 / A2 rated) or an SSD connected via USB.',
  };
}

// ── Wiring / Role Guide Step ─────────────────────────────────────────────────

function getWiringStep(state: WizardState): GuideStep | null {
  const configs = state.roleConfigs ?? [];
  if (configs.length === 0) return null;

  const tableRows: string[] = [];
  tableRows.push('| Board | Role | Bus | Pin / Address | Notes |');
  tableRows.push('|---|---|---|---|---|');

  for (const rc of configs) {
    const board = BOARD_BY_ID[rc.boardId];
    if (!board) continue;
    for (const a of rc.assignments) {
      const role = ROLE_BY_ID[a.roleId];
      if (!role) continue;
      tableRows.push(
        `| ${board.displayName} | ${role.icon} ${role.label} | ${a.bus} | ${a.pinOrAddress || '—'} | ${a.notes || '—'} |`
      );
    }
  }

  // Build wiring instructions per role
  const wiringInstructions: CodeBlock[] = [];

  for (const rc of configs) {
    const board = BOARD_BY_ID[rc.boardId];
    if (!board) continue;
    for (const a of rc.assignments) {
      const role = ROLE_BY_ID[a.roleId];
      if (!role) continue;

      let wiringText = '';

      if (a.bus === 'I2C') {
        wiringText = `# ${board.displayName} → ${role.label} (I2C)\n` +
          `# Connect SDA → SDA on host board\n` +
          `# Connect SCL → SCL on host board\n` +
          `# Connect VCC → 3.3V (or 5V if module requires it)\n` +
          `# Connect GND → GND\n` +
          (a.pinOrAddress ? `# I2C address: ${a.pinOrAddress}\n` : '') +
          `# Verify the device is detected:\n` +
          `sudo i2cdetect -y 1`;
      } else if (a.bus === 'SPI') {
        wiringText = `# ${board.displayName} → ${role.label} (SPI)\n` +
          `# Connect MOSI → MOSI on host\n` +
          `# Connect MISO → MISO on host\n` +
          `# Connect SCLK → SCLK on host\n` +
          `# Connect CS   → ${a.pinOrAddress || 'GPIO (your choice)'}\n` +
          `# Connect VCC → 3.3V, GND → GND`;
      } else if (a.bus === 'UART') {
        wiringText = `# ${board.displayName} → ${role.label} (UART)\n` +
          `# Connect TX  → RX on host\n` +
          `# Connect RX  → TX on host\n` +
          `# Connect VCC → 3.3V, GND → GND\n` +
          (a.pinOrAddress ? `# UART port: ${a.pinOrAddress}\n` : '') +
          `# Verify the device is visible:\n` +
          `ls /dev/ttyUSB* /dev/ttyACM*`;
      } else if (a.bus === 'USB') {
        wiringText = `# ${board.displayName} → ${role.label} (USB)\n` +
          `# Plug the device into a USB port on your host machine.\n` +
          `# Verify it appears as an audio/serial device:\n` +
          `lsusb\n` +
          `arecord -l   # for USB audio devices`;
      } else if (a.bus === 'I2S') {
        wiringText = `# ${board.displayName} → ${role.label} (I2S)\n` +
          `# Connect BCLK (Bit Clock)  → I2S BCLK pin\n` +
          `# Connect LRCLK (Word Sel)  → I2S WS pin\n` +
          `# Connect DATA              → I2S DATA pin\n` +
          (a.pinOrAddress ? `# I2S channel / address: ${a.pinOrAddress}\n` : '') +
          `# Connect VCC → 3.3V, GND → GND`;
      } else if (a.bus === 'CSI') {
        wiringText = `# ${board.displayName} → ${role.label} (CSI Camera)\n` +
          `# Connect the flat ribbon cable from the camera module to the CSI port on the board.\n` +
          `# Ensure the cable is fully seated and the latch is locked.\n` +
          (a.pinOrAddress ? `# CSI port: ${a.pinOrAddress}\n` : '') +
          `# Verify the camera is detected:\n` +
          `libcamera-hello --list-cameras`;
      } else if (a.bus === 'Wi-Fi/MQTT') {
        wiringText = `# ${board.displayName} → ${role.label} (Wi-Fi / MQTT)\n` +
          `# No physical wiring needed — this connection is wireless.\n` +
          `# Ensure both devices are on the same Wi-Fi network.\n` +
          `# The MQTT broker address is configured in config.toml under [spine].`;
      } else {
        wiringText = `# ${board.displayName} → ${role.label} (${a.bus})\n` +
          (a.pinOrAddress ? `# Pin / Address: ${a.pinOrAddress}\n` : '') +
          (a.notes ? `# Notes: ${a.notes}` : '');
      }

      wiringInstructions.push({
        label: `${board.displayName} → ${role.icon} ${role.label} (${a.bus})`,
        language: 'bash',
        code: wiringText,
        copyable: true,
        platform: 'all',
      });
    }
  }

  return {
    id: 'role-wiring',
    title: 'Component Role Assignment and Wiring',
    description:
      'This section shows how each component in your deployment is connected and what role it plays. ' +
      'Follow the wiring instructions for each connection carefully before powering on your system. ' +
      'The table below is a summary; detailed wiring instructions follow.\n\n' +
      tableRows.join('\n'),
    commands: wiringInstructions,
    tip: 'Always connect GND first and disconnect it last. Never connect or disconnect components while the system is powered on.',
    warning: 'Double-check all voltage levels before connecting. Most ESP32 boards use 3.3V logic. Connecting a 5V signal to a 3.3V GPIO pin can permanently damage the board.',
  };
}

// ── Main Guide Generator ──────────────────────────────────────────────────────

export function generateGuide(state: WizardState): GeneratedGuide {
  const steps: GuideStep[] = [];
  const os = state.hostOS || 'linux';

  // Determine title and summary
  const hostBoardInfo = state.hostBoard ? BOARD_BY_ID[state.hostBoard] : null;
  const peripheralNames = state.peripheralBoards
    .map(id => BOARD_BY_ID[id]?.displayName || id)
    .join(', ');

  let title = 'Oh-Ben-Claw Deployment Guide';
  let summary = 'A step-by-step guide to deploying Oh-Ben-Claw.';

  if (state.goal === 'host') {
    title = `Oh-Ben-Claw Host Agent Setup — ${hostBoardInfo?.displayName || 'Your Machine'}`;
    summary = `This guide will walk you through setting up the Oh-Ben-Claw brain agent on your ${hostBoardInfo?.displayName || 'machine'} running ${os}.`;
  } else if (state.goal === 'peripheral') {
    title = `Oh-Ben-Claw Peripheral Node Setup — ${peripheralNames}`;
    summary = `This guide will walk you through setting up ${peripheralNames} as peripheral nodes for Oh-Ben-Claw.`;
  } else if (state.goal === 'full') {
    title = `Full Oh-Ben-Claw Deployment — ${hostBoardInfo?.displayName || 'Host'} + ${peripheralNames}`;
    summary = `This guide covers a complete Oh-Ben-Claw deployment: the brain agent on your ${hostBoardInfo?.displayName || 'host machine'} and peripheral nodes (${peripheralNames}).`;
  }

  // Step 1: OS Prerequisites
  if (state.goal !== 'peripheral' || state.hostBoard) {
    // Special case for Raspberry Pi as host
    if (state.hostBoard === 'raspberry-pi') {
      steps.push(getRaspberryPiSetupStep());
    }
    steps.push(getOSPrerequisites(os));
  }

  // Step 2: MQTT Broker (if needed)
  if (state.goal === 'full' || state.goal === 'host' || state.featureDesires.includes('WirelessMesh')) {
    steps.push(getMQTTBrokerStep(os));
  }

  // Step 3: Clone and Build (for host/full deployments)
  if (state.goal === 'host' || state.goal === 'full') {
    steps.push(getCloneAndBuildStep(os, state.featureDesires));
  }

  // Step 4: Configure
  if (state.goal === 'host' || state.goal === 'full') {
    steps.push(getConfigStep(state));
  }

  // Step 4b: Wiring / Role Assignment (if roles have been assigned)
  const wiringStep = getWiringStep(state);
  if (wiringStep) steps.push(wiringStep);

  // Step 5: Peripheral firmware
  for (const boardId of state.peripheralBoards) {
    const board = BOARD_BY_ID[boardId];
    if (!board) continue;

    // USB Driver setup
    const driverStep = getUSBDriverStep(boardId, os);
    if (driverStep) steps.push(driverStep);

    // Toolchain-specific firmware setup
    if (board.category === 'esp32' && board.firmwareSupported) {
      if (state.toolchain === 'arduino-ide') {
        steps.push(getArduinoIDEStep(boardId, os));
      } else if (state.toolchain === 'vscode-platformio') {
        steps.push(getVSCodePlatformIOStep(boardId, os));
      } else {
        // Default: Rust/Cargo (recommended)
        steps.push(getESP32FirmwareStep(boardId, os));
      }
    } else if (board.category === 'arduino') {
      if (state.toolchain === 'vscode-platformio') {
        steps.push(getVSCodePlatformIOStep(boardId, os));
      } else {
        steps.push(getArduinoIDEStep(boardId, os));
      }
    } else if (board.category === 'stm32') {
      steps.push(getArduinoIDEStep(boardId, os));
    }

    // OTA setup
    if (state.featureDesires.includes('OTA') && board.otaSupported) {
      steps.push(getOTAStep(boardId));
    }
  }

  // Step 6: Run
  if (state.goal === 'host' || state.goal === 'full') {
    steps.push(getRunStep());
    steps.push(getGUIStep(os));
  }

  // Estimate time
  let estimatedMinutes = 15;
  if (state.goal !== 'peripheral') estimatedMinutes += 20; // Build time
  estimatedMinutes += state.peripheralBoards.length * 15;
  if (state.featureDesires.includes('OTA')) estimatedMinutes += 10;
  const estimatedTime = estimatedMinutes < 60
    ? `${estimatedMinutes} minutes`
    : `${Math.round(estimatedMinutes / 60)} hour${estimatedMinutes >= 120 ? 's' : ''}`;

  const difficulty = state.peripheralBoards.some(id => BOARD_BY_ID[id]?.category === 'stm32')
    ? 'advanced'
    : state.peripheralBoards.some(id => BOARD_BY_ID[id]?.category === 'esp32')
    ? 'intermediate'
    : 'beginner';

  return {
    title,
    summary,
    estimatedTime,
    difficulty,
    steps,
    configToml: (state.goal === 'host' || state.goal === 'full') ? generateConfigToml(state) : undefined,
  };
}

export function guideToMarkdown(guide: GeneratedGuide): string {
  const lines: string[] = [];

  lines.push(`# ${guide.title}`);
  lines.push(``);
  lines.push(`> ${guide.summary}`);
  lines.push(``);
  lines.push(`**Estimated Time:** ${guide.estimatedTime} | **Difficulty:** ${guide.difficulty}`);
  lines.push(``);
  lines.push(`---`);
  lines.push(``);

  guide.steps.forEach((step, i) => {
    lines.push(`## Step ${i + 1}: ${step.title}`);
    lines.push(``);
    lines.push(step.description);
    lines.push(``);

    if (step.warning) {
      lines.push(`> ⚠️ **Warning:** ${step.warning}`);
      lines.push(``);
    }

    if (step.commands) {
      for (const cmd of step.commands) {
        if (cmd.label) {
          lines.push(`### ${cmd.label}`);
          lines.push(``);
        }
        lines.push(`\`\`\`${cmd.language}`);
        lines.push(cmd.code);
        lines.push(`\`\`\``);
        lines.push(``);
      }
    }

    if (step.tip) {
      lines.push(`> 💡 **Tip:** ${step.tip}`);
      lines.push(``);
    }

    lines.push(`---`);
    lines.push(``);
  });

  if (guide.configToml) {
    lines.push(`## Generated config.toml`);
    lines.push(``);
    lines.push(`\`\`\`toml`);
    lines.push(guide.configToml);
    lines.push(`\`\`\``);
    lines.push(``);
  }

  return lines.join('\n');
}
