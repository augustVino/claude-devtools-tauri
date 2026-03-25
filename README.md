# claude-devtools-tauri

Tauri v2 desktop app for visualizing Claude Code session execution — explore conversations, track context usage, and analyze tool calls.

A [Tauri v2](https://tauri.app/) port of [claude-devtools](https://github.com/matt1398/claude-devtools) (Electron), rewritten with a pure Rust backend for significantly reduced binary size and memory usage.

## Features

- Visualize Claude Code session timelines
- Track context window usage across 6 categories
- Analyze tool calls and subagent orchestration
- Search across sessions
- Real-time file watching with incremental updates

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) 1.77.2+
- [Node.js](https://nodejs.org/) 18+
- [pnpm](https://pnpm.io/) 8+

### Install & Run

```bash
pnpm install
pnpm tauri dev
```

### Build

```bash
pnpm tauri build
```

## Tech Stack

- **Backend:** Rust + Tauri v2
- **Frontend:** React 18, TypeScript, Vite, Tailwind CSS, Zustand
- **Key crates:** tokio, serde, notify, moka

## License

[MIT](./LICENSE) — see [NOTICE](./NOTICE) for attribution details.

## Acknowledgments

This project is derived from [claude-devtools](https://github.com/matt1398/claude-devtools) by matt1398 and contributors, licensed under the MIT License.
