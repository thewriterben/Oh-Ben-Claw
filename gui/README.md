# Oh-Ben-Claw GUI

The native desktop application for Oh-Ben-Claw, built with [Tauri 2](https://tauri.app) + [React 18](https://react.dev) + [TypeScript](https://www.typescriptlang.org) + [TailwindCSS](https://tailwindcss.com).

## Architecture

The GUI is a Tauri 2 application with two layers. The **frontend** is a React/TypeScript SPA served from Vite, styled with TailwindCSS using a custom dark ocean theme. The **backend** is a Rust binary (`gui/src-tauri`) that embeds the `oh-ben-claw` core library and exposes all agent, session, node, vault, and settings operations as Tauri commands.

Communication between the two layers uses Tauri's IPC bridge: the frontend calls `invoke()` for request-response operations, and the backend emits events (`assistant-token`, `tool-call-event`, `node-status-change`) for streaming and push updates.

## Panels

| Panel | Description |
|---|---|
| **Chat** | Multi-session conversation interface with tool-call bubbles and streaming support |
| **Devices** | Peripheral node status cards — USB scan, add/remove, tool list per node |
| **Tool Log** | Filterable history of all tool calls with arguments, results, and duration |
| **Vault** | AES-256-GCM encrypted secrets management — unlock, add, delete |
| **Settings** | LLM provider/model, Spine MQTT config, security toggles, agent start/stop |

## Prerequisites

The following must be installed before building the GUI:

- **Rust** (stable, 1.77+): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js 20+** and **pnpm**: `npm install -g pnpm`
- **Tauri system dependencies** (Linux):
  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev libssl-dev
  ```
- **Tauri system dependencies** (macOS): Xcode Command Line Tools (`xcode-select --install`)
- **Tauri system dependencies** (Windows): Microsoft Visual C++ Build Tools + WebView2

## Development

```bash
# From the gui/ directory
pnpm install
pnpm tauri dev
```

This starts the Vite dev server on `http://localhost:1420` and the Tauri window simultaneously. Hot module replacement is enabled for the frontend.

## Production Build

```bash
# From the gui/ directory
pnpm tauri build
```

This produces platform-specific installers in `gui/src-tauri/target/release/bundle/`:

| Platform | Output |
|---|---|
| Linux | `.deb` (Debian/Ubuntu), `.AppImage` |
| macOS | `.dmg`, `.app` |
| Windows | `.msi`, `.exe` (NSIS) |

## System Tray

Oh-Ben-Claw runs in the system tray when the window is closed. Left-clicking the tray icon shows the window. Right-clicking opens a menu with **Show** and **Quit** options. Launch-at-login can be enabled in the Settings panel.

## Tauri Commands Reference

All backend commands are defined in `gui/src-tauri/src/commands.rs` and bridged to the frontend via `gui/src/hooks/useTauri.ts`. The command categories are:

- **Agent**: `start_agent`, `stop_agent`, `send_message`, `get_agent_status`
- **Sessions**: `list_sessions`, `create_session`, `load_session_history`, `clear_session`, `delete_session`
- **Nodes**: `list_nodes`, `add_node`, `remove_node`, `scan_usb_devices`
- **Tool Log**: `get_tool_log`, `clear_tool_log`
- **Vault**: `get_vault_status`, `unlock_vault`, `lock_vault`, `list_vault_secrets`, `set_vault_secret`, `delete_vault_secret`
- **Settings**: `get_settings`, `save_settings`
